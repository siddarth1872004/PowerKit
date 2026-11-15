//! The Registry Preview window: a .reg file's text on the left, the keys and values
//! it would write on the right, and Open / Save / Save as / Write to registry /
//! Open key buttons above. Selecting a row puts the caret on its line.
//!
//! The window only shows what its owner gives it and reports what the user does as
//! [`RegistryEvent`]s on the event loop; edits are reported once typing pauses.
//! Closing it hides it.

use crate::dialog::{self, FileKinds};
use crate::event_loop::push;
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::{Event, RegistryEvent};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use windows::core::PWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.RegistryPreview";
const ID_OPEN: i32 = 1;
const ID_SAVE: i32 = 2;
const ID_SAVE_AS: i32 = 3;
const ID_WRITE: i32 = 4;
const ID_REGEDIT: i32 = 5;
const ID_TEXT: i32 = 6;
const ID_LIST: i32 = 7;
const ID_STATUS: i32 = 8;
const BUTTONS: [(i32, &str, i32); 5] = [
    (ID_OPEN, "Open…", 90),
    (ID_SAVE, "Save", 90),
    (ID_SAVE_AS, "Save as…", 90),
    (ID_WRITE, "Write to registry", 140),
    (ID_REGEDIT, "Open key in Registry Editor", 210),
];
const COLUMNS: [(&str, i32); 3] = [("Key or value", 300), ("Type", 120), ("Data", 320)];
/// Fires once typing has paused this long, so large files are not re-read per key.
const EDIT_TIMER: usize = 1;
const EDIT_DELAY_MS: u32 = 300;
// Static control styles; their windows-crate constants sit behind a feature this crate
// does not otherwise need.
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;

// Sizes in 96-DPI pixels.
const WIDTH: i32 = 1200;
const HEIGHT: i32 = 720;
const PAD: i32 = 12;
const LINE: i32 = 28;
const FONT_PT: i32 = 10;

const FILE_KINDS: FileKinds = FileKinds {
    filters: &[("Registry files (*.reg)", "*.reg"), ("All files (*.*)", "*.*")],
    extension: "reg",
};

struct State {
    dpi: u32,
    font: HFONT,
    mono: HFONT,
    /// The text line (0-based) each listed row came from.
    lines: Vec<usize>,
    /// Set while the owner changes the text or list, so that is not reported back.
    quiet: bool,
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// # Safety
/// `hwnd` must be a Registry Preview window whose state pointer has not been cleared.
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

fn make_font(face: &str, dpi: u32) -> HFONT {
    let face = wide(face);
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
    let old = std::mem::replace(&mut st.font, make_font("Segoe UI", dpi));
    let old_mono = std::mem::replace(&mut st.mono, make_font("Consolas", dpi));
    st.dpi = dpi;
    for id in ID_OPEN..=ID_STATUS {
        let font = if id == ID_TEXT { st.mono } else { st.font };
        send(item(hwnd, id), WM_SETFONT, font.0 as usize, 1);
    }
    for (i, (_, w)) in COLUMNS.iter().enumerate() {
        send(item(hwnd, ID_LIST), LVM_SETCOLUMNWIDTH, i, scale(*w, dpi) as isize);
    }
    // SAFETY: no control uses the old fonts any more.
    unsafe {
        let _ = DeleteObject(old.into());
        let _ = DeleteObject(old_mono.into());
    }
}

fn layout(hwnd: HWND, dpi: u32) {
    let mut rc = RECT::default();
    // SAFETY: own window; `rc` is a valid out pointer.
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let s = |v| scale(v, dpi);
    let (w, h) = (rc.right, rc.bottom);
    let place = |id: i32, x: i32, y: i32, cx: i32, cy: i32| {
        // SAFETY: own child window.
        let _ = unsafe { MoveWindow(item(hwnd, id), x, y, cx.max(0), cy.max(0), true) };
    };
    let mut x = s(PAD);
    for (id, _, width) in BUTTONS {
        place(id, x, s(PAD), s(width), s(LINE));
        x += s(width) + s(PAD / 2);
    }
    let top = s(PAD) * 2 + s(LINE);
    let status = h - s(PAD) - s(LINE);
    let body = status - s(PAD / 2) - top;
    let half = (w - s(PAD) * 3) / 2;
    place(ID_TEXT, s(PAD), top, half, body);
    place(ID_LIST, s(PAD) * 2 + half, top, w - s(PAD) * 3 - half, body);
    place(ID_STATUS, s(PAD), status, w - s(PAD) * 2, s(LINE));
}

fn text_of(hwnd: HWND) -> String {
    // SAFETY: the buffer is sized from the control's text length.
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(hwnd) as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn set_text(hwnd: HWND, text: &str) {
    let t = wide(text);
    // SAFETY: `t` outlives the call.
    let _ = unsafe { SetWindowTextW(hwnd, pcwstr(&t)) };
}

fn selected_row(hwnd: HWND) -> Option<usize> {
    let i = send(item(hwnd, ID_LIST), LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    usize::try_from(i).ok()
}

/// Select text line `line` and scroll it into view.
fn select_line(hwnd: HWND, line: usize) {
    let edit = item(hwnd, ID_TEXT);
    let start = send(edit, EM_LINEINDEX, line, 0);
    if start < 0 {
        return;
    }
    let len = send(edit, EM_LINELENGTH, start as usize, 0);
    send(edit, EM_SETSEL, start as usize, start + len);
    send(edit, EM_SCROLLCARET, 0, 0);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a Registry Preview window (this is its class's procedure).
    let st = unsafe { state(hwnd) };
    match (msg, st) {
        (WM_SIZE, Some(st)) => {
            layout(hwnd, st.dpi);
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
                // SAFETY: own window; restarting the timer is the debounce.
                unsafe { SetTimer(Some(hwnd), EDIT_TIMER, EDIT_DELAY_MS, None) };
            } else if code == BN_CLICKED {
                let event = match id {
                    ID_OPEN => Some(RegistryEvent::Open),
                    ID_SAVE => Some(RegistryEvent::Save),
                    ID_SAVE_AS => Some(RegistryEvent::SaveAs),
                    ID_WRITE => Some(RegistryEvent::Write),
                    ID_REGEDIT => selected_row(hwnd).map(RegistryEvent::OpenKey),
                    _ => None,
                };
                if let Some(e) = event {
                    push(Event::Registry(e));
                }
            }
            LRESULT(0)
        }
        (WM_TIMER, Some(_)) if wparam.0 == EDIT_TIMER => {
            // SAFETY: own window and timer.
            let _ = unsafe { KillTimer(Some(hwnd), EDIT_TIMER) };
            push(Event::Registry(RegistryEvent::Changed(text_of(item(hwnd, ID_TEXT)))));
            LRESULT(0)
        }
        (WM_NOTIFY, Some(st)) => {
            // SAFETY: WM_NOTIFY always carries an NMHDR.
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom == ID_LIST as usize && hdr.code == LVN_ITEMCHANGED && !st.quiet {
                // SAFETY: LVN_ITEMCHANGED carries an NMLISTVIEW.
                let nm = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
                let newly = nm.uNewState & LVIS_SELECTED.0 != 0 && nm.uOldState & LVIS_SELECTED.0 == 0;
                if newly {
                    let line = usize::try_from(nm.iItem).ok().and_then(|r| st.lines.get(r));
                    if let Some(&line) = line {
                        select_line(hwnd, line);
                    }
                }
            } else if hdr.idFrom == ID_LIST as usize && hdr.code == NM_DBLCLK {
                // SAFETY: NM_DBLCLK from a list view carries an NMITEMACTIVATE.
                let nm = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
                if let Ok(row) = usize::try_from(nm.iItem) {
                    push(Event::Registry(RegistryEvent::OpenKey(row)));
                }
            }
            LRESULT(0)
        }
        (WM_CLOSE, _) => {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        (WM_GETMINMAXINFO, Some(st)) => {
            // SAFETY: WM_GETMINMAXINFO carries a MINMAXINFO to fill in.
            let mm = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mm.ptMinTrackSize.x = scale(WIDTH * 3 / 4, st.dpi);
            mm.ptMinTrackSize.y = scale(HEIGHT / 2, st.dpi);
            LRESULT(0)
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// One listed key or value: its columns and the text line (0-based) it is on.
pub struct RegistryRow {
    pub cols: [String; 3],
    pub line: usize,
}

pub struct RegistryWindow {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl RegistryWindow {
    /// Creates the window hidden; [`RegistryWindow::show`] brings it up.
    pub fn new() -> Result<RegistryWindow> {
        let class = wide(CLASS_NAME);
        let title = wide("Registry Preview");
        // SAFETY: strings outlive the calls; `wndproc` matches its callback contract.
        // The state pointer stays valid until `Drop` clears it.
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                // Only the list view needs registering; the rest are user32 classes.
                dwICC: ICC_LISTVIEW_CLASSES,
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
                hIcon: LoadIconW(
                    Some(instance.into()),
                    windows::core::PCWSTR(std::ptr::without_provenance(1)),
                )
                .unwrap_or_default(),
                hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
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
            .context("CreateWindowEx(registry preview)")?;
            let child = |class: &str, text: &str, style: u32, ex: WINDOW_EX_STYLE, id: i32| {
                let (class, text) = (wide(class), wide(text));
                CreateWindowExW(
                    ex,
                    pcwstr(&class),
                    pcwstr(&text),
                    WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                    0,
                    0,
                    0,
                    0,
                    Some(hwnd),
                    Some(HMENU(id as isize as *mut _)),
                    Some(instance.into()),
                    None,
                )
                .context("CreateWindowEx(registry preview control)")
            };
            let created = (|| -> Result<()> {
                let button = WS_TABSTOP.0 | BS_PUSHBUTTON as u32;
                let none = WINDOW_EX_STYLE(0);
                for (id, text, _) in BUTTONS {
                    child("BUTTON", text, button, none, id)?;
                }
                let edit = WS_TABSTOP.0
                    | WS_VSCROLL.0
                    | WS_HSCROLL.0
                    | (ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32;
                let text = child("EDIT", "", edit, WS_EX_CLIENTEDGE, ID_TEXT)?;
                // Lift the 32K default; exported registry branches run to megabytes.
                send(text, EM_SETLIMITTEXT, 0, 0);
                let list = child(
                    "SysListView32",
                    "",
                    WS_TABSTOP.0 | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER,
                    WS_EX_CLIENTEDGE,
                    ID_LIST,
                )?;
                send(
                    list,
                    LVM_SETEXTENDEDLISTVIEWSTYLE,
                    0,
                    (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
                );
                for (i, (name, w)) in COLUMNS.into_iter().enumerate() {
                    let mut text = wide(name);
                    let col = LVCOLUMNW {
                        mask: LVCF_TEXT | LVCF_WIDTH,
                        cx: scale(w, dpi),
                        pszText: PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    send(list, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
                }
                child("STATIC", "", SS_NOPREFIX | SS_CENTERIMAGE, none, ID_STATUS)?;
                Ok(())
            })();
            if let Err(e) = created {
                let _ = DestroyWindow(hwnd);
                return Err(e);
            }
            let mut state = Box::new(State {
                dpi,
                font: make_font("Segoe UI", dpi),
                mono: make_font("Consolas", dpi),
                lines: Vec::new(),
                quiet: false,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            let dpi = GetDpiForWindow(hwnd).max(96);
            apply_font(hwnd, &mut state, dpi);
            layout(hwnd, dpi);
            Ok(RegistryWindow {
                hwnd,
                state,
                _not_send: PhantomData,
            })
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

    pub fn is_visible(&self) -> bool {
        // SAFETY: own window.
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Replace the text (lines end in CR LF) without reporting it as an edit.
    pub fn set_text(&mut self, text: &str) {
        self.state.quiet = true;
        set_text(item(self.hwnd, ID_TEXT), text);
        self.state.quiet = false;
        // SAFETY: own window and timer; an edit made just before is superseded.
        let _ = unsafe { KillTimer(Some(self.hwnd), EDIT_TIMER) };
    }

    pub fn text(&self) -> String {
        text_of(item(self.hwnd, ID_TEXT))
    }

    pub fn set_rows(&mut self, rows: &[RegistryRow]) {
        let list = item(self.hwnd, ID_LIST);
        self.state.quiet = true;
        send(list, WM_SETREDRAW, 0, 0);
        send(list, LVM_DELETEALLITEMS, 0, 0);
        for (i, row) in rows.iter().enumerate() {
            for (col, text) in row.cols.iter().enumerate() {
                let mut t = wide(text);
                let it = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: i as i32,
                    iSubItem: col as i32,
                    pszText: PWSTR(t.as_mut_ptr()),
                    ..Default::default()
                };
                let msg = if col == 0 { LVM_INSERTITEMW } else { LVM_SETITEMTEXTW };
                send(list, msg, if col == 0 { 0 } else { i }, &it as *const _ as isize);
            }
        }
        send(list, WM_SETREDRAW, 1, 0);
        self.state.lines = rows.iter().map(|r| r.line).collect();
        self.state.quiet = false;
    }

    pub fn set_status(&mut self, text: &str) {
        set_text(item(self.hwnd, ID_STATUS), text);
    }

    pub fn set_title(&mut self, text: &str) {
        set_text(self.hwnd, text);
    }

    /// Ask which .reg file to open; `None` when the user cancelled.
    pub fn pick_open(&self) -> Result<Option<PathBuf>> {
        dialog::pick(self.hwnd, false, &FILE_KINDS, None)
    }

    /// Ask where to save, suggesting `name`; `None` when the user cancelled.
    pub fn pick_save(&self, name: Option<&Path>) -> Result<Option<PathBuf>> {
        dialog::pick(self.hwnd, true, &FILE_KINDS, name)
    }
}

impl Drop for RegistryWindow {
    fn drop(&mut self) {
        // SAFETY: own window, on the thread that created it; the state pointer is
        // cleared before the box is freed.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.state.font.into());
            let _ = DeleteObject(self.state.mono.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_point_at_their_lines() {
        let mut w = RegistryWindow::new().unwrap();
        w.set_text("REGEDIT4\r\n\r\n[HKEY_CURRENT_USER\\X]\r\n\"a\"=\"b\"\r\n");
        assert!(w.text().starts_with("REGEDIT4\r\n"));
        let row = |name: &str, line| RegistryRow {
            cols: [name.into(), String::new(), String::new()],
            line,
        };
        w.set_rows(&[row("HKEY_CURRENT_USER\\X", 2), row("a", 3)]);
        assert_eq!(send(item(w.hwnd, ID_LIST), LVM_GETITEMCOUNT, 0, 0), 2);
        select_line(w.hwnd, 3);
        let (mut start, mut end) = (0u32, 0u32);
        send(
            item(w.hwnd, ID_TEXT),
            EM_GETSEL,
            &mut start as *mut u32 as usize,
            &mut end as *mut u32 as isize,
        );
        assert_eq!((start, end), (35, 42), "line 3 is selected");
    }
}
