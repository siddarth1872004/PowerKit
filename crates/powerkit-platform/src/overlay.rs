//! A topmost, click-through window for drawing over the desktop: either one color cut
//! to a shape made of rectangles and ellipses (spotlights, crosshairs, click circles),
//! or a picture. Shapes cost a window region, not a bitmap, so they can follow the
//! pointer every frame even across several monitors.

use crate::magnifier::Dib;
use crate::util::{colorref, pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Overlay";
/// USERDATA value meaning "showing a picture, do not fill".
const NO_FILL: isize = -1;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: standard paint sequence on the window this procedure was called for; the
    // brush is created and deleted within the block.
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let color = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if color != NO_FILL {
                    let brush = CreateSolidBrush(COLORREF(color as u32));
                    FillRect(hdc, &ps.rcPaint, brush);
                    let _ = DeleteObject(brush.into());
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Part of a shape, in screen coordinates. Pieces apply in order: `Rect`/`Ellipse`
/// add area, `Cut*` remove it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    Rect(Rect),
    Ellipse(Rect),
    CutRect(Rect),
    CutEllipse(Rect),
}

#[derive(Debug, Clone, PartialEq)]
enum Shown {
    Shape {
        bounds: Rect,
        color: u32,
        alpha: u8,
        pieces: Vec<Piece>,
    },
    Picture,
}

pub struct Overlay {
    hwnd: HWND,
    shown: Option<Shown>,
    /// The last picture's bitmap and size, reused while the size stays.
    dib: Option<((i32, i32), Dib)>,
    _not_send: PhantomData<*const ()>,
}

impl Overlay {
    pub fn new() -> Result<Overlay> {
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
            .context("CreateWindowEx(overlay)")?
        };
        Ok(Overlay {
            hwnd,
            shown: None,
            dib: None,
            _not_send: PhantomData,
        })
    }

    /// Cover `bounds` with `color` (`#RRGGBB`) at `alpha`, limited to `pieces`.
    pub fn show_shape(&mut self, bounds: Rect, color: &str, alpha: u8, pieces: &[Piece]) -> Result<()> {
        if bounds.is_empty() {
            return Err(Error::Other("empty overlay area".into()));
        }
        let color = colorref(color);
        let (old_bounds, old_color, old_alpha, old_pieces) = match &self.shown {
            Some(Shown::Shape {
                bounds,
                color,
                alpha,
                pieces,
            }) => (Some(*bounds), Some(*color), Some(*alpha), Some(pieces.as_slice())),
            _ => (None, None, None, None),
        };
        let pieces_changed = old_pieces != Some(pieces) || old_bounds != Some(bounds);
        let local = |r: &Rect| r.offset(-bounds.left, -bounds.top);
        // SAFETY: own window; regions are created here and either handed to the system
        // by SetWindowRgn or deleted.
        unsafe {
            if old_color != Some(color) {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, color as isize);
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            if old_alpha != Some(alpha) {
                SetLayeredWindowAttributes(self.hwnd, COLORREF(0), alpha, LWA_ALPHA)
                    .context("SetLayeredWindowAttributes(overlay)")?;
            }
            if pieces_changed {
                let region = CreateRectRgn(0, 0, 0, 0);
                for p in pieces {
                    let (r, ellipse, mode) = match p {
                        Piece::Rect(r) => (local(r), false, RGN_OR),
                        Piece::Ellipse(r) => (local(r), true, RGN_OR),
                        Piece::CutRect(r) => (local(r), false, RGN_DIFF),
                        Piece::CutEllipse(r) => (local(r), true, RGN_DIFF),
                    };
                    let part = if ellipse {
                        CreateEllipticRgn(r.left, r.top, r.right, r.bottom)
                    } else {
                        CreateRectRgn(r.left, r.top, r.right, r.bottom)
                    };
                    CombineRgn(Some(region), Some(region), Some(part), mode);
                    let _ = DeleteObject(part.into());
                }
                if SetWindowRgn(self.hwnd, Some(region), true) == 0 {
                    let _ = DeleteObject(region.into());
                }
            }
            if old_bounds != Some(bounds) || !self.is_visible() {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    bounds.left,
                    bounds.top,
                    bounds.width(),
                    bounds.height(),
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
                .context("SetWindowPos(overlay)")?;
            }
        }
        self.shown = Some(Shown::Shape {
            bounds,
            color,
            alpha,
            pieces: pieces.to_vec(),
        });
        Ok(())
    }

    /// Show `pixels` (`0x00RRGGBB`, row by row, `at.width()` per row) at `at`.
    pub fn show_picture(&mut self, at: Rect, pixels: &[u32]) -> Result<()> {
        if pixels.len() != (at.width().max(0) * at.height().max(0)) as usize {
            return Err(Error::Other("picture size does not match its area".into()));
        }
        self.show_drawn(at, |out| out.copy_from_slice(pixels))
    }

    /// Show at `at` what `draw` paints into its pixels (`0x00RRGGBB`, row by row,
    /// `at.width()` per row; they hold the previous picture of that size). The bitmap is
    /// kept between calls: at a frame rate, making a screen-sized one each time showed.
    pub fn show_drawn(&mut self, at: Rect, draw: impl FnOnce(&mut [u32])) -> Result<()> {
        let (w, h) = (at.width(), at.height());
        if w <= 0 || h <= 0 {
            return Err(Error::Other("the picture is empty".into()));
        }
        let dib = match &mut self.dib {
            Some((size, dib)) if *size == (w, h) => dib,
            slot => &mut slot.insert(((w, h), Dib::new(w, h)?)).1,
        };
        draw(dib.pixels());
        let src = dib.dc;
        self.place(at)?;
        // SAFETY: own window; the DC obtained is released on every path; `src` is the
        // kept bitmap's DC, alive while `self` is.
        unsafe {
            let dc = GetDC(Some(self.hwnd));
            if dc.is_invalid() {
                return Err(Error::Other("GetDC(overlay) failed".into()));
            }
            let blt = BitBlt(dc, 0, 0, w, h, Some(src), 0, 0, SRCCOPY);
            ReleaseDC(Some(self.hwnd), dc);
            blt.context("BitBlt(overlay)")?;
        }
        Ok(())
    }

    /// Show the screen's `area` stretched over `at`, copied straight from the screen to
    /// this window. Call [`Overlay::exclude_from_capture`] first, or the window shows
    /// itself.
    pub fn show_screen(&mut self, at: Rect, area: Rect) -> Result<()> {
        let (w, h) = (at.width(), at.height());
        if w <= 0 || h <= 0 || area.width() <= 0 || area.height() <= 0 {
            return Err(Error::Other("empty capture area".into()));
        }
        self.place(at)?;
        // SAFETY: both DCs are released on every path.
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err(Error::Other("GetDC(screen) failed".into()));
            }
            let dc = GetDC(Some(self.hwnd));
            if dc.is_invalid() {
                ReleaseDC(None, screen);
                return Err(Error::Other("GetDC(overlay) failed".into()));
            }
            SetStretchBltMode(dc, HALFTONE);
            let _ = SetBrushOrgEx(dc, 0, 0, None);
            let blt = StretchBlt(
                dc,
                0,
                0,
                w,
                h,
                Some(screen),
                area.left,
                area.top,
                area.width(),
                area.height(),
                SRCCOPY | CAPTUREBLT,
            );
            ReleaseDC(Some(self.hwnd), dc);
            ReleaseDC(None, screen);
            if !blt.as_bool() {
                return Err(Error::Other("StretchBlt(screen) failed".into()));
            }
        }
        Ok(())
    }

    /// Make the window an opaque rectangle at `at`, shown on top.
    fn place(&mut self, at: Rect) -> Result<()> {
        // SAFETY: own window.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, NO_FILL);
            let _ = SetWindowRgn(self.hwnd, None, false);
            SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 255, LWA_ALPHA)
                .context("SetLayeredWindowAttributes(overlay)")?;
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                at.left,
                at.top,
                at.width(),
                at.height(),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .context("SetWindowPos(overlay)")?;
        }
        self.shown = Some(Shown::Picture);
        Ok(())
    }

    /// Leave this window out of screen captures, so a live view of the screen can be
    /// shown on top of it without showing itself (Windows 10 2004 and later).
    pub fn exclude_from_capture(&self) -> Result<()> {
        // SAFETY: own window.
        unsafe { SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE) }.context("SetWindowDisplayAffinity")
    }

    pub fn hide(&mut self) {
        self.dib = None;
        if self.shown.take().is_some() {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    pub fn is_visible(&self) -> bool {
        self.shown.is_some()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // SAFETY: the window was created by this object on this thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}
