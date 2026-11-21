//! A low-level mouse hook: every callback sees moves and button presses system-wide.
//! Watchers never affect the input; filters may keep a press from reaching its
//! application.
//!
//! Windows holds every mouse move and wheel notch until the hooking thread answers,
//! so the OS hook lives on a thread of its own that does nothing else: the pointer
//! and scrolling never wait for the event loop. Moves and wheel notches go on at once
//! and reach the callbacks afterwards; only presses and releases wait for them,
//! because a filter may keep those.

use crate::event_loop::helper;
use crate::{Error, Result};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Posted to the event loop's helper window: moves or wheel notches are waiting.
pub(crate) const WM_MOUSE_PENDING: u32 = WM_APP + 3;
/// Sent to the helper window for a press or release; the answer is whether to keep it.
pub(crate) const WM_MOUSE_BUTTON: u32 = WM_APP + 4;
/// How long a press waits for the callbacks before it goes on anyway.
const BUTTON_WAIT_MS: u32 = 200;

/// The helper window of the event loop that owns the callbacks.
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// The latest pointer position not yet given to the callbacks, and whether one is.
static LAST_MOVE: AtomicU64 = AtomicU64::new(0);
static MOVE_PENDING: AtomicBool = AtomicBool::new(false);
/// The callbacks that may keep a click from its application, with the keys one of
/// which must be held for them to want it (`None`: any click). A click no filter wants
/// is passed on at once instead of waiting for the event loop.
static FILTERS: Mutex<Vec<(u64, Option<Vec<u16>>)>> = Mutex::new(Vec::new());
/// Buttons whose press waited for the event loop, so their release waits too.
static WAITED: AtomicU8 = AtomicU8::new(0);

/// On the hook thread: whether this click must wait for the callbacks' answer.
fn must_wait(button: Button, down: bool) -> bool {
    let bit = 1u8 << button_code(button);
    if !down {
        return WAITED.fetch_and(!bit, Ordering::AcqRel) & bit != 0;
    }
    let wait = FILTERS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .any(|(_, keys)| match keys {
            None => true,
            // SAFETY: plain query.
            Some(keys) => keys.iter().any(|&k| unsafe { GetAsyncKeyState(i32::from(k)) } < 0),
        });
    if wait {
        WAITED.fetch_or(bit, Ordering::AcqRel);
    }
    wait
}
static WHEEL: Mutex<Vec<MouseInput>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseInput {
    Move {
        x: i32,
        y: i32,
    },
    Down {
        button: Button,
        x: i32,
        y: i32,
    },
    Up {
        button: Button,
        x: i32,
        y: i32,
    },
    /// One notch is 120; positive is away from the user, or right when `horizontal`.
    Wheel {
        delta: i32,
        horizontal: bool,
    },
}

/// Returns whether to swallow the input.
type Callback = Box<dyn FnMut(MouseInput) -> bool>;

#[derive(Default)]
struct Hooks {
    /// The thread holding the OS hook.
    os: Option<u32>,
    next_id: u64,
    callbacks: Vec<(u64, Callback)>,
}

thread_local! {
    static HOOKS: RefCell<Hooks> = RefCell::new(Hooks::default());
}

fn pack(x: i32, y: i32) -> u64 {
    (u64::from(x as u32) << 32) | u64::from(y as u32)
}

fn unpack(v: u64) -> (i32, i32) {
    ((v >> 32) as u32 as i32, v as u32 as i32)
}

fn button_code(button: Button) -> usize {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::X1 => 3,
        Button::X2 => 4,
    }
}

fn button_of(code: usize) -> Button {
    match code {
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::X1,
        4 => Button::X2,
        _ => Button::Left,
    }
}

fn post_pending(target: HWND) {
    // SAFETY: posting to a window handle; a stale one only makes the call fail.
    let _ = unsafe { PostMessageW(Some(target), WM_MOUSE_PENDING, WPARAM(0), LPARAM(0)) };
}

/// Runs on the hook thread.
unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let target = HWND(TARGET.load(Ordering::Acquire) as *mut _);
    if code == HC_ACTION as i32 && !target.is_invalid() {
        // SAFETY: for HC_ACTION, `lparam` points to an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let (x, y) = (info.pt.x, info.pt.y);
        let high = (info.mouseData >> 16) as u16;
        let xbutton = if high == 2 { Button::X2 } else { Button::X1 };
        let button = match wparam.0 as u32 {
            WM_LBUTTONDOWN => Some((Button::Left, true)),
            WM_RBUTTONDOWN => Some((Button::Right, true)),
            WM_MBUTTONDOWN => Some((Button::Middle, true)),
            WM_XBUTTONDOWN => Some((xbutton, true)),
            WM_LBUTTONUP => Some((Button::Left, false)),
            WM_RBUTTONUP => Some((Button::Right, false)),
            WM_MBUTTONUP => Some((Button::Middle, false)),
            WM_XBUTTONUP => Some((xbutton, false)),
            _ => None,
        };
        match (wparam.0 as u32, button) {
            (WM_MOUSEMOVE, _) => {
                LAST_MOVE.store(pack(x, y), Ordering::Release);
                if !MOVE_PENDING.swap(true, Ordering::AcqRel) {
                    post_pending(target);
                }
            }
            (WM_MOUSEWHEEL | WM_MOUSEHWHEEL, _) => {
                WHEEL.lock().unwrap_or_else(|p| p.into_inner()).push(MouseInput::Wheel {
                    delta: high as i16 as i32,
                    horizontal: wparam.0 as u32 == WM_MOUSEHWHEEL,
                });
                post_pending(target);
            }
            (_, Some((button, down))) => {
                // By value, not by pointer: after a timeout the message may still be
                // handled, when this frame is gone.
                let w = WPARAM(button_code(button) | (usize::from(down) << 8));
                let l = LPARAM(pack(x, y) as i64 as isize);
                if !must_wait(button, down) {
                    // SAFETY: plain value arguments.
                    let _ = unsafe { PostMessageW(Some(target), WM_MOUSE_BUTTON, w, l) };
                    // SAFETY: forwarding the unmodified arguments of this callback.
                    return unsafe { CallNextHookEx(None, code, wparam, lparam) };
                }
                let mut keep = 0usize;
                // SAFETY: `keep` outlives the call, which returns before this does.
                let answered = unsafe {
                    SendMessageTimeoutW(
                        target,
                        WM_MOUSE_BUTTON,
                        w,
                        l,
                        SMTO_ABORTIFHUNG,
                        BUTTON_WAIT_MS,
                        Some(&mut keep),
                    )
                };
                if answered.0 != 0 && keep != 0 {
                    return LRESULT(1);
                }
            }
            _ => {}
        }
    }
    // SAFETY: forwarding the unmodified arguments of this callback.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Give `input` to every callback; whether one wants it kept from its application.
fn deliver(input: MouseInput) -> bool {
    // A panic must not unwind into the OS; a busy list (re-entered while a callback
    // pumped messages) skips this event.
    let keep = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        HOOKS.with(|h| {
            let Ok(mut h) = h.try_borrow_mut() else { return false };
            // Every callback sees the input, even after one swallowed it.
            h.callbacks.iter_mut().fold(false, |sw, (_, f)| f(input) | sw)
        })
    }));
    matches!(keep, Ok(true))
}

/// On the event loop thread: hand the waiting move and wheel notches to the callbacks.
pub(crate) fn deliver_pending() {
    if MOVE_PENDING.swap(false, Ordering::AcqRel) {
        let (x, y) = unpack(LAST_MOVE.load(Ordering::Acquire));
        deliver(MouseInput::Move { x, y });
    }
    let wheel = std::mem::take(&mut *WHEEL.lock().unwrap_or_else(|p| p.into_inner()));
    for input in wheel {
        deliver(input);
    }
}

/// On the event loop thread: a press or release sent by the hook thread.
pub(crate) fn deliver_button(wparam: WPARAM, lparam: LPARAM) -> bool {
    // The pointer got where it was pressed before the press.
    deliver_pending();
    let button = button_of(wparam.0 & 0xFF);
    let (x, y) = unpack(lparam.0 as i64 as u64);
    deliver(if wparam.0 >> 8 != 0 {
        MouseInput::Down { button, x, y }
    } else {
        MouseInput::Up { button, x, y }
    })
}

/// Start the hook thread; its id, to stop it with `WM_QUIT`.
fn start_thread() -> Result<u32> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("mouse hook".into())
        .spawn(move || {
            // SAFETY: the procedure matches HOOKPROC and lives for the whole program;
            // the hook is removed on this thread before it ends.
            let hook = unsafe {
                // Input waits for this thread, so it should get the CPU first.
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
                GetModuleHandleW(None)
                    .and_then(|instance| SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(instance.into()), 0))
            };
            let hook = match hook {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(Error::Os {
                        context: "mouse hook".into(),
                        source: e,
                    }));
                    return;
                }
            };
            // SAFETY: plain query.
            let _ = tx.send(Ok(unsafe { GetCurrentThreadId() }));
            let mut msg = MSG::default();
            // SAFETY: `msg` is a valid out pointer; the loop only services the hook.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
            // SAFETY: installed above, removed once.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        })
        .map_err(|e| Error::Other(format!("mouse hook thread: {e}")))?;
    rx.recv()
        .map_err(|_| Error::Other("the mouse hook thread ended".into()))?
}

/// Active while it exists; dropping it removes the callback, and the OS hook with the
/// last one.
pub struct MouseHook {
    id: u64,
    filter: bool,
    _not_send: PhantomData<*const ()>,
}

impl MouseHook {
    /// Must be called on the event-loop thread, whose message loop services the hook.
    /// The callback runs for every mouse move, so it must be quick.
    pub fn install(mut callback: impl FnMut(MouseInput) + 'static) -> Result<MouseHook> {
        Self::add(
            Box::new(move |input| {
                callback(input);
                false
            }),
            None,
        )
    }

    /// Like [`MouseHook::install`], but the callback returns `true` to keep the input
    /// from its application. Swallow a release only when its press was swallowed too.
    pub fn install_filter(callback: impl FnMut(MouseInput) -> bool + 'static) -> Result<MouseHook> {
        Self::add(Box::new(callback), Some(None))
    }

    /// Like [`MouseHook::install_filter`] for a callback that keeps clicks only while
    /// one of the virtual keys `keys` is held; other clicks reach their application
    /// without waiting for the event loop. The callback still sees every click.
    pub fn install_filter_while(keys: &[u16], callback: impl FnMut(MouseInput) -> bool + 'static) -> Result<MouseHook> {
        Self::add(Box::new(callback), Some(Some(keys.to_vec())))
    }

    /// `filter`: `None` for a callback that never keeps input.
    fn add(callback: Callback, filter: Option<Option<Vec<u16>>>) -> Result<MouseHook> {
        let target = helper()?;
        HOOKS.with(|h| {
            let mut h = h
                .try_borrow_mut()
                .map_err(|_| Error::Other("mouse hook installed from inside a mouse callback".into()))?;
            if h.os.is_none() {
                TARGET.store(target.0 as isize, Ordering::Release);
                h.os = Some(start_thread()?);
            }
            let id = h.next_id;
            h.next_id += 1;
            h.callbacks.push((id, callback));
            let filter = filter
                .map(|keys| FILTERS.lock().unwrap_or_else(|p| p.into_inner()).push((id, keys)))
                .is_some();
            Ok(MouseHook {
                id,
                filter,
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
            if self.filter {
                FILTERS
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retain(|(id, _)| *id != self.id);
            }
            if h.callbacks.is_empty() {
                if let Some(thread) = h.os.take() {
                    // SAFETY: plain value arguments; the thread removes its hook and ends.
                    if let Err(e) = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) } {
                        tracing::warn!("mouse hook thread not stopped: {e}");
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_and_buttons_cross_threads_unchanged() {
        for (x, y) in [(0, 0), (-1920, -5), (3839, 2159), (i32::MIN, i32::MAX)] {
            assert_eq!(unpack(pack(x, y)), (x, y));
            assert_eq!(unpack(LPARAM(pack(x, y) as i64 as isize).0 as i64 as u64), (x, y));
        }
        for b in [Button::Left, Button::Right, Button::Middle, Button::X1, Button::X2] {
            assert_eq!(button_of(button_code(b)), b);
            assert_eq!(button_of((button_code(b) | 1 << 8) & 0xFF), b);
        }
    }
}
