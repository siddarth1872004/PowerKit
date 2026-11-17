//! Choosing the area, and the recording itself, which runs on a thread of its own.

use crate::file_name;
use anyhow::{Context, Result};
use powerkit_config::{RecordingFormat, ScreenRecorder as Settings};
use powerkit_core::{Event, MouseEvent, Rect};
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::overlay::{Overlay, Piece};
use powerkit_platform::recorder::{self, Format, Recording};
use powerkit_platform::{event_loop, monitor, shell, time};
use powerkit_plugin_api::Ctx;
use screenshot::{free_path, selection};
use std::path::PathBuf;

const FRAME_MS: u32 = 16;
/// How often a recording is checked for having failed by itself (a full disk).
const CHECK_MS: u32 = 500;
const VK_ESCAPE: u32 = 0x1B;
const DIM: &str = "#000000";
const DIM_ALPHA: u8 = 90;
/// The frame around the area being recorded, outside it so it is not recorded.
const FRAME: &str = "#E81123";
const FRAME_WIDTH: i32 = 3;
const TITLE: &str = "Screen Recorder";

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

struct Running {
    recording: Recording,
    path: PathBuf,
    frame: Overlay,
    timer: usize,
}

#[derive(Default)]
pub struct Live {
    selecting: Option<Selecting>,
    running: Option<Running>,
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

/// The folder chosen in the settings, or Videos\Screen recordings.
fn folder(settings: &Settings) -> Result<PathBuf> {
    if !settings.folder.is_empty() {
        return Ok(PathBuf::from(&settings.folder));
    }
    Ok(shell::videos_dir()?.join("Screen recordings"))
}

impl Live {
    pub fn recording(&self) -> bool {
        self.running.is_some()
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

    fn start(&mut self, area: Rect, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        let format = match settings.format {
            RecordingFormat::Mp4 => Format::Mp4,
            RecordingFormat::Gif => Format::Gif,
        };
        let dir = folder(settings)?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let now = time::local_now();
        let name = file_name(
            (now.year, now.month, now.day),
            (now.hour, now.minute, now.second),
            settings.format,
        );
        let path = free_path(&dir, &name);
        let recorded = recorder::recorded_area(area, format);
        let outer = recorded.inset(-FRAME_WIDTH);
        let mut frame = Overlay::new()?;
        frame.show_shape(outer, FRAME, 255, &[Piece::Rect(outer), Piece::CutRect(recorded)])?;
        let recording = Recording::start(area, settings.fps, settings.show_cursor, format, &path)?;
        self.running = Some(Running {
            recording,
            path,
            frame,
            timer: event_loop::set_timer(CHECK_MS)?,
        });
        ctx.notify(TITLE, "Recording. Press the shortcut again to stop.");
        Ok(())
    }

    /// End the recording, if one is running, and say where it went.
    pub fn stop(&mut self, ctx: &mut Ctx) {
        let Some(Running {
            recording,
            path,
            frame,
            timer,
        }) = self.running.take()
        else {
            return;
        };
        event_loop::kill_timer(timer);
        drop(frame);
        match recording.stop() {
            Ok(()) => ctx.notify(TITLE, format!("Saved as {}", path.display())),
            Err(e) => ctx.notify(TITLE, format!("The recording could not be saved: {e}")),
        }
    }

    pub fn on_event(&mut self, event: &Event, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        if let (Event::Timer(id), Some(r)) = (event, &self.running) {
            if *id == r.timer {
                if r.recording.failed() {
                    self.stop(ctx);
                }
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
                // The dimming must be gone before the first frame.
                drop(s);
                if let Some(area) = area {
                    self.start(area, settings, ctx)?;
                }
            }
            Event::Mouse(MouseEvent::RightUp { .. }) | Event::Key(VK_ESCAPE) => self.selecting = None,
            _ => {}
        }
        Ok(())
    }
}
