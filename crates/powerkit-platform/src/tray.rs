//! Notification-area (system tray) icon with a context menu and balloon messages.

use crate::event_loop::{helper, WM_TRAY};
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use std::cell::RefCell;
use std::marker::PhantomData;
use windows::Win32::Foundation::POINT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const TRAY_ID: u32 = 1;

thread_local! {
    /// Data needed to re-add the icon when Explorer restarts.
    static CURRENT: RefCell<Option<NOTIFYICONDATAW>> = const { RefCell::new(None) };
}

fn copy_into(dst: &mut [u16], text: &str) {
    let src: Vec<u16> = text.encode_utf16().take(dst.len() - 1).collect();
    dst[..src.len()].copy_from_slice(&src);
    dst[src.len()] = 0;
}

/// Explorer restarted: the icon must be added again.
pub(crate) fn taskbar_recreated() {
    CURRENT.with(|c| {
        if let Some(data) = c.borrow().as_ref() {
            // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
            if !unsafe { Shell_NotifyIconW(NIM_ADD, data) }.as_bool() {
                tracing::warn!("could not restore the tray icon after Explorer restarted");
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
    _not_send: PhantomData<*const ()>,
}

impl Tray {
    /// Add the icon. The image is icon resource 1 of the executable, falling back to
    /// the stock application icon.
    pub fn new(tooltip: &str) -> Result<Tray> {
        let hwnd = helper()?;
        // SAFETY: resource lookups with constant arguments; a missing resource is
        // handled by the fallback.
        let icon = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            LoadIconW(Some(instance.into()), windows::core::PCWSTR(1 as *const u16))
                .or_else(|_| LoadIconW(None, IDI_APPLICATION))
                .context("LoadIcon")?
        };
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
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
        CURRENT.with(|c| *c.borrow_mut() = Some(data));
        Ok(Tray { _not_send: PhantomData })
    }

    /// Show a balloon / toast style notification from the tray icon.
    pub fn notify(&self, title: &str, message: &str) -> Result<()> {
        CURRENT.with(|c| {
            let Some(mut data) = *c.borrow() else {
                return Err(Error::Other("tray icon is not present".into()));
            };
            data.uFlags = NIF_INFO;
            data.dwInfoFlags = NIIF_INFO;
            copy_into(&mut data.szInfoTitle, title);
            copy_into(&mut data.szInfo, message);
            // SAFETY: `data` is fully initialized and outlives the call.
            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
                Ok(())
            } else {
                Err(Error::Other("the notification could not be shown".into()))
            }
        })
    }

    /// Show a context menu at the cursor and return the chosen item id. Blocks (running
    /// a nested message loop) until the menu closes.
    pub fn show_menu(&self, items: &[MenuItem]) -> Result<Option<u32>> {
        let hwnd = helper()?;
        // SAFETY: the menu is created and destroyed within this block; label buffers
        // outlive the AppendMenu calls that copy them.
        unsafe {
            let menu = CreatePopupMenu().context("CreatePopupMenu")?;
            for item in items {
                match item {
                    MenuItem::Separator => AppendMenuW(menu, MF_SEPARATOR, 0, None),
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
                        AppendMenuW(menu, flags, *id as usize, pcwstr(&text))
                    }
                }
                .context("AppendMenu")?;
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

impl Drop for Tray {
    fn drop(&mut self) {
        CURRENT.with(|c| {
            if let Some(data) = c.borrow_mut().take() {
                // SAFETY: `data` identifies the icon added in `new`.
                let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
            }
        });
    }
}
