//! A table editor window: rows in a list (optionally with a check box each), edit
//! boxes under it that selecting a row fills, and Add / Update / Delete / Reload /
//! Save. An optional selector above the list switches what is shown. A
//! [`TableSpec`] says which columns, boxes and extras a window has.
//!
//! The window only shows what its owner gives it and reports what the user does as
//! [`Event::Table`]s tagged with the spec's id. Closing it hides it.

use crate::event_loop::push;
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::{Event, TableEvent};
use std::marker::PhantomData;
use windows::core::PWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Table";
const ID_LIST: i32 = 1;
const ID_CHOICE: i32 = 2;
const ID_ADD: i32 = 3;
const ID_UPDATE: i32 = 4;
const ID_DELETE: i32 = 5;
const ID_STATUS: i32 = 6;
const ID_RELOAD: i32 = 7;
const ID_SAVE: i32 = 8;
/// Field `i` has its label at `FIELD_BASE + 2i` and its box right after.
const FIELD_BASE: i32 = 100;
// Static control styles; their windows-crate constants sit behind a feature this crate
// does not otherwise need.
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
/// List view check box states, as state image indexes.
const UNCHECKED: u32 = 1;
const CHECKED: u32 = 2;

// Sizes in 96-DPI pixels.
const WIDTH: i32 = 920;
const HEIGHT: i32 = 600;
const PAD: i32 = 12;
const LINE: i32 = 26;
const BUTTON_W: i32 = 90;
const CHOICE_W: i32 = 260;
const FONT_PT: i32 = 10;

/// An edit box under the list, filled from the column with the same index.
pub struct Field {
    pub label: &'static str,
    pub label_width: i32,
    /// Box width in 96-DPI pixels; 0 shares out the width left over.
    pub width: i32,
}

pub struct TableSpec {
    /// Tags this window's events.
    pub id: &'static str,
    pub title: &'static str,
    /// Column names and 96-DPI widths.
    pub columns: &'static [(&'static str, i32)],
    pub fields: &'static [Field],
    /// Whether rows have a check box.
    pub checks: bool,
    /// Entries of the selector above the list; none means no selector.
    pub choices: &'static [&'static str],
}

fn label_id(i: usize) -> i32 {
    FIELD_BASE + 2 * i as i32
}

fn field_id(i: usize) -> i32 {
    label_id(i) + 1
}

struct State {
    spec: &'static TableSpec,
    dpi: u32,
    font: HFONT,
    /// The text of each row's columns that have a box, copied in on selection.
    rows: Vec<Vec<String>>,
    /// Set while the owner fills the list, so check boxes set then are not reported.
    quiet: bool,
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// # Safety
/// `hwnd` must be a table window whose state pointer has not been cleared.
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
    let fields = (0..st.spec.fields.len()).flat_map(|i| [label_id(i), field_id(i)]);
    for id in (ID_LIST..=ID_SAVE).chain(fields) {
        send(item(hwnd, id), WM_SETFONT, st.font.0 as usize, 1);
    }
    for (i, (_, w)) in st.spec.columns.iter().enumerate() {
        send(item(hwnd, ID_LIST), LVM_SETCOLUMNWIDTH, i, scale(*w, dpi) as isize);
    }
    // SAFETY: no control uses the old font any more.
    let _ = unsafe { DeleteObject(old.into()) };
}

fn layout(hwnd: HWND, spec: &TableSpec, dpi: u32) {
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
    let bottom = h - s(PAD) - s(LINE);
    let buttons = bottom - s(PAD) - s(LINE);
    let fields = buttons - s(PAD / 2) - s(LINE);
    let mut top = s(PAD);
    if !spec.choices.is_empty() {
        // A drop-down list's height includes its opened list.
        place(ID_CHOICE, s(PAD), top, s(CHOICE_W), s(LINE * 8));
        top += s(LINE) + s(PAD / 2);
    }
    place(ID_LIST, s(PAD), top, w - s(PAD) * 2, fields - s(PAD) - top);

    let gaps = s(PAD / 2) * (spec.fields.len() * 2).saturating_sub(1) as i32;
    let fixed: i32 = spec.fields.iter().map(|f| s(f.label_width) + s(f.width)).sum();
    let flexible = spec.fields.iter().filter(|f| f.width == 0).count().max(1) as i32;
    let share = (w - s(PAD) * 2 - fixed - gaps) / flexible;
    let mut x = s(PAD);
    for (i, f) in spec.fields.iter().enumerate() {
        let width = if f.width == 0 { share } else { s(f.width) };
        place(label_id(i), x, fields, s(f.label_width), s(LINE));
        x += s(f.label_width) + s(PAD / 2);
        place(field_id(i), x, fields, width, s(LINE));
        x += width + s(PAD / 2);
    }

    for (i, id) in [ID_ADD, ID_UPDATE, ID_DELETE].into_iter().enumerate() {
        place(
            id,
            s(PAD) + i as i32 * (s(BUTTON_W) + s(PAD / 2)),
            buttons,
            s(BUTTON_W),
            s(LINE),
        );
    }
    let right = s(BUTTON_W) * 2 + s(PAD / 2);
    place(ID_STATUS, s(PAD), bottom, w - s(PAD) * 3 - right, s(LINE));
    place(ID_RELOAD, w - s(PAD) - right, bottom, s(BUTTON_W), s(LINE));
    place(ID_SAVE, w - s(PAD) - s(BUTTON_W), bottom, s(BUTTON_W), s(LINE));
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

fn read_fields(hwnd: HWND, spec: &TableSpec) -> Vec<String> {
    (0..spec.fields.len())
        .map(|i| text_of(item(hwnd, field_id(i))))
        .collect()
}

fn selected_row(hwnd: HWND) -> Option<usize> {
    let i = send(item(hwnd, ID_LIST), LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    usize::try_from(i).ok()
}

fn check_state(state: u32) -> u32 {
    (state & LVIS_STATEIMAGEMASK.0) >> 12
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a table window (this is its class's procedure).
    let st = unsafe { state(hwnd) };
    match (msg, st) {
        (WM_SIZE, Some(st)) => {
            layout(hwnd, st.spec, st.dpi);
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
            let event = if id == ID_CHOICE && code == CBN_SELCHANGE {
                usize::try_from(send(item(hwnd, ID_CHOICE), CB_GETCURSEL, 0, 0))
                    .ok()
                    .map(TableEvent::Choice)
            } else if code == BN_CLICKED {
                match (id, selected_row(hwnd)) {
                    (ID_ADD, _) => Some(TableEvent::Add(read_fields(hwnd, st.spec))),
                    (ID_UPDATE, Some(row)) => Some(TableEvent::Update {
                        row,
                        fields: read_fields(hwnd, st.spec),
                    }),
                    (ID_DELETE, Some(row)) => Some(TableEvent::Delete(row)),
                    (ID_RELOAD, _) => Some(TableEvent::Reload),
                    (ID_SAVE, _) => Some(TableEvent::Save),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(event) = event {
                push(Event::Table {
                    table: st.spec.id,
                    event,
                });
            }
            LRESULT(0)
        }
        (WM_NOTIFY, Some(st)) => {
            // SAFETY: WM_NOTIFY always carries an NMHDR.
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom == ID_LIST as usize && hdr.code == LVN_ITEMCHANGED {
                // SAFETY: LVN_ITEMCHANGED carries an NMLISTVIEW.
                let nm = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
                let Ok(row) = usize::try_from(nm.iItem) else {
                    return LRESULT(0);
                };
                let (now, before) = (check_state(nm.uNewState), check_state(nm.uOldState));
                if st.spec.checks && !st.quiet && before != 0 && now != before {
                    push(Event::Table {
                        table: st.spec.id,
                        event: TableEvent::Toggle {
                            row,
                            on: now == CHECKED,
                        },
                    });
                }
                let selected = nm.uNewState & LVIS_SELECTED.0 != 0;
                if selected && nm.uOldState & LVIS_SELECTED.0 == 0 {
                    if let Some(texts) = st.rows.get(row) {
                        for (i, t) in texts.iter().enumerate() {
                            set_text(item(hwnd, field_id(i)), t);
                        }
                    }
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

/// One listed row: its check box (ignored without check boxes), then each column.
pub type TableRow = (bool, Vec<String>);

pub struct TableWindow {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl TableWindow {
    /// Creates the window hidden; [`TableWindow::show`] brings it up.
    pub fn new(spec: &'static TableSpec) -> Result<TableWindow> {
        let class = wide(CLASS_NAME);
        let title = wide(spec.title);
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
            .context("CreateWindowEx(table)")?;
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
                .context("CreateWindowEx(table control)")
            };
            let created = (|| -> Result<()> {
                let label = SS_NOPREFIX | SS_CENTERIMAGE;
                let edit = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
                let button = WS_TABSTOP.0 | BS_PUSHBUTTON as u32;
                let none = WINDOW_EX_STYLE(0);
                let list = child(
                    "SysListView32",
                    "",
                    WS_TABSTOP.0 | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER,
                    WS_EX_CLIENTEDGE,
                    ID_LIST,
                )?;
                let checks = if spec.checks { LVS_EX_CHECKBOXES } else { 0 };
                send(
                    list,
                    LVM_SETEXTENDEDLISTVIEWSTYLE,
                    0,
                    (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | checks) as isize,
                );
                for (i, &(name, w)) in spec.columns.iter().enumerate() {
                    let mut text = wide(name);
                    let col = LVCOLUMNW {
                        mask: LVCF_TEXT | LVCF_WIDTH,
                        cx: scale(w, dpi),
                        pszText: PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    send(list, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
                }
                if !spec.choices.is_empty() {
                    let combo = WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32;
                    let c = child("COMBOBOX", "", combo, none, ID_CHOICE)?;
                    for choice in spec.choices {
                        let t = wide(choice);
                        send(c, CB_ADDSTRING, 0, t.as_ptr() as isize);
                    }
                    send(c, CB_SETCURSEL, 0, 0);
                }
                for (i, f) in spec.fields.iter().enumerate() {
                    child("STATIC", f.label, label, none, label_id(i))?;
                    child("EDIT", "", edit, WS_EX_CLIENTEDGE, field_id(i))?;
                }
                child("BUTTON", "Add", button, none, ID_ADD)?;
                child("BUTTON", "Update", button, none, ID_UPDATE)?;
                child("BUTTON", "Delete", button, none, ID_DELETE)?;
                child("STATIC", "", label, none, ID_STATUS)?;
                child("BUTTON", "Reload", button, none, ID_RELOAD)?;
                child("BUTTON", "Save", button, none, ID_SAVE)?;
                Ok(())
            })();
            if let Err(e) = created {
                let _ = DestroyWindow(hwnd);
                return Err(e);
            }
            let mut state = Box::new(State {
                spec,
                dpi,
                font: make_font(dpi),
                rows: Vec::new(),
                quiet: false,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            let dpi = GetDpiForWindow(hwnd).max(96);
            apply_font(hwnd, &mut state, dpi);
            layout(hwnd, spec, dpi);
            Ok(TableWindow {
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

    /// Replace the listed rows and select `selected`, which also fills the boxes.
    pub fn set_rows(&mut self, rows: &[TableRow], selected: Option<usize>) {
        let list = item(self.hwnd, ID_LIST);
        let boxes = self.state.spec.fields.len();
        self.state.quiet = true;
        send(list, WM_SETREDRAW, 0, 0);
        send(list, LVM_DELETEALLITEMS, 0, 0);
        self.state.rows = rows
            .iter()
            .map(|(_, cols)| cols.iter().take(boxes).cloned().collect())
            .collect();
        for (i, (checked, cols)) in rows.iter().enumerate() {
            for (col, text) in cols.iter().enumerate() {
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
            if self.state.spec.checks {
                let check = if *checked { CHECKED } else { UNCHECKED };
                let it = LVITEMW {
                    state: LIST_VIEW_ITEM_STATE_FLAGS(check << 12),
                    stateMask: LVIS_STATEIMAGEMASK,
                    ..Default::default()
                };
                send(list, LVM_SETITEMSTATE, i, &it as *const _ as isize);
            }
        }
        send(list, WM_SETREDRAW, 1, 0);
        self.state.quiet = false;
        for i in 0..boxes {
            set_text(item(self.hwnd, field_id(i)), "");
        }
        if let Some(row) = selected.filter(|&r| r < rows.len()) {
            let it = LVITEMW {
                state: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
                stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
                ..Default::default()
            };
            send(list, LVM_SETITEMSTATE, row, &it as *const _ as isize);
            send(list, LVM_ENSUREVISIBLE, row, 0);
        }
    }

    /// Show `index` in the selector without reporting it as a change.
    pub fn set_choice(&mut self, index: usize) {
        send(item(self.hwnd, ID_CHOICE), CB_SETCURSEL, index, 0);
    }

    pub fn set_status(&mut self, text: &str) {
        set_text(item(self.hwnd, ID_STATUS), text);
    }

    /// Show `ico` in the title bar and the taskbar.
    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }

    pub fn set_title(&mut self, text: &str) {
        set_text(self.hwnd, text);
    }
}

impl Drop for TableWindow {
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

    static SPEC: TableSpec = TableSpec {
        id: "test",
        title: "Test",
        columns: &[("A", 100), ("B", 100), ("Note", 80)],
        fields: &[
            Field {
                label: "A",
                label_width: 30,
                width: 120,
            },
            Field {
                label: "B",
                label_width: 30,
                width: 0,
            },
        ],
        checks: true,
        choices: &["One", "Two"],
    };

    #[test]
    fn rows_fill_the_list_with_check_boxes() {
        let mut w = TableWindow::new(&SPEC).unwrap();
        let row = |on, a: &str| (on, vec![a.to_string(), "x.local".into(), "note".into()]);
        w.set_rows(&[row(true, "1.1.1.1"), row(false, "2.2.2.2")], Some(1));
        let list = item(w.hwnd, ID_LIST);
        assert_eq!(send(list, LVM_GETITEMCOUNT, 0, 0), 2);
        let state = |i| send(list, LVM_GETITEMSTATE, i, LVIS_STATEIMAGEMASK.0 as isize) as u32;
        assert_eq!(check_state(state(0)), CHECKED);
        assert_eq!(check_state(state(1)), UNCHECKED);
        assert_eq!(selected_row(w.hwnd), Some(1));
        assert_eq!(
            text_of(item(w.hwnd, field_id(0))),
            "2.2.2.2",
            "selection fills the boxes"
        );
        assert_eq!(text_of(item(w.hwnd, field_id(1))), "x.local");
        w.set_choice(1);
        assert_eq!(send(item(w.hwnd, ID_CHOICE), CB_GETCURSEL, 0, 0), 1);
    }
}
