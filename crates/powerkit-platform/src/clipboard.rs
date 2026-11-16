//! Clipboard access: text, HTML, pictures and files.

use crate::event_loop::helper;
use crate::{Context, Error, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

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

fn register(name: &str) -> Result<u32> {
    let wide = crate::util::wide(name);
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    match unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) } {
        0 => Err(Error::Os {
            context: format!("RegisterClipboardFormat({name})"),
            source: windows::core::Error::from_thread(),
        }),
        id => Ok(id),
    }
}

/// The bytes of one clipboard format, or `None` when the clipboard has none.
fn get_bytes(format: u32) -> Result<Option<Vec<u8>>> {
    // SAFETY: format availability needs no open clipboard.
    if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
        return Ok(None);
    }
    let _open = Open::new()?;
    // SAFETY: the clipboard is open; the data handle stays owned by the system and is
    // only read while locked, never beyond its allocated size.
    unsafe {
        let Ok(h) = GetClipboardData(format) else {
            return Ok(None);
        };
        let mem = HGLOBAL(h.0);
        let ptr = GlobalLock(mem) as *const u8;
        if ptr.is_null() {
            return Err(Error::Other("GlobalLock(clipboard) failed".into()));
        }
        let bytes = std::slice::from_raw_parts(ptr, GlobalSize(mem)).to_vec();
        let _ = GlobalUnlock(mem);
        Ok(Some(bytes))
    }
}

/// Replace the clipboard contents with these formats.
fn put(formats: &[(u32, Vec<u8>)]) -> Result<()> {
    let _open = Open::new()?;
    // SAFETY: as in `set_text`, for each block.
    unsafe {
        EmptyClipboard().context("EmptyClipboard")?;
        for (format, data) in formats {
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)).context("GlobalAlloc")?;
            let ptr = GlobalLock(mem) as *mut u8;
            if ptr.is_null() {
                let _ = GlobalFree(Some(mem));
                return Err(Error::Other("GlobalLock failed".into()));
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            let _ = GlobalUnlock(mem);
            if let Err(e) = SetClipboardData(*format, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(Error::Os {
                    context: "SetClipboardData".into(),
                    source: e,
                });
            }
        }
    }
    Ok(())
}

/// The clipboard's HTML (the CF_HTML text, header included), or `None`.
pub fn get_html() -> Result<Option<String>> {
    let Some(bytes) = get_bytes(register("HTML Format")?)? else {
        return Ok(None);
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(Some(String::from_utf8_lossy(&bytes[..end]).into_owned()))
}

/// The clipboard's picture as a packed DIB (header, colour masks or palette, then
/// pixels), or `None`. Windows makes this from any bitmap put on the clipboard.
pub fn get_dib() -> Result<Option<Vec<u8>>> {
    get_bytes(CF_DIB)
}

/// Put files on the clipboard as File Explorer copies them, so pasting into a folder
/// copies them there.
pub fn set_files(paths: &[std::path::PathBuf]) -> Result<()> {
    // DROPFILES: offset of the list, a point, "non-client", then "wide" = 1.
    let mut drop: Vec<u8> = [20u32, 0, 0, 0, 1].iter().flat_map(|v| v.to_le_bytes()).collect();
    for path in paths {
        use std::os::windows::ffi::OsStrExt;
        drop.extend(path.as_os_str().encode_wide().chain([0]).flat_map(u16::to_le_bytes));
    }
    drop.extend([0, 0]);
    // DROPEFFECT_COPY, so a paste copies rather than moves.
    let effect = 1u32.to_le_bytes().to_vec();
    put(&[(CF_HDROP, drop), (register("Preferred DropEffect")?, effect)])
}
