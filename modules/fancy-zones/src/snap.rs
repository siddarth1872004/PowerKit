//! The Windows side of FancyZones: tracking window drags, showing the zone overlay,
//! moving windows into zones, and remembering which windows are snapped.

use crate::engine::{self, Step};
use anyhow::{anyhow, bail, Result};
use powerkit_config::{DragModifier, FancyZones as Settings};
use powerkit_core::{Event, Rect, WindowEventKind};
use powerkit_platform::event_loop::{kill_timer, set_timer};
use powerkit_platform::input::{cursor_pos, key_down};
use powerkit_platform::monitor::{self, Monitor};
use powerkit_platform::window::{self, Hwnd, WindowInfo, WS_THICKFRAME_BITS};
use powerkit_platform::zones::{ZoneOverlay, ZoneView};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
/// Drag tracking interval; about one frame at 60 Hz.
const TICK_MS: u32 = 15;
/// Size changes smaller than this are noise.
const RESIZE_TOLERANCE: i32 = 2;
/// Dragging a maximized or snapped-by-the-shell window changes its size once when it
/// is restored; a second change means the user is resizing, not moving.
const MAX_SIZE_CHANGES_WHILE_MOVING: u8 = 1;
/// How long the zones show after the layout changes.
const FLASH_MS: u32 = 800;

fn friendly(e: powerkit_platform::Error) -> anyhow::Error {
    if e.is_access_denied() {
        anyhow!(
            "That window belongs to an application running as administrator. \
             PowerKit can only move it when PowerKit itself runs as administrator."
        )
    } else {
        anyhow!(e)
    }
}

/// Whether this event needs the snapper to exist.
pub(crate) fn starts_drag(event: &Event) -> bool {
    matches!(
        event,
        Event::Window {
            kind: WindowEventKind::MoveSizeStart,
            ..
        }
    )
}

fn drag_key_held(m: DragModifier) -> bool {
    match m {
        DragModifier::Shift => key_down(VK_SHIFT),
        DragModifier::Ctrl => key_down(VK_CONTROL),
        DragModifier::Alt => key_down(VK_MENU),
        DragModifier::None => true,
    }
}

/// Key that extends the selection over several zones; Ctrl, or Alt when Ctrl is
/// already the drag modifier.
fn span_key_held(m: DragModifier) -> bool {
    key_down(if m == DragModifier::Ctrl { VK_MENU } else { VK_CONTROL })
}

/// The zones of the layout assigned to a monitor, in screen pixels.
fn monitor_zones(settings: &Settings, m: &Monitor) -> (String, Vec<Rect>) {
    let l = settings.layout_for_monitor(&m.id);
    let rects = engine::zone_rects(l, m.work_area, m.scale(l.margin), m.scale(l.spacing));
    (l.id.clone(), rects)
}

fn slack(settings: &Settings, m: &Monitor) -> i32 {
    let l = settings.layout_for_monitor(&m.id);
    m.scale(l.margin.max(l.spacing))
}

fn size_of(r: Rect) -> (i32, i32) {
    (r.width(), r.height())
}

/// A window PowerKit may move, or why not.
fn eligible(hwnd: Hwnd, settings: &Settings) -> Result<WindowInfo> {
    let info = window::info(hwnd).map_err(friendly)?;
    if info.pid == std::process::id() {
        bail!("PowerKit's own windows are not arranged");
    }
    let mon = monitor::from_window(hwnd).map(|m| m.rect).unwrap_or_default();
    if let Err(reason) = info.manageable(&mon) {
        bail!("This window cannot be arranged ({reason:?})");
    }
    if settings.excluded_apps.contains(&info.exe) {
        bail!("{} is on the FancyZones exclusion list", info.exe);
    }
    Ok(info)
}

struct Snapped {
    /// Process that owned the handle when it was snapped; detects handle reuse.
    pid: u32,
    monitor: String,
    layout: String,
    zones: Vec<usize>,
    /// Size before the window first went into a zone.
    restore: (i32, i32),
}

struct Drag {
    hwnd: Hwnd,
    size: (i32, i32),
    size_changes: u8,
    timer: usize,
    monitor: Option<Monitor>,
    rects: Vec<Rect>,
    layout: String,
    anchor: Option<usize>,
    selection: Vec<usize>,
}

impl Drop for Drag {
    fn drop(&mut self) {
        kill_timer(self.timer);
    }
}

/// Where an app's window was last put, for its next new window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LastZone {
    monitor: String,
    layout: String,
    zones: Vec<usize>,
}

#[derive(Default)]
pub(crate) struct Snapper {
    overlay: Option<ZoneOverlay>,
    drag: Option<Drag>,
    /// The overlay was opened with the toggle shortcut.
    pinned_overlay: bool,
    /// The timer that hides zones shown because the layout changed.
    flash: Option<usize>,
    snapped: HashMap<isize, Snapped>,
    /// By executable name.
    pub(crate) last: HashMap<String, LastZone>,
    /// `last` changed since it was saved.
    pub(crate) last_changed: bool,
    /// Windows already shown once, so only new ones are moved to their app's zone.
    seen: HashSet<isize>,
}

impl Snapper {
    pub(crate) fn status(&self) -> Value {
        json!({
            "dragging": self.drag.is_some(),
            "overlay": self.overlay.as_ref().is_some_and(ZoneOverlay::is_visible),
            "snapped": self.snapped.len(),
        })
    }

    /// Abandon a drag in progress and hide the overlay.
    pub(crate) fn cancel(&mut self) {
        self.drag = None;
        self.pinned_overlay = false;
        self.stop_flash();
        self.hide();
    }

    fn stop_flash(&mut self) {
        if let Some(t) = self.flash.take() {
            kill_timer(t);
        }
    }

    fn hide(&mut self) {
        if let Some(o) = self.overlay.as_mut() {
            o.hide();
        }
    }

    fn show(&mut self, settings: &Settings, m: &Monitor, zones: Vec<Rect>, highlighted: Vec<usize>) -> Result<()> {
        let view = ZoneView {
            area: m.work_area,
            zones,
            highlighted,
            zone_color: settings.zone_color.clone(),
            highlight_color: settings.highlight_color.clone(),
            opacity_percent: settings.overlay_opacity,
            dpi: m.dpi,
        };
        if self.overlay.is_none() {
            self.overlay = Some(ZoneOverlay::new()?);
        }
        let overlay = self.overlay.as_mut().expect("overlay created above");
        overlay.show(&view)?;
        Ok(())
    }

    pub(crate) fn on_event(&mut self, event: &Event, settings: &Settings) -> Result<()> {
        match *event {
            Event::Window { kind, hwnd } => {
                let hwnd = Hwnd(hwnd);
                match kind {
                    WindowEventKind::MoveSizeStart => self.begin_drag(hwnd, settings),
                    WindowEventKind::MoveSizeEnd if self.dragging(hwnd) => self.end_drag(settings),
                    WindowEventKind::Shown if settings.move_new_windows => {
                        self.place_new(hwnd, settings);
                        Ok(())
                    }
                    WindowEventKind::Destroyed => {
                        self.seen.remove(&hwnd.0);
                        self.snapped.remove(&hwnd.0);
                        if self.dragging(hwnd) {
                            self.cancel();
                        }
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            Event::Timer(id) if self.drag.as_ref().is_some_and(|d| d.timer == id) => self.tick(settings),
            Event::Timer(id) if self.flash == Some(id) => {
                self.stop_flash();
                if !self.pinned_overlay && self.drag.is_none() {
                    self.hide();
                }
                Ok(())
            }
            Event::DisplayChanged => {
                self.cancel();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn dragging(&self, hwnd: Hwnd) -> bool {
        self.drag.as_ref().is_some_and(|d| d.hwnd == hwnd)
    }

    /// Put a window an app just opened where that app's last window went, unless the
    /// layout there changed since.
    fn place_new(&mut self, hwnd: Hwnd, settings: &Settings) {
        // Destroyed windows are reported only for windows tracked per thread.
        if self.seen.len() > 1024 {
            self.seen.retain(|h| window::is_window(Hwnd(*h)));
        }
        if !self.seen.insert(hwnd.0) || self.snapped.contains_key(&hwnd.0) || self.drag.is_some() {
            return;
        }
        let Ok(info) = eligible(hwnd, settings) else { return };
        // Fixed-size windows are dialogs and splash screens, not app windows.
        if info.maximized || !info.has_style(WS_THICKFRAME_BITS) {
            return;
        }
        let Some(last) = self.last.get(&info.exe).cloned() else {
            return;
        };
        let Some(mon) = monitor::enumerate()
            .ok()
            .and_then(|all| all.into_iter().find(|m| m.id == last.monitor))
        else {
            return;
        };
        let (layout, rects) = monitor_zones(settings, &mon);
        if layout != last.layout || last.zones.iter().any(|z| *z >= rects.len()) {
            return;
        }
        let Some(target) = engine::union(&rects, &last.zones) else {
            return;
        };
        if let Err(e) = self.place(hwnd, target, &mon, &layout, last.zones, size_of(info.rect)) {
            tracing::debug!(error = %e, "new window not moved to its app's zone");
        }
    }

    fn begin_drag(&mut self, hwnd: Hwnd, settings: &Settings) -> Result<()> {
        self.cancel();
        // Windows that cannot be arranged are simply not tracked.
        let Ok(info) = eligible(hwnd, settings) else {
            return Ok(());
        };
        self.drag = Some(Drag {
            hwnd,
            size: size_of(info.rect),
            size_changes: 0,
            timer: set_timer(TICK_MS)?,
            monitor: None,
            rects: Vec::new(),
            layout: String::new(),
            anchor: None,
            selection: Vec::new(),
        });
        Ok(())
    }

    fn tick(&mut self, settings: &Settings) -> Result<()> {
        let Some(d) = self.drag.as_ref() else {
            return Ok(());
        };
        let Ok(rect) = window::visible_rect(d.hwnd) else {
            self.cancel();
            return Ok(());
        };
        let (w, h) = size_of(rect);
        if (w - d.size.0).abs() > RESIZE_TOLERANCE || (h - d.size.1).abs() > RESIZE_TOLERANCE {
            if d.size_changes >= MAX_SIZE_CHANGES_WHILE_MOVING {
                // A resize, not a move: zones do not apply.
                self.cancel();
                return Ok(());
            }
            let d = self.drag.as_mut().expect("checked above");
            d.size = (w, h);
            d.size_changes += 1;
        }
        if !drag_key_held(settings.drag_modifier) {
            if let Some(d) = self.drag.as_mut() {
                d.selection.clear();
                d.anchor = None;
            }
            self.hide();
            return Ok(());
        }
        let cursor = cursor_pos()?;
        let mon = monitor::from_point(cursor.0, cursor.1).ok_or_else(|| anyhow!("the cursor is not on a monitor"))?;
        let (layout, rects) = monitor_zones(settings, &mon);
        let current = engine::zone_at(&rects, cursor, slack(settings, &mon));
        let spanning = span_key_held(settings.drag_modifier);
        let d = self.drag.as_mut().expect("checked above");
        if d.monitor.as_ref().map(|m| &m.id) != Some(&mon.id) || d.layout != layout {
            d.anchor = None;
        }
        d.selection = match (spanning, current) {
            (true, Some(c)) => {
                let a = *d.anchor.get_or_insert(c);
                engine::span(&rects, a, c)
            }
            (false, Some(c)) => {
                d.anchor = None;
                vec![c]
            }
            (_, None) => Vec::new(),
        };
        d.monitor = Some(mon.clone());
        d.rects = rects.clone();
        d.layout = layout;
        let selection = d.selection.clone();
        self.show(settings, &mon, rects, selection)
    }

    fn end_drag(&mut self, settings: &Settings) -> Result<()> {
        let Some(d) = self.drag.take() else {
            return Ok(());
        };
        self.cancel();
        let target = engine::union(&d.rects, &d.selection);
        match (target, d.monitor.as_ref()) {
            (Some(target), Some(mon)) => {
                let restore = self.snapped.get(&d.hwnd.0).map_or(d.size, |s| s.restore);
                self.place(d.hwnd, target, mon, &d.layout, d.selection.clone(), restore)
            }
            _ => {
                let Some(s) = self.snapped.remove(&d.hwnd.0) else {
                    return Ok(());
                };
                // Taken out of its zone: the app's next window opens where Windows puts it.
                if let Ok(info) = window::info(d.hwnd) {
                    self.last_changed |= self.last.remove(&info.exe).is_some();
                }
                if !settings.restore_size_on_unsnap || s.restore == d.size {
                    return Ok(());
                }
                let rect = window::visible_rect(d.hwnd).map_err(friendly)?;
                let work = monitor::from_window(d.hwnd).map_or(rect, |m| m.work_area);
                let cursor_x = cursor_pos().map_or(rect.center().0, |c| c.0);
                let target = engine::unsnap_rect(rect, cursor_x, s.restore, work);
                window::set_visible_rect(d.hwnd, target).map_err(friendly)
            }
        }
    }

    fn place(
        &mut self,
        hwnd: Hwnd,
        target: Rect,
        mon: &Monitor,
        layout: &str,
        zones: Vec<usize>,
        restore: (i32, i32),
    ) -> Result<()> {
        window::set_visible_rect(hwnd, target).map_err(friendly)?;
        let pid = window::thread_process(hwnd).map_or(0, |(pid, _)| pid);
        self.seen.insert(hwnd.0);
        if let Some(exe) = window::process_exe(pid).filter(|e| !e.is_empty()) {
            let last = LastZone {
                monitor: mon.id.clone(),
                layout: layout.to_string(),
                zones: zones.clone(),
            };
            if self.last.get(&exe) != Some(&last) {
                self.last.insert(exe, last);
                self.last_changed = true;
            }
        }
        self.snapped.insert(
            hwnd.0,
            Snapped {
                pid,
                monitor: mon.id.clone(),
                layout: layout.to_string(),
                zones,
                restore,
            },
        );
        Ok(())
    }

    /// The snap record for a window, if it still describes the same process.
    fn record(&self, hwnd: Hwnd, pid: u32) -> Option<&Snapped> {
        self.snapped.get(&hwnd.0).filter(|s| s.pid == pid)
    }

    fn foreground(settings: &Settings) -> Result<WindowInfo> {
        let hwnd = window::foreground().ok_or_else(|| anyhow!("there is no active window"))?;
        eligible(hwnd, settings)
    }

    fn restore_size(&self, info: &WindowInfo) -> (i32, i32) {
        self.record(info.hwnd, info.pid)
            .map_or(size_of(info.rect), |s| s.restore)
    }

    fn snap_to_zone(&mut self, info: &WindowInfo, settings: &Settings, mon: &Monitor, zone: usize) -> Result<()> {
        let (layout, rects) = monitor_zones(settings, mon);
        let Some(target) = rects.get(zone).copied() else {
            bail!(
                "The layout on this monitor has {} zones; there is no zone {}",
                rects.len(),
                zone + 1
            );
        };
        let restore = self.restore_size(info);
        self.place(info.hwnd, target, mon, &layout, vec![zone], restore)
    }

    pub(crate) fn foreground_to_zone(&mut self, settings: &Settings, zone: usize) -> Result<()> {
        let info = Self::foreground(settings)?;
        let mon = monitor::from_window(info.hwnd).ok_or_else(|| anyhow!("the window is not on a monitor"))?;
        self.snap_to_zone(&info, settings, &mon, zone)
    }

    pub(crate) fn step_foreground(&mut self, settings: &Settings, forward: bool) -> Result<()> {
        let info = Self::foreground(settings)?;
        let mon = monitor::from_window(info.hwnd).ok_or_else(|| anyhow!("the window is not on a monitor"))?;
        let (layout, rects) = monitor_zones(settings, &mon);
        let current = match self.record(info.hwnd, info.pid) {
            Some(s) if s.monitor == mon.id && s.layout == layout && s.zones.len() == 1 => Some(s.zones[0]),
            _ => engine::zone_of(&rects, info.rect),
        };
        let Some(step) = engine::step(current, rects.len(), forward, settings.move_across_monitors) else {
            return Ok(());
        };
        match step {
            Step::Zone(i) => self.snap_to_zone(&info, settings, &mon, i),
            Step::NextMonitor | Step::PrevMonitor => {
                let mut all = monitor::enumerate()?;
                all.sort_by_key(|m| (m.rect.left, m.rect.top));
                let here = all.iter().position(|m| m.id == mon.id).unwrap_or(0);
                let n = all.len();
                let next = if step == Step::NextMonitor {
                    &all[(here + 1) % n]
                } else {
                    &all[(here + n - 1) % n]
                };
                let count = monitor_zones(settings, next).1.len();
                let zone = if step == Step::NextMonitor { 0 } else { count - 1 };
                self.snap_to_zone(&info, settings, next, zone)
            }
        }
    }

    pub(crate) fn toggle_overlay(&mut self, settings: &Settings) -> Result<()> {
        if self.pinned_overlay || self.drag.is_some() {
            self.cancel();
            return Ok(());
        }
        let (x, y) = cursor_pos()?;
        let mon = monitor::from_point(x, y).ok_or_else(|| anyhow!("the cursor is not on a monitor"))?;
        let (_, rects) = monitor_zones(settings, &mon);
        self.show(settings, &mon, rects, Vec::new())?;
        self.pinned_overlay = true;
        Ok(())
    }

    /// Switch the monitor under the cursor to the next layout and move the windows
    /// snapped there into the matching zones of the new one. Returns the monitor id and
    /// the new layout id.
    pub(crate) fn cycle_layout(&mut self, settings: &Settings) -> Result<(String, String)> {
        let (x, y) = cursor_pos()?;
        let mon = monitor::from_point(x, y).ok_or_else(|| anyhow!("the cursor is not on a monitor"))?;
        let current = settings.layout_for_monitor(&mon.id).id.clone();
        let next = crate::next_layout(settings, &current).id.clone();
        let mut updated = settings.clone();
        updated.monitor_layouts.insert(mon.id.clone(), next.clone());
        let (_, rects) = monitor_zones(&updated, &mon);

        let on_monitor: Vec<isize> = self
            .snapped
            .iter()
            .filter(|(_, s)| s.monitor == mon.id)
            .map(|(h, _)| *h)
            .collect();
        for h in on_monitor {
            let s = self.snapped.remove(&h).expect("key listed above");
            let hwnd = Hwnd(h);
            let same_process = window::thread_process(hwnd).map(|(pid, _)| pid) == Some(s.pid);
            if !same_process {
                continue;
            }
            // Windows whose zones do not exist in the new layout stay where they are.
            if let Some(target) = (s.zones.iter().all(|z| *z < rects.len()))
                .then(|| engine::union(&rects, &s.zones))
                .flatten()
            {
                if let Err(e) = self.place(hwnd, target, &mon, &next, s.zones.clone(), s.restore) {
                    tracing::warn!(error = %e, "could not move a window into the new layout");
                }
            }
        }
        if !self.pinned_overlay && self.drag.is_none() {
            self.stop_flash();
            self.flash = Some(set_timer(FLASH_MS)?);
        }
        if self.flash.is_some() || self.pinned_overlay {
            self.show(&updated, &mon, rects, Vec::new())?;
        }
        Ok((mon.id, next))
    }
}
