//! The Quick Notes window: one text box filling a small window. Edits are reported
//! as [`Event::Notes`] once typing pauses, and right away when the window is closed,
//! so nothing typed is lost. Closing it hides it.

use crate::event_loop::push;
use crate::util::{pcwstr, wide};
use crate::{Context, Result};
use powerkit_core::Event;
use std::marker::PhantomData;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::EM_SETLIMITTEXT;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.QuickNotes";
const ID_TEXT: i32 = 1;
const EDIT_TIMER: usize = 1;
const EDIT_DELAY_MS: u32 = 500;

// Sizes in 96-DPI pixels.
const WIDTH: i32 = 420;
const HEIGHT: i32 = 480;
const FONT_PT: i32 = 11;

struct State {
    font: HFONT,
    /// Set while the owner replaces the text, so that is not reported back.
    quiet: bool,
    /// Edited since the text was last reported.
    dirty: bool,
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// # Safety
/// `hwnd` must be a Quick Notes window whose state pointer has not been cleared.
unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    // SAFETY: the pointer is set in `new` and cleared in `Drop` before the box is freed.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

fn edit(parent: HWND) -> HWND {
    // SAFETY: accepts any handle; returns null when there is no such child.
    unsafe { GetDlgItem(Some(parent), ID_TEXT) }.unwrap_or_default()
}

fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: only used with messages whose pointer arguments outlive the call.
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

fn make_font(dpi: u32) -> HFONT {
    let face = wide("Segoe UI");
    // SAFETY: `face` outlives the call.
    unsafe {
        CreateFontW(
            -((FONT_PT * dpi as i32 + 36) / 72),
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
    }
}

fn apply_font(hwnd: HWND, st: &mut State, dpi: u32) {
    let old = std::mem::replace(&mut st.font, make_font(dpi));
    send(edit(hwnd), WM_SETFONT, st.font.0 as usize, 1);
    // SAFETY: the text box no longer uses the old font.
    let _ = unsafe { DeleteObject(old.into()) };
}

fn text_of(hwnd: HWND) -> String {
    // SAFETY: the buffer is sized from the control's text length.
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(hwnd) as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn report(hwnd: HWND, st: &mut State) {
    // SAFETY: own window and timer.
    let _ = unsafe { KillTimer(Some(hwnd), EDIT_TIMER) };
    if std::mem::take(&mut st.dirty) {
        push(Event::Notes(text_of(edit(hwnd))));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a Quick Notes window (this is its class's procedure).
    let st = unsafe { state(hwnd) };
    match (msg, st) {
        (WM_SIZE, _) => {
            let (w, h) = ((lparam.0 & 0xFFFF) as i32, ((lparam.0 >> 16) & 0xFFFF) as i32);
            // SAFETY: own child window.
            let _ = unsafe { MoveWindow(edit(hwnd), 0, 0, w, h, true) };
            LRESULT(0)
        }
        (WM_SETFOCUS, _) => {
            // SAFETY: own child window.
            let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit(hwnd))) };
            LRESULT(0)
        }
        (WM_DPICHANGED, Some(st)) => {
            apply_font(hwnd, st, (wparam.0 & 0xFFFF) as u32);
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
        (WM_COMMAND, Some(st)) => {
            let (id, code) = ((wparam.0 & 0xFFFF) as i32, ((wparam.0 >> 16) & 0xFFFF) as u32);
            if id == ID_TEXT && code == EN_CHANGE && !st.quiet {
                st.dirty = true;
                // SAFETY: own window; restarting the timer is the debounce.
                unsafe { SetTimer(Some(hwnd), EDIT_TIMER, EDIT_DELAY_MS, None) };
            }
            LRESULT(0)
        }
        (WM_TIMER, Some(st)) if wparam.0 == EDIT_TIMER => {
            report(hwnd, st);
            LRESULT(0)
        }
        (WM_CLOSE, st) => {
            if let Some(st) = st {
                report(hwnd, st);
            }
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub struct NotesWindow {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl NotesWindow {
    /// Creates the window hidden, holding `text`; [`NotesWindow::show`] brings it up.
    pub fn new(text: &str) -> Result<NotesWindow> {
        let class = wide(CLASS_NAME);
        let title = wide("Quick Notes");
        // SAFETY: strings outlive the calls; `wndproc` matches its callback contract.
        // The state pointer stays valid until `Drop` clears it.
        unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                ..Default::default()
            };
            RegisterClassW(&wc); // "already registered" is expected after the first one
            let dpi = GetDpiForSystem().max(96);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                pcwstr(&class),
                pcwstr(&title),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                scale(WIDTH, dpi),
                scale(HEIGHT, dpi),
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(quick notes)")?;
            let edit_class = wide("EDIT");
            let style = WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32);
            let created = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                pcwstr(&edit_class),
                None,
                style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(ID_TEXT as isize as *mut _)),
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(quick notes text)");
            let text_box = match created {
                Ok(h) => h,
                Err(e) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(e);
                }
            };
            // Lift the 32K default; notes kept for months grow past it.
            send(text_box, EM_SETLIMITTEXT, 0, 0);
            let mut state = Box::new(State {
                font: make_font(dpi),
                quiet: false,
                dirty: false,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            apply_font(hwnd, &mut state, GetDpiForWindow(hwnd).max(96));
            let mut w = NotesWindow {
                hwnd,
                state,
                _not_send: PhantomData,
            };
            w.set_text(text);
            Ok(w)
        }
    }

    pub fn show(&mut self) {
        // SAFETY: own window. Return values are the previous state, not errors.
        unsafe {
            let cmd = if IsIconic(self.hwnd).as_bool() {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            let _ = ShowWindow(self.hwnd, cmd);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    /// Hide it as closing it does, reporting unsaved edits first.
    pub fn hide(&mut self) {
        send(self.hwnd, WM_CLOSE, 0, 0);
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: own window.
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Whether it is the window in front, so the shortcut can put it away.
    pub fn is_foreground(&self) -> bool {
        // SAFETY: no arguments.
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    pub fn set_topmost(&mut self, on: bool) {
        let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        // SAFETY: own window.
        if let Err(e) = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        } {
            tracing::warn!("Quick Notes on top: {e}");
        }
    }

    /// Replace the text (lines end in CR LF) without reporting it as an edit.
    pub fn set_text(&mut self, text: &str) {
        self.state.quiet = true;
        let t = wide(text);
        // SAFETY: `t` outlives the call.
        let _ = unsafe { SetWindowTextW(edit(self.hwnd), pcwstr(&t)) };
        self.state.quiet = false;
        self.state.dirty = false;
    }

    pub fn text(&self) -> String {
        text_of(edit(self.hwnd))
    }

    /// Show `ico` in the title bar.
    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }
}

impl Drop for NotesWindow {
    fn drop(&mut self) {
        // SAFETY: own window, on the thread that created it; the state pointer is
        // cleared before the box is freed.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.state.font.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_text_it_is_given() {
        let mut w = NotesWindow::new("first\r\nsecond").unwrap();
        assert_eq!(w.text(), "first\r\nsecond");
        assert!(!w.state.dirty, "text from the owner is not an edit");
        w.set_text("");
        assert_eq!(w.text(), "");
        assert!(!w.is_visible());
    }
}
