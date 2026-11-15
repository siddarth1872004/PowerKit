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

/// Whether a mouse button, as applications see it (after swapping for left-handed
/// use), is physically held right now.
pub fn button_down(button: crate::mousehook::Button) -> bool {
    use crate::mousehook::Button;
    // SAFETY: plain value argument.
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    // GetAsyncKeyState reports physical buttons.
    let vk = match (button, swapped) {
        (Button::Left, false) | (Button::Right, true) => 0x01,
        (Button::Right, false) | (Button::Left, true) => 0x02,
        (Button::Middle, _) => 0x04,
    };
    key_down(vk)
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

/// Move the pointer to a point in physical virtual-desktop pixels.
pub fn set_cursor_pos(x: i32, y: i32) -> Result<()> {
    // SAFETY: plain value arguments.
    unsafe { SetCursorPos(x, y) }.context("SetCursorPos")
}

/// Click the left button where the pointer is.
pub fn click_left() -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
    };
    let button = |flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    crate::keyhook::send(&[button(MOUSEEVENTF_LEFTDOWN), button(MOUSEEVENTF_LEFTUP)])
}

/// Whether the pointer is drawn; applications hide it while typing or playing video.
pub fn cursor_visible() -> bool {
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a valid out pointer with its size set.
    match unsafe { GetCursorInfo(&mut info) } {
        Ok(()) => info.flags.0 & CURSOR_SHOWING.0 != 0,
        // Unknown counts as shown, so nothing disappears by mistake.
        Err(_) => true,
    }
}
