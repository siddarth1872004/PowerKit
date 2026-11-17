//! The selection overlay, taking the picture and where it goes.

use crate::{file_name, free_path, selection, Mode};
use anyhow::{Context, Result};
use powerkit_config::Screenshot as Settings;
use powerkit_core::{Event, MouseEvent, Rect};
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::overlay::{Overlay, Piece};
use powerkit_platform::{clipboard, event_loop, image, magnifier, monitor, shell, time, window};
use powerkit_plugin_api::Ctx;
use std::path::PathBuf;

const FRAME_MS: u32 = 16;
const VK_ESCAPE: u32 = 0x1B;
const DIM: &str = "#000000";
const DIM_ALPHA: u8 = 90;
const TITLE: &str = "Screenshot";

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

#[derive(Default)]
pub struct Live {
    selecting: Option<Selecting>,
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

/// The folder chosen in the settings, or Pictures\Screenshots.
pub fn folder(settings: &Settings) -> Result<PathBuf> {
    if !settings.folder.is_empty() {
        return Ok(PathBuf::from(&settings.folder));
    }
    Ok(shell::pictures_dir()?.join("Screenshots"))
}

impl Live {
    pub fn take(&mut self, mode: Mode, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        match mode {
            Mode::Region => self.begin(),
            Mode::Screen => self.save(desktop()?, settings, ctx),
            Mode::Window => {
                let hwnd = window::foreground().context("no window is in front")?;
                let area = window::visible_rect(hwnd)?.intersect(&desktop()?);
                let area = area.context("the window in front is off screen")?;
                self.save(area, settings, ctx)
            }
        }
    }

    fn begin(&mut self) -> Result<()> {
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

    fn save(&mut self, area: Rect, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        let (w, h) = (area.width() as u32, area.height() as u32);
        let pixels: Vec<u32> = magnifier::capture(area)?.into_iter().map(|p| p | 0xFF00_0000).collect();
        if settings.copy_to_clipboard {
            clipboard::set_picture(&pixels, w, h).context("copying the picture")?;
        }
        if !settings.save_to_folder {
            ctx.notify(TITLE, "The picture was copied.");
            return Ok(());
        }
        let dir = folder(settings)?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let now = time::local_now();
        let name = file_name((now.year, now.month, now.day), (now.hour, now.minute, now.second));
        let path = free_path(&dir, &name);
        image::save_png(&pixels, w, h, &path).with_context(|| format!("saving {}", path.display()))?;
        let copied = if settings.copy_to_clipboard {
            "Copied and saved"
        } else {
            "Saved"
        };
        ctx.notify(TITLE, format!("{copied} as {}", path.display()));
        Ok(())
    }

    pub fn on_event(&mut self, event: &Event, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
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
                    self.save(area, settings, ctx)?;
                }
            }
            Event::Mouse(MouseEvent::RightUp { .. }) | Event::Key(VK_ESCAPE) => self.selecting = None,
            _ => {}
        }
        Ok(())
    }
}
