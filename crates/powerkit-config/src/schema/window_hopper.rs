use super::{fix_apps, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowHopper {
    pub enabled: bool,
    /// Switch to the next window of the focused app.
    pub next_shortcut: String,
    pub previous_shortcut: String,
    /// Also switch to minimized windows, restoring them.
    pub include_minimized: bool,
    /// Executable names whose windows are left to the app's own shortcuts.
    pub excluded_apps: Vec<String>,
}

impl Default for WindowHopper {
    fn default() -> Self {
        Self {
            enabled: true,
            next_shortcut: "Alt+Backtick".into(),
            previous_shortcut: "Alt+Shift+Backtick".into(),
            include_minimized: true,
            excluded_apps: Vec::new(),
        }
    }
}

impl WindowHopper {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = WindowHopper::default();
        fix_shortcut(&mut self.next_shortcut, &d.next_shortcut, "Window Hopper next", w);
        fix_shortcut(
            &mut self.previous_shortcut,
            &d.previous_shortcut,
            "Window Hopper previous",
            w,
        );
        fix_apps(&mut self.excluded_apps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repairs_values() {
        let mut c = WindowHopper {
            next_shortcut: "Backtick".into(),
            excluded_apps: vec![" Code.exe".into()],
            ..WindowHopper::default()
        };
        let mut w = Vec::new();
        c.sanitize(&mut w);
        assert_eq!(c.next_shortcut, "Alt+Backtick");
        assert_eq!(c.excluded_apps, ["code.exe"]);
        assert_eq!(w.len(), 1, "{w:?}");
    }
}
