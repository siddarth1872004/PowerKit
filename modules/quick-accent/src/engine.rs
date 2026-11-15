//! The Quick Accent state machine, fed every physical key event. Holding a key that has
//! accents and pressing the trigger opens the bar; further trigger presses move the
//! selection; releasing the held key replaces the typed character with the selected
//! one. Esc or any other key closes the bar and leaves the typed character alone.

use crate::tables::accents;
use powerkit_config::AccentTrigger;
use std::collections::HashSet;

const VK_BACK: u32 = 0x08;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

/// Backspace, as a key down and up.
pub const ERASE: [(u32, bool); 2] = [(VK_BACK, true), (VK_BACK, false)];

/// The US-layout character a virtual key types, for keys that can have accents.
pub fn key_char(vk: u32) -> Option<char> {
    Some(match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_ascii_lowercase(),
        0xBA => ';',
        0xBB => '=',
        0xBC => ',',
        0xBD => '-',
        0xBE => '.',
        0xBF => '/',
        0xDE => '\'',
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Show {
        chars: Vec<String>,
        selected: usize,
    },
    Select(usize),
    /// Erase the character the held key typed and type this instead.
    Commit(String),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the key event is swallowed.
    pub swallow: bool,
    pub effect: Effect,
}

impl Outcome {
    fn pass(effect: Effect) -> Outcome {
        Outcome { swallow: false, effect }
    }
    fn swallow(effect: Effect) -> Outcome {
        Outcome { swallow: true, effect }
    }
}

struct Held {
    vk: u32,
    since_ms: u64,
    /// Shift was down when the key was pressed.
    shift: bool,
}

struct Open {
    vk: u32,
    chars: Vec<String>,
    selected: usize,
}

pub struct Engine {
    pub trigger: AccentTrigger,
    pub delay_ms: u64,
    pub languages: Vec<String>,
    held: Option<Held>,
    open: Option<Open>,
    shift: bool,
    /// Ctrl, Alt or Win is down: the keys form a shortcut, not text.
    other_mods: HashSet<u32>,
    /// Keys whose press was swallowed, so their release is swallowed too.
    eaten: HashSet<u32>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            trigger: AccentTrigger::Both,
            delay_ms: 300,
            languages: vec!["all".into()],
            held: None,
            open: None,
            shift: false,
            other_mods: HashSet::new(),
            eaten: HashSet::new(),
        }
    }
}

fn is_shift(vk: u32) -> bool {
    matches!(vk, VK_SHIFT | 0xA0 | 0xA1)
}

fn is_other_mod(vk: u32) -> bool {
    matches!(vk, VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN | 0xA2..=0xA5)
}

impl Engine {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Forget everything, for when the hook goes away.
    pub fn reset(&mut self) -> Effect {
        self.held = None;
        self.eaten.clear();
        self.other_mods.clear();
        self.shift = false;
        if self.open.take().is_some() {
            Effect::Close
        } else {
            Effect::None
        }
    }

    /// The direction a trigger key moves the selection: `Some(true)` forward.
    fn trigger_direction(&self, vk: u32) -> Option<bool> {
        let arrows = self.trigger != AccentTrigger::Space;
        let space = self.trigger != AccentTrigger::Arrows;
        match vk {
            VK_RIGHT if arrows => Some(true),
            VK_LEFT if arrows => Some(false),
            VK_SPACE if space => Some(!self.shift),
            _ => None,
        }
    }

    /// `upper`: given whether Shift was down when the held key was pressed, whether it
    /// typed a capital (Shift xor Caps Lock); asked only when the bar opens. `allowed`: whether the bar may open in the focused app.
    pub fn handle(
        &mut self,
        vk: u32,
        down: bool,
        now_ms: u64,
        upper: impl FnOnce(bool) -> bool,
        allowed: impl FnOnce() -> bool,
    ) -> Outcome {
        if is_shift(vk) {
            self.shift = down;
            return Outcome::pass(Effect::None);
        }
        if is_other_mod(vk) {
            if down {
                self.other_mods.insert(vk);
            } else {
                self.other_mods.remove(&vk);
            }
        }
        if !down && self.eaten.remove(&vk) {
            return Outcome::swallow(Effect::None);
        }
        let direction = self.trigger_direction(vk);
        if let Some(open) = &mut self.open {
            if vk == open.vk {
                if down {
                    // Auto-repeat of the held key would type more characters.
                    return Outcome::swallow(Effect::None);
                }
                let c = open.chars[open.selected].clone();
                self.open = None;
                self.held = None;
                return Outcome::pass(Effect::Commit(c));
            }
            if !down {
                return Outcome::pass(Effect::None);
            }
            if let Some(forward) = direction {
                let len = open.chars.len();
                open.selected = if forward {
                    (open.selected + 1) % len
                } else {
                    (open.selected + len - 1) % len
                };
                self.eaten.insert(vk);
                return Outcome::swallow(Effect::Select(open.selected));
            }
            self.open = None;
            self.held = None;
            if vk == VK_ESCAPE {
                self.eaten.insert(vk);
                return Outcome::swallow(Effect::Close);
            }
            return Outcome::pass(Effect::Close);
        }

        if !down {
            if self.held.as_ref().is_some_and(|h| h.vk == vk) {
                self.held = None;
            }
            return Outcome::pass(Effect::None);
        }
        if let (Some(h), Some(forward)) = (&self.held, direction) {
            if now_ms.saturating_sub(h.since_ms) >= self.delay_ms {
                let (held_vk, shift) = (h.vk, h.shift);
                let chars = key_char(held_vk)
                    .map(|k| accents(&self.languages, k, upper(shift)))
                    .unwrap_or_default();
                if !chars.is_empty() && allowed() {
                    let selected = if forward { 0 } else { chars.len() - 1 };
                    self.open = Some(Open {
                        vk: held_vk,
                        chars: chars.clone(),
                        selected,
                    });
                    self.eaten.insert(vk);
                    return Outcome::swallow(Effect::Show { chars, selected });
                }
            }
            return Outcome::pass(Effect::None);
        }
        let typable = self.other_mods.is_empty() && key_char(vk).is_some();
        match &self.held {
            Some(h) if h.vk == vk => {}
            _ if typable => {
                self.held = Some(Held {
                    vk,
                    since_ms: now_ms,
                    shift: self.shift,
                })
            }
            _ => self.held = None,
        }
        Outcome::pass(Effect::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u32 = 0x41;
    const Q: u32 = 0x51;

    fn engine() -> Engine {
        Engine {
            languages: vec!["french".into()],
            ..Engine::default()
        }
    }

    fn key(e: &mut Engine, vk: u32, down: bool, t: u64) -> Outcome {
        e.handle(vk, down, t, |shift| shift, || true)
    }

    fn show(chars: &[&str], selected: usize) -> Effect {
        Effect::Show {
            chars: chars.iter().map(|s| s.to_string()).collect(),
            selected,
        }
    }

    #[test]
    fn hold_trigger_release_commits() {
        let mut e = engine();
        assert!(!key(&mut e, A, true, 0).swallow);
        assert!(!key(&mut e, A, true, 200).swallow, "repeat before opening passes");
        let o = key(&mut e, VK_SPACE, true, 400);
        assert_eq!(o, Outcome::swallow(show(&["à", "â", "æ"], 0)));
        assert!(key(&mut e, A, true, 450).swallow, "repeat while open is eaten");
        assert!(key(&mut e, VK_SPACE, false, 460).swallow);
        assert_eq!(key(&mut e, VK_RIGHT, true, 500), Outcome::swallow(Effect::Select(1)));
        assert!(key(&mut e, VK_RIGHT, false, 510).swallow);
        assert_eq!(key(&mut e, A, false, 600), Outcome::pass(Effect::Commit("â".into())));
        assert!(!e.is_open());
    }

    #[test]
    fn left_starts_at_end_and_wraps() {
        let mut e = engine();
        key(&mut e, A, true, 0);
        assert_eq!(
            key(&mut e, VK_LEFT, true, 500),
            Outcome::swallow(show(&["à", "â", "æ"], 2))
        );
        key(&mut e, VK_LEFT, false, 510);
        assert_eq!(key(&mut e, VK_RIGHT, true, 520).effect, Effect::Select(0));
        assert_eq!(key(&mut e, VK_LEFT, true, 530).effect, Effect::Select(2));
        key(&mut e, VK_SHIFT, true, 540);
        assert_eq!(
            key(&mut e, VK_SPACE, true, 550).effect,
            Effect::Select(1),
            "Shift+Space goes back"
        );
    }

    #[test]
    fn fast_typing_is_not_caught() {
        let mut e = engine();
        key(&mut e, A, true, 0);
        assert_eq!(key(&mut e, VK_SPACE, true, 100), Outcome::pass(Effect::None));
        assert!(!e.is_open());
    }

    #[test]
    fn trigger_setting_limits_keys() {
        let mut e = Engine {
            trigger: AccentTrigger::Arrows,
            ..engine()
        };
        key(&mut e, A, true, 0);
        assert!(!key(&mut e, VK_SPACE, true, 500).swallow);
        assert!(
            !key(&mut e, VK_RIGHT, true, 600).swallow,
            "the typed space ended the hold"
        );
        key(&mut e, A, true, 1000);
        assert!(key(&mut e, VK_RIGHT, true, 1500).swallow);
    }

    #[test]
    fn escape_and_other_keys_close_without_commit() {
        let mut e = engine();
        key(&mut e, A, true, 0);
        key(&mut e, VK_SPACE, true, 500);
        assert_eq!(key(&mut e, VK_ESCAPE, true, 600), Outcome::swallow(Effect::Close));
        assert!(key(&mut e, VK_ESCAPE, false, 610).swallow);
        assert_eq!(key(&mut e, A, false, 700), Outcome::pass(Effect::None));

        key(&mut e, A, true, 1000);
        key(&mut e, VK_SPACE, true, 1500);
        assert_eq!(key(&mut e, Q, true, 1600), Outcome::pass(Effect::Close));
    }

    #[test]
    fn keys_without_accents_and_shortcuts_do_nothing() {
        let mut e = engine();
        key(&mut e, Q, true, 0);
        assert!(!key(&mut e, VK_SPACE, true, 500).swallow);

        key(&mut e, VK_CONTROL, true, 1000);
        key(&mut e, A, true, 1000);
        assert!(
            !key(&mut e, VK_RIGHT, true, 1500).swallow,
            "Ctrl+A then Right is a shortcut"
        );
    }

    #[test]
    fn capitals_and_excluded_apps() {
        let mut e = engine();
        key(&mut e, VK_SHIFT, true, 0);
        key(&mut e, A, true, 0);
        key(&mut e, VK_SHIFT, false, 100);
        let o = key(&mut e, VK_RIGHT, true, 500);
        assert_eq!(o.effect, show(&["À", "Â", "Æ"], 0));

        let mut e = engine();
        key(&mut e, A, true, 0);
        let o = e.handle(VK_RIGHT, true, 500, |s| s, || false);
        assert_eq!(o, Outcome::pass(Effect::None));
    }
}
