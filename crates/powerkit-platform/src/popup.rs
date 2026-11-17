//! The launcher popup: a search box with a list of result rows underneath, centred
//! near the top of the monitor under the cursor.
//!
//! The popup reports what the user does as [`PopupEvent`]s on the event loop; the
//! owner answers query changes with [`Popup::set_rows`]. Up/Down/Tab move the
//! selection, Enter or a click chooses a row, Esc or clicking elsewhere closes it.

use crate::event_loop::push;
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::{Event, PopupEvent};
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_TAB, VK_UP};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Launcher";
const SUBCLASS_ID: usize = 1;
const EN_CHANGE: u32 = 0x0300;

// Sizes in 96-DPI pixels.
const WIDTH: u32 = 680;
const HEADER: u32 = 60;
const EDIT_FONT: u32 = 22;
const ROW: u32 = 52;
const TITLE_FONT: u32 = 16;
const SUB_FONT: u32 = 12;
const PAD: u32 = 16;
const ACCENT_W: u32 = 3;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}
const BG: COLORREF = rgb(0x20, 0x20, 0x20);
const SELECTED: COLORREF = rgb(0x38, 0x38, 0x38);
const ACCENT: COLORREF = rgb(0x4C, 0xC2, 0xFF);
const TEXT: COLORREF = rgb(0xF2, 0xF2, 0xF2);
const SUBTEXT: COLORREF = rgb(0xA8, 0xA8, 0xA8);
const LINE: COLORREF = rgb(0x48, 0x48, 0x48);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub title: String,
    pub subtitle: String,
}

/// Window height for `rows` result rows at `scale(px96)`.
fn height(rows: usize, scale: impl Fn(u32) -> i32) -> i32 {
    let list = if rows == 0 {
        0
    } else {
        scale(ROW) * rows as i32 + scale(PAD) / 2
    };
    scale(HEADER) + list
}

/// The row under client coordinate `y`, if any.
fn row_at(y: i32, rows: usize, scale: impl Fn(u32) -> i32) -> Option<usize> {
    // Rows start a quarter padding below the header, as painted.
    let y = y - scale(HEADER) - scale(PAD) / 4;
    if y < 0 {
        return None;
    }
    let i = (y / scale(ROW)) as usize;
    (i < rows).then_some(i)
}

/// Selection after moving by `delta`, wrapping around.
fn step(selected: usize, delta: i32, rows: usize) -> usize {
    if rows == 0 {
        return 0;
    }
    (selected as i64 + delta as i64).rem_euclid(rows as i64) as usize
}

struct Fonts {
    dpi: u32,
    edit: HFONT,
    title: HFONT,
    sub: HFONT,
}

impl Fonts {
    fn delete(&self) {
        // SAFETY: fonts created by `make_font`, no longer selected anywhere (each paint
        // restores the original font) and not used by the edit after replacement.
        unsafe {
            let _ = DeleteObject(self.edit.into());
            let _ = DeleteObject(self.title.into());
            let _ = DeleteObject(self.sub.into());
        }
    }
}

fn make_font(px: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    // SAFETY: `face` outlives the call.
    unsafe {
        CreateFontW(
            -px,
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
}

/// State shared with the window procedures through `GWLP_USERDATA`.
struct State {
    /// The id of the utility the popup belongs to, so its events reach only that one.
    owner: &'static str,
    edit: HWND,
    rows: Vec<Row>,
    selected: usize,
    dpi: u32,
    visible: bool,
    fonts: Option<Fonts>,
    bg_brush: HBRUSH,
}

impl State {
    fn scale(&self, px96: u32) -> i32 {
        (px96 as i64 * self.dpi as i64 / 96) as i32
    }
}

/// # Safety
/// `hwnd` must be a popup window created by [`Popup::new`] whose state is still alive.
unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    // SAFETY: the pointer was stored by `Popup::new` and is cleared before the state is
    // freed; callbacks run on the owning thread, one at a time.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

fn hide_window(hwnd: HWND, st: &mut State) {
    if std::mem::take(&mut st.visible) {
        // SAFETY: own window. The return value is the previous visibility.
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

fn edit_text(edit: HWND) -> String {
    // SAFETY: the buffer is sized from the reported length plus the terminator.
    unsafe {
        let len = GetWindowTextLengthW(edit).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(edit, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

fn paint(hwnd: HWND, st: &State) {
    // SAFETY: standard BeginPaint/EndPaint pair on our own window; the back buffer and
    // every selected object are released or restored before returning.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right, client.bottom);
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, w, h);
        let old_bmp = SelectObject(mem, bmp.into());

        let fill = |r: RECT, c: COLORREF| {
            let b = CreateSolidBrush(c);
            FillRect(mem, &r, b);
            let _ = DeleteObject(b.into());
        };
        fill(client, BG);
        let border = CreateSolidBrush(LINE);
        FrameRect(mem, &client, border);
        let _ = DeleteObject(border.into());

        if let Some(fonts) = &st.fonts {
            let header = st.scale(HEADER);
            let row_h = st.scale(ROW);
            let pad = st.scale(PAD);
            if !st.rows.is_empty() {
                fill(
                    RECT {
                        left: 1,
                        top: header - 1,
                        right: w - 1,
                        bottom: header,
                    },
                    LINE,
                );
            }
            SetBkMode(mem, TRANSPARENT);
            let old_font = SelectObject(mem, fonts.title.into());
            for (i, row) in st.rows.iter().enumerate() {
                let top = header + row_h * i as i32 + pad / 4;
                if i == st.selected {
                    fill(
                        RECT {
                            left: pad / 2,
                            top,
                            right: w - pad / 2,
                            bottom: top + row_h,
                        },
                        SELECTED,
                    );
                    let bar = RECT {
                        left: pad / 2,
                        top: top + row_h / 4,
                        right: pad / 2 + st.scale(ACCENT_W),
                        bottom: top + row_h * 3 / 4,
                    };
                    fill(bar, ACCENT);
                }
                let flags = DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS | DT_VCENTER;
                let mut title: Vec<u16> = row.title.encode_utf16().collect();
                let mut rc = RECT {
                    left: pad + pad / 2,
                    top: top + row_h / 10,
                    right: w - pad,
                    bottom: top + row_h / 2 + row_h / 10,
                };
                SelectObject(mem, fonts.title.into());
                SetTextColor(mem, TEXT);
                DrawTextW(mem, &mut title, &mut rc, flags);
                let mut sub: Vec<u16> = row.subtitle.encode_utf16().collect();
                let mut rc = RECT {
                    left: rc.left,
                    top: top + row_h / 2,
                    right: rc.right,
                    bottom: top + row_h - row_h / 10,
                };
                SelectObject(mem, fonts.sub.into());
                SetTextColor(mem, SUBTEXT);
                DrawTextW(mem, &mut sub, &mut rc, flags | DT_PATH_ELLIPSIS);
            }
            SelectObject(mem, old_font);
        }

        let _ = BitBlt(dc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn move_selection(hwnd: HWND, st: &mut State, delta: i32) {
    st.selected = step(st.selected, delta, st.rows.len());
    // SAFETY: own window.
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn choose(hwnd: HWND, st: &mut State, row: usize) {
    if row < st.rows.len() {
        // Hide first so focus returns to the previous window before the action runs
        // (a copied result is then pasted where the user was).
        hide_window(hwnd, st);
        push(Event::Popup {
            owner: st.owner,
            event: PopupEvent::Activate(row),
        });
    }
}

fn dismiss(hwnd: HWND, st: &mut State) {
    if st.visible {
        hide_window(hwnd, st);
        push(Event::Popup {
            owner: st.owner,
            event: PopupEvent::Dismiss,
        });
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a popup window (only this class uses this procedure).
    let Some(st) = (unsafe { state(hwnd) }) else {
        // SAFETY: forwarding the unmodified arguments of this callback.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd, st);
            LRESULT(0)
        }
        WM_COMMAND if (wparam.0 >> 16) as u32 == EN_CHANGE => {
            push(Event::Popup {
                owner: st.owner,
                event: PopupEvent::Query(edit_text(st.edit)),
            });
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            let dc = HDC(wparam.0 as *mut _);
            // SAFETY: the DC belongs to the edit control for the duration of this message.
            unsafe {
                SetTextColor(dc, TEXT);
                SetBkColor(dc, BG);
            }
            LRESULT(st.bg_brush.0 as isize)
        }
        WM_LBUTTONDOWN => {
            let y = (lparam.0 >> 16) as i16 as i32;
            if let Some(row) = row_at(y, st.rows.len(), |p| st.scale(p)) {
                st.selected = row;
                choose(hwnd, st, row);
            }
            LRESULT(0)
        }
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            dismiss(hwnd, st);
            LRESULT(0)
        }
        WM_CLOSE => {
            dismiss(hwnd, st);
            LRESULT(0)
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Keyboard handling for the search box; `parent` (in `refdata`) owns the state.
unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    refdata: usize,
) -> LRESULT {
    let parent = HWND(refdata as *mut _);
    // SAFETY: `refdata` is the popup window passed to SetWindowSubclass.
    if let Some(st) = unsafe { state(parent) } {
        let vk = wparam.0 as u16;
        match msg {
            WM_KEYDOWN if vk == VK_DOWN.0 || vk == VK_TAB.0 => {
                move_selection(parent, st, 1);
                return LRESULT(0);
            }
            WM_KEYDOWN if vk == VK_UP.0 => {
                move_selection(parent, st, -1);
                return LRESULT(0);
            }
            WM_KEYDOWN if vk == VK_RETURN.0 => {
                let row = st.selected;
                choose(parent, st, row);
                return LRESULT(0);
            }
            WM_KEYDOWN if vk == VK_ESCAPE.0 => {
                dismiss(parent, st);
                return LRESULT(0);
            }
            // Swallow the characters of the keys handled above so the edit does not beep.
            WM_CHAR if matches!(wparam.0, 0x09 | 0x0D | 0x1B) => return LRESULT(0),
            // Ctrl+Backspace: delete the previous word instead of inserting a box glyph.
            WM_CHAR if wparam.0 == 0x7F => {
                let text = edit_text(hwnd);
                let trimmed = text.trim_end();
                let keep = trimmed.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1);
                let w = wide(&text[..keep]);
                // SAFETY: own child window; `w` outlives the calls.
                unsafe {
                    let _ = SetWindowTextW(hwnd, pcwstr(&w));
                    let end = (w.len() - 1) as isize;
                    SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(end as usize)), Some(LPARAM(end)));
                }
                return LRESULT(0);
            }
            _ => {}
        }
    }
    // SAFETY: forwarding the unmodified arguments of this callback.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

pub struct Popup {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl Popup {
    pub fn new(owner: &'static str) -> Result<Popup> {
        let class = wide(CLASS_NAME);
        let edit_class = wide("EDIT");
        let empty = wide("");
        // SAFETY: strings outlive the calls; `wndproc` and `edit_proc` match their
        // callback contracts. The state pointer stays valid until `Drop` clears it.
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
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
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
            .context("CreateWindowEx(launcher)")?;
            let edit = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                pcwstr(&edit_class),
                pcwstr(&empty),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                0,
                0,
                0,
                0,
                Some(hwnd),
                None,
                Some(instance.into()),
                None,
            ) {
                Ok(e) => e,
                Err(e) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(e).context("CreateWindowEx(launcher search box)");
                }
            };
            let mut state = Box::new(State {
                owner,
                edit,
                rows: Vec::new(),
                selected: 0,
                dpi: 96,
                visible: false,
                fonts: None,
                bg_brush: CreateSolidBrush(BG),
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            if !SetWindowSubclass(edit, Some(edit_proc), SUBCLASS_ID, hwnd.0 as usize).as_bool() {
                let _ = DestroyWindow(hwnd);
                let _ = DeleteObject(state.bg_brush.into());
                return Err(Error::Other("SetWindowSubclass(launcher search box) failed".into()));
            }
            // Rounded corners on Windows 11; older versions ignore the attribute.
            let pref = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const _,
                std::mem::size_of_val(&pref) as u32,
            );
            Ok(Popup {
                hwnd,
                state,
                _not_send: PhantomData,
            })
        }
    }

    pub fn is_visible(&self) -> bool {
        self.state.visible
    }

    /// Show an empty search box on the monitor under the cursor and take focus. Clearing
    /// the text reports an empty [`PopupEvent::Query`].
    pub fn show(&mut self) -> Result<()> {
        let (x, y) = crate::input::cursor_pos()?;
        let monitor =
            crate::monitor::from_point(x, y).ok_or_else(|| Error::Other("the cursor is not on a monitor".into()))?;
        let st = &mut *self.state;
        if st.fonts.as_ref().map(|f| f.dpi) != Some(monitor.dpi) {
            st.dpi = monitor.dpi;
            let fonts = Fonts {
                dpi: monitor.dpi,
                edit: make_font(st.scale(EDIT_FONT), FW_NORMAL.0 as i32),
                title: make_font(st.scale(TITLE_FONT), FW_SEMIBOLD.0 as i32),
                sub: make_font(st.scale(SUB_FONT), FW_NORMAL.0 as i32),
            };
            // SAFETY: own child window; the old font is deleted only after the edit
            // switched to the new one.
            unsafe {
                SendMessageW(
                    st.edit,
                    WM_SETFONT,
                    Some(WPARAM(fonts.edit.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            if let Some(old) = st.fonts.replace(fonts) {
                old.delete();
            }
        }
        let work = monitor.work_area;
        let w = st.scale(WIDTH).min(work.right - work.left);
        let left = work.left + (work.right - work.left - w) / 2;
        let top = work.top + (work.bottom - work.top) / 5;
        let pad = st.scale(PAD);
        let edit_h = st.scale(EDIT_FONT) * 3 / 2;
        let w_edit = wide("");
        st.rows.clear();
        st.selected = 0;
        st.visible = true;
        // SAFETY: own windows; `w_edit` outlives the call.
        unsafe {
            SetWindowPos(
                st.edit,
                None,
                pad,
                (st.scale(HEADER) - edit_h) / 2,
                w - 2 * pad,
                edit_h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .context("SetWindowPos(launcher search box)")?;
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                left,
                top,
                w,
                height(0, |p| st.scale(p)),
                SWP_SHOWWINDOW,
            )
            .context("SetWindowPos(launcher)")?;
            let _ = SetWindowTextW(st.edit, pcwstr(&w_edit));
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            if !SetForegroundWindow(self.hwnd).as_bool() {
                tracing::debug!("Windows refused to bring the launcher to the foreground");
            }
            let _ = SetFocus(Some(st.edit));
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        hide_window(self.hwnd, &mut self.state);
    }

    /// Replace the result rows, select the first and resize to fit.
    pub fn set_rows(&mut self, rows: Vec<Row>) -> Result<()> {
        let st = &mut *self.state;
        st.rows = rows;
        st.selected = 0;
        if !st.visible {
            return Ok(());
        }
        let h = height(st.rows.len(), |p| st.scale(p));
        let mut r = RECT::default();
        // SAFETY: own window.
        unsafe {
            GetWindowRect(self.hwnd, &mut r).context("GetWindowRect(launcher)")?;
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                r.right - r.left,
                h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .context("SetWindowPos(launcher)")?;
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        Ok(())
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        // SAFETY: windows and GDI objects were created by this object on this thread.
        // The state pointer is cleared before the windows go so no callback sees freed
        // memory.
        unsafe {
            let _ = RemoveWindowSubclass(self.state.edit, Some(edit_proc), SUBCLASS_ID);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.state.bg_brush.into());
        }
        if let Some(f) = self.state.fonts.take() {
            f.delete();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scale(p: u32) -> i32 {
        p as i32 * 3 / 2
    }

    #[test]
    fn height_grows_with_rows() {
        assert_eq!(height(0, scale), 90);
        assert_eq!(height(2, scale), 90 + 156 + 12);
    }

    #[test]
    fn rows_are_hit_tested_below_the_header() {
        assert_eq!(row_at(10, 3, scale), None);
        assert_eq!(row_at(92, 3, scale), None);
        assert_eq!(row_at(96, 3, scale), Some(0));
        assert_eq!(row_at(96 + 78, 3, scale), Some(1));
        assert_eq!(row_at(96 + 78 * 3, 3, scale), None);
    }

    #[test]
    fn selection_wraps() {
        assert_eq!(step(0, -1, 3), 2);
        assert_eq!(step(2, 1, 3), 0);
        assert_eq!(step(0, 1, 0), 0);
    }
}
