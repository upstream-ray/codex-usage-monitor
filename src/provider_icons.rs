use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::native_interop::Color;

pub const SIZE: i32 = 16;
pub const RIGHT_MARGIN: i32 = 5;

#[derive(Clone, Copy)]
pub enum Provider {
    Claude,
    Codex,
}

pub fn codex_color(is_dark: bool) -> Color {
    Color::from_hex(if is_dark { "#34D399" } else { "#0F8F70" })
}

fn icon(provider: Provider) -> HICON {
    // Embedded resources and native handles live for the process lifetime.
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

const SOURCE_SIZE: usize = 64;

fn codex_alpha() -> Option<&'static [u8]> {
    static ALPHA: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    ALPHA
        .get_or_init(|| unsafe {
            let mut info = ICONINFO::default();
            GetIconInfo(icon(Provider::Codex), &mut info).ok()?;
            let dc = CreateCompatibleDC(HDC::default());
            let mut bitmap_info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: SOURCE_SIZE as i32,
                    biHeight: -(SOURCE_SIZE as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u32; SOURCE_SIZE * SOURCE_SIZE];
            let copied = if dc.is_invalid() || info.hbmColor.is_invalid() {
                0
            } else {
                GetDIBits(
                    dc,
                    info.hbmColor,
                    0,
                    SOURCE_SIZE as u32,
                    Some(pixels.as_mut_ptr().cast()),
                    &mut bitmap_info,
                    DIB_RGB_COLORS,
                )
            };
            let _ = DeleteObject(info.hbmColor);
            let _ = DeleteObject(info.hbmMask);
            let _ = DeleteDC(dc);
            if copied != SOURCE_SIZE as i32 || pixels.iter().all(|p| p >> 24 == 0) {
                return None;
            }
            Some(pixels.into_iter().map(|p| (p >> 24) as u8).collect())
        })
        .as_deref()
}

/// Average coverage over each destination pixel, including fractional source
/// pixels at non-integral DPI sizes. Avoid GDI's aliased icon scaling.
fn resample_alpha(source: &[u8], source_size: usize, size: usize) -> Vec<u8> {
    let ratio = source_size as f64 / size as f64;
    let mut mask = vec![0; size * size];
    for y in 0..size {
        let top = y as f64 * ratio;
        let bottom = (y + 1) as f64 * ratio;
        for x in 0..size {
            let left = x as f64 * ratio;
            let right = (x + 1) as f64 * ratio;
            let mut alpha = 0.0;
            for sy in top.floor() as usize..(bottom.ceil() as usize).min(source_size) {
                let dy = bottom.min((sy + 1) as f64) - top.max(sy as f64);
                for sx in left.floor() as usize..(right.ceil() as usize).min(source_size) {
                    let dx = right.min((sx + 1) as f64) - left.max(sx as f64);
                    alpha += source[sy * source_size + sx] as f64 * dx * dy;
                }
            }
            mask[y * size + x] = (alpha / (ratio * ratio)).round().clamp(0.0, 255.0) as u8;
        }
    }
    mask
}

fn blend(pixels: &mut [u32], alpha: &[u8], color: Color) {
    for (pixel, &alpha) in pixels.iter_mut().zip(alpha) {
        let alpha = alpha as u32;
        let channel = |shift: u32, foreground: u8| {
            (((*pixel >> shift) & 255u32) * (255 - alpha) + foreground as u32 * alpha + 127) / 255
        };
        *pixel = channel(16, color.r) << 16 | channel(8, color.g) << 8 | channel(0, color.b);
    }
}

fn draw_codex(hdc: HDC, x: i32, y: i32, size: i32) -> bool {
    static MASKS: OnceLock<Mutex<BTreeMap<i32, Vec<u8>>>> = OnceLock::new();
    let Some(source) = codex_alpha() else {
        return false;
    };
    let alpha = {
        let mut masks = MASKS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        masks
            .entry(size)
            .or_insert_with(|| resample_alpha(source, SOURCE_SIZE, size as usize))
            .clone()
    };
    unsafe {
        let dc = CreateCompatibleDC(hdc);
        if dc.is_invalid() {
            return false;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap =
            CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
        if bitmap.is_invalid() || bits.is_null() {
            let _ = DeleteDC(dc);
            return false;
        }
        let previous = SelectObject(dc, bitmap);
        let mut drawn = false;
        if BitBlt(dc, 0, 0, size, size, hdc, x, y, SRCCOPY).is_ok() {
            let _ = GdiFlush();
            let pixels = std::slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize);
            // The existing artwork is a single green mark. Blend its coverage
            // over the actual background so transparent edges have no halo.
            blend(pixels, &alpha, Color::from_hex("#10A37F"));
            drawn = BitBlt(hdc, x, y, size, size, dc, 0, 0, SRCCOPY).is_ok();
        }
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        drawn
    }
}

pub fn draw(hdc: HDC, x: i32, y: i32, size: i32, provider: Provider) {
    if size <= 0 {
        return;
    }
    if matches!(provider, Provider::Codex) && draw_codex(hdc, x, y, size) {
        return;
    }
    let handle = icon(provider);
    if !handle.is_invalid() {
        unsafe {
            let _ = DrawIconEx(
                hdc,
                x,
                y,
                handle,
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

    #[test]
    fn area_filter_preserves_coverage_at_fractional_dpi_sizes() {
        assert_eq!(resample_alpha(&[255, 0, 0, 0], 2, 1), vec![64]);
        assert_eq!(
            resample_alpha(&[255, 0, 0, 255], 2, 3),
            vec![255, 128, 0, 128, 128, 128, 0, 128, 255]
        );
        for size in [16, 20, 24, 28, 32] {
            assert!(resample_alpha(&vec![255; 64 * 64], 64, size)
                .iter()
                .all(|&a| a == 255));
        }
    }

    #[test]
    fn logo_edges_blend_without_changing_transparent_background() {
        let mut dark = [0x1c1c1c; 3];
        let mut light = [0xf9f9f9; 3];
        let color = Color::from_hex("#10A37F");
        blend(&mut dark, &[0, 128, 255], color);
        blend(&mut light, &[0, 128, 255], color);
        assert_eq!(dark[0], 0x1c1c1c);
        assert_eq!(light[0], 0xf9f9f9);
        assert_eq!(dark[2], 0x10a37f);
        assert_eq!(light[2], 0x10a37f);
        assert_ne!(dark[1], dark[0]);
        assert_ne!(dark[1], dark[2]);
        assert_ne!(light[1], dark[1]);
    }

    #[test]
    fn native_codex_resource_has_alpha_for_smooth_small_curves_and_open_gaps() {
        let source = codex_alpha().expect("embedded logo must decode with alpha");
        assert_eq!(source.len(), SOURCE_SIZE * SOURCE_SIZE);
        for size in [16, 20, 24, 28, 32] {
            let mask = resample_alpha(source, SOURCE_SIZE, size);
            assert!(mask.iter().any(|&a| a > 0 && a < 255));
            assert!(
                mask.iter().any(|&a| a >= 128),
                "thin strokes must stay visible"
            );
            let original_coverage =
                source.iter().map(|&a| a as f64).sum::<f64>() / source.len() as f64;
            let scaled_coverage = mask.iter().map(|&a| a as f64).sum::<f64>() / mask.len() as f64;
            assert!(
                (original_coverage - scaled_coverage).abs() < 0.6,
                "scaling must preserve the mark's coverage"
            );
            assert_eq!(
                mask[(size / 2) * size + size / 2],
                0,
                "center must stay open"
            );
            assert_eq!(mask[0], 0, "outer corners must stay transparent");
        }
    }
}
