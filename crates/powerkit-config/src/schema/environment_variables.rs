use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvironmentVariables {
    pub enabled: bool,
    /// Opens the editor. Empty: only from the tray menu or `powerkit --env`.
    pub shortcut: String,
}

impl Default for EnvironmentVariables {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: String::new(),
        }
    }
}

impl EnvironmentVariables {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(&mut self.shortcut, "", "Environment Variables", w);
    }
}
