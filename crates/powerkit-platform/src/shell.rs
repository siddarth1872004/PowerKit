//! Shell integration: opening things, startup registration, single-instance guard.

use crate::util::{from_wide, pcwstr, wide};
use crate::{Context, Error, Result};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, HANDLE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_BINARY,
    RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{CreateMutexW, GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, ShellExecuteW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};

/// A known folder of the user's, wherever it was moved to (OneDrive included).
fn known_folder(id: &windows::core::GUID, name: &str) -> Result<std::path::PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    // SAFETY: the returned string is read, then freed with the allocator Windows
    // used for it.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).context(format!("finding the {name} folder"))?;
        let text = String::from_utf16_lossy(path.as_wide());
        CoTaskMemFree(Some(path.0 as *const core::ffi::c_void));
        Ok(text.into())
    }
}

pub fn documents_dir() -> Result<std::path::PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Documents, "Documents")
}

pub fn pictures_dir() -> Result<std::path::PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Pictures, "Pictures")
}

pub fn videos_dir() -> Result<std::path::PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Videos, "Videos")
}

/// Move a file to the Recycle Bin, so a delete can be undone there.
pub fn recycle(path: &std::path::Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    // The list of paths ends with an empty one, hence two NULs.
    let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` is double-NUL terminated and outlives the call.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(crate::Error::Other(format!(
            "{} was not moved to the Recycle Bin (code {code:#x})",
            path.display()
        )));
    }
    Ok(())
}

/// Open a program, document, folder or URI with its default handler, the same way a
/// double-click in Explorer would. `target` and `args` are passed to the shell as-is;
/// no command interpreter is involved, so there is no shell-injection surface.
pub fn open(target: &str, args: Option<&str>) -> Result<()> {
    if target.trim().is_empty() {
        return Err(Error::Other("nothing to open".into()));
    }
    let wtarget = wide(target);
    let wargs = args.map(wide);
    // SAFETY: all strings are NUL-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            pcwstr(&wtarget),
            wargs
                .as_ref()
                .map(|a| pcwstr(a))
                .unwrap_or(windows::core::PCWSTR::null()),
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value greater than 32 on success.
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(Error::Other(match code {
            2 | 3 => format!("{target:?} was not found"),
            5 => format!("access to {target:?} was denied"),
            31 => format!("no application is associated with {target:?}"),
            other => format!("could not open {target:?} (shell error {other})"),
        }))
    }
}

/// Run `exe` with `args` as administrator (Windows asks the user first), wait for it
/// to finish and return its exit code. Blocks, so call it off the event loop thread.
pub fn run_elevated(exe: &std::path::Path, args: &str) -> Result<u32> {
    let wexe = wide(&exe.to_string_lossy());
    let wargs = wide(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: windows::core::w!("runas"),
        lpFile: pcwstr(&wexe),
        lpParameters: pcwstr(&wargs),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; `info` is a valid in/out structure.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            Error::Other("administrator permission was not given".into())
        } else {
            Error::Other(format!("could not start {} as administrator: {e}", exe.display()))
        });
    }
    let process = info.hProcess;
    if process.is_invalid() {
        return Err(Error::Other("the elevated process handle is missing".into()));
    }
    let mut code = 0u32;
    // SAFETY: `process` was returned open by ShellExecuteEx and is closed once here.
    let got = unsafe {
        WaitForSingleObject(process, INFINITE);
        let got = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        got
    };
    got.context("GetExitCodeProcess")?;
    Ok(code)
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `open_key` and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open_key(path: &str) -> Result<Key> {
    let wpath = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `wpath` is NUL-terminated; `key` is a valid out pointer.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            pcwstr(&wpath),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut key,
            None,
        )
    }
    .ok()
    .context(format!("opening HKCU\\{path}"))?;
    Ok(Key(key))
}

/// Write a string value under `HKEY_CURRENT_USER` (per-user, no elevation needed).
pub fn registry_set(path: &str, name: &str, value: &str) -> Result<()> {
    let key = open_key(path)?;
    let wname = wide(name);
    let wvalue = wide(value);
    // SAFETY: the byte slice covers exactly the NUL-terminated UTF-16 value.
    let bytes = unsafe { std::slice::from_raw_parts(wvalue.as_ptr().cast::<u8>(), wvalue.len() * 2) };
    // SAFETY: strings are NUL-terminated and outlive the call.
    unsafe { RegSetValueExW(key.0, pcwstr(&wname), None, REG_SZ, Some(bytes)) }
        .ok()
        .context(format!("writing HKCU\\{path}\\{name}"))
}

/// Read a string value under `HKEY_CURRENT_USER`; `None` when it does not exist.
pub fn registry_get(path: &str, name: &str) -> Result<Option<String>> {
    let wpath = wide(path);
    let wname = wide(name);
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf` is writable for `size` bytes; strings are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&wpath),
            pcwstr(&wname),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok().context(format!("reading HKCU\\{path}\\{name}"))?;
    Ok(Some(from_wide(&buf)))
}

/// Write a DWORD value under `HKEY_CURRENT_USER`.
pub fn registry_set_dword(path: &str, name: &str, value: u32) -> Result<()> {
    let key = open_key(path)?;
    let wname = wide(name);
    let bytes = value.to_le_bytes();
    // SAFETY: `wname` is NUL-terminated; `bytes` holds exactly one DWORD.
    unsafe { RegSetValueExW(key.0, pcwstr(&wname), None, REG_DWORD, Some(&bytes)) }
        .ok()
        .context(format!("writing HKCU\\{path}\\{name}"))
}

/// Read a DWORD value under `HKEY_CURRENT_USER`; `None` when it does not exist.
pub fn registry_get_dword(path: &str, name: &str) -> Result<Option<u32>> {
    let wpath = wide(path);
    let wname = wide(name);
    let mut value = 0u32;
    let mut size = 4u32;
    // SAFETY: `value` is writable for `size` bytes; strings are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&wpath),
            pcwstr(&wname),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok().context(format!("reading HKCU\\{path}\\{name}"))?;
    Ok(Some(value))
}

/// A binary value under `HKEY_CURRENT_USER`, or `None` when it is absent.
pub fn registry_get_binary(path: &str, name: &str) -> Result<Option<Vec<u8>>> {
    let wpath = wide(path);
    let wname = wide(name);
    let mut buf = vec![0u8; 256];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` is writable for `size` bytes; strings are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&wpath),
            pcwstr(&wname),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok().context(format!("reading HKCU\\{path}\\{name}"))?;
    buf.truncate(size as usize);
    Ok(Some(buf))
}

/// Delete a value under `HKEY_CURRENT_USER`; succeeds if it is already absent.
pub fn registry_delete(path: &str, name: &str) -> Result<()> {
    let key = open_key(path)?;
    let wname = wide(name);
    // SAFETY: `wname` is NUL-terminated.
    let status = unsafe { RegDeleteValueW(key.0, pcwstr(&wname)) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    status.ok().context(format!("deleting HKCU\\{path}\\{name}"))
}

/// Delete a key under `HKEY_CURRENT_USER` with all its subkeys and values; succeeds if
/// it is already absent.
pub fn registry_delete_tree(path: &str) -> Result<()> {
    let wpath = wide(path);
    // SAFETY: `wpath` is NUL-terminated.
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, pcwstr(&wpath)) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    status.ok().context(format!("deleting HKCU\\{path}"))
}

/// Register or unregister `command` to run when the user signs in (HKCU Run key).
pub fn set_launch_at_startup(name: &str, command: Option<&str>) -> Result<()> {
    match command {
        Some(cmd) => registry_set(RUN_KEY, name, cmd),
        None => registry_delete(RUN_KEY, name),
    }
}

pub fn launch_at_startup_command(name: &str) -> Result<Option<String>> {
    registry_get(RUN_KEY, name)
}

/// Put `<name>.lnk` starting `target` in the user's Start menu, which is what makes a
/// program show up in Windows search, or remove it when `target` is `None`.
pub fn set_start_menu_shortcut(name: &str, target: Option<&std::path::Path>) -> Result<()> {
    let link = known_folder(&windows::Win32::UI::Shell::FOLDERID_Programs, "Start menu")?.join(format!("{name}.lnk"));
    let Some(target) = target else {
        return match std::fs::remove_file(&link) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(Error::Other(format!("removing {}: {e}", link.display())))
            }
            _ => Ok(()),
        };
    };
    write_link(&link, target, "")
}

/// Put `<name>.lnk` on the desktop, starting `target` with `args`; returns where it went.
pub fn create_desktop_shortcut(name: &str, target: &std::path::Path, args: &str) -> Result<std::path::PathBuf> {
    let link = known_folder(&windows::Win32::UI::Shell::FOLDERID_Desktop, "Desktop")?.join(format!("{name}.lnk"));
    write_link(&link, target, args)?;
    Ok(link)
}

fn write_link(link: &std::path::Path, target: &std::path::Path, args: &str) -> Result<()> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    let exe = wide(&target.to_string_lossy());
    let dir = wide(&target.parent().unwrap_or(target).to_string_lossy());
    let arguments = wide(args);
    let file = wide(&link.to_string_lossy());
    // SAFETY: the strings are NUL-terminated and outlive the calls that read them.
    unsafe {
        // Already initialised on this thread is fine; the link object works either way.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let shell_link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).context("creating a shortcut")?;
        let persist = shell_link.cast::<IPersistFile>().context("IPersistFile")?;
        // The Start menu one is written at every start; leave a link that already
        // points here untouched.
        if link.exists() && persist.Load(pcwstr(&file), STGM_READ).is_ok() {
            let mut current = [0u16; 1024];
            let mut current_args = [0u16; 1024];
            if shell_link.GetPath(&mut current, std::ptr::null_mut(), 0).is_ok()
                && shell_link.GetArguments(&mut current_args).is_ok()
                && from_wide(&current).eq_ignore_ascii_case(&target.to_string_lossy())
                && from_wide(&current_args) == args
            {
                return Ok(());
            }
        }
        shell_link
            .SetPath(pcwstr(&exe))
            .context("setting the shortcut target")?;
        shell_link
            .SetArguments(pcwstr(&arguments))
            .context("setting the shortcut arguments")?;
        shell_link
            .SetWorkingDirectory(pcwstr(&dir))
            .context("setting the shortcut folder")?;
        persist
            .Save(pcwstr(&file), true)
            .context(format!("saving {}", link.display()))
    }
}

/// Holds a named mutex for the lifetime of the process to prevent a second instance.
pub struct SingleInstance {
    handle: HANDLE,
}

// SAFETY: a mutex handle may be closed from any thread.
unsafe impl Send for SingleInstance {}

impl SingleInstance {
    /// Returns `Ok(None)` when another process already holds `name`.
    pub fn acquire(name: &str) -> Result<Option<SingleInstance>> {
        let wname = wide(&format!("Local\\{name}"));
        // SAFETY: `wname` is NUL-terminated. GetLastError is read immediately after.
        unsafe {
            let handle = CreateMutexW(None, false, pcwstr(&wname)).context("CreateMutex")?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(SingleInstance { handle }))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: closing our own handle once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// Whether a full-screen Direct3D program (usually a game) is in front, which is what
/// Windows' own notification quiet hours treat as game mode.
pub fn game_running() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
    // SAFETY: no arguments; a failure only means the state is unknown.
    match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => state == QUNS_RUNNING_D3D_FULL_SCREEN,
        Err(e) => {
            tracing::debug!(error = %e, "could not query the notification state");
            false
        }
    }
}

/// Make the calling worker thread able to use COM and Windows Runtime objects (PDF
/// rendering, for one) for the rest of its life.
pub fn init_com_thread() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    // SAFETY: no pointers. S_FALSE (already initialised) is as good as S_OK here.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok() {
        tracing::debug!(error = %e, "COM was already set up differently on this thread");
    }
}

/// The picture Explorer shows for `path` in its largest views (a video frame, a
/// document's first page, album art, a folder's contents), at most `size` pixels on its
/// longer side: width, height and premultiplied BGRA pixels, top row first. Fails when
/// the file has only an icon, so callers can show something more useful instead.
pub fn thumbnail(path: &std::path::Path, size: u32) -> Result<(u32, u32, Vec<u32>)> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY,
    };
    let text = wide(&path.to_string_lossy());
    let side = i32::try_from(size).unwrap_or(i32::MAX);
    // SAFETY: the bitmap Windows returns is read into a buffer of the size described to
    // GetDIBits, then deleted; the memory DC is deleted on every path.
    unsafe {
        // Already initialised on this thread is fine.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let item: IShellItemImageFactory =
            SHCreateItemFromParsingName(pcwstr(&text), None).context(format!("finding {}", path.display()))?;
        let bitmap = item
            .GetImage(SIZE { cx: side, cy: side }, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK)
            .context("this file has no thumbnail")?;
        let mut info = BITMAP::default();
        let got = GetObjectW(
            HGDIOBJ(bitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut core::ffi::c_void),
        );
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        let result = if got == 0 || w <= 0 || h <= 0 {
            Err(Error::Other("the thumbnail could not be read".into()))
        } else {
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u32; w as usize * h as usize];
            let dc = CreateCompatibleDC(None);
            let lines = GetDIBits(
                dc,
                bitmap,
                0,
                h as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            let _ = DeleteDC(dc);
            if lines == 0 {
                Err(Error::Other("the thumbnail could not be read".into()))
            } else {
                // Thumbnails without transparency come back with alpha 0 everywhere.
                if pixels.iter().all(|p| p >> 24 == 0) {
                    pixels.iter_mut().for_each(|p| *p |= 0xFF00_0000);
                }
                Ok((w as u32, h as u32, pixels))
            }
        };
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        result
    }
}

/// Let whichever process comes next (the running PowerKit) bring a window to the
/// front. A process started from Explorer may; the instance it hands over to may not.
pub fn allow_foreground() {
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
    // SAFETY: plain call. Failure only means the next window may not come forward.
    if let Err(e) = unsafe { AllowSetForegroundWindow(ASFW_ANY) } {
        tracing::debug!("AllowSetForegroundWindow: {e}");
    }
}

/// Show `path` selected in its folder's Explorer window, with its name ready to edit.
/// Blocks while Explorer finds or opens the window, so call it off the event loop.
pub fn select_to_rename(path: &std::path::Path) -> Result<()> {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID, SHOpenFolderAndSelectItems, OFASI_EDIT,
    };
    let text = wide(&path.to_string_lossy());
    // SAFETY: the item list is created, cloned and freed here; the child pointer
    // points into `item`, which outlives the call that reads it.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let item = ILCreateFromPathW(pcwstr(&text));
        if item.is_null() {
            return Err(Error::Other(format!("{} was not found", path.display())));
        }
        let folder = ILClone(item);
        let result = if folder.is_null() || !ILRemoveLastID(Some(folder)).as_bool() {
            Err(Error::Other(format!("{} has no folder to show it in", path.display())))
        } else {
            let child = ILFindLastID(item).cast_const();
            SHOpenFolderAndSelectItems(folder, Some(&[child]), OFASI_EDIT).context("SHOpenFolderAndSelectItems")
        };
        ILFree(Some(folder));
        ILFree(Some(item));
        result
    }
}

/// Tell Explorer that file type registrations changed, so it picks up new or removed
/// preview handlers and thumbnail providers without a restart.
pub fn file_types_changed() {
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    // SAFETY: the notification takes no items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_instance_guard() {
        let name = format!("PowerKit.Test.Instance.{}", std::process::id());
        let first = SingleInstance::acquire(&name).unwrap();
        assert!(first.is_some());
        assert!(SingleInstance::acquire(&name).unwrap().is_none());
        drop(first);
        assert!(SingleInstance::acquire(&name).unwrap().is_some());
    }

    #[test]
    fn registry_round_trip() {
        let path = r"Software\PowerKit\Tests";
        let name = format!("value-{}", std::process::id());
        assert_eq!(registry_get(path, &name).unwrap(), None);
        registry_set(path, &name, "héllo \"quoted\"").unwrap();
        assert_eq!(registry_get(path, &name).unwrap().as_deref(), Some("héllo \"quoted\""));
        registry_delete(path, &name).unwrap();
        registry_delete(path, &name).unwrap(); // idempotent
        assert_eq!(registry_get(path, &name).unwrap(), None);
    }

    #[test]
    fn open_rejects_empty_and_missing_targets() {
        assert!(open("  ", None).is_err());
        let err = open(r"C:\definitely\missing\powerkit-test.exe", None).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }
}
