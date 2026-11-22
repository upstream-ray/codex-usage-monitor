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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use windows::core::{w, PCSTR};
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmFlush, DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::Sleep;
use windows::Win32::UI::Controls::{
    CDDS_PREPAINT, CDIS_DISABLED, CDIS_FOCUS, CDIS_HOT, CDIS_SELECTED, CDIS_SHOWKEYBOARDCUES,
    CDRF_DODEFAULT, CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, MARGINS, NMCUSTOMDRAW, NMHDR, NM_CUSTOMDRAW,
    ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED, WM_MOUSELEAVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetCapture, GetFocus, IsWindowEnabled, ReleaseCapture, SetCapture, SetFocus,
    TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};

#[path = "settings_dropdown.rs"]
mod dropdown;
#[path = "settings_raster.rs"]
mod raster;
use raster::opaque;

const CLASS: PCWSTR = w!("CodexUsageSettings");
const CONTENT_CLASS: PCWSTR = w!("CodexUsageSettingsContent");
const OVERLAY_CLASS: PCWSTR = w!("CodexUsageSettingsTransition");
const NAV: u16 = 990;
const FIRST_FIELD: u16 = 1000;
const FIELDS_PER_ROW: u16 = 4;
const REFRESH_TIMER: usize = 1;
const SCROLLBAR_FADE_TIMER: usize = 2;
const SCROLLBAR_FADE_DELAY_MS: u32 = 1400;
const SCROLLBAR_FADE_IN: Duration = Duration::from_millis(120);
const SCROLLBAR_FADE_OUT: Duration = Duration::from_millis(300);
const SCROLLBAR_EXPAND_DURATION: Duration = Duration::from_millis(120);
const SCROLLBAR_HOT_WIDTH: i32 = 16;
const SWITCH_DURATION: Duration = Duration::from_millis(160);
const SWITCH_HOVER_DURATION: Duration = Duration::from_millis(110);
const SWITCH_PRESS_DURATION: Duration = Duration::from_millis(90);
const PAGE_DURATION: Duration = Duration::from_millis(210);
const PAGE_OLD_FADE_DURATION: Duration = Duration::from_millis(75);
const PAGE_TITLE_FADE_DURATION: Duration = Duration::from_millis(140);
const PAGE_CONTENT_FADE_DELAY: Duration = Duration::from_millis(20);
const PAGE_CONTENT_FADE_DURATION: Duration = Duration::from_millis(130);
const THEME_DURATION: Duration = Duration::from_millis(180);
const APPLY_SELECTION: u32 = WM_APP + 1;
const ENSURE_VISIBLE: u32 = WM_APP + 2;
const ANIMATION_TICK: u32 = WM_APP + 3;
const SUBCLASS_ID: usize = 1;

// Layout in 96-DPI pixels.
const WIDTH: i32 = 900;
const HEIGHT: i32 = 640;
const MIN_WIDTH: i32 = 720;
const MIN_HEIGHT: i32 = 480;
const CAPTION_HEIGHT: i32 = 44;
const CAPTION_BTN_WIDTH: i32 = 46;
const CAPTION_BTN_HEIGHT: i32 = 32;
const NAV_WIDTH: i32 = 236;
const NAV_TOP: i32 = 52;
const NAV_ITEM: i32 = 40;
const CONTENT_LEFT: i32 = 12;
const CONTENT_RIGHT: i32 = 28;
const CONTENT_MAX: i32 = 1000;
const CONTENT_TOP: i32 = 4;
const TITLE_HEIGHT: i32 = 42;
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
const CARD_RADIUS: f32 = 8.0;
const CONTROL_RADIUS: f32 = 4.0;
const WHEEL_STEP: i32 = 64;

// Segoe Fluent Icons / Segoe MDL2 Assets code points.
const GLYPH_CHEVRON_DOWN: char = '\u{E70D}';
const GLYPH_ERROR: char = '\u{EA39}';
const GLYPH_MINIMIZE: char = '\u{E921}';
const GLYPH_MAXIMIZE: char = '\u{E922}';
const GLYPH_RESTORE: char = '\u{E923}';
const GLYPH_CLOSE: char = '\u{E8BB}';
const PAGE_GLYPHS: [char; 5] = ['\u{E713}', '\u{E9D2}', '\u{E790}', '\u{EA8F}', '\u{E72E}'];

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
    // Kept outside the panel so control colors never wait on a panel borrow.
    static BACKGROUND: Cell<u32> = const { Cell::new(0xF3_F3F3) };
    static REDRAW_PENDING: Cell<bool> = const { Cell::new(false) };
    static NAV_HOVER: Cell<i32> = const { Cell::new(-1) };
    static NAV_PRESSED: Cell<i32> = const { Cell::new(-1) };
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

    fn control_enabled(&self, column: usize) -> bool {
        match self {
            Self::Check { enabled, .. } | Self::Choice { enabled, .. } => *enabled,
            Self::Action { buttons, .. } => buttons
                .get(column)
                .is_some_and(|button| button.action.is_some()),
            _ => false,
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
            let startup = startup_status();
            let startup_hint = if STARTUP_WRITE_FAILED.load(Ordering::Relaxed) {
                Some(Hint::Warning(strings.startup_write_failed.into()))
            } else if startup == StartupStatus::Blocked {
                Some(Hint::Warning(strings.startup_disabled_by_windows.into()))
            } else {
                info(strings.start_with_windows_hint)
            };
            let startup_row = if startup == StartupStatus::Blocked {
                action(
                    strings.start_with_windows,
                    startup_hint,
                    vec![OptionItem::command(
                        strings.open_startup_apps,
                        IDM_WINDOWS_STARTUP_SETTINGS,
                    )],
                )
            } else {
                check(
                    strings.start_with_windows,
                    IDM_START_WITH_WINDOWS,
                    startup == StartupStatus::Enabled,
                )
                .hint(startup_hint)
            };
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
                startup_row,
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
                check(
                    strings.show_period_labels,
                    IDM_PERIOD_LABELS,
                    !a.hide_period_labels,
                ),
                Row::Choice {
                    label: strings.percentage_position.into(),
                    items: vec![
                        OptionItem::command(
                            strings.percentage_outside_battery,
                            IDM_PERCENT_OUTSIDE_BATTERY,
                        ),
                        OptionItem::command(
                            strings.percentage_inside_battery,
                            IDM_PERCENT_INSIDE_BATTERY,
                        ),
                    ],
                    selected: usize::from(a.shows_percent_in_battery()),
                    enabled: a.bar_style == BarStyle::Battery,
                    hint: None,
                },
                choice(
                    strings.reset_time_display,
                    &[
                        (IDM_RESET_COUNTDOWN, strings.reset_countdown),
                        (IDM_RESET_CLOCK, strings.reset_clock),
                    ],
                    match a.reset_time_display {
                        ResetTimeDisplay::Countdown => IDM_RESET_COUNTDOWN,
                        ResetTimeDisplay::Clock => IDM_RESET_CLOCK,
                    },
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
    control_elevation: u32,
    strong_stroke: u32,
    knob: u32,
    nav_selected: u32,
    nav_hover: u32,
    nav_pressed: u32,
    caption_hover: u32,
    caption_pressed: u32,
    close_hover: u32,
    close_pressed: u32,
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
            // Subtle accent-tinted Mica backdrop while keeping opaque ClearType rendering.
            let background = mix_color(0x20_2020, accent, 0.025);
            Self {
                dark: true,
                background,
                card: 0x2B_2B2B,
                card_hover: 0x32_3232,
                card_pressed: 0x26_2626,
                card_border: 0x35_3535,
                text: 0xFF_FFFF,
                secondary: 0xC5_C5C5,
                disabled: 0x78_7878,
                accent,
                on_accent: readable_on(accent),
                control: 0x31_3131,
                control_hover: 0x38_3838,
                control_pressed: 0x2A_2A2A,
                control_border: 0x3D_3D3D,
                control_elevation: 0x4D_4D4D,
                strong_stroke: 0x9A_9A9A,
                knob: 0xCF_CFCF,
                nav_selected: mix_color(background, 0xFF_FFFF, 0.075),
                nav_hover: mix_color(background, 0xFF_FFFF, 0.045),
                nav_pressed: mix_color(background, 0xFF_FFFF, 0.025),
                caption_hover: mix_color(background, 0xFF_FFFF, 0.075),
                caption_pressed: mix_color(background, 0xFF_FFFF, 0.04),
                close_hover: 0xC4_2B1C,
                close_pressed: 0xB3_271C,
                caution: 0xFC_E100,
                critical: 0xFF_99A4,
                critical_background: 0x44_2726,
            }
        } else {
            let accent = palette.map_or(0x00_5FB8, |palette| palette[4]);
            let background = mix_color(0xF3_F3F3, accent, 0.03);
            Self {
                dark: false,
                background,
                card: 0xFF_FFFF,
                card_hover: 0xF9_F9F9,
                card_pressed: 0xF4_F4F4,
                card_border: 0xE5_E5E5,
                text: 0x1A_1A1A,
                secondary: 0x5D_5D5D,
                disabled: 0x9E_9E9E,
                accent,
                on_accent: readable_on(accent),
                control: 0xFF_FFFF,
                control_hover: 0xF9_F9F9,
                control_pressed: 0xF4_F4F4,
                control_border: 0xE0_E0E0,
                control_elevation: 0xC4_C4C4,
                strong_stroke: 0x86_8686,
                knob: 0x5F_5F5F,
                nav_selected: mix_color(background, 0x00_0000, 0.055),
                nav_hover: mix_color(background, 0x00_0000, 0.032),
                nav_pressed: mix_color(background, 0x00_0000, 0.018),
                caption_hover: mix_color(background, 0x00_0000, 0.055),
                caption_pressed: mix_color(background, 0x00_0000, 0.09),
                close_hover: 0xC4_2B1C,
                close_pressed: 0xB2_2A1E,
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
    caption_title: HFONT,
    title: HFONT,
    icons: HFONT,
    small_icons: HFONT,
    caption_icons: HFONT,
    has_icons: bool,
}

unsafe fn ui_font_face(language: LanguageId) -> PCWSTR {
    let preferred: &[PCWSTR] = match language {
        // Windows font linking supplies the YaHei UI glyphs missing from Segoe.
        LanguageId::SimplifiedChinese => &[
            w!("Segoe UI Variable Text"),
            w!("Segoe UI"),
            w!("Microsoft YaHei UI"),
            w!("Microsoft YaHei"),
        ],
        LanguageId::TraditionalChinese => &[w!("Microsoft JhengHei UI"), w!("Segoe UI")],
        LanguageId::Japanese => &[w!("Yu Gothic UI"), w!("Segoe UI")],
        LanguageId::Korean => &[w!("Malgun Gothic"), w!("Segoe UI")],
        _ => &[w!("Segoe UI Variable Text"), w!("Segoe UI")],
    };
    preferred
        .iter()
        .copied()
        .find(|face| font_installed(*face))
        .unwrap_or(w!("Segoe UI"))
}

impl Fonts {
    unsafe fn create(dpi: u32, language: LanguageId) -> Self {
        let ui_face = ui_font_face(language);
        let body_size = match language {
            LanguageId::SimplifiedChinese => 13,
            _ => 14,
        };
        let icon_face = [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")]
            .into_iter()
            .find(|face| font_installed(*face));
        let icons = |size| {
            icon_face.map_or(HFONT::default(), |face| {
                make_font(face, scale(size, dpi), FW_NORMAL.0)
            })
        };
        Self {
            body: make_font(ui_face, scale(body_size, dpi), FW_NORMAL.0),
            strong: make_font(ui_face, scale(body_size, dpi), FW_SEMIBOLD.0),
            caption: make_font(ui_face, scale(12, dpi), FW_NORMAL.0),
            caption_title: make_font(ui_face, scale(13, dpi), FW_SEMIBOLD.0),
            title: make_font(ui_face, scale(26, dpi), FW_SEMIBOLD.0),
            icons: icons(16),
            small_icons: icons(12),
            caption_icons: icons(10),
            has_icons: icon_face.is_some(),
        }
    }

    unsafe fn delete(self) {
        for font in [
            self.body,
            self.strong,
            self.caption,
            self.caption_title,
            self.title,
            self.icons,
            self.small_icons,
            self.caption_icons,
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

fn translated(rect: RECT, x: i32, y: i32) -> RECT {
    RECT {
        left: rect.left + x,
        right: rect.right + x,
        ..offset(rect, y)
    }
}

fn intersection(a: RECT, b: RECT) -> RECT {
    RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

fn has_area(rect: RECT) -> bool {
    rect.right > rect.left && rect.bottom > rect.top
}

struct Bitmap {
    dc: HDC,
    handle: HBITMAP,
    previous: HGDIOBJ,
    bits: std::ptr::NonNull<u32>,
    width: i32,
    height: i32,
}

impl Bitmap {
    unsafe fn new(width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let dc = CreateCompatibleDC(HDC::default());
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let handle =
            CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
        let previous = SelectObject(dc, handle);
        match std::ptr::NonNull::new(bits.cast()) {
            Some(bits) if !previous.is_invalid() => Some(Self {
                dc,
                handle,
                previous,
                bits,
                width,
                height,
            }),
            _ => {
                if !previous.is_invalid() {
                    SelectObject(dc, previous);
                }
                let _ = DeleteObject(handle);
                let _ = DeleteDC(dc);
                None
            }
        }
    }

    fn bounds(&self) -> RECT {
        RECT {
            right: self.width,
            bottom: self.height,
            ..Default::default()
        }
    }

    unsafe fn pixels(&self) -> &[u32] {
        let _ = GdiFlush();
        std::slice::from_raw_parts(self.bits.as_ptr(), (self.width * self.height) as usize)
    }

    unsafe fn pixels_mut(&mut self) -> &mut [u32] {
        let _ = GdiFlush();
        std::slice::from_raw_parts_mut(self.bits.as_ptr(), (self.width * self.height) as usize)
    }

    unsafe fn clone_bitmap(&self) -> Option<Self> {
        let mut copy = Self::new(self.width, self.height)?;
        copy.pixels_mut().copy_from_slice(self.pixels());
        Some(copy)
    }

    unsafe fn blit(&self, target: HDC, area: RECT, x: i32, y: i32) -> bool {
        BitBlt(
            target,
            area.left,
            area.top,
            area.right - area.left,
            area.bottom - area.top,
            self.dc,
            x,
            y,
            SRCCOPY,
        )
        .is_ok()
    }
}

struct Drawing<'a> {
    dc: HDC,
    bitmap: Option<&'a mut Bitmap>,
}

impl<'a> Drawing<'a> {
    fn new(dc: HDC, bitmap: Option<&'a mut Bitmap>) -> Self {
        Self {
            dc: bitmap.as_ref().map_or(dc, |bitmap| bitmap.dc),
            bitmap,
        }
    }

    fn for_bitmap(bitmap: &'a mut Bitmap) -> Self {
        Self {
            dc: bitmap.dc,
            bitmap: Some(bitmap),
        }
    }

    unsafe fn canvas(&mut self, clip: RECT) -> Option<raster::Canvas<'_>> {
        let bitmap = self.bitmap.as_mut()?;
        // Flush queued GDI writes before accessing the DIB section from the CPU.
        let _ = GdiFlush();
        let width = bitmap.width as usize;
        let pixels =
            std::slice::from_raw_parts_mut(bitmap.bits.as_ptr(), width * bitmap.height as usize);
        Some(raster::Canvas::new(pixels, width, clip))
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.handle);
            let _ = DeleteDC(self.dc);
        }
    }
}

unsafe fn fill_rect(dc: HDC, rect: RECT, color: u32) {
    let _ = SetDCBrushColor(dc, colorref(color));
    FillRect(dc, &rect, HBRUSH(GetStockObject(DC_BRUSH).0));
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
    text: TextMetrics,
}

#[derive(Clone, Copy, Default)]
struct TextMetrics {
    label: i32,
    hint: i32,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    rect: RECT,
    label_width: i32,
}

#[derive(Clone, Copy)]
struct ScalarAnim {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl ScalarAnim {
    fn settled(value: f32, now: Instant, duration: Duration) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            duration,
        }
    }

    fn value(self, now: Instant) -> f32 {
        if (self.to - self.from).abs() < 1e-4 {
            return self.to;
        }
        let t = ease_out(now.duration_since(self.start), self.duration);
        self.from + (self.to - self.from) * t
    }

    fn is_active(self, now: Instant) -> bool {
        (self.to - self.from).abs() >= 1e-4 && now.duration_since(self.start) < self.duration
    }

    fn retarget(&mut self, to: f32, now: Instant) -> bool {
        self.retarget_with(to, self.duration, now)
    }

    fn retarget_with(&mut self, to: f32, duration: Duration, now: Instant) -> bool {
        if (self.to - to).abs() < 1e-4 {
            return false;
        }
        self.from = self.value(now);
        self.to = to;
        self.duration = duration;
        self.start = now;
        true
    }
}

#[derive(Clone, Copy)]
struct SwitchState {
    checked: ScalarAnim,
    hover: ScalarAnim,
    press: ScalarAnim,
    hot: bool,
    pressed: bool,
}

impl SwitchState {
    fn new(checked: bool, now: Instant) -> Self {
        Self {
            checked: ScalarAnim::settled(f32::from(u8::from(checked)), now, SWITCH_DURATION),
            hover: ScalarAnim::settled(0.0, now, SWITCH_HOVER_DURATION),
            press: ScalarAnim::settled(0.0, now, SWITCH_PRESS_DURATION),
            hot: false,
            pressed: false,
        }
    }

    fn set_checked(&mut self, checked: bool, now: Instant) -> bool {
        self.checked.retarget(f32::from(u8::from(checked)), now)
    }

    fn update_interaction(
        &mut self,
        hot: bool,
        pressed: bool,
        disabled: bool,
        now: Instant,
    ) -> bool {
        let hot = hot && !disabled;
        let pressed = pressed && !disabled;
        let mut changed = false;
        if self.hot != hot {
            self.hot = hot;
            changed |= self.hover.retarget(if hot { 1.0 } else { 0.0 }, now);
        }
        if self.pressed != pressed {
            self.pressed = pressed;
            changed |= self.press.retarget(if pressed { 1.0 } else { 0.0 }, now);
        }
        changed
    }

    fn is_active(&self, now: Instant) -> bool {
        self.checked.is_active(now) || self.hover.is_active(now) || self.press.is_active(now)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarMetrics {
    track_top: f32,
    track_bottom: f32,
    thumb_top: f32,
    thumb_height: f32,
}

fn scrollbar_thumb_metrics(
    client_h: i32,
    content_h: i32,
    scroll: i32,
    dpi: u32,
) -> Option<ScrollbarMetrics> {
    if content_h <= client_h || client_h <= 0 {
        return None;
    }
    let margin = scale(8, dpi) as f32;
    let track_top = margin;
    let track_bottom = (client_h as f32 - margin).max(track_top + 1.0);
    let track_h = track_bottom - track_top;
    let min_thumb = scale(32, dpi) as f32;
    let thumb_height = (track_h * (client_h as f32 / content_h as f32))
        .max(min_thumb)
        .min(track_h);
    let max_scroll = (content_h - client_h).max(1) as f32;
    let ratio = (scroll as f32 / max_scroll).clamp(0.0, 1.0);
    let thumb_top = track_top + (track_h - thumb_height) * ratio;
    Some(ScrollbarMetrics {
        track_top,
        track_bottom,
        thumb_top,
        thumb_height,
    })
}

struct OverlayScrollbar {
    opacity: ScalarAnim,
    expand: ScalarAnim,
    hovered: bool,
    dragging: bool,
    drag_anchor_y: i32,
    drag_start_scroll: i32,
}

impl OverlayScrollbar {
    fn new(now: Instant) -> Self {
        Self {
            opacity: ScalarAnim::settled(0.0, now, SCROLLBAR_FADE_IN),
            expand: ScalarAnim::settled(0.0, now, SCROLLBAR_EXPAND_DURATION),
            hovered: false,
            dragging: false,
            drag_anchor_y: 0,
            drag_start_scroll: 0,
        }
    }

    fn show(&mut self, now: Instant) -> bool {
        self.opacity.retarget_with(1.0, SCROLLBAR_FADE_IN, now)
    }

    fn fade_out(&mut self, now: Instant) -> bool {
        if self.hovered || self.dragging {
            return false;
        }
        self.opacity.retarget_with(0.0, SCROLLBAR_FADE_OUT, now)
    }

    fn set_expanded(&mut self, expanded: bool, now: Instant) -> bool {
        let target = if expanded { 1.0 } else { 0.0 };
        let mut changed = self
            .expand
            .retarget_with(target, SCROLLBAR_EXPAND_DURATION, now);
        if expanded {
            changed |= self.show(now);
        }
        changed
    }

    fn is_active(&self, now: Instant) -> bool {
        self.opacity.is_active(now) || self.expand.is_active(now)
    }
}

fn ease_out_cubic_f(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn ease_out_quart_f(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(4)
}

fn ease_out(elapsed: Duration, duration: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.0);
    ease_out_cubic_f(t)
}

fn page_slide(elapsed: Duration, distance: i32) -> i32 {
    let t = (elapsed.as_secs_f32() / PAGE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    (distance as f32 * (1.0 - ease_out_quart_f(t))).round() as i32
}

fn page_alphas(elapsed: Duration) -> (u8, u8, u8) {
    let old_t = (elapsed.as_secs_f32() / PAGE_OLD_FADE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    let old_alpha = ((1.0 - old_t).powi(2) * 255.0).round() as u8;

    let title_t = (elapsed.as_secs_f32() / PAGE_TITLE_FADE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    let title_alpha = (ease_out_cubic_f(title_t) * 255.0).round() as u8;

    let content_elapsed = elapsed.saturating_sub(PAGE_CONTENT_FADE_DELAY);
    let content_t =
        (content_elapsed.as_secs_f32() / PAGE_CONTENT_FADE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    let content_alpha = (ease_out_cubic_f(content_t) * 255.0).round() as u8;

    (old_alpha, title_alpha, content_alpha)
}

/// Non-blocking DWM VSync pacer. Sleeps on a condition variable when idle,
/// wakes only during active animations, and exits cleanly when Settings closes.
struct VSyncState {
    active: Mutex<bool>,
    cond: Condvar,
    stop: AtomicBool,
    tick_pending: AtomicBool,
    hwnd: AtomicUsize,
}

struct VSyncPacer {
    state: Arc<VSyncState>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl VSyncPacer {
    fn new(hwnd: HWND) -> Self {
        let state = Arc::new(VSyncState {
            active: Mutex::new(false),
            cond: Condvar::new(),
            stop: AtomicBool::new(false),
            tick_pending: AtomicBool::new(false),
            hwnd: AtomicUsize::new(hwnd.0 as usize),
        });
        let thread_state = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("settings-vsync".into())
            .spawn(move || {
                let mut use_dwm = true;
                loop {
                    {
                        let mut active = match thread_state.active.lock() {
                            Ok(guard) => guard,
                            Err(_) => break,
                        };
                        while !*active && !thread_state.stop.load(Ordering::Acquire) {
                            active = match thread_state.cond.wait(active) {
                                Ok(guard) => guard,
                                Err(_) => return,
                            };
                        }
                    }
                    if thread_state.stop.load(Ordering::Acquire) {
                        break;
                    }
                    unsafe {
                        if use_dwm {
                            use_dwm = DwmFlush().is_ok();
                        }
                        if !use_dwm {
                            Sleep(8);
                        }
                    }
                    if thread_state.stop.load(Ordering::Acquire) {
                        break;
                    }
                    if !thread_state.tick_pending.swap(true, Ordering::AcqRel) {
                        let raw = thread_state.hwnd.load(Ordering::Acquire);
                        if raw != 0 {
                            unsafe {
                                if PostMessageW(
                                    HWND(raw as *mut _),
                                    ANIMATION_TICK,
                                    WPARAM(0),
                                    LPARAM(0),
                                )
                                .is_err()
                                {
                                    thread_state.tick_pending.store(false, Ordering::Release);
                                }
                            }
                        }
                    }
                }
            })
            .ok();
        Self { state, handle }
    }

    fn set_active(&self, active: bool) {
        if let Ok(mut guard) = self.state.active.lock() {
            if *guard != active {
                *guard = active;
                if active {
                    self.state.cond.notify_one();
                }
            }
        }
    }

    fn clear_pending(&self) {
        self.state.tick_pending.store(false, Ordering::Release);
    }

    fn stop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        self.state.hwnd.store(0, Ordering::Release);
        if let Ok(mut guard) = self.state.active.lock() {
            *guard = false;
            self.state.cond.notify_one();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for VSyncPacer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

fn caption_button_rect(client_width: i32, dpi: u32, button: CaptionButton) -> RECT {
    let w = scale(CAPTION_BTN_WIDTH, dpi);
    let h = scale(CAPTION_BTN_HEIGHT, dpi);
    let slot = match button {
        CaptionButton::Minimize => 3,
        CaptionButton::Maximize => 2,
        CaptionButton::Close => 1,
    };
    RECT {
        left: client_width - slot * w,
        top: 0,
        right: client_width - (slot - 1) * w,
        bottom: h,
    }
}

fn caption_button_at(client_width: i32, dpi: u32, pt: POINT) -> Option<CaptionButton> {
    for button in [
        CaptionButton::Close,
        CaptionButton::Maximize,
        CaptionButton::Minimize,
    ] {
        let r = caption_button_rect(client_width, dpi, button);
        if pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom {
            return Some(button);
        }
    }
    None
}

fn hit_test_caption(client: RECT, pt: POINT, dpi: u32, maximized: bool) -> i32 {
    let resize = scale(5, dpi);
    let corner = scale(12, dpi);
    let caption_h = scale(CAPTION_HEIGHT, dpi);
    if !maximized && pt.y >= 0 && pt.y < resize {
        if pt.x < corner {
            return HTTOPLEFT as i32;
        }
        if pt.x >= client.right - corner {
            return HTTOPRIGHT as i32;
        }
        return HTTOP as i32;
    }
    if let Some(button) = caption_button_at(client.right, dpi, pt) {
        return match button {
            CaptionButton::Minimize => HTMINBUTTON as i32,
            CaptionButton::Maximize => HTMAXBUTTON as i32,
            CaptionButton::Close => HTCLOSE as i32,
        };
    }
    if pt.y >= 0 && pt.y < caption_h && pt.x >= 0 && pt.x < client.right {
        return HTCAPTION as i32;
    }
    HTCLIENT as i32
}

struct Overlay(HWND);

impl Overlay {
    unsafe fn new(parent: HWND, bounds: RECT) -> Option<Self> {
        let overlay = Self(
            CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                OVERLAY_CLASS,
                PCWSTR::null(),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                parent,
                HMENU::default(),
                GetModuleHandleW(None).unwrap_or_default(),
                None,
            )
            .ok()?,
        );
        SetWindowPos(
            overlay.0,
            HWND_TOP,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_DEFERERASE,
        )
        .ok()?;
        Some(overlay)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = SetWindowPos(
                self.0,
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_HIDEWINDOW
                    | SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | SWP_NOREDRAW,
            );
            let _ = DestroyWindow(self.0);
        }
    }
}

enum TransitionKind {
    Page {
        old_image: Option<Bitmap>,
        new_image: Bitmap,
        old_rows: Vec<Row>,
        background: u32,
        title_bottom: i32,
        distance: i32,
    },
    Theme {
        old_image: Bitmap,
        new_image: Bitmap,
    },
}

struct Transition {
    overlay: Overlay,
    composed: Bitmap,
    kind: TransitionKind,
    start: Instant,
    drawn_elapsed: Duration,
    trace: Option<TransitionTrace>,
}

impl Transition {
    fn duration(&self) -> Duration {
        match &self.kind {
            TransitionKind::Page { .. } => PAGE_DURATION,
            TransitionKind::Theme { .. } => THEME_DURATION,
        }
    }

    fn is_page(&self) -> bool {
        matches!(self.kind, TransitionKind::Page { .. })
    }

    unsafe fn compose(&mut self, elapsed: Duration) {
        let width = self.composed.width as usize;
        let height = self.composed.height as usize;
        match &self.kind {
            TransitionKind::Page {
                old_image,
                new_image,
                background,
                title_bottom,
                distance,
                ..
            } => {
                let (old_alpha, title_alpha, content_alpha) = page_alphas(elapsed);
                let slide = page_slide(elapsed, *distance).clamp(0, height as i32) as usize;
                let title_h = (*title_bottom).clamp(0, height as i32) as usize;

                let dst = self.composed.pixels_mut();
                dst.fill(*background);

                if old_alpha > 0 {
                    if let Some(old) = old_image {
                        let w = width.min(old.width as usize);
                        let rows = height.min(old.height as usize);
                        raster::blend_over_rows(
                            dst,
                            width,
                            0,
                            old.pixels(),
                            old.width as usize,
                            0,
                            w,
                            rows,
                            old_alpha,
                        );
                    }
                }

                let nw = width.min(new_image.width as usize);
                let nh = height.min(new_image.height as usize);
                let new_px = new_image.pixels();

                if title_alpha > 0 && title_h > 0 {
                    let rows = title_h.min(nh);
                    raster::blend_over_rows(
                        dst,
                        width,
                        0,
                        new_px,
                        new_image.width as usize,
                        0,
                        nw,
                        rows,
                        title_alpha,
                    );
                }

                if content_alpha > 0 && title_h < nh {
                    let dst_y = title_h + slide;
                    if dst_y < height {
                        let avail_dst = height - dst_y;
                        let avail_src = nh - title_h;
                        let rows = avail_dst.min(avail_src);
                        raster::blend_over_rows(
                            dst,
                            width,
                            dst_y,
                            new_px,
                            new_image.width as usize,
                            title_h,
                            nw,
                            rows,
                            content_alpha,
                        );
                    }
                }
            }
            TransitionKind::Theme {
                old_image,
                new_image,
            } => {
                let t = ease_out(elapsed, THEME_DURATION);
                let alpha = (t * 255.0).round() as u8;
                let dst = self.composed.pixels_mut();
                let ow = width.min(old_image.width as usize);
                let oh = height.min(old_image.height as usize);
                raster::blend_over_rows(
                    dst,
                    width,
                    0,
                    old_image.pixels(),
                    old_image.width as usize,
                    0,
                    ow,
                    oh,
                    255,
                );
                let nw = width.min(new_image.width as usize);
                let nh = height.min(new_image.height as usize);
                raster::blend_over_rows(
                    dst,
                    width,
                    0,
                    new_image.pixels(),
                    new_image.width as usize,
                    0,
                    nw,
                    nh,
                    alpha,
                );
            }
        }
    }

    unsafe fn draw(&mut self, now: Instant) -> (bool, bool) {
        let elapsed = now.duration_since(self.start);
        let finished = elapsed >= self.duration();
        self.compose(elapsed);
        let dc = GetDC(self.overlay.0);
        if dc.is_invalid() {
            return (false, true);
        }
        let drew = self.composed.blit(dc, self.composed.bounds(), 0, 0);
        ReleaseDC(self.overlay.0, dc);
        let _ = GdiFlush();
        self.drawn_elapsed = elapsed;
        if let Some(trace) = &mut self.trace {
            trace
                .frames
                .push((elapsed.as_micros(), now.elapsed().as_micros()));
        }
        (drew, finished)
    }

    unsafe fn repaint(&self, dc: HDC) {
        let _ = self.composed.blit(dc, self.composed.bounds(), 0, 0);
    }
}

struct TransitionTrace {
    clicked: Instant,
    first_frame_us: Option<u128>,
    frames: Vec<(u128, u128)>,
}

impl TransitionTrace {
    fn new(clicked: Instant) -> Self {
        Self {
            clicked,
            first_frame_us: None,
            frames: Vec::with_capacity(24),
        }
    }
}

impl Drop for TransitionTrace {
    fn drop(&mut self) {
        use std::fmt::Write;
        let mut lines = String::new();
        if let Some(first_frame_us) = self.first_frame_us {
            let _ = writeln!(
                lines,
                "settings transition click_to_first_frame_us={first_frame_us}"
            );
        }
        for (t, cost) in &self.frames {
            let _ = writeln!(lines, "settings transition frame t_us={t} cost_us={cost}");
        }
        let _ = write!(
            lines,
            "settings transition total_frames={}",
            self.frames.len()
        );
        diagnose::log(lines);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PreviewKey {
    appearance: Appearance,
    system_dark: bool,
    language: LanguageId,
    widget_dpi: u32,
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
    content: HWND,
    widget: HWND,
    nav: HWND,
    pacer: VSyncPacer,
    theme: Theme,
    preference: SettingsTheme,
    switches: RefCell<Vec<SwitchState>>,
    scrollbar: OverlayScrollbar,
    transition: Option<Transition>,
    transition_press: Option<(HWND, RECT)>,
    page_buffer: RefCell<Option<Bitmap>>,
    content_buffer: RefCell<Option<Bitmap>>,
    control_buffer: RefCell<Option<Bitmap>>,
    preview: RefCell<Option<(PreviewKey, Bitmap)>>,
    fonts: Fonts,
    icon: HICON,
    dpi: u32,
    language: Option<LanguageId>,
    save_failed: bool,
    window_active: bool,
    caption_hover: Option<CaptionButton>,
    caption_pressed: Option<CaptionButton>,
    page: usize,
    rows: Vec<Row>,
    controls: Vec<Vec<HWND>>,
    frames: Vec<Frame>,
    slots: Vec<Vec<Slot>>,
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
        let (preference, initial_lang) = lock_state()
            .as_ref()
            .map_or((SettingsTheme::System, LanguageId::English), |s| {
                (s.settings_theme, s.language)
            });
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
        let content = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            CONTENT_CLASS,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            0,
            0,
            hwnd,
            HMENU::default(),
            GetModuleHandleW(None).unwrap_or_default(),
            None,
        )
        .unwrap_or_default();
        let dpi = GetDpiForWindow(hwnd).max(96);
        let now = Instant::now();
        let mut panel = Self {
            hwnd,
            content,
            widget,
            nav,
            pacer: VSyncPacer::new(hwnd),
            theme,
            preference,
            switches: RefCell::new(Vec::new()),
            scrollbar: OverlayScrollbar::new(now),
            transition: None,
            transition_press: None,
            page_buffer: RefCell::new(None),
            content_buffer: RefCell::new(None),
            control_buffer: RefCell::new(None),
            preview: RefCell::new(None),
            fonts: Fonts::default(),
            icon: HICON::default(),
            dpi: 0,
            language: None,
            save_failed: false,
            window_active: true,
            caption_hover: None,
            caption_pressed: None,
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
        panel.set_dpi(dpi, initial_lang);
        panel.apply_theme(panel.theme);
        panel
    }

    fn px(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    fn pxf(&self, value: f32) -> f32 {
        value * self.dpi as f32 / 96.0
    }

    unsafe fn set_dpi(&mut self, dpi: u32, language: LanguageId) {
        self.preview.get_mut().take();
        let old = std::mem::replace(&mut self.fonts, Fonts::create(dpi, language));
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
        let size = self.px(20);
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
        self.preview.get_mut().take();
        self.theme = theme;
        BACKGROUND.with(|color| color.set(theme.background));
        let dark = BOOL::from(self.theme.dark);
        let _ = DwmSetWindowAttribute(
            self.hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
        if let Some(function) = uxtheme_export(133) {
            let allow_dark_mode: unsafe extern "system" fn(HWND, BOOL) -> BOOL =
                std::mem::transmute(function);
            let _ = allow_dark_mode(self.hwnd, dark);
            let _ = allow_dark_mode(self.content, dark);
        }
        request_redraw();
    }

    unsafe fn start_theme_transition(&mut self, old_image: Option<Bitmap>) {
        if !IsWindowVisible(self.hwnd).as_bool() {
            return;
        }
        let Some(old_image) = old_image else {
            return;
        };
        let Some(new_image) = self.capture_window() else {
            return;
        };
        let bounds = new_image.bounds();
        let Some(composed) = Bitmap::new(bounds.right, bounds.bottom) else {
            return;
        };
        let Some(overlay) = Overlay::new(self.hwnd, bounds) else {
            return;
        };
        let start = Instant::now();
        let mut transition = Transition {
            overlay,
            composed,
            kind: TransitionKind::Theme {
                old_image,
                new_image,
            },
            start,
            drawn_elapsed: Duration::ZERO,
            trace: None,
        };
        let (drew, finished) = transition.draw(start);
        if drew && !finished {
            self.transition = Some(transition);
            self.update_pacer(start);
        }
    }

    unsafe fn sync(&mut self, force: bool) {
        if self.menu_open || (!force && self.transition.is_some()) {
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
            if matches!(Page::ALL[self.page], Page::Appearance)
                && self
                    .preview
                    .get_mut()
                    .as_ref()
                    .is_some_and(|(key, _)| key.widget_dpi != CURRENT_DPI.load(Ordering::Relaxed))
            {
                self.preview.get_mut().take();
                request_redraw();
            }
            return;
        }

        let new_theme = Theme::current(preference);
        let animate_theme = self.preference != preference
            && new_theme != self.theme
            && IsWindowVisible(self.hwnd).as_bool();
        let old_theme_snapshot = if animate_theme {
            self.finish_transition();
            self.capture_window()
        } else {
            None
        };

        let redraw = SuspendRedraw::new(self.hwnd);
        self.apply_model(language, preference, save_failed);
        let old_rows = std::mem::replace(&mut self.rows, next);
        let lost_focus = self.sync_controls(&old_rows, force);
        self.layout();
        drop(redraw);
        self.restore_focus(lost_focus);
        if animate_theme {
            self.start_theme_transition(old_theme_snapshot);
        }
        self.update_pacer(Instant::now());
    }

    unsafe fn apply_model(
        &mut self,
        language: LanguageId,
        preference: SettingsTheme,
        save_failed: bool,
    ) {
        if self.preference != preference {
            self.preference = preference;
            self.apply_theme(Theme::current(preference));
        }
        if self.language != Some(language) {
            let changed_from_existing = self.language.is_some();
            self.language = Some(language);
            if changed_from_existing {
                self.set_dpi(self.dpi, language);
            }
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
    }

    unsafe fn sync_controls(&mut self, old_rows: &[Row], force: bool) -> Option<usize> {
        let focus = GetFocus();
        let mut lost_focus = None;
        let mut controls_changed = self.controls.len() != self.rows.len();
        let old_controls = std::mem::take(&mut self.controls);
        let mut old_controls = old_controls.into_iter();
        let now = Instant::now();
        let mut switches = self.switches.borrow_mut();
        switches.resize(self.rows.len(), SwitchState::new(false, now));
        for (index, row) in self.rows.iter().enumerate() {
            let old = old_rows.get(index);
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
                        checked: old_checked,
                        ..
                    }),
                    Row::Check { item, checked, .. },
                ) if reused && old_item.action == item.action => {
                    if old_checked != checked {
                        switches[index].set_checked(*checked, now);
                    }
                }
                (_, Row::Check { checked, .. }) => {
                    switches[index] = SwitchState::new(*checked, now);
                }
                _ => {
                    switches[index] = SwitchState::new(false, now);
                }
            }
            controls_changed |= !reused;
            if force || !reused || old != Some(row) {
                update_row(row, &controls);
            }
            self.controls.push(controls);
        }
        drop(switches);
        for controls in old_controls {
            if controls.contains(&focus) {
                lost_focus = Some(self.rows.len());
            }
            for control in controls {
                let _ = DestroyWindow(control);
            }
        }
        if controls_changed {
            // Dialog navigation uses HWND z-order within the content container.
            let mut previous = HWND_TOP;
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
        lost_focus
    }

    unsafe fn restore_focus(&self, lost_focus: Option<usize>) {
        if let Some(index) = lost_focus.filter(|_| IsWindowVisible(self.hwnd).as_bool()) {
            let replacement = self
                .controls
                .get(index)
                .and_then(|controls| controls.first())
                .copied();
            let _ = SetFocus(replacement.unwrap_or(self.nav));
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
                    self.content,
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

    unsafe fn content_client(&self) -> RECT {
        let mut rect = RECT::default();
        let _ = GetClientRect(self.content, &mut rect);
        rect
    }

    unsafe fn measure_text(
        &self,
        dc: HDC,
        label: &str,
        hint: Option<&Hint>,
        text_width: i32,
    ) -> TextMetrics {
        TextMetrics {
            label: text_height(dc, self.fonts.body, label, text_width),
            hint: hint.map_or(0, |hint| {
                text_height(dc, self.fonts.caption, hint.text(), text_width)
            }),
        }
    }

    fn card_height(&self, text: TextMetrics, hint: Option<&Hint>) -> i32 {
        let gap = if hint.is_some() { self.px(3) } else { 0 };
        (text.label + text.hint + gap + 2 * self.px(CARD_VPAD)).max(self.px(CARD_MIN))
    }

    fn button_width(&self, label_width: i32, item: &OptionItem) -> i32 {
        let swatch = if item.swatch.is_some() {
            self.px(22)
        } else {
            0
        };
        (label_width + self.px(32) + swatch).max(self.px(88))
    }

    /// Measure every row, then position the controls.
    unsafe fn layout(&mut self) {
        let _redraw = SuspendRedraw::new(self.hwnd);
        let client = self.client();
        if self
            .control_buffer
            .get_mut()
            .as_ref()
            .is_some_and(|bitmap| bitmap.width > client.right || bitmap.height > client.bottom)
        {
            self.control_buffer.get_mut().take();
        }
        let nav_w = self.px(NAV_WIDTH);
        let cap_h = self.px(CAPTION_HEIGHT);
        // The frame paints the empty sidebar below the native list box.
        let nav_h = (client.bottom - self.px(NAV_TOP) - self.px(8))
            .clamp(0, self.px(NAV_ITEM) * Page::ALL.len() as i32);
        let _ = SetWindowPos(
            self.nav,
            HWND_TOP,
            self.px(8),
            self.px(NAV_TOP),
            self.px(NAV_WIDTH - 16),
            nav_h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOREDRAW,
        );
        let _ = SetWindowPos(
            self.content,
            self.nav,
            nav_w,
            cap_h,
            (client.right - nav_w).max(0),
            (client.bottom - cap_h).max(0),
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOREDRAW,
        );
        self.measure_page();
        self.place_controls();

        request_redraw();
    }

    unsafe fn measure_page(&mut self) {
        self.measure();
        self.update_scrollbar();
    }

    unsafe fn measure(&mut self) {
        let content_client = self.content_client();
        let dc = GetDC(self.content);
        let left = self.px(CONTENT_LEFT);
        // Overlay scrollbar sits inside the right padding so card width is always identical.
        let right = (content_client.right - self.px(CONTENT_RIGHT))
            .min(left + self.px(CONTENT_MAX))
            .max(left + self.px(240));
        let width = right - left;
        let pad = self.px(CARD_PAD);
        let mut y = self.px(CONTENT_TOP + TITLE_HEIGHT) + self.px(14);
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
                            ..Default::default()
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
                            ..Default::default()
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
                            ..Default::default()
                        },
                        Vec::new(),
                    )
                }
                Row::Check { item, hint, .. } => {
                    let text_right = right - pad - self.px(TOGGLE_WIDTH + CONTROL_GAP);
                    let text =
                        self.measure_text(dc, &item.label, hint.as_ref(), text_right - left - pad);
                    let height = self.card_height(text, hint.as_ref());
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
                            text_right,
                            text,
                        },
                        vec![Slot {
                            rect,
                            ..Default::default()
                        }],
                    )
                }
                Row::Choice { label, hint, .. } => {
                    let control = self.px(DROPDOWN_WIDTH).min(width * 45 / 100);
                    let text_right = right - pad - control - self.px(CONTROL_GAP);
                    let text = self.measure_text(dc, label, hint.as_ref(), text_right - left - pad);
                    let height = self.card_height(text, hint.as_ref());
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
                    (
                        Frame {
                            rect,
                            text_right,
                            text,
                        },
                        vec![Slot {
                            rect: slot,
                            ..Default::default()
                        }],
                    )
                }
                Row::Action {
                    label,
                    hint,
                    buttons,
                } => {
                    let widths: Vec<(i32, i32)> = buttons
                        .iter()
                        .map(|button| {
                            let label_width = text_width(dc, self.fonts.body, &button.label);
                            (self.button_width(label_width, button), label_width)
                        })
                        .collect();
                    let total = widths.iter().map(|(width, _)| width).sum::<i32>()
                        + self.px(BUTTON_GAP) * (widths.len() as i32 - 1).max(0);
                    let text_right = if buttons.is_empty() {
                        right - pad
                    } else {
                        right - pad - total - self.px(CONTROL_GAP)
                    };
                    let text = self.measure_text(dc, label, hint.as_ref(), text_right - left - pad);
                    let height = self.card_height(text, hint.as_ref());
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
                        .map(|&(width, label_width)| {
                            let rect = RECT {
                                left: x,
                                top,
                                right: x + width,
                                bottom: top + self.px(BUTTON_HEIGHT),
                            };
                            x += width + self.px(BUTTON_GAP);
                            Slot { rect, label_width }
                        })
                        .collect();
                    (
                        Frame {
                            rect,
                            text_right,
                            text,
                        },
                        row_slots,
                    )
                }
            };
            frames.push(frame);
            slots.push(row_slots);
        }
        ReleaseDC(self.content, dc);
        self.frames = frames;
        self.slots = slots;
        self.content_left = left;
        self.content_right = right;
        self.content_height = y + self.px(28);
    }

    unsafe fn update_scrollbar(&mut self) {
        let client = self.content_client();
        let height = client.bottom - client.top;
        self.scroll = self
            .scroll
            .min((self.content_height - height).max(0))
            .max(0);
        if self.content_height <= height {
            self.scrollbar.hovered = false;
            self.scrollbar.dragging = false;
            let now = Instant::now();
            self.scrollbar.opacity = ScalarAnim::settled(0.0, now, SCROLLBAR_FADE_IN);
            self.scrollbar.expand = ScalarAnim::settled(0.0, now, SCROLLBAR_EXPAND_DURATION);
            let _ = KillTimer(self.hwnd, SCROLLBAR_FADE_TIMER);
        }
    }

    unsafe fn reveal_scrollbar(&mut self, now: Instant) {
        let client_h = self.content_client().bottom;
        if self.content_height <= client_h {
            return;
        }
        let changed = self.scrollbar.show(now);
        SetTimer(
            self.hwnd,
            SCROLLBAR_FADE_TIMER,
            SCROLLBAR_FADE_DELAY_MS,
            None,
        );
        if changed {
            self.update_pacer(now);
            self.invalidate_scrollbar();
        }
    }

    unsafe fn scrollbar_strip(&self) -> RECT {
        let client = self.content_client();
        RECT {
            left: (client.right - self.px(SCROLLBAR_HOT_WIDTH + 2)).max(0),
            top: 0,
            right: client.right,
            bottom: client.bottom,
        }
    }

    unsafe fn invalidate_scrollbar(&self) {
        let strip = self.scrollbar_strip();
        if has_area(strip) {
            let _ = InvalidateRect(self.content, Some(&strip), false);
        }
    }

    unsafe fn scrollbar_mouse_move(&mut self, x: i32, y: i32) {
        let client = self.content_client();
        let now = Instant::now();
        if self.scrollbar.dragging {
            if let Some(metrics) =
                scrollbar_thumb_metrics(client.bottom, self.content_height, self.scroll, self.dpi)
            {
                let travel =
                    (metrics.track_bottom - metrics.track_top - metrics.thumb_height).max(1.0);
                let max_scroll = (self.content_height - client.bottom).max(1) as f32;
                let dy = (y - self.scrollbar.drag_anchor_y) as f32;
                let target =
                    self.scrollbar.drag_start_scroll + (dy * max_scroll / travel).round() as i32;
                self.scroll_to(target);
            }
            return;
        }
        let in_zone = self.content_height > client.bottom
            && x >= client.right - self.px(SCROLLBAR_HOT_WIDTH)
            && x <= client.right;
        if self.scrollbar.hovered != in_zone {
            self.scrollbar.hovered = in_zone;
            let changed = self.scrollbar.set_expanded(in_zone, now);
            if in_zone {
                let _ = KillTimer(self.hwnd, SCROLLBAR_FADE_TIMER);
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.content,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
            } else {
                SetTimer(
                    self.hwnd,
                    SCROLLBAR_FADE_TIMER,
                    SCROLLBAR_FADE_DELAY_MS,
                    None,
                );
            }
            if changed {
                self.update_pacer(now);
                self.invalidate_scrollbar();
            }
        }
    }

    unsafe fn scrollbar_mouse_leave(&mut self) {
        if self.scrollbar.dragging || !self.scrollbar.hovered {
            return;
        }
        let now = Instant::now();
        self.scrollbar.hovered = false;
        let changed = self.scrollbar.set_expanded(false, now);
        SetTimer(
            self.hwnd,
            SCROLLBAR_FADE_TIMER,
            SCROLLBAR_FADE_DELAY_MS,
            None,
        );
        if changed {
            self.update_pacer(now);
            self.invalidate_scrollbar();
        }
    }

    unsafe fn scrollbar_mouse_down(&mut self, x: i32, y: i32) {
        let client = self.content_client();
        if self.transition.is_some()
            || self.content_height <= client.bottom
            || x < client.right - self.px(SCROLLBAR_HOT_WIDTH)
        {
            return;
        }
        let Some(metrics) =
            scrollbar_thumb_metrics(client.bottom, self.content_height, self.scroll, self.dpi)
        else {
            return;
        };
        let yf = y as f32;
        if yf >= metrics.thumb_top && yf <= metrics.thumb_top + metrics.thumb_height {
            let now = Instant::now();
            self.scrollbar.dragging = true;
            self.scrollbar.hovered = true;
            self.scrollbar.drag_anchor_y = y;
            self.scrollbar.drag_start_scroll = self.scroll;
            let _ = KillTimer(self.hwnd, SCROLLBAR_FADE_TIMER);
            let _ = self.scrollbar.set_expanded(true, now);
            SetCapture(self.content);
            self.update_pacer(now);
            self.invalidate_scrollbar();
        } else {
            // Track click: jump smoothly toward clicked position.
            let travel = (metrics.track_bottom - metrics.track_top - metrics.thumb_height).max(1.0);
            let target_top = (yf - metrics.thumb_height * 0.5).clamp(
                metrics.track_top,
                metrics.track_bottom - metrics.thumb_height,
            );
            let ratio = (target_top - metrics.track_top) / travel;
            let max_scroll = (self.content_height - client.bottom).max(0) as f32;
            self.scroll_to((ratio * max_scroll).round() as i32);
        }
    }

    unsafe fn scrollbar_end_drag(&mut self) {
        if !self.scrollbar.dragging {
            return;
        }
        let now = Instant::now();
        self.scrollbar.dragging = false;
        let _ = ReleaseCapture();
        let mut pt = POINT::default();
        let still_hot = GetCursorPos(&mut pt).is_ok() && {
            let _ = ScreenToClient(self.content, &mut pt);
            let client = self.content_client();
            pt.x >= client.right - self.px(SCROLLBAR_HOT_WIDTH)
                && pt.x <= client.right
                && pt.y >= 0
                && pt.y <= client.bottom
        };
        self.scrollbar.hovered = still_hot;
        let changed = self.scrollbar.set_expanded(still_hot, now);
        if !still_hot {
            SetTimer(
                self.hwnd,
                SCROLLBAR_FADE_TIMER,
                SCROLLBAR_FADE_DELAY_MS,
                None,
            );
        }
        if changed {
            self.update_pacer(now);
            self.invalidate_scrollbar();
        }
    }

    /// Position controls in the suspended layout or scroll transaction.
    unsafe fn place_controls(&self) {
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOREDRAW | SWP_DEFERERASE;
        for (controls, slots) in self.controls.iter().zip(&self.slots) {
            for (&control, slot) in controls.iter().zip(slots) {
                let rect = slot.rect;
                let _ = SetWindowPos(
                    control,
                    HWND::default(),
                    rect.left,
                    rect.top - self.scroll,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    flags,
                );
            }
        }
    }

    unsafe fn scroll_to(&mut self, position: i32) {
        if self.transition.is_some() {
            return;
        }
        let height = self.content_client().bottom;
        let position = position.min((self.content_height - height).max(0)).max(0);
        let now = Instant::now();
        if self.content_height > height {
            self.reveal_scrollbar(now);
        }
        if position != self.scroll {
            let _redraw = SuspendRedraw::new(self.content);
            self.scroll = position;
            self.update_scrollbar();
            self.place_controls();
            request_redraw();
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
        let height = self.content_client().bottom;
        let margin = self.px(16);
        if frame.rect.top - margin < self.scroll {
            self.scroll_to(frame.rect.top - margin);
        } else if frame.rect.bottom + margin > self.scroll + height {
            self.scroll_to(frame.rect.bottom + margin - height);
        }
    }

    unsafe fn change_page(&mut self, page: usize, clicked: Option<Instant>) {
        if page >= Page::ALL.len() || self.page == page || self.menu_open {
            return;
        }
        if self.transition_press.take().is_some() && GetCapture() == self.content {
            let _ = ReleaseCapture();
        }
        // If already transitioning between pages, carry over the current composed
        // visual frame and original underlying HWNDs without rebuilding intermediate controls.
        let carried = match self.transition.take() {
            Some(mut previous) if previous.is_page() => {
                let snapshot = previous.composed.clone_bitmap();
                drop(previous.trace.take());
                let old_rows = match previous.kind {
                    TransitionKind::Page { old_rows, .. } => old_rows,
                    TransitionKind::Theme { .. } => unreachable!(),
                };
                Some((snapshot, old_rows))
            }
            Some(previous) => {
                self.transition = Some(previous);
                self.finish_transition();
                None
            }
            None => None,
        };

        let Some((language, preference, next)) = lock_state()
            .as_ref()
            .map(|s| (s.language, s.settings_theme, rows(Page::ALL[page], s)))
        else {
            if let Some((_, old_rows)) = carried {
                self.install_page(&old_rows);
            }
            return;
        };

        let old_image = if let Some((snapshot, _)) = &carried {
            snapshot.as_ref().and_then(|bmp| bmp.clone_bitmap())
        } else {
            self.capture_page()
        };
        let restore_focus = IsChild(self.hwnd, GetFocus()).as_bool();

        let redraw = SuspendRedraw::new(self.content);
        self.page = page;
        self.scroll = 0;
        self.switches.borrow_mut().clear();
        self.apply_model(language, preference, SAVE_FAILED.load(Ordering::Relaxed));
        let replaced_rows = std::mem::replace(&mut self.rows, next);
        let old_rows = carried.map_or(replaced_rows, |(_, rows)| rows);
        self.measure_page();
        // Rasterize from the model and layout. Underlying HWNDs stay untouched
        // until the transition finishes so page switches start immediately.
        let new_image = self.capture_page();
        drop(redraw);

        let prepared = new_image.and_then(|new_image| {
            let bounds = new_image.bounds();
            let composed = Bitmap::new(bounds.right, bounds.bottom)?;
            let overlay = Overlay::new(self.content, bounds)?;
            Some((overlay, composed, new_image))
        });
        if let Some((overlay, composed, new_image)) = prepared {
            let start = Instant::now();
            let mut transition = Transition {
                overlay,
                composed,
                kind: TransitionKind::Page {
                    old_image,
                    new_image,
                    old_rows,
                    background: self.theme.background,
                    title_bottom: self.px(CONTENT_TOP + TITLE_HEIGHT),
                    distance: self.px(14),
                },
                start,
                drawn_elapsed: Duration::ZERO,
                trace: clicked.map(TransitionTrace::new),
            };
            let (drew, finished) = transition.draw(start);
            if let Some(trace) = transition.trace.as_mut() {
                trace.first_frame_us =
                    Some(Instant::now().duration_since(trace.clicked).as_micros());
            }
            self.transition = Some(transition);
            if !drew || finished {
                self.finish_transition();
            }
        } else {
            self.install_page(&old_rows);
        }
        if restore_focus {
            let _ = SetFocus(self.nav);
        }
        self.update_pacer(Instant::now());
        request_redraw();
    }

    unsafe fn install_page(&mut self, old_rows: &[Row]) {
        let _redraw = SuspendRedraw::new(self.content);
        self.sync_controls(old_rows, true);
        let now = Instant::now();
        let mut switches = self.switches.borrow_mut();
        for (row, state) in self.rows.iter().zip(switches.iter_mut()) {
            if let Row::Check { checked, .. } = row {
                *state = SwitchState::new(*checked, now);
            }
        }
        drop(switches);
        self.place_controls();
    }

    unsafe fn finish_transition(&mut self) {
        let mut reveal = false;
        if let Some(mut transition) = self.transition.take() {
            match &transition.kind {
                TransitionKind::Page { old_rows, .. } => {
                    let old_rows = old_rows.clone();
                    // Keep the final bitmap over the content as native HWNDs are
                    // created or reused, then reveal the new page in one redraw.
                    self.install_page(&old_rows);
                    reveal = true;
                }
                TransitionKind::Theme { .. } => {}
            }
            drop(transition.trace.take());
            drop(transition);
            request_redraw();
        }
        let now = Instant::now();
        if reveal {
            self.reveal_scrollbar(now);
        }
        self.update_pacer(now);
    }

    unsafe fn press_transition_control(&mut self, overlay: HWND, point: POINT) -> Option<HWND> {
        let transition = self
            .transition
            .as_ref()
            .filter(|t| t.overlay.0 == overlay)?;
        let TransitionKind::Page { distance, .. } = &transition.kind else {
            return None;
        };
        let (_, _, opacity) = page_alphas(transition.drawn_elapsed);
        if opacity == 0 {
            return None;
        }
        let slide = page_slide(transition.drawn_elapsed, *distance);
        let (row, column, area) =
            self.rows
                .iter()
                .zip(&self.slots)
                .enumerate()
                .find_map(|(row, (model, slots))| {
                    slots.iter().enumerate().find_map(|(column, slot)| {
                        let area = translated(slot.rect, 0, slide - self.scroll);
                        (model.control_enabled(column) && PtInRect(&area, point).as_bool())
                            .then_some((row, column, area))
                    })
                })?;

        self.finish_transition();
        let control = *self.controls.get(row)?.get(column)?;
        // Both the pressed pose and the settled pose belong to the same control.
        let hit_area = RECT {
            top: area.top - slide,
            ..area
        };
        self.transition_press = Some((control, hit_area));
        SetCapture(self.content);
        Some(control)
    }

    fn animation_active(&self, now: Instant) -> bool {
        self.transition.is_some()
            || self.scrollbar.is_active(now)
            || self.switches.borrow().iter().any(|s| s.is_active(now))
    }

    fn update_pacer(&self, now: Instant) {
        self.pacer.set_active(self.animation_active(now));
    }

    unsafe fn tick_animations(
        &mut self,
        redraw_controls: &mut Vec<HWND>,
        redraw_scrollbar: &mut bool,
    ) {
        self.pacer.clear_pending();
        let now = Instant::now();

        if let Some(transition) = self.transition.as_mut() {
            let (drew, finished) = transition.draw(now);
            if !drew || finished {
                self.finish_transition();
            }
        }

        if self.scrollbar.is_active(now) {
            *redraw_scrollbar = true;
        }

        if self.transition.is_none() {
            let switches = self.switches.borrow();
            for (state, controls) in switches.iter().zip(&self.controls) {
                if state.is_active(now) {
                    if let Some(&control) = controls.first() {
                        redraw_controls.push(control);
                    }
                }
            }
        }

        self.update_pacer(now);
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

    unsafe fn paint_frame(&self, target: HDC, dirty: RECT) {
        let client = self.client();
        let dirty = intersection(client, dirty);
        if !has_area(dirty) {
            return;
        }
        let mut buffer = self.page_buffer.borrow_mut();
        if buffer
            .as_ref()
            .is_none_or(|bitmap| bitmap.bounds() != client)
        {
            *buffer = Bitmap::new(client.right, client.bottom);
        }
        let drawing = Drawing::new(target, buffer.as_mut());
        let dc = drawing.dc;
        fill_rect(dc, dirty, self.theme.background);
        let caption_rect = RECT {
            bottom: self.px(CAPTION_HEIGHT),
            ..client
        };
        if has_area(intersection(caption_rect, dirty)) {
            self.paint_caption(dc, client.right);
        }
        if let Some(buffer) = buffer.as_ref() {
            buffer.blit(target, dirty, dirty.left, dirty.top);
        }
    }

    unsafe fn paint_content(&self, target: HDC, dirty: RECT) {
        if self.transition.as_ref().is_some_and(Transition::is_page) {
            return;
        }
        let client = self.content_client();
        let dirty = intersection(client, dirty);
        if !has_area(dirty) {
            return;
        }
        let mut buffer = self.content_buffer.borrow_mut();
        if buffer
            .as_ref()
            .is_none_or(|bitmap| bitmap.bounds() != client)
        {
            *buffer = Bitmap::new(client.right, client.bottom);
        }
        let drawing = Drawing::new(target, buffer.as_mut());
        fill_rect(drawing.dc, dirty, self.theme.background);
        self.paint_page(drawing, dirty, POINT::default());
        if let Some(buffer) = buffer.as_mut() {
            self.paint_scrollbar(Drawing::for_bitmap(buffer), client, dirty, Instant::now());
            buffer.blit(target, dirty, dirty.left, dirty.top);
        }
    }

    unsafe fn paint_scrollbar(
        &self,
        mut drawing: Drawing<'_>,
        client: RECT,
        dirty: RECT,
        now: Instant,
    ) {
        let opacity = self.scrollbar.opacity.value(now);
        if opacity <= 0.01 {
            return;
        }
        let Some(metrics) =
            scrollbar_thumb_metrics(client.bottom, self.content_height, self.scroll, self.dpi)
        else {
            return;
        };
        let expand = self.scrollbar.expand.value(now);
        let Some(mut canvas) = drawing.canvas(dirty) else {
            return;
        };

        // Subtle pill track fades in when hovered or dragging.
        if expand > 0.02 {
            let track_w = self.pxf(12.0);
            let track_right = (client.right - self.px(2)) as f32;
            let track_left = track_right - track_w;
            let track_alpha =
                (expand * opacity * if self.theme.dark { 22.0 } else { 18.0 }).round() as u32;
            let track_rgb = if self.theme.dark {
                0xFF_FFFF
            } else {
                0x00_0000
            };
            canvas.pill(
                track_left,
                metrics.track_top - self.pxf(2.0),
                track_right,
                metrics.track_bottom + self.pxf(2.0),
                track_w * 0.5,
                (track_alpha << 24) | track_rgb,
            );
        }

        let thumb_w = self.pxf(3.0 + 3.0 * expand);
        let inset_right = self.pxf(5.0);
        let thumb_right = client.right as f32 - inset_right;
        let thumb_left = thumb_right - thumb_w;
        let base_alpha = if self.scrollbar.dragging {
            225.0
        } else {
            135.0 + 65.0 * expand
        };
        let thumb_alpha = (base_alpha * opacity).round().clamp(0.0, 255.0) as u32;
        let thumb_rgb = if self.theme.dark {
            0xB8_B8B8
        } else {
            0x6E_6E6E
        };
        canvas.pill(
            thumb_left,
            metrics.thumb_top,
            thumb_right,
            metrics.thumb_top + metrics.thumb_height,
            thumb_w * 0.5,
            (thumb_alpha << 24) | thumb_rgb,
        );
    }

    unsafe fn paint_page(&self, mut drawing: Drawing<'_>, visible: RECT, origin: POINT) {
        let dc = drawing.dc;
        let theme = self.theme;
        let x_offset = origin.x;
        let scroll = self.scroll - origin.y;
        // Shapes first; GDI text follows once the canvas is released.
        if let Some(mut canvas) = drawing.canvas(visible) {
            let radius = self.pxf(CARD_RADIUS);
            if let Some(banner) = self.banner {
                canvas.card(
                    translated(banner, x_offset, -scroll),
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
                    let rect = translated(frame.rect, x_offset, -scroll);
                    if has_area(intersection(rect, visible)) {
                        canvas.card(rect, radius, theme.card, theme.card_border);
                    }
                }
            }
        }

        let language = self.language.unwrap_or(LanguageId::English);
        let title = page_titles(language)[self.page];
        draw_text(
            dc,
            self.fonts.title,
            theme.text,
            title,
            RECT {
                left: self.content_left + x_offset,
                top: self.px(CONTENT_TOP) - scroll,
                right: self.content_right + x_offset,
                bottom: self.px(CONTENT_TOP + TITLE_HEIGHT) - scroll,
            },
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );

        if let Some(banner) = self.banner {
            let banner = translated(banner, x_offset, -scroll);
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
            let rect = translated(frame.rect, x_offset, -scroll);
            if !has_area(intersection(rect, visible)) {
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
                    self.draw_preview(
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
                        right: frame.text_right + x_offset,
                        ..rect
                    },
                    frame.text,
                    label,
                    hint.as_ref(),
                    *enabled,
                ),
                Row::Action { label, hint, .. } => self.card_text(
                    dc,
                    RECT {
                        left: rect.left + self.px(CARD_PAD),
                        right: frame.text_right + x_offset,
                        ..rect
                    },
                    frame.text,
                    label,
                    hint.as_ref(),
                    true,
                ),
                Row::Check { .. } => {}
            }
        }
    }

    unsafe fn paint_caption(&self, dc: HDC, client_width: i32) {
        let cap_h = self.px(CAPTION_HEIGHT);
        let icon_size = self.px(20);
        let icon_top = (cap_h - icon_size) / 2;
        if !self.icon.is_invalid() {
            let _ = DrawIconEx(
                dc,
                self.px(16),
                icon_top,
                self.icon,
                icon_size,
                icon_size,
                0,
                HBRUSH::default(),
                DI_NORMAL,
            );
        }
        let title_color = if self.window_active {
            self.theme.text
        } else {
            self.theme.secondary
        };
        let buttons_left = client_width - 3 * self.px(CAPTION_BTN_WIDTH);
        draw_text(
            dc,
            self.fonts.caption_title,
            title_color,
            "Codex Usage",
            RECT {
                left: self.px(46),
                top: 0,
                right: buttons_left.max(self.px(46)),
                bottom: cap_h,
            },
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );

        let maximized = IsZoomed(self.hwnd).as_bool();
        for button in [
            CaptionButton::Minimize,
            CaptionButton::Maximize,
            CaptionButton::Close,
        ] {
            let rect = caption_button_rect(client_width, self.dpi, button);
            let hovered = self.caption_hover == Some(button);
            let pressed = self.caption_pressed == Some(button);
            let (bg, fg) = match (button, pressed, hovered) {
                (CaptionButton::Close, true, _) => (Some(self.theme.close_pressed), 0xFF_FFFF),
                (CaptionButton::Close, false, true) => (Some(self.theme.close_hover), 0xFF_FFFF),
                (_, true, _) => (Some(self.theme.caption_pressed), self.theme.text),
                (_, false, true) => (Some(self.theme.caption_hover), self.theme.text),
                (_, false, false) => (
                    None,
                    if self.window_active {
                        self.theme.text
                    } else {
                        self.theme.disabled
                    },
                ),
            };
            if let Some(bg) = bg {
                fill_rect(dc, rect, bg);
            }
            let glyph = match button {
                CaptionButton::Minimize => GLYPH_MINIMIZE,
                CaptionButton::Maximize if maximized => GLYPH_RESTORE,
                CaptionButton::Maximize => GLYPH_MAXIMIZE,
                CaptionButton::Close => GLYPH_CLOSE,
            };
            if self.fonts.has_icons {
                draw_text(
                    dc,
                    self.fonts.caption_icons,
                    fg,
                    &glyph.to_string(),
                    rect,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );
            } else {
                let fallback = match button {
                    CaptionButton::Minimize => "−",
                    CaptionButton::Maximize if maximized => "❐",
                    CaptionButton::Maximize => "□",
                    CaptionButton::Close => "×",
                };
                draw_text(
                    dc,
                    self.fonts.body,
                    fg,
                    fallback,
                    rect,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );
            }
        }
    }

    unsafe fn invalidate_caption(&self) {
        let client = self.client();
        let rect = RECT {
            bottom: self.px(CAPTION_HEIGHT),
            ..client
        };
        let _ = InvalidateRect(self.hwnd, Some(&rect), false);
    }

    unsafe fn draw_preview(
        &self,
        dc: HDC,
        area: RECT,
        appearance: Appearance,
        system_dark: bool,
        language: LanguageId,
    ) {
        let available = area.right - area.left;
        if available <= 0 {
            return;
        }
        let key = PreviewKey {
            appearance,
            system_dark,
            language,
            widget_dpi: CURRENT_DPI.load(Ordering::Relaxed),
        };
        let mut cache = self.preview.borrow_mut();
        if cache.as_ref().is_none_or(|(cached, _)| *cached != key) {
            let width = total_widget_width_for(3, language, appearance);
            let height = sc(WIDGET_HEIGHT);
            *cache = Bitmap::new(width, height).map(|bitmap| {
                appearance_preview::paint_sample(
                    bitmap.dc,
                    width,
                    height,
                    appearance,
                    system_dark,
                    language,
                );
                (key, bitmap)
            });
        }
        let Some((_, bitmap)) = cache.as_ref() else {
            return;
        };
        let width = bitmap.width.min(available);
        let height = bitmap.height * width / bitmap.width;
        let left = area.left + (available - width) / 2;
        let top = area.top + (area.bottom - area.top - height) / 2;
        if width == bitmap.width {
            bitmap.blit(
                dc,
                RECT {
                    left,
                    top,
                    right: left + width,
                    bottom: top + height,
                },
                0,
                0,
            );
        } else {
            let mode = SetStretchBltMode(dc, HALFTONE);
            let _ = StretchBlt(
                dc,
                left,
                top,
                width,
                height,
                bitmap.dc,
                0,
                0,
                bitmap.width,
                bitmap.height,
                SRCCOPY,
            );
            let _ = SetStretchBltMode(dc, STRETCH_BLT_MODE(mode));
        }
    }

    unsafe fn capture_page(&self) -> Option<Bitmap> {
        let client = self.content_client();
        let mut bitmap = Bitmap::new(client.right, client.bottom)?;
        let bounds = bitmap.bounds();
        fill_rect(bitmap.dc, bounds, self.theme.background);
        self.paint_page(Drawing::for_bitmap(&mut bitmap), bounds, POINT::default());
        let now = Instant::now();
        for (row, (model, slots)) in self.rows.iter().zip(&self.slots).enumerate() {
            for (column, slot) in slots.iter().enumerate() {
                let rect = translated(slot.rect, 0, -self.scroll);
                if !has_area(intersection(bounds, rect)) {
                    continue;
                }
                let state = ControlState {
                    disabled: !model.control_enabled(column),
                    ..Default::default()
                };
                self.draw_control_at(
                    Drawing::for_bitmap(&mut bitmap),
                    rect,
                    row,
                    column,
                    state,
                    now,
                    false,
                );
            }
        }
        Some(bitmap)
    }

    unsafe fn capture_window(&self) -> Option<Bitmap> {
        let client = self.client();
        let mut bitmap = Bitmap::new(client.right, client.bottom)?;
        let bounds = bitmap.bounds();
        fill_rect(bitmap.dc, bounds, self.theme.background);
        self.paint_caption(bitmap.dc, client.right);

        // Paint navigation sidebar directly into the snapshot.
        let nav_x = self.px(8);
        let nav_y = self.px(NAV_TOP);
        let nav_w = self.px(NAV_WIDTH - 16);
        let item_h = self.px(NAV_ITEM);
        let titles = page_titles(self.language.unwrap_or(LanguageId::English));
        for (index, &title) in titles.iter().enumerate() {
            let item_rect = RECT {
                left: nav_x,
                top: nav_y + index as i32 * item_h,
                right: nav_x + nav_w,
                bottom: nav_y + (index as i32 + 1) * item_h,
            };
            self.paint_nav_item(
                Drawing::for_bitmap(&mut bitmap),
                item_rect,
                index,
                title,
                index == self.page,
                false,
                false,
                false,
            );
        }

        if let Some(mut page) = self.capture_page() {
            let page_bounds = page.bounds();
            self.paint_scrollbar(
                Drawing::for_bitmap(&mut page),
                page_bounds,
                page_bounds,
                Instant::now(),
            );
            let nav_w = self.px(NAV_WIDTH);
            let cap_h = self.px(CAPTION_HEIGHT);
            let _ = page.blit(
                bitmap.dc,
                RECT {
                    left: nav_w,
                    top: cap_h,
                    right: (nav_w + page.width).min(client.right),
                    bottom: (cap_h + page.height).min(client.bottom),
                },
                0,
                0,
            );
        }
        Some(bitmap)
    }

    /// Label with an optional hint below it, centered vertically in `area`.
    unsafe fn card_text(
        &self,
        dc: HDC,
        area: RECT,
        metrics: TextMetrics,
        label: &str,
        hint: Option<&Hint>,
        enabled: bool,
    ) {
        let TextMetrics {
            label: label_height,
            hint: hint_height,
        } = metrics;
        let gap = if hint.is_some() { self.px(3) } else { 0 };
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
        if self.transition.is_some() {
            return;
        }
        let control = draw.hdr.hwndFrom;
        let Some((row, column)) = self.locate(draw.hdr.idFrom as u16, control) else {
            return;
        };
        let flags = draw.uItemState;
        let state = ControlState {
            hot: flags.0 & CDIS_HOT.0 != 0,
            pressed: flags.0 & CDIS_SELECTED.0 != 0,
            focused: flags.0 & CDIS_FOCUS.0 != 0 && flags.0 & CDIS_SHOWKEYBOARDCUES.0 != 0,
            disabled: flags.0 & CDIS_DISABLED.0 != 0 || !IsWindowEnabled(control).as_bool(),
        };
        let now = Instant::now();
        self.paint_control_buffered(draw.hdc, draw.rc, |drawing, area| {
            self.draw_control_at(drawing, area, row, column, state, now, true);
        });
    }

    unsafe fn paint_control_buffered(
        &self,
        target: HDC,
        area: RECT,
        draw: impl FnOnce(Drawing<'_>, RECT),
    ) {
        // A scrolled control can be larger than its visible portion.
        let mut clip = RECT::default();
        GetClipBox(target, &mut clip);
        let clip = intersection(clip, area);
        if !has_area(clip) {
            return;
        }
        let mut buffer = self.control_buffer.borrow_mut();
        let width = clip.right - clip.left;
        let height = clip.bottom - clip.top;
        let (old_width, old_height) = buffer.as_ref().map_or((0, 0), |b| (b.width, b.height));
        if width > old_width || height > old_height {
            *buffer = Bitmap::new(width.max(old_width), height.max(old_height));
        }
        if let Some(buffer) = buffer.as_mut() {
            draw(
                Drawing::for_bitmap(buffer),
                translated(area, -clip.left, -clip.top),
            );
            buffer.blit(target, clip, 0, 0);
        } else {
            draw(Drawing::new(target, None), area);
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_control_at(
        &self,
        drawing: Drawing<'_>,
        area: RECT,
        row: usize,
        column: usize,
        state: ControlState,
        now: Instant,
        interactive: bool,
    ) {
        let (Some(model), Some(frame)) = (self.rows.get(row), self.frames.get(row)) else {
            return;
        };
        match model {
            Row::Check {
                item,
                checked,
                hint,
                ..
            } => {
                let (progress, hover_t, press_t) = {
                    let mut switches = self.switches.borrow_mut();
                    if let Some(sw) = switches.get_mut(row) {
                        if interactive
                            && sw.update_interaction(state.hot, state.pressed, state.disabled, now)
                        {
                            self.pacer.set_active(true);
                        }
                        (
                            sw.checked.value(now),
                            if interactive {
                                sw.hover.value(now)
                            } else {
                                0.0
                            },
                            if interactive {
                                sw.press.value(now)
                            } else {
                                0.0
                            },
                        )
                    } else {
                        (f32::from(u8::from(*checked)), 0.0, 0.0)
                    }
                };
                self.draw_switch_card(
                    drawing,
                    area,
                    *frame,
                    &item.label,
                    hint.as_ref(),
                    progress,
                    hover_t,
                    press_t,
                    state,
                );
            }
            Row::Choice {
                items, selected, ..
            } => {
                if let Some(item) = items.get(*selected) {
                    self.draw_button(drawing, area, item, None, state);
                }
            }
            Row::Action { buttons, .. } => {
                let Some(item) = buttons.get(column) else {
                    return;
                };
                self.draw_button(
                    drawing,
                    area,
                    item,
                    Some(self.slots[row][column].label_width),
                    state,
                );
            }
            Row::Section(_) | Row::Note(_) | Row::Preview { .. } => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_switch_card(
        &self,
        mut drawing: Drawing<'_>,
        rect: RECT,
        frame: Frame,
        label: &str,
        hint: Option<&Hint>,
        progress: f32,
        hover_t: f32,
        press_t: f32,
        state: ControlState,
    ) {
        let dc = drawing.dc;
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
        let top = rect.top + (rect.bottom - rect.top - toggle_height) / 2;
        let toggle = RECT {
            left: rect.right - self.px(CARD_PAD) - toggle_width,
            top,
            right: rect.right - self.px(CARD_PAD),
            bottom: top + toggle_height,
        };
        if let Some(mut canvas) = drawing.canvas(rect) {
            let radius = self.pxf(CARD_RADIUS);
            canvas.card(rect, radius, fill, theme.card_border);
            let pill = toggle_height as f32 / 2.0;
            let knob_h = self.pxf(12.0 + 2.0 * hover_t.max(press_t));
            let stretch = self.pxf(3.5 * press_t);
            let knob_w = knob_h + stretch;
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
            canvas.fill(toggle, pill, opaque(track));
            canvas.stroke(toggle, pill, opaque(stroke), self.pxf(1.0));
            let travel = (toggle.right - toggle.left - toggle_height) as f32;
            let center_x = toggle.left as f32 + pill + travel * progress;
            // Anchor stretch inward so the knob stays inside the track at both ends.
            let min_left = toggle.left as f32 + (toggle_height as f32 - knob_h) * 0.5;
            let max_right = toggle.right as f32 - (toggle_height as f32 - knob_h) * 0.5;
            let knob_left =
                (center_x - knob_w * 0.5).clamp(min_left, (max_right - knob_w).max(min_left));
            let knob_right = (knob_left + knob_w).min(max_right);
            canvas.pill(
                knob_left,
                center_y - knob_h * 0.5,
                knob_right,
                center_y + knob_h * 0.5,
                knob_h * 0.5,
                opaque(mix_color(off_knob, on_knob, progress)),
            );
            if state.focused {
                canvas.stroke(rect, radius, opaque(theme.text), self.pxf(2.0));
            }
        }
        self.card_text(
            dc,
            RECT {
                left: rect.left + self.px(CARD_PAD),
                right: rect.left + frame.text_right - frame.rect.left,
                ..rect
            },
            frame.text,
            label,
            hint,
            !state.disabled,
        );
    }

    unsafe fn draw_button(
        &self,
        mut drawing: Drawing<'_>,
        rect: RECT,
        item: &OptionItem,
        label_width: Option<i32>,
        state: ControlState,
    ) {
        let dc = drawing.dc;
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
        let elevation = if state.disabled || state.pressed {
            theme.control_border
        } else {
            theme.control_elevation
        };
        let radius = self.pxf(CONTROL_RADIUS);
        let padding = self.px(12);
        let swatch_size = self.px(14);
        let mut text = inset(rect, padding, 0);
        if let Some(label_width) = label_width {
            let content_width = label_width + item.swatch.map_or(0, |_| swatch_size + self.px(8));
            text.left = rect.left + (rect.right - rect.left - content_width) / 2;
        }
        if let Some(mut canvas) = drawing.canvas(rect) {
            canvas.control(
                rect,
                radius,
                fill,
                theme.control_border,
                elevation,
                theme.dark,
            );
            if let Some(color) = item.swatch {
                let center_y = (rect.top + rect.bottom) as f32 / 2.0;
                let diameter = swatch_size as f32;
                canvas.circle(
                    text.left as f32 + diameter / 2.0,
                    center_y,
                    diameter,
                    opaque(theme.control_border),
                );
                canvas.circle(
                    text.left as f32 + diameter / 2.0,
                    center_y,
                    diameter - self.pxf(2.0),
                    opaque(color),
                );
                text.left += swatch_size + self.px(8);
            }
            if state.focused {
                canvas.stroke(rect, radius, opaque(theme.text), self.pxf(2.0));
            }
        }
        let color = if state.disabled {
            theme.disabled
        } else if state.pressed {
            theme.secondary
        } else {
            theme.text
        };
        if label_width.is_none() {
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
                &item.label,
                text,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        } else {
            draw_text(
                dc,
                self.fonts.body,
                color,
                &item.label,
                RECT {
                    right: rect.right - padding,
                    ..text
                },
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn paint_nav_item(
        &self,
        mut drawing: Drawing<'_>,
        rect: RECT,
        index: usize,
        title: &str,
        selected: bool,
        hot: bool,
        pressed: bool,
        focused: bool,
    ) {
        let dc = drawing.dc;
        let theme = self.theme;
        fill_rect(dc, rect, theme.background);
        let pill = inset(rect, self.px(4), self.px(2));

        if let Some(mut canvas) = drawing.canvas(rect) {
            let radius = self.pxf(CONTROL_RADIUS);
            if selected || hot || pressed {
                let bg = if pressed {
                    theme.nav_pressed
                } else if selected {
                    theme.nav_selected
                } else {
                    theme.nav_hover
                };
                canvas.fill(pill, radius, opaque(bg));
            }
            if focused {
                canvas.stroke(pill, radius, opaque(theme.text), self.pxf(2.0));
            }
        }
        let mut text = RECT {
            left: pill.left + self.px(14),
            right: pill.right - self.px(8),
            ..pill
        };
        let fg = if pressed { theme.secondary } else { theme.text };
        if self.fonts.has_icons {
            if let Some(glyph) = PAGE_GLYPHS.get(index) {
                draw_text(
                    dc,
                    self.fonts.icons,
                    fg,
                    &glyph.to_string(),
                    text,
                    DT_SINGLELINE | DT_VCENTER,
                );
            }
            text.left += self.px(30);
        }
        draw_text(
            dc,
            if selected {
                self.fonts.strong
            } else {
                self.fonts.body
            },
            fg,
            title,
            text,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
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
        let pressed = NAV_PRESSED.with(Cell::get) == index as i32;
        self.paint_control_buffered(item.hDC, item.rcItem, |drawing, rect| {
            self.paint_nav_item(drawing, rect, index, title, selected, hot, pressed, focused);
        });
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.pacer.stop();
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
        WS_CHILD | WS_CLIPSIBLINGS | WINDOW_STYLE(style),
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
            hint,
            ..
        } => {
            set_text(controls[0], &accessible_text(&item.label, hint.as_ref()));
            let _ = SendMessageW(
                controls[0],
                BM_SETCHECK,
                WPARAM(usize::from(*checked)),
                LPARAM(0),
            );
        }
        Row::Choice {
            label,
            items,
            selected,
            ..
        } => {
            let value = items.get(*selected).map_or("", |item| item.label.as_str());
            set_text(controls[0], &format!("{label}: {value}"));
        }
        Row::Action { buttons, .. } => {
            for (item, &control) in buttons.iter().zip(controls) {
                set_text(control, &item.label);
            }
        }
        Row::Section(_) | Row::Note(_) | Row::Preview { .. } => {}
    }
    for (column, &control) in controls.iter().enumerate() {
        let _ = EnableWindow(control, row.control_enabled(column));
    }
}

fn request_redraw() {
    REDRAW_PENDING.with(|pending| pending.set(true));
}

fn with_panel<T>(f: impl FnOnce(&mut Panel) -> T) -> Option<T> {
    let (hwnd, nav, content, page_transitioning, theme_transitioning, result) =
        PANEL.with(|state| {
            let mut state = state.try_borrow_mut().ok()?;
            let panel = state.as_mut()?;
            let result = f(panel);
            let page_t = panel.transition.as_ref().is_some_and(Transition::is_page);
            let theme_t = panel.transition.as_ref().is_some_and(|t| !t.is_page());
            Some((
                panel.hwnd,
                panel.nav,
                panel.content,
                page_t,
                theme_t,
                result,
            ))
        })?;
    if REDRAW_PENDING.with(|pending| pending.replace(false)) {
        unsafe {
            let flags = RDW_INVALIDATE | RDW_NOERASE | RDW_UPDATENOW;
            if theme_transitioning {
                // Theme cross-fade overlay covers the full client area.
            } else if page_transitioning {
                // Keep the caption bar and navigation sidebar crisp while the page overlay animates.
                let _ = RedrawWindow(hwnd, None, HRGN::default(), flags | RDW_NOCHILDREN);
                let _ = RedrawWindow(nav, None, HRGN::default(), flags);
            } else {
                let _ = RedrawWindow(hwnd, None, HRGN::default(), flags | RDW_NOCHILDREN);
                let _ = RedrawWindow(nav, None, HRGN::default(), flags);
                let _ = RedrawWindow(
                    content,
                    None,
                    HRGN::default(),
                    flags | RDW_ALLCHILDREN | RDW_FRAME,
                );
            }
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
    WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN
}

unsafe fn outer_size(width: i32, height: i32, dpi: u32) -> (i32, i32) {
    let mut rect = RECT {
        right: scale(width, dpi),
        bottom: scale(height, dpi),
        ..Default::default()
    };
    let _ = AdjustWindowRectExForDpi(&mut rect, window_style(), false, WINDOW_EX_STYLE(0), dpi);
    // The top caption is merged into the client area, so only side/bottom frames add outer size.
    (rect.right - rect.left, rect.bottom)
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
    let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
    let _ = RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(settings_proc),
        hInstance: instance,
        lpszClassName: CLASS,
        hCursor: cursor,
        hIcon: LoadIconW(instance, PCWSTR(1usize as *const u16)).unwrap_or_default(),
        ..Default::default()
    });
    let _ = RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(content_proc),
        hInstance: instance,
        lpszClassName: CONTENT_CLASS,
        hCursor: cursor,
        ..Default::default()
    });
    let _ = RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(overlay_proc),
        hInstance: instance,
        lpszClassName: OVERLAY_CLASS,
        hCursor: cursor,
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
    // Preserve native Windows 11 drop shadow and rounded corners after removing the top caption.
    let margins = MARGINS {
        cyTopHeight: 1,
        ..Default::default()
    };
    let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    let _ = SetWindowPos(
        hwnd,
        HWND::default(),
        0,
        0,
        0,
        0,
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
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

fn caption_button_from_ht(ht: usize) -> Option<CaptionButton> {
    match ht as u32 {
        HTMINBUTTON => Some(CaptionButton::Minimize),
        HTMAXBUTTON => Some(CaptionButton::Maximize),
        HTCLOSE => Some(CaptionButton::Close),
        _ => None,
    }
}

unsafe fn track_nc_leave(hwnd: HWND) {
    let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE | TME_NONCLIENT,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = TrackMouseEvent(&mut track);
}

unsafe fn show_system_menu(hwnd: HWND, x: i32, y: i32) {
    let menu = GetSystemMenu(hwnd, false);
    if menu.is_invalid() {
        return;
    }
    let maximized = IsZoomed(hwnd).as_bool();
    let _ = EnableMenuItem(
        menu,
        SC_RESTORE,
        MF_BYCOMMAND | if maximized { MF_ENABLED } else { MF_GRAYED },
    );
    let _ = EnableMenuItem(
        menu,
        SC_MOVE,
        MF_BYCOMMAND | if maximized { MF_GRAYED } else { MF_ENABLED },
    );
    let _ = EnableMenuItem(
        menu,
        SC_SIZE,
        MF_BYCOMMAND | if maximized { MF_GRAYED } else { MF_ENABLED },
    );
    let _ = EnableMenuItem(
        menu,
        SC_MAXIMIZE,
        MF_BYCOMMAND | if maximized { MF_GRAYED } else { MF_ENABLED },
    );
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
        x,
        y,
        0,
        hwnd,
        None,
    )
    .0;
    if command > 0 {
        let _ = PostMessageW(hwnd, WM_SYSCOMMAND, WPARAM(command as usize), LPARAM(0));
    }
}

unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCALCSIZE if wparam.0 != 0 => {
            let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
            let original_top = params.rgrc[0].top;
            let _ = DefWindowProcW(hwnd, msg, wparam, lparam);
            if IsZoomed(hwnd).as_bool() {
                let dpi = GetDpiForWindow(hwnd).max(96);
                let frame = GetSystemMetricsForDpi(SM_CYFRAME, dpi)
                    + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                params.rgrc[0].top = original_top + frame;
            } else {
                params.rgrc[0].top = original_top;
            }
            return LRESULT(0);
        }
        WM_NCHITTEST => {
            let mut pt = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            let _ = ScreenToClient(hwnd, &mut pt);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let dpi = GetDpiForWindow(hwnd).max(96);
            let hit = hit_test_caption(client, pt, dpi, IsZoomed(hwnd).as_bool());
            if hit != HTCLIENT as i32 {
                return LRESULT(hit as isize);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_NCMOUSEMOVE => {
            let hovered = caption_button_from_ht(wparam.0);
            let changed = with_panel(|panel| {
                if panel.caption_hover != hovered {
                    panel.caption_hover = hovered;
                    panel.invalidate_caption();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            if changed && hovered.is_some() {
                track_nc_leave(hwnd);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_NCMOUSELEAVE => {
            with_panel(|panel| {
                if panel.caption_hover.is_some() || panel.caption_pressed.is_some() {
                    panel.caption_hover = None;
                    panel.caption_pressed = None;
                    panel.invalidate_caption();
                }
            });
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            with_panel(|panel| {
                if panel.caption_hover.is_some() {
                    panel.caption_hover = None;
                    panel.invalidate_caption();
                }
            });
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_NCLBUTTONDOWN => {
            if let Some(button) = caption_button_from_ht(wparam.0) {
                with_panel(|panel| {
                    panel.caption_hover = Some(button);
                    panel.caption_pressed = Some(button);
                    panel.invalidate_caption();
                });
                return LRESULT(0);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_NCLBUTTONUP => {
            let released = caption_button_from_ht(wparam.0);
            let action = with_panel(|panel| {
                let pressed = panel.caption_pressed.take();
                if pressed.is_some() {
                    panel.invalidate_caption();
                }
                pressed.filter(|&btn| Some(btn) == released)
            })
            .flatten();
            if let Some(button) = action {
                let command = match button {
                    CaptionButton::Minimize => SC_MINIMIZE,
                    CaptionButton::Maximize if IsZoomed(hwnd).as_bool() => SC_RESTORE,
                    CaptionButton::Maximize => SC_MAXIMIZE,
                    CaptionButton::Close => SC_CLOSE,
                };
                let _ = PostMessageW(hwnd, WM_SYSCOMMAND, WPARAM(command as usize), LPARAM(0));
                return LRESULT(0);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_NCRBUTTONUP if wparam.0 == HTCAPTION as usize => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            show_system_menu(hwnd, x, y);
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) != 0;
            with_panel(|panel| {
                // Keep the settings title bar looking active while our own dropdown flyout has focus.
                panel.window_active = active || panel.menu_open;
                if !active {
                    panel.caption_hover = None;
                    panel.caption_pressed = None;
                }
                panel.invalidate_caption();
            });
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_COMMAND => {
            let id = wparam.0 as u16;
            let code = (wparam.0 >> 16) as u16;
            let control = HWND(lparam.0 as *mut _);
            if id == IDCANCEL.0 as u16 {
                let _ = DestroyWindow(hwnd);
            } else if id == NAV && code == LBN_SELCHANGE as u16 {
                let clicked = diagnose::is_enabled().then(Instant::now);
                let page = SendMessageW(control, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                with_panel(|panel| panel.change_page(page as usize, clicked));
            } else if code == BN_CLICKED as u16 {
                let request = with_panel(|panel| {
                    panel.finish_transition();
                    let request = panel.choice_request(id, control);
                    panel.menu_open = request.is_some();
                    request.map(|request| (request, panel.theme, panel.fonts, panel.dpi))
                })
                .flatten();
                if let Some(((items, selected, anchor), theme, fonts, dpi)) = request {
                    // The dropdown runs a nested message loop: never hold the panel here.
                    let picked =
                        dropdown::pick(control, &items, selected, anchor, theme, fonts, dpi);
                    with_panel(|panel| {
                        panel.menu_open = false;
                        panel.window_active = true;
                        panel.invalidate_caption();
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
                with_panel(|panel| {
                    panel.finish_transition();
                    panel.sync(true);
                });
            }
        }
        ENSURE_VISIBLE => {
            let control = HWND(wparam.0 as *mut _);
            if control == GetFocus() {
                with_panel(|panel| {
                    panel.finish_transition();
                    panel.ensure_visible(control);
                });
            }
        }
        ANIMATION_TICK => {
            let mut redraw_controls = Vec::new();
            let mut redraw_scrollbar = false;
            let handles = with_panel(|panel| {
                if panel.hwnd == hwnd {
                    panel.tick_animations(&mut redraw_controls, &mut redraw_scrollbar);
                }
                (panel.content, panel.scrollbar_strip())
            });
            if let Some((content, strip)) = handles {
                if redraw_scrollbar && has_area(strip) {
                    let _ = RedrawWindow(
                        content,
                        Some(&strip),
                        HRGN::default(),
                        RDW_INVALIDATE | RDW_NOCHILDREN | RDW_NOERASE | RDW_UPDATENOW,
                    );
                }
            }
            for control in redraw_controls {
                let _ = RedrawWindow(
                    control,
                    None,
                    HRGN::default(),
                    RDW_INVALIDATE | RDW_NOCHILDREN | RDW_NOERASE | RDW_UPDATENOW,
                );
            }
        }
        WM_TIMER if wparam.0 == REFRESH_TIMER => {
            with_panel(|panel| panel.sync(false));
        }
        WM_TIMER if wparam.0 == SCROLLBAR_FADE_TIMER => {
            let _ = KillTimer(hwnd, SCROLLBAR_FADE_TIMER);
            with_panel(|panel| {
                let now = Instant::now();
                if panel.scrollbar.fade_out(now) {
                    panel.update_pacer(now);
                    panel.invalidate_scrollbar();
                }
            });
        }
        WM_SIZE => {
            with_panel(|panel| {
                panel.finish_transition();
                panel.layout();
            });
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            let (width, height) = outer_size(MIN_WIDTH, MIN_HEIGHT, GetDpiForWindow(hwnd).max(96));
            info.ptMinTrackSize = POINT {
                x: width,
                y: height,
            };
        }
        WM_MOUSEWHEEL => {
            let delta = i32::from((wparam.0 >> 16) as u16 as i16);
            let content = with_panel(|panel| {
                let step = panel.px(WHEEL_STEP);
                panel.scroll_to(panel.scroll - delta * step / 120);
                panel.content
            });
            if let Some(content) = content {
                let _ = UpdateWindow(content);
            }
        }
        WM_DPICHANGED => {
            let dpi = wparam.0 as u16 as u32;
            let suggested = *(lparam.0 as *const RECT);
            with_panel(|panel| {
                panel.finish_transition();
                let _redraw = SuspendRedraw::new(hwnd);
                let language = panel.language.unwrap_or(LanguageId::English);
                panel.set_dpi(dpi, language);
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
                    panel.finish_transition();
                    let old_snapshot = panel.capture_window();
                    let redraw = SuspendRedraw::new(hwnd);
                    panel.apply_theme(theme);
                    drop(redraw);
                    panel.start_theme_transition(old_snapshot);
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
            let started = diagnose::is_enabled().then(Instant::now);
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let painted = with_panel_ref(|panel| panel.paint_frame(dc, paint.rcPaint));
            let _ = EndPaint(hwnd, &paint);
            if let (Some(started), Ok(Some(()))) = (started, painted) {
                diagnose::log(format!(
                    "settings frame paint cost_us={}",
                    started.elapsed().as_micros()
                ));
            }
            if painted.is_err() {
                request_redraw();
            }
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, REFRESH_TIMER);
            let _ = KillTimer(hwnd, SCROLLBAR_FADE_TIMER);
            PANEL.with(|state| {
                if let Some(panel) = state.borrow_mut().as_mut() {
                    panel.pacer.stop();
                    panel.transition.take();
                }
            });
        }
        WM_NCDESTROY => {
            PANEL.with(|state| {
                state.borrow_mut().take();
            });
            let _ = GdiFlush();
            REDRAW_PENDING.with(|pending| pending.set(false));
            NAV_HOVER.with(|hover| hover.set(-1));
            NAV_PRESSED.with(|pressed| pressed.set(-1));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

unsafe extern "system" fn content_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND | WM_NOTIFY | ENSURE_VISIBLE => {
            let parent = GetParent(hwnd).unwrap_or_default();
            if !parent.is_invalid() {
                return SendMessageW(parent, msg, wparam, lparam);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            with_panel(|panel| panel.scrollbar_mouse_move(x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_panel(|panel| panel.scrollbar_mouse_leave());
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            with_panel(|panel| panel.scrollbar_mouse_down(x, y));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let pressed = with_panel(|panel| {
                panel.scrollbar_end_drag();
                panel.transition_press.take()
            })
            .flatten();
            if let Some((control, area)) = pressed {
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                    let point = POINT {
                        x: (lparam.0 & 0xFFFF) as i16 as i32,
                        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                    };
                    if PtInRect(&area, point).as_bool() && IsWindowEnabled(control).as_bool() {
                        let _ = SendMessageW(control, BM_CLICK, WPARAM(0), LPARAM(0));
                    }
                }
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with_panel(|panel| {
                panel.transition_press.take();
                panel.scrollbar_end_drag();
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = i32::from((wparam.0 >> 16) as u16 as i16);
            with_panel(|panel| {
                let step = panel.px(WHEEL_STEP);
                panel.scroll_to(panel.scroll - delta * step / 120);
            });
            let _ = UpdateWindow(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let started = diagnose::is_enabled().then(Instant::now);
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let painted = with_panel_ref(|panel| panel.paint_content(dc, paint.rcPaint));
            let _ = EndPaint(hwnd, &paint);
            if let (Some(started), Ok(Some(()))) = (started, painted) {
                diagnose::log(format!(
                    "settings content paint cost_us={}",
                    started.elapsed().as_micros()
                ));
            }
            if painted.is_err() {
                request_redraw();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            let point = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            if let Some(control) =
                with_panel(|panel| panel.press_transition_control(hwnd, point)).flatten()
            {
                let _ = SetFocus(control);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            // Restore the current composed pose if the OS exposes this window.
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            if has_area(paint.rcPaint) {
                let _ = with_panel_ref(|panel| {
                    if let Some(transition) =
                        panel.transition.as_ref().filter(|t| t.overlay.0 == hwnd)
                    {
                        transition.repaint(dc);
                    }
                });
            }
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Navigation hover/press tracking, and scrolling focused settings into view.
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
        WM_LBUTTONDOWN if is_nav => {
            let point = lparam.0 as u32;
            let hit = SendMessageW(hwnd, LB_ITEMFROMPOINT, WPARAM(0), LPARAM(point as isize)).0;
            let index = if hit >> 16 & 0xFFFF == 0 {
                (hit & 0xFFFF) as i32
            } else {
                -1
            };
            let previous = NAV_PRESSED.with(|pressed| pressed.replace(index));
            invalidate_nav_item(hwnd, previous);
            invalidate_nav_item(hwnd, index);
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED if is_nav => {
            let previous = NAV_PRESSED.with(|pressed| pressed.replace(-1));
            invalidate_nav_item(hwnd, previous);
        }
        WM_MOUSELEAVE if is_nav => {
            let previous = NAV_HOVER.with(|hover| hover.replace(-1));
            invalidate_nav_item(hwnd, previous);
            let prev_pressed = NAV_PRESSED.with(|pressed| pressed.replace(-1));
            invalidate_nav_item(hwnd, prev_pressed);
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

    #[test]
    fn page_slide_eases_into_the_final_position() {
        let distance = scale(14, 192);
        assert_eq!(page_slide(Duration::ZERO, distance), 28);
        assert!(page_slide(PAGE_DURATION / 2, distance) < 4);
        assert_eq!(page_slide(PAGE_DURATION, distance), 0);
        assert_eq!(page_slide(PAGE_DURATION * 2, distance), 0);
    }

    #[test]
    fn page_alphas_fade_old_out_and_new_in() {
        let (old0, title0, content0) = page_alphas(Duration::ZERO);
        assert_eq!(old0, 255);
        assert_eq!(title0, 0);
        assert_eq!(content0, 0);

        let (old_mid, title_mid, content_mid) = page_alphas(Duration::from_millis(80));
        assert_eq!(old_mid, 0);
        assert!(title_mid > 128);
        assert!(content_mid > 0 && content_mid < 255);

        let (old_end, title_end, content_end) = page_alphas(PAGE_DURATION);
        assert_eq!((old_end, title_end, content_end), (0, 255, 255));
    }

    #[test]
    fn reversing_a_switch_starts_at_its_interpolated_position() {
        let start = Instant::now();
        let mut sw = SwitchState::new(false, start);
        assert!(sw.set_checked(true, start));
        let interrupted = start + SWITCH_DURATION / 2;
        assert_eq!(sw.checked.value(interrupted), 0.875);
        assert!(sw.set_checked(false, interrupted));
        assert_eq!(sw.checked.value(interrupted), 0.875);
        assert_eq!(sw.checked.value(interrupted + SWITCH_DURATION), 0.0);
    }

    #[test]
    fn caption_hit_test_routes_resize_buttons_and_drag_region() {
        let client = RECT {
            left: 0,
            top: 0,
            right: 900,
            bottom: 640,
        };
        assert_eq!(
            hit_test_caption(client, POINT { x: 400, y: 2 }, 96, false),
            HTTOP as i32
        );
        assert_eq!(
            hit_test_caption(client, POINT { x: 4, y: 2 }, 96, false),
            HTTOPLEFT as i32
        );
        // Maximized windows disable the top resize strip so buttons reach screen top.
        assert_eq!(
            hit_test_caption(client, POINT { x: 890, y: 2 }, 96, true),
            HTCLOSE as i32
        );
        assert_eq!(
            hit_test_caption(client, POINT { x: 880, y: 16 }, 96, false),
            HTCLOSE as i32
        );
        assert_eq!(
            hit_test_caption(client, POINT { x: 830, y: 16 }, 96, false),
            HTMAXBUTTON as i32
        );
        assert_eq!(
            hit_test_caption(client, POINT { x: 780, y: 16 }, 96, false),
            HTMINBUTTON as i32
        );
        assert_eq!(
            hit_test_caption(client, POINT { x: 300, y: 20 }, 96, false),
            HTCAPTION as i32
        );
        assert_eq!(
            hit_test_caption(client, POINT { x: 300, y: 60 }, 96, false),
            HTCLIENT as i32
        );
    }

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

    #[test]
    #[ignore = "set CODEX_SETTINGS_SNAPSHOT_DIR to render settings window snapshots"]
    fn render_settings_snapshots() {
        let dir = std::path::PathBuf::from(
            std::env::var("CODEX_SETTINGS_SNAPSHOT_DIR").expect("snapshot directory"),
        );
        std::fs::create_dir_all(&dir).unwrap();

        let save_bmp = |path: &std::path::Path, bitmap: &Bitmap| unsafe {
            let pixels = bitmap.pixels();
            let width = bitmap.width;
            let height = bitmap.height;
            let mut bmp = Vec::with_capacity(54 + pixels.len() * 4);
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&(54u32 + pixels.len() as u32 * 4).to_le_bytes());
            bmp.extend_from_slice(&[0u8; 4]);
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&width.to_le_bytes());
            bmp.extend_from_slice(&(-height).to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0u8; 24]);
            for &pixel in pixels {
                bmp.extend_from_slice(&(0xFF00_0000 | (pixel & 0x00FF_FFFF)).to_le_bytes());
            }
            std::fs::write(path, bmp).unwrap();
        };

        unsafe {
            {
                let mut state = lock_state();
                *state = Some(AppState {
                    hwnd: SendHwnd::from_hwnd(HWND::default()),
                    taskbar_hwnd: None,
                    tray_notify_hwnd: None,
                    win_event_hook: None,
                    is_dark: false,
                    embedded: false,
                    language_override: Some(LanguageId::SimplifiedChinese),
                    language: LanguageId::SimplifiedChinese,
                    appearance: Appearance::default(),
                    quota_display_mode: QuotaDisplayMode::Remaining,
                    settings_theme: SettingsTheme::Light,
                    install_channel: InstallChannel::Portable,
                    session_percent: 25.0,
                    session_text: "75% 3h".into(),
                    weekly_percent: 40.0,
                    weekly_text: "60% 4d".into(),
                    codex_session_percent: 25.0,
                    codex_session_text: "75% 3h".into(),
                    codex_weekly_percent: 40.0,
                    codex_weekly_text: "60% 4d".into(),
                    antigravity_session_percent: 20.0,
                    antigravity_session_text: "80% 3h".into(),
                    antigravity_weekly_percent: 30.0,
                    antigravity_weekly_text: "70% 4d".into(),
                    show_claude_code: true,
                    show_codex: true,
                    show_antigravity: true,
                    antigravity_quota_group: None,
                    antigravity_groups: Vec::new(),
                    show_session_window: true,
                    show_weekly_window: true,
                    alert_threshold_percent: 20,
                    notified_quota_windows: BTreeSet::new(),
                    notify_on_reset: true,
                    reset_observations: Default::default(),
                    mcp_enabled: false,
                    mcp_port: None,
                    mcp_unavailable: false,
                    data: None,
                    poll_interval_ms: POLL_15_MIN,
                    adaptive_refresh: true,
                    monitor: provider_poll::Monitor::new([true, true, true], Instant::now()),
                    recovery_watch: None,
                    recovery_debounce: recovery_events::Debounce::default(),
                    has_poll_result: false,
                    update_status: UpdateStatus::Idle,
                    last_update_check_unix: None,
                    taskbar_index: 0,
                    monitor_device: None,
                    tray_offset: 0,
                    anchor_left: true,
                    dragging: false,
                    drag_start_mouse_x: 0,
                    drag_start_client_x: 0,
                    drag_start_offset: 0,
                    widget_visible: true,
                });
            }

            let module = GetModuleHandleW(None).unwrap();
            let instance = HINSTANCE(module.0);
            let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
            let _ = RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(settings_proc),
                hInstance: instance,
                lpszClassName: CLASS,
                hCursor: cursor,
                hIcon: LoadIconW(instance, PCWSTR(1usize as *const u16)).unwrap_or_default(),
                ..Default::default()
            });
            let _ = RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(content_proc),
                hInstance: instance,
                lpszClassName: CONTENT_CLASS,
                hCursor: cursor,
                ..Default::default()
            });
            let _ = RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                hInstance: instance,
                lpszClassName: OVERLAY_CLASS,
                hCursor: cursor,
                ..Default::default()
            });
            let (outer_w, outer_h) = outer_size(WIDTH, HEIGHT, 96);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                w!("Codex Usage"),
                window_style(),
                -2000,
                -2000,
                outer_w,
                outer_h,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )
            .unwrap();
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let panel = Panel::new(hwnd, HWND::default());
            PANEL.with(|s| *s.borrow_mut() = Some(panel));
            with_panel(|p| p.sync(true));

            // 1. Light General page
            let light_general = with_panel(|p| p.capture_window()).flatten().unwrap();
            save_bmp(&dir.join("01-general-light.bmp"), &light_general);
            let width_general = with_panel(|p| p.content_right - p.content_left).unwrap();

            // 2. Dark General page
            if let Some(s) = lock_state().as_mut() {
                s.settings_theme = SettingsTheme::Dark;
                s.is_dark = true;
            }
            with_panel(|p| p.sync(true));
            let dark_general = with_panel(|p| p.capture_window()).flatten().unwrap();
            save_bmp(&dir.join("02-general-dark.bmp"), &dark_general);

            // 3. Light Appearance page (scroll bar active, verify identical card width!)
            if let Some(s) = lock_state().as_mut() {
                s.settings_theme = SettingsTheme::Light;
                s.is_dark = false;
            }
            with_panel(|p| {
                p.page = 2;
                p.sync(true);
            });
            let light_appearance = with_panel(|p| p.capture_window()).flatten().unwrap();
            save_bmp(&dir.join("03-appearance-light.bmp"), &light_appearance);
            let width_appearance = with_panel(|p| p.content_right - p.content_left).unwrap();
            assert_eq!(
                width_general, width_appearance,
                "card width must not shift between short and scrollable pages"
            );

            // 4. Mid-transition frame (General -> Appearance at 70ms) + measure compose speed
            let compose_us = with_panel(|p| {
                p.page = 0;
                p.sync(true);
                let old_img = p.capture_page();
                p.page = 2;
                p.sync(true);
                let new_img = p.capture_page().unwrap();
                let bounds = new_img.bounds();
                let composed = Bitmap::new(bounds.right, bounds.bottom).unwrap();
                let overlay = Overlay::new(p.content, bounds).unwrap();
                let mut tr = Transition {
                    overlay,
                    composed,
                    kind: TransitionKind::Page {
                        old_image: old_img,
                        new_image: new_img,
                        old_rows: Vec::new(),
                        background: p.theme.background,
                        title_bottom: p.px(CONTENT_TOP + TITLE_HEIGHT),
                        distance: p.px(14),
                    },
                    start: Instant::now(),
                    drawn_elapsed: Duration::ZERO,
                    trace: None,
                };
                let t0 = Instant::now();
                tr.compose(Duration::from_millis(65));
                let us = t0.elapsed().as_micros();
                save_bmp(&dir.join("04-transition-mid.bmp"), &tr.composed);
                us
            })
            .unwrap();
            eprintln!("page transition compose cost_us={compose_us}");

            let _ = with_panel(|p| {
                // 5. Fluent dropdown flyout with color swatches (Single color selector)
                let strings = LanguageId::SimplifiedChinese.strings();
                let swatch_items: Vec<OptionItem> = SingleColor::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(i, color)| {
                        let name = match color {
                            SingleColor::Glacier => strings.color_glacier,
                            SingleColor::Lilac => strings.color_lilac,
                            SingleColor::Clay => strings.color_clay,
                            SingleColor::Rose => strings.color_rose,
                            SingleColor::Neutral => strings.color_neutral,
                        };
                        OptionItem::command(name, IDM_SINGLE_COLOR_FIRST + i as u16)
                            .with_swatch(color.bar(false))
                    })
                    .collect();
                if let Some(bmp) = dropdown::render_preview_bitmap(
                    &swatch_items,
                    0,
                    Some(2),
                    p.px(DROPDOWN_WIDTH),
                    p.theme,
                    p.fonts,
                    p.dpi,
                ) {
                    save_bmp(&dir.join("05-dropdown-swatch-light.bmp"), &bmp);
                }

                // 6. Appearance page with overlay scrollbar revealed & hovered
                p.page = 2;
                p.sync(true);
                let now = Instant::now();
                p.scrollbar.opacity = ScalarAnim::settled(1.0, now, SCROLLBAR_FADE_IN);
                p.scrollbar.expand = ScalarAnim::settled(1.0, now, SCROLLBAR_EXPAND_DURATION);
                p.scrollbar.hovered = true;
                if let Some(bmp) = p.capture_window() {
                    save_bmp(&dir.join("06-appearance-scrollbar-light.bmp"), &bmp);
                }
            });

            let _ = DestroyWindow(hwnd);
            *lock_state() = None;
        }
    }

    #[test]
    fn overlay_scrollbar_metrics_hide_short_content_and_clamp_thumb() {
        assert!(scrollbar_thumb_metrics(500, 400, 0, 96).is_none());
        assert!(scrollbar_thumb_metrics(500, 500, 0, 96).is_none());

        let top = scrollbar_thumb_metrics(500, 1000, 0, 96).unwrap();
        let mid = scrollbar_thumb_metrics(500, 1000, 250, 96).unwrap();
        let bot = scrollbar_thumb_metrics(500, 1000, 500, 96).unwrap();
        assert_eq!(top.thumb_top, top.track_top);
        assert!((bot.thumb_top + bot.thumb_height - bot.track_bottom).abs() < 0.01);
        assert!(mid.thumb_top > top.thumb_top && mid.thumb_top < bot.thumb_top);

        // Very tall content keeps at least a 32px thumb.
        let huge = scrollbar_thumb_metrics(500, 50_000, 0, 96).unwrap();
        assert!(huge.thumb_height >= 32.0);
    }
}
