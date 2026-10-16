//! Compact reset-time popup, centered above the taskbar like its app tooltips.
use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_HOVER, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub background: u32,
    pub text: u32,
    pub font_px: i32,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            background: 0x2b2b2b,
            text: 0xffffff,
            font_px: 12,
        }
    }
}

#[derive(Default)]
struct State {
    window: isize,
    owner: isize,
    font: isize,
    regions: Vec<(RECT, String)>,
    style: Style,
    hover: Option<usize>,
    point: Option<POINT>,
    text: String,
}
static STATE: Mutex<State> = Mutex::new(State {
    window: 0,
    owner: 0,
    font: 0,
    regions: Vec::new(),
    style: Style {
        background: 0x2b2b2b,
        text: 0xffffff,
        font_px: 12,
    },
    hover: None,
    point: None,
    text: String::new(),
});

fn same_regions(a: &[(RECT, String)], b: &[(RECT, String)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((a, at), (b, bt))| {
            (a.left, a.top, a.right, a.bottom, at) == (b.left, b.top, b.right, b.bottom, bt)
        })
}

pub fn sync(owner: HWND, regions: Vec<(RECT, String)>, style: Style) {
    let owner_changed = STATE.lock().unwrap_or_else(|e| e.into_inner()).owner != owner.0 as isize;
    if owner_changed {
        clear();
    }
    let (old_font, refresh, window) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if !owner_changed && state.style == style && same_regions(&state.regions, &regions) {
            return;
        }
        let old_font = if state.font == 0 || state.style != style {
            let old = state.font;
            state.font = unsafe {
                CreateFontW(
                    -style.font_px,
                    0,
                    0,
                    0,
                    FW_NORMAL.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET.0 as u32,
                    OUT_TT_PRECIS.0 as u32,
                    CLIP_DEFAULT_PRECIS.0 as u32,
                    CLEARTYPE_QUALITY.0 as u32,
                    (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                    w!("Segoe UI"),
                )
                .0 as isize
            };
            old
        } else {
            0
        };
        state.owner = owner.0 as isize;
        state.style = style;
        state.regions = regions;
        if state.hover.is_some() {
            state.hover = state.point.and_then(|point| {
                state.regions.iter().position(|(r, _)| {
                    point.x >= r.left && point.x < r.right && point.y >= r.top && point.y < r.bottom
                })
            });
        }
        (old_font, state.hover.is_some(), state.window)
    };
    unsafe {
        if old_font != 0 {
            let _ = DeleteObject(HFONT(old_font as *mut _));
        }
    }
    if window != 0 {
        if refresh && unsafe { IsWindowVisible(HWND(window as *mut _)).as_bool() } {
            show();
        } else if !refresh {
            hide();
        }
    }
}

pub fn mouse_move(owner: HWND, point: POINT) {
    let changed = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.owner != owner.0 as isize {
            return;
        }
        let next = state.regions.iter().position(|(r, _)| {
            point.x >= r.left && point.x < r.right && point.y >= r.top && point.y < r.bottom
        });
        let changed = state.hover != next;
        state.hover = next;
        state.point = Some(point);
        changed
    };
    if changed {
        hide_window();
        unsafe {
            let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_HOVER | TME_LEAVE,
                hwndTrack: owner,
                dwHoverTime: 500,
            });
        }
    }
}

pub fn hide() {
    {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover = None;
        state.point = None;
    }
    hide_window();
}

fn hide_window() {
    let window = STATE.lock().unwrap_or_else(|e| e.into_inner()).window;
    if window != 0 {
        unsafe {
            let _ = ShowWindow(HWND(window as *mut _), SW_HIDE);
        }
    }
}

fn scale(px: i32, dpi: u32) -> i32 {
    (px as i64 * dpi.max(96) as i64 / 96) as i32
}

fn popup_rect(owner: RECT, row: RECT, size: SIZE, work: RECT, gap: i32) -> RECT {
    let center = owner.left + row.left + (row.right - row.left) / 2;
    let x = (center - size.cx / 2).clamp(work.left, (work.right - size.cx).max(work.left));
    let preferred_y = owner.top - gap - size.cy;
    let y = if preferred_y >= work.top {
        preferred_y
    } else {
        owner.bottom + gap
    }
    .clamp(work.top, (work.bottom - size.cy).max(work.top));
    RECT {
        left: x,
        top: y,
        right: x + size.cx,
        bottom: y + size.cy,
    }
}

unsafe fn ensure_window(owner: HWND) -> Option<HWND> {
    let existing = STATE.lock().unwrap_or_else(|e| e.into_inner()).window;
    if existing != 0 {
        return Some(HWND(existing as *mut _));
    }
    let instance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
    let class = WNDCLASSW {
        style: CS_DROPSHADOW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        lpszClassName: w!("CodexResetTooltip"),
        ..Default::default()
    };
    RegisterClassW(&class);
    let window = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("CodexResetTooltip"),
        w!(""),
        WS_POPUP | WS_THICKFRAME,
        0,
        0,
        0,
        0,
        owner,
        None,
        instance,
        None,
    )
    .ok()?;
    let preference = DWMWCP_ROUNDSMALL;
    let _ = DwmSetWindowAttribute(
        window,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &preference as *const _ as *const _,
        std::mem::size_of_val(&preference) as u32,
    );
    STATE.lock().unwrap_or_else(|e| e.into_inner()).window = window.0 as isize;
    Some(window)
}

pub fn show() {
    let (owner, row, text, font, style) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(index) = state.hover else {
            return;
        };
        let Some((row, text)) = state.regions.get(index).cloned() else {
            return;
        };
        state.text = text.clone();
        (
            HWND(state.owner as *mut _),
            row,
            text,
            HFONT(state.font as *mut _),
            state.style,
        )
    };
    unsafe {
        if !IsWindowVisible(owner).as_bool() {
            return;
        }
        let Some(window) = ensure_window(owner) else {
            return;
        };
        let mut bounds = RECT::default();
        if GetWindowRect(owner, &mut bounds).is_err() {
            return;
        }
        // The widget can be shorter than its taskbar. Anchor the gap to the
        // taskbar edge rather than to the inset widget's own top or bottom.
        let root = GetAncestor(owner, GA_ROOT);
        let mut class_name = [0u16; 64];
        let length = GetClassNameW(root, &mut class_name);
        let class_name = String::from_utf16_lossy(&class_name[..length as usize]);
        if matches!(
            class_name.as_str(),
            "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            let mut taskbar = RECT::default();
            if GetWindowRect(root, &mut taskbar).is_ok() {
                bounds.top = taskbar.top;
                bounds.bottom = taskbar.bottom;
            }
        }
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(owner);
        let padding_x = scale(10, dpi);
        let padding_y = scale(7, dpi);
        let dc = GetDC(owner);
        let old_font = SelectObject(dc, font);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE::default();
        let measured = GetTextExtentPoint32W(dc, &wide, &mut size).as_bool();
        SelectObject(dc, old_font);
        ReleaseDC(owner, dc);
        if !measured {
            return;
        }
        size.cx += padding_x * 2;
        size.cy += padding_y * 2;
        let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let rect = popup_rect(bounds, row, size, info.rcWork, scale(12, dpi));
        let title: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let _ = SetWindowTextW(window, PCWSTR(title.as_ptr()));
        let dark = i32::from(style.background == 0x2b2b2b);
        let border = if dark != 0 { 0x3a3a3au32 } else { 0xd0d0d0u32 };
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            4,
        );
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_BORDER_COLOR,
            &border as *const _ as *const _,
            4,
        );
        let _ = SetWindowPos(
            window,
            HWND_TOPMOST,
            rect.left,
            rect.top,
            size.cx,
            size.cy,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = InvalidateRect(window, None, false);
    }
}

unsafe fn paint(window: HWND, dc: HDC) {
    let (style, text, font, owner) = {
        let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        (
            state.style,
            state.text.clone(),
            HFONT(state.font as *mut _),
            HWND(state.owner as *mut _),
        )
    };
    let mut rect = RECT::default();
    let _ = GetClientRect(window, &mut rect);
    let brush = CreateSolidBrush(COLORREF(style.background));
    FillRect(dc, &rect, brush);
    let _ = DeleteObject(brush);
    let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(owner);
    rect.left += scale(10, dpi);
    rect.right -= scale(10, dpi);
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(style.text));
    let old_font = SelectObject(dc, font);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(
        dc,
        &mut wide,
        &mut rect,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
    );
    SelectObject(dc, old_font);
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCALCSIZE => LRESULT(0),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(window, &mut ps);
            paint(window, dc);
            let _ = EndPaint(window, &ps);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            paint(window, HDC(wparam.0 as *mut _));
            LRESULT(0)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

pub fn clear() {
    let (window, font) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let handles = (state.window, state.font);
        *state = State::default();
        handles
    };
    unsafe {
        if window != 0 {
            let _ = DestroyWindow(HWND(window as *mut _));
        }
        if font != 0 {
            let _ = DeleteObject(HFONT(font as *mut _));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_centers_over_row_with_taskbar_gap_and_clamps_to_monitor() {
        let owner = RECT {
            left: 166,
            top: 1392,
            right: 466,
            bottom: 1440,
        };
        let row = RECT {
            left: 30,
            top: 0,
            right: 280,
            bottom: 48,
        };
        let size = SIZE { cx: 170, cy: 30 };
        let work = RECT {
            left: 0,
            top: 0,
            right: 3440,
            bottom: 1392,
        };
        let r = popup_rect(owner, row, size, work, 12);
        assert_eq!((r.left, r.top, r.right, r.bottom), (236, 1350, 406, 1380));
        let edge = popup_rect(
            RECT {
                left: -1440,
                right: -1140,
                ..owner
            },
            row,
            SIZE { cx: 500, cy: 30 },
            RECT {
                left: -1440,
                right: 0,
                ..work
            },
            12,
        );
        assert_eq!(edge.left, -1440);
    }
    #[test]
    fn top_edge_taskbar_uses_below_fallback_and_dpi_scales_spacing() {
        let r = popup_rect(
            RECT {
                left: 0,
                top: 0,
                right: 300,
                bottom: 48,
            },
            RECT {
                left: 30,
                top: 0,
                right: 280,
                bottom: 48,
            },
            SIZE { cx: 170, cy: 30 },
            RECT {
                left: 0,
                top: 48,
                right: 1920,
                bottom: 1080,
            },
            12,
        );
        assert_eq!(r.top, 60);
        assert_eq!(scale(12, 144), 18);
    }
    #[test]
    fn native_popup_preserves_timestamp_without_activation_or_resize_frame() {
        unsafe {
            let owner = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("quota-tip-test"),
                WS_POPUP,
                100,
                500,
                300,
                48,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let _ = ShowWindow(owner, SW_SHOWNOACTIVATE);
            let row = RECT {
                left: 145,
                top: 17,
                right: 193,
                bottom: 31,
            };
            sync(
                owner,
                vec![(row, "Thu 10/15/26 8:52AM".into())],
                Style::default(),
            );
            mouse_move(owner, POINT { x: 60, y: 20 });
            show();
            assert_eq!(
                STATE.lock().unwrap().window,
                0,
                "bar/percentage must not open a popup"
            );
            mouse_move(owner, POINT { x: 150, y: 20 });
            let foreground = GetForegroundWindow();
            show();
            let window = HWND(STATE.lock().unwrap().window as *mut _);
            assert!(IsWindowVisible(window).as_bool());
            let mut title = [0u16; 128];
            let length = GetWindowTextW(window, &mut title);
            assert_eq!(
                String::from_utf16(&title[..length as usize]).unwrap(),
                "Thu 10/15/26 8:52AM"
            );
            let mut bounds = RECT::default();
            GetWindowRect(window, &mut bounds).unwrap();
            assert_eq!(bounds.bottom, 488);
            assert!(
                ((bounds.left + bounds.right) / 2 - (100 + (row.left + row.right) / 2)).abs() <= 1
            );
            assert_eq!(GetForegroundWindow(), foreground);
            assert_eq!(
                SendMessageW(window, WM_NCHITTEST, WPARAM(0), LPARAM(0)).0,
                HTCLIENT as isize
            );
            assert_eq!(
                SendMessageW(window, WM_MOUSEACTIVATE, WPARAM(0), LPARAM(0)).0,
                MA_NOACTIVATE as isize
            );
            let mut corners = DWM_WINDOW_CORNER_PREFERENCE::default();
            if DwmGetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &mut corners as *mut _ as *mut _,
                std::mem::size_of_val(&corners) as u32,
            )
            .is_ok()
            {
                assert_eq!(corners, DWMWCP_ROUNDSMALL);
            }
            let screen = GetDC(owner);
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(
                screen,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
            );
            let previous = SelectObject(dc, bitmap);
            SendMessageW(
                window,
                WM_PRINTCLIENT,
                WPARAM(dc.0 as usize),
                LPARAM(PRF_CLIENT as isize),
            );
            assert_eq!(GetPixel(dc, 5, 5).0, 0x2b2b2b);
            let light = Style {
                background: 0xf9f9f9,
                text: 0x1a1a1a,
                ..Style::default()
            };
            sync(owner, vec![(row, "Sun 10/18/26 12:52AM".into())], light);
            let length = GetWindowTextW(window, &mut title);
            assert_eq!(
                String::from_utf16(&title[..length as usize]).unwrap(),
                "Sun 10/18/26 12:52AM"
            );
            SendMessageW(
                window,
                WM_PRINTCLIENT,
                WPARAM(dc.0 as usize),
                LPARAM(PRF_CLIENT as isize),
            );
            assert_eq!(GetPixel(dc, 5, 5).0, 0xf9f9f9);
            SelectObject(dc, previous);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
            ReleaseDC(owner, screen);
            mouse_move(owner, POINT { x: 0, y: 20 });
            assert!(!IsWindowVisible(window).as_bool());
            mouse_move(owner, POINT { x: 150, y: 20 });
            show();
            assert!(IsWindowVisible(window).as_bool());
            sync(
                owner,
                vec![(RECT { left: 170, ..row }, "Sun 10/18/26 12:52AM".into())],
                light,
            );
            assert!(
                !IsWindowVisible(window).as_bool(),
                "changed countdown bounds must recheck the hover point"
            );
            sync(owner, Vec::new(), light);
            assert!(!IsWindowVisible(window).as_bool());
            hide();
            assert!(!IsWindowVisible(window).as_bool());
            clear();
            let _ = DestroyWindow(owner);
        }
    }
}
