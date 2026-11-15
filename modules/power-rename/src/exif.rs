//! Reads the capture time from a JPEG's EXIF block. Input comes from arbitrary files,
//! so every offset is bounds-checked and malformed data yields `None`, never a panic.

use crate::date::DateTime;
use std::io::Read;
use std::path::Path;

/// EXIF lives in an APP1 segment near the start of the file; this bounds the read.
const MAX_READ: u64 = 128 * 1024;

const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;
const TAG_DATE_TIME: u16 = 0x0132;

pub fn date_taken(path: &Path) -> std::io::Result<Option<DateTime>> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.take(MAX_READ).read_to_end(&mut buf)?;
    Ok(parse_jpeg(&buf))
}

pub fn parse_jpeg(data: &[u8]) -> Option<DateTime> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = 2;
    loop {
        if *data.get(pos)? != 0xFF {
            return None;
        }
        let marker = *data.get(pos + 1)?;
        // Start of scan or end of image: no metadata follows.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([*data.get(pos + 2)?, *data.get(pos + 3)?]));
        if len < 2 {
            return None;
        }
        let body = data.get(pos + 4..pos + 2 + len)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return parse_tiff(&body[6..]);
        }
        pos += 2 + len;
    }
}

struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    /// Find `tag` in the IFD at `ifd` and return the offset of its 4-byte value field.
    fn find(&self, ifd: usize, tag: u16) -> Option<usize> {
        let count = usize::from(self.u16(ifd)?);
        (0..count)
            .map(|i| ifd + 2 + i * 12)
            .find(|&entry| self.u16(entry) == Some(tag))
            .map(|entry| entry + 8)
    }

    fn ascii_date(&self, value_field: usize) -> Option<DateTime> {
        let offset = self.u32(value_field)? as usize;
        let raw = self.data.get(offset..offset.checked_add(19)?)?;
        DateTime::parse_exif(std::str::from_utf8(raw).ok()?)
    }
}

fn parse_tiff(data: &[u8]) -> Option<DateTime> {
    let little = match data.get(..4)? {
        [b'I', b'I', 42, 0] => true,
        [b'M', b'M', 0, 42] => false,
        _ => return None,
    };
    let tiff = Tiff { data, little };
    let ifd0 = tiff.u32(4)? as usize;
    let original = tiff
        .find(ifd0, TAG_EXIF_IFD)
        .and_then(|f| tiff.u32(f))
        .and_then(|exif| tiff.find(exif as usize, TAG_DATE_TIME_ORIGINAL))
        .and_then(|f| tiff.ascii_date(f));
    original.or_else(|| tiff.find(ifd0, TAG_DATE_TIME).and_then(|f| tiff.ascii_date(f)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Minimal little-endian JPEG with IFD0 -> ExifIFD -> DateTimeOriginal.
    pub(crate) fn jpeg_with_date(date: &str) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II\x2a\x00");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        // IFD0 at 8: one entry pointing to the Exif IFD at 26.
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&TAG_EXIF_IFD.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        // Exif IFD at 26: DateTimeOriginal, ASCII, 20 bytes at 44.
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&TAG_DATE_TIME_ORIGINAL.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&20u32.to_le_bytes());
        tiff.extend_from_slice(&44u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(date.as_bytes());
        tiff.push(0);

        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xE1];
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        out
    }

    #[test]
    fn reads_date_time_original() {
        let d = parse_jpeg(&jpeg_with_date("2019:12:31 23:59:58")).unwrap();
        assert_eq!(d.format("%Y-%m-%d %H:%M:%S"), "2019-12-31 23:59:58");
    }

    #[test]
    fn rejects_non_jpeg_and_truncation() {
        assert_eq!(parse_jpeg(b"\x89PNG\r\n"), None);
        let full = jpeg_with_date("2019:12:31 23:59:58");
        for cut in 0..full.len() - 4 {
            assert_eq!(parse_jpeg(&full[..cut]), None, "cut at {cut}");
        }
    }

    proptest! {
        #[test]
        fn never_panics(mut data in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = parse_jpeg(&data);
            if data.len() > 2 {
                data[0] = 0xFF;
                data[1] = 0xD8;
                let _ = parse_jpeg(&data);
            }
        }

        #[test]
        fn corrupted_valid_file_never_panics(idx in 0usize..120, byte in any::<u8>()) {
            let mut data = jpeg_with_date("2019:12:31 23:59:58");
            let i = idx % data.len();
            data[i] = byte;
            let _ = parse_jpeg(&data);
        }
    }
}
