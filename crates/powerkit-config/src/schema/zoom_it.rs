use super::{clamp, fix_color, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoomIt {
    pub enabled: bool,
    /// Freezes the screen and zooms in on the cursor; a click starts drawing on it.
    pub zoom_shortcut: String,
    /// Draws on the screen as it is.
    pub draw_shortcut: String,
    /// A countdown across the screen.
    pub break_shortcut: String,
    /// Zooms in on the screen while it keeps changing.
    pub live_zoom_shortcut: String,
    /// How far the first zoom goes, in percent (125-800).
    pub zoom_percent: u32,
    pub pen_color: String,
    /// Pen width in pixels (1-40).
    pub pen_width: u32,
    /// Length of the break countdown (1-99).
    pub break_minutes: u32,
}

impl Default for ZoomIt {
    fn default() -> Self {
        Self {
            enabled: true,
            zoom_shortcut: "Ctrl+1".into(),
            draw_shortcut: "Ctrl+2".into(),
            break_shortcut: "Ctrl+3".into(),
            live_zoom_shortcut: "Ctrl+4".into(),
            zoom_percent: 200,
            pen_color: "#FF0000".into(),
            pen_width: 4,
            break_minutes: 10,
        }
    }
}

impl ZoomIt {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = ZoomIt::default();
        fix_shortcut(&mut self.zoom_shortcut, &d.zoom_shortcut, "ZoomIt zoom", w);
        fix_shortcut(&mut self.draw_shortcut, &d.draw_shortcut, "ZoomIt draw", w);
        fix_shortcut(&mut self.break_shortcut, &d.break_shortcut, "ZoomIt break", w);
        fix_shortcut(
            &mut self.live_zoom_shortcut,
            &d.live_zoom_shortcut,
            "ZoomIt live zoom",
            w,
        );
        clamp(&mut self.zoom_percent, 125, 800, "zoom_it.zoom_percent", w);
        fix_color(&mut self.pen_color, &d.pen_color, "zoom_it.pen_color", w);
        clamp(&mut self.pen_width, 1, 40, "zoom_it.pen_width", w);
        clamp(&mut self.break_minutes, 1, 99, "zoom_it.break_minutes", w);
    }
}
