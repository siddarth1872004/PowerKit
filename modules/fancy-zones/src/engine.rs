//! Pure zone geometry: turning a layout into pixel rectangles, finding the zone under
//! the cursor, spanning several zones, and keyboard navigation between zones.

use powerkit_config::{Layout, UNITS};
use powerkit_core::Rect;

/// Pixel rectangles of a layout's zones inside `area` (a monitor work area).
/// Edges on the border of the work area are inset by `margin`, edges between zones by
/// half of `spacing` on each side, so neighbouring zones are exactly `spacing` apart.
/// A zone too small to take its gaps keeps its unpadded rectangle.
pub fn zone_rects(layout: &Layout, area: Rect, margin: i32, spacing: i32) -> Vec<Rect> {
    let px = |origin: i32, extent: i32, u: u32| origin + (u as i64 * extent as i64 / UNITS as i64) as i32;
    let (lead, trail) = (spacing - spacing / 2, spacing / 2);
    let pad_start = |u: u32| if u == 0 { margin } else { lead };
    let pad_end = |u: u32| if u >= UNITS { margin } else { trail };
    layout
        .zones
        .iter()
        .map(|z| {
            let raw = Rect::new(
                px(area.left, area.width(), z.x),
                px(area.top, area.height(), z.y),
                px(area.left, area.width(), z.x + z.w),
                px(area.top, area.height(), z.y + z.h),
            );
            let padded = Rect::new(
                raw.left + pad_start(z.x),
                raw.top + pad_start(z.y),
                raw.right - pad_end(z.x + z.w),
                raw.bottom - pad_end(z.y + z.h),
            );
            if padded.is_empty() {
                raw
            } else {
                padded
            }
        })
        .collect()
}

fn distance(r: &Rect, (x, y): (i32, i32)) -> i64 {
    let dx = (r.left - x).max(x - (r.right - 1)).max(0) as i64;
    let dy = (r.top - y).max(y - (r.bottom - 1)).max(0) as i64;
    dx.max(dy)
}

/// Zone under a point. Overlapping zones resolve to the smallest one (it is the one
/// drawn on top); a point in a gap picks the nearest zone within `slack` pixels.
pub fn zone_at(rects: &[Rect], point: (i32, i32), slack: i32) -> Option<usize> {
    let inside = rects
        .iter()
        .enumerate()
        .filter(|(_, r)| r.contains(point.0, point.1))
        .min_by_key(|(_, r)| r.area())
        .map(|(i, _)| i);
    inside.or_else(|| {
        rects
            .iter()
            .enumerate()
            .map(|(i, r)| (i, distance(r, point)))
            .filter(|(_, d)| *d <= slack.max(0) as i64)
            .min_by_key(|(_, d)| *d)
            .map(|(i, _)| i)
    })
}

/// Bounding box of the given zones.
pub fn union(rects: &[Rect], zones: &[usize]) -> Option<Rect> {
    zones.iter().filter_map(|i| rects.get(*i)).fold(None, |acc, r| {
        Some(match acc {
            None => *r,
            Some(a) => Rect::new(
                a.left.min(r.left),
                a.top.min(r.top),
                a.right.max(r.right),
                a.bottom.max(r.bottom),
            ),
        })
    })
}

/// Every zone a window dragged from zone `a` to zone `b` should cover: the zones lying
/// mostly inside the bounding box of `a` and `b`, in index order.
pub fn span(rects: &[Rect], a: usize, b: usize) -> Vec<usize> {
    if a >= rects.len() || b >= rects.len() {
        return Vec::new();
    }
    let Some(bounds) = union(rects, &[a, b]) else {
        return Vec::new();
    };
    rects
        .iter()
        .enumerate()
        .filter(|(i, r)| *i == a || *i == b || r.intersect(&bounds).is_some_and(|x| x.area() * 2 >= r.area()))
        .map(|(i, _)| i)
        .collect()
}

/// Zones whose union a window currently fills (within a small tolerance), so keyboard
/// moves continue from where the window is. `None` when it is not in any zone.
pub fn zone_of(rects: &[Rect], window: Rect) -> Option<usize> {
    let overlap = |r: &Rect| r.intersect(&window).map_or(0, |x| x.area());
    rects
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let common = overlap(r);
            // Intersection over union, in percent.
            let iou = common * 100 / (r.area() + window.area() - common).max(1);
            (i, iou)
        })
        .filter(|(_, iou)| *iou >= 80)
        .max_by_key(|(_, iou)| *iou)
        .map(|(i, _)| i)
}

/// Where a keyboard zone move goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Zone(usize),
    /// Continue on the next monitor (its first zone, or last zone when moving back).
    NextMonitor,
    PrevMonitor,
}

pub fn step(current: Option<usize>, count: usize, forward: bool, across_monitors: bool) -> Option<Step> {
    if count == 0 {
        return None;
    }
    Some(match (current, forward) {
        (None, true) => Step::Zone(0),
        (None, false) => Step::Zone(count - 1),
        (Some(i), true) if i + 1 < count => Step::Zone(i + 1),
        (Some(i), false) if i > 0 && i < count => Step::Zone(i - 1),
        (Some(_), true) if across_monitors => Step::NextMonitor,
        (Some(_), false) if across_monitors => Step::PrevMonitor,
        (Some(_), true) => Step::Zone(0),
        (Some(_), false) => Step::Zone(count - 1),
    })
}

/// Where a window dragged out of its zone goes when it gets its old size back: same
/// top edge as where it was dropped, centred horizontally under the cursor, kept inside
/// the work area.
pub fn unsnap_rect(dropped: Rect, cursor_x: i32, size: (i32, i32), work_area: Rect) -> Rect {
    let (w, h) = (size.0.max(1), size.1.max(1));
    Rect::from_size(cursor_x - w / 2, dropped.top, w, h).clamp_into(&work_area)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 500);

    #[test]
    fn columns_get_margins_and_exact_spacing() {
        let r = zone_rects(&Layout::columns(2), AREA, 10, 8);
        assert_eq!(r, vec![Rect::new(10, 10, 496, 490), Rect::new(504, 10, 990, 490)]);
        assert_eq!(r[1].left - r[0].right, 8);
    }

    #[test]
    fn odd_spacing_still_separates_by_exactly_spacing() {
        let r = zone_rects(&Layout::grid(3, 3), Rect::new(-1920, 40, 0, 1080), 0, 7);
        assert_eq!(r.len(), 9);
        assert_eq!(r[1].left - r[0].right, 7);
        assert_eq!(r[3].top - r[0].bottom, 7);
        assert_eq!((r[0].left, r[0].top), (-1920, 40));
        assert_eq!((r[8].right, r[8].bottom), (0, 1080));
    }

    #[test]
    fn tiny_zones_keep_their_raw_rect() {
        let r = zone_rects(&Layout::columns(12), Rect::new(0, 0, 60, 60), 30, 30);
        assert!(r.iter().all(|z| !z.is_empty()), "{r:?}");
    }

    #[test]
    fn hit_test_inside_gap_and_far_away() {
        let r = zone_rects(&Layout::columns(2), AREA, 10, 8);
        assert_eq!(zone_at(&r, (100, 100), 8), Some(0));
        assert_eq!(zone_at(&r, (989, 489), 8), Some(1));
        // In the gap, closer to the right zone.
        assert_eq!(zone_at(&r, (502, 100), 8), Some(1));
        assert_eq!(zone_at(&r, (5000, 100), 8), None);
    }

    #[test]
    fn overlapping_zones_prefer_the_smallest() {
        let r = vec![Rect::new(0, 0, 100, 100), Rect::new(40, 40, 60, 60)];
        assert_eq!(zone_at(&r, (50, 50), 0), Some(1));
        assert_eq!(zone_at(&r, (10, 10), 0), Some(0));
    }

    #[test]
    fn span_covers_the_zones_between() {
        let r = zone_rects(&Layout::grid(3, 2), AREA, 0, 0);
        assert_eq!(span(&r, 0, 4), vec![0, 1, 3, 4]);
        assert_eq!(span(&r, 2, 2), vec![2]);
        assert_eq!(span(&r, 5, 0), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(union(&r, &span(&r, 0, 4)), Some(Rect::new(0, 0, 666, 500)));
        assert!(span(&r, 0, 99).is_empty());
    }

    #[test]
    fn zone_of_needs_a_close_match() {
        let r = zone_rects(&Layout::columns(2), AREA, 0, 0);
        assert_eq!(zone_of(&r, Rect::new(2, 0, 500, 498)), Some(0));
        assert_eq!(zone_of(&r, Rect::new(200, 0, 700, 500)), None);
    }

    #[test]
    fn stepping() {
        assert_eq!(step(None, 3, true, true), Some(Step::Zone(0)));
        assert_eq!(step(None, 3, false, true), Some(Step::Zone(2)));
        assert_eq!(step(Some(1), 3, true, true), Some(Step::Zone(2)));
        assert_eq!(step(Some(2), 3, true, true), Some(Step::NextMonitor));
        assert_eq!(step(Some(2), 3, true, false), Some(Step::Zone(0)));
        assert_eq!(step(Some(0), 3, false, true), Some(Step::PrevMonitor));
        assert_eq!(step(Some(0), 3, false, false), Some(Step::Zone(2)));
        assert_eq!(step(Some(7), 3, false, false), Some(Step::Zone(2)));
        assert_eq!(step(None, 0, true, true), None);
    }

    #[test]
    fn unsnap_centres_under_cursor_and_stays_on_screen() {
        let work = Rect::new(0, 0, 1920, 1040);
        let dropped = Rect::new(100, 200, 1060, 1040);
        assert_eq!(
            unsnap_rect(dropped, 600, (800, 600), work),
            Rect::from_size(200, 200, 800, 600)
        );
        assert_eq!(
            unsnap_rect(dropped, 10, (800, 600), work),
            Rect::from_size(0, 200, 800, 600)
        );
        assert_eq!(
            unsnap_rect(dropped, 10, (800, 2000), work),
            Rect::from_size(0, 0, 800, 1040)
        );
    }

    proptest! {
        #[test]
        fn grid_zones_stay_inside_and_disjoint(
            cols in 1u32..=6, rows in 1u32..=6,
            l in -4000i32..4000, t in -4000i32..4000, w in 200i32..5000, h in 200i32..5000,
            margin in 0i32..64, spacing in 0i32..64,
        ) {
            let area = Rect::from_size(l, t, w, h);
            let r = zone_rects(&Layout::grid(cols, rows), area, margin, spacing);
            for (i, a) in r.iter().enumerate() {
                prop_assert!(!a.is_empty());
                prop_assert!(a.intersect(&area) == Some(*a));
                for b in &r[i + 1..] {
                    prop_assert!(a.intersect(b).is_none(), "{a:?} {b:?}");
                }
                let c = a.center();
                prop_assert_eq!(zone_at(&r, c, 0), Some(i));
            }
        }
    }
}
