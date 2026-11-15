use super::{clamp, fix_apps, fix_shortcut, Theme};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutGuide {
    pub enabled: bool,
    /// Shows the guide until it is pressed again or Esc.
    pub shortcut: String,
    /// Holding the Windows key down also shows the guide, until it is released.
    pub hold_win: bool,
    /// How long the Windows key must be held before the guide appears.
    pub press_time_ms: u32,
    /// Opacity of the dimmed background.
    pub opacity_percent: u32,
    pub theme: Theme,
    /// Executable names (e.g. "game.exe") whose windows never show the guide.
    pub excluded_apps: Vec<String>,
    /// Number the taskbar buttons that Win+1..Win+0 open.
    pub show_taskbar_numbers: bool,
}

impl Default for ShortcutGuide {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Shift+Slash".into(),
            hold_win: true,
            press_time_ms: 900,
            opacity_percent: 60,
            theme: Theme::System,
            excluded_apps: Vec::new(),
            show_taskbar_numbers: true,
        }
    }
}

impl ShortcutGuide {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = ShortcutGuide::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "Shortcut Guide", w);
        clamp(&mut self.press_time_ms, 100, 10_000, "Shortcut Guide press time", w);
        clamp(&mut self.opacity_percent, 0, 100, "Shortcut Guide opacity", w);
        fix_apps(&mut self.excluded_apps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repairs_values() {
        let mut c = ShortcutGuide {
            shortcut: "Slash".into(),
            press_time_ms: 5,
            ..ShortcutGuide::default()
        };
        let mut w = Vec::new();
        c.sanitize(&mut w);
        assert_eq!(c.shortcut, "Win+Shift+Slash");
        assert_eq!(c.press_time_ms, 100);
        assert_eq!(w.len(), 2, "{w:?}");
    }
}
