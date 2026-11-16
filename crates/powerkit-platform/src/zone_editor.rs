//! The FancyZones layout editor: the layouts in a list and the chosen one drawn in
//! the shape of the main display, where clicking a zone splits it (Shift splits it
//! across), dragging the line between zones moves it and right-clicking a zone
//! removes it. Layouts start from templates.
//!
//! Save reports every layout as [`Event::ZoneLayouts`]; closing with unsaved changes
//! asks first. Closing hides the window.

use crate::event_loop::push;
use crate::util::{pcwstr, wide};
use crate::{Context, Result};
use powerkit_core::zone_layout::{self, Split, ZoneRect, ZONE_UNITS};
use powerkit_core::{Event, ZoneLayout};
use std::marker::PhantomData;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.ZoneEditor";
const ID_LIST: i32 = 1;
const ID_NEW: i32 = 2;
const ID_COPY: i32 = 3;
const ID_DELETE: i32 = 4;
const ID_NAME_LABEL: i32 = 5;
const ID_NAME: i32 = 6;
const ID_SPACING_LABEL: i32 = 7;
const ID_SPACING: i32 = 8;
const ID_MARGIN_LABEL: i32 = 9;
const ID_MARGIN: i32 = 10;
const ID_TEMPLATE_LABEL: i32 = 11;
const ID_TEMPLATE: i32 = 12;
const ID_COUNT_LABEL: i32 = 13;
const ID_COUNT: i32 = 14;
const ID_APPLY: i32 = 15;
const ID_STATUS: i32 = 16;
const ID_CLOSE: i32 = 17;
const ID_SAVE: i32 = 18;
const ALL_IDS: [i32; 18] = [
    ID_LIST,
    ID_NEW,
    ID_COPY,
    ID_DELETE,
    ID_NAME_LABEL,
    ID_NAME,
    ID_SPACING_LABEL,
    ID_SPACING,
    ID_MARGIN_LABEL,
    ID_MARGIN,
    ID_TEMPLATE_LABEL,
    ID_TEMPLATE,
    ID_COUNT_LABEL,
    ID_COUNT,
    ID_APPLY,
    ID_STATUS,
    ID_CLOSE,
    ID_SAVE,
];
// Static control styles; their windows-crate constants sit behind a feature this crate
// does not otherwise need.
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const MK_SHIFT: usize = 0x4;

// Sizes in 96-DPI pixels.
const WIDTH: i32 = 1040;
const HEIGHT: i32 = 680;
const PAD: i32 = 12;
const LINE: i32 = 26;
const LIST_W: i32 = 220;
const LABEL_W: i32 = 64;
const SMALL_W: i32 = 56;
const COMBO_W: i32 = 170;
const BUTTON_W: i32 = 90;
const FONT_PT: i32 = 10;
/// How near a line between zones the pointer must be to drag it.
const GRAB: i32 = 6;
/// Splits land on multiples of this many units (half a percent).
const SNAP: u32 = 50;

const HINT: &str =
    "Click a zone to split it (hold Shift to split it across). Drag a line between zones to move it. Right-click a zone to remove it.";

/// A way to start a layout.
pub struct ZoneTemplate {
    pub name: String,
    /// The zones for one zone, two zones and so on.
    pub by_count: Vec<Vec<ZoneRect>>,
}

struct State {
    dpi: u32,
    font: HFONT,
    /// Set while the window fills in its own controls, so that is not taken as typing.
    quiet: bool,
    layouts: Vec<ZoneLayout>,
    current: usize,
    templates: Vec<ZoneTemplate>,
    /// Where the layout is drawn, in client coordinates.
    canvas: RECT,
    /// Width over height of the main display's work area.
    aspect: f32,
    /// The line being dragged.
    drag: Option<Split>,
    /// The line under the pointer.
    hover: Option<Split>,
    dirty: bool,
}

impl State {
    fn layout(&mut self) -> Option<&mut ZoneLayout> {
        self.layouts.get_mut(self.current)
    }

    /// The canvas point `(x, y)` in units, when it is on the canvas.
    fn units(&self, x: i32, y: i32) -> Option<(u32, u32)> {
        let c = self.canvas;
        let (w, h) = (c.right - c.left, c.bottom - c.top);
        if w <= 0 || h <= 0 || x < c.left || y < c.top || x > c.right || y > c.bottom {
            return None;
        }
        let u = |v: i32, len: i32| (v as i64 * ZONE_UNITS as i64 / len as i64) as u32;
        Some((u(x - c.left, w), u(y - c.top, h)))
    }

    /// How many units the grab distance is.
    fn grab_units(&self) -> u32 {
        let w = (self.canvas.right - self.canvas.left).max(1);
        (scale(GRAB, self.dpi) as i64 * ZONE_UNITS as i64 / w as i64) as u32
    }

    fn zone_rect(&self, z: &ZoneRect) -> RECT {
        let c = self.canvas;
        let (w, h) = ((c.right - c.left) as i64, (c.bottom - c.top) as i64);
        let px = |v: u32, len: i64| (v as i64 * len / ZONE_UNITS as i64) as i32;
        RECT {
            left: c.left + px(z[0], w),
            top: c.top + px(z[1], h),
            right: c.left + px(z[0] + z[2], w),
            bottom: c.top + px(z[1] + z[3], h),
        }
    }
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// # Safety
/// `hwnd` must be a zone editor window whose state pointer has not been cleared.
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

fn make_font(pt: i32, dpi: u32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
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
}

fn apply_font(hwnd: HWND, st: &mut State, dpi: u32) {
    let old = std::mem::replace(&mut st.font, make_font(FONT_PT, dpi, FW_NORMAL.0 as i32));
    st.dpi = dpi;
    for id in ALL_IDS {
        send(item(hwnd, id), WM_SETFONT, st.font.0 as usize, 1);
    }
    // SAFETY: no control uses the old font any more.
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

fn set_text(hwnd: HWND, text: &str) {
    let t = wide(text);
    // SAFETY: `t` outlives the call.
    let _ = unsafe { SetWindowTextW(hwnd, pcwstr(&t)) };
}

fn invalidate(hwnd: HWND, r: Option<&RECT>) {
    // SAFETY: own window; `r` outlives the call.
    let _ = unsafe { InvalidateRect(Some(hwnd), r.map(|r| r as *const RECT), false) };
}

fn place_all(hwnd: HWND, st: &mut State) {
    let mut rc = RECT::default();
    // SAFETY: own window; `rc` is a valid out pointer.
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let s = |v| scale(v, st.dpi);
    let (w, h) = (rc.right, rc.bottom);
    let place = |id: i32, x: i32, y: i32, cx: i32, cy: i32| {
        // SAFETY: own child window.
        let _ = unsafe { MoveWindow(item(hwnd, id), x, y, cx.max(0), cy.max(0), true) };
    };
    let row = |n: i32| s(PAD) + n * (s(LINE) + s(PAD / 2));
    let bottom = h - s(PAD) - s(LINE);

    // The list with its buttons under it.
    let buttons_y = bottom - s(PAD) - s(LINE);
    place(ID_LIST, s(PAD), row(0), s(LIST_W), buttons_y - s(PAD / 2) - row(0));
    let bw = (s(LIST_W) - 2 * s(PAD / 2)) / 3;
    for (i, id) in [ID_NEW, ID_COPY, ID_DELETE].into_iter().enumerate() {
        place(id, s(PAD) + i as i32 * (bw + s(PAD / 2)), buttons_y, bw, s(LINE));
    }

    // The chosen layout's settings on two lines.
    let x0 = s(PAD) * 2 + s(LIST_W);
    let right = w - s(PAD);
    let small = s(LABEL_W) + s(SMALL_W) + s(PAD);
    let name_w = (right - x0 - s(LABEL_W) - 2 * small).max(s(80));
    place(ID_NAME_LABEL, x0, row(0), s(LABEL_W), s(LINE));
    place(ID_NAME, x0 + s(LABEL_W), row(0), name_w, s(LINE));
    let mut x = x0 + s(LABEL_W) + name_w + s(PAD);
    for (label, edit) in [(ID_SPACING_LABEL, ID_SPACING), (ID_MARGIN_LABEL, ID_MARGIN)] {
        place(label, x, row(0), s(LABEL_W), s(LINE));
        place(edit, x + s(LABEL_W), row(0), s(SMALL_W), s(LINE));
        x += small;
    }
    place(ID_TEMPLATE_LABEL, x0, row(1), s(LABEL_W), s(LINE));
    // Combo boxes are sized with their drop-down list included.
    place(ID_TEMPLATE, x0 + s(LABEL_W), row(1), s(COMBO_W), s(LINE) * 8);
    let x = x0 + s(LABEL_W) + s(COMBO_W) + s(PAD);
    place(ID_COUNT_LABEL, x, row(1), s(LABEL_W), s(LINE));
    place(ID_COUNT, x + s(LABEL_W), row(1), s(SMALL_W), s(LINE));
    let x = x + s(LABEL_W) + s(SMALL_W) + s(PAD);
    place(ID_APPLY, x, row(1), s(BUTTON_W) * 2, s(LINE));

    // The drawing, as large as fits in the display's shape.
    let top = row(2) + s(PAD / 2);
    let (aw, ah) = ((right - x0).max(1), (bottom - s(PAD) - top).max(1));
    let (cw, ch) = if aw as f32 / ah as f32 > st.aspect {
        ((ah as f32 * st.aspect) as i32, ah)
    } else {
        (aw, (aw as f32 / st.aspect) as i32)
    };
    let (cx, cy) = (x0 + (aw - cw) / 2, top + (ah - ch) / 2);
    st.canvas = RECT {
        left: cx,
        top: cy,
        right: cx + cw,
        bottom: cy + ch,
    };

    let buttons = s(BUTTON_W) * 2 + s(PAD);
    place(ID_STATUS, s(PAD), bottom, w - s(PAD) * 3 - buttons, s(LINE));
    place(ID_CLOSE, w - s(PAD) - buttons, bottom, s(BUTTON_W), s(LINE));
    place(ID_SAVE, w - s(PAD) - s(BUTTON_W), bottom, s(BUTTON_W), s(LINE));
    invalidate(hwnd, None);
}

/// Fill the list and the boxes from the layouts.
fn show_layouts(hwnd: HWND, st: &mut State) {
    st.quiet = true;
    let list = item(hwnd, ID_LIST);
    send(list, LB_RESETCONTENT, 0, 0);
    for l in &st.layouts {
        let t = wide(&l.name);
        send(list, LB_ADDSTRING, 0, t.as_ptr() as isize);
    }
    send(list, LB_SETCURSEL, st.current, 0);
    show_current(hwnd, st);
    st.quiet = false;
}

fn show_current(hwnd: HWND, st: &mut State) {
    let quiet = std::mem::replace(&mut st.quiet, true);
    let (name, spacing, margin) = st
        .layouts
        .get(st.current)
        .map(|l| (l.name.clone(), l.spacing.to_string(), l.margin.to_string()))
        .unwrap_or_default();
    set_text(item(hwnd, ID_NAME), &name);
    set_text(item(hwnd, ID_SPACING), &spacing);
    set_text(item(hwnd, ID_MARGIN), &margin);
    st.hover = None;
    st.drag = None;
    st.quiet = quiet;
    invalidate(hwnd, Some(&st.canvas));
}

/// Rename the list row of the current layout.
fn relabel(hwnd: HWND, st: &State) {
    let list = item(hwnd, ID_LIST);
    let Some(l) = st.layouts.get(st.current) else { return };
    let t = wide(&l.name);
    send(list, LB_DELETESTRING, st.current, 0);
    send(list, LB_INSERTSTRING, st.current, t.as_ptr() as isize);
    send(list, LB_SETCURSEL, st.current, 0);
}

/// The zones of the template and count chosen above the drawing.
fn template_zones(hwnd: HWND, st: &State) -> Option<Vec<ZoneRect>> {
    let t = usize::try_from(send(item(hwnd, ID_TEMPLATE), CB_GETCURSEL, 0, 0)).ok()?;
    let by_count = &st.templates.get(t)?.by_count;
    let n = text_of(item(hwnd, ID_COUNT)).trim().parse::<usize>().unwrap_or(3);
    by_count.get(n.clamp(1, by_count.len()) - 1).cloned()
}

fn new_id(layouts: &[ZoneLayout]) -> String {
    (1..)
        .map(|n| format!("custom-{n}"))
        .find(|id| !layouts.iter().any(|l| l.id == *id))
        .expect("an unused number exists")
}

fn status(hwnd: HWND, text: &str) {
    set_text(item(hwnd, ID_STATUS), text);
}

fn changed(hwnd: HWND, st: &mut State) {
    st.dirty = true;
    invalidate(hwnd, Some(&st.canvas));
}

fn on_command(hwnd: HWND, st: &mut State, id: i32, code: u32) {
    match (id, code) {
        (ID_LIST, LBN_SELCHANGE) => {
            if let Ok(i) = usize::try_from(send(item(hwnd, ID_LIST), LB_GETCURSEL, 0, 0)) {
                st.current = i.min(st.layouts.len().saturating_sub(1));
                show_current(hwnd, st);
            }
        }
        (ID_NAME, EN_CHANGE) if !st.quiet => {
            let name = text_of(item(hwnd, ID_NAME));
            if let Some(l) = st.layout() {
                l.name = name;
                st.dirty = true;
                relabel(hwnd, st);
            }
        }
        (ID_SPACING | ID_MARGIN, EN_CHANGE) if !st.quiet => {
            if let Ok(v) = text_of(item(hwnd, id)).trim().parse::<u32>() {
                if let Some(l) = st.layout() {
                    if id == ID_SPACING {
                        l.spacing = v.min(64);
                    } else {
                        l.margin = v.min(64);
                    }
                    changed(hwnd, st);
                }
            }
        }
        (ID_NEW, BN_CLICKED) => {
            let Some(zones) = template_zones(hwnd, st) else { return };
            let n = st.layouts.len() + 1;
            st.layouts.push(ZoneLayout {
                id: new_id(&st.layouts),
                name: format!("Custom layout {n}"),
                zones,
                spacing: 8,
                margin: 8,
            });
            st.current = st.layouts.len() - 1;
            st.dirty = true;
            show_layouts(hwnd, st);
        }
        (ID_COPY, BN_CLICKED) => {
            let Some(mut l) = st.layouts.get(st.current).cloned() else {
                return;
            };
            l.id = new_id(&st.layouts);
            l.name = format!("{} copy", l.name);
            st.layouts.insert(st.current + 1, l);
            st.current += 1;
            st.dirty = true;
            show_layouts(hwnd, st);
        }
        (ID_DELETE, BN_CLICKED) => {
            if st.layouts.len() <= 1 {
                return status(hwnd, "Keep at least one layout.");
            }
            st.layouts.remove(st.current);
            st.current = st.current.min(st.layouts.len() - 1);
            st.dirty = true;
            show_layouts(hwnd, st);
        }
        (ID_APPLY, BN_CLICKED) => {
            let Some(zones) = template_zones(hwnd, st) else { return };
            if let Some(l) = st.layout() {
                l.zones = zones;
                changed(hwnd, st);
            }
        }
        (ID_SAVE, BN_CLICKED) => {
            st.dirty = false;
            push(Event::ZoneLayouts(st.layouts.clone()));
        }
        (ID_CLOSE, BN_CLICKED) => {
            // SAFETY: own window; handled by the WM_CLOSE arm.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        _ => {}
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

fn snap(v: u32) -> u32 {
    (v + SNAP / 2) / SNAP * SNAP
}

fn on_mouse(hwnd: HWND, st: &mut State, msg: u32, wparam: WPARAM, lparam: LPARAM) {
    let (x, y) = point(lparam);
    let at = st.units(x, y);
    let grab = st.grab_units();
    match msg {
        WM_LBUTTONDOWN => {
            let Some((ux, uy)) = at else { return };
            let Some(l) = st.layouts.get_mut(st.current) else {
                return;
            };
            if let Some(s) = zone_layout::split_at(&l.zones, ux, uy, grab) {
                st.drag = Some(s);
                // SAFETY: own window.
                unsafe { SetCapture(hwnd) };
            } else if let Some(i) = zone_layout::zone_at(&l.zones, ux, uy) {
                let across = wparam.0 & MK_SHIFT != 0;
                let cut = if across { snap(uy) } else { snap(ux) };
                if zone_layout::split_zone(&mut l.zones, i, cut, !across) {
                    changed(hwnd, st);
                } else {
                    status(hwnd, "That zone is too small to split there.");
                }
            }
        }
        WM_MOUSEMOVE => {
            if let Some(mut s) = st.drag.take() {
                let c = st.canvas;
                let to = if s.vertical {
                    (x - c.left).max(0) as i64 * ZONE_UNITS as i64 / (c.right - c.left).max(1) as i64
                } else {
                    (y - c.top).max(0) as i64 * ZONE_UNITS as i64 / (c.bottom - c.top).max(1) as i64
                };
                if let Some(l) = st.layouts.get_mut(st.current) {
                    zone_layout::move_split(&mut l.zones, &mut s, snap(to as u32));
                }
                st.drag = Some(s);
                changed(hwnd, st);
            } else {
                let hover = at.and_then(|(ux, uy)| {
                    let l = st.layouts.get(st.current)?;
                    zone_layout::split_at(&l.zones, ux, uy, grab)
                });
                if hover != st.hover {
                    st.hover = hover;
                    invalidate(hwnd, Some(&st.canvas));
                }
            }
        }
        WM_LBUTTONUP => {
            if st.drag.take().is_some() {
                // SAFETY: releases the capture taken on button down.
                let _ = unsafe { ReleaseCapture() };
                invalidate(hwnd, Some(&st.canvas));
            }
        }
        WM_RBUTTONDOWN => {
            let Some((ux, uy)) = at else { return };
            let Some(l) = st.layouts.get_mut(st.current) else {
                return;
            };
            let Some(i) = zone_layout::zone_at(&l.zones, ux, uy) else {
                return;
            };
            if l.zones.len() <= 1 {
                return status(hwnd, "A layout needs at least one zone.");
            }
            l.zones.remove(i);
            st.hover = None;
            changed(hwnd, st);
        }
        _ => {}
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn paint(hwnd: HWND, st: &State) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: BeginPaint/EndPaint pair on own window; the memory DC, bitmap, brushes
    // and font are created and freed here.
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let (cw, ch) = (client.right.max(1), client.bottom.max(1));
        let dc = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, cw, ch);
        let old = SelectObject(dc, bmp.into());
        FillRect(dc, &client, GetSysColorBrush(COLOR_BTNFACE));
        let fill = |r: &RECT, c: COLORREF| {
            let b = CreateSolidBrush(c);
            FillRect(dc, r, b);
            let _ = DeleteObject(b.into());
        };
        fill(&st.canvas, rgb(43, 43, 43));
        let font = make_font(20, st.dpi, FW_SEMIBOLD.0 as i32);
        let old_font = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, rgb(0, 60, 116));
        if let Some(l) = st.layouts.get(st.current) {
            // Gaps as they will look on the display, at least a line so zones stay apart.
            let work_w = (st.canvas.right - st.canvas.left).max(1) as f32;
            let gap = ((l.spacing as f32 * work_w / 1920.0) / 2.0).max(1.0) as i32 + 1;
            for (i, z) in l.zones.iter().enumerate() {
                let mut r = st.zone_rect(z);
                r.left += gap;
                r.top += gap;
                r.right -= gap;
                r.bottom -= gap;
                fill(&r, rgb(0, 103, 192));
                let mut inner = RECT {
                    left: r.left + 2,
                    top: r.top + 2,
                    right: r.right - 2,
                    bottom: r.bottom - 2,
                };
                fill(&inner, rgb(204, 228, 247));
                let mut t = wide(&(i + 1).to_string());
                let len = t.len() - 1;
                DrawTextW(
                    dc,
                    &mut t[..len],
                    &mut inner,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
            if let Some(s) = st.drag.as_ref().or(st.hover.as_ref()) {
                let rects = s.before.iter().chain(&s.after).filter_map(|&i| l.zones.get(i));
                let (mut lo, mut hi) = (u32::MAX, 0);
                for z in rects {
                    let (a, n) = if s.vertical { (z[1], z[3]) } else { (z[0], z[2]) };
                    lo = lo.min(a);
                    hi = hi.max(a + n);
                }
                let line = if s.vertical {
                    st.zone_rect(&[s.at, lo, 0, hi.saturating_sub(lo)])
                } else {
                    st.zone_rect(&[lo, s.at, hi.saturating_sub(lo), 0])
                };
                let th = scale(2, st.dpi);
                let bar = RECT {
                    left: line.left - th,
                    top: line.top - th,
                    right: line.right + th,
                    bottom: line.bottom + th,
                };
                fill(&bar, rgb(255, 140, 0));
            }
        }
        SelectObject(dc, old_font);
        let _ = DeleteObject(font.into());
        let _ = BitBlt(hdc, 0, 0, cw, ch, Some(dc), 0, 0, SRCCOPY);
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Ask whether to keep unsaved changes; false when closing should stop.
fn confirm_close(hwnd: HWND, st: &mut State) -> bool {
    if !st.dirty {
        return true;
    }
    let text = wide("Save the changes to your layouts?");
    let title = wide("FancyZones layouts");
    // SAFETY: strings outlive the modal call; `hwnd` is our window.
    let answer = unsafe {
        MessageBoxW(
            Some(hwnd),
            pcwstr(&text),
            pcwstr(&title),
            MB_YESNOCANCEL | MB_ICONQUESTION,
        )
    };
    match answer {
        IDYES => {
            push(Event::ZoneLayouts(st.layouts.clone()));
            st.dirty = false;
            true
        }
        IDNO => {
            st.dirty = false;
            true
        }
        _ => false,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: `hwnd` is a zone editor window (this is its class's procedure).
    let st = unsafe { state(hwnd) };
    match (msg, st) {
        (WM_SIZE, Some(st)) => {
            place_all(hwnd, st);
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
        (WM_ERASEBKGND, _) => LRESULT(1),
        (WM_PAINT, Some(st)) => {
            paint(hwnd, st);
            LRESULT(0)
        }
        (WM_COMMAND, Some(st)) => {
            let (id, code) = ((wparam.0 & 0xFFFF) as i32, ((wparam.0 >> 16) & 0xFFFF) as u32);
            on_command(hwnd, st, id, code);
            LRESULT(0)
        }
        (WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE | WM_RBUTTONDOWN, Some(st)) => {
            on_mouse(hwnd, st, msg, wparam, lparam);
            LRESULT(0)
        }
        (WM_SETCURSOR, Some(st)) if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            match st.drag.as_ref().or(st.hover.as_ref()) {
                Some(s) => {
                    let id = if s.vertical { IDC_SIZEWE } else { IDC_SIZENS };
                    // SAFETY: system cursors need no instance and are never freed.
                    unsafe { SetCursor(LoadCursorW(None, id).ok()) };
                    LRESULT(1)
                }
                // SAFETY: forwarding the unmodified arguments of this callback.
                None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            }
        }
        (WM_CLOSE, Some(st)) => {
            if confirm_close(hwnd, st) {
                // SAFETY: own window. The return value is the previous visibility.
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
            LRESULT(0)
        }
        (WM_GETMINMAXINFO, Some(st)) => {
            // SAFETY: WM_GETMINMAXINFO carries a MINMAXINFO to fill in.
            let mm = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mm.ptMinTrackSize.x = scale(WIDTH * 3 / 4, st.dpi);
            mm.ptMinTrackSize.y = scale(HEIGHT * 3 / 4, st.dpi);
            LRESULT(0)
        }
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Width over height of the main display's work area.
fn display_aspect() -> f32 {
    crate::monitor::enumerate()
        .ok()
        .and_then(|ms| ms.into_iter().find(|m| m.primary))
        .map(|m| {
            let a = m.work_area;
            (a.right - a.left) as f32 / (a.bottom - a.top).max(1) as f32
        })
        .filter(|a| a.is_finite() && *a > 0.2)
        .unwrap_or(16.0 / 9.0)
}

pub struct ZoneEditor {
    hwnd: HWND,
    state: Box<State>,
    _not_send: PhantomData<*const ()>,
}

impl ZoneEditor {
    /// Creates the window hidden; `templates` fill the template drop-down.
    pub fn new(templates: Vec<ZoneTemplate>) -> Result<ZoneEditor> {
        let class = wide(CLASS_NAME);
        let title = wide("FancyZones layouts");
        // SAFETY: strings outlive the calls; `wndproc` matches its callback contract.
        // The state pointer stays valid until `Drop` clears it.
        unsafe {
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
            .context("CreateWindowEx(zone editor)")?;
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
                .context("CreateWindowEx(zone editor control)")
            };
            let created = (|| -> Result<()> {
                let label = SS_NOPREFIX | SS_CENTERIMAGE;
                let edit = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
                let number = edit | ES_NUMBER as u32;
                let button = WS_TABSTOP.0 | BS_PUSHBUTTON as u32;
                let none = WINDOW_EX_STYLE(0);
                child(
                    "LISTBOX",
                    "",
                    WS_TABSTOP.0 | WS_VSCROLL.0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
                    WS_EX_CLIENTEDGE,
                    ID_LIST,
                )?;
                child("BUTTON", "New", button, none, ID_NEW)?;
                child("BUTTON", "Copy", button, none, ID_COPY)?;
                child("BUTTON", "Delete", button, none, ID_DELETE)?;
                child("STATIC", "Name", label, none, ID_NAME_LABEL)?;
                child("EDIT", "", edit, WS_EX_CLIENTEDGE, ID_NAME)?;
                child("STATIC", "Spacing", label, none, ID_SPACING_LABEL)?;
                child("EDIT", "", number, WS_EX_CLIENTEDGE, ID_SPACING)?;
                child("STATIC", "Margin", label, none, ID_MARGIN_LABEL)?;
                child("EDIT", "", number, WS_EX_CLIENTEDGE, ID_MARGIN)?;
                child("STATIC", "Template", label, none, ID_TEMPLATE_LABEL)?;
                let combo = child(
                    "COMBOBOX",
                    "",
                    WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32,
                    none,
                    ID_TEMPLATE,
                )?;
                for t in &templates {
                    let w = wide(&t.name);
                    send(combo, CB_ADDSTRING, 0, w.as_ptr() as isize);
                }
                send(combo, CB_SETCURSEL, 0, 0);
                child("STATIC", "Zones", label, none, ID_COUNT_LABEL)?;
                child("EDIT", "3", number, WS_EX_CLIENTEDGE, ID_COUNT)?;
                child("BUTTON", "Use template", button, none, ID_APPLY)?;
                child("STATIC", HINT, SS_NOPREFIX | SS_CENTERIMAGE, none, ID_STATUS)?;
                child("BUTTON", "Close", button, none, ID_CLOSE)?;
                child("BUTTON", "Save", WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, none, ID_SAVE)?;
                Ok(())
            })();
            if let Err(e) = created {
                let _ = DestroyWindow(hwnd);
                return Err(e);
            }
            let mut state = Box::new(State {
                dpi,
                font: make_font(FONT_PT, dpi, FW_NORMAL.0 as i32),
                quiet: true,
                layouts: Vec::new(),
                current: 0,
                templates,
                canvas: RECT::default(),
                aspect: display_aspect(),
                drag: None,
                hover: None,
                dirty: false,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            let dpi = GetDpiForWindow(hwnd).max(96);
            apply_font(hwnd, &mut state, dpi);
            place_all(hwnd, &mut state);
            Ok(ZoneEditor {
                hwnd,
                state,
                _not_send: PhantomData,
            })
        }
    }

    /// Show `layouts` as saved, with the one at `selected` chosen (or the one chosen
    /// before when `None`).
    pub fn set_layouts(&mut self, layouts: Vec<ZoneLayout>, selected: Option<usize>) {
        let st = &mut *self.state;
        st.current = selected.unwrap_or(st.current).min(layouts.len().saturating_sub(1));
        st.layouts = layouts;
        st.dirty = false;
        show_layouts(self.hwnd, st);
    }

    /// Whether there are changes not yet saved.
    pub fn is_dirty(&self) -> bool {
        self.state.dirty
    }

    pub fn show(&mut self) {
        self.state.aspect = display_aspect();
        place_all(self.hwnd, &mut self.state);
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

    pub fn set_status(&mut self, text: &str) {
        status(self.hwnd, text);
    }

    /// Show `ico` in the title bar and the taskbar.
    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }
}

impl Drop for ZoneEditor {
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
    fn new_ids_skip_used_ones() {
        let l = |id: &str| ZoneLayout {
            id: id.into(),
            name: String::new(),
            zones: Vec::new(),
            spacing: 0,
            margin: 0,
        };
        assert_eq!(new_id(&[]), "custom-1");
        assert_eq!(new_id(&[l("custom-1"), l("custom-3")]), "custom-2");
    }

    #[test]
    fn splits_snap_to_half_percents() {
        assert_eq!(snap(4_024), 4_000);
        assert_eq!(snap(4_026), 4_050);
    }
}
