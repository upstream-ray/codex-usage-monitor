//! A bounded, persistent library of named Codex color schemes.
use serde::{Deserialize, Deserializer, Serialize};

use crate::codex_colors::CodexColors;

pub const MAX_USER_PRESETS: usize = 32;
pub const MAX_NAME_CHARS: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPreset {
    pub name: String,
    pub colors: CodexColors,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct UserPresets(Vec<UserPreset>);

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    InvalidName,
    Full,
}

impl UserPresets {
    pub fn entries(&self) -> &[UserPreset] {
        &self.0
    }

    pub fn find(&self, name: &str) -> Option<&UserPreset> {
        let key = name.trim().to_lowercase();
        self.0.iter().find(|entry| entry.name.to_lowercase() == key)
    }

    pub fn save(&mut self, name: &str, colors: CodexColors) -> Result<(), SaveError> {
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return Err(SaveError::InvalidName);
        }
        let key = name.to_lowercase();
        if let Some(entry) = self
            .0
            .iter_mut()
            .find(|entry| entry.name.to_lowercase() == key)
        {
            entry.name = name.to_string();
            entry.colors = colors;
        } else {
            if self.0.len() >= MAX_USER_PRESETS {
                return Err(SaveError::Full);
            }
            self.0.push(UserPreset {
                name: name.to_string(),
                colors,
            });
        }
        Ok(())
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }
}

impl<'de> Deserialize<'de> for UserPresets {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut library = Self::default();
        if let Some(entries) = value.as_array() {
            for value in entries {
                if let Ok(entry) = serde_json::from_value::<UserPreset>(value.clone()) {
                    let _ = library.save(&entry.name, entry.colors);
                }
            }
        }
        Ok(library)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_colors::{ColorPreset, ColorRole, Rgb};

    #[test]
    fn saved_scheme_round_trips_with_independent_custom_colors() {
        let mut colors = CodexColors::from_preset(ColorPreset::Rose);
        colors.set(ColorRole::Icon, Rgb::from_colorref(0x123456));
        let mut library = UserPresets::default();
        library.save("  Evening & calm  ", colors).unwrap();
        let json = serde_json::to_string(&library).unwrap();
        let restored: UserPresets = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, library);
        assert_eq!(restored.find("EVENING & CALM").unwrap().colors, colors);
    }

    #[test]
    fn duplicate_names_replace_in_place_and_deleting_preserves_other_entries() {
        let mut library = UserPresets::default();
        library.save("One", CodexColors::default()).unwrap();
        library.save("Two", CodexColors::default()).unwrap();
        let colors = CodexColors::from_preset(ColorPreset::Clay);
        library.save("one", colors).unwrap();
        assert_eq!(library.entries().len(), 2);
        assert_eq!(library.entries()[0].colors, colors);
        library.remove(0);
        assert!(library.find("One").is_none());
        assert!(library.find("Two").is_some());
        library.remove(usize::MAX);
        assert_eq!(library.entries().len(), 1);
    }

    #[test]
    fn invalid_or_excess_entries_do_not_discard_valid_presets() {
        let mut library = UserPresets::default();
        for name in ["", " \t ", "bad\nname", &"x".repeat(MAX_NAME_CHARS + 1)] {
            assert_eq!(
                library.save(name, CodexColors::default()),
                Err(SaveError::InvalidName)
            );
        }
        for index in 0..MAX_USER_PRESETS {
            library
                .save(&format!("Preset {index}"), CodexColors::default())
                .unwrap();
        }
        assert_eq!(
            library.save("Extra", CodexColors::default()),
            Err(SaveError::Full)
        );
        library
            .save("Preset 0", CodexColors::from_preset(ColorPreset::Rose))
            .unwrap();
        let restored: UserPresets = serde_json::from_str(
            r#"[null,{"name":"Bad"},{"name":"Valid","colors":{"preset":"rose"}}]"#,
        )
        .unwrap();
        assert_eq!(restored.entries().len(), 1);
        assert_eq!(restored.entries()[0].name, "Valid");
    }
}
