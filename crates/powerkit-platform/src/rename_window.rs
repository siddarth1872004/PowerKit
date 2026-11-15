//! The PowerRename window: search and replace boxes, options, a preview of every
//! item's old and new name, and Rename / Undo buttons.
//!
//! Like the settings window it only shows what its owner gives it and reports what the
//! user does as [`RenameEvent`]s on the event loop. Closing it hides it.

use crate::event_loop::push;
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::{Event, RenameEvent, RenameForm};
use std::marker::PhantomData;
use windows::core::PWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Rename";
const ID_SEARCH_LABEL: i32 = 1;
const ID_SEARCH: i32 = 2;
const ID_REPLACE_LABEL: i32 = 3;
const ID_REPLACE: i32 = 4;
const ID_REGEX: i32 = 5;
const ID_MATCH_CASE: i32 = 6;
const ID_ALL: i32 = 7;
const ID_ENUMERATE: i32 = 8;
const ID_TARGET_LABEL: i32 = 9;
const ID_TARGET: i32 = 10;
const ID_CASE_LABEL: i32 = 11;
const ID_CASE: i32 = 12;
const ID_FILES: i32 = 13;
const ID_FOLDERS: i32 = 14;
const ID_ITEMS: i32 = 15;
const ID_STATUS: i32 = 16;
const ID_UNDO: i32 = 17;
const ID_RENAME: i32 = 18;
const ALL_IDS: [i32; 18] = [
    ID_SEARCH_LABEL,
    ID_SEARCH,
    ID_REPLACE_LABEL,
    ID_REPLACE,
    ID_REGEX,
    ID_MATCH_CASE,
    ID_ALL,
    ID_ENUMERATE,
    ID_TARGET_LABEL,
    ID_TARGET,
    ID_CASE_LABEL,
    ID_CASE,
    ID_FILES,
    ID_FOLDERS,
    ID_ITEMS,
    ID_STATUS,
    ID_UNDO,
    ID_RENAME,
];
const CHECKBOXES: [(i32, &str); 6] = [
    (ID_REGEX, "Use regular expressions"),
    (ID_MATCH_CASE, "Match case"),
    (ID_ALL, "Match all occurrences"),
    (ID_ENUMERATE, "Enumerate items"),
    (ID_FILES, "Include files"),
    (ID_FOLDERS, "Include folders"),
];
const TARGETS: [&str; 3] = ["Name only", "Extension only", "Name and extension"];
const CASES: [&str; 5] = ["Unchanged", "lowercase", "UPPERCASE", "Title Case", "Sentence case"];
const COLUMNS: [(&str, i32); 3] = [("Original", 300), ("Renamed", 300), ("Status", 200)];
// Static control style; its windows-crate constant sits behind a feature this crate
// does not otherwise need.
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;

// Sizes in 96-DPI pixels.
const WIDTH: i32 = 900;
const HEIGHT: i32 = 640;
const PAD: i32 = 12;
const LINE: i32 = 26;
const LABEL_W: i32 = 100;
const CHECK_W: i32 = 190;
const COMBO_W: i32 = 170;
const BUTTON_W: i32 = 90;
const FONT_PT: i32 = 10;

struct State {
    dpi: u32,
    font: HFONT,
    /// Set while the owner fills in controls, so that does not report changes back.
    quiet: bool,
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// # Safety
/// `hwnd` must be a rename window whose state pointer has not been cleared.
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
    st.dpi = dpi;
    for id in ALL_IDS {
        send(item(hwnd, id), WM_SETFONT, st.font.0 as usize, 1);
    }
    for (i, (_, w)) in COLUMNS.iter().enumerate() {
        send(item(hwnd, ID_ITEMS), LVM_SETCOLUMNWIDTH, i, scale(*w, dpi) as isize);
    }
    // SAFETY: no control uses the old font any more.
    let _ = unsafe { DeleteObject(old.into()) };
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
    let row = |n: i32| s(PAD) + n * (s(LINE) + s(PAD / 2));
    let field_x = s(PAD) + s(LABEL_W);
    let field_w = w - field_x - s(PAD);
    place(ID_SEARCH_LABEL, s(PAD), row(0), s(LABEL_W), s(LINE));
    place(ID_SEARCH, field_x, row(0), field_w, s(LINE));
    place(ID_REPLACE_LABEL, s(PAD), row(1), s(LABEL_W), s(LINE));
    place(ID_REPLACE, field_x, row(1), field_w, s(LINE));
    for (i, id) in [ID_REGEX, ID_MATCH_CASE, ID_ALL, ID_ENUMERATE].into_iter().enumerate() {
        place(id, field_x + i as i32 * s(CHECK_W), row(2), s(CHECK_W), s(LINE));
    }
    // Combo boxes are sized with their drop-down list included.
    let combo_h = s(LINE) * 7;
    place(ID_TARGET_LABEL, s(PAD), row(3), s(LABEL_W), s(LINE));
    place(ID_TARGET, field_x, row(3), s(COMBO_W), combo_h);
    let x = field_x + s(COMBO_W) + s(PAD);
    place(ID_CASE_LABEL, x, row(3), s(LABEL_W), s(LINE));
    place(ID_CASE, x + s(LABEL_W), row(3), s(COMBO_W), combo_h);
    let x = x + s(LABEL_W) + s(COMBO_W) + s(PAD);
    place(ID_FILES, x, row(3), s(CHECK_W * 2 / 3), s(LINE));
    place(ID_FOLDERS, x + s(CHECK_W * 2 / 3), row(3), s(CHECK_W * 2 / 3), s(LINE));
    let bottom = h - s(PAD) - s(LINE);
    let top = row(4) + s(PAD / 2);
    place(ID_ITEMS, s(PAD), top, w - s(PAD) * 2, bottom - s(PAD) - top);
    let buttons = s(BUTTON_W) * 2 + s(PAD);
    place(ID_STATUS, s(PAD), bottom, w - s(PAD) * 3 - buttons, s(LINE));
    place(ID_UNDO, w - s(PAD) - buttons, bottom, s(BUTTON_W), s(LINE));
    place(ID_RENAME, w - s(PAD) - s(BUTTON_W), bottom, s(BUTTON_W), s(LINE));
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

fn checked(hwnd: HWND, id: i32) -> bool {
    send(item(hwnd, id), BM_GETCHECK, 0, 0) == BST_CHECKED.0 as isize
}

fn set_checked(hwnd: HWND, id: i32, on: bool) {
    let v = if on { BST_CHECKED } else { BST_UNCHECKED };
    send(item(hwnd, id), BM_SETCHECK, v.0 as usize, 0);
}

fn choice(hwnd: HWND, id: i32) -> usize {
    usize::try_from(send(item(hwnd, id), CB_GETCURSEL, 0, 0)).unwrap_or(0)
}

fn read_form(hwnd: HWND) -> RenameForm {
    RenameForm {
        search: text_of(item(hwnd, ID_SEARCH)),
        replace: text_of(item(hwnd, ID_REPLACE)),
        regex: checked(hwnd, ID_REGEX),
        match_case: checked(hwnd, ID_MATCH_CASE),
        all_occurrences: checked(hwnd, ID_ALL),
        target: choice(hwnd, ID_TARGET),
        files: checked(hwnd, ID_FILES),
        folders: checked(hwnd, ID_FOLDERS),
        case: choice(hwnd, ID_CASE),
        enumerate: checked(hwnd, ID_ENUMERATE),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a rename window (this is its class's procedure).
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
            let changed = match id {
                ID_SEARCH | ID_REPLACE => code == EN_CHANGE,
                ID_TARGET | ID_CASE => code == CBN_SELCHANGE,
                ID_REGEX | ID_MATCH_CASE | ID_ALL | ID_ENUMERATE | ID_FILES | ID_FOLDERS => code == BN_CLICKED,
                _ => false,
            };
            if changed && !st.quiet {
                push(Event::Rename(RenameEvent::Changed(read_form(hwnd))));
            } else if id == ID_RENAME && code == BN_CLICKED {
                push(Event::Rename(RenameEvent::Apply));
            } else if id == ID_UNDO && code == BN_CLICKED {
                push(Event::Rename(RenameEvent::Undo));
            }
            LRESULT(0)
        }
        (WM_CLOSE, _) => {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            push(Event::Rename(RenameEvent::Closed));
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

pub struct RenameWindow {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl RenameWindow {
    /// Creates the window hidden, its options filled in from `form`.
    pub fn new(form: &RenameForm) -> Result<RenameWindow> {
        let class = wide(CLASS_NAME);
        let title = wide("PowerRename");
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
            .context("CreateWindowEx(rename)")?;
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
                .context("CreateWindowEx(rename control)")
            };
            let created = (|| -> Result<()> {
                let label = SS_NOPREFIX | SS_CENTERIMAGE;
                let edit = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
                let combo = WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32;
                let none = WINDOW_EX_STYLE(0);
                child("STATIC", "Search for", label, none, ID_SEARCH_LABEL)?;
                child("EDIT", "", edit, WS_EX_CLIENTEDGE, ID_SEARCH)?;
                child("STATIC", "Replace with", label, none, ID_REPLACE_LABEL)?;
                child("EDIT", "", edit, WS_EX_CLIENTEDGE, ID_REPLACE)?;
                for (id, text) in CHECKBOXES {
                    child("BUTTON", text, WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32, none, id)?;
                }
                child("STATIC", "Apply to", label, none, ID_TARGET_LABEL)?;
                let target = child("COMBOBOX", "", combo, none, ID_TARGET)?;
                child("STATIC", "Text case", label, none, ID_CASE_LABEL)?;
                let case = child("COMBOBOX", "", combo, none, ID_CASE)?;
                for (list, names) in [(target, &TARGETS[..]), (case, &CASES[..])] {
                    for name in names {
                        let t = wide(name);
                        send(list, CB_ADDSTRING, 0, t.as_ptr() as isize);
                    }
                }
                let items = child(
                    "SysListView32",
                    "",
                    WS_TABSTOP.0 | LVS_REPORT | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER,
                    WS_EX_CLIENTEDGE,
                    ID_ITEMS,
                )?;
                send(
                    items,
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
                    send(items, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
                }
                child("STATIC", "", SS_NOPREFIX | SS_CENTERIMAGE, none, ID_STATUS)?;
                child("BUTTON", "Undo", WS_TABSTOP.0 | BS_PUSHBUTTON as u32, none, ID_UNDO)?;
                child(
                    "BUTTON",
                    "Rename",
                    WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
                    none,
                    ID_RENAME,
                )?;
                Ok(())
            })();
            if let Err(e) = created {
                let _ = DestroyWindow(hwnd);
                return Err(e);
            }
            let mut state = Box::new(State {
                dpi,
                font: make_font(dpi),
                quiet: true,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            let dpi = GetDpiForWindow(hwnd).max(96);
            apply_font(hwnd, &mut state, dpi);
            layout(hwnd, dpi);
            let mut w = RenameWindow {
                hwnd,
                state,
                _not_send: PhantomData,
            };
            w.set_form(form);
            Ok(w)
        }
    }

    fn set_form(&mut self, form: &RenameForm) {
        let h = self.hwnd;
        self.state.quiet = true;
        set_text(item(h, ID_SEARCH), &form.search);
        set_text(item(h, ID_REPLACE), &form.replace);
        set_checked(h, ID_REGEX, form.regex);
        set_checked(h, ID_MATCH_CASE, form.match_case);
        set_checked(h, ID_ALL, form.all_occurrences);
        set_checked(h, ID_ENUMERATE, form.enumerate);
        set_checked(h, ID_FILES, form.files);
        set_checked(h, ID_FOLDERS, form.folders);
        send(item(h, ID_TARGET), CB_SETCURSEL, form.target.min(TARGETS.len() - 1), 0);
        send(item(h, ID_CASE), CB_SETCURSEL, form.case.min(CASES.len() - 1), 0);
        self.state.quiet = false;
    }

    /// Bring the window up with the keyboard in the search box.
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
            let _ = SetFocus(Some(item(self.hwnd, ID_SEARCH)));
        }
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: own window.
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Replace the preview rows: `[original, renamed, status]` for each item.
    pub fn set_items(&mut self, rows: &[[String; 3]]) {
        let list = item(self.hwnd, ID_ITEMS);
        send(list, WM_SETREDRAW, 0, 0);
        send(list, LVM_DELETEALLITEMS, 0, 0);
        for (i, row) in rows.iter().enumerate() {
            for (col, text) in row.iter().enumerate() {
                let mut t = wide(text);
                let it = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: i as i32,
                    iSubItem: col as i32,
                    pszText: PWSTR(t.as_mut_ptr()),
                    ..Default::default()
                };
                if col == 0 {
                    send(list, LVM_INSERTITEMW, 0, &it as *const _ as isize);
                } else {
                    send(list, LVM_SETITEMTEXTW, i, &it as *const _ as isize);
                }
            }
        }
        send(list, WM_SETREDRAW, 1, 0);
    }

    pub fn set_status(&mut self, text: &str) {
        set_text(item(self.hwnd, ID_STATUS), text);
    }

    /// Show `ico` in the title bar and the taskbar.
    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }
}

impl Drop for RenameWindow {
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
    fn form_round_trips_through_the_controls() {
        let form = RenameForm {
            search: "a".into(),
            replace: "b".into(),
            regex: true,
            target: 2,
            case: 3,
            folders: false,
            ..RenameForm::default()
        };
        let mut w = RenameWindow::new(&form).unwrap();
        assert_eq!(read_form(w.hwnd), form);
        w.set_items(&[["x.txt".into(), "y.txt".into(), String::new()]]);
        assert_eq!(send(item(w.hwnd, ID_ITEMS), LVM_GETITEMCOUNT, 0, 0), 1);
    }
}
