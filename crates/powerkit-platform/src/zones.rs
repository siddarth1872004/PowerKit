//! The FancyZones overlay: a topmost, click-through window covering one monitor's work
//! area that shows the zones of a layout, numbered, with the selected ones highlighted.
//! Gaps between zones stay see-through (color key), the zones themselves are drawn
//! semi-transparent (layered alpha).

use crate::magnifier::Dib;
use crate::util::{colorref, pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Zones";
/// Background color made fully transparent. A configured color equal to it is nudged
/// by one step so zones never vanish.
const KEY: u32 = 0x00FF_00FE;
const BORDER: u32 = 2;
const FONT_SIZE: u32 = 28;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Content is blitted on every change; nothing to erase.
        WM_ERASEBKGND => LRESULT(1),
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// What the overlay shows; identical requests are not redrawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneView {
    /// Monitor work area the overlay covers.
    pub area: Rect,
    /// Zones in screen coordinates.
    pub zones: Vec<Rect>,
    pub highlighted: Vec<usize>,
    /// `#RRGGBB`.
    pub zone_color: String,
    pub highlight_color: String,
    /// 10-90.
    pub opacity_percent: u32,
    pub dpi: u32,
}

pub struct ZoneOverlay {
    hwnd: HWND,
    font: Option<(u32, HFONT)>,
    shown: Option<ZoneView>,
    _not_send: PhantomData<*const ()>,
}

fn brush_fill(dc: HDC, rc: &RECT, color: u32) {
    // SAFETY: the brush is created and deleted here; `dc` is a valid memory DC.
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color));
        FillRect(dc, rc, brush);
        let _ = DeleteObject(brush.into());
    }
}

impl ZoneOverlay {
    pub fn new() -> Result<ZoneOverlay> {
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
            .context("CreateWindowEx(zones)")?
        };
        Ok(ZoneOverlay {
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
                        FW_SEMIBOLD.0 as i32,
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

    pub fn show(&mut self, view: &ZoneView) -> Result<()> {
        if self.shown.as_ref() == Some(view) {
            return Ok(());
        }
        let area = view.area;
        let (w, h) = (area.width(), area.height());
        if w <= 0 || h <= 0 {
            return Err(Error::Other("the monitor work area is empty".into()));
        }
        let avoid_key = |c: u32| if c == KEY { c ^ 1 } else { c };
        let fill = avoid_key(colorref(&view.zone_color));
        let highlight = avoid_key(colorref(&view.highlight_color));
        let border = ((BORDER * view.dpi + 48) / 96).max(1) as i32;
        let font = self.font(view.dpi);
        let dib = Dib::new(w, h)?;
        let local = |r: &Rect| RECT {
            left: r.left - area.left,
            top: r.top - area.top,
            right: r.right - area.left,
            bottom: r.bottom - area.top,
        };
        brush_fill(
            dib.dc,
            &RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            },
            KEY,
        );
        // SAFETY: drawing into our own DIB and window; the original font is restored and
        // every DC obtained is released.
        unsafe {
            let old = SelectObject(dib.dc, font.into());
            SetBkMode(dib.dc, TRANSPARENT);
            for (i, z) in view.zones.iter().enumerate() {
                let on = view.highlighted.contains(&i);
                let rc = local(z);
                brush_fill(dib.dc, &rc, highlight);
                let inner = RECT {
                    left: rc.left + border,
                    top: rc.top + border,
                    right: rc.right - border,
                    bottom: rc.bottom - border,
                };
                if !on && inner.right > inner.left && inner.bottom > inner.top {
                    brush_fill(dib.dc, &inner, fill);
                }
                SetTextColor(dib.dc, COLORREF(0x00FF_FFFF));
                let mut text: Vec<u16> = (i + 1).to_string().encode_utf16().collect();
                let mut trc = rc;
                DrawTextW(
                    dib.dc,
                    &mut text,
                    &mut trc,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
            SelectObject(dib.dc, old);

            let alpha = (view.opacity_percent.clamp(10, 90) * 255 / 100) as u8;
            SetLayeredWindowAttributes(self.hwnd, COLORREF(KEY), alpha, LWA_ALPHA | LWA_COLORKEY)
                .context("SetLayeredWindowAttributes(zones)")?;
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                area.left,
                area.top,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .context("SetWindowPos(zones)")?;
            self.shown = Some(view.clone());
            let dc = GetDC(Some(self.hwnd));
            if dc.is_invalid() {
                return Err(Error::Other("GetDC(zones) failed".into()));
            }
            let blt = BitBlt(dc, 0, 0, w, h, Some(dib.dc), 0, 0, SRCCOPY);
            ReleaseDC(Some(self.hwnd), dc);
            blt.context("BitBlt(zones)")?;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.shown.take().is_some() {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    pub fn is_visible(&self) -> bool {
        self.shown.is_some()
    }
}

impl Drop for ZoneOverlay {
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
