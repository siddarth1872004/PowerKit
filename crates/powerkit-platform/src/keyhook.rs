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
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC_EX, VIRTUAL_KEY,
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

/// Several modules may hook the keyboard at once; they share one OS hook and are asked
/// in installation order, the first that does not pass deciding.
#[derive(Default)]
struct Hooks {
    os: Option<HHOOK>,
    next_id: u64,
    callbacks: Vec<(u64, Callback)>,
}

thread_local! {
    static HOOKS: RefCell<Hooks> = RefCell::new(Hooks::default());
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
                HOOKS.with(|h| match h.try_borrow_mut() {
                    Ok(mut h) => h
                        .callbacks
                        .iter_mut()
                        .map(|(_, f)| f(info.vkCode, down))
                        .find(|d| *d != KeyDecision::Pass)
                        .unwrap_or(KeyDecision::Pass),
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

/// Win, Shift, Ctrl and Alt, left and right.
const MODIFIERS: [u32; 8] = [0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5];
/// An unassigned key: pressing it between Win or Alt going down and up keeps their
/// release from opening Start or a menu bar.
const VK_NONE: u32 = 0xE8;
const VK_CONTROL: u32 = 0x11;
const VK_V: u32 = 0x56;

/// Press Ctrl+V in the window in front. Modifiers still held from the shortcut would
/// turn it into another shortcut, so give the user a moment to let go, then release
/// any still down. The waiting happens on a thread of its own: on the event loop it
/// would hold up the keyboard hook, and with it the very key releases it waits for.
pub fn press_paste() -> Result<()> {
    std::thread::Builder::new()
        .name("paste".into())
        .spawn(|| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while MODIFIERS.iter().any(|&vk| crate::input::key_down(vk)) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let mut strokes = vec![(VK_NONE, true), (VK_NONE, false)];
            strokes.extend(
                MODIFIERS
                    .iter()
                    .filter(|&&vk| crate::input::key_down(vk))
                    .map(|&vk| (vk, false)),
            );
            strokes.extend([(VK_CONTROL, true), (VK_V, true), (VK_V, false), (VK_CONTROL, false)]);
            if let Err(e) = send_keys(&strokes) {
                tracing::warn!(error = %e, "could not press Ctrl+V");
            }
        })
        .map(|_| ())
        .map_err(|e| Error::Other(format!("paste thread: {e}")))
}

/// Send key strokes as if typed, marked so the hook ignores them.
pub fn send_keys(strokes: &[Stroke]) -> Result<()> {
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
    send(&inputs)
}

/// Type `text` into the focused application as Unicode characters, marked so the hook
/// ignores them.
pub fn send_text(text: &str) -> Result<()> {
    let inputs: Vec<INPUT> = text
        .encode_utf16()
        .flat_map(|unit| [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP].map(move |f| (unit, f)))
        .map(|(unit, flags)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: MARKER,
                },
            },
        })
        .collect();
    send(&inputs)
}

pub(crate) fn send(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a valid slice of keyboard INPUT structures.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(Error::Os {
            context: format!("SendInput sent {sent} of {} keys", inputs.len()),
            source: windows::core::Error::from_thread(),
        });
    }
    Ok(())
}

/// Active while it exists; dropping it removes the callback, and the OS hook with the
/// last one.
pub struct KeyboardHook {
    id: u64,
    _not_send: PhantomData<*const ()>,
}

impl KeyboardHook {
    /// Must be called on the event-loop thread, whose message loop services the hook.
    pub fn install(callback: impl FnMut(u32, bool) -> KeyDecision + 'static) -> Result<KeyboardHook> {
        helper()?;
        HOOKS.with(|h| {
            let mut h = h
                .try_borrow_mut()
                .map_err(|_| Error::Other("keyboard hook installed from inside a key callback".into()))?;
            if h.os.is_none() {
                // SAFETY: the procedure matches HOOKPROC and lives for the whole program.
                let hook = unsafe {
                    let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance.into()), 0)
                }
                .context("keyboard hook")?;
                h.os = Some(hook);
            }
            let id = h.next_id;
            h.next_id += 1;
            h.callbacks.push((id, Box::new(callback)));
            Ok(KeyboardHook {
                id,
                _not_send: PhantomData,
            })
        })
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        HOOKS.with(|h| {
            let Ok(mut h) = h.try_borrow_mut() else {
                tracing::warn!("keyboard hook dropped inside a key callback; it stays installed");
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
