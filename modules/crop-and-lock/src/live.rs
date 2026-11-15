//! The selection overlay and the open crop windows.

use crate::selection;
use anyhow::{anyhow, bail, Result};
use powerkit_core::{Event, MouseEvent, Rect};
use powerkit_platform::crop::{CropMode, CropWindow};
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::overlay::{Overlay, Piece};
use powerkit_platform::window::{self, Hwnd, Unmanageable};
use powerkit_platform::{event_loop, monitor};

const FRAME_MS: u32 = 16;
const VK_ESCAPE: u32 = 0x1B;
const DIM: &str = "#000000";
const DIM_ALPHA: u8 = 110;

struct Selecting {
    mode: CropMode,
    target: Hwnd,
    title: String,
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
    /// Dim the window except the part being selected.
    fn draw(&mut self, cursor: (i32, i32)) -> Result<()> {
        let mut pieces = vec![Piece::Rect(self.bounds)];
        if let Some(r) = self.start.and_then(|s| selection(s, cursor, self.bounds)) {
            pieces.push(Piece::CutRect(r));
        }
        self.overlay.show_shape(self.bounds, DIM, DIM_ALPHA, &pieces)?;
        Ok(())
    }
}

#[derive(Default)]
pub struct Live {
    selecting: Option<Selecting>,
    windows: Vec<CropWindow>,
}

impl Live {
    pub fn selecting(&self) -> bool {
        self.selecting.is_some()
    }

    pub fn open_count(&self) -> usize {
        self.windows.iter().filter(|w| w.is_open()).count()
    }

    pub fn begin(&mut self, mode: CropMode) -> Result<()> {
        self.windows.retain(|w| w.is_open());
        if self.selecting.is_some() {
            self.cancel();
            return Ok(());
        }
        let target = window::foreground().ok_or_else(|| anyhow!("no window is focused"))?;
        let info = window::info(target)?;
        if info.pid == std::process::id() {
            bail!("PowerKit's own windows cannot be cropped");
        }
        let mon = monitor::from_window(target).map(|m| m.rect).unwrap_or_default();
        if let Err(
            reason @ (Unmanageable::NotVisible
            | Unmanageable::Minimized
            | Unmanageable::Cloaked
            | Unmanageable::ChildWindow
            | Unmanageable::ShellWindow),
        ) = info.manageable(&mon)
        {
            bail!("This window cannot be cropped ({reason:?})");
        }
        if mode == CropMode::Reparent && info.maximized {
            bail!("Restore the window from maximized before cropping it this way");
        }
        let mut s = Selecting {
            mode,
            target,
            title: info.title,
            bounds: info.rect,
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

    pub fn close_all(&mut self) {
        self.selecting = None;
        self.windows.clear();
    }

    /// Returns whether anything visible changed.
    pub fn on_event(&mut self, event: &Event) -> Result<bool> {
        let Some(s) = &mut self.selecting else {
            return Ok(false);
        };
        match *event {
            Event::Timer(id) if id == s.timer => {
                if s.start.is_some() {
                    s.draw(input::cursor_pos()?)?;
                }
                Ok(false)
            }
            Event::Mouse(MouseEvent::LeftDown { x, y }) => {
                s.start = Some((x, y));
                Ok(false)
            }
            Event::Mouse(MouseEvent::LeftUp { x, y }) => {
                let Some(s) = self.selecting.take() else {
                    return Ok(false);
                };
                let Some(area) = s.start.and_then(|start| selection(start, (x, y), s.bounds)) else {
                    return Ok(true);
                };
                let (mode, target, title) = (s.mode, s.target, format!("{} (cropped)", s.title));
                // The overlay and input capture must be gone before the new window shows.
                drop(s);
                let w = CropWindow::new(mode, target, area, &title)?;
                self.windows.push(w);
                Ok(true)
            }
            Event::Mouse(MouseEvent::RightUp { .. }) | Event::Key(VK_ESCAPE) => {
                self.selecting = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
