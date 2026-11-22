//! Static sample painted by the same renderer as the taskbar widget.
use super::*;

const USED: [[f64; 3]; 2] = [[26.0, 62.0, 82.0], [11.0, 52.0, 83.0]];

fn sample(
    appearance: Appearance,
    system_dark: bool,
    language: LanguageId,
) -> WidgetContent<'static> {
    let strings = language.strings();
    let suffixes = [strings.hour_suffix, strings.day_suffix];
    let today = SYSTEMTIME {
        wYear: 2026,
        wMonth: 11,
        wDay: 22,
        ..Default::default()
    };
    let text = std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            let remaining = 100.0 - USED[row][column];
            let time = column + 1;
            let reset = if appearance.reset_time_display == ResetTimeDisplay::Clock {
                let reset = SYSTEMTIME {
                    wDay: if row == 0 {
                        today.wDay
                    } else {
                        28 + column as u16
                    },
                    wHour: 20 + column as u16,
                    wMinute: 4,
                    ..today
                };
                short_reset_time(reset, today, row == 1)
            } else {
                format!("{time}{}", suffixes[row])
            };
            format!("{remaining:.0}% {reset}").into()
        })
    });
    WidgetContent {
        system_dark,
        appearance,
        language,
        mode: QuotaDisplayMode::Remaining,
        providers: [true; 3],
        visible_rows: [true; 2],
        used: USED,
        text,
    }
}

pub(super) fn paint_sample(
    dc: HDC,
    width: i32,
    height: i32,
    appearance: Appearance,
    system_dark: bool,
    language: LanguageId,
) {
    paint_content(
        dc,
        width,
        height,
        &sample(appearance, system_dark, language),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "set CODEX_APPEARANCE_PREVIEW_DIR to render appearance fixtures"]
    fn render_appearance_previews() {
        let directory =
            PathBuf::from(std::env::var("CODEX_APPEARANCE_PREVIEW_DIR").expect("output directory"));
        std::fs::create_dir_all(&directory).unwrap();
        let sizes = [FontSize::Px12, FontSize::Px14, FontSize::Px16];
        let thicknesses = [
            BarThickness::Thin,
            BarThickness::Medium,
            BarThickness::Thick,
        ];
        for (style_index, style) in [
            BarStyle::Continuous,
            BarStyle::Segmented,
            BarStyle::Dots,
            BarStyle::Battery,
        ]
        .into_iter()
        .enumerate()
        {
            for (thin_index, thickness) in thicknesses.into_iter().enumerate() {
                let size = sizes[(style_index + thin_index) % sizes.len()];
                for color_mode in [ColorMode::Brand, ColorMode::Remaining, ColorMode::Single] {
                    for dark in [false, true] {
                        let appearance = Appearance {
                            palette: if dark {
                                Palette::HighContrastDark
                            } else {
                                Palette::HighContrastLight
                            },
                            bar_style: style,
                            bar_thickness: thickness,
                            font_size: size,
                            color_mode,
                            ..Appearance::default()
                        };
                        let background = if dark { "dark" } else { "light" };
                        let name = format!(
                            "{style:?}-{thickness:?}-{}-{color_mode:?}-{background}.bmp",
                            size.px()
                        );
                        render_fixture(&directory.join(name), appearance, dark);
                    }
                }
            }
        }
        for dark in [false, true] {
            for style in [BarStyle::Battery, BarStyle::Dots] {
                let appearance = Appearance {
                    bar_style: style,
                    bar_thickness: BarThickness::Thick,
                    font_size: FontSize::Px16,
                    ..Appearance::default()
                };
                let background = if dark { "dark" } else { "light" };
                render_fixture(
                    &directory.join(format!("{style:?}-system-{background}.bmp")),
                    appearance,
                    dark,
                );
            }
        }
        for (language, reset_time_display) in [
            (LanguageId::SimplifiedChinese, ResetTimeDisplay::Countdown),
            (LanguageId::SimplifiedChinese, ResetTimeDisplay::Clock),
            (LanguageId::English, ResetTimeDisplay::Clock),
        ] {
            for (font_size, dark) in [(FontSize::Px12, true), (FontSize::Px16, false)] {
                let appearance = Appearance {
                    bar_style: BarStyle::Battery,
                    bar_thickness: BarThickness::Thin,
                    font_size,
                    hide_period_labels: true,
                    percent_inside_battery: true,
                    reset_time_display,
                    ..Appearance::default()
                };
                let name = format!(
                    "compact-{language:?}-{reset_time_display:?}-{}.bmp",
                    font_size.px()
                );
                render_fixture_in_language(&directory.join(name), appearance, dark, language);
            }
        }
    }

    fn render_fixture(path: &std::path::Path, appearance: Appearance, dark: bool) {
        render_fixture_in_language(path, appearance, dark, LanguageId::SimplifiedChinese);
    }

    fn render_fixture_in_language(
        path: &std::path::Path,
        appearance: Appearance,
        dark: bool,
        language: LanguageId,
    ) {
        let width = total_widget_width_for(3, language, appearance);
        let height = sc(WIDGET_HEIGHT);
        unsafe {
            let dc = CreateCompatibleDC(HDC::default());
            assert!(!dc.is_invalid());
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bitmap);
            let content = sample(appearance, dark, language);
            paint_content(dc, width, height, &content);
            let _ = GdiFlush();
            let pixels =
                std::slice::from_raw_parts_mut(bits.cast::<u32>(), (width * height) as usize);
            check_fixture(path, pixels, width, height, &content);
            let original = pixels.to_vec();
            apply_widget_alpha(pixels, appearance, dark);
            let (background, _, _) = appearance_colors(appearance, dark);
            for (&before, &after) in original.iter().zip(pixels.iter()) {
                let rgb = before & 0x00FFFFFF;
                let expected =
                    if appearance.palette == Palette::System && rgb == dib_rgb(background) {
                        0x01000000
                    } else {
                        0xFF000000 | rgb
                    };
                assert_eq!(after, expected, "{}", path.display());
            }
            // BMP viewers often discard alpha. Composite the backdrop onto its sample taskbar color.
            let mut bmp = Vec::with_capacity(54 + pixels.len() * 4);
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&(54u32 + pixels.len() as u32 * 4).to_le_bytes());
            bmp.extend_from_slice(&[0u8; 4]);
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&width.to_le_bytes());
            bmp.extend_from_slice(&(-height).to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0u8; 24]);
            for pixel in original {
                bmp.extend_from_slice(&(0xFF000000 | pixel).to_le_bytes());
            }
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
            std::fs::write(path, bmp).unwrap();
        }
    }

    fn dib_rgb(color: Color) -> u32 {
        u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b)
    }

    fn check_fixture(
        path: &std::path::Path,
        pixels: &[u32],
        width: i32,
        height: i32,
        content: &WidgetContent<'_>,
    ) {
        let appearance = content.appearance;
        let dark = content.system_dark;
        let (background, _, _) = appearance_colors(appearance, dark);
        let bg = dib_rgb(background);
        let pixel = |x, y| pixels[(y * width + x) as usize] & 0x00FFFFFF;
        let (label_width, text_width) = usage_layout_widths(content.language, appearance, false);
        let segments = row_bar_segment_count(3);
        let bar_width = usage_bar_width(segments, appearance);
        let model_width =
            model_usage_width(segments, text_width, appearance) + sc(MODEL_RIGHT_MARGIN);
        let bar_left = sc(LEFT_DIVIDER_W)
            + sc(DIVIDER_RIGHT_MARGIN)
            + period_label_column_width(label_width, appearance)
            + provider_mark_width(appearance);
        let row_height = appearance.row_height();
        let gap = (height - 2 * row_height).clamp(0, sc(5));
        let row_top = (height - 2 * row_height - gap) / 2;
        let bar_height = drawn_bar_height(appearance, row_height);
        let inset = (row_height - bar_height) / 2;

        for (column, provider) in WIDGET_PROVIDERS.into_iter().enumerate() {
            let left = bar_left + column as i32 * model_width;
            let text_left = left + bar_width + sc(BAR_RIGHT_MARGIN);
            for (row, values) in content.used.iter().enumerate() {
                let top = row_top + row as i32 * (row_height + gap);
                let bar = top + inset..top + inset + bar_height;
                let accent = dib_rgb(appearance.bar_color(provider, values[column], dark));
                let mut lit_pixels = 0;
                for y in top..top + row_height {
                    for x in left..left + bar_width {
                        let color = pixel(x, y);
                        assert!(
                            bar.contains(&y) || color == bg,
                            "bar outside its vertical bounds in {}",
                            path.display()
                        );
                        if !appearance.shows_percent_in_battery() {
                            assert_ne!(color, 0, "black pixel in bar of {}", path.display());
                        }
                        lit_pixels += usize::from(color == accent);
                    }
                }
                assert!(lit_pixels > 0, "missing fill in {}", path.display());
                let ink_x: Vec<_> = (text_left..text_left + text_width)
                    .filter(|&x| (top..top + row_height).any(|y| pixel(x, y) != bg))
                    .collect();
                assert!(
                    !ink_x.is_empty(),
                    "missing percentage in {}",
                    path.display()
                );
                assert!(
                    ink_x.last().unwrap() < &(text_left + text_width - 1),
                    "text reaches right edge in {}",
                    path.display()
                );
                for y in [top, top + row_height - 1] {
                    assert!(
                        (text_left..text_left + text_width).all(|x| pixel(x, y) == bg),
                        "text reaches row edge in {}",
                        path.display()
                    );
                }
            }
        }
    }
}
