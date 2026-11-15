//! A click-through, topmost window that shows a picture drawn with text and filled
//! shapes: the Shortcut Guide panel and its taskbar badges.

use crate::magnifier::Dib;
use crate::util::{colorref, pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Canvas";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Title,
    Body,
    Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
}

/// Font sizes in pixels at 96 DPI, and weights.
fn spec(font: Font) -> (u32, i32) {
    match font {
        Font::Title => (18, FW_SEMIBOLD.0 as i32),
        Font::Body => (14, FW_NORMAL.0 as i32),
        Font::Key => (13, FW_SEMIBOLD.0 as i32),
    }
}

struct Fonts {
    dpi: u32,
    fonts: [HFONT; 3],
}

impl Fonts {
    fn new(dpi: u32) -> Fonts {
        let face = wide("Segoe UI");
        let make = |font: Font| {
            let (px, weight) = spec(font);
            let size = ((px * dpi + 48) / 96) as i32;
            // SAFETY: `face` outlives the call.
            unsafe {
                CreateFontW(
                    -size,
                    0,
                    0,
                    0,
                    weight,
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
            }
        };
        Fonts {
            dpi,
            fonts: [make(Font::Title), make(Font::Body), make(Font::Key)],
        }
    }

    fn get(&self, font: Font) -> HFONT {
        self.fonts[font as usize]
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for f in self.fonts {
            // SAFETY: created by `Fonts::new`; never left selected (each use restores).
            let _ = unsafe { DeleteObject(f.into()) };
        }
    }
}

/// Draws into the canvas picture; coordinates are relative to its top-left corner.
pub struct Painter<'a> {
    dc: HDC,
    fonts: &'a Fonts,
}

impl Painter<'_> {
    /// `v` pixels at 96 DPI in this canvas's DPI.
    pub fn scale(&self, v: i32) -> i32 {
        (v * self.fonts.dpi as i32 + 48) / 96
    }

    pub fn measure(&self, text: &str, font: Font) -> (i32, i32) {
        let t: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE::default();
        // SAFETY: the font is restored before returning; `size` is a valid out pointer.
        unsafe {
            let old = SelectObject(self.dc, self.fonts.get(font).into());
            let _ = GetTextExtentPoint32W(self.dc, &t, &mut size);
            SelectObject(self.dc, old);
        }
        (size.cx, size.cy)
    }

    pub fn fill(&self, r: Rect, color: &str) {
        self.round(r, 0, color);
    }

    pub fn round(&self, r: Rect, radius: i32, color: &str) {
        // SAFETY: brush and region are created and deleted here.
        unsafe {
            let brush = CreateSolidBrush(COLORREF(colorref(color)));
            let rgn = CreateRoundRectRgn(r.left, r.top, r.right + 1, r.bottom + 1, radius * 2, radius * 2);
            let _ = FillRgn(self.dc, rgn, brush);
            let _ = DeleteObject(rgn.into());
            let _ = DeleteObject(brush.into());
        }
    }

    /// One line of text, vertically centred in `r` and cut with an ellipsis if too long.
    pub fn text(&self, r: Rect, text: &str, color: &str, font: Font, align: Align) {
        let mut t: Vec<u16> = text.encode_utf16().collect();
        let mut rc = RECT {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
        let flags = DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS;
        let flags = match align {
            Align::Left => flags | DT_LEFT,
            Align::Center => flags | DT_CENTER,
        };
        // SAFETY: the font is restored before returning.
        unsafe {
            let old = SelectObject(self.dc, self.fonts.get(font).into());
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, COLORREF(colorref(color)));
            DrawTextW(self.dc, &mut t, &mut rc, flags);
            SelectObject(self.dc, old);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub struct Canvas {
    hwnd: HWND,
    fonts: Option<Fonts>,
    visible: bool,
    _not_send: PhantomData<*const ()>,
}

impl Canvas {
    pub fn new() -> Result<Canvas> {
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
            .context("CreateWindowEx(canvas)")?
        };
        Ok(Canvas {
            hwnd,
            fonts: None,
            visible: false,
            _not_send: PhantomData,
        })
    }

    /// `layout` measures what it needs and says where the picture goes (screen pixels);
    /// `draw` then paints it. `alpha` is the whole window's opacity.
    pub fn show(
        &mut self,
        dpi: u32,
        alpha: u8,
        layout: impl FnOnce(&Painter) -> Rect,
        draw: impl FnOnce(&Painter),
    ) -> Result<()> {
        if self.fonts.as_ref().is_none_or(|f| f.dpi != dpi) {
            self.fonts = Some(Fonts::new(dpi));
        }
        let fonts = self.fonts.as_ref().expect("fonts were just created");
        let measure = Dib::new(1, 1)?;
        let at = layout(&Painter { dc: measure.dc, fonts });
        drop(measure);
        let (w, h) = (at.width(), at.height());
        if w <= 0 || h <= 0 {
            self.hide();
            return Ok(());
        }
        let dib = Dib::new(w, h)?;
        draw(&Painter { dc: dib.dc, fonts });
        // SAFETY: own window; the window DC is released on every path.
        unsafe {
            SetLayeredWindowAttributes(self.hwnd, COLORREF(0), alpha, LWA_ALPHA)
                .context("SetLayeredWindowAttributes(canvas)")?;
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                at.left,
                at.top,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .context("SetWindowPos(canvas)")?;
            self.visible = true;
            let dc = GetDC(Some(self.hwnd));
            if dc.is_invalid() {
                return Err(Error::Other("GetDC(canvas) failed".into()));
            }
            let blt = BitBlt(dc, 0, 0, w, h, Some(dib.dc), 0, 0, SRCCOPY);
            ReleaseDC(Some(self.hwnd), dc);
            blt.context("BitBlt(canvas)")?;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if std::mem::take(&mut self.visible) {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: the window was created by this object on this thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}
