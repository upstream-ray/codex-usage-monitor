//! Windows wake and connectivity notifications, without a polling thread.
use crate::diagnose;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::{s, w};
use windows::Win32::Foundation::{BOOLEAN, HANDLE, HWND, LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, PNETWORK_CONNECTIVITY_HINT_CHANGE_CALLBACK,
};
use windows::Win32::Networking::WinSock::{
    NetworkConnectivityLevelHintInternetAccess, NL_NETWORK_CONNECTIVITY_HINT,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Power::{
    RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification, HPOWERNOTIFY,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, DEVICE_NOTIFY_WINDOW_HANDLE};

pub const MESSAGE: u32 = 0x8000 + 4;
pub const DISCONNECTED_MESSAGE: u32 = 0x8000 + 5;
static CONNECTIVITY: Mutex<Connectivity> = Mutex::new(Connectivity { online: None });
#[derive(Default)]
struct Connectivity {
    online: Option<bool>,
}
impl Connectivity {
    fn observe(&mut self, online: bool) -> Option<u32> {
        let previous = self.online.replace(online);
        match (previous, online) {
            (Some(false), true) => Some(MESSAGE),
            (None | Some(true), false) => Some(DISCONNECTED_MESSAGE),
            _ => None,
        }
    }
}
#[derive(Default)]
pub struct Debounce {
    last: [Option<Instant>; 2],
}
impl Debounce {
    pub fn accept(&mut self, network: bool, now: Instant) -> bool {
        let last = &mut self.last[network as usize];
        if last.is_some_and(|time| now.saturating_duration_since(time) < Duration::from_secs(5)) {
            return false;
        }
        *last = Some(now);
        true
    }
}
type NotifyHintChange = unsafe extern "system" fn(
    PNETWORK_CONNECTIVITY_HINT_CHANGE_CALLBACK,
    *const c_void,
    BOOLEAN,
    *mut HANDLE,
) -> WIN32_ERROR;

/// Windows 10 2004+ only. Resolved at runtime so older builds still start, without network recovery.
unsafe fn notify_network_connectivity_hint_change() -> Option<NotifyHintChange> {
    let module = LoadLibraryW(w!("iphlpapi.dll")).ok()?;
    let function = GetProcAddress(module, s!("NotifyNetworkConnectivityHintChange"))?;
    Some(std::mem::transmute::<
        unsafe extern "system" fn() -> isize,
        NotifyHintChange,
    >(function))
}
pub struct Watch {
    power: Option<HPOWERNOTIFY>,
    network: isize,
}
unsafe extern "system" fn network_changed(
    context: *const c_void,
    hint: NL_NETWORK_CONNECTIVITY_HINT,
) {
    let online = hint.ConnectivityLevel == NetworkConnectivityLevelHintInternetAccess;
    let message = CONNECTIVITY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .observe(online);
    if let Some(message) = message {
        let _ = PostMessageW(HWND(context as *mut _), message, WPARAM(1), LPARAM(0));
    }
}
impl Watch {
    pub fn register(hwnd: HWND) -> Self {
        unsafe {
            let power = RegisterSuspendResumeNotification(hwnd, DEVICE_NOTIFY_WINDOW_HANDLE)
                .map_err(|e| diagnose::log_error("wake notification registration failed", e))
                .ok();
            let mut network = HANDLE::default();
            let Some(notify) = notify_network_connectivity_hint_change() else {
                diagnose::log("network notification unavailable on this Windows version");
                return Self { power, network: 0 };
            };
            let status = notify(
                Some(network_changed),
                hwnd.0 as *const c_void,
                BOOLEAN(1),
                &mut network,
            );
            if status.0 != 0 {
                diagnose::log(format!(
                    "network notification registration failed code={}",
                    status.0
                ));
            }
            diagnose::log(format!(
                "recovery notifications registered wake={} network={}",
                power.is_some(),
                status.0 == 0
            ));
            Self {
                power,
                network: if status.0 == 0 { network.0 as isize } else { 0 },
            }
        }
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            if self.network != 0 {
                let _ = CancelMibChangeNotify2(HANDLE(self.network as *mut _));
            }
            if let Some(power) = self.power {
                let _ = UnregisterSuspendResumeNotification(power);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_or_cost_only_changes_do_not_refresh_but_restored_internet_does() {
        let mut state = Connectivity::default();
        assert_eq!(state.observe(true), None);
        assert_eq!(state.observe(true), None);
        assert_eq!(state.observe(false), Some(DISCONNECTED_MESSAGE));
        assert_eq!(state.observe(false), None);
        assert_eq!(state.observe(true), Some(MESSAGE));
        assert_eq!(state.observe(true), None);
        let mut offline_start = Connectivity::default();
        assert_eq!(offline_start.observe(false), Some(DISCONNECTED_MESSAGE));
    }
    #[test]
    fn duplicate_wake_events_coalesce_without_suppressing_a_later_network_recovery() {
        let now = Instant::now();
        let mut events = Debounce::default();
        assert!(events.accept(false, now));
        assert!(!events.accept(false, now + Duration::from_secs(1)));
        assert!(events.accept(true, now + Duration::from_secs(1)));
        assert!(!events.accept(true, now + Duration::from_secs(2)));
        assert!(events.accept(true, now + Duration::from_secs(6)));
    }
}
