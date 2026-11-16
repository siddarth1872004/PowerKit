use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Peek {
    pub enabled: bool,
    /// Shows the file selected in File Explorer. Only taken while File Explorer or
    /// the desktop is in front, so other programs keep the shortcut.
    pub shortcut: String,
}

impl Default for Peek {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Ctrl+Space".into(),
        }
    }
}

impl Peek {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = Peek::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "Peek", w);
    }
}
