use serde::{Deserialize, Serialize};

/// Screen rectangle in virtual-desktop pixels. `right`/`bottom` are exclusive, and
/// coordinates may be negative (monitors left of / above the primary one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn from_size(left: i32, top: i32, width: i32, height: i32) -> Self {
        Self {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub const fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            self.width() as i64 * self.height() as i64
        }
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn center(&self) -> (i32, i32) {
        // Floor division: truncating toward zero would put the centre of a 1-pixel
        // rect at negative coordinates on its exclusive right/bottom edge.
        (
            (self.left as i64 + self.right as i64).div_euclid(2) as i32,
            (self.top as i64 + self.bottom as i64).div_euclid(2) as i32,
        )
    }

    /// Overlapping region, or `None` when the rectangles do not overlap.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let r = Rect {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (!r.is_empty()).then_some(r)
    }

    /// Shrink by `amount` on every side (grow when negative).
    pub fn inset(&self, amount: i32) -> Rect {
        Rect {
            left: self.left + amount,
            top: self.top + amount,
            right: self.right - amount,
            bottom: self.bottom - amount,
        }
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        }
    }

    /// Move (and if needed shrink) `self` so it lies inside `bounds`.
    pub fn clamp_into(&self, bounds: &Rect) -> Rect {
        let w = self.width().min(bounds.width()).max(0);
        let h = self.height().min(bounds.height()).max(0);
        let left = self.left.clamp(bounds.left, bounds.right - w);
        let top = self.top.clamp(bounds.top, bounds.bottom - h);
        Rect::from_size(left, top, w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rect() -> impl Strategy<Value = Rect> {
        (-5000i32..5000, -5000i32..5000, 1i32..4000, 1i32..4000).prop_map(|(l, t, w, h)| Rect::from_size(l, t, w, h))
    }

    #[test]
    fn basics() {
        let r = Rect::new(-10, -10, 10, 10);
        assert_eq!((r.width(), r.height()), (20, 20));
        assert!(r.contains(-10, -10) && !r.contains(10, 10));
        assert_eq!(r.center(), (0, 0));
        let thin = Rect::new(-19, -19, -18, -18);
        assert!(thin.contains(thin.center().0, thin.center().1));
        assert!(Rect::new(0, 0, 0, 5).is_empty());
        assert_eq!(r.intersect(&Rect::new(10, 0, 20, 5)), None);
    }

    proptest! {
        #[test]
        fn clamp_stays_inside(a in rect(), b in rect()) {
            let c = a.clamp_into(&b);
            prop_assert!(c.left >= b.left && c.top >= b.top);
            prop_assert!(c.right <= b.right && c.bottom <= b.bottom);
            prop_assert!(c.width() <= a.width() && c.height() <= a.height());
        }

        #[test]
        fn intersect_is_commutative_and_contained(a in rect(), b in rect()) {
            prop_assert_eq!(a.intersect(&b), b.intersect(&a));
            if let Some(i) = a.intersect(&b) {
                prop_assert!(i.area() <= a.area() && i.area() <= b.area());
            }
        }
    }
}
