//! System-wide key remapping support: a low-level keyboard hook whose callback decides
//! synchronously, for every physical key event, whether it passes through or is
//! replaced by other key strokes (sent with `SendInput`).

use crate::event_loop::helper;
use crate::{Context, Error, Result};
use std::cell::RefCell;
use std::marker::PhantomData;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC_EX, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Marks input PowerKit injected so the hook lets it through untouched.
const MARKER: usize = 0x504B_4D31;

/// One key going down (`true`) or up.
pub type Stroke = (u32, bool);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyDecision {
    Pass,
    /// Swallow the event and send these strokes instead (none: just swallow).
    Replace(Vec<Stroke>),
}

type Callback = Box<dyn FnMut(u32, bool) -> KeyDecision>;

thread_local! {
    static CALLBACK: RefCell<Option<Callback>> = const { RefCell::new(None) };
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.dwExtraInfo != MARKER {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            // A panic must not unwind into the OS, and a busy callback (re-entered while
            // the thread pumped messages) means the key passes unchanged.
            let decision = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                CALLBACK.with(|c| match c.try_borrow_mut() {
                    Ok(mut cb) => cb.as_mut().map_or(KeyDecision::Pass, |f| f(info.vkCode, down)),
                    Err(_) => KeyDecision::Pass,
                })
            }))
            .unwrap_or(KeyDecision::Pass);
            if let KeyDecision::Replace(strokes) = decision {
                if let Err(e) = send_keys(&strokes) {
                    tracing::warn!(error = %e, "could not send remapped keys");
                }
                return LRESULT(1);
            }
        }
    }
    // SAFETY: forwarding the unmodified arguments of this callback.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Keys whose scan code needs the extended flag to be told apart from the numpad or
/// left-hand variants.
fn extended(vk: u32) -> bool {
    matches!(
        vk,
        0x21..=0x28 // PageUp..Down arrows
            | 0x2C..=0x2E // PrintScreen, Insert, Delete
            | 0x5B..=0x5D // LWin, RWin, Menu
            | 0x6F // NumpadDivide
            | 0x90 // NumLock
            | 0xA3 | 0xA5 // RCtrl, RAlt
            | 0xA6..=0xB7 // browser and media keys
    )
}

/// Send key strokes as if typed, marked so the hook ignores them.
pub fn send_keys(strokes: &[Stroke]) -> Result<()> {
    if strokes.is_empty() {
        return Ok(());
    }
    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|&(vk, down)| {
            // SAFETY: plain value arguments.
            let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX) };
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            if extended(vk) || scan & 0xFF00 == 0xE000 {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk as u16),
                        wScan: (scan & 0xFF) as u16,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: MARKER,
                    },
                },
            }
        })
        .collect();
    // SAFETY: `inputs` is a valid slice of keyboard INPUT structures.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(Error::Os {
            context: format!("SendInput sent {sent} of {} keys", inputs.len()),
            source: windows::core::Error::from_thread(),
        });
    }
    Ok(())
}

/// Active while it exists; dropping it removes the hook and the callback.
pub struct KeyboardHook {
    hook: HHOOK,
    _not_send: PhantomData<*const ()>,
}

impl KeyboardHook {
    /// Must be called on the event-loop thread, whose message loop services the hook.
    /// Only one hook may be installed per thread.
    pub fn install(callback: impl FnMut(u32, bool) -> KeyDecision + 'static) -> Result<KeyboardHook> {
        helper()?;
        let busy = CALLBACK.with(|c| c.borrow().is_some());
        if busy {
            return Err(Error::Other("a keyboard remapping hook is already installed".into()));
        }
        CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(callback)));
        // SAFETY: the procedure matches HOOKPROC and lives for the whole program.
        let hook = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance.into()), 0)
        };
        match hook {
            Ok(hook) => Ok(KeyboardHook {
                hook,
                _not_send: PhantomData,
            }),
            Err(e) => {
                CALLBACK.with(|c| *c.borrow_mut() = None);
                Err(e).context("keyboard hook")
            }
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        // SAFETY: the hook was installed by this object and is removed once.
        let _ = unsafe { UnhookWindowsHookEx(self.hook) };
        CALLBACK.with(|c| {
            if let Ok(mut c) = c.try_borrow_mut() {
                *c = None;
            }
        });
    }
}
