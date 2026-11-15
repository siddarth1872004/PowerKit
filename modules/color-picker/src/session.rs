//! One picking session: the input capture, the magnifier and its refresh timer.

use anyhow::{anyhow, Result};
use powerkit_core::Rect;
use powerkit_platform::event_loop;
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::magnifier::{self, Magnifier};

/// About 60 refreshes per second.
const REFRESH_MS: u32 = 16;

pub struct Session {
    // Field order is drop order: give input back before the magnifier disappears.
    _capture: InputCapture,
    magnifier: Magnifier,
    timer: usize,
    pub zoom: u32,
}

impl Session {
    pub fn start(zoom: u32) -> Result<Session> {
        let magnifier = Magnifier::new().map_err(|e| anyhow!(e))?;
        let capture = InputCapture::new().map_err(|e| anyhow!("Could not capture the mouse: {e}"))?;
        let timer = event_loop::set_timer(REFRESH_MS).map_err(|e| anyhow!(e))?;
        Ok(Session {
            _capture: capture,
            magnifier,
            timer,
            zoom,
        })
    }

    pub fn timer(&self) -> usize {
        self.timer
    }

    /// Sample around the cursor, redraw the magnifier and return the color under the
    /// cursor (`0x00RRGGBB`). `label` turns that color into the text shown.
    pub fn refresh(&mut self, radius: u32, grid: bool, label: impl Fn(u32) -> String) -> Result<u32> {
        let (x, y) = input::cursor_pos().map_err(|e| anyhow!(e))?;
        let side = 2 * radius + 1;
        let r = radius as i32;
        let pixels =
            magnifier::capture(Rect::from_size(x - r, y - r, side as i32, side as i32)).map_err(|e| anyhow!(e))?;
        let center = pixels[pixels.len() / 2];
        self.magnifier
            .show((x, y), &pixels, side, self.zoom, grid, &label(center))
            .map_err(|e| anyhow!(e))?;
        Ok(center)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        event_loop::kill_timer(self.timer);
        self.magnifier.hide();
    }
}

pub fn pixel_at(x: i32, y: i32) -> Result<u32> {
    let px = magnifier::capture(Rect::from_size(x, y, 1, 1)).map_err(|e| anyhow!(e))?;
    Ok(px[0])
}

pub fn cursor_pos() -> Result<(i32, i32)> {
    input::cursor_pos().map_err(|e| anyhow!(e))
}

pub fn copy(text: &str) -> Result<()> {
    powerkit_platform::clipboard::set_text(text).map_err(|e| anyhow!("Could not copy to the clipboard: {e}"))
}
