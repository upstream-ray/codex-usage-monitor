//! Adaptive refresh cadence and honest timestamps for displayed quota data.
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::models::UsageData;
use crate::native_interop;

const FAST_INTERVAL_MS: u32 = 60_000;
const LOW_REMAINING_PERCENT: f64 = 20.0;
pub static POLLS: PollGate = PollGate::new();

#[derive(Default)]
struct Pending {
    running: bool,
    requested: bool,
}
pub struct PollGate(Mutex<Pending>);
impl PollGate {
    pub const fn new() -> Self {
        Self(Mutex::new(Pending {
            running: false,
            requested: false,
        }))
    }
    /// Many requests during a poll become one follow-up, without losing a click.
    pub fn request(&self, queue: bool) -> Option<PollGuard<'_>> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.running {
            state.requested |= queue;
            None
        } else {
            state.running = true;
            Some(PollGuard {
                gate: self,
                active: true,
            })
        }
    }
}
pub struct PollGuard<'a> {
    gate: &'a PollGate,
    active: bool,
}
impl PollGuard<'_> {
    pub fn next(&mut self) -> bool {
        let mut state = self.gate.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.requested {
            state.requested = false;
            true
        } else {
            // Release ownership under the same lock as the final pending check.
            state.running = false;
            self.active = false;
            false
        }
    }
}
impl Drop for PollGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            let mut state = self.gate.0.lock().unwrap_or_else(|e| e.into_inner());
            state.running = false;
            state.requested = false;
        }
    }
}

/// Limit retries of a reset timestamp that the service keeps returning unchanged.
#[derive(Default)]
pub struct ResetRefresh {
    reset: Option<SystemTime>,
    started: Option<Instant>,
}
impl ResetRefresh {
    pub fn interval_ms(&mut self, reset: Option<SystemTime>, now: Instant) -> Option<u32> {
        if self.reset != reset {
            self.reset = reset;
            self.started = reset.map(|_| now);
        }
        let elapsed = now.saturating_duration_since(self.started?);
        if elapsed < Duration::from_secs(30) {
            Some(5_000)
        } else if elapsed < Duration::from_secs(120) {
            Some(30_000)
        } else {
            None
        }
    }
}

pub fn interval_ms<'a>(
    base: u32,
    adaptive: bool,
    providers: impl IntoIterator<Item = &'a UsageData>,
) -> u32 {
    let low = adaptive
        && providers.into_iter().any(|usage| {
            let sections = [&usage.session, &usage.weekly];
            // A provider blocked by an exhausted window cannot consume more quota.
            if sections.iter().any(|s| s.percentage >= 100.0) {
                return false;
            }
            sections.iter().any(|s| {
                s.percentage.is_finite()
                    && s.percentage >= 100.0 - LOW_REMAINING_PERCENT
                    && s.percentage < 100.0
            })
        });
    if low {
        base.min(FAST_INTERVAL_MS)
    } else {
        base
    }
}

pub fn retry_interval_ms(base: u32, retries: u32) -> u32 {
    30_000u32
        .saturating_mul(
            1u32.checked_shl(retries.saturating_sub(1))
                .unwrap_or(u32::MAX),
        )
        .min(base)
}

#[derive(Default)]
pub struct History {
    pub last_success: Option<SystemTime>,
    pub expected_interval_ms: u32,
}
impl History {
    pub fn record_success(&mut self, now: SystemTime, interval_ms: u32) {
        self.last_success = Some(now);
        self.expected_interval_ms = interval_ms;
    }

    pub fn is_stale(&self, now: SystemTime) -> bool {
        self.last_success.is_some_and(|last| {
            let grace = (self.expected_interval_ms / 10).max(30_000);
            now.duration_since(last).unwrap_or_default()
                > Duration::from_millis(self.expected_interval_ms as u64 + grace as u64)
        })
    }

    pub fn description(
        &self,
        now: SystemTime,
        chinese: bool,
        last_poll_ok: bool,
        interval_ms: u32,
    ) -> String {
        let Some(last) = self.last_success else {
            return if chinese {
                "等待首次成功更新。"
            } else {
                "Waiting for the first successful update."
            }
            .into();
        };
        let age = now.duration_since(last).unwrap_or_default().as_secs() / 60;
        let time = native_interop::system_time_to_local(last)
            .map(|t| format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond))
            .unwrap_or_else(|| "--".into());
        let cadence = if interval_ms < 60_000 {
            if chinese {
                format!("{} 秒", interval_ms / 1000)
            } else {
                format!("{} sec", interval_ms / 1000)
            }
        } else if chinese {
            format!("{} 分钟", interval_ms / 60_000)
        } else {
            format!("{} min", interval_ms / 60_000)
        };
        let mut result = if chinese {
            format!(
                "数字表示剩余额度。\n上次成功更新：{time}（{age} 分钟前）\n当前刷新间隔：{cadence}。"
            )
        } else {
            format!("Numbers indicate quota used.\nLast successful update: {time} ({age} min ago)\nCurrent refresh interval: {cadence}.")
        };
        if !last_poll_ok {
            result.push_str(if chinese {
                "\n最近更新失败，正在等待重试或重新登录。"
            } else {
                "\nThe latest update failed; awaiting retry or sign-in."
            });
        }
        if self.is_stale(now) {
            result.push_str(if chinese {
                "\n* 数据已过期，请刷新确认。"
            } else {
                "\n* Data is stale; refresh to confirm."
            });
        }
        result
    }
}

pub fn mark_stale(value: &mut String, stale: bool) {
    if stale && value.contains('%') && !value.ends_with(" *") {
        value.push_str(" *");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageSection;

    fn usage(session: f64, weekly: f64) -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: session,
                resets_at: None,
                reported: true,
            },
            weekly: UsageSection {
                percentage: weekly,
                resets_at: None,
                reported: true,
            },
        }
    }
    #[test]
    fn low_quota_accelerates_and_recovery_restores_the_base_interval() {
        assert_eq!(interval_ms(900_000, true, [&usage(98.0, 13.0)]), 60_000);
        assert_eq!(interval_ms(900_000, true, [&usage(80.0, 13.0)]), 60_000);
        assert_eq!(interval_ms(900_000, true, [&usage(79.0, 13.0)]), 900_000);
        assert_eq!(interval_ms(30_000, true, [&usage(98.0, 13.0)]), 30_000);
        assert_eq!(interval_ms(900_000, false, [&usage(98.0, 13.0)]), 900_000);
    }
    #[test]
    fn missing_and_exhausted_windows_do_not_accelerate() {
        assert_eq!(interval_ms(900_000, true, [&UsageData::default()]), 900_000);
        assert_eq!(interval_ms(900_000, true, [&usage(98.0, 100.0)]), 900_000);
        assert_eq!(
            interval_ms(900_000, true, [&usage(100.0, 13.0), &usage(95.0, 20.0)]),
            60_000
        );
    }
    #[test]
    fn failed_requests_keep_exponential_backoff_independent_of_low_quota() {
        assert_eq!(retry_interval_ms(900_000, 1), 30_000);
        assert_eq!(retry_interval_ms(900_000, 2), 60_000);
        assert_eq!(retry_interval_ms(900_000, 6), 900_000);
    }
    #[test]
    fn timestamps_age_only_after_success_and_recovery_clears_staleness() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let mut history = History::default();
        assert!(!history.is_stale(now));
        history.record_success(now, 60_000);
        assert!(!history.is_stale(now + Duration::from_secs(90)));
        assert!(history.is_stale(now + Duration::from_secs(91)));
        history.record_success(now + Duration::from_secs(100), 60_000);
        assert!(!history.is_stale(now + Duration::from_secs(101)));
        let mut missing = "--".to_string();
        mark_stale(&mut missing, true);
        assert_eq!(missing, "--");
        let mut percentage = "2% 01:59重置".to_string();
        mark_stale(&mut percentage, true);
        assert_eq!(percentage, "2% 01:59重置 *");
    }
    #[test]
    fn polls_cannot_overlap_and_the_guard_is_released() {
        let gate = PollGate::new();
        let guard = gate.request(true).unwrap();
        assert!(gate.request(true).is_none());
        drop(guard);
        assert!(gate.request(true).is_some());
    }
    #[test]
    fn clicks_coalesce_and_finishing_cannot_release_a_new_worker() {
        let gate = PollGate::new();
        let mut guard = gate.request(true).unwrap();
        for _ in 0..100 {
            assert!(gate.request(true).is_none());
        }
        assert!(guard.next());
        assert!(!guard.next());
        let next = gate.request(true).unwrap();
        drop(guard);
        assert!(gate.request(true).is_none());
        drop(next);
        assert!(gate.request(true).is_some());
    }
    #[test]
    fn periodic_ticks_do_not_queue_a_retry_that_bypasses_backoff() {
        let gate = PollGate::new();
        let mut guard = gate.request(false).unwrap();
        assert!(gate.request(false).is_none());
        assert!(!guard.next());
    }
    #[test]
    fn stale_reset_retries_slow_down_then_stop_until_a_new_window() {
        let start = Instant::now();
        let reset = Some(SystemTime::UNIX_EPOCH);
        let mut refresh = ResetRefresh::default();
        assert_eq!(refresh.interval_ms(reset, start), Some(5_000));
        assert_eq!(
            refresh.interval_ms(reset, start + Duration::from_secs(30)),
            Some(30_000)
        );
        assert_eq!(
            refresh.interval_ms(reset, start + Duration::from_secs(120)),
            None
        );
        assert_eq!(
            refresh.interval_ms(reset, start + Duration::from_secs(600)),
            None
        );
        assert_eq!(
            refresh.interval_ms(None, start + Duration::from_secs(601)),
            None
        );
        assert_eq!(
            refresh.interval_ms(reset, start + Duration::from_secs(602)),
            Some(5_000)
        );
    }
    #[test]
    fn retry_tooltip_reports_seconds_instead_of_zero_minutes() {
        let mut history = History::default();
        history.record_success(SystemTime::now(), 60_000);
        let text = history.description(SystemTime::now(), true, false, 30_000);
        assert!(text.contains("30 秒"));
        assert!(!text.contains("0 分钟。"));
    }
}
