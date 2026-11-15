use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RegistryPreview {
    pub enabled: bool,
    /// Opens the editor. Empty: only from the tray menu or `powerkit --reg`.
    pub shortcut: String,
}

impl Default for RegistryPreview {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: String::new(),
        }
    }
}

impl RegistryPreview {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(&mut self.shortcut, "", "Registry Preview", w);
    }
}
