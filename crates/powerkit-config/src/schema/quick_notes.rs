use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickNotes {
    pub enabled: bool,
    /// Shows the notes window, or hides it when it is in front.
    pub shortcut: String,
    /// Keep the notes window above other windows.
    pub always_on_top: bool,
}

impl Default for QuickNotes {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Alt+N".into(),
            always_on_top: false,
        }
    }
}

impl QuickNotes {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(&mut self.shortcut, &QuickNotes::default().shortcut, "Quick Notes", w);
    }
}
