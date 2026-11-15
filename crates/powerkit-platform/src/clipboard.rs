//! Plain-text clipboard access.

use crate::event_loop::helper;
use crate::{Context, Error, Result};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

const CF_UNICODETEXT: u32 = 13;

/// Closes the clipboard when dropped so every exit path releases it.
struct Open;

impl Open {
    fn new() -> Result<Open> {
        // Another process may hold the clipboard briefly (clipboard managers, RDP).
        let owner = helper()?;
        for attempt in 0..10 {
            // SAFETY: the helper window belongs to this thread.
            match unsafe { OpenClipboard(Some(owner)) } {
                Ok(()) => return Ok(Open),
                Err(_) if attempt < 9 => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(e) => {
                    return Err(Error::Os {
                        context: "OpenClipboard (another application is using the clipboard)".into(),
                        source: e,
                    })
                }
            }
        }
        unreachable!()
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the clipboard was opened by this object.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Replace the clipboard contents with `text`.
pub fn set_text(text: &str) -> Result<()> {
    let data: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = data.len() * 2;
    let _open = Open::new()?;
    // SAFETY: the memory block is allocated with room for `data`, locked while it is
    // written, and owned by the system once SetClipboardData succeeds; it is freed here
    // on every path where ownership was not transferred.
    unsafe {
        EmptyClipboard().context("EmptyClipboard")?;
        let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes).context("GlobalAlloc")?;
        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(Error::Other("GlobalLock failed".into()));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        let _ = GlobalUnlock(mem);
        if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))) {
            let _ = GlobalFree(Some(mem));
            return Err(Error::Os {
                context: "SetClipboardData".into(),
                source: e,
            });
        }
    }
    Ok(())
}

/// Text on the clipboard, or `None` when it holds no text.
pub fn get_text() -> Result<Option<String>> {
    // SAFETY: format availability needs no open clipboard.
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_err() {
        return Ok(None);
    }
    let _open = Open::new()?;
    // SAFETY: the clipboard is open; the data handle stays owned by the system and is
    // only read while locked, never beyond its allocated size.
    unsafe {
        let Ok(h) = GetClipboardData(CF_UNICODETEXT) else {
            return Ok(None);
        };
        let mem = HGLOBAL(h.0);
        let ptr = GlobalLock(mem) as *const u16;
        if ptr.is_null() {
            return Err(Error::Other("GlobalLock(clipboard) failed".into()));
        }
        let units = GlobalSize(mem) / 2;
        let slice = std::slice::from_raw_parts(ptr, units);
        let text = crate::util::from_wide(slice);
        let _ = GlobalUnlock(mem);
        Ok(Some(text))
    }
}
