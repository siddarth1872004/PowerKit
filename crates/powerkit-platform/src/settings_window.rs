//! The settings window: the sections on the left, the chosen section's settings as a
//! two-column list on the right, and a box underneath to edit the selected value.
//! A utility's section starts with its animated walkthrough.
//!
//! The window only shows what its owner gives it and reports what the user does as
//! [`SettingsEvent`]s on the event loop. Closing it hides it.

use crate::event_loop::push;
use crate::guide_view::{self, Scene};
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use powerkit_core::{Event, SettingsEvent};
use std::marker::PhantomData;
use windows::core::PWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_RETURN};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Settings";
const ID_SECTIONS: i32 = 1;
const ID_ROWS: i32 = 2;
const ID_EDIT: i32 = 3;
const ID_APPLY: i32 = 4;
const ID_STATUS: i32 = 5;
const ID_GUIDE: i32 = 6;
const SUBCLASS_ID: usize = 1;
// Static control styles; their windows-crate constants sit behind a feature this crate
// does not otherwise need.
const SS_LEFTNOWORDWRAP: u32 = 0x0C;
const SS_NOPREFIX: u32 = 0x80;

// Sizes in 96-DPI pixels.
const WIDTH: i32 = 940;
const HEIGHT: i32 = 620;
const PAD: i32 = 12;
const SECTIONS_W: i32 = 210;
const NAME_COL: i32 = 300;
const LINE: i32 = 28;
const BUTTON_W: i32 = 90;
const FONT_PT: i32 = 10;
const SECTION_ROW: i32 = 30;
const SECTION_ICON: i32 = 20;

struct State {
    dpi: u32,
    font: HFONT,
    /// The values shown in the second column, copied into the edit box on selection.
    values: Vec<String>,
    /// Section names and their icons (.ico files), drawn by the window itself.
    sections: Vec<(String, Option<&'static [u8]>)>,
    guide: bool,
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// # Safety
/// `hwnd` must be a settings window whose state pointer has not been cleared.
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
    for id in [ID_SECTIONS, ID_ROWS, ID_EDIT, ID_APPLY, ID_STATUS] {
        send(item(hwnd, id), WM_SETFONT, st.font.0 as usize, 1);
    }
    send(
        item(hwnd, ID_ROWS),
        LVM_SETCOLUMNWIDTH,
        0,
        scale(NAME_COL, dpi) as isize,
    );
    send(
        item(hwnd, ID_SECTIONS),
        LB_SETITEMHEIGHT,
        0,
        scale(SECTION_ROW, dpi) as isize,
    );
    // SAFETY: no control uses the old font any more.
    let _ = unsafe { DeleteObject(old.into()) };
}

fn layout(hwnd: HWND, st: &State) {
    let dpi = st.dpi;
    let mut rc = RECT::default();
    // SAFETY: own window; `rc` is a valid out pointer.
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let s = |v| scale(v, dpi);
    let (w, h) = (rc.right, rc.bottom);
    let left = s(PAD) + s(SECTIONS_W) + s(PAD);
    let status_top = h - s(PAD) - s(LINE);
    let edit_top = status_top - s(PAD / 2) - s(LINE);
    let place = |id: i32, x: i32, y: i32, cx: i32, cy: i32| {
        // SAFETY: own child window.
        let _ = unsafe { MoveWindow(item(hwnd, id), x, y, cx.max(0), cy.max(0), true) };
    };
    place(ID_SECTIONS, s(PAD), s(PAD), s(SECTIONS_W), status_top - s(PAD) * 3 / 2);
    let mut rows_top = s(PAD);
    if st.guide {
        // The walkthrough takes at most half the height, keeping its shape.
        let gw = w - left - s(PAD);
        let gh = guide_view::height_for(gw, dpi).min((edit_top - s(PAD) * 3) / 2);
        let gw = gw.min(
            ((gh - guide_view::height_for(0, dpi)) as f32 * powerkit_guide::WIDTH / powerkit_guide::HEIGHT) as i32,
        );
        place(ID_GUIDE, left + (w - left - s(PAD) - gw) / 2, s(PAD), gw, gh);
        rows_top += gh + s(PAD);
    }
    place(
        ID_ROWS,
        left,
        rows_top,
        w - left - s(PAD),
        edit_top - s(PAD) / 2 - rows_top,
    );
    place(ID_EDIT, left, edit_top, w - left - s(PAD) * 2 - s(BUTTON_W), s(LINE));
    place(ID_APPLY, w - s(PAD) - s(BUTTON_W), edit_top, s(BUTTON_W), s(LINE));
    place(ID_STATUS, s(PAD), status_top, w - s(PAD) * 2, s(LINE));
    send(
        item(hwnd, ID_ROWS),
        LVM_SETCOLUMNWIDTH,
        1,
        LVSCW_AUTOSIZE_USEHEADER as isize,
    );
}

fn selected_row(hwnd: HWND) -> Option<usize> {
    let i = send(item(hwnd, ID_ROWS), LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    usize::try_from(i).ok()
}

fn submit(hwnd: HWND) {
    let Some(row) = selected_row(hwnd) else { return };
    let edit = item(hwnd, ID_EDIT);
    // SAFETY: own child window; the buffer is sized from its text length.
    let text = unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(edit) as usize + 1];
        let n = GetWindowTextW(edit, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    };
    push(Event::Settings(SettingsEvent::Edit { row, text }));
}

fn set_text(hwnd: HWND, text: &str) {
    let t = wide(text);
    // SAFETY: `t` outlives the call.
    let _ = unsafe { SetWindowTextW(hwnd, pcwstr(&t)) };
}

/// One row of the section list: icon, then name.
fn draw_section(di: &DRAWITEMSTRUCT, st: &State) {
    let Some((name, ico)) = st.sections.get(di.itemID as usize) else {
        return;
    };
    let selected = di.itemState.0 & ODS_SELECTED.0 != 0;
    let (back, ink) = if selected {
        (COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT)
    } else {
        (COLOR_WINDOW, COLOR_WINDOWTEXT)
    };
    let s = |v| scale(v, st.dpi);
    let r = di.rcItem;
    let size = s(SECTION_ICON);
    // SAFETY: drawing into the DC Windows lent for this item, within its rectangle;
    // the DC's state is put back before returning.
    unsafe {
        FillRect(di.hDC, &r, GetSysColorBrush(back));
        if let Some(icon) = ico.and_then(|i| crate::icon::load(i, size).ok()) {
            let y = r.top + (r.bottom - r.top - size) / 2;
            let _ = DrawIconEx(di.hDC, r.left + s(6), y, icon, size, size, 0, None, DI_NORMAL);
        }
        let old_font = SelectObject(di.hDC, st.font.into());
        SetBkMode(di.hDC, TRANSPARENT);
        SetTextColor(di.hDC, COLORREF(GetSysColor(ink)));
        let mut text: Vec<u16> = name.encode_utf16().collect();
        let mut tr = RECT {
            left: r.left + size + s(14),
            ..r
        };
        DrawTextW(
            di.hDC,
            &mut text,
            &mut tr,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(di.hDC, old_font);
        if di.itemState.0 & ODS_FOCUS.0 != 0 {
            let _ = DrawFocusRect(di.hDC, &r);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a settings window (this is its class's procedure).
    let st = unsafe { state(hwnd) };
    match (msg, st) {
        (WM_SIZE, Some(st)) => {
            layout(hwnd, st);
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
        (WM_COMMAND, Some(_)) => {
            let (id, code) = ((wparam.0 & 0xFFFF) as i32, ((wparam.0 >> 16) & 0xFFFF) as u32);
            if id == ID_SECTIONS && code == LBN_SELCHANGE {
                if let Ok(i) = usize::try_from(send(item(hwnd, ID_SECTIONS), LB_GETCURSEL, 0, 0)) {
                    push(Event::Settings(SettingsEvent::Section(i)));
                }
            } else if id == ID_APPLY && code == BN_CLICKED {
                submit(hwnd);
            }
            LRESULT(0)
        }
        (WM_NOTIFY, Some(st)) => {
            // SAFETY: WM_NOTIFY always carries an NMHDR.
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom == ID_ROWS as usize {
                if hdr.code == LVN_ITEMCHANGED {
                    // SAFETY: LVN_ITEMCHANGED carries an NMLISTVIEW.
                    let nm = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
                    let now = nm.uNewState & LVIS_SELECTED.0 != 0;
                    let before = nm.uOldState & LVIS_SELECTED.0 != 0;
                    if now && !before {
                        let value = usize::try_from(nm.iItem).ok().and_then(|i| st.values.get(i));
                        set_text(item(hwnd, ID_EDIT), value.map_or("", |v| v.as_str()));
                    }
                } else if hdr.code == NM_DBLCLK {
                    if let Some(row) = selected_row(hwnd) {
                        push(Event::Settings(SettingsEvent::Toggle(row)));
                    }
                }
            }
            LRESULT(0)
        }
        (WM_MEASUREITEM, st) => {
            // SAFETY: WM_MEASUREITEM carries a MEASUREITEMSTRUCT to fill in. The list
            // asks while it is being created, before the state is set.
            let mi = unsafe { &mut *(lparam.0 as *mut MEASUREITEMSTRUCT) };
            let dpi = st.map_or_else(|| unsafe { GetDpiForSystem() }.max(96), |s| s.dpi);
            mi.itemHeight = scale(SECTION_ROW, dpi) as u32;
            LRESULT(1)
        }
        (WM_DRAWITEM, Some(st)) => {
            // SAFETY: WM_DRAWITEM carries a DRAWITEMSTRUCT for one of our controls.
            let di = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
            if di.CtlID == ID_SECTIONS as u32 {
                draw_section(di, st);
            }
            LRESULT(1)
        }
        (WM_CLOSE, _) => {
            // SAFETY: own window. The return value is the previous visibility.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        (WM_GETMINMAXINFO, Some(st)) => {
            // SAFETY: WM_GETMINMAXINFO carries a MINMAXINFO to fill in.
            let mm = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mm.ptMinTrackSize.x = scale(WIDTH / 2, st.dpi);
            mm.ptMinTrackSize.y = scale(HEIGHT / 2, st.dpi);
            LRESULT(0)
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Enter in the edit box applies the value instead of beeping.
unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    refdata: usize,
) -> LRESULT {
    let enter = wparam.0 == VK_RETURN.0 as usize;
    match msg {
        WM_KEYDOWN if enter => {
            submit(HWND(refdata as *mut _));
            LRESULT(0)
        }
        WM_CHAR if wparam.0 == '\r' as usize => LRESULT(0),
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

pub struct SettingsWindow {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl SettingsWindow {
    /// Creates the window hidden; [`SettingsWindow::show`] brings it up.
    pub fn new() -> Result<SettingsWindow> {
        let class = wide(CLASS_NAME);
        let title = wide("PowerKit settings");
        let empty = wide("");
        let apply = wide("Apply");
        // SAFETY: strings outlive the calls; `wndproc` and `edit_proc` match their
        // callback contracts. The state pointer stays valid until `Drop` clears it.
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES | ICC_STANDARD_CLASSES,
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
            .context("CreateWindowEx(settings)")?;
            let child = |class: &[u16], text: &[u16], style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: i32| {
                CreateWindowExW(
                    ex,
                    pcwstr(class),
                    pcwstr(text),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
                    0,
                    0,
                    0,
                    0,
                    Some(hwnd),
                    Some(HMENU(id as isize as *mut _)),
                    Some(instance.into()),
                    None,
                )
            };
            let created = (|| -> Result<HWND> {
                child(
                    &wide("LISTBOX"),
                    &empty,
                    WS_VSCROLL
                        | WINDOW_STYLE(
                            (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32,
                        ),
                    WS_EX_CLIENTEDGE,
                    ID_SECTIONS,
                )
                .context("CreateWindowEx(settings sections)")?;
                let rows = child(
                    &wide("SysListView32"),
                    &empty,
                    WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS),
                    WS_EX_CLIENTEDGE,
                    ID_ROWS,
                )
                .context("CreateWindowEx(settings list)")?;
                let edit = child(
                    &wide("EDIT"),
                    &empty,
                    WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                    WS_EX_CLIENTEDGE,
                    ID_EDIT,
                )
                .context("CreateWindowEx(settings edit box)")?;
                child(
                    &wide("BUTTON"),
                    &apply,
                    WINDOW_STYLE(BS_PUSHBUTTON as u32),
                    WINDOW_EX_STYLE(0),
                    ID_APPLY,
                )
                .context("CreateWindowEx(settings apply)")?;
                child(
                    &wide("STATIC"),
                    &empty,
                    WINDOW_STYLE(SS_LEFTNOWORDWRAP | SS_NOPREFIX),
                    WINDOW_EX_STYLE(0),
                    ID_STATUS,
                )
                .context("CreateWindowEx(settings status)")?;
                send(
                    rows,
                    LVM_SETEXTENDEDLISTVIEWSTYLE,
                    0,
                    (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
                );
                for (i, name) in ["Setting", "Value"].into_iter().enumerate() {
                    let mut text = wide(name);
                    let col = LVCOLUMNW {
                        mask: LVCF_TEXT | LVCF_WIDTH,
                        cx: scale(NAME_COL, dpi),
                        pszText: PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    send(rows, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
                }
                if !SetWindowSubclass(edit, Some(edit_proc), SUBCLASS_ID, hwnd.0 as usize).as_bool() {
                    return Err(Error::Other("SetWindowSubclass(settings edit box) failed".into()));
                }
                guide_view::create(hwnd, instance.into(), ID_GUIDE)?;
                Ok(edit)
            })();
            if let Err(e) = created {
                let _ = DestroyWindow(hwnd);
                return Err(e);
            }
            let mut state = Box::new(State {
                dpi,
                font: make_font(dpi),
                values: Vec::new(),
                sections: Vec::new(),
                guide: false,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            let dpi = GetDpiForWindow(hwnd).max(96);
            apply_font(hwnd, &mut state, dpi);
            layout(hwnd, &state);
            Ok(SettingsWindow {
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

    /// The sections, each with an optional icon (the bytes of an .ico file).
    pub fn set_sections(&mut self, sections: &[(String, Option<&'static [u8]>)], selected: usize) {
        let list = item(self.hwnd, ID_SECTIONS);
        self.state.sections = sections.to_vec();
        send(list, LB_RESETCONTENT, 0, 0);
        for (name, _) in sections {
            let t = wide(name);
            send(list, LB_ADDSTRING, 0, t.as_ptr() as isize);
        }
        send(list, LB_SETCURSEL, selected, 0);
    }

    /// Play `scene` above the settings, or hide the walkthrough area.
    pub fn set_guide(&mut self, scene: Option<Scene>) {
        let view = item(self.hwnd, ID_GUIDE);
        self.state.guide = scene.is_some();
        guide_view::set_scene(view, scene);
        // SAFETY: own child window. The return value is the previous visibility.
        let _ = unsafe { ShowWindow(view, if self.state.guide { SW_SHOWNA } else { SW_HIDE }) };
        layout(self.hwnd, &self.state);
    }

    /// Show `ico` in the title bar and the taskbar.
    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }

    /// Replace the rows (`(name, value)`) and select `selected`, which also puts its
    /// value in the edit box.
    pub fn set_rows(&mut self, rows: &[(String, String)], selected: Option<usize>) {
        let list = item(self.hwnd, ID_ROWS);
        send(list, WM_SETREDRAW, 0, 0);
        send(list, LVM_DELETEALLITEMS, 0, 0);
        self.state.values = rows.iter().map(|(_, v)| v.clone()).collect();
        for (i, (name, value)) in rows.iter().enumerate() {
            let mut n = wide(name);
            let mut v = wide(value);
            let mut it = LVITEMW {
                mask: LVIF_TEXT,
                iItem: i as i32,
                pszText: PWSTR(n.as_mut_ptr()),
                ..Default::default()
            };
            send(list, LVM_INSERTITEMW, 0, &it as *const _ as isize);
            it.iSubItem = 1;
            it.pszText = PWSTR(v.as_mut_ptr());
            send(list, LVM_SETITEMTEXTW, i, &it as *const _ as isize);
        }
        send(list, WM_SETREDRAW, 1, 0);
        layout(self.hwnd, &self.state);
        set_text(item(self.hwnd, ID_EDIT), "");
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

    /// Put the keyboard focus in the edit box, with its text selected.
    pub fn focus_edit(&mut self) {
        let edit = item(self.hwnd, ID_EDIT);
        send(edit, EM_SETSEL, 0, -1);
        // SAFETY: own child window.
        let _ = unsafe { SetFocus(Some(edit)) };
    }

    pub fn set_status(&mut self, text: &str) {
        set_text(item(self.hwnd, ID_STATUS), text);
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        // SAFETY: own windows, on the thread that created them; the state pointer is
        // cleared before the box is freed.
        unsafe {
            let _ = RemoveWindowSubclass(item(self.hwnd, ID_EDIT), Some(edit_proc), SUBCLASS_ID);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.state.font.into());
        }
    }
}
