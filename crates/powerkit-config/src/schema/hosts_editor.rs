use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostsEditor {
    pub enabled: bool,
    /// Opens the editor. Empty: only from the tray menu or `powerkit --hosts`.
    pub shortcut: String,
}

impl Default for HostsEditor {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: String::new(),
        }
    }
}

impl HostsEditor {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(&mut self.shortcut, "", "Hosts File Editor", w);
    }
}
