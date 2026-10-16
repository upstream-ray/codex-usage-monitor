//! Measure the taskbar's existing Widgets button without blocking its UI thread.
use std::sync::{Mutex, Once};
use std::time::Duration;

use windows::core::VARIANT;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_AutomationIdPropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const WM_WIDGETS_CHANGED: u32 = crate::native_interop::WM_APP + 6;

#[derive(Default)]
struct State {
    taskbar: isize,
    owner: isize,
    ready: bool,
    weather: Option<RECT>,
}
static STATE: Mutex<State> = Mutex::new(State {
    taskbar: 0,
    owner: 0,
    ready: false,
    weather: None,
});
static START: Once = Once::new();

pub fn watch(taskbar: HWND, owner: HWND) {
    {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.taskbar != taskbar.0 as isize || state.owner != owner.0 as isize {
            *state = State {
                taskbar: taskbar.0 as isize,
                owner: owner.0 as isize,
                ..Default::default()
            };
        }
    }
    if taskbar == HWND::default() {
        return;
    }
    START.call_once(|| {
        std::thread::spawn(worker);
    });
}

pub fn left_edge(taskbar: HWND, bounds: RECT, gap: i32) -> i32 {
    let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.taskbar == taskbar.0 as isize && state.ready {
        left_edge_after_widgets(bounds, state.weather, gap)
    } else {
        // Avoid covering weather while the first accessibility query is pending.
        bounds.left + (bounds.bottom - bounds.top).max(0).saturating_mul(4)
    }
}

fn left_edge_after_widgets(bounds: RECT, weather: Option<RECT>, gap: i32) -> i32 {
    weather
        .filter(|r| {
            r.left >= bounds.left
                && r.right > r.left
                && r.right <= bounds.left + (bounds.right - bounds.left) / 2
                && r.top < bounds.bottom
                && r.bottom > bounds.top
        })
        .map(|r| (r.right + gap.max(0)).min(bounds.right))
        .unwrap_or(bounds.left)
}

fn key(rect: Option<RECT>) -> Option<(i32, i32, i32, i32)> {
    rect.map(|r| (r.left, r.top, r.right, r.bottom))
}

fn worker() {
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return;
        }
        {
            if let Ok(automation) =
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            {
                if let Ok(condition) = automation.CreatePropertyCondition(
                    UIA_AutomationIdPropertyId,
                    &VARIANT::from("WidgetsButton"),
                ) {
                    loop {
                        let (target, owner) = {
                            let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
                            (state.taskbar, state.owner)
                        };
                        if target != 0 {
                            let result = (|| -> windows::core::Result<Option<RECT>> {
                                let root = automation.ElementFromHandle(HWND(target as *mut _))?;
                                let matches = root.FindAll(TreeScope_Descendants, &condition)?;
                                if matches.Length()? == 0 {
                                    return Ok(None);
                                }
                                let button = matches.GetElement(0)?;
                                if button.CurrentIsOffscreen()?.as_bool() {
                                    return Ok(None);
                                }
                                Ok(Some(button.CurrentBoundingRectangle()?))
                            })();
                            if let Ok(weather) = result {
                                let changed = {
                                    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
                                    if state.taskbar != target || state.owner != owner {
                                        false
                                    } else {
                                        let changed =
                                            !state.ready || key(state.weather) != key(weather);
                                        state.ready = true;
                                        state.weather = weather;
                                        changed
                                    }
                                };
                                if changed {
                                    let _ = PostMessageW(
                                        HWND(owner as *mut _),
                                        WM_WIDGETS_CHANGED,
                                        WPARAM(0),
                                        LPARAM(0),
                                    );
                                }
                            }
                        }
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        }
        CoUninitialize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinning_clears_weather_and_tracks_its_width() {
        let bar = RECT {
            left: 0,
            top: 1392,
            right: 3440,
            bottom: 1440,
        };
        let weather = RECT {
            left: 6,
            top: 1392,
            right: 158,
            bottom: 1440,
        };
        assert_eq!(left_edge_after_widgets(bar, Some(weather), 8), 166);
        assert_eq!(
            left_edge_after_widgets(
                bar,
                Some(RECT {
                    right: 240,
                    ..weather
                }),
                8
            ),
            248
        );
        assert_eq!(left_edge_after_widgets(bar, None, 8), 0);
        assert_eq!(
            left_edge_after_widgets(
                bar,
                Some(RECT {
                    top: -48,
                    bottom: 0,
                    ..weather
                }),
                8
            ),
            0
        );
        assert_eq!(
            left_edge_after_widgets(
                bar,
                Some(RECT {
                    left: 3300,
                    right: 3400,
                    ..weather
                }),
                8
            ),
            0
        );
    }
}
