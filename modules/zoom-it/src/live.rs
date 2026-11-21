//! The full-screen windows. Still zoom and drawing work on one capture of the monitor
//! under the cursor, taken before anything is shown; live zoom captures every frame
//! with its own window left out of the capture.

use crate::{countdown, magnify, paint, parse_color, pen_color, view, wheel_zoom, Shape, Stroke};
use anyhow::{Context as _, Result};
use powerkit_config::ZoomIt as Settings;
use powerkit_core::{Event, MouseEvent, Rect};
use powerkit_platform::canvas::{Align, Canvas, Font};
use powerkit_platform::event_loop;
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::magnifier::{self, Pixels};
use powerkit_platform::monitor::{self, Monitor};
use powerkit_platform::overlay::Overlay;
use powerkit_plugin_api::Ctx;
use std::time::{Duration, Instant};

const FRAME_MS: u32 = 16;
/// Live zoom copies the whole screen each frame; this is as often as that keeps up.
const LIVE_FRAME_MS: u32 = 33;
const BREAK_TICK_MS: u32 = 250;
const VK_ESCAPE: u32 = 0x1B;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
/// Pen colors, E erase, W whiteboard, K blackboard, Ctrl+Z undo.
const KEYS: [u32; 10] = [
    'R' as u32, 'G' as u32, 'B' as u32, 'Y' as u32, 'O' as u32, 'P' as u32, 'E' as u32, 'W' as u32, 'K' as u32,
    'Z' as u32,
];

struct Drawing {
    /// What was on screen when drawing started.
    base: Pixels,
    board: Option<u32>,
    strokes: Vec<Stroke>,
    current: Option<Stroke>,
    color: u32,
    width: i32,
    /// Right-click goes back to zooming rather than ending.
    from_zoom: bool,
}

enum Mode {
    Still {
        shot: Pixels,
        zoom: f64,
        /// The picture shown now; drawing starts on it.
        frame: Pixels,
        shown: Option<Rect>,
        drawing: Option<Drawing>,
    },
    Live {
        zoom: f64,
        overlay_ready: bool,
    },
    Break {
        canvas: Canvas,
        end: Instant,
        shown: String,
    },
}

struct Session {
    mode: Mode,
    monitor: Monitor,
    overlay: Overlay,
    dirty: bool,
    timer: usize,
    _capture: InputCapture,
}

impl Drop for Session {
    fn drop(&mut self) {
        event_loop::kill_timer(self.timer);
    }
}

impl Session {
    fn new(mode: Mode, monitor: Monitor, frame_ms: u32) -> Result<Session> {
        Ok(Session {
            mode,
            monitor,
            overlay: Overlay::new()?,
            dirty: true,
            timer: event_loop::set_timer(frame_ms)?,
            _capture: InputCapture::with_keys(&KEYS)?,
        })
    }

    fn size(&self) -> (i32, i32) {
        (self.monitor.rect.width(), self.monitor.rect.height())
    }

    fn local(&self, (x, y): (i32, i32)) -> (i32, i32) {
        (x - self.monitor.rect.left, y - self.monitor.rect.top)
    }

    fn frame(&mut self) -> Result<()> {
        let cursor = self.local(input::cursor_pos()?);
        let size = self.size();
        let rect = self.monitor.rect;
        match &mut self.mode {
            Mode::Still {
                shot,
                zoom,
                frame,
                shown,
                drawing: None,
            } => {
                let area = view(size, cursor, *zoom);
                if *shown != Some(area) {
                    *frame = magnify(shot, size.0, area, size);
                    self.overlay.show_picture(rect, frame)?;
                    *shown = Some(area);
                }
            }
            Mode::Still { drawing: Some(d), .. } => {
                if input::key_down(0x01) || input::key_down(0x02) {
                    if let Some(s) = &mut d.current {
                        let before = s.shape.clone();
                        s.shape.extend(cursor);
                        self.dirty |= s.shape != before;
                    }
                }
                if std::mem::take(&mut self.dirty) {
                    self.overlay.show_drawn(rect, |pixels| {
                        match d.board {
                            Some(c) => pixels.fill(c),
                            None => pixels.copy_from_slice(&d.base),
                        }
                        for s in d.strokes.iter().chain(&d.current) {
                            paint(pixels, size.0, s);
                        }
                    })?;
                }
            }
            Mode::Live { zoom, overlay_ready } => {
                if !*overlay_ready {
                    self.overlay.exclude_from_capture()?;
                    *overlay_ready = true;
                }
                let area = view(size, cursor, *zoom).offset(rect.left, rect.top);
                self.overlay.show_screen(rect, area)?;
            }
            Mode::Break { canvas, end, shown } => {
                let left = end.saturating_duration_since(Instant::now());
                let text = if left.is_zero() {
                    "Break is over".to_string()
                } else {
                    countdown(left)
                };
                if *shown != text {
                    *shown = text.clone();
                    canvas.show(
                        self.monitor.dpi,
                        235,
                        |_| rect,
                        |p| {
                            let (w, h) = (rect.width(), rect.height());
                            let all = Rect::new(0, 0, w, h);
                            p.fill(all, "#101010");
                            p.text(all, &text, "#FFFFFF", Font::Huge, Align::Center);
                            let hint = Rect::new(0, h * 3 / 4, w, h * 3 / 4 + p.scale(40));
                            p.text(
                                hint,
                                "Press Escape to end the break",
                                "#A0A0A0",
                                Font::Title,
                                Align::Center,
                            );
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    fn start_drawing(&mut self, settings: &Settings, from_zoom: bool) {
        if let Mode::Still { frame, drawing, .. } = &mut self.mode {
            *drawing = Some(Drawing {
                base: frame.clone(),
                board: None,
                strokes: Vec::new(),
                current: None,
                color: parse_color(&settings.pen_color),
                width: settings.pen_width as i32,
                from_zoom,
            });
            self.dirty = true;
        }
    }

    /// Handles input; `false` ends the session.
    fn input(&mut self, event: &Event, settings: &Settings) -> bool {
        let cursor = |x: i32, y: i32| (x - self.monitor.rect.left, y - self.monitor.rect.top);
        let (shift, ctrl) = (input::key_down(VK_SHIFT), input::key_down(VK_CONTROL));
        let mut back = false;
        match (&mut self.mode, event) {
            (_, Event::Key(VK_ESCAPE)) => return false,
            (Mode::Break { .. }, Event::Mouse(MouseEvent::RightUp { .. })) => return false,
            (Mode::Live { zoom, .. }, Event::Mouse(MouseEvent::Wheel { delta })) => *zoom = wheel_zoom(*zoom, *delta),
            (Mode::Live { .. }, Event::Mouse(MouseEvent::RightUp { .. })) => return false,
            (
                Mode::Still {
                    drawing: Some(d),
                    shown,
                    ..
                },
                e,
            ) => match *e {
                Event::Mouse(MouseEvent::LeftDown { x, y }) => {
                    d.current = Some(Stroke {
                        shape: Shape::start(cursor(x, y), shift, ctrl),
                        color: d.color,
                        width: d.width,
                    });
                }
                Event::Mouse(MouseEvent::LeftUp { x, y }) => {
                    if let Some(mut s) = d.current.take() {
                        s.shape.extend(cursor(x, y));
                        d.strokes.push(s);
                    }
                }
                Event::Mouse(MouseEvent::Wheel { delta }) => d.width = (d.width + delta.signum()).clamp(1, 40),
                Event::Mouse(MouseEvent::RightUp { .. }) if d.from_zoom => {
                    // Back to zooming, where the view picks up from the cursor.
                    *shown = None;
                    back = true;
                }
                Event::Mouse(MouseEvent::RightUp { .. }) => return false,
                Event::Key(k) => match char::from_u32(k) {
                    Some('Z') if ctrl => {
                        d.strokes.pop();
                    }
                    Some('E') => d.strokes.clear(),
                    Some('W') => d.board = (d.board != Some(0x00FF_FFFF)).then_some(0x00FF_FFFF),
                    Some('K') => d.board = (d.board != Some(0)).then_some(0),
                    _ => {
                        if let Some(c) = pen_color(k) {
                            d.color = c;
                        }
                    }
                },
                _ => {}
            },
            (Mode::Still { zoom, .. }, e) => match *e {
                Event::Mouse(MouseEvent::Wheel { delta }) => *zoom = wheel_zoom(*zoom, delta),
                Event::Mouse(MouseEvent::LeftDown { .. }) => self.start_drawing(settings, true),
                Event::Mouse(MouseEvent::RightUp { .. }) => return false,
                _ => {}
            },
            _ => {}
        }
        if back {
            if let Mode::Still { drawing, .. } = &mut self.mode {
                *drawing = None;
            }
        }
        self.dirty = true;
        true
    }
}

#[derive(Default)]
pub struct Live {
    session: Option<Session>,
}

fn monitor_under_cursor() -> Result<Monitor> {
    let (x, y) = input::cursor_pos()?;
    monitor::from_point(x, y).context("no monitor under the cursor")
}

fn still(zoom: f64) -> Result<(Mode, Monitor)> {
    let m = monitor_under_cursor()?;
    let shot = magnifier::capture(m.rect)?;
    Ok((
        Mode::Still {
            frame: shot.clone(),
            shot,
            zoom,
            shown: None,
            drawing: None,
        },
        m,
    ))
}

impl Live {
    pub fn active(&self) -> Option<&'static str> {
        Some(match &self.session.as_ref()?.mode {
            Mode::Still { drawing: Some(_), .. } => "draw",
            Mode::Still { .. } => "zoom",
            Mode::Live { .. } => "live_zoom",
            Mode::Break { .. } => "break",
        })
    }

    pub fn cancel(&mut self) {
        self.session = None;
    }

    pub fn act(&mut self, action: &str, settings: &Settings) -> Result<()> {
        let active = self.active();
        // Drawing on a zoomed screen: the draw shortcut starts drawing on what is shown.
        if (active, action) == (Some("zoom"), "draw") {
            if let Some(s) = &mut self.session {
                s.start_drawing(settings, true);
            }
            return Ok(());
        }
        let toggling_off = active == Some(action) || (active == Some("draw") && action == "zoom");
        // The old windows must be gone before the screen is captured.
        self.session = None;
        if toggling_off {
            return Ok(());
        }
        let zoom = settings.zoom_percent as f64 / 100.0;
        let mut session = match action {
            "zoom" => {
                let (mode, m) = still(zoom)?;
                Session::new(mode, m, FRAME_MS)?
            }
            "draw" => {
                let (mode, m) = still(1.0)?;
                let mut s = Session::new(mode, m, FRAME_MS)?;
                s.start_drawing(settings, false);
                s
            }
            "live_zoom" => Session::new(
                Mode::Live {
                    zoom,
                    overlay_ready: false,
                },
                monitor_under_cursor()?,
                LIVE_FRAME_MS,
            )?,
            _ => Session::new(
                Mode::Break {
                    canvas: Canvas::new()?,
                    end: Instant::now() + Duration::from_secs(settings.break_minutes as u64 * 60),
                    shown: String::new(),
                },
                monitor_under_cursor()?,
                BREAK_TICK_MS,
            )?,
        };
        session.frame()?;
        self.session = Some(session);
        Ok(())
    }

    pub fn on_event(&mut self, event: &Event, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        let Some(s) = &mut self.session else {
            return Ok(());
        };
        let result = match event {
            Event::Timer(id) if *id == s.timer => s.frame(),
            Event::DisplayChanged => {
                self.session = None;
                return Ok(());
            }
            Event::Mouse(_) | Event::Key(_) => {
                if !s.input(event, settings) {
                    self.session = None;
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.session = None;
            ctx.notify("ZoomIt", format!("{e:#}"));
        }
        Ok(())
    }
}
