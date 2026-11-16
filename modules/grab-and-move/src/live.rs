//! The drag itself. The mouse hook only records what happens; windows are moved from a
//! timer on the event loop, because moving another program's window waits for that
//! program, and a hook that waits too long is removed by Windows.

use crate::{grab_edges, moved, resized, restored_under_cursor};
use powerkit_config::{GrabAndMove as Settings, GrabModifier};
use powerkit_core::Rect;
use powerkit_platform::keyhook;
use powerkit_platform::mousehook::{Button, MouseInput};
use powerkit_platform::window::{self, Hwnd};
use powerkit_platform::{event_loop, input, monitor};
use std::cell::RefCell;
use std::rc::Rc;

const FRAME_MS: u32 = 10;
const VK_ALT: u32 = 0x12;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// An unassigned key, pressed once per drag so that releasing Win does not open Start
/// and releasing Alt does not open the window's menu bar.
const VK_NONE: u32 = 0xE8;

struct Drag {
    hwnd: Hwnd,
    button: Button,
    /// `None` moves; otherwise the edges being resized.
    edges: Option<(i32, i32)>,
    from: (i32, i32),
    start: Rect,
    cursor: (i32, i32),
    applied: Rect,
    /// The maximized bounds, until the first tick restores the window.
    maximized: Option<Rect>,
    prepared: bool,
    released: bool,
    timer: usize,
}

enum Step {
    Prepare {
        hwnd: Hwnd,
        maximized: Option<Rect>,
        activate: bool,
        cursor: (i32, i32),
    },
    Place {
        hwnd: Hwnd,
        rect: Rect,
    },
}

pub struct Live {
    pub settings: Settings,
    drag: Option<Drag>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Live {
    pub fn new(settings: Settings) -> Live {
        Live { settings, drag: None }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn cancel(&mut self) {
        if let Some(d) = self.drag.take() {
            event_loop::kill_timer(d.timer);
        }
    }

    fn modifier_held(&self) -> bool {
        match self.settings.modifier {
            GrabModifier::Win => input::key_down(VK_LWIN) || input::key_down(VK_RWIN),
            GrabModifier::Alt => input::key_down(VK_ALT),
        }
    }

    /// Mouse hook callback; returns whether to swallow the input.
    pub fn on_mouse(&mut self, input: MouseInput) -> bool {
        match input {
            MouseInput::Move { x, y } => {
                if let Some(d) = self.drag.as_mut().filter(|d| !d.released) {
                    d.cursor = (x, y);
                }
                false
            }
            MouseInput::Down { button, x, y } => {
                if self.drag.as_ref().is_some_and(|d| !d.released) {
                    return true;
                }
                self.cancel();
                self.grab(button, (x, y))
            }
            MouseInput::Up { button, x, y } => match self.drag.as_mut() {
                Some(d) => {
                    if d.button == button && !d.released {
                        d.cursor = (x, y);
                        d.released = true;
                    }
                    true
                }
                None => false,
            },
            MouseInput::Wheel { .. } => false,
        }
    }

    fn grab(&mut self, button: Button, at: (i32, i32)) -> bool {
        if !matches!(button, Button::Left | Button::Right) || !self.modifier_held() {
            return false;
        }
        let Some(hwnd) = window::at_point(at.0, at.1) else {
            return false;
        };
        let Ok(info) = window::info(hwnd) else {
            return false;
        };
        if info.pid == std::process::id() || self.settings.excluded_apps.contains(&info.exe) {
            return false;
        }
        let mon = monitor::from_window(hwnd).map(|m| m.rect).unwrap_or_default();
        if info.manageable(&mon).is_err() {
            return false;
        }
        let edges = match button {
            Button::Right if !self.settings.resize || !info.is_resizable() || info.maximized => return false,
            Button::Right => Some(grab_edges(info.rect, at)),
            _ => None,
        };
        let timer = match event_loop::set_timer(FRAME_MS) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "Grab and Move timer failed");
                return false;
            }
        };
        if let Err(e) = keyhook::send_keys(&[(VK_NONE, true), (VK_NONE, false)]) {
            tracing::debug!(error = %e, "could not mask the modifier release");
        }
        self.drag = Some(Drag {
            hwnd,
            button,
            edges,
            from: at,
            start: info.rect,
            cursor: at,
            applied: info.rect,
            maximized: info.maximized.then_some(info.rect),
            prepared: false,
            released: false,
            timer,
        });
        true
    }

    fn step(&mut self, id: usize) -> Option<Step> {
        let d = self.drag.as_mut().filter(|d| d.timer == id)?;
        if !d.prepared {
            d.prepared = true;
            if d.maximized.is_some() || self.settings.activate {
                return Some(Step::Prepare {
                    hwnd: d.hwnd,
                    maximized: d.maximized.take(),
                    activate: self.settings.activate,
                    cursor: d.cursor,
                });
            }
        }
        let done = d.released || !input::button_down(d.button);
        let rect = match d.edges {
            Some(edges) => resized(d.start, edges, d.from, d.cursor),
            None => moved(d.start, d.from, d.cursor),
        };
        let changed = rect != d.applied;
        d.applied = rect;
        let hwnd = d.hwnd;
        if done {
            self.cancel();
        }
        changed.then_some(Step::Place { hwnd, rect })
    }

    /// After a maximized window was restored: drag from its restored size.
    fn restarted(&mut self, start: Rect, applied: Rect, cursor: (i32, i32)) {
        if let Some(d) = &mut self.drag {
            d.start = start;
            d.applied = applied;
            d.from = cursor;
        }
    }
}

/// Timer tick. The window calls run without the state borrowed, since they can let the
/// mouse hook run again in the meantime.
pub fn tick(live: &Rc<RefCell<Live>>, id: usize) {
    let step = match live.try_borrow_mut() {
        Ok(mut l) => l.step(id),
        Err(_) => return,
    };
    match step {
        None => {}
        Some(Step::Prepare {
            hwnd,
            maximized,
            activate,
            cursor,
        }) => {
            if activate {
                if let Err(e) = window::activate(hwnd) {
                    tracing::debug!(error = %e, "grabbed window not activated");
                }
            }
            if let Some(max) = maximized {
                window::restore(hwnd);
                match window::visible_rect(hwnd) {
                    Ok(normal) => {
                        let start = restored_under_cursor(max, normal, cursor);
                        if let Ok(mut l) = live.try_borrow_mut() {
                            l.restarted(start, normal, cursor);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "restored window has no bounds");
                        if let Ok(mut l) = live.try_borrow_mut() {
                            l.cancel();
                        }
                    }
                }
            }
        }
        Some(Step::Place { hwnd, rect }) => {
            if let Err(e) = window::set_visible_rect(hwnd, rect) {
                // Windows of programs running as administrator refuse this.
                tracing::debug!(error = %e, "Grab and Move could not place the window");
                if let Ok(mut l) = live.try_borrow_mut() {
                    l.cancel();
                }
            }
        }
    }
}
