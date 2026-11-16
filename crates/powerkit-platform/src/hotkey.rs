//! System-wide hotkeys via `RegisterHotKey`. The OS delivers them to the event loop,
//! which reports [`powerkit_core::Event::Hotkey`] with the id used at registration.
//!
//! Windows keeps some combinations for itself (and other programs take others), so
//! when `RegisterHotKey` refuses one it is caught in the low-level keyboard hook
//! instead, which sees keys before Windows acts on them. PowerToys does the same.

use crate::event_loop::{helper, push};
use crate::input::key_down;
use crate::keyhook::{KeyDecision, KeyboardHook};
use crate::{Context, Error, Result};
use powerkit_core::{Event, Mods, Shortcut};
use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};

/// A key no keyboard has; sent while Win or Alt is held so releasing it does not open
/// the Start menu or the menu bar after the shortcut was swallowed.
const NO_KEY: u32 = 0xFF;

thread_local! {
    static HOOKED: RefCell<HashMap<i32, KeyboardHook>> = RefCell::new(HashMap::new());
}

/// Register `shortcut` under `id`, from the keyboard hook when Windows or another
/// program already owns the combination.
pub fn register(id: i32, shortcut: Shortcut) -> Result<()> {
    match register_os(id, shortcut) {
        Err(Error::Other(taken)) if taken.ends_with(TAKEN) => {
            let hook = KeyboardHook::install(matcher(id, shortcut))
                .map_err(|e| Error::Other(format!("{taken}, and the keyboard hook failed: {e}")))?;
            tracing::info!("{shortcut} is caught by the keyboard hook because it is {TAKEN}");
            HOOKED.with(|h| h.borrow_mut().insert(id, hook));
            Ok(())
        }
        other => other,
    }
}

const TAKEN: &str = "already used by another application or by Windows";

/// The modifiers physically held now.
fn held() -> Mods {
    let mut m = Mods::NONE;
    for (flag, vks) in [
        (Mods::CTRL, &[0x11][..]),
        (Mods::ALT, &[0x12]),
        (Mods::SHIFT, &[0x10]),
        (Mods::WIN, &[0x5B, 0x5C]),
    ] {
        if vks.iter().any(|&vk| key_down(vk)) {
            m = m.union(flag);
        }
    }
    m
}

/// Swallows the shortcut's key while exactly its modifiers are held and reports it
/// once per press, as `MOD_NOREPEAT` does.
fn matcher(id: i32, shortcut: Shortcut) -> impl FnMut(u32, bool) -> KeyDecision {
    let mut pressed = false;
    move |vk, down| {
        if vk != shortcut.key {
            return KeyDecision::Pass;
        }
        if !down {
            // The release of a swallowed press is swallowed too.
            return if std::mem::take(&mut pressed) {
                KeyDecision::Replace(Vec::new())
            } else {
                KeyDecision::Pass
            };
        }
        if !pressed && held() != shortcut.mods {
            return KeyDecision::Pass;
        }
        if !pressed {
            pressed = true;
            tracing::debug!("{shortcut} pressed");
            push(Event::Hotkey(id));
        }
        if shortcut.mods.contains(Mods::WIN) || shortcut.mods.contains(Mods::ALT) {
            KeyDecision::Replace(vec![(NO_KEY, true), (NO_KEY, false)])
        } else {
            KeyDecision::Replace(Vec::new())
        }
    }
}

fn register_os(id: i32, shortcut: Shortcut) -> Result<()> {
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
            Error::Other(format!("{shortcut} is {TAKEN}"))
        } else {
            Error::Os {
                context: format!("RegisterHotKey({shortcut})"),
                source,
            }
        }
    })
}

pub fn unregister(id: i32) -> Result<()> {
    if HOOKED.with(|h| h.borrow_mut().remove(&id)).is_some() {
        return Ok(());
    }
    // SAFETY: plain value arguments.
    unsafe { UnregisterHotKey(Some(helper()?), id) }.context("UnregisterHotKey")
}
