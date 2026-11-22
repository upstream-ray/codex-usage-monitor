use std::ops::Range;
use windows::Win32::Foundation::RECT;

/// An opaque BGRX DIB. The high byte is unused by GDI. Shape colors may carry
/// their own alpha, which is multiplied by the pixel's edge coverage.
pub(super) struct Canvas<'a> {
    pixels: &'a mut [u32],
    stride: usize,
    clip: RECT,
}

#[derive(Clone, Copy)]
struct RoundedRect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    radius: f32,
}

impl RoundedRect {
    fn new(left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> Self {
        let radius = radius.clamp(0.0, ((right - left).min(bottom - top) * 0.5).max(0.0));
        Self {
            left,
            top,
            right,
            bottom,
            radius,
        }
    }

    fn from_rect(rect: RECT, radius: f32) -> Self {
        Self::new(
            rect.left as f32,
            rect.top as f32,
            rect.right as f32,
            rect.bottom as f32,
            radius,
        )
    }

    fn inset(self, width: f32) -> Option<Self> {
        let inner = Self::new(
            self.left + width,
            self.top + width,
            self.right - width,
            self.bottom - width,
            self.radius - width,
        );
        (inner.left < inner.right && inner.top < inner.bottom).then_some(inner)
    }

    fn vertical_distance(self, y: f32) -> f32 {
        (self.top + self.radius - y).max(y - (self.bottom - self.radius))
    }

    fn coverage(self, x: f32, dy: f32) -> f32 {
        let dx = (self.left + self.radius - x).max(x - (self.right - self.radius));
        let outside_x = dx.max(0.0);
        let outside_y = dy.max(0.0);
        let distance = (outside_x * outside_x + outside_y * outside_y).sqrt() + dx.max(dy).min(0.0)
            - self.radius;
        (0.5 - distance).clamp(0.0, 1.0)
    }

    /// Pixel centers whose signed distance is at most `limit` on this row.
    fn span(self, dy: f32, limit: f32, clip: &Range<usize>) -> Range<usize> {
        let radius = self.radius + limit;
        if dy > radius {
            return clip.start..clip.start;
        }
        let reach = if dy > 0.0 {
            (radius * radius - dy * dy).sqrt()
        } else {
            radius
        };
        let left = (self.left + self.radius - reach - 0.5).ceil() as usize;
        let right = ((self.right - self.radius + reach - 0.5).floor() as i32 + 1).max(0) as usize;
        let start = left.clamp(clip.start, clip.end);
        let end = right.clamp(start, clip.end);
        start..end
    }
}

impl<'a> Canvas<'a> {
    pub(super) fn new(pixels: &'a mut [u32], stride: usize, clip: RECT) -> Self {
        let width = stride as i32;
        let height = (pixels.len() / stride) as i32;
        let clip = RECT {
            left: clip.left.clamp(0, width),
            right: clip.right.clamp(0, width),
            top: clip.top.clamp(0, height),
            bottom: clip.bottom.clamp(0, height),
        };
        Self {
            pixels,
            stride,
            clip,
        }
    }

    pub(super) fn fill(&mut self, rect: RECT, radius: f32, argb: u32) {
        self.draw(RoundedRect::from_rect(rect, radius), None, argb);
    }

    pub(super) fn stroke(&mut self, rect: RECT, radius: f32, argb: u32, width: f32) {
        if width > 0.0 {
            // The former centered stroke used an inset centerline.
            let outer = RoundedRect::from_rect(rect, radius + width * 0.5);
            self.draw(outer, outer.inset(width), argb);
        }
    }

    pub(super) fn circle(&mut self, x: f32, y: f32, diameter: f32, argb: u32) {
        let r = diameter * 0.5;
        self.draw(RoundedRect::new(x - r, y - r, x + r, y + r, r), None, argb);
    }

    pub(super) fn card(&mut self, rect: RECT, radius: f32, fill: u32, border: u32) {
        self.fill(rect, radius, opaque(border));
        self.fill(
            RECT {
                left: rect.left + 1,
                top: rect.top + 1,
                right: rect.right - 1,
                bottom: rect.bottom - 1,
            },
            (radius - 1.0).max(0.0),
            opaque(fill),
        );
    }

    fn draw(&mut self, outer: RoundedRect, inner: Option<RoundedRect>, argb: u32) {
        if outer.left >= outer.right
            || outer.top >= outer.bottom
            || self.clip.left >= self.clip.right
        {
            return;
        }
        let alpha = (argb >> 24) as u8;
        let color = argb & 0x00FF_FFFF;
        let clip_x = self.clip.left.max(outer.left.floor() as i32) as usize
            ..self.clip.right.min(outer.right.ceil() as i32).max(0) as usize;
        if alpha == 0 || clip_x.is_empty() {
            return;
        }
        let top = self.clip.top.max(outer.top.floor() as i32);
        let bottom = self.clip.bottom.min(outer.bottom.ceil() as i32);
        for y in top..bottom {
            let center_y = y as f32 + 0.5;
            let dy = outer.vertical_distance(center_y);
            let visible = outer.span(dy, 0.5, &clip_x);
            if visible.is_empty() {
                continue;
            }
            let row = &mut self.pixels[y as usize * self.stride..][..self.stride];
            if let Some(inner) = inner {
                let inner_dy = inner.vertical_distance(center_y);
                let hole = inner.span(inner_dy, -0.5, &visible);
                // Skip the fully transparent interior of the stroke.
                for x in (visible.start..hole.start).chain(hole.end..visible.end) {
                    let center_x = x as f32 + 0.5;
                    let coverage =
                        outer.coverage(center_x, dy) - inner.coverage(center_x, inner_dy);
                    blend(&mut row[x], color, alpha, coverage);
                }
            } else {
                let solid = outer.span(dy, -0.5, &visible);
                if alpha == 255 {
                    row[solid.clone()].fill(color);
                } else {
                    for pixel in &mut row[solid.clone()] {
                        blend(pixel, color, alpha, 1.0);
                    }
                }
                // Straight sides and the interior are handled as a span. Only
                // the edge pixels need an individual distance calculation.
                for x in (visible.start..solid.start).chain(solid.end..visible.end) {
                    blend(
                        &mut row[x],
                        color,
                        alpha,
                        outer.coverage(x as f32 + 0.5, dy),
                    );
                }
            }
        }
    }
}

pub(super) fn opaque(rgb: u32) -> u32 {
    0xFF00_0000 | rgb
}

fn blend(pixel: &mut u32, color: u32, alpha: u8, coverage: f32) {
    let weight = (coverage * f32::from(alpha)).round() as u32;
    if weight == 255 {
        *pixel = color;
    } else if weight != 0 {
        let channel = |shift: u32| -> u32 {
            let source = (color >> shift) & 255u32;
            let target = (*pixel >> shift) & 255u32;
            ((source * weight + target * (255 - weight) + 127) / 255) << shift
        };
        *pixel = channel(16) | channel(8) | channel(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn rounded_fill_has_symmetric_partial_edges_and_opaque_interior() {
        let mut pixels = [0; 100];
        Canvas::new(&mut pixels, 10, rect(0, 0, 10, 10)).fill(
            rect(1, 1, 9, 9),
            3.0,
            opaque(0xFF_FFFF),
        );
        assert_eq!(pixels[0], 0);
        assert_eq!(pixels[11], 0);
        assert!((1..255).contains(&(pixels[12] & 255)));
        assert_eq!(pixels[15], 0xFF_FFFF);
        assert_eq!(pixels[55], 0xFF_FFFF);
        for y in 0..10 {
            for x in 0..10 {
                assert_eq!(pixels[10 * y + x], pixels[10 * y + 9 - x]);
                assert_eq!(pixels[10 * y + x], pixels[10 * (9 - y) + x]);
            }
        }
    }

    #[test]
    fn coverage_multiplies_source_alpha_before_blending() {
        let mut opaque = [0; 64];
        let mut translucent = [0x00_0020; 64];
        let bounds = rect(0, 0, 8, 8);
        Canvas::new(&mut opaque, 8, bounds).fill(bounds, 3.0, 0xFFFF_0000);
        Canvas::new(&mut translucent, 8, bounds).fill(bounds, 3.0, 0x80FF_0000);
        let edge = opaque[1] >> 16;
        assert!((1..255).contains(&edge));
        assert!((translucent[1] >> 16).abs_diff(edge.div_ceil(2)) <= 1);
        assert_eq!(translucent[3], 0x80_0010);
    }

    #[test]
    fn fractional_strokes_keep_the_interior_and_are_mirrored() {
        for (width, at_edge, inside_edge) in [(0.5, 128, 0), (2.0, 255, 255), (2.5, 255, 255)] {
            let mut pixels = [0; 144];
            let bounds = rect(0, 0, 12, 12);
            Canvas::new(&mut pixels, 12, bounds).stroke(bounds, 4.0, opaque(0x00_FF00), width);
            assert_eq!(pixels[5] >> 8, at_edge);
            assert_eq!(pixels[17] >> 8, inside_edge);
            assert_eq!(pixels[29] >> 8, if width == 2.5 { 128 } else { 0 });
            assert_eq!(pixels[65], 0);
            for y in 0..12 {
                for x in 0..12 {
                    assert_eq!(pixels[12 * y + x], pixels[12 * y + 11 - x]);
                    assert_eq!(pixels[12 * y + x], pixels[12 * (11 - y) + x]);
                }
            }
        }
    }

    #[test]
    fn circles_have_symmetric_edges_and_shapes_clip_to_the_dib_and_dirty_rect() {
        let mut circle = [0; 64];
        Canvas::new(&mut circle, 8, rect(0, 0, 8, 8)).circle(4.0, 4.0, 6.0, opaque(0xFF_FFFF));
        assert!((1..255).contains(&(circle[10] & 255)));
        assert_eq!(circle[10], circle[13]);
        assert_eq!(circle[10], circle[50]);
        assert_eq!(circle[27], 0xFF_FFFF);

        let mut memory = [0x12_3456; 66];
        let mut canvas = Canvas::new(&mut memory[1..65], 8, rect(2, -8, 12, 5));
        canvas.card(rect(-6, -6, 14, 14), 6.0, 0x11_1111, 0x22_2222);
        canvas.stroke(rect(-2, -2, 10, 10), 4.0, opaque(0xFF_FFFF), 2.5);
        canvas.circle(8.0, 0.0, 16.0, opaque(0x00_00FF));
        canvas.fill(rect(-20, -20, -10, -10), 2.0, opaque(0xFF_0000));
        assert_eq!(memory[0], 0x12_3456);
        assert_eq!(memory[65], 0x12_3456);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(memory[1 + 8 * y + x] == 0x12_3456, x < 2 || y >= 5);
            }
        }
    }
}
