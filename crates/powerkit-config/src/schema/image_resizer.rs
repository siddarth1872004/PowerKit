use serde::{Deserialize, Serialize};

/// How a picture is made to match a size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeFit {
    /// Scale to cover the size, then cut off what sticks out.
    Fill,
    /// Scale to fit inside the size, keeping the proportions.
    #[default]
    Fit,
    /// Scale to exactly the size, changing the proportions.
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeUnit {
    Centimeter,
    Inch,
    Percent,
    #[default]
    Pixel,
}

/// A size offered in the Image Resizer window. A width or height of 0 follows from
/// the other one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResizeSize {
    pub name: String,
    pub fit: ResizeFit,
    pub width: f64,
    pub height: f64,
    pub unit: ResizeUnit,
}

impl Default for ResizeSize {
    fn default() -> Self {
        Self {
            name: String::new(),
            fit: ResizeFit::Fit,
            width: 0.0,
            height: 0.0,
            unit: ResizeUnit::Pixel,
        }
    }
}

/// File format for resized pictures whose own format Windows cannot write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackFormat {
    Bmp,
    Gif,
    #[default]
    Jpeg,
    Png,
    Tiff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageResizer {
    pub enabled: bool,
    pub sizes: Vec<ResizeSize>,
    /// Leave pictures that are already smaller than the size as they are.
    pub shrink_only: bool,
    /// Overwrite the pictures instead of saving resized copies next to them.
    pub replace_originals: bool,
    /// Swap a size's width and height when the picture is turned the other way
    /// (portrait against landscape).
    pub ignore_orientation: bool,
    /// Drop camera and other metadata from the resized pictures.
    pub remove_metadata: bool,
    /// Give resized pictures the modification time of the originals.
    pub keep_date_modified: bool,
    pub fallback_format: FallbackFormat,
    /// JPEG quality, 1 to 100.
    pub jpeg_quality: u32,
    /// Name of a resized copy: `%1` original name, `%2` size name, `%3` and `%4` the
    /// size's width and height, `%5` and `%6` the picture's new width and height.
    pub file_name: String,
}

fn size(name: &str, width: f64, height: f64) -> ResizeSize {
    ResizeSize {
        name: name.into(),
        width,
        height,
        ..ResizeSize::default()
    }
}

impl Default for ImageResizer {
    fn default() -> Self {
        Self {
            enabled: true,
            sizes: vec![
                size("Small", 854.0, 480.0),
                size("Medium", 1366.0, 768.0),
                size("Large", 1920.0, 1080.0),
                size("Phone", 320.0, 568.0),
            ],
            shrink_only: false,
            replace_originals: false,
            ignore_orientation: true,
            remove_metadata: false,
            keep_date_modified: false,
            fallback_format: FallbackFormat::Jpeg,
            jpeg_quality: 90,
            file_name: "%1 (%2)".into(),
        }
    }
}

impl ImageResizer {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        if !(1..=100).contains(&self.jpeg_quality) {
            w.push(format!(
                "image_resizer.jpeg_quality {} is not between 1 and 100; using 90",
                self.jpeg_quality
            ));
            self.jpeg_quality = 90;
        }
        if self.file_name.trim().is_empty() {
            w.push("image_resizer.file_name is empty; using \"%1 (%2)\"".into());
            self.file_name = "%1 (%2)".into();
        }
        let before = self.sizes.len();
        self.sizes.retain(|s| {
            let ok = |v: f64| v.is_finite() && v >= 0.0;
            !s.name.trim().is_empty() && ok(s.width) && ok(s.height) && (s.width > 0.0 || s.height > 0.0)
        });
        if self.sizes.len() < before {
            w.push(format!(
                "image_resizer.sizes: removed {} without a name or a usable width and height",
                before - self.sizes.len()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unusable_sizes_and_values_are_repaired() {
        let mut r = ImageResizer {
            jpeg_quality: 0,
            file_name: " ".into(),
            ..ImageResizer::default()
        };
        r.sizes.push(size("", 1.0, 1.0));
        r.sizes.push(size("Nothing", 0.0, 0.0));
        r.sizes.push(size("Odd", f64::NAN, 5.0));
        r.sizes.push(size("Wide", 500.0, 0.0));
        let mut w = Vec::new();
        r.sanitize(&mut w);
        assert_eq!(w.len(), 3);
        assert_eq!(r.jpeg_quality, 90);
        assert_eq!(r.file_name, "%1 (%2)");
        assert_eq!(r.sizes.len(), 5);
        assert_eq!(r.sizes[4].name, "Wide");
    }
}
