//! Geometry, timing and gesture detection, free of OS calls.

use powerkit_core::Rect;
use std::collections::VecDeque;

/// Part of an overlay shape; mirrors the platform overlay's pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Rect(Rect),
    CutRect(Rect),
    Ellipse(Rect),
    CutEllipse(Rect),
}

pub fn scale(px96: u32, dpi: u32) -> i32 {
    ((px96 as u64 * dpi as u64 + 48) / 96) as i32
}

/// The rectangle that holds every monitor.
pub fn union(rects: &[Rect]) -> Option<Rect> {
    rects.iter().copied().reduce(|a, b| {
        Rect::new(
            a.left.min(b.left),
            a.top.min(b.top),
            a.right.max(b.right),
            a.bottom.max(b.bottom),
        )
    })
}

/// A square of half-size `r` around a point.
pub fn around((x, y): (i32, i32), r: i32) -> Rect {
    Rect::new(x - r, y - r, x + r, y + r)
}

/// Two lines through the pointer, `thickness` wide, leaving `gap` free on each side of
/// it; arms are `length` long, or reach the edges of `bounds` when `length` is 0.
pub fn crosshair(cursor: (i32, i32), bounds: Rect, thickness: i32, gap: i32, length: i32) -> Vec<Part> {
    let (x, y) = cursor;
    let t = thickness.max(1);
    let (top, left) = (y - t / 2, x - t / 2);
    let (x0, x1, y0, y1) = if length > 0 {
        (x - gap - length, x + gap + length, y - gap - length, y + gap + length)
    } else {
        (bounds.left, bounds.right, bounds.top, bounds.bottom)
    };
    let mut parts = vec![
        Part::Rect(Rect::new(x0, top, x1, top + t)),
        Part::Rect(Rect::new(left, y0, left + t, y1)),
    ];
    if gap > 0 {
        parts.push(Part::CutRect(Rect::new(x - gap, y - gap, x + gap, y + gap)));
    }
    parts
}

/// The dimmed screen with a round hole of `radius` at the pointer.
pub fn spotlight(cursor: (i32, i32), bounds: Rect, radius: i32) -> Vec<Part> {
    vec![Part::Rect(bounds), Part::CutEllipse(around(cursor, radius))]
}

/// Spotlight radius `elapsed_ms` into the opening animation: from `zoom` times `radius`
/// down to `radius`, slowing towards the end.
pub fn spotlight_radius(radius: i32, zoom: u32, elapsed_ms: u64, duration_ms: u64) -> i32 {
    if elapsed_ms >= duration_ms {
        return radius;
    }
    let p = elapsed_ms as f64 / duration_ms as f64;
    let eased = 1.0 - (1.0 - p).powi(3);
    let start = radius as f64 * zoom.max(1) as f64;
    (start + (radius as f64 - start) * eased).round() as i32
}

/// Opacity of a click circle: full while held and for `delay_ms` after release, then
/// fading out over `fade_ms`. `None` once it has gone.
pub fn highlight_alpha(alpha: u8, since_release_ms: Option<u64>, delay_ms: u64, fade_ms: u64) -> Option<u8> {
    let Some(t) = since_release_ms else {
        return Some(alpha);
    };
    if t < delay_ms {
        return Some(alpha);
    }
    let into = t - delay_ms;
    if into >= fade_ms {
        return None;
    }
    let left = 1.0 - into as f64 / fade_ms as f64;
    Some((alpha as f64 * left).round() as u8)
}

pub fn percent_alpha(percent: u32) -> u8 {
    (percent.min(100) * 255 / 100) as u8
}

const VK_CONTROL: u32 = 0x11;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;

/// Recognises Ctrl pressed twice in quick succession with nothing in between.
#[derive(Debug, Default)]
pub struct DoubleCtrl {
    held: bool,
    /// When the first press was released.
    released_at: Option<u64>,
    /// The held press completed a double, so its release starts nothing.
    completed: bool,
}

impl DoubleCtrl {
    pub const WINDOW_MS: u64 = 500;

    /// Whether this event completes a double press.
    pub fn feed(&mut self, vk: u32, down: bool, now_ms: u64) -> bool {
        if !matches!(vk, VK_CONTROL | VK_LCONTROL | VK_RCONTROL) {
            self.released_at = None;
            return false;
        }
        match (down, self.held) {
            (true, true) => false, // auto-repeat
            (true, false) => {
                self.held = true;
                let double = self
                    .released_at
                    .is_some_and(|t| now_ms.saturating_sub(t) <= Self::WINDOW_MS);
                self.released_at = None;
                self.completed = double;
                double
            }
            (false, _) => {
                if self.held && !self.completed {
                    self.released_at = Some(now_ms);
                }
                self.held = false;
                self.completed = false;
                false
            }
        }
    }

    /// Something else happened (a click), so the next press starts over.
    pub fn interrupt(&mut self) {
        self.released_at = None;
    }
}

/// Recognises a quick back-and-forth shake: within the last second the pointer
/// travelled far, but ended up near where it started.
#[derive(Debug, Default)]
pub struct Shake {
    samples: VecDeque<(u64, i32, i32)>,
}

impl Shake {
    const SPAN_MS: u64 = 1000;
    /// Total path, in 96-DPI pixels, a shake must cover.
    const MIN_PATH: u32 = 1200;
    /// Path length divided by the size of the area it stayed in.
    const MIN_RATIO: f64 = 4.0;

    pub fn feed(&mut self, x: i32, y: i32, now_ms: u64, dpi: u32) -> bool {
        self.samples.push_back((now_ms, x, y));
        while self
            .samples
            .front()
            .is_some_and(|s| now_ms.saturating_sub(s.0) > Self::SPAN_MS)
        {
            self.samples.pop_front();
        }
        let mut path = 0.0;
        let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
        for w in self.samples.iter().zip(self.samples.iter().skip(1)) {
            let ((_, ax, ay), (_, bx, by)) = w;
            path += f64::from(bx - ax).hypot(f64::from(by - ay));
        }
        for &(_, sx, sy) in &self.samples {
            (x0, y0, x1, y1) = (x0.min(sx), y0.min(sy), x1.max(sx), y1.max(sy));
        }
        let diagonal = f64::from(x1 - x0).hypot(f64::from(y1 - y0)).max(1.0);
        let shaken = path >= scale(Self::MIN_PATH, dpi) as f64 && path / diagonal >= Self::MIN_RATIO;
        if shaken {
            self.samples.clear();
        }
        shaken
    }
}

/// Largest size with the aspect ratio of `w`x`h` that fits in `max_w`x`max_h`.
pub fn fit(w: i32, h: i32, max_w: i32, max_h: i32) -> (i32, i32) {
    if w <= 0 || h <= 0 {
        return (0, 0);
    }
    let s = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    (
        ((w as f64 * s).round() as i32).max(1),
        ((h as f64 * s).round() as i32).max(1),
    )
}

/// A `w`x`h` rectangle centred in `area`.
pub fn centered(w: i32, h: i32, area: Rect) -> Rect {
    let (cx, cy) = area.center();
    Rect::from_size(cx - w / 2, cy - h / 2, w, h)
}

/// The desktop point a click on the picture stands for; `None` outside the picture.
pub fn map_point(picture: Rect, desktop: Rect, (x, y): (i32, i32)) -> Option<(i32, i32)> {
    if x < picture.left || x >= picture.right || y < picture.top || y >= picture.bottom {
        return None;
    }
    let fx = (x - picture.left) as f64 / picture.width() as f64;
    let fy = (y - picture.top) as f64 / picture.height() as f64;
    Some((
        desktop.left + (fx * desktop.width() as f64) as i32,
        desktop.top + (fy * desktop.height() as f64) as i32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(-1920, 0, 1920, 1080);

    #[test]
    fn crosshair_reaches_edges_or_has_length() {
        let p = crosshair((100, 200), SCREEN, 4, 10, 0);
        assert_eq!(
            p,
            vec![
                Part::Rect(Rect::new(-1920, 198, 1920, 202)),
                Part::Rect(Rect::new(98, 0, 102, 1080)),
                Part::CutRect(Rect::new(90, 190, 110, 210)),
            ]
        );
        let p = crosshair((0, 0), SCREEN, 1, 0, 50);
        assert_eq!(
            p,
            vec![
                Part::Rect(Rect::new(-50, 0, 50, 1)),
                Part::Rect(Rect::new(0, -50, 1, 50))
            ]
        );
    }

    #[test]
    fn spotlight_shrinks_then_holds() {
        assert_eq!(spotlight_radius(100, 9, 0, 500), 900);
        let mid = spotlight_radius(100, 9, 250, 500);
        assert!(mid > 100 && mid < 900, "{mid}");
        assert_eq!(spotlight_radius(100, 9, 500, 500), 100);
        assert_eq!(spotlight_radius(100, 9, 0, 0), 100);
        assert_eq!(
            spotlight((10, 10), SCREEN, 5),
            vec![Part::Rect(SCREEN), Part::CutEllipse(Rect::new(5, 5, 15, 15))]
        );
    }

    #[test]
    fn highlight_fades_after_delay() {
        assert_eq!(highlight_alpha(200, None, 500, 250), Some(200));
        assert_eq!(highlight_alpha(200, Some(400), 500, 250), Some(200));
        assert_eq!(highlight_alpha(200, Some(625), 500, 250), Some(100));
        assert_eq!(highlight_alpha(200, Some(750), 500, 250), None);
        assert_eq!(highlight_alpha(200, Some(0), 0, 0), None);
    }

    #[test]
    fn double_ctrl() {
        let mut d = DoubleCtrl::default();
        assert!(!d.feed(VK_LCONTROL, true, 0));
        assert!(!d.feed(VK_LCONTROL, true, 30), "repeat");
        assert!(!d.feed(VK_LCONTROL, false, 100));
        assert!(d.feed(VK_LCONTROL, true, 300));
        d.feed(VK_LCONTROL, false, 350);
        assert!(!d.feed(VK_LCONTROL, true, 400), "a third press starts over");
        d.feed(VK_LCONTROL, false, 450);
        assert!(!d.feed(VK_LCONTROL, true, 1000), "too slow");
        d.feed(VK_LCONTROL, false, 1050);
        d.feed(0x43, true, 1100); // C, as in Ctrl+C
        assert!(!d.feed(VK_LCONTROL, true, 1200), "another key in between");
        d.feed(VK_LCONTROL, false, 1250);
        d.interrupt();
        assert!(!d.feed(VK_LCONTROL, true, 1300), "a click in between");
    }

    #[test]
    fn shake_needs_back_and_forth() {
        let mut s = Shake::default();
        let mut t = 0;
        let mut hit = false;
        for i in 0..40 {
            t += 20;
            let x = if i % 2 == 0 { 0 } else { 150 };
            hit |= s.feed(x, 500, t, 96);
        }
        assert!(hit);

        let mut s = Shake::default();
        let mut hit = false;
        for i in 0..100 {
            hit |= s.feed(i * 30, 500, i as u64 * 10, 96);
        }
        assert!(!hit, "a long straight move is not a shake");
    }

    #[test]
    fn jump_mapping() {
        assert_eq!(fit(3840, 1080, 1600, 1200), (1600, 450));
        assert_eq!(fit(1000, 2000, 1600, 1200), (600, 1200));
        let pic = centered(1600, 450, Rect::new(0, 0, 1920, 1080));
        assert_eq!(pic, Rect::new(160, 315, 1760, 765));
        assert_eq!(map_point(pic, SCREEN, (160, 315)), Some((-1920, 0)));
        assert_eq!(map_point(pic, SCREEN, (960, 540)), Some((0, 540)));
        assert_eq!(map_point(pic, SCREEN, (1760, 540)), None);
        assert_eq!(
            union(&[Rect::new(0, 0, 10, 10), Rect::new(-5, 3, 2, 20)]),
            Some(Rect::new(-5, 0, 10, 20))
        );
    }
}
