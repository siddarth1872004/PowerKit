//! The selection overlay, and reading the text off the event loop thread: recognition
//! takes a noticeable moment, and the mouse and keyboard hooks run on that thread.

use crate::{join, read_size, selection};
use anyhow::Result;
use powerkit_config::TextExtractor as Settings;
use powerkit_core::{Event, MouseEvent, Rect};
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::overlay::{Overlay, Piece};
use powerkit_platform::{clipboard, event_loop, magnifier, monitor, ocr};
use powerkit_plugin_api::Ctx;
use std::sync::mpsc::{self, Receiver, TryRecvError};

const FRAME_MS: u32 = 16;
/// How often a running recognition is checked for its result.
const POLL_MS: u32 = 50;
const VK_ESCAPE: u32 = 0x1B;
const DIM: &str = "#000000";
const DIM_ALPHA: u8 = 90;
const TITLE: &str = "Text Extractor";

struct Selecting {
    bounds: Rect,
    start: Option<(i32, i32)>,
    overlay: Overlay,
    timer: usize,
    _capture: InputCapture,
}

impl Drop for Selecting {
    fn drop(&mut self) {
        event_loop::kill_timer(self.timer);
    }
}

impl Selecting {
    fn draw(&mut self, cursor: (i32, i32)) -> Result<()> {
        let mut pieces = vec![Piece::Rect(self.bounds)];
        if let Some(r) = self.start.and_then(|s| selection(s, cursor, self.bounds)) {
            pieces.push(Piece::CutRect(r));
        }
        self.overlay.show_shape(self.bounds, DIM, DIM_ALPHA, &pieces)?;
        Ok(())
    }
}

struct Reading {
    rx: Receiver<Result<Vec<String>, String>>,
    timer: usize,
}

impl Drop for Reading {
    fn drop(&mut self) {
        event_loop::kill_timer(self.timer);
    }
}

#[derive(Default)]
pub struct Live {
    selecting: Option<Selecting>,
    reading: Option<Reading>,
}

/// Every monitor together.
fn desktop() -> Result<Rect> {
    let monitors = monitor::enumerate()?;
    let mut r = monitors.first().map(|m| m.rect).unwrap_or_default();
    for m in &monitors {
        r = Rect::new(
            r.left.min(m.rect.left),
            r.top.min(m.rect.top),
            r.right.max(m.rect.right),
            r.bottom.max(m.rect.bottom),
        );
    }
    Ok(r)
}

impl Live {
    pub fn busy(&self) -> bool {
        self.selecting.is_some() || self.reading.is_some()
    }

    pub fn begin(&mut self) -> Result<()> {
        if self.selecting.is_some() {
            self.cancel();
            return Ok(());
        }
        let mut s = Selecting {
            bounds: desktop()?,
            start: None,
            overlay: Overlay::new()?,
            timer: event_loop::set_timer(FRAME_MS)?,
            _capture: InputCapture::new()?,
        };
        s.draw(input::cursor_pos()?)?;
        self.selecting = Some(s);
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.selecting = None;
    }

    fn read(&mut self, area: Rect, language: String) -> Result<()> {
        let (w, h) = read_size(area.width(), area.height(), ocr::max_dimension());
        let pixels = if (w, h) == (area.width(), area.height()) {
            magnifier::capture(area)?
        } else {
            magnifier::capture_scaled(area, w, h)?
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = ocr::recognize(&pixels, w as u32, h as u32, &language).map_err(|e| e.to_string());
            // The receiver is gone only when PowerKit is shutting down.
            let _ = tx.send(result);
        });
        self.reading = Some(Reading {
            rx,
            timer: event_loop::set_timer(POLL_MS)?,
        });
        Ok(())
    }

    fn finish(&mut self, settings: &Settings, ctx: &mut Ctx) {
        let Some(reading) = &self.reading else { return };
        let result = match reading.rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(r) => r,
            Err(TryRecvError::Disconnected) => Err("text recognition stopped unexpectedly".into()),
        };
        self.reading = None;
        match result.map(|lines| join(&lines, settings.single_line)) {
            Ok(text) if text.is_empty() => ctx.notify(TITLE, "No text was found in that area."),
            Ok(text) => {
                if let Err(e) = clipboard::set_text(&text) {
                    ctx.notify(TITLE, format!("The text could not be copied: {e}"));
                }
            }
            Err(e) => ctx.notify(TITLE, e),
        }
    }

    pub fn on_event(&mut self, event: &Event, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        if let (Event::Timer(id), Some(r)) = (event, &self.reading) {
            if *id == r.timer {
                self.finish(settings, ctx);
                return Ok(());
            }
        }
        let Some(s) = &mut self.selecting else {
            return Ok(());
        };
        match *event {
            Event::Timer(id) if id == s.timer => {
                if s.start.is_some() {
                    s.draw(input::cursor_pos()?)?;
                }
            }
            Event::Mouse(MouseEvent::LeftDown { x, y }) => s.start = Some((x, y)),
            Event::Mouse(MouseEvent::LeftUp { x, y }) => {
                let Some(s) = self.selecting.take() else { return Ok(()) };
                let area = s.start.and_then(|start| selection(start, (x, y), s.bounds));
                // The dimming must be gone before the screen is captured.
                drop(s);
                if let Some(area) = area {
                    if self.reading.is_none() {
                        self.read(area, settings.language.clone())?;
                    }
                }
            }
            Event::Mouse(MouseEvent::RightUp { .. }) | Event::Key(VK_ESCAPE) => self.selecting = None,
            _ => {}
        }
        Ok(())
    }
}
