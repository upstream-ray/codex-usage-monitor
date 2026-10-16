//! Small progress pills with subpixel coverage, composited over the widget.
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::*;

use crate::native_interop::Color;

const SAMPLES: usize = 8;

fn inside(x: f64, y: f64, width: f64, height: f64) -> bool {
    if width <= 0.0 || x >= width {
        return false;
    }
    let radius = (height / 2.0).min(width / 2.0);
    let dx = x - x.clamp(radius, width - radius);
    let dy = y - y.clamp(radius, height - radius);
    dx * dx + dy * dy <= radius * radius
}

fn coverage(x: usize, y: usize, width: usize, height: usize, fraction: f64) -> (u32, u32) {
    let mut track = 0;
    let mut fill = 0;
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let px = x as f64 + (sx as f64 + 0.5) / SAMPLES as f64;
            let py = y as f64 + (sy as f64 + 0.5) / SAMPLES as f64;
            if inside(px, py, width as f64, height as f64) {
                track += 1;
                if inside(px, py, width as f64 * fraction, height as f64) {
                    fill += 1;
                }
            }
        }
    }
    (track, fill)
}

fn composite(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    fraction: f64,
    track: &Color,
    accent: &Color,
) {
    let samples = (SAMPLES * SAMPLES) as u32;
    for y in 0..height {
        for x in 0..width {
            let (covered, filled) = coverage(x, y, width, height, fraction.clamp(0.0, 1.0));
            let pixel = &mut pixels[y * width + x];
            let blend = |shift: u32, track_channel: u32, accent_channel: u32| {
                let background = (*pixel >> shift) & 255u32;
                (background * (samples - covered)
                    + track_channel * (covered - filled)
                    + accent_channel * filled
                    + samples / 2)
                    / samples
            };
            *pixel = (blend(16, track.r as u32, accent.r as u32) << 16)
                | (blend(8, track.g as u32, accent.g as u32) << 8)
                | blend(0, track.b as u32, accent.b as u32);
        }
    }
}

pub fn draw(hdc: HDC, rect: RECT, fraction: f64, track: &Color, accent: &Color) {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return;
    }
    unsafe {
        let dc = CreateCompatibleDC(hdc);
        if dc.is_invalid() {
            return;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
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
            return;
        }
        let old = SelectObject(dc, bitmap);
        if BitBlt(dc, 0, 0, width, height, hdc, rect.left, rect.top, SRCCOPY).is_ok() {
            // Complete GDI writes before reading the DIB from the CPU.
            let _ = GdiFlush();
            let pixels =
                std::slice::from_raw_parts_mut(bits.cast::<u32>(), (width * height) as usize);
            composite(
                pixels,
                width as usize,
                height as usize,
                fraction,
                track,
                accent,
            );
            let _ = BitBlt(hdc, rect.left, rect.top, width, height, dc, 0, 0, SRCCOPY);
        }
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capsule_edges_have_partial_coverage_and_symmetric_rounding() {
        let samples = (SAMPLES * SAMPLES) as u32;
        let mut partial = false;
        for y in 0..6 {
            for x in 0..40 {
                let c = coverage(x, y, 40, 6, 1.0);
                partial |= c.0 > 0 && c.0 < samples;
                assert_eq!(c.0, c.1);
                assert_eq!(c, coverage(39 - x, y, 40, 6, 1.0));
                assert_eq!(c, coverage(x, 5 - y, 40, 6, 1.0));
            }
        }
        assert!(
            partial,
            "edges must be antialiased rather than binary clipped"
        );
    }

    #[test]
    fn partial_and_empty_fill_stay_inside_the_track() {
        for fraction in [0.0, 0.01, 0.37, 1.0] {
            for y in 0..6 {
                for x in 0..40 {
                    let (track, fill) = coverage(x, y, 40, 6, fraction);
                    assert!(fill <= track);
                    if fraction == 0.0 {
                        assert_eq!(fill, 0);
                    }
                }
            }
        }
        assert!(coverage(14, 0, 40, 6, 0.37).1 < coverage(8, 0, 40, 6, 0.37).1);
    }

    #[test]
    fn edge_pixels_blend_with_background_without_changing_interior_colors() {
        let track = Color {
            r: 68,
            g: 68,
            b: 68,
        };
        let accent = Color {
            r: 32,
            g: 192,
            b: 128,
        };
        let mut pixels = vec![0x1c1c1c; 40 * 6];
        composite(&mut pixels, 40, 6, 0.5, &track, &accent);
        assert_eq!(pixels[2 * 40 + 8], 0x20c080);
        assert_eq!(pixels[2 * 40 + 28], 0x444444);
        assert!(pixels
            .iter()
            .any(|&p| p != 0x1c1c1c && p != 0x20c080 && p != 0x444444));
    }
}
