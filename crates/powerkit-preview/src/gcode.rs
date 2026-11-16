//! The pictures of the print that slicers put into G-code: base64 comment blocks in
//! text G-code (PrusaSlicer, Cura, OrcaSlicer) and thumbnail blocks in Prusa's binary
//! .bgcode.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Png,
    Jpeg,
    Qoi,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub encoding: Encoding,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Every picture in a G-code file of either kind.
pub fn thumbnails(bytes: &[u8]) -> Vec<Thumbnail> {
    if bytes.starts_with(b"GCDE") {
        binary(bytes)
    } else {
        text(&String::from_utf8_lossy(bytes))
    }
}

/// The largest picture, which looks best in the preview pane and when shrunk.
pub fn largest(bytes: &[u8]) -> Option<Thumbnail> {
    thumbnails(bytes)
        .into_iter()
        .max_by_key(|t| t.width as u64 * t.height as u64)
}

/// Blocks from `; thumbnail begin 300x300 12345` (or `thumbnail_JPG`, `thumbnail_QOI`)
/// to the matching `end`, with the base64 data on `; ` lines between.
fn text(src: &str) -> Vec<Thumbnail> {
    let mut found = Vec::new();
    let mut open: Option<(Encoding, u32, u32, String)> = None;
    for line in src.lines() {
        let Some(rest) = line.trim().strip_prefix(';') else {
            if open.is_some() {
                open = None;
            }
            continue;
        };
        let rest = rest.trim();
        let mut words = rest.split_whitespace();
        let (Some(tag), second) = (words.next(), words.next()) else {
            continue;
        };
        let encoding = match tag {
            "thumbnail" | "thumbnail_PNG" => Some(Encoding::Png),
            "thumbnail_JPG" => Some(Encoding::Jpeg),
            "thumbnail_QOI" => Some(Encoding::Qoi),
            _ => None,
        };
        match (encoding, second) {
            (Some(encoding), Some("begin")) => {
                let size = words.next().and_then(|s| s.split_once('x'));
                let (w, h) = size
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .unwrap_or((0, 0));
                open = Some((encoding, w, h, String::new()));
            }
            (Some(_), Some("end")) => {
                if let Some((encoding, width, height, b64)) = open.take() {
                    if let Some(data) = base64(&b64) {
                        found.push(Thumbnail {
                            encoding,
                            width,
                            height,
                            data,
                        });
                    }
                }
            }
            _ => {
                if let Some((.., b64)) = open.as_mut() {
                    b64.push_str(rest);
                }
            }
        }
    }
    found
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Prusa binary G-code: a 10-byte file header, then blocks of type, compression,
/// sizes, parameters, data and an optional CRC32. Pictures come before the G-code.
fn binary(bytes: &[u8]) -> Vec<Thumbnail> {
    const GCODE_BLOCK: u16 = 1;
    const THUMBNAIL_BLOCK: u16 = 5;
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut found = Vec::new();
    let Some(checksum) = u16_at(8) else { return found };
    let crc = if checksum == 1 { 4 } else { 0 };
    let mut at = 10;
    while let (Some(kind), Some(compression), Some(size)) = (u16_at(at), u16_at(at + 2), u32_at(at + 4)) {
        if kind == GCODE_BLOCK {
            break;
        }
        at += 8;
        let stored = if compression == 0 {
            size
        } else {
            let Some(compressed) = u32_at(at) else { break };
            at += 4;
            compressed
        };
        let params = if kind == THUMBNAIL_BLOCK { 6 } else { 2 };
        let data_at = at + params;
        let Some(data) = bytes.get(data_at..data_at + stored as usize) else {
            break;
        };
        if kind == THUMBNAIL_BLOCK && compression == 0 {
            let encoding = match u16_at(at) {
                Some(0) => Some(Encoding::Png),
                Some(1) => Some(Encoding::Jpeg),
                Some(2) => Some(Encoding::Qoi),
                _ => None,
            };
            if let (Some(encoding), Some(width), Some(height)) = (encoding, u16_at(at + 2), u16_at(at + 4)) {
                found.push(Thumbnail {
                    encoding,
                    width: width.into(),
                    height: height.into(),
                    data: data.to_vec(),
                });
            }
        }
        at = data_at + stored as usize + crc;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_with_and_without_padding() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64("aGVs\nbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64("a*b"), None);
    }

    #[test]
    fn finds_every_text_block_and_picks_the_largest() {
        let src = "; generated by PrusaSlicer\n;\n; thumbnail begin 16x16 8\n; aGVs\n; bG8=\n; thumbnail end\n\
                   ;\n; thumbnail_JPG begin 300x200 4\n; d29y\n; bGQ=\n; thumbnail_JPG end\nG28\n";
        let all = thumbnails(src.as_bytes());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].data, b"hello");
        assert_eq!((all[0].encoding, all[0].width), (Encoding::Png, 16));
        let big = largest(src.as_bytes()).unwrap();
        assert_eq!((big.encoding, big.width, big.height), (Encoding::Jpeg, 300, 200));
        assert_eq!(big.data, b"world");
    }

    #[test]
    fn a_block_cut_off_by_gcode_is_dropped() {
        let src = "; thumbnail begin 16x16 8\n; aGVs\nG28\n; bG8=\n; thumbnail end\n";
        assert!(thumbnails(src.as_bytes()).is_empty());
    }

    #[test]
    fn reads_binary_thumbnail_blocks() {
        let mut f = b"GCDE".to_vec();
        f.extend(1u32.to_le_bytes());
        f.extend(1u16.to_le_bytes()); // CRC32 after each block
                                      // File metadata block: type 0, uncompressed, 3 bytes, encoding parameter.
        f.extend([0, 0, 0, 0, 3, 0, 0, 0, 0, 0]);
        f.extend(b"a=b");
        f.extend([0; 4]);
        // Thumbnail block: QOI, 2x1.
        f.extend([5, 0, 0, 0, 4, 0, 0, 0, 2, 0, 2, 0, 1, 0]);
        f.extend(b"qoif");
        f.extend([0; 4]);
        // G-code block ends the search.
        f.extend([1, 0, 0, 0, 0, 0, 0, 0]);
        let all = thumbnails(&f);
        assert_eq!(
            all,
            vec![Thumbnail {
                encoding: Encoding::Qoi,
                width: 2,
                height: 1,
                data: b"qoif".to_vec()
            }]
        );
    }
}
