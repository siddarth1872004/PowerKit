//! STL 3D models (binary and ASCII), drawn shaded from above and in front.

use crate::image::Image;

pub type Triangle = [[f32; 3]; 3];

/// Models with more triangles than this are drawn from the first ones only.
const MAX_TRIANGLES: usize = 5_000_000;

pub fn parse(bytes: &[u8]) -> Vec<Triangle> {
    let count = bytes
        .get(80..84)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    // An ASCII file starts with "solid" too, so the size decides.
    if count.is_some_and(|n| bytes.len() as u64 == 84 + 50 * n as u64) {
        binary(&bytes[84..])
    } else {
        ascii(&String::from_utf8_lossy(bytes))
    }
}

fn binary(data: &[u8]) -> Vec<Triangle> {
    let f = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    data.as_chunks::<50>()
        .0
        .iter()
        .take(MAX_TRIANGLES)
        .map(|t| {
            let v = |i: usize| {
                let at = 12 + i * 12;
                [f(&t[at..]), f(&t[at + 4..]), f(&t[at + 8..])]
            };
            [v(0), v(1), v(2)]
        })
        .collect()
}

fn ascii(src: &str) -> Vec<Triangle> {
    let mut tris = Vec::new();
    let mut corners = Vec::with_capacity(3);
    for line in src.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("facet") => corners.clear(),
            Some("vertex") => {
                let xyz: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                if let [x, y, z] = xyz[..] {
                    corners.push([x, y, z]);
                }
            }
            Some("endfacet") => {
                if let [a, b, c] = corners[..] {
                    tris.push([a, b, c]);
                    if tris.len() == MAX_TRIANGLES {
                        break;
                    }
                }
                corners.clear();
            }
            _ => {}
        }
    }
    tris
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f32; 3]) -> [f32; 3] {
    let len = dot(a, a).sqrt();
    if len > 0.0 {
        [a[0] / len, a[1] / len, a[2] / len]
    } else {
        a
    }
}

/// The model in a `size` x `size` picture with a transparent background, seen from the
/// front right and above with Z up, as slicers show it. `None` when there is nothing
/// to draw.
pub fn render(tris: &[Triangle], size: u32) -> Option<Image> {
    let toward_viewer = unit([1.0, -1.4, 1.0]);
    let right = unit(cross([0.0, 0.0, 1.0], toward_viewer));
    let up = cross(toward_viewer, right);
    let finite = |t: &&Triangle| t.iter().flatten().all(|c| c.is_finite());
    let view: Vec<[[f32; 3]; 3]> = tris
        .iter()
        .filter(finite)
        .map(|t| t.map(|p| [dot(p, right), dot(p, up), dot(p, toward_viewer)]))
        .collect();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in view.iter().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if view.is_empty() || extent <= 0.0 {
        return None;
    }
    let mut img = Image::new(size, size)?;
    let scale = size as f32 * 0.92 / extent;
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let half = size as f32 / 2.0;
    let mut depth = vec![f32::MIN; (size * size) as usize];
    let light = unit([-0.3, 0.5, 0.8]);
    for t in &view {
        let normal = unit(cross(sub(t[1], t[0]), sub(t[2], t[0])));
        // Both sides lit, so models with flipped or missing normals still read.
        let shade = 0.3 + 0.7 * dot(normal, light).abs();
        let color = [
            (70.0 * shade + 30.0) as u8,
            (140.0 * shade + 30.0) as u8,
            (215.0 * shade + 30.0) as u8,
            255,
        ];
        let s = t.map(|p| [half + (p[0] - cx) * scale, half - (p[1] - cy) * scale, p[2]]);
        fill(&mut img, &mut depth, s, color);
    }
    Some(img)
}

/// Fill a screen-space triangle (x, y, depth toward the viewer) where it is nearer than
/// what is drawn already.
fn fill(img: &mut Image, depth: &mut [f32], s: [[f32; 3]; 3], color: [u8; 4]) {
    let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
    let area = edge(s[0], s[1], s[2][0], s[2][1]);
    if area.abs() < 1e-6 {
        return;
    }
    let (w, h) = (img.width as f32, img.height as f32);
    let x0 = s.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let x1 = s
        .iter()
        .map(|p| p[0])
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(w - 1.0)
        .max(0.0) as u32;
    let y0 = s.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let y1 = s
        .iter()
        .map(|p| p[1])
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(h - 1.0)
        .max(0.0) as u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(s[1], s[2], px, py) / area;
            let w1 = edge(s[2], s[0], px, py) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
            let i = (y * img.width + x) as usize;
            if z > depth[i] {
                depth[i] = z;
                img.put_rgba(x, y, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Vec<Triangle> {
        let quad = |a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]| [[a, b, c], [a, c, d]];
        let p = |x: f32, y: f32, z: f32| [x, y, z];
        [
            quad(p(0., 0., 0.), p(1., 0., 0.), p(1., 1., 0.), p(0., 1., 0.)),
            quad(p(0., 0., 1.), p(1., 0., 1.), p(1., 1., 1.), p(0., 1., 1.)),
            quad(p(0., 0., 0.), p(1., 0., 0.), p(1., 0., 1.), p(0., 0., 1.)),
            quad(p(0., 1., 0.), p(1., 1., 0.), p(1., 1., 1.), p(0., 1., 1.)),
            quad(p(0., 0., 0.), p(0., 1., 0.), p(0., 1., 1.), p(0., 0., 1.)),
            quad(p(1., 0., 0.), p(1., 1., 0.), p(1., 1., 1.), p(1., 0., 1.)),
        ]
        .concat()
    }

    fn binary_file(tris: &[Triangle]) -> Vec<u8> {
        let mut f = b"solid but really binary".to_vec();
        f.resize(80, 0);
        f.extend((tris.len() as u32).to_le_bytes());
        for t in tris {
            f.extend([0u8; 12]);
            for c in t.iter().flatten() {
                f.extend(c.to_le_bytes());
            }
            f.extend([0, 0]);
        }
        f
    }

    #[test]
    fn reads_binary_and_ascii_models() {
        let tris = cube();
        assert_eq!(parse(&binary_file(&tris)), tris);
        let ascii = "solid t\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 1e0 1 0\n  endloop\n endfacet\nendsolid t\n";
        assert_eq!(
            parse(ascii.as_bytes()),
            vec![[[0., 0., 0.], [1., 0., 0.], [1., 1., 0.]]]
        );
    }

    #[test]
    fn draws_the_model_in_the_middle_on_transparency() {
        let img = render(&cube(), 64).unwrap();
        let alpha = |x: u32, y: u32| img.pixels[((y * 64 + x) * 4 + 3) as usize];
        assert_eq!(alpha(32, 32), 255);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(63, 63), 0);
        // The top is lit differently from the sides.
        let px = |x: u32, y: u32| img.pixels[((y * 64 + x) * 4) as usize..][..3].to_vec();
        assert_ne!(px(32, 12), px(32, 52));
    }

    #[test]
    fn nothing_to_draw_gives_none() {
        assert!(render(&[], 64).is_none());
        assert!(render(&[[[0.0; 3]; 3]], 64).is_none());
    }
}
