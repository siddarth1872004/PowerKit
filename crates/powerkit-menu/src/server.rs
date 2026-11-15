//! The COM server Explorer loads to show PowerKit's entries. It never panics: a panic
//! here would take Explorer down with it, so every failure becomes an HRESULT.

use crate::{command_line, reg, Entry, ENTRIES, EXE_VALUE};
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{
    CloseHandle, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOTIMPL, E_POINTER, HINSTANCE, HMODULE,
    S_FALSE,
};
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx, IClassFactory, IClassFactory_Impl, IServiceProvider};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Ole::{IObjectWithSite, IObjectWithSite_Impl};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
use windows::Win32::UI::Shell::{
    IEnumExplorerCommand, IExplorerCommand, IExplorerCommand_Impl, IFolderView, IShellBrowser, IShellItem,
    IShellItemArray, SHStrDupW, SID_STopLevelBrowser, ECF_DEFAULT, ECS_ENABLED, ECS_HIDDEN, SIGDN_FILESYSPATH,
};
use windows_core::{implement, IUnknown, Interface, Ref, BOOL, GUID, HRESULT, HSTRING, PCWSTR, PWSTR};

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

#[no_mangle]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0, Ordering::Relaxed);
    }
    true.into()
}

/// # Safety
/// Called by COM with valid pointers, as for any in-process server.
#[no_mangle]
unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null above; COM passes valid pointers.
    unsafe { *out = std::ptr::null_mut() };
    let wanted = unsafe { (*clsid).to_u128() };
    let Some(entry) = ENTRIES.iter().find(|e| e.clsid == wanted) else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    let factory: IClassFactory = Factory { entry }.into();
    // SAFETY: iid and out are valid, as checked above.
    unsafe { factory.query(iid, out) }
}

/// The DLL stays loaded for the life of the process: Explorer keeps it anyway, and
/// unloading while an object is still alive would crash the host.
#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[implement(IClassFactory)]
struct Factory {
    entry: &'static Entry,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        if out.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked non-null above.
        unsafe { *out = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let command: IExplorerCommand = Command {
            entry: self.entry,
            site: Mutex::new(None),
        }
        .into();
        // SAFETY: iid and out are valid, as checked above.
        unsafe { command.query(iid, out) }.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> windows_core::Result<()> {
        Ok(())
    }
}

#[implement(IExplorerCommand, IObjectWithSite)]
struct Command {
    entry: &'static Entry,
    /// The Explorer window asking, which knows the open folder when the background was
    /// right-clicked and no item is selected.
    site: Mutex<Option<IUnknown>>,
}

fn co_string(s: &str) -> windows_core::Result<PWSTR> {
    // SAFETY: the HSTRING outlives the call, which copies it into CoTaskMem memory.
    unsafe { SHStrDupW(&HSTRING::from(s)) }
}

/// The folder this DLL was loaded from, where powerkit.exe puts the entry icons.
fn own_folder() -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let module = HMODULE(MODULE.load(Ordering::Relaxed));
    // SAFETY: buf is a valid buffer; module is this DLL's handle from DllMain.
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if len == 0 || len == buf.len() {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len]);
    path.rfind('\\').map(|i| path[..i].to_string())
}

fn path_of(item: &IShellItem) -> windows_core::Result<String> {
    // SAFETY: item is a live shell item; the returned string is freed below.
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s.map_err(|_| E_FAIL.into())
    }
}

fn selected(items: &IShellItemArray) -> windows_core::Result<Vec<String>> {
    // SAFETY: items is a live array from Explorer.
    let count = unsafe { items.GetCount()? };
    (0..count)
        // SAFETY: i is below the count just read.
        .map(|i| path_of(&unsafe { items.GetItemAt(i)? }))
        .collect()
}

/// The folder shown in the Explorer window that is the site.
fn site_folder(site: &IUnknown) -> windows_core::Result<String> {
    // SAFETY: each call is on a live interface obtained just before.
    unsafe {
        let provider: IServiceProvider = site.cast()?;
        let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
        let view: IFolderView = browser.QueryActiveShellView()?.cast()?;
        path_of(&view.GetFolder::<IShellItem>()?)
    }
}

fn start(command: &str) -> windows_core::Result<()> {
    let mut line: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: line is a writable NUL-terminated buffer, as CreateProcessW requires.
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            Default::default(),
            None,
            PCWSTR::null(),
            &startup,
            &mut info,
        )?;
        // Closing a handle we own cannot meaningfully fail.
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(())
}

impl IExplorerCommand_Impl for Command_Impl {
    fn GetTitle(&self, _items: Ref<IShellItemArray>) -> windows_core::Result<PWSTR> {
        co_string(self.entry.title)
    }

    fn GetIcon(&self, _items: Ref<IShellItemArray>) -> windows_core::Result<PWSTR> {
        let folder = own_folder().ok_or(E_FAIL)?;
        co_string(&format!(r"{folder}\{}.ico", self.entry.module))
    }

    fn GetToolTip(&self, _items: Ref<IShellItemArray>) -> windows_core::Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> windows_core::Result<GUID> {
        Ok(GUID::from_u128(self.entry.clsid))
    }

    fn GetState(&self, _items: Ref<IShellItemArray>, _slow_ok: BOOL) -> windows_core::Result<u32> {
        let shown = reg::dword(self.entry.module) == Some(1) && reg::string(EXE_VALUE).is_some();
        Ok(if shown {
            ECS_ENABLED.0 as u32
        } else {
            ECS_HIDDEN.0 as u32
        })
    }

    fn Invoke(&self, items: Ref<IShellItemArray>, _bind: Ref<IBindCtx>) -> windows_core::Result<()> {
        let exe = reg::string(EXE_VALUE).ok_or(E_FAIL)?;
        let mut paths = match items.as_ref() {
            Some(items) => selected(items)?,
            None => Vec::new(),
        };
        if paths.is_empty() {
            let site = self.site.lock().map_err(|_| E_FAIL)?.clone().ok_or(E_FAIL)?;
            paths.push(site_folder(&site)?);
        }
        start(&command_line(&exe, self.entry, &paths))
    }

    fn GetFlags(&self) -> windows_core::Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }

    fn EnumSubCommands(&self) -> windows_core::Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}

impl IObjectWithSite_Impl for Command_Impl {
    fn SetSite(&self, site: Ref<IUnknown>) -> windows_core::Result<()> {
        *self.site.lock().map_err(|_| E_FAIL)? = site.cloned();
        Ok(())
    }

    fn GetSite(&self, iid: *const GUID, out: *mut *mut c_void) -> windows_core::Result<()> {
        if out.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked non-null above.
        unsafe { *out = std::ptr::null_mut() };
        let site = self.site.lock().map_err(|_| E_FAIL)?.clone().ok_or(E_FAIL)?;
        // SAFETY: iid and out are valid, as checked above.
        unsafe { site.query(iid, out) }.ok()
    }
}
