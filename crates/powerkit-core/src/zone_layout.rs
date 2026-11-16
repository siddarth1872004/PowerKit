//! FancyZones layouts as the layout editor changes them: zones in units of the
//! monitor's work area, and the edits the editor makes (splitting a zone, dragging the
//! line between zones, removing a zone).

/// Units per work-area width or height; the same as the configuration's.
pub const ZONE_UNITS: u32 = 10_000;
/// Smallest zone edge, in units.
pub const MIN_ZONE: u32 = 200;
pub const MAX_ZONES: usize = 40;

/// `[x, y, w, h]` in [`ZONE_UNITS`].
pub type ZoneRect = [u32; 4];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneLayout {
    pub id: String,
    pub name: String,
    pub zones: Vec<ZoneRect>,
    /// Pixels at 96 DPI.
    pub spacing: u32,
    pub margin: u32,
}

/// A line between zones that can be dragged: the zones on either side whose edges
/// meet on it and touch each other along it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// A line running top to bottom (it moves left and right).
    pub vertical: bool,
    pub at: u32,
    /// Zones that end on the line.
    pub before: Vec<usize>,
    /// Zones that start on the line.
    pub after: Vec<usize>,
}

/// `(start, size)` across the line and `(start, size)` along it.
fn axes(z: &ZoneRect, vertical: bool) -> ((u32, u32), (u32, u32)) {
    if vertical {
        ((z[0], z[2]), (z[1], z[3]))
    } else {
        ((z[1], z[3]), (z[0], z[2]))
    }
}

/// The line within `tolerance` of `(x, y)`, nearest first. Lines on the edge of the
/// work area are not lines between zones and are not found.
pub fn split_at(zones: &[ZoneRect], x: u32, y: u32, tolerance: u32) -> Option<Split> {
    let mut best: Option<(u32, Split)> = None;
    for vertical in [true, false] {
        let (across, along) = if vertical { (x, y) } else { (y, x) };
        for z in zones {
            let ((start, size), _) = axes(z, vertical);
            for at in [start, start + size] {
                if at == 0 || at >= ZONE_UNITS {
                    continue;
                }
                let d = at.abs_diff(across);
                if d > tolerance || best.as_ref().is_some_and(|(bd, _)| *bd <= d) {
                    continue;
                }
                if let Some(s) = line(zones, vertical, at, along) {
                    best = Some((d, s));
                }
            }
        }
    }
    best.map(|(_, s)| s)
}

/// The zones with an edge on the line `at`, connected along it to the point `along`.
fn line(zones: &[ZoneRect], vertical: bool, at: u32, along: u32) -> Option<Split> {
    let edges: Vec<(usize, bool, u32, u32)> = zones
        .iter()
        .enumerate()
        .filter_map(|(i, z)| {
            let ((s, n), (a, m)) = axes(z, vertical);
            if s + n == at {
                Some((i, true, a, a + m))
            } else if s == at {
                Some((i, false, a, a + m))
            } else {
                None
            }
        })
        .collect();
    let (mut lo, mut hi) = edges
        .iter()
        .find(|e| e.2 <= along && along <= e.3)
        .map(|e| (e.2, e.3))?;
    let mut taken = vec![false; edges.len()];
    loop {
        let mut grew = false;
        for (t, e) in taken.iter_mut().zip(&edges) {
            if !*t && e.2 <= hi && e.3 >= lo {
                *t = true;
                lo = lo.min(e.2);
                hi = hi.max(e.3);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let pick = |end: bool| -> Vec<usize> {
        edges
            .iter()
            .zip(&taken)
            .filter(|(e, t)| **t && e.1 == end)
            .map(|(e, _)| e.0)
            .collect()
    };
    Some(Split {
        vertical,
        at,
        before: pick(true),
        after: pick(false),
    })
}

/// Move the line `split` towards `to`, as far as every zone on it keeps its smallest
/// size; returns where it ended up.
pub fn move_split(zones: &mut [ZoneRect], split: &mut Split, to: u32) -> u32 {
    let (lo_i, hi_i) = if split.vertical { (0, 2) } else { (1, 3) };
    let lo = split
        .before
        .iter()
        .map(|&i| zones[i][lo_i] + MIN_ZONE)
        .max()
        .unwrap_or(MIN_ZONE);
    let hi = split
        .after
        .iter()
        .map(|&i| (zones[i][lo_i] + zones[i][hi_i]).saturating_sub(MIN_ZONE))
        .min()
        .unwrap_or(ZONE_UNITS - MIN_ZONE);
    if lo > hi {
        return split.at;
    }
    let at = to.clamp(lo, hi);
    for &i in &split.before {
        zones[i][hi_i] = at - zones[i][lo_i];
    }
    for &i in &split.after {
        let end = zones[i][lo_i] + zones[i][hi_i];
        zones[i][lo_i] = at;
        zones[i][hi_i] = end - at;
    }
    split.at = at;
    at
}

/// The zone drawn on top at `(x, y)`.
pub fn zone_at(zones: &[ZoneRect], x: u32, y: u32) -> Option<usize> {
    zones
        .iter()
        .rposition(|z| z[0] <= x && x < z[0] + z[2] && z[1] <= y && y < z[1] + z[3])
}

/// Cut zone `i` in two at `at` with a line top to bottom (`vertical`) or across.
/// Nothing happens when either half would be too small or there are too many zones.
pub fn split_zone(zones: &mut Vec<ZoneRect>, i: usize, at: u32, vertical: bool) -> bool {
    let Some(&z) = zones.get(i) else { return false };
    let ((start, size), _) = axes(&z, vertical);
    if zones.len() >= MAX_ZONES || at < start + MIN_ZONE || at + MIN_ZONE > start + size {
        return false;
    }
    let (mut first, mut second) = (z, z);
    let (s, n) = if vertical { (0, 2) } else { (1, 3) };
    first[n] = at - start;
    second[s] = at;
    second[n] = start + size - at;
    zones[i] = first;
    zones.insert(i + 1, second);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u32 = ZONE_UNITS / 2;

    fn quad() -> Vec<ZoneRect> {
        vec![[0, 0, H, H], [H, 0, H, H], [0, H, H, H], [H, H, H, H]]
    }

    #[test]
    fn finds_the_whole_line_between_zones() {
        let s = split_at(&quad(), H + 30, 100, 50).unwrap();
        assert!(s.vertical);
        assert_eq!(s.at, H);
        assert_eq!(s.before, [0, 2]);
        assert_eq!(s.after, [1, 3]);
        assert!(split_at(&quad(), H + 300, 100, 50).is_none());
        // The edge of the work area is not a line between zones.
        assert!(split_at(&quad(), 10, 100, 50).is_none());
    }

    #[test]
    fn a_line_stops_where_the_zones_stop_touching() {
        // Left half split in two at 3000; right half split at 7000.
        let zones = vec![[0, 0, H, 3000], [0, 3000, H, 7000], [H, 0, H, 7000], [H, 7000, H, 3000]];
        let s = split_at(&zones, 2000, 3010, 50).unwrap();
        assert!(!s.vertical);
        assert_eq!((s.before, s.after), (vec![0], vec![1]));
    }

    #[test]
    fn moving_a_line_resizes_both_sides_and_keeps_the_smallest_size() {
        let mut zones = quad();
        let mut s = split_at(&zones, H, 100, 50).unwrap();
        assert_eq!(move_split(&mut zones, &mut s, 3000), 3000);
        assert_eq!(zones[0], [0, 0, 3000, H]);
        assert_eq!(zones[3], [3000, H, 7000, H]);
        assert_eq!(move_split(&mut zones, &mut s, 0), MIN_ZONE);
        assert_eq!(move_split(&mut zones, &mut s, ZONE_UNITS), ZONE_UNITS - MIN_ZONE);
        assert_eq!(zones[1], [ZONE_UNITS - MIN_ZONE, 0, MIN_ZONE, H]);
    }

    #[test]
    fn splitting_a_zone_puts_the_new_half_after_it() {
        let mut zones = vec![[0, 0, ZONE_UNITS, ZONE_UNITS]];
        assert!(split_zone(&mut zones, 0, 4000, true));
        assert_eq!(zones, [[0, 0, 4000, ZONE_UNITS], [4000, 0, 6000, ZONE_UNITS]]);
        assert!(split_zone(&mut zones, 1, 2500, false));
        assert_eq!(zones[2], [4000, 2500, 6000, 7500]);
        assert!(!split_zone(&mut zones, 0, 100, true));
        assert_eq!(zone_at(&zones, 5000, 9000), Some(2));
        assert_eq!(zone_at(&zones, ZONE_UNITS, 0), None);
    }
}
