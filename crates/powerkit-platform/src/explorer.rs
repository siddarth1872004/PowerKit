//! What is shown and selected in a File Explorer window or on the desktop, read through
//! the shell's automation interfaces.

use crate::window::{self, Hwnd};
use crate::{Context, Result};
use std::path::PathBuf;
use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IDispatch, IServiceProvider, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IFolderView, IShellBrowser, IShellItemArray, IShellView, IShellWindows, IWebBrowserApp, SID_STopLevelBrowser,
    ShellWindows, CSIDL_DESKTOP, SIGDN_FILESYSPATH, SVGIO_ALLVIEW, SVGIO_SELECTION, SWC_DESKTOP, SWFO_NEEDDISPATCH,
};
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

/// The files and folders of a view in the order shown, and which of them are selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub items: Vec<PathBuf>,
    pub selected: Vec<usize>,
}

/// Whether `class` is File Explorer's or the desktop's window class.
pub fn is_explorer_class(class: &str) -> bool {
    matches!(class, "CabinetWClass" | "ExploreWClass" | "Progman" | "WorkerW")
}

fn view_of(dispatch: &IDispatch) -> Result<IShellView> {
    // SAFETY: COM calls on live interfaces.
    unsafe {
        let sp: IServiceProvider = dispatch.cast().context("reaching the Explorer window")?;
        let browser: IShellBrowser = sp
            .QueryService(&SID_STopLevelBrowser)
            .context("reaching the Explorer window")?;
        browser.QueryActiveShellView().context("reading the Explorer view")
    }
}

/// The view of the Explorer window `hwnd` (its visible tab) or of the desktop.
fn find_view(hwnd: Hwnd, class: &str) -> Result<Option<IShellView>> {
    // SAFETY: COM calls on interfaces created here; VARIANTs outlive the calls.
    unsafe {
        let windows: IShellWindows =
            CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).context("reaching the shell windows")?;
        if matches!(class, "Progman" | "WorkerW") {
            let mut h = 0;
            let desktop = windows
                .FindWindowSW(
                    &VARIANT::from(CSIDL_DESKTOP as i32),
                    &VARIANT::default(),
                    SWC_DESKTOP,
                    &mut h,
                    SWFO_NEEDDISPATCH,
                )
                .context("reaching the desktop")?;
            return view_of(&desktop).map(Some);
        }
        for i in 0..windows.Count().context("listing the Explorer windows")? {
            let Ok(dispatch) = windows.Item(&VARIANT::from(i)) else {
                continue;
            };
            let Ok(app) = dispatch.cast::<IWebBrowserApp>() else {
                continue;
            };
            if app.HWND().map(|h| h.0) != Ok(hwnd.0) {
                continue;
            }
            // Tabs of one window share its handle; only the shown tab's view is visible.
            let Ok(view) = view_of(&dispatch) else { continue };
            if view.GetWindow().is_ok_and(|w| IsWindowVisible(w).as_bool()) {
                return Ok(Some(view));
            }
        }
        Ok(None)
    }
}

fn paths(array: &IShellItemArray) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // SAFETY: COM calls on a live array; each returned string is freed once.
    unsafe {
        for i in 0..array.GetCount().unwrap_or(0) {
            let Ok(item) = array.GetItemAt(i) else { continue };
            // Items outside the file system (This PC, Recycle Bin) have no path.
            let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH) else {
                continue;
            };
            if let Ok(s) = name.to_string() {
                out.push(PathBuf::from(s));
            }
            CoTaskMemFree(Some(name.0 as *const _));
        }
    }
    out
}

/// What the Explorer window or desktop `hwnd` shows and has selected; `None` when
/// `hwnd` is not one.
pub fn selection(hwnd: Hwnd) -> Result<Option<Selection>> {
    let class = window::info(hwnd)?.class;
    if !is_explorer_class(&class) {
        return Ok(None);
    }
    // SAFETY: initialising COM on this thread; already initialised is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let Some(view) = find_view(hwnd, &class)? else {
        return Ok(None);
    };
    let folder: IFolderView = view.cast().context("reading the Explorer view")?;
    // SAFETY: COM calls on a live view. Nothing selected is reported as an error.
    let (all, chosen) = unsafe {
        (
            folder.Items::<IShellItemArray>(SVGIO_ALLVIEW).map(|a| paths(&a)),
            folder
                .Items::<IShellItemArray>(SVGIO_SELECTION)
                .map(|a| paths(&a))
                .unwrap_or_default(),
        )
    };
    let items = all.unwrap_or_else(|_| chosen.clone());
    let selected = chosen
        .iter()
        .filter_map(|p| items.iter().position(|i| i == p))
        .collect();
    Ok(Some(Selection { items, selected }))
}
