//! Keep a low-quota notification latched across refreshes and reset-time jitter.
use std::collections::BTreeSet;

const RESET_JITTER_SECONDS: u64 = 300;

pub fn should_notify(
    notified: &mut BTreeSet<String>,
    prefix: &str,
    reset: Option<u64>,
    remaining: u8,
    threshold: u8,
) -> bool {
    let previous: Vec<_> = notified
        .iter()
        .filter(|key| key.starts_with(prefix))
        .cloned()
        .collect();
    let previous_reset = previous
        .iter()
        .filter_map(|key| key.strip_prefix(prefix)?.parse::<u64>().ok())
        .max();
    // Require a meaningful forward change; second-level drift and older responses
    // must not erase an already delivered alert.
    let new_window = match (previous_reset, reset) {
        (Some(old), Some(new)) => new > old.saturating_add(RESET_JITTER_SECONDS),
        _ => false,
    };
    if new_window {
        notified.retain(|key| !key.starts_with(prefix));
    } else if !previous.is_empty() {
        // Fill in a previously unknown reset without issuing a second alert.
        if previous_reset.is_none() {
            if let Some(reset) = reset {
                notified.retain(|key| !key.starts_with(prefix));
                notified.insert(format!("{prefix}{reset}"));
            } else if remaining > threshold {
                // With no reset metadata, recovered quota is the only evidence
                // that the previous low-quota episode has ended.
                notified.retain(|key| !key.starts_with(prefix));
            }
        }
        return false;
    }
    if remaining > threshold {
        return false;
    }
    notified.insert(format!(
        "{prefix}{}",
        reset.map_or_else(|| "unknown".into(), |v| v.to_string())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_reset_and_restart_do_not_repeat_an_alert() {
        let mut keys = BTreeSet::new();
        assert!(should_notify(&mut keys, "claude:session:", None, 5, 10));
        assert!(!should_notify(
            &mut keys,
            "claude:session:",
            Some(1000),
            4,
            10
        ));
        let json = serde_json::to_string(&keys).unwrap();
        let mut restored = serde_json::from_str(&json).unwrap();
        assert!(!should_notify(
            &mut restored,
            "claude:session:",
            None,
            3,
            10
        ));
        assert!(!should_notify(
            &mut restored,
            "claude:session:",
            Some(1001),
            2,
            10
        ));
        assert!(should_notify(
            &mut restored,
            "claude:session:",
            Some(19_000),
            5,
            10
        ));
    }

    #[test]
    fn jitter_does_not_accumulate_and_other_windows_remain_independent() {
        let mut keys = BTreeSet::new();
        assert!(should_notify(
            &mut keys,
            "claude:session:",
            Some(1000),
            5,
            10
        ));
        for reset in [1001, 999, 1100, 1000] {
            assert!(!should_notify(
                &mut keys,
                "claude:session:",
                Some(reset),
                4,
                10
            ));
        }
        assert!(should_notify(&mut keys, "codex:weekly:", Some(1000), 5, 10));
        assert!(!should_notify(
            &mut keys,
            "claude:session:",
            Some(1000),
            20,
            10
        ));
        assert!(!should_notify(
            &mut keys,
            "claude:session:",
            Some(1000),
            4,
            10
        ));
    }
}
