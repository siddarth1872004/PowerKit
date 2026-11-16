//! Screen Ruler: press the shortcut and the distance between the edges around the
//! cursor is shown, found where the color changes. The wheel switches between
//! measuring across, down, both, or a dragged box (or keys 1-4); a click copies the measurement.

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_config::{RulerUnit, ScreenRuler as Settings};
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "screen_ruler";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Spacing,
    Horizontal,
    Vertical,
    Bounds,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::Spacing, Mode::Horizontal, Mode::Vertical, Mode::Bounds];

    /// The next mode for a wheel turn; wheeling down goes forward.
    pub fn turn(self, wheel_delta: i32) -> Mode {
        let i = Mode::ALL.iter().position(|m| *m == self).unwrap_or(0) as i32;
        let step = if wheel_delta < 0 { 1 } else { -1 };
        Mode::ALL[(i + step).rem_euclid(Mode::ALL.len() as i32) as usize]
    }

    /// The mode for a number key: 1 bounds, 2 spacing, 3 horizontal, 4 vertical, as in
    /// the PowerToys toolbar.
    pub fn for_key(vk: u32) -> Option<Mode> {
        Some(match char::from_u32(vk)? {
            '1' => Mode::Bounds,
            '2' => Mode::Spacing,
            '3' => Mode::Horizontal,
            '4' => Mode::Vertical,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Spacing => "Spacing",
            Mode::Horizontal => "Horizontal",
            Mode::Vertical => "Vertical",
            Mode::Bounds => "Bounds: drag a box",
        }
    }
}

fn similar(a: u32, b: u32, tolerance: u8) -> bool {
    let t = tolerance as i32;
    [0, 8, 16]
        .iter()
        .all(|s| ((a >> s & 0xFF) as i32 - (b >> s & 0xFF) as i32).abs() <= t)
}

/// The run of pixels like the one at `(x, y)` along its row and column, in an image
/// `width` pixels wide given as `0x00RRGGBB` row by row. The edges are the first
/// pixels that differ by more than `tolerance`, or the image's sides.
pub fn span(pixels: &[u32], width: i32, x: i32, y: i32, tolerance: u8) -> Rect {
    let height = pixels.len() as i32 / width.max(1);
    if !(0..width).contains(&x) || !(0..height).contains(&y) {
        return Rect::new(x, y, x, y);
    }
    let at = |x: i32, y: i32| pixels[(y * width + x) as usize];
    let here = at(x, y);
    let walk = |mut p: i32, step: i32, limit: i32, get: &dyn Fn(i32) -> u32| {
        while p + step != limit && similar(get(p + step), here, tolerance) {
            p += step;
        }
        p
    };
    let left = walk(x, -1, -1, &|p| at(p, y));
    let right = walk(x, 1, width, &|p| at(p, y));
    let top = walk(y, -1, -1, &|p| at(x, p));
    let bottom = walk(y, 1, height, &|p| at(x, p));
    Rect::new(left, top, right + 1, bottom + 1)
}

/// The box between two drag points, both pixels included.
pub fn bounds(start: (i32, i32), end: (i32, i32)) -> Rect {
    Rect::new(
        start.0.min(end.0),
        start.1.min(end.1),
        start.0.max(end.0) + 1,
        start.1.max(end.1) + 1,
    )
}

/// What is measured: a width, a height, or both.
pub fn measured(mode: Mode, r: Rect) -> (Option<i32>, Option<i32>) {
    match mode {
        Mode::Spacing | Mode::Bounds => (Some(r.width()), Some(r.height())),
        Mode::Horizontal => (Some(r.width()), None),
        Mode::Vertical => (None, Some(r.height())),
    }
}

/// The lines to draw for `r` measured in `mode` through the cursor at `at`, each
/// `thickness` pixels wide, with feet across their ends when asked.
pub fn lines(mode: Mode, r: Rect, at: (i32, i32), thickness: i32, feet: bool) -> Vec<Rect> {
    let t = thickness.max(1);
    let foot = t * 6;
    // Centered on the cursor's pixel.
    let (x0, y0) = (at.0 - (t - 1) / 2, at.1 - (t - 1) / 2);
    let across = || {
        let mut v = vec![Rect::new(r.left, y0, r.right, y0 + t)];
        if feet {
            let top = at.1 - foot / 2;
            v.push(Rect::new(r.left, top, r.left + t, top + foot));
            v.push(Rect::new(r.right - t, top, r.right, top + foot));
        }
        v
    };
    let down = || {
        let mut v = vec![Rect::new(x0, r.top, x0 + t, r.bottom)];
        if feet {
            let left = at.0 - foot / 2;
            v.push(Rect::new(left, r.top, left + foot, r.top + t));
            v.push(Rect::new(left, r.bottom - t, left + foot, r.bottom));
        }
        v
    };
    match mode {
        Mode::Horizontal => across(),
        Mode::Vertical => down(),
        Mode::Spacing => [across(), down()].concat(),
        Mode::Bounds => vec![
            Rect::new(r.left, r.top, r.right, r.top + t),
            Rect::new(r.left, r.bottom - t, r.right, r.bottom),
            Rect::new(r.left, r.top, r.left + t, r.bottom),
            Rect::new(r.right - t, r.top, r.right, r.bottom),
        ],
    }
}

/// `px` in `unit` on a monitor scaled to `dpi`, and the unit's short name.
fn convert(px: i32, unit: RulerUnit, dpi: u32) -> Option<(f64, &'static str)> {
    let inches = px as f64 / dpi.max(1) as f64;
    match unit {
        RulerUnit::None => None,
        RulerUnit::Inches => Some((inches, "in")),
        RulerUnit::Centimeters => Some((inches * 2.54, "cm")),
        RulerUnit::Millimeters => Some((inches * 25.4, "mm")),
    }
}

/// The measurement in pixels ("120 × 40 px"), and in `unit` when one is asked for.
pub fn label(size: (Option<i32>, Option<i32>), unit: RulerUnit, dpi: u32) -> (String, Option<String>) {
    let parts: Vec<i32> = [size.0, size.1].into_iter().flatten().collect();
    let join = |v: Vec<String>| v.join(" \u{00D7} ");
    let px = format!("{} px", join(parts.iter().map(i32::to_string).collect()));
    let other = parts
        .iter()
        .map(|p| convert(*p, unit, dpi))
        .collect::<Option<Vec<_>>>()
        .filter(|v| !v.is_empty())
        .map(|v| {
            let name = v[0].1;
            let digits = if unit == RulerUnit::Millimeters { 1 } else { 2 };
            format!(
                "{} {name}",
                join(v.iter().map(|(n, _)| format!("{n:.digits$}")).collect())
            )
        });
    (px, other)
}

#[derive(Default)]
pub struct ScreenRuler {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: live::Live,
}

impl ScreenRuler {
    pub fn new() -> Self {
        Self::default()
    }

    fn toggle(&mut self) -> Result<()> {
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Screen Ruler is not running");
        }
        #[cfg(windows)]
        self.live.toggle()?;
        Ok(())
    }

    fn cancel(&mut self) {
        #[cfg(windows)]
        self.live.cancel();
    }
}

impl Module for ScreenRuler {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Screen Ruler".into(),
            description: "Measure the pixels between edges on screen.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::MouseHook,
                Permission::ClipboardWrite,
            ],
            commands: vec![CommandSpec {
                id: "measure".into(),
                title: "Start or stop measuring".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.screen_ruler.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        self.cancel();
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        self.cancel();
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.screen_ruler.clone();
        if !self.settings.enabled {
            self.cancel();
        }
        #[cfg(windows)]
        self.live.redraw();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        match action {
            "measure" => self.toggle(),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        self.live.on_event(event, &self.settings, ctx)?;
        #[cfg(not(windows))]
        let _ = (event, ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        match id {
            "measure" => self.toggle()?,
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let measuring = self.live.measuring();
        #[cfg(not(windows))]
        let measuring = false;
        json!({ "running": self.running && self.settings.enabled, "measuring": measuring })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::Config;

    /// A 10x6 white image with a gray box from (2, 1) to (7, 4) inclusive, its left
    /// column slightly lighter, as antialiasing leaves it.
    fn picture() -> Vec<u32> {
        let mut p = vec![0x00FF_FFFF; 60];
        for y in 1..=4 {
            for x in 2..=7 {
                p[y * 10 + x] = if x == 2 { 0x0090_9090 } else { 0x0080_8080 };
            }
        }
        p
    }

    #[test]
    fn spans_stop_where_the_color_changes() {
        let p = picture();
        assert_eq!(span(&p, 10, 4, 2, 30), Rect::new(2, 1, 8, 5));
        assert_eq!(
            span(&p, 10, 4, 2, 0),
            Rect::new(3, 1, 8, 5),
            "exact match stops at the light column"
        );
        assert_eq!(
            span(&p, 10, 0, 0, 30),
            Rect::new(0, 0, 10, 6),
            "white runs to the image sides"
        );
        assert_eq!(span(&p, 10, 0, 3, 30), Rect::new(0, 0, 2, 6));
        assert_eq!(span(&p, 10, 12, 3, 30), Rect::new(12, 3, 12, 3), "outside");
    }

    #[test]
    fn bounds_include_both_points() {
        assert_eq!(bounds((10, 40), (4, 20)), Rect::new(4, 20, 11, 41));
        assert_eq!(bounds((5, 5), (5, 5)).width(), 1);
    }

    #[test]
    fn the_wheel_cycles_the_modes() {
        assert_eq!(Mode::Spacing.turn(-120), Mode::Horizontal);
        assert_eq!(Mode::Bounds.turn(-120), Mode::Spacing);
        assert_eq!(Mode::Spacing.turn(120), Mode::Bounds);
        assert_eq!(Mode::for_key('1' as u32), Some(Mode::Bounds));
        assert_eq!(Mode::for_key('4' as u32), Some(Mode::Vertical));
        assert_eq!(Mode::for_key('5' as u32), None);
    }

    #[test]
    fn lines_follow_the_mode() {
        let r = Rect::new(100, 50, 300, 150);
        assert_eq!(
            lines(Mode::Horizontal, r, (180, 90), 1, false),
            [Rect::new(100, 90, 300, 91)]
        );
        assert_eq!(
            lines(Mode::Vertical, r, (180, 90), 2, false),
            [Rect::new(180, 50, 182, 150)]
        );
        let feet = lines(Mode::Horizontal, r, (180, 90), 1, true);
        assert_eq!(&feet[1..], [Rect::new(100, 87, 101, 93), Rect::new(299, 87, 300, 93)]);
        assert_eq!(lines(Mode::Spacing, r, (180, 90), 1, true).len(), 6);
        assert_eq!(lines(Mode::Bounds, r, (0, 0), 1, true).len(), 4);
    }

    #[test]
    fn labels_show_pixels_and_the_extra_unit() {
        let r = Rect::new(0, 0, 192, 48);
        assert_eq!(
            label(measured(Mode::Spacing, r), RulerUnit::None, 96),
            ("192 \u{00D7} 48 px".into(), None)
        );
        assert_eq!(label(measured(Mode::Vertical, r), RulerUnit::None, 96).0, "48 px");
        assert_eq!(
            label(measured(Mode::Spacing, r), RulerUnit::Inches, 96).1.as_deref(),
            Some("2.00 \u{00D7} 0.50 in")
        );
        assert_eq!(
            label(measured(Mode::Horizontal, r), RulerUnit::Millimeters, 192)
                .1
                .as_deref(),
            Some("25.4 mm")
        );
        assert_eq!(
            label(measured(Mode::Horizontal, r), RulerUnit::Centimeters, 96)
                .1
                .as_deref(),
            Some("5.08 cm")
        );
    }

    #[test]
    fn the_shortcut_is_registered() {
        let mut c = Config::new();
        let ours: Vec<_> = c.shortcuts().into_iter().filter(|(m, _, _)| *m == ID).collect();
        assert_eq!(ours, [(ID, "measure".to_string(), "Win+Ctrl+Shift+M".to_string())]);
        assert!(c.sanitize().is_empty());
    }
}
