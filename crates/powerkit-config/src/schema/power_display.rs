use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerDisplay {
    pub enabled: bool,
    /// Opens the window with each monitor's controls.
    pub shortcut: String,
    pub contrast: bool,
    pub volume: bool,
    /// These three can blank the picture, so they are off until asked for and each
    /// change is confirmed.
    pub input_source: bool,
    pub color_preset: bool,
    pub power_state: bool,
}

impl Default for PowerDisplay {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Alt+M".into(),
            contrast: true,
            volume: true,
            input_source: false,
            color_preset: false,
            power_state: false,
        }
    }
}

impl PowerDisplay {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(
            &mut self.shortcut,
            &PowerDisplay::default().shortcut,
            "Power Display",
            w,
        );
    }
}
