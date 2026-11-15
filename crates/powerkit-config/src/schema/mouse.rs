use super::{clamp, fix_apps, fix_color, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindMouseActivation {
    /// Press Ctrl twice quickly, with no other key in between.
    #[default]
    DoubleCtrl,
    /// Shake the mouse left and right.
    Shake,
    /// Only the shortcut, if one is set.
    ShortcutOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FindMyMouse {
    pub enabled: bool,
    pub activation: FindMouseActivation,
    /// Optional; works with every activation.
    pub shortcut: String,
    pub background_color: String,
    /// Opacity of the dimmed background, 10-100.
    pub opacity_percent: u32,
    /// Spotlight radius at 96 DPI, 20-500.
    pub radius: u32,
    /// The spotlight starts this many times larger and shrinks, 1-15.
    pub initial_zoom: u32,
    /// 0-5000 ms; 0 shows the final size at once.
    pub animation_ms: u32,
    /// Stay off while a full-screen game or other Direct3D program runs.
    pub skip_in_game_mode: bool,
    /// Lowercase executable names; stays off while one of them is in front.
    pub excluded_apps: Vec<String>,
}

impl Default for FindMyMouse {
    fn default() -> Self {
        Self {
            enabled: true,
            activation: FindMouseActivation::DoubleCtrl,
            shortcut: String::new(),
            background_color: "#000000".into(),
            opacity_percent: 50,
            radius: 100,
            initial_zoom: 9,
            animation_ms: 500,
            skip_in_game_mode: true,
            excluded_apps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseHighlighter {
    pub enabled: bool,
    /// Turns highlighting of clicks on and off.
    pub shortcut: String,
    pub left_color: String,
    pub right_color: String,
    /// 10-100.
    pub opacity_percent: u32,
    /// Circle radius at 96 DPI, 5-100.
    pub radius: u32,
    /// How long the circle stays after the button is released, 0-5000 ms.
    pub fade_delay_ms: u32,
    /// How long it takes to fade out, 0-5000 ms.
    pub fade_ms: u32,
}

impl Default for MouseHighlighter {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Shift+H".into(),
            left_color: "#FFFF00".into(),
            right_color: "#0000FF".into(),
            opacity_percent: 50,
            radius: 20,
            fade_delay_ms: 500,
            fade_ms: 250,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crosshairs {
    pub enabled: bool,
    /// Turns the crosshairs on and off.
    pub shortcut: String,
    pub color: String,
    /// 10-100.
    pub opacity_percent: u32,
    /// Line thickness at 96 DPI, 1-50.
    pub thickness: u32,
    /// Empty space around the pointer at 96 DPI, 0-500.
    pub center_gap: u32,
    /// Length of each arm at 96 DPI, 0-10000; 0 reaches the screen edges.
    pub length: u32,
    /// Outline drawn around the lines, so they stay visible on their own color.
    pub border_color: String,
    /// Outline width at 96 DPI, 0-50; 0 draws none.
    pub border_size: u32,
}

impl Default for Crosshairs {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Alt+P".into(),
            color: "#FF0000".into(),
            opacity_percent: 75,
            thickness: 5,
            center_gap: 20,
            length: 0,
            border_color: "#FFFFFF".into(),
            border_size: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseJump {
    pub enabled: bool,
    /// Shows a small picture of every screen; clicking it moves the pointer there.
    pub shortcut: String,
    /// Largest picture size at 96 DPI, 160-4000.
    pub max_width: u32,
    pub max_height: u32,
}

impl Default for MouseJump {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Shift+D".into(),
            max_width: 1600,
            max_height: 1200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseUtilities {
    pub enabled: bool,
    pub find_my_mouse: FindMyMouse,
    pub highlighter: MouseHighlighter,
    pub crosshairs: Crosshairs,
    pub jump: MouseJump,
}

impl Default for MouseUtilities {
    fn default() -> Self {
        Self {
            enabled: true,
            find_my_mouse: FindMyMouse::default(),
            highlighter: MouseHighlighter::default(),
            crosshairs: Crosshairs::default(),
            jump: MouseJump::default(),
        }
    }
}

impl MouseUtilities {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let (f, d) = (&mut self.find_my_mouse, FindMyMouse::default());
        fix_shortcut(&mut f.shortcut, &d.shortcut, "Find My Mouse", w);
        fix_color(
            &mut f.background_color,
            &d.background_color,
            "Find My Mouse background",
            w,
        );
        clamp(&mut f.opacity_percent, 10, 100, "Find My Mouse opacity", w);
        clamp(&mut f.radius, 20, 500, "Find My Mouse radius", w);
        clamp(&mut f.initial_zoom, 1, 15, "Find My Mouse initial zoom", w);
        clamp(&mut f.animation_ms, 0, 5000, "Find My Mouse animation", w);
        fix_apps(&mut f.excluded_apps);
        if f.activation == FindMouseActivation::ShortcutOnly && f.shortcut.is_empty() && f.enabled {
            w.push("Find My Mouse: \"shortcut only\" has no shortcut; using double Ctrl".into());
            f.activation = FindMouseActivation::DoubleCtrl;
        }

        let (h, d) = (&mut self.highlighter, MouseHighlighter::default());
        fix_shortcut(&mut h.shortcut, &d.shortcut, "Mouse Highlighter", w);
        fix_color(&mut h.left_color, &d.left_color, "Mouse Highlighter left button", w);
        fix_color(&mut h.right_color, &d.right_color, "Mouse Highlighter right button", w);
        clamp(&mut h.opacity_percent, 10, 100, "Mouse Highlighter opacity", w);
        clamp(&mut h.radius, 5, 100, "Mouse Highlighter radius", w);
        clamp(&mut h.fade_delay_ms, 0, 5000, "Mouse Highlighter fade delay", w);
        clamp(&mut h.fade_ms, 0, 5000, "Mouse Highlighter fade", w);

        let (c, d) = (&mut self.crosshairs, Crosshairs::default());
        fix_shortcut(&mut c.shortcut, &d.shortcut, "Crosshairs", w);
        fix_color(&mut c.color, &d.color, "Crosshairs color", w);
        clamp(&mut c.opacity_percent, 10, 100, "Crosshairs opacity", w);
        clamp(&mut c.thickness, 1, 50, "Crosshairs thickness", w);
        clamp(&mut c.center_gap, 0, 500, "Crosshairs center gap", w);
        clamp(&mut c.length, 0, 10_000, "Crosshairs length", w);
        fix_color(&mut c.border_color, &d.border_color, "Crosshairs border color", w);
        clamp(&mut c.border_size, 0, 50, "Crosshairs border size", w);

        let (j, d) = (&mut self.jump, MouseJump::default());
        fix_shortcut(&mut j.shortcut, &d.shortcut, "Mouse Jump", w);
        clamp(&mut j.max_width, 160, 4000, "Mouse Jump width", w);
        clamp(&mut j.max_height, 160, 4000, "Mouse Jump height", w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repairs_values() {
        let mut m = MouseUtilities::default();
        m.find_my_mouse.activation = FindMouseActivation::ShortcutOnly;
        m.find_my_mouse.radius = 1;
        m.highlighter.left_color = "yellow".into();
        m.crosshairs.thickness = 0;
        m.jump.shortcut = "D".into();
        m.find_my_mouse.excluded_apps = vec![" Game.EXE".into(), "game.exe".into()];
        let mut w = Vec::new();
        m.sanitize(&mut w);
        assert_eq!(m.find_my_mouse.activation, FindMouseActivation::DoubleCtrl);
        assert_eq!(m.find_my_mouse.radius, 20);
        assert_eq!(m.highlighter.left_color, "#FFFF00");
        assert_eq!(m.crosshairs.thickness, 1);
        assert_eq!(m.jump.shortcut, "Win+Shift+D");
        assert_eq!(m.find_my_mouse.excluded_apps, ["game.exe"]);
        assert_eq!(w.len(), 5, "{w:?}");
    }

    #[test]
    fn defaults_are_clean() {
        let mut m = MouseUtilities::default();
        let mut w = Vec::new();
        m.sanitize(&mut w);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(m, MouseUtilities::default());
    }
}
