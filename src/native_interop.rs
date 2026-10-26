use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, FILETIME, HWND, LPARAM, RECT, SYSTEMTIME};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETTASKBARPOS, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::*;

// Window style constants
pub const WS_POPUP_STYLE: u32 = 0x80000000;
pub const WS_CHILD_STYLE: u32 = 0x40000000;
pub const WS_CLIPSIBLINGS_STYLE: u32 = 0x04000000;

// Win event constants
pub const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
pub const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;

// Timer IDs
pub const TIMER_POLL: usize = 1;
pub const TIMER_COUNTDOWN: usize = 2;
pub const TIMER_UPDATE_CHECK: usize = 4;
pub const TIMER_FRESHNESS: usize = 5;
pub const TIMER_TASKBAR_RETRY: usize = 6;

// Custom messages
pub const WM_APP: u32 = 0x8000;
pub const WM_APP_USAGE_UPDATED: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 3;

const WINDOWS_TO_UNIX_EPOCH_SECONDS: u64 = 11_644_473_600;

pub fn system_time_to_local(value: SystemTime) -> Option<SYSTEMTIME> {
    let unix_seconds = value.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let file_ticks = unix_seconds
        .checked_add(WINDOWS_TO_UNIX_EPOCH_SECONDS)?
        .checked_mul(10_000_000)?;
    let file_time = FILETIME {
        dwLowDateTime: file_ticks as u32,
        dwHighDateTime: (file_ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&file_time, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(local)
}

#[derive(Clone, Copy, Debug)]
pub struct TaskbarWindow {
    pub hwnd: HWND,
    pub rect: RECT,
    pub is_primary: bool,
}

/// Windows display device name (for example \\.\DISPLAY2) for a taskbar.
/// Unlike enumeration order, this survives taskbar recreation and rearrangement.
pub fn taskbar_monitor_device(taskbar: &TaskbarWindow) -> Option<String> {
    let point = windows::Win32::Foundation::POINT {
        x: taskbar.rect.left + (taskbar.rect.right - taskbar.rect.left) / 2,
        y: taskbar.rect.top + (taskbar.rect.bottom - taskbar.rect.top) / 2,
    };
    unsafe {
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
            return None;
        }
        let len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        Some(String::from_utf16_lossy(&info.szDevice[..len]))
    }
}

pub fn find_taskbars() -> Vec<TaskbarWindow> {
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let taskbars = &mut *(lparam.0 as *mut Vec<TaskbarWindow>);
        let mut class_name = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class_name) };
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_name[..len as usize]);
            if class_name == "Shell_TrayWnd" || class_name == "Shell_SecondaryTrayWnd" {
                if let Some(rect) = get_taskbar_rect(hwnd).or_else(|| get_window_rect_safe(hwnd)) {
                    taskbars.push(TaskbarWindow {
                        hwnd,
                        rect,
                        is_primary: class_name == "Shell_TrayWnd",
                    });
                }
            }
        }
        BOOL(1)
    }

    let mut taskbars: Vec<TaskbarWindow> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut taskbars as *mut _ as isize));
    }
    taskbars.sort_by_key(|taskbar| {
        (
            taskbar.rect.top,
            taskbar.rect.left,
            taskbar.rect.bottom,
            taskbar.rect.right,
        )
    });
    taskbars
}

/// Find a child window by class name
pub fn find_child_window(parent: HWND, class_name: &str) -> Option<HWND> {
    unsafe {
        let class = wide_str(class_name);
        match FindWindowExW(
            parent,
            HWND::default(),
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::null(),
        ) {
            Ok(h) if h != HWND::default() => Some(h),
            _ => None,
        }
    }
}

/// Get taskbar position via SHAppBarMessage
pub fn get_taskbar_rect(taskbar_hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut class_name = [0u16; 64];
        let len = GetClassNameW(taskbar_hwnd, &mut class_name);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_name[..len as usize]);
            if class_name == "Shell_SecondaryTrayWnd" {
                return get_window_rect_safe(taskbar_hwnd);
            }
        }

        let mut abd = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar_hwnd,
            ..Default::default()
        };
        let result = SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd);
        if result == 0 {
            return None;
        }
        Some(abd.rc)
    }
}

/// Get the bounding rectangle of a window
pub fn get_window_rect_safe(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            Some(rect)
        } else {
            None
        }
    }
}

/// Embed our window as a child of the taskbar
pub fn embed_in_taskbar(hwnd: HWND, taskbar_hwnd: HWND) {
    unsafe {
        // Preserve existing extended style, add tool window + no activate
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        // Reset alpha mode used by the startup popup fallback before switching
        // back to UpdateLayeredWindow rendering inside the taskbar.
        if ex_style & WS_EX_LAYERED.0 as i32 != 0 {
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style & !(WS_EX_LAYERED.0 as i32));
        }
        let _ = SetWindowLongW(
            hwnd,
            GWL_EXSTYLE,
            ex_style | WS_EX_TOOLWINDOW.0 as i32 | WS_EX_NOACTIVATE.0 as i32,
        );

        // Change from popup to child
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let new_style = (style & !WS_POPUP_STYLE) | WS_CHILD_STYLE | WS_CLIPSIBLINGS_STYLE;
        let _ = SetWindowLongW(hwnd, GWL_STYLE, new_style as i32);

        let _ = SetParent(hwnd, taskbar_hwnd);
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, POINT, SIZE};
    use windows::Win32::Graphics::Gdi::*;

    #[test]
    fn startup_popup_and_reattached_child_can_use_layered_rendering() {
        unsafe {
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("attachment-test"),
                WS_POPUP,
                0,
                0,
                20,
                20,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let second_parent = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("second-attachment-test"),
                WS_POPUP,
                20,
                0,
                20,
                20,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let child = CreateWindowExW(
                WS_EX_LAYERED,
                w!("STATIC"),
                w!("widget-test"),
                WS_POPUP,
                0,
                0,
                2,
                2,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            SetLayeredWindowAttributes(child, COLORREF(0), 255, LWA_ALPHA).unwrap();
            embed_in_taskbar(child, parent);
            // Switching displays reattaches an existing child rather than a startup popup.
            embed_in_taskbar(child, second_parent);
            assert_eq!(GetParent(child).unwrap(), second_parent);
            embed_in_taskbar(child, parent);
            assert_eq!(GetParent(child).unwrap(), parent);
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: 40,
                    biWidth: 2,
                    biHeight: -2,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            std::slice::from_raw_parts_mut(bits as *mut u32, 4).fill(0xffffffff);
            let old = SelectObject(dc, bitmap);
            let point = POINT { x: 0, y: 0 };
            let size = SIZE { cx: 2, cy: 2 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
                ..Default::default()
            };
            let rendered = UpdateLayeredWindow(
                child,
                None,
                Some(&point),
                Some(&size),
                dc,
                Some(&point),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            let _ = SelectObject(dc, old);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
            let _ = DestroyWindow(child);
            let _ = DestroyWindow(second_parent);
            let _ = DestroyWindow(parent);
            assert!(
                rendered.is_ok(),
                "fallback alpha mode must not prevent taskbar rendering: {rendered:?}"
            );
        }
    }
}

/// Move the window
pub fn move_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = MoveWindow(hwnd, x, y, w, h, true);
    }
}

/// Set up a WinEvent hook for tray location changes
pub fn set_tray_event_hook(
    thread_id: u32,
    callback: unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32),
) -> Option<HWINEVENTHOOK> {
    unsafe {
        let hook = SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(callback),
            0,
            thread_id,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_invalid() {
            None
        } else {
            Some(hook)
        }
    }
}

/// Get the thread ID that owns a window
pub fn get_window_thread_id(hwnd: HWND) -> u32 {
    unsafe { GetWindowThreadProcessId(hwnd, None) }
}

/// Unhook a WinEvent hook
pub fn unhook_win_event(hook: HWINEVENTHOOK) {
    unsafe {
        let _ = UnhookWinEvent(hook);
    }
}

/// Convert a Rust string to a null-terminated wide string
pub fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// COLORREF wrapper (RGB packed into u32)
pub fn colorref(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// Color helper
#[derive(Clone, Copy, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    #[allow(dead_code)]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn from_hex(hex: &str) -> Self {
        let hex = hex.trim_start_matches('#');
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        Self { r, g, b }
    }

    pub fn to_colorref(self) -> u32 {
        colorref(self.r, self.g, self.b)
    }
}
