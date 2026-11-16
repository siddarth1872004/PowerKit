//! Writes powerkit_preview.dll to PowerKit's folder and registers its handlers for the
//! user under HKCU\Software\Classes. A handler that replaces another one for an
//! extension keeps the old one's CLSID, which is put back when PowerKit's goes.

use anyhow::{Context, Result};
use powerkit_platform::shell;
use powerkit_preview::{Handler, Kind, DLL, HANDLERS, PREVHOST_APPID};
use std::path::{Path, PathBuf};

pub(crate) const SERVER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/powerkit_preview.dll"));
const CLASSES: &str = r"Software\Classes";
const PREVIEW_LIST: &str = r"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers";
/// The handlers PowerKit's replaced, by `<extension>|<shellex key>`.
const BACKUP: &str = r"Software\PowerKit\Preview\Backup";

pub(crate) fn folder(root: &Path) -> PathBuf {
    root.join("preview")
}

/// The folder for this build's DLL: a new DLL gets a new folder, because the old one
/// stays loaded (and locked) in prevhost.exe and Explorer for a while.
fn folder_name() -> String {
    // FNV-1a: stable across builds and Rust versions, unlike the std hasher.
    let hash = SERVER.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    format!("{hash:016x}")
}

pub(crate) fn write_dll(root: &Path) -> Result<PathBuf> {
    let dir = folder(root).join(folder_name());
    let dll = dir.join(DLL);
    if std::fs::read(&dll).ok().as_deref() != Some(SERVER) {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&dll, SERVER).with_context(|| format!("writing {}", dll.display()))?;
    }
    Ok(dll)
}

/// Delete the DLLs of earlier builds (all of them with `keep` None); one still loaded
/// somewhere is left for next time.
fn remove_old_folders(root: &Path, keep: Option<&str>) {
    let Ok(dir) = std::fs::read_dir(folder(root)) else {
        return;
    };
    for entry in dir.flatten() {
        if Some(entry.file_name().to_string_lossy().as_ref()) != keep && entry.path().is_dir() {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                tracing::debug!(folder = %entry.path().display(), "old preview handler files not removed yet: {e}");
            }
        }
    }
}

fn clsid_key(handler: &Handler) -> String {
    format!(r"{CLASSES}\CLSID\{}", handler.clsid_string())
}

fn shellex_key(handler: &Handler, extension: &str) -> String {
    format!(r"{CLASSES}\{extension}\shellex\{}", handler.shellex_iid())
}

fn backup_name(handler: &Handler, extension: &str) -> String {
    format!("{extension}|{}", handler.shellex_iid())
}

fn is_ours(value: Option<&str>, handler: &Handler) -> bool {
    value.is_some_and(|v| v.eq_ignore_ascii_case(&handler.clsid_string()))
}

fn register(handler: &Handler, dll: &Path) -> Result<()> {
    let clsid = handler.clsid_string();
    let key = clsid_key(handler);
    shell::registry_set(&key, "", handler.name)?;
    if handler.kind == Kind::Preview {
        shell::registry_set(&key, "AppID", PREVHOST_APPID)?;
        // The preview host otherwise runs it at low integrity, where it could not
        // open the links in a document.
        shell::registry_set_dword(&key, "DisableLowILProcessIsolation", 1)?;
        shell::registry_set(PREVIEW_LIST, &clsid, handler.name)?;
    }
    let server = format!(r"{key}\InprocServer32");
    shell::registry_set(&server, "", &dll.to_string_lossy())?;
    shell::registry_set(&server, "ThreadingModel", "Apartment")?;
    for extension in handler.extensions {
        let shellex = shellex_key(handler, extension);
        let current = shell::registry_get(&shellex, "")?;
        if is_ours(current.as_deref(), handler) {
            continue;
        }
        if let Some(other) = current.filter(|c| !c.is_empty()) {
            shell::registry_set(BACKUP, &backup_name(handler, extension), &other)?;
        }
        shell::registry_set(&shellex, "", &clsid)?;
    }
    Ok(())
}

fn unregister(handler: &Handler) -> Result<()> {
    for extension in handler.extensions {
        let shellex = shellex_key(handler, extension);
        if !is_ours(shell::registry_get(&shellex, "")?.as_deref(), handler) {
            continue;
        }
        let name = backup_name(handler, extension);
        match shell::registry_get(BACKUP, &name)? {
            Some(other) => {
                shell::registry_set(&shellex, "", &other)?;
                shell::registry_delete(BACKUP, &name)?;
            }
            None => shell::registry_delete_tree(&shellex)?,
        }
    }
    shell::registry_delete(PREVIEW_LIST, &handler.clsid_string())?;
    shell::registry_delete_tree(&clsid_key(handler))?;
    Ok(())
}

/// Register the handlers `on` says to and unregister the rest. Returns how many are on.
pub(crate) fn sync(root: &Path, on: impl Fn(&Handler) -> bool) -> Result<usize> {
    let wanted: Vec<&Handler> = HANDLERS.iter().filter(|h| on(h)).collect();
    let dll = if wanted.is_empty() {
        None
    } else {
        Some(write_dll(root)?)
    };
    for handler in &HANDLERS {
        match &dll {
            Some(dll) if on(handler) => register(handler, dll),
            _ => unregister(handler),
        }
        .with_context(|| format!("setting up {}", handler.name))?;
    }
    shell::file_types_changed();
    let keep = dll.is_some().then(folder_name);
    remove_old_folders(root, keep.as_deref());
    Ok(wanted.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP, HBITMAP};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, IClassFactory, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::Controls::RichEdit::{EM_GETTEXTEX, GETTEXTEX, GT_NOHIDDENTEXT};
    use windows::Win32::UI::Shell::PropertiesSystem::IInitializeWithStream;
    use windows::Win32::UI::Shell::{
        IPreviewHandler, IThumbnailProvider, SHCreateMemStream, WTSAT_ARGB, WTS_ALPHATYPE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, FindWindowExW, IsWindow, SendMessageW, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows_core::{s, w, Interface, GUID, HRESULT, HSTRING};

    type GetClassObject = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

    fn initialize(object: &impl Interface, bytes: &[u8]) {
        // SAFETY: COM calls on live interfaces; the stream copies the bytes.
        unsafe {
            let stream = SHCreateMemStream(Some(bytes)).expect("a memory stream");
            object
                .cast::<IInitializeWithStream>()
                .unwrap()
                .Initialize(&stream, 0)
                .unwrap();
        }
    }

    /// Loads the embedded DLL the way Explorer does, without registering it.
    #[test]
    fn the_dll_draws_thumbnails_and_shows_previews() {
        let root = std::env::temp_dir().join("powerkit-preview-test");
        let dll = write_dll(&root).unwrap();
        // SAFETY: COM and window calls on this test's own objects; the DLL stays loaded
        // until the process ends, as it would in Explorer.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let module = LoadLibraryW(&HSTRING::from(dll.as_os_str())).unwrap();
            let get: GetClassObject = std::mem::transmute(GetProcAddress(module, s!("DllGetClassObject")).unwrap());
            let factory = |setting: &str| {
                let handler = HANDLERS.iter().find(|h| h.setting == setting).unwrap();
                let mut out = std::ptr::null_mut();
                get(&GUID::from_u128(handler.clsid), &IClassFactory::IID, &mut out)
                    .ok()
                    .unwrap();
                IClassFactory::from_raw(out)
            };
            let mut out = std::ptr::null_mut();
            assert!(get(&GUID::from_u128(1), &IClassFactory::IID, &mut out).is_err());

            let thumbnails: IThumbnailProvider = factory("svg_thumbnails").CreateInstance(None).unwrap();
            initialize(
                &thumbnails,
                br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#00f"/></svg>"##,
            );
            let (mut bitmap, mut alpha) = (HBITMAP::default(), WTS_ALPHATYPE::default());
            thumbnails.GetThumbnail(64, &mut bitmap, &mut alpha).unwrap();
            assert_eq!(alpha, WTSAT_ARGB);
            let mut info = BITMAP::default();
            GetObjectW(
                bitmap.into(),
                size_of::<BITMAP>() as i32,
                Some((&mut info as *mut BITMAP).cast()),
            );
            assert_eq!((info.bmWidth, info.bmHeight.abs(), info.bmBitsPixel), (64, 32, 32));
            let _ = DeleteObject(bitmap.into());

            let broken: IThumbnailProvider = factory("qoi_thumbnails").CreateInstance(None).unwrap();
            initialize(&broken, b"not a picture");
            assert!(broken.GetThumbnail(64, &mut bitmap, &mut alpha).is_err());

            let preview: IPreviewHandler = factory("markdown_preview").CreateInstance(None).unwrap();
            initialize(&preview, b"# Title\n\nSee [the site](https://example.com).\n");
            let host = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WS_OVERLAPPED,
                0,
                0,
                400,
                300,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let rect = RECT {
                left: 0,
                top: 0,
                right: 400,
                bottom: 300,
            };
            preview.SetWindow(host, &rect).unwrap();
            preview.DoPreview().unwrap();
            let pane = FindWindowExW(Some(host), None, w!("PowerKitPreviewPane"), None).unwrap();
            let edit = FindWindowExW(Some(pane), None, w!("RICHEDIT50W"), None).unwrap();
            // Without the hidden field instruction that carries a link's address.
            let mut text = [0u16; 256];
            let request = GETTEXTEX {
                cb: 512,
                flags: GT_NOHIDDENTEXT,
                codepage: 1200,
                ..Default::default()
            };
            let len = SendMessageW(
                edit,
                EM_GETTEXTEX,
                Some(WPARAM(&request as *const GETTEXTEX as usize)),
                Some(LPARAM(text.as_mut_ptr() as isize)),
            )
            .0 as usize;
            let text = String::from_utf16_lossy(&text[..len.min(256)]);
            assert!(text.starts_with("Title") && text.contains("See the site."), "{text:?}");
            preview.Unload().unwrap();
            assert!(!IsWindow(Some(pane)).as_bool());

            let preview: IPreviewHandler = factory("qoi_preview").CreateInstance(None).unwrap();
            initialize(&preview, b"qoif broken");
            preview.SetWindow(host, &rect).unwrap();
            preview.DoPreview().unwrap();
            assert_ne!(
                FindWindowExW(Some(host), None, w!("PowerKitPreviewPane"), None).unwrap(),
                HWND::default()
            );
            preview.Unload().unwrap();

            let _ = DestroyWindow(host);
            drop((thumbnails, broken, preview));
            CoUninitialize();
        }
    }
}
