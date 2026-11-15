//! The animated walkthrough shown for each utility in settings: a small desktop
//! drawn in code, showing the tool being used with the user's own shortcuts.
//!
//! A walkthrough is a function of time that returns a [`Frame`]: shapes in a
//! [`WIDTH`] x [`HEIGHT`] space, painted in order, and a caption for the current
//! step. Drawing the shapes is left to the platform layer.

mod scenes;

use powerkit_config::Config;

pub const WIDTH: f32 = 480.0;
pub const HEIGHT: f32 = 270.0;

/// 0xAARRGGBB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color(0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
    }

    pub fn alpha(self, a: f32) -> Color {
        let a = (a.clamp(0.0, 1.0) * ((self.0 >> 24) as f32)).round() as u32;
        Color(a << 24 | (self.0 & 0x00FF_FFFF))
    }

    /// Part way from `self` to `other`.
    pub fn mix(self, other: Color, p: f32) -> Color {
        let p = p.clamp(0.0, 1.0);
        let ch = |c: Color, s: u32| ((c.0 >> s) & 0xFF) as f32;
        let at = |s: u32| ((ch(self, s) + (ch(other, s) - ch(self, s)) * p).round() as u32) << s;
        Color(at(24) | at(16) | at(8) | at(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        fill: Option<Color>,
        stroke: Option<(Color, f32)>,
    },
    Ellipse {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Color>,
        stroke: Option<(Color, f32)>,
    },
    Line {
        from: (f32, f32),
        to: (f32, f32),
        color: Color,
        width: f32,
    },
    Polygon {
        points: Vec<(f32, f32)>,
        fill: Color,
        stroke: Option<(Color, f32)>,
    },
    /// One line of text, vertically centred in its box.
    Text {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        text: String,
        size: f32,
        color: Color,
        align: Align,
        bold: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub shapes: Vec<Shape>,
    pub caption: String,
}

/// A utility's walkthrough. It loops every `duration` seconds.
#[derive(Clone, Copy)]
pub struct Walkthrough {
    pub duration: f32,
    draw: fn(&mut Canvas, f32, &Config),
}

impl Walkthrough {
    /// The picture `t` seconds in; `t` wraps around.
    pub fn frame(&self, t: f32, config: &Config) -> Frame {
        let mut c = Canvas::default();
        (self.draw)(&mut c, t.rem_euclid(self.duration), config);
        Frame {
            shapes: c.shapes,
            caption: c.caption,
        }
    }
}

/// The walkthrough for module `id`, if it has one.
pub fn walkthrough(id: &str) -> Option<Walkthrough> {
    scenes::ALL
        .iter()
        .find(|(m, _, _)| *m == id)
        .map(|&(_, duration, draw)| Walkthrough { duration, draw })
}

/// How far (0 to 1) `t` is through the span from `a` to `b`.
pub fn span(t: f32, a: f32, b: f32) -> f32 {
    ((t - a) / (b - a)).clamp(0.0, 1.0)
}

/// Smooth start and stop.
pub fn ease(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

pub fn lerp(a: f32, b: f32, p: f32) -> f32 {
    a + (b - a) * p
}

/// Position along `keys` (`(time, x, y)`, in time order) at `t`, easing between
/// them and holding still before the first and after the last.
pub fn path(t: f32, keys: &[(f32, f32, f32)]) -> (f32, f32) {
    let Some(&(_, x, y)) = keys.first() else {
        return (0.0, 0.0);
    };
    let mut at = (x, y);
    for pair in keys.windows(2) {
        let ((t0, x0, y0), (t1, x1, y1)) = (pair[0], pair[1]);
        if t >= t0 {
            let p = ease(span(t, t0, t1));
            at = (lerp(x0, x1, p), lerp(y0, y1, p));
        }
    }
    at
}

/// The part of `text` typed by `t`, starting at `start`, `per_char` seconds a key.
pub fn typed(text: &str, t: f32, start: f32, per_char: f32) -> &str {
    let n = if t < start {
        0
    } else {
        ((t - start) / per_char) as usize + 1
    };
    match text.char_indices().nth(n) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// A key's name as printed on its cap.
pub fn key_label(name: &str) -> String {
    match name {
        "Slash" => "/".into(),
        "Backtick" => "`".into(),
        "Period" => ".".into(),
        "Comma" => ",".into(),
        "Left" => "\u{2190}".into(),
        "Right" => "\u{2192}".into(),
        "Up" => "\u{2191}".into(),
        "Down" => "\u{2193}".into(),
        "Return" => "Enter".into(),
        other => other.into(),
    }
}

pub(crate) mod palette {
    use super::Color;
    pub const WALL_TOP: Color = Color::rgb(58, 108, 176);
    pub const WALL_BOTTOM: Color = Color::rgb(112, 84, 170);
    pub const TASKBAR: Color = Color(0xE01C1F26);
    pub const WINDOW: Color = Color::rgb(249, 250, 252);
    pub const TITLE: Color = Color::rgb(238, 241, 246);
    pub const TEXT: Color = Color::rgb(32, 34, 38);
    pub const MUTED: Color = Color::rgb(110, 116, 128);
    pub const LINE: Color = Color::rgb(214, 219, 228);
    pub const ACCENT: Color = Color::rgb(46, 116, 236);
    pub const ACCENT_SOFT: Color = Color::rgb(214, 228, 252);
    pub const DARK_WINDOW: Color = Color::rgb(40, 42, 48);
    pub const DARK_TITLE: Color = Color::rgb(30, 32, 37);
    pub const DARK_TEXT: Color = Color::rgb(232, 234, 238);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const SHADOW: Color = Color(0x30000000);
    pub const YELLOW: Color = Color::rgb(255, 214, 92);
}

use palette::*;

/// Collects the shapes of one frame, with helpers for the pieces every scene uses.
#[derive(Default)]
pub struct Canvas {
    pub shapes: Vec<Shape>,
    pub caption: String,
}

pub const TITLE_H: f32 = 18.0;
pub const MENU_ROW: f32 = 18.0;

impl Canvas {
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Color) {
        self.shapes.push(Shape::Rect {
            x,
            y,
            w,
            h,
            radius,
            fill: Some(fill),
            stroke: None,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color, width: f32) {
        self.shapes.push(Shape::Rect {
            x,
            y,
            w,
            h,
            radius,
            fill: None,
            stroke: Some((color, width)),
        });
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Color) {
        self.shapes.push(Shape::Ellipse {
            x: cx - r,
            y: cy - r,
            w: r * 2.0,
            h: r * 2.0,
            fill: Some(fill),
            stroke: None,
        });
    }

    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, color: Color, width: f32) {
        self.shapes.push(Shape::Ellipse {
            x: cx - r,
            y: cy - r,
            w: r * 2.0,
            h: r * 2.0,
            fill: None,
            stroke: Some((color, width)),
        });
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: Color, width: f32) {
        self.shapes.push(Shape::Line {
            from: (x1, y1),
            to: (x2, y2),
            color,
            width,
        });
    }

    pub fn polygon(&mut self, points: Vec<(f32, f32)>, fill: Color, stroke: Option<(Color, f32)>) {
        self.shapes.push(Shape::Polygon { points, fill, stroke });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_box(&mut self, x: f32, y: f32, w: f32, h: f32, text: &str, size: f32, color: Color, align: Align) {
        self.shapes.push(Shape::Text {
            x,
            y,
            w,
            h,
            text: text.into(),
            size,
            color,
            align,
            bold: false,
        });
    }

    /// Left-aligned text whose line is centred on `y`.
    pub fn text(&mut self, x: f32, y: f32, text: &str, size: f32, color: Color) {
        self.text_box(x, y - size, WIDTH, size * 2.0, text, size, color, Align::Left);
    }

    pub fn bold(&mut self, x: f32, y: f32, text: &str, size: f32, color: Color) {
        self.text(x, y, text, size, color);
        if let Some(Shape::Text { bold, .. }) = self.shapes.last_mut() {
            *bold = true;
        }
    }

    /// The caption for `t`: the last step that has started.
    pub fn steps(&mut self, t: f32, steps: &[(f32, &str)]) {
        if let Some((_, s)) = steps.iter().rev().find(|(start, _)| t >= *start) {
            self.caption = (*s).into();
        }
    }

    pub fn desktop(&mut self) {
        let bands = 12;
        let h = (HEIGHT - 18.0) / bands as f32;
        for i in 0..bands {
            let c = WALL_TOP.mix(WALL_BOTTOM, i as f32 / (bands - 1) as f32);
            self.rect(0.0, i as f32 * h, WIDTH, h + 1.0, 0.0, c);
        }
        self.circle(390.0, 40.0, 90.0, WHITE.alpha(0.06));
        self.circle(70.0, 230.0, 70.0, WHITE.alpha(0.05));
        self.taskbar();
    }

    pub fn taskbar(&mut self) {
        self.rect(0.0, HEIGHT - 18.0, WIDTH, 18.0, 0.0, TASKBAR);
        for (i, c) in [
            ACCENT,
            Color::rgb(240, 180, 60),
            Color::rgb(90, 190, 120),
            Color::rgb(220, 90, 90),
        ]
        .into_iter()
        .enumerate()
        {
            self.rect(200.0 + i as f32 * 20.0, HEIGHT - 14.0, 10.0, 10.0, 2.0, c);
        }
        self.text(440.0, HEIGHT - 9.0, "9:41", 7.0, WHITE.alpha(0.8));
    }

    /// A window; returns the top of its content area.
    pub fn window(&mut self, x: f32, y: f32, w: f32, h: f32, title: &str) -> f32 {
        self.window_toned(x, y, w, h, title, 0.0)
    }

    /// A window part way (`dark`, 0 to 1) to the dark theme.
    pub fn window_toned(&mut self, x: f32, y: f32, w: f32, h: f32, title: &str, dark: f32) -> f32 {
        self.rect(x + 2.0, y + 3.0, w, h, 6.0, SHADOW);
        self.rect(x, y, w, h, 6.0, WINDOW.mix(DARK_WINDOW, dark));
        self.rect(x, y, w, TITLE_H, 6.0, TITLE.mix(DARK_TITLE, dark));
        self.rect(x, y + TITLE_H - 6.0, w, 6.0, 0.0, TITLE.mix(DARK_TITLE, dark));
        self.text(x + 8.0, y + TITLE_H / 2.0, title, 7.5, TEXT.mix(DARK_TEXT, dark));
        let mark = MUTED.mix(DARK_TEXT, dark);
        let cx = x + w - 10.0;
        let cy = y + TITLE_H / 2.0;
        self.line(cx - 3.0, cy - 3.0, cx + 3.0, cy + 3.0, mark, 1.0);
        self.line(cx - 3.0, cy + 3.0, cx + 3.0, cy - 3.0, mark, 1.0);
        self.line(cx - 22.0, cy, cx - 16.0, cy, mark, 1.0);
        y + TITLE_H
    }

    /// The mouse pointer with its tip at `(x, y)`.
    pub fn cursor(&mut self, x: f32, y: f32) {
        let pts = [
            (0.0, 0.0),
            (0.0, 15.0),
            (3.6, 11.6),
            (6.2, 17.0),
            (8.6, 16.0),
            (6.0, 10.8),
            (11.0, 10.8),
        ];
        self.polygon(
            pts.iter().map(|(a, b)| (x + a, y + b)).collect(),
            WHITE,
            Some((Color::rgb(20, 20, 24), 1.2)),
        );
    }

    /// A click at `(x, y)`, `p` (0 to 1) through its ripple.
    pub fn click(&mut self, x: f32, y: f32, p: f32) {
        if p > 0.0 && p < 1.0 {
            self.ring(x, y, 4.0 + 12.0 * p, ACCENT.alpha(1.0 - p), 2.0);
        }
    }

    /// A row of key caps for `shortcut` ("Win+Shift+C"), centred on `cx`.
    pub fn keys(&mut self, cx: f32, y: f32, shortcut: &str, down: bool) {
        let labels: Vec<String> = shortcut.split('+').map(key_label).collect();
        self.key_caps(cx, y, &labels, down);
    }

    pub fn key_caps(&mut self, cx: f32, y: f32, labels: &[String], down: bool) {
        let size = 9.0;
        let widths: Vec<f32> = labels
            .iter()
            .map(|l| (l.chars().count() as f32 * size * 0.62 + 14.0).max(24.0))
            .collect();
        let gap = 14.0;
        let total = widths.iter().sum::<f32>() + gap * (labels.len().max(1) - 1) as f32;
        let mut x = cx - total / 2.0;
        let lift = if down { 0.0 } else { 2.0 };
        for (i, (label, w)) in labels.iter().zip(&widths).enumerate() {
            if i > 0 {
                self.text_box(x - gap, y, gap, 22.0, "+", 10.0, WHITE, Align::Center);
            }
            self.rect(x, y + 2.0, *w, 22.0, 5.0, Color::rgb(150, 156, 170));
            let face = if down { ACCENT_SOFT } else { WHITE };
            self.rect(x, y + 2.0 - lift, *w, 20.0, 5.0, face);
            self.text_box(x, y + 2.0 - lift, *w, 20.0, label, size, TEXT, Align::Center);
            x += w + gap;
        }
    }

    /// A pop-up menu; returns its height.
    pub fn menu(&mut self, x: f32, y: f32, w: f32, items: &[&str], hot: Option<usize>) -> f32 {
        let h = items.len() as f32 * MENU_ROW + 6.0;
        self.rect(x + 2.0, y + 3.0, w, h, 5.0, SHADOW);
        self.rect(x, y, w, h, 5.0, WINDOW);
        self.outline(x, y, w, h, 5.0, LINE, 1.0);
        for (i, item) in items.iter().enumerate() {
            let iy = y + 3.0 + i as f32 * MENU_ROW;
            if item.is_empty() {
                self.line(
                    x + 6.0,
                    iy + MENU_ROW / 2.0,
                    x + w - 6.0,
                    iy + MENU_ROW / 2.0,
                    LINE,
                    1.0,
                );
                continue;
            }
            if hot == Some(i) {
                self.rect(x + 3.0, iy, w - 6.0, MENU_ROW, 3.0, ACCENT_SOFT);
            }
            self.text(x + 10.0, iy + MENU_ROW / 2.0, item, 7.5, TEXT);
        }
        h
    }

    pub fn button(&mut self, x: f32, y: f32, w: f32, label: &str, pressed: bool) {
        let fill = if pressed { ACCENT } else { WHITE };
        let ink = if pressed { WHITE } else { TEXT };
        self.rect(x, y, w, 16.0, 4.0, fill);
        self.outline(x, y, w, 16.0, 4.0, if pressed { ACCENT } else { LINE }, 1.0);
        self.text_box(x, y, w, 16.0, label, 7.0, ink, Align::Center);
    }

    /// A text box, with a caret when `focused`.
    pub fn field(&mut self, x: f32, y: f32, w: f32, text: &str, focused: bool) {
        self.rect(x, y, w, 16.0, 3.0, WHITE);
        self.outline(x, y, w, 16.0, 3.0, if focused { ACCENT } else { LINE }, 1.0);
        self.text(x + 5.0, y + 8.0, text, 7.5, TEXT);
        if focused {
            let cx = x + 6.0 + text.chars().count() as f32 * 4.1;
            self.line(cx, y + 3.5, cx, y + 12.5, TEXT, 1.0);
        }
    }

    /// A short notice near the top of the screen, `p` (0 to 1) through showing.
    pub fn toast(&mut self, text: &str, p: f32) {
        if p <= 0.0 || p >= 1.0 {
            return;
        }
        let a = (span(p, 0.0, 0.15) * (1.0 - span(p, 0.85, 1.0))).min(1.0);
        let w = text.chars().count() as f32 * 4.6 + 28.0;
        let x = (WIDTH - w) / 2.0;
        self.rect(x, 14.0, w, 22.0, 11.0, Color::rgb(30, 32, 38).alpha(0.92 * a));
        self.text_box(x, 14.0, w, 22.0, text, 8.0, WHITE.alpha(a), Align::Center);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_has_a_walkthrough() {
        let config = Config::new();
        for id in powerkit_core::MODULE_IDS {
            let w = walkthrough(id).unwrap_or_else(|| panic!("{id} has no walkthrough"));
            assert!(w.duration >= 4.0, "{id}");
            let mut captions = std::collections::BTreeSet::new();
            let mut t = 0.0;
            while t < w.duration {
                let f = w.frame(t, &config);
                assert!(f.shapes.len() > 10, "{id} at {t}s draws almost nothing");
                assert!(!f.caption.is_empty(), "{id} at {t}s has no caption");
                for s in &f.shapes {
                    if let Shape::Text { text, .. } = s {
                        assert!(!text.contains('\n'), "{id}: text is one line");
                    }
                }
                captions.insert(f.caption);
                t += 0.1;
            }
            assert!(captions.len() >= 2, "{id} never moves on to a second step");
        }
        assert_eq!(
            scenes::ALL.len(),
            powerkit_core::MODULE_IDS.len(),
            "no walkthrough for a module that does not exist"
        );
    }

    #[test]
    fn walkthroughs_use_the_configured_shortcut() {
        let mut config = Config::new();
        config.color_picker.shortcut = "Ctrl+Alt+P".into();
        let w = walkthrough("color_picker").unwrap();
        let texts: Vec<String> = (0..w.duration as usize * 10)
            .flat_map(|i| w.frame(i as f32 / 10.0, &config).shapes)
            .filter_map(|s| match s {
                Shape::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "P") && texts.iter().any(|t| t == "Alt"));
        assert!(!texts.iter().any(|t| t == "Win"));
    }

    #[test]
    fn helpers() {
        assert_eq!(typed("calc", 0.0, 1.0, 0.2), "");
        assert_eq!(typed("calc", 1.0, 1.0, 0.2), "c");
        assert_eq!(typed("calc", 1.45, 1.0, 0.2), "cal");
        assert_eq!(typed("café", 9.0, 1.0, 0.2), "café");
        assert_eq!(path(0.0, &[(1.0, 0.0, 0.0), (2.0, 10.0, 20.0)]), (0.0, 0.0));
        assert_eq!(path(1.5, &[(1.0, 0.0, 0.0), (2.0, 10.0, 20.0)]), (5.0, 10.0));
        assert_eq!(path(9.0, &[(1.0, 0.0, 0.0), (2.0, 10.0, 20.0)]), (10.0, 20.0));
        assert_eq!(
            Color::rgb(0, 0, 0).mix(Color::rgb(255, 255, 255), 0.5),
            Color::rgb(128, 128, 128)
        );
        assert_eq!(WHITE.alpha(0.5).0 >> 24, 128);
        assert_eq!(key_label("Slash"), "/");
    }
}
