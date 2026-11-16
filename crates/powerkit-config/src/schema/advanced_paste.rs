use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedPaste {
    pub enabled: bool,
    /// Shows the ways to paste what is on the clipboard, at the cursor.
    pub shortcut: String,
    /// Pastes the clipboard's text without its formatting. Empty for none.
    pub plain_shortcut: String,
    /// Pastes HTML on the clipboard as Markdown. Empty for none.
    pub markdown_shortcut: String,
    /// Pastes JSON, or a CSV or tab-separated table, as JSON. Empty for none.
    pub json_shortcut: String,
    /// Language tag for reading text in a picture (`en-US`); empty uses the Windows
    /// display languages.
    pub ocr_language: String,
}

impl Default for AdvancedPaste {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Shift+V".into(),
            plain_shortcut: "Win+Ctrl+Alt+V".into(),
            markdown_shortcut: String::new(),
            json_shortcut: String::new(),
            ocr_language: String::new(),
        }
    }
}

impl AdvancedPaste {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = AdvancedPaste::default();
        for (field, default) in [
            (&mut self.shortcut, &d.shortcut),
            (&mut self.plain_shortcut, &d.plain_shortcut),
            (&mut self.markdown_shortcut, &d.markdown_shortcut),
            (&mut self.json_shortcut, &d.json_shortcut),
        ] {
            fix_shortcut(field, default, "Advanced Paste", w);
        }
    }
}
