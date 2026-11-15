use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropAndLock {
    pub enabled: bool,
    /// Crop the focused window into a live, read-only copy.
    pub thumbnail_shortcut: String,
    /// Crop the focused window by moving it into a smaller window that still takes input.
    pub reparent_shortcut: String,
}

impl Default for CropAndLock {
    fn default() -> Self {
        Self {
            enabled: true,
            thumbnail_shortcut: "Win+Ctrl+Shift+T".into(),
            reparent_shortcut: "Win+Ctrl+Shift+R".into(),
        }
    }
}

impl CropAndLock {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = CropAndLock::default();
        fix_shortcut(
            &mut self.thumbnail_shortcut,
            &d.thumbnail_shortcut,
            "Crop and Lock thumbnail",
            w,
        );
        fix_shortcut(
            &mut self.reparent_shortcut,
            &d.reparent_shortcut,
            "Crop and Lock reparent",
            w,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repairs_shortcuts() {
        let mut c = CropAndLock {
            thumbnail_shortcut: "T".into(),
            ..CropAndLock::default()
        };
        let mut w = Vec::new();
        c.sanitize(&mut w);
        assert_eq!(c, CropAndLock::default());
        assert_eq!(w.len(), 1, "{w:?}");
    }
}
