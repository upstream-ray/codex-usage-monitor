mod dutch;
mod english;
mod french;
mod german;
mod italian;
mod japanese;
mod korean;
mod portuguese_brazil;
mod russian;
mod simplified_chinese;
mod spanish;
mod traditional_chinese;

use windows::core::PWSTR;
use windows::Win32::Globalization::{
    GetUserDefaultLocaleName, GetUserDefaultUILanguage, GetUserPreferredUILanguages,
    LCIDToLocaleName, LOCALE_ALLOW_NEUTRAL_NAMES, MAX_LOCALE_NAME, MUI_LANGUAGE_NAME,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageId {
    English,
    Dutch,
    Spanish,
    French,
    German,
    Italian,
    Japanese,
    Korean,
    SimplifiedChinese,
    TraditionalChinese,
    Russian,
    PortugueseBrazil,
}

impl LanguageId {
    pub const ALL: [LanguageId; 12] = [
        LanguageId::English,
        LanguageId::Dutch,
        LanguageId::Spanish,
        LanguageId::French,
        LanguageId::German,
        LanguageId::Italian,
        LanguageId::Japanese,
        LanguageId::Korean,
        LanguageId::SimplifiedChinese,
        LanguageId::TraditionalChinese,
        LanguageId::Russian,
        LanguageId::PortugueseBrazil,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Dutch => "nl",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::German => "de",
            Self::Italian => "it",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::SimplifiedChinese => "zh-CN",
            Self::TraditionalChinese => "zh-TW",
            Self::Russian => "ru",
            Self::PortugueseBrazil => "pt-BR",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Dutch => "Nederlands",
            Self::Spanish => "Español",
            Self::French => "Français",
            Self::German => "Deutsch",
            Self::Italian => "Italiano",
            Self::Japanese => "日本語",
            Self::Korean => "한국어",
            Self::SimplifiedChinese => "简体中文",
            Self::TraditionalChinese => "繁體中文",
            Self::Russian => "Русский",
            Self::PortugueseBrazil => "Português (Brasil)",
        }
    }

    pub fn strings(self) -> Strings {
        match self {
            Self::English => english::STRINGS,
            Self::Dutch => dutch::STRINGS,
            Self::Spanish => spanish::STRINGS,
            Self::French => french::STRINGS,
            Self::German => german::STRINGS,
            Self::Italian => italian::STRINGS,
            Self::Japanese => japanese::STRINGS,
            Self::Korean => korean::STRINGS,
            Self::SimplifiedChinese => simplified_chinese::STRINGS,
            Self::TraditionalChinese => traditional_chinese::STRINGS,
            Self::Russian => russian::STRINGS,
            Self::PortugueseBrazil => portuguese_brazil::STRINGS,
        }
    }

    pub fn update_via_winget_label(self) -> &'static str {
        match self {
            Self::English => english::UPDATE_VIA_WINGET_LABEL,
            Self::Dutch => dutch::UPDATE_VIA_WINGET_LABEL,
            Self::Spanish => spanish::UPDATE_VIA_WINGET_LABEL,
            Self::French => french::UPDATE_VIA_WINGET_LABEL,
            Self::German => german::UPDATE_VIA_WINGET_LABEL,
            Self::Italian => italian::UPDATE_VIA_WINGET_LABEL,
            Self::Japanese => japanese::UPDATE_VIA_WINGET_LABEL,
            Self::Korean => korean::UPDATE_VIA_WINGET_LABEL,
            Self::SimplifiedChinese => simplified_chinese::UPDATE_VIA_WINGET_LABEL,
            Self::TraditionalChinese => traditional_chinese::UPDATE_VIA_WINGET_LABEL,
            Self::Russian => russian::UPDATE_VIA_WINGET_LABEL,
            Self::PortugueseBrazil => portuguese_brazil::UPDATE_VIA_WINGET_LABEL,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        let normalized = code.trim().replace('_', "-").to_ascii_lowercase();
        if normalized.is_empty() || normalized == "system" {
            return None;
        }

        let prefix = normalized.split('-').next().unwrap_or_default();
        match prefix {
            "en" => Some(Self::English),
            "nl" => Some(Self::Dutch),
            "es" => Some(Self::Spanish),
            "fr" => Some(Self::French),
            "de" => Some(Self::German),
            "it" => Some(Self::Italian),
            "ja" => Some(Self::Japanese),
            "ko" => Some(Self::Korean),
            "zh" => {
                if normalized.contains("tw")
                    || normalized.contains("hk")
                    || normalized.contains("hant")
                {
                    Some(Self::TraditionalChinese)
                } else {
                    Some(Self::SimplifiedChinese)
                }
            }
            "ru" => Some(Self::Russian),
            "pt" => Some(Self::PortugueseBrazil),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LanguageId;

    #[test]
    fn parses_simplified_chinese_locales() {
        assert_eq!(
            LanguageId::from_code("zh-CN"),
            Some(LanguageId::SimplifiedChinese)
        );
        assert_eq!(
            LanguageId::from_code("zh_SG"),
            Some(LanguageId::SimplifiedChinese)
        );
        assert_eq!(
            LanguageId::from_code("zh-Hans"),
            Some(LanguageId::SimplifiedChinese)
        );
    }

    #[test]
    fn preserves_traditional_chinese_locales() {
        assert_eq!(
            LanguageId::from_code("zh-TW"),
            Some(LanguageId::TraditionalChinese)
        );
        assert_eq!(
            LanguageId::from_code("zh-HK"),
            Some(LanguageId::TraditionalChinese)
        );
        assert_eq!(
            LanguageId::from_code("zh-Hant"),
            Some(LanguageId::TraditionalChinese)
        );
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Strings {
    pub window_title: &'static str,
    pub refresh: &'static str,
    pub update_frequency: &'static str,
    pub one_minute: &'static str,
    pub five_minutes: &'static str,
    pub fifteen_minutes: &'static str,
    pub one_hour: &'static str,
    pub models: &'static str,
    pub claude_code_model: &'static str,
    pub codex_model: &'static str,
    pub antigravity_model: &'static str,
    pub settings: &'static str,
    pub start_with_windows: &'static str,
    pub reset_position: &'static str,
    pub language: &'static str,
    pub system_default: &'static str,
    pub check_for_updates: &'static str,
    pub checking_for_updates: &'static str,
    pub updates: &'static str,
    pub update_in_progress: &'static str,
    pub up_to_date: &'static str,
    pub up_to_date_short: &'static str,
    pub update_failed: &'static str,
    pub applying_update: &'static str,
    pub update_to: &'static str,
    pub update_available: &'static str,
    pub update_prompt_now: &'static str,
    pub exit: &'static str,
    pub show_widget: &'static str,
    pub session_window: &'static str,
    pub weekly_window: &'static str,
    pub now: &'static str,
    pub day_suffix: &'static str,
    pub hour_suffix: &'static str,
    pub minute_suffix: &'static str,
    pub second_suffix: &'static str,
    pub token_expired_title: &'static str,
    pub token_expired_body: &'static str,
    pub codex_token_expired_title: &'static str,
    pub codex_token_expired_body: &'static str,
    pub antigravity_token_expired_title: &'static str,
    pub antigravity_token_expired_body: &'static str,
    pub codex_window_title: &'static str,
    pub antigravity_window_title: &'static str,
    // Settings window
    pub display_label: &'static str,
    pub start_with_windows_hint: &'static str,
    pub taskbar_widget_section: &'static str,
    pub show_widget_hint: &'static str,
    pub anchor_left: &'static str,
    pub anchor_left_hint: &'static str,
    pub display_hint: &'static str,
    pub single_taskbar_hint: &'static str,
    pub widget_position: &'static str,
    pub widget_position_hint: &'static str,
    pub keep_one_service_hint: &'static str,
    pub antigravity_quota_group: &'static str,
    pub antigravity_quota_group_hint: &'static str,
    pub refresh_section: &'static str,
    pub adaptive_refresh: &'static str,
    pub adaptive_refresh_hint: &'static str,
    pub display_section: &'static str,
    pub show_session_quota: &'static str,
    pub show_weekly_quota: &'static str,
    pub keep_one_quota_hint: &'static str,
    pub quota_value: &'static str,
    pub quota_used: &'static str,
    pub quota_remaining: &'static str,
    pub widget_style_section: &'static str,
    pub background_style: &'static str,
    pub palette_system: &'static str,
    pub settings_theme: &'static str,
    pub palette_dark: &'static str,
    pub palette_light: &'static str,
    pub bar_dots: &'static str,
    pub bar_battery: &'static str,
    pub bar_thin: &'static str,
    pub bar_medium: &'static str,
    pub bar_thick: &'static str,
    pub appearance_preview: &'static str,
    pub colors_section: &'static str,
    pub color_mode: &'static str,
    pub color_brand: &'static str,
    pub color_remaining: &'static str,
    pub color_single: &'static str,
    pub single_color: &'static str,
    pub color_glacier: &'static str,
    pub color_lilac: &'static str,
    pub color_clay: &'static str,
    pub color_rose: &'static str,
    pub color_neutral: &'static str,
    pub text_12_px: &'static str,
    pub text_14_px: &'static str,
    pub text_16_px: &'static str,
    pub bar_style: &'static str,
    pub bar_continuous: &'static str,
    pub bar_segmented: &'static str,
    pub bar_thickness: &'static str,
    pub text_size: &'static str,
    pub show_provider_logos: &'static str,
    pub reset_section: &'static str,
    pub quick_looks: &'static str,
    pub quick_looks_hint: &'static str,
    pub translucent_dark: &'static str,
    pub restore_defaults: &'static str,
    pub low_quota_alert: &'static str,
    pub alert_off: &'static str,
    pub alert_remaining: &'static str,
    pub low_quota_alert_hint: &'static str,
    pub notify_on_reset: &'static str,
    pub notify_on_reset_hint: &'static str,
    pub privacy_section: &'static str,
    pub passive_monitoring: &'static str,
    pub passive_monitoring_hint: &'static str,
    pub local_mcp_server: &'static str,
    pub mcp_server_hint: &'static str,
    pub mcp_port_unavailable: &'static str,
    pub managed_by_policy: &'static str,
    pub allow_updates: &'static str,
    pub update_checks_off: &'static str,
    pub version_available: &'static str,
}

pub fn resolve_language(language_override: Option<LanguageId>) -> LanguageId {
    language_override.unwrap_or_else(detect_system_language)
}

pub fn detect_system_language() -> LanguageId {
    preferred_ui_languages()
        .into_iter()
        .find_map(|locale| LanguageId::from_code(&locale))
        .or_else(default_ui_locale)
        .or_else(default_locale_name)
        .unwrap_or(LanguageId::English)
}

pub fn update_via_winget(language: LanguageId) -> &'static str {
    language.update_via_winget_label()
}

fn preferred_ui_languages() -> Vec<String> {
    unsafe {
        let mut num_languages = 0u32;
        let mut buffer_len = 0u32;
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut num_languages,
            PWSTR::null(),
            &mut buffer_len,
        )
        .is_err()
            || buffer_len == 0
        {
            return Vec::new();
        }

        let mut buffer = vec![0u16; buffer_len as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut num_languages,
            PWSTR(buffer.as_mut_ptr()),
            &mut buffer_len,
        )
        .is_err()
        {
            return Vec::new();
        }

        buffer
            .split(|unit| *unit == 0)
            .filter(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

fn default_ui_locale() -> Option<LanguageId> {
    unsafe {
        let lang_id = GetUserDefaultUILanguage();
        let mut buffer = [0u16; MAX_LOCALE_NAME as usize];
        let len = LCIDToLocaleName(
            lang_id as u32,
            Some(&mut buffer),
            LOCALE_ALLOW_NEUTRAL_NAMES,
        );
        if len <= 1 {
            return None;
        }
        let locale = String::from_utf16_lossy(&buffer[..(len as usize - 1)]);
        LanguageId::from_code(&locale)
    }
}

fn default_locale_name() -> Option<LanguageId> {
    unsafe {
        let mut buffer = [0u16; MAX_LOCALE_NAME as usize];
        let len = GetUserDefaultLocaleName(&mut buffer);
        if len <= 1 {
            return None;
        }
        let locale = String::from_utf16_lossy(&buffer[..(len as usize - 1)]);
        LanguageId::from_code(&locale)
    }
}
