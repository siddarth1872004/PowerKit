use super::{clamp, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowTiling {
    pub enabled: bool,
    /// Left half; pressed again, a third, then two thirds.
    pub left_shortcut: String,
    /// Right half; pressed again, a third, then two thirds.
    pub right_shortcut: String,
    /// From a half, the top quarter of that side; otherwise maximized.
    pub up_shortcut: String,
    /// From a half, the bottom quarter of that side; from maximized, back to its size
    /// before; otherwise centered.
    pub down_shortcut: String,
    /// Space left around tiled windows, in pixels at 100% scaling.
    pub gap: u32,
}

impl Default for WindowTiling {
    fn default() -> Self {
        Self {
            enabled: true,
            left_shortcut: "Win+Alt+Left".into(),
            right_shortcut: "Win+Alt+Right".into(),
            up_shortcut: "Win+Alt+Up".into(),
            down_shortcut: "Win+Alt+Down".into(),
            gap: 0,
        }
    }
}

impl WindowTiling {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = WindowTiling::default();
        fix_shortcut(&mut self.left_shortcut, &d.left_shortcut, "Window Tiling", w);
        fix_shortcut(&mut self.right_shortcut, &d.right_shortcut, "Window Tiling", w);
        fix_shortcut(&mut self.up_shortcut, &d.up_shortcut, "Window Tiling", w);
        fix_shortcut(&mut self.down_shortcut, &d.down_shortcut, "Window Tiling", w);
        clamp(&mut self.gap, 0, 50, "window_tiling.gap", w);
    }
}
