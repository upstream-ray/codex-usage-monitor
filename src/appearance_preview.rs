//! Opt-in GDI render fixtures; no network or user credentials are accessed.
use super::*;

#[test]
#[ignore = "set CODEX_APPEARANCE_PREVIEW_DIR to render appearance fixtures"]
fn render_appearance_previews() {
    let directory =
        PathBuf::from(std::env::var("CODEX_APPEARANCE_PREVIEW_DIR").expect("output directory"));
    std::fs::create_dir_all(&directory).unwrap();
    for (index, preset) in ColorPreset::ALL.into_iter().enumerate() {
        for dark in [false, true] {
            render_fixture(
                &directory.join(format!(
                    "{index}-{}.bmp",
                    if dark { "dark" } else { "light" }
                )),
                preset,
                dark,
                BarStyle::ClassicSegmented,
            );
        }
    }
    for (name, style) in [
        ("continuous", BarStyle::Continuous),
        ("segmented", BarStyle::Segmented),
    ] {
        render_fixture(
            &directory.join(format!("{name}.bmp")),
            ColorPreset::Glacier,
            true,
            style,
        );
    }
}

fn render_fixture(path: &std::path::Path, preset: ColorPreset, dark: bool, style: BarStyle) {
    let appearance = Appearance {
        palette: if dark {
            Palette::HighContrastDark
        } else {
            Palette::HighContrastLight
        },
        bar_style: style,
        font_size: FontSize::Large,
        codex_colors: CodexColors::from_preset(preset),
        ..Appearance::default()
    };
    let language = LanguageId::SimplifiedChinese;
    let width = total_widget_width_for(1, language, appearance);
    let height = WIDGET_HEIGHT;
    let (bg, text, track) = appearance_colors(appearance, dark);
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
        paint_content(
            dc,
            width,
            height,
            dark,
            appearance,
            &bg,
            &text,
            &claude_accent_color(),
            &track,
            language,
            language.strings(),
            0.0,
            "--",
            0.0,
            "--",
            64.0,
            "36% 18:57重置",
            25.0,
            "75% 10/25重置",
            0.0,
            "--",
            0.0,
            "--",
            false,
            true,
            false,
            true,
            true,
            &appearance.codex_colors.resolve(dark).bar,
            &antigravity_accent_color(),
        );
        let _ = GdiFlush();
        let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), (width * height * 4) as usize);
        // Check the actual native icon output, including the DIB -> HICON tint path.
        if appearance.codex_colors.icon_tint(dark).is_some() {
            let (label_width, _) = usage_layout_widths(language, appearance);
            let icon_x = LEFT_DIVIDER_W + DIVIDER_RIGHT_MARGIN + label_width + LABEL_RIGHT_MARGIN;
            let icon_y = (height - provider_icons::SIZE) / 2;
            let expected = appearance.codex_colors.resolve(dark).icon;
            let mut matching_pixels = 0;
            for y in icon_y..icon_y + provider_icons::SIZE {
                for x in icon_x..icon_x + provider_icons::SIZE {
                    let offset = ((y * width + x) * 4) as usize;
                    if pixels[offset..offset + 3] == [expected.b, expected.g, expected.r] {
                        matching_pixels += 1;
                    }
                }
            }
            assert!(
                matching_pixels > 2,
                "{preset:?}, dark={dark}: tinted icon is missing"
            );
        }
        let mut bmp = Vec::with_capacity(54 + pixels.len());
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes());
        bmp.extend_from_slice(&[0u8; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&width.to_le_bytes());
        bmp.extend_from_slice(&(-height).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0u8; 24]);
        bmp.extend_from_slice(pixels);
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        std::fs::write(path, bmp).unwrap();
    }
}
