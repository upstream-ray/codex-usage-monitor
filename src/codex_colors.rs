//! Codex-only color preferences, independent of window layout and authentication.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::native_interop::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(u32);

impl Rgb {
    pub fn from_colorref(value: u32) -> Self {
        Self(((value & 0xff) << 16) | (value & 0xff00) | ((value >> 16) & 0xff))
    }

    pub fn color(self) -> Color {
        Color::new((self.0 >> 16) as u8, (self.0 >> 8) as u8, self.0 as u8)
    }

    fn parse(value: &str) -> Option<Self> {
        let digits = value.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(digits, 16).ok().map(Self)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("#{:06X}", self.0))
    }
}

// A malformed optional color must not discard the user's other settings.
fn deserialize_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Rgb>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_str().and_then(Rgb::parse))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorPreset {
    Clay,
    #[serde(alias = "mint")]
    Rose,
    Glacier,
    Lilac,
    #[default]
    #[serde(other)]
    Original,
}

impl ColorPreset {
    pub const ALL: [Self; 5] = [
        Self::Original,
        Self::Clay,
        Self::Rose,
        Self::Glacier,
        Self::Lilac,
    ];

    pub fn label(self, chinese: bool) -> &'static str {
        match (self, chinese) {
            (Self::Original, true) => "原始配色",
            (Self::Clay, true) => "陶土橙",
            (Self::Rose, true) => "玫瑰粉",
            (Self::Glacier, true) => "冰川蓝",
            (Self::Lilac, true) => "柔雾紫",
            (Self::Original, false) => "Original",
            (Self::Clay, false) => "Terracotta",
            (Self::Rose, false) => "Rose",
            (Self::Glacier, false) => "Glacier",
            (Self::Lilac, false) => "Lilac",
        }
    }

    fn colors(self, dark: bool) -> ResolvedColors {
        let (accent, percentage) = match (self, dark) {
            (Self::Original, true) => (0x34D399, 0x34D399),
            (Self::Original, false) => (0x0F8F70, 0x0F8F70),
            (Self::Clay, true) => (0xD97757, 0xF3D6C5),
            (Self::Clay, false) => (0xA94F32, 0x803B25),
            (Self::Rose, true) => (0xF08FA3, 0xFFD8E1),
            (Self::Rose, false) => (0xB83B61, 0x8F2448),
            (Self::Glacier, true) => (0x60A5FA, 0xD3E5FF),
            (Self::Glacier, false) => (0x2563EB, 0x1E40AF),
            (Self::Lilac, true) => (0xB8A2F2, 0xE9DFFC),
            (Self::Lilac, false) => (0x7C3AED, 0x5B21B6),
        };
        ResolvedColors {
            icon: Rgb(accent).color(),
            bar: Rgb(accent).color(),
            percentage: Rgb(percentage).color(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum ColorRole {
    Icon,
    Bar,
    Percentage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodexColors {
    pub preset: ColorPreset,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_color"
    )]
    icon: Option<Rgb>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_color"
    )]
    bar: Option<Rgb>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_color"
    )]
    percentage: Option<Rgb>,
}

pub struct ResolvedColors {
    pub icon: Color,
    pub bar: Color,
    pub percentage: Color,
}

impl CodexColors {
    pub fn from_preset(preset: ColorPreset) -> Self {
        Self {
            preset,
            ..Self::default()
        }
    }

    pub fn is_preset(self, preset: ColorPreset) -> bool {
        self == Self::from_preset(preset)
    }

    pub fn resolve(self, dark: bool) -> ResolvedColors {
        let defaults = self.preset.colors(dark);
        ResolvedColors {
            icon: self.icon.map(Rgb::color).unwrap_or(defaults.icon),
            bar: self.bar.map(Rgb::color).unwrap_or(defaults.bar),
            percentage: self
                .percentage
                .map(Rgb::color)
                .unwrap_or(defaults.percentage),
        }
    }

    pub fn icon_tint(self, dark: bool) -> Option<Color> {
        if self.preset == ColorPreset::Original && self.icon.is_none() {
            None // Preserve the original provider artwork unless customization is requested.
        } else {
            Some(self.resolve(dark).icon)
        }
    }

    pub fn set(&mut self, role: ColorRole, color: Rgb) {
        match role {
            ColorRole::Icon => self.icon = Some(color),
            ColorRole::Bar => self.bar = Some(color),
            ColorRole::Percentage => self.percentage = Some(color),
        }
    }

    pub fn color(self, role: ColorRole, dark: bool) -> Color {
        let colors = self.resolve(dark);
        match role {
            ColorRole::Icon => colors.icon,
            ColorRole::Bar => colors.bar,
            ColorRole::Percentage => colors.percentage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_colors_round_trip_and_remain_independent() {
        let mut colors = CodexColors::from_preset(ColorPreset::Glacier);
        colors.set(ColorRole::Bar, Rgb::from_colorref(0x563412));
        let json = serde_json::to_string(&colors).unwrap();
        assert!(json.contains("#123456"));
        assert_eq!(serde_json::from_str::<CodexColors>(&json).unwrap(), colors);
        for dark in [false, true] {
            assert_eq!(colors.resolve(dark).bar.to_colorref(), 0x563412);
            assert_eq!(
                colors.resolve(dark).icon.to_colorref(),
                ColorPreset::Glacier.colors(dark).icon.to_colorref()
            );
        }
        assert!(!colors.is_preset(ColorPreset::Glacier));
        assert!(CodexColors::from_preset(ColorPreset::Rose).is_preset(ColorPreset::Rose));
    }

    #[test]
    fn malformed_colors_do_not_discard_valid_preferences() {
        for invalid in ["\"#xx1234\"", "\"#123\"", "\"#é1234\"", "123", "null"] {
            let json = format!(r##"{{"preset":"clay","icon":{invalid},"bar":"#123456"}}"##);
            let colors: CodexColors = serde_json::from_str(&json).unwrap();
            assert_eq!(colors.preset, ColorPreset::Clay);
            assert!(colors.icon.is_none());
            assert_eq!(colors.bar, Some(Rgb(0x123456)));
        }
        let unknown: CodexColors = serde_json::from_str(r#"{"preset":"future"}"#).unwrap();
        assert_eq!(unknown, CodexColors::default());
    }

    #[test]
    fn legacy_mint_preset_migrates_to_rose_without_losing_custom_colors() {
        let colors: CodexColors =
            serde_json::from_str(r##"{"preset":"mint","bar":"#123456"}"##).unwrap();
        assert_eq!(colors.preset, ColorPreset::Rose);
        assert_eq!(colors.bar, Some(Rgb(0x123456)));
        let saved = serde_json::to_string(&colors).unwrap();
        assert!(saved.contains("\"preset\":\"rose\""));
        assert!(!saved.contains("mint"));
    }

    #[test]
    fn new_presets_adapt_to_both_backgrounds_and_keep_numbers_readable() {
        fn luminance(color: Color) -> f64 {
            let linear = |byte: u8| {
                let value = byte as f64 / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
        }
        // Original intentionally preserves upstream colors, including its light-mode contrast.
        for preset in ColorPreset::ALL
            .into_iter()
            .filter(|preset| *preset != ColorPreset::Original)
        {
            for dark in [false, true] {
                let foreground = luminance(preset.colors(dark).percentage);
                let background = luminance(if dark {
                    Color::from_hex("#1C1C1C")
                } else {
                    Color::from_hex("#F3F3F3")
                });
                let contrast =
                    (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
                assert!(contrast >= 4.5, "{preset:?}, dark={dark}: {contrast}");
            }
        }
    }
}
