use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::UsageSection;

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ResetObservations([Option<u64>; 6]);

impl ResetObservations {
    pub fn clear_provider(&mut self, provider: usize) {
        self.0[provider * 2..provider * 2 + 2].fill(None);
    }

    /// Returns (baseline changed, new window evidenced by the provider).
    pub fn observe(
        &mut self,
        provider: usize,
        weekly: bool,
        reading: &UsageSection,
        now: SystemTime,
    ) -> (bool, bool) {
        let time = |t: SystemTime| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs());
        let (Some(reset), Some(now)) = (reading.resets_at.and_then(time), time(now)) else {
            return (false, false);
        };
        if !reading.has_reading() || reset <= now {
            return (false, false);
        }
        let previous = &mut self.0[provider * 2 + usize::from(weekly)];
        match *previous {
            Some(old) if now >= old && reset > old && reading.percentage < 100.0 => {
                *previous = (reading.percentage > 0.0).then_some(reset);
                (true, true)
            }
            // Unused quotas can report a rolling future deadline. Track their
            // first actual usage instead of persisting that deadline each poll.
            None if reading.percentage > 0.0 => {
                *previous = Some(reset);
                (true, false)
            }
            Some(old) if reset.abs_diff(old) > 300 && reading.percentage > 0.0 => {
                *previous = Some(reset);
                (true, false)
            }
            _ => (false, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_a_confirmed_new_window_notifies_and_the_baseline_survives_restart() {
        let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
        let mut observed = ResetObservations::default();
        let before = UsageSection::reported(100.0, Some(at(1000)));
        assert_eq!(observed.observe(1, false, &before, at(900)), (true, false));
        let jitter = UsageSection::reported(100.0, Some(at(1030)));
        assert_eq!(observed.observe(1, false, &jitter, at(920)), (false, false));
        assert_eq!(
            observed.observe(1, false, &before, at(1001)),
            (false, false)
        );

        let mut restored: ResetObservations =
            serde_json::from_str(&serde_json::to_string(&observed).unwrap()).unwrap();
        let after = UsageSection::reported(0.0, Some(at(19_000)));
        assert_eq!(restored.observe(1, false, &after, at(1001)), (true, true));
        assert_eq!(restored.observe(1, false, &after, at(1002)), (false, false));
        assert_eq!(restored.observe(1, true, &after, at(1002)), (false, false));
        restored.clear_provider(1);
    }
}
