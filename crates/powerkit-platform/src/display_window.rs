//! The Power Display window: a section per monitor holding sliders and drop-down
//! lists, a Refresh button and a status line, anchored to the bottom-right corner of
//! the work area like a tray flyout.
//!
//! The window only shows what its owner gives it and reports what the user does as
//! [`Event::PowerDisplay`]s. Choices marked with a warning ask before they are
//! reported, and go back to what they showed when the user says no. Closing hides it.

use crate::event_loop::push;
use crate::monitor;
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Hwnd, Result};
use powerkit_core::{Event, PowerDisplayEvent};
use std::marker::PhantomData;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.PowerDisplay";
const TITLE: &str = "Power Display";
const ID_REFRESH: i32 = 1;
const ID_STATUS: i32 = 2;
/// Dynamic child `i` has the id `FIRST_CHILD + i`.
const FIRST_CHILD: i32 = 100;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const SS_RIGHT: u32 = 0x2;
const TBM_GETPOS: u32 = 0x400;
const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_CLIPCHILDREN.0);
const EX_STYLE: WINDOW_EX_STYLE = WS_EX_TOOLWINDOW;

// Sizes in 96-DPI pixels.
const WIDTH: i32 = 400;
const PAD: i32 = 14;
const LINE: i32 = 26;
const GAP: i32 = 6;
const LABEL_W: i32 = 92;
const VALUE_W: i32 = 40;
const BUTTON_W: i32 = 90;
const FONT_PT: i32 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Slider {
        label: String,
        value: u32,
        max: u32,
    },
    Choice {
        label: String,
        options: Vec<String>,
        selected: Option<usize>,
        /// When set, choosing asks first and shows this under the question.
        warning: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub controls: Vec<Control>,
}

enum Role {
    Title,
    Label,
    Slider { section: usize, row: usize, value: HWND },
    Value,
    Choice { section: usize, row: usize },
}

struct Child {
    hwnd: HWND,
    role: Role,
}

struct State {
    dpi: u32,
    font: HFONT,
    bold: HFONT,
    sections: Vec<Section>,
    children: Vec<Child>,
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// # Safety
/// `hwnd` must be a Power Display window whose state pointer has not been cleared.
unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    // SAFETY: the pointer is set in `new` and cleared in `Drop` before the box is freed.
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

fn item(parent: HWND, id: i32) -> HWND {
    // SAFETY: accepts any handle; returns null when there is no such child.
    unsafe { GetDlgItem(Some(parent), id) }.unwrap_or_default()
}

fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: only used with messages whose pointer arguments outlive the call.
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

fn set_text(hwnd: HWND, text: &str) {
    let t = wide(text);
    // SAFETY: `t` outlives the call.
    let _ = unsafe { SetWindowTextW(hwnd, pcwstr(&t)) };
}

fn make_font(dpi: u32, weight: FONT_WEIGHT) -> HFONT {
    let face = wide("Segoe UI");
    // SAFETY: `face` outlives the call.
    unsafe {
        CreateFontW(
            -((FONT_PT * dpi as i32 + 36) / 72),
            0,
            0,
            0,
            weight.0 as i32,
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

fn create(parent: HWND, class: &str, text: &str, style: u32, id: i32, r: (i32, i32, i32, i32)) -> Result<HWND> {
    let (class, text) = (wide(class), wide(text));
    // SAFETY: strings outlive the call; `parent` is our own window.
    unsafe {
        let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            pcwstr(&class),
            pcwstr(&text),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
            r.0,
            r.1,
            r.2,
            r.3,
            Some(parent),
            Some(HMENU(id as isize as *mut _)),
            Some(instance.into()),
            None,
        )
        .context("CreateWindowEx(power display control)")
    }
}

/// Recreates the controls for `st.sections` at `st.dpi`, then fits and anchors the window.
fn rebuild(hwnd: HWND, st: &mut State) -> Result<()> {
    for c in st.children.drain(..) {
        // SAFETY: our own child window.
        let _ = unsafe { DestroyWindow(c.hwnd) };
    }
    let s = |v| scale(v, st.dpi);
    let w = s(WIDTH);
    let label = SS_NOPREFIX | SS_CENTERIMAGE;
    let mut y = s(PAD);
    let mut children = Vec::new();
    let mut add = |class: &str, text: &str, style: u32, r, role| -> Result<HWND> {
        let h = create(hwnd, class, text, style, FIRST_CHILD + children.len() as i32, r)?;
        children.push(Child { hwnd: h, role });
        Ok(h)
    };
    for (si, sec) in st.sections.iter().enumerate() {
        add(
            "STATIC",
            &sec.title,
            label,
            (s(PAD), y, w - s(PAD) * 2, s(LINE)),
            Role::Title,
        )?;
        y += s(LINE) + s(GAP) / 2;
        let x = s(PAD) + s(LABEL_W) + s(GAP);
        for (row, c) in sec.controls.iter().enumerate() {
            match c {
                Control::Slider { label: l, value, max } => {
                    add("STATIC", l, label, (s(PAD), y, s(LABEL_W), s(LINE)), Role::Label)?;
                    let vx = w - s(PAD) - s(VALUE_W);
                    let value_label = add(
                        "STATIC",
                        &value.to_string(),
                        label | SS_RIGHT,
                        (vx, y, s(VALUE_W), s(LINE)),
                        Role::Value,
                    )?;
                    let style = WS_TABSTOP.0 | TBS_HORZ | TBS_NOTICKS;
                    let bar = add(
                        "msctls_trackbar32",
                        "",
                        style,
                        (x, y, vx - s(GAP) - x, s(LINE)),
                        Role::Slider {
                            section: si,
                            row,
                            value: value_label,
                        },
                    )?;
                    send(bar, TBM_SETRANGEMIN, 0, 0);
                    send(bar, TBM_SETRANGEMAX, 0, (*max).max(1) as isize);
                    send(bar, TBM_SETPAGESIZE, 0, ((*max / 10).max(1)) as isize);
                    send(bar, TBM_SETPOS, 1, (*value).min(*max) as isize);
                }
                Control::Choice {
                    label: l,
                    options,
                    selected,
                    ..
                } => {
                    add("STATIC", l, label, (s(PAD), y, s(LABEL_W), s(LINE)), Role::Label)?;
                    let style = WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32;
                    // A drop-down list's height includes its opened list.
                    let combo = add(
                        "COMBOBOX",
                        "",
                        style,
                        (x, y, w - s(PAD) - x, s(LINE) * 10),
                        Role::Choice { section: si, row },
                    )?;
                    for o in options {
                        let t = wide(o);
                        send(combo, CB_ADDSTRING, 0, t.as_ptr() as isize);
                    }
                    send(combo, CB_SETCURSEL, selected.unwrap_or(usize::MAX), 0);
                }
            }
            y += s(LINE) + s(GAP);
        }
        y += s(GAP) * 2;
    }
    for c in &children {
        let font = if matches!(c.role, Role::Title) {
            st.bold
        } else {
            st.font
        };
        send(c.hwnd, WM_SETFONT, font.0 as usize, 1);
    }
    st.children = children;
    let place = |id: i32, x: i32, cx: i32, cy: i32| {
        // SAFETY: own child window.
        let _ = unsafe { MoveWindow(item(hwnd, id), x, y, cx, cy, true) };
    };
    place(ID_REFRESH, s(PAD), s(BUTTON_W), s(LINE));
    let sx = s(PAD) + s(BUTTON_W) + s(GAP) * 2;
    place(ID_STATUS, sx, w - s(PAD) - sx, s(LINE) * 2);
    for id in [ID_REFRESH, ID_STATUS] {
        send(item(hwnd, id), WM_SETFONT, st.font.0 as usize, 1);
    }
    fit(hwnd, w, y + s(LINE) * 2 + s(PAD), st.dpi)
}

/// Sizes the window to a `w` x `h` client area in the bottom-right of its monitor's work area.
fn fit(hwnd: HWND, w: i32, h: i32, dpi: u32) -> Result<()> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    // SAFETY: valid in/out pointer.
    unsafe { AdjustWindowRectExForDpi(&mut r, STYLE, false, EX_STYLE, dpi) }.context("AdjustWindowRectExForDpi")?;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let work = monitor::from_window(Hwnd::from_raw(hwnd))
        .map(|m| m.work_area)
        .ok_or_else(|| Error::Other("no monitor for the Power Display window".into()))?;
    let margin = scale(12, dpi);
    let x = (work.right - w - margin).max(work.left);
    let y = (work.bottom - h - margin).max(work.top);
    // SAFETY: own window.
    unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) }
        .context("SetWindowPos(power display)")
}

/// Asks before applying a choice; true when the user agrees.
fn confirm(hwnd: HWND, section: &str, label: &str, option: &str, warning: &str) -> bool {
    let text = wide(&format!("Set {label} on {section} to {option}?\n\n{warning}"));
    let title = wide(TITLE);
    // SAFETY: strings outlive the modal call; `hwnd` is our window.
    let answer = unsafe {
        MessageBoxW(
            Some(hwnd),
            pcwstr(&text),
            pcwstr(&title),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    answer == IDYES
}

fn on_choice(hwnd: HWND, combo: HWND, section: usize, row: usize) {
    let Ok(index) = usize::try_from(send(combo, CB_GETCURSEL, 0, 0)) else {
        return;
    };
    // SAFETY: called from this window's procedure; the borrow ends before the modal
    // message box below can re-enter it.
    let Some(st) = (unsafe { state(hwnd) }) else { return };
    let Some(sec) = st.sections.get(section) else { return };
    let Some(Control::Choice {
        label,
        options,
        selected,
        warning,
    }) = sec.controls.get(row)
    else {
        return;
    };
    let (title, label, option, previous, warning) = (
        sec.title.clone(),
        label.clone(),
        options.get(index).cloned().unwrap_or_default(),
        *selected,
        warning.clone(),
    );
    if let Some(warning) = warning {
        if !confirm(hwnd, &title, &label, &option, &warning) {
            send(combo, CB_SETCURSEL, previous.unwrap_or(usize::MAX), 0);
            return;
        }
    }
    // SAFETY: as above; the message box has returned.
    if let Some(Control::Choice { selected, .. }) =
        unsafe { state(hwnd) }.and_then(|st| st.sections.get_mut(section)?.controls.get_mut(row))
    {
        *selected = Some(index);
    }
    push(Event::PowerDisplay(PowerDisplayEvent::Choice { section, row, index }));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HSCROLL => {
            let bar = HWND(lparam.0 as *mut _);
            // SAFETY: this window's procedure.
            let Some(st) = (unsafe { state(hwnd) }) else {
                return LRESULT(0);
            };
            let found = st.children.iter().find_map(|c| match c.role {
                Role::Slider { section, row, value } if c.hwnd == bar => Some((section, row, value)),
                _ => None,
            });
            if let Some((section, row, value_label)) = found {
                let pos = send(bar, TBM_GETPOS, 0, 0).max(0) as u32;
                if let Some(Control::Slider { value, .. }) =
                    st.sections.get_mut(section).and_then(|s| s.controls.get_mut(row))
                {
                    if *value != pos {
                        *value = pos;
                        set_text(value_label, &pos.to_string());
                        push(Event::PowerDisplay(PowerDisplayEvent::Slider {
                            section,
                            row,
                            value: pos,
                        }));
                    }
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let (id, code) = ((wparam.0 & 0xFFFF) as i32, ((wparam.0 >> 16) & 0xFFFF) as u32);
            if id == ID_REFRESH && code == BN_CLICKED {
                push(Event::PowerDisplay(PowerDisplayEvent::Refresh));
            } else if code == CBN_SELCHANGE {
                let combo = HWND(lparam.0 as *mut _);
                // SAFETY: this window's procedure.
                let found = unsafe { state(hwnd) }.and_then(|st| {
                    st.children.iter().find_map(|c| match c.role {
                        Role::Choice { section, row } if c.hwnd == combo => Some((section, row)),
                        _ => None,
                    })
                });
                if let Some((section, row)) = found {
                    on_choice(hwnd, combo, section, row);
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // SAFETY: this window's procedure.
            if let Some(st) = unsafe { state(hwnd) } {
                let dpi = (wparam.0 & 0xFFFF) as u32;
                let (font, bold) = (st.font, st.bold);
                st.dpi = dpi;
                st.font = make_font(dpi, FW_NORMAL);
                st.bold = make_font(dpi, FW_SEMIBOLD);
                if let Err(e) = rebuild(hwnd, st) {
                    tracing::warn!("Power Display: {e}");
                }
                // SAFETY: the controls using the old fonts were destroyed or switched.
                unsafe {
                    let _ = DeleteObject(font.into());
                    let _ = DeleteObject(bold.into());
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            push(Event::PowerDisplay(PowerDisplayEvent::Closed));
            LRESULT(0)
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub struct PowerDisplayWindow {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl PowerDisplayWindow {
    /// Creates the window hidden; [`PowerDisplayWindow::show`] brings it up.
    pub fn new() -> Result<PowerDisplayWindow> {
        let class = wide(CLASS_NAME);
        let title = wide(TITLE);
        // SAFETY: strings outlive the calls; `wndproc` matches its callback contract.
        // The state pointer stays valid until `Drop` clears it.
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            };
            if !InitCommonControlsEx(&icc).as_bool() {
                return Err(Error::Other("InitCommonControlsEx failed".into()));
            }
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
                ..Default::default()
            };
            RegisterClassW(&wc); // "already registered" is expected after the first one
            let dpi = GetDpiForSystem().max(96);
            let hwnd = CreateWindowExW(
                EX_STYLE,
                pcwstr(&class),
                pcwstr(&title),
                STYLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                scale(WIDTH, dpi),
                scale(200, dpi),
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(power display)")?;
            let created = (|| -> Result<()> {
                let button = WS_TABSTOP.0 | BS_PUSHBUTTON as u32;
                create(hwnd, "BUTTON", "Refresh", button, ID_REFRESH, (0, 0, 0, 0))?;
                create(hwnd, "STATIC", "", SS_NOPREFIX, ID_STATUS, (0, 0, 0, 0))?;
                Ok(())
            })();
            if let Err(e) = created {
                let _ = DestroyWindow(hwnd);
                return Err(e);
            }
            let dpi = GetDpiForWindow(hwnd).max(96);
            let mut state = Box::new(State {
                dpi,
                font: make_font(dpi, FW_NORMAL),
                bold: make_font(dpi, FW_SEMIBOLD),
                sections: Vec::new(),
                children: Vec::new(),
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            let mut w = PowerDisplayWindow {
                hwnd,
                state,
                _not_send: PhantomData,
            };
            rebuild(hwnd, &mut w.state)?;
            Ok(w)
        }
    }

    pub fn show(&mut self) {
        if let Err(e) = rebuild(self.hwnd, &mut self.state) {
            tracing::warn!("Power Display: {e}");
        }
        // SAFETY: own window. Return values are the previous state, not errors.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn hide(&mut self) {
        // SAFETY: own window. The return value is the previous visibility.
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: own window.
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Show these sections in place of the current ones.
    pub fn set_sections(&mut self, sections: Vec<Section>) -> Result<()> {
        if self.state.sections == sections {
            return Ok(());
        }
        self.state.sections = sections;
        rebuild(self.hwnd, &mut self.state)
    }

    pub fn set_status(&mut self, text: &str) {
        set_text(item(self.hwnd, ID_STATUS), text);
    }

    /// Show `ico` in the title bar.
    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }
}

impl Drop for PowerDisplayWindow {
    fn drop(&mut self) {
        // SAFETY: own window, on the thread that created it; the state pointer is
        // cleared before the box is freed.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.state.font.into());
            let _ = DeleteObject(self.state.bold.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(hwnd: HWND) -> String {
        let mut buf = [0u16; 64];
        // SAFETY: valid buffer.
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..n as usize])
    }

    #[test]
    fn sections_become_controls() {
        let mut w = PowerDisplayWindow::new().unwrap();
        let sections = vec![Section {
            title: "Monitor".into(),
            controls: vec![
                Control::Slider {
                    label: "Brightness".into(),
                    value: 40,
                    max: 100,
                },
                Control::Choice {
                    label: "Input".into(),
                    options: vec!["HDMI 1".into(), "DisplayPort 1".into()],
                    selected: Some(1),
                    warning: Some("careful".into()),
                },
            ],
        }];
        w.set_sections(sections).unwrap();
        let roles: Vec<_> = w
            .state
            .children
            .iter()
            .map(|c| match c.role {
                Role::Title => "title",
                Role::Label => "label",
                Role::Slider { .. } => "slider",
                Role::Value => "value",
                Role::Choice { .. } => "choice",
            })
            .collect();
        assert_eq!(roles, ["title", "label", "value", "slider", "label", "choice"]);
        let c = &w.state.children;
        assert_eq!(send(c[3].hwnd, TBM_GETPOS, 0, 0), 40);
        assert_eq!(text_of(c[2].hwnd), "40");
        assert_eq!(send(c[5].hwnd, CB_GETCOUNT, 0, 0), 2);
        assert_eq!(send(c[5].hwnd, CB_GETCURSEL, 0, 0), 1);
        w.set_sections(Vec::new()).unwrap();
        assert!(w.state.children.is_empty(), "old controls are destroyed");
    }
}
