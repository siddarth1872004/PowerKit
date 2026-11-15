//! Crop and Lock windows: a part of another application's window shown on its own,
//! either as a live DWM thumbnail (read-only) or by moving the real window inside a
//! smaller container (still takes input).

use crate::util::{pcwstr, wide};
use crate::window::{raw_rect, Hwnd};
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES,
    DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.CropAndLock";
/// How often a crop window checks that the window it shows still exists.
const WATCH_MS: u32 = 500;
const WATCH_TIMER: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropMode {
    Thumbnail,
    Reparent,
}

/// What a reparented window looked like before, to put it back.
struct Restore {
    style: isize,
    rect: Rect,
}

struct State {
    target: HWND,
    alive: Rc<Cell<bool>>,
    thumbnail: Option<isize>,
    /// The cropped part, relative to the target's window rectangle.
    source: RECT,
    restore: Option<Restore>,
}

impl State {
    /// Give a reparented window back to the desktop where it was. Runs once.
    fn release(&mut self) {
        if let Some(r) = self.restore.take() {
            // SAFETY: plain calls on a window handle that may be gone, which they report.
            unsafe {
                if IsWindow(Some(self.target)).as_bool() {
                    let _ = SetParent(self.target, None);
                    SetWindowLongPtrW(self.target, GWL_STYLE, r.style);
                    let _ = SetWindowPos(
                        self.target,
                        None,
                        r.rect.left,
                        r.rect.top,
                        r.rect.width(),
                        r.rect.height(),
                        SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    );
                }
            }
        }
        if let Some(t) = self.thumbnail.take() {
            // SAFETY: the thumbnail was registered by this window and is released once.
            let _ = unsafe { DwmUnregisterThumbnail(t) };
        }
    }
}

/// Largest rectangle with the source's aspect ratio that fits `area`, centered.
pub fn fit_center(src_w: i32, src_h: i32, area: Rect) -> Rect {
    if src_w <= 0 || src_h <= 0 || area.is_empty() {
        return area;
    }
    let (aw, ah) = (i64::from(area.width()), i64::from(area.height()));
    let (sw, sh) = (i64::from(src_w), i64::from(src_h));
    let (w, h) = if aw * sh <= ah * sw {
        (aw, aw * sh / sw)
    } else {
        (ah * sw / sh, ah)
    };
    let (w, h) = (w as i32, h as i32);
    Rect::from_size(
        area.left + (area.width() - w) / 2,
        area.top + (area.height() - h) / 2,
        w,
        h,
    )
}

fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    // SAFETY: USERDATA is either 0 or the Box<State> this module put there, freed only
    // in WM_NCDESTROY; the window procedure runs on one thread.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

fn place_thumbnail(hwnd: HWND, s: &State) {
    let Some(t) = s.thumbnail else { return };
    let mut client = RECT::default();
    // SAFETY: `client` is a valid out pointer.
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return;
    }
    let src = s.source;
    let dest = fit_center(
        src.right - src.left,
        src.bottom - src.top,
        Rect::new(client.left, client.top, client.right, client.bottom),
    );
    let props = DWM_THUMBNAIL_PROPERTIES {
        dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
        rcDestination: RECT {
            left: dest.left,
            top: dest.top,
            right: dest.right,
            bottom: dest.bottom,
        },
        rcSource: src,
        fVisible: true.into(),
        fSourceClientAreaOnly: false.into(),
        ..Default::default()
    };
    // SAFETY: `props` is fully initialised; `t` is this window's thumbnail.
    if let Err(e) = unsafe { DwmUpdateThumbnailProperties(t, &props) } {
        tracing::warn!(error = %e, "could not place the cropped thumbnail");
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            if let Some(s) = state(hwnd) {
                place_thumbnail(hwnd, s);
            }
        }
        WM_TIMER if wparam.0 == WATCH_TIMER => {
            // SAFETY: plain checks; the window closes itself when its target is gone.
            unsafe {
                if state(hwnd).is_some_and(|s| !IsWindow(Some(s.target)).as_bool()) {
                    let _ = DestroyWindow(hwnd);
                }
            }
            return LRESULT(0);
        }
        // Give the window back before the container (and so its children) goes away.
        WM_CLOSE | WM_DESTROY => {
            if let Some(s) = state(hwnd) {
                s.release();
            }
        }
        WM_NCDESTROY => {
            // SAFETY: take back the Box placed in USERDATA at creation, exactly once.
            unsafe {
                let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
                if !p.is_null() {
                    let mut s = Box::from_raw(p);
                    s.release();
                    s.alive.set(false);
                }
            }
        }
        _ => {}
    }
    // SAFETY: forwarding the unmodified arguments of this callback.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A crop window. Closing it (by the user or by dropping this) puts a reparented window
/// back where it was.
pub struct CropWindow {
    hwnd: HWND,
    target: Hwnd,
    alive: Rc<Cell<bool>>,
    _not_send: PhantomData<*const ()>,
}

impl CropWindow {
    /// Show `crop` (screen pixels, inside the target's window rectangle) of `target` in
    /// a window placed over the same spot.
    pub fn new(mode: CropMode, target: Hwnd, crop: Rect, title: &str) -> Result<CropWindow> {
        let window = raw_rect(target)?;
        let crop = crop
            .intersect(&window)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| Error::Other("the selection is outside the window".into()))?;
        let (style, ex_style) = match mode {
            CropMode::Thumbnail => (WS_OVERLAPPEDWINDOW, WINDOW_EX_STYLE(0)),
            // The real window keeps its size inside, so the container cannot be resized.
            CropMode::Reparent => (
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
                WINDOW_EX_STYLE(0),
            ),
        };
        let class = wide(CLASS_NAME);
        let text = wide(title);
        // SAFETY: the strings outlive the calls; `wndproc` matches WNDPROC. On any later
        // failure the window is destroyed, which frees its state.
        unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            };
            RegisterClassW(&wc); // "already registered" is expected after the first window
            let dpi = GetDpiForWindow(target.raw()).max(96);
            let mut outer = RECT {
                left: crop.left,
                top: crop.top,
                right: crop.right,
                bottom: crop.bottom,
            };
            AdjustWindowRectExForDpi(&mut outer, style, false, ex_style, dpi).context("AdjustWindowRectExForDpi")?;
            let hwnd = CreateWindowExW(
                ex_style,
                pcwstr(&class),
                pcwstr(&text),
                style,
                outer.left,
                outer.top,
                outer.right - outer.left,
                outer.bottom - outer.top,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(crop)")?;
            let alive = Rc::new(Cell::new(true));
            let state = Box::new(State {
                target: target.raw(),
                alive: Rc::clone(&alive),
                thumbnail: None,
                source: RECT {
                    left: crop.left - window.left,
                    top: crop.top - window.top,
                    right: crop.right - window.left,
                    bottom: crop.bottom - window.top,
                },
                restore: None,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            let me = CropWindow {
                hwnd,
                target,
                alive,
                _not_send: PhantomData,
            };
            me.attach(mode, crop, window)?;
            SetTimer(Some(hwnd), WATCH_TIMER, WATCH_MS, None);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            Ok(me)
        }
    }

    fn attach(&self, mode: CropMode, crop: Rect, window: Rect) -> Result<()> {
        let s = state(self.hwnd).ok_or_else(|| Error::Other("crop window has no state".into()))?;
        let target = self.target.raw();
        match mode {
            CropMode::Thumbnail => {
                // SAFETY: both windows exist; the id is stored and released by `release`.
                s.thumbnail = Some(unsafe { DwmRegisterThumbnail(self.hwnd, target) }.context("DwmRegisterThumbnail")?);
                place_thumbnail(self.hwnd, s);
            }
            CropMode::Reparent => {
                // SAFETY: plain calls; the original style and place are kept so `release`
                // can undo them, and are set before SetParent so a failure restores too.
                unsafe {
                    let style = GetWindowLongPtrW(target, GWL_STYLE);
                    s.restore = Some(Restore { style, rect: window });
                    let child = (style | WS_CHILD.0 as isize) & !(WS_POPUP.0 as isize);
                    SetWindowLongPtrW(target, GWL_STYLE, child);
                    SetParent(target, Some(self.hwnd)).context("SetParent")?;
                    SetWindowPos(
                        target,
                        None,
                        window.left - crop.left,
                        window.top - crop.top,
                        window.width(),
                        window.height(),
                        SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    )
                    .context("SetWindowPos")?;
                }
            }
        }
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.alive.get()
    }

    pub fn target(&self) -> Hwnd {
        self.target
    }

    pub fn hwnd(&self) -> Hwnd {
        Hwnd::from_raw(self.hwnd)
    }
}

impl Drop for CropWindow {
    fn drop(&mut self) {
        if self.alive.get() {
            // SAFETY: the window is still ours (`alive` is cleared when it is destroyed).
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_centers() {
        let area = Rect::new(0, 0, 400, 400);
        assert_eq!(fit_center(200, 100, area), Rect::new(0, 100, 400, 300));
        assert_eq!(fit_center(100, 200, area), Rect::new(100, 0, 300, 400));
        assert_eq!(fit_center(0, 10, area), area);
    }
}
