//! Modal input capture for tools that take over the mouse while active (Color Picker).
//! Low-level hooks swallow clicks, the wheel and Escape/Enter so they never reach the
//! application under the cursor, and report them through the event loop as
//! [`Event::Mouse`] and [`Event::Key`]. Cursor movement is not captured.

use crate::event_loop::{helper, push};
use crate::{Context, Result};
use powerkit_core::{Event, MouseEvent};
use std::marker::PhantomData;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let (x, y) = (info.pt.x, info.pt.y);
        let event = match wparam.0 as u32 {
            WM_LBUTTONDOWN => Some(MouseEvent::LeftDown { x, y }),
            WM_LBUTTONUP => Some(MouseEvent::LeftUp { x, y }),
            WM_RBUTTONDOWN => Some(MouseEvent::RightDown { x, y }),
            WM_RBUTTONUP => Some(MouseEvent::RightUp { x, y }),
            WM_MOUSEWHEEL => Some(MouseEvent::Wheel {
                delta: (info.mouseData >> 16) as u16 as i16 as i32,
            }),
            WM_MBUTTONDOWN | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP | WM_MOUSEHWHEEL => None,
            // SAFETY: forwarding the unmodified arguments of this callback.
            _ => return unsafe { CallNextHookEx(None, code, wparam, lparam) },
        };
        if let Some(e) = event {
            push(Event::Mouse(e));
        }
        return LRESULT(1);
    }
    // SAFETY: as above.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT.
        let vk = unsafe { (*(lparam.0 as *const KBDLLHOOKSTRUCT)).vkCode };
        if vk == VK_ESCAPE.0 as u32 || vk == VK_RETURN.0 as u32 {
            if matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
                push(Event::Key(vk));
            }
            return LRESULT(1);
        }
    }
    // SAFETY: forwarding the unmodified arguments of this callback.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Active while it exists; dropping it gives the mouse and keyboard back.
pub struct InputCapture {
    mouse: HHOOK,
    keyboard: HHOOK,
    _not_send: PhantomData<*const ()>,
}

impl InputCapture {
    /// Must be called on the event-loop thread: low-level hooks are serviced by its
    /// message loop, which must keep running for input to stay responsive.
    pub fn new() -> Result<InputCapture> {
        helper()?;
        // SAFETY: both procedures match HOOKPROC and live for the whole program; the
        // first hook is removed if the second cannot be installed.
        unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let mouse =
                SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(instance.into()), 0).context("mouse hook")?;
            let keyboard = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance.into()), 0) {
                Ok(h) => h,
                Err(e) => {
                    let _ = UnhookWindowsHookEx(mouse);
                    return Err(e).context("keyboard hook");
                }
            };
            Ok(InputCapture {
                mouse,
                keyboard,
                _not_send: PhantomData,
            })
        }
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        // SAFETY: both hooks were installed by this object and are removed once.
        unsafe {
            let _ = UnhookWindowsHookEx(self.mouse);
            let _ = UnhookWindowsHookEx(self.keyboard);
        }
    }
}

/// Whether a key (virtual-key code) is physically held right now. Works while another
/// application has focus, which is what drag tracking needs.
pub fn key_down(vk: u32) -> bool {
    // SAFETY: plain value argument.
    let state = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) };
    state as u16 & 0x8000 != 0
}

/// Current cursor position in physical virtual-desktop pixels.
pub fn cursor_pos() -> Result<(i32, i32)> {
    let mut pt = POINT::default();
    // SAFETY: `pt` is a valid out pointer.
    unsafe { GetCursorPos(&mut pt) }.context("GetCursorPos")?;
    Ok((pt.x, pt.y))
}

/// Whether Caps Lock is toggled on.
pub fn caps_lock_on() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
    // SAFETY: plain value argument.
    let state = unsafe { GetKeyState(VK_CAPITAL.0 as i32) };
    state & 1 != 0
}
