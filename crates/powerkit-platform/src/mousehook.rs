//! A low-level mouse hook that only watches: every callback sees moves and button
//! presses system-wide, and the input always reaches its application.

use crate::event_loop::helper;
use crate::{Context, Error, Result};
use std::cell::RefCell;
use std::marker::PhantomData;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseInput {
    Move { x: i32, y: i32 },
    Down { button: Button, x: i32, y: i32 },
    Up { button: Button, x: i32, y: i32 },
}

type Callback = Box<dyn FnMut(MouseInput)>;

#[derive(Default)]
struct Hooks {
    os: Option<HHOOK>,
    next_id: u64,
    callbacks: Vec<(u64, Callback)>,
}

thread_local! {
    static HOOKS: RefCell<Hooks> = RefCell::new(Hooks::default());
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let (x, y) = (info.pt.x, info.pt.y);
        let input = match wparam.0 as u32 {
            WM_MOUSEMOVE => Some(MouseInput::Move { x, y }),
            WM_LBUTTONDOWN => Some(MouseInput::Down {
                button: Button::Left,
                x,
                y,
            }),
            WM_RBUTTONDOWN => Some(MouseInput::Down {
                button: Button::Right,
                x,
                y,
            }),
            WM_MBUTTONDOWN => Some(MouseInput::Down {
                button: Button::Middle,
                x,
                y,
            }),
            WM_LBUTTONUP => Some(MouseInput::Up {
                button: Button::Left,
                x,
                y,
            }),
            WM_RBUTTONUP => Some(MouseInput::Up {
                button: Button::Right,
                x,
                y,
            }),
            WM_MBUTTONUP => Some(MouseInput::Up {
                button: Button::Middle,
                x,
                y,
            }),
            _ => None,
        };
        if let Some(input) = input {
            // A panic must not unwind into the OS; a busy list (re-entered while a
            // callback pumped messages) skips this event.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                HOOKS.with(|h| {
                    if let Ok(mut h) = h.try_borrow_mut() {
                        for (_, f) in h.callbacks.iter_mut() {
                            f(input);
                        }
                    }
                })
            }));
        }
    }
    // SAFETY: forwarding the unmodified arguments of this callback.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Active while it exists; dropping it removes the callback, and the OS hook with the
/// last one.
pub struct MouseHook {
    id: u64,
    _not_send: PhantomData<*const ()>,
}

impl MouseHook {
    /// Must be called on the event-loop thread, whose message loop services the hook.
    /// The callback runs for every mouse move, so it must be quick.
    pub fn install(callback: impl FnMut(MouseInput) + 'static) -> Result<MouseHook> {
        helper()?;
        HOOKS.with(|h| {
            let mut h = h
                .try_borrow_mut()
                .map_err(|_| Error::Other("mouse hook installed from inside a mouse callback".into()))?;
            if h.os.is_none() {
                // SAFETY: the procedure matches HOOKPROC and lives for the whole program.
                let hook = unsafe {
                    let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
                    SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(instance.into()), 0)
                }
                .context("mouse hook")?;
                h.os = Some(hook);
            }
            let id = h.next_id;
            h.next_id += 1;
            h.callbacks.push((id, Box::new(callback)));
            Ok(MouseHook {
                id,
                _not_send: PhantomData,
            })
        })
    }
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        HOOKS.with(|h| {
            let Ok(mut h) = h.try_borrow_mut() else {
                tracing::warn!("mouse hook dropped inside a mouse callback; it stays installed");
                return;
            };
            h.callbacks.retain(|(id, _)| *id != self.id);
            if h.callbacks.is_empty() {
                if let Some(hook) = h.os.take() {
                    // SAFETY: the hook was installed above and is removed once.
                    let _ = unsafe { UnhookWindowsHookEx(hook) };
                }
            }
        });
    }
}
