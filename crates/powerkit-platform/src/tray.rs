//! Notification-area (system tray) icon with a context menu and balloon messages.

use crate::event_loop::{helper, WM_TRAY};
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use std::cell::RefCell;
use std::marker::PhantomData;
use windows::Win32::Foundation::POINT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// The id of PowerKit's own icon, made by [`Tray::new`].
pub const MAIN: u32 = 1;

thread_local! {
    /// The icons shown, kept to re-add them when Explorer restarts.
    static CURRENT: RefCell<Vec<NOTIFYICONDATAW>> = const { RefCell::new(Vec::new()) };
}

fn copy_into(dst: &mut [u16], text: &str) {
    let src: Vec<u16> = text.encode_utf16().take(dst.len() - 1).collect();
    dst[..src.len()].copy_from_slice(&src);
    dst[src.len()] = 0;
}

/// Explorer restarted: the icon must be added again.
pub(crate) fn taskbar_recreated() {
    CURRENT.with(|c| {
        for data in c.borrow().iter() {
            // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
            if !unsafe { Shell_NotifyIconW(NIM_ADD, data) }.as_bool() {
                tracing::warn!("could not restore tray icon {} after Explorer restarted", data.uID);
            }
        }
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Item {
        id: u32,
        label: String,
        checked: bool,
        enabled: bool,
    },
    Separator,
    Submenu {
        label: String,
        items: Vec<MenuItem>,
    },
}

impl MenuItem {
    pub fn new(id: u32, label: impl Into<String>) -> MenuItem {
        MenuItem::Item {
            id,
            label: label.into(),
            checked: false,
            enabled: true,
        }
    }

    pub fn checked(id: u32, label: impl Into<String>, checked: bool) -> MenuItem {
        MenuItem::Item {
            id,
            label: label.into(),
            checked,
            enabled: true,
        }
    }
}

pub struct Tray {
    id: u32,
    _not_send: PhantomData<*const ()>,
}

impl Tray {
    /// Add PowerKit's icon, with id [`MAIN`]. The image is icon resource 1 of the
    /// executable, falling back to the stock application icon.
    pub fn new(tooltip: &str) -> Result<Tray> {
        // SAFETY: resource lookups with constant arguments; a missing resource is
        // handled by the fallback.
        let icon = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            LoadIconW(
                Some(instance.into()),
                windows::core::PCWSTR(std::ptr::without_provenance(1)),
            )
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .context("LoadIcon")?
        };
        Self::add(MAIN, tooltip, icon)
    }

    /// Add another icon, showing the .ico file `ico`. Clicks on it are reported with
    /// `id`, which must differ from [`MAIN`] and from the other icons shown.
    pub fn with_icon(id: u32, tooltip: &str, ico: &'static [u8]) -> Result<Tray> {
        // SAFETY: plain queries.
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
        Self::add(id, tooltip, crate::icon::load(ico, size)?)
    }

    fn add(id: u32, tooltip: &str, icon: HICON) -> Result<Tray> {
        let hwnd = helper()?;
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: id,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        copy_into(&mut data.szTip, tooltip);
        // SAFETY: `data` is fully initialized and outlives the call.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            // The shell may not be running yet (early logon); TaskbarCreated re-adds it.
            tracing::warn!("tray icon could not be added yet; will retry when the taskbar appears");
        }
        CURRENT.with(|c| c.borrow_mut().push(data));
        Ok(Tray {
            id,
            _not_send: PhantomData,
        })
    }

    /// Change the text shown when the pointer rests on the icon.
    pub fn set_tooltip(&self, tooltip: &str) -> Result<()> {
        self.modify(|data| {
            data.uFlags = NIF_TIP;
            copy_into(&mut data.szTip, tooltip);
        })
        .map_err(|_| Error::Other("the tray icon text could not be changed".into()))
    }

    /// Apply `change` to this icon's data (keeping it for a re-add) and send it.
    fn modify(&self, change: impl FnOnce(&mut NOTIFYICONDATAW)) -> Result<()> {
        CURRENT.with(|c| {
            let mut c = c.borrow_mut();
            let Some(kept) = c.iter_mut().find(|d| d.uID == self.id) else {
                return Err(Error::Other("tray icon is not present".into()));
            };
            let mut data = *kept;
            change(&mut data);
            if data.uFlags & NIF_TIP == NIF_TIP {
                kept.szTip = data.szTip;
            }
            // SAFETY: `data` is fully initialized and outlives the call.
            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
                Ok(())
            } else {
                Err(Error::Other("the tray icon could not be changed".into()))
            }
        })
    }

    /// Show a balloon / toast style notification from the tray icon.
    pub fn notify(&self, title: &str, message: &str) -> Result<()> {
        self.modify(|data| {
            data.uFlags = NIF_INFO;
            data.dwInfoFlags = NIIF_INFO;
            copy_into(&mut data.szInfoTitle, title);
            copy_into(&mut data.szInfo, message);
        })
        .map_err(|_| Error::Other("the notification could not be shown".into()))
    }

    /// Show a context menu at the cursor and return the chosen item id. Blocks (running
    /// a nested message loop) until the menu closes.
    pub fn show_menu(&self, items: &[MenuItem]) -> Result<Option<u32>> {
        let hwnd = helper()?;
        // SAFETY: the menu is created and destroyed within this block; label buffers
        // outlive the AppendMenu calls that copy them.
        unsafe {
            let menu = CreatePopupMenu().context("CreatePopupMenu")?;
            if let Err(e) = fill(menu, items) {
                let _ = DestroyMenu(menu);
                return Err(e);
            }
            let mut pt = POINT::default();
            GetCursorPos(&mut pt).context("GetCursorPos")?;
            // Required so the menu closes when the user clicks elsewhere.
            let _ = SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                None,
                hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            Ok((chosen.0 > 0).then_some(chosen.0 as u32))
        }
    }
}

/// Append `items` to `menu`; submenus are owned by `menu` and destroyed with it.
///
/// # Safety
/// `menu` must be a menu handle created by this thread.
unsafe fn fill(menu: HMENU, items: &[MenuItem]) -> Result<()> {
    for item in items {
        match item {
            MenuItem::Separator => unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, None) },
            MenuItem::Item {
                id,
                label,
                checked,
                enabled,
            } => {
                let mut flags = MF_STRING;
                if *checked {
                    flags |= MF_CHECKED;
                }
                if !*enabled {
                    flags |= MF_GRAYED;
                }
                let text = wide(label);
                unsafe { AppendMenuW(menu, flags, *id as usize, pcwstr(&text)) }
            }
            MenuItem::Submenu { label, items } => {
                let sub = unsafe { CreatePopupMenu() }.context("CreatePopupMenu")?;
                if let Err(e) = unsafe { fill(sub, items) } {
                    let _ = unsafe { DestroyMenu(sub) };
                    return Err(e);
                }
                let text = wide(label);
                let added = unsafe { AppendMenuW(menu, MF_POPUP, sub.0 as usize, pcwstr(&text)) };
                if added.is_err() {
                    let _ = unsafe { DestroyMenu(sub) };
                }
                added
            }
        }
        .context("AppendMenu")?;
    }
    Ok(())
}

impl Drop for Tray {
    fn drop(&mut self) {
        CURRENT.with(|c| {
            let mut c = c.borrow_mut();
            if let Some(at) = c.iter().position(|d| d.uID == self.id) {
                let data = c.remove(at);
                // SAFETY: `data` identifies the icon added in `add`.
                let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
            }
        });
    }
}
