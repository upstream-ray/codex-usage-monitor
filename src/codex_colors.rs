use serde::{Deserialize, Serialize};

use crate::native_interop::Color;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SingleColor {
    #[default]
    Glacier,
    Lilac,
    Clay,
    Rose,
    Neutral,
}

impl SingleColor {
    pub const ALL: [Self; 5] = [
        Self::Glacier,
        Self::Lilac,
        Self::Clay,
        Self::Rose,
        Self::Neutral,
    ];

    pub fn bar(self, dark: bool) -> Color {
        let (light, dark_color) = match self {
            Self::Glacier => ("#2563EB", "#60A5FA"),
            Self::Lilac => ("#7C3AED", "#B8A2F2"),
            Self::Clay => ("#A94F32", "#D97757"),
            Self::Rose => ("#B83B61", "#F08FA3"),
            Self::Neutral => ("#475569", "#CBD5E1"),
        };
        Color::from_hex(if dark { dark_color } else { light })
    }
}

pub fn codex_brand_color(dark: bool) -> Color {
    Color::from_hex(if dark { "#34D399" } else { "#0F8F70" })
}
