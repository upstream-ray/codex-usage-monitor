use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
use crate::models::AppUsageData;
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
    claude_code_available: bool,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_session_window: bool,
    show_weekly_window: bool,
    alert_threshold_percent: u8,
    notified_quota_windows: BTreeSet<String>,

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
const IDM_BAR_STANDARD: u16 = 95;
const IDM_BAR_SLIM: u16 = 96;
const IDM_FONT_STANDARD: u16 = 97;
const IDM_FONT_LARGE: u16 = 98;
const IDM_APPEARANCE_RECOMMENDED: u16 = 99;
const IDM_APPEARANCE_RESET: u16 = 100;
const IDM_MONITOR_FIRST: u16 = 200;
const MAX_MONITOR_MENU_ITEMS: usize = 100;

const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_APP_UPDATE_CHECK_COMPLETE: u32 = WM_APP + 2;
const TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS: u64 = 750;

/// How often the watchdog thread polls for an explorer.exe restart (which
/// recreates the taskbar and wipes our tray-icon registration).
const TASKBAR_WATCH_INTERVAL_SECS: u64 = 2;

static SUPPRESS_TRAY_REPOSITION_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Current system DPI (96 = 100% scaling, 144 = 150%, 192 = 200%, etc.)
static CURRENT_DPI: AtomicU32 = AtomicU32::new(96);

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

/// Detect explorer.exe restarts and recover from them.
///
/// Once explorer destroys the taskbar, our embedded child window is destroyed
/// and the UI message loop is dead, so recovery cannot happen in-process. This
/// dedicated thread (independent of the dead message loop) polls the taskbar
/// handle and, when it changes, relaunches the widget as a fresh process.
fn spawn_taskbar_watchdog() {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TASKBAR_WATCH_INTERVAL_SECS));
        let stored = {
            let state = lock_state();
            state.as_ref().and_then(|s| s.taskbar_hwnd)
        };
        // Only relevant once we have embedded into a taskbar at least once.
        let Some(old) = stored else {
            continue;
        };
        let taskbars = native_interop::find_taskbars();
        if !taskbars.is_empty() && !taskbars.iter().any(|taskbar| taskbar.hwnd == old) {
            let new = taskbars[0].hwnd;
            diagnose::log(format!(
                "watchdog: taskbar changed old={:?} new={:?} -> relaunching",
                old.0, new.0
            ));
            relaunch_self();
        }
    });
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
    Segmented,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BarThickness {
    #[default]
    Standard,
    Slim,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FontSize {
    #[default]
    Standard,
    Large,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Appearance {
    palette: Palette,
    bar_style: BarStyle,
    bar_thickness: BarThickness,
    font_size: FontSize,
    #[serde(flatten)]
    decoration: crate::appearance::Appearance,
}

impl Appearance {
    fn translucent_dark_taskbar() -> Self {
        Self {
            palette: Palette::HighContrastDark,
            bar_style: BarStyle::Continuous,
            bar_thickness: BarThickness::Slim,
            font_size: FontSize::Large,
            ..Default::default()
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
        match self.font_size {
            FontSize::Standard => 12,
            FontSize::Large => 13,
        }
    }

    /// Physical row height; large text needs one more pixel of line height.
    fn row_height(self) -> i32 {
        sc(match self.font_size {
            FontSize::Standard => ROW_HEIGHT,
            FontSize::Large => ROW_HEIGHT + 1,
        })
    }

    fn bar_height(self) -> i32 {
        sc(match self.bar_thickness {
            BarThickness::Standard => SEGMENT_H,
            BarThickness::Slim => 8,
        })
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
    #[serde(default = "default_show_usage_window")]
    show_session_window: bool,
    #[serde(default = "default_show_usage_window")]
    show_weekly_window: bool,
    #[serde(default)]
    alert_threshold_percent: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notified_quota_windows: Vec<String>,
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
            last_update_check_unix: None,
            widget_visible: true,
            show_claude_code: false,
            show_codex: true,
            show_antigravity: false,
            show_session_window: true,
            show_weekly_window: true,
            alert_threshold_percent: 0,
            notified_quota_windows: Vec::new(),
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

fn load_settings(claude_code_available: bool) -> SettingsFile {
    let current_path = settings_path();
    let legacy_path = legacy_settings_path();
    let (settings, migrated) = load_settings_from_paths(&current_path, &legacy_path)
        .unwrap_or_else(|| (SettingsFile::default(), false));
    let settings = normalize_settings(settings);
    let (settings, claude_auto_disabled) =
        apply_claude_code_availability(settings, claude_code_available);
    if migrated || claude_auto_disabled {
        save_settings(&settings);
        if migrated {
            diagnose::log(format!(
                "migrated settings from {} to {}",
                legacy_path.display(),
                current_path.display()
            ));
        }
        if claude_auto_disabled {
            diagnose::log(
                "disabled Claude Code monitoring because no CLI credentials are available",
            );
        }
    }
    settings
}

fn apply_claude_code_availability(
    mut settings: SettingsFile,
    claude_code_available: bool,
) -> (SettingsFile, bool) {
    let disabled = settings.show_claude_code && !claude_code_available;
    if disabled {
        settings.show_claude_code = false;
        settings = normalize_settings(settings);
    }
    (settings, disabled)
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
            last_update_check_unix: s.last_update_check_unix,
            widget_visible: s.widget_visible,
            show_claude_code: s.show_claude_code,
            show_codex: s.show_codex,
            show_antigravity: s.show_antigravity,
            show_session_window: s.show_session_window,
            show_weekly_window: s.show_weekly_window,
            alert_threshold_percent: s.alert_threshold_percent,
            notified_quota_windows: s.notified_quota_windows.iter().cloned().collect(),
        })
    });
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

fn claude_code_menu_label(
    strings: Strings,
    language: LanguageId,
    claude_code_available: bool,
) -> String {
    if claude_code_available {
        strings.claude_code_model.to_string()
    } else if language == LanguageId::SimplifiedChinese {
        "Claude Code（需登录 CLI）".to_string()
    } else {
        "Claude Code (CLI login required)".to_string()
    }
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
    let prefix = format!("{provider_key}:{window_key}:");
    let reset = section
        .resets_at
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs());
    let remaining = poller::remaining_percentage(section.percentage).round() as u8;
    if !crate::quota_alerts::should_notify(notified, &prefix, reset, remaining, threshold) {
        return;
    }

    let reset = format_precise_reset_time(section.resets_at);
    let (title, message) = if language == LanguageId::SimplifiedChinese {
        (
            format!("{provider_label} 额度提醒"),
            format!(
                "{window_label}额度仅剩 {remaining}%，重置时间：{}",
                reset.unwrap_or_else(|| "未知".to_string())
            ),
        )
    } else {
        (
            format!("{provider_label} quota alert"),
            format!(
                "{window_label} quota has {remaining}% remaining. Reset: {}",
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
                    strings.antigravity_model,
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

fn attach_to_taskbar(hwnd: HWND, requested_index: usize) -> bool {
    let taskbars = native_interop::find_taskbars();
    if taskbars.is_empty() {
        diagnose::log("taskbar not found; using fallback popup window");
        return false;
    }

    let index = requested_index.min(taskbars.len().saturating_sub(1));
    let taskbar = taskbars[index];
    diagnose::log(format!(
        "taskbar selected index={index} count={} hwnd={:?} rect=({}, {}, {}, {})",
        taskbars.len(),
        taskbar.hwnd,
        taskbar.rect.left,
        taskbar.rect.top,
        taskbar.rect.right,
        taskbar.rect.bottom
    ));

    let old_hook = {
        let mut state = lock_state();
        state.as_mut().and_then(|s| s.win_event_hook.take())
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook);
    }

    native_interop::embed_in_taskbar(hwnd, taskbar.hwnd);

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

fn refresh_taskbar_selection(hwnd: HWND) {
    let taskbars = native_interop::find_taskbars();
    if taskbars.is_empty() {
        return;
    }
    let (preferred, legacy_index, current) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else { return };
        (s.monitor_device.clone(), s.taskbar_index, s.taskbar_hwnd)
    };
    let index = preferred_taskbar_index(&taskbars, preferred.as_deref(), legacy_index);
    if current != Some(taskbars[index].hwnd) {
        attach_to_taskbar(hwnd, index);
    }
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

fn refresh_usage_texts(state: &mut AppState) {
    let strings = state.language.strings();
    let show_remaining = state.language == LanguageId::SimplifiedChinese;
    let Some(data) = state.data.as_ref() else {
        return;
    };

    if let Some(claude_code) = data.claude_code.as_ref() {
        state.session_text = poller::format_line(
            &claude_code.session,
            strings,
            show_remaining,
            poller::UsageWindowKind::Session,
        );
        state.weekly_text = poller::format_line(
            &claude_code.weekly,
            strings,
            show_remaining,
            poller::UsageWindowKind::Weekly,
        );
    } else if state.show_claude_code {
        let label = state.monitor.services[0]
            .error
            .map(|e| poll_error_display_label(e, state.language))
            .unwrap_or("...");
        state.session_text = label.to_string();
        state.weekly_text = label.to_string();
    }

    if let Some(codex) = data.codex.as_ref() {
        state.codex_session_text = poller::format_codex_line(
            &codex.session,
            strings,
            show_remaining,
            poller::UsageWindowKind::Session,
        );
        state.codex_weekly_text = poller::format_codex_line(
            &codex.weekly,
            strings,
            show_remaining,
            poller::UsageWindowKind::Weekly,
        );
    } else if state.show_codex {
        let label = state.monitor.services[1]
            .error
            .map(|e| poll_error_display_label(e, state.language))
            .unwrap_or("...");
        state.codex_session_text = label.to_string();
        state.codex_weekly_text = label.to_string();
    }

    if let Some(antigravity) = data.antigravity.as_ref() {
        state.antigravity_session_text = poller::format_line(
            &antigravity.session,
            strings,
            show_remaining,
            poller::UsageWindowKind::Session,
        );
        state.antigravity_weekly_text =
            if antigravity.weekly.resets_at.is_none() && antigravity.weekly.percentage == 0.0 {
                "--".to_string()
            } else {
                poller::format_line(
                    &antigravity.weekly,
                    strings,
                    show_remaining,
                    poller::UsageWindowKind::Weekly,
                )
            };
    } else if state.show_antigravity {
        let label = state.monitor.services[2]
            .error
            .map(|e| poll_error_display_label(e, state.language))
            .unwrap_or("...");
        state.antigravity_session_text = label.to_string();
        state.antigravity_weekly_text = label.to_string();
    }
    for (id, session, weekly) in [
        (0, &mut state.session_text, &mut state.weekly_text),
        (
            1,
            &mut state.codex_session_text,
            &mut state.codex_weekly_text,
        ),
        (
            2,
            &mut state.antigravity_session_text,
            &mut state.antigravity_weekly_text,
        ),
    ] {
        let stale = state.monitor.services[id].stale(SystemTime::now());
        quota_refresh::mark_stale(session, stale);
        quota_refresh::mark_stale(weekly, stale);
    }
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

fn version_action_label(
    strings: Strings,
    language: LanguageId,
    install_channel: InstallChannel,
    status: &UpdateStatus,
) -> String {
    let current = env!("CARGO_PKG_VERSION");
    match status {
        UpdateStatus::Idle => format!("v{current} - {}", strings.check_for_updates),
        UpdateStatus::Checking => format!("v{current} - {}", strings.checking_for_updates),
        UpdateStatus::Applying => format!("v{current} - {}", strings.applying_update),
        UpdateStatus::UpToDate => format!("v{current} - {}", strings.up_to_date_short),
        UpdateStatus::Available(release) => match install_channel {
            InstallChannel::Portable => {
                format!(
                    "v{current} - {} v{}",
                    strings.update_to, release.latest_version
                )
            }
            InstallChannel::Winget => format!(
                "v{current} - {} v{}",
                localization::update_via_winget(language),
                release.latest_version
            ),
        },
    }
}

fn begin_update_check(hwnd: HWND, interactive: bool) {
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

/// Returns true only if the startup registry value points to this executable.
fn is_startup_enabled() -> bool {
    let Some(reg_value) = read_startup_value(STARTUP_REGISTRY_KEY) else {
        return false;
    };
    let Some(current_exe) = current_exe_path_string() else {
        return false;
    };
    startup_command_matches(&reg_value, &current_exe)
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
        set_startup_enabled(true);
    }

    if read_startup_value(STARTUP_REGISTRY_KEY).is_some() {
        delete_startup_value(LEGACY_STARTUP_REGISTRY_KEY);
        diagnose::log("migrated legacy startup registry entry to CodexUsage");
    }
}

fn should_write_migrated_startup(legacy_exists: bool, current_exists: bool) -> bool {
    legacy_exists && !current_exists
}

fn set_startup_enabled(enable: bool) {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if result.is_err() {
            return;
        }

        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        if enable {
            let mut exe_buf = [0u16; 260];
            let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
            if len > 0 {
                // Quoting also supports installation paths containing spaces.
                let command = native_interop::wide_str(&format!(
                    "\"{}\"",
                    String::from_utf16_lossy(&exe_buf[..len])
                ));
                let byte_len = (command.len() * 2) as u32;
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR::from_raw(key_name.as_ptr()),
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        command.as_ptr() as *const u8,
                        byte_len as usize,
                    )),
                );
            }
        } else {
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
            let legacy_key_name = native_interop::wide_str(LEGACY_STARTUP_REGISTRY_KEY);
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(legacy_key_name.as_ptr()));
        }

        let _ = RegCloseKey(hkey);
    }
}

// Dimensions matching the C# version
const SEGMENT_W: i32 = 10;
const SEGMENT_H: i32 = 13;
const ROW_HEIGHT: i32 = 18;
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
const MODEL_RIGHT_MARGIN: i32 = 3;
const RIGHT_MARGIN: i32 = 1;
const WIDGET_HEIGHT: i32 = 46;

fn is_drag_handle_point(client_x: i32, client_y: i32) -> bool {
    let divider_h = sc(25);
    let divider_top = (sc(WIDGET_HEIGHT) - divider_h) / 2;
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
fn usage_layout_samples(language: LanguageId) -> (Vec<&'static str>, Vec<String>) {
    let strings = language.strings();
    let labels = vec![strings.session_window, strings.weekly_window];
    let resets = if language == LanguageId::SimplifiedChinese {
        vec!["23:59重置 *".to_string(), "12/31重置 *".to_string()]
    } else {
        [
            strings.day_suffix,
            strings.hour_suffix,
            strings.minute_suffix,
            strings.second_suffix,
        ]
        .into_iter()
        .map(|suffix| format!("99{suffix} *"))
        .chain(std::iter::once(format!("{} *", strings.now)))
        .collect()
    };
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
fn usage_layout_widths(language: LanguageId, appearance: Appearance) -> (i32, i32) {
    let reset_x = sc(quota_text::reset_offset(appearance.font_px()));
    let fallback = if language == LanguageId::SimplifiedChinese {
        (
            sc(SIMPLIFIED_CHINESE_LABEL_WIDTH),
            sc(SIMPLIFIED_CHINESE_TEXT_WIDTH),
        )
    } else {
        (sc(LABEL_WIDTH), sc(TEXT_WIDTH))
    };
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
        let (labels, resets) = usage_layout_samples(language);
        let label_font = create_widget_font(appearance);
        let reset_font = quota_text::create_reset_font(sc(appearance.font_px()));
        let widths = if label_font.is_invalid() || reset_font.is_invalid() {
            let _ = DeleteObject(label_font);
            let _ = DeleteObject(reset_font);
            fallback
        } else {
            (
                measure(label_font, &mut labels.into_iter()) + sc(2),
                (reset_x + measure(reset_font, &mut resets.iter().map(String::as_str)) + sc(3))
                    .max(fallback.1),
            )
        };
        ReleaseDC(HWND::default(), hdc);
        widths
    }
}

fn usage_percent_for_display(language: LanguageId, used_percentage: f64) -> f64 {
    if language == LanguageId::SimplifiedChinese {
        poller::remaining_percentage(used_percentage)
    } else {
        used_percentage.clamp(0.0, 100.0)
    }
}

fn total_widget_width_for(active_models: i32, language: LanguageId, appearance: Appearance) -> i32 {
    let bar_segments = row_bar_segment_count(active_models);
    let (label_width, text_width) = usage_layout_widths(language, appearance);
    let model_width = model_usage_width(bar_segments, text_width, appearance);

    sc(LEFT_DIVIDER_W)
        + sc(DIVIDER_RIGHT_MARGIN)
        + label_width
        + sc(LABEL_RIGHT_MARGIN)
        + model_width * active_models
        + sc(MODEL_RIGHT_MARGIN) * (active_models - 1)
        + sc(RIGHT_MARGIN)
}

fn total_widget_width_for_state(state: &AppState) -> i32 {
    total_widget_width_for(
        active_model_count(
            state.show_claude_code,
            state.show_codex,
            state.show_antigravity,
        ),
        state.language,
        state.appearance,
    )
}

fn total_widget_width() -> i32 {
    let (active_models, language, appearance) = {
        let state = lock_state();
        state
            .as_ref()
            .map(|s| {
                (
                    active_model_count(s.show_claude_code, s.show_codex, s.show_antigravity),
                    s.language,
                    s.appearance,
                )
            })
            .unwrap_or((1, LanguageId::English, Appearance::default()))
    };
    total_widget_width_for(active_models, language, appearance)
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
            style: CS_HREDRAW | CS_VREDRAW,
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

        let claude_code_available = poller::claude_code_credentials_available();
        let settings = load_settings(claude_code_available);
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
                claude_code_available,
                show_claude_code: settings.show_claude_code,
                show_codex: settings.show_codex,
                show_antigravity: settings.show_antigravity,
                show_session_window: settings.show_session_window,
                show_weekly_window: settings.show_weekly_window,
                alert_threshold_percent: settings.alert_threshold_percent,
                notified_quota_windows: settings.notified_quota_windows.into_iter().collect(),
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
        if attach_to_taskbar(hwnd, selected_index) {
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
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Keep missing-window explanations aligned with the Codex quota cells.
fn update_quota_tooltips() {
    let (hwnd, regions) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else { return };
        let mut regions = Vec::new();
        if !s.dragging {
            let chinese = s.language == LanguageId::SimplifiedChinese;
            let count = active_model_count(s.show_claude_code, s.show_codex, s.show_antigravity);
            let (label_width, text_width) = usage_layout_widths(s.language, s.appearance);
            let width = model_usage_width(row_bar_segment_count(count), text_width, s.appearance);
            let mut left = sc(LEFT_DIVIDER_W)
                + sc(DIVIDER_RIGHT_MARGIN)
                + label_width
                + sc(LABEL_RIGHT_MARGIN);
            for (id, visible, name, session, weekly) in [
                (
                    0,
                    s.show_claude_code,
                    "Claude Code",
                    &s.session_text,
                    &s.weekly_text,
                ),
                (
                    1,
                    s.show_codex,
                    "Codex",
                    &s.codex_session_text,
                    &s.codex_weekly_text,
                ),
                (
                    2,
                    s.show_antigravity,
                    "Antigravity",
                    &s.antigravity_session_text,
                    &s.antigravity_weekly_text,
                ),
            ] {
                if !visible {
                    continue;
                }
                let description =
                    s.monitor.services[id].description(SystemTime::now(), Instant::now(), chinese);
                let mut text = format!("{name}\n{description}");
                if s.monitor.services[id].error.is_some() && session == "!" {
                    text.push_str(if chinese {
                        "\n本服务本次未返回额度。"
                    } else {
                        "\nThis provider did not return quota in the latest update."
                    });
                }
                if name == "Codex" && s.monitor.services[id].data.is_some() {
                    for (missing, is_weekly) in [(session == "--", false), (weekly == "--", true)] {
                        if missing {
                            text.push('\n');
                            text.push_str(quota_tooltip::message(chinese, is_weekly));
                        }
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
fn render_layered() {
    refresh_dpi();
    update_quota_tooltips();
    let (
        hwnd_val,
        is_dark,
        embedded,
        language,
        appearance,
        strings,
        session_pct,
        session_text,
        weekly_pct,
        weekly_text,
        codex_session_pct,
        codex_session_text,
        codex_weekly_pct,
        codex_weekly_text,
        antigravity_session_pct,
        antigravity_session_text,
        antigravity_weekly_pct,
        antigravity_weekly_text,
        show_claude_code,
        show_codex,
        show_antigravity,
        show_session_window,
        show_weekly_window,
    ) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.hwnd,
                s.is_dark,
                s.embedded,
                s.language,
                s.appearance,
                s.language.strings(),
                s.session_percent,
                s.session_text.clone(),
                s.weekly_percent,
                s.weekly_text.clone(),
                s.codex_session_percent,
                s.codex_session_text.clone(),
                s.codex_weekly_percent,
                s.codex_weekly_text.clone(),
                s.antigravity_session_percent,
                s.antigravity_session_text.clone(),
                s.antigravity_weekly_percent,
                s.antigravity_weekly_text.clone(),
                s.show_claude_code,
                s.show_codex,
                s.show_antigravity,
                s.show_session_window,
                s.show_weekly_window,
            ),
            None => return,
        }
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

    let accent = claude_accent_color();
    let (bg_color, text_color, track) = appearance_colors(appearance, is_dark);
    let codex_accent = appearance.decoration.color(appearance.is_dark(is_dark));
    let antigravity_accent = antigravity_accent_color();

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
        paint_content(
            mem_dc,
            width,
            height,
            is_dark,
            appearance,
            &bg_color,
            &text_color,
            &accent,
            &track,
            language,
            strings,
            session_pct,
            &session_text,
            weekly_pct,
            &weekly_text,
            codex_session_pct,
            &codex_session_text,
            codex_weekly_pct,
            &codex_weekly_text,
            antigravity_session_pct,
            &antigravity_session_text,
            antigravity_weekly_pct,
            &antigravity_weekly_text,
            show_claude_code,
            show_codex,
            show_antigravity,
            show_session_window,
            show_weekly_window,
            &codex_accent,
            &antigravity_accent,
        );

        // System mode blends with the taskbar. High contrast modes use a solid
        // backdrop, so text stays legible even over translucent wallpapers.
        let bg_bgr = bg_color.to_colorref();
        let bg_alpha = if appearance.palette == Palette::System {
            1u32
        } else {
            255u32
        };
        let pixel_data = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);
        for px in pixel_data.iter_mut() {
            let rgb = *px & 0x00FFFFFF;
            if rgb == bg_bgr {
                *px = (bg_alpha << 24) | if bg_alpha == 1 { 0 } else { rgb };
            } else {
                *px = rgb | 0xFF000000;
            }
        }

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

        let _ = UpdateLayeredWindow(
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

        // Cleanup
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

/// Paint all widget content onto a DC with a given background color.
fn paint_content(
    hdc: HDC,
    width: i32,
    height: i32,
    is_dark: bool,
    appearance: Appearance,
    bg: &Color,
    text_color: &Color,
    accent: &Color,
    track: &Color,
    language: LanguageId,
    strings: Strings,
    session_pct: f64,
    session_text: &str,
    weekly_pct: f64,
    weekly_text: &str,
    codex_session_pct: f64,
    codex_session_text: &str,
    codex_weekly_pct: f64,
    codex_weekly_text: &str,
    antigravity_session_pct: f64,
    antigravity_session_text: &str,
    antigravity_weekly_pct: f64,
    antigravity_weekly_text: &str,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_session_window: bool,
    show_weekly_window: bool,
    codex_accent: &Color,
    antigravity_accent: &Color,
) {
    unsafe {
        let is_dark = appearance.is_dark(is_dark);
        let session_pct = usage_percent_for_display(language, session_pct);
        let weekly_pct = usage_percent_for_display(language, weekly_pct);
        let codex_session_pct = usage_percent_for_display(language, codex_session_pct);
        let codex_weekly_pct = usage_percent_for_display(language, codex_weekly_pct);
        let antigravity_session_pct = usage_percent_for_display(language, antigravity_session_pct);
        let antigravity_weekly_pct = usage_percent_for_display(language, antigravity_weekly_pct);
        let (label_width, text_width) = usage_layout_widths(language, appearance);

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
        let divider_h = sc(25);
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
        let row_gap = if height < sc(WIDGET_HEIGHT) {
            sc(2)
        } else {
            sc(8)
        };
        let row1_y = (height - 2 * appearance.row_height() - row_gap).max(0) / 2;
        let row2_y = row1_y + appearance.row_height() + row_gap;
        let single_row_y = (height - appearance.row_height()) / 2;

        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));

        let font = create_widget_font(appearance);
        let old_font = SelectObject(hdc, font);

        // One provider mark per column, centered across the visible quota rows.
        let icon_size = sc(provider_icons::SIZE);
        let icon_y = (height - icon_size) / 2;
        let active_models = active_model_count(show_claude_code, show_codex, show_antigravity);
        let model_width =
            model_usage_width(row_bar_segment_count(active_models), text_width, appearance)
                + sc(MODEL_RIGHT_MARGIN);
        let mut icon_x = content_x + label_width + sc(LABEL_RIGHT_MARGIN);
        if show_claude_code && appearance.decoration.show_provider_logos {
            provider_icons::draw(hdc, icon_x, icon_y, icon_size, Provider::Claude);
            icon_x += model_width;
        }
        if show_codex && appearance.decoration.show_provider_logos {
            provider_icons::draw(hdc, icon_x, icon_y, icon_size, Provider::Codex);
        }

        if show_session_window {
            draw_row(
                hdc,
                content_x,
                if show_weekly_window {
                    row1_y
                } else {
                    single_row_y
                },
                is_dark,
                appearance,
                text_color,
                strings.session_window,
                session_pct,
                session_text,
                codex_session_pct,
                codex_session_text,
                antigravity_session_pct,
                antigravity_session_text,
                show_claude_code,
                show_codex,
                show_antigravity,
                accent,
                codex_accent,
                antigravity_accent,
                track,
                label_width,
                text_width,
            );
        }
        if show_weekly_window {
            draw_row(
                hdc,
                content_x,
                if show_session_window {
                    row2_y
                } else {
                    single_row_y
                },
                is_dark,
                appearance,
                text_color,
                strings.weekly_window,
                weekly_pct,
                weekly_text,
                codex_weekly_pct,
                codex_weekly_text,
                antigravity_weekly_pct,
                antigravity_weekly_text,
                show_claude_code,
                show_codex,
                show_antigravity,
                accent,
                codex_accent,
                antigravity_accent,
                track,
                label_width,
                text_width,
            );
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
        s.monitor.plan(Instant::now())
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
            provider_poll::execute(
                job,
                || poller::poll_provider(job.id),
                poller::credential_watch_snapshot,
            )
        },
        move |job, attempt| complete_provider_poll(send_hwnd, job, attempt),
    );
}

fn complete_provider_poll(
    send_hwnd: SendHwnd,
    job: &provider_poll::Job,
    attempt: provider_poll::Attempt,
) {
    let hwnd = send_hwnd.to_hwnd();
    let (alerts, auth_notice, alert_state_changed) = {
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
                _ => fresh.antigravity = data,
            }
        }
        let previous_alert_keys = s.notified_quota_windows.clone();
        let alerts = collect_low_quota_alerts(s, &fresh);
        let alert_state_changed = previous_alert_keys != s.notified_quota_windows;
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
        (alerts, notice, alert_state_changed)
    };
    for alert in &alerts {
        tray_icon::notify_balloon(hwnd, alert.kind, &alert.title, &alert.message);
    }
    if alert_state_changed {
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

fn update_display() {
    let mut state = lock_state();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };

    refresh_usage_texts(s);
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
    let anchor_bottom = anchor_top + anchor_height;
    (anchor_bottom - widget_height).max(anchor_top)
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
                    update_display();
                    render_layered();
                    schedule_countdown_timer();
                }
                TIMER_TASKBAR_RETRY => {
                    let attachment = {
                        let state = lock_state();
                        state.as_ref().map(|s| (s.embedded, s.taskbar_index))
                    };
                    match attachment {
                        Some((true, _)) | None => {
                            let _ = KillTimer(hwnd, TIMER_TASKBAR_RETRY);
                        }
                        Some((false, index)) => {
                            if attach_to_taskbar(hwnd, index) {
                                let _ = KillTimer(hwnd, TIMER_TASKBAR_RETRY);
                                position_at_taskbar();
                                render_layered();
                                diagnose::log("taskbar attached after startup retry");
                            }
                        }
                    }
                }
                TIMER_FRESHNESS => {
                    update_display();
                    render_layered();
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
                        {
                            let mut state = lock_state();
                            if let Some(s) = state.as_mut() {
                                s.tray_offset = new_offset;
                            }
                        }
                        if attach_to_taskbar(hwnd, target_index) {
                            let mut state = lock_state();
                            if let Some(s) = state.as_mut() {
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
                crate::appearance::FIRST_COMMAND..=crate::appearance::LAST_COMMAND => {
                    if let Some(s) = lock_state().as_mut() {
                        s.appearance.decoration.apply_command(id);
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                }
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
                id if (IDM_MONITOR_FIRST..IDM_MONITOR_FIRST + MAX_MONITOR_MENU_ITEMS as u16)
                    .contains(&id) =>
                {
                    let index = (id - IDM_MONITOR_FIRST) as usize;
                    if let Some(taskbar) = native_interop::find_taskbars().get(index).copied() {
                        if attach_to_taskbar(hwnd, index) {
                            {
                                let mut state = lock_state();
                                if let Some(s) = state.as_mut() {
                                    s.monitor_device =
                                        native_interop::taskbar_monitor_device(&taskbar);
                                    s.tray_offset = 0;
                                }
                            }
                            save_state_settings();
                            position_at_taskbar();
                            render_layered();
                        }
                    }
                }
                IDM_START_WITH_WINDOWS => {
                    set_startup_enabled(!is_startup_enabled());
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
                IDM_APPEARANCE_RECOMMENDED
                | IDM_APPEARANCE_RESET
                | IDM_PALETTE_SYSTEM
                | IDM_PALETTE_DARK
                | IDM_PALETTE_LIGHT
                | IDM_BAR_CONTINUOUS
                | IDM_BAR_SEGMENTED
                | IDM_BAR_STANDARD
                | IDM_BAR_SLIM
                | IDM_FONT_STANDARD
                | IDM_FONT_LARGE => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            match id {
                                IDM_APPEARANCE_RECOMMENDED => {
                                    s.appearance = Appearance::translucent_dark_taskbar()
                                }
                                IDM_APPEARANCE_RESET => s.appearance = Appearance::default(),
                                IDM_PALETTE_SYSTEM => s.appearance.palette = Palette::System,
                                IDM_PALETTE_DARK => {
                                    s.appearance.palette = Palette::HighContrastDark
                                }
                                IDM_PALETTE_LIGHT => {
                                    s.appearance.palette = Palette::HighContrastLight
                                }
                                IDM_BAR_CONTINUOUS => s.appearance.bar_style = BarStyle::Continuous,
                                IDM_BAR_SEGMENTED => s.appearance.bar_style = BarStyle::Segmented,
                                IDM_BAR_STANDARD => {
                                    s.appearance.bar_thickness = BarThickness::Standard
                                }
                                IDM_BAR_SLIM => s.appearance.bar_thickness = BarThickness::Slim,
                                IDM_FONT_STANDARD => s.appearance.font_size = FontSize::Standard,
                                IDM_FONT_LARGE => s.appearance.font_size = FontSize::Large,
                                _ => {}
                            }
                        }
                    }
                    save_state_settings();
                    // Font size and weight change the measured widget width.
                    position_at_taskbar();
                    render_layered();
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
                                    if s.claude_code_available
                                        && (s.show_codex
                                            || s.show_antigravity
                                            || !s.show_claude_code)
                                    {
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
                IDM_LANG_SYSTEM
                | IDM_LANG_ENGLISH
                | IDM_LANG_DUTCH
                | IDM_LANG_SPANISH
                | IDM_LANG_FRENCH
                | IDM_LANG_GERMAN
                | IDM_LANG_JAPANESE
                | IDM_LANG_KOREAN
                | IDM_LANG_SIMPLIFIED_CHINESE
                | IDM_LANG_TRADITIONAL_CHINESE
                | IDM_LANG_RUSSIAN
                | IDM_LANG_PORTUGUESE_BRAZIL => {
                    let language_override = match id {
                        IDM_LANG_SYSTEM => None,
                        IDM_LANG_ENGLISH => Some(LanguageId::English),
                        IDM_LANG_DUTCH => Some(LanguageId::Dutch),
                        IDM_LANG_SPANISH => Some(LanguageId::Spanish),
                        IDM_LANG_FRENCH => Some(LanguageId::French),
                        IDM_LANG_GERMAN => Some(LanguageId::German),
                        IDM_LANG_JAPANESE => Some(LanguageId::Japanese),
                        IDM_LANG_KOREAN => Some(LanguageId::Korean),
                        IDM_LANG_SIMPLIFIED_CHINESE => Some(LanguageId::SimplifiedChinese),
                        IDM_LANG_TRADITIONAL_CHINESE => Some(LanguageId::TraditionalChinese),
                        IDM_LANG_RUSSIAN => Some(LanguageId::Russian),
                        IDM_LANG_PORTUGUESE_BRAZIL => Some(LanguageId::PortugueseBrazil),
                        _ => None,
                    };
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
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn append_appearance_choice(menu: HMENU, id: u16, selected: bool, label: &str) {
    let wide = native_interop::wide_str(label);
    let flags = if selected {
        MF_CHECKED
    } else {
        MENU_ITEM_FLAGS(0)
    };
    let _ = AppendMenuW(menu, flags, id as usize, PCWSTR::from_raw(wide.as_ptr()));
}

unsafe fn append_appearance_submenu(parent: HMENU, child: HMENU, label: &str) {
    let wide = native_interop::wide_str(label);
    let _ = AppendMenuW(
        parent,
        MF_POPUP,
        child.0 as usize,
        PCWSTR::from_raw(wide.as_ptr()),
    );
}

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let (
            current_interval,
            strings,
            language,
            language_override,
            install_channel,
            update_status,
            widget_visible,
            show_claude_code,
            claude_code_available,
            show_codex,
            show_antigravity,
            show_session_window,
            show_weekly_window,
            alert_threshold_percent,
            appearance,
        ) = {
            let state = lock_state();
            match state.as_ref() {
                Some(s) => (
                    s.poll_interval_ms,
                    s.language.strings(),
                    s.language,
                    s.language_override,
                    s.install_channel,
                    s.update_status.clone(),
                    s.widget_visible,
                    s.show_claude_code,
                    s.claude_code_available,
                    s.show_codex,
                    s.show_antigravity,
                    s.show_session_window,
                    s.show_weekly_window,
                    s.alert_threshold_percent,
                    s.appearance,
                ),
                None => (
                    POLL_15_MIN,
                    LanguageId::English.strings(),
                    LanguageId::English,
                    None,
                    InstallChannel::Portable,
                    UpdateStatus::Idle,
                    true,
                    true,
                    false,
                    false,
                    false,
                    true,
                    true,
                    0,
                    Appearance::default(),
                ),
            }
        };

        let menu = CreatePopupMenu().unwrap();

        let refresh_str = native_interop::wide_str(strings.refresh);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            1,
            PCWSTR::from_raw(refresh_str.as_ptr()),
        );

        // Update Frequency submenu
        let freq_menu = CreatePopupMenu().unwrap();
        let freq_items: [(u16, u32, &str); 4] = [
            (IDM_FREQ_1MIN, POLL_1_MIN, strings.one_minute),
            (IDM_FREQ_5MIN, POLL_5_MIN, strings.five_minutes),
            (IDM_FREQ_15MIN, POLL_15_MIN, strings.fifteen_minutes),
            (IDM_FREQ_1HOUR, POLL_1_HOUR, strings.one_hour),
        ];
        for (id, interval, label) in freq_items {
            let label_str = native_interop::wide_str(label);
            let flags = if interval == current_interval {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                freq_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }

        let adaptive = lock_state().as_ref().is_some_and(|s| s.adaptive_refresh);
        let adaptive_label =
            native_interop::wide_str(if language == LanguageId::SimplifiedChinese {
                "剩余 ≤20% 时每分钟刷新"
            } else {
                "Refresh every minute when remaining ≤20%"
            });
        let _ = AppendMenuW(freq_menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(
            freq_menu,
            if adaptive {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            },
            IDM_ADAPTIVE_REFRESH as usize,
            PCWSTR::from_raw(adaptive_label.as_ptr()),
        );
        let freq_label = native_interop::wide_str(strings.update_frequency);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            freq_menu.0 as usize,
            PCWSTR::from_raw(freq_label.as_ptr()),
        );

        // Models submenu
        let models_menu = CreatePopupMenu().unwrap();
        let claude_label = claude_code_menu_label(strings, language, claude_code_available);
        let claude_model = native_interop::wide_str(&claude_label);
        let claude_flags = if !claude_code_available {
            MF_GRAYED
        } else if show_claude_code {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            models_menu,
            claude_flags,
            IDM_MODEL_CLAUDE_CODE as usize,
            PCWSTR::from_raw(claude_model.as_ptr()),
        );

        let codex_model = native_interop::wide_str(strings.codex_model);
        let codex_flags = if show_codex {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            models_menu,
            codex_flags,
            IDM_MODEL_CODEX as usize,
            PCWSTR::from_raw(codex_model.as_ptr()),
        );

        let antigravity_model = native_interop::wide_str(strings.antigravity_model);
        let antigravity_flags = if show_antigravity {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            models_menu,
            antigravity_flags,
            IDM_MODEL_ANTIGRAVITY as usize,
            PCWSTR::from_raw(antigravity_model.as_ptr()),
        );

        let models_label = native_interop::wide_str(strings.models);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            models_menu.0 as usize,
            PCWSTR::from_raw(models_label.as_ptr()),
        );

        // Usage window visibility submenu. Keep at least one window enabled.
        let usage_menu = CreatePopupMenu().unwrap();
        let session_label =
            native_interop::wide_str(if language == LanguageId::SimplifiedChinese {
                "5 小时额度"
            } else {
                "5-hour quota"
            });
        let session_flags = if show_session_window {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            usage_menu,
            session_flags,
            IDM_SHOW_SESSION_WINDOW as usize,
            PCWSTR::from_raw(session_label.as_ptr()),
        );
        let weekly_label = native_interop::wide_str(if language == LanguageId::SimplifiedChinese {
            "每周额度"
        } else {
            "Weekly quota"
        });
        let weekly_flags = if show_weekly_window {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            usage_menu,
            weekly_flags,
            IDM_SHOW_WEEKLY_WINDOW as usize,
            PCWSTR::from_raw(weekly_label.as_ptr()),
        );
        let usage_label = native_interop::wide_str(if language == LanguageId::SimplifiedChinese {
            "显示用量"
        } else {
            "Usage display"
        });
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            usage_menu.0 as usize,
            PCWSTR::from_raw(usage_label.as_ptr()),
        );

        let chinese = language == LanguageId::SimplifiedChinese;
        let appearance_menu = CreatePopupMenu().unwrap();
        appearance.decoration.append_menu(appearance_menu, chinese);
        let _ = AppendMenuW(appearance_menu, MF_SEPARATOR, 0, PCWSTR::null());
        append_appearance_choice(
            appearance_menu,
            IDM_APPEARANCE_RECOMMENDED,
            appearance == Appearance::translucent_dark_taskbar(),
            if chinese {
                "推荐：半透明深色任务栏"
            } else {
                "Recommended: translucent dark taskbar"
            },
        );
        append_appearance_choice(
            appearance_menu,
            IDM_APPEARANCE_RESET,
            appearance == Appearance::default(),
            if chinese {
                "恢复默认外观"
            } else {
                "Reset appearance"
            },
        );
        let _ = AppendMenuW(appearance_menu, MF_SEPARATOR, 0, PCWSTR::null());
        let palette_menu = CreatePopupMenu().unwrap();
        append_appearance_choice(
            palette_menu,
            IDM_PALETTE_SYSTEM,
            appearance.palette == Palette::System,
            if chinese {
                "跟随系统"
            } else {
                "Follow system"
            },
        );
        append_appearance_choice(
            palette_menu,
            IDM_PALETTE_DARK,
            appearance.palette == Palette::HighContrastDark,
            if chinese {
                "高对比深色"
            } else {
                "High contrast dark"
            },
        );
        append_appearance_choice(
            palette_menu,
            IDM_PALETTE_LIGHT,
            appearance.palette == Palette::HighContrastLight,
            if chinese {
                "高对比浅色"
            } else {
                "High contrast light"
            },
        );
        append_appearance_submenu(
            appearance_menu,
            palette_menu,
            if chinese {
                "文字与颜色"
            } else {
                "Text and colors"
            },
        );

        let style_menu = CreatePopupMenu().unwrap();
        append_appearance_choice(
            style_menu,
            IDM_BAR_CONTINUOUS,
            appearance.bar_style == BarStyle::Continuous,
            if chinese {
                "连续圆角"
            } else {
                "Continuous rounded"
            },
        );
        append_appearance_choice(
            style_menu,
            IDM_BAR_SEGMENTED,
            appearance.bar_style == BarStyle::Segmented,
            if chinese { "分段" } else { "Segmented" },
        );
        append_appearance_submenu(
            appearance_menu,
            style_menu,
            if chinese {
                "进度条样式"
            } else {
                "Bar style"
            },
        );

        let thickness_menu = CreatePopupMenu().unwrap();
        append_appearance_choice(
            thickness_menu,
            IDM_BAR_STANDARD,
            appearance.bar_thickness == BarThickness::Standard,
            if chinese { "标准" } else { "Standard" },
        );
        append_appearance_choice(
            thickness_menu,
            IDM_BAR_SLIM,
            appearance.bar_thickness == BarThickness::Slim,
            if chinese { "细" } else { "Slim" },
        );
        append_appearance_submenu(
            appearance_menu,
            thickness_menu,
            if chinese {
                "进度条粗细"
            } else {
                "Bar thickness"
            },
        );

        let font_menu = CreatePopupMenu().unwrap();
        append_appearance_choice(
            font_menu,
            IDM_FONT_STANDARD,
            appearance.font_size == FontSize::Standard,
            if chinese { "标准" } else { "Standard" },
        );
        append_appearance_choice(
            font_menu,
            IDM_FONT_LARGE,
            appearance.font_size == FontSize::Large,
            if chinese { "较大" } else { "Larger" },
        );
        append_appearance_submenu(
            appearance_menu,
            font_menu,
            if chinese { "文字大小" } else { "Text size" },
        );
        append_appearance_submenu(
            menu,
            appearance_menu,
            if chinese { "外观" } else { "Appearance" },
        );

        // Low-quota alert threshold submenu. Zero means opt-out.
        let alert_menu = CreatePopupMenu().unwrap();
        let alert_items = [
            (
                IDM_ALERT_OFF,
                0u8,
                if language == LanguageId::SimplifiedChinese {
                    "关闭"
                } else {
                    "Off"
                },
            ),
            (
                IDM_ALERT_10,
                10u8,
                if language == LanguageId::SimplifiedChinese {
                    "剩余 10%"
                } else {
                    "10% remaining"
                },
            ),
            (
                IDM_ALERT_20,
                20u8,
                if language == LanguageId::SimplifiedChinese {
                    "剩余 20%"
                } else {
                    "20% remaining"
                },
            ),
            (
                IDM_ALERT_30,
                30u8,
                if language == LanguageId::SimplifiedChinese {
                    "剩余 30%"
                } else {
                    "30% remaining"
                },
            ),
        ];
        for (id, threshold, label) in alert_items {
            let label = native_interop::wide_str(label);
            let flags = if alert_threshold_percent == threshold {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                alert_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label.as_ptr()),
            );
        }
        let alert_label = native_interop::wide_str(if language == LanguageId::SimplifiedChinese {
            "额度提醒"
        } else {
            "Quota alerts"
        });
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            alert_menu.0 as usize,
            PCWSTR::from_raw(alert_label.as_ptr()),
        );

        // Settings submenu
        let settings_menu = CreatePopupMenu().unwrap();

        let startup_str = native_interop::wide_str(strings.start_with_windows);
        let startup_flags = if is_startup_enabled() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            settings_menu,
            startup_flags,
            IDM_START_WITH_WINDOWS as usize,
            PCWSTR::from_raw(startup_str.as_ptr()),
        );

        let anchored = lock_state().as_ref().is_some_and(|s| s.anchor_left);
        let left_label = native_interop::wide_str(if language == LanguageId::SimplifiedChinese {
            "固定在任务栏最左侧"
        } else {
            "Pin to the left edge of the taskbar"
        });
        let _ = AppendMenuW(
            settings_menu,
            if anchored {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            },
            IDM_ANCHOR_LEFT as usize,
            PCWSTR::from_raw(left_label.as_ptr()),
        );
        let reset_pos_str = native_interop::wide_str(strings.reset_position);
        let _ = AppendMenuW(
            settings_menu,
            MENU_ITEM_FLAGS(0),
            IDM_RESET_POSITION as usize,
            PCWSTR::from_raw(reset_pos_str.as_ptr()),
        );

        let taskbars = native_interop::find_taskbars();
        if taskbars.len() > 1 {
            let selected_taskbar = {
                let state = lock_state();
                state.as_ref().and_then(|s| s.taskbar_hwnd)
            };
            let monitor_menu = CreatePopupMenu().unwrap();
            for (index, taskbar) in taskbars.iter().take(MAX_MONITOR_MENU_ITEMS).enumerate() {
                let device = native_interop::taskbar_monitor_device(taskbar)
                    .unwrap_or_else(|| format!("{}", index + 1));
                let label = if language == LanguageId::SimplifiedChinese {
                    format!("显示器 {} ({device})", index + 1)
                } else {
                    format!("Display {} ({device})", index + 1)
                };
                let wide = native_interop::wide_str(&label);
                let flags = if selected_taskbar == Some(taskbar.hwnd) {
                    MF_CHECKED
                } else {
                    MENU_ITEM_FLAGS(0)
                };
                let _ = AppendMenuW(
                    monitor_menu,
                    flags,
                    (IDM_MONITOR_FIRST as usize) + index,
                    PCWSTR::from_raw(wide.as_ptr()),
                );
            }
            let label = native_interop::wide_str(if language == LanguageId::SimplifiedChinese {
                "显示器"
            } else {
                "Display"
            });
            let _ = AppendMenuW(
                settings_menu,
                MF_POPUP,
                monitor_menu.0 as usize,
                PCWSTR::from_raw(label.as_ptr()),
            );
        }

        let language_menu = CreatePopupMenu().unwrap();
        let system_label = native_interop::wide_str(strings.system_default);
        let system_flags = if language_override.is_none() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            language_menu,
            system_flags,
            IDM_LANG_SYSTEM as usize,
            PCWSTR::from_raw(system_label.as_ptr()),
        );

        for language in LanguageId::ALL {
            let id = match language {
                LanguageId::English => IDM_LANG_ENGLISH,
                LanguageId::Dutch => IDM_LANG_DUTCH,
                LanguageId::Spanish => IDM_LANG_SPANISH,
                LanguageId::French => IDM_LANG_FRENCH,
                LanguageId::German => IDM_LANG_GERMAN,
                LanguageId::Japanese => IDM_LANG_JAPANESE,
                LanguageId::Korean => IDM_LANG_KOREAN,
                LanguageId::SimplifiedChinese => IDM_LANG_SIMPLIFIED_CHINESE,
                LanguageId::TraditionalChinese => IDM_LANG_TRADITIONAL_CHINESE,
                LanguageId::Russian => IDM_LANG_RUSSIAN,
                LanguageId::PortugueseBrazil => IDM_LANG_PORTUGUESE_BRAZIL,
            };
            let label_str = native_interop::wide_str(language.native_name());
            let flags = if language_override == Some(language) {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                language_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }

        let language_label = native_interop::wide_str(strings.language);
        let _ = AppendMenuW(
            settings_menu,
            MF_POPUP,
            language_menu.0 as usize,
            PCWSTR::from_raw(language_label.as_ptr()),
        );

        let _ = AppendMenuW(settings_menu, MF_SEPARATOR, 0, PCWSTR::null());

        let version_label =
            version_action_label(strings, language, install_channel, &update_status);
        let version_str = native_interop::wide_str(&version_label);
        let version_flags = if matches!(
            update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            MF_GRAYED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            settings_menu,
            version_flags,
            IDM_VERSION_ACTION as usize,
            PCWSTR::from_raw(version_str.as_ptr()),
        );

        let settings_label = native_interop::wide_str(strings.settings);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            settings_menu.0 as usize,
            PCWSTR::from_raw(settings_label.as_ptr()),
        );

        let widget_label = native_interop::wide_str(strings.show_widget);
        let widget_flags = if widget_visible {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            menu,
            widget_flags,
            tray_icon::IDM_TOGGLE_WIDGET as usize,
            PCWSTR::from_raw(widget_label.as_ptr()),
        );

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let exit_str = native_interop::wide_str(strings.exit);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            2,
            PCWSTR::from_raw(exit_str.as_ptr()),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

/// Paint for non-embedded fallback (normal WM_PAINT path)
fn paint(hdc: HDC, hwnd: HWND) {
    let (
        is_dark,
        appearance,
        language,
        strings,
        session_pct,
        session_text,
        weekly_pct,
        weekly_text,
        codex_session_pct,
        codex_session_text,
        codex_weekly_pct,
        codex_weekly_text,
        antigravity_session_pct,
        antigravity_session_text,
        antigravity_weekly_pct,
        antigravity_weekly_text,
        show_claude_code,
        show_codex,
        show_antigravity,
        show_session_window,
        show_weekly_window,
    ) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.is_dark,
                s.appearance,
                s.language,
                s.language.strings(),
                s.session_percent,
                s.session_text.clone(),
                s.weekly_percent,
                s.weekly_text.clone(),
                s.codex_session_percent,
                s.codex_session_text.clone(),
                s.codex_weekly_percent,
                s.codex_weekly_text.clone(),
                s.antigravity_session_percent,
                s.antigravity_session_text.clone(),
                s.antigravity_weekly_percent,
                s.antigravity_weekly_text.clone(),
                s.show_claude_code,
                s.show_codex,
                s.show_antigravity,
                s.show_session_window,
                s.show_weekly_window,
            ),
            None => return,
        }
    };

    let accent = claude_accent_color();
    let (bg_color, text_color, track) = appearance_colors(appearance, is_dark);
    let codex_accent = appearance.decoration.color(appearance.is_dark(is_dark));
    let antigravity_accent = antigravity_accent_color();

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

        paint_content(
            mem_dc,
            width,
            height,
            is_dark,
            appearance,
            &bg_color,
            &text_color,
            &accent,
            &track,
            language,
            strings,
            session_pct,
            &session_text,
            weekly_pct,
            &weekly_text,
            codex_session_pct,
            &codex_session_text,
            codex_weekly_pct,
            &codex_weekly_text,
            antigravity_session_pct,
            &antigravity_session_text,
            antigravity_weekly_pct,
            &antigravity_weekly_text,
            show_claude_code,
            show_codex,
            show_antigravity,
            show_session_window,
            show_weekly_window,
            &codex_accent,
            &antigravity_accent,
        );

        let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);

        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp);
        let _ = DeleteDC(mem_dc);
    }
}

fn draw_row(
    hdc: HDC,
    x: i32,
    y: i32,
    is_dark: bool,
    appearance: Appearance,
    text_color: &Color,
    label: &str,
    claude_percent: f64,
    claude_text: &str,
    codex_percent: f64,
    codex_text: &str,
    antigravity_percent: f64,
    antigravity_text: &str,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    claude_accent: &Color,
    codex_accent: &Color,
    antigravity_accent: &Color,
    track: &Color,
    label_width: i32,
    text_width: i32,
) {
    let active_models = active_model_count(show_claude_code, show_codex, show_antigravity);
    let segment_count = row_bar_segment_count(active_models);
    let use_model_text_colors = active_models > 1 && appearance.palette == Palette::System;
    let is_dark = appearance.is_dark(is_dark);
    let claude_value_color = if use_model_text_colors {
        claude_usage_text_color(is_dark)
    } else {
        *text_color
    };
    let codex_value_color = *codex_accent;
    let antigravity_value_color = if use_model_text_colors {
        antigravity_usage_text_color(is_dark)
    } else {
        *text_color
    };

    unsafe {
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));
        let mut label_wide: Vec<u16> = label.encode_utf16().collect();
        let mut label_rect = RECT {
            left: x,
            top: y,
            right: x + label_width,
            bottom: y + appearance.row_height(),
        };
        let _ = DrawTextW(
            hdc,
            &mut label_wide,
            &mut label_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        let mut model_x = x + label_width + sc(LABEL_RIGHT_MARGIN);
        if show_claude_code {
            draw_usage_bar(
                hdc,
                model_x + sc(appearance.decoration.icon_width()),
                y,
                segment_count,
                claude_percent,
                claude_text,
                claude_accent,
                track,
                &claude_value_color,
                text_width,
                is_dark,
                appearance,
            );
            model_x +=
                model_usage_width(segment_count, text_width, appearance) + sc(MODEL_RIGHT_MARGIN);
        }
        if show_codex {
            draw_usage_bar(
                hdc,
                model_x + sc(appearance.decoration.icon_width()),
                y,
                segment_count,
                codex_percent,
                codex_text,
                codex_accent,
                track,
                &codex_value_color,
                text_width,
                is_dark,
                appearance,
            );
            model_x +=
                model_usage_width(segment_count, text_width, appearance) + sc(MODEL_RIGHT_MARGIN);
        }
        if show_antigravity {
            draw_usage_bar(
                hdc,
                model_x + sc(appearance.decoration.icon_width()),
                y,
                segment_count,
                antigravity_percent,
                antigravity_text,
                antigravity_accent,
                track,
                &antigravity_value_color,
                text_width,
                is_dark,
                appearance,
            );
        }
    }
}

fn model_usage_width(segment_count: i32, text_width: i32, appearance: Appearance) -> i32 {
    sc(appearance.decoration.icon_width()) + (sc(SEGMENT_W) + sc(SEGMENT_GAP)) * segment_count
        - sc(SEGMENT_GAP)
        + sc(BAR_RIGHT_MARGIN)
        + text_width
}

fn usage_bar_fill_percentage(percent: f64, text: &str) -> f64 {
    if text == "--" {
        0.0
    } else {
        percent.clamp(0.0, 100.0)
    }
}

fn draw_usage_bar(
    hdc: HDC,
    bar_x: i32,
    y: i32,
    segment_count: i32,
    percent: f64,
    text: &str,
    accent: &Color,
    track: &Color,
    text_color: &Color,
    text_width: i32,
    is_dark: bool,
    appearance: Appearance,
) {
    let seg_w = sc(SEGMENT_W);
    let seg_h = appearance.bar_height();
    let seg_gap = sc(SEGMENT_GAP);
    let bar_width = segment_count * (seg_w + seg_gap) - seg_gap;
    let corner_r = seg_h / 2;
    let bar_y = y + (appearance.row_height() - seg_h) / 2;

    let percent_clamped = usage_bar_fill_percentage(percent, text);
    if appearance.bar_style == BarStyle::Segmented {
        for i in 0..segment_count {
            let rect = RECT {
                left: bar_x + i * (seg_w + seg_gap),
                top: bar_y,
                right: bar_x + i * (seg_w + seg_gap) + seg_w,
                bottom: bar_y + seg_h,
            };
            draw_rounded_rect(hdc, &rect, track, corner_r);
            let fraction =
                (percent_clamped * segment_count as f64 / 100.0 - i as f64).clamp(0.0, 1.0);
            fill_rounded_bar(hdc, &rect, fraction, accent, corner_r);
        }
    } else {
        let rect = RECT {
            left: bar_x,
            top: bar_y,
            right: bar_x + bar_width,
            bottom: bar_y + seg_h,
        };
        draw_rounded_rect(hdc, &rect, track, corner_r);
        fill_rounded_bar(hdc, &rect, percent_clamped / 100.0, accent, corner_r);
    }

    let text_x = bar_x + bar_width + sc(BAR_RIGHT_MARGIN);
    quota_text::draw(
        hdc,
        text_x,
        y,
        appearance.row_height(),
        text_width,
        text,
        text_color,
        &reset_text_color(appearance, is_dark),
        appearance.font_px(),
        sc,
    );
}

fn fill_rounded_bar(hdc: HDC, rect: &RECT, fraction: f64, color: &Color, radius: i32) {
    let width = ((rect.right - rect.left) as f64 * fraction).round() as i32;
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
        let _ = SelectClipRgn(hdc, clip);
        let fill = RECT {
            right: rect.left + width,
            ..*rect
        };
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        FillRect(hdc, &fill, brush);
        let _ = DeleteObject(brush);
        let _ = SelectClipRgn(hdc, HRGN::default());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_render_uses_selected_color_and_reclaims_logo_space() {
        use crate::appearance::CodexColor;
        for language in [LanguageId::English, LanguageId::SimplifiedChinese] {
            for dark in [false, true] {
                for color in [
                    CodexColor::Green,
                    CodexColor::Neutral,
                    CodexColor::Blue,
                    CodexColor::Purple,
                ] {
                    for logos in [false, true] {
                        let appearance = Appearance {
                            decoration: crate::appearance::Appearance {
                                codex_color: color,
                                show_provider_logos: logos,
                            },
                            ..Default::default()
                        };
                        let width = total_widget_width_for(2, language, appearance);
                        let hidden = Appearance {
                            decoration: crate::appearance::Appearance {
                                show_provider_logos: false,
                                ..appearance.decoration
                            },
                            ..appearance
                        };
                        assert_eq!(
                            width - total_widget_width_for(2, language, hidden),
                            if logos { 2 * sc(21) } else { 0 }
                        );
                        let height = sc(WIDGET_HEIGHT);
                        unsafe {
                            let dc = CreateCompatibleDC(None);
                            let info = BITMAPINFO {
                                bmiHeader: BITMAPINFOHEADER {
                                    biSize: 40,
                                    biWidth: width,
                                    biHeight: -height,
                                    biPlanes: 1,
                                    biBitCount: 32,
                                    ..Default::default()
                                },
                                ..Default::default()
                            };
                            let mut bits = std::ptr::null_mut();
                            let bitmap =
                                CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)
                                    .unwrap();
                            let old = SelectObject(dc, bitmap);
                            let bg = Color::from_hex(if dark { "#1C1C1C" } else { "#F3F3F3" });
                            let fg = Color::from_hex(if dark { "#888888" } else { "#404040" });
                            let track = Color::from_hex(if dark { "#444444" } else { "#AAAAAA" });
                            let accent = appearance.decoration.color(dark);
                            paint_content(
                                dc,
                                width,
                                height,
                                dark,
                                appearance,
                                &bg,
                                &fg,
                                &claude_accent_color(),
                                &track,
                                language,
                                language.strings(),
                                50.0,
                                "50%",
                                50.0,
                                "50%",
                                50.0,
                                "50%",
                                50.0,
                                "50%",
                                0.0,
                                "--",
                                0.0,
                                "--",
                                true,
                                true,
                                false,
                                true,
                                true,
                                &accent,
                                &antigravity_accent_color(),
                            );
                            let (label_width, text_width) =
                                usage_layout_widths(language, appearance);
                            let codex_x = sc(LEFT_DIVIDER_W
                                + DIVIDER_RIGHT_MARGIN
                                + label_width
                                + LABEL_RIGHT_MARGIN)
                                + model_usage_width(
                                    row_bar_segment_count(2),
                                    text_width,
                                    appearance,
                                )
                                + sc(MODEL_RIGHT_MARGIN)
                                + sc(appearance.decoration.icon_width());
                            let sample = GetPixel(
                                dc,
                                codex_x + sc(5),
                                (height - 2 * appearance.row_height() - sc(8)).max(0) / 2
                                    + appearance.row_height()
                                    + sc(8)
                                    + appearance.row_height() / 2,
                            );
                            assert_eq!(sample.0, accent.to_colorref());
                            if let Some(directory) =
                                std::env::var_os("CODEX_USAGE_APPEARANCE_PREVIEW")
                            {
                                let count = (width * height * 4) as usize;
                                let mut bytes = Vec::new();
                                bytes.extend(b"BM");
                                bytes.extend(((54 + count) as u32).to_le_bytes());
                                bytes.extend([0u8; 4]);
                                bytes.extend(54u32.to_le_bytes());
                                bytes.extend(40u32.to_le_bytes());
                                bytes.extend(width.to_le_bytes());
                                bytes.extend((-height).to_le_bytes());
                                bytes.extend(1u16.to_le_bytes());
                                bytes.extend(32u16.to_le_bytes());
                                bytes.extend([0u8; 24]);
                                bytes.extend(std::slice::from_raw_parts(bits as *const u8, count));
                                std::fs::create_dir_all(&directory).unwrap();
                                std::fs::write(
                                    std::path::Path::new(&directory).join(format!(
                                        "{}-{dark}-{color:?}-{logos}.bmp",
                                        language.code()
                                    )),
                                    bytes,
                                )
                                .unwrap();
                            }
                            SelectObject(dc, old);
                            let _ = DeleteObject(bitmap);
                            let _ = DeleteDC(dc);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unavailable_quota_has_no_bar_fill() {
        assert_eq!(usage_bar_fill_percentage(100.0, "--"), 0.0);
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
                    (Palette::System, FontSize::Standard),
                    (Palette::System, FontSize::Large),
                    (Palette::HighContrastDark, FontSize::Standard),
                    (Palette::HighContrastDark, FontSize::Large),
                ] {
                    let appearance = Appearance {
                        palette,
                        font_size,
                        ..Appearance::default()
                    };
                    let (label_width, text_width) = usage_layout_widths(language, appearance);
                    let reset_width =
                        text_width - sc(quota_text::reset_offset(appearance.font_px()));
                    let label_font = create_widget_font(appearance);
                    let reset_font = quota_text::create_reset_font(sc(appearance.font_px()));
                    assert!(!label_font.is_invalid() && !reset_font.is_invalid());
                    let (labels, resets) = usage_layout_samples(language);
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

    #[test]
    fn unavailable_claude_cli_has_an_explicit_menu_label() {
        assert_eq!(
            claude_code_menu_label(
                LanguageId::SimplifiedChinese.strings(),
                LanguageId::SimplifiedChinese,
                false,
            ),
            "Claude Code（需登录 CLI）"
        );
        assert_eq!(
            claude_code_menu_label(LanguageId::English.strings(), LanguageId::English, true),
            "Claude Code"
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
        let mut selected = restored;
        selected.appearance.decoration.apply_command(111);
        selected.appearance.decoration.apply_command(114);
        let json = serde_json::to_string(&selected).unwrap();
        let restored: SettingsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.appearance, selected.appearance);
        assert_eq!(restored.appearance.palette, Palette::HighContrastDark);
        assert_eq!(restored.appearance.font_size, FontSize::Large);
        assert!(json.contains("\"codex_color\":\"neutral\""));
        assert!(json.contains("\"show_provider_logos\":false"));
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
    fn unavailable_claude_cli_is_disabled_without_disabling_codex() {
        let settings = SettingsFile {
            show_claude_code: true,
            show_codex: false,
            show_antigravity: false,
            ..SettingsFile::default()
        };

        let (settings, changed) = apply_claude_code_availability(settings, false);

        assert!(changed);
        assert!(!settings.show_claude_code);
        assert!(settings.show_codex);
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
    fn low_quota_alert_does_not_repeat_when_reset_time_drifts() {
        let mut alerts = Vec::new();
        let mut notified = BTreeSet::new();
        for seconds in [2_000_000_000, 2_000_000_001, 1_999_999_999] {
            append_quota_alert(
                &mut alerts,
                &mut notified,
                10,
                LanguageId::SimplifiedChinese,
                tray_icon::TrayIconKind::Claude,
                "claude",
                "Claude",
                "session",
                "5h",
                &crate::models::UsageSection {
                    percentage: 95.0,
                    resets_at: Some(UNIX_EPOCH + Duration::from_secs(seconds)),
                },
            );
        }
        assert_eq!(
            alerts.len(),
            1,
            "reset timestamp drift must not trigger another low-quota alert"
        );
    }

    #[test]
    fn low_quota_alert_is_deduplicated_until_reset_window_changes() {
        let mut alerts = Vec::new();
        let mut notified = BTreeSet::new();
        let first_reset = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let first = crate::models::UsageSection {
            percentage: 85.0,
            resets_at: Some(first_reset),
        };

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

        let next = crate::models::UsageSection {
            percentage: 90.0,
            resets_at: Some(first_reset + Duration::from_secs(18_000)),
        };
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
}
