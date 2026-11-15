//! Color conversions and the text formats the picker copies.

use powerkit_config::ColorFormat;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// From `0x00RRGGBB`.
    pub fn from_u32(v: u32) -> Rgb {
        Rgb {
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
        }
    }

    /// Accepts `#RRGGBB`, `RRGGBB` and `#RGB`, in any case.
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        match h.len() {
            6 => u32::from_str_radix(h, 16).ok().map(Rgb::from_u32),
            3 => {
                let d: Vec<u8> = h.chars().map(|c| c.to_digit(16).unwrap() as u8 * 17).collect();
                Some(Rgb {
                    r: d[0],
                    g: d[1],
                    b: d[2],
                })
            }
            _ => None,
        }
    }

    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    fn unit(self) -> (f64, f64, f64) {
        (self.r as f64 / 255.0, self.g as f64 / 255.0, self.b as f64 / 255.0)
    }

    /// Hue in degrees (0 for greys), and the max and min channel in 0..=1.
    fn hue(self) -> (f64, f64, f64) {
        let (r, g, b) = self.unit();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        (h, max, min)
    }

    /// Hue (degrees), saturation and lightness (percent), rounded.
    pub fn hsl(self) -> (u32, u32, u32) {
        let (h, max, min) = self.hue();
        let l = (max + min) / 2.0;
        let s = if max == min {
            0.0
        } else {
            (max - min) / (1.0 - (2.0 * l - 1.0).abs())
        };
        (round_hue(h), pct(s), pct(l))
    }

    /// Hue (degrees), saturation and value (percent), rounded.
    pub fn hsv(self) -> (u32, u32, u32) {
        let (h, max, min) = self.hue();
        let s = if max == 0.0 { 0.0 } else { (max - min) / max };
        (round_hue(h), pct(s), pct(max))
    }

    /// Cyan, magenta, yellow and key (percent), rounded.
    pub fn cmyk(self) -> (u32, u32, u32, u32) {
        let (r, g, b) = self.unit();
        let k = 1.0 - r.max(g).max(b);
        if k >= 1.0 {
            return (0, 0, 0, 100);
        }
        let part = |c: f64| pct((1.0 - c - k) / (1.0 - k));
        (part(r), part(g), part(b), pct(k))
    }
}

fn pct(v: f64) -> u32 {
    (v * 100.0).round() as u32
}

fn round_hue(h: f64) -> u32 {
    (h.round() as u32) % 360
}

pub fn format(c: Rgb, f: ColorFormat, custom: &str) -> String {
    let Rgb { r, g, b } = c;
    match f {
        ColorFormat::Hex => c.hex(),
        ColorFormat::HexAlpha => format!("{}FF", c.hex()),
        ColorFormat::Rgb => format!("rgb({r}, {g}, {b})"),
        ColorFormat::Rgba => format!("rgba({r}, {g}, {b}, 1)"),
        ColorFormat::Hsl => {
            let (h, s, l) = c.hsl();
            format!("hsl({h}, {s}%, {l}%)")
        }
        ColorFormat::Hsv => {
            let (h, s, v) = c.hsv();
            format!("hsv({h}, {s}%, {v}%)")
        }
        ColorFormat::Cmyk => {
            let (c, m, y, k) = c.cmyk();
            format!("cmyk({c}%, {m}%, {y}%, {k}%)")
        }
        ColorFormat::CssRgb => format!("rgb({r} {g} {b})"),
        ColorFormat::CssHsl => {
            let (h, s, l) = c.hsl();
            format!("hsl({h} {s}% {l}%)")
        }
        ColorFormat::Custom => format_custom(c, custom),
    }
}

/// Expand a user template. Placeholders: `%R %G %B` (0-255), `%X`/`%x` (RRGGBB in
/// upper/lower case), `%H %S %L` (HSL), `%C %M %Y %K` (CMYK), `%%` (a literal `%`).
/// Anything else is copied as written.
pub fn format_custom(c: Rgb, template: &str) -> String {
    let (h, s, l) = c.hsl();
    let (cy, m, y, k) = c.cmyk();
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let value = match chars.peek() {
            Some('R') => c.r as u32,
            Some('G') => c.g as u32,
            Some('B') => c.b as u32,
            Some('H') => h,
            Some('S') => s,
            Some('L') => l,
            Some('C') => cy,
            Some('M') => m,
            Some('Y') => y,
            Some('K') => k,
            Some('X') => {
                chars.next();
                let _ = write!(out, "{:02X}{:02X}{:02X}", c.r, c.g, c.b);
                continue;
            }
            Some('x') => {
                chars.next();
                let _ = write!(out, "{:02x}{:02x}{:02x}", c.r, c.g, c.b);
                continue;
            }
            Some('%') => {
                chars.next();
                out.push('%');
                continue;
            }
            _ => {
                out.push('%');
                continue;
            }
        };
        chars.next();
        let _ = write!(out, "{value}");
    }
    out
}

/// Put `hex` at the front of `history` (newest first), without duplicates, keeping at
/// most `limit` entries.
pub fn remember(history: &mut Vec<String>, hex: &str, limit: usize) {
    history.retain(|h| !h.eq_ignore_ascii_case(hex));
    history.insert(0, hex.to_string());
    history.truncate(limit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ORANGE: Rgb = Rgb { r: 255, g: 128, b: 0 };

    #[test]
    fn every_format() {
        let expect = [
            (ColorFormat::Hex, "#FF8000"),
            (ColorFormat::HexAlpha, "#FF8000FF"),
            (ColorFormat::Rgb, "rgb(255, 128, 0)"),
            (ColorFormat::Rgba, "rgba(255, 128, 0, 1)"),
            (ColorFormat::Hsl, "hsl(30, 100%, 50%)"),
            (ColorFormat::Hsv, "hsv(30, 100%, 100%)"),
            (ColorFormat::Cmyk, "cmyk(0%, 50%, 100%, 0%)"),
            (ColorFormat::CssRgb, "rgb(255 128 0)"),
            (ColorFormat::CssHsl, "hsl(30 100% 50%)"),
            (ColorFormat::Custom, "255, 128, 0"),
        ];
        assert_eq!(expect.len(), ColorFormat::ALL.len());
        for (f, s) in expect {
            assert_eq!(format(ORANGE, f, "%R, %G, %B"), s, "{f:?}");
        }
    }

    #[test]
    fn greys_and_extremes() {
        let black = Rgb { r: 0, g: 0, b: 0 };
        let white = Rgb { r: 255, g: 255, b: 255 };
        let grey = Rgb { r: 128, g: 128, b: 128 };
        assert_eq!(black.hsl(), (0, 0, 0));
        assert_eq!(white.hsl(), (0, 0, 100));
        assert_eq!(grey.hsv(), (0, 0, 50));
        assert_eq!(black.cmyk(), (0, 0, 0, 100));
        assert_eq!(Rgb { r: 0, g: 0, b: 255 }.hsl(), (240, 100, 50));
        // Hue 359.8 rounds to 360, which wraps to 0.
        assert_eq!(Rgb { r: 255, g: 0, b: 1 }.hsv(), (0, 100, 100));
    }

    #[test]
    fn custom_template() {
        assert_eq!(format_custom(ORANGE, "0x%X / %x"), "0xFF8000 / ff8000");
        assert_eq!(format_custom(ORANGE, "%H° %S% %L%%"), "30° 100% 50%");
        assert_eq!(format_custom(ORANGE, "C%C M%M Y%Y K%K"), "C0 M50 Y100 K0");
        assert_eq!(format_custom(ORANGE, "100%% %Q %"), "100% %Q %");
    }

    #[test]
    fn parse_hex() {
        assert_eq!(Rgb::parse("#ff8000"), Some(ORANGE));
        assert_eq!(Rgb::parse(" FF8000 "), Some(ORANGE));
        assert_eq!(Rgb::parse("#F80"), Some(Rgb { r: 255, g: 136, b: 0 }));
        for bad in ["", "#", "#12345", "#GG0000", "#+12345", "rgb(1,2,3)"] {
            assert_eq!(Rgb::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn history_is_newest_first_unique_and_bounded() {
        let mut h = vec!["#000000".to_string(), "#111111".to_string()];
        remember(&mut h, "#111111", 3);
        assert_eq!(h, ["#111111", "#000000"]);
        remember(&mut h, "#222222", 2);
        assert_eq!(h, ["#222222", "#111111"]);
        remember(&mut h, "#333333", 0);
        assert!(h.is_empty());
    }

    proptest! {
        #[test]
        fn hex_round_trips(v in 0u32..=0xFF_FFFF) {
            let c = Rgb::from_u32(v);
            prop_assert_eq!(Rgb::parse(&c.hex()), Some(c));
        }

        #[test]
        fn components_stay_in_range(v in 0u32..=0xFF_FFFF) {
            let c = Rgb::from_u32(v);
            let (h, s, l) = c.hsl();
            let (h2, s2, v2) = c.hsv();
            let (cy, m, y, k) = c.cmyk();
            prop_assert!(h < 360 && h2 < 360);
            prop_assert!([s, l, s2, v2, cy, m, y, k].iter().all(|p| *p <= 100));
        }
    }
}
