//! The settings window, drawn in the Windows 11 style: a navigation pane on the
//! left and a page of setting cards on the right, with toggle switches, shortcut
//! key caps, drop-downs and text boxes, in light or dark.
//!
//! Everything but the text boxes and the walkthrough is drawn by the window itself.
//! It only shows what its owner gives it and reports what the user does as
//! [`SettingsEvent`]s on the event loop. Closing it hides it.

mod layout;
mod model;
mod painter;
mod palette;

pub use model::{Control, Group, Item, LayoutChoice, NavEntry, Page, Severity};

use crate::event_loop::push;
use crate::guide_view::{self, Scene};
use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use layout::{DialogButton, Hit, Input, ItemRef, Layout, Look, Node, Part, RADIUS};
use painter::{Align, Face, Fonts, Measure, Painter, Rect, Style};
use palette::{over, Palette};
use powerkit_core::{Event, Mods, SettingValue, SettingsEvent, Shortcut};
use std::cell::Cell;
use std::collections::HashSet;
use std::marker::PhantomData;
use windows::core::BOOL;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};
use windows::Win32::UI::Controls::{EM_SETCUEBANNER, EM_SETSEL, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "PowerKit.Settings";
const ID_GUIDE: i32 = 6;
const ID_FIELD: i32 = 100;
const SUBCLASS_ID: usize = 1;
/// A key seen by the keyboard hook while the shortcut dialog is open:
/// wparam is the key message, lparam the virtual-key code.
const WM_APP_KEY: u32 = WM_APP + 1;
// Sizes in 96-DPI pixels.
const WIDTH: i32 = 1180;
const HEIGHT: i32 = 780;
const MIN_W: i32 = 520;
const MIN_H: i32 = 420;
const WHEEL_STEP: f32 = 48.0;
const SCROLLBAR_W: f32 = 12.0;
const EDIT_FONT_PX: i32 = 14;

/// A text box over a field drawn on the page.
struct Field {
    key: String,
    at: ItemRef,
    hwnd: HWND,
    multiline: bool,
    /// The value last given by the owner; a commit sends only a change from it.
    value: String,
}

/// The shortcut dialog while it is open.
struct Recording {
    key: String,
    default: String,
    held: Mods,
    captured: Option<Shortcut>,
    nodes: Vec<Node>,
}

struct State {
    hwnd: HWND,
    k: f32,
    dark_pref: Option<bool>,
    palette: Palette,
    fonts: Fonts,
    edit_font: HFONT,
    /// Brushes for the text boxes' backgrounds: normal and focused.
    field_brushes: (HBRUSH, HBRUSH),
    nav: Vec<NavEntry>,
    folded: HashSet<usize>,
    selected: usize,
    page: Page,
    expanded: HashSet<String>,
    info: Option<(Severity, String)>,
    has_scene: bool,
    layout: Layout,
    scroll: f32,
    nav_scroll: f32,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    scroll_drag: Option<(f32, f32)>,
    over_scrollbar: bool,
    tracking: bool,
    /// Where the keyboard focus is when it is not in a text box; drawn only after
    /// the keyboard was used.
    focus: Option<Hit>,
    focus_visible: bool,
    fields: Vec<Field>,
    guide: HWND,
    recording: Option<Recording>,
    hook: Option<HHOOK>,
}

thread_local! {
    /// The settings window the keyboard hook reports to, while it records a shortcut.
    static RECORDER: Cell<isize> = const { Cell::new(0) };
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

fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: only used with messages whose pointer arguments outlive the call.
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

fn colorref(argb: u32) -> COLORREF {
    COLORREF((argb & 0xFF) << 16 | (argb & 0xFF00) | (argb >> 16) & 0xFF)
}

fn make_edit_font(k: f32) -> HFONT {
    let face = wide("Segoe UI");
    // SAFETY: `face` outlives the call.
    unsafe {
        CreateFontW(
            -((EDIT_FONT_PX as f32 * k).round() as i32),
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

fn text_of(hwnd: HWND) -> String {
    // SAFETY: the buffer is sized from the reported length.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn set_text(hwnd: HWND, s: &str) {
    let w = wide(s);
    // SAFETY: `w` outlives the call.
    let _ = unsafe { SetWindowTextW(hwnd, pcwstr(&w)) };
}

/// The opaque color behind a text box, for the edit control to paint with.
fn field_color(p: &Palette, focused: bool) -> u32 {
    let card = over(over(p.window, p.layer), p.card);
    over(card, if focused { p.field_focus } else { p.control })
}

impl State {
    fn dark(&self) -> bool {
        self.dark_pref
            .unwrap_or_else(|| !crate::theme::is_light(false).unwrap_or(true))
    }

    /// Re-read the theme and accent, and color the title bar to match.
    fn apply_theme(&mut self) {
        self.palette = Palette::current(self.dark());
        // SAFETY: own window; plain values passed by pointer for the call only.
        unsafe {
            let dark = BOOL::from(self.palette.dark);
            if let Err(e) = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const BOOL as *const _,
                std::mem::size_of::<BOOL>() as u32,
            ) {
                tracing::debug!("dark title bar: {e}");
            }
            let caption = colorref(self.palette.window);
            // Fails before Windows 11, where the title bar keeps its own color.
            let _ = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_CAPTION_COLOR,
                &caption as *const COLORREF as *const _,
                std::mem::size_of::<COLORREF>() as u32,
            );
            for b in [self.field_brushes.0, self.field_brushes.1] {
                if !b.is_invalid() {
                    let _ = DeleteObject(b.into());
                }
            }
            self.field_brushes = (
                CreateSolidBrush(colorref(field_color(&self.palette, false))),
                CreateSolidBrush(colorref(field_color(&self.palette, true))),
            );
        }
        guide_view::set_colors(
            self.guide,
            guide_view::Colors {
                back: over(over(self.palette.window, self.palette.layer), self.palette.card),
                ink: self.palette.text2 | 0xFF00_0000,
                accent: self.palette.accent,
            },
        );
    }

    fn client(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        // SAFETY: own window.
        let _ = unsafe { GetClientRect(self.hwnd, &mut rc) };
        (rc.right as f32, rc.bottom as f32)
    }

    fn relayout(&mut self) {
        let (w, h) = self.client();
        let k = self.k;
        let guide_h = |gw: f32| guide_view::height_for(gw as i32, (k * 96.0) as u32) as f32;
        // SAFETY: own window; the DC is released below.
        let hdc = unsafe { GetDC(Some(self.hwnd)) };
        let layout = {
            // SAFETY: `hdc` stays valid until it is released after this block.
            match unsafe { Painter::new(hdc, &mut self.fonts) } {
                Some(mut p) => {
                    let input = Input {
                        nav: &self.nav,
                        folded: &self.folded,
                        selected: self.selected,
                        page: &self.page,
                        expanded: &self.expanded,
                        info: self.info.as_ref(),
                        guide: self.has_scene.then_some(&guide_h as &dyn Fn(f32) -> f32),
                        width: w,
                        k,
                    };
                    Some(layout::layout(&input, &mut p))
                }
                None => None,
            }
        };
        // SAFETY: obtained above from the same window.
        unsafe { ReleaseDC(Some(self.hwnd), hdc) };
        if let Some(l) = layout {
            self.layout = l;
        }
        if let Some(rec) = &mut self.recording {
            rec.nodes = dialog_nodes(&mut self.fonts, self.hwnd, rec, w, h, k);
        }
        self.clamp_scroll();
        self.sync_children();
        // SAFETY: own window.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn clamp_scroll(&mut self) {
        let (_, h) = self.client();
        self.scroll = self.scroll.clamp(0.0, (self.layout.content_height - h).max(0.0));
        self.nav_scroll = self.nav_scroll.clamp(0.0, (self.layout.nav_height - h).max(0.0));
    }

    /// Create, place and remove the text boxes and walkthrough to match the layout.
    fn sync_children(&mut self) {
        let k = self.k;
        let (_, h) = self.client();
        // Child windows would draw over the shortcut dialog.
        let dialog = self.recording.is_some();
        let visible = |r: Rect| !dialog && r.bottom() > 0.0 && r.y < h;
        // SAFETY: own child windows, on this thread.
        unsafe {
            match self.layout.guide {
                Some(r) => {
                    let r = r.offset(0.0, -self.scroll);
                    let _ = SetWindowPos(
                        self.guide,
                        None,
                        r.x.round() as i32,
                        r.y.round() as i32,
                        r.w.round() as i32,
                        r.h.round() as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    let rgn = CreateRoundRectRgn(
                        0,
                        0,
                        r.w.round() as i32 + 1,
                        r.h.round() as i32 + 1,
                        (8.0 * k) as i32,
                        (8.0 * k) as i32,
                    );
                    // The window owns the region from here on.
                    SetWindowRgn(self.guide, Some(rgn), true);
                    let _ = ShowWindow(self.guide, if visible(r) { SW_SHOWNA } else { SW_HIDE });
                }
                None => {
                    let _ = ShowWindow(self.guide, SW_HIDE);
                }
            }
        }
        let wanted: Vec<(ItemRef, Rect)> = self.layout.fields.clone();
        let mut keep = Vec::new();
        for (at, r) in wanted {
            let Some(item) = at.get(&self.page) else { continue };
            let (value, multiline, placeholder) = match &item.control {
                Control::Text {
                    value,
                    lines,
                    placeholder,
                    ..
                } => (value.clone(), *lines > 1, placeholder.clone()),
                Control::Number { value } => (value.clone(), false, String::new()),
                _ => continue,
            };
            let enabled = item.enabled && at.child.is_none_or(|_| self.parent_enabled(at));
            let key = item.key.clone();
            let existing = self
                .fields
                .iter()
                .position(|f| f.key == key && f.multiline == multiline);
            let field = match existing {
                Some(i) => self.fields.swap_remove(i),
                None => match self.make_field(&key, at, multiline, &placeholder) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!("settings text box: {e}");
                        continue;
                    }
                },
            };
            let mut field = field;
            field.at = at;
            // SAFETY: own child window.
            let focused = unsafe { GetFocus() } == field.hwnd;
            if field.value != value || existing.is_none() {
                if !focused || existing.is_none() {
                    set_text(field.hwnd, &value);
                }
                field.value = value;
            }
            let r = r.offset(0.0, -self.scroll);
            let pad_x = 11.0 * k;
            let inner = if multiline {
                r.inset(pad_x, 6.0 * k)
            } else {
                let line = (20.0 * k).round();
                Rect::new(r.x + pad_x, r.y + (r.h - line) / 2.0, r.w - 2.0 * pad_x, line)
            };
            // SAFETY: own child window.
            unsafe {
                let _ = SetWindowPos(
                    field.hwnd,
                    None,
                    inner.x.round() as i32,
                    inner.y.round() as i32,
                    inner.w.round() as i32,
                    inner.h.round() as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = EnableWindow(field.hwnd, enabled);
                let _ = ShowWindow(field.hwnd, if visible(r) { SW_SHOWNA } else { SW_HIDE });
            }
            keep.push(field);
        }
        for gone in self.fields.drain(..) {
            // SAFETY: own child window, no longer shown.
            unsafe {
                let _ = RemoveWindowSubclass(gone.hwnd, Some(field_proc), SUBCLASS_ID);
                let _ = DestroyWindow(gone.hwnd);
            }
        }
        self.fields = keep;
    }

    fn parent_enabled(&self, at: ItemRef) -> bool {
        ItemRef { child: None, ..at }.get(&self.page).is_none_or(|p| p.enabled)
    }

    fn make_field(&self, key: &str, at: ItemRef, multiline: bool, placeholder: &str) -> Result<Field> {
        let class = wide("EDIT");
        let style = if multiline {
            ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN
        } else {
            ES_AUTOHSCROLL
        };
        // SAFETY: strings outlive the calls; the subclass gets the parent window as
        // its data and is removed before the edit control is destroyed.
        unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                pcwstr(&class),
                None,
                WS_CHILD | WS_TABSTOP | WINDOW_STYLE(style as u32),
                0,
                0,
                0,
                0,
                Some(self.hwnd),
                Some(HMENU(ID_FIELD as isize as *mut _)),
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(settings text box)")?;
            send(hwnd, WM_SETFONT, self.edit_font.0 as usize, 0);
            if !placeholder.is_empty() {
                let cue = wide(placeholder);
                send(hwnd, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
            }
            if !SetWindowSubclass(hwnd, Some(field_proc), SUBCLASS_ID, self.hwnd.0 as usize).as_bool() {
                let _ = DestroyWindow(hwnd);
                return Err(Error::Other("SetWindowSubclass(settings text box) failed".into()));
            }
            Ok(Field {
                key: key.to_string(),
                at,
                hwnd,
                multiline,
                value: String::new(),
            })
        }
    }

    /// Send the text box's value when it changed.
    fn commit(&mut self, edit: HWND) {
        let Some(f) = self.fields.iter_mut().find(|f| f.hwnd == edit) else {
            return;
        };
        let text = text_of(edit);
        let text = if f.multiline { text.replace("\r\n", "\n") } else { text };
        if text != f.value {
            f.value = text.clone();
            push(Event::Settings(SettingsEvent::Set {
                key: f.key.clone(),
                value: SettingValue::Text(text),
            }));
        }
    }

    fn revert(&mut self, edit: HWND) {
        if let Some(f) = self.fields.iter().find(|f| f.hwnd == edit) {
            set_text(edit, &f.value);
        }
    }

    /// What is under the window point `(x, y)`.
    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some(rec) = &self.recording {
            return layout::hit_test(&rec.nodes, x, y);
        }
        if x < self.layout.nav_w {
            layout::hit_test(&self.layout.nav, x, y + self.nav_scroll)
        } else {
            layout::hit_test(&self.layout.content, x, y + self.scroll)
        }
    }

    fn scrollbar(&self) -> Option<(Rect, Rect)> {
        let (w, h) = self.client();
        let total = self.layout.content_height;
        if total <= h || h <= 0.0 {
            return None;
        }
        let track = Rect::new(w - SCROLLBAR_W * self.k, 0.0, SCROLLBAR_W * self.k, h);
        let thumb_h = (h * h / total).max(32.0 * self.k);
        let y = (h - thumb_h) * self.scroll / (total - h);
        Some((track, Rect::new(track.x, y, track.w, thumb_h)))
    }

    fn scroll_to(&mut self, y: f32) {
        let old = self.scroll;
        self.scroll = y;
        self.clamp_scroll();
        if self.scroll != old {
            self.sync_children();
            // SAFETY: own window.
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        }
    }

    /// Everything the keyboard can reach, in order.
    fn focusable(&self) -> Vec<Hit> {
        let mut out: Vec<Hit> = Vec::new();
        if let Some(rec) = &self.recording {
            return rec.nodes.iter().filter_map(|n| n.hit).collect();
        }
        for n in self.layout.nav.iter().chain(&self.layout.content) {
            if let Some(h) = n.hit {
                if !out.contains(&h) {
                    out.push(h);
                }
            }
            if let Look::Field(at) = n.look {
                out.push(Hit::Item(at, Part::Field));
            }
        }
        out
    }

    fn move_focus(&mut self, back: bool) {
        let all = self.focusable();
        if all.is_empty() {
            return;
        }
        // SAFETY: plain query.
        let in_field = unsafe { GetFocus() };
        let current = self
            .fields
            .iter()
            .find(|f| f.hwnd == in_field)
            .map(|f| Hit::Item(f.at, Part::Field))
            .or(self.focus);
        let i = current.and_then(|c| all.iter().position(|h| *h == c));
        let n = all.len();
        let next = match (i, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        };
        self.focus_visible = true;
        self.set_focus(all[next]);
    }

    fn set_focus(&mut self, hit: Hit) {
        self.focus = Some(hit);
        // SAFETY: own windows.
        unsafe {
            if let Hit::Item(at, Part::Field) = hit {
                if let Some(f) = self.fields.iter().find(|f| f.at == at) {
                    let _ = SetFocus(Some(f.hwnd));
                    send(f.hwnd, EM_SETSEL, 0, -1);
                }
            } else {
                let _ = SetFocus(Some(self.hwnd));
            }
        }
        self.reveal(hit);
        // SAFETY: own window.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Scroll so that `hit` is on screen.
    fn reveal(&mut self, hit: Hit) {
        let (_, h) = self.client();
        let rect = self
            .layout
            .content
            .iter()
            .find(|n| n.hit == Some(hit))
            .map(|n| n.rect)
            .or_else(|| match hit {
                Hit::Item(at, Part::Field) => self.layout.fields.iter().find(|(a, _)| *a == at).map(|(_, r)| *r),
                _ => None,
            });
        if let Some(r) = rect {
            let margin = 16.0 * self.k;
            if r.y - margin < self.scroll {
                self.scroll_to(r.y - margin);
            } else if r.bottom() + margin > self.scroll + h {
                self.scroll_to(r.bottom() + margin - h);
            }
        }
    }

    fn activate(&mut self, hit: Hit) {
        match hit {
            Hit::Nav(i) => match self.nav.get(i) {
                Some(NavEntry::Category { .. }) => {
                    if !self.folded.remove(&i) {
                        self.folded.insert(i);
                    }
                    self.relayout();
                }
                Some(NavEntry::Page { .. }) => {
                    self.selected = i;
                    push(Event::Settings(SettingsEvent::Page(i)));
                    // SAFETY: own window.
                    let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
                }
                None => {}
            },
            Hit::Item(at, part) => {
                let Some(item) = at.get(&self.page).cloned() else {
                    return;
                };
                let set = |value| {
                    push(Event::Settings(SettingsEvent::Set {
                        key: item.key.clone(),
                        value,
                    }))
                };
                match (part, &item.control) {
                    (Part::Toggle, Control::Toggle(on)) => set(SettingValue::Bool(!on)),
                    (Part::Expand, _) => {
                        if !self.expanded.remove(&item.key) {
                            self.expanded.insert(item.key.clone());
                        }
                        self.relayout();
                        self.reveal(hit);
                    }
                    (Part::Shortcut, Control::Shortcut { value, default }) => {
                        self.start_recording(&item.key, value, default)
                    }
                    (Part::Choice, Control::Choice { options, selected }) => {
                        if let Some(v) = self.choose(hit, options, selected) {
                            set(SettingValue::Text(v));
                        }
                    }
                    (Part::Color, Control::Color(c)) => {
                        if let Some(v) = self.pick_color(c) {
                            set(SettingValue::Text(v));
                        }
                    }
                    (Part::Button | Part::Link, _) => push(Event::Settings(SettingsEvent::Action(item.key.clone()))),
                    (Part::Field, _) => self.set_focus(hit),
                    (Part::Pick(i), Control::Layouts { options, .. }) => {
                        if let Some(o) = options.get(i) {
                            set(SettingValue::Text(o.id.clone()));
                        }
                    }
                    _ => {}
                }
            }
            Hit::CloseInfo => {
                self.info = None;
                self.relayout();
            }
            Hit::Dialog(b) => self.finish_recording(b),
        }
    }

    /// Show the options of a drop-down under it; the value chosen, if any.
    fn choose(&self, hit: Hit, options: &[(String, String)], selected: &str) -> Option<String> {
        let r = self.layout.content.iter().find(|n| n.hit == Some(hit))?.rect;
        // SAFETY: the menu is made and destroyed here; strings outlive their calls.
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            for (i, (value, label)) in options.iter().enumerate() {
                let w = wide(label);
                let flags = if value == selected {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                let _ = AppendMenuW(menu, flags, i + 1, pcwstr(&w));
            }
            let mut pt = POINT {
                x: r.x as i32,
                y: (r.bottom() - self.scroll) as i32 + 2,
            };
            let _ = ClientToScreen(self.hwnd, &mut pt);
            let cmd = TrackPopupMenuEx(
                menu,
                (TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN).0,
                pt.x,
                pt.y,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let i = cmd.0 as usize;
            (i >= 1).then(|| options.get(i - 1).map(|(v, _)| v.clone())).flatten()
        }
    }

    /// The Windows color dialog, starting at `current`; the color chosen, if any.
    fn pick_color(&self, current: &str) -> Option<String> {
        thread_local! {
            static CUSTOM: Cell<[COLORREF; 16]> = const { Cell::new([COLORREF(0x00FF_FFFF); 16]) };
        }
        let rgb = u32::from_str_radix(current.trim().trim_start_matches('#'), 16).unwrap_or(0);
        let mut custom = CUSTOM.get();
        let mut cc = CHOOSECOLORW {
            lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: self.hwnd,
            rgbResult: colorref(rgb),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        // SAFETY: `cc` and the custom colors outlive the modal call.
        let ok = unsafe { ChooseColorW(&mut cc) }.as_bool();
        CUSTOM.set(custom);
        ok.then(|| {
            let c = cc.rgbResult.0;
            format!("#{:02X}{:02X}{:02X}", c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF)
        })
    }

    fn start_recording(&mut self, key: &str, current: &str, default: &str) {
        let (w, h) = self.client();
        let mut rec = Recording {
            key: key.to_string(),
            default: default.to_string(),
            held: Mods::NONE,
            captured: current.parse().ok(),
            nodes: Vec::new(),
        };
        rec.nodes = dialog_nodes(&mut self.fonts, self.hwnd, &rec, w, h, self.k);
        self.recording = Some(rec);
        self.sync_children();
        // SAFETY: the hook procedure only posts to this window, and is removed in
        // `stop_recording` or when the window goes.
        unsafe {
            RECORDER.set(self.hwnd.0 as isize);
            match GetModuleHandleW(None)
                .and_then(|m| SetWindowsHookExW(WH_KEYBOARD_LL, Some(record_hook), Some(m.into()), 0))
            {
                Ok(hk) => self.hook = Some(hk),
                Err(e) => tracing::warn!("cannot record shortcuts: {e}"),
            }
            let _ = SetFocus(Some(self.hwnd));
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        self.focus = None;
    }

    fn stop_recording(&mut self) {
        if let Some(hk) = self.hook.take() {
            // SAFETY: a hook this window installed.
            let _ = unsafe { UnhookWindowsHookEx(hk) };
        }
        RECORDER.set(0);
        self.recording = None;
        self.focus = None;
        self.sync_children();
        // SAFETY: own window.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn finish_recording(&mut self, b: DialogButton) {
        let Some(rec) = &self.recording else { return };
        let value = match b {
            DialogButton::Save => rec.captured.filter(Shortcut::is_valid_global).map(|s| s.to_string()),
            DialogButton::Reset => Some(rec.default.clone()),
            DialogButton::Clear => Some(String::new()),
            DialogButton::Cancel => None,
        };
        let key = rec.key.clone();
        if b == DialogButton::Save && value.is_none() {
            return;
        }
        self.stop_recording();
        if let Some(v) = value {
            push(Event::Settings(SettingsEvent::Set {
                key,
                value: SettingValue::Text(v),
            }));
        }
    }

    /// A key reported by the hook while recording.
    fn record_key(&mut self, msg: u32, vk: u32) {
        let Some(rec) = &mut self.recording else { return };
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        if let Some(m) = Mods::from_vk(vk) {
            rec.held = if down { rec.held.union(m) } else { rec.held.without(m) };
            if down && rec.captured.is_some_and(|c| !c.mods.contains(m)) {
                rec.captured = None;
            }
        } else if down {
            if rec.held.is_empty() && vk == u32::from(VK_ESCAPE.0) {
                self.finish_recording(DialogButton::Cancel);
                return;
            }
            if rec.held.is_empty() && vk == u32::from(VK_RETURN.0) {
                self.finish_recording(DialogButton::Save);
                return;
            }
            rec.captured = Some(Shortcut::new(rec.held, vk));
        }
        let (w, h) = self.client();
        let rec = self.recording.as_mut().expect("checked above");
        rec.nodes = dialog_nodes(&mut self.fonts, self.hwnd, rec, w, h, self.k);
        // SAFETY: own window.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        // SAFETY: plain query.
        let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
        let (_, h) = self.client();
        match vk {
            VK_TAB => self.move_focus(shift),
            VK_RETURN | VK_SPACE => {
                if let Some(f) = self.focus {
                    self.activate(f);
                }
            }
            VK_ESCAPE if self.recording.is_some() => self.finish_recording(DialogButton::Cancel),
            VK_UP | VK_DOWN if matches!(self.focus, Some(Hit::Nav(_))) => {
                let rows: Vec<Hit> = self.layout.nav.iter().filter_map(|n| n.hit).collect();
                if let Some(i) = rows.iter().position(|r| Some(*r) == self.focus) {
                    let j = if vk == VK_UP {
                        i.saturating_sub(1)
                    } else {
                        (i + 1).min(rows.len() - 1)
                    };
                    self.focus_visible = true;
                    self.set_focus(rows[j]);
                }
            }
            VK_PRIOR => self.scroll_to(self.scroll - h * 0.9),
            VK_NEXT => self.scroll_to(self.scroll + h * 0.9),
            VK_HOME => self.scroll_to(0.0),
            VK_END => self.scroll_to(f32::MAX),
            _ => return false,
        }
        true
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: `tme` outlives the call.
            self.tracking = unsafe { TrackMouseEvent(&mut tme) }.is_ok();
        }
        if let Some((start_y, start_scroll)) = self.scroll_drag {
            let (_, h) = self.client();
            let total = self.layout.content_height;
            if let Some((_, thumb)) = self.scrollbar() {
                let per_px = (total - h) / (h - thumb.h).max(1.0);
                self.scroll_to(start_scroll + (y - start_y) * per_px);
            }
            return;
        }
        let hover = self.hit(x, y);
        let over = self.recording.is_none() && self.scrollbar().is_some_and(|(t, _)| t.contains(x, y));
        if hover != self.hover || over != self.over_scrollbar {
            self.hover = hover;
            self.over_scrollbar = over;
            // SAFETY: own window.
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        }
    }

    fn on_button_down(&mut self, x: f32, y: f32) {
        self.focus_visible = false;
        if self.recording.is_none() {
            if let Some((track, thumb)) = self.scrollbar() {
                if track.contains(x, y) {
                    if !thumb.contains(x, y) {
                        let (_, h) = self.client();
                        let dir = if y < thumb.y { -1.0 } else { 1.0 };
                        self.scroll_to(self.scroll + dir * h * 0.9);
                    }
                    self.scroll_drag = Some((y, self.scroll));
                    // SAFETY: own window.
                    unsafe { SetCapture(self.hwnd) };
                    return;
                }
            }
        }
        self.pressed = self.hit(x, y);
        // SAFETY: own window.
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
            SetCapture(self.hwnd);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn on_button_up(&mut self, x: f32, y: f32) {
        // SAFETY: plain call.
        let _ = unsafe { ReleaseCapture() };
        if self.scroll_drag.take().is_some() {
            return;
        }
        let pressed = self.pressed.take();
        if let Some(hit) = pressed.filter(|p| Some(*p) == self.hit(x, y)) {
            if !matches!(hit, Hit::Item(_, Part::Field)) {
                self.focus = Some(hit);
            }
            self.activate(hit);
        }
        // SAFETY: own window.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn on_wheel(&mut self, delta: i16, x: f32) {
        if self.recording.is_some() {
            return;
        }
        let step = f32::from(delta) / 120.0 * WHEEL_STEP * self.k;
        if x < self.layout.nav_w {
            self.nav_scroll -= step;
            self.clamp_scroll();
            // SAFETY: own window.
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        } else {
            self.scroll_to(self.scroll - step);
        }
    }

    fn set_dpi(&mut self, dpi: u32) {
        self.k = dpi.max(96) as f32 / 96.0;
        let old = std::mem::replace(&mut self.edit_font, make_edit_font(self.k));
        for f in &self.fields {
            send(f.hwnd, WM_SETFONT, self.edit_font.0 as usize, 0);
        }
        // SAFETY: no longer selected into any control.
        let _ = unsafe { DeleteObject(old.into()) };
    }
}

fn dialog_nodes(fonts: &mut Fonts, hwnd: HWND, rec: &Recording, w: f32, h: f32, k: f32) -> Vec<Node> {
    let keys = match rec.captured {
        Some(s) => layout::key_caps(&s.to_string()),
        None => {
            let mut held = Vec::new();
            for (m, n) in [
                (Mods::WIN, "Win"),
                (Mods::CTRL, "Ctrl"),
                (Mods::ALT, "Alt"),
                (Mods::SHIFT, "Shift"),
            ] {
                if rec.held.contains(m) {
                    held.push(n);
                }
            }
            layout::key_caps(&held.join("+"))
        }
    };
    let valid = rec.captured.is_some_and(|s| s.is_valid_global());
    // SAFETY: own window; the DC is released below.
    let hdc = unsafe { GetDC(Some(hwnd)) };
    // SAFETY: `hdc` is valid until released after the painter is dropped.
    let nodes = unsafe { Painter::new(hdc, fonts) }
        .map(|mut p| layout::dialog(&mut p, &keys, valid || rec.captured.is_none(), w, h, k))
        .unwrap_or_default();
    // SAFETY: obtained above from the same window.
    unsafe { ReleaseDC(Some(hwnd), hdc) };
    nodes
}

/// While the shortcut dialog is open, keys pressed in the settings window go to it
/// instead of Windows, so that shortcuts such as Win+Shift+S can be recorded.
unsafe extern "system" fn record_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let owner = RECORDER.get();
    // SAFETY: called by Windows with a KBDLLHOOKSTRUCT for HC_ACTION.
    unsafe {
        if code == HC_ACTION as i32 && owner != 0 && GetForegroundWindow().0 as isize == owner {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let _ = PostMessageW(
                Some(HWND(owner as *mut _)),
                WM_APP_KEY,
                WPARAM(wparam.0),
                LPARAM(kb.vkCode as isize),
            );
            return LRESULT(1);
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Text boxes: Enter (single line) and leaving the box save; Esc undoes; Tab moves on.
unsafe extern "system" fn field_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    parent: usize,
) -> LRESULT {
    let parent = HWND(parent as *mut _);
    // SAFETY: `parent` is the settings window that made this box; its state outlives
    // its children.
    unsafe {
        match msg {
            WM_KEYDOWN => {
                let multiline = (GetWindowLongW(hwnd, GWL_STYLE) as u32 & ES_MULTILINE as u32) != 0;
                if let Some(st) = state(parent) {
                    match VIRTUAL_KEY(wparam.0 as u16) {
                        VK_RETURN if !multiline => {
                            st.commit(hwnd);
                            return LRESULT(0);
                        }
                        VK_ESCAPE => {
                            st.revert(hwnd);
                            return LRESULT(0);
                        }
                        VK_TAB => {
                            st.commit(hwnd);
                            st.move_focus(GetKeyState(i32::from(VK_SHIFT.0)) < 0);
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                }
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            WM_CHAR if wparam.0 == '\t' as usize || wparam.0 == 0x1B => LRESULT(0),
            WM_CHAR
                if wparam.0 == '\r' as usize && (GetWindowLongW(hwnd, GWL_STYLE) as u32 & ES_MULTILINE as u32) == 0 =>
            {
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                if let Some(st) = state(parent) {
                    st.commit(hwnd);
                }
                let _ = InvalidateRect(Some(parent), None, false);
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            WM_SETFOCUS => {
                if let Some(st) = state(parent) {
                    if let Some(f) = st.fields.iter().find(|f| f.hwnd == hwnd) {
                        st.focus = Some(Hit::Item(f.at, Part::Field));
                    }
                }
                let _ = InvalidateRect(Some(parent), None, false);
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}

fn mouse_xy(lparam: LPARAM) -> (f32, f32) {
    (
        (lparam.0 & 0xFFFF) as i16 as f32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as f32,
    )
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the state pointer is valid while set (see `state`).
    let Some(st) = (unsafe { state(hwnd) }) else {
        // SAFETY: forwarding the unmodified arguments of this callback.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    // SAFETY: own window and its children, on the thread that made them.
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(st);
                LRESULT(0)
            }
            WM_SIZE => {
                st.relayout();
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96);
                info.ptMinTrackSize = POINT {
                    x: scale(MIN_W, dpi),
                    y: scale(MIN_H, dpi),
                };
                LRESULT(0)
            }
            WM_DPICHANGED => {
                st.set_dpi(u32::from((wparam.0 & 0xFFFF) as u16));
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                st.relayout();
                LRESULT(0)
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
                st.apply_theme();
                st.relayout();
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEMOVE => {
                let (x, y) = mouse_xy(lparam);
                st.on_mouse_move(x, y);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                st.tracking = false;
                st.over_scrollbar = false;
                if st.hover.take().is_some() || st.scroll_drag.is_none() {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = mouse_xy(lparam);
                st.on_button_down(x, y);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = mouse_xy(lparam);
                st.on_button_up(x, y);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let mut pt = POINT {
                    x: (lparam.0 & 0xFFFF) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                let _ = ScreenToClient(hwnd, &mut pt);
                st.on_wheel(((wparam.0 >> 16) & 0xFFFF) as i16, pt.x as f32);
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if st.on_key(VIRTUAL_KEY(wparam.0 as u16)) {
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_GETDLGCODE => LRESULT(DLGC_WANTALLKEYS as isize),
            WM_APP_KEY => {
                st.record_key(wparam.0 as u32, lparam.0 as u32);
                LRESULT(0)
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                let hdc = HDC(wparam.0 as *mut _);
                let edit = HWND(lparam.0 as *mut _);
                let focused = GetFocus() == edit;
                let p = &st.palette;
                let ink = if IsWindowEnabled(edit).as_bool() {
                    p.text
                } else {
                    p.disabled
                };
                SetTextColor(hdc, colorref(over(field_color(p, focused), ink)));
                SetBkColor(hdc, colorref(field_color(p, focused)));
                let brush = if focused {
                    st.field_brushes.1
                } else {
                    st.field_brushes.0
                };
                LRESULT(brush.0 as isize)
            }
            WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE && st.recording.is_some() => {
                st.finish_recording(DialogButton::Cancel);
                LRESULT(0)
            }
            WM_CLOSE => {
                if st.recording.is_some() {
                    st.stop_recording();
                }
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// # Safety
/// Called while handling the window's WM_PAINT.
unsafe fn paint(st: &mut State) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: standard paint sequence into an off-screen bitmap; everything made
    // here is released before returning.
    unsafe {
        let hdc = BeginPaint(st.hwnd, &mut ps);
        let (w, h) = st.client();
        let (wi, hi) = (w as i32, h as i32);
        if wi > 0 && hi > 0 {
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, wi, hi);
            let old = SelectObject(mem, bmp.into());
            let palette = st.palette;
            let k = st.k;
            let scrollbar = st.scrollbar();
            let thumb_wide = st.scroll_drag.is_some() || st.over_scrollbar;
            if let Some(mut p) = Painter::new(mem, &mut st.fonts) {
                let view = View {
                    palette: &palette,
                    k,
                    nav: &st.nav,
                    folded: &st.folded,
                    page: &st.page,
                    hover: st.hover,
                    pressed: st.pressed,
                    focus: if st.focus_visible { st.focus } else { None },
                    focused_field: st.fields.iter().find(|f| f.hwnd == GetFocus()).map(|f| f.at),
                };
                p.fill(Rect::new(0.0, 0.0, w, h), 0.0, palette.window);
                let nav_w = st.layout.nav_w;
                p.set_clip(Some(Rect::new(0.0, 0.0, nav_w, h)));
                for n in &st.layout.nav {
                    view.draw(&mut p, n, -st.nav_scroll);
                }
                // The page sits on a layer with a rounded top-left corner.
                p.set_clip(None);
                let layer = Rect::new(nav_w, 0.0, w - nav_w + 16.0 * k, h + 16.0 * k);
                p.fill(layer, 8.0 * k, palette.layer);
                p.stroke(layer, 8.0 * k, palette.layer_stroke, k);
                p.set_clip(Some(Rect::new(nav_w + k, k, w - nav_w - k, h - k)));
                for n in &st.layout.content {
                    view.draw(&mut p, n, -st.scroll);
                }
                if let Some((track, thumb)) = scrollbar {
                    let tw = if thumb_wide { 6.0 } else { 3.0 } * k;
                    let r = Rect::new(track.right() - tw - 3.0 * k, thumb.y + 2.0 * k, tw, thumb.h - 4.0 * k);
                    p.fill(r, tw / 2.0, palette.text3);
                }
                p.set_clip(None);
                if let Some(rec) = &st.recording {
                    p.fill(Rect::new(0.0, 0.0, w, h), 0.0, palette.smoke);
                    for n in &rec.nodes {
                        view.draw(&mut p, n, 0.0);
                    }
                }
            }
            let _ = BitBlt(hdc, 0, 0, wi, hi, Some(mem), 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
        }
        let _ = EndPaint(st.hwnd, &ps);
    }
}

/// What drawing needs besides the node itself.
struct View<'a> {
    palette: &'a Palette,
    k: f32,
    nav: &'a [NavEntry],
    folded: &'a HashSet<usize>,
    page: &'a Page,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    focus: Option<Hit>,
    focused_field: Option<ItemRef>,
}

impl View<'_> {
    fn ink(&self, role: u32) -> u32 {
        match role {
            1 => self.palette.text,
            2 => self.palette.text2,
            _ => self.palette.disabled,
        }
    }

    /// The fill for a control in its rest, hover and pressed states.
    fn state_fill(&self, hit: Option<Hit>, rest: u32, hover: u32, pressed: u32) -> u32 {
        match hit {
            Some(h) if self.pressed == Some(h) && self.hover == Some(h) => pressed,
            Some(h) if self.hover == Some(h) => hover,
            _ => rest,
        }
    }

    fn icon(&self, p: &mut Painter, ico: &'static [u8], r: Rect) {
        let size = r.w.round() as i32;
        match crate::icon::load(ico, size) {
            Ok(icon) => p.icon(icon, r),
            Err(e) => tracing::debug!("settings icon: {e}"),
        }
    }

    fn draw(&self, p: &mut Painter, n: &Node, dy: f32) {
        let pal = self.palette;
        let k = self.k;
        let r = n.rect.offset(0.0, dy);
        let radius = RADIUS * k;
        let body = |color: u32| Style {
            face: Face::Text,
            px: layout::BODY_PX * k,
            color,
            align: Align::Left,
            wrap: false,
        };
        match &n.look {
            Look::NavRow {
                entry,
                selected,
                compact,
            } => {
                let fill = if *selected { pal.subtle_hover } else { 0 };
                let fill = self.state_fill(n.hit, fill, pal.subtle_hover, pal.subtle_pressed);
                p.fill(r, radius, fill);
                if *selected {
                    let pill = Rect::new(r.x, r.y + (r.h - 16.0 * k) / 2.0, 3.0 * k, 16.0 * k);
                    p.fill(pill, 1.5 * k, pal.accent);
                }
                let icon_box = if *compact {
                    Rect::new(
                        r.x + (r.w - 20.0 * k) / 2.0,
                        r.y + (r.h - 20.0 * k) / 2.0,
                        20.0 * k,
                        20.0 * k,
                    )
                } else {
                    Rect::new(r.x + 12.0 * k, r.y + (r.h - 20.0 * k) / 2.0, 20.0 * k, 20.0 * k)
                };
                let text_r = Rect::new(r.x + 44.0 * k, r.y, r.w - 44.0 * k - 36.0 * k, r.h);
                match self.nav.get(*entry) {
                    Some(NavEntry::Page { title, icon, glyph, .. }) => {
                        if let Some(ico) = icon {
                            self.icon(p, ico, icon_box);
                        } else if let Some(g) = glyph {
                            p.glyph(*g, icon_box, 16.0 * k, pal.text);
                        }
                        if !compact {
                            p.text(title, text_r, &body(pal.text));
                        }
                    }
                    Some(NavEntry::Category { title, glyph }) => {
                        p.glyph(*glyph, icon_box, 16.0 * k, pal.text);
                        if !compact {
                            p.text(title, text_r, &body(pal.text));
                            let folded = self.folded.contains(entry);
                            let chevron = Rect::new(r.right() - 32.0 * k, r.y, 24.0 * k, r.h);
                            p.glyph(
                                if folded { '\u{E70D}' } else { '\u{E70E}' },
                                chevron,
                                12.0 * k,
                                pal.text2,
                            );
                        }
                    }
                    None => {}
                }
            }
            Look::Text { text, style } => {
                let mut s = *style;
                s.color = self.ink(style.color);
                p.text(text, r, &s);
            }
            Look::Card => {
                p.fill(r, radius, pal.card);
                p.stroke(r, radius, pal.card_stroke, k);
            }
            Look::CardHover => {
                let fill = self.state_fill(n.hit, 0, pal.subtle_hover, pal.subtle_pressed);
                p.fill(r.inset(k, k), radius, fill);
            }
            Look::Divider => p.fill(r, 0.0, pal.divider),
            Look::ItemIcon(at) => {
                if let Some(item) = at.get(self.page) {
                    if let Some(ico) = item.icon {
                        self.icon(p, ico, r);
                    } else if let Some(g) = item.glyph {
                        p.glyph(g, r, 20.0 * k, if item.enabled { pal.text } else { pal.disabled });
                    }
                }
            }
            Look::Toggle { on, enabled } => {
                let sw = Rect::new(r.right() - 40.0 * k, r.y + (r.h - 20.0 * k) / 2.0, 40.0 * k, 20.0 * k);
                let label_r = Rect::new(r.x, r.y, sw.x - r.x - 12.0 * k, r.h);
                let mut st = body(if *enabled { pal.text } else { pal.disabled });
                st.align = Align::Right;
                p.text(if *on { "On" } else { "Off" }, label_r, &st);
                let hovered = self.hover == n.hit && n.hit.is_some();
                let knob = if hovered { 14.0 } else { 12.0 } * k;
                let ky = sw.y + (sw.h - knob) / 2.0;
                if *on {
                    let fill = match (enabled, hovered) {
                        (false, _) => pal.disabled,
                        (true, true) => pal.accent_hover,
                        (true, false) => pal.accent,
                    };
                    p.fill(sw, sw.h / 2.0, fill);
                    let kx = sw.right() - 4.0 * k - knob + (if hovered { k } else { 0.0 });
                    p.ellipse(
                        Rect::new(kx, ky, knob, knob),
                        if *enabled { pal.on_accent } else { pal.window },
                    );
                } else {
                    let stroke = if *enabled { pal.strong_stroke } else { pal.disabled };
                    p.fill(
                        sw,
                        sw.h / 2.0,
                        if hovered { pal.subtle_hover } else { pal.subtle_pressed },
                    );
                    p.stroke(sw, sw.h / 2.0, stroke, k);
                    let kx = sw.x + 4.0 * k - (if hovered { k } else { 0.0 });
                    p.ellipse(
                        Rect::new(kx, ky, knob, knob),
                        if *enabled { pal.text2 } else { pal.disabled },
                    );
                }
            }
            Look::Keys { keys, enabled, large } => {
                let px = if *large { 18.0 } else { layout::KEY_PX } * k;
                let h = if *large { 48.0 } else { 32.0 } * k;
                let st = Style {
                    face: Face::Strong,
                    px,
                    color: if *enabled { pal.on_accent } else { pal.window },
                    align: Align::Center,
                    wrap: false,
                };
                let total = layout::keys_width(p, keys, if *large { 18.0 } else { layout::KEY_PX }, k);
                let mut x = r.right() - total;
                if *large {
                    x = r.x + (r.w - total) / 2.0;
                }
                let fill = if *enabled {
                    self.state_fill(n.hit, pal.accent, pal.accent_hover, pal.accent_hover)
                } else {
                    pal.disabled
                };
                for key in keys {
                    let (tw, _) = p.measure(key, &st, 0.0);
                    let kw = (tw + 20.0 * k).max(32.0 * k);
                    let cap = Rect::new(x, r.y + (r.h - h) / 2.0, kw, h);
                    p.fill(cap, radius, fill);
                    p.text(key, cap, &st);
                    x += kw + 4.0 * k;
                }
            }
            Look::EditButton { enabled } => {
                p.fill(
                    r,
                    radius,
                    self.state_fill(n.hit, 0, pal.subtle_hover, pal.subtle_pressed),
                );
                p.glyph('\u{E70F}', r, 14.0 * k, if *enabled { pal.text } else { pal.disabled });
            }
            Look::Combo { label, enabled } => {
                p.fill(
                    r,
                    radius,
                    self.state_fill(n.hit, pal.control, pal.control_hover, pal.control_pressed),
                );
                p.control_border(r, radius, pal.control_stroke, pal.control_stroke_bottom, k);
                let ink = if *enabled { pal.text } else { pal.disabled };
                p.text(label, Rect::new(r.x + 11.0 * k, r.y, r.w - 48.0 * k, r.h), &body(ink));
                p.glyph(
                    '\u{E70D}',
                    Rect::new(r.right() - 36.0 * k, r.y, 24.0 * k, r.h),
                    12.0 * k,
                    pal.text2,
                );
            }
            Look::Field(at) => {
                let focused = self.focused_field == Some(*at);
                let fill = if focused { pal.field_focus } else { pal.control };
                p.fill(r, radius, fill);
                p.control_border(r, radius, pal.control_stroke, pal.control_stroke_bottom, k);
                if focused {
                    p.fill(
                        Rect::new(r.x + radius / 2.0, r.bottom() - 2.0 * k, r.w - radius, 2.0 * k),
                        k,
                        pal.accent,
                    );
                }
            }
            Look::Swatch { color, enabled } => {
                p.fill(
                    r,
                    radius,
                    self.state_fill(n.hit, pal.control, pal.control_hover, pal.control_pressed),
                );
                p.control_border(r, radius, pal.control_stroke, pal.control_stroke_bottom, k);
                let sw = Rect::new(r.x + 6.0 * k, r.y + 6.0 * k, r.w - 40.0 * k, r.h - 12.0 * k);
                p.fill(sw, 2.0 * k, if *enabled { *color } else { pal.disabled });
                p.stroke(sw, 2.0 * k, pal.control_stroke_bottom, k);
                p.glyph(
                    '\u{E70D}',
                    Rect::new(r.right() - 30.0 * k, r.y, 24.0 * k, r.h),
                    12.0 * k,
                    pal.text2,
                );
            }
            Look::Button { label, accent, enabled } => {
                if *accent && *enabled {
                    p.fill(
                        r,
                        radius,
                        self.state_fill(n.hit, pal.accent, pal.accent_hover, pal.accent_hover),
                    );
                } else {
                    p.fill(
                        r,
                        radius,
                        self.state_fill(n.hit, pal.control, pal.control_hover, pal.control_pressed),
                    );
                    p.control_border(r, radius, pal.control_stroke, pal.control_stroke_bottom, k);
                }
                let ink = match (enabled, accent) {
                    (false, _) => pal.disabled,
                    (true, true) => pal.on_accent,
                    (true, false) => pal.text,
                };
                let mut st = body(ink);
                st.align = Align::Center;
                p.text(label, r, &st);
            }
            Look::Chevron { glyph, enabled } => {
                p.glyph(*glyph, r, 12.0 * k, if *enabled { pal.text } else { pal.disabled });
            }
            Look::LayoutTile {
                name,
                zones,
                selected,
                enabled,
            } => {
                p.fill(
                    r,
                    radius,
                    self.state_fill(n.hit, pal.control, pal.control_hover, pal.control_pressed),
                );
                if *selected {
                    p.stroke(r.inset(k, k), radius, pal.accent, 2.0 * k);
                } else {
                    p.control_border(r, radius, pal.control_stroke, pal.control_stroke_bottom, k);
                }
                let ink = match (enabled, selected) {
                    (false, _) => pal.disabled,
                    (true, true) => pal.accent,
                    (true, false) => pal.text2,
                };
                let pic = Rect::new(r.x + 12.0 * k, r.y + 12.0 * k, r.w - 24.0 * k, r.h - 48.0 * k);
                for z in zones {
                    let zr = Rect::new(pic.x + z[0] * pic.w, pic.y + z[1] * pic.h, z[2] * pic.w, z[3] * pic.h)
                        .inset(1.5 * k, 1.5 * k);
                    p.fill(zr, 2.0 * k, (ink & 0x00FF_FFFF) | 0x4000_0000);
                    p.stroke(zr, 2.0 * k, ink, k);
                }
                let mut st = body(if *enabled { pal.text } else { pal.disabled });
                st.align = Align::Center;
                p.text(
                    name,
                    Rect::new(r.x + 6.0 * k, pic.bottom() + 6.0 * k, r.w - 12.0 * k, 24.0 * k),
                    &st,
                );
            }
            Look::Info { severity } => {
                let (bg, _) = severity_colors(pal, *severity);
                p.fill(r, radius, bg);
                p.stroke(r, radius, pal.card_stroke, k);
            }
            Look::InfoGlyph { severity } => {
                let (_, fg) = severity_colors(pal, *severity);
                let glyph = match severity {
                    Severity::Info => '\u{F167}',
                    Severity::Success => '\u{EC61}',
                    Severity::Warning => '\u{F167}',
                    Severity::Error => '\u{EB90}',
                };
                let c = Rect::new(r.x, r.y + (r.h - 16.0 * k) / 2.0, 16.0 * k, 16.0 * k);
                p.ellipse(c, fg);
                p.glyph(
                    if *severity == Severity::Warning { '!' } else { glyph },
                    c,
                    if *severity == Severity::Warning { 12.0 } else { 16.0 } * k,
                    if pal.dark { 0xFF00_0000 } else { 0xFFFF_FFFF },
                );
            }
            Look::Close => {
                p.fill(
                    r,
                    radius,
                    self.state_fill(n.hit, 0, pal.subtle_hover, pal.subtle_pressed),
                );
                p.glyph('\u{E711}', r, 12.0 * k, pal.text);
            }
            Look::Dialog => {
                p.fill(r, 8.0 * k, pal.dialog);
                p.stroke(r, 8.0 * k, pal.card_stroke, k);
            }
            Look::DialogFooter => {
                p.fill(r.inset(k, 0.0), 8.0 * k, pal.dialog_footer);
                p.fill(Rect::new(r.x + k, r.y, r.w - 2.0 * k, 8.0 * k), 0.0, pal.dialog_footer);
                p.fill(Rect::new(r.x, r.y, r.w, k), 0.0, pal.card_stroke);
            }
        }
        if self.focus.is_some() && self.focus == n.hit {
            let ring = r.inset(-3.0 * k, -3.0 * k);
            p.stroke(ring, radius + 3.0 * k, pal.text | 0xFF00_0000, 2.0 * k);
        }
    }
}

fn severity_colors(p: &Palette, s: Severity) -> (u32, u32) {
    match s {
        Severity::Info => p.info,
        Severity::Success => p.success,
        Severity::Warning => p.warning,
        Severity::Error => p.error,
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
        if !guide_view::gdiplus_ready() {
            return Err(Error::Other("GDI+ is not available".into()));
        }
        let class = wide(CLASS_NAME);
        let title = wide("PowerKit Settings");
        // SAFETY: strings outlive the calls; `wndproc` matches its callback contract.
        // The state pointer stays valid until `Drop` clears it.
        unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
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
            let guide = match guide_view::create(hwnd, instance.into(), ID_GUIDE) {
                Ok(g) => g,
                Err(e) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(e);
                }
            };
            let k = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let mut state = Box::new(State {
                hwnd,
                k,
                dark_pref: None,
                palette: Palette::current(false),
                fonts: Fonts::new(),
                edit_font: make_edit_font(k),
                field_brushes: (HBRUSH::default(), HBRUSH::default()),
                nav: Vec::new(),
                folded: HashSet::new(),
                selected: 0,
                page: Page {
                    id: String::new(),
                    title: String::new(),
                    description: String::new(),
                    groups: Vec::new(),
                },
                expanded: HashSet::new(),
                info: None,
                has_scene: false,
                layout: Layout::default(),
                scroll: 0.0,
                nav_scroll: 0.0,
                hover: None,
                pressed: None,
                scroll_drag: None,
                over_scrollbar: false,
                tracking: false,
                focus: None,
                focus_visible: false,
                fields: Vec::new(),
                guide,
                recording: None,
                hook: None,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
            state.apply_theme();
            state.relayout();
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
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// `Some(true)` for dark, `Some(false)` for light, `None` to follow Windows.
    pub fn set_dark(&mut self, dark: Option<bool>) {
        if self.state.dark_pref != dark {
            self.state.dark_pref = dark;
            self.state.apply_theme();
            self.state.relayout();
        }
    }

    pub fn set_nav(&mut self, entries: Vec<NavEntry>, selected: usize) {
        let st = &mut self.state;
        if st.nav != entries {
            st.folded.clear();
            st.nav = entries;
        }
        st.selected = selected;
        st.relayout();
    }

    /// Show `page` with `scene` playing at its top. The scroll position and open
    /// cards are kept while the page's id stays the same.
    pub fn set_page(&mut self, page: Page, scene: Option<Scene>) {
        let st = &mut self.state;
        if st.page.id != page.id {
            st.scroll = 0.0;
            st.expanded.clear();
            st.info = None;
            st.focus = None;
            if st.recording.is_some() {
                st.stop_recording();
            }
        }
        st.page = page;
        st.has_scene = scene.is_some();
        guide_view::set_scene(st.guide, scene);
        st.relayout();
    }

    /// A message under the page title, or none.
    pub fn set_info(&mut self, info: Option<(Severity, String)>) {
        if self.state.info != info {
            self.state.info = info;
            self.state.relayout();
        }
    }

    /// Use the icon file `ico` in the title bar and taskbar.
    pub fn set_icon(&mut self, ico: &'static [u8]) -> Result<()> {
        crate::icon::set_window_icon(self.hwnd, ico)
    }

    /// Ask for a settings file to open, or where to save one (suggesting `name`);
    /// `None` when the user cancelled.
    pub fn pick_settings_file(&self, save: bool, name: Option<&std::path::Path>) -> Result<Option<std::path::PathBuf>> {
        const KINDS: crate::dialog::FileKinds = crate::dialog::FileKinds {
            filters: &[("PowerKit settings", "*.json")],
            extension: "json",
        };
        crate::dialog::pick(self.hwnd, save, &KINDS, name)
    }

    /// Ask a yes-or-no question over the window; true for yes.
    pub fn confirm(&self, title: &str, text: &str) -> bool {
        let (title, text) = (wide(title), wide(text));
        // SAFETY: the strings outlive the modal call; `hwnd` is our window.
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                pcwstr(&text),
                pcwstr(&title),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        answer == IDYES
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        // SAFETY: own windows, on the thread that created them; the state pointer is
        // cleared before the box is freed.
        unsafe {
            if self.state.recording.is_some() {
                self.state.stop_recording();
            }
            for f in &self.state.fields {
                let _ = RemoveWindowSubclass(f.hwnd, Some(field_proc), SUBCLASS_ID);
            }
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.state.edit_font.into());
            for b in [self.state.field_brushes.0, self.state.field_brushes.1] {
                let _ = DeleteObject(b.into());
            }
        }
    }
}
