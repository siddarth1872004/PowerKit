use super::{fix_color, fix_shortcut};
use serde::{Deserialize, Serialize};

/// A second unit shown under the pixel measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulerUnit {
    #[default]
    None,
    Inches,
    Centimeters,
    Millimeters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenRuler {
    pub enabled: bool,
    /// Starts measuring; pressing it again, Escape or a right click stops.
    pub shortcut: String,
    /// How far (0-255 per color channel) a pixel may differ from the one under the
    /// cursor and still count as the same surface.
    pub tolerance: u8,
    /// Short bars across the ends of each measured line.
    pub draw_feet: bool,
    pub line_color: String,
    pub extra_unit: RulerUnit,
}

impl Default for ScreenRuler {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Ctrl+Shift+M".into(),
            tolerance: 30,
            draw_feet: true,
            line_color: "#FF4500".into(),
            extra_unit: RulerUnit::None,
        }
    }
}

impl ScreenRuler {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = ScreenRuler::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "Screen Ruler", w);
        fix_color(&mut self.line_color, &d.line_color, "screen_ruler.line_color", w);
    }
}
