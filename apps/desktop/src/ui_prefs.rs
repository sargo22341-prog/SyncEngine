//! Presentation-only settings owned by the desktop shell (theme, folded
//! groups, empty groups created by the user, close button behavior). Kept in
//! a small versioned JSON file next to the application state, separate from
//! synchronization policy. New fields are optional: older files still load.

use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE: &str = "desktop-ui.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
    /// Full black backgrounds for OLED screens.
    Amoled,
}

/// What the window close button (×) does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CloseAction {
    /// Minimize to the taskbar; quitting is explicit (icon menu, Ctrl+Q).
    #[default]
    Minimize,
    /// Quit the application (a running task is cancelled first).
    Quit,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UiPrefs {
    pub(crate) version: u32,
    pub(crate) theme: ThemeChoice,
    pub(crate) collapsed: Vec<String>,
    pub(crate) groups: Vec<String>,
    pub(crate) close: CloseAction,
}

impl UiPrefs {
    /// Missing file means defaults; unreadable or future files are reported.
    pub(crate) fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    version: VERSION,
                    ..Self::default()
                });
            }
            Err(e) => return Err(format!("préférences d’affichage illisibles : {e}")),
        };
        let prefs: Self = serde_json::from_str(&text)
            .map_err(|e| format!("préférences d’affichage invalides : {e}"))?;
        if prefs.version > VERSION {
            return Err("préférences d’affichage d’une version plus récente ignorées".into());
        }
        Ok(Self {
            version: VERSION,
            ..prefs
        })
    }

    /// Writes through a temporary file then renames it over the old one.
    pub(crate) fn save(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| format!("écriture des préférences : {e}"))?;
        std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| format!("écriture des préférences : {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let empty = UiPrefs::load(dir.path()).unwrap();
        assert_eq!(empty.theme, ThemeChoice::System);
        let prefs = UiPrefs {
            version: VERSION,
            theme: ThemeChoice::Light,
            collapsed: vec!["Travail".into()],
            groups: vec!["Photos".into()],
            close: CloseAction::Quit,
        };
        prefs.save(dir.path()).unwrap();
        assert_eq!(UiPrefs::load(dir.path()).unwrap(), prefs);
        // A file written before the close setting existed keeps loading.
        std::fs::write(
            dir.path().join(FILE),
            r#"{"version":1,"theme":"dark","collapsed":[],"groups":[]}"#,
        )
        .unwrap();
        let legacy = UiPrefs::load(dir.path()).unwrap();
        assert_eq!(legacy.theme, ThemeChoice::Dark);
        assert_eq!(legacy.close, CloseAction::Minimize);
        std::fs::write(dir.path().join(FILE), r#"{"version":99}"#).unwrap();
        assert!(UiPrefs::load(dir.path()).is_err());
        std::fs::write(dir.path().join(FILE), r#"{"version":1,"theme":"amoled"}"#).unwrap();
        assert_eq!(
            UiPrefs::load(dir.path()).unwrap().theme,
            ThemeChoice::Amoled
        );
        std::fs::write(dir.path().join(FILE), "{").unwrap();
        assert!(UiPrefs::load(dir.path()).is_err());
    }
}
