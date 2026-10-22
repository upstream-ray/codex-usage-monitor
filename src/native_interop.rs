use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{Error, Result, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, SetLastError, BOOL, COLORREF, ERROR_SUCCESS, E_FAIL, E_INVALIDARG, FILETIME,
    HWND, LPARAM, POINT, RECT, SYSTEMTIME,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MapWindowPoints, MonitorFromPoint, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST,
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

/// Convert a desktop point using the actual parent client origin. Subtracting
/// a taskbar's outer RECT is incorrect when its client area has a border offset.
pub fn screen_to_client_point(hwnd: HWND, point: POINT) -> Option<POINT> {
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return None;
        }
        let mut points = [point];
        SetLastError(ERROR_SUCCESS);
        let offset = MapWindowPoints(HWND::default(), hwnd, &mut points);
        // A zero offset is also a successful mapping at the desktop origin.
        if offset == 0 && GetLastError() != ERROR_SUCCESS {
            None
        } else {
            Some(points[0])
        }
    }
}

/// Find the notification/clock area on this taskbar, including nested primary
/// trays and secondary clocks. Unknown layouts return None rather than claiming
/// that the full right edge is available to the widget.
pub fn taskbar_notification_rect(taskbar_hwnd: HWND) -> Option<RECT> {
    struct Search {
        taskbar: RECT,
        notification: Option<RECT>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut class_name = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut class_name);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_name[..len as usize]);
            if let Some(rect) = get_window_rect_safe(hwnd)
                .and_then(|rect| notification_candidate_rect(search.taskbar, &class_name, rect))
            {
                search.notification = Some(match search.notification {
                    Some(previous) => RECT {
                        left: previous.left.min(rect.left),
                        top: previous.top.min(rect.top),
                        right: previous.right.max(rect.right),
                        bottom: previous.bottom.max(rect.bottom),
                    },
                    None => rect,
                });
            }
        }
        BOOL(1)
    }

    let mut search = Search {
        taskbar: get_window_rect_safe(taskbar_hwnd)?,
        notification: None,
    };
    unsafe {
        // EnumChildWindows visits descendants as well as direct children.
        let _ = EnumChildWindows(
            taskbar_hwnd,
            Some(enum_proc),
            LPARAM(&mut search as *mut _ as isize),
        );
    }
    search.notification
}

fn notification_candidate_rect(taskbar: RECT, class_name: &str, child: RECT) -> Option<RECT> {
    let rect = RECT {
        left: taskbar.left.max(child.left),
        top: taskbar.top.max(child.top),
        right: taskbar.right.min(child.right),
        bottom: taskbar.bottom.min(child.bottom),
    };
    if rect.left >= rect.right || rect.top >= rect.bottom {
        return None;
    }
    match class_name {
        "TrayNotifyWnd" | "TrayClockWClass" | "ClockButton" => Some(rect),
        // Some Windows 11 secondary taskbars expose a distinct composition
        // window for the clock. Reject the full-taskbar composition canvas and
        // centered task-button hosts; only accept a narrow right-edge region.
        "Windows.UI.Composition.DesktopWindowContentBridge"
            if rect.left > taskbar.left + (taskbar.right - taskbar.left) / 2
                && rect.right == taskbar.right =>
        {
            Some(rect)
        }
        _ => None,
    }
}

fn set_window_style(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, style: i32) -> Result<()> {
    unsafe {
        SetLastError(ERROR_SUCCESS);
        let previous = SetWindowLongW(hwnd, index, style);
        if previous == 0 && GetLastError() != ERROR_SUCCESS {
            Err(Error::from_win32())
        } else {
            Ok(())
        }
    }
}

/// Embed our window as a child of the taskbar. Preserve an existing layered
/// child's backing image across display changes, and leave the old attachment
/// intact if native reparenting or its final positioning fails.
pub fn embed_in_taskbar(hwnd: HWND, taskbar_hwnd: HWND) -> Result<()> {
    embed_in_taskbar_with(hwnd, taskbar_hwnd, |hwnd, was_visible| unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )?;
        if was_visible {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(())
    })
}

fn embed_in_taskbar_with(
    hwnd: HWND,
    taskbar_hwnd: HWND,
    finish: impl FnOnce(HWND, bool) -> Result<()>,
) -> Result<()> {
    unsafe {
        if hwnd == taskbar_hwnd || !IsWindow(hwnd).as_bool() || !IsWindow(taskbar_hwnd).as_bool() {
            return Err(Error::new(
                E_INVALIDARG,
                "The widget or target taskbar is unavailable.",
            ));
        }
        let original_style = GetWindowLongW(hwnd, GWL_STYLE);
        let original_ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let original_parent = GetParent(hwnd).unwrap_or_default();
        let original_rect = get_window_rect_safe(hwnd);
        let was_visible = original_style as u32 & WS_VISIBLE.0 != 0;
        let was_child = original_style as u32 & WS_CHILD_STYLE != 0;
        let reset_alpha_mode = !was_child && original_ex_style as u32 & WS_EX_LAYERED.0 != 0;
        let mut original_color_key = COLORREF(0);
        let mut original_alpha = 0;
        let mut original_alpha_flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
        let restore_alpha = reset_alpha_mode
            && GetLayeredWindowAttributes(
                hwnd,
                Some(&mut original_color_key),
                Some(&mut original_alpha),
                Some(&mut original_alpha_flags),
            )
            .is_ok();

        let result = (|| {
            // The startup fallback uses SetLayeredWindowAttributes. Its mode
            // must be reset before UpdateLayeredWindow; child-to-child display
            // changes must not discard the existing per-pixel backing image.
            if reset_alpha_mode {
                set_window_style(
                    hwnd,
                    GWL_EXSTYLE,
                    original_ex_style & !(WS_EX_LAYERED.0 as i32),
                )?;
            }
            set_window_style(
                hwnd,
                GWL_EXSTYLE,
                original_ex_style | WS_EX_TOOLWINDOW.0 as i32 | WS_EX_NOACTIVATE.0 as i32,
            )?;
            let child_style =
                (original_style as u32 & !WS_POPUP_STYLE) | WS_CHILD_STYLE | WS_CLIPSIBLINGS_STYLE;
            set_window_style(hwnd, GWL_STYLE, child_style as i32)?;

            SetLastError(ERROR_SUCCESS);
            let reparented = SetParent(hwnd, taskbar_hwnd);
            let parent_error = GetLastError();
            // SetParent returns the PREVIOUS parent, which can legitimately be
            // null on first attachment. Verify the resulting parent directly.
            if GetParent(hwnd).ok() != Some(taskbar_hwnd) {
                return Err(if parent_error != ERROR_SUCCESS {
                    Error::from_hresult(windows::core::HRESULT::from_win32(parent_error.0))
                } else {
                    reparented.err().unwrap_or_else(|| {
                        Error::new(E_FAIL, "The widget did not attach to the selected taskbar.")
                    })
                });
            }
            finish(hwnd, was_visible)
        })();

        if result.is_err() {
            let _ = SetParent(hwnd, original_parent);
            let _ = set_window_style(hwnd, GWL_STYLE, original_style);
            let _ = set_window_style(hwnd, GWL_EXSTYLE, original_ex_style);
            if restore_alpha {
                let _ = SetLayeredWindowAttributes(
                    hwnd,
                    original_color_key,
                    original_alpha,
                    original_alpha_flags,
                );
            }
            if let Some(rect) = original_rect {
                let point = POINT {
                    x: rect.left,
                    y: rect.top,
                };
                let point = if was_child {
                    screen_to_client_point(original_parent, point)
                } else {
                    Some(point)
                };
                if let Some(point) = point {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND::default(),
                        point.x,
                        point.y,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                }
            }
            let _ = ShowWindow(
                hwnd,
                if was_visible {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_HIDE
                },
            );
        }
        result
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    use std::sync::Once;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, LRESULT, POINT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    struct TestWindow(HWND);

    unsafe extern "system" fn test_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, message, wparam, lparam)
    }

    impl TestWindow {
        fn new(class_name: &str, parent: Option<HWND>, rect: RECT, visible: bool) -> Self {
            static REGISTER_CLASSES: Once = Once::new();
            REGISTER_CLASSES.call_once(|| unsafe {
                let instance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
                for name in ["TrayNotifyWnd", "TrayClockWClass", "ClockButton"] {
                    let name = wide_str(name);
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(test_window_proc),
                        hInstance: instance,
                        lpszClassName: PCWSTR(name.as_ptr()),
                        ..Default::default()
                    };
                    assert_ne!(RegisterClassW(&class), 0);
                }
            });
            unsafe {
                let class_name = wide_str(class_name);
                let style = if parent.is_some() { WS_CHILD } else { WS_POPUP }
                    | if visible { WS_VISIBLE } else { WINDOW_STYLE(0) };
                Self(
                    CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        PCWSTR(class_name.as_ptr()),
                        w!("taskbar-boundary-test"),
                        style,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        parent.unwrap_or_default(),
                        None,
                        HINSTANCE(GetModuleHandleW(None).unwrap().0),
                        None,
                    )
                    .unwrap(),
                )
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    fn rect(left: i32, top: i32, width: i32, height: i32) -> RECT {
        RECT {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    #[test]
    fn discovers_nested_primary_tray_without_hidden_or_outside_children() {
        let parent = TestWindow::new("STATIC", None, rect(50, 100, 1_000, 50), true);
        let nested = TestWindow::new("STATIC", Some(parent.0), rect(800, 0, 200, 50), true);
        let tray = TestWindow::new("TrayNotifyWnd", Some(nested.0), rect(20, 0, 180, 50), true);
        let _clock = TestWindow::new("TrayClockWClass", Some(tray.0), rect(100, 0, 80, 50), true);
        let _hidden = TestWindow::new("ClockButton", Some(parent.0), rect(0, 0, 90, 50), false);
        let _outside = TestWindow::new("ClockButton", Some(parent.0), rect(1_050, 0, 90, 50), true);

        assert_eq!(
            taskbar_notification_rect(parent.0),
            get_window_rect_safe(tray.0)
        );
    }

    #[test]
    fn secondary_taskbar_clock_is_detected_at_negative_desktop_origin() {
        let parent = TestWindow::new("STATIC", None, rect(-1_600, 400, 1_400, 40), true);
        let clock = TestWindow::new("ClockButton", Some(parent.0), rect(1_280, 0, 120, 40), true);
        let unrelated = TestWindow::new("STATIC", None, rect(0, 400, 1_000, 40), true);
        let _other_clock = TestWindow::new(
            "ClockButton",
            Some(unrelated.0),
            rect(800, 0, 200, 40),
            true,
        );

        assert_eq!(
            taskbar_notification_rect(parent.0),
            get_window_rect_safe(clock.0)
        );
        assert_eq!(taskbar_notification_rect(parent.0).unwrap().left, -320);
    }

    #[test]
    fn composition_detection_rejects_the_full_taskbar_and_centered_app_canvas() {
        let taskbar = rect(-1_200, 900, 1_200, 48);
        let class = "Windows.UI.Composition.DesktopWindowContentBridge";
        assert!(notification_candidate_rect(taskbar, class, taskbar).is_none());
        assert!(notification_candidate_rect(taskbar, class, rect(-800, 900, 400, 48)).is_none());
        assert!(
            notification_candidate_rect(taskbar, "ClockButton", rect(20, 900, 120, 48)).is_none()
        );
        assert_eq!(
            notification_candidate_rect(taskbar, class, rect(-140, 900, 140, 48)),
            Some(rect(-140, 900, 140, 48))
        );
    }

    #[test]
    fn screen_mapping_uses_client_origin_and_handles_a_zero_offset() {
        let parent = TestWindow::new("STATIC", None, rect(-900, -100, 300, 180), true);
        unsafe {
            let style = GetWindowLongW(parent.0, GWL_STYLE) | WS_BORDER.0 as i32;
            set_window_style(parent.0, GWL_STYLE, style).unwrap();
            SetWindowPos(
                parent.0,
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .unwrap();
        }
        let child = TestWindow::new("STATIC", Some(parent.0), rect(17, 9, 20, 10), true);
        let child_rect = get_window_rect_safe(child.0).unwrap();
        assert_eq!(
            screen_to_client_point(
                parent.0,
                POINT {
                    x: child_rect.left,
                    y: child_rect.top
                }
            ),
            Some(POINT { x: 17, y: 9 })
        );

        let origin = TestWindow::new("STATIC", None, rect(0, 0, 20, 20), false);
        assert_eq!(
            screen_to_client_point(origin.0, POINT::default()),
            Some(POINT::default())
        );
        assert!(screen_to_client_point(HWND::default(), POINT::default()).is_none());
    }

    #[test]
    fn display_switch_keeps_visible_child_and_failed_switch_restores_attachment() {
        let first = TestWindow::new("STATIC", None, rect(-900, 100, 400, 100), true);
        let second = TestWindow::new("STATIC", None, rect(800, 250, 400, 100), true);
        let widget = TestWindow::new("STATIC", None, rect(-880, 120, 100, 30), true);
        unsafe {
            set_window_style(widget.0, GWL_EXSTYLE, WS_EX_LAYERED.0 as i32).unwrap();
            SetLayeredWindowAttributes(widget.0, COLORREF(0), 255, LWA_ALPHA).unwrap();
        }
        embed_in_taskbar(widget.0, first.0).unwrap();
        // Layer state is an observable sentinel for clearing WS_EX_LAYERED.
        // The startup transition must reset its old mode; later display
        // switches must preserve a child's existing composition state.
        unsafe {
            SetLayeredWindowAttributes(widget.0, COLORREF(0), 173, LWA_ALPHA).unwrap();
        }
        embed_in_taskbar(widget.0, second.0).unwrap();
        unsafe {
            assert_eq!(GetParent(widget.0).unwrap(), second.0);
            assert!(IsWindowVisible(widget.0).as_bool());
            let mut alpha = 0;
            GetLayeredWindowAttributes(widget.0, None, Some(&mut alpha), None).unwrap();
            assert_eq!(
                alpha, 173,
                "display changes must retain the child's layer state"
            );
            let original_style = GetWindowLongW(widget.0, GWL_STYLE);
            let original_ex_style = GetWindowLongW(widget.0, GWL_EXSTYLE);
            let original_rect = get_window_rect_safe(widget.0);
            // Simulate the final native positioning step failing after the
            // parent has changed. The existing visible attachment must survive.
            let failed = embed_in_taskbar_with(widget.0, first.0, |_, _| {
                Err(Error::new(E_FAIL, "injected positioning failure"))
            });
            assert!(failed.is_err());
            assert_eq!(GetParent(widget.0).unwrap(), second.0);
            assert_eq!(GetWindowLongW(widget.0, GWL_STYLE), original_style);
            assert_eq!(GetWindowLongW(widget.0, GWL_EXSTYLE), original_ex_style);
            assert_eq!(get_window_rect_safe(widget.0), original_rect);
            assert!(IsWindowVisible(widget.0).as_bool());

            assert!(embed_in_taskbar(widget.0, HWND::default()).is_err());
            assert_eq!(GetParent(widget.0).unwrap(), second.0);
        }
    }

    #[test]
    fn attaching_hidden_widget_does_not_override_user_visibility() {
        let parent = TestWindow::new("STATIC", None, rect(80, 100, 400, 100), true);
        let widget = TestWindow::new("STATIC", None, rect(0, 0, 100, 30), false);
        embed_in_taskbar(widget.0, parent.0).unwrap();
        unsafe {
            assert_eq!(GetParent(widget.0).unwrap(), parent.0);
            assert_eq!(GetWindowLongW(widget.0, GWL_STYLE) as u32 & WS_VISIBLE.0, 0);
        }
    }

    #[test]
    fn startup_popup_can_switch_to_layered_child_rendering() {
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
            embed_in_taskbar(child, parent).unwrap();
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
            let _ = DestroyWindow(parent);
            assert!(
                rendered.is_ok(),
                "fallback alpha mode must not prevent taskbar rendering: {rendered:?}"
            );
        }
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
