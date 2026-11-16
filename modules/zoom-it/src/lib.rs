//! ZoomIt: zoom in on the screen and draw on it while presenting, and a break timer.
//! Zoom freezes the screen and follows the cursor, the wheel zooms, and a click starts
//! drawing on what is shown; live zoom keeps the screen moving underneath.

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_config::ZoomIt as Settings;
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "zoom_it";
pub const MAX_ZOOM: f64 = 32.0;
/// Each wheel notch zooms by this much.
const ZOOM_STEP: f64 = 1.25;

/// The part of a `size` screen to show enlarged `zoom` times so the point under the
/// cursor stays where it is: the screen's edges line up with the view's edges when
/// the cursor reaches them.
pub fn view(size: (i32, i32), cursor: (i32, i32), zoom: f64) -> Rect {
    let zoom = zoom.clamp(1.0, MAX_ZOOM);
    let (w, h) = (
        ((size.0 as f64 / zoom).round() as i32).max(1),
        ((size.1 as f64 / zoom).round() as i32).max(1),
    );
    let at = |c: i32, side: i32, len: i32| {
        let c = c.clamp(0, side) as f64;
        ((c - c / zoom).round() as i32).clamp(0, side - len)
    };
    Rect::from_size(at(cursor.0, size.0, w), at(cursor.1, size.1, h), w, h)
}

/// `area` of a picture `width` pixels wide, enlarged to `out` with square pixels.
pub fn magnify(src: &[u32], width: i32, area: Rect, out: (i32, i32)) -> Vec<u32> {
    let height = src.len() as i32 / width.max(1);
    let (ow, oh) = (out.0.max(1), out.1.max(1));
    let cols: Vec<usize> = (0..ow)
        .map(|x| (area.left + x * area.width() / ow).clamp(0, width - 1) as usize)
        .collect();
    let mut pixels = Vec::with_capacity((ow * oh) as usize);
    for y in 0..oh {
        let sy = (area.top + y * area.height() / oh).clamp(0, height - 1) as usize;
        let row = &src[sy * width as usize..];
        pixels.extend(cols.iter().map(|x| row[*x]));
    }
    pixels
}

/// The zoom after turning the wheel by `delta` (120 per notch; up zooms in).
pub fn wheel_zoom(zoom: f64, delta: i32) -> f64 {
    (zoom * ZOOM_STEP.powf(delta as f64 / 120.0)).clamp(1.0, MAX_ZOOM)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Free(Vec<(i32, i32)>),
    Line((i32, i32), (i32, i32)),
    Rect((i32, i32), (i32, i32)),
    Arrow((i32, i32), (i32, i32)),
}

impl Shape {
    /// A new shape at `at`: Shift draws a line, Ctrl a box, both an arrow.
    pub fn start(at: (i32, i32), shift: bool, ctrl: bool) -> Shape {
        match (shift, ctrl) {
            (false, false) => Shape::Free(vec![at]),
            (true, false) => Shape::Line(at, at),
            (false, true) => Shape::Rect(at, at),
            (true, true) => Shape::Arrow(at, at),
        }
    }

    /// The pen moved to `to`.
    pub fn extend(&mut self, to: (i32, i32)) {
        match self {
            Shape::Free(points) => {
                if points.last() != Some(&to) {
                    points.push(to);
                }
            }
            Shape::Line(_, b) | Shape::Rect(_, b) | Shape::Arrow(_, b) => *b = to,
        }
    }

    fn segments(&self, width: i32) -> Vec<((i32, i32), (i32, i32))> {
        match self {
            Shape::Free(p) if p.len() == 1 => vec![(p[0], p[0])],
            Shape::Free(p) => p.windows(2).map(|w| (w[0], w[1])).collect(),
            Shape::Line(a, b) => vec![(*a, *b)],
            Shape::Rect(a, b) => {
                let (c, d) = ((b.0, a.1), (a.0, b.1));
                vec![(*a, c), (c, *b), (*b, d), (d, *a)]
            }
            Shape::Arrow(a, b) => {
                let (dx, dy) = ((a.0 - b.0) as f64, (a.1 - b.1) as f64);
                let len = dx.hypot(dy);
                let mut v = vec![(*a, *b)];
                if len >= 1.0 {
                    let head = (width as f64 * 3.0 + 10.0).min(len);
                    for turn in [0.5f64, -0.5] {
                        let (s, c) = turn.sin_cos();
                        let (ux, uy) = ((dx * c - dy * s) / len, (dx * s + dy * c) / len);
                        let tip = (b.0 + (ux * head).round() as i32, b.1 + (uy * head).round() as i32);
                        v.push((*b, tip));
                    }
                }
                v
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub shape: Shape,
    /// `0x00RRGGBB`
    pub color: u32,
    pub width: i32,
}

/// Paint `stroke` onto a picture `width` pixels wide with round pen ends.
pub fn paint(pixels: &mut [u32], width: i32, stroke: &Stroke) {
    let height = pixels.len() as i32 / width.max(1);
    let r = stroke.width.max(1) as f64 / 2.0;
    for (a, b) in stroke.shape.segments(stroke.width) {
        let pad = r.ceil() as i32;
        let (x0, x1) = ((a.0.min(b.0) - pad).max(0), (a.0.max(b.0) + pad).min(width - 1));
        let (y0, y1) = ((a.1.min(b.1) - pad).max(0), (a.1.max(b.1) + pad).min(height - 1));
        let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
        let len2 = dx * dx + dy * dy;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = ((x - a.0) as f64, (y - a.1) as f64);
                let t = if len2 == 0.0 {
                    0.0
                } else {
                    ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
                };
                let (ex, ey) = (px - t * dx, py - t * dy);
                // Measured from pixel centres, so a 1-pixel pen still covers its pixel.
                if ex * ex + ey * ey <= r * r + 0.25 {
                    pixels[(y * width + x) as usize] = stroke.color;
                }
            }
        }
    }
}

/// The pen color for a key, as in ZoomIt: Red, Green, Blue, Yellow, Orange, Pink.
pub fn pen_color(vk: u32) -> Option<u32> {
    Some(match char::from_u32(vk)? {
        'R' => 0x00FF_0000,
        'G' => 0x0000_C000,
        'B' => 0x0000_60FF,
        'Y' => 0x00FF_E000,
        'O' => 0x00FF_8000,
        'P' => 0x00FF_40C0,
        _ => return None,
    })
}

pub fn parse_color(hex: &str) -> u32 {
    u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x00FF_0000) & 0x00FF_FFFF
}

/// Minutes and seconds left, rounded up so the timer shows 0:00 only when it ends.
pub fn countdown(left: std::time::Duration) -> String {
    let s = left.as_millis().div_ceil(1000);
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Default)]
pub struct ZoomIt {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: live::Live,
}

impl ZoomIt {
    pub fn new() -> Self {
        Self::default()
    }

    fn act(&mut self, action: &str) -> Result<()> {
        if !matches!(action, "zoom" | "draw" | "break" | "live_zoom") {
            bail!("unknown action {action:?}");
        }
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("ZoomIt is not running");
        }
        #[cfg(windows)]
        self.live.act(action, &self.settings)?;
        Ok(())
    }

    fn cancel(&mut self) {
        #[cfg(windows)]
        self.live.cancel();
    }
}

impl Module for ZoomIt {
    fn manifest(&self) -> Manifest {
        let command = |id: &str, title: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        };
        Manifest {
            id: ID.into(),
            name: "ZoomIt".into(),
            description: "Zoom in on the screen and draw on it while presenting.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys, Permission::MouseHook],
            commands: vec![
                command("zoom", "Zoom in, or stop"),
                command("draw", "Draw on the screen, or stop"),
                command("break", "Start or end a break timer"),
                command("live_zoom", "Live zoom, or stop"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.zoom_it.clone();
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
        self.settings = ctx.config.zoom_it.clone();
        if !self.settings.enabled {
            self.cancel();
        }
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        self.act(action)
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        self.live.on_event(event, &self.settings, ctx)?;
        #[cfg(not(windows))]
        let _ = (event, ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        self.act(id)?;
        Ok(self.status())
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let active = self.live.active();
        #[cfg(not(windows))]
        let active: Option<&str> = None;
        json!({ "running": self.running && self.settings.enabled, "active": active })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::Config;
    use std::time::Duration;

    #[test]
    fn the_view_keeps_the_point_under_the_cursor() {
        let size = (1920, 1080);
        assert_eq!(view(size, (0, 0), 2.0), Rect::new(0, 0, 960, 540));
        assert_eq!(view(size, (1920, 1080), 2.0), Rect::new(960, 540, 1920, 1080));
        let v = view(size, (960, 540), 2.0);
        assert_eq!(v, Rect::new(480, 270, 1440, 810));
        // Shown at 2x, (960, 540) of the view's origin lands on the cursor again.
        assert_eq!((v.left + 960 / 2, v.top + 540 / 2), (960, 540));
        assert_eq!(view(size, (500, 500), 1.0), Rect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn magnify_repeats_pixels() {
        let src = [1, 2, 3, 4];
        assert_eq!(
            magnify(&src, 2, Rect::new(0, 0, 2, 2), (4, 2)),
            [1, 1, 2, 2, 3, 3, 4, 4]
        );
        assert_eq!(magnify(&src, 2, Rect::new(1, 1, 2, 2), (2, 2)), [4, 4, 4, 4]);
    }

    #[test]
    fn the_wheel_zooms_in_steps() {
        assert_eq!(wheel_zoom(2.0, 120), 2.5);
        assert_eq!(wheel_zoom(2.5, -120), 2.0);
        assert_eq!(wheel_zoom(1.1, -240), 1.0);
        assert_eq!(wheel_zoom(30.0, 240), MAX_ZOOM);
    }

    fn drawn(shape: Shape, width: i32) -> Vec<u32> {
        let mut p = vec![0; 100];
        paint(&mut p, 10, &Stroke { shape, color: 7, width });
        p
    }

    fn set(p: &[u32]) -> Vec<(i32, i32)> {
        (0..100)
            .filter(|i| p[*i as usize] == 7)
            .map(|i| (i % 10, i / 10))
            .collect()
    }

    #[test]
    fn shapes_are_painted() {
        assert_eq!(
            set(&drawn(Shape::Line((1, 2), (4, 2)), 1)),
            [(1, 2), (2, 2), (3, 2), (4, 2)]
        );
        assert_eq!(set(&drawn(Shape::Free(vec![(5, 5)]), 1)), [(5, 5)]);
        assert_eq!(set(&drawn(Shape::Free(vec![(5, 5)]), 3)).len(), 9);
        let boxed = set(&drawn(Shape::Rect((2, 2), (5, 4)), 1));
        assert_eq!(boxed.len(), 10, "{boxed:?}");
        assert!(!boxed.contains(&(3, 3)), "the inside stays clear");
        // Off the picture is clipped, not a panic.
        assert_eq!(set(&drawn(Shape::Line((-5, 0), (20, 0)), 1)).len(), 10);
    }

    #[test]
    fn arrows_have_heads() {
        let mut s = Shape::start((1, 5), true, true);
        s.extend((8, 5));
        assert_eq!(s.segments(1).len(), 3);
        let tips: Vec<_> = s.segments(1)[1..].iter().map(|(_, t)| *t).collect();
        assert!(tips.iter().all(|t| t.0 < 8), "{tips:?}");
        assert!(tips[0].1 != tips[1].1);
    }

    #[test]
    fn modifiers_pick_the_shape() {
        assert!(matches!(Shape::start((0, 0), false, false), Shape::Free(_)));
        assert!(matches!(Shape::start((0, 0), true, false), Shape::Line(..)));
        assert!(matches!(Shape::start((0, 0), false, true), Shape::Rect(..)));
        let mut f = Shape::start((0, 0), false, false);
        f.extend((0, 0));
        f.extend((1, 1));
        assert_eq!(f, Shape::Free(vec![(0, 0), (1, 1)]));
    }

    #[test]
    fn colors_and_countdown() {
        assert_eq!(pen_color('R' as u32), Some(0x00FF_0000));
        assert_eq!(pen_color('Q' as u32), None);
        assert_eq!(parse_color("#00FF80"), 0x0000_FF80);
        assert_eq!(countdown(Duration::from_secs(600)), "10:00");
        assert_eq!(countdown(Duration::from_millis(59_001)), "1:00");
        assert_eq!(countdown(Duration::ZERO), "0:00");
    }

    #[test]
    fn the_shortcuts_are_registered() {
        let mut c = Config::new();
        let ours: Vec<_> = c
            .shortcuts()
            .into_iter()
            .filter(|(m, _, _)| *m == ID)
            .map(|(_, a, s)| (a, s))
            .collect();
        assert_eq!(
            ours,
            [
                ("zoom".to_string(), "Ctrl+1".to_string()),
                ("draw".into(), "Ctrl+2".into()),
                ("break".into(), "Ctrl+3".into()),
                ("live_zoom".into(), "Ctrl+4".into()),
            ]
        );
        assert!(c.sanitize().is_empty());
    }
}
