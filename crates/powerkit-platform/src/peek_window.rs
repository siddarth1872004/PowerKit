//! The Peek window: shows a picture, text or a file's details, and reports the keys
//! that browse, open and close as [`Event::Peek`]s. Closing it hides it.

use crate::event_loop::push;
use crate::util::{pcwstr, wide};
use crate::{Context, Result};
use powerkit_core::{Event, PeekEvent, Rect};
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Peek";
/// Margins and the footer's height in 96-DPI pixels.
const PAD: i32 = 16;
const FOOTER: i32 = 28;

/// What the window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeekView {
    /// Opaque BGRA pixels, row by row; shown at most at this size, smaller to fit.
    Picture {
        width: u32,
        height: u32,
        pixels: Vec<u32>,
    },
    Text(String),
    Details {
        name: String,
        rows: Vec<(String, String)>,
    },
    /// A line in the middle: loading, or why nothing can be shown.
    Message(String),
}

struct Colors {
    back: COLORREF,
    text: COLORREF,
    muted: COLORREF,
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn colors(light: bool) -> Colors {
    if light {
        Colors {
            back: rgb(243, 243, 243),
            text: rgb(26, 26, 26),
            muted: rgb(96, 96, 96),
        }
    } else {
        Colors {
            back: rgb(32, 32, 32),
            text: rgb(240, 240, 240),
            muted: rgb(160, 160, 160),
        }
    }
}

/// The background as packed BGRA, for flattening pictures with transparency.
pub fn background(light: bool) -> u32 {
    let c = colors(light).back.0;
    let (r, g, b) = (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF);
    0xFF00_0000 | r << 16 | g << 8 | b
}

struct Fonts {
    body: HFONT,
    title: HFONT,
    mono: HFONT,
}

impl Fonts {
    fn new(dpi: u32) -> Fonts {
        let make = |pt: i32, weight: i32, face: &str| {
            let face = wide(face);
            // SAFETY: `face` outlives the call.
            unsafe {
                CreateFontW(
                    -((pt * dpi as i32 + 36) / 72),
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
            body: make(10, FW_NORMAL.0 as i32, "Segoe UI"),
            title: make(16, FW_SEMIBOLD.0 as i32, "Segoe UI"),
            mono: make(10, FW_NORMAL.0 as i32, "Consolas"),
        }
    }

    fn delete(&self) {
        // SAFETY: fonts created by `new`, deleted once.
        unsafe {
            let _ = DeleteObject(self.body.into());
            let _ = DeleteObject(self.title.into());
            let _ = DeleteObject(self.mono.into());
        }
    }
}

struct State {
    view: PeekView,
    footer: String,
    /// First text line shown.
    scroll: usize,
    dpi: u32,
    fonts: Fonts,
    light: bool,
}

/// # Safety
/// `hwnd` must be a Peek window whose state pointer has not been cleared.
unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    // SAFETY: the pointer is set in `new` and cleared in `Drop` before the box is freed.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

fn text(dc: HDC, s: &str, mut r: RECT, font: HFONT, color: COLORREF, flags: DRAW_TEXT_FORMAT) {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    // SAFETY: `dc` is a valid memory DC; `w` and `r` outlive the call.
    unsafe {
        SelectObject(dc, font.into());
        SetTextColor(dc, color);
        DrawTextW(dc, &mut w, &mut r, flags | DT_NOPREFIX);
    }
}

fn line_height(dc: HDC, font: HFONT) -> i32 {
    let mut tm = TEXTMETRICW::default();
    // SAFETY: valid DC and out pointer.
    unsafe {
        SelectObject(dc, font.into());
        let _ = GetTextMetricsW(dc, &mut tm);
    }
    tm.tmHeight.max(1)
}

/// Where a `w`x`h` picture goes in `area`: centered, scaled down to fit, never up.
pub fn fit(w: i32, h: i32, area: Rect) -> Rect {
    let s = (area.width() as f64 / w.max(1) as f64)
        .min(area.height() as f64 / h.max(1) as f64)
        .min(1.0);
    let (fw, fh) = (((w as f64 * s) as i32).max(1), ((h as f64 * s) as i32).max(1));
    Rect::from_size(
        area.left + (area.width() - fw) / 2,
        area.top + (area.height() - fh) / 2,
        fw,
        fh,
    )
}

fn paint(hwnd: HWND, st: &State) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: BeginPaint/EndPaint pair on own window; the memory DC and bitmap are
    // created and freed here; the pixel buffer outlives StretchDIBits.
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let (cw, ch) = (client.right.max(1), client.bottom.max(1));
        let dc = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, cw, ch);
        let old = SelectObject(dc, bmp.into());
        let c = colors(st.light);
        let brush = CreateSolidBrush(c.back);
        FillRect(dc, &client, brush);
        let _ = DeleteObject(brush.into());
        SetBkMode(dc, TRANSPARENT);

        let pad = scale(PAD, st.dpi);
        let footer = scale(FOOTER, st.dpi);
        let body = RECT {
            left: pad,
            top: pad,
            right: cw - pad,
            bottom: ch - footer,
        };
        match &st.view {
            PeekView::Picture { width, height, pixels } => {
                let at = fit(
                    *width as i32,
                    *height as i32,
                    Rect::new(body.left, body.top, body.right, body.bottom),
                );
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: *width as i32,
                        biHeight: -(*height as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                SetStretchBltMode(dc, HALFTONE);
                StretchDIBits(
                    dc,
                    at.left,
                    at.top,
                    at.width(),
                    at.height(),
                    0,
                    0,
                    *width as i32,
                    *height as i32,
                    Some(pixels.as_ptr().cast()),
                    &info,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            }
            PeekView::Text(s) => {
                let lh = line_height(dc, st.fonts.mono);
                let mut y = body.top;
                for line in s.lines().skip(st.scroll) {
                    if y >= body.bottom {
                        break;
                    }
                    let r = RECT {
                        left: body.left,
                        top: y,
                        right: body.right,
                        bottom: y + lh,
                    };
                    text(
                        dc,
                        line,
                        r,
                        st.fonts.mono,
                        c.text,
                        DT_SINGLELINE | DT_EXPANDTABS | DT_END_ELLIPSIS,
                    );
                    y += lh;
                }
            }
            PeekView::Details { name, rows } => {
                let th = line_height(dc, st.fonts.title);
                let lh = line_height(dc, st.fonts.body);
                let mut y = body.top + (body.bottom - body.top - th - 12 - rows.len() as i32 * (lh + 4)).max(0) / 3;
                let full = |y: i32, h: i32| RECT {
                    left: body.left,
                    top: y,
                    right: body.right,
                    bottom: y + h,
                };
                text(
                    dc,
                    name,
                    full(y, th),
                    st.fonts.title,
                    c.text,
                    DT_CENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                );
                y += th + 12;
                let mid = (body.left + body.right) / 2;
                for (k, v) in rows {
                    let mut left = full(y, lh);
                    left.right = mid - 8;
                    text(dc, k, left, st.fonts.body, c.muted, DT_RIGHT | DT_SINGLELINE);
                    let mut right = full(y, lh);
                    right.left = mid + 8;
                    text(
                        dc,
                        v,
                        right,
                        st.fonts.body,
                        c.text,
                        DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS,
                    );
                    y += lh + 4;
                }
            }
            PeekView::Message(m) => {
                text(
                    dc,
                    m,
                    body,
                    st.fonts.body,
                    c.muted,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
            }
        }
        let foot = RECT {
            left: pad,
            top: ch - footer,
            right: cw - pad,
            bottom: ch,
        };
        text(
            dc,
            &st.footer,
            foot,
            st.fonts.body,
            c.muted,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );

        let _ = BitBlt(hdc, 0, 0, cw, ch, Some(dc), 0, 0, SRCCOPY);
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn scroll_by(hwnd: HWND, st: &mut State, lines: i32) {
    if let PeekView::Text(s) = &st.view {
        let max = s.lines().count().saturating_sub(1);
        st.scroll = (st.scroll as i64 + lines as i64).clamp(0, max as i64) as usize;
        // SAFETY: own window.
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a Peek window (this is its class's procedure).
    let st = unsafe { state(hwnd) };
    match (msg, st) {
        (WM_PAINT, Some(st)) => {
            paint(hwnd, st);
            LRESULT(0)
        }
        (WM_ERASEBKGND, _) => LRESULT(1),
        (WM_SIZE, _) => {
            // SAFETY: own window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            LRESULT(0)
        }
        (WM_DPICHANGED, Some(st)) => {
            st.fonts.delete();
            st.dpi = (wparam.0 & 0xFFFF) as u32;
            st.fonts = Fonts::new(st.dpi);
            // SAFETY: Windows passes the suggested window rectangle in `lparam`.
            let r = unsafe { &*(lparam.0 as *const RECT) };
            // SAFETY: own window.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            LRESULT(0)
        }
        (WM_KEYDOWN, Some(st)) => {
            let key = windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(wparam.0 as u16);
            let text = matches!(st.view, PeekView::Text(_));
            let event = match key {
                VK_LEFT => Some(PeekEvent::Previous),
                VK_RIGHT => Some(PeekEvent::Next),
                VK_UP if !text => Some(PeekEvent::Previous),
                VK_DOWN if !text => Some(PeekEvent::Next),
                VK_UP => {
                    scroll_by(hwnd, st, -1);
                    None
                }
                VK_DOWN => {
                    scroll_by(hwnd, st, 1);
                    None
                }
                VK_PRIOR => {
                    scroll_by(hwnd, st, -20);
                    None
                }
                VK_NEXT => {
                    scroll_by(hwnd, st, 20);
                    None
                }
                VK_RETURN => Some(PeekEvent::Open),
                VK_ESCAPE => {
                    // SAFETY: own window.
                    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                    Some(PeekEvent::Closed)
                }
                _ => None,
            };
            if let Some(e) = event {
                push(Event::Peek(e));
            }
            LRESULT(0)
        }
        (WM_MOUSEWHEEL, Some(st)) => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            scroll_by(hwnd, st, -delta / 40);
            LRESULT(0)
        }
        (WM_CLOSE, _) => {
            // SAFETY: own window.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            push(Event::Peek(PeekEvent::Closed));
            LRESULT(0)
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub struct PeekWindow {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl PeekWindow {
    /// Creates the window hidden; `light` picks the light or dark colors.
    pub fn new(light: bool) -> Result<PeekWindow> {
        let class = wide(CLASS_NAME);
        // SAFETY: strings outlive the calls; `wndproc` matches its callback contract.
        // The state pointer stays valid until `Drop` clears it.
        unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc); // "already registered" is expected after the first one
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                pcwstr(&class),
                pcwstr(&class),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                800,
                600,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(peek)")?;
            let dark = windows::core::BOOL::from(!light);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                std::mem::size_of_val(&dark) as u32,
            );
            let dpi = GetDpiForWindow(hwnd).max(96);
            let mut state = Box::new(State {
                view: PeekView::Message(String::new()),
                footer: String::new(),
                scroll: 0,
                dpi,
                fonts: Fonts::new(dpi),
                light,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            Ok(PeekWindow {
                hwnd,
                state,
                _not_send: PhantomData,
            })
        }
    }

    /// Bring the window up in the middle of `area` if it is hidden, at `share`
    /// percent of its size, and give it the keyboard.
    pub fn show(&mut self, area: Rect, share: u32) {
        // SAFETY: own window. Return values are the previous state, not errors.
        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool() {
                let (w, h) = (area.width() * share as i32 / 100, area.height() * share as i32 / 100);
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    area.left + (area.width() - w) / 2,
                    area.top + (area.height() - h) / 2,
                    w,
                    h,
                    SWP_NOZORDER,
                );
                let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn hide(&mut self) {
        // SAFETY: own window.
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: own window.
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// The size a picture would be shown at most, in pixels.
    pub fn picture_area(&self) -> (u32, u32) {
        let mut r = RECT::default();
        // SAFETY: own window and a valid out pointer.
        let _ = unsafe { GetClientRect(self.hwnd, &mut r) };
        let (pad, footer) = (scale(PAD, self.state.dpi), scale(FOOTER, self.state.dpi));
        (
            (r.right - 2 * pad).max(1) as u32,
            (r.bottom - pad - footer).max(1) as u32,
        )
    }

    pub fn set(&mut self, title: &str, footer: &str, view: PeekView) {
        self.state.view = view;
        self.state.footer = footer.into();
        self.state.scroll = 0;
        let t = wide(title);
        // SAFETY: own window; `t` outlives the calls.
        unsafe {
            let _ = SetWindowTextW(self.hwnd, pcwstr(&t));
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }

    pub fn hwnd(&self) -> crate::Hwnd {
        crate::Hwnd::from_raw(self.hwnd)
    }
}

impl Drop for PeekWindow {
    fn drop(&mut self) {
        // SAFETY: own window, on the thread that created it; the state pointer is
        // cleared before the box is freed.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
        self.state.fonts.delete();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_centered_and_only_shrunk() {
        let area = Rect::new(0, 0, 800, 600);
        assert_eq!(fit(400, 300, area), Rect::new(200, 150, 600, 450));
        assert_eq!(fit(1600, 600, area), Rect::new(0, 150, 800, 450));
    }
}
