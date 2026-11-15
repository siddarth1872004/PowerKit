//! A click-through, topmost colored frame drawn around another window (used as the
//! "pinned" indicator). It is a real window shaped into a ring with a window region,
//! so it costs no per-frame bitmap work and never intercepts input.

use crate::util::{colorref, pcwstr, wide};
use crate::{Context, Result};
use powerkit_core::Rect;
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    SetWindowRgn, PAINTSTRUCT, RGN_DIFF,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Frame";

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: standard paint sequence on the window this procedure was called for; the
    // brush is created and deleted within the block.
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let color = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32;
                let brush = CreateSolidBrush(COLORREF(color));
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(hdc, &rc, brush);
                let _ = DeleteObject(brush.into());
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

pub struct FrameWindow {
    hwnd: HWND,
    last: Option<(Rect, i32)>,
    _not_send: PhantomData<*const ()>,
}

impl FrameWindow {
    /// Create a hidden frame with the given `#RRGGBB` color.
    pub fn new(color: &str) -> Result<FrameWindow> {
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
            RegisterClassW(&wc); // "already registered" is expected after the first frame
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
            .context("CreateWindowEx(frame)")?
        };
        let frame = FrameWindow {
            hwnd,
            last: None,
            _not_send: PhantomData,
        };
        // SAFETY: the window was just created by this thread.
        unsafe {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA).context("SetLayeredWindowAttributes")?;
        }
        frame.set_color(color);
        Ok(frame)
    }

    pub fn set_color(&self, color: &str) {
        // SAFETY: own window; USERDATA holds a plain COLORREF value.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, colorref(color) as isize);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Show the frame as a ring of `thickness` pixels whose outer edge is `outer`.
    pub fn show(&mut self, outer: Rect, thickness: i32) -> Result<()> {
        let thickness = thickness.clamp(1, (outer.width().min(outer.height()) / 2).max(1));
        if self.last == Some((outer, thickness)) {
            return Ok(());
        }
        let (w, h) = (outer.width(), outer.height());
        // SAFETY: regions are created here; SetWindowRgn takes ownership of `ring` on
        // success, `hole` is deleted after use.
        unsafe {
            let ring = CreateRectRgn(0, 0, w, h);
            let hole = CreateRectRgn(thickness, thickness, w - thickness, h - thickness);
            CombineRgn(Some(ring), Some(ring), Some(hole), RGN_DIFF);
            let _ = DeleteObject(hole.into());
            if SetWindowRgn(self.hwnd, Some(ring), true) == 0 {
                let _ = DeleteObject(ring.into());
            }
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                outer.left,
                outer.top,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .context("SetWindowPos(frame)")?;
        }
        self.last = Some((outer, thickness));
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.last.take().is_some() {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    pub fn is_visible(&self) -> bool {
        self.last.is_some()
    }
}

impl Drop for FrameWindow {
    fn drop(&mut self) {
        // SAFETY: the window was created by this object on this thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}
