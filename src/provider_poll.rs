//! Per-provider freshness, retry deadlines and selection-safe request completion.
use crate::models::{AppUsageData, UsageData};
use crate::poller::{CredentialWatchMode, CredentialWatchSnapshot, PollError};
use crate::quota_refresh::{self, History, ResetRefresh};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub struct Service {
    pub data: Option<UsageData>,
    pub history: History,
    pub error: Option<PollError>,
    pub interval_ms: u32,
    enabled: bool,
    generation: u64,
    in_flight: Option<u64>,
    next_ticket: u64,
    forced: bool,
    retries: u32,
    due: Instant,
    retry_not_before: Option<Instant>,
    auth: Option<(CredentialWatchMode, CredentialWatchSnapshot)>,
    reset: ResetRefresh,
}
#[derive(Clone)]
pub struct Job {
    pub id: usize,
    generation: u64,
    ticket: u64,
    forced: bool,
    auth: Option<(CredentialWatchMode, CredentialWatchSnapshot)>,
}
pub struct Attempt {
    result: Option<Result<UsageData, PollError>>,
    auth: Option<(CredentialWatchMode, CredentialWatchSnapshot)>,
}
pub struct Completion {
    pub accepted: bool,
    pub notify_auth: bool,
    pub successful: bool,
}
pub struct Monitor {
    pub services: [Service; 3],
}

fn is_auth(error: PollError) -> bool {
    matches!(
        error,
        PollError::AuthRequired | PollError::TokenExpired | PollError::NoCredentials
    )
}
fn watch_mode(id: usize, error: PollError) -> CredentialWatchMode {
    if id == 1 {
        return CredentialWatchMode::Codex;
    }
    if id == 2 {
        CredentialWatchMode::Antigravity
    } else if error == PollError::NoCredentials {
        CredentialWatchMode::AllSources
    } else {
        CredentialWatchMode::ActiveSource
    }
}

fn next_reset_ms(data: &UsageData, wall: SystemTime) -> Option<u32> {
    [data.session.resets_at, data.weekly.resets_at]
        .into_iter()
        .flatten()
        .filter_map(|time| time.duration_since(wall).ok())
        .filter(|delay| !delay.is_zero())
        .min()
        .map(|delay| delay.as_millis().clamp(1000, u32::MAX as u128) as u32)
}

/// Also used by deterministic flow tests: paused services only call the API
/// after a credential change or an explicit manual refresh.
pub fn execute(
    job: &Job,
    poll: impl FnOnce() -> Result<UsageData, PollError>,
    credentials: impl Fn(CredentialWatchMode) -> CredentialWatchSnapshot,
) -> Attempt {
    if !job.forced {
        if let Some((mode, old)) = &job.auth {
            let current = credentials(*mode);
            if current == *old {
                return Attempt {
                    result: None,
                    auth: Some((*mode, current)),
                };
            }
        }
    }
    let result = poll();
    let auth = result
        .as_ref()
        .err()
        .copied()
        .filter(|e| is_auth(*e))
        .map(|error| {
            let mode = watch_mode(job.id, error);
            (mode, credentials(mode))
        });
    Attempt {
        result: Some(result),
        auth,
    }
}

/// Run planned jobs off the caller thread and publish each result as it arrives.
pub fn launch(
    jobs: Vec<Job>,
    poll: impl Fn(&Job) -> Attempt + Send + Sync + 'static,
    complete: impl Fn(&Job, Attempt) + Send + Sync + 'static,
) {
    let poll = Arc::new(poll);
    let complete = Arc::new(complete);
    for job in jobs {
        let worker_job = job.clone();
        let worker_poll = poll.clone();
        let worker_complete = complete.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("quota-provider-{}", job.id))
            .spawn(move || {
                crate::diagnose::log(format!("provider query started id={}", worker_job.id));
                worker_complete(&worker_job, worker_poll(&worker_job));
            })
        {
            crate::diagnose::log_error("unable to start provider query", error);
            complete(
                &job,
                Attempt {
                    result: Some(Err(PollError::RequestFailed)),
                    auth: None,
                },
            );
        }
    }
}

impl Monitor {
    pub fn new(enabled: [bool; 3], now: Instant) -> Self {
        Self {
            services: std::array::from_fn(|id| Service {
                data: None,
                history: History::default(),
                error: None,
                interval_ms: 900_000,
                enabled: enabled[id],
                generation: 0,
                in_flight: None,
                next_ticket: 0,
                forced: false,
                retries: 0,
                due: now,
                retry_not_before: None,
                auth: None,
                reset: ResetRefresh::default(),
            }),
        }
    }
    pub fn configure(&mut self, enabled: [bool; 3], now: Instant) {
        for (service, enabled) in self.services.iter_mut().zip(enabled) {
            if service.enabled != enabled {
                service.enabled = enabled;
                service.generation = service.generation.wrapping_add(1);
                service.due = service
                    .retry_not_before
                    .map_or(now, |deadline| deadline.max(now));
                service.forced = enabled;
            }
        }
    }
    pub fn force(&mut self, now: Instant, include_auth: bool) {
        for service in self.services.iter_mut().filter(|s| s.enabled) {
            service.due = service
                .retry_not_before
                .map_or(now, |deadline| deadline.max(now));
            service.forced |= include_auth || service.auth.is_none();
        }
    }
    pub fn plan(&mut self, now: Instant) -> Vec<Job> {
        self.services
            .iter_mut()
            .enumerate()
            .filter_map(|(id, s)| {
                if !s.enabled || s.in_flight.is_some() || s.due > now {
                    return None;
                }
                s.next_ticket = s.next_ticket.wrapping_add(1);
                s.in_flight = Some(s.next_ticket);
                let forced = std::mem::take(&mut s.forced);
                Some(Job {
                    id,
                    generation: s.generation,
                    ticket: s.next_ticket,
                    forced,
                    auth: s.auth.clone(),
                })
            })
            .collect()
    }
    pub fn finish(
        &mut self,
        job: &Job,
        attempt: Attempt,
        now: Instant,
        wall: SystemTime,
        base: u32,
        adaptive: bool,
    ) -> Completion {
        let s = &mut self.services[job.id];
        let owns_request = s.in_flight == Some(job.ticket);
        if owns_request {
            s.in_flight = None;
            // A hidden/obsolete response still carries a server wait deadline.
            if let Some(Err(PollError::RateLimited(Some(ms)))) = attempt.result {
                let deadline = now + Duration::from_millis(ms as u64);
                s.retry_not_before = Some(deadline);
                s.due = s.due.max(deadline);
            }
        }
        if !owns_request || !s.enabled || s.generation != job.generation {
            return Completion {
                accepted: false,
                successful: false,
                notify_auth: false,
            };
        }
        let mut notify_auth = false;
        let mut successful = false;
        match attempt.result {
            Some(Ok(data)) => {
                successful = true;
                s.retries = 0;
                s.error = None;
                s.auth = None;
                s.retry_not_before = None;
                let reset = [data.session.resets_at, data.weekly.resets_at]
                    .into_iter()
                    .flatten()
                    .filter(|t| *t <= wall)
                    .max();
                s.interval_ms = quota_refresh::interval_ms(base, adaptive, [&data]);
                if let Some(reset_ms) = s.reset.interval_ms(reset, now) {
                    s.interval_ms = s.interval_ms.min(reset_ms);
                }
                if let Some(reset_ms) = next_reset_ms(&data, wall) {
                    s.interval_ms = s.interval_ms.min(reset_ms);
                }
                s.history.record_success(wall, s.interval_ms);
                s.data = Some(data);
            }
            Some(Err(error)) => {
                notify_auth = is_auth(error) && (s.error != Some(error) || job.forced);
                s.error = Some(error);
                s.retries = s.retries.saturating_add(1);
                s.auth = attempt.auth;
                s.retry_not_before = match error {
                    PollError::RateLimited(Some(ms)) => {
                        Some(now + Duration::from_millis(ms as u64))
                    }
                    _ => None,
                };
                s.interval_ms = if let PollError::RateLimited(Some(ms)) = error {
                    ms
                } else if s.auth.is_some() {
                    base
                } else {
                    quota_refresh::retry_interval_ms(base, s.retries)
                };
            }
            None => {
                s.interval_ms = base;
            }
        }
        s.due = if s.forced {
            s.retry_not_before.map_or(now, |deadline| deadline.max(now))
        } else {
            now + Duration::from_millis(s.interval_ms as u64)
        };
        Completion {
            accepted: true,
            successful,
            notify_auth,
        }
    }
    pub fn reconfigure(&mut self, base: u32, adaptive: bool, now: Instant, wall: SystemTime) {
        for s in self
            .services
            .iter_mut()
            .filter(|s| s.enabled && s.error.is_none())
        {
            s.interval_ms = quota_refresh::interval_ms(base, adaptive, s.data.iter());
            if let Some(reset_ms) = s.data.as_ref().and_then(|data| next_reset_ms(data, wall)) {
                s.interval_ms = s.interval_ms.min(reset_ms);
            }
            s.history.expected_interval_ms = s.interval_ms;
            if !s.forced {
                s.due = now + Duration::from_millis(s.interval_ms as u64);
            }
        }
    }
    pub fn delay_ms(&self, now: Instant, base: u32) -> u32 {
        self.services
            .iter()
            .filter(|s| s.enabled && s.in_flight.is_none())
            .map(|s| {
                s.due
                    .saturating_duration_since(now)
                    .as_millis()
                    .clamp(1000, u32::MAX as u128) as u32
            })
            .min()
            .unwrap_or(base)
    }
    pub fn cached(&self) -> AppUsageData {
        let get = |id: usize| {
            self.services[id]
                .enabled
                .then(|| self.services[id].data.clone())
                .flatten()
        };
        AppUsageData {
            claude_code: get(0),
            codex: get(1),
            antigravity: get(2),
        }
    }
}
impl Service {
    pub fn stale(&self, now: SystemTime) -> bool {
        self.error.is_some() || self.history.is_stale(now)
    }
    pub fn description(&self, now: SystemTime, clock: Instant, chinese: bool) -> String {
        let mut text =
            self.history
                .description(now, chinese, self.error.is_none(), self.interval_ms);
        if self.in_flight.is_some() {
            text.push_str(if chinese {
                "\n正在刷新…"
            } else {
                "\nRefreshing…"
            });
        } else if self.enabled {
            let delay = self.due.saturating_duration_since(clock);
            let time = now
                .checked_add(delay)
                .and_then(crate::native_interop::system_time_to_local)
                .map(|t| format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond))
                .unwrap_or_else(|| "--".into());
            let label = if self.auth.is_some() {
                if chinese {
                    "下次检查登录凭据"
                } else {
                    "Next credential check"
                }
            } else if self.error.is_some() {
                if chinese {
                    "下次重试"
                } else {
                    "Next retry"
                }
            } else {
                if chinese {
                    "下次刷新"
                } else {
                    "Next refresh"
                }
            };
            text.push_str(&format!("\n{label}：{time}"));
        }
        if let Some(error) = self.error {
            let reason = if chinese {
                match error {
                    PollError::AuthRequired => "需要重新登录",
                    PollError::NoCredentials => "未找到登录凭据",
                    PollError::TokenExpired => "登录已过期",
                    PollError::NetworkUnavailable => "网络连接失败",
                    PollError::RateLimited(_) => "请求过于频繁，正在退避",
                    PollError::ServerError => "服务暂时不可用",
                    PollError::RequestFailed => "无法读取额度响应",
                }
            } else {
                error.category()
            };
            text.push_str(&format!(
                "\n{}: {}",
                if chinese {
                    "最近请求失败"
                } else {
                    "Latest request failed"
                },
                reason
            ));
            if self.auth.is_some() {
                text.push_str(if chinese {
                    "\n已暂停接口重试，等待登录凭据更新；也可手动刷新。"
                } else {
                    "\nAPI retries paused until credentials change; manual refresh is available."
                });
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};

    #[test]
    fn server_retry_deadline_survives_manual_refresh_and_selection_changes() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut monitor = Monitor::new([true, true, false], now);
        let jobs = monitor.plan(now);
        monitor.force(now, true); // A click during the request must also wait.
        complete(
            &mut monitor,
            &jobs[0],
            Err(PollError::RateLimited(Some(1_800_000))),
            now,
            wall,
        );
        complete(&mut monitor, &jobs[1], Ok(usage(20.0)), now, wall);
        assert_eq!(monitor.services[0].interval_ms, 1_800_000);
        let follow_up = monitor.plan(now);
        assert_eq!(follow_up.len(), 1);
        assert_eq!(follow_up[0].id, 1);
        complete(&mut monitor, &follow_up[0], Ok(usage(20.0)), now, wall);
        monitor.force(now, true);
        monitor.configure([false, true, false], now);
        monitor.configure([true, true, false], now);
        assert!(monitor.plan(now).iter().all(|job| job.id != 0));
        let due = now + Duration::from_secs(1800);
        let job = monitor
            .plan(due)
            .into_iter()
            .find(|job| job.id == 0)
            .unwrap();
        complete(&mut monitor, &job, Ok(usage(20.0)), due, wall);
        assert!(monitor.services[0].retry_not_before.is_none());
        monitor.force(due, true);
        let job = monitor
            .plan(due)
            .into_iter()
            .find(|job| job.id == 0)
            .unwrap();
        monitor.configure([false, true, false], due);
        assert!(
            !complete(
                &mut monitor,
                &job,
                Err(PollError::RateLimited(Some(120_000))),
                due,
                wall
            )
            .accepted
        );
        monitor.configure([true, true, false], due);
        assert!(monitor.plan(due).iter().all(|job| job.id != 0));
    }

    #[test]
    fn hover_status_tracks_running_requests_schedules_and_auth_checks() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut monitor = Monitor::new([true, false, false], now);
        let job = monitor.plan(now).remove(0);
        assert!(monitor.services[0]
            .description(wall, now, true)
            .contains("正在刷新"));
        assert!(monitor.services[0]
            .description(wall, now, false)
            .contains("Refreshing"));
        complete(&mut monitor, &job, Ok(usage(20.0)), now, wall);
        let description = monitor.services[0].description(wall, now, true);
        assert!(description.contains("下次刷新："));
        assert!(!description.contains("正在刷新"));
        monitor.force(now, true);
        let job = monitor.plan(now).remove(0);
        complete(
            &mut monitor,
            &job,
            Err(PollError::RateLimited(None)),
            now,
            wall,
        );
        assert!(monitor.services[0]
            .description(wall, now, true)
            .contains("下次重试："));
        monitor.force(now, true);
        let job = monitor.plan(now).remove(0);
        complete(&mut monitor, &job, Err(PollError::NoCredentials), now, wall);
        assert!(monitor.services[0]
            .description(wall, now, true)
            .contains("下次检查登录凭据："));
    }

    #[test]
    fn codex_publishes_while_claude_is_still_waiting() {
        let now = Instant::now();
        let mut monitor = Monitor::new([true, true, false], now);
        let jobs = monitor.plan(now);
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let (finished, received) = mpsc::channel();
        launch(
            jobs,
            move |job| {
                if job.id == 0 {
                    wait.lock().unwrap().recv().unwrap();
                }
                execute(job, || Ok(usage(40.0)), |_| vec![])
            },
            move |job, _| {
                finished.send(job.id).unwrap();
            },
        );
        let first = received.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        assert_eq!(
            first.unwrap(),
            1,
            "Codex must update before the blocked Claude query completes"
        );
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    }

    #[test]
    fn fast_service_can_poll_again_while_the_other_service_is_still_in_flight() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let monitor = Arc::new(Mutex::new(Monitor::new([true, true, false], now)));
        let jobs = monitor.lock().unwrap().plan(now);
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let (finished, received) = mpsc::channel();
        let state = monitor.clone();
        let published = finished.clone();
        launch(
            jobs,
            move |job| {
                if job.id == 0 {
                    wait.lock().unwrap().recv().unwrap();
                }
                execute(job, || Ok(usage(90.0)), |_| vec![])
            },
            move |job, attempt| {
                assert!(
                    state
                        .lock()
                        .unwrap()
                        .finish(job, attempt, now, wall, 900_000, true)
                        .accepted
                );
                published.send(job.id).unwrap();
            },
        );
        let first = received.recv_timeout(Duration::from_secs(2));
        if first.is_err() {
            let _ = release.send(());
        }
        assert_eq!(first.unwrap(), 1);
        let later = now + Duration::from_secs(60);
        let jobs = monitor.lock().unwrap().plan(later);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, 1);
        let state = monitor.clone();
        launch(
            jobs,
            |job| execute(job, || Ok(usage(85.0)), |_| vec![]),
            move |job, attempt| {
                assert!(
                    state
                        .lock()
                        .unwrap()
                        .finish(
                            job,
                            attempt,
                            later,
                            wall + Duration::from_secs(60),
                            900_000,
                            true
                        )
                        .accepted
                );
                finished.send(job.id).unwrap();
            },
        );
        let second = received.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        assert_eq!(
            second.unwrap(),
            1,
            "Codex's next update must not wait for Claude"
        );
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    }

    #[test]
    fn reenabled_service_waits_for_its_old_worker_and_duplicate_completion_cannot_release_a_new_job(
    ) {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut monitor = Monitor::new([false, true, false], now);
        let old = monitor.plan(now).remove(0);
        monitor.configure([true, false, false], now);
        monitor.configure([true, true, false], now);
        let jobs = monitor.plan(now);
        assert_eq!(jobs.iter().map(|j| j.id).collect::<Vec<_>>(), vec![0]);
        assert!(!complete(&mut monitor, &old, Ok(usage(20.0)), now, wall).accepted);
        let new = monitor.plan(now).remove(0);
        assert_eq!(new.id, 1);
        assert!(!complete(&mut monitor, &old, Ok(usage(20.0)), now, wall).accepted);
        assert!(monitor.plan(now).is_empty());
        assert!(complete(&mut monitor, &new, Ok(usage(30.0)), now, wall).accepted);
    }
    fn usage(used: f64) -> UsageData {
        let mut data = UsageData::default();
        data.session.percentage = used;
        data
    }
    fn complete(
        m: &mut Monitor,
        j: &Job,
        result: Result<UsageData, PollError>,
        now: Instant,
        wall: SystemTime,
    ) -> Completion {
        let attempt = execute(j, || result, |_| vec!["credential-v1".into()]);
        m.finish(j, attempt, now, wall, 900_000, true)
    }
    #[test]
    fn mixed_failure_retry_and_recovery_keep_independent_timestamps_and_deadlines() {
        let start = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let mut m = Monitor::new([true, true, false], start);
        let jobs = m.plan(start);
        complete(&mut m, &jobs[0], Ok(usage(40.0)), start, wall);
        complete(
            &mut m,
            &jobs[1],
            Err(PollError::NetworkUnavailable),
            start,
            wall,
        );
        assert_eq!(m.services[0].history.last_success, Some(wall));
        assert_eq!(m.services[1].history.last_success, None);
        let retry = m.plan(start + Duration::from_secs(30));
        assert_eq!(retry.iter().map(|j| j.id).collect::<Vec<_>>(), vec![1]);
        complete(
            &mut m,
            &retry[0],
            Err(PollError::ServerError),
            start + Duration::from_secs(30),
            wall,
        );
        assert!(m.plan(start + Duration::from_secs(89)).is_empty());
        let retry = m.plan(start + Duration::from_secs(90));
        complete(
            &mut m,
            &retry[0],
            Ok(usage(90.0)),
            start + Duration::from_secs(90),
            wall + Duration::from_secs(90),
        );
        assert_eq!(m.services[0].history.last_success, Some(wall));
        assert_eq!(m.services[1].interval_ms, 60_000);
        assert!(!m.services[1].stale(wall + Duration::from_secs(90)));
    }
    #[test]
    fn switching_services_during_a_request_discards_old_completion_and_fetches_new_selection() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut m = Monitor::new([false, true, false], now);
        let old = m.plan(now).remove(0);
        m.configure([true, false, false], now);
        assert!(!complete(&mut m, &old, Ok(usage(95.0)), now, wall).accepted);
        let new = m.plan(now).remove(0);
        assert_eq!(new.id, 0);
        assert!(complete(&mut m, &new, Ok(usage(20.0)), now, wall).accepted);
        assert!(m.cached().codex.is_none());
        assert_eq!(m.cached().claude_code.unwrap().session.percentage, 20.0);
        m.configure([true, true, false], now);
        let new = m.plan(now).remove(0);
        assert_eq!(new.id, 1);
        assert!(!complete(&mut m, &old, Ok(usage(95.0)), now, wall).accepted);
        assert!(complete(&mut m, &new, Ok(usage(25.0)), now, wall).accepted);
    }
    #[test]
    fn auth_failure_does_not_pause_other_service_and_recovers_only_after_credentials_change() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut m = Monitor::new([true, true, false], now);
        let jobs = m.plan(now);
        assert!(complete(&mut m, &jobs[0], Err(PollError::AuthRequired), now, wall).notify_auth);
        complete(&mut m, &jobs[1], Ok(usage(90.0)), now, wall);
        assert_eq!(m.plan(now + Duration::from_secs(60))[0].id, 1);
        let jobs = m.plan(now + Duration::from_secs(900));
        let cc = jobs.iter().find(|j| j.id == 0).unwrap();
        let skipped = execute(
            cc,
            || panic!("must not retry unchanged auth"),
            |_| vec!["credential-v1".into()],
        );
        m.finish(
            cc,
            skipped,
            now + Duration::from_secs(900),
            wall,
            900_000,
            true,
        );
        m.force(now + Duration::from_secs(901), false);
        let jobs = m.plan(now + Duration::from_secs(901));
        let cc = jobs.iter().find(|j| j.id == 0).unwrap();
        let changed = execute(cc, || Ok(usage(20.0)), |_| vec!["credential-v2".into()]);
        assert!(
            m.finish(
                cc,
                changed,
                now + Duration::from_secs(901),
                wall,
                900_000,
                true
            )
            .successful
        );
    }
    #[test]
    fn failed_refresh_keeps_cached_data_and_pending_manual_request_survives_completion() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut m = Monitor::new([false, true, false], now);
        let job = m.plan(now).remove(0);
        complete(&mut m, &job, Ok(usage(55.0)), now, wall);
        m.force(now, true);
        let job = m.plan(now).remove(0);
        m.force(now, true);
        complete(&mut m, &job, Err(PollError::RateLimited(None)), now, wall);
        assert_eq!(m.cached().codex.unwrap().session.percentage, 55.0);
        assert_eq!(m.services[1].history.last_success, Some(wall));
        assert!(m.services[1].stale(wall));
        let pending = m.plan(now).remove(0);
        complete(&mut m, &pending, Ok(usage(65.0)), now, wall);
        assert_eq!(m.services[1].error, None);
        assert!(m.plan(now).is_empty());
    }
    #[test]
    fn all_services_fail_independently_and_only_the_transient_failure_retries() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut m = Monitor::new([true, true, true], now);
        let jobs = m.plan(now);
        for (job, error) in jobs.iter().zip([
            PollError::AuthRequired,
            PollError::ServerError,
            PollError::NoCredentials,
        ]) {
            complete(&mut m, job, Err(error), now, wall);
        }
        assert!(m.services.iter().all(|s| s.history.last_success.is_none()));
        assert_eq!(m.services[0].error, Some(PollError::AuthRequired));
        assert_eq!(m.services[1].error, Some(PollError::ServerError));
        assert_eq!(m.services[2].error, Some(PollError::NoCredentials));
        let retry = m.plan(now + Duration::from_secs(30));
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].id, 1);
    }
    #[test]
    fn network_recovery_refreshes_failed_data_without_retrying_unchanged_auth() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut m = Monitor::new([true, true, false], now);
        let jobs = m.plan(now);
        complete(&mut m, &jobs[0], Err(PollError::AuthRequired), now, wall);
        complete(
            &mut m,
            &jobs[1],
            Err(PollError::NetworkUnavailable),
            now,
            wall,
        );
        m.force(now + Duration::from_secs(1), false);
        let restored = m.plan(now + Duration::from_secs(1));
        let auth = execute(
            &restored[0],
            || panic!("wake must not retry paused authentication"),
            |_| vec!["credential-v1".into()],
        );
        m.finish(&restored[0], auth, now, wall, 900_000, true);
        assert!(complete(&mut m, &restored[1], Ok(usage(20.0)), now, wall).successful);
        assert_eq!(m.services[0].error, Some(PollError::AuthRequired));
        assert_eq!(m.services[1].error, None);
        assert!(m.services[1].history.last_success.is_some());
    }
    #[test]
    fn codex_auth_watch_uses_codex_credentials_and_manual_refresh_survives_a_recovery_event() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut m = Monitor::new([false, true, false], now);
        let job = m.plan(now).remove(0);
        let attempt = execute(
            &job,
            || Err(PollError::TokenExpired),
            |mode| {
                assert_eq!(mode, CredentialWatchMode::Codex);
                vec!["codex-credential-v1".into()]
            },
        );
        m.finish(&job, attempt, now, wall, 900_000, true);
        m.force(now, true);
        m.force(now, false);
        let job = m.plan(now).remove(0);
        let fresh = execute(
            &job,
            || Ok(usage(50.0)),
            |_| panic!("manual refresh bypasses unchanged credentials"),
        );
        assert!(m.finish(&job, fresh, now, wall, 900_000, true).successful);
    }
    #[test]
    fn upcoming_reset_is_polled_on_time_instead_of_waiting_for_the_base_interval() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut m = Monitor::new([false, true, false], now);
        let job = m.plan(now).remove(0);
        let mut data = usage(50.0);
        data.session.resets_at = Some(wall + Duration::from_secs(60));
        complete(&mut m, &job, Ok(data.clone()), now, wall);
        assert_eq!(m.services[1].interval_ms, 60_000);
        m.reconfigure(3_600_000, false, now, wall);
        assert!(m.plan(now + Duration::from_secs(59)).is_empty());
        let job = m.plan(now + Duration::from_secs(60)).remove(0);
        complete(
            &mut m,
            &job,
            Ok(data),
            now + Duration::from_secs(60),
            wall + Duration::from_secs(60),
        );
        assert_eq!(m.services[1].interval_ms, 5_000);
        assert_eq!(m.plan(now + Duration::from_secs(65))[0].id, 1);
    }
}
