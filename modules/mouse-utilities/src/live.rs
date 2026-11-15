//! The tools at work on Windows: their overlays, what the hooks report, and a frame
//! timer that runs only while something is on screen.

use crate::logic::{self, DoubleCtrl, Part, Shake};
use anyhow::{anyhow, Result};
use powerkit_config::{FindMouseActivation, MouseUtilities as Settings};
use powerkit_core::{Event, MouseEvent, Rect};
use powerkit_platform::input::{self, InputCapture};
use powerkit_platform::mousehook::{Button, MouseInput};
use powerkit_platform::overlay::{Overlay, Piece};
use powerkit_platform::{event_loop, magnifier, monitor, shell, window};
use std::time::Instant;

const FRAME_MS: u32 = 16;
/// Click circles shown at once; the oldest goes first.
const MAX_SPOTS: usize = 8;
const VK_RETURN: u32 = 0x0D;
const VK_ESCAPE: u32 = 0x1B;
/// Mouse Jump: where no monitor is, and the line around each one.
const JUMP_BACKGROUND: u32 = 0x202020;
const JUMP_EDGE: u32 = 0x808080;

fn pieces(parts: &[Part]) -> Vec<Piece> {
    parts
        .iter()
        .map(|p| match *p {
            Part::Rect(r) => Piece::Rect(r),
            Part::CutRect(r) => Piece::CutRect(r),
            Part::Ellipse(r) => Piece::Ellipse(r),
            Part::CutEllipse(r) => Piece::CutEllipse(r),
        })
        .collect()
}

fn is_ctrl(vk: u32) -> bool {
    matches!(vk, 0x11 | 0xA2 | 0xA3)
}

struct Spot {
    button: Button,
    at: (i32, i32),
    released_at: Option<u64>,
    overlay: Overlay,
}

struct Jump {
    picture: Rect,
    desktop: Rect,
    overlay: Overlay,
    _capture: InputCapture,
}

pub struct Live {
    pub settings: Settings,
    epoch: Instant,
    timer: Option<usize>,
    desktop: Option<Rect>,
    /// Start time and window of the Find My Mouse spotlight.
    spotlight: Option<(u64, Overlay)>,
    /// The lines and their outline, which is a separate shape in its own color.
    crosshairs: Option<(Overlay, Overlay)>,
    highlighter: bool,
    spots: Vec<Spot>,
    jump: Option<Jump>,
    double_ctrl: DoubleCtrl,
    shake: Shake,
}

impl Live {
    pub fn new(settings: Settings) -> Live {
        Live {
            settings,
            epoch: Instant::now(),
            timer: None,
            desktop: None,
            spotlight: None,
            crosshairs: None,
            highlighter: false,
            spots: Vec::new(),
            jump: None,
            double_ctrl: DoubleCtrl::default(),
            shake: Shake::default(),
        }
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn desktop(&mut self) -> Result<Rect> {
        if let Some(d) = self.desktop {
            return Ok(d);
        }
        let rects: Vec<Rect> = monitor::enumerate()?.iter().map(|m| m.rect).collect();
        let d = logic::union(&rects).ok_or_else(|| anyhow!("no monitors"))?;
        self.desktop = Some(d);
        Ok(d)
    }

    pub fn display_changed(&mut self) {
        self.desktop = None;
        self.close_jump();
    }

    pub fn is_timer(&self, id: usize) -> bool {
        self.timer == Some(id)
    }

    pub fn spotlight_on(&self) -> bool {
        self.spotlight.is_some()
    }

    pub fn jump_open(&self) -> bool {
        self.jump.is_some()
    }

    fn animating(&self) -> bool {
        self.spotlight.is_some() || self.crosshairs.is_some() || !self.spots.is_empty()
    }

    fn ensure_timer(&mut self) -> Result<()> {
        if self.timer.is_none() {
            self.timer = Some(event_loop::set_timer(FRAME_MS)?);
        }
        Ok(())
    }

    /// Draw one frame of everything that follows the pointer or animates.
    pub fn tick(&mut self) -> Result<()> {
        if !self.animating() {
            if let Some(id) = self.timer.take() {
                event_loop::kill_timer(id);
            }
            return Ok(());
        }
        let now = self.now();
        let cursor = input::cursor_pos()?;
        let dpi = monitor::from_point(cursor.0, cursor.1).map_or(96, |m| m.dpi);
        let desktop = self.desktop()?;
        let s = &self.settings;

        if let Some((start, overlay)) = &mut self.spotlight {
            let f = &s.find_my_mouse;
            let r = logic::spotlight_radius(
                logic::scale(f.radius, dpi),
                f.initial_zoom,
                now.saturating_sub(*start),
                u64::from(f.animation_ms),
            );
            let shape = pieces(&logic::spotlight(cursor, desktop, r));
            overlay.show_shape(
                desktop,
                &f.background_color,
                logic::percent_alpha(f.opacity_percent),
                &shape,
            )?;
        }

        if let Some((lines, outline)) = &mut self.crosshairs {
            let c = &s.crosshairs;
            let (t, gap, len) = (
                logic::scale(c.thickness, dpi),
                logic::scale(c.center_gap, dpi),
                logic::scale(c.length, dpi),
            );
            let alpha = logic::percent_alpha(c.opacity_percent);
            let shape = pieces(&logic::crosshair(cursor, desktop, t, gap, len));
            lines.show_shape(desktop, &c.color, alpha, &shape)?;
            let border = logic::crosshair_border(cursor, desktop, t, gap, len, logic::scale(c.border_size, dpi));
            if border.is_empty() {
                outline.hide();
            } else {
                outline.show_shape(desktop, &c.border_color, alpha, &pieces(&border))?;
            }
        }

        let h = &s.highlighter;
        let base = logic::percent_alpha(h.opacity_percent);
        let r = logic::scale(h.radius, dpi);
        let mut result = Ok(());
        self.spots.retain_mut(|spot| {
            let since = spot.released_at.map(|t| now.saturating_sub(t));
            let Some(alpha) = logic::highlight_alpha(base, since, u64::from(h.fade_delay_ms), u64::from(h.fade_ms))
            else {
                return false;
            };
            if spot.released_at.is_none() {
                spot.at = cursor;
            }
            let color = if spot.button == Button::Right {
                &h.right_color
            } else {
                &h.left_color
            };
            let area = logic::around(spot.at, r);
            if let Err(e) = spot.overlay.show_shape(area, color, alpha, &[Piece::Ellipse(area)]) {
                result = Err(e);
            }
            true
        });
        Ok(result?)
    }

    pub fn toggle_spotlight(&mut self) -> Result<()> {
        if self.spotlight.take().is_some() || self.spotlight_blocked() {
            return Ok(());
        }
        self.spotlight = Some((self.now(), Overlay::new()?));
        self.ensure_timer()?;
        self.tick()
    }

    /// Find My Mouse stays off during games and in front of excluded apps.
    fn spotlight_blocked(&self) -> bool {
        let f = &self.settings.find_my_mouse;
        if f.skip_in_game_mode && shell::game_running() {
            return true;
        }
        if f.excluded_apps.is_empty() {
            return false;
        }
        window::foreground()
            .and_then(window::thread_process)
            .and_then(|(pid, _)| window::process_exe(pid))
            .is_some_and(|exe| f.excluded_apps.contains(&exe))
    }

    pub fn set_crosshairs(&mut self, on: bool) -> Result<()> {
        match (on, self.crosshairs.is_some()) {
            (true, false) => {
                self.crosshairs = Some((Overlay::new()?, Overlay::new()?));
                self.ensure_timer()?;
                self.tick()
            }
            (false, true) => {
                self.crosshairs = None;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn set_highlighter(&mut self, on: bool) {
        self.highlighter = on;
        if !on {
            self.spots.clear();
        }
    }

    /// Hide everything and stop the timer.
    pub fn clear(&mut self) {
        self.spotlight = None;
        self.crosshairs = None;
        self.spots.clear();
        self.close_jump();
        if let Some(id) = self.timer.take() {
            event_loop::kill_timer(id);
        }
    }

    /// From the keyboard hook. Keys always reach their application.
    pub fn on_key(&mut self, vk: u32, down: bool) -> Result<()> {
        let f = &self.settings.find_my_mouse;
        if down && !is_ctrl(vk) && self.spotlight.is_some() {
            self.spotlight = None;
        }
        if f.enabled && f.activation == FindMouseActivation::DoubleCtrl {
            let now = self.now();
            if self.double_ctrl.feed(vk, down, now) {
                self.toggle_spotlight()?;
            }
        }
        Ok(())
    }

    /// From the mouse hook. Clicks always reach their application.
    pub fn on_mouse(&mut self, input: MouseInput) -> Result<()> {
        let now = self.now();
        match input {
            MouseInput::Move { x, y } => {
                let f = &self.settings.find_my_mouse;
                // Gesture size in raw pixels: looking up the monitor on every move
                // would cost more than the precision is worth.
                if f.enabled
                    && f.activation == FindMouseActivation::Shake
                    && self.spotlight.is_none()
                    && self.shake.feed(x, y, now, 96)
                {
                    self.toggle_spotlight()?;
                }
            }
            MouseInput::Down { button, x, y } => {
                self.double_ctrl.interrupt();
                self.spotlight = None;
                if self.highlighter && button != Button::Middle {
                    if self.spots.len() >= MAX_SPOTS {
                        self.spots.remove(0);
                    }
                    self.spots.push(Spot {
                        button,
                        at: (x, y),
                        released_at: None,
                        overlay: Overlay::new()?,
                    });
                    self.ensure_timer()?;
                }
            }
            MouseInput::Up { button, .. } => {
                for s in self
                    .spots
                    .iter_mut()
                    .filter(|s| s.button == button && s.released_at.is_none())
                {
                    s.released_at = Some(now);
                }
            }
        }
        Ok(())
    }

    pub fn toggle_jump(&mut self) -> Result<()> {
        if self.jump.is_some() {
            self.close_jump();
            return Ok(());
        }
        let desktop = self.desktop()?;
        let cursor = input::cursor_pos()?;
        let m = monitor::from_point(cursor.0, cursor.1).ok_or_else(|| anyhow!("no monitor under the pointer"))?;
        let j = &self.settings.jump;
        let max_w = logic::scale(j.max_width, m.dpi).min(m.work_area.width() * 9 / 10);
        let max_h = logic::scale(j.max_height, m.dpi).min(m.work_area.height() * 9 / 10);
        let (w, h) = logic::fit(desktop.width(), desktop.height(), max_w, max_h);
        let picture = logic::centered(w, h, m.work_area);
        let mut pixels = magnifier::capture_scaled(desktop, w, h)?;
        let monitors: Vec<Rect> = monitor::enumerate()?.iter().map(|m| m.rect).collect();
        logic::frame_monitors(&mut pixels, w, h, desktop, &monitors, JUMP_BACKGROUND, JUMP_EDGE);
        let mut overlay = Overlay::new()?;
        overlay.show_picture(picture, &pixels)?;
        let capture = InputCapture::new()?;
        self.jump = Some(Jump {
            picture,
            desktop,
            overlay,
            _capture: capture,
        });
        Ok(())
    }

    fn close_jump(&mut self) {
        if let Some(mut j) = self.jump.take() {
            j.overlay.hide();
        }
    }

    /// Clicks and keys taken while the Mouse Jump picture is open.
    pub fn on_capture(&mut self, event: &Event) -> Result<()> {
        let Some(j) = &self.jump else { return Ok(()) };
        match *event {
            Event::Mouse(MouseEvent::LeftUp { x, y }) => {
                let target = logic::map_point(j.picture, j.desktop, (x, y));
                self.close_jump();
                if let Some((tx, ty)) = target {
                    input::set_cursor_pos(tx, ty)?;
                }
            }
            Event::Mouse(MouseEvent::RightUp { .. }) | Event::Key(VK_ESCAPE | VK_RETURN) => self.close_jump(),
            _ => {}
        }
        Ok(())
    }
}
