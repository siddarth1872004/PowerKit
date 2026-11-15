//! The values under [`KEY`] that powerkit.exe and the DLL share.

use crate::KEY;
use std::ffi::c_void;
use windows::Win32::System::Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD};
use windows_core::HSTRING;

pub(crate) fn dword(name: &str) -> Option<u32> {
    let (mut value, mut size) = (0u32, 4u32);
    // SAFETY: value and size point to locals of the size given.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(KEY),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(value)
}

#[cfg(feature = "server")]
pub(crate) fn string(name: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: buf is size bytes long.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(KEY),
            &HSTRING::from(name),
            windows::Win32::System::Registry::RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

pub(crate) fn set_dword(name: &str, value: u32) -> windows_core::Result<()> {
    let bytes = value.to_le_bytes();
    // SAFETY: the strings outlive the call; `bytes` is 4 bytes long, as REG_DWORD needs.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(KEY),
            &HSTRING::from(name),
            REG_DWORD.0,
            Some(bytes.as_ptr() as *const c_void),
            bytes.len() as u32,
        )
    }
    .ok()
}
