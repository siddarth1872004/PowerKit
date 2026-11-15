use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileLocksmith {
    pub enabled: bool,
    /// Show "Unlock with PowerKit" only in the extended menu (Shift + right-click).
    pub extended_menu_only: bool,
}

impl Default for FileLocksmith {
    fn default() -> Self {
        Self {
            enabled: true,
            extended_menu_only: false,
        }
    }
}
