//! What a resize produces: the new pixel size and crop for a picture, and the name of
//! the resized file. No Windows calls, so it is tested anywhere.

use powerkit_config::{ResizeFit, ResizeSize, ResizeUnit};
use std::path::{Path, PathBuf};

/// The picture is scaled to `width` x `height`, then the `crop` rectangle
/// (x, y, width, height) of that is kept, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    pub crop: Option<(u32, u32, u32, u32)>,
}

impl Target {
    /// Size of the saved picture.
    pub fn final_size(&self) -> (u32, u32) {
        match self.crop {
            Some((_, _, w, h)) => (w, h),
            None => (self.width, self.height),
        }
    }
}

/// `value` in `unit` as pixels; `original` is the picture's side for percentages and
/// `dpi` its resolution for physical units.
fn pixels(value: f64, unit: ResizeUnit, original: u32, dpi: f64) -> f64 {
    // Pictures that do not say their resolution report 0 or junk; Windows assumes 96.
    let dpi = if dpi.is_finite() && dpi > 1.0 { dpi } else { 96.0 };
    match unit {
        ResizeUnit::Pixel => value,
        ResizeUnit::Percent => value * original as f64 / 100.0,
        ResizeUnit::Inch => value * dpi,
        ResizeUnit::Centimeter => value * dpi / 2.54,
    }
}

fn side(v: f64) -> u32 {
    v.round().clamp(1.0, u32::MAX as f64) as u32
}

/// The target for a `width` x `height` picture with the given resolution. Returns
/// None when the picture is to stay as it is (it is already small enough and only
/// shrinking was asked for).
pub fn target(
    width: u32,
    height: u32,
    dpi: (f64, f64),
    size: &ResizeSize,
    shrink_only: bool,
    ignore_orientation: bool,
) -> Option<Target> {
    let (ow, oh) = (width.max(1) as f64, height.max(1) as f64);
    let mut w = pixels(size.width, size.unit, width, dpi.0);
    let mut h = pixels(size.height, size.unit, height, dpi.1);
    let auto = w <= 0.0 || h <= 0.0;
    if ignore_orientation && !auto && size.unit != ResizeUnit::Percent && ((ow < oh) != (w < h)) {
        std::mem::swap(&mut w, &mut h);
    }
    let (mut sx, mut sy) = (w / ow, h / oh);
    if auto {
        // One side given: keep the proportions from it, whatever the fit.
        let s = if w > 0.0 { sx } else { sy };
        (sx, sy) = (s, s);
    } else {
        match size.fit {
            ResizeFit::Fit => (sx, sy) = (sx.min(sy), sx.min(sy)),
            ResizeFit::Fill => (sx, sy) = (sx.max(sy), sx.max(sy)),
            ResizeFit::Stretch => {}
        }
    }
    if shrink_only && size.unit != ResizeUnit::Percent && (sx > 1.0 || sy > 1.0) {
        return None;
    }
    let (sw, sh) = (side(ow * sx), side(oh * sy));
    let crop = (size.fit == ResizeFit::Fill && !auto).then(|| {
        let (cw, ch) = (side(w).min(sw), side(h).min(sh));
        ((sw - cw) / 2, (sh - ch) / 2, cw, ch)
    });
    Some(Target {
        width: sw,
        height: sh,
        crop: crop.filter(|&(_, _, cw, ch)| (cw, ch) != (sw, sh)),
    })
}

/// Number text as typed in a size: no trailing `.0`.
pub fn number(v: f64) -> String {
    format!("{}", (v * 1000.0).round() / 1000.0)
}

/// The file name (without extension) of a resized copy of `original` (a name without
/// extension), from the `format` with `%1`...`%6`. Characters Windows does not allow
/// in names are dropped.
pub fn file_stem(format: &str, original: &str, size: &ResizeSize, saved: (u32, u32)) -> String {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        let value = match (c, chars.peek()) {
            ('%', Some('1')) => Some(original.to_string()),
            ('%', Some('2')) => Some(size.name.clone()),
            ('%', Some('3')) => Some(number(size.width)),
            ('%', Some('4')) => Some(number(size.height)),
            ('%', Some('5')) => Some(saved.0.to_string()),
            ('%', Some('6')) => Some(saved.1.to_string()),
            _ => None,
        };
        match value {
            Some(v) => {
                chars.next();
                out += &v;
            }
            None => out.push(c),
        }
    }
    let out: String = out
        .chars()
        .filter(|c| !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .collect();
    let out = out.trim_end_matches(['.', ' ']).trim_start();
    if out.is_empty() {
        original.to_string()
    } else {
        out.to_string()
    }
}

/// `folder\stem.ext`, or with ` (1)`, ` (2)`... added while `taken` says the name is
/// in use.
pub fn free_path(folder: &Path, stem: &str, ext: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let name = |n: u32| {
        let stem = if n == 0 {
            stem.to_string()
        } else {
            format!("{stem} ({n})")
        };
        folder.join(if ext.is_empty() { stem } else { format!("{stem}.{ext}") })
    };
    (0..).map(name).find(|p| !taken(p)).expect("some number is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: f64, height: f64, unit: ResizeUnit, fit: ResizeFit) -> ResizeSize {
        ResizeSize {
            name: "Medium".into(),
            fit,
            width,
            height,
            unit,
        }
    }

    const DPI: (f64, f64) = (96.0, 96.0);

    fn t(w: u32, h: u32, s: &ResizeSize, shrink: bool, orient: bool) -> Option<Target> {
        target(w, h, DPI, s, shrink, orient)
    }

    #[test]
    fn fit_keeps_the_proportions_inside_the_size() {
        let s = size(1366.0, 768.0, ResizeUnit::Pixel, ResizeFit::Fit);
        let r = t(4000, 3000, &s, false, false).unwrap();
        assert_eq!((r.width, r.height, r.crop), (1024, 768, None));
    }

    #[test]
    fn fill_covers_the_size_and_crops_the_middle() {
        let s = size(1000.0, 1000.0, ResizeUnit::Pixel, ResizeFit::Fill);
        let r = t(4000, 2000, &s, false, false).unwrap();
        assert_eq!((r.width, r.height), (2000, 1000));
        assert_eq!(r.crop, Some((500, 0, 1000, 1000)));
        assert_eq!(r.final_size(), (1000, 1000));
    }

    #[test]
    fn stretch_takes_the_size_exactly() {
        let s = size(300.0, 300.0, ResizeUnit::Pixel, ResizeFit::Stretch);
        assert_eq!(t(4000, 2000, &s, false, false).unwrap().final_size(), (300, 300));
    }

    #[test]
    fn a_portrait_picture_turns_a_landscape_size_when_orientation_is_ignored() {
        let s = size(1920.0, 1080.0, ResizeUnit::Pixel, ResizeFit::Fit);
        assert_eq!(t(3000, 4000, &s, false, true).unwrap().final_size(), (1080, 1440));
        assert_eq!(t(3000, 4000, &s, false, false).unwrap().final_size(), (810, 1080));
    }

    #[test]
    fn units_convert_with_the_picture() {
        let half = size(50.0, 50.0, ResizeUnit::Percent, ResizeFit::Stretch);
        assert_eq!(t(801, 600, &half, false, true).unwrap().final_size(), (401, 300));
        let inch = size(2.0, 1.0, ResizeUnit::Inch, ResizeFit::Stretch);
        assert_eq!(
            target(10, 10, (300.0, 0.0), &inch, false, false).unwrap().final_size(),
            (600, 96)
        );
        let cm = size(2.54, 2.54, ResizeUnit::Centimeter, ResizeFit::Stretch);
        assert_eq!(t(10, 10, &cm, false, false).unwrap().final_size(), (96, 96));
    }

    #[test]
    fn a_missing_side_follows_the_other() {
        let s = size(500.0, 0.0, ResizeUnit::Pixel, ResizeFit::Fill);
        let r = t(1000, 400, &s, false, true).unwrap();
        assert_eq!((r.width, r.height, r.crop), (500, 200, None));
    }

    #[test]
    fn shrink_only_leaves_small_pictures_alone() {
        let s = size(1920.0, 1080.0, ResizeUnit::Pixel, ResizeFit::Fit);
        assert_eq!(t(800, 600, &s, true, true), None);
        assert!(t(4000, 3000, &s, true, true).is_some());
        let double = size(200.0, 200.0, ResizeUnit::Percent, ResizeFit::Fit);
        assert!(t(800, 600, &double, true, true).is_some(), "percentages always apply");
    }

    #[test]
    fn names_fill_in_the_placeholders_and_drop_bad_characters() {
        let s = size(1366.0, 768.5, ResizeUnit::Pixel, ResizeFit::Fit);
        assert_eq!(file_stem("%1 (%2)", "photo", &s, (1024, 768)), "photo (Medium)");
        assert_eq!(
            file_stem("%1_%3x%4_%5x%6_%7%", "a", &s, (1024, 768)),
            "a_1366x768.5_1024x768_%7%"
        );
        assert_eq!(file_stem("%1: <%2>?. ", "a", &s, (1, 1)), "a Medium");
        assert_eq!(file_stem("???", "a", &s, (1, 1)), "a");
    }

    #[test]
    fn taken_names_get_a_number() {
        let folder = Path::new("C:\\p");
        let taken = |p: &Path| p.ends_with("a (M).jpg") || p.ends_with("a (M) (1).jpg");
        assert_eq!(free_path(folder, "a (M)", "jpg", taken), folder.join("a (M) (2).jpg"));
        assert_eq!(free_path(folder, "b", "", |_| false), folder.join("b"));
    }
}
