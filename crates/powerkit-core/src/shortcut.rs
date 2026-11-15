//! Keyboard shortcuts such as `Win+Ctrl+T`.

use crate::keys;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Modifier set (side-agnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Mods(pub u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);
    pub const WIN: Mods = Mods(8);

    pub const fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    pub const fn without(self, other: Mods) -> Mods {
        Mods(self.0 & !other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The modifier a virtual key represents, if it is a modifier key.
    pub fn from_vk(vk: u32) -> Option<Mods> {
        match vk {
            keys::VK_CONTROL | keys::VK_LCONTROL | keys::VK_RCONTROL => Some(Mods::CTRL),
            keys::VK_MENU | keys::VK_LMENU | keys::VK_RMENU => Some(Mods::ALT),
            keys::VK_SHIFT | keys::VK_LSHIFT | keys::VK_RSHIFT => Some(Mods::SHIFT),
            keys::VK_LWIN | keys::VK_RWIN => Some(Mods::WIN),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ShortcutError {
    #[error("shortcut is empty")]
    Empty,
    #[error("unknown key {0:?}")]
    UnknownKey(String),
    #[error("shortcut needs exactly one non-modifier key")]
    KeyCount,
    #[error("modifier {0:?} is repeated")]
    DuplicateModifier(String),
}

/// A modifier set plus one non-modifier key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub mods: Mods,
    /// Virtual-key code of the non-modifier key.
    pub key: u32,
}

impl Shortcut {
    pub const fn new(mods: Mods, key: u32) -> Self {
        Self { mods, key }
    }

    /// Global hotkeys must carry a modifier so they cannot shadow ordinary typing.
    /// Function keys are allowed bare.
    pub fn is_valid_global(&self) -> bool {
        !self.mods.is_empty() || (0x70..=0x87).contains(&self.key)
    }
}

impl FromStr for Shortcut {
    type Err = ShortcutError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err(ShortcutError::Empty);
        }
        let mut mods = Mods::NONE;
        let mut key = None;
        for part in s.split('+') {
            let part = part.trim();
            let m = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Some(Mods::CTRL),
                "alt" => Some(Mods::ALT),
                "shift" => Some(Mods::SHIFT),
                "win" | "super" | "meta" => Some(Mods::WIN),
                _ => None,
            };
            if let Some(m) = m {
                if mods.contains(m) {
                    return Err(ShortcutError::DuplicateModifier(part.to_string()));
                }
                mods = mods.union(m);
                continue;
            }
            let vk = keys::parse(part).ok_or_else(|| ShortcutError::UnknownKey(part.to_string()))?;
            if keys::is_modifier(vk) || key.replace(vk).is_some() {
                return Err(ShortcutError::KeyCount);
            }
        }
        Ok(Shortcut {
            mods,
            key: key.ok_or(ShortcutError::KeyCount)?,
        })
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (m, n) in [
            (Mods::WIN, "Win"),
            (Mods::CTRL, "Ctrl"),
            (Mods::ALT, "Alt"),
            (Mods::SHIFT, "Shift"),
        ] {
            if self.mods.contains(m) {
                write!(f, "{n}+")?;
            }
        }
        match keys::name(self.key) {
            Some(n) => f.write_str(&n),
            None => write!(f, "VK{:02X}", self.key),
        }
    }
}

impl Serialize for Shortcut {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Shortcut {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_and_display() {
        let s: Shortcut = "ctrl + WIN+t".parse().unwrap();
        assert_eq!(s, Shortcut::new(Mods::CTRL.union(Mods::WIN), 0x54));
        assert_eq!(s.to_string(), "Win+Ctrl+T");
        assert_eq!("Alt+Space".parse::<Shortcut>().unwrap().to_string(), "Alt+Space");
    }

    #[test]
    fn errors() {
        assert_eq!("".parse::<Shortcut>(), Err(ShortcutError::Empty));
        assert_eq!("Ctrl+Alt".parse::<Shortcut>(), Err(ShortcutError::KeyCount));
        assert_eq!("Ctrl+A+B".parse::<Shortcut>(), Err(ShortcutError::KeyCount));
        assert_eq!("Ctrl+LShift".parse::<Shortcut>(), Err(ShortcutError::KeyCount));
        assert!(matches!(
            "Ctrl+Ctrl+A".parse::<Shortcut>(),
            Err(ShortcutError::DuplicateModifier(_))
        ));
        assert!(matches!(
            "Ctrl+Nope".parse::<Shortcut>(),
            Err(ShortcutError::UnknownKey(_))
        ));
        assert!(matches!(
            "Ctrl++".parse::<Shortcut>(),
            Err(ShortcutError::UnknownKey(_))
        ));
    }

    #[test]
    fn global_validity() {
        assert!(!"A".parse::<Shortcut>().unwrap().is_valid_global());
        assert!("F9".parse::<Shortcut>().unwrap().is_valid_global());
        assert!("Win+A".parse::<Shortcut>().unwrap().is_valid_global());
    }

    #[test]
    fn serde_as_string() {
        let s: Shortcut = serde_json::from_str("\"Win+Shift+C\"").unwrap();
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"Win+Shift+C\"");
        assert!(serde_json::from_str::<Shortcut>("\"Win+\"").is_err());
    }

    proptest! {
        #[test]
        fn display_round_trips(mods in 0u8..16, idx in 0usize..1000) {
            let all: Vec<u32> = keys::all().into_iter().map(|(_, v)| v).filter(|v| !keys::is_modifier(*v)).collect();
            let s = Shortcut::new(Mods(mods), all[idx % all.len()]);
            prop_assert_eq!(s.to_string().parse::<Shortcut>(), Ok(s));
        }

        #[test]
        fn parse_never_panics(s in ".{0,40}") {
            let _ = s.parse::<Shortcut>();
        }
    }
}
