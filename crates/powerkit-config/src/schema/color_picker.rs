use super::{clamp, fix_shortcut};
use serde::{Deserialize, Serialize};

/// Text representations the color picker can copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorFormat {
    /// `#RRGGBB`
    #[default]
    Hex,
    /// `#RRGGBBAA`
    HexAlpha,
    /// `rgb(r, g, b)`
    Rgb,
    /// `rgba(r, g, b, a)`
    Rgba,
    /// `hsl(h, s%, l%)`
    Hsl,
    /// `hsv(h, s%, v%)`
    Hsv,
    /// `cmyk(c%, m%, y%, k%)`
    Cmyk,
    /// CSS Color 4 space-separated syntax: `rgb(r g b)`
    CssRgb,
    /// CSS Color 4 space-separated syntax: `hsl(h s% l%)`
    CssHsl,
    /// The user-defined template in `ColorPicker::custom_format`.
    Custom,
}

impl ColorFormat {
    pub const ALL: [ColorFormat; 10] = [
        ColorFormat::Hex,
        ColorFormat::HexAlpha,
        ColorFormat::Rgb,
        ColorFormat::Rgba,
        ColorFormat::Hsl,
        ColorFormat::Hsv,
        ColorFormat::Cmyk,
        ColorFormat::CssRgb,
        ColorFormat::CssHsl,
        ColorFormat::Custom,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorPicker {
    pub enabled: bool,
    pub shortcut: String,
    /// Format placed on the clipboard when a color is picked.
    pub copy_format: ColorFormat,
    /// Template for [`ColorFormat::Custom`]; see `color_picker::format_custom`.
    pub custom_format: String,
    /// Screen pixels shown on each side of the cursor in the magnifier (2-15).
    pub magnifier_radius: u32,
    /// Size of one magnified pixel, in device pixels (4-24).
    pub magnifier_zoom: u32,
    pub show_grid: bool,
    /// Number of picked colors kept in the history (0 disables history).
    pub history_limit: u32,
}

impl Default for ColorPicker {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Shift+C".into(),
            copy_format: ColorFormat::Hex,
            custom_format: "%R, %G, %B".into(),
            magnifier_radius: 7,
            magnifier_zoom: 10,
            show_grid: true,
            history_limit: 40,
        }
    }
}

impl ColorPicker {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = Self::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "Color Picker", w);
        clamp(&mut self.magnifier_radius, 2, 15, "Color Picker magnifier radius", w);
        clamp(&mut self.magnifier_zoom, 4, 24, "Color Picker magnifier zoom", w);
        clamp(&mut self.history_limit, 0, 500, "Color Picker history limit", w);
        if self.custom_format.chars().count() > 200 {
            w.push("Color Picker: custom format is too long; restored default".into());
            self.custom_format = d.custom_format;
        }
    }
}
