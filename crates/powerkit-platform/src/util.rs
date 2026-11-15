use windows::core::PCWSTR;

/// NUL-terminated UTF-16 copy of `s`. Interior NULs are replaced so the string cannot
/// be truncated silently.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .map(|c| if c == 0 { 0xFFFD } else { c })
        .chain(std::iter::once(0))
        .collect()
}

pub(crate) fn pcwstr(buf: &[u16]) -> PCWSTR {
    debug_assert_eq!(buf.last(), Some(&0));
    PCWSTR(buf.as_ptr())
}

/// Lossy conversion of a UTF-16 buffer up to its first NUL.
pub(crate) fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Parse `#RRGGBB` into a GDI `COLORREF` value (0x00BBGGRR). Invalid input yields black.
pub(crate) fn colorref(hex: &str) -> u32 {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    ((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(wide("a\0b"), vec![97, 0xFFFD, 98, 0]);
        assert_eq!(from_wide(&[104, 105, 0, 120]), "hi");
        assert_eq!(colorref("#FF8000"), 0x000080FF);
        assert_eq!(colorref("junk"), 0);
    }
}
