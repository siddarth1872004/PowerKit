//! Modal input capture for tools that take over the mouse while active (Color Picker).
//! Low-level hooks swallow clicks, the wheel and Escape/Enter so they never reach the
//! application under the cursor, and report them through the event loop as
//! [`Event::Mouse`] and [`Event::Key`]. Cursor movement is not captured.

use crate::event_loop::{helper, push};
use crate::{Context, Result};
use powerkit_core::{Event, MouseEvent};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Keys, besides Escape and Enter, that the current capture takes: one bit per
/// virtual-key code.
static TAKEN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn taken(vk: u32) -> bool {
    vk < 256 && TAKEN[(vk / 64) as usize].load(Ordering::Relaxed) & (1 << (vk % 64)) != 0
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT.
        let vk = unsafe { (*(lparam.0 as *const KBDLLHOOKSTRUCT)).vkCode };
        if vk == VK_ESCAPE.0 as u32 || vk == VK_RETURN.0 as u32 || taken(vk) {
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
        Self::with_keys(&[])
    }

    /// Also takes `keys` (virtual-key codes, any modifiers) and reports them as
    /// [`Event::Key`]. Only one capture is active at a time.
    pub fn with_keys(keys: &[u32]) -> Result<InputCapture> {
        helper()?;
        for (i, slot) in TAKEN.iter().enumerate() {
            let bits = keys
                .iter()
                .filter(|k| (**k / 64) as usize == i)
                .fold(0u64, |b, k| b | 1 << (k % 64));
            slot.store(bits, Ordering::Relaxed);
        }
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
        for slot in &TAKEN {
            slot.store(0, Ordering::Relaxed);
        }
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
        (Button::X1, _) => 0x05,
        (Button::X2, _) => 0x06,
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

fn mouse(
    dx: i32,
    dy: i32,
    data: i32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT};
    crate::keyhook::send(&[INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }])
}

/// Move the pointer as the mouse would, to a point in physical virtual-desktop
/// pixels; unlike [`set_cursor_pos`] this wakes the screen and shows hover effects.
pub fn send_move(x: i32, y: i32) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
    };
    let v = crate::monitor::virtual_screen();
    let scale = |p: i32, lo: i32, size: i32| ((i64::from(p - lo) * 65535) / i64::from((size - 1).max(1))) as i32;
    mouse(
        scale(x, v.left, v.width()),
        scale(y, v.top, v.height()),
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

/// Press or release a mouse button as the mouse would.
pub fn send_button(button: crate::mousehook::Button, down: bool) -> Result<()> {
    use crate::mousehook::Button;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let (flags, data) = match (button, down) {
        (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::X1, true) => (MOUSEEVENTF_XDOWN, 1),
        (Button::X1, false) => (MOUSEEVENTF_XUP, 1),
        (Button::X2, true) => (MOUSEEVENTF_XDOWN, 2),
        (Button::X2, false) => (MOUSEEVENTF_XUP, 2),
    };
    mouse(0, 0, data, flags)
}

/// Turn the wheel; 120 is one notch.
pub fn send_wheel(delta: i32, horizontal: bool) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL};
    mouse(
        0,
        0,
        delta,
        if horizontal {
            MOUSEEVENTF_HWHEEL
        } else {
            MOUSEEVENTF_WHEEL
        },
    )
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
