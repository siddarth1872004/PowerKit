//! Virtual-key codes and their stable, user-facing names.
//!
//! Names are what configuration files and the UI use (`"CapsLock"`, `"LCtrl"`, `"F5"`).
//! Codes are Windows virtual-key values, kept as plain integers so this crate stays
//! platform-independent.

pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_DELETE: u32 = 0x2E;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;

const NAMED: &[(&str, u32)] = &[
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Shift", VK_SHIFT),
    ("Ctrl", VK_CONTROL),
    ("Alt", VK_MENU),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Esc", VK_ESCAPE),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Insert", 0x2D),
    ("Delete", VK_DELETE),
    ("LWin", VK_LWIN),
    ("RWin", VK_RWIN),
    ("Menu", 0x5D),
    ("NumpadMultiply", 0x6A),
    ("NumpadAdd", 0x6B),
    ("NumpadSubtract", 0x6D),
    ("NumpadDecimal", 0x6E),
    ("NumpadDivide", 0x6F),
    ("NumLock", 0x90),
    ("ScrollLock", 0x91),
    ("LShift", VK_LSHIFT),
    ("RShift", VK_RSHIFT),
    ("LCtrl", VK_LCONTROL),
    ("RCtrl", VK_RCONTROL),
    ("LAlt", VK_LMENU),
    ("RAlt", VK_RMENU),
    ("BrowserBack", 0xA6),
    ("BrowserForward", 0xA7),
    ("BrowserRefresh", 0xA8),
    ("BrowserHome", 0xAC),
    ("VolumeMute", 0xAD),
    ("VolumeDown", 0xAE),
    ("VolumeUp", 0xAF),
    ("MediaNext", 0xB0),
    ("MediaPrev", 0xB1),
    ("MediaStop", 0xB2),
    ("MediaPlayPause", 0xB3),
    ("Semicolon", 0xBA),
    ("Equals", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backtick", 0xC0),
    ("LBracket", 0xDB),
    ("Backslash", 0xDC),
    ("RBracket", 0xDD),
    ("Quote", 0xDE),
];

const ALIASES: &[(&str, u32)] = &[
    ("escape", VK_ESCAPE),
    ("return", 0x0D),
    ("del", VK_DELETE),
    ("ins", 0x2D),
    ("pgup", 0x21),
    ("pgdn", 0x22),
    ("apps", 0x5D),
    ("plus", 0xBB),
    ("control", VK_CONTROL),
    ("win", VK_LWIN),
];

/// Name of a virtual key, or `None` for codes PowerKit does not know.
pub fn name(vk: u32) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(char::from(vk as u8).to_string()),
        0x60..=0x69 => Some(format!("Numpad{}", vk - 0x60)),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        _ => NAMED.iter().find(|(_, v)| *v == vk).map(|(n, _)| (*n).to_string()),
    }
}

/// Parse a key name (case-insensitive) into its virtual-key code.
pub fn parse(name: &str) -> Option<u32> {
    let n = name.trim();
    if n.is_empty() || !n.is_ascii() {
        return None;
    }
    let lower = n.to_ascii_lowercase();
    if lower.len() == 1 {
        let c = lower.as_bytes()[0];
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            return Some(c.to_ascii_uppercase() as u32);
        }
    }
    if let Some(d) = lower.strip_prefix("numpad") {
        if let Ok(d) = d.parse::<u32>() {
            return (d <= 9 && lower.len() == 7).then_some(0x60 + d);
        }
    }
    if let Some(d) = lower.strip_prefix('f') {
        if let Ok(d) = d.parse::<u32>() {
            return ((1..=24).contains(&d) && !lower[1..].starts_with('0')).then(|| 0x70 + d - 1);
        }
    }
    NAMED
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&lower))
        .or_else(|| ALIASES.iter().find(|(k, _)| *k == lower))
        .map(|(_, v)| *v)
}

/// All keys PowerKit can name, as `(name, code)`, for populating UI pickers.
pub fn all() -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = (0x41..=0x5Au32)
        .chain(0x30..=0x39)
        .chain(0x70..=0x87)
        .chain(0x60..=0x69)
        .filter_map(|vk| name(vk).map(|n| (n, vk)))
        .collect();
    out.extend(NAMED.iter().map(|(n, v)| ((*n).to_string(), *v)));
    out
}

pub fn is_modifier(vk: u32) -> bool {
    matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN | 0xA0..=0xA5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for (n, vk) in all() {
            assert_eq!(parse(&n), Some(vk), "{n}");
            assert_eq!(name(vk).as_deref(), Some(n.as_str()));
        }
    }

    #[test]
    fn parsing_rules() {
        assert_eq!(parse("a"), Some(0x41));
        assert_eq!(parse(" f12 "), Some(0x7B));
        assert_eq!(parse("F24"), Some(0x87));
        assert_eq!(parse("F25"), None);
        assert_eq!(parse("F01"), None);
        assert_eq!(parse("numpad5"), Some(0x65));
        assert_eq!(parse("Numpad10"), None);
        assert_eq!(parse("ESCAPE"), Some(VK_ESCAPE));
        assert_eq!(parse("é"), None);
        assert_eq!(parse(""), None);
        assert_eq!(name(0xFF), None);
    }
}
