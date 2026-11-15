//! Grab and Move: hold the modifier (Win by default) and left-drag anywhere in a window
//! to move it, or right-drag to resize it from the nearest edge or corner.

#[cfg(windows)]
mod live;

use anyhow::Result;
use powerkit_config::GrabAndMove as Settings;
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
#[cfg(windows)]
use std::{cell::RefCell, rc::Rc};

pub const ID: &str = "grab_and_move";
/// Smallest size a resize leaves, in pixels.
pub const MIN_SIZE: i32 = 120;

/// Which edges a resize moves: -1 the left/top edge, 1 the right/bottom edge, 0 neither.
/// The window is split in thirds each way; the middle third of one axis leaves that axis
/// alone, and the very centre picks the nearest corner.
pub fn grab_edges(r: Rect, (x, y): (i32, i32)) -> (i32, i32) {
    let third = |p: i32, lo: i32, hi: i32| {
        let w = (hi - lo).max(1);
        match (p - lo) * 3 / w {
            i32::MIN..=0 => -1,
            1 => 0,
            _ => 1,
        }
    };
    let (h, v) = (third(x, r.left, r.right), third(y, r.top, r.bottom));
    if (h, v) != (0, 0) {
        return (h, v);
    }
    let near = |p: i32, lo: i32, hi: i32| if p * 2 < lo + hi { -1 } else { 1 };
    (near(x, r.left, r.right), near(y, r.top, r.bottom))
}

/// `start` moved by how far the cursor went.
pub fn moved(start: Rect, from: (i32, i32), to: (i32, i32)) -> Rect {
    start.offset(to.0 - from.0, to.1 - from.1)
}

/// `start` with the grabbed edges moved by the cursor, never smaller than [`MIN_SIZE`].
pub fn resized(start: Rect, (h, v): (i32, i32), from: (i32, i32), to: (i32, i32)) -> Rect {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let mut r = start;
    match h {
        -1 => r.left = (start.left + dx).min(start.right - MIN_SIZE),
        1 => r.right = (start.right + dx).max(start.left + MIN_SIZE),
        _ => {}
    }
    match v {
        -1 => r.top = (start.top + dy).min(start.bottom - MIN_SIZE),
        1 => r.bottom = (start.bottom + dy).max(start.top + MIN_SIZE),
        _ => {}
    }
    r
}

/// Where a window that was maximized goes once restored, so the cursor keeps the same
/// relative place across its width and stays inside its title bar area.
pub fn restored_under_cursor(maximized: Rect, restored: Rect, cursor: (i32, i32)) -> Rect {
    let frac = (cursor.0 - maximized.left) as f64 / maximized.width().max(1) as f64;
    let left = cursor.0 - (restored.width() as f64 * frac).round() as i32;
    let grip = (cursor.1 - maximized.top).clamp(0, restored.height() / 2);
    let top = cursor.1 - grip;
    Rect::new(left, top, left + restored.width(), top + restored.height())
}

#[derive(Default)]
pub struct GrabAndMove {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: Option<Rc<RefCell<live::Live>>>,
    #[cfg(windows)]
    hook: Option<powerkit_platform::mousehook::MouseHook>,
}

impl GrabAndMove {
    pub fn new() -> Self {
        Self::default()
    }

    fn active(&self) -> bool {
        self.running && !self.paused && self.settings.enabled
    }

    #[cfg(windows)]
    fn apply(&mut self) -> Result<()> {
        use powerkit_platform::mousehook::MouseHook;
        if !self.active() {
            self.hook = None;
            if let Some(l) = self.live.take() {
                l.borrow_mut().cancel();
            }
            return Ok(());
        }
        let live = Rc::clone(
            self.live
                .get_or_insert_with(|| Rc::new(RefCell::new(live::Live::new(self.settings.clone())))),
        );
        live.borrow_mut().settings = self.settings.clone();
        if self.hook.is_none() {
            self.hook = Some(MouseHook::install_filter(move |input| match live.try_borrow_mut() {
                Ok(mut l) => l.on_mouse(input),
                Err(_) => false,
            })?);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply(&mut self) -> Result<()> {
        Ok(())
    }

    fn dragging(&self) -> bool {
        #[cfg(windows)]
        return self.live.as_ref().is_some_and(|l| l.borrow().dragging());
        #[cfg(not(windows))]
        false
    }
}

impl Module for GrabAndMove {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Grab and Move".into(),
            description: "Hold Win (or Alt) and drag anywhere in a window to move or resize it.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::MouseHook, Permission::WindowManagement],
            commands: vec![],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.grab_and_move.clone();
        self.running = true;
        self.paused = false;
        self.apply()
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "Grab and Move did not stop cleanly");
        }
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "Grab and Move did not pause cleanly");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.apply() {
            ctx.notify("Grab and Move", format!("Could not turn back on: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.grab_and_move.clone();
        self.apply()
    }

    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        if let (Event::Timer(id), Some(l)) = (event, &self.live) {
            live::tick(l, *id);
        }
        #[cfg(not(windows))]
        let _ = event;
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        anyhow::bail!("unknown command {id:?}")
    }

    fn status(&self) -> Value {
        json!({ "dragging": self.dragging() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect {
        left: 100,
        top: 100,
        right: 400,
        bottom: 400,
    };

    #[test]
    fn edges_follow_the_grabbed_third() {
        assert_eq!(grab_edges(R, (110, 110)), (-1, -1));
        assert_eq!(grab_edges(R, (390, 250)), (1, 0));
        assert_eq!(grab_edges(R, (250, 390)), (0, 1));
        assert_eq!(grab_edges(R, (240, 260)), (-1, 1), "centre picks the nearest corner");
    }

    #[test]
    fn move_and_resize() {
        assert_eq!(moved(R, (150, 150), (160, 130)), Rect::new(110, 80, 410, 380));
        assert_eq!(
            resized(R, (1, 1), (390, 390), (420, 400)),
            Rect::new(100, 100, 430, 410)
        );
        assert_eq!(
            resized(R, (-1, 0), (110, 250), (500, 0)),
            Rect::new(400 - MIN_SIZE, 100, 400, 400),
            "never smaller than the minimum, and the other axis stays"
        );
    }

    #[test]
    fn restore_keeps_cursor_proportion() {
        let max = Rect::new(0, 0, 2000, 1000);
        let normal = Rect::new(300, 200, 1300, 800);
        let r = restored_under_cursor(max, normal, (1500, 10));
        assert_eq!((r.width(), r.height()), (1000, 600));
        assert_eq!(r.left, 1500 - 750);
        assert_eq!(r.top, 0);
    }
}
