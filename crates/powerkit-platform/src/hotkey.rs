//! System-wide hotkeys via `RegisterHotKey`. The OS delivers them to the event loop,
//! which reports [`powerkit_core::Event::Hotkey`] with the id used at registration.

use crate::event_loop::helper;
use crate::{Context, Error, Result};
use powerkit_core::{Mods, Shortcut};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};

/// Register `shortcut` under `id`. Fails when another application (or Windows itself)
/// already owns the combination; the error message says so.
pub fn register(id: i32, shortcut: Shortcut) -> Result<()> {
    if !shortcut.is_valid_global() {
        return Err(Error::Other(format!(
            "{shortcut} needs a modifier key to be a global shortcut"
        )));
    }
    let mut mods = MOD_NOREPEAT;
    for (m, flag) in [
        (Mods::CTRL, MOD_CONTROL),
        (Mods::ALT, MOD_ALT),
        (Mods::SHIFT, MOD_SHIFT),
        (Mods::WIN, MOD_WIN),
    ] {
        if shortcut.mods.contains(m) {
            mods |= flag;
        }
    }
    let _: HOT_KEY_MODIFIERS = mods;
    // SAFETY: plain value arguments; the helper window belongs to this thread.
    unsafe { RegisterHotKey(Some(helper()?), id, mods, shortcut.key) }.map_err(|source| {
        // ERROR_HOTKEY_ALREADY_REGISTERED
        if source.code().0 as u32 == 0x8007_0581 {
            Error::Other(format!(
                "{shortcut} is already used by another application or by Windows"
            ))
        } else {
            Error::Os {
                context: format!("RegisterHotKey({shortcut})"),
                source,
            }
        }
    })
}

pub fn unregister(id: i32) -> Result<()> {
    // SAFETY: plain value arguments.
    unsafe { UnregisterHotKey(Some(helper()?), id) }.context("UnregisterHotKey")
}
