//! Environment variables as Windows stores them: the user's under
//! `HKCU\Environment`, the machine's under `HKLM\...\Session Manager\Environment`
//! (readable by anyone, writable only by administrators).

use crate::util::{pcwstr, wide};
use crate::{Context, Result};
use windows::core::PWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SAM_FLAGS, REG_SZ,
    REG_VALUE_TYPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

const USER_KEY: &str = "Environment";
const SYSTEM_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub name: String,
    pub value: String,
    /// Stored as REG_EXPAND_SZ, so `%OTHER%` references are expanded when read.
    pub expand: bool,
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `open` and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn where_(system: bool) -> String {
    if system {
        format!(r"HKLM\{SYSTEM_KEY}")
    } else {
        format!(r"HKCU\{USER_KEY}")
    }
}

fn open(system: bool, access: REG_SAM_FLAGS) -> Result<Key> {
    let (root, path) = if system {
        (HKEY_LOCAL_MACHINE, SYSTEM_KEY)
    } else {
        (HKEY_CURRENT_USER, USER_KEY)
    };
    let wpath = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `wpath` is NUL-terminated; `key` is a valid out pointer.
    unsafe { RegOpenKeyExW(root, pcwstr(&wpath), None, access, &mut key) }
        .ok()
        .context(format!("opening {}", where_(system)))?;
    Ok(Key(key))
}

/// The string variables of one scope, in registry order. Values of other types are
/// not environment variables Windows hands to programs, so they are left out.
pub fn list(system: bool) -> Result<Vec<Var>> {
    let key = open(system, KEY_QUERY_VALUE)?;
    let (mut name_max, mut data_max) = (0u32, 0u32);
    // SAFETY: out pointers are valid for the call.
    unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&raw mut name_max),
            Some(&raw mut data_max),
            None,
            None,
        )
    }
    .ok()
    .context(format!("reading {}", where_(system)))?;
    let mut name = vec![0u16; name_max as usize + 1];
    let mut data = vec![0u8; data_max as usize + 2];
    let mut vars = Vec::new();
    let mut index = 0;
    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: the lengths passed are the buffers' sizes.
        let err = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&raw mut kind),
                Some(data.as_mut_ptr()),
                Some(&raw mut data_len),
            )
        };
        if err == ERROR_NO_MORE_ITEMS {
            break;
        }
        if err == ERROR_MORE_DATA {
            // Another program grew a value after the sizes were read; retry larger.
            name.resize(name.len() * 2, 0);
            data.resize(data.len() * 2, 0);
            continue;
        }
        err.ok().context(format!("reading {}", where_(system)))?;
        index += 1;
        let kind = REG_VALUE_TYPE(kind);
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            continue;
        }
        let units: Vec<u16> = data[..data_len as usize]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        vars.push(Var {
            name: String::from_utf16_lossy(&name[..name_len as usize]),
            value: String::from_utf16_lossy(&units[..end]),
            expand: kind == REG_EXPAND_SZ,
        });
    }
    Ok(vars)
}

/// Create or replace a variable. System variables need administrator rights.
pub fn set(system: bool, name: &str, value: &str, expand: bool) -> Result<()> {
    let key = open(system, KEY_SET_VALUE)?;
    let wname = wide(name);
    let wvalue = wide(value);
    // SAFETY: the byte slice covers exactly the NUL-terminated UTF-16 value.
    let bytes = unsafe { std::slice::from_raw_parts(wvalue.as_ptr().cast::<u8>(), wvalue.len() * 2) };
    let kind = if expand { REG_EXPAND_SZ } else { REG_SZ };
    // SAFETY: strings are NUL-terminated and outlive the call.
    unsafe { RegSetValueExW(key.0, pcwstr(&wname), None, kind, Some(bytes)) }
        .ok()
        .context(format!("writing {name} in {}", where_(system)))
}

/// Delete a variable; one that does not exist counts as deleted.
pub fn remove(system: bool, name: &str) -> Result<()> {
    let key = open(system, KEY_SET_VALUE)?;
    let wname = wide(name);
    // SAFETY: `wname` is NUL-terminated and outlives the call.
    let err = unsafe { RegDeleteValueW(key.0, pcwstr(&wname)) };
    if err == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    err.ok().context(format!("deleting {name} in {}", where_(system)))
}

/// Tell running programs (Explorer above all, which starts new ones) that the
/// variables changed. Waits up to a few seconds per hung window, so call it off the
/// event loop thread.
pub fn broadcast() {
    let what = wide("Environment");
    // SAFETY: `what` outlives the call, which returns only after every window answered
    // or timed out.
    let sent = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(what.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        )
    };
    if sent.0 == 0 {
        // Programs that missed it see the change after they restart.
        tracing::warn!(error = %windows::core::Error::from_thread(), "announcing the environment change failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_variables() {
        let system = list(true).unwrap();
        assert!(system
            .iter()
            .any(|v| v.name.eq_ignore_ascii_case("OS") && v.value == "Windows_NT"));

        let name = format!("POWERKIT_TEST_{}", std::process::id());
        set(false, &name, "%TEMP%\\x", true).unwrap();
        let found = list(false).unwrap().into_iter().find(|v| v.name == name);
        remove(false, &name).unwrap();
        assert_eq!(
            found,
            Some(Var {
                name: name.clone(),
                value: "%TEMP%\\x".into(),
                expand: true
            })
        );
        assert!(!list(false).unwrap().iter().any(|v| v.name == name));
        remove(false, &name).unwrap();
    }
}
