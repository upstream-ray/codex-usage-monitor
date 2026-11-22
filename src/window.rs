use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::{ExtractIconExW, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::codex_colors::{codex_brand_color, SingleColor};
use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
use crate::mcp_server;
use crate::models::{AppUsageData, QuotaDisplayMode};
use crate::native_interop::{
    self, Color, TIMER_COUNTDOWN, TIMER_FRESHNESS, TIMER_POLL, TIMER_TASKBAR_RETRY,
    TIMER_UPDATE_CHECK, WM_APP_TRAY, WM_APP_USAGE_UPDATED,
};
use crate::poller;
use crate::provider_icons::{self, Provider};
use crate::provider_poll;
use crate::quota_refresh;
use crate::quota_text;
use crate::quota_tooltip;
use crate::recovery_events;
use crate::settings_store;
use crate::theme;
use crate::tray_icon;
use crate::updater::{self, InstallChannel, ReleaseDescriptor, UpdateCheckResult};

#[path = "settings_window.rs"]
mod settings_window;

/// Wrapper to make HWND sendable across threads (safe for PostMessage usage)
#[derive(Clone, Copy)]
struct SendHwnd(isize);

unsafe impl Send for SendHwnd {}

impl SendHwnd {
    fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
    fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

/// Shared application state
struct AppState {
    hwnd: SendHwnd,
    taskbar_hwnd: Option<HWND>,
    tray_notify_hwnd: Option<HWND>,
    win_event_hook: Option<HWINEVENTHOOK>,
    is_dark: bool,
    embedded: bool,
    language_override: Option<LanguageId>,
    language: LanguageId,
    appearance: Appearance,
    quota_display_mode: QuotaDisplayMode,
    settings_theme: SettingsTheme,
    install_channel: InstallChannel,

    session_percent: f64,
    session_text: String,
    weekly_percent: f64,
    weekly_text: String,
    codex_session_percent: f64,
    codex_session_text: String,
    codex_weekly_percent: f64,
    codex_weekly_text: String,
    antigravity_session_percent: f64,
    antigravity_session_text: String,
    antigravity_weekly_percent: f64,
    antigravity_weekly_text: String,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    antigravity_quota_group: Option<String>,
    antigravity_groups: Vec<String>,
    show_session_window: bool,
    show_weekly_window: bool,
    alert_threshold_percent: u8,
    notified_quota_windows: BTreeSet<String>,
    notify_on_reset: bool,
    reset_observations: crate::reset_alerts::ResetObservations,
    mcp_enabled: bool,
    mcp_port: Option<u16>,
    mcp_unavailable: bool,

    data: Option<AppUsageData>,

    poll_interval_ms: u32,
    adaptive_refresh: bool,
    monitor: provider_poll::Monitor,
    recovery_watch: Option<recovery_events::Watch>,
    recovery_debounce: recovery_events::Debounce,
    has_poll_result: bool,
    update_status: UpdateStatus,
    last_update_check_unix: Option<u64>,

    taskbar_index: usize,
    monitor_device: Option<String>,
    tray_offset: i32,
    anchor_left: bool,
    dragging: bool,
    drag_start_mouse_x: i32,
    drag_start_client_x: i32,
    drag_start_offset: i32,

    widget_visible: bool,
}

#[derive(Clone, Debug)]
enum UpdateStatus {
    Idle,
    Checking,
    Applying,
    UpToDate,
    Available(ReleaseDescriptor),
}

const POLL_1_MIN: u32 = 60_000;
const POLL_5_MIN: u32 = 300_000;
const POLL_15_MIN: u32 = 900_000;
const POLL_1_HOUR: u32 = 3_600_000;

// Menu item IDs for update frequency
const IDM_FREQ_1MIN: u16 = 10;
const IDM_FREQ_5MIN: u16 = 11;
const IDM_FREQ_15MIN: u16 = 12;
const IDM_FREQ_1HOUR: u16 = 13;
const IDM_ADAPTIVE_REFRESH: u16 = 14;
const IDM_START_WITH_WINDOWS: u16 = 20;
const IDM_SETTINGS: u16 = 21;
const IDM_RESET_POSITION: u16 = 30;
const IDM_ANCHOR_LEFT: u16 = 34;
const IDM_VERSION_ACTION: u16 = 31;
const IDM_LANG_SYSTEM: u16 = 40;
const IDM_LANG_ENGLISH: u16 = 41;
const IDM_LANG_DUTCH: u16 = 42;
const IDM_LANG_SPANISH: u16 = 43;
const IDM_LANG_FRENCH: u16 = 44;
const IDM_LANG_GERMAN: u16 = 45;
const IDM_LANG_JAPANESE: u16 = 46;
const IDM_LANG_KOREAN: u16 = 47;
const IDM_LANG_TRADITIONAL_CHINESE: u16 = 48;
const IDM_LANG_RUSSIAN: u16 = 49;
const IDM_LANG_PORTUGUESE_BRAZIL: u16 = 50;
const IDM_LANG_SIMPLIFIED_CHINESE: u16 = 51;
const IDM_LANG_ITALIAN: u16 = 52;
const IDM_QUOTA_USED: u16 = 53;
const IDM_QUOTA_REMAINING: u16 = 54;
const IDM_PASSIVE_MONITORING: u16 = 55;
const IDM_UPDATES_ENABLED: u16 = 56;
const IDM_MCP_SERVER: u16 = 35;
const MAX_QUOTA_GROUPS: usize = 32;
const IDM_ALERT_RESET: u16 = 84;
const IDM_MODEL_CLAUDE_CODE: u16 = 60;
const IDM_MODEL_CODEX: u16 = 61;
const IDM_MODEL_ANTIGRAVITY: u16 = 62;
const IDM_SHOW_SESSION_WINDOW: u16 = 71;
const IDM_SHOW_WEEKLY_WINDOW: u16 = 72;
const IDM_ALERT_OFF: u16 = 80;
const IDM_ALERT_10: u16 = 81;
const IDM_ALERT_20: u16 = 82;
const IDM_ALERT_30: u16 = 83;
const IDM_PALETTE_SYSTEM: u16 = 90;
const IDM_PALETTE_DARK: u16 = 91;
const IDM_PALETTE_LIGHT: u16 = 92;
const IDM_BAR_CONTINUOUS: u16 = 93;
const IDM_BAR_SEGMENTED: u16 = 94;
const IDM_PROVIDER_LOGOS: u16 = 121;
const IDM_BAR_MEDIUM: u16 = 95;
const IDM_BAR_THIN: u16 = 96;
const IDM_FONT_12: u16 = 97;
const IDM_FONT_14: u16 = 98;
const IDM_APPEARANCE_RECOMMENDED: u16 = 99;
const IDM_APPEARANCE_RESET: u16 = 100;
const IDM_BAR_DOTS: u16 = 101;
const IDM_BAR_BATTERY: u16 = 102;
const IDM_BAR_THICK: u16 = 103;
const IDM_FONT_16: u16 = 104;
const IDM_COLOR_BRAND: u16 = 105;
const IDM_COLOR_REMAINING: u16 = 106;
const IDM_COLOR_SINGLE: u16 = 107;
const IDM_SINGLE_COLOR_FIRST: u16 = 110;
const IDM_SINGLE_COLOR_LAST: u16 = IDM_SINGLE_COLOR_FIRST + SingleColor::ALL.len() as u16 - 1;
const IDM_THEME_SYSTEM: u16 = 122;
const IDM_THEME_LIGHT: u16 = 123;
const IDM_THEME_DARK: u16 = 124;
const IDM_PERIOD_LABELS: u16 = 125;
const IDM_PERCENT_OUTSIDE_BATTERY: u16 = 126;
const IDM_PERCENT_INSIDE_BATTERY: u16 = 127;
const IDM_RESET_COUNTDOWN: u16 = 128;
const IDM_RESET_CLOCK: u16 = 129;
const IDM_WINDOWS_STARTUP_SETTINGS: u16 = 130;

const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_APP_UPDATE_CHECK_COMPLETE: u32 = WM_APP + 2;
const WM_APP_TASKBARS_CHANGED: u32 = WM_APP + 6;
const TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS: u64 = 750;

/// How often the watchdog thread polls for an explorer.exe restart (which
/// recreates the taskbar and wipes our tray-icon registration).
const TASKBAR_WATCH_INTERVAL_SECS: u64 = 2;

static SUPPRESS_TRAY_REPOSITION_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Current system DPI (96 = 100% scaling, 144 = 150%, 192 = 200%, etc.)
static CURRENT_DPI: AtomicU32 = AtomicU32::new(96);
static EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Scale a base pixel value (designed at 96 DPI) to the current DPI.
fn sc(px: i32) -> i32 {
    let dpi = CURRENT_DPI.load(Ordering::Relaxed);
    (px as f64 * dpi as f64 / 96.0).round() as i32
}

/// Re-query the monitor DPI for our window and update the cached value.
/// Uses GetDpiForWindow which returns the live DPI (unlike GetDpiForSystem
/// which is cached at process startup and never changes).
fn refresh_dpi() {
    let hwnd = {
        let state = lock_state();
        state.as_ref().map(|s| s.hwnd.to_hwnd())
    };
    if let Some(hwnd) = hwnd {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            CURRENT_DPI.store(dpi, Ordering::Relaxed);
        }
    }
}

/// Spacing below which two relaunches are treated as a storm (e.g. explorer.exe
/// crash-looping); when detected we back off instead of spawning in a tight loop.
const RELAUNCH_THROTTLE_SECS: u64 = 10;
const RELAUNCH_BACKOFF_SECS: u64 = 30;
/// Environment flag set on a relaunched child so it waits for the previous
/// instance's single-instance mutex instead of exiting immediately.
const ENV_RELAUNCH: &str = "CODEX_USAGE_RELAUNCH";
/// Unix timestamp (seconds) of the relaunch that spawned this process, passed to
/// the child so it can detect a relaunch storm.
const ENV_LAST_RELAUNCH_UNIX: &str = "CODEX_USAGE_LAST_RELAUNCH_UNIX";

/// Relaunch the widget as a fresh process after explorer.exe has restarted.
///
/// When the shell restarts it destroys our embedded child window outright (the
/// window is gone, not merely orphaned - `IsWindow` returns false) and leaves
/// the UI thread parked in `GetMessage` with no window to recreate in place.
/// Spawning a clean new process - which re-embeds into the freshly created
/// taskbar - and exiting this one is the robust recovery. The child is flagged
/// via `ENV_RELAUNCH` so it waits for this instance's single-instance mutex to
/// be released before taking over (see the guard in `run`).
fn relaunch_self() {
    // Back off if we are relaunching very soon after the relaunch that spawned
    // us: that signals the shell is crash-looping, not a one-off restart.
    let now = now_unix_secs();
    let last = std::env::var(ENV_LAST_RELAUNCH_UNIX)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if last != 0 && now.saturating_sub(last) < RELAUNCH_THROTTLE_SECS {
        diagnose::log("relaunch storm detected; backing off before relaunching");
        std::thread::sleep(Duration::from_secs(RELAUNCH_BACKOFF_SECS));
    }

    if EXIT_REQUESTED.load(Ordering::Relaxed) {
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            diagnose::log_error("watchdog: unable to resolve current executable", error);
            return;
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    match std::process::Command::new(exe)
        .args(&args)
        .env(ENV_RELAUNCH, "1")
        .env(ENV_LAST_RELAUNCH_UNIX, now.to_string())
        .spawn()
    {
        Ok(_) => {
            diagnose::log("watchdog: relaunched fresh instance, exiting old one");
            std::process::exit(0);
        }
        Err(error) => {
            diagnose::log_error("watchdog: unable to spawn relaunched instance", error);
        }
    }
}

/// Recover after Explorer replaces the taskbar or destroys our embedded window.
fn spawn_taskbar_watchdog() {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TASKBAR_WATCH_INTERVAL_SECS));
        if EXIT_REQUESTED.load(Ordering::Relaxed) {
            return;
        }
        let attachment = {
            let state = lock_state();
            state.as_ref().and_then(|s| {
                s.taskbar_hwnd
                    .map(|taskbar| (taskbar, s.hwnd, s.monitor_device.clone()))
            })
        };
        let Some((old, widget, preferred)) = attachment else {
            continue;
        };
        let taskbars = native_interop::find_taskbars();
        let widget_alive = unsafe { IsWindow(widget.to_hwnd()).as_bool() };
        let taskbar_present = taskbars.iter().any(|taskbar| taskbar.hwnd == old);
        if needs_taskbar_relaunch(widget_alive, taskbar_present, !taskbars.is_empty()) {
            diagnose::log(format!(
                "watchdog: widget_alive={widget_alive} taskbar old={:?} new={:?} -> relaunching",
                old.0, taskbars[0].hwnd.0
            ));
            relaunch_self();
        } else if let Some(preferred) = preferred {
            if taskbars.iter().any(|taskbar| {
                taskbar.hwnd != old
                    && native_interop::taskbar_monitor_device(taskbar).as_deref()
                        == Some(&preferred)
            }) {
                // An embedded child may not receive the display-change broadcast.
                unsafe {
                    let _ = PostMessageW(
                        widget.to_hwnd(),
                        WM_APP_TASKBARS_CHANGED,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }
    });
}

fn needs_taskbar_relaunch(
    widget_alive: bool,
    taskbar_present: bool,
    taskbar_available: bool,
) -> bool {
    taskbar_available && (!widget_alive || !taskbar_present)
}

fn load_embedded_app_icons() -> (HICON, HICON) {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return (HICON::default(), HICON::default());
        }

        let mut large_icon = HICON::default();
        let mut small_icon = HICON::default();
        let extracted = ExtractIconExW(
            PCWSTR::from_raw(exe_buf.as_ptr()),
            0,
            Some(&mut large_icon),
            Some(&mut small_icon),
            1,
        );

        if extracted == 0 {
            (HICON::default(), HICON::default())
        } else {
            (large_icon, small_icon)
        }
    }
}

unsafe impl Send for AppState {}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

/// Lock STATE safely, recovering from poisoned mutex
fn lock_state() -> MutexGuard<'static, Option<AppState>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

const SETTINGS_DIR: &str = "CodexUsage";
const LEGACY_SETTINGS_DIR: &str = "ClaudeCodeUsageMonitor";

fn appdata_path(directory: &str) -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join(directory).join("settings.json")
}

fn settings_path() -> PathBuf {
    appdata_path(SETTINGS_DIR)
}

fn legacy_settings_path() -> PathBuf {
    appdata_path(LEGACY_SETTINGS_DIR)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Palette {
    #[default]
    System,
    HighContrastDark,
    HighContrastLight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BarStyle {
    #[default]
    Continuous,
    #[serde(alias = "classic_segmented")]
    Segmented,
    Dots,
    Battery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BarThickness {
    #[serde(alias = "slim")]
    Thin,
    #[default]
    #[serde(alias = "standard")]
    Medium,
    Thick,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FontSize {
    #[default]
    #[serde(rename = "12", alias = "standard")]
    Px12,
    #[serde(rename = "14", alias = "large")]
    Px14,
    #[serde(rename = "16")]
    Px16,
}

impl FontSize {
    fn px(self) -> i32 {
        match self {
            Self::Px12 => 12,
            Self::Px14 => 14,
            Self::Px16 => 16,
        }
    }

    fn fit(self, height: i32, rows: i32, scale: impl Fn(i32) -> i32) -> Self {
        let fits = |size: Self| rows * scale(size.px() + 6) <= height;
        match self {
            Self::Px16 if fits(Self::Px16) => Self::Px16,
            Self::Px14 | Self::Px16 if fits(Self::Px14) => Self::Px14,
            _ => Self::Px12,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettingsTheme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ColorMode {
    #[default]
    Brand,
    Remaining,
    Single,
}

fn remaining_bar_color(remaining: f64, dark: bool) -> Color {
    let (light, dark_color) = if remaining > 50.0 {
        ("#137A46", "#4ADE80")
    } else if remaining >= 20.0 {
        ("#8A5900", "#FBBF24")
    } else {
        ("#BA3039", "#FB7185")
    };
    Color::from_hex(if dark { dark_color } else { light })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResetTimeDisplay {
    #[default]
    Countdown,
    Clock,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Appearance {
    palette: Palette,
    bar_style: BarStyle,
    bar_thickness: BarThickness,
    font_size: FontSize,
    color_mode: ColorMode,
    single_color: SingleColor,
    hide_provider_logos: bool,
    hide_period_labels: bool,
    percent_inside_battery: bool,
    reset_time_display: ResetTimeDisplay,
}

impl Appearance {
    fn translucent_dark_taskbar() -> Self {
        Self {
            palette: Palette::HighContrastDark,
            bar_style: BarStyle::Continuous,
            bar_thickness: BarThickness::Thin,
            font_size: FontSize::Px14,
            ..Self::default()
        }
    }
    fn is_dark(self, system_is_dark: bool) -> bool {
        match self.palette {
            Palette::System => system_is_dark,
            Palette::HighContrastDark => true,
            Palette::HighContrastLight => false,
        }
    }

    fn font_px(self) -> i32 {
        self.font_size.px()
    }

    fn shows_percent_in_battery(self) -> bool {
        self.bar_style == BarStyle::Battery && self.percent_inside_battery
    }

    fn row_height(self) -> i32 {
        sc(self.font_px() + 6)
    }

    fn bar_height(self, row_height: i32) -> i32 {
        sc(match self.bar_thickness {
            BarThickness::Thin => 4,
            BarThickness::Medium => 7,
            BarThickness::Thick => 10,
        })
        .min((row_height - sc(2)).max(1))
    }

    fn bar_color(self, provider: Provider, used: f64, dark: bool) -> Color {
        match self.color_mode {
            ColorMode::Remaining => remaining_bar_color(poller::remaining_percentage(used), dark),
            ColorMode::Single => self.single_color.bar(dark),
            ColorMode::Brand => match provider {
                Provider::Claude => claude_accent_color(),
                Provider::Codex => codex_brand_color(dark),
                Provider::Antigravity => antigravity_accent_color(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    tray_offset: i32,
    #[serde(default)]
    anchor_left: bool,
    #[serde(default)]
    taskbar_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    monitor_device: Option<String>,
    #[serde(default = "default_poll_interval")]
    poll_interval_ms: u32,
    #[serde(default = "default_adaptive_refresh")]
    adaptive_refresh: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(default)]
    appearance: Appearance,
    #[serde(default)]
    settings_theme: SettingsTheme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quota_display_mode: Option<QuotaDisplayMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_update_check_unix: Option<u64>,
    #[serde(default = "default_widget_visible")]
    widget_visible: bool,
    #[serde(default = "default_show_claude_code")]
    show_claude_code: bool,
    #[serde(default = "default_show_codex")]
    show_codex: bool,
    #[serde(default = "default_show_antigravity")]
    show_antigravity: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    antigravity_quota_group: Option<String>,
    #[serde(default = "default_show_usage_window")]
    show_session_window: bool,
    #[serde(default = "default_show_usage_window")]
    show_weekly_window: bool,
    #[serde(default)]
    alert_threshold_percent: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notified_quota_windows: Vec<String>,
    #[serde(default = "crate::privacy::default_passive_monitoring")]
    passive_monitoring: bool,
    #[serde(default = "crate::privacy::default_updates_enabled")]
    updates_enabled: bool,
    #[serde(default)]
    notify_on_reset: bool,
    #[serde(default)]
    reset_observations: crate::reset_alerts::ResetObservations,
    #[serde(default)]
    mcp_server_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_port: Option<u16>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            tray_offset: 0,
            anchor_left: false,
            taskbar_index: 0,
            monitor_device: None,
            poll_interval_ms: default_poll_interval(),
            adaptive_refresh: true,
            language: None,
            appearance: Appearance::default(),
            quota_display_mode: Some(QuotaDisplayMode::Remaining),
            settings_theme: SettingsTheme::default(),
            last_update_check_unix: None,
            widget_visible: true,
            show_claude_code: false,
            show_codex: true,
            show_antigravity: false,
            antigravity_quota_group: None,
            show_session_window: true,
            show_weekly_window: true,
            alert_threshold_percent: 0,
            notified_quota_windows: Vec::new(),
            passive_monitoring: true,
            updates_enabled: true,
            notify_on_reset: false,
            reset_observations: Default::default(),
            mcp_server_enabled: false,
            mcp_port: None,
        }
    }
}

fn default_adaptive_refresh() -> bool {
    true
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}

fn default_widget_visible() -> bool {
    true
}

fn default_show_claude_code() -> bool {
    false
}

fn default_show_codex() -> bool {
    true
}

fn default_show_antigravity() -> bool {
    false
}

fn default_show_usage_window() -> bool {
    true
}

fn load_settings() -> SettingsFile {
    let current_path = settings_path();
    let legacy_path = legacy_settings_path();
    let (settings, migrated) = load_settings_from_paths(&current_path, &legacy_path)
        .unwrap_or_else(|| {
            let privacy = crate::privacy::preferences();
            (
                SettingsFile {
                    passive_monitoring: privacy.passive_monitoring,
                    updates_enabled: privacy.updates_enabled,
                    ..SettingsFile::default()
                },
                false,
            )
        });
    let mut settings = normalize_settings(settings);
    let legacy_display = settings.quota_display_mode.is_none();
    if legacy_display {
        let language = localization::resolve_language(
            settings.language.as_deref().and_then(LanguageId::from_code),
        );
        settings.quota_display_mode = Some(if language == LanguageId::SimplifiedChinese {
            QuotaDisplayMode::Remaining
        } else {
            QuotaDisplayMode::Used
        });
    }
    crate::privacy::set_preferences(crate::privacy::Preferences {
        passive_monitoring: settings.passive_monitoring,
        updates_enabled: settings.updates_enabled,
    });
    if migrated || legacy_display {
        save_settings(&settings);
        if migrated {
            diagnose::log(format!(
                "migrated settings from {} to {}",
                legacy_path.display(),
                current_path.display()
            ));
        }
    }
    settings
}

fn load_settings_from_paths(
    current_path: &std::path::Path,
    legacy_path: &std::path::Path,
) -> Option<(SettingsFile, bool)> {
    if let Ok(content) = std::fs::read_to_string(current_path) {
        return serde_json::from_str(&content)
            .ok()
            .map(|settings| (settings, false));
    }

    let content = std::fs::read_to_string(legacy_path).ok()?;
    serde_json::from_str(&content)
        .ok()
        .map(|settings| (settings, true))
}

fn normalize_settings(mut settings: SettingsFile) -> SettingsFile {
    if !(POLL_1_MIN..=POLL_1_HOUR).contains(&settings.poll_interval_ms) {
        settings.poll_interval_ms = POLL_15_MIN;
    }
    if !settings.show_claude_code && !settings.show_codex && !settings.show_antigravity {
        settings.show_codex = true;
    }
    if !settings.show_session_window && !settings.show_weekly_window {
        settings.show_session_window = true;
    }
    if !matches!(settings.alert_threshold_percent, 0 | 10 | 20 | 30) {
        settings.alert_threshold_percent = 0;
    }
    settings.antigravity_quota_group = settings
        .antigravity_quota_group
        .as_deref()
        .and_then(poller::quota_group_name)
        .filter(|name| !name.eq_ignore_ascii_case(poller::GEMINI_QUOTA_GROUP));
    settings.notified_quota_windows.sort();
    settings.notified_quota_windows.dedup();
    settings
}

fn save_settings(settings: &SettingsFile) {
    if let Err(error) = settings_store::save(&settings_path(), || Some(settings.clone())) {
        diagnose::log_error("unable to save settings", error);
    }
}

fn save_state_settings() {
    let result = settings_store::save(&settings_path(), || {
        let state = lock_state();
        let privacy = crate::privacy::preferences();
        state.as_ref().map(|s| SettingsFile {
            tray_offset: s.tray_offset,
            anchor_left: s.anchor_left,
            taskbar_index: s.taskbar_index,
            monitor_device: s.monitor_device.clone(),
            poll_interval_ms: s.poll_interval_ms,
            adaptive_refresh: s.adaptive_refresh,
            language: s
                .language_override
                .map(|language| language.code().to_string()),
            appearance: s.appearance,
            quota_display_mode: Some(s.quota_display_mode),
            settings_theme: s.settings_theme,
            last_update_check_unix: s.last_update_check_unix,
            widget_visible: s.widget_visible,
            show_claude_code: s.show_claude_code,
            show_codex: s.show_codex,
            show_antigravity: s.show_antigravity,
            antigravity_quota_group: s.antigravity_quota_group.clone(),
            show_session_window: s.show_session_window,
            show_weekly_window: s.show_weekly_window,
            alert_threshold_percent: s.alert_threshold_percent,
            notified_quota_windows: s.notified_quota_windows.iter().cloned().collect(),
            passive_monitoring: privacy.passive_monitoring,
            updates_enabled: privacy.updates_enabled,
            notify_on_reset: s.notify_on_reset,
            reset_observations: s.reset_observations.clone(),
            mcp_server_enabled: s.mcp_enabled,
            mcp_port: s.mcp_port,
        })
    });
    settings_window::note_save_result(result.is_ok());
    if let Err(error) = result {
        diagnose::log_error("unable to save settings", error);
    }
}

fn format_precise_reset_time(resets_at: Option<SystemTime>) -> Option<String> {
    let local = native_interop::system_time_to_local(resets_at?)?;
    Some(format_local_system_time(local))
}

fn format_local_system_time(local: SYSTEMTIME) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

fn short_reset_time(reset: SYSTEMTIME, today: SYSTEMTIME, weekly: bool) -> String {
    let reset_date = (reset.wYear, reset.wMonth, reset.wDay);
    let today_date = (today.wYear, today.wMonth, today.wDay);
    let clock = format!("{:02}:{:02}", reset.wHour, reset.wMinute);
    let date = format!("{:02}/{:02}", reset.wMonth, reset.wDay);
    if weekly {
        date
    } else if reset_date == today_date {
        clock
    } else {
        format!("{date} {clock}")
    }
}

fn clock_quota_line(
    section: &crate::models::UsageSection,
    mode: QuotaDisplayMode,
    today: SYSTEMTIME,
    weekly: bool,
) -> String {
    if !section.has_reading() {
        return "--".into();
    }
    let percent = poller::format_percent(mode.percentage(section.percentage));
    match section
        .resets_at
        .and_then(native_interop::system_time_to_local)
    {
        Some(reset) => format!("{percent}  {}", short_reset_time(reset, today, weekly)),
        None => percent,
    }
}

fn service_tooltip(
    service: &str,
    session_text: &str,
    weekly_text: &str,
    show_session_window: bool,
    show_weekly_window: bool,
) -> String {
    let mut parts = Vec::new();
    if show_session_window {
        parts.push(format!("5h {session_text}"));
    }
    if show_weekly_window {
        parts.push(format!("7d {weekly_text}"));
    }
    format!("{service}: {}", parts.join(" | "))
}

struct QuotaAlert {
    kind: tray_icon::TrayIconKind,
    title: String,
    message: String,
}

fn collect_low_quota_alerts(state: &mut AppState, data: &AppUsageData) -> Vec<QuotaAlert> {
    let threshold = state.alert_threshold_percent;
    if threshold == 0 {
        return Vec::new();
    }

    let strings = state.language.strings();
    let mut alerts = Vec::new();
    if state.show_claude_code {
        if let Some(usage) = data.claude_code.as_ref() {
            append_provider_alerts(
                &mut alerts,
                &mut state.notified_quota_windows,
                threshold,
                state.language,
                tray_icon::TrayIconKind::Claude,
                "claude",
                strings.claude_code_model,
                usage,
                strings,
            );
        }
    }
    if state.show_codex {
        if let Some(usage) = data.codex.as_ref() {
            append_provider_alerts(
                &mut alerts,
                &mut state.notified_quota_windows,
                threshold,
                state.language,
                tray_icon::TrayIconKind::Codex,
                "codex",
                strings.codex_model,
                usage,
                strings,
            );
        }
    }
    if state.show_antigravity {
        if let Some(usage) = data.antigravity.as_ref() {
            append_provider_alerts(
                &mut alerts,
                &mut state.notified_quota_windows,
                threshold,
                state.language,
                tray_icon::TrayIconKind::Antigravity,
                "antigravity",
                strings.antigravity_model,
                usage,
                strings,
            );
        }
    }
    alerts
}

#[allow(clippy::too_many_arguments)]
fn append_provider_alerts(
    alerts: &mut Vec<QuotaAlert>,
    notified: &mut BTreeSet<String>,
    threshold: u8,
    language: LanguageId,
    kind: tray_icon::TrayIconKind,
    provider_key: &str,
    provider_label: &str,
    usage: &crate::models::UsageData,
    strings: Strings,
) {
    append_quota_alert(
        alerts,
        notified,
        threshold,
        language,
        kind,
        provider_key,
        provider_label,
        "session",
        strings.session_window,
        &usage.session,
    );
    append_quota_alert(
        alerts,
        notified,
        threshold,
        language,
        kind,
        provider_key,
        provider_label,
        "weekly",
        strings.weekly_window,
        &usage.weekly,
    );
}

#[allow(clippy::too_many_arguments)]
fn append_quota_alert(
    alerts: &mut Vec<QuotaAlert>,
    notified: &mut BTreeSet<String>,
    threshold: u8,
    language: LanguageId,
    kind: tray_icon::TrayIconKind,
    provider_key: &str,
    provider_label: &str,
    window_key: &str,
    window_label: &str,
    section: &crate::models::UsageSection,
) {
    if !section.has_reading() {
        return;
    }
    let prefix = format!("{provider_key}:{window_key}:");
    let reset_epoch = section
        .resets_at
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs());
    let remaining = poller::remaining_percentage(section.percentage);
    if !crate::quota_alerts::should_notify(
        notified,
        &prefix,
        reset_epoch,
        remaining,
        f64::from(threshold),
    ) {
        return;
    }

    let remaining = poller::format_percent(remaining);
    let reset = format_precise_reset_time(section.resets_at);
    let (title, message) = if language == LanguageId::SimplifiedChinese {
        (
            format!("{provider_label} 额度提醒"),
            format!(
                "{window_label}额度仅剩 {remaining}，重置时间：{}",
                reset.unwrap_or_else(|| "未知".to_string())
            ),
        )
    } else {
        (
            format!("{provider_label} quota alert"),
            format!(
                "{window_label} quota has {remaining} remaining. Reset: {}",
                reset.unwrap_or_else(|| "unknown".to_string())
            ),
        )
    };
    alerts.push(QuotaAlert {
        kind,
        title,
        message,
    });
}

fn antigravity_label(state: &AppState) -> String {
    let group = state
        .antigravity_quota_group
        .as_deref()
        .unwrap_or(poller::GEMINI_QUOTA_GROUP);
    format!("Antigravity ({group})")
}

fn tray_icon_data_from_state() -> Option<tray_icon::TrayIconData> {
    let state = lock_state();
    match state.as_ref() {
        Some(s) if s.has_poll_result => {
            let mut services = Vec::new();
            let strings = s.language.strings();
            if s.show_claude_code {
                services.push(service_tooltip(
                    strings.claude_code_model,
                    &s.session_text,
                    &s.weekly_text,
                    s.show_session_window,
                    s.show_weekly_window,
                ));
            }
            if s.show_codex {
                services.push(service_tooltip(
                    strings.codex_model,
                    &s.codex_session_text,
                    &s.codex_weekly_text,
                    s.show_session_window,
                    s.show_weekly_window,
                ));
            }
            if s.show_antigravity {
                services.push(service_tooltip(
                    &antigravity_label(s),
                    &s.antigravity_session_text,
                    &s.antigravity_weekly_text,
                    s.show_session_window,
                    s.show_weekly_window,
                ));
            }
            Some(tray_icon::TrayIconData {
                tooltip: if services.is_empty() {
                    strings.window_title.to_string()
                } else {
                    services.join("\n")
                },
            })
        }
        Some(s) => {
            let strings = s.language.strings();
            let tooltip = match (s.show_claude_code, s.show_codex, s.show_antigravity) {
                (false, true, false) => strings.codex_window_title,
                (false, false, true) => strings.antigravity_window_title,
                _ => strings.window_title,
            };
            Some(tray_icon::TrayIconData {
                tooltip: tooltip.to_string(),
            })
        }
        None => None,
    }
}

fn sync_tray_icons(hwnd: HWND) {
    let icon = tray_icon_data_from_state();
    tray_icon::sync(hwnd, icon.as_ref());
}

fn toggle_widget_visibility(hwnd: HWND) {
    let new_visible = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.widget_visible = !s.widget_visible;
            s.widget_visible
        } else {
            return;
        }
    };
    save_state_settings();
    unsafe {
        if new_visible {
            position_at_taskbar();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            render_layered();
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn attach_to_taskbar(hwnd: HWND, taskbar: native_interop::TaskbarWindow, index: usize) -> bool {
    if !native_interop::embed_in_taskbar(hwnd, taskbar.hwnd) {
        diagnose::log(format!(
            "unable to attach widget to taskbar index={index} hwnd={:?}",
            taskbar.hwnd
        ));
        return false;
    }

    diagnose::log(format!(
        "taskbar selected index={index} hwnd={:?} rect=({}, {}, {}, {})",
        taskbar.hwnd, taskbar.rect.left, taskbar.rect.top, taskbar.rect.right, taskbar.rect.bottom
    ));

    let old_hook = {
        let mut state = lock_state();
        state.as_mut().and_then(|s| s.win_event_hook.take())
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook);
    }

    let tray_notify = native_interop::find_child_window(taskbar.hwnd, "TrayNotifyWnd");
    if tray_notify.is_some() {
        diagnose::log("TrayNotifyWnd found");
    } else {
        diagnose::log("TrayNotifyWnd not found");
    }

    let hook = tray_notify.and_then(|tray_hwnd| {
        let thread_id = native_interop::get_window_thread_id(tray_hwnd);
        native_interop::set_tray_event_hook(thread_id, on_tray_location_changed)
    });
    if hook.is_some() {
        diagnose::log("tray event hook installed");
    } else {
        diagnose::log("tray event hook could not be installed");
    }

    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.taskbar_hwnd = Some(taskbar.hwnd);
        s.tray_notify_hwnd = tray_notify;
        s.win_event_hook = hook;
        s.taskbar_index = index;
        s.embedded = true;
    }
    true
}

fn preferred_taskbar_index(
    taskbars: &[native_interop::TaskbarWindow],
    monitor_device: Option<&str>,
    legacy_index: usize,
) -> usize {
    match monitor_device {
        Some(device) => taskbars
            .iter()
            .position(|taskbar| {
                native_interop::taskbar_monitor_device(taskbar).as_deref() == Some(device)
            })
            .unwrap_or_else(|| {
                taskbars
                    .iter()
                    .position(|taskbar| taskbar.is_primary)
                    .unwrap_or(0)
            }),
        None => legacy_index.min(taskbars.len().saturating_sub(1)),
    }
}

fn refresh_taskbar_selection(hwnd: HWND) -> bool {
    let taskbars = native_interop::find_taskbars();
    if taskbars.is_empty() {
        return false;
    }
    let (preferred, legacy_index, current) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return false;
        };
        if s.dragging {
            return false;
        }
        (s.monitor_device.clone(), s.taskbar_index, s.taskbar_hwnd)
    };
    let index = preferred_taskbar_index(&taskbars, preferred.as_deref(), legacy_index);
    let taskbar = taskbars[index];
    current != Some(taskbar.hwnd) && attach_to_taskbar(hwnd, taskbar, index)
}

fn taskbar_at_point(pt: POINT) -> Option<(usize, native_interop::TaskbarWindow)> {
    native_interop::find_taskbars()
        .into_iter()
        .enumerate()
        .find(|(_, taskbar)| {
            pt.x >= taskbar.rect.left
                && pt.x < taskbar.rect.right
                && pt.y >= taskbar.rect.top
                && pt.y < taskbar.rect.bottom
        })
}

fn tray_left_for_taskbar(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    let mut tray_left = taskbar_rect.right;
    if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
        if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
            tray_left = tray_rect.left;
        }
    }
    tray_left
}

fn clamp_offset_for_taskbar(taskbar_hwnd: HWND, taskbar_rect: RECT, offset: i32) -> i32 {
    let tray_left = tray_left_for_taskbar(taskbar_hwnd, taskbar_rect);
    let max_offset = (tray_left - taskbar_rect.left - total_widget_width()).max(0);
    offset.clamp(0, max_offset)
}

fn offset_for_drop_point(
    taskbar_hwnd: HWND,
    taskbar_rect: RECT,
    pt: POINT,
    drag_start_client_x: i32,
) -> i32 {
    let tray_left = tray_left_for_taskbar(taskbar_hwnd, taskbar_rect);
    let desired_left = pt.x - taskbar_rect.left - drag_start_client_x;
    let offset = tray_left - taskbar_rect.left - total_widget_width() - desired_left;
    clamp_offset_for_taskbar(taskbar_hwnd, taskbar_rect, offset)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn update_check_interval() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

fn auto_update_check_due(last_update_check_unix: Option<u64>) -> bool {
    let Some(last_update_check_unix) = last_update_check_unix else {
        return true;
    };

    now_unix_secs().saturating_sub(last_update_check_unix) >= update_check_interval().as_secs()
}

fn schedule_auto_update_check(hwnd: HWND) {
    if !crate::privacy::updates_enabled() {
        unsafe {
            let _ = KillTimer(hwnd, TIMER_UPDATE_CHECK);
        }
        return;
    }
    let delay_ms = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };

        if auto_update_check_due(s.last_update_check_unix) {
            None
        } else {
            let elapsed = now_unix_secs().saturating_sub(s.last_update_check_unix.unwrap_or(0));
            let remaining_secs = update_check_interval().as_secs().saturating_sub(elapsed);
            Some((remaining_secs.saturating_mul(1000)).min(u32::MAX as u64) as u32)
        }
    };

    unsafe {
        let _ = KillTimer(hwnd, TIMER_UPDATE_CHECK);
        if let Some(delay_ms) = delay_ms {
            SetTimer(hwnd, TIMER_UPDATE_CHECK, delay_ms.max(1), None);
        }
    }
}

fn effective_poll_interval(state: &AppState) -> u32 {
    state
        .monitor
        .delay_ms(Instant::now(), state.poll_interval_ms)
}

const RESET_ROLLOVER_GRACE: Duration = Duration::from_secs(300);

/// A window's last reading relative to its reset time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowReset {
    /// No reset has passed since the reading was taken. This includes a
    /// reading taken after the reset that still reports it (the provider's
    /// own answer, not an old observation).
    Current,
    /// The reset passed after the reading, less than the grace ago.
    Rollover,
    /// The reset passed after the reading, longer ago: the old value no
    /// longer describes any current window.
    Passed,
}

fn window_reset(
    resets_at: Option<SystemTime>,
    observed_at: Option<SystemTime>,
    now: SystemTime,
) -> WindowReset {
    match (resets_at, observed_at) {
        (Some(reset), Some(observed)) if observed < reset && reset <= now => {
            if now.duration_since(reset).unwrap_or_default() < RESET_ROLLOVER_GRACE {
                WindowReset::Rollover
            } else {
                WindowReset::Passed
            }
        }
        _ => WindowReset::Current,
    }
}

fn window_text(
    section: &crate::models::UsageSection,
    observed_at: Option<SystemTime>,
    now: SystemTime,
    format: impl FnOnce(&crate::models::UsageSection) -> String,
) -> String {
    match window_reset(section.resets_at, observed_at, now) {
        WindowReset::Current => format(section),
        WindowReset::Rollover => {
            let mut previous = section.clone();
            previous.resets_at = None;
            let mut text = format(&previous);
            quota_refresh::mark_stale(&mut text, true);
            text
        }
        WindowReset::Passed => "--".to_string(),
    }
}

/// Tooltip lines for reset times and unavailable or rolled-over quota windows.
fn window_notes(
    usage: &crate::models::UsageData,
    observed_at: Option<SystemTime>,
    now: SystemTime,
    language: LanguageId,
) -> Vec<String> {
    let chinese = language == LanguageId::SimplifiedChinese;
    let mut notes = Vec::new();
    for (section, weekly) in [(&usage.session, false), (&usage.weekly, true)] {
        if !section.has_reading() {
            notes.push(quota_tooltip::message(chinese, weekly).to_string());
            continue;
        }
        if window_reset(section.resets_at, observed_at, now) == WindowReset::Current {
            if let Some(at) = format_precise_reset_time(section.resets_at.filter(|&t| t > now)) {
                let label = match (language, weekly) {
                    (LanguageId::SimplifiedChinese, false) => "5 小时额度重置",
                    (LanguageId::SimplifiedChinese, true) => "周额度重置",
                    (LanguageId::TraditionalChinese, false) => "5 小時額度重置",
                    (LanguageId::TraditionalChinese, true) => "每週額度重置",
                    (_, false) => "5-hour reset",
                    (_, true) => "Weekly reset",
                };
                notes.push(format!("{label} · {at}"));
            }
            continue;
        }
        let at = format_precise_reset_time(section.resets_at).unwrap_or_default();
        notes.push(match (chinese, weekly) {
            (true, false) => format!("5 小时限额已于 {at} 重置（晚于上次读数），正在等待新读数。"),
            (true, true) => format!("每周限额已于 {at} 重置（晚于上次读数），正在等待新读数。"),
            (false, false) => format!(
                "The 5-hour window reset at {at}, after the last reading. Waiting for a new reading."
            ),
            (false, true) => format!(
                "The weekly window reset at {at}, after the last reading. Waiting for a new reading."
            ),
        });
    }
    notes
}

fn refresh_usage_texts(state: &mut AppState) -> bool {
    let strings = state.language.strings();
    let Some(data) = state.data.as_ref() else {
        return false;
    };
    let mut changed = false;
    let now = SystemTime::now();
    let local_today = (state.appearance.reset_time_display == ResetTimeDisplay::Clock)
        .then(|| native_interop::system_time_to_local(now))
        .flatten();
    for (id, visible, usage, session, weekly) in [
        (
            0,
            state.show_claude_code,
            data.claude_code.as_ref(),
            &mut state.session_text,
            &mut state.weekly_text,
        ),
        (
            1,
            state.show_codex,
            data.codex.as_ref(),
            &mut state.codex_session_text,
            &mut state.codex_weekly_text,
        ),
        (
            2,
            state.show_antigravity,
            data.antigravity.as_ref(),
            &mut state.antigravity_session_text,
            &mut state.antigravity_weekly_text,
        ),
    ] {
        if !visible {
            continue;
        }
        let service = &state.monitor.services[id];
        let antigravity = id == 2;
        if let Some(usage) = usage.filter(|_| !antigravity || service.error.is_none()) {
            let observed_at = service.history.last_success;
            let stale = service.stale(now);
            for (weekly, output, section) in [
                (false, session, &usage.session),
                (true, weekly, &usage.weekly),
            ] {
                let mut text = window_text(section, observed_at, now, |section| {
                    if let Some(today) = local_today {
                        clock_quota_line(section, state.quota_display_mode, today, weekly)
                    } else {
                        poller::format_quota_line(section, strings, state.quota_display_mode, now)
                    }
                });
                quota_refresh::mark_stale(&mut text, stale);
                changed |= *output != text;
                *output = text;
            }
        } else {
            let label = if antigravity {
                "--"
            } else {
                service
                    .error
                    .map(|error| poll_error_display_label(error, state.language))
                    .unwrap_or("...")
            };
            changed |= session != label || weekly != label;
            *session = label.to_string();
            *weekly = label.to_string();
        }
    }
    changed
}

fn set_window_title(hwnd: HWND, strings: Strings) {
    unsafe {
        let title = native_interop::wide_str(strings.window_title);
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(title.as_ptr()));
    }
}

fn show_info_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_error_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn show_update_prompt(hwnd: HWND, strings: Strings, release: &ReleaseDescriptor) -> bool {
    let message = strings
        .update_prompt_now
        .replace("{version}", &release.latest_version);

    unsafe {
        let title_wide = native_interop::wide_str(strings.update_available);
        let message_wide = native_interop::wide_str(&message);
        MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

fn language_command(language: LanguageId) -> u16 {
    match language {
        LanguageId::English => IDM_LANG_ENGLISH,
        LanguageId::Dutch => IDM_LANG_DUTCH,
        LanguageId::Spanish => IDM_LANG_SPANISH,
        LanguageId::French => IDM_LANG_FRENCH,
        LanguageId::German => IDM_LANG_GERMAN,
        LanguageId::Italian => IDM_LANG_ITALIAN,
        LanguageId::Japanese => IDM_LANG_JAPANESE,
        LanguageId::Korean => IDM_LANG_KOREAN,
        LanguageId::SimplifiedChinese => IDM_LANG_SIMPLIFIED_CHINESE,
        LanguageId::TraditionalChinese => IDM_LANG_TRADITIONAL_CHINESE,
        LanguageId::Russian => IDM_LANG_RUSSIAN,
        LanguageId::PortugueseBrazil => IDM_LANG_PORTUGUESE_BRAZIL,
    }
}

fn apply_language_to_state(state: &mut AppState, language_override: Option<LanguageId>) {
    state.language_override = language_override;
    state.language = localization::resolve_language(language_override);
    set_window_title(state.hwnd.to_hwnd(), state.language.strings());
    refresh_usage_texts(state);
}

fn update_language_change() -> bool {
    let mut state = lock_state();
    let Some(app_state) = state.as_mut() else {
        return false;
    };

    if app_state.language_override.is_some() {
        return false;
    }

    let new_language = localization::detect_system_language();
    if new_language == app_state.language {
        return false;
    }

    apply_language_to_state(app_state, None);
    true
}

fn begin_update_check(hwnd: HWND, interactive: bool) {
    if !crate::privacy::updates_enabled() {
        return;
    }
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let (strings, install_channel) = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            if interactive {
                show_info_message(
                    hwnd,
                    app_state.language.strings().updates,
                    app_state.language.strings().update_in_progress,
                );
            }
            return;
        }

        app_state.update_status = UpdateStatus::Checking;
        (app_state.language.strings(), app_state.install_channel)
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        let checked_at = now_unix_secs();
        match updater::check_for_updates() {
            Ok(UpdateCheckResult::UpToDate) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::UpToDate;
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive {
                    show_info_message(hwnd, strings.updates, strings.up_to_date);
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
            Ok(UpdateCheckResult::Available(release)) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release.clone());
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive && show_update_prompt(hwnd, strings, &release) {
                    match install_channel {
                        InstallChannel::Portable => begin_update_apply(hwnd, release),
                        InstallChannel::Winget => begin_winget_update(hwnd),
                    }
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Idle;
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive {
                    let message = format!("{}.\n\n{}", strings.update_failed, error);
                    show_error_message(hwnd, strings.updates, &message);
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
}

fn begin_update_apply(hwnd: HWND, release: ReleaseDescriptor) {
    if !crate::privacy::updates_enabled() {
        return;
    }
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let strings = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            show_info_message(
                hwnd,
                app_state.language.strings().updates,
                app_state.language.strings().update_in_progress,
            );
            return;
        }

        app_state.update_status = UpdateStatus::Applying;
        app_state.language.strings()
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        match updater::begin_self_update(&release) {
            Ok(()) => unsafe {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release);
                    }
                }
                let message = format!("{}.\n\n{}", strings.update_failed, error);
                show_error_message(hwnd, strings.updates, &message);
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
}

fn begin_winget_update(hwnd: HWND) {
    if !crate::privacy::updates_enabled() {
        return;
    }
    let strings = {
        let state = lock_state();
        state.as_ref().map(|s| s.language.strings())
    }
    .unwrap_or(LanguageId::English.strings());

    match updater::begin_winget_update() {
        Ok(()) => unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        Err(error) => {
            let message = format!("{}.\n\n{}", strings.update_failed, error);
            show_error_message(hwnd, strings.updates, &message);
        }
    }
}

const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "CodexUsage";
const LEGACY_STARTUP_REGISTRY_KEY: &str = "ClaudeCodeUsageMonitor";
const STARTUP_APPROVAL_PATH: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
static STARTUP_WRITE_FAILED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
enum StartupStatus {
    Off,
    Enabled,
    Blocked,
}

fn startup_status() -> StartupStatus {
    let registered = read_startup_value(STARTUP_REGISTRY_KEY)
        .zip(current_exe_path_string())
        .is_some_and(|(command, exe)| startup_command_matches(&command, &exe));
    if !registered {
        StartupStatus::Off
    } else if startup_blocked_by_windows() {
        StartupStatus::Blocked
    } else {
        StartupStatus::Enabled
    }
}

fn startup_blocked_by_windows() -> bool {
    let mut entry = [0u8; 12];
    let mut size = entry.len() as u32;
    let name = native_interop::wide_str(STARTUP_REGISTRY_KEY);
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            STARTUP_APPROVAL_PATH,
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(entry.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    result.is_ok() && startup_approval_is_disabled(&entry[..size as usize])
}

fn startup_approval_is_disabled(entry: &[u8]) -> bool {
    matches!(entry.get(..4), Some([3 | 7, 0, 0, 0]))
}

fn startup_command_matches(value: &str, exe: &str) -> bool {
    value.trim().trim_matches('"').eq_ignore_ascii_case(exe)
}

fn current_exe_path_string() -> Option<String> {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        (len > 0).then(|| String::from_utf16_lossy(&exe_buf[..len]))
    }
}

fn read_startup_value(key: &str) -> Option<String> {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let key_name = native_interop::wide_str(key);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        );
        if result.is_err() {
            return None;
        }

        // Query the size of the value
        let mut data_size: u32 = 0;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut data_size),
        );
        if result.is_err() || data_size == 0 {
            let _ = RegCloseKey(hkey);
            return None;
        }

        // Read the value
        let mut buf = vec![0u8; data_size as usize];
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut data_size),
        );
        let _ = RegCloseKey(hkey);
        if result.is_err() {
            return None;
        }

        // Convert the registry value (UTF-16) to a string
        let wide_slice =
            std::slice::from_raw_parts(buf.as_ptr() as *const u16, data_size as usize / 2);
        Some(
            String::from_utf16_lossy(wide_slice)
                .trim_end_matches('\0')
                .to_string(),
        )
    }
}

fn delete_startup_value(key: &str) {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let key_name = native_interop::wide_str(key);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
        .is_ok()
        {
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
            let _ = RegCloseKey(hkey);
        }
    }
}

fn migrate_legacy_startup_entry() {
    let legacy_exists = read_startup_value(LEGACY_STARTUP_REGISTRY_KEY).is_some();
    let current_exists = read_startup_value(STARTUP_REGISTRY_KEY).is_some();
    if !legacy_exists {
        return;
    }

    if should_write_migrated_startup(legacy_exists, current_exists) {
        let _ = set_startup_enabled(true);
    }

    if read_startup_value(STARTUP_REGISTRY_KEY).is_some() {
        delete_startup_value(LEGACY_STARTUP_REGISTRY_KEY);
        diagnose::log("migrated legacy startup registry entry to CodexUsage");
    }
}

fn should_write_migrated_startup(legacy_exists: bool, current_exists: bool) -> bool {
    legacy_exists && !current_exists
}

fn set_startup_enabled(enable: bool) -> bool {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let mut hkey = HKEY::default();
        let result = if enable {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if result.is_err() {
            return !enable && result == ERROR_FILE_NOT_FOUND;
        }

        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);
        let written = if enable {
            current_exe_path_string().is_some_and(|exe| {
                let command = native_interop::wide_str(&format!("\"{exe}\""));
                RegSetValueExW(
                    hkey,
                    PCWSTR::from_raw(key_name.as_ptr()),
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        command.as_ptr() as *const u8,
                        command.len() * 2,
                    )),
                )
                .is_ok()
            })
        } else {
            let removed = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
            let legacy_key_name = native_interop::wide_str(LEGACY_STARTUP_REGISTRY_KEY);
            let legacy_removed = RegDeleteValueW(hkey, PCWSTR::from_raw(legacy_key_name.as_ptr()));
            [removed, legacy_removed]
                .into_iter()
                .all(|result| result.is_ok() || result == ERROR_FILE_NOT_FOUND)
        };

        let _ = RegCloseKey(hkey);
        written
            && if enable {
                startup_status() != StartupStatus::Off
            } else {
                read_startup_value(STARTUP_REGISTRY_KEY).is_none()
            }
    }
}

// Dimensions matching the C# version
const SEGMENT_W: i32 = 10;
const MULTI_PROVIDER_SEGMENT_W: i32 = 8;
const SEGMENT_GAP: i32 = 1;
const SEGMENT_COUNT: i32 = 10;

const LEFT_DIVIDER_W: i32 = 3;
const DIVIDER_RIGHT_MARGIN: i32 = 10;
const LABEL_WIDTH: i32 = 18;
const LABEL_RIGHT_MARGIN: i32 = 10;
const BAR_RIGHT_MARGIN: i32 = 4;
const TEXT_WIDTH: i32 = 110;
const SIMPLIFIED_CHINESE_LABEL_WIDTH: i32 = 20;
const SIMPLIFIED_CHINESE_TEXT_WIDTH: i32 = 138;
const MODEL_RIGHT_MARGIN: i32 = 9;
const RIGHT_MARGIN: i32 = 4;
const WIDGET_HEIGHT: i32 = 46;

fn is_drag_handle_point(client_x: i32, client_y: i32) -> bool {
    let divider_h = sc(25);
    let height = {
        let state = lock_state();
        state
            .as_ref()
            .and_then(|s| unsafe {
                let mut rect = RECT::default();
                GetClientRect(s.hwnd.to_hwnd(), &mut rect)
                    .ok()
                    .map(|_| rect.bottom)
            })
            .unwrap_or(sc(WIDGET_HEIGHT))
    };
    let divider_top = (height - divider_h) / 2;
    client_x >= 0
        && client_x < sc(LEFT_DIVIDER_W)
        && client_y >= divider_top
        && client_y < divider_top + divider_h
}

fn cursor_is_on_drag_handle(hwnd: HWND) -> bool {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() || !ScreenToClient(hwnd, &mut pt).as_bool() {
            return false;
        }
        is_drag_handle_point(pt.x, pt.y)
    }
}

fn active_model_count(show_claude_code: bool, show_codex: bool, show_antigravity: bool) -> i32 {
    (show_claude_code as i32 + show_codex as i32 + show_antigravity as i32).max(1)
}

fn row_bar_segment_count(active_models: i32) -> i32 {
    match active_models {
        1 => SEGMENT_COUNT,
        2 => 5,
        _ => 4,
    }
}

/// Labels and reset texts that can occupy the widest space in each column.
fn usage_layout_samples(
    language: LanguageId,
    reset_time_display: ResetTimeDisplay,
    cross_day: bool,
) -> (Vec<&'static str>, Vec<String>) {
    let strings = language.strings();
    let labels = vec![strings.session_window, strings.weekly_window];
    if reset_time_display == ResetTimeDisplay::Clock {
        let mut resets = vec!["88/88 *".into(), "88:88 *".into()];
        if cross_day {
            resets.push("88/88 88:88 *".into());
        }
        return (labels, resets);
    }
    let resets = [
        strings.day_suffix,
        strings.hour_suffix,
        strings.minute_suffix,
        strings.second_suffix,
    ]
    .into_iter()
    .map(|suffix| format!("99{suffix} *"))
    .chain(std::iter::once(format!("{} *", strings.now)))
    .collect();
    (labels, resets)
}

unsafe fn create_widget_font(appearance: Appearance) -> HFONT {
    let font_name = native_interop::wide_str("Microsoft YaHei UI");
    CreateFontW(
        -sc(appearance.font_px()),
        0,
        0,
        0,
        if appearance.palette == Palette::System {
            FW_SEMIBOLD.0 as i32
        } else {
            FW_BOLD.0 as i32
        },
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_TT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
        PCWSTR::from_raw(font_name.as_ptr()),
    )
}

/// Label and quota-text column widths in physical pixels, measured with the
/// fonts used for drawing so larger text and longer translations still fit.
fn usage_layout_widths(
    language: LanguageId,
    appearance: Appearance,
    cross_day: bool,
) -> (i32, i32) {
    let reset_x = if appearance.shows_percent_in_battery() {
        0
    } else {
        sc(quota_text::reset_offset(appearance.font_px()))
    };
    let (fallback_label, fallback_text) = if language == LanguageId::SimplifiedChinese {
        (
            sc(SIMPLIFIED_CHINESE_LABEL_WIDTH),
            sc(SIMPLIFIED_CHINESE_TEXT_WIDTH),
        )
    } else {
        (sc(LABEL_WIDTH), sc(TEXT_WIDTH))
    };
    let fallback = (
        fallback_label,
        if appearance.reset_time_display == ResetTimeDisplay::Clock {
            fallback_text.max(reset_x + sc(13 * appearance.font_px() + 3))
        } else {
            fallback_text
        },
    );
    unsafe {
        let hdc = GetDC(HWND::default());
        if hdc.is_invalid() {
            return fallback;
        }
        let measure = |font: HFONT, values: &mut dyn Iterator<Item = &str>| {
            let old_font = SelectObject(hdc, font);
            let widest = values
                .map(|value| {
                    let wide: Vec<u16> = value.encode_utf16().collect();
                    let mut size = SIZE::default();
                    if GetTextExtentPoint32W(hdc, &wide, &mut size).as_bool() {
                        size.cx
                    } else {
                        0
                    }
                })
                .max()
                .unwrap_or(0);
            SelectObject(hdc, old_font);
            let _ = DeleteObject(font);
            widest
        };
        let (labels, resets) =
            usage_layout_samples(language, appearance.reset_time_display, cross_day);
        let label_font = create_widget_font(appearance);
        let reset_font = quota_text::create_reset_font(sc(appearance.font_px()));
        let widths = if label_font.is_invalid() || reset_font.is_invalid() {
            let _ = DeleteObject(label_font);
            let _ = DeleteObject(reset_font);
            fallback
        } else {
            let label_width = measure(label_font, &mut labels.into_iter());
            let reset_width = measure(reset_font, &mut resets.iter().map(String::as_str));
            if label_width > 0 && reset_width > 0 {
                (label_width + sc(2), reset_x + reset_width + sc(3))
            } else {
                fallback
            }
        };
        ReleaseDC(HWND::default(), hdc);
        widths
    }
}

fn period_label_column_width(label_width: i32, appearance: Appearance) -> i32 {
    if appearance.hide_period_labels {
        0
    } else {
        label_width + sc(LABEL_RIGHT_MARGIN)
    }
}

fn total_widget_width_for(active_models: i32, language: LanguageId, appearance: Appearance) -> i32 {
    widget_width_from_columns(
        active_models,
        appearance,
        usage_layout_widths(language, appearance, false),
    )
}

fn widget_width_from_columns(
    active_models: i32,
    appearance: Appearance,
    (label_width, text_width): (i32, i32),
) -> i32 {
    let bar_segments = row_bar_segment_count(active_models);
    let model_width = model_usage_width(bar_segments, text_width, appearance);

    sc(LEFT_DIVIDER_W)
        + sc(DIVIDER_RIGHT_MARGIN)
        + period_label_column_width(label_width, appearance)
        + model_width * active_models
        + sc(MODEL_RIGHT_MARGIN) * (active_models - 1)
        + sc(RIGHT_MARGIN)
}

fn total_widget_width_for_state(state: &AppState) -> i32 {
    widget_width_from_columns(
        active_model_count(
            state.show_claude_code,
            state.show_codex,
            state.show_antigravity,
        ),
        state.appearance,
        current_usage_layout_widths(state),
    )
}

fn has_cross_day_reset(
    appearance: Appearance,
    session_visible: bool,
    providers: [bool; 3],
    session: [&str; 3],
) -> bool {
    appearance.reset_time_display == ResetTimeDisplay::Clock
        && session_visible
        && providers.into_iter().zip(session).any(|(visible, value)| {
            let reset = quota_text::parts(value).1.as_bytes();
            // Cross-day session labels start with MM/DD HH:MM.
            visible && reset.get(2) == Some(&b'/') && reset.get(8) == Some(&b':')
        })
}

fn current_usage_layout_widths(state: &AppState) -> (i32, i32) {
    let cross_day = has_cross_day_reset(
        state.appearance,
        state.show_session_window,
        [
            state.show_claude_code,
            state.show_codex,
            state.show_antigravity,
        ],
        [
            &state.session_text,
            &state.codex_session_text,
            &state.antigravity_session_text,
        ],
    );
    usage_layout_widths(state.language, state.appearance, cross_day)
}

fn total_widget_width() -> i32 {
    lock_state()
        .as_ref()
        .map(total_widget_width_for_state)
        .unwrap_or_else(|| total_widget_width_for(1, LanguageId::English, Appearance::default()))
}

fn claude_accent_color() -> Color {
    Color::from_hex("#D97757")
}

fn appearance_colors(appearance: Appearance, system_is_dark: bool) -> (Color, Color, Color) {
    match appearance.palette {
        Palette::System if system_is_dark => (
            Color::from_hex("#1C1C1C"),
            Color::from_hex("#888888"),
            Color::from_hex("#444444"),
        ),
        Palette::System => (
            Color::from_hex("#F3F3F3"),
            Color::from_hex("#404040"),
            Color::from_hex("#AAAAAA"),
        ),
        Palette::HighContrastDark => (
            Color::from_hex("#1C1C1C"),
            Color::from_hex("#FFFFFF"),
            Color::from_hex("#30343B"),
        ),
        Palette::HighContrastLight => (
            Color::from_hex("#F3F3F3"),
            Color::from_hex("#101820"),
            Color::from_hex("#A0A8B2"),
        ),
    }
}

/// Reset times stay quieter than percentages; high-contrast palettes keep them legible.
fn reset_text_color(appearance: Appearance, is_dark: bool) -> Color {
    Color::from_hex(match appearance.palette {
        Palette::System if is_dark => "#B5B5B5",
        Palette::System => "#666666",
        Palette::HighContrastDark => "#D8D8D8",
        Palette::HighContrastLight => "#38424C",
    })
}

fn antigravity_accent_color() -> Color {
    Color::from_hex("#4285F4")
}

fn claude_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#F09A7A")
    } else {
        Color::from_hex("#A94F32")
    }
}

fn antigravity_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#8AB4F8")
    } else {
        Color::from_hex("#1967D2")
    }
}

pub fn run() {
    // Enable Per-Monitor DPI Awareness V2 for crisp rendering at any scale factor
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CURRENT_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
    }
    diagnose::log("window::run started");

    // Single-instance guard: silently exit if another instance is running.
    // Exception: when relaunched after an explorer restart (ENV_RELAUNCH set),
    // wait for the previous instance to release the mutex, then take over.
    let is_relaunch = std::env::var(ENV_RELAUNCH).is_ok();
    let mutex_name = native_interop::wide_str("Global\\CodexUsage");
    let _mutex = unsafe {
        let handle = CreateMutexW(None, true, PCWSTR::from_raw(mutex_name.as_ptr()));
        match handle {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    if is_relaunch {
                        diagnose::log("relaunch: waiting for previous instance to exit");
                        let wait_result = WaitForSingleObject(h, 10_000);
                        if wait_result != WAIT_OBJECT_0 && wait_result != WAIT_ABANDONED {
                            diagnose::log(format!(
                                "startup aborted: previous instance did not exit cleanly ({wait_result:?})"
                            ));
                            return;
                        }
                    } else {
                        diagnose::log("startup aborted: another instance is already running");
                        return;
                    }
                }
                h
            }
            Err(error) => {
                diagnose::log_error(
                    "startup aborted: unable to create single-instance mutex",
                    error,
                );
                return;
            }
        }
    };

    migrate_legacy_startup_entry();

    let class_name = native_interop::wide_str("CodexUsage");

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let (large_icon, small_icon) = load_embedded_app_icons();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            hIcon: large_icon,
            hIconSm: small_icon,
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            diagnose::log("RegisterClassExW returned 0");
        }

        let settings = load_settings();
        settings_window::apply_menu_theme(settings.settings_theme);
        let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
        let language = localization::resolve_language(language_override);
        let install_channel = updater::current_install_channel();

        // Create as layered popup (will be reparented into taskbar)
        let title = native_interop::wide_str(language.strings().window_title);
        let initial_model_count = active_model_count(
            settings.show_claude_code,
            settings.show_codex,
            settings.show_antigravity,
        );
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            total_widget_width_for(initial_model_count, language, settings.appearance),
            sc(WIDGET_HEIGHT),
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .unwrap();

        if !large_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as usize),
                LPARAM(large_icon.0 as isize),
            );
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_SMALL as usize),
                LPARAM(small_icon.0 as isize),
            );
        }

        diagnose::log(format!("main window created hwnd={:?}", hwnd));

        let is_dark = theme::is_dark_mode();
        let mut embedded = false;

        {
            let mut state = lock_state();
            *state = Some(AppState {
                hwnd: SendHwnd::from_hwnd(hwnd),
                taskbar_hwnd: None,
                tray_notify_hwnd: None,
                win_event_hook: None,
                is_dark,
                embedded: false,
                language_override,
                language,
                appearance: settings.appearance,
                quota_display_mode: settings.quota_display_mode.unwrap_or_default(),
                settings_theme: settings.settings_theme,
                install_channel,
                session_percent: 0.0,
                session_text: "--".to_string(),
                weekly_percent: 0.0,
                weekly_text: "--".to_string(),
                codex_session_percent: 0.0,
                codex_session_text: "--".to_string(),
                codex_weekly_percent: 0.0,
                codex_weekly_text: "--".to_string(),
                antigravity_session_percent: 0.0,
                antigravity_session_text: "--".to_string(),
                antigravity_weekly_percent: 0.0,
                antigravity_weekly_text: "--".to_string(),
                show_claude_code: settings.show_claude_code,
                show_codex: settings.show_codex,
                show_antigravity: settings.show_antigravity,
                antigravity_quota_group: settings.antigravity_quota_group,
                antigravity_groups: Vec::new(),

                show_session_window: settings.show_session_window,
                show_weekly_window: settings.show_weekly_window,
                alert_threshold_percent: settings.alert_threshold_percent,
                notified_quota_windows: settings.notified_quota_windows.into_iter().collect(),
                notify_on_reset: settings.notify_on_reset,
                reset_observations: settings.reset_observations,
                mcp_enabled: settings.mcp_server_enabled,
                mcp_port: settings.mcp_port,
                mcp_unavailable: false,
                data: None,
                poll_interval_ms: settings.poll_interval_ms,
                adaptive_refresh: settings.adaptive_refresh,
                monitor: provider_poll::Monitor::new(
                    [
                        settings.show_claude_code,
                        settings.show_codex,
                        settings.show_antigravity,
                    ],
                    Instant::now(),
                ),
                recovery_watch: Some(recovery_events::Watch::register(hwnd)),
                recovery_debounce: recovery_events::Debounce::default(),
                has_poll_result: false,
                update_status: UpdateStatus::Idle,
                last_update_check_unix: settings.last_update_check_unix,
                taskbar_index: settings.taskbar_index,
                monitor_device: settings.monitor_device.clone(),
                tray_offset: settings.tray_offset,
                anchor_left: settings.anchor_left,
                dragging: false,
                drag_start_mouse_x: 0,
                drag_start_client_x: 0,
                drag_start_offset: 0,
                widget_visible: settings.widget_visible,
            });
        }

        // Try to embed in taskbar
        let taskbars = native_interop::find_taskbars();
        let selected_index = preferred_taskbar_index(
            &taskbars,
            settings.monitor_device.as_deref(),
            settings.taskbar_index,
        );
        if taskbars
            .get(selected_index)
            .is_some_and(|taskbar| attach_to_taskbar(hwnd, *taskbar, selected_index))
        {
            embedded = true;
        } else {
            // During login Explorer may create its taskbar after this app starts.
            SetTimer(hwnd, TIMER_TASKBAR_RETRY, 2_000, None);
        }

        // If not embedded, fall back to topmost popup with SetLayeredWindowAttributes
        if !embedded {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }

        // Register system tray icon(s)
        sync_tray_icons(hwnd);

        // Position and show (only if widget_visible preference is true)
        position_at_taskbar();
        if settings.widget_visible {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        diagnose::log("window shown");

        // Initial render via UpdateLayeredWindow (for embedded) or InvalidateRect (fallback)
        render_layered();

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| s.poll_interval_ms)
                .unwrap_or(POLL_15_MIN)
        };
        SetTimer(hwnd, TIMER_POLL, initial_poll_ms, None);
        SetTimer(hwnd, TIMER_FRESHNESS, 30_000, None);

        // Watch for explorer.exe restarts so we can re-embed and re-add the tray
        // icon (the shell discards tray registrations when it restarts). This
        // runs on a dedicated thread, NOT a window timer: once explorer destroys
        // the taskbar, our embedded child window stops receiving all messages
        // (WM_TIMER included), so a timer would never fire again.
        spawn_taskbar_watchdog();

        // Initial poll
        let send_hwnd = SendHwnd::from_hwnd(hwnd);
        request_poll(send_hwnd, false);
        apply_mcp_setting();

        schedule_auto_update_check(hwnd);
        let should_check_updates = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| auto_update_check_due(s.last_update_check_unix))
                .unwrap_or(false)
        };
        if should_check_updates {
            begin_update_check(hwnd, false);
        }

        // Initial theme check
        check_theme_change();

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
            if !settings_window::handle_dialog_message(&msg) {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        settings_window::close();
        mcp_server::stop();
    }
}

/// Explain missing and expired quota in the provider’s hover region.
/// The optional MCP server's port: `mcp_port` in settings, else the default.
fn mcp_port(setting: Option<u16>) -> u16 {
    setting
        .filter(|port| *port >= 1024)
        .unwrap_or(mcp_server::DEFAULT_PORT)
}

/// Start or stop the MCP server to match the setting. Polling is unaffected:
/// the server only copies the monitor's state when a tool is called.
fn apply_mcp_setting() {
    let Some((enabled, port)) = lock_state()
        .as_ref()
        .map(|s| (s.mcp_enabled, mcp_port(s.mcp_port)))
    else {
        return;
    };
    let unavailable = if enabled {
        match mcp_server::start(port, mcp_snapshot) {
            Ok(()) => false,
            Err(error) => {
                diagnose::log_error("MCP server could not start", error);
                true
            }
        }
    } else {
        mcp_server::stop();
        false
    };
    if let Some(s) = lock_state().as_mut() {
        s.mcp_unavailable = unavailable;
    }
}

fn mcp_settings_hint(language: LanguageId, unavailable: bool, port: u16) -> String {
    let strings = language.strings();
    let template = if unavailable {
        strings.mcp_port_unavailable
    } else {
        strings.mcp_server_hint
    };
    template.replace("{port}", &port.to_string())
}

fn mcp_snapshot() -> mcp_server::Snapshot {
    let state = lock_state();
    let s = state.as_ref()?;
    // Tool order: Claude Code (service 0), then Codex (service 1).
    Some([0, 1].map(|id| mcp_server::ProviderRecord::from_service(&s.monitor.services[id])))
}

fn update_quota_tooltips() {
    let (hwnd, regions) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else { return };
        let mut regions = Vec::new();
        if !s.dragging {
            let chinese = s.language == LanguageId::SimplifiedChinese;
            let count = active_model_count(s.show_claude_code, s.show_codex, s.show_antigravity);
            let (label_width, text_width) = current_usage_layout_widths(s);
            let width = model_usage_width(row_bar_segment_count(count), text_width, s.appearance);
            let mut left = sc(LEFT_DIVIDER_W)
                + sc(DIVIDER_RIGHT_MARGIN)
                + period_label_column_width(label_width, s.appearance);
            let antigravity_name = antigravity_label(s);
            for (id, visible, name, session) in [
                (0, s.show_claude_code, "Claude Code", &s.session_text),
                (1, s.show_codex, "Codex", &s.codex_session_text),
                (
                    2,
                    s.show_antigravity,
                    antigravity_name.as_str(),
                    &s.antigravity_session_text,
                ),
            ] {
                if !visible {
                    continue;
                }
                let service = &s.monitor.services[id];
                let now = SystemTime::now();
                let description = service.description(now, Instant::now(), chinese);
                let mut text = format!("{name}\n{description}");
                if service.error.is_some() && session == "!" {
                    text.push_str(if chinese {
                        "\n本服务本次未返回额度。"
                    } else {
                        "\nThis provider did not return quota in the latest update."
                    });
                }
                if let Some(usage) = service
                    .data
                    .as_ref()
                    .filter(|_| id != 2 || service.error.is_none())
                {
                    for note in window_notes(usage, service.history.last_success, now, s.language) {
                        text.push('\n');
                        text.push_str(&note);
                    }
                }
                regions.push((
                    RECT {
                        left,
                        top: 0,
                        right: left + width,
                        bottom: sc(WIDGET_HEIGHT),
                    },
                    text,
                ));
                left += width + sc(MODEL_RIGHT_MARGIN);
            }
        }
        (s.hwnd.to_hwnd(), regions)
    };
    quota_tooltip::sync(hwnd, regions);
}

/// Render opaque taskbar content with ClearType via UpdateLayeredWindow.
struct WidgetContent<'a> {
    system_dark: bool,
    appearance: Appearance,
    language: LanguageId,
    mode: QuotaDisplayMode,
    // Provider column order: Claude Code, Codex, Antigravity.
    providers: [bool; 3],
    visible_rows: [bool; 2],
    used: [[f64; 3]; 2],
    text: [[std::borrow::Cow<'a, str>; 3]; 2],
}

const WIDGET_PROVIDERS: [Provider; 3] = [Provider::Claude, Provider::Codex, Provider::Antigravity];

impl WidgetContent<'static> {
    fn from_state(s: &AppState) -> Self {
        let values = |texts: [&String; 3]| texts.map(|text| text.clone().into());
        Self {
            system_dark: s.is_dark,
            appearance: s.appearance,
            language: s.language,
            mode: s.quota_display_mode,
            providers: [s.show_claude_code, s.show_codex, s.show_antigravity],
            visible_rows: [s.show_session_window, s.show_weekly_window],
            used: [
                [
                    s.session_percent,
                    s.codex_session_percent,
                    s.antigravity_session_percent,
                ],
                [
                    s.weekly_percent,
                    s.codex_weekly_percent,
                    s.antigravity_weekly_percent,
                ],
            ],
            text: [
                values([
                    &s.session_text,
                    &s.codex_session_text,
                    &s.antigravity_session_text,
                ]),
                values([
                    &s.weekly_text,
                    &s.codex_weekly_text,
                    &s.antigravity_weekly_text,
                ]),
            ],
        }
    }
}

fn render_layered() {
    let needs_resize = {
        let state = lock_state();
        state.as_ref().is_some_and(|s| unsafe {
            let mut rect = RECT::default();
            GetClientRect(s.hwnd.to_hwnd(), &mut rect).is_ok()
                && rect.right - rect.left != total_widget_width_for_state(s)
        })
    };
    if needs_resize {
        position_at_taskbar();
    }
    update_quota_tooltips();

    let (hwnd_val, embedded, content) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else { return };
        (s.hwnd, s.embedded, WidgetContent::from_state(s))
    };

    let hwnd = hwnd_val.to_hwnd();

    // For non-embedded fallback, just invalidate and let WM_PAINT handle it
    if !embedded {
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
        return;
    }

    let mut client_rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client_rect);
    }
    let width = client_rect.right - client_rect.left;
    let height = client_rect.bottom - client_rect.top;
    if width <= 0 || height <= 0 {
        return;
    }

    unsafe {
        let screen_dc = GetDC(hwnd);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mem_dc = CreateCompatibleDC(screen_dc);
        let dib =
            CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();

        if dib.is_invalid() || bits.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, screen_dc);
            return;
        }

        let old_bmp = SelectObject(mem_dc, dib);
        let pixel_count = (width * height) as usize;

        // Render once with the actual taskbar background colour.
        // Using an opaque background lets us use CLEARTYPE_QUALITY for
        // sub-pixel font rendering that matches the rest of the OS.
        paint_content(mem_dc, width, height, &content);
        let _ = GdiFlush();
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u32>(), pixel_count);
        apply_widget_alpha(pixels, content.appearance, content.system_dark);

        // Push to window via UpdateLayeredWindow
        let pt_src = POINT { x: 0, y: 0 };
        let sz = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA
        };

        let rendered = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&sz),
            mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        if let Err(error) = rendered {
            diagnose::log_error("UpdateLayeredWindow failed", error);
        }

        // Cleanup
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

fn apply_widget_alpha(pixels: &mut [u32], appearance: Appearance, system_dark: bool) {
    let (background, _, _) = appearance_colors(appearance, system_dark);
    let background =
        u32::from(background.r) << 16 | u32::from(background.g) << 8 | u32::from(background.b);
    for pixel in pixels {
        let rgb = *pixel & 0x00FFFFFF;
        // System mode uses the underlying taskbar for the backdrop.
        *pixel = if appearance.palette == Palette::System && rgb == background {
            0x01000000
        } else {
            rgb | 0xFF000000
        };
    }
}

/// Paint all widget content onto a DC with a given background color.
fn paint_content(hdc: HDC, width: i32, height: i32, content: &WidgetContent<'_>) {
    let WidgetContent {
        system_dark,
        mut appearance,
        language,
        mode,
        providers,
        visible_rows,
        used,
        ref text,
    } = *content;
    let rows = visible_rows
        .into_iter()
        .filter(|visible| *visible)
        .count()
        .max(1) as i32;
    let is_dark = appearance.is_dark(system_dark);
    let (bg, text_color, track) = appearance_colors(appearance, system_dark);
    let cross_day = has_cross_day_reset(
        appearance,
        visible_rows[0],
        providers,
        text[0].each_ref().map(|value| value.as_ref()),
    );
    let (label_width, text_width) = usage_layout_widths(language, appearance, cross_day);
    let active_models = providers
        .into_iter()
        .filter(|visible| *visible)
        .count()
        .max(1) as i32;
    let segment_count = row_bar_segment_count(active_models);
    let bar_width = usage_bar_width(segment_count, appearance);
    let model_width =
        model_usage_width(segment_count, text_width, appearance) + sc(MODEL_RIGHT_MARGIN);
    appearance.font_size = appearance.font_size.fit(height, rows, sc);

    unsafe {
        let client_rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };

        let bg_brush = CreateSolidBrush(COLORREF(bg.to_colorref()));
        FillRect(hdc, &client_rect, bg_brush);
        let _ = DeleteObject(bg_brush);

        // Left divider
        let divider_h = sc(25).min(height);
        let divider_top = (height - divider_h) / 2;
        let divider_bottom = divider_top + divider_h;

        let (div_left, div_right) = if is_dark {
            ((80, 80, 80), (40, 40, 40))
        } else {
            ((160, 160, 160), (230, 230, 230))
        };

        let left_brush = CreateSolidBrush(COLORREF(native_interop::colorref(
            div_left.0, div_left.1, div_left.2,
        )));
        let left_rect = RECT {
            left: 0,
            top: divider_top,
            right: sc(2),
            bottom: divider_bottom,
        };
        FillRect(hdc, &left_rect, left_brush);
        let _ = DeleteObject(left_brush);

        let right_brush = CreateSolidBrush(COLORREF(native_interop::colorref(
            div_right.0,
            div_right.1,
            div_right.2,
        )));
        let right_rect = RECT {
            left: sc(2),
            top: divider_top,
            right: sc(3),
            bottom: divider_bottom,
        };
        FillRect(hdc, &right_rect, right_brush);
        let _ = DeleteObject(right_brush);

        let content_x = sc(LEFT_DIVIDER_W) + sc(DIVIDER_RIGHT_MARGIN);
        let row_height = appearance.row_height().min((height / rows).max(1));
        let row_gap = if rows == 2 {
            (height - 2 * row_height).clamp(0, sc(5))
        } else {
            0
        };
        let mut row_y = ((height - rows * row_height - (rows - 1) * row_gap) / 2).max(0);

        let _ = SetBkMode(hdc, TRANSPARENT);
        let font = create_widget_font(appearance);
        let old_font = SelectObject(hdc, font);

        let model_left = content_x + period_label_column_width(label_width, appearance);
        if !appearance.hide_provider_logos {
            let size = sc(provider_icons::SIZE).min(height);
            let mut icon_x = model_left;
            for (provider, visible) in WIDGET_PROVIDERS.into_iter().zip(providers) {
                if visible {
                    provider_icons::draw(hdc, icon_x, (height - size) / 2, size, provider);
                    icon_x += model_width;
                }
            }
        }

        let strings = language.strings();
        let provider_text_colors = active_models > 1 && appearance.palette == Palette::System;
        for (row, label) in [strings.session_window, strings.weekly_window]
            .into_iter()
            .enumerate()
        {
            if !visible_rows[row] {
                continue;
            }
            if !appearance.hide_period_labels {
                let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));
                let mut label_wide: Vec<u16> = label.encode_utf16().collect();
                let _ = DrawTextW(
                    hdc,
                    &mut label_wide,
                    &mut RECT {
                        left: content_x,
                        top: row_y,
                        right: content_x + label_width,
                        bottom: row_y + row_height,
                    },
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE,
                );
            }
            let mut model_x = model_left + provider_mark_width(appearance);
            for (column, provider) in WIDGET_PROVIDERS.into_iter().enumerate() {
                if !providers[column] {
                    continue;
                }
                let value_color = match provider {
                    Provider::Codex => codex_brand_color(is_dark),
                    Provider::Claude if provider_text_colors => claude_usage_text_color(is_dark),
                    Provider::Antigravity if provider_text_colors => {
                        antigravity_usage_text_color(is_dark)
                    }
                    _ => text_color,
                };
                let used = used[row][column];
                draw_usage_bar(
                    hdc,
                    model_x,
                    row_y,
                    row_height,
                    segment_count,
                    bar_width,
                    mode.percentage(used),
                    &text[row][column],
                    &appearance.bar_color(provider, used, is_dark),
                    &track,
                    &value_color,
                    text_width,
                    is_dark,
                    appearance,
                );
                model_x += model_width;
            }
            row_y += row_height + row_gap;
        }

        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);
    }
}

fn poll_error_display_label(error: poller::PollError, language: LanguageId) -> &'static str {
    match error {
        poller::PollError::AuthRequired
        | poller::PollError::NoCredentials
        | poller::PollError::TokenExpired => "!",
        poller::PollError::NetworkUnavailable => {
            if language == LanguageId::SimplifiedChinese {
                "网络"
            } else {
                "NET"
            }
        }
        poller::PollError::RateLimited(_) => {
            if language == LanguageId::SimplifiedChinese {
                "限流"
            } else {
                "429"
            }
        }
        poller::PollError::ServerError => {
            if language == LanguageId::SimplifiedChinese {
                "服务"
            } else {
                "5XX"
            }
        }
        poller::PollError::RequestFailed => {
            if language == LanguageId::SimplifiedChinese {
                "错误"
            } else {
                "ERR"
            }
        }
    }
}

fn request_poll(hwnd: SendHwnd, force: bool) {
    {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            let enabled = [s.show_claude_code, s.show_codex, s.show_antigravity];
            s.monitor.configure(enabled, Instant::now());
            if force {
                s.monitor.force(Instant::now(), true);
            }
        }
    }
    start_poll(hwnd, force);
}

fn start_poll(hwnd: SendHwnd, queue: bool) {
    let Some(mut guard) = quota_refresh::POLLS.request(queue) else {
        return;
    };
    if let Err(error) = std::thread::Builder::new()
        .name("quota-poll".into())
        .spawn(move || loop {
            do_poll(hwnd);
            if !guard.next() {
                break;
            }
        })
    {
        diagnose::log_error("unable to start quota poll", error);
    }
}

fn do_poll(send_hwnd: SendHwnd) {
    let jobs = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let (jobs, delay) = s.monitor.wake(Instant::now(), s.poll_interval_ms);
        // A Windows timer is periodic. Even an early wake with no jobs must
        // replace that period with the actual remaining time to the deadline.
        unsafe {
            SetTimer(send_hwnd.to_hwnd(), TIMER_POLL, delay, None);
        }
        jobs
    };
    if !jobs.is_empty() {
        unsafe {
            let _ = PostMessageW(
                send_hwnd.to_hwnd(),
                WM_APP_USAGE_UPDATED,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
    provider_poll::launch(
        jobs,
        |job| {
            let group = {
                let state = lock_state();
                let Some(state) = state.as_ref() else {
                    return provider_poll::Attempt::cancelled();
                };
                let enabled = [
                    state.show_claude_code,
                    state.show_codex,
                    state.show_antigravity,
                ];
                if !enabled[job.id] || !state.monitor.job_is_current(job) {
                    return provider_poll::Attempt::cancelled();
                }
                state.antigravity_quota_group.clone()
            };
            provider_poll::execute(
                job,
                || poller::poll_provider(job.id, group.as_deref()),
                poller::credential_watch_snapshot,
            )
        },
        move |job, attempt| complete_provider_poll(send_hwnd, job, attempt),
    );
}

fn collect_reset_alerts(
    state: &mut AppState,
    provider: usize,
    usage: &crate::models::UsageData,
    now: SystemTime,
    alerts: &mut Vec<QuotaAlert>,
) -> bool {
    if !state.notify_on_reset {
        return false;
    }
    let strings = state.language.strings();
    let (name, kind) = [
        (strings.claude_code_model, tray_icon::TrayIconKind::Claude),
        (strings.codex_model, tray_icon::TrayIconKind::Codex),
        (
            strings.antigravity_model,
            tray_icon::TrayIconKind::Antigravity,
        ),
    ][provider];
    let mut changed = false;
    for (weekly, section, window) in [
        (false, &usage.session, strings.session_window),
        (true, &usage.weekly, strings.weekly_window),
    ] {
        let (saved, reset) = state
            .reset_observations
            .observe(provider, weekly, section, now);
        changed |= saved;
        if reset {
            let (title, message) = if state.language == LanguageId::SimplifiedChinese {
                (
                    format!("{name} 额度已重置"),
                    format!("{window} 窗口的额度已恢复。"),
                )
            } else {
                (
                    format!("{name} quota reset"),
                    format!("The {window} quota is available again."),
                )
            };
            alerts.push(QuotaAlert {
                kind,
                title,
                message,
            });
        }
    }
    changed
}

fn complete_provider_poll(
    send_hwnd: SendHwnd,
    job: &provider_poll::Job,
    attempt: provider_poll::Attempt,
) {
    let hwnd = send_hwnd.to_hwnd();
    let (alerts, auth_notice, settings_changed) = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let completion = s.monitor.finish(
            &job,
            attempt,
            Instant::now(),
            SystemTime::now(),
            s.poll_interval_ms,
            s.adaptive_refresh,
        );
        if !completion.accepted {
            // A selection change can leave a new request waiting for the old
            // worker to finish. Release it without publishing the stale result.
            unsafe {
                SetTimer(hwnd, TIMER_POLL, effective_poll_interval(s), None);
            }
            return;
        }
        let mut fresh = AppUsageData::default();
        if completion.successful {
            let data = s.monitor.services[job.id].data.clone();
            match job.id {
                0 => fresh.claude_code = data,
                1 => fresh.codex = data,
                _ => {
                    if let Some(usage) = &data {
                        if !usage.antigravity_groups.is_empty() {
                            s.antigravity_groups = usage.antigravity_groups.clone();
                        }
                    }
                    fresh.antigravity = data;
                }
            }
        }
        let previous_keys = s.notified_quota_windows.clone();
        let mut alerts = collect_low_quota_alerts(s, &fresh);
        let mut settings_changed = previous_keys != s.notified_quota_windows;
        let usage = match job.id {
            0 => &fresh.claude_code,
            1 => &fresh.codex,
            _ => &fresh.antigravity,
        };
        if let Some(usage) = usage.as_ref() {
            settings_changed |=
                collect_reset_alerts(s, job.id, usage, SystemTime::now(), &mut alerts);
        }
        let cached = s.monitor.cached();
        for (usage, session, weekly) in [
            (
                cached.claude_code.as_ref(),
                &mut s.session_percent,
                &mut s.weekly_percent,
            ),
            (
                cached.codex.as_ref(),
                &mut s.codex_session_percent,
                &mut s.codex_weekly_percent,
            ),
            (
                cached.antigravity.as_ref(),
                &mut s.antigravity_session_percent,
                &mut s.antigravity_weekly_percent,
            ),
        ] {
            *session = usage.map_or(0.0, |u| u.session.percentage);
            *weekly = usage.map_or(0.0, |u| u.weekly.percentage);
        }
        s.data = Some(cached);
        s.has_poll_result = true;
        refresh_usage_texts(s);
        unsafe {
            SetTimer(hwnd, TIMER_POLL, effective_poll_interval(s), None);
        }
        let service = &s.monitor.services[job.id];
        diagnose::log(format!(
            "provider poll completed id={} error={:?} interval_ms={}",
            job.id, service.error, service.interval_ms
        ));
        let notice = completion.notify_auth.then(|| {
            let strings = s.language.strings();
            match job.id {
                0 => (
                    tray_icon::TrayIconKind::Claude,
                    strings.token_expired_title,
                    strings.token_expired_body,
                ),
                1 => (
                    tray_icon::TrayIconKind::Codex,
                    strings.codex_token_expired_title,
                    strings.codex_token_expired_body,
                ),
                _ => (
                    tray_icon::TrayIconKind::Antigravity,
                    strings.antigravity_token_expired_title,
                    strings.antigravity_token_expired_body,
                ),
            }
        });
        (alerts, notice, settings_changed)
    };
    for alert in &alerts {
        tray_icon::notify_balloon(hwnd, alert.kind, &alert.title, &alert.message);
    }
    if settings_changed {
        save_state_settings();
    }
    if let Some((kind, title, body)) = auth_notice {
        tray_icon::notify_balloon(hwnd, kind, title, body);
    }
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
    }
}

fn schedule_countdown_timer() {
    let mut state = lock_state();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };

    let hwnd = s.hwnd.to_hwnd();
    let Some(data) = s.data.as_ref() else {
        return;
    };
    let delays = [
        data.claude_code
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.session.resets_at)),
        data.claude_code
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.weekly.resets_at)),
        data.codex
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.session.resets_at)),
        data.codex
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.weekly.resets_at)),
        data.antigravity
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.session.resets_at)),
        data.antigravity
            .as_ref()
            .and_then(|usage| poller::time_until_display_change(usage.weekly.resets_at)),
    ];
    let min_delay = delays.into_iter().flatten().min();

    let ms = min_delay
        .unwrap_or(Duration::from_secs(60))
        .as_millis()
        .max(1000) as u32;

    unsafe {
        SetTimer(hwnd, TIMER_COUNTDOWN, ms, None);
    }
}

fn check_theme_change() {
    let new_dark = theme::is_dark_mode();
    let changed = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            if s.is_dark != new_dark {
                s.is_dark = new_dark;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if changed {
        render_layered();
    }
}

fn check_language_change() {
    if update_language_change() {
        render_layered();
    }
}

fn update_display() -> bool {
    lock_state().as_mut().is_some_and(refresh_usage_texts)
}

fn suppress_tray_reposition_for(duration: Duration) {
    let mut until = SUPPRESS_TRAY_REPOSITION_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *until = Some(Instant::now() + duration);
}

fn tray_reposition_is_suppressed() -> bool {
    let now = Instant::now();
    let mut until = SUPPRESS_TRAY_REPOSITION_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    match *until {
        Some(deadline) if now < deadline => true,
        Some(_) => {
            *until = None;
            false
        }
        None => false,
    }
}

fn position_at_taskbar() {
    refresh_dpi();
    // Drop the app-state lock before any Win32 call that may synchronously
    // re-enter our window procedure.
    let (hwnd, embedded, tray_offset, anchor_left, taskbar_hwnd) = {
        let state = lock_state();
        let s = match state.as_ref() {
            Some(s) => s,
            None => return,
        };

        // Don't fight the user's drag
        if s.dragging {
            return;
        }

        let taskbar_hwnd = match s.taskbar_hwnd {
            Some(h) => h,
            None => {
                diagnose::log("position_at_taskbar skipped: no taskbar handle");
                return;
            }
        };

        (
            s.hwnd.to_hwnd(),
            s.embedded,
            s.tray_offset,
            s.anchor_left,
            taskbar_hwnd,
        )
    };

    let taskbar_rect = match native_interop::get_taskbar_rect(taskbar_hwnd) {
        Some(r) => r,
        None => {
            diagnose::log("position_at_taskbar skipped: unable to query taskbar rect");
            return;
        }
    };

    let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
    let mut tray_left = taskbar_rect.right;
    let anchor_top = taskbar_rect.top;
    let anchor_height = taskbar_height;

    if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
        if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
            tray_left = tray_rect.left;
        }
    }

    let widget_width = total_widget_width();
    let max_offset = (tray_left - taskbar_rect.left - widget_width).max(0);
    let tray_offset = tray_offset.clamp(0, max_offset);
    let offset_changed = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            if !anchor_left && s.tray_offset != tray_offset {
                s.tray_offset = tray_offset;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if offset_changed {
        save_state_settings();
    }

    let widget_height = sc(WIDGET_HEIGHT).min(taskbar_height);
    let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
    if embedded {
        // Child window: coordinates relative to parent (taskbar)
        let x = compute_anchor_x(
            taskbar_rect.left,
            tray_left,
            widget_width,
            tray_offset,
            anchor_left,
        ) - taskbar_rect.left;
        native_interop::move_window(hwnd, x, y - taskbar_rect.top, widget_width, widget_height);
        diagnose::log(format!(
            "positioned embedded widget at x={x} y={} w={widget_width} h={widget_height}",
            y - taskbar_rect.top
        ));
    } else {
        // Topmost popup: screen coordinates
        let x = compute_anchor_x(
            taskbar_rect.left,
            tray_left,
            widget_width,
            tray_offset,
            anchor_left,
        );
        native_interop::move_window(hwnd, x, y, widget_width, widget_height);
        diagnose::log(format!(
            "positioned fallback widget at x={x} y={y} w={widget_width} h={widget_height}"
        ));
    }
}

fn compute_anchor_x(
    taskbar_left: i32,
    tray_left: i32,
    width: i32,
    offset: i32,
    anchor_left: bool,
) -> i32 {
    if anchor_left {
        taskbar_left
    } else {
        (tray_left - width - offset).max(taskbar_left)
    }
}

fn compute_anchor_y(anchor_top: i32, anchor_height: i32, widget_height: i32) -> i32 {
    anchor_top + (anchor_height - widget_height).max(0)
}

/// WinEvent callback for tray icon location changes
unsafe extern "system" fn on_tray_location_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    static LAST_REPOSITION: Mutex<Option<std::time::Instant>> = Mutex::new(None);

    let is_tray = {
        let state = lock_state();
        state
            .as_ref()
            .and_then(|s| s.tray_notify_hwnd)
            .map(|h| h == hwnd)
            .unwrap_or(false)
    };

    if is_tray {
        if tray_reposition_is_suppressed() {
            return;
        }

        let should_reposition = {
            let mut last = LAST_REPOSITION.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if last
                .map(|t| now.duration_since(t).as_millis() > 500)
                .unwrap_or(true)
            {
                *last = Some(now);
                true
            } else {
                false
            }
        };
        if should_reposition {
            position_at_taskbar();
            render_layered();
        }
    }
}

fn recover_polling(hwnd: HWND, network: bool) {
    let accepted = {
        let mut state = lock_state();
        state.as_mut().is_some_and(|s| {
            let now = Instant::now();
            if !s.recovery_debounce.accept(network, now) {
                return false;
            }
            s.monitor.force(now, false);
            true
        })
    };
    if accepted {
        diagnose::log(if network {
            "network restored: refreshing providers"
        } else {
            "system resumed: refreshing providers"
        });
        start_poll(SendHwnd::from_hwnd(hwnd), true);
    }
}

/// Main window procedure
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            // For non-embedded fallback, paint normally
            let embedded = {
                let state = lock_state();
                state.as_ref().map(|s| s.embedded).unwrap_or(false)
            };
            if embedded {
                // Layered windows don't use WM_PAINT; just validate the region
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            } else {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                paint(hdc, hwnd);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_POWERBROADCAST
            if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize
                || wparam.0 == PBT_APMRESUMESUSPEND as usize =>
        {
            recover_polling(hwnd, false);
            LRESULT(1)
        }
        _ if msg == recovery_events::MESSAGE => {
            recover_polling(hwnd, true);
            LRESULT(0)
        }
        WM_APP_TASKBARS_CHANGED => {
            if refresh_taskbar_selection(hwnd) {
                position_at_taskbar();
                render_layered();
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED_MSG | WM_SETTINGCHANGE => {
            if msg == WM_DPICHANGED_MSG {
                let new_dpi = (wparam.0 & 0xFFFF) as u32;
                CURRENT_DPI.store(new_dpi, Ordering::Relaxed);
            }
            if msg == WM_SETTINGCHANGE {
                check_theme_change();
                check_language_change();
            }
            refresh_dpi();
            if msg == WM_DISPLAYCHANGE {
                refresh_taskbar_selection(hwnd);
            }
            position_at_taskbar();
            render_layered();
            LRESULT(0)
        }
        WM_TIMER => {
            let timer_id = wparam.0;
            match timer_id {
                TIMER_POLL => {
                    request_poll(SendHwnd::from_hwnd(hwnd), false);
                }
                TIMER_COUNTDOWN => {
                    let changed = update_display();
                    render_layered();
                    schedule_countdown_timer();
                    if changed {
                        sync_tray_icons(hwnd);
                    }
                }
                TIMER_TASKBAR_RETRY => {
                    let embedded = lock_state().as_ref().is_some_and(|s| s.embedded);
                    if embedded {
                        let _ = KillTimer(hwnd, TIMER_TASKBAR_RETRY);
                    } else if refresh_taskbar_selection(hwnd) {
                        let _ = KillTimer(hwnd, TIMER_TASKBAR_RETRY);
                        position_at_taskbar();
                        render_layered();
                        diagnose::log("taskbar attached after startup retry");
                    }
                }
                TIMER_FRESHNESS => {
                    let changed = update_display();
                    render_layered();
                    if changed {
                        sync_tray_icons(hwnd);
                    }
                }
                TIMER_UPDATE_CHECK => {
                    begin_update_check(hwnd, false);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_USAGE_UPDATED => {
            check_theme_change();
            check_language_change();
            render_layered();
            schedule_countdown_timer();
            suppress_tray_reposition_for(Duration::from_millis(
                TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS,
            ));
            sync_tray_icons(hwnd);
            LRESULT(0)
        }
        WM_APP_UPDATE_CHECK_COMPLETE => {
            schedule_auto_update_check(hwnd);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE).unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            if cursor_is_on_drag_handle(hwnd) {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE).unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDBLCLK => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let click_in_content = {
                let state = lock_state();
                state.as_ref().is_some_and(|s| {
                    let (label_width, _) = usage_layout_widths(s.language, s.appearance, false);
                    !s.dragging
                        && x >= sc(LEFT_DIVIDER_W + DIVIDER_RIGHT_MARGIN)
                            + period_label_column_width(label_width, s.appearance)
                })
            };
            if click_in_content {
                request_poll(SendHwnd::from_hwnd(hwnd), true);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let client_x = (lparam.0 & 0xFFFF) as i16 as i32;
            let client_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if !is_drag_handle_point(client_x, client_y) {
                return LRESULT(0);
            }

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                if s.anchor_left {
                    return LRESULT(0);
                }
                s.dragging = true;
                s.drag_start_mouse_x = pt.x;
                s.drag_start_client_x = client_x;
                s.drag_start_offset = s.tray_offset;
            }
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let move_target = {
                    let mut state = lock_state();
                    let s = match state.as_mut() {
                        Some(s) => s,
                        None => return LRESULT(0),
                    };

                    // Moving mouse left = positive delta = larger offset (further left)
                    let delta = s.drag_start_mouse_x - pt.x;
                    let mut new_offset = s.drag_start_offset + delta;

                    // Clamp: offset >= 0 (can't go right of default)
                    if new_offset < 0 {
                        new_offset = 0;
                    }

                    let taskbar_hwnd = s.taskbar_hwnd;
                    let embedded = s.embedded;
                    let hwnd_val = s.hwnd.to_hwnd();

                    // Clamp: don't go past left edge of taskbar
                    if let Some(taskbar_hwnd) = taskbar_hwnd {
                        if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
                            let mut tray_left = taskbar_rect.right;
                            if let Some(tray_hwnd) =
                                native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd")
                            {
                                if let Some(tray_rect) =
                                    native_interop::get_window_rect_safe(tray_hwnd)
                                {
                                    tray_left = tray_rect.left;
                                }
                            }
                            let widget_width = total_widget_width_for_state(s);
                            let max_offset = (tray_left - taskbar_rect.left - widget_width).max(0);
                            if new_offset > max_offset {
                                new_offset = max_offset;
                            }

                            s.tray_offset = new_offset;

                            let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
                            let anchor_top = taskbar_rect.top;
                            let anchor_height = taskbar_height;
                            let widget_height = sc(WIDGET_HEIGHT).min(taskbar_height);
                            let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
                            let x = if embedded {
                                tray_left - taskbar_rect.left - widget_width - new_offset
                            } else {
                                tray_left - widget_width - new_offset
                            };
                            Some((
                                hwnd_val,
                                embedded,
                                x,
                                y,
                                taskbar_rect.top,
                                widget_width,
                                widget_height,
                            ))
                        } else {
                            s.tray_offset = new_offset;
                            None
                        }
                    } else {
                        s.tray_offset = new_offset;
                        None
                    }
                };

                if let Some((hwnd_val, embedded, x, y, taskbar_top, widget_width, widget_height)) =
                    move_target
                {
                    if embedded {
                        native_interop::move_window(
                            hwnd_val,
                            x,
                            y - taskbar_top,
                            widget_width,
                            widget_height,
                        );
                    } else {
                        native_interop::move_window(hwnd_val, x, y, widget_width, widget_height);
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let drag_result = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if s.dragging {
                        s.dragging = false;
                        Some((s.taskbar_index, s.drag_start_client_x))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some((current_taskbar_index, drag_start_client_x)) = drag_result {
                let _ = ReleaseCapture();
                if let Some((target_index, target_taskbar)) = taskbar_at_point(pt) {
                    if target_index != current_taskbar_index {
                        let new_offset = offset_for_drop_point(
                            target_taskbar.hwnd,
                            target_taskbar.rect,
                            pt,
                            drag_start_client_x,
                        );
                        if attach_to_taskbar(hwnd, target_taskbar, target_index) {
                            let mut state = lock_state();
                            if let Some(s) = state.as_mut() {
                                s.tray_offset = new_offset;
                                s.monitor_device =
                                    native_interop::taskbar_monitor_device(&target_taskbar);
                            }
                            drop(state);
                            position_at_taskbar();
                            render_layered();
                        }
                    }
                }
                save_state_settings();
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 as u16;
            match id {
                1 => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.session_text = "...".to_string();
                            s.weekly_text = "...".to_string();
                            s.codex_session_text = "...".to_string();
                            s.codex_weekly_text = "...".to_string();
                        }
                    }
                    render_layered();
                    let sh = SendHwnd::from_hwnd(hwnd);
                    request_poll(sh, true);
                }
                IDM_SETTINGS => settings_window::show(hwnd),
                IDM_VERSION_ACTION => {
                    let (install_channel, release) = {
                        let state = lock_state();
                        match state.as_ref() {
                            Some(s) => (
                                s.install_channel,
                                match &s.update_status {
                                    UpdateStatus::Available(release) => Some(release.clone()),
                                    _ => None,
                                },
                            ),
                            None => (InstallChannel::Portable, None),
                        }
                    };

                    match install_channel {
                        InstallChannel::Winget => {
                            if release.is_some() {
                                begin_winget_update(hwnd);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                        InstallChannel::Portable => {
                            if let Some(release) = release {
                                begin_update_apply(hwnd, release);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                    }
                }
                2 => {
                    EXIT_REQUESTED.store(true, Ordering::Relaxed);
                    let hook = {
                        let state = lock_state();
                        state.as_ref().and_then(|s| s.win_event_hook)
                    };
                    if let Some(h) = hook {
                        native_interop::unhook_win_event(h);
                    }
                    PostQuitMessage(0);
                }
                IDM_RESET_POSITION => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.tray_offset = 0;
                            s.anchor_left = false;
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                }
                IDM_ANCHOR_LEFT => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.anchor_left = !s.anchor_left;
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                }
                IDM_PASSIVE_MONITORING | IDM_UPDATES_ENABLED => {
                    let mut preferences = crate::privacy::preferences();
                    let policy = crate::privacy::policy();
                    if id == IDM_PASSIVE_MONITORING && !policy.force_passive_monitoring {
                        preferences.passive_monitoring = !preferences.passive_monitoring;
                    } else if id == IDM_UPDATES_ENABLED && !policy.disable_updates {
                        preferences.updates_enabled = !preferences.updates_enabled;
                    }
                    crate::privacy::set_preferences(preferences);
                    save_state_settings();
                    schedule_auto_update_check(hwnd);
                    if id == IDM_UPDATES_ENABLED && crate::privacy::updates_enabled() {
                        begin_update_check(hwnd, false);
                    }
                }
                IDM_MCP_SERVER => {
                    if let Some(s) = lock_state().as_mut() {
                        s.mcp_enabled = !s.mcp_enabled;
                    }
                    save_state_settings();
                    apply_mcp_setting();
                }
                IDM_START_WITH_WINDOWS => {
                    let enable = startup_status() != StartupStatus::Enabled;
                    STARTUP_WRITE_FAILED.store(!set_startup_enabled(enable), Ordering::Relaxed);
                }
                IDM_WINDOWS_STARTUP_SETTINGS => {
                    let _ = ShellExecuteW(
                        hwnd,
                        w!("open"),
                        w!("ms-settings:startupapps"),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
                IDM_FREQ_1MIN | IDM_FREQ_5MIN | IDM_FREQ_15MIN | IDM_FREQ_1HOUR => {
                    let new_interval = match id {
                        IDM_FREQ_1MIN => POLL_1_MIN,
                        IDM_FREQ_5MIN => POLL_5_MIN,
                        IDM_FREQ_15MIN => POLL_15_MIN,
                        IDM_FREQ_1HOUR => POLL_1_HOUR,
                        _ => POLL_15_MIN,
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.poll_interval_ms = new_interval;
                            if s.has_poll_result {
                                s.monitor.reconfigure(
                                    s.poll_interval_ms,
                                    s.adaptive_refresh,
                                    Instant::now(),
                                    SystemTime::now(),
                                );
                                SetTimer(hwnd, TIMER_POLL, effective_poll_interval(s), None);
                            }
                        }
                    }
                    save_state_settings();
                }
                IDM_ADAPTIVE_REFRESH => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.adaptive_refresh = !s.adaptive_refresh;
                            if s.has_poll_result {
                                s.monitor.reconfigure(
                                    s.poll_interval_ms,
                                    s.adaptive_refresh,
                                    Instant::now(),
                                    SystemTime::now(),
                                );
                                SetTimer(hwnd, TIMER_POLL, effective_poll_interval(s), None);
                            }
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                IDM_SHOW_SESSION_WINDOW | IDM_SHOW_WEEKLY_WINDOW => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            match id {
                                IDM_SHOW_SESSION_WINDOW
                                    if s.show_weekly_window || !s.show_session_window =>
                                {
                                    s.show_session_window = !s.show_session_window;
                                }
                                IDM_SHOW_WEEKLY_WINDOW
                                    if s.show_session_window || !s.show_weekly_window =>
                                {
                                    s.show_weekly_window = !s.show_weekly_window;
                                }
                                _ => {}
                            }
                        }
                    }
                    save_state_settings();
                    render_layered();
                    sync_tray_icons(hwnd);
                }
                IDM_THEME_SYSTEM | IDM_THEME_LIGHT | IDM_THEME_DARK => {
                    let theme = match id {
                        IDM_THEME_LIGHT => SettingsTheme::Light,
                        IDM_THEME_DARK => SettingsTheme::Dark,
                        _ => SettingsTheme::System,
                    };
                    if let Some(s) = lock_state().as_mut() {
                        s.settings_theme = theme;
                    }
                    settings_window::apply_menu_theme(theme);
                    save_state_settings();
                }
                IDM_APPEARANCE_RECOMMENDED
                | IDM_APPEARANCE_RESET
                | IDM_PALETTE_SYSTEM
                | IDM_PALETTE_DARK
                | IDM_PALETTE_LIGHT
                | IDM_BAR_CONTINUOUS
                | IDM_BAR_SEGMENTED
                | IDM_BAR_DOTS
                | IDM_BAR_BATTERY
                | IDM_BAR_MEDIUM
                | IDM_BAR_THIN
                | IDM_BAR_THICK
                | IDM_FONT_12
                | IDM_FONT_14
                | IDM_FONT_16
                | IDM_COLOR_BRAND
                | IDM_COLOR_REMAINING
                | IDM_COLOR_SINGLE
                | IDM_SINGLE_COLOR_FIRST..=IDM_SINGLE_COLOR_LAST
                | IDM_PROVIDER_LOGOS
                | IDM_PERIOD_LABELS
                | IDM_PERCENT_OUTSIDE_BATTERY
                | IDM_PERCENT_INSIDE_BATTERY
                | IDM_RESET_COUNTDOWN
                | IDM_RESET_CLOCK => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            match id {
                                IDM_APPEARANCE_RECOMMENDED => {
                                    s.appearance = Appearance::translucent_dark_taskbar()
                                }
                                IDM_APPEARANCE_RESET => s.appearance = Appearance::default(),
                                IDM_PROVIDER_LOGOS => {
                                    s.appearance.hide_provider_logos =
                                        !s.appearance.hide_provider_logos
                                }
                                IDM_PERIOD_LABELS => {
                                    s.appearance.hide_period_labels =
                                        !s.appearance.hide_period_labels;
                                }
                                IDM_PERCENT_OUTSIDE_BATTERY | IDM_PERCENT_INSIDE_BATTERY => {
                                    s.appearance.percent_inside_battery =
                                        id == IDM_PERCENT_INSIDE_BATTERY;
                                }
                                IDM_RESET_COUNTDOWN => {
                                    s.appearance.reset_time_display = ResetTimeDisplay::Countdown
                                }
                                IDM_RESET_CLOCK => {
                                    s.appearance.reset_time_display = ResetTimeDisplay::Clock
                                }
                                IDM_PALETTE_SYSTEM => s.appearance.palette = Palette::System,
                                IDM_PALETTE_DARK => {
                                    s.appearance.palette = Palette::HighContrastDark
                                }
                                IDM_PALETTE_LIGHT => {
                                    s.appearance.palette = Palette::HighContrastLight
                                }
                                IDM_BAR_CONTINUOUS => s.appearance.bar_style = BarStyle::Continuous,
                                IDM_BAR_SEGMENTED => s.appearance.bar_style = BarStyle::Segmented,
                                IDM_BAR_DOTS => s.appearance.bar_style = BarStyle::Dots,
                                IDM_BAR_BATTERY => s.appearance.bar_style = BarStyle::Battery,
                                IDM_BAR_MEDIUM => s.appearance.bar_thickness = BarThickness::Medium,
                                IDM_BAR_THIN => s.appearance.bar_thickness = BarThickness::Thin,
                                IDM_BAR_THICK => s.appearance.bar_thickness = BarThickness::Thick,
                                IDM_FONT_12 => s.appearance.font_size = FontSize::Px12,
                                IDM_FONT_14 => s.appearance.font_size = FontSize::Px14,
                                IDM_FONT_16 => s.appearance.font_size = FontSize::Px16,
                                IDM_COLOR_BRAND => s.appearance.color_mode = ColorMode::Brand,
                                IDM_COLOR_REMAINING => {
                                    s.appearance.color_mode = ColorMode::Remaining
                                }
                                IDM_COLOR_SINGLE => s.appearance.color_mode = ColorMode::Single,
                                IDM_SINGLE_COLOR_FIRST..=IDM_SINGLE_COLOR_LAST => {
                                    s.appearance.single_color =
                                        SingleColor::ALL[(id - IDM_SINGLE_COLOR_FIRST) as usize];
                                }
                                _ => {}
                            }
                        }
                    }
                    save_state_settings();
                    let text_changed = update_display();
                    position_at_taskbar();
                    render_layered();
                    if text_changed {
                        sync_tray_icons(hwnd);
                    }
                }
                IDM_ALERT_RESET => {
                    if let Some(s) = lock_state().as_mut() {
                        s.notify_on_reset = !s.notify_on_reset;
                        s.reset_observations = Default::default();
                    }
                    save_state_settings();
                }
                IDM_ALERT_OFF | IDM_ALERT_10 | IDM_ALERT_20 | IDM_ALERT_30 => {
                    let threshold = match id {
                        IDM_ALERT_10 => 10,
                        IDM_ALERT_20 => 20,
                        IDM_ALERT_30 => 30,
                        _ => 0,
                    };
                    let alerts = {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.alert_threshold_percent = threshold;
                            if threshold == 0 {
                                s.notified_quota_windows.clear();
                                Vec::new()
                            } else if let Some(data) = s.data.clone() {
                                collect_low_quota_alerts(s, &data)
                            } else {
                                Vec::new()
                            }
                        } else {
                            Vec::new()
                        }
                    };
                    for alert in &alerts {
                        tray_icon::notify_balloon(hwnd, alert.kind, &alert.title, &alert.message);
                    }
                    save_state_settings();
                }
                IDM_MODEL_CLAUDE_CODE | IDM_MODEL_CODEX | IDM_MODEL_ANTIGRAVITY => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            match id {
                                IDM_MODEL_CLAUDE_CODE => {
                                    if s.show_codex || s.show_antigravity || !s.show_claude_code {
                                        s.show_claude_code = !s.show_claude_code;
                                    }
                                }
                                IDM_MODEL_CODEX => {
                                    if s.show_claude_code || s.show_antigravity || !s.show_codex {
                                        s.show_codex = !s.show_codex;
                                    }
                                }
                                IDM_MODEL_ANTIGRAVITY => {
                                    if s.show_claude_code || s.show_codex || !s.show_antigravity {
                                        s.show_antigravity = !s.show_antigravity;
                                    }
                                }
                                _ => {}
                            }
                            s.session_text = "...".to_string();
                            s.weekly_text = "...".to_string();
                            s.codex_session_text = "...".to_string();
                            s.codex_weekly_text = "...".to_string();
                            s.antigravity_session_text = "...".to_string();
                            s.antigravity_weekly_text = "...".to_string();
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                    sync_tray_icons(hwnd);
                    let sh = SendHwnd::from_hwnd(hwnd);
                    request_poll(sh, true);
                }
                IDM_QUOTA_USED | IDM_QUOTA_REMAINING => {
                    if let Some(s) = lock_state().as_mut() {
                        s.quota_display_mode = if id == IDM_QUOTA_USED {
                            QuotaDisplayMode::Used
                        } else {
                            QuotaDisplayMode::Remaining
                        };
                        refresh_usage_texts(s);
                    }
                    save_state_settings();
                    render_layered();
                    sync_tray_icons(hwnd);
                }
                id if (IDM_LANG_SYSTEM..=IDM_LANG_ITALIAN).contains(&id) => {
                    let language_override = LanguageId::ALL
                        .into_iter()
                        .find(|&language| language_command(language) == id);
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            apply_language_to_state(s, language_override);
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                id if id == tray_icon::IDM_TOGGLE_WIDGET => {
                    toggle_widget_visibility(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ if msg == WM_APP_TRAY => {
            match tray_icon::handle_message(lparam) {
                tray_icon::TrayAction::ToggleWidget => {
                    toggle_widget_visibility(hwnd);
                }
                tray_icon::TrayAction::ShowContextMenu => {
                    show_context_menu(hwnd);
                }
                tray_icon::TrayAction::None => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // The updater also closes the window through this message.
            EXIT_REQUESTED.store(true, Ordering::Relaxed);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DESTROY => {
            let watch = {
                let mut state = lock_state();
                state.as_mut().and_then(|s| s.recovery_watch.take())
            };
            drop(watch);
            quota_tooltip::clear();
            let hook = {
                let state = lock_state();
                state.as_ref().and_then(|s| s.win_event_hook)
            };
            if let Some(h) = hook {
                native_interop::unhook_win_event(h);
            }
            tray_icon::remove_all(hwnd);
            if EXIT_REQUESTED.load(Ordering::Relaxed) {
                PostQuitMessage(0);
            } else {
                // Explorer destroys child windows when their taskbar disappears.
                // The process must stay alive until the watchdog can relaunch it.
                diagnose::log(
                    "widget window destroyed unexpectedly; waiting for watchdog recovery",
                );
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn select_antigravity_group(hwnd: HWND, group: Option<String>) {
    let changed = lock_state().as_mut().is_some_and(|s| {
        if group == s.antigravity_quota_group {
            return false;
        }
        s.antigravity_quota_group = group;
        s.monitor.reselect(2, Instant::now());
        s.reset_observations.clear_provider(2);
        s.notified_quota_windows
            .retain(|key| !key.starts_with("antigravity:"));
        s.data = Some(s.monitor.cached());
        refresh_usage_texts(s);
        true
    });
    if changed {
        save_state_settings();
        unsafe {
            let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
        }
        start_poll(SendHwnd::from_hwnd(hwnd), true);
    }
}

fn select_settings_display(hwnd: HWND, target: HWND) {
    let taskbars = native_interop::find_taskbars();
    let Some((index, taskbar)) = taskbars
        .into_iter()
        .enumerate()
        .find(|(_, item)| item.hwnd == target)
    else {
        return;
    };
    if !attach_to_taskbar(hwnd, taskbar, index) {
        return;
    }
    if let Some(s) = lock_state().as_mut() {
        s.monitor_device = native_interop::taskbar_monitor_device(&taskbar);
        s.tray_offset = 0;
    }
    save_state_settings();
    position_at_taskbar();
    render_layered();
}

fn show_context_menu(hwnd: HWND) {
    let (strings, widget_visible) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else { return };
        (s.language.strings(), s.widget_visible)
    };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (id, label, flags) in [
            (1, strings.refresh.to_owned(), MENU_ITEM_FLAGS(0)),
            (
                IDM_SETTINGS,
                format!("{}…", strings.settings),
                MENU_ITEM_FLAGS(0),
            ),
            (
                tray_icon::IDM_TOGGLE_WIDGET,
                strings.show_widget.to_owned(),
                if widget_visible {
                    MF_CHECKED
                } else {
                    MENU_ITEM_FLAGS(0)
                },
            ),
            (2, strings.exit.to_owned(), MENU_ITEM_FLAGS(0)),
        ] {
            let text = native_interop::wide_str(&label);
            let _ = AppendMenuW(menu, flags, id as usize, PCWSTR::from_raw(text.as_ptr()));
        }
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            point.x,
            point.y,
            0,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        if command.0 != 0 {
            let _ = PostMessageW(hwnd, WM_COMMAND, WPARAM(command.0 as usize), LPARAM(0));
        }
    }
}

/// Paint for non-embedded fallback (normal WM_PAINT path)
fn paint(hdc: HDC, hwnd: HWND) {
    let content = {
        let state = lock_state();
        let Some(s) = state.as_ref() else { return };
        WidgetContent::from_state(s)
    };
    unsafe {
        let mut client_rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut client_rect);
        let width = client_rect.right - client_rect.left;
        let height = client_rect.bottom - client_rect.top;

        if width <= 0 || height <= 0 {
            return;
        }

        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = CreateCompatibleBitmap(hdc, width, height);
        let old_bmp = SelectObject(mem_dc, mem_bmp);

        paint_content(mem_dc, width, height, &content);

        let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);

        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp);
        let _ = DeleteDC(mem_dc);
    }
}

fn segment_width(segment_count: i32) -> i32 {
    sc(if segment_count == SEGMENT_COUNT {
        SEGMENT_W
    } else {
        MULTI_PROVIDER_SEGMENT_W
    })
}

fn usage_bar_width(segment_count: i32, appearance: Appearance) -> i32 {
    let width =
        segment_count * segment_width(segment_count) + (segment_count - 1) * sc(SEGMENT_GAP);
    if appearance.shows_percent_in_battery() {
        width.max(sc(quota_text::percent_width(appearance.font_px()) + 7))
    } else {
        width
    }
}

fn provider_mark_width(appearance: Appearance) -> i32 {
    if appearance.hide_provider_logos {
        0
    } else {
        sc(provider_icons::SIZE + provider_icons::RIGHT_MARGIN)
    }
}

fn model_usage_width(segment_count: i32, text_width: i32, appearance: Appearance) -> i32 {
    provider_mark_width(appearance)
        + usage_bar_width(segment_count, appearance)
        + sc(BAR_RIGHT_MARGIN)
        + text_width
}

fn usage_bar_fill_percentage(percent: f64, text: &str) -> f64 {
    if !text.contains('%') {
        0.0
    } else {
        percent.clamp(0.0, 100.0)
    }
}

fn draw_usage_bar(
    hdc: HDC,
    bar_x: i32,
    y: i32,
    row_height: i32,
    segment_count: i32,
    bar_width: i32,
    percent: f64,
    text: &str,
    accent: &Color,
    track: &Color,
    text_color: &Color,
    text_width: i32,
    is_dark: bool,
    appearance: Appearance,
) {
    let bar_height = appearance.bar_height(row_height);
    let bar_y = y + (row_height - bar_height) / 2;
    let rect = RECT {
        left: bar_x,
        top: bar_y,
        right: bar_x + bar_width,
        bottom: bar_y + bar_height,
    };
    let ratio = usage_bar_fill_percentage(percent, text) / 100.0;
    let radius = bar_height / 2;
    let percent_inside = appearance.shows_percent_in_battery();
    match appearance.bar_style {
        BarStyle::Continuous => {
            draw_rounded_rect(hdc, &rect, track, radius);
            fill_rounded_bar(hdc, &rect, ratio, accent, radius);
        }
        BarStyle::Segmented => {
            let width = segment_width(segment_count);
            for i in 0..segment_count {
                let left = bar_x + i * (width + sc(SEGMENT_GAP));
                let segment = RECT {
                    left,
                    right: left + width,
                    ..rect
                };
                // Square-ish corners keep segments distinct from the dot style.
                let radius = sc(2).min(bar_height / 2).min(width / 2);
                draw_rounded_rect(hdc, &segment, track, radius);
                let fraction = (ratio * f64::from(segment_count) - f64::from(i)).clamp(0.0, 1.0);
                fill_rounded_bar(hdc, &segment, fraction, accent, radius);
            }
        }
        BarStyle::Dots => {
            let diameter = drawn_bar_height(appearance, row_height);
            let rect = RECT {
                top: y + (row_height - diameter) / 2,
                bottom: y + (row_height - diameter) / 2 + diameter,
                ..rect
            };
            let count = bar_dot_count(bar_width, diameter, segment_count);
            let lit = (ratio * f64::from(count)).round() as i32;
            for i in 0..count {
                let left = bar_x + i * (bar_width - diameter) / (count - 1);
                let dot = RECT {
                    left,
                    right: left + diameter,
                    ..rect
                };
                draw_ellipse(hdc, &dot, if i < lit { accent } else { track });
            }
        }
        BarStyle::Battery => {
            let height = drawn_bar_height(appearance, row_height);
            let top = y + (row_height - height) / 2;
            let rect = RECT {
                top,
                bottom: top + height,
                ..rect
            };
            if percent_inside {
                let (label, reset) = quota_text::parts(text);
                draw_battery(hdc, rect, ratio, accent, track, is_dark, Some(label));
                let left = bar_x + bar_width + sc(BAR_RIGHT_MARGIN);
                quota_text::draw_reset(
                    hdc,
                    RECT {
                        left,
                        top: y,
                        right: left + text_width,
                        bottom: y + row_height,
                    },
                    reset,
                    &reset_text_color(appearance, is_dark),
                    sc(appearance.font_px()),
                );
                return;
            }
            draw_battery(hdc, rect, ratio, accent, track, is_dark, None);
        }
    }

    let text_x = bar_x + bar_width + sc(BAR_RIGHT_MARGIN);
    quota_text::draw(
        hdc,
        text_x,
        y,
        row_height,
        text_width,
        text,
        text_color,
        &reset_text_color(appearance, is_dark),
        appearance.font_px(),
        sc,
    );
}

/// Height the bar style actually paints. Thin dots read as noise, so they keep
/// at least 6 px; a battery is 4 px taller so its outline and fill both show.
fn drawn_bar_height(appearance: Appearance, row_height: i32) -> i32 {
    let bar_height = appearance.bar_height(row_height);
    let height = match appearance.bar_style {
        BarStyle::Dots => bar_height.max(sc(6)),
        BarStyle::Battery if appearance.shows_percent_in_battery() => {
            (bar_height + sc(4)).max(sc(appearance.font_px() + 4))
        }
        BarStyle::Battery => bar_height + sc(4),
        BarStyle::Continuous | BarStyle::Segmented => bar_height,
    };
    height.min((row_height - sc(2)).max(1))
}

fn bar_dot_count(width: i32, diameter: i32, segment_count: i32) -> i32 {
    let gap = sc(1);
    ((width + gap) / (diameter + gap)).clamp(2, segment_count)
}

// GDI regions exclude the bottom and right pixels from their supplied bounds.
fn draw_ellipse(hdc: HDC, rect: &RECT, color: &Color) {
    unsafe {
        let region = CreateEllipticRgn(rect.left, rect.top, rect.right + 1, rect.bottom + 1);
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        let _ = FillRgn(hdc, region, brush);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(region);
    }
}

fn draw_battery(
    hdc: HDC,
    rect: RECT,
    ratio: f64,
    accent: &Color,
    track: &Color,
    dark: bool,
    label: Option<&str>,
) {
    let edge = sc(1);
    let height = rect.bottom - rect.top;
    let terminal_height = (height / 2).max(1);
    let terminal_y = rect.top + (height - terminal_height) / 2;
    let body = RECT {
        right: rect.right - sc(3),
        ..rect
    };
    let terminal = RECT {
        left: body.right - edge,
        top: terminal_y,
        bottom: terminal_y + terminal_height,
        ..rect
    };
    let border = Color::from_hex(if dark { "#8A9099" } else { "#626C7A" });
    draw_rounded_rect(hdc, &body, &border, sc(2));
    unsafe {
        let brush = CreateSolidBrush(COLORREF(border.to_colorref()));
        FillRect(hdc, &terminal, brush);
        let _ = DeleteObject(brush);
    }
    let inside = RECT {
        left: body.left + edge,
        top: body.top + edge,
        right: body.right - edge,
        bottom: body.bottom - edge,
    };
    draw_rounded_rect(hdc, &inside, track, edge);
    // In compact mode the text sits over two continuous backgrounds.
    let vertical_gap = if label.is_some() {
        0
    } else {
        ((height - 2 * edge - 1) / 2).clamp(0, edge)
    };
    let fill = RECT {
        left: inside.left + edge,
        top: inside.top + vertical_gap,
        right: inside.right - edge,
        bottom: inside.bottom - vertical_gap,
    };
    fill_rounded_bar(hdc, &fill, ratio, accent, 0);
    if let Some(label) = label {
        let filled_right = fill.left + fill_width(&fill, ratio);
        let regions = [
            (inside.left, fill.left, track),
            (fill.left, filled_right, accent),
            (filled_right, inside.right, track),
        ];
        for (left, right, background) in regions {
            if left >= right {
                continue;
            }
            unsafe {
                let saved = SaveDC(hdc);
                if saved != 0 {
                    let _ = IntersectClipRect(hdc, left, inside.top, right, inside.bottom);
                    quota_text::draw_centered(hdc, body, label, battery_text_color(background));
                    let _ = RestoreDC(hdc, saved);
                }
            }
        }
    }
}

fn battery_text_color(background: &Color) -> COLORREF {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(background.r)
        + 0.7152 * linear(background.g)
        + 0.0722 * linear(background.b);
    COLORREF(if luminance >= 0.179 { 0 } else { 0xFF_FFFF })
}

fn fill_width(rect: &RECT, fraction: f64) -> i32 {
    ((rect.right - rect.left) as f64 * fraction).round() as i32
}

fn fill_rounded_bar(hdc: HDC, rect: &RECT, fraction: f64, color: &Color, radius: i32) {
    let width = fill_width(rect, fraction);
    if width <= 0 {
        return;
    }
    unsafe {
        let clip = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        );
        let saved = SaveDC(hdc);
        let _ = ExtSelectClipRgn(hdc, clip, RGN_AND);
        let fill = RECT {
            right: rect.left + width,
            ..*rect
        };
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        FillRect(hdc, &fill, brush);
        let _ = DeleteObject(brush);
        let _ = RestoreDC(hdc, saved);
        let _ = DeleteObject(clip);
    }
}

fn draw_rounded_rect(hdc: HDC, rect: &RECT, color: &Color, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        let rgn = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        );
        let _ = FillRgn(hdc, rgn, brush);
        let _ = DeleteObject(rgn);
        let _ = DeleteObject(brush);
    }
}

#[path = "appearance_preview.rs"]
mod appearance_preview;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taskbar_watchdog_recovers_when_either_the_window_or_its_host_disappears() {
        assert!(needs_taskbar_relaunch(false, true, true));
        assert!(needs_taskbar_relaunch(true, false, true));
        assert!(!needs_taskbar_relaunch(true, true, true));
        assert!(!needs_taskbar_relaunch(false, false, false));
    }

    #[test]
    fn old_appearance_loads_without_losing_other_settings() {
        for (font, thickness, expected_font, expected_thickness) in [
            ("standard", "slim", FontSize::Px12, BarThickness::Thin),
            ("large", "standard", FontSize::Px14, BarThickness::Medium),
        ] {
            let old = serde_json::json!({
                "tray_offset": 321, "language": "it", "show_codex": false,
                "quota_display_mode": "used", "color_presets": [{"name":"Old"}],
                "appearance": {"font_size":font, "bar_thickness":thickness,
                     "bar_style":"classic_segmented", "compact_layout":true,
                     "hide_provider_logos":true, "codex_colors":{"bar":"#112233"}}
            });
            let settings: SettingsFile = serde_json::from_value(old).unwrap();
            assert_eq!(settings.appearance.bar_style, BarStyle::Segmented);
            assert_eq!(settings.appearance.font_size, expected_font);
            assert_eq!(settings.appearance.bar_thickness, expected_thickness);
            assert_eq!(settings.appearance.color_mode, ColorMode::Brand);
            assert!(settings.appearance.hide_provider_logos);
            assert_eq!(settings.tray_offset, 321);
            assert_eq!(settings.language.as_deref(), Some("it"));
            assert!(!settings.show_codex);
            assert_eq!(settings.quota_display_mode, Some(QuotaDisplayMode::Used));
            assert_eq!(settings.settings_theme, SettingsTheme::System);
            let saved = serde_json::to_value(settings).unwrap();
            assert!(saved.get("color_presets").is_none());
            assert!(saved["appearance"].get("compact_layout").is_none());
            assert!(saved["appearance"].get("codex_colors").is_none());
        }
    }

    #[test]
    fn remaining_color_uses_inclusive_yellow_bounds_in_both_palettes() {
        for dark in [false, true] {
            let green = remaining_bar_color(75.0, dark).to_colorref();
            let yellow = remaining_bar_color(35.0, dark).to_colorref();
            let red = remaining_bar_color(5.0, dark).to_colorref();
            assert_ne!(green, yellow);
            assert_ne!(yellow, red);
            let appearance = Appearance {
                color_mode: ColorMode::Remaining,
                ..Appearance::default()
            };
            for (remaining, expected) in
                [(50.01, green), (50.0, yellow), (20.0, yellow), (19.99, red)]
            {
                assert_eq!(remaining_bar_color(remaining, dark).to_colorref(), expected);
                for provider in [Provider::Claude, Provider::Codex, Provider::Antigravity] {
                    assert_eq!(
                        appearance
                            .bar_color(provider, 100.0 - remaining, dark)
                            .to_colorref(),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn font_size_steps_down_only_when_needed() {
        for dpi in [96, 120, 144, 192] {
            let scale = |px| ((px * dpi + 48) / 96) as i32;
            for (height, rows, selected, expected) in [
                (46, 2, FontSize::Px16, FontSize::Px16),
                (42, 2, FontSize::Px16, FontSize::Px14),
                (40, 2, FontSize::Px16, FontSize::Px14),
                (38, 2, FontSize::Px16, FontSize::Px12),
                (38, 2, FontSize::Px14, FontSize::Px12),
                (24, 1, FontSize::Px16, FontSize::Px16),
                (46, 2, FontSize::Px12, FontSize::Px12),
            ] {
                assert_eq!(
                    selected.fit(scale(height), rows, scale),
                    expected,
                    "{dpi} DPI, height {height}"
                );
            }
        }
    }

    #[test]
    fn widget_stays_anchored_to_the_bottom_of_the_taskbar() {
        assert_eq!(compute_anchor_y(100, 48, 32), 116);
        assert_eq!(compute_anchor_y(100, 48, 64), 100);
    }

    #[test]
    fn unavailable_quota_has_no_bar_fill() {
        for status in ["--", "...", "!", "网络", "429"] {
            assert_eq!(usage_bar_fill_percentage(100.0, status), 0.0);
        }
        assert_eq!(usage_bar_fill_percentage(61.0, "剩余61%"), 61.0);
        assert_eq!(usage_bar_fill_percentage(100.0, "剩余100%"), 100.0);
    }

    #[test]
    fn localized_widget_text_fits_measured_columns_and_rows() {
        unsafe {
            let hdc = GetDC(HWND::default());
            assert!(!hdc.is_invalid());
            let width_of = |font: HFONT, text: &str| {
                let old_font = SelectObject(hdc, font);
                let mut wide: Vec<u16> = text.encode_utf16().collect();
                let mut rect = RECT::default();
                DrawTextW(hdc, &mut wide, &mut rect, DT_CALCRECT | DT_SINGLELINE);
                SelectObject(hdc, old_font);
                (rect.right, rect.bottom)
            };
            for language in LanguageId::ALL {
                for (palette, font_size) in [
                    (Palette::System, FontSize::Px12),
                    (Palette::System, FontSize::Px14),
                    (Palette::System, FontSize::Px16),
                    (Palette::HighContrastDark, FontSize::Px12),
                    (Palette::HighContrastDark, FontSize::Px14),
                    (Palette::HighContrastDark, FontSize::Px16),
                ] {
                    let appearance = Appearance {
                        palette,
                        font_size,
                        ..Appearance::default()
                    };
                    let (label_width, text_width) =
                        usage_layout_widths(language, appearance, false);
                    let reset_width =
                        text_width - sc(quota_text::reset_offset(appearance.font_px()));
                    let label_font = create_widget_font(appearance);
                    let reset_font = quota_text::create_reset_font(sc(appearance.font_px()));
                    assert!(!label_font.is_invalid() && !reset_font.is_invalid());
                    let (labels, resets) =
                        usage_layout_samples(language, appearance.reset_time_display, false);
                    for label in labels {
                        let (width, height) = width_of(label_font, label);
                        assert!(width <= label_width, "{language:?}: {label:?}");
                        assert!(height <= appearance.row_height(), "{language:?}: {label:?}");
                    }
                    for reset in &resets {
                        let (width, height) = width_of(reset_font, reset);
                        assert!(width <= reset_width, "{language:?}: {reset:?}");
                        assert!(height <= appearance.row_height(), "{language:?}: {reset:?}");
                    }
                    let _ = DeleteObject(label_font);
                    let _ = DeleteObject(reset_font);
                }
            }
            ReleaseDC(HWND::default(), hdc);
        }
    }

    #[test]
    fn service_tooltip_combines_visible_quota_rows() {
        assert_eq!(
            service_tooltip(
                "Codex",
                "剩余13% 19:04重置",
                "剩余86% 07/18重置",
                true,
                true
            ),
            "Codex: 5h 剩余13% 19:04重置 | 7d 剩余86% 07/18重置"
        );
        assert_eq!(
            service_tooltip("Claude Code", "13%", "86%", false, true),
            "Claude Code: 7d 86%"
        );
    }

    fn test_settings_json(language: &str) -> String {
        format!(
            r#"{{
  "tray_offset": 321,
  "taskbar_index": 1,
  "poll_interval_ms": 60000,
  "language": "{language}",
  "widget_visible": true,
  "show_claude_code": false,
  "show_codex": true,
  "show_antigravity": false
}}"#
        )
    }

    #[test]
    fn appearance_settings_preserve_legacy_defaults_and_round_trip() {
        let old: SettingsFile = serde_json::from_str(&test_settings_json("zh-CN")).unwrap();
        assert_eq!(old.appearance, Appearance::default());
        assert_eq!(old.monitor_device, None);

        let customized = SettingsFile {
            appearance: Appearance::translucent_dark_taskbar(),
            ..old
        };
        assert_eq!(customized.appearance.bar_style, BarStyle::Continuous);
        let json = serde_json::to_string(&customized).unwrap();
        let restored: SettingsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.appearance, Appearance::translucent_dark_taskbar());
    }

    #[test]
    fn chosen_monitor_device_survives_settings_round_trip() {
        let settings = SettingsFile {
            monitor_device: Some(r"\\.\DISPLAY2".to_string()),
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let restored: SettingsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.monitor_device, settings.monitor_device);
    }

    #[test]
    fn loads_legacy_settings_when_new_path_is_missing() {
        let base = std::env::temp_dir().join(format!(
            "codex-usage-settings-test-{}-{}",
            std::process::id(),
            now_unix_secs()
        ));
        let current = base.join("CodexUsage").join("settings.json");
        let legacy = base.join("ClaudeCodeUsageMonitor").join("settings.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, test_settings_json("zh-CN")).unwrap();

        let (settings, migrated) = load_settings_from_paths(&current, &legacy).unwrap();

        assert!(migrated);
        assert_eq!(settings.tray_offset, 321);
        assert_eq!(settings.poll_interval_ms, 60_000);
        assert_eq!(settings.language.as_deref(), Some("zh-CN"));
        assert!(settings.show_codex);
        assert!(!settings.show_claude_code);
        assert!(settings.show_session_window);
        assert!(settings.show_weekly_window);
        assert_eq!(settings.alert_threshold_percent, 0);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn new_settings_take_precedence_over_legacy_settings() {
        let base = std::env::temp_dir().join(format!(
            "codex-usage-settings-precedence-test-{}-{}",
            std::process::id(),
            now_unix_secs()
        ));
        let current = base.join("CodexUsage").join("settings.json");
        let legacy = base.join("ClaudeCodeUsageMonitor").join("settings.json");
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&current, test_settings_json("en")).unwrap();
        std::fs::write(&legacy, test_settings_json("zh-CN")).unwrap();

        let (settings, migrated) = load_settings_from_paths(&current, &legacy).unwrap();

        assert!(!migrated);
        assert_eq!(settings.language.as_deref(), Some("en"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn startup_migration_only_writes_when_legacy_exists_without_current_entry() {
        assert!(should_write_migrated_startup(true, false));
        assert!(!should_write_migrated_startup(false, false));
        assert!(!should_write_migrated_startup(true, true));
        assert!(!should_write_migrated_startup(false, true));
    }

    #[test]
    fn windows_startup_approval_detects_disabled_entries() {
        assert!(startup_approval_is_disabled(&[3, 0, 0, 0, 1, 2, 3, 4]));
        assert!(startup_approval_is_disabled(&[7, 0, 0, 0]));
        assert!(!startup_approval_is_disabled(&[2, 0, 0, 0]));
        assert!(!startup_approval_is_disabled(&[6, 0, 0, 0]));
        assert!(!startup_approval_is_disabled(&[3]));
    }

    #[test]
    fn displays_distinct_transient_error_categories() {
        assert_eq!(
            poll_error_display_label(
                poller::PollError::NetworkUnavailable,
                LanguageId::SimplifiedChinese,
            ),
            "网络"
        );
        assert_eq!(
            poll_error_display_label(
                poller::PollError::RateLimited(None),
                LanguageId::SimplifiedChinese,
            ),
            "限流"
        );
        assert_eq!(
            poll_error_display_label(poller::PollError::ServerError, LanguageId::English),
            "5XX"
        );
        assert_eq!(
            poll_error_display_label(poller::PollError::RequestFailed, LanguageId::English),
            "ERR"
        );
    }

    #[test]
    fn left_anchor_is_stable_across_tray_width_widget_size_and_monitor_changes() {
        for left in [0, -2560, 2560] {
            for tray_width in [1600, 1800, 2000] {
                for widget_width in [480, 606, 970] {
                    assert_eq!(
                        compute_anchor_x(left, left + tray_width, widget_width, 321, true),
                        left
                    );
                }
            }
        }
        assert_eq!(compute_anchor_x(0, 300, 606, 321, true), 0);
        assert_eq!(compute_anchor_x(0, 2000, 606, 321, false), 1073);
    }

    #[test]
    fn left_anchor_setting_round_trips_without_changing_manual_position() {
        let mut settings = SettingsFile {
            tray_offset: 321,
            anchor_left: true,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        settings = serde_json::from_str(&json).unwrap();
        assert!(settings.anchor_left);
        assert_eq!(settings.tray_offset, 321);
        let legacy: SettingsFile = serde_json::from_str("{}").unwrap();
        assert!(!legacy.anchor_left);
    }

    #[test]
    fn startup_recognizes_quoted_commands_and_legacy_unquoted_paths() {
        let path = r"C:\Program Files\Codex Usage\codex-usage.exe";
        assert!(startup_command_matches(&format!("\"{path}\""), path));
        assert!(startup_command_matches(path, path));
        assert!(!startup_command_matches(r"C:\Other\codex-usage.exe", path));
    }

    #[test]
    fn invalid_poll_intervals_cannot_create_a_busy_timer() {
        for interval in [0, 1, u32::MAX] {
            let settings = normalize_settings(SettingsFile {
                poll_interval_ms: interval,
                ..SettingsFile::default()
            });
            assert_eq!(settings.poll_interval_ms, POLL_15_MIN);
        }
    }

    #[test]
    fn normalizes_usage_display_and_alert_settings() {
        let settings = normalize_settings(SettingsFile {
            show_session_window: false,
            show_weekly_window: false,
            alert_threshold_percent: 17,
            notified_quota_windows: vec!["codex:weekly:1".into(), "codex:weekly:1".into()],
            ..SettingsFile::default()
        });

        assert!(settings.show_session_window);
        assert!(!settings.show_weekly_window);
        assert_eq!(settings.alert_threshold_percent, 0);
        assert_eq!(settings.notified_quota_windows.len(), 1);
    }

    #[test]
    fn formats_precise_local_reset_time() {
        let local = SYSTEMTIME {
            wYear: 2026,
            wMonth: 7,
            wDay: 17,
            wHour: 18,
            wMinute: 30,
            ..Default::default()
        };
        assert_eq!(format_local_system_time(local), "2026-07-17 18:30");
        assert_eq!(format_precise_reset_time(None), None);
    }

    #[test]
    fn formats_short_reset_times_without_a_suffix() {
        let today = SYSTEMTIME {
            wYear: 2026,
            wMonth: 11,
            wDay: 22,
            wHour: 20,
            wMinute: 4,
            ..Default::default()
        };
        let week = SYSTEMTIME { wDay: 28, ..today };
        let tomorrow = SYSTEMTIME {
            wDay: 23,
            wHour: 0,
            wMinute: 30,
            ..today
        };
        assert_eq!(short_reset_time(today, today, false), "20:04");
        assert_eq!(short_reset_time(week, today, true), "11/28");
        assert_eq!(short_reset_time(tomorrow, today, false), "11/23 00:30");
    }

    #[test]
    fn compact_battery_and_hidden_period_labels_reclaim_widget_width() {
        let mut appearance = Appearance {
            bar_style: BarStyle::Battery,
            ..serde_json::from_str("{}").unwrap()
        };
        assert_eq!(appearance.reset_time_display, ResetTimeDisplay::Countdown);
        assert!(!appearance.hide_period_labels);
        assert!(!appearance.percent_inside_battery);
        let language = LanguageId::SimplifiedChinese;
        let default_width = total_widget_width_for(3, language, appearance);
        appearance.hide_period_labels = true;
        let without_labels = total_widget_width_for(3, language, appearance);
        assert!(without_labels < default_width);
        appearance.percent_inside_battery = true;
        let compact = total_widget_width_for(3, language, appearance);
        assert!(compact < without_labels);
        appearance.reset_time_display = ResetTimeDisplay::Clock;
        assert!(total_widget_width_for(3, language, appearance) > compact);
        let short_clock = usage_layout_widths(language, appearance, false).1;
        let extended_clock = usage_layout_widths(language, appearance, true).1;
        assert!(extended_clock > short_clock);
        let values = ["74%  20:04", "89%  11/23 00:30", "100%  11/28"];
        assert!(has_cross_day_reset(appearance, true, [true; 3], values));
        assert!(!has_cross_day_reset(
            appearance,
            true,
            [true, false, true],
            values
        ));
        assert!(!has_cross_day_reset(appearance, false, [true; 3], values));
        let json = serde_json::to_string(&appearance).unwrap();
        assert_eq!(
            serde_json::from_str::<Appearance>(&json).unwrap(),
            appearance
        );
    }

    #[test]
    fn quota_hover_shows_both_reset_times_to_the_minute() {
        use crate::models::{UsageData, UsageSection};

        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let session = now + Duration::from_secs(17_999);
        let weekly = now + Duration::from_secs(604_799);
        let mut usage = UsageData {
            session: UsageSection::reported(20.0, Some(session)),
            weekly: UsageSection::reported(40.0, Some(weekly)),
            ..UsageData::default()
        };
        let session_time = format_precise_reset_time(Some(session)).unwrap();
        let weekly_time = format_precise_reset_time(Some(weekly)).unwrap();
        assert_eq!(
            window_notes(&usage, Some(now), now, LanguageId::SimplifiedChinese),
            [
                format!("5 小时额度重置 · {session_time}"),
                format!("周额度重置 · {weekly_time}"),
            ]
        );
        assert_eq!(
            window_notes(&usage, Some(now), now, LanguageId::English),
            [
                format!("5-hour reset · {session_time}"),
                format!("Weekly reset · {weekly_time}"),
            ]
        );
        usage.session.resets_at = None;
        assert_eq!(
            window_notes(&usage, Some(now), session, LanguageId::SimplifiedChinese),
            [format!("周额度重置 · {weekly_time}")]
        );
        assert!(
            window_notes(&usage, Some(now), weekly, LanguageId::SimplifiedChinese)[0]
                .contains("已于")
        );
    }

    #[test]
    fn rollover_shows_stale_readings_then_hides_them_until_a_new_reading_arrives() {
        let reset = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let observed = Some(reset - Duration::from_secs(60));
        let reading = crate::models::UsageSection::reported(42.0, Some(reset));
        let chinese = LanguageId::SimplifiedChinese.strings();
        let english = LanguageId::English.strings();
        let at_reset = window_text(&reading, observed, reset, |section| {
            poller::format_quota_line(section, chinese, QuotaDisplayMode::Remaining, reset)
        });
        assert_eq!(at_reset, "58% *");
        assert_eq!(
            window_reset(Some(reset), observed, reset + Duration::from_secs(299)),
            WindowReset::Rollover
        );
        let expired = reset + Duration::from_secs(300);
        let after_grace = window_text(&reading, observed, expired, |section| {
            poller::format_quota_line(section, chinese, QuotaDisplayMode::Remaining, expired)
        });
        assert_eq!(after_grace, "--");
        assert_eq!(usage_bar_fill_percentage(58.0, &after_grace), 0.0);
        let new_reading = window_text(&reading, Some(expired), expired, |section| {
            poller::format_quota_line(section, english, QuotaDisplayMode::Remaining, expired)
        });
        assert!(new_reading.starts_with("58%"));
        assert!(!new_reading.ends_with('*'));
    }

    #[test]
    fn alerts_compare_unrounded_capacity_and_skip_unreported_windows() {
        let mut alerts = Vec::new();
        let mut notified = BTreeSet::new();
        for section in [
            crate::models::UsageSection::default(),
            crate::models::UsageSection::reported(89.6, None),
            crate::models::UsageSection::reported(90.4, None),
        ] {
            append_quota_alert(
                &mut alerts,
                &mut notified,
                10,
                LanguageId::English,
                tray_icon::TrayIconKind::Codex,
                "codex",
                "Codex",
                "session",
                "5h",
                &section,
            );
        }
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].message.contains("10% remaining"));
    }

    #[test]
    fn low_quota_alert_is_deduplicated_until_reset_window_changes() {
        let mut alerts = Vec::new();
        let mut notified = BTreeSet::new();
        let first_reset = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let first = crate::models::UsageSection::reported(85.0, Some(first_reset));

        append_quota_alert(
            &mut alerts,
            &mut notified,
            20,
            LanguageId::SimplifiedChinese,
            tray_icon::TrayIconKind::Codex,
            "codex",
            "Codex",
            "session",
            "5小时",
            &first,
        );
        append_quota_alert(
            &mut alerts,
            &mut notified,
            20,
            LanguageId::SimplifiedChinese,
            tray_icon::TrayIconKind::Codex,
            "codex",
            "Codex",
            "session",
            "5小时",
            &first,
        );
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].message.contains("仅剩 15%"));

        let next = crate::models::UsageSection::reported(
            90.0,
            Some(first_reset + Duration::from_secs(18_000)),
        );
        append_quota_alert(
            &mut alerts,
            &mut notified,
            20,
            LanguageId::SimplifiedChinese,
            tray_icon::TrayIconKind::Codex,
            "codex",
            "Codex",
            "session",
            "5小时",
            &next,
        );
        assert_eq!(alerts.len(), 2);
        assert_eq!(notified.len(), 1);
    }
    #[test]
    fn mcp_server_is_opt_in_and_keeps_older_settings_compatible() {
        let older: SettingsFile = serde_json::from_str(r#"{"show_codex":true}"#).unwrap();
        assert!(!older.mcp_server_enabled);
        assert_eq!(mcp_port(older.mcp_port), mcp_server::DEFAULT_PORT);
        let saved = serde_json::to_value(&older).unwrap();
        assert_eq!(saved["mcp_server_enabled"], false);
        assert!(saved.get("mcp_port").is_none());

        let custom: SettingsFile =
            serde_json::from_str(r#"{"mcp_server_enabled":true,"mcp_port":47000}"#).unwrap();
        assert!(custom.mcp_server_enabled);
        assert_eq!(mcp_port(custom.mcp_port), 47000);
        // Privileged or zero ports fall back to the default.
        assert_eq!(mcp_port(Some(80)), mcp_server::DEFAULT_PORT);
        assert_eq!(mcp_port(Some(0)), mcp_server::DEFAULT_PORT);
        assert!(mcp_settings_hint(LanguageId::English, true, 47000).contains("47000"));
    }
    #[test]
    fn hiding_provider_logos_reclaims_their_columns() {
        let original = Appearance::default();
        let hidden = Appearance {
            hide_provider_logos: true,
            ..original
        };
        let regular = total_widget_width_for(2, LanguageId::SimplifiedChinese, original);
        let without_logos = total_widget_width_for(2, LanguageId::SimplifiedChinese, hidden);
        assert_eq!(regular - without_logos, 2 * provider_mark_width(original));
        assert_eq!(provider_mark_width(hidden), 0);
    }

    #[test]
    fn italian_is_selectable_and_settings_round_trip() {
        let italian = LanguageId::from_code("it-IT").unwrap();
        assert_eq!(italian, LanguageId::Italian);
        assert!(LanguageId::ALL.contains(&italian));
        assert_eq!(italian.native_name(), "Italiano");
        let saved: SettingsFile =
            serde_json::from_str(r#"{"language":"it","quota_display_mode":"remaining"}"#).unwrap();
        assert_eq!(saved.quota_display_mode, Some(QuotaDisplayMode::Remaining));
        assert_eq!(serde_json::to_value(saved).unwrap()["language"], "it");
    }
}
