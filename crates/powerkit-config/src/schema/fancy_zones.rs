use super::{clamp, fix_apps, fix_color, fix_shortcut};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Zone coordinates are expressed in this many units per monitor work-area dimension,
/// which makes layouts independent of resolution and DPI.
pub const UNITS: u32 = 10_000;
/// Smallest zone edge, in units (2% of the work area).
pub const MIN_ZONE_UNITS: u32 = 200;
pub const MAX_ZONES: usize = 40;
pub const MAX_LAYOUTS: usize = 64;

/// One zone of a layout, in [`UNITS`] relative to the monitor work area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Zone {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Default for Zone {
    fn default() -> Self {
        Self {
            name: String::new(),
            x: 0,
            y: 0,
            w: UNITS,
            h: UNITS,
        }
    }
}

impl Zone {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self {
            name: String::new(),
            x,
            y,
            w,
            h,
        }
    }

    /// Clamp into the unit square, enforcing the minimum size.
    pub fn normalize(&mut self) {
        self.w = self.w.clamp(MIN_ZONE_UNITS, UNITS);
        self.h = self.h.clamp(MIN_ZONE_UNITS, UNITS);
        self.x = self.x.min(UNITS - self.w);
        self.y = self.y.min(UNITS - self.h);
        if self.name.chars().count() > 40 {
            self.name = self.name.chars().take(40).collect();
        }
    }
}

/// Where a layout came from; purely informational (the zones are the source of truth).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    Columns,
    Rows,
    Grid,
    Priority,
    #[default]
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub id: String,
    pub name: String,
    pub kind: LayoutKind,
    pub zones: Vec<Zone>,
    /// Gap between neighbouring zones, in pixels at 96 DPI.
    pub spacing: u32,
    /// Gap between zones and the edge of the work area, in pixels at 96 DPI.
    pub margin: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "Layout".into(),
            kind: LayoutKind::Custom,
            zones: Vec::new(),
            spacing: 8,
            margin: 8,
        }
    }
}

/// Split `UNITS` into `n` contiguous spans that sum exactly to `UNITS`.
fn spans(n: u32) -> Vec<(u32, u32)> {
    (0..n)
        .map(|i| {
            let start = UNITS * i / n;
            let end = UNITS * (i + 1) / n;
            (start, end - start)
        })
        .collect()
}

impl Layout {
    fn template(id: &str, name: &str, kind: LayoutKind, zones: Vec<Zone>) -> Layout {
        Layout {
            id: id.into(),
            name: name.into(),
            kind,
            zones,
            ..Default::default()
        }
    }

    /// `n` equal columns.
    pub fn columns(n: u32) -> Layout {
        let n = n.clamp(1, 12);
        let zones = spans(n).into_iter().map(|(x, w)| Zone::new(x, 0, w, UNITS)).collect();
        Self::template(
            &format!("columns-{n}"),
            &format!("{n} columns"),
            LayoutKind::Columns,
            zones,
        )
    }

    /// `n` equal rows.
    pub fn rows(n: u32) -> Layout {
        let n = n.clamp(1, 12);
        let zones = spans(n).into_iter().map(|(y, h)| Zone::new(0, y, UNITS, h)).collect();
        Self::template(&format!("rows-{n}"), &format!("{n} rows"), LayoutKind::Rows, zones)
    }

    /// A `cols` x `rows` grid.
    pub fn grid(cols: u32, rows: u32) -> Layout {
        let (cols, rows) = (cols.clamp(1, 6), rows.clamp(1, 6));
        let mut zones = Vec::new();
        for (y, h) in spans(rows) {
            for (x, w) in spans(cols) {
                zones.push(Zone::new(x, y, w, h));
            }
        }
        Self::template(
            &format!("grid-{cols}x{rows}"),
            &format!("{cols} x {rows} grid"),
            LayoutKind::Grid,
            zones,
        )
    }

    /// One wide primary zone with `n - 1` narrower side zones (side columns for
    /// n <= 3, a stacked side column otherwise).
    pub fn priority(n: u32) -> Layout {
        let n = n.clamp(1, 8);
        let zones = match n {
            1 => vec![Zone::new(0, 0, UNITS, UNITS)],
            2 => vec![Zone::new(0, 0, 6_600, UNITS), Zone::new(6_600, 0, 3_400, UNITS)],
            3 => vec![
                Zone::new(0, 0, 2_500, UNITS),
                Zone::new(2_500, 0, 5_000, UNITS),
                Zone::new(7_500, 0, 2_500, UNITS),
            ],
            _ => {
                let mut z = vec![Zone::new(0, 0, 6_000, UNITS)];
                z.extend(spans(n - 1).into_iter().map(|(y, h)| Zone::new(6_000, y, 4_000, h)));
                z
            }
        };
        Self::template(
            &format!("priority-{n}"),
            &format!("Priority ({n} zones)"),
            LayoutKind::Priority,
            zones,
        )
    }

    /// Layouts shipped with a fresh configuration.
    pub fn builtin() -> Vec<Layout> {
        vec![
            Layout::priority(3),
            Layout::columns(2),
            Layout::columns(3),
            Layout::rows(2),
            Layout::grid(2, 2),
            Layout::priority(4),
        ]
    }

    pub(crate) fn sanitize(&mut self, index: usize, w: &mut Vec<String>) {
        let valid_id = !self.id.is_empty()
            && self.id.len() <= 64
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid_id {
            self.id = format!("layout-{index}");
        }
        if self.name.trim().is_empty() {
            self.name = format!("Layout {}", index + 1);
        } else if self.name.chars().count() > 60 {
            self.name = self.name.chars().take(60).collect();
        }
        if self.zones.len() > MAX_ZONES {
            w.push(format!(
                "layout {:?}: only the first {MAX_ZONES} zones are kept",
                self.name
            ));
            self.zones.truncate(MAX_ZONES);
        }
        self.zones.iter_mut().for_each(Zone::normalize);
        if self.zones.is_empty() {
            w.push(format!(
                "layout {:?} had no zones; a full-screen zone was added",
                self.name
            ));
            self.zones.push(Zone::default());
        }
        self.spacing = self.spacing.min(64);
        self.margin = self.margin.min(64);
    }
}

/// Modifier that must be held while dragging a window to show zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DragModifier {
    #[default]
    Shift,
    Ctrl,
    Alt,
    /// Zones appear on every drag.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FancyZones {
    pub enabled: bool,
    pub layouts: Vec<Layout>,
    /// Layout used by monitors without an explicit assignment.
    pub default_layout: String,
    /// Monitor id (stable device name, see `powerkit_platform::monitor`) -> layout id.
    pub monitor_layouts: BTreeMap<String, String>,
    pub drag_modifier: DragModifier,
    /// Give a window its previous size back when it is dragged out of a zone.
    pub restore_size_on_unsnap: bool,
    /// Keyboard zone moves may continue onto the next monitor.
    pub move_across_monitors: bool,
    pub zone_color: String,
    pub highlight_color: String,
    /// Opacity of the zone overlay, 10-90 percent.
    pub overlay_opacity: u32,
    pub next_zone_shortcut: String,
    pub prev_zone_shortcut: String,
    pub cycle_layout_shortcut: String,
    pub toggle_overlay_shortcut: String,
    /// Modifiers which, combined with 1-9, send the active window to that zone
    /// (for example `Win+Ctrl+Alt`). Empty disables these shortcuts.
    pub zone_number_modifiers: String,
    /// Executable names FancyZones never moves.
    pub excluded_apps: Vec<String>,
}

impl Default for FancyZones {
    fn default() -> Self {
        Self {
            enabled: true,
            layouts: Layout::builtin(),
            default_layout: "priority-3".into(),
            monitor_layouts: BTreeMap::new(),
            drag_modifier: DragModifier::Shift,
            restore_size_on_unsnap: true,
            move_across_monitors: true,
            zone_color: "#3B82F6".into(),
            highlight_color: "#0F7BFF".into(),
            overlay_opacity: 45,
            next_zone_shortcut: "Win+Ctrl+Alt+Right".into(),
            prev_zone_shortcut: "Win+Ctrl+Alt+Left".into(),
            cycle_layout_shortcut: "Win+Ctrl+Alt+L".into(),
            toggle_overlay_shortcut: "Win+Ctrl+Alt+Z".into(),
            zone_number_modifiers: "Win+Ctrl+Alt".into(),
            excluded_apps: Vec::new(),
        }
    }
}

impl FancyZones {
    pub fn layout(&self, id: &str) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.id == id)
    }

    /// Layout assigned to a monitor, falling back to the default layout.
    pub fn layout_for_monitor(&self, monitor_id: &str) -> &Layout {
        self.monitor_layouts
            .get(monitor_id)
            .and_then(|id| self.layout(id))
            .or_else(|| self.layout(&self.default_layout))
            .unwrap_or(&self.layouts[0])
    }

    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = Self::default();
        if self.layouts.len() > MAX_LAYOUTS {
            w.push(format!("FancyZones: only the first {MAX_LAYOUTS} layouts are kept"));
            self.layouts.truncate(MAX_LAYOUTS);
        }
        if self.layouts.is_empty() {
            w.push("FancyZones: no layouts found; restored the built-in layouts".into());
            self.layouts = Layout::builtin();
        }
        for (i, l) in self.layouts.iter_mut().enumerate() {
            l.sanitize(i, w);
        }
        // Layout ids must be unique: later duplicates get a suffix.
        let mut seen = std::collections::HashSet::new();
        for i in 0..self.layouts.len() {
            let mut id = self.layouts[i].id.clone();
            let mut n = 2;
            while !seen.insert(id.clone()) {
                id = format!("{}-{n}", self.layouts[i].id);
                n += 1;
            }
            self.layouts[i].id = id;
        }
        if self.layout(&self.default_layout).is_none() {
            self.default_layout = self.layouts[0].id.clone();
        }
        let ids: std::collections::HashSet<&str> = self.layouts.iter().map(|l| l.id.as_str()).collect();
        self.monitor_layouts
            .retain(|k, v| !k.is_empty() && k.len() <= 260 && ids.contains(v.as_str()));
        while self.monitor_layouts.len() > 32 {
            self.monitor_layouts.pop_last();
        }
        fix_color(&mut self.zone_color, &d.zone_color, "FancyZones zone color", w);
        fix_color(
            &mut self.highlight_color,
            &d.highlight_color,
            "FancyZones highlight color",
            w,
        );
        clamp(&mut self.overlay_opacity, 10, 90, "FancyZones overlay opacity", w);
        fix_shortcut(
            &mut self.next_zone_shortcut,
            &d.next_zone_shortcut,
            "FancyZones next zone",
            w,
        );
        fix_shortcut(
            &mut self.prev_zone_shortcut,
            &d.prev_zone_shortcut,
            "FancyZones previous zone",
            w,
        );
        fix_shortcut(
            &mut self.cycle_layout_shortcut,
            &d.cycle_layout_shortcut,
            "FancyZones cycle layout",
            w,
        );
        fix_shortcut(
            &mut self.toggle_overlay_shortcut,
            &d.toggle_overlay_shortcut,
            "FancyZones overlay",
            w,
        );
        if !self.zone_number_modifiers.trim().is_empty() {
            let mut probe = format!("{}+1", self.zone_number_modifiers);
            let fallback = format!("{}+1", d.zone_number_modifiers);
            fix_shortcut(&mut probe, &fallback, "FancyZones zone number modifiers", w);
            self.zone_number_modifiers = probe.trim_end_matches("+1").to_string();
        } else {
            self.zone_number_modifiers.clear();
        }
        fix_apps(&mut self.excluded_apps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles_exactly(l: &Layout) -> bool {
        let area: u64 = l.zones.iter().map(|z| z.w as u64 * z.h as u64).sum();
        let inside = l.zones.iter().all(|z| z.x + z.w <= UNITS && z.y + z.h <= UNITS);
        let disjoint = l.zones.iter().enumerate().all(|(i, a)| {
            l.zones[i + 1..]
                .iter()
                .all(|b| a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y)
        });
        inside && disjoint && area == UNITS as u64 * UNITS as u64
    }

    #[test]
    fn templates_tile_the_work_area() {
        for n in 1..=12 {
            assert!(tiles_exactly(&Layout::columns(n)), "columns {n}");
            assert!(tiles_exactly(&Layout::rows(n)), "rows {n}");
        }
        for n in 1..=8 {
            assert!(tiles_exactly(&Layout::priority(n)), "priority {n}");
        }
        for c in 1..=6 {
            for r in 1..=6 {
                assert!(tiles_exactly(&Layout::grid(c, r)), "grid {c}x{r}");
            }
        }
    }

    #[test]
    fn sanitize_fixes_layout_references() {
        let mut fz = FancyZones::default();
        fz.default_layout = "missing".into();
        fz.monitor_layouts.insert("MON1".into(), "missing".into());
        fz.monitor_layouts.insert("MON2".into(), "columns-2".into());
        fz.layouts.push(Layout::columns(2));
        fz.layouts.push(Layout {
            id: "bad id!".into(),
            zones: vec![],
            ..Default::default()
        });
        let mut w = Vec::new();
        fz.sanitize(&mut w);
        assert_eq!(fz.default_layout, "priority-3");
        assert_eq!(fz.monitor_layouts.len(), 1);
        let ids: Vec<&str> = fz.layouts.iter().map(|l| l.id.as_str()).collect();
        assert!(ids.contains(&"columns-2-2"), "{ids:?}");
        assert_eq!(fz.layouts.last().unwrap().zones.len(), 1);
        assert_eq!(fz.layout_for_monitor("MON2").id, "columns-2");
        assert_eq!(fz.layout_for_monitor("other").id, "priority-3");
    }

    #[test]
    fn zone_normalize() {
        let mut z = Zone::new(9_990, 50_000, 0, 20_000);
        z.normalize();
        assert_eq!((z.x, z.y, z.w, z.h), (UNITS - MIN_ZONE_UNITS, 0, MIN_ZONE_UNITS, UNITS));
    }
}
