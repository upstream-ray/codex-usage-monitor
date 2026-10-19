use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, CreateIconIndirect, DestroyIcon, DrawIconEx, DI_NORMAL, HICON,
    ICONINFO, LR_DEFAULTCOLOR,
};

use crate::native_interop::Color;

pub const SIZE: i32 = 16;
pub const RIGHT_MARGIN: i32 = 5;

#[derive(Clone, Copy)]
pub enum Provider {
    Claude,
    Codex,
}

pub fn draw(hdc: HDC, x: i32, y: i32, size: i32, provider: Provider) {
    draw_icon(hdc, x, y, size, original_icon(provider));
}

fn original_icon(provider: Provider) -> HICON {
    // These two handles live for the process lifetime. DrawIconEx scales the
    // embedded 64px artwork to the current monitor's DPI without disk access.
    static CLAUDE: OnceLock<isize> = OnceLock::new();
    static CODEX: OnceLock<isize> = OnceLock::new();
    let (cache, png): (&OnceLock<isize>, &[u8]) = match provider {
        Provider::Claude => (&CLAUDE, include_bytes!("icons/providers/claude.png")),
        Provider::Codex => (&CODEX, include_bytes!("icons/providers/openai.png")),
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

struct TintedIcon {
    color: u32,
    handle: isize,
}

impl Drop for TintedIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(HICON(self.handle as *mut _));
        }
    }
}

/// Keep at most one tinted handle; repeated color changes release the old one.
pub fn draw_codex(hdc: HDC, x: i32, y: i32, size: i32, tint: Option<Color>) {
    static TINTED: Mutex<Option<TintedIcon>> = Mutex::new(None);
    let original = original_icon(Provider::Codex);
    let Some(color) = tint else {
        draw_icon(hdc, x, y, size, original);
        return;
    };
    let mut cached = TINTED.lock().unwrap_or_else(|error| error.into_inner());
    if cached.as_ref().map(|icon| icon.color) != Some(color.to_colorref()) {
        let Some(icon) = tint_icon(original, color) else {
            draw_icon(hdc, x, y, size, original);
            return;
        };
        *cached = Some(TintedIcon {
            color: color.to_colorref(),
            handle: icon.0 as isize,
        });
    }
    if let Some(icon) = cached.as_ref() {
        draw_icon(hdc, x, y, size, HICON(icon.handle as *mut _));
    }
}

fn tint_pixels(pixels: &mut [u8], color: Color) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u16;
        // Windows icon bitmaps use BGRA with premultiplied alpha.
        pixel[0] = (color.b as u16 * alpha / 255) as u8;
        pixel[1] = (color.g as u16 * alpha / 255) as u8;
        pixel[2] = (color.r as u16 * alpha / 255) as u8;
    }
}

fn tint_icon(original: HICON, color: Color) -> Option<HICON> {
    const EDGE: i32 = 64;
    if original.is_invalid() {
        return None;
    }
    unsafe {
        let dc = CreateCompatibleDC(HDC::default());
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: EDGE,
                biHeight: -EDGE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old = SelectObject(dc, bitmap);
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (EDGE * EDGE * 4) as usize);
        pixels.fill(0);
        let drawn = DrawIconEx(
            dc,
            0,
            0,
            original,
            EDGE,
            EDGE,
            0,
            HBRUSH::default(),
            DI_NORMAL,
        );
        let _ = GdiFlush();
        SelectObject(dc, old);
        let mut result = None;
        if drawn.is_ok() {
            tint_pixels(pixels, color);
            let mask_bits = [0u8; (EDGE * EDGE / 8) as usize];
            let mask = CreateBitmap(EDGE, EDGE, 1, 1, Some(mask_bits.as_ptr().cast()));
            if !mask.is_invalid() {
                result = CreateIconIndirect(&ICONINFO {
                    fIcon: true.into(),
                    hbmMask: mask,
                    hbmColor: bitmap,
                    ..Default::default()
                })
                .ok();
                let _ = DeleteObject(mask);
            }
        }
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_preserves_alpha_and_premultiplies_color_channels() {
        let mut pixels = [0, 0, 0, 0, 1, 2, 3, 128, 1, 2, 3, 255];
        tint_pixels(&mut pixels, Color::new(200, 100, 50));
        assert_eq!(pixels, [0, 0, 0, 0, 25, 50, 100, 128, 50, 100, 200, 255]);
    }
}
