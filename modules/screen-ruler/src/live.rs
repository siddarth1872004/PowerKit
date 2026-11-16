//! The overlay. Every monitor is captured once when measuring starts, before anything
//! is drawn, so the lines and the label never measure themselves.

use crate::{bounds, label, lines, measured, span, Mode};
use anyhow::Result;
use powerkit_config::ScreenRuler as Settings;
use powerkit_core::{Event, MouseEvent, Rect};
use powerkit_platform::canvas::{Align, Canvas, Font};
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::magnifier::{self, Pixels};
use powerkit_platform::monitor::{self, Monitor};
use powerkit_platform::overlay::{Overlay, Piece};
use powerkit_platform::{clipboard, event_loop};
use powerkit_plugin_api::Ctx;

const FRAME_MS: u32 = 16;
const VK_ESCAPE: u32 = 0x1B;
const VK_RETURN: u32 = 0x0D;
const TITLE: &str = "Screen Ruler";
/// Gap between the cursor and the label, in 96-DPI pixels.
const LABEL_GAP: i32 = 18;
const PAD: i32 = 8;
const BACK: &str = "#202020";
const TEXT: &str = "#FFFFFF";
const HINT: &str = "#A0A0A0";

struct Shot {
    monitor: Monitor,
    pixels: Pixels,
}

/// What was last drawn, to skip frames where nothing changed.
type Drawn = ((i32, i32), Mode, Option<(i32, i32)>);

struct Measuring {
    shots: Vec<Shot>,
    desktop: Rect,
    mode: Mode,
    /// Where a box drag started, in [`Mode::Bounds`].
    start: Option<(i32, i32)>,
    /// The measurement shown, which a click copies.
    text: String,
    drawn: Option<Drawn>,
    overlay: Overlay,
    label: Canvas,
    timer: usize,
    _capture: InputCapture,
}

impl Drop for Measuring {
    fn drop(&mut self) {
        event_loop::kill_timer(self.timer);
    }
}

impl Measuring {
    fn start() -> Result<Measuring> {
        let mut shots = Vec::new();
        for m in monitor::enumerate()? {
            let pixels = magnifier::capture(m.rect)?;
            shots.push(Shot { monitor: m, pixels });
        }
        let mut desktop = shots.first().map(|s| s.monitor.rect).unwrap_or_default();
        for s in &shots {
            let r = s.monitor.rect;
            desktop = Rect::new(
                desktop.left.min(r.left),
                desktop.top.min(r.top),
                desktop.right.max(r.right),
                desktop.bottom.max(r.bottom),
            );
        }
        Ok(Measuring {
            shots,
            desktop,
            mode: Mode::default(),
            start: None,
            text: String::new(),
            drawn: None,
            overlay: Overlay::new()?,
            label: Canvas::new()?,
            timer: event_loop::set_timer(FRAME_MS)?,
            _capture: InputCapture::new()?,
        })
    }

    fn draw(&mut self, cursor: (i32, i32), settings: &Settings) -> Result<()> {
        let key = (cursor, self.mode, self.start);
        if self.drawn == Some(key) {
            return Ok(());
        }
        self.drawn = Some(key);
        let Some(shot) = self.shots.iter().find(|s| s.monitor.rect.contains(cursor.0, cursor.1)) else {
            self.overlay.hide();
            self.label.hide();
            return Ok(());
        };
        let m = &shot.monitor;
        let area = match (self.mode, self.start) {
            (Mode::Bounds, Some(start)) => Some(bounds(start, cursor)),
            (Mode::Bounds, None) => None,
            _ => Some(
                span(
                    &shot.pixels,
                    m.rect.width(),
                    cursor.0 - m.rect.left,
                    cursor.1 - m.rect.top,
                    settings.tolerance,
                )
                .offset(m.rect.left, m.rect.top),
            ),
        };
        let mut rows = Vec::new();
        match area {
            Some(r) => {
                let pieces: Vec<_> = lines(self.mode, r, cursor, m.scale(1), settings.draw_feet)
                    .into_iter()
                    .filter_map(|l| l.intersect(&self.desktop))
                    .map(Piece::Rect)
                    .collect();
                self.overlay
                    .show_shape(self.desktop, &settings.line_color, 255, &pieces)?;
                let (px, other) = label(measured(self.mode, r), settings.extra_unit, m.dpi);
                self.text = px.clone();
                rows.push((px, Font::Title, TEXT));
                if let Some(o) = other {
                    rows.push((o, Font::Body, TEXT));
                }
            }
            None => {
                self.overlay.hide();
                self.text.clear();
            }
        }
        rows.push((
            format!("{} \u{00B7} wheel to change", self.mode.label()),
            Font::Body,
            HINT,
        ));
        let screen = m.rect;
        self.label.show(
            m.dpi,
            235,
            |p| {
                let (pad, gap) = (p.scale(PAD), p.scale(LABEL_GAP));
                let sizes: Vec<_> = rows.iter().map(|(t, f, _)| p.measure(t, *f)).collect();
                let w = sizes.iter().map(|s| s.0).max().unwrap_or(0) + 2 * pad;
                let h = sizes.iter().map(|s| s.1).sum::<i32>() + 2 * pad;
                // Below and right of the cursor, flipped where that leaves the monitor.
                let x = if cursor.0 + gap + w > screen.right {
                    cursor.0 - gap - w
                } else {
                    cursor.0 + gap
                };
                let y = if cursor.1 + gap + h > screen.bottom {
                    cursor.1 - gap - h
                } else {
                    cursor.1 + gap
                };
                Rect::from_size(x, y, w, h)
            },
            |p| {
                let pad = p.scale(PAD);
                let sizes: Vec<_> = rows.iter().map(|(t, f, _)| p.measure(t, *f)).collect();
                let w = sizes.iter().map(|s| s.0).max().unwrap_or(0) + 2 * pad;
                let h = sizes.iter().map(|s| s.1).sum::<i32>() + 2 * pad;
                p.round(Rect::new(0, 0, w, h), p.scale(6), BACK);
                let mut y = pad;
                for ((text, font, color), (_, th)) in rows.iter().zip(&sizes) {
                    p.text(Rect::new(pad, y, w - pad, y + th), text, color, *font, Align::Left);
                    y += th;
                }
            },
        )?;
        Ok(())
    }
}

#[derive(Default)]
pub struct Live {
    measuring: Option<Measuring>,
}

impl Live {
    pub fn measuring(&self) -> bool {
        self.measuring.is_some()
    }

    pub fn toggle(&mut self) -> Result<()> {
        if self.measuring.take().is_none() {
            self.measuring = Some(Measuring::start()?);
        }
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.measuring = None;
    }

    /// Draw again on the next frame, for changed settings.
    pub fn redraw(&mut self) {
        if let Some(m) = &mut self.measuring {
            m.drawn = None;
        }
    }

    fn copy(m: &Measuring, ctx: &mut Ctx) {
        if m.text.is_empty() {
            return;
        }
        if let Err(e) = clipboard::set_text(&m.text) {
            ctx.notify(TITLE, format!("The measurement could not be copied: {e}"));
        }
    }

    pub fn on_event(&mut self, event: &Event, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        let Some(m) = &mut self.measuring else {
            return Ok(());
        };
        match *event {
            Event::Timer(id) if id == m.timer => {
                let cursor = input::cursor_pos()?;
                if let Err(e) = m.draw(cursor, settings) {
                    self.measuring = None;
                    return Err(e);
                }
            }
            Event::Mouse(MouseEvent::Wheel { delta }) => {
                m.mode = m.mode.turn(delta);
                m.start = None;
            }
            Event::Mouse(MouseEvent::LeftDown { x, y }) => match m.mode {
                Mode::Bounds => m.start = Some((x, y)),
                _ => Self::copy(m, ctx),
            },
            Event::Mouse(MouseEvent::LeftUp { x, y }) if m.mode == Mode::Bounds && m.start.is_some() => {
                // The box as released, even if no frame drew it yet.
                m.draw((x, y), settings)?;
                Self::copy(m, ctx);
                m.start = None;
            }
            Event::Key(VK_RETURN) => Self::copy(m, ctx),
            Event::Mouse(MouseEvent::RightUp { .. }) | Event::Key(VK_ESCAPE) | Event::DisplayChanged => {
                self.measuring = None
            }
            _ => {}
        }
        Ok(())
    }
}
