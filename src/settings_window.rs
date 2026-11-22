//! Modeless settings for the taskbar widget. Controls reflect the live app state.
//!
//! The window is drawn in the Windows 11 settings style: grouped cards with
//! switches, drop-down buttons and push buttons. It follows the selected app
//! theme and Windows accent color. Native buttons keep keyboard and accessibility
//! behavior; only their painting is customized. The existing widget command
//! handlers own validation, rendering, and persistence.
use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::OnceLock;
use windows::core::{w, PCSTR};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::Graphics::GdiPlus as gp;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::{
    SetScrollInfo, SetWindowTheme, CDDS_PREPAINT, CDIS_DISABLED, CDIS_FOCUS, CDIS_HOT,
    CDIS_SELECTED, CDIS_SHOWKEYBOARDCUES, CDRF_DODEFAULT, CDRF_SKIPDEFAULT, DRAWITEMSTRUCT,
    NMCUSTOMDRAW, NMHDR, NM_CUSTOMDRAW, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED, WM_MOUSELEAVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, IsWindowEnabled, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};

const CLASS: PCWSTR = w!("CodexUsageSettings");
const NAV: u16 = 990;
const FIRST_FIELD: u16 = 1000;
const FIELDS_PER_ROW: u16 = 4;
const REFRESH_TIMER: usize = 1;
const ANIMATION_TIMER: usize = 2;
const SWITCH_DURATION: Duration = Duration::from_millis(150);
const PAGE_DURATION: Duration = Duration::from_millis(180);
const APPLY_SELECTION: u32 = WM_APP + 1;
const ENSURE_VISIBLE: u32 = WM_APP + 2;
const SUBCLASS_ID: usize = 1;

// Layout in 96-DPI pixels.
const WIDTH: i32 = 900;
const HEIGHT: i32 = 640;
const MIN_WIDTH: i32 = 720;
const MIN_HEIGHT: i32 = 480;
const NAV_WIDTH: i32 = 236;
const NAV_TOP: i32 = 72;
const NAV_ITEM: i32 = 40;
const CONTENT_LEFT: i32 = 12;
const CONTENT_RIGHT: i32 = 32;
const CONTENT_MAX: i32 = 1000;
const TOP: i32 = 24;
const TITLE_HEIGHT: i32 = 44;
const CARD_MIN: i32 = 56;
const CARD_VPAD: i32 = 13;
const CARD_PAD: i32 = 16;
const CARD_GAP: i32 = 4;
const CONTROL_GAP: i32 = 16;
const BUTTON_HEIGHT: i32 = 32;
const BUTTON_GAP: i32 = 8;
const DROPDOWN_WIDTH: i32 = 240;
const TOGGLE_WIDTH: i32 = 40;
const TOGGLE_HEIGHT: i32 = 20;
const CARD_RADIUS: f32 = 6.0;
const CONTROL_RADIUS: f32 = 4.0;
const WHEEL_STEP: i32 = 64;

// Segoe Fluent Icons / Segoe MDL2 Assets code points.
const GLYPH_CHEVRON_DOWN: char = '\u{E70D}';
const GLYPH_ERROR: char = '\u{EA39}';
const PAGE_GLYPHS: [char; 5] = ['\u{E713}', '\u{E9D2}', '\u{E790}', '\u{EA8F}', '\u{E72E}'];

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
    // Kept outside the panel so control colors never wait on a panel borrow.
    static BACKGROUND: Cell<u32> = const { Cell::new(0xF3_F3F3) };
    static REDRAW_PENDING: Cell<bool> = const { Cell::new(false) };
    static NAV_HOVER: Cell<i32> = const { Cell::new(-1) };
}
static SAVE_FAILED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Command(u16),
    Display(HWND),
    Group(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OptionItem {
    label: String,
    action: Option<Action>,
    /// Color sample (0xRRGGBB) drawn before a button label.
    swatch: Option<u32>,
}

impl OptionItem {
    fn command(label: impl Into<String>, id: u16) -> Self {
        Self {
            label: label.into(),
            action: Some(Action::Command(id)),
            swatch: None,
        }
    }

    fn with_swatch(mut self, color: Color) -> Self {
        self.swatch = Some(u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b));
        self
    }
}

/// Secondary text under a setting. Warnings use the caution color and are
/// included in the control's accessible name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hint {
    Info(String),
    Warning(String),
}

impl Hint {
    fn text(&self) -> &str {
        match self {
            Self::Info(text) | Self::Warning(text) => text,
        }
    }
}

fn info(text: impl Into<String>) -> Option<Hint> {
    Some(Hint::Info(text.into()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Section(String),
    Preview {
        appearance: Appearance,
        system_dark: bool,
    },
    Check {
        item: OptionItem,
        checked: bool,
        enabled: bool,
        hint: Option<Hint>,
    },
    Choice {
        label: String,
        items: Vec<OptionItem>,
        selected: usize,
        enabled: bool,
        hint: Option<Hint>,
    },
    /// A card with a description on the left and buttons on the right.
    Action {
        label: String,
        hint: Option<Hint>,
        buttons: Vec<OptionItem>,
    },
    Note(Hint),
}

impl Row {
    fn hint(mut self, value: Option<Hint>) -> Self {
        match &mut self {
            Self::Check { hint, .. } | Self::Choice { hint, .. } | Self::Action { hint, .. } => {
                *hint = value;
            }
            Self::Section(_) | Self::Note(_) | Self::Preview { .. } => {}
        }
        self
    }

    fn control_count(&self) -> usize {
        match self {
            Self::Check { .. } | Self::Choice { .. } => 1,
            Self::Action { buttons, .. } => buttons.len(),
            Self::Section(_) | Self::Note(_) | Self::Preview { .. } => 0,
        }
    }
}

fn check(label: impl Into<String>, id: u16, checked: bool) -> Row {
    limited_check(label, id, checked, true)
}

fn limited_check(label: impl Into<String>, id: u16, checked: bool, enabled: bool) -> Row {
    Row::Check {
        item: OptionItem::command(label, id),
        checked,
        enabled,
        hint: None,
    }
}

fn section(title: impl Into<String>) -> Row {
    Row::Section(title.into())
}

fn action(label: impl Into<String>, hint: Option<Hint>, buttons: Vec<OptionItem>) -> Row {
    Row::Action {
        label: label.into(),
        hint,
        buttons,
    }
}

fn service_status(
    monitored: bool,
    error: Option<poller::PollError>,
    language: LanguageId,
) -> Option<&'static str> {
    use poller::PollError;

    let reauthenticate = match (monitored, error) {
        (true, Some(PollError::NoCredentials)) => false,
        (true, Some(PollError::AuthRequired | PollError::TokenExpired)) => true,
        _ => return None,
    };
    let (signed_out, sign_in_again) = match language {
        LanguageId::English => ("Not signed in", "Sign in again"),
        LanguageId::Dutch => ("Niet aangemeld", "Meld je opnieuw aan"),
        LanguageId::Spanish => ("Sesión no iniciada", "Vuelve a iniciar sesión"),
        LanguageId::French => ("Non connecté", "Reconnectez-vous"),
        LanguageId::German => ("Nicht angemeldet", "Erneut anmelden"),
        LanguageId::Italian => ("Accesso non effettuato", "Accedi di nuovo"),
        LanguageId::Japanese => ("未ログイン", "再ログインが必要"),
        LanguageId::Korean => ("로그인 안 됨", "다시 로그인 필요"),
        LanguageId::SimplifiedChinese => ("未登录", "需重新登录"),
        LanguageId::TraditionalChinese => ("未登入", "需重新登入"),
        LanguageId::Russian => ("Вход не выполнен", "Войдите снова"),
        LanguageId::PortugueseBrazil => ("Sessão não iniciada", "Entre novamente"),
    };
    Some(if reauthenticate {
        sign_in_again
    } else {
        signed_out
    })
}

/// Accessible name of a setting: its label plus any warning shown under it.
fn accessible_text(label: &str, hint: Option<&Hint>) -> String {
    match hint {
        Some(Hint::Warning(warning)) => format!("{label} ({warning})"),
        _ => label.into(),
    }
}

fn choice(label: &str, values: &[(u16, &str)], selected_command: u16) -> Row {
    Row::Choice {
        label: label.into(),
        items: values
            .iter()
            .map(|(id, name)| OptionItem::command(*name, *id))
            .collect(),
        selected: values
            .iter()
            .position(|(id, _)| *id == selected_command)
            .unwrap_or(0),
        enabled: true,
        hint: None,
    }
}

/// "Display 2" from `\\.\DISPLAY2`, so the number matches Windows display settings.
fn display_label(prefix: &str, index: usize, device: Option<&str>) -> String {
    let number = device
        .and_then(|device| device.strip_prefix(r"\\.\DISPLAY"))
        .and_then(|number| number.parse::<u32>().ok());
    match (number, device) {
        (Some(number), _) => format!("{prefix} {number}"),
        (None, Some(device)) => format!("{prefix} {} ({device})", index + 1),
        (None, None) => format!("{prefix} {}", index + 1),
    }
}

#[derive(Clone, Copy)]
enum Page {
    General,
    Usage,
    Appearance,
    Alerts,
    Privacy,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Usage,
        Self::Appearance,
        Self::Alerts,
        Self::Privacy,
    ];
}

fn page_titles(language: LanguageId) -> [&'static str; 5] {
    match language {
        LanguageId::English => [
            "General",
            "Usage",
            "Appearance",
            "Alerts",
            "Privacy & updates",
        ],
        LanguageId::Dutch => [
            "Algemeen",
            "Gebruik",
            "Weergave",
            "Meldingen",
            "Privacy en updates",
        ],
        LanguageId::Spanish => [
            "General",
            "Uso",
            "Apariencia",
            "Avisos",
            "Privacidad y actualizaciones",
        ],
        LanguageId::French => [
            "Général",
            "Utilisation",
            "Apparence",
            "Alertes",
            "Confidentialité et mises à jour",
        ],
        LanguageId::German => [
            "Allgemein",
            "Nutzung",
            "Darstellung",
            "Hinweise",
            "Datenschutz & Updates",
        ],
        LanguageId::Italian => [
            "Generali",
            "Utilizzo",
            "Aspetto",
            "Avvisi",
            "Privacy e aggiornamenti",
        ],
        LanguageId::Japanese => ["一般", "使用量", "外観", "通知", "プライバシーと更新"],
        LanguageId::Korean => ["일반", "사용량", "모양", "알림", "개인정보 및 업데이트"],
        LanguageId::SimplifiedChinese => ["常规", "用量", "外观", "提醒", "隐私与更新"],
        LanguageId::TraditionalChinese => ["一般", "用量", "外觀", "通知", "隱私與更新"],
        LanguageId::Russian => [
            "Общие",
            "Использование",
            "Оформление",
            "Уведомления",
            "Конфиденциальность",
        ],
        LanguageId::PortugueseBrazil => [
            "Geral",
            "Uso",
            "Aparência",
            "Alertas",
            "Privacidade e atualizações",
        ],
    }
}

fn rows(page: Page, s: &AppState) -> Vec<Row> {
    let strings = s.language.strings();
    match page {
        Page::General => {
            let mut values = vec![(IDM_LANG_SYSTEM, strings.system_default)];
            values.extend(
                LanguageId::ALL
                    .map(|language| (language_command(language), language.native_name())),
            );
            let selected_language = s
                .language_override
                .map_or(IDM_LANG_SYSTEM, language_command);
            let taskbars = native_interop::find_taskbars();
            let displays = taskbars
                .iter()
                .enumerate()
                .map(|(index, taskbar)| {
                    let device = native_interop::taskbar_monitor_device(taskbar);
                    OptionItem {
                        label: display_label(strings.display_label, index, device.as_deref()),
                        action: Some(Action::Display(taskbar.hwnd)),
                        swatch: None,
                    }
                })
                .collect();
            let selected_display = taskbars
                .iter()
                .position(|bar| Some(bar.hwnd) == s.taskbar_hwnd);
            let mut result = vec![
                choice(
                    strings.settings_theme,
                    &[
                        (IDM_THEME_SYSTEM, strings.palette_system),
                        (IDM_THEME_LIGHT, strings.palette_light),
                        (IDM_THEME_DARK, strings.palette_dark),
                    ],
                    match s.settings_theme {
                        SettingsTheme::System => IDM_THEME_SYSTEM,
                        SettingsTheme::Light => IDM_THEME_LIGHT,
                        SettingsTheme::Dark => IDM_THEME_DARK,
                    },
                ),
                choice(strings.language, &values, selected_language),
                check(
                    strings.start_with_windows,
                    IDM_START_WITH_WINDOWS,
                    is_startup_enabled(),
                )
                .hint(info(strings.start_with_windows_hint)),
                section(strings.taskbar_widget_section),
                check(
                    strings.show_widget,
                    tray_icon::IDM_TOGGLE_WIDGET,
                    s.widget_visible,
                )
                .hint(info(strings.show_widget_hint)),
                check(strings.anchor_left, IDM_ANCHOR_LEFT, s.anchor_left)
                    .hint(info(strings.anchor_left_hint)),
            ];
            if !taskbars.is_empty() {
                let several = taskbars.len() > 1;
                result.push(Row::Choice {
                    label: strings.display_label.into(),
                    items: displays,
                    selected: selected_display.unwrap_or(0),
                    enabled: several,
                    hint: info(if several {
                        strings.display_hint
                    } else {
                        strings.single_taskbar_hint
                    }),
                });
            }
            result.push(action(
                strings.widget_position,
                info(strings.widget_position_hint),
                vec![OptionItem::command(
                    strings.reset_position,
                    IDM_RESET_POSITION,
                )],
            ));
            result
        }
        Page::Usage => {
            let providers = [s.show_codex, s.show_claude_code, s.show_antigravity];
            let count = providers.iter().filter(|enabled| **enabled).count();
            let [claude, codex, antigravity] = &s.monitor.services;
            let provider = |name: &str, id, monitored, error| {
                let hint = match service_status(monitored, error, s.language) {
                    Some(status) => Some(Hint::Warning(status.into())),
                    None if monitored && count == 1 => info(strings.keep_one_service_hint),
                    None => None,
                };
                limited_check(name, id, monitored, !monitored || count > 1).hint(hint)
            };
            let mut groups = vec![None];
            groups.extend(
                s.antigravity_groups
                    .iter()
                    .filter(|name| !name.eq_ignore_ascii_case(poller::GEMINI_QUOTA_GROUP))
                    .take(MAX_QUOTA_GROUPS)
                    .cloned()
                    .map(Some),
            );
            if let Some(group) = &s.antigravity_quota_group {
                if !groups.iter().any(|option| option.as_ref() == Some(group)) {
                    groups.truncate(MAX_QUOTA_GROUPS);
                    groups.push(Some(group.clone()));
                }
            }
            let selected_group = groups
                .iter()
                .position(|group| group == &s.antigravity_quota_group)
                .unwrap_or(0);
            let group_items = groups
                .into_iter()
                .map(|group| OptionItem {
                    label: group
                        .as_deref()
                        .unwrap_or(poller::GEMINI_QUOTA_GROUP)
                        .into(),
                    action: Some(Action::Group(group)),
                    swatch: None,
                })
                .collect();
            let one_window = info(strings.keep_one_quota_hint);
            vec![
                section(strings.models),
                provider(
                    strings.codex_model,
                    IDM_MODEL_CODEX,
                    s.show_codex,
                    codex.error,
                ),
                provider(
                    strings.claude_code_model,
                    IDM_MODEL_CLAUDE_CODE,
                    s.show_claude_code,
                    claude.error,
                ),
                provider(
                    strings.antigravity_model,
                    IDM_MODEL_ANTIGRAVITY,
                    s.show_antigravity,
                    antigravity.error,
                ),
                Row::Choice {
                    label: strings.antigravity_quota_group.into(),
                    items: group_items,
                    selected: selected_group,
                    enabled: s.show_antigravity,
                    hint: info(strings.antigravity_quota_group_hint),
                },
                section(strings.refresh_section),
                choice(
                    strings.update_frequency,
                    &[
                        (IDM_FREQ_1MIN, strings.one_minute),
                        (IDM_FREQ_5MIN, strings.five_minutes),
                        (IDM_FREQ_15MIN, strings.fifteen_minutes),
                        (IDM_FREQ_1HOUR, strings.one_hour),
                    ],
                    match s.poll_interval_ms {
                        POLL_1_MIN => IDM_FREQ_1MIN,
                        POLL_5_MIN => IDM_FREQ_5MIN,
                        POLL_1_HOUR => IDM_FREQ_1HOUR,
                        _ => IDM_FREQ_15MIN,
                    },
                ),
                check(
                    strings.adaptive_refresh,
                    IDM_ADAPTIVE_REFRESH,
                    s.adaptive_refresh,
                )
                .hint(info(strings.adaptive_refresh_hint)),
                section(strings.display_section),
                limited_check(
                    strings.show_session_quota,
                    IDM_SHOW_SESSION_WINDOW,
                    s.show_session_window,
                    s.show_weekly_window,
                )
                .hint(if s.show_weekly_window {
                    None
                } else {
                    one_window.clone()
                }),
                limited_check(
                    strings.show_weekly_quota,
                    IDM_SHOW_WEEKLY_WINDOW,
                    s.show_weekly_window,
                    s.show_session_window,
                )
                .hint(if s.show_session_window {
                    None
                } else {
                    one_window
                }),
                choice(
                    strings.quota_value,
                    &[
                        (IDM_QUOTA_REMAINING, strings.quota_remaining),
                        (IDM_QUOTA_USED, strings.quota_used),
                    ],
                    if s.quota_display_mode == QuotaDisplayMode::Used {
                        IDM_QUOTA_USED
                    } else {
                        IDM_QUOTA_REMAINING
                    },
                ),
            ]
        }
        Page::Appearance => {
            let a = s.appearance;
            let mut result = vec![
                Row::Preview {
                    appearance: a,
                    system_dark: s.is_dark,
                },
                section(strings.widget_style_section),
                choice(
                    strings.background_style,
                    &[
                        (IDM_PALETTE_SYSTEM, strings.palette_system),
                        (IDM_PALETTE_DARK, strings.palette_dark),
                        (IDM_PALETTE_LIGHT, strings.palette_light),
                    ],
                    match a.palette {
                        Palette::System => IDM_PALETTE_SYSTEM,
                        Palette::HighContrastDark => IDM_PALETTE_DARK,
                        Palette::HighContrastLight => IDM_PALETTE_LIGHT,
                    },
                ),
                choice(
                    strings.bar_style,
                    &[
                        (IDM_BAR_CONTINUOUS, strings.bar_continuous),
                        (IDM_BAR_SEGMENTED, strings.bar_segmented),
                        (IDM_BAR_DOTS, strings.bar_dots),
                        (IDM_BAR_BATTERY, strings.bar_battery),
                    ],
                    match a.bar_style {
                        BarStyle::Continuous => IDM_BAR_CONTINUOUS,
                        BarStyle::Segmented => IDM_BAR_SEGMENTED,
                        BarStyle::Dots => IDM_BAR_DOTS,
                        BarStyle::Battery => IDM_BAR_BATTERY,
                    },
                ),
                choice(
                    strings.bar_thickness,
                    &[
                        (IDM_BAR_THIN, strings.bar_thin),
                        (IDM_BAR_MEDIUM, strings.bar_medium),
                        (IDM_BAR_THICK, strings.bar_thick),
                    ],
                    match a.bar_thickness {
                        BarThickness::Thin => IDM_BAR_THIN,
                        BarThickness::Medium => IDM_BAR_MEDIUM,
                        BarThickness::Thick => IDM_BAR_THICK,
                    },
                ),
                choice(
                    strings.text_size,
                    &[
                        (IDM_FONT_12, strings.text_12_px),
                        (IDM_FONT_14, strings.text_14_px),
                        (IDM_FONT_16, strings.text_16_px),
                    ],
                    match a.font_size {
                        FontSize::Px12 => IDM_FONT_12,
                        FontSize::Px14 => IDM_FONT_14,
                        FontSize::Px16 => IDM_FONT_16,
                    },
                ),
                check(
                    strings.show_provider_logos,
                    IDM_PROVIDER_LOGOS,
                    !a.hide_provider_logos,
                ),
                section(strings.colors_section),
                choice(
                    strings.color_mode,
                    &[
                        (IDM_COLOR_BRAND, strings.color_brand),
                        (IDM_COLOR_REMAINING, strings.color_remaining),
                        (IDM_COLOR_SINGLE, strings.color_single),
                    ],
                    match a.color_mode {
                        ColorMode::Brand => IDM_COLOR_BRAND,
                        ColorMode::Remaining => IDM_COLOR_REMAINING,
                        ColorMode::Single => IDM_COLOR_SINGLE,
                    },
                ),
            ];
            if a.color_mode == ColorMode::Single {
                let mut selected = 0;
                let items = SingleColor::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, color)| {
                        if color == a.single_color {
                            selected = index;
                        }
                        let name = match color {
                            SingleColor::Glacier => strings.color_glacier,
                            SingleColor::Lilac => strings.color_lilac,
                            SingleColor::Clay => strings.color_clay,
                            SingleColor::Rose => strings.color_rose,
                            SingleColor::Neutral => strings.color_neutral,
                        };
                        OptionItem::command(name, IDM_SINGLE_COLOR_FIRST + index as u16)
                            .with_swatch(color.bar(a.is_dark(s.is_dark)))
                    })
                    .collect();
                result.push(Row::Choice {
                    label: strings.single_color.into(),
                    items,
                    selected,
                    enabled: true,
                    hint: None,
                });
            }
            result.extend([
                section(strings.reset_section),
                action(
                    strings.quick_looks,
                    info(strings.quick_looks_hint),
                    vec![
                        OptionItem::command(strings.translucent_dark, IDM_APPEARANCE_RECOMMENDED),
                        OptionItem::command(strings.restore_defaults, IDM_APPEARANCE_RESET),
                    ],
                ),
            ]);
            result
        }
        Page::Alerts => {
            let alert_10 = strings
                .alert_remaining
                .replace("{percent}", &10.to_string());
            let alert_20 = strings
                .alert_remaining
                .replace("{percent}", &20.to_string());
            let alert_30 = strings
                .alert_remaining
                .replace("{percent}", &30.to_string());
            vec![
                choice(
                    strings.low_quota_alert,
                    &[
                        (IDM_ALERT_OFF, strings.alert_off),
                        (IDM_ALERT_10, &alert_10),
                        (IDM_ALERT_20, &alert_20),
                        (IDM_ALERT_30, &alert_30),
                    ],
                    match s.alert_threshold_percent {
                        10 => IDM_ALERT_10,
                        20 => IDM_ALERT_20,
                        30 => IDM_ALERT_30,
                        _ => IDM_ALERT_OFF,
                    },
                )
                .hint(info(strings.low_quota_alert_hint)),
                check(strings.notify_on_reset, IDM_ALERT_RESET, s.notify_on_reset)
                    .hint(info(strings.notify_on_reset_hint)),
            ]
        }
        Page::Privacy => {
            let policy = crate::privacy::policy();
            let updates = crate::privacy::updates_enabled();
            let port = mcp_port(s.mcp_port);
            let mcp_hint = mcp_settings_hint(s.language, s.mcp_unavailable, port);
            let mut result = vec![
                section(strings.privacy_section),
                limited_check(
                    strings.passive_monitoring,
                    IDM_PASSIVE_MONITORING,
                    crate::privacy::passive_monitoring(),
                    !policy.force_passive_monitoring,
                )
                .hint(info(strings.passive_monitoring_hint)),
                check(strings.local_mcp_server, IDM_MCP_SERVER, s.mcp_enabled).hint(Some(
                    if s.mcp_unavailable {
                        Hint::Warning(mcp_hint)
                    } else {
                        Hint::Info(mcp_hint)
                    },
                )),
            ];
            if policy.force_passive_monitoring || policy.disable_updates {
                result.push(Row::Note(Hint::Info(strings.managed_by_policy.into())));
            }
            result.push(section(strings.updates));
            result.push(limited_check(
                strings.allow_updates,
                IDM_UPDATES_ENABLED,
                updates,
                !policy.disable_updates,
            ));
            result.push(version_row(s, updates));
            result
        }
    }
}

/// The installed version, the update status, and the next update action.
fn version_row(s: &AppState, updates: bool) -> Row {
    let strings = s.language.strings();
    let label = format!("Codex Usage v{}", env!("CARGO_PKG_VERSION"));
    let check = || {
        vec![OptionItem::command(
            strings.check_for_updates,
            IDM_VERSION_ACTION,
        )]
    };
    if !updates {
        return action(label, info(strings.update_checks_off), Vec::new());
    }
    let (hint, buttons) = match &s.update_status {
        UpdateStatus::Idle => (None, check()),
        UpdateStatus::Checking => (info(strings.checking_for_updates), Vec::new()),
        UpdateStatus::Applying => (info(strings.applying_update), Vec::new()),
        UpdateStatus::UpToDate => (info(strings.up_to_date_short), check()),
        UpdateStatus::Available(release) => {
            let button = match s.install_channel {
                InstallChannel::Portable => {
                    format!("{} v{}", strings.update_to, release.latest_version)
                }
                InstallChannel::Winget => format!(
                    "{} v{}",
                    localization::update_via_winget(s.language),
                    release.latest_version
                ),
            };
            let available = strings
                .version_available
                .replace("{version}", &release.latest_version.to_string());
            (
                Some(Hint::Warning(available)),
                vec![OptionItem::command(button, IDM_VERSION_ACTION)],
            )
        }
    };
    action(label, hint, buttons)
}

// ---------------------------------------------------------------------------
// Theme

/// Colors as 0xRRGGBB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Theme {
    dark: bool,
    background: u32,
    card: u32,
    card_hover: u32,
    card_pressed: u32,
    card_border: u32,
    text: u32,
    secondary: u32,
    disabled: u32,
    accent: u32,
    on_accent: u32,
    control: u32,
    control_hover: u32,
    control_pressed: u32,
    control_border: u32,
    strong_stroke: u32,
    knob: u32,
    nav_selected: u32,
    nav_hover: u32,
    caution: u32,
    critical: u32,
    critical_background: u32,
}

impl Theme {
    fn current(preference: SettingsTheme) -> Self {
        let palette = accent_palette();
        let dark = match effective_preference(preference) {
            SettingsTheme::Light => false,
            SettingsTheme::Dark => true,
            SettingsTheme::System => apps_use_dark_theme(),
        };
        if dark {
            let accent = palette.map_or(0x4C_C2FF, |palette| palette[1]);
            Self {
                dark: true,
                background: 0x20_2020,
                card: 0x2B_2B2B,
                card_hover: 0x32_3232,
                card_pressed: 0x27_2727,
                card_border: 0x1D_1D1D,
                text: 0xFF_FFFF,
                secondary: 0xC5_C5C5,
                disabled: 0x78_7878,
                accent,
                on_accent: readable_on(accent),
                control: 0x2D_2D2D,
                control_hover: 0x32_3232,
                control_pressed: 0x27_2727,
                control_border: 0x3A_3A3A,
                strong_stroke: 0x9A_9A9A,
                knob: 0xCF_CFCF,
                nav_selected: 0x2D_2D2D,
                nav_hover: 0x29_2929,
                caution: 0xFC_E100,
                critical: 0xFF_99A4,
                critical_background: 0x44_2726,
            }
        } else {
            let accent = palette.map_or(0x00_5FB8, |palette| palette[4]);
            Self {
                dark: false,
                background: 0xF3_F3F3,
                card: 0xFB_FBFB,
                card_hover: 0xF6_F6F6,
                card_pressed: 0xF3_F3F3,
                card_border: 0xE5_E5E5,
                text: 0x1A_1A1A,
                secondary: 0x5D_5D5D,
                disabled: 0xA0_A0A0,
                accent,
                on_accent: readable_on(accent),
                control: 0xFD_FDFD,
                control_hover: 0xF7_F7F7,
                control_pressed: 0xF3_F3F3,
                control_border: 0xD9_D9D9,
                strong_stroke: 0x86_8686,
                knob: 0x5F_5F5F,
                nav_selected: 0xE9_E9E9,
                nav_hover: 0xED_EDED,
                caution: 0x9D_5D00,
                critical: 0xC4_2B1C,
                critical_background: 0xFD_E7E9,
            }
        }
    }
}

fn readable_on(color: u32) -> u32 {
    let channel = |shift: u32| ((color >> shift) & 0xFF) as f32 / 255.0;
    let luminance = 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0);
    if luminance > 0.55 {
        0x00_0000
    } else {
        0xFF_FFFF
    }
}

fn colorref(color: u32) -> COLORREF {
    COLORREF((color & 0xFF) << 16 | (color & 0xFF00) | (color >> 16) & 0xFF)
}

fn effective_preference(preference: SettingsTheme) -> SettingsTheme {
    if cfg!(debug_assertions) && preference == SettingsTheme::System {
        match std::env::var("CODEX_USAGE_SETTINGS_THEME").as_deref() {
            Ok(theme) if theme.eq_ignore_ascii_case("dark") => return SettingsTheme::Dark,
            Ok(theme) if theme.eq_ignore_ascii_case("light") => return SettingsTheme::Light,
            _ => {}
        }
    }
    preference
}

fn apps_use_dark_theme() -> bool {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && value == 0
}

/// The eight accent shades Windows derives from the accent color, light to dark.
fn accent_palette() -> Option<[u32; 8]> {
    let mut data = [0u8; 32];
    let mut size = data.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS && size == 32).then(|| {
        std::array::from_fn(|index| {
            let shade = &data[index * 4..index * 4 + 3];
            u32::from(shade[0]) << 16 | u32::from(shade[1]) << 8 | u32::from(shade[2])
        })
    })
}

fn windows_build() -> u32 {
    let mut data = [0u16; 16];
    let mut size = std::mem::size_of_val(&data) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return 0;
    }
    let end = data
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(data.len());
    String::from_utf16_lossy(&data[..end]).parse().unwrap_or(0)
}

// Ordinal 135 changed signature before Windows 10 1903.
fn uxtheme_export(ordinal: usize) -> windows::Win32::Foundation::FARPROC {
    static UXTHEME: OnceLock<usize> = OnceLock::new();
    let module = *UXTHEME.get_or_init(|| unsafe {
        if windows_build() < 18362 {
            return 0;
        }
        LoadLibraryW(w!("uxtheme.dll")).map_or(0, |module| module.0 as usize)
    });
    if module == 0 {
        return None;
    }
    unsafe { GetProcAddress(HMODULE(module as *mut _), PCSTR(ordinal as *const u8)) }
}

pub(super) fn apply_menu_theme(preference: SettingsTheme) {
    let mode = match effective_preference(preference) {
        SettingsTheme::System => 1, // AllowDark
        SettingsTheme::Dark => 2,   // ForceDark
        SettingsTheme::Light => 3,  // ForceLight
    };
    unsafe {
        if let Some(function) = uxtheme_export(135) {
            let set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32 =
                std::mem::transmute(function);
            set_preferred_app_mode(mode);
        }
        if let Some(function) = uxtheme_export(136) {
            let flush_menu_themes: unsafe extern "system" fn() = std::mem::transmute(function);
            flush_menu_themes();
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing

#[derive(Clone, Copy, Default)]
struct Fonts {
    body: HFONT,
    strong: HFONT,
    caption: HFONT,
    title: HFONT,
    icons: HFONT,
    small_icons: HFONT,
    has_icons: bool,
}

impl Fonts {
    unsafe fn create(dpi: u32) -> Self {
        let icon_face = [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")]
            .into_iter()
            .find(|face| font_installed(*face));
        let icons = |size| {
            icon_face.map_or(HFONT::default(), |face| {
                make_font(face, scale(size, dpi), FW_NORMAL.0)
            })
        };
        Self {
            body: make_font(w!("Segoe UI"), scale(14, dpi), FW_NORMAL.0),
            strong: make_font(w!("Segoe UI"), scale(14, dpi), FW_SEMIBOLD.0),
            caption: make_font(w!("Segoe UI"), scale(12, dpi), FW_NORMAL.0),
            title: make_font(w!("Segoe UI"), scale(28, dpi), FW_SEMIBOLD.0),
            icons: icons(16),
            small_icons: icons(12),
            has_icons: icon_face.is_some(),
        }
    }

    unsafe fn delete(self) {
        for font in [
            self.body,
            self.strong,
            self.caption,
            self.title,
            self.icons,
            self.small_icons,
        ] {
            if !font.is_invalid() {
                let _ = DeleteObject(font);
            }
        }
    }
}

unsafe fn make_font(face: PCWSTR, size: i32, weight: u32) -> HFONT {
    CreateFontW(
        -size,
        0,
        0,
        0,
        weight as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        face,
    )
}

unsafe fn font_installed(face: PCWSTR) -> bool {
    let font = make_font(face, 16, FW_NORMAL.0);
    let dc = GetDC(HWND::default());
    let previous = SelectObject(dc, font);
    let mut name = [0u16; 64];
    let length = GetTextFaceW(dc, Some(&mut name)).max(0) as usize;
    SelectObject(dc, previous);
    ReleaseDC(HWND::default(), dc);
    let _ = DeleteObject(font);
    let actual = String::from_utf16_lossy(&name[..length.min(name.len())]);
    let actual = actual.trim_end_matches('\0');
    actual.eq_ignore_ascii_case(&face.to_string().unwrap_or_default())
}

fn gdiplus_ready() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| unsafe {
        let mut token = 0usize;
        let input = gp::GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        gp::GdiplusStartup(&mut token, &input, std::ptr::null_mut()) == gp::Ok
    })
}

/// Anti-aliased shapes. Text is drawn with GDI after the canvas is dropped.
struct Canvas(*mut gp::GpGraphics);

impl Canvas {
    unsafe fn new(dc: HDC) -> Option<Self> {
        if !gdiplus_ready() {
            return None;
        }
        let mut graphics = std::ptr::null_mut();
        if gp::GdipCreateFromHDC(dc, &mut graphics) != gp::Ok {
            return None;
        }
        gp::GdipSetSmoothingMode(graphics, gp::SmoothingModeAntiAlias);
        gp::GdipSetPixelOffsetMode(graphics, gp::PixelOffsetModeHalf);
        Some(Self(graphics))
    }

    unsafe fn rounded(x: f32, y: f32, width: f32, height: f32, radius: f32) -> *mut gp::GpPath {
        let mut path = std::ptr::null_mut();
        gp::GdipCreatePath(gp::FillModeAlternate, &mut path);
        let diameter = (radius * 2.0).min(width).min(height).max(0.1);
        gp::GdipAddPathArc(path, x, y, diameter, diameter, 180.0, 90.0);
        gp::GdipAddPathArc(
            path,
            x + width - diameter,
            y,
            diameter,
            diameter,
            270.0,
            90.0,
        );
        gp::GdipAddPathArc(
            path,
            x + width - diameter,
            y + height - diameter,
            diameter,
            diameter,
            0.0,
            90.0,
        );
        gp::GdipAddPathArc(
            path,
            x,
            y + height - diameter,
            diameter,
            diameter,
            90.0,
            90.0,
        );
        gp::GdipClosePathFigure(path);
        path
    }

    unsafe fn fill(&self, rect: RECT, radius: f32, color: u32) {
        let mut brush = std::ptr::null_mut();
        gp::GdipCreateSolidFill(0xFF00_0000 | color, &mut brush);
        let path = Self::rounded(
            rect.left as f32,
            rect.top as f32,
            (rect.right - rect.left) as f32,
            (rect.bottom - rect.top) as f32,
            radius,
        );
        gp::GdipFillPath(self.0, brush.cast(), path);
        gp::GdipDeletePath(path);
        gp::GdipDeleteBrush(brush.cast());
    }

    unsafe fn stroke(&self, rect: RECT, radius: f32, color: u32, width: f32) {
        let mut pen = std::ptr::null_mut();
        gp::GdipCreatePen1(0xFF00_0000 | color, width, gp::UnitPixel, &mut pen);
        let inset = width / 2.0;
        let path = Self::rounded(
            rect.left as f32 + inset,
            rect.top as f32 + inset,
            (rect.right - rect.left) as f32 - width,
            (rect.bottom - rect.top) as f32 - width,
            radius,
        );
        gp::GdipDrawPath(self.0, pen, path);
        gp::GdipDeletePath(path);
        gp::GdipDeletePen(pen);
    }

    unsafe fn circle(&self, center_x: f32, center_y: f32, diameter: f32, color: u32) {
        let mut brush = std::ptr::null_mut();
        gp::GdipCreateSolidFill(0xFF00_0000 | color, &mut brush);
        gp::GdipFillEllipse(
            self.0,
            brush.cast(),
            center_x - diameter / 2.0,
            center_y - diameter / 2.0,
            diameter,
            diameter,
        );
        gp::GdipDeleteBrush(brush.cast());
    }

    /// A filled rounded rectangle with a one-pixel border.
    unsafe fn card(&self, rect: RECT, radius: f32, fill: u32, border: u32) {
        self.fill(rect, radius, border);
        self.fill(inset(rect, 1, 1), (radius - 1.0).max(0.0), fill);
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            gp::GdipDeleteGraphics(self.0);
        }
    }
}

fn inset(rect: RECT, x: i32, y: i32) -> RECT {
    RECT {
        left: rect.left + x,
        top: rect.top + y,
        right: rect.right - x,
        bottom: rect.bottom - y,
    }
}

fn offset(rect: RECT, y: i32) -> RECT {
    RECT {
        top: rect.top + y,
        bottom: rect.bottom + y,
        ..rect
    }
}

/// Paints off screen, then copies the result, so controls never flicker.
unsafe fn buffered(target: HDC, area: RECT, paint: impl FnOnce(HDC, RECT)) {
    let width = area.right - area.left;
    let height = area.bottom - area.top;
    if width <= 0 || height <= 0 {
        return;
    }
    let dc = CreateCompatibleDC(target);
    let bitmap = CreateCompatibleBitmap(target, width, height);
    let previous = SelectObject(dc, bitmap);
    paint(
        dc,
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
    );
    let _ = BitBlt(
        target, area.left, area.top, width, height, dc, 0, 0, SRCCOPY,
    );
    SelectObject(dc, previous);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(dc);
}

unsafe fn fill_rect(dc: HDC, rect: RECT, color: u32) {
    let brush = CreateSolidBrush(colorref(color));
    FillRect(dc, &rect, brush);
    let _ = DeleteObject(brush);
}

unsafe fn draw_text(
    dc: HDC,
    font: HFONT,
    color: u32,
    text: &str,
    rect: RECT,
    flags: DRAW_TEXT_FORMAT,
) {
    if text.is_empty() || font.is_invalid() {
        return;
    }
    let previous = SelectObject(dc, font);
    SetTextColor(dc, colorref(color));
    SetBkMode(dc, TRANSPARENT);
    let mut text: Vec<u16> = text.encode_utf16().collect();
    let mut rect = rect;
    DrawTextW(dc, &mut text, &mut rect, flags | DT_NOPREFIX);
    SelectObject(dc, previous);
}

/// Height of `text` wrapped to `width`.
unsafe fn text_height(dc: HDC, font: HFONT, text: &str, width: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let previous = SelectObject(dc, font);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT {
        right: width.max(1),
        ..Default::default()
    };
    DrawTextW(
        dc,
        &mut wide,
        &mut rect,
        DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
    );
    SelectObject(dc, previous);
    rect.bottom
}

unsafe fn text_width(dc: HDC, font: HFONT, text: &str) -> i32 {
    let previous = SelectObject(dc, font);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT::default();
    DrawTextW(
        dc,
        &mut wide,
        &mut rect,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(dc, previous);
    rect.right
}

#[derive(Clone, Copy, Default)]
struct ControlState {
    hot: bool,
    pressed: bool,
    focused: bool,
    disabled: bool,
}

// ---------------------------------------------------------------------------
// Panel

#[derive(Clone, Copy, Default)]
struct Frame {
    /// Row bounds in content coordinates (before scrolling).
    rect: RECT,
    /// Right edge of the label and hint text.
    text_right: i32,
}

#[derive(Clone, Copy)]
struct SwitchAnimation {
    from: f32,
    to: f32,
    start: Instant,
}

impl SwitchAnimation {
    fn value(self, now: Instant) -> f32 {
        self.from
            + (self.to - self.from) * ease_out(now.duration_since(self.start), SWITCH_DURATION)
    }
}

fn ease_out(elapsed: Duration, duration: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.0);
    1.0 - (1.0 - t).powi(3)
}

fn mix_color(from: u32, to: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let from = ((from >> shift) & 255) as f32;
        let to = ((to >> shift) & 255) as f32;
        ((from + (to - from) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

struct SuspendRedraw(HWND);

impl SuspendRedraw {
    unsafe fn new(hwnd: HWND) -> Self {
        if IsWindowVisible(hwnd).as_bool() {
            let _ = SendMessageW(hwnd, WM_SETREDRAW, WPARAM(0), LPARAM(0));
            Self(hwnd)
        } else {
            // WM_SETREDRAW(TRUE) would set WS_VISIBLE on an initially hidden window.
            Self(HWND::default())
        }
    }
}

impl Drop for SuspendRedraw {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = SendMessageW(self.0, WM_SETREDRAW, WPARAM(1), LPARAM(0));
            }
        }
    }
}

struct Panel {
    hwnd: HWND,
    widget: HWND,
    nav: HWND,
    theme: Theme,
    preference: SettingsTheme,
    switches: Vec<Option<SwitchAnimation>>,
    page_animation: Option<Instant>,
    page_offset: i32,
    fonts: Fonts,
    icon: HICON,
    dpi: u32,
    language: Option<LanguageId>,
    save_failed: bool,
    page: usize,
    rows: Vec<Row>,
    controls: Vec<Vec<HWND>>,
    frames: Vec<Frame>,
    slots: Vec<Vec<RECT>>,
    banner: Option<RECT>,
    content_left: i32,
    content_right: i32,
    content_height: i32,
    scroll: i32,
    pending: VecDeque<Option<Action>>,
    menu_open: bool,
}

impl Panel {
    unsafe fn new(hwnd: HWND, widget: HWND) -> Self {
        let preference = lock_state()
            .as_ref()
            .map_or(SettingsTheme::System, |s| s.settings_theme);
        let theme = Theme::current(preference);
        BACKGROUND.with(|color| color.set(theme.background));
        let nav = child(
            hwnd,
            w!("LISTBOX"),
            NAV,
            WS_TABSTOP.0
                | LBS_HASSTRINGS as u32
                | LBS_OWNERDRAWFIXED as u32
                | LBS_NOTIFY as u32
                | LBS_NOINTEGRALHEIGHT as u32,
        );
        subclass(nav);
        let mut panel = Self {
            hwnd,
            widget,
            nav,
            theme,
            preference,
            switches: Vec::new(),
            page_animation: None,
            page_offset: 0,
            fonts: Fonts::default(),
            icon: HICON::default(),
            dpi: 0,
            language: None,
            save_failed: false,
            page: 0,
            rows: Vec::new(),
            controls: Vec::new(),
            frames: Vec::new(),
            slots: Vec::new(),
            banner: None,
            content_left: 0,
            content_right: 0,
            content_height: 0,
            scroll: 0,
            pending: VecDeque::new(),
            menu_open: false,
        };
        panel.set_dpi(GetDpiForWindow(hwnd).max(96));
        panel.apply_theme(panel.theme);
        panel
    }

    fn px(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    fn pxf(&self, value: f32) -> f32 {
        value * self.dpi as f32 / 96.0
    }

    unsafe fn set_dpi(&mut self, dpi: u32) {
        let old = std::mem::replace(&mut self.fonts, Fonts::create(dpi));
        self.dpi = dpi;
        set_font(self.nav, self.fonts.body);
        let _ = SendMessageW(
            self.nav,
            LB_SETITEMHEIGHT,
            WPARAM(0),
            LPARAM(self.px(NAV_ITEM) as isize),
        );
        for controls in &self.controls {
            for &control in controls {
                set_font(control, self.fonts.body);
            }
        }
        old.delete();
        let size = self.px(24);
        let icon = GetModuleHandleW(None)
            .ok()
            .and_then(|module| {
                LoadImageW(
                    HINSTANCE(module.0),
                    PCWSTR(1usize as *const u16),
                    IMAGE_ICON,
                    size,
                    size,
                    LR_DEFAULTCOLOR,
                )
                .ok()
            })
            .map_or(HICON::default(), |handle| HICON(handle.0));
        let old_icon = std::mem::replace(&mut self.icon, icon);
        if !old_icon.is_invalid() {
            let _ = DestroyIcon(old_icon);
        }
    }

    unsafe fn apply_theme(&mut self, theme: Theme) {
        self.theme = theme;
        BACKGROUND.with(|color| color.set(theme.background));
        let dark = BOOL::from(self.theme.dark);
        let _ = DwmSetWindowAttribute(
            self.hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
        // Windows 11 only: blend the title bar into the page background.
        let caption = colorref(self.theme.background);
        let _ = DwmSetWindowAttribute(
            self.hwnd,
            DWMWA_CAPTION_COLOR,
            &caption as *const COLORREF as *const _,
            std::mem::size_of::<COLORREF>() as u32,
        );
        if let Some(function) = uxtheme_export(133) {
            let allow_dark_mode: unsafe extern "system" fn(HWND, BOOL) -> BOOL =
                std::mem::transmute(function);
            let _ = allow_dark_mode(self.hwnd, dark);
        }
        let scroll_theme = if self.theme.dark {
            w!("DarkMode_Explorer")
        } else {
            w!("Explorer")
        };
        let _ = SetWindowTheme(self.hwnd, scroll_theme, PCWSTR::null());
        request_redraw();
    }

    unsafe fn sync(&mut self, force: bool) {
        if self.menu_open {
            return;
        }
        let Some((language, preference, next)) = lock_state()
            .as_ref()
            .map(|s| (s.language, s.settings_theme, rows(Page::ALL[self.page], s)))
        else {
            return;
        };

        let save_failed = SAVE_FAILED.load(Ordering::Relaxed);
        if !force
            && self.language == Some(language)
            && self.preference == preference
            && self.save_failed == save_failed
            && self.rows == next
        {
            return;
        }

        let focus = GetFocus();
        let redraw = SuspendRedraw::new(self.hwnd);
        if self.preference != preference {
            self.preference = preference;
            self.apply_theme(Theme::current(preference));
        }
        if self.language != Some(language) {
            self.language = Some(language);
            set_text(
                self.hwnd,
                &format!("Codex Usage — {}", language.strings().settings),
            );
            let _ = SendMessageW(self.nav, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
            for title in page_titles(language) {
                let title = native_interop::wide_str(title);
                let _ = SendMessageW(
                    self.nav,
                    LB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(title.as_ptr() as isize),
                );
            }
            let _ = SendMessageW(self.nav, LB_SETCURSEL, WPARAM(self.page), LPARAM(0));
        }
        self.save_failed = save_failed;
        let mut lost_focus = None;
        let mut controls_changed = self.controls.len() != next.len();
        let old_controls = std::mem::take(&mut self.controls);
        let mut old_controls = old_controls.into_iter();
        let now = Instant::now();
        self.switches.resize(next.len(), None);
        for (index, row) in next.iter().enumerate() {
            let old = self.rows.get(index);
            let (controls, reused) = match old_controls.next() {
                Some(controls) if old.is_some_and(|old| same_controls(old, row)) => {
                    (controls, true)
                }
                removed => {
                    if let Some(controls) = removed {
                        if controls.contains(&focus) {
                            lost_focus = Some(index);
                        }
                        for control in controls {
                            let _ = DestroyWindow(control);
                        }
                    }
                    (self.create_row(index, row), false)
                }
            };
            match (old, row) {
                (
                    Some(Row::Check {
                        item: old_item,
                        checked: old,
                        ..
                    }),
                    Row::Check { item, checked, .. },
                ) if reused && old_item.action == item.action => {
                    if old != checked {
                        let from = self.switches[index]
                            .map_or(f32::from(u8::from(*old)), |animation| animation.value(now));
                        self.switches[index] = Some(SwitchAnimation {
                            from,
                            to: f32::from(u8::from(*checked)),
                            start: now,
                        });
                    }
                }
                _ => self.switches[index] = None,
            }
            controls_changed |= !reused;
            if force || !reused || old != Some(row) {
                update_row(row, &controls);
            }
            self.controls.push(controls);
        }
        for controls in old_controls {
            if controls.contains(&focus) {
                lost_focus = Some(next.len());
            }
            for control in controls {
                let _ = DestroyWindow(control);
            }
        }
        self.rows = next;
        if controls_changed {
            // Dialog navigation uses HWND z-order, including controls retained across pages.
            let mut previous = self.nav;
            for &control in self.controls.iter().flatten() {
                if SetWindowPos(
                    control,
                    previous,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_DEFERERASE,
                )
                .is_ok()
                {
                    previous = control;
                }
            }
        }
        self.layout();
        drop(redraw);
        if let Some(index) = lost_focus {
            let replacement = self
                .controls
                .get(index)
                .and_then(|controls| controls.first())
                .copied();
            let _ = SetFocus(replacement.unwrap_or(self.nav));
        }
        if self.switches.iter().any(Option::is_some) {
            SetTimer(self.hwnd, ANIMATION_TIMER, 15, None);
        }
    }

    unsafe fn create_row(&self, index: usize, row: &Row) -> Vec<HWND> {
        let base = FIRST_FIELD + index as u16 * FIELDS_PER_ROW;
        let style = if matches!(row, Row::Check { .. }) {
            BS_AUTOCHECKBOX
        } else {
            BS_PUSHBUTTON
        } as u32;
        (0..row.control_count())
            .map(|column| {
                let control = child(
                    self.hwnd,
                    w!("BUTTON"),
                    base + column as u16,
                    WS_TABSTOP.0 | style,
                );
                set_font(control, self.fonts.body);
                subclass(control);
                control
            })
            .collect()
    }

    unsafe fn client(&self) -> RECT {
        let mut rect = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rect);
        rect
    }

    /// Height of a card that shows `label` and `hint` in `text_width` pixels.
    unsafe fn card_height(
        &self,
        dc: HDC,
        label: &str,
        hint: Option<&Hint>,
        text_width: i32,
    ) -> i32 {
        let label_height = text_height(dc, self.fonts.body, label, text_width);
        let hint_height = hint.map_or(0, |hint| {
            self.px(2) + text_height(dc, self.fonts.caption, hint.text(), text_width)
        });
        (label_height + hint_height + 2 * self.px(CARD_VPAD)).max(self.px(CARD_MIN))
    }

    unsafe fn button_width(&self, dc: HDC, item: &OptionItem) -> i32 {
        let swatch = if item.swatch.is_some() {
            self.px(22)
        } else {
            0
        };
        (text_width(dc, self.fonts.body, &item.label) + self.px(32) + swatch).max(self.px(88))
    }

    /// Measure every row, then position the controls.
    unsafe fn layout(&mut self) {
        // Showing or hiding the scroll bar resizes the client area while the
        // panel is borrowed, so the nested WM_SIZE cannot lay out again.
        for _ in 0..2 {
            let width = self.client().right;
            self.measure();
            if self.client().right == width {
                break;
            }
        }
    }

    unsafe fn measure(&mut self) {
        let client = self.client();
        let _ = SetWindowPos(
            self.nav,
            HWND::default(),
            self.px(8),
            self.px(NAV_TOP),
            self.px(NAV_WIDTH - 16),
            (client.bottom - self.px(NAV_TOP) - self.px(8)).max(0),
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOREDRAW,
        );

        let dc = GetDC(self.hwnd);
        let left = self.px(NAV_WIDTH + CONTENT_LEFT);
        let right = (client.right - self.px(CONTENT_RIGHT))
            .min(left + self.px(CONTENT_MAX))
            .max(left + self.px(240));
        let width = right - left;
        let pad = self.px(CARD_PAD);
        let mut y = self.px(TOP + TITLE_HEIGHT) + self.px(12);
        self.banner = self.save_failed.then(|| {
            let rect = RECT {
                left,
                top: y,
                right,
                bottom: y + self.px(52),
            };
            y = rect.bottom + self.px(12);
            rect
        });
        let mut frames = Vec::with_capacity(self.rows.len());
        let mut slots = Vec::with_capacity(self.rows.len());
        for (index, row) in self.rows.iter().enumerate() {
            let (frame, row_slots) = match row {
                Row::Section(_) => {
                    if index > 0 {
                        y += self.px(20);
                    }
                    let rect = RECT {
                        left,
                        top: y,
                        right,
                        bottom: y + self.px(32),
                    };
                    y = rect.bottom;
                    (
                        Frame {
                            rect,
                            text_right: right,
                        },
                        Vec::new(),
                    )
                }
                Row::Preview { .. } => {
                    let rect = RECT {
                        left,
                        top: y,
                        right,
                        bottom: y + self.px(WIDGET_HEIGHT + 54),
                    };
                    y = rect.bottom + self.px(CARD_GAP);
                    (
                        Frame {
                            rect,
                            text_right: right,
                        },
                        Vec::new(),
                    )
                }
                Row::Note(hint) => {
                    y += self.px(4);
                    let height = text_height(dc, self.fonts.caption, hint.text(), width);
                    let rect = RECT {
                        left,
                        top: y,
                        right,
                        bottom: y + height,
                    };
                    y = rect.bottom + self.px(CARD_GAP);
                    (
                        Frame {
                            rect,
                            text_right: right,
                        },
                        Vec::new(),
                    )
                }
                Row::Check { item, hint, .. } => {
                    let text_right = right - pad - self.px(TOGGLE_WIDTH + CONTROL_GAP);
                    let height =
                        self.card_height(dc, &item.label, hint.as_ref(), text_right - left - pad);
                    let rect = RECT {
                        left,
                        top: y,
                        right,
                        bottom: y + height,
                    };
                    y = rect.bottom + self.px(CARD_GAP);
                    (Frame { rect, text_right }, vec![rect])
                }
                Row::Choice { label, hint, .. } => {
                    let control = self.px(DROPDOWN_WIDTH).min(width * 45 / 100);
                    let text_right = right - pad - control - self.px(CONTROL_GAP);
                    let height =
                        self.card_height(dc, label, hint.as_ref(), text_right - left - pad);
                    let rect = RECT {
                        left,
                        top: y,
                        right,
                        bottom: y + height,
                    };
                    y = rect.bottom + self.px(CARD_GAP);
                    let top = rect.top + (height - self.px(BUTTON_HEIGHT)) / 2;
                    let slot = RECT {
                        left: right - pad - control,
                        top,
                        right: right - pad,
                        bottom: top + self.px(BUTTON_HEIGHT),
                    };
                    (Frame { rect, text_right }, vec![slot])
                }
                Row::Action {
                    label,
                    hint,
                    buttons,
                } => {
                    let widths: Vec<i32> = buttons
                        .iter()
                        .map(|button| self.button_width(dc, button))
                        .collect();
                    let total = widths.iter().sum::<i32>()
                        + self.px(BUTTON_GAP) * (widths.len() as i32 - 1).max(0);
                    let text_right = if buttons.is_empty() {
                        right - pad
                    } else {
                        right - pad - total - self.px(CONTROL_GAP)
                    };
                    let height =
                        self.card_height(dc, label, hint.as_ref(), text_right - left - pad);
                    let rect = RECT {
                        left,
                        top: y,
                        right,
                        bottom: y + height,
                    };
                    y = rect.bottom + self.px(CARD_GAP);
                    let top = rect.top + (height - self.px(BUTTON_HEIGHT)) / 2;
                    let mut x = right - pad - total;
                    let row_slots = widths
                        .iter()
                        .map(|width| {
                            let slot = RECT {
                                left: x,
                                top,
                                right: x + width,
                                bottom: top + self.px(BUTTON_HEIGHT),
                            };
                            x += width + self.px(BUTTON_GAP);
                            slot
                        })
                        .collect();
                    (Frame { rect, text_right }, row_slots)
                }
            };
            frames.push(frame);
            slots.push(row_slots);
        }
        ReleaseDC(self.hwnd, dc);
        self.frames = frames;
        self.slots = slots;
        self.content_left = left;
        self.content_right = right;
        self.content_height = y + self.px(28);
        self.place();
    }

    /// Position controls for the current scroll offset and repaint the page.
    unsafe fn place(&mut self) {
        let client = self.client();
        let height = client.bottom - client.top;
        self.scroll = self
            .scroll
            .min((self.content_height - height).max(0))
            .max(0);
        let count = self.controls.iter().map(Vec::len).sum::<usize>();
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOREDRAW | SWP_DEFERERASE;
        let positions = || {
            self.controls
                .iter()
                .zip(&self.slots)
                .flat_map(|(controls, slots)| controls.iter().copied().zip(slots.iter().copied()))
        };
        if count > 0 {
            let dy = self.page_offset - self.scroll;
            let result = BeginDeferWindowPos(count as i32)
                .and_then(|batch| {
                    positions().try_fold(batch, |batch, (control, slot)| {
                        DeferWindowPos(
                            batch,
                            control,
                            HWND::default(),
                            slot.left,
                            slot.top + dy,
                            slot.right - slot.left,
                            slot.bottom - slot.top,
                            flags,
                        )
                    })
                })
                .and_then(|batch| EndDeferWindowPos(batch));
            if result.is_err() {
                for (control, slot) in positions() {
                    let _ = SetWindowPos(
                        control,
                        HWND::default(),
                        slot.left,
                        slot.top + dy,
                        slot.right - slot.left,
                        slot.bottom - slot.top,
                        flags,
                    );
                }
            }
        }
        let scroll_info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: (self.content_height - 1).max(0),
            nPage: height.max(0) as u32,
            nPos: self.scroll,
            ..Default::default()
        };
        SetScrollInfo(self.hwnd, SB_VERT, &scroll_info, false);
        request_redraw();
    }

    unsafe fn scroll_to(&mut self, position: i32) {
        let height = self.client().bottom;
        let position = position.min((self.content_height - height).max(0)).max(0);
        if position != self.scroll {
            self.scroll = position;
            self.place();
        }
    }

    unsafe fn ensure_visible(&mut self, control: HWND) {
        let Some(row) = self
            .controls
            .iter()
            .position(|controls| controls.contains(&control))
        else {
            return;
        };
        let Some(frame) = self.frames.get(row) else {
            return;
        };
        let height = self.client().bottom;
        let margin = self.px(16);
        if frame.rect.top - margin < self.scroll {
            self.scroll_to(frame.rect.top - margin);
        } else if frame.rect.bottom + margin > self.scroll + height {
            self.scroll_to(frame.rect.bottom + margin - height);
        }
    }

    unsafe fn change_page(&mut self, page: usize) {
        if page < Page::ALL.len() && self.page != page {
            self.page = page;
            self.scroll = 0;
            self.switches.clear();
            self.page_animation = Some(Instant::now());
            self.page_offset = self.px(24);
            self.sync(true);
            SetTimer(self.hwnd, ANIMATION_TIMER, 15, None);
        }
    }

    unsafe fn animate(&mut self) {
        let now = Instant::now();
        let mut active = false;
        if let Some(start) = self.page_animation {
            let progress = ease_out(now.duration_since(start), PAGE_DURATION);
            self.page_offset = (self.px(24) as f32 * (1.0 - progress)).round() as i32;
            if progress == 1.0 {
                self.page_animation = None;
            } else {
                active = true;
            }
            self.place();
        }
        for (animation, controls) in self.switches.iter_mut().zip(&self.controls) {
            if let Some(switch) = animation {
                if now.duration_since(switch.start) >= SWITCH_DURATION {
                    *animation = None;
                } else {
                    active = true;
                }
                if let Some(control) = controls.first() {
                    let _ = InvalidateRect(*control, None, false);
                }
            }
        }
        if !active {
            let _ = KillTimer(self.hwnd, ANIMATION_TIMER);
        }
    }

    fn locate(&self, id: u16, control: HWND) -> Option<(usize, usize)> {
        let index = id.checked_sub(FIRST_FIELD)?;
        let row = usize::from(index / FIELDS_PER_ROW);
        let column = usize::from(index % FIELDS_PER_ROW);
        (self.controls.get(row)?.get(column) == Some(&control)).then_some((row, column))
    }

    /// Items and screen position for a drop-down button that was clicked.
    unsafe fn choice_request(
        &self,
        id: u16,
        control: HWND,
    ) -> Option<(Vec<OptionItem>, usize, RECT)> {
        let (row, _) = self.locate(id, control)?;
        let Row::Choice {
            items,
            selected,
            enabled: true,
            ..
        } = self.rows.get(row)?
        else {
            return None;
        };
        let mut anchor = RECT::default();
        GetWindowRect(control, &mut anchor).ok()?;
        Some((items.clone(), *selected, anchor))
    }

    unsafe fn queue_action(&mut self, id: u16, control: HWND) {
        let Some((row, column)) = self.locate(id, control) else {
            return;
        };
        let item = match self.rows.get(row) {
            Some(Row::Check {
                item,
                enabled: true,
                ..
            }) => item,
            Some(Row::Action { buttons, .. }) => match buttons.get(column) {
                Some(item) => item,
                None => return,
            },
            _ => return,
        };
        self.enqueue(item.action.clone());
    }

    /// Run after the control has completed its click, including opening modal pickers.
    unsafe fn enqueue(&mut self, action: Option<Action>) {
        if self.pending.back() != Some(&action) {
            self.pending.push_back(action);
            let _ = PostMessageW(self.hwnd, APPLY_SELECTION, WPARAM(0), LPARAM(0));
        }
    }

    // -- painting ---------------------------------------------------------

    unsafe fn paint(&self, target: HDC) {
        let client = self.client();
        buffered(target, client, |dc, client| {
            let theme = self.theme;
            fill_rect(dc, client, theme.background);
            let scroll = self.scroll - self.page_offset;

            // Shapes first; GDI text follows once the canvas is released.
            if let Some(canvas) = Canvas::new(dc) {
                let radius = self.pxf(CARD_RADIUS);
                if let Some(banner) = self.banner {
                    canvas.card(
                        offset(banner, -scroll),
                        radius,
                        theme.critical_background,
                        theme.card_border,
                    );
                }
                for (row, frame) in self.rows.iter().zip(&self.frames) {
                    if matches!(
                        row,
                        Row::Choice { .. } | Row::Action { .. } | Row::Preview { .. }
                    ) {
                        let rect = offset(frame.rect, -scroll);
                        if rect.bottom >= 0 && rect.top <= client.bottom {
                            canvas.card(rect, radius, theme.card, theme.card_border);
                        }
                    }
                }
            }

            // Application header above the navigation.
            let icon_size = self.px(24);
            let header_top = self.px(22);
            if !self.icon.is_invalid() {
                let _ = DrawIconEx(
                    dc,
                    self.px(20),
                    header_top,
                    self.icon,
                    icon_size,
                    icon_size,
                    0,
                    HBRUSH::default(),
                    DI_NORMAL,
                );
            }
            draw_text(
                dc,
                self.fonts.strong,
                theme.text,
                "Codex Usage",
                RECT {
                    left: self.px(56),
                    top: header_top,
                    right: self.px(NAV_WIDTH),
                    bottom: header_top + icon_size,
                },
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );

            let language = self.language.unwrap_or(LanguageId::English);
            let title = page_titles(language)[self.page];
            draw_text(
                dc,
                self.fonts.title,
                theme.text,
                title,
                RECT {
                    left: self.content_left,
                    top: self.px(TOP) - scroll,
                    right: self.content_right,
                    bottom: self.px(TOP + TITLE_HEIGHT) - scroll,
                },
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );

            if let Some(banner) = self.banner {
                let banner = offset(banner, -scroll);
                let message = if language == LanguageId::SimplifiedChinese {
                    "无法保存设置。请检查设置文件的写入权限。"
                } else {
                    "Could not save settings. Check write access to the settings file."
                };
                let mut text = inset(banner, self.px(CARD_PAD), 0);
                if self.fonts.has_icons {
                    draw_text(
                        dc,
                        self.fonts.icons,
                        theme.critical,
                        &GLYPH_ERROR.to_string(),
                        text,
                        DT_SINGLELINE | DT_VCENTER,
                    );
                    text.left += self.px(28);
                }
                draw_text(
                    dc,
                    self.fonts.body,
                    theme.text,
                    message,
                    text,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                );
            }

            for (row, frame) in self.rows.iter().zip(&self.frames) {
                let rect = offset(frame.rect, -scroll);
                if rect.bottom < 0 || rect.top > client.bottom {
                    continue;
                }
                match row {
                    Row::Section(title) => draw_text(
                        dc,
                        self.fonts.strong,
                        theme.text,
                        title,
                        RECT {
                            bottom: rect.bottom - self.px(8),
                            ..rect
                        },
                        DT_SINGLELINE | DT_BOTTOM | DT_END_ELLIPSIS,
                    ),
                    Row::Preview {
                        appearance,
                        system_dark,
                    } => {
                        draw_text(
                            dc,
                            self.fonts.strong,
                            theme.text,
                            language.strings().appearance_preview,
                            RECT {
                                left: rect.left + self.px(CARD_PAD),
                                right: rect.right - self.px(CARD_PAD),
                                top: rect.top + self.px(12),
                                bottom: rect.top + self.px(32),
                            },
                            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                        );
                        appearance_preview::draw(
                            dc,
                            RECT {
                                left: rect.left + self.px(CARD_PAD),
                                right: rect.right - self.px(CARD_PAD),
                                top: rect.top + self.px(34),
                                bottom: rect.bottom - self.px(12),
                            },
                            *appearance,
                            *system_dark,
                            language,
                        );
                    }
                    Row::Note(hint) => draw_text(
                        dc,
                        self.fonts.caption,
                        hint_color(&theme, hint, true),
                        hint.text(),
                        rect,
                        DT_WORDBREAK,
                    ),
                    Row::Choice {
                        label,
                        hint,
                        enabled,
                        ..
                    } => self.card_text(
                        dc,
                        RECT {
                            left: rect.left + self.px(CARD_PAD),
                            right: frame.text_right,
                            ..rect
                        },
                        label,
                        hint.as_ref(),
                        *enabled,
                    ),
                    Row::Action { label, hint, .. } => self.card_text(
                        dc,
                        RECT {
                            left: rect.left + self.px(CARD_PAD),
                            right: frame.text_right,
                            ..rect
                        },
                        label,
                        hint.as_ref(),
                        true,
                    ),
                    Row::Check { .. } => {}
                }
            }
        });
    }

    /// Label with an optional hint below it, centered vertically in `area`.
    unsafe fn card_text(
        &self,
        dc: HDC,
        area: RECT,
        label: &str,
        hint: Option<&Hint>,
        enabled: bool,
    ) {
        let width = area.right - area.left;
        let label_height = text_height(dc, self.fonts.body, label, width);
        let hint_height = hint.map_or(0, |hint| {
            text_height(dc, self.fonts.caption, hint.text(), width)
        });
        let gap = if hint.is_some() { self.px(2) } else { 0 };
        let mut top = area.top + (area.bottom - area.top - label_height - gap - hint_height) / 2;
        let theme = self.theme;
        draw_text(
            dc,
            self.fonts.body,
            if enabled { theme.text } else { theme.disabled },
            label,
            RECT {
                top,
                bottom: top + label_height,
                ..area
            },
            DT_WORDBREAK,
        );
        if let Some(hint) = hint {
            top += label_height + gap;
            draw_text(
                dc,
                self.fonts.caption,
                hint_color(&theme, hint, enabled),
                hint.text(),
                RECT {
                    top,
                    bottom: top + hint_height,
                    ..area
                },
                DT_WORDBREAK,
            );
        }
    }

    /// Paint using the last applied row model so native input cannot skip the animation.
    unsafe fn draw_control(&self, draw: &NMCUSTOMDRAW) {
        let control = draw.hdr.hwndFrom;
        let Some((row, column)) = self.locate(draw.hdr.idFrom as u16, control) else {
            return;
        };
        let index = row;
        let (Some(row), Some(frame)) = (self.rows.get(index), self.frames.get(index)) else {
            return;
        };
        let flags = draw.uItemState;
        let state = ControlState {
            hot: flags.0 & CDIS_HOT.0 != 0,
            pressed: flags.0 & CDIS_SELECTED.0 != 0,
            focused: flags.0 & CDIS_FOCUS.0 != 0 && flags.0 & CDIS_SHOWKEYBOARDCUES.0 != 0,
            disabled: flags.0 & CDIS_DISABLED.0 != 0 || !IsWindowEnabled(control).as_bool(),
        };
        let area = draw.rc;
        match row {
            Row::Check {
                item,
                checked,
                hint,
                ..
            } => {
                let progress = self.switches[index]
                    .map_or(f32::from(u8::from(*checked)), |animation| {
                        animation.value(Instant::now())
                    });
                let text_right = frame.text_right - frame.rect.left;
                buffered(draw.hdc, area, |dc, rect| {
                    self.draw_switch_card(
                        dc,
                        rect,
                        text_right,
                        &item.label,
                        hint.as_ref(),
                        progress,
                        state,
                    )
                });
            }
            Row::Choice {
                items, selected, ..
            } => {
                if let Some(item) = items.get(*selected) {
                    buffered(draw.hdc, area, |dc, rect| {
                        self.draw_button(dc, rect, &item.label, item.swatch, true, state)
                    });
                }
            }
            Row::Action { buttons, .. } => {
                let Some(item) = buttons.get(column) else {
                    return;
                };
                buffered(draw.hdc, area, |dc, rect| {
                    self.draw_button(dc, rect, &item.label, item.swatch, false, state)
                });
            }
            Row::Section(_) | Row::Note(_) | Row::Preview { .. } => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_switch_card(
        &self,
        dc: HDC,
        rect: RECT,
        text_right: i32,
        label: &str,
        hint: Option<&Hint>,
        progress: f32,
        state: ControlState,
    ) {
        let theme = self.theme;
        fill_rect(dc, rect, theme.background);
        let fill = if state.disabled {
            theme.card
        } else if state.pressed {
            theme.card_pressed
        } else if state.hot {
            theme.card_hover
        } else {
            theme.card
        };
        let toggle_width = self.px(TOGGLE_WIDTH);
        let toggle_height = self.px(TOGGLE_HEIGHT);
        let toggle = RECT {
            left: rect.right - self.px(CARD_PAD) - toggle_width,
            top: (rect.bottom - toggle_height) / 2,
            right: rect.right - self.px(CARD_PAD),
            bottom: (rect.bottom - toggle_height) / 2 + toggle_height,
        };
        if let Some(canvas) = Canvas::new(dc) {
            let radius = self.pxf(CARD_RADIUS);
            canvas.card(rect, radius, fill, theme.card_border);
            let pill = toggle_height as f32 / 2.0;
            let knob = self.pxf(if state.hot && !state.disabled {
                14.0
            } else {
                12.0
            });
            let center_y = toggle.top as f32 + pill;
            let (on_track, on_knob, off_stroke, off_knob) = if state.disabled {
                (theme.disabled, theme.card, theme.disabled, theme.disabled)
            } else {
                (
                    theme.accent,
                    theme.on_accent,
                    theme.strong_stroke,
                    theme.knob,
                )
            };
            let track = mix_color(fill, on_track, progress);
            let stroke = mix_color(off_stroke, on_track, progress);
            canvas.fill(toggle, pill, track);
            canvas.stroke(toggle, pill, stroke, self.pxf(1.0));
            let travel = (toggle.right - toggle.left - toggle_height) as f32;
            let center_x = toggle.left as f32 + pill + travel * progress;
            canvas.circle(
                center_x,
                center_y,
                knob,
                mix_color(off_knob, on_knob, progress),
            );
            if state.focused {
                canvas.stroke(rect, radius, theme.text, self.pxf(2.0));
            }
        }
        self.card_text(
            dc,
            RECT {
                left: rect.left + self.px(CARD_PAD),
                right: text_right,
                ..rect
            },
            label,
            hint,
            !state.disabled,
        );
    }

    unsafe fn draw_button(
        &self,
        dc: HDC,
        rect: RECT,
        label: &str,
        swatch: Option<u32>,
        dropdown: bool,
        state: ControlState,
    ) {
        let theme = self.theme;
        fill_rect(dc, rect, theme.card);
        let fill = if state.disabled {
            theme.control
        } else if state.pressed {
            theme.control_pressed
        } else if state.hot {
            theme.control_hover
        } else {
            theme.control
        };
        let radius = self.pxf(CONTROL_RADIUS);
        let padding = self.px(12);
        let swatch_size = self.px(14);
        let content_width = if dropdown {
            0
        } else {
            let dc_width = text_width(dc, self.fonts.body, label);
            dc_width + swatch.map_or(0, |_| swatch_size + self.px(8))
        };
        let mut text = inset(rect, padding, 0);
        if !dropdown {
            text.left = rect.left + (rect.right - rect.left - content_width) / 2;
        }
        if let Some(canvas) = Canvas::new(dc) {
            canvas.card(rect, radius, fill, theme.control_border);
            if let Some(color) = swatch {
                let center_y = (rect.top + rect.bottom) as f32 / 2.0;
                let diameter = swatch_size as f32;
                canvas.circle(
                    text.left as f32 + diameter / 2.0,
                    center_y,
                    diameter,
                    theme.control_border,
                );
                canvas.circle(
                    text.left as f32 + diameter / 2.0,
                    center_y,
                    diameter - self.pxf(2.0),
                    color,
                );
                text.left += swatch_size + self.px(8);
            }
            if state.focused {
                canvas.stroke(rect, radius, theme.text, self.pxf(2.0));
            }
        }
        let color = if state.disabled {
            theme.disabled
        } else if state.pressed {
            theme.secondary
        } else {
            theme.text
        };
        if dropdown {
            let chevron = self.px(12);
            if self.fonts.has_icons {
                draw_text(
                    dc,
                    self.fonts.small_icons,
                    if state.disabled {
                        theme.disabled
                    } else {
                        theme.secondary
                    },
                    &GLYPH_CHEVRON_DOWN.to_string(),
                    RECT {
                        left: text.right - chevron,
                        ..text
                    },
                    DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
                );
            }
            text.right -= chevron + self.px(8);
            draw_text(
                dc,
                self.fonts.body,
                color,
                label,
                text,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        } else {
            draw_text(
                dc,
                self.fonts.body,
                color,
                label,
                RECT {
                    right: rect.right - padding,
                    ..text
                },
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
    }

    unsafe fn draw_nav(&self, item: &DRAWITEMSTRUCT) {
        let Some(title) = page_titles(self.language.unwrap_or(LanguageId::English))
            .get(item.itemID as usize)
            .copied()
        else {
            return;
        };
        let index = item.itemID as usize;
        let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
        let focused =
            item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0;
        let hot = NAV_HOVER.with(Cell::get) == index as i32;
        let theme = self.theme;
        buffered(item.hDC, item.rcItem, |dc, rect| {
            fill_rect(dc, rect, theme.background);
            let pill = inset(rect, self.px(4), self.px(2));
            if let Some(canvas) = Canvas::new(dc) {
                let radius = self.pxf(CONTROL_RADIUS);
                if selected || hot {
                    canvas.fill(
                        pill,
                        radius,
                        if selected {
                            theme.nav_selected
                        } else {
                            theme.nav_hover
                        },
                    );
                }
                if selected {
                    let height = self.px(16);
                    let top = (pill.top + pill.bottom - height) / 2;
                    canvas.fill(
                        RECT {
                            left: pill.left,
                            top,
                            right: pill.left + self.px(3),
                            bottom: top + height,
                        },
                        self.pxf(1.5),
                        theme.accent,
                    );
                }
                if focused {
                    canvas.stroke(pill, radius, theme.text, self.pxf(2.0));
                }
            }
            let mut text = RECT {
                left: pill.left + self.px(14),
                right: pill.right - self.px(8),
                ..pill
            };
            if self.fonts.has_icons {
                if let Some(glyph) = PAGE_GLYPHS.get(index) {
                    draw_text(
                        dc,
                        self.fonts.icons,
                        theme.text,
                        &glyph.to_string(),
                        text,
                        DT_SINGLELINE | DT_VCENTER,
                    );
                }
                text.left += self.px(30);
            }
            draw_text(
                dc,
                self.fonts.body,
                theme.text,
                title,
                text,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        });
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            self.fonts.delete();
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

fn hint_color(theme: &Theme, hint: &Hint, enabled: bool) -> u32 {
    match (hint, enabled) {
        (Hint::Warning(_), _) => theme.caution,
        (Hint::Info(_), true) => theme.secondary,
        (Hint::Info(_), false) => theme.disabled,
    }
}

fn same_controls(old: &Row, new: &Row) -> bool {
    std::mem::discriminant(old) == std::mem::discriminant(new)
        && old.control_count() == new.control_count()
}

fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

unsafe fn child(owner: HWND, class: PCWSTR, id: u16, style: u32) -> HWND {
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR::null(),
        WS_CHILD | WINDOW_STYLE(style),
        0,
        0,
        0,
        0,
        owner,
        HMENU(id as usize as *mut _),
        GetModuleHandleW(None).unwrap_or_default(),
        None,
    )
    .unwrap_or_default()
}

unsafe fn subclass(control: HWND) {
    let _ = SetWindowSubclass(control, Some(control_proc), SUBCLASS_ID, 0);
}

unsafe fn set_text(hwnd: HWND, text: &str) {
    let text = native_interop::wide_str(text);
    let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
}

unsafe fn set_font(hwnd: HWND, font: HFONT) {
    let _ = SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
}

unsafe fn update_row(row: &Row, controls: &[HWND]) {
    match row {
        Row::Check {
            item,
            checked,
            enabled,
            hint,
        } => {
            set_text(controls[0], &accessible_text(&item.label, hint.as_ref()));
            let _ = SendMessageW(
                controls[0],
                BM_SETCHECK,
                WPARAM(usize::from(*checked)),
                LPARAM(0),
            );
            let _ = EnableWindow(controls[0], *enabled);
        }
        Row::Choice {
            label,
            items,
            selected,
            enabled,
            ..
        } => {
            let value = items.get(*selected).map_or("", |item| item.label.as_str());
            set_text(controls[0], &format!("{label}: {value}"));
            let _ = EnableWindow(controls[0], *enabled);
        }
        Row::Action { buttons, .. } => {
            for (item, &control) in buttons.iter().zip(controls) {
                set_text(control, &item.label);
                let _ = EnableWindow(control, item.action.is_some());
            }
        }
        Row::Section(_) | Row::Note(_) | Row::Preview { .. } => {}
    }
}

/// Show the items of a drop-down button as a menu below it.
unsafe fn pick_from_menu(
    owner: HWND,
    items: &[OptionItem],
    selected: usize,
    anchor: RECT,
    dpi: u32,
) -> Option<usize> {
    let menu = CreatePopupMenu().ok()?;
    for (index, item) in items.iter().enumerate() {
        let text = native_interop::wide_str(&item.label);
        let flags = if item.action.is_some() {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        let _ = AppendMenuW(menu, flags, index + 1, PCWSTR(text.as_ptr()));
    }
    let _ = CheckMenuRadioItem(
        menu,
        1,
        items.len() as u32,
        selected as u32 + 1,
        MF_BYCOMMAND.0,
    );
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_NONOTIFY,
        anchor.left,
        anchor.bottom + scale(4, dpi),
        0,
        owner,
        None,
    )
    .0;
    let _ = DestroyMenu(menu);
    (command > 0).then(|| command as usize - 1)
}

fn request_redraw() {
    REDRAW_PENDING.with(|pending| pending.set(true));
}

fn with_panel<T>(f: impl FnOnce(&mut Panel) -> T) -> Option<T> {
    let (hwnd, result) = PANEL.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        let panel = state.as_mut()?;
        Some((panel.hwnd, f(panel)))
    })?;
    if REDRAW_PENDING.with(|pending| pending.replace(false)) {
        unsafe {
            let _ = RedrawWindow(
                hwnd,
                None,
                HRGN::default(),
                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_FRAME | RDW_NOERASE | RDW_UPDATENOW,
            );
        }
    }
    Some(result)
}

/// Read-only access for painting. `Err` means the panel is busy and the
/// caller should repaint later.
fn with_panel_ref<T>(f: impl FnOnce(&Panel) -> T) -> Result<Option<T>, ()> {
    PANEL.with(|state| {
        state
            .try_borrow()
            .map(|state| state.as_ref().map(f))
            .map_err(|_| ())
    })
}

fn current_window() -> HWND {
    with_panel(|panel| panel.hwnd).unwrap_or_default()
}

pub(super) fn note_save_result(saved: bool) {
    SAVE_FAILED.store(!saved, Ordering::Relaxed);
}

pub(super) unsafe fn handle_dialog_message(message: &MSG) -> bool {
    let hwnd = current_window();
    !hwnd.is_invalid()
        && (message.hwnd == hwnd || IsChild(hwnd, message.hwnd).as_bool())
        && IsDialogMessageW(hwnd, message).as_bool()
}

pub(super) unsafe fn close() {
    let hwnd = current_window();
    if !hwnd.is_invalid() {
        let _ = DestroyWindow(hwnd);
    }
}

fn window_style() -> WINDOW_STYLE {
    WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VSCROLL
}

unsafe fn outer_size(width: i32, height: i32, dpi: u32) -> (i32, i32) {
    let mut rect = RECT {
        right: scale(width, dpi),
        bottom: scale(height, dpi),
        ..Default::default()
    };
    let _ = AdjustWindowRectExForDpi(&mut rect, window_style(), false, WINDOW_EX_STYLE(0), dpi);
    (rect.right - rect.left, rect.bottom - rect.top)
}

pub(super) unsafe fn show(widget: HWND) {
    let existing = current_window();
    if !existing.is_invalid() {
        let _ = ShowWindow(existing, SW_RESTORE);
        let _ = SetForegroundWindow(existing);
        return;
    }
    let Ok(module) = GetModuleHandleW(None) else {
        return;
    };
    let instance = HINSTANCE(module.0);
    let _ = RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(settings_proc),
        hInstance: instance,
        lpszClassName: CLASS,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hIcon: LoadIconW(instance, PCWSTR(1usize as *const u16)).unwrap_or_default(),
        ..Default::default()
    });
    let dpi = GetDpiForWindow(widget).max(96);
    let (outer_width, outer_height) = outer_size(WIDTH, HEIGHT, dpi);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let monitor = MonitorFromWindow(widget, MONITOR_DEFAULTTONEAREST);
    let (x, y, width, height) = if GetMonitorInfoW(monitor, &mut info).as_bool() {
        let work = info.rcWork;
        let width = outer_width.min(work.right - work.left);
        let height = outer_height.min(work.bottom - work.top);
        (
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
            width,
            height,
        )
    } else {
        (CW_USEDEFAULT, CW_USEDEFAULT, outer_width, outer_height)
    };
    let Ok(hwnd) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        CLASS,
        w!("Codex Usage"),
        window_style(),
        x,
        y,
        width,
        height,
        HWND::default(),
        HMENU::default(),
        instance,
        None,
    ) else {
        diagnose::log("unable to open settings window");
        return;
    };
    let panel = Panel::new(hwnd, widget);
    PANEL.with(|state| *state.borrow_mut() = Some(panel));
    with_panel(|panel| panel.sync(true));
    SetTimer(hwnd, REFRESH_TIMER, 1000, None);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    with_panel(|panel| {
        let _ = SetFocus(panel.nav);
    });
}

unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = wparam.0 as u16;
            let code = (wparam.0 >> 16) as u16;
            let control = HWND(lparam.0 as *mut _);
            if id == IDCANCEL.0 as u16 {
                let _ = DestroyWindow(hwnd);
            } else if id == NAV && code == LBN_SELCHANGE as u16 {
                let page = SendMessageW(control, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                with_panel(|panel| panel.change_page(page as usize));
            } else if code == BN_CLICKED as u16 {
                let request = with_panel(|panel| {
                    let request = panel.choice_request(id, control);
                    panel.menu_open = request.is_some();
                    request.map(|request| (request, panel.dpi))
                })
                .flatten();
                if let Some(((items, selected, anchor), dpi)) = request {
                    // The menu runs a nested message loop: never hold the panel here.
                    let picked = pick_from_menu(hwnd, &items, selected, anchor, dpi);
                    with_panel(|panel| {
                        panel.menu_open = false;
                        if let Some(item) = picked
                            .filter(|index| *index != selected)
                            .and_then(|index| items.get(index))
                        {
                            panel.enqueue(item.action.clone());
                        }
                    });
                } else {
                    with_panel(|panel| panel.queue_action(id, control));
                }
            }
        }
        APPLY_SELECTION => {
            if let Some((widget, action)) = with_panel(|panel| {
                panel
                    .pending
                    .pop_front()
                    .map(|action| (panel.widget, action))
            })
            .flatten()
            {
                // The existing handlers own validation, rendering, and persistence.
                match action {
                    Some(Action::Command(id)) => {
                        let _ = SendMessageW(widget, WM_COMMAND, WPARAM(id as usize), LPARAM(0));
                    }
                    Some(Action::Display(target)) => select_settings_display(widget, target),
                    Some(Action::Group(group)) => select_antigravity_group(widget, group),
                    None => {}
                }
                with_panel(|panel| panel.sync(true));
            }
        }
        ENSURE_VISIBLE => {
            with_panel(|panel| panel.ensure_visible(HWND(wparam.0 as *mut _)));
        }
        WM_TIMER if wparam.0 == ANIMATION_TIMER => {
            with_panel(|panel| panel.animate());
        }
        WM_TIMER if wparam.0 == REFRESH_TIMER => {
            with_panel(|panel| panel.sync(false));
        }
        WM_SIZE => {
            with_panel(|panel| panel.layout());
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            let (width, height) = outer_size(MIN_WIDTH, MIN_HEIGHT, GetDpiForWindow(hwnd).max(96));
            info.ptMinTrackSize = POINT {
                x: width,
                y: height,
            };
        }
        WM_VSCROLL => {
            with_panel(|panel| {
                let height = panel.client().bottom;
                let line = panel.px(48);
                let target = match SCROLLBAR_COMMAND(wparam.0 as u16 as i32) {
                    SB_LINEUP => panel.scroll - line,
                    SB_LINEDOWN => panel.scroll + line,
                    SB_PAGEUP => panel.scroll - (height - line).max(line),
                    SB_PAGEDOWN => panel.scroll + (height - line).max(line),
                    SB_TOP => 0,
                    SB_BOTTOM => i32::MAX,
                    SB_THUMBTRACK | SB_THUMBPOSITION => {
                        let mut info = SCROLLINFO {
                            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                            fMask: SIF_TRACKPOS,
                            ..Default::default()
                        };
                        if GetScrollInfo(hwnd, SB_VERT, &mut info).is_err() {
                            return;
                        }
                        info.nTrackPos
                    }
                    _ => return,
                };
                panel.scroll_to(target);
            });
            let _ = UpdateWindow(hwnd);
        }
        WM_MOUSEWHEEL => {
            let delta = i32::from((wparam.0 >> 16) as u16 as i16);
            with_panel(|panel| {
                let step = panel.px(WHEEL_STEP);
                panel.scroll_to(panel.scroll - delta * step / 120);
            });
            let _ = UpdateWindow(hwnd);
        }
        WM_DPICHANGED => {
            let dpi = wparam.0 as u16 as u32;
            let suggested = *(lparam.0 as *const RECT);
            with_panel(|panel| {
                let _redraw = SuspendRedraw::new(hwnd);
                panel.set_dpi(dpi);
                let _ = SetWindowPos(
                    hwnd,
                    HWND::default(),
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
                );
                panel.layout();
            });
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
            with_panel(|panel| {
                let theme = Theme::current(panel.preference);
                if theme != panel.theme {
                    let _redraw = SuspendRedraw::new(hwnd);
                    panel.apply_theme(theme);
                }
            });
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_NOTIFY => {
            let header = &*(lparam.0 as *const NMHDR);
            if header.code == NM_CUSTOMDRAW && header.idFrom >= usize::from(FIRST_FIELD) {
                let draw = &*(lparam.0 as *const NMCUSTOMDRAW);
                // Buttons send CDDS_PREERASE first; skipping it would also skip
                // the CDDS_PREPAINT notification that draws the control.
                if draw.dwDrawStage != CDDS_PREPAINT {
                    return LRESULT(CDRF_DODEFAULT as isize);
                }
                if with_panel_ref(|panel| panel.draw_control(draw)).is_err() {
                    request_redraw();
                }
                // A control can request a paint while its HWND or row is being changed.
                return LRESULT(CDRF_SKIPDEFAULT as isize);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_DRAWITEM if wparam.0 == NAV as usize => {
            let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
            if with_panel_ref(|panel| panel.draw_nav(item)).is_err() {
                request_redraw();
            }
            return LRESULT(1);
        }
        WM_CTLCOLORLISTBOX => {
            let dc = HDC(wparam.0 as *mut _);
            let _ = SetDCBrushColor(dc, colorref(BACKGROUND.with(Cell::get)));
            return LRESULT(GetStockObject(DC_BRUSH).0 as isize);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let painted = with_panel_ref(|panel| panel.paint(dc));
            let _ = EndPaint(hwnd, &paint);
            if painted.is_err() {
                request_redraw();
            }
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, REFRESH_TIMER);
            let _ = KillTimer(hwnd, ANIMATION_TIMER);
        }
        WM_NCDESTROY => {
            PANEL.with(|state| {
                state.borrow_mut().take();
            });
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

/// Navigation hover tracking, and scrolling focused settings into view.
unsafe extern "system" fn control_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    let is_nav = GetDlgCtrlID(hwnd) == i32::from(NAV);
    match msg {
        WM_MOUSEMOVE if is_nav => {
            let point = lparam.0 as u32;
            let hit = SendMessageW(hwnd, LB_ITEMFROMPOINT, WPARAM(0), LPARAM(point as isize)).0;
            let index = if hit >> 16 & 0xFFFF == 0 {
                (hit & 0xFFFF) as i32
            } else {
                -1
            };
            let previous = NAV_HOVER.with(|hover| hover.replace(index));
            if previous != index {
                invalidate_nav_item(hwnd, previous);
                invalidate_nav_item(hwnd, index);
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
            }
        }
        WM_MOUSELEAVE if is_nav => {
            let previous = NAV_HOVER.with(|hover| hover.replace(-1));
            invalidate_nav_item(hwnd, previous);
        }
        WM_SETFOCUS if !is_nav => {
            let _ = PostMessageW(
                GetParent(hwnd).unwrap_or_default(),
                ENSURE_VISIBLE,
                WPARAM(hwnd.0 as usize),
                LPARAM(0),
            );
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(control_proc), id);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe fn invalidate_nav_item(nav: HWND, index: i32) {
    if index < 0 {
        return;
    }
    let mut rect = RECT::default();
    if SendMessageW(
        nav,
        LB_GETITEMRECT,
        WPARAM(index as usize),
        LPARAM(&mut rect as *mut RECT as isize),
    )
    .0 != -1
    {
        let _ = InvalidateRect(nav, Some(&rect), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poller::PollError;

    fn service_label(name: &str, monitored: bool, error: Option<PollError>) -> String {
        let hint = service_status(monitored, error, LanguageId::SimplifiedChinese)
            .map(|status| Hint::Warning(status.into()));
        accessible_text(name, hint.as_ref())
    }

    #[test]
    fn service_labels_only_show_confirmed_authentication_errors() {
        let label = |monitored, error| service_label("Codex", monitored, error);

        assert_eq!(
            label(true, Some(PollError::NoCredentials)),
            "Codex (未登录)"
        );
        for error in [PollError::AuthRequired, PollError::TokenExpired] {
            assert_eq!(label(true, Some(error)), "Codex (需重新登录)");
        }
        for error in [
            None,
            Some(PollError::NetworkUnavailable),
            Some(PollError::RateLimited(None)),
            Some(PollError::ServerError),
            Some(PollError::RequestFailed),
        ] {
            assert_eq!(label(true, error), "Codex");
        }
        assert_eq!(label(false, Some(PollError::NoCredentials)), "Codex");
    }

    #[test]
    fn informational_hints_stay_out_of_accessible_names() {
        assert_eq!(
            accessible_text("Codex", Some(&Hint::Info("detail".into()))),
            "Codex"
        );
        assert_eq!(
            accessible_text(
                "Codex",
                Some(&Hint::Warning("Port 1 is unavailable".into()))
            ),
            "Codex (Port 1 is unavailable)"
        );
    }

    #[test]
    fn display_labels_follow_windows_numbering() {
        assert_eq!(
            display_label("显示器", 0, Some(r"\\.\DISPLAY2")),
            "显示器 2"
        );
        assert_eq!(
            display_label("Display", 1, Some("MONITOR")),
            "Display 2 (MONITOR)"
        );
        assert_eq!(display_label("Display", 2, None), "Display 3");
    }

    #[test]
    fn accent_text_stays_readable() {
        assert_eq!(readable_on(0x00_5FB8), 0xFF_FFFF);
        assert_eq!(readable_on(0x4C_C2FF), 0x00_0000);
        assert_eq!(colorref(0x12_3456), COLORREF(0x56_3412));
    }

    #[test]
    fn rows_keep_one_control_per_setting() {
        let option = OptionItem::command("A", 1);
        assert_eq!(check("A", 1, true).control_count(), 1);
        assert_eq!(section("A").control_count(), 0);
        assert_eq!(
            action("A", None, vec![option.clone(), option]).control_count(),
            2
        );
        assert!(!same_controls(&section("A"), &check("A", 1, true)));
    }
}
