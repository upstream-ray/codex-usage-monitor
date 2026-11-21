//! A small, native, keyboard-accessible dialog for naming a color preset.
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{EM_SETLIMITTEXT, EM_SETSEL};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::native_interop::wide_str;
use crate::user_presets::MAX_NAME_CHARS;

const NAME_EDIT: i32 = 100;

struct DialogState {
    initial: Vec<u16>,
    chinese: bool,
    result: Option<String>,
}

pub fn prompt(owner: HWND, chinese: bool, initial: &str) -> Option<String> {
    let template = template(chinese);
    let mut state = DialogState {
        initial: wide_str(initial),
        chinese,
        result: None,
    };
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let result = DialogBoxIndirectParamW(
            HINSTANCE(module.0),
            template.as_ptr().cast(),
            owner,
            Some(dialog_proc),
            LPARAM((&mut state as *mut DialogState) as isize),
        );
        if result == -1 {
            crate::diagnose::log_error(
                "unable to open preset name dialog",
                windows::core::Error::from_win32(),
            );
        }
    }
    state.result
}

unsafe extern "system" fn dialog_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if message == WM_INITDIALOG {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
        let state = &*(lparam.0 as *const DialogState);
        let _ = SetDlgItemTextW(hwnd, NAME_EDIT, PCWSTR(state.initial.as_ptr()));
        // Allow supplementary Unicode characters; validate by character count on Save.
        SendDlgItemMessageW(
            hwnd,
            NAME_EDIT,
            EM_SETLIMITTEXT,
            WPARAM(MAX_NAME_CHARS * 2),
            LPARAM(0),
        );
        SendDlgItemMessageW(hwnd, NAME_EDIT, EM_SETSEL, WPARAM(0), LPARAM(-1));
        return 1;
    }
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
    if state.is_null() {
        return 0;
    }
    match message {
        WM_COMMAND if wparam.0 as u16 == IDOK.0 as u16 => {
            let mut buffer = [0u16; MAX_NAME_CHARS * 2 + 1];
            let length = GetDlgItemTextW(hwnd, NAME_EDIT, &mut buffer) as usize;
            let name = String::from_utf16_lossy(&buffer[..length]);
            let name = name.trim();
            if name.is_empty()
                || name.chars().count() > MAX_NAME_CHARS
                || name.chars().any(char::is_control)
            {
                let text = wide_str(if (*state).chinese {
                    "请输入 1–40 个字符的名称。"
                } else {
                    "Enter a name of 1–40 characters."
                });
                let title = wide_str(if (*state).chinese {
                    "预设名称"
                } else {
                    "Preset name"
                });
                MessageBoxW(
                    hwnd,
                    PCWSTR(text.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_OK | MB_ICONINFORMATION,
                );
            } else {
                (*state).result = Some(name.to_string());
                let _ = EndDialog(hwnd, 1);
            }
            1
        }
        WM_CLOSE => {
            let _ = EndDialog(hwnd, 0);
            1
        }
        WM_COMMAND if wparam.0 as u16 == IDCANCEL.0 as u16 => {
            let _ = EndDialog(hwnd, 0);
            1
        }
        _ => 0,
    }
}

fn word(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn dword(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn text(bytes: &mut Vec<u8>, value: &str) {
    for unit in value.encode_utf16().chain(Some(0)) {
        word(bytes, unit);
    }
}
fn align(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

fn control(bytes: &mut Vec<u8>, class: u16, id: u16, rect: [u16; 4], style: u32, label: &str) {
    align(bytes);
    dword(bytes, WS_CHILD.0 | WS_VISIBLE.0 | style);
    dword(bytes, 0);
    for value in rect {
        word(bytes, value);
    }
    word(bytes, id);
    word(bytes, 0xffff);
    word(bytes, class);
    text(bytes, label);
    word(bytes, 0);
}

fn template(chinese: bool) -> Vec<u32> {
    let mut bytes = Vec::new();
    dword(
        &mut bytes,
        WS_POPUP.0
            | WS_CAPTION.0
            | WS_SYSMENU.0
            | DS_MODALFRAME as u32
            | DS_SETFONT as u32
            | DS_CENTER as u32,
    );
    dword(&mut bytes, 0);
    for value in [4, 0, 0, 254, 86] {
        word(&mut bytes, value);
    }
    word(&mut bytes, 0); // No menu.
    word(&mut bytes, 0); // Default dialog class.
    text(
        &mut bytes,
        if chinese {
            "保存当前配色"
        } else {
            "Save current colors"
        },
    );
    word(&mut bytes, 9);
    text(&mut bytes, "Segoe UI");
    control(
        &mut bytes,
        0x82,
        0xffff,
        [10, 10, 234, 12],
        0,
        if chinese {
            "预设名称（最多 40 个字符）"
        } else {
            "Preset name (up to 40 characters)"
        },
    );
    control(
        &mut bytes,
        0x81,
        NAME_EDIT as u16,
        [10, 27, 234, 15],
        WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
        "",
    );
    control(
        &mut bytes,
        0x80,
        IDOK.0 as u16,
        [139, 59, 50, 16],
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        if chinese { "保存" } else { "Save" },
    );
    control(
        &mut bytes,
        0x80,
        IDCANCEL.0 as u16,
        [194, 59, 50, 16],
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        if chinese { "取消" } else { "Cancel" },
    );
    align(&mut bytes);
    // The Win32 template and every control must begin at a DWORD-aligned address.
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "interactive: cancel the first dialog, then save Evening rose in the second"]
    fn native_dialog_cancel_and_save() {
        assert_eq!(prompt(HWND::default(), true, "我的配色 1"), None);
        assert_eq!(
            prompt(HWND::default(), false, "Custom 1"),
            Some("Evening rose".to_string())
        );
    }
}
