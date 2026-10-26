use std::sync::OnceLock;

use windows::Win32::Graphics::Gdi::{HBRUSH, HDC};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DrawIconEx, DI_NORMAL, HICON, LR_DEFAULTCOLOR,
};

use crate::native_interop::Color;

pub const SIZE: i32 = 16;
pub const RIGHT_MARGIN: i32 = 5;

#[derive(Clone, Copy)]
pub enum Provider {
    Claude,
    Codex,
    Antigravity,
}

pub fn codex_color(is_dark: bool) -> Color {
    Color::from_hex(if is_dark { "#34D399" } else { "#0F8F70" })
}

pub fn draw(hdc: HDC, x: i32, y: i32, size: i32, provider: Provider) {
    // These handles live for the process lifetime. DrawIconEx scales the
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
    if handle != 0 {
        unsafe {
            let _ = DrawIconEx(
                hdc,
                x,
                y,
                HICON(handle as *mut _),
                size,
                size,
                0,
                HBRUSH::default(),
                DI_NORMAL,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::*;

    #[test]
    fn embedded_provider_icons_render_at_taskbar_scale_factors() {
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, 32, 32);
            assert!(!bitmap.is_invalid());
            let old = SelectObject(dc, bitmap);
            let mut visible = Vec::new();
            for provider in [Provider::Claude, Provider::Codex, Provider::Antigravity] {
                for size in [16, 20, 32] {
                    PatBlt(dc, 0, 0, 32, 32, BLACKNESS).unwrap();
                    draw(dc, 0, 0, size, provider);
                    let colored = (0..size)
                        .flat_map(|y| (0..size).map(move |x| (x, y)))
                        .filter(|&(x, y)| {
                            let pixel = GetPixel(dc, x, y).0;
                            pixel != 0 && pixel != u32::MAX
                        })
                        .count();
                    visible.push(colored > 20);
                }
            }
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            assert!(
                visible.iter().all(|&value| value),
                "all providers must render"
            );
        }
    }
}
