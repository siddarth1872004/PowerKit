//! Showing the guide: Windows-key tracking, the dimmed background, the panel and the
//! taskbar number badges.

use crate::logic::{app_section, cap_label, flow, windows_sections, Line, Section};
use anyhow::Result;
use powerkit_config::{ShortcutGuide as Settings, Theme};
use powerkit_core::Rect;
use powerkit_platform::canvas::{Align, Canvas, Font, Painter};
use powerkit_platform::keyhook::KeyDecision;
use powerkit_platform::overlay::{Overlay, Piece};
use powerkit_platform::{event_loop, input, monitor, taskbar, theme, window};
use std::time::{Duration, Instant};

const VK_ESCAPE: u32 = 0x1B;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// An unassigned key. Pressing it before the Windows key comes up stops the Start menu
/// from opening, since Windows then treats the press as part of a combination.
const VK_NONE: u32 = 0xE8;
const POLL_MS: u32 = 50;

const PANEL_ALPHA: u8 = 250;
const PAD: u32 = 24;
const TITLE_H: u32 = 44;
const COL_W: u32 = 340;
const COL_GAP: u32 = 28;
const ROW_H: u32 = 30;
const KEYS_W: u32 = 150;

struct Palette {
    panel: &'static str,
    border: &'static str,
    text: &'static str,
    dim_text: &'static str,
    cap: &'static str,
    accent: &'static str,
    on_accent: &'static str,
}

const DARK: Palette = Palette {
    panel: "#202020",
    border: "#3A3A3A",
    text: "#FFFFFF",
    dim_text: "#A8A8A8",
    cap: "#383838",
    accent: "#60CDFF",
    on_accent: "#000000",
};

const LIGHT: Palette = Palette {
    panel: "#F3F3F3",
    border: "#D0D0D0",
    text: "#1A1A1A",
    dim_text: "#5C5C5C",
    cap: "#FFFFFF",
    accent: "#005FB8",
    on_accent: "#FFFFFF",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// Shown while the Windows key is held.
    Hold,
    /// Shown by the shortcut until it is pressed again or Esc.
    Toggle,
}

struct Shown {
    trigger: Trigger,
    _dim: Option<Overlay>,
    _panel: Canvas,
    _badges: Vec<Canvas>,
}

fn is_win(vk: u32) -> bool {
    vk == VK_LWIN || vk == VK_RWIN
}

fn win_held() -> bool {
    input::key_down(VK_LWIN) || input::key_down(VK_RWIN)
}

pub struct Live {
    pub settings: Settings,
    pub powerkit: Section,
    /// When the Windows key went down alone, while waiting for the press time.
    pending: Option<Instant>,
    timer: Option<usize>,
    shown: Option<Shown>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop_timer();
    }
}

impl Live {
    pub fn new(settings: Settings, powerkit: Section) -> Live {
        Live {
            settings,
            powerkit,
            pending: None,
            timer: None,
            shown: None,
        }
    }

    pub fn is_shown(&self) -> bool {
        self.shown.is_some()
    }

    pub fn is_timer(&self, id: usize) -> bool {
        self.timer == Some(id)
    }

    fn stop_timer(&mut self) {
        if let Some(t) = self.timer.take() {
            event_loop::kill_timer(t);
        }
    }

    pub fn hide(&mut self) {
        self.pending = None;
        self.shown = None;
        self.stop_timer();
    }

    pub fn toggle(&mut self) -> Result<()> {
        if self.shown.is_some() {
            self.hide();
            return Ok(());
        }
        self.pending = None;
        self.stop_timer();
        self.show(Trigger::Toggle)
    }

    /// Keyboard hook callback. Only hides; showing waits for the timer so the hook
    /// returns quickly.
    pub fn on_key(&mut self, vk: u32, down: bool) -> KeyDecision {
        let held = self.shown.as_ref().map(|s| s.trigger);
        if is_win(vk) {
            if down {
                if held.is_none() && self.pending.is_none() && self.settings.hold_win {
                    self.pending = Some(Instant::now());
                    if self.timer.is_none() {
                        match event_loop::set_timer(POLL_MS) {
                            Ok(t) => self.timer = Some(t),
                            Err(e) => tracing::warn!(error = %e, "Shortcut Guide timer failed"),
                        }
                    }
                }
                return KeyDecision::Pass;
            }
            self.pending = None;
            if held == Some(Trigger::Hold) {
                self.hide();
                return KeyDecision::Replace(vec![(VK_NONE, true), (VK_NONE, false), (vk, false)]);
            }
            return KeyDecision::Pass;
        }
        if down {
            // Another key with Windows held is a shortcut being typed, not a request for
            // the guide; one typed while the guide shows is what the user went looking for.
            self.pending = None;
            if held == Some(Trigger::Hold) || (held == Some(Trigger::Toggle) && vk == VK_ESCAPE) {
                self.hide();
            }
        }
        KeyDecision::Pass
    }

    /// Timer tick: show after the press time, and hide when the Windows key was let go
    /// without the hook seeing it.
    pub fn tick(&mut self) -> Result<()> {
        if let Some(since) = self.pending {
            if !win_held() {
                self.pending = None;
            } else if since.elapsed() >= Duration::from_millis(self.settings.press_time_ms.into()) {
                self.pending = None;
                self.show(Trigger::Hold)?;
            }
        }
        let hold = self.shown.as_ref().is_some_and(|s| s.trigger == Trigger::Hold);
        if hold && !win_held() {
            self.shown = None;
        }
        if self.pending.is_none() && !(hold && self.shown.is_some()) {
            self.stop_timer();
        }
        Ok(())
    }

    fn excluded(&self) -> (bool, Option<String>) {
        let exe = window::foreground()
            .and_then(window::thread_process)
            .and_then(|(pid, _)| window::process_exe(pid));
        let excluded = exe.as_ref().is_some_and(|e| self.settings.excluded_apps.contains(e));
        (excluded, exe)
    }

    fn show(&mut self, trigger: Trigger) -> Result<()> {
        let (excluded, exe) = self.excluded();
        if excluded {
            return Ok(());
        }
        let light = match self.settings.theme {
            Theme::Light => true,
            Theme::Dark => false,
            Theme::System => theme::is_light(false).unwrap_or(false),
        };
        let pal = if light { &LIGHT } else { &DARK };
        let mon = window::foreground()
            .and_then(monitor::from_window)
            .or_else(|| input::cursor_pos().ok().and_then(|(x, y)| monitor::from_point(x, y)))
            .ok_or_else(|| anyhow::anyhow!("no monitor to show the guide on"))?;

        let mut sections = Vec::new();
        if let Some(app) = exe.as_deref().and_then(app_section) {
            sections.push(app);
        }
        sections.push(self.powerkit.clone());
        sections.extend(windows_sections());

        let dim = match self.settings.opacity_percent {
            0 => None,
            p => {
                let mut o = Overlay::new()?;
                let alpha = (p * 255 / 100) as u8;
                o.show_shape(mon.rect, "#000000", alpha, &[Piece::Rect(mon.rect)])?;
                Some(o)
            }
        };
        let mut panel = Canvas::new()?;
        draw_panel(&mut panel, &mon, &sections, trigger, pal)?;
        let badges = if self.settings.show_taskbar_numbers {
            taskbar_badges(pal).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "taskbar numbers unavailable");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        self.shown = Some(Shown {
            trigger,
            _dim: dim,
            _panel: panel,
            _badges: badges,
        });
        Ok(())
    }
}

fn draw_panel(
    panel: &mut Canvas,
    mon: &monitor::Monitor,
    sections: &[Section],
    trigger: Trigger,
    pal: &Palette,
) -> Result<()> {
    let s = |v: u32| mon.scale(v);
    let area = mon.work_area;
    let (pad, title_h, col_w, gap, row_h) = (s(PAD), s(TITLE_H), s(COL_W), s(COL_GAP), s(ROW_H));
    let rows = ((area.height() * 85 / 100 - title_h - 2 * pad) / row_h).max(2) as usize;
    let max_cols = ((area.width() * 92 / 100 - 2 * pad + gap) / (col_w + gap)).max(1) as usize;
    let mut cols = flow(sections, rows);
    cols.truncate(max_cols);
    let n = cols.len().max(1) as i32;
    let longest = cols.iter().map(Vec::len).max().unwrap_or(0) as i32;
    let w = 2 * pad + n * col_w + (n - 1) * gap;
    let h = 2 * pad + title_h + longest * row_h;
    let left = area.left + (area.width() - w) / 2;
    let top = area.top + (area.height() - h) / 2;
    let at = Rect::new(left, top, left + w, top + h);
    let hint = match trigger {
        Trigger::Hold => "Release the Windows key to close",
        Trigger::Toggle => "Press Esc to close",
    };

    panel.show(
        mon.dpi,
        PANEL_ALPHA,
        |_| at,
        |p| {
            let one = p.scale(1).max(1);
            p.fill(Rect::new(0, 0, w, h), pal.border);
            p.fill(Rect::new(one, one, w - one, h - one), pal.panel);
            let title = Rect::new(pad, pad, w - pad, pad + p.scale(28));
            p.text(title, "Keyboard shortcuts", pal.text, Font::Title, Align::Left);
            let (hw, _) = p.measure(hint, Font::Body);
            let hint_r = Rect::new(w - pad - hw, title.top, w - pad, title.bottom);
            p.text(hint_r, hint, pal.dim_text, Font::Body, Align::Left);
            for (c, col) in cols.iter().enumerate() {
                let x = pad + c as i32 * (col_w + gap);
                for (i, line) in col.iter().enumerate() {
                    let y = pad + title_h + i as i32 * row_h;
                    let row = Rect::new(x, y, x + col_w, y + row_h);
                    match line {
                        Line::Header(t) => p.text(row, t, pal.accent, Font::Key, Align::Left),
                        Line::Item(item) => {
                            let end = draw_keys(p, row, &item.keys, pal);
                            let dx = (x + p.scale(KEYS_W as i32)).max(end + p.scale(10));
                            let r = Rect::new(dx, y, row.right, row.bottom);
                            p.text(r, &item.what, pal.text, Font::Body, Align::Left);
                        }
                    }
                }
            }
        },
    )?;
    Ok(())
}

/// Key caps for "Ctrl+Shift+S" from the row's left edge; returns where they end.
fn draw_keys(p: &Painter, row: Rect, keys: &str, pal: &Palette) -> i32 {
    let cap_h = p.scale(22);
    let top = row.top + (row.height() - cap_h) / 2;
    let mut x = row.left;
    for key in keys.split('+').filter(|k| !k.is_empty()) {
        let label = cap_label(key);
        let (tw, _) = p.measure(label, Font::Key);
        let cap_w = (tw + p.scale(14)).max(p.scale(24));
        let r = Rect::new(x, top, x + cap_w, top + cap_h);
        p.round(r, p.scale(4), pal.cap);
        p.text(r, label, pal.text, Font::Key, Align::Center);
        x += cap_w + p.scale(4);
    }
    x
}

/// A numbered badge over each of the first ten taskbar buttons (Win+1..Win+0).
fn taskbar_badges(pal: &Palette) -> Result<Vec<Canvas>> {
    let mut out = Vec::new();
    for (i, b) in taskbar::buttons()?.into_iter().take(10).enumerate() {
        let (cx, cy) = (b.left + b.width() / 2, b.top + b.height() / 2);
        let dpi = monitor::from_point(cx, cy).map_or(96, |m| m.dpi);
        let size = (22 * dpi as i32 + 48) / 96;
        let at = Rect::new(cx - size / 2, b.top + 1, cx - size / 2 + size, b.top + 1 + size);
        let label = ((i + 1) % 10).to_string();
        let mut c = Canvas::new()?;
        c.show(
            dpi,
            255,
            |_| at,
            |p| {
                let r = Rect::new(0, 0, size, size);
                p.fill(r, pal.panel);
                p.round(r, p.scale(4), pal.accent);
                p.text(r, &label, pal.on_accent, Font::Key, Align::Center);
            },
        )?;
        out.push(c);
    }
    Ok(out)
}
