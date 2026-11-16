use serde::{Deserialize, Serialize};

/// Off by default: turning it on edits the user's PowerShell profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandNotFound {
    pub enabled: bool,
}

impl CommandNotFound {
    pub(crate) fn sanitize(&mut self, _w: &mut Vec<String>) {}
}
