//! Optional provider decoration, shared by sizing, painting and settings.
use crate::native_interop::{wide_str, Color};
use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const FIRST_COMMAND: u16 = 110;
pub const LAST_COMMAND: u16 = 114;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexColor {
    #[default]
    Green,
    Neutral,
    Blue,
    Purple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub codex_color: CodexColor,
    pub show_provider_logos: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            codex_color: CodexColor::Green,
            show_provider_logos: true,
        }
    }
}

impl Appearance {
    pub fn color(self, dark: bool) -> Color {
        let (light, dark_color) = match self.codex_color {
            CodexColor::Green => ("#0F8F70", "#34D399"),
            CodexColor::Neutral => ("#404040", "#EEEEEE"),
            CodexColor::Blue => ("#2563EB", "#60A5FA"),
            CodexColor::Purple => ("#7C3AED", "#C4B5FD"),
        };
        Color::from_hex(if dark { dark_color } else { light })
    }

    pub fn icon_width(self) -> i32 {
        if self.show_provider_logos {
            crate::provider_icons::SIZE + crate::provider_icons::RIGHT_MARGIN
        } else {
            0
        }
    }

    pub fn apply_command(&mut self, id: u16) {
        match id {
            110 => self.codex_color = CodexColor::Green,
            111 => self.codex_color = CodexColor::Neutral,
            112 => self.codex_color = CodexColor::Blue,
            113 => self.codex_color = CodexColor::Purple,
            114 => self.show_provider_logos = !self.show_provider_logos,
            _ => {}
        }
    }

    pub fn append_menu(self, parent: HMENU, chinese: bool) {
        unsafe {
            let menu = parent;
            let Ok(colors) = CreatePopupMenu() else {
                return;
            };
            for (id, color, cn, en) in [
                (110, CodexColor::Green, "绿色", "Green"),
                (
                    111,
                    CodexColor::Neutral,
                    "中性（深色主题白色／浅色主题深灰）",
                    "Neutral (white / dark gray)",
                ),
                (112, CodexColor::Blue, "蓝色", "Blue"),
                (113, CodexColor::Purple, "紫色", "Purple"),
            ] {
                let text = wide_str(if chinese { cn } else { en });
                let _ = AppendMenuW(
                    colors,
                    if self.codex_color == color {
                        MF_CHECKED
                    } else {
                        MENU_ITEM_FLAGS(0)
                    },
                    id,
                    PCWSTR(text.as_ptr()),
                );
            }
            let label = wide_str(if chinese {
                "Codex 颜色"
            } else {
                "Codex color"
            });
            let _ = AppendMenuW(menu, MF_POPUP, colors.0 as usize, PCWSTR(label.as_ptr()));
            let label = wide_str(if chinese {
                "显示服务 Logo"
            } else {
                "Show provider logos"
            });
            let _ = AppendMenuW(
                menu,
                if self.show_provider_logos {
                    MF_CHECKED
                } else {
                    MENU_ITEM_FLAGS(0)
                },
                114,
                PCWSTR(label.as_ptr()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_attached_to_the_existing_appearance_menu() {
        unsafe {
            let menu = CreatePopupMenu().unwrap();
            Appearance::default().append_menu(menu, false);
            assert_eq!(GetMenuItemCount(menu), 2);
            let colors = GetSubMenu(menu, 0);
            assert_eq!(GetMenuItemCount(colors), 4);
            assert_eq!(GetMenuItemID(colors, 0), 110);
            assert_eq!(GetMenuItemID(menu, 1), 114);
            assert_ne!(GetMenuState(menu, 114, MF_BYCOMMAND) & MF_CHECKED.0, 0);
            let _ = DestroyMenu(menu);
        }
    }

    #[test]
    fn legacy_defaults_and_preferences_round_trip() {
        let default: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(default, Appearance::default());
        let mut selected = default;
        selected.apply_command(111);
        selected.apply_command(114);
        assert_eq!(selected.icon_width(), 0);
        assert_eq!(default.icon_width(), 21);
        let restored: Appearance =
            serde_json::from_str(&serde_json::to_string(&selected).unwrap()).unwrap();
        assert_eq!(restored, selected);
        assert_eq!(
            restored.color(true).to_colorref(),
            Color::from_hex("#EEEEEE").to_colorref()
        );
        assert_eq!(
            restored.color(false).to_colorref(),
            Color::from_hex("#404040").to_colorref()
        );
    }
}
