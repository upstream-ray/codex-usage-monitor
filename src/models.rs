use std::time::SystemTime;

#[derive(Clone, Debug, Default)]
pub struct UsageSection {
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
    reported: bool,
}

impl UsageSection {
    pub fn reported(percentage: f64, resets_at: Option<SystemTime>) -> Self {
        Self {
            percentage,
            resets_at,
            reported: true,
        }
    }

    pub fn has_reading(&self) -> bool {
        self.reported && self.percentage.is_finite()
    }
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
    pub credit_balance: Option<f64>,
    pub antigravity_groups: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AppUsageData {
    pub claude_code: Option<UsageData>,
    pub codex: Option<UsageData>,
    pub antigravity: Option<UsageData>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaDisplayMode {
    Used,
    #[default]
    Remaining,
}

impl QuotaDisplayMode {
    pub fn percentage(self, used: f64) -> f64 {
        match self {
            Self::Used => used.clamp(0.0, 100.0),
            Self::Remaining => (100.0 - used).clamp(0.0, 100.0),
        }
    }
}
