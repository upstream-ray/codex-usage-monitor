//! Keep native settings menus open while applying checkbox/radio commands.
use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy)]
struct Session {
    owner: HWND,
    root: HMENU,
    selected: Option<(HMENU, u16)>,
    refresh: fn(HMENU),
    keep: fn(u16) -> bool,
    busy: bool,
}
thread_local! { static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) }; }

pub fn select(owner: HWND, wparam: WPARAM, lparam: LPARAM) {
    SESSION.with(|slot| {
        if let Some(s) = slot.borrow_mut().as_mut() {
            if s.owner != owner {
                return;
            }
            let flags = (wparam.0 >> 16) as u32;
            s.selected = if lparam.0 != 0
                && flags & (MF_POPUP.0 | MF_SEPARATOR.0 | MF_DISABLED.0 | MF_GRAYED.0) == 0
            {
                Some((HMENU(lparam.0 as *mut _), wparam.0 as u16))
            } else {
                None
            };
        }
    });
}

unsafe extern "system" fn filter(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == MSGF_MENU as i32 && lp.0 != 0 && activate(&*(lp.0 as *const MSG)) {
        return LRESULT(1);
    }
    CallNextHookEx(None, code, wp, lp)
}

unsafe fn activate(message: &MSG) -> bool {
    let Some(session) = SESSION.with(|slot| *slot.borrow()) else {
        return false;
    };
    let Some((menu, id)) = session.selected else {
        return false;
    };
    if session.busy || !(session.keep)(id) {
        return false;
    }
    let keyboard = message.message == WM_KEYDOWN && matches!(message.wParam.0, 13 | 32);
    let mouse = message.message == WM_LBUTTONUP;
    if !keyboard && !mouse {
        return false;
    }
    let Some(index) = (0..GetMenuItemCount(menu)).find(|&i| GetMenuItemID(menu, i) == id as u32)
    else {
        return false;
    };
    let mut rect = RECT::default();
    if mouse
        && (GetMenuItemRect(session.owner, menu, index as u32, &mut rect).is_err()
            || message.pt.x < rect.left
            || message.pt.x >= rect.right
            || message.pt.y < rect.top
            || message.pt.y >= rect.bottom)
    {
        return false;
    }
    // Commands and painting reenter the UI, so never hold the RefCell here.
    SESSION.with(|slot| {
        if let Some(s) = slot.borrow_mut().as_mut() {
            s.busy = true;
        }
    });
    SendMessageW(session.owner, WM_COMMAND, WPARAM(id as usize), LPARAM(0));
    (session.refresh)(session.root);
    SESSION.with(|slot| {
        if let Some(s) = slot.borrow_mut().as_mut() {
            s.busy = false;
        }
    });
    if mouse {
        let window = WindowFromPoint(message.pt);
        let _ = InvalidateRect(window, None, false);
        let _ = UpdateWindow(window);
    }
    true
}

pub unsafe fn track(
    owner: HWND,
    root: HMENU,
    point: POINT,
    refresh: fn(HMENU),
    keep: fn(u16) -> bool,
) {
    SESSION.with(|slot| {
        *slot.borrow_mut() = Some(Session {
            owner,
            root,
            selected: None,
            refresh,
            keep,
            busy: false,
        })
    });
    let hook = SetWindowsHookExW(WH_MSGFILTER, Some(filter), None, GetCurrentThreadId()).ok();
    let _ = TrackPopupMenu(root, TPM_RIGHTBUTTON, point.x, point.y, 0, owner, None);
    if let Some(hook) = hook {
        let _ = UnhookWindowsHookEx(hook);
    }
    SESSION.with(|slot| *slot.borrow_mut() = None);
}

/// Update state and labels without replacing open submenu handles.
pub unsafe fn refresh(current: HMENU, fresh: HMENU) {
    for index in 0..GetMenuItemCount(current).min(GetMenuItemCount(fresh)) {
        let mut text = [0u16; 512];
        let mut next = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STATE | MIIM_STRING | MIIM_SUBMENU,
            dwTypeData: windows::core::PWSTR(text.as_mut_ptr()),
            cch: text.len() as u32,
            ..Default::default()
        };
        let mut old = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STATE | MIIM_SUBMENU,
            ..Default::default()
        };
        if GetMenuItemInfoW(fresh, index as u32, true, &mut next).is_err()
            || GetMenuItemInfoW(current, index as u32, true, &mut old).is_err()
        {
            continue;
        }
        if !old.hSubMenu.is_invalid() && !next.hSubMenu.is_invalid() {
            refresh(old.hSubMenu, next.hSubMenu);
        }
        next.fMask = MIIM_STATE | MIIM_STRING;
        next.fState |= old.fState & MFS_HILITE;
        let _ = SetMenuItemInfoW(current, index as u32, true, &next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::core::w;
    static COMMANDS: AtomicUsize = AtomicUsize::new(0);
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static HOOK_CLICKS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_COMMAND {
            COMMANDS.fetch_add(1, Ordering::Relaxed);
            return LRESULT(0);
        }
        if msg == WM_MENUSELECT {
            select(h, wp, lp);
        }
        DefWindowProcW(h, msg, wp, lp)
    }
    #[test]
    fn settings_commands_and_checks_update_without_replacing_submenus() {
        let _serial = SERIAL.lock().unwrap();
        COMMANDS.store(0, Ordering::Relaxed);
        unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                lpszClassName: w!("PersistentMenuTest"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let owner = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("PersistentMenuTest"),
                w!("test"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let menu = CreatePopupMenu().unwrap();
            let fresh = CreatePopupMenu().unwrap();
            let child = CreatePopupMenu().unwrap();
            let fresh_child = CreatePopupMenu().unwrap();
            AppendMenuW(menu, MF_POPUP, child.0 as usize, w!("Settings")).unwrap();
            AppendMenuW(fresh, MF_POPUP, fresh_child.0 as usize, w!("Settings")).unwrap();
            AppendMenuW(child, MF_STRING, 73, w!("Used")).unwrap();
            AppendMenuW(fresh_child, MF_STRING | MF_CHECKED, 73, w!("Used")).unwrap();
            refresh(menu, fresh);
            assert_eq!(GetSubMenu(menu, 0), child);
            assert_ne!(GetMenuState(child, 73, MF_BYCOMMAND) & MF_CHECKED.0, 0);
            SESSION.with(|slot| {
                *slot.borrow_mut() = Some(Session {
                    owner,
                    root: menu,
                    selected: Some((child, 73)),
                    refresh: |_| {},
                    keep: |id| id == 73,
                    busy: false,
                })
            });
            let key = MSG {
                message: WM_KEYDOWN,
                wParam: WPARAM(13),
                ..Default::default()
            };
            assert!(activate(&key));
            assert!(activate(&key));
            assert_eq!(COMMANDS.load(Ordering::Relaxed), 2);
            assert!(!activate(&MSG {
                wParam: WPARAM(27),
                ..key
            }));
            select(
                owner,
                WPARAM(73 | ((MF_GRAYED.0 as usize) << 16)),
                LPARAM(child.0 as isize),
            );
            assert!(!activate(&key));
            SESSION.with(|slot| *slot.borrow_mut() = None);
            let _ = DestroyMenu(menu);
            let _ = DestroyMenu(fresh);
            let _ = DestroyWindow(owner);
        }
    }

    unsafe extern "system" fn tick(owner: HWND, _: u32, id: usize, _: u32) {
        let Some(session) = SESSION.with(|s| *s.borrow()) else {
            let _ = EndMenu();
            return;
        };
        let mut rect = RECT::default();
        let mut success = false;
        if GetMenuItemRect(owner, session.root, 0, &mut rect).is_ok() {
            SendMessageW(
                owner,
                WM_MENUSELECT,
                WPARAM(73),
                LPARAM(session.root.0 as isize),
            );
            let mut click = MSG {
                hwnd: WindowFromPoint(POINT {
                    x: rect.left + 5,
                    y: rect.top + 5,
                }),
                message: WM_LBUTTONUP,
                pt: POINT {
                    x: rect.left + 5,
                    y: rect.top + 5,
                },
                ..Default::default()
            };
            success = CallMsgFilterW(&mut click, MSGF_MENU as i32).as_bool();
            if success {
                success = CallMsgFilterW(&mut click, MSGF_MENU as i32).as_bool();
                let mut escape = MSG {
                    message: WM_KEYDOWN,
                    wParam: WPARAM(27),
                    ..Default::default()
                };
                success &= !CallMsgFilterW(&mut escape, MSGF_MENU as i32).as_bool();
            }
        }
        HOOK_CLICKS.store(usize::from(success), Ordering::Relaxed);
        let _ = KillTimer(owner, id);
        let _ = EndMenu();
    }

    #[test]
    #[ignore = "requires interactive Windows desktop; run explicitly for menu-loop validation"]
    fn real_native_menu_loop_consumes_two_clicks_without_dismissing() {
        let _serial = SERIAL.lock().unwrap();
        COMMANDS.store(0, Ordering::Relaxed);
        HOOK_CLICKS.store(0, Ordering::Relaxed);
        unsafe {
            let foreground = GetForegroundWindow();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                lpszClassName: w!("PersistentMenuLoopTest"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let owner = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("PersistentMenuLoopTest"),
                w!("menu test"),
                WS_POPUP,
                100,
                100,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let _ = ShowWindow(owner, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(owner);
            let menu = CreatePopupMenu().unwrap();
            AppendMenuW(menu, MF_STRING, 73, w!("Usage remaining")).unwrap();
            SetTimer(owner, 777, 100, Some(tick));
            track(owner, menu, POINT { x: 100, y: 100 }, |_| {}, |id| id == 73);
            let _ = KillTimer(owner, 777);
            let _ = DestroyMenu(menu);
            let _ = DestroyWindow(owner);
            let _ = SetForegroundWindow(foreground);
            assert_eq!(
                HOOK_CLICKS.load(Ordering::Relaxed),
                1,
                "real thread menu hook must consume clicks and leave Escape available"
            );
            assert_eq!(COMMANDS.load(Ordering::Relaxed), 2);
        }
    }
}
