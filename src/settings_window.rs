//! Modeless settings for the taskbar widget. Controls reflect the live app state.
use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use windows::core::w;
use windows::Win32::UI::Controls::{SetScrollInfo, DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};

const CLASS: PCWSTR = w!("CodexUsageSettings");
const NAV: u16 = 990;
const FIRST_FIELD: u16 = 1000;
const FIELDS_PER_ROW: u16 = 4;
const REFRESH_TIMER: usize = 1;
const APPLY_SELECTION: u32 = WM_APP + 1;
const WIDTH: i32 = 760;
const HEIGHT: i32 = 554;
const SIDEBAR: i32 = 166;
const PAD: i32 = 24;
const FIRST_ROW: i32 = 78;
const ROW_HEIGHT: i32 = 36;
const FOOTER: i32 = 52;
const STATIC_TEXT: u32 = 0x0200 | 0x0080; // SS_CENTERIMAGE | SS_NOPREFIX

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
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
}

impl OptionItem {
    fn command(label: impl Into<String>, id: u16) -> Self {
        Self {
            label: label.into(),
            action: Some(Action::Command(id)),
        }
    }

    fn placeholder(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Check {
        item: OptionItem,
        checked: bool,
        enabled: bool,
    },
    Choice {
        label: &'static str,
        items: Vec<OptionItem>,
        selected: usize,
        enabled: bool,
    },
    Buttons(Vec<OptionItem>),
    Note(String),
}

fn check(label: impl Into<String>, id: u16, checked: bool) -> Row {
    Row::Check {
        item: OptionItem::command(label, id),
        checked,
        enabled: true,
    }
}

fn limited_check(label: impl Into<String>, id: u16, checked: bool, enabled: bool) -> Row {
    Row::Check {
        item: OptionItem::command(label, id),
        checked,
        enabled,
    }
}

fn service_label(
    name: &str,
    monitored: bool,
    error: Option<poller::PollError>,
    language: LanguageId,
) -> String {
    use poller::PollError;

    let reauthenticate = match (monitored, error) {
        (true, Some(PollError::NoCredentials)) => false,
        (true, Some(PollError::AuthRequired | PollError::TokenExpired)) => true,
        _ => return name.into(),
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
    let status = if reauthenticate {
        sign_in_again
    } else {
        signed_out
    };
    format!("{name} ({status})")
}

fn choice(label: &'static str, values: &[(u16, &str)], selected_command: u16) -> Row {
    Row::Choice {
        label,
        items: values
            .iter()
            .map(|(id, name)| OptionItem::command(*name, *id))
            .collect(),
        selected: values
            .iter()
            .position(|(id, _)| *id == selected_command)
            .unwrap_or(0),
        enabled: true,
    }
}

fn buttons(values: &[(u16, &str)]) -> Row {
    Row::Buttons(
        values
            .iter()
            .map(|(id, name)| OptionItem::command(*name, *id))
            .collect(),
    )
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
    let chinese = s.language == LanguageId::SimplifiedChinese;
    let t = |zh, en| if chinese { zh } else { en };
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
                    let label = format!("{} {}", t("显示器", "Display"), index + 1);
                    let device = native_interop::taskbar_monitor_device(taskbar);
                    OptionItem {
                        label: device
                            .map_or_else(|| label.clone(), |device| format!("{label} ({device})")),
                        action: Some(Action::Display(taskbar.hwnd)),
                    }
                })
                .collect();
            let selected_display = taskbars
                .iter()
                .position(|bar| Some(bar.hwnd) == s.taskbar_hwnd);
            let mut result = vec![
                choice(strings.language, &values, selected_language),
                check(
                    strings.start_with_windows,
                    IDM_START_WITH_WINDOWS,
                    is_startup_enabled(),
                ),
                check(
                    strings.show_widget,
                    tray_icon::IDM_TOGGLE_WIDGET,
                    s.widget_visible,
                ),
                check(
                    t("固定在任务栏最左侧", "Pin to the left edge of the taskbar"),
                    IDM_ANCHOR_LEFT,
                    s.anchor_left,
                ),
            ];
            if !taskbars.is_empty() {
                result.push(Row::Choice {
                    label: t("显示器", "Display"),
                    items: displays,
                    selected: selected_display.unwrap_or(0),
                    enabled: taskbars.len() > 1,
                });
            }
            result.push(buttons(&[(IDM_RESET_POSITION, strings.reset_position)]));
            result
        }
        Page::Usage => {
            let providers = [s.show_codex, s.show_claude_code, s.show_antigravity];
            let count = providers.iter().filter(|enabled| **enabled).count();
            let [claude, codex, antigravity] = &s.monitor.services;
            let provider = |name, id, monitored, error| {
                limited_check(
                    service_label(name, monitored, error, s.language),
                    id,
                    monitored,
                    !monitored || count > 1,
                )
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
                })
                .collect();
            let (quota, used, remaining) = match s.language {
                LanguageId::SimplifiedChinese => ("额度显示", "已用", "剩余"),
                LanguageId::Italian => ("Visualizzazione quota", "Utilizzata", "Rimanente"),
                _ => ("Quota display", "Used", "Remaining"),
            };
            vec![
                Row::Note(strings.models.into()),
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
                    label: t("Antigravity 额度组", "Antigravity quota group"),
                    items: group_items,
                    selected: selected_group,
                    enabled: s.show_antigravity,
                },
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
                    t(
                        "剩余 ≤20% 时每分钟刷新",
                        "Refresh every minute when remaining ≤20%",
                    ),
                    IDM_ADAPTIVE_REFRESH,
                    s.adaptive_refresh,
                ),
                limited_check(
                    t("显示 5 小时额度", "Show 5-hour quota"),
                    IDM_SHOW_SESSION_WINDOW,
                    s.show_session_window,
                    s.show_weekly_window,
                ),
                limited_check(
                    t("显示每周额度", "Show weekly quota"),
                    IDM_SHOW_WEEKLY_WINDOW,
                    s.show_weekly_window,
                    s.show_session_window,
                ),
                choice(
                    quota,
                    &[(IDM_QUOTA_REMAINING, remaining), (IDM_QUOTA_USED, used)],
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
            let library = s.color_presets.entries();
            let mut colors = vec![OptionItem::placeholder(t("自定义", "Custom"))];
            colors.extend(ColorPreset::ALL.iter().enumerate().map(|(index, preset)| {
                OptionItem::command(preset.label(chinese), IDM_CODEX_PRESET_FIRST + index as u16)
            }));
            colors.extend(library.iter().enumerate().map(|(index, preset)| {
                OptionItem::command(&preset.name, IDM_USER_PRESET_FIRST + index as u16)
            }));
            let selected = ColorPreset::ALL
                .iter()
                .position(|preset| a.codex_colors.is_preset(*preset))
                .map(|index| index + 1)
                .or_else(|| {
                    library
                        .iter()
                        .position(|preset| a.codex_colors == preset.colors)
                        .map(|index| index + ColorPreset::ALL.len() + 1)
                })
                .unwrap_or(0);
            let mut deletions = vec![OptionItem::placeholder(t("选择预设…", "Select preset…"))];
            deletions.extend(library.iter().enumerate().map(|(index, preset)| {
                OptionItem::command(&preset.name, IDM_DELETE_PRESET_FIRST + index as u16)
            }));
            vec![
                choice(
                    t("背景样式", "Background style"),
                    &[
                        (IDM_PALETTE_SYSTEM, t("跟随系统", "Follow system")),
                        (IDM_PALETTE_DARK, t("高对比深色", "High contrast dark")),
                        (IDM_PALETTE_LIGHT, t("高对比浅色", "High contrast light")),
                    ],
                    match a.palette {
                        Palette::System => IDM_PALETTE_SYSTEM,
                        Palette::HighContrastDark => IDM_PALETTE_DARK,
                        Palette::HighContrastLight => IDM_PALETTE_LIGHT,
                    },
                ),
                choice(
                    t("进度条样式", "Bar style"),
                    &[
                        (IDM_BAR_CONTINUOUS, t("连续圆角", "Continuous rounded")),
                        (IDM_BAR_SEGMENTED, t("分段", "Segmented")),
                        (
                            IDM_BAR_CLASSIC,
                            t("经典分段（矩形）", "Classic segmented (tiles)"),
                        ),
                    ],
                    match a.bar_style {
                        BarStyle::Continuous => IDM_BAR_CONTINUOUS,
                        BarStyle::Segmented => IDM_BAR_SEGMENTED,
                        BarStyle::ClassicSegmented => IDM_BAR_CLASSIC,
                    },
                ),
                choice(
                    t("进度条粗细", "Bar thickness"),
                    &[
                        (IDM_BAR_STANDARD, t("标准", "Standard")),
                        (IDM_BAR_SLIM, t("细", "Slim")),
                    ],
                    match a.bar_thickness {
                        BarThickness::Standard => IDM_BAR_STANDARD,
                        BarThickness::Slim => IDM_BAR_SLIM,
                    },
                ),
                choice(
                    t("文字大小", "Text size"),
                    &[
                        (IDM_FONT_STANDARD, t("标准", "Standard")),
                        (IDM_FONT_LARGE, t("较大", "Larger")),
                    ],
                    match a.font_size {
                        FontSize::Standard => IDM_FONT_STANDARD,
                        FontSize::Large => IDM_FONT_LARGE,
                    },
                ),
                check(
                    t("显示服务图标", "Show provider logos"),
                    IDM_PROVIDER_LOGOS,
                    !a.hide_provider_logos,
                ),
                check(
                    t(
                        "紧凑布局（简体中文，单服务）",
                        "Compact layout (Simplified Chinese, one service)",
                    ),
                    IDM_COMPACT_LAYOUT,
                    a.compact_layout,
                ),
                Row::Choice {
                    label: t("Codex 配色", "Codex colors"),
                    items: colors,
                    selected,
                    enabled: true,
                },
                buttons(&[
                    (IDM_CODEX_ICON_COLOR, t("图标颜色…", "Icon color…")),
                    (IDM_CODEX_BAR_COLOR, t("进度条颜色…", "Bar color…")),
                    (
                        IDM_CODEX_PERCENT_COLOR,
                        t("百分比颜色…", "Percentage color…"),
                    ),
                ]),
                buttons(&[(
                    IDM_SAVE_COLOR_PRESET,
                    t("保存当前配色为预设…", "Save current colors as preset…"),
                )]),
                Row::Choice {
                    label: t("删除自定义预设", "Delete custom preset"),
                    items: deletions,
                    selected: 0,
                    enabled: !library.is_empty(),
                },
                Row::Buttons(vec![
                    OptionItem::command(
                        t("半透明深色预设", "Translucent dark preset"),
                        IDM_APPEARANCE_RECOMMENDED,
                    ),
                    OptionItem::command(
                        t("恢复默认外观", "Reset appearance"),
                        IDM_APPEARANCE_RESET,
                    ),
                ]),
            ]
        }
        Page::Alerts => vec![
            choice(
                t("低额度提醒", "Low-quota alert"),
                &[
                    (IDM_ALERT_OFF, t("关闭", "Off")),
                    (IDM_ALERT_10, t("剩余 10%", "10% remaining")),
                    (IDM_ALERT_20, t("剩余 20%", "20% remaining")),
                    (IDM_ALERT_30, t("剩余 30%", "30% remaining")),
                ],
                match s.alert_threshold_percent {
                    10 => IDM_ALERT_10,
                    20 => IDM_ALERT_20,
                    30 => IDM_ALERT_30,
                    _ => IDM_ALERT_OFF,
                },
            ),
            check(
                t("额度重置时通知", "Notify on reset"),
                IDM_ALERT_RESET,
                s.notify_on_reset,
            ),
        ],
        Page::Privacy => {
            let policy = crate::privacy::policy();
            let updates = crate::privacy::updates_enabled();
            let label =
                version_action_label(strings, s.language, s.install_channel, &s.update_status);
            let mut result = vec![
                limited_check(
                    t("被动监控", "Passive monitoring"),
                    IDM_PASSIVE_MONITORING,
                    crate::privacy::passive_monitoring(),
                    !policy.force_passive_monitoring,
                ),
                check(
                    mcp_settings_label(chinese, s.mcp_unavailable, mcp_port(s.mcp_port)),
                    IDM_MCP_SERVER,
                    s.mcp_enabled,
                ),
                limited_check(
                    t("允许检查并安装更新", "Allow update checks and installation"),
                    IDM_UPDATES_ENABLED,
                    updates,
                    !policy.disable_updates,
                ),
            ];
            if policy.force_passive_monitoring || policy.disable_updates {
                result.push(Row::Note(
                    t(
                        "部分选项由设备策略管理。",
                        "Some options are managed by device policy.",
                    )
                    .into(),
                ));
            }
            let busy = matches!(
                s.update_status,
                UpdateStatus::Checking | UpdateStatus::Applying
            );
            if updates && !busy {
                result.push(Row::Buttons(vec![OptionItem::command(
                    label,
                    IDM_VERSION_ACTION,
                )]));
            } else {
                result.push(Row::Note(label));
            }
            result
        }
    }
}

struct Panel {
    hwnd: HWND,
    widget: HWND,
    nav: HWND,
    heading: HWND,
    footer: HWND,
    font: HFONT,
    title_font: HFONT,
    dpi: u32,
    language: Option<LanguageId>,
    save_failed: bool,
    page: usize,
    rows: Vec<Row>,
    controls: Vec<Vec<HWND>>,
    scroll: usize,
    pending: VecDeque<Option<Action>>,
}

impl Panel {
    unsafe fn new(hwnd: HWND, widget: HWND) -> Self {
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
        let heading = child(hwnd, w!("STATIC"), 0, STATIC_TEXT);
        let footer = child(hwnd, w!("STATIC"), 0, STATIC_TEXT);
        let mut panel = Self {
            hwnd,
            widget,
            nav,
            heading,
            footer,
            font: HFONT::default(),
            title_font: HFONT::default(),
            dpi: 0,
            language: None,
            save_failed: false,
            page: 0,
            rows: Vec::new(),
            controls: Vec::new(),
            scroll: 0,
            pending: VecDeque::new(),
        };
        panel.set_dpi(GetDpiForWindow(hwnd).max(96));
        panel
    }

    fn px(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    unsafe fn set_dpi(&mut self, dpi: u32) {
        let old = std::mem::replace(&mut self.font, ui_font(scale(16, dpi), FW_NORMAL.0));
        let old_title =
            std::mem::replace(&mut self.title_font, ui_font(scale(26, dpi), FW_SEMIBOLD.0));
        self.dpi = dpi;
        set_font(self.heading, self.title_font);
        set_font(self.footer, self.font);
        set_font(self.nav, self.font);
        let _ = SendMessageW(
            self.nav,
            LB_SETITEMHEIGHT,
            WPARAM(0),
            LPARAM(self.px(39) as isize),
        );
        for controls in &self.controls {
            for &control in controls {
                set_font(control, self.font);
            }
        }
        let _ = DeleteObject(old);
        let _ = DeleteObject(old_title);
    }

    unsafe fn sync(&mut self, force: bool) {
        let Some((language, next)) = lock_state()
            .as_ref()
            .map(|s| (s.language, rows(Page::ALL[self.page], s)))
        else {
            return;
        };
        // A live quota reply must not replace the list while a choice is open.
        if self
            .rows
            .iter()
            .zip(&self.controls)
            .any(|(row, controls)| matches!(row, Row::Choice { .. }) && is_dropped(controls[1]))
        {
            return;
        }

        let titles = page_titles(language);
        let language_changed = self.language != Some(language);
        if language_changed {
            self.language = Some(language);
            set_text(
                self.hwnd,
                &format!("Codex Usage — {}", language.strings().settings),
            );
            let _ = SendMessageW(self.nav, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
            for title in titles {
                let title = native_interop::wide_str(title);
                let _ = SendMessageW(
                    self.nav,
                    LB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(title.as_ptr() as isize),
                );
            }
            let _ = SendMessageW(self.nav, LB_SETCURSEL, WPARAM(self.page), LPARAM(0));
            let _ = InvalidateRect(self.nav, None, false);
        }
        if language_changed || force {
            set_text(self.heading, titles[self.page]);
        }
        let save_failed = SAVE_FAILED.load(Ordering::Relaxed);
        if language_changed || save_failed != self.save_failed {
            self.save_failed = save_failed;
            set_text(
                self.footer,
                match (language == LanguageId::SimplifiedChinese, save_failed) {
                    (true, false) => "更改会立即生效并保存。",
                    (true, true) => "无法保存设置。请检查设置文件的写入权限。",
                    (false, false) => "Changes take effect and save immediately.",
                    (false, true) => {
                        "Could not save settings. Check write access to the settings file."
                    }
                },
            );
        }
        if !force && self.rows == next {
            return;
        }
        if self.rows.len() != next.len()
            || self
                .rows
                .iter()
                .zip(&next)
                .any(|(old, new)| !same_controls(old, new))
        {
            let focus = GetFocus();
            let focused_row = self
                .controls
                .iter()
                .position(|controls| controls.contains(&focus));
            for control in self.controls.drain(..).flatten() {
                let _ = DestroyWindow(control);
            }
            self.controls = next
                .iter()
                .enumerate()
                .map(|(index, row)| self.create_row(index, row))
                .collect();
            self.rows.clear();
            if let Some((index, control)) = focused_row.and_then(|index| {
                self.controls
                    .get(index)
                    .and_then(|controls| controls.last())
                    .map(|control| (index, *control))
            }) {
                if index >= self.scroll {
                    let _ = SetFocus(control);
                }
            }
        }
        for (index, (row, controls)) in next.iter().zip(&self.controls).enumerate() {
            if force || self.rows.get(index) != Some(row) {
                update_row(row, controls);
            }
        }
        self.rows = next;
        self.layout();
    }

    unsafe fn create_row(&self, index: usize, row: &Row) -> Vec<HWND> {
        let base = FIRST_FIELD + index as u16 * FIELDS_PER_ROW;
        let controls = match row {
            Row::Check { .. } => vec![child(
                self.hwnd,
                w!("BUTTON"),
                base,
                WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            )],
            Row::Choice { .. } => vec![
                child(self.hwnd, w!("STATIC"), base, STATIC_TEXT),
                child(
                    self.hwnd,
                    w!("COMBOBOX"),
                    base + 1,
                    WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32 | CBS_HASSTRINGS as u32,
                ),
            ],
            Row::Buttons(items) => items
                .iter()
                .enumerate()
                .map(|(col, _)| child(self.hwnd, w!("BUTTON"), base + col as u16, WS_TABSTOP.0))
                .collect(),
            Row::Note(_) => vec![child(self.hwnd, w!("STATIC"), base, STATIC_TEXT)],
        };
        for &control in &controls {
            set_font(control, self.font);
        }
        controls
    }

    unsafe fn visible_rows(&self) -> usize {
        let mut rect = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rect);
        ((rect.bottom - self.px(FIRST_ROW + FOOTER)) / self.px(ROW_HEIGHT)).max(1) as usize
    }

    unsafe fn layout(&mut self) {
        let mut rect = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rect);
        let dpi = self.dpi;
        let px = |value| scale(value, dpi);
        let left = px(SIDEBAR + PAD);
        let right = rect.right - px(PAD);
        resize(self.nav, px(12), px(24), px(SIDEBAR - 24), px(39 * 5), true);
        resize(self.heading, left, px(20), right - left, px(40), true);
        resize(
            self.footer,
            left,
            rect.bottom - px(41),
            right - left,
            px(26),
            true,
        );
        let visible = self.visible_rows();
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(visible));
        let scroll_info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: self.rows.len().saturating_sub(1) as i32,
            nPage: visible as u32,
            nPos: self.scroll as i32,
            ..Default::default()
        };
        SetScrollInfo(self.hwnd, SB_VERT, &scroll_info, true);

        for (index, (row, controls)) in self.rows.iter().zip(&self.controls).enumerate() {
            let show = (self.scroll..self.scroll + visible).contains(&index);
            let y = px(FIRST_ROW) + (index as i32 - self.scroll as i32) * px(ROW_HEIGHT);
            let width = right - left;
            match row {
                Row::Check { .. } | Row::Note(_) => {
                    resize(controls[0], left, y, width, px(28), show)
                }
                Row::Choice { .. } => {
                    let split = width * 46 / 100;
                    resize(controls[0], left, y, split - px(12), px(28), show);
                    resize(controls[1], left + split, y, width - split, px(230), show);
                }
                Row::Buttons(items) => {
                    let count = items.len() as i32;
                    let gap = px(8);
                    let button_width = (width - gap * (count - 1)) / count;
                    for (column, &control) in controls.iter().enumerate() {
                        resize(
                            control,
                            left + column as i32 * (button_width + gap),
                            y,
                            button_width.min(if count == 1 { px(310) } else { button_width }),
                            px(28),
                            show,
                        );
                    }
                }
            }
        }
    }

    unsafe fn scroll(&mut self, offset: i32, absolute: bool) {
        let position = if absolute {
            offset
        } else {
            self.scroll as i32 + offset
        };
        let limit = self.rows.len().saturating_sub(self.visible_rows());
        let position = (position.max(0) as usize).min(limit);
        if position != self.scroll {
            self.scroll = position;
            self.layout();
        }
    }

    unsafe fn change_page(&mut self, page: usize) {
        if page < Page::ALL.len() && self.page != page {
            self.page = page;
            self.scroll = 0;
            self.sync(true);
        }
    }

    unsafe fn queue_action(&mut self, id: u16, code: u16, control: HWND) {
        let Some(index) = id.checked_sub(FIRST_FIELD) else {
            return;
        };
        let row = usize::from(index / FIELDS_PER_ROW);
        let column = usize::from(index % FIELDS_PER_ROW);
        if self
            .controls
            .get(row)
            .and_then(|controls| controls.get(column))
            != Some(&control)
        {
            return;
        }
        let item = match self.rows.get(row) {
            Some(Row::Check {
                item,
                enabled: true,
                ..
            }) if code == BN_CLICKED as u16 => Some(item),
            Some(Row::Buttons(items)) if code == BN_CLICKED as u16 => items.get(column),
            Some(Row::Choice {
                items,
                enabled: true,
                ..
            }) if column == 1
                && (code == CBN_SELENDOK as u16
                    || (code == CBN_SELCHANGE as u16 && !is_dropped(control))) =>
            {
                let selected = SendMessageW(control, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                items.get(selected as usize)
            }
            _ => return,
        };
        // Run after the control has completed its selection, including opening modal pickers.
        let action = item.and_then(|item| item.action.clone());
        if self.pending.back() != Some(&action) {
            self.pending.push_back(action);
            let _ = PostMessageW(self.hwnd, APPLY_SELECTION, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font);
            let _ = DeleteObject(self.title_font);
        }
    }
}

fn same_controls(old: &Row, new: &Row) -> bool {
    match (old, new) {
        (Row::Buttons(old), Row::Buttons(new)) => old.len() == new.len(),
        _ => std::mem::discriminant(old) == std::mem::discriminant(new),
    }
}

fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

unsafe fn ui_font(size: i32, weight: u32) -> HFONT {
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
        w!("Segoe UI"),
    )
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

unsafe fn set_text(hwnd: HWND, text: &str) {
    let text = native_interop::wide_str(text);
    let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
}

unsafe fn set_font(hwnd: HWND, font: HFONT) {
    let _ = SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
}

unsafe fn resize(hwnd: HWND, x: i32, y: i32, width: i32, height: i32, show: bool) {
    let flag = if show { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
    let _ = SetWindowPos(
        hwnd,
        HWND::default(),
        x,
        y,
        width.max(0),
        height,
        flag | SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

unsafe fn is_dropped(control: HWND) -> bool {
    SendMessageW(control, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)).0 != 0
}

unsafe fn update_row(row: &Row, controls: &[HWND]) {
    match row {
        Row::Check {
            item,
            checked,
            enabled,
        } => {
            set_text(controls[0], &item.label);
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
        } => {
            set_text(controls[0], label);
            let control = controls[1];
            let _ = SendMessageW(control, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
            for item in items {
                let text = native_interop::wide_str(&item.label);
                let _ = SendMessageW(
                    control,
                    CB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(text.as_ptr() as isize),
                );
            }
            let _ = SendMessageW(control, CB_SETCURSEL, WPARAM(*selected), LPARAM(0));
            let _ = EnableWindow(control, *enabled);
        }
        Row::Buttons(items) => {
            for (item, &control) in items.iter().zip(controls) {
                set_text(control, &item.label);
            }
        }
        Row::Note(text) => set_text(controls[0], text),
    }
}

fn with_panel<T>(f: impl FnOnce(&mut Panel) -> T) -> Option<T> {
    PANEL.with(|state| {
        state
            .try_borrow_mut()
            .ok()
            .and_then(|mut state| state.as_mut().map(f))
    })
}

fn current_window() -> HWND {
    with_panel(|panel| panel.hwnd).unwrap_or_default()
}

pub(super) fn note_save_result(saved: bool) {
    SAVE_FAILED.store(!saved, Ordering::Relaxed);
}

pub(super) fn dialog_owner(widget: HWND) -> HWND {
    let settings = current_window();
    if settings.is_invalid() {
        widget
    } else {
        settings
    }
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
        hbrBackground: GetSysColorBrush(COLOR_WINDOW),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hIcon: LoadIconW(instance, PCWSTR(1usize as *const u16)).unwrap_or_default(),
        ..Default::default()
    });
    let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN | WS_VSCROLL;
    let dpi = GetDpiForWindow(widget).max(96);
    let mut rect = RECT {
        right: scale(WIDTH, dpi),
        bottom: scale(HEIGHT, dpi),
        ..Default::default()
    };
    let _ = AdjustWindowRectExForDpi(&mut rect, style, false, WINDOW_EX_STYLE(0), dpi);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let monitor = MonitorFromWindow(widget, MONITOR_DEFAULTTONEAREST);
    let (x, y, width, height) = if GetMonitorInfoW(monitor, &mut info).as_bool() {
        let work = info.rcWork;
        let width = (rect.right - rect.left).min(work.right - work.left);
        let height = (rect.bottom - rect.top).min(work.bottom - work.top);
        (
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
            width,
            height,
        )
    } else {
        (
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
        )
    };
    let Ok(hwnd) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        CLASS,
        w!("Codex Usage"),
        style,
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
            } else {
                with_panel(|panel| panel.queue_action(id, code, control));
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
        WM_TIMER => {
            with_panel(|panel| panel.sync(false));
        }
        WM_SIZE => {
            with_panel(|panel| panel.layout());
        }
        WM_VSCROLL => {
            with_panel(|panel| {
                let page = panel.visible_rows() as i32;
                let (offset, absolute) = match SCROLLBAR_COMMAND(wparam.0 as u16 as i32) {
                    SB_LINEUP => (-1, false),
                    SB_LINEDOWN => (1, false),
                    SB_PAGEUP => (-page, false),
                    SB_PAGEDOWN => (page, false),
                    SB_TOP => (0, true),
                    SB_BOTTOM => (i32::MAX, true),
                    SB_THUMBTRACK | SB_THUMBPOSITION => ((wparam.0 >> 16) as i32, true),
                    _ => return,
                };
                panel.scroll(offset, absolute);
            });
        }
        WM_MOUSEWHEEL => {
            let wheel = (wparam.0 >> 16) as i16;
            with_panel(|panel| panel.scroll(-i32::from(wheel) / 40, false));
        }
        WM_DPICHANGED => {
            let dpi = wparam.0 as u16 as u32;
            let suggested = *(lparam.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            with_panel(|panel| {
                panel.set_dpi(dpi);
                panel.layout();
            });
        }
        WM_DRAWITEM if wparam.0 == NAV as usize => {
            let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
            with_panel(|panel| draw_navigation(panel, item));
            return LRESULT(1);
        }
        WM_CTLCOLORSTATIC => {
            let dc = HDC(wparam.0 as *mut _);
            SetBkMode(dc, TRANSPARENT);
            return LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize);
        }
        WM_ERASEBKGND => {
            let dc = HDC(wparam.0 as *mut _);
            let mut bounds = RECT::default();
            let _ = GetClientRect(hwnd, &mut bounds);
            FillRect(dc, &bounds, GetSysColorBrush(COLOR_WINDOW));
            if let Some(width) = with_panel(|panel| panel.px(SIDEBAR)) {
                bounds.right = width;
                FillRect(dc, &bounds, GetSysColorBrush(COLOR_BTNFACE));
            }
            return LRESULT(1);
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, REFRESH_TIMER);
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

unsafe fn draw_navigation(panel: &Panel, item: &DRAWITEMSTRUCT) {
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
    let (background, foreground) = if selected {
        (COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT)
    } else {
        (COLOR_BTNFACE, COLOR_BTNTEXT)
    };
    FillRect(item.hDC, &item.rcItem, GetSysColorBrush(background));
    let Some(label) = page_titles(panel.language.unwrap_or(LanguageId::English))
        .get(item.itemID as usize)
        .copied()
    else {
        return;
    };
    SetTextColor(item.hDC, COLORREF(GetSysColor(foreground)));
    SetBkMode(item.hDC, TRANSPARENT);
    let previous_font = SelectObject(item.hDC, panel.font);
    let mut text = native_interop::wide_str(label);
    let mut rect = item.rcItem;
    rect.left += panel.px(10);
    rect.right -= panel.px(8);
    DrawTextW(
        item.hDC,
        &mut text,
        &mut rect,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
    );
    let _ = SelectObject(item.hDC, previous_font);
    if item.itemState.0 & ODS_FOCUS.0 != 0 {
        let _ = DrawFocusRect(item.hDC, &item.rcItem);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poller::PollError;

    #[test]
    fn service_labels_only_show_confirmed_authentication_errors() {
        let label = |monitored, error| {
            service_label("Codex", monitored, error, LanguageId::SimplifiedChinese)
        };

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
}
