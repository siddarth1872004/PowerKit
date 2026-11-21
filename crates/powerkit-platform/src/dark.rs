//! The dark look for windows built from standard controls: a dark title bar,
//! background and text, and the dark themes Windows has for its controls.

use crate::util::{pcwstr, wide};
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// Windows 11's dark colors for a window, its boxes and text.
const WINDOW: u32 = 0x0020_2020;
const BOX: u32 = 0x002D_2D2D;
const TEXT: u32 = 0x00F0_F0F0;

/// Owned by a window's state; [`DarkLook::on_message`] goes first in its procedure.
#[derive(Default)]
pub(crate) struct DarkLook {
    /// Brushes for the window and its boxes while dark.
    brushes: Option<(HBRUSH, HBRUSH)>,
}

impl DarkLook {
    /// Follow PowerKit's theme setting.
    pub fn update(&mut self, hwnd: HWND) {
        self.apply(hwnd, crate::theme::app_dark());
    }

    fn apply(&mut self, hwnd: HWND, dark: bool) {
        if self.brushes.is_some() == dark {
            return;
        }
        self.free();
        // SAFETY: plain GDI and DWM calls on our own window.
        unsafe {
            if dark {
                self.brushes = Some((CreateSolidBrush(COLORREF(WINDOW)), CreateSolidBrush(COLORREF(BOX))));
            }
            let flag = BOOL::from(dark);
            if let Err(e) = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &flag as *const BOOL as *const _,
                std::mem::size_of::<BOOL>() as u32,
            ) {
                tracing::debug!("dark title bar: {e}");
            }
            let mut dark_flag = dark;
            let _ = EnumChildWindows(
                Some(hwnd),
                Some(theme_child),
                LPARAM(&mut dark_flag as *mut bool as isize),
            );
            let _ = RedrawWindow(
                Some(hwnd),
                None,
                None,
                RDW_ERASE | RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN,
            );
        }
    }

    /// Paint the background and colour the controls while dark, and pick up a theme
    /// change; `None` leaves the message to the window.
    pub fn on_message(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM) -> Option<LRESULT> {
        match msg {
            // The theme may have changed in the settings or in Windows.
            WM_ACTIVATE | WM_SETTINGCHANGE => {
                self.update(hwnd);
                None
            }
            WM_ERASEBKGND => {
                let (window, _) = self.brushes?;
                let mut r = RECT::default();
                // SAFETY: own window; Windows passes the DC to paint in `wparam`.
                unsafe {
                    let _ = GetClientRect(hwnd, &mut r);
                    FillRect(HDC(wparam.0 as *mut _), &r, window);
                }
                Some(LRESULT(1))
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX => {
                let (window, field) = self.brushes?;
                let (brush, color) = if msg == WM_CTLCOLORSTATIC || msg == WM_CTLCOLORBTN {
                    (window, WINDOW)
                } else {
                    (field, BOX)
                };
                let hdc = HDC(wparam.0 as *mut _);
                // SAFETY: Windows passes the control's DC in `wparam`.
                unsafe {
                    SetTextColor(hdc, COLORREF(TEXT));
                    SetBkColor(hdc, COLORREF(color));
                }
                Some(LRESULT(brush.0 as isize))
            }
            _ => None,
        }
    }

    fn free(&mut self) {
        if let Some((a, b)) = self.brushes.take() {
            // SAFETY: brushes this look created and nothing selects any more.
            unsafe {
                let _ = DeleteObject(a.into());
                let _ = DeleteObject(b.into());
            }
        }
    }
}

impl Drop for DarkLook {
    fn drop(&mut self) {
        self.free();
    }
}

/// Give one control the dark or the normal theme; `lparam` points at the wanted
/// darkness.
unsafe extern "system" fn theme_child(child: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `apply` passes a bool that outlives the enumeration.
    let dark = unsafe { *(lparam.0 as *const bool) };
    let mut class = [0u16; 32];
    // SAFETY: the buffer outlives the call.
    let n = unsafe { GetClassNameW(child, &mut class) };
    let class = String::from_utf16_lossy(&class[..n.max(0) as usize]).to_ascii_lowercase();
    // SAFETY: plain query of the control's style.
    let style = unsafe { GetWindowLongW(child, GWL_STYLE) } as u32;
    let name = match class.as_str() {
        "syslistview32" => {
            let (back, text) = if dark {
                (BOX, TEXT)
            } else {
                // SAFETY: plain queries.
                unsafe { (GetSysColor(COLOR_WINDOW), GetSysColor(COLOR_WINDOWTEXT)) }
            };
            for (msg, v) in [
                (LVM_SETBKCOLOR, back),
                (LVM_SETTEXTBKCOLOR, back),
                (LVM_SETTEXTCOLOR, text),
            ] {
                // SAFETY: list view messages to a list view.
                unsafe { SendMessageW(child, msg, Some(WPARAM(0)), Some(LPARAM(v as isize))) };
            }
            "DarkMode_Explorer"
        }
        // The list's header and the boxes' parts are enumerated too.
        "sysheader32" => "DarkMode_ItemsView",
        "edit" if style & ES_MULTILINE as u32 != 0 => "DarkMode_Explorer",
        "edit" | "combobox" => "DarkMode_CFD",
        "button" => {
            let kind = style & BS_TYPEMASK as u32;
            let check = [
                BS_CHECKBOX,
                BS_AUTOCHECKBOX,
                BS_RADIOBUTTON,
                BS_AUTORADIOBUTTON,
                BS_3STATE,
                BS_AUTO3STATE,
            ]
            .iter()
            .any(|&k| kind == k as u32);
            if check {
                // Themed check boxes draw their text black whatever the window says;
                // unthemed ones use the text color the window gives.
                let empty = wide("");
                let (app, ids) = if dark {
                    (pcwstr(&empty), pcwstr(&empty))
                } else {
                    (PCWSTR::null(), PCWSTR::null())
                };
                // SAFETY: the strings outlive the call.
                let _ = unsafe { SetWindowTheme(child, app, ids) };
                return BOOL(1);
            }
            "DarkMode_Explorer"
        }
        "scrollbar" | "systreeview32" => "DarkMode_Explorer",
        _ => return BOOL(1),
    };
    let name = wide(name);
    let name = if dark { pcwstr(&name) } else { PCWSTR::null() };
    // SAFETY: the string outlives the call.
    let _ = unsafe { SetWindowTheme(child, name, PCWSTR::null()) };
    BOOL(1)
}
