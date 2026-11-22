use std::sync::OnceLock;

use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DrawIconEx, DI_NORMAL, HICON, LR_DEFAULTCOLOR,
};

pub const SIZE: i32 = 16;
pub const RIGHT_MARGIN: i32 = 5;

#[derive(Clone, Copy)]
pub enum Provider {
    Claude,
    Codex,
    Antigravity,
}

pub fn draw(hdc: HDC, x: i32, y: i32, size: i32, provider: Provider) {
    draw_icon(hdc, x, y, size, original_icon(provider));
}

fn original_icon(provider: Provider) -> HICON {
    // The handles live for the process lifetime. DrawIconEx scales the
    // embedded 64px artwork to the current monitor's DPI without disk access.
    static CLAUDE: OnceLock<isize> = OnceLock::new();
    static CODEX: OnceLock<isize> = OnceLock::new();
    static ANTIGRAVITY: OnceLock<isize> = OnceLock::new();
    let (cache, png): (&OnceLock<isize>, &[u8]) = match provider {
        Provider::Claude => (&CLAUDE, include_bytes!("icons/providers/claude.png")),
        Provider::Codex => (&CODEX, include_bytes!("icons/providers/openai.png")),
        Provider::Antigravity => (
            &ANTIGRAVITY,
            include_bytes!("icons/providers/antigravity.png"),
        ),
    };
    let handle = *cache.get_or_init(|| unsafe {
        CreateIconFromResourceEx(png, true, 0x00030000, 64, 64, LR_DEFAULTCOLOR)
            .map(|icon| icon.0 as isize)
            .unwrap_or(0)
    });
    HICON(handle as *mut _)
}

fn draw_icon(hdc: HDC, x: i32, y: i32, size: i32, icon: HICON) {
    if !icon.is_invalid() {
        unsafe {
            let _ = DrawIconEx(hdc, x, y, icon, size, size, 0, HBRUSH::default(), DI_NORMAL);
        }
    }
}
