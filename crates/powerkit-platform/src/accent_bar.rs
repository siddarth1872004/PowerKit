//! The Quick Accent bar: a row of characters near the top of a monitor with the
//! selected one highlighted. It never takes focus or input, so the application being
//! typed into keeps the caret.

use crate::magnifier::Dib;
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.AccentBar";
/// Sizes at 96 DPI.
const CELL: u32 = 52;
const PAD: u32 = 6;
const TOP_GAP: u32 = 24;
const FONT_SIZE: u32 = 26;
const BACKGROUND: u32 = 0x0020_2020;
const SELECTED: u32 = 0x00D4_7800; // #0078D4 as COLORREF
const TEXT: u32 = 0x00FF_FFFF;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        // Clicks fall through to whatever is underneath.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccentView {
    pub chars: Vec<String>,
    pub selected: usize,
    /// Work area of the monitor to show the bar on.
    pub area: Rect,
    pub dpi: u32,
}

pub struct AccentBar {
    hwnd: HWND,
    font: Option<(u32, HFONT)>,
    shown: Option<AccentView>,
    _not_send: PhantomData<*const ()>,
}

fn fill(dc: HDC, rc: &RECT, color: u32) {
    // SAFETY: the brush is created and deleted here; `dc` is a valid memory DC.
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color));
        FillRect(dc, rc, brush);
        let _ = DeleteObject(brush.into());
    }
}

/// First visible cell so that `selected` is shown when only `fit` of `len` cells fit.
pub fn first_visible(len: usize, selected: usize, fit: usize) -> usize {
    if fit == 0 || len <= fit {
        return 0;
    }
    selected.saturating_sub(fit / 2).min(len - fit)
}

impl AccentBar {
    pub fn new() -> Result<AccentBar> {
        let class = wide(CLASS_NAME);
        // SAFETY: `class` outlives the calls; `wndproc` matches the WNDPROC contract.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                ..Default::default()
            };
            RegisterClassW(&wc); // "already registered" is expected after the first one
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
                pcwstr(&class),
                pcwstr(&class),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(accent bar)")?
        };
        Ok(AccentBar {
            hwnd,
            font: None,
            shown: None,
            _not_send: PhantomData,
        })
    }

    fn font(&mut self, dpi: u32) -> HFONT {
        match self.font {
            Some((d, f)) if d == dpi => f,
            _ => {
                let face = wide("Segoe UI");
                let size = ((FONT_SIZE * dpi + 48) / 96) as i32;
                // SAFETY: `face` outlives the call; the previous font is not selected
                // anywhere (each draw selects and restores it).
                let f = unsafe {
                    if let Some((_, old)) = self.font.take() {
                        let _ = DeleteObject(old.into());
                    }
                    CreateFontW(
                        -size,
                        0,
                        0,
                        0,
                        FW_NORMAL.0 as i32,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_DEFAULT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY,
                        0,
                        pcwstr(&face),
                    )
                };
                self.font = Some((dpi, f));
                f
            }
        }
    }

    pub fn show(&mut self, view: &AccentView) -> Result<()> {
        if self.shown.as_ref() == Some(view) {
            return Ok(());
        }
        if view.chars.is_empty() {
            self.hide();
            return Ok(());
        }
        let scale = |v: u32| ((v * view.dpi + 48) / 96) as i32;
        let (cell, pad) = (scale(CELL), scale(PAD));
        let fit = ((view.area.width() - 2 * pad).max(cell) / cell) as usize;
        let first = first_visible(view.chars.len(), view.selected, fit);
        let shown = &view.chars[first..(first + fit).min(view.chars.len())];
        let w = shown.len() as i32 * cell + 2 * pad;
        let h = cell + 2 * pad;
        let x = view.area.left + (view.area.width() - w) / 2;
        let y = view.area.top + scale(TOP_GAP);
        let font = self.font(view.dpi);
        let dib = Dib::new(w, h)?;
        fill(
            dib.dc,
            &RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            },
            BACKGROUND,
        );
        // SAFETY: drawing into our own DIB and window; the original font is restored and
        // every DC obtained is released.
        unsafe {
            let old = SelectObject(dib.dc, font.into());
            SetBkMode(dib.dc, TRANSPARENT);
            SetTextColor(dib.dc, COLORREF(TEXT));
            for (i, c) in shown.iter().enumerate() {
                let left = pad + i as i32 * cell;
                let mut rc = RECT {
                    left,
                    top: pad,
                    right: left + cell,
                    bottom: pad + cell,
                };
                if first + i == view.selected {
                    fill(dib.dc, &rc, SELECTED);
                }
                let mut text: Vec<u16> = c.encode_utf16().collect();
                DrawTextW(
                    dib.dc,
                    &mut text,
                    &mut rc,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
            SelectObject(dib.dc, old);

            SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 245, LWA_ALPHA)
                .context("SetLayeredWindowAttributes(accent bar)")?;
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .context("SetWindowPos(accent bar)")?;
            self.shown = Some(view.clone());
            let dc = GetDC(Some(self.hwnd));
            if dc.is_invalid() {
                return Err(Error::Other("GetDC(accent bar) failed".into()));
            }
            let blt = BitBlt(dc, 0, 0, w, h, Some(dib.dc), 0, 0, SRCCOPY);
            ReleaseDC(Some(self.hwnd), dc);
            blt.context("BitBlt(accent bar)")?;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.shown.take().is_some() {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }
}

impl Drop for AccentBar {
    fn drop(&mut self) {
        // SAFETY: window and font were created by this object on this thread.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            if let Some((_, f)) = self.font.take() {
                let _ = DeleteObject(f.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::first_visible;

    #[test]
    fn scrolls_to_keep_selection_visible() {
        assert_eq!(first_visible(5, 4, 10), 0);
        assert_eq!(first_visible(20, 0, 5), 0);
        assert_eq!(first_visible(20, 10, 5), 8);
        assert_eq!(first_visible(20, 19, 5), 15);
    }
}
