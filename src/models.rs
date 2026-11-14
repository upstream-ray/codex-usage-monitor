use std::time::SystemTime;

/// One quota window as the provider reported it. `Default` is a window the
/// provider did not report (`available == false`), shown as `--`; a reported
/// window with 0% used and no reset time is a real reading.
#[derive(Clone, Debug, Default)]
pub struct UsageSection {
    /// Percent used, unrounded.
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
    /// Whether the provider reported this window at all.
    pub available: bool,
}

impl UsageSection {
    /// A window the provider reported.
    pub fn reported(percentage: f64, resets_at: Option<SystemTime>) -> Self {
        Self {
            percentage,
            resets_at,
            available: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
}

#[derive(Clone, Debug, Default)]
pub struct AppUsageData {
    pub claude_code: Option<UsageData>,
    pub codex: Option<UsageData>,
    pub antigravity: Option<UsageData>,
}
