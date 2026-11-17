use super::{clamp, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardHistory {
    pub enabled: bool,
    /// Opens the list of copied text at the cursor.
    pub shortcut: String,
    /// How many copies are kept.
    pub max_items: u32,
    /// Keep the history when PowerKit or Windows restarts. Off by default: it writes
    /// everything copied to disk, including passwords from apps that do not mark them.
    pub remember: bool,
}

impl Default for ClipboardHistory {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Alt+V".into(),
            max_items: 50,
            remember: false,
        }
    }
}

impl ClipboardHistory {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(
            &mut self.shortcut,
            &ClipboardHistory::default().shortcut,
            "Clipboard History",
            w,
        );
        clamp(&mut self.max_items, 1, 500, "clipboard_history.max_items", w);
    }
}
