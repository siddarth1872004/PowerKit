//! The QOI ("Quite OK Image") format, decoded as its specification describes.

use crate::image::Image;

pub fn decode(bytes: &[u8]) -> Option<Image> {
    let header = bytes.get(..14)?;
    if &header[..4] != b"qoif" {
        return None;
    }
    let width = u32::from_be_bytes(header[4..8].try_into().ok()?);
    let height = u32::from_be_bytes(header[8..12].try_into().ok()?);
    let mut img = Image::new(width, height)?;
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut run = 0u32;
    let mut at = 14;
    let mut next = || {
        let b = *bytes.get(at)?;
        at += 1;
        Some(b)
    };
    for i in 0..width as u64 * height as u64 {
        if run > 0 {
            run -= 1;
        } else {
            let op = next()?;
            match op {
                0xfe => px = [next()?, next()?, next()?, px[3]],
                0xff => px = [next()?, next()?, next()?, next()?],
                _ => match op >> 6 {
                    0 => px = index[op as usize & 63],
                    1 => {
                        let d = |shift: u8| ((op >> shift) & 3).wrapping_sub(2);
                        px = [
                            px[0].wrapping_add(d(4)),
                            px[1].wrapping_add(d(2)),
                            px[2].wrapping_add(d(0)),
                            px[3],
                        ];
                    }
                    2 => {
                        let dg = (op & 63).wrapping_sub(32);
                        let b = next()?;
                        let dr = (b >> 4).wrapping_sub(8).wrapping_add(dg);
                        let db = (b & 15).wrapping_sub(8).wrapping_add(dg);
                        px = [
                            px[0].wrapping_add(dr),
                            px[1].wrapping_add(dg),
                            px[2].wrapping_add(db),
                            px[3],
                        ];
                    }
                    _ => run = (op & 63) as u32,
                },
            }
            let [r, g, b, a] = px.map(|c| c as usize);
            index[(r * 3 + g * 5 + b * 7 + a * 11) % 64] = px;
        }
        img.put_rgba((i % width as u64) as u32, (i / width as u64) as u32, px);
    }
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"qoif".to_vec();
        v.extend(w.to_be_bytes());
        v.extend(h.to_be_bytes());
        v.extend([4, 0]);
        v
    }

    #[test]
    fn decodes_every_kind_of_chunk() {
        let mut f = header(6, 1);
        f.extend([0xff, 10, 20, 30, 255]); // RGBA
        f.extend([0b01_11_10_01]); // DIFF: r+1, g+0, b-1
        f.extend([0b10_100001, 0x9a]); // LUMA: dg=+1, dr=dg+1, db=dg+2
        f.extend([0xc1]); // RUN of 2
        let first = [10usize, 20, 30, 255];
        f.push(((first[0] * 3 + first[1] * 5 + first[2] * 7 + first[3] * 11) % 64) as u8); // INDEX
        f.extend([0; 7]);
        f.extend([0, 0, 0, 0, 0, 0, 0, 1]);
        let img = decode(&f).unwrap();
        let px = |i: usize| img.pixels[i * 4..i * 4 + 4].to_vec();
        assert_eq!(px(0), vec![30, 20, 10, 255]);
        assert_eq!(px(1), vec![29, 20, 11, 255]);
        assert_eq!(px(2), vec![32, 21, 13, 255]);
        assert_eq!(px(3), px(2));
        assert_eq!(px(4), px(2));
        assert_eq!(px(5), px(0));
    }

    #[test]
    fn rejects_other_and_short_files() {
        assert!(decode(b"PNG.....").is_none());
        let mut f = header(2, 2);
        f.extend([0xfe, 1, 2]);
        assert!(decode(&f).is_none());
    }
}
