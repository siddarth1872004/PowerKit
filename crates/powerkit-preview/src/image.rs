//! A decoded picture: premultiplied BGRA, top row first, as Windows bitmaps want it.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Pictures larger than this are refused rather than allocated.
pub const MAX_PIXELS: u64 = 1 << 26;

impl Image {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let count = width as u64 * height as u64;
        (width > 0 && height > 0 && count <= MAX_PIXELS).then(|| Image {
            width,
            height,
            pixels: vec![0; count as usize * 4],
        })
    }

    /// Set a pixel from straight (not premultiplied) RGBA.
    pub fn put_rgba(&mut self, x: u32, y: u32, [r, g, b, a]: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let pre = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
        self.pixels[i..i + 4].copy_from_slice(&[pre(b), pre(g), pre(r), a]);
    }

    /// The same picture drawn over an opaque `[r, g, b]` background.
    pub fn over(&self, [r, g, b]: [u8; 3]) -> Image {
        let mut out = self.clone();
        for p in out.pixels.as_chunks_mut::<4>().0 {
            let rest = 255 - p[3] as u32;
            for (c, bg) in p[..3].iter_mut().zip([b, g, r]) {
                *c = (*c as u32 + (bg as u32 * rest + 127) / 255).min(255) as u8;
            }
            p[3] = 255;
        }
        out
    }

    /// The picture shrunk to fit in `max` by `max` pixels, averaging the pixels each new
    /// one covers; one that fits already is returned as it is.
    pub fn fit(&self, max: u32) -> Image {
        let (w, h) = fit_size(self.width, self.height, max);
        if (w, h) == (self.width, self.height) {
            return self.clone();
        }
        let mut out = Image::new(w, h).expect("smaller than the source");
        for y in 0..h {
            let (y0, y1) = span(y, h, self.height);
            for x in 0..w {
                let (x0, x1) = span(x, w, self.width);
                let mut sum = [0u32; 4];
                for sy in y0..y1 {
                    let row = sy as usize * self.width as usize;
                    for sx in x0..x1 {
                        let i = (row + sx as usize) * 4;
                        for (s, &c) in sum.iter_mut().zip(&self.pixels[i..i + 4]) {
                            *s += c as u32;
                        }
                    }
                }
                let n = (y1 - y0) * (x1 - x0);
                let i = (y as usize * w as usize + x as usize) * 4;
                for (o, s) in out.pixels[i..i + 4].iter_mut().zip(sum) {
                    *o = ((s + n / 2) / n) as u8;
                }
            }
        }
        out
    }
}

/// The source pixels `[start, end)` that output pixel `i` of `out` covers in `src`.
fn span(i: u32, out: u32, src: u32) -> (u32, u32) {
    let start = (i as u64 * src as u64 / out as u64) as u32;
    let end = ((i as u64 + 1) * src as u64).div_ceil(out as u64) as u32;
    (start, end.max(start + 1).min(src))
}

/// The largest size with the aspect of `width` x `height` that fits in `max` x `max`,
/// never larger than the original and at least one pixel each way.
pub fn fit_size(width: u32, height: u32, max: u32) -> (u32, u32) {
    if width <= max && height <= max {
        return (width, height);
    }
    let scale = max as f64 / width.max(height) as f64;
    let side = |v: u32| ((v as f64 * scale).round() as u32).clamp(1, max);
    (side(width), side(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_keeps_the_aspect_and_never_enlarges() {
        assert_eq!(fit_size(400, 200, 100), (100, 50));
        assert_eq!(fit_size(10, 2000, 100), (1, 100));
        assert_eq!(fit_size(30, 20, 100), (30, 20));
    }

    #[test]
    fn shrinking_averages_the_covered_pixels() {
        let mut img = Image::new(2, 2).unwrap();
        img.put_rgba(0, 0, [255, 255, 255, 255]);
        img.put_rgba(1, 1, [255, 255, 255, 255]);
        let small = img.fit(1);
        assert_eq!(small.pixels, vec![128, 128, 128, 128]);
    }

    #[test]
    fn a_transparent_pixel_takes_the_background() {
        let mut img = Image::new(2, 1).unwrap();
        img.put_rgba(1, 0, [255, 0, 0, 128]);
        let out = img.over([0, 0, 255]);
        assert_eq!(&out.pixels[..4], &[255, 0, 0, 255]);
        assert_eq!(&out.pixels[4..], &[127, 0, 128, 255]);
    }

    #[test]
    fn huge_pictures_are_refused() {
        assert!(Image::new(1 << 14, 1 << 14).is_none());
        assert!(Image::new(0, 5).is_none());
    }
}
