//! The remapping state machine. It sees every physical key event and answers, right
//! away, whether the event passes or is replaced by other strokes.
//!
//! Two stages run in order: key remaps turn one physical key into another key, a
//! shortcut, or nothing; shortcut remaps then look at the resulting keys together with
//! the modifiers currently held. A key held down is remembered with the remap it
//! started, so its release always undoes exactly what its press did, even if the
//! configuration changes in between.

use powerkit_config::{Profile, RemapTarget};
use powerkit_core::keys::{self, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_SHIFT};
use powerkit_core::{Mods, Shortcut};
use std::collections::HashMap;

/// One key going down (`true`) or up.
pub type Stroke = (u32, bool);

/// An unassigned virtual key, tapped before releasing Win or Alt on the user's behalf so
/// the release does not open the Start menu or the menu bar.
const VK_DUMMY: u32 = 0xE8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Pass,
    /// Swallow the event and send these strokes instead (none: just swallow).
    Replace(Vec<Stroke>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Key(u32),
    Shortcut(Shortcut),
    Disabled,
}

impl Target {
    fn parse(t: &RemapTarget) -> Option<Target> {
        Some(match t {
            RemapTarget::Key(k) => Target::Key(keys::parse(k)?),
            RemapTarget::Shortcut(s) => Target::Shortcut(s.parse().ok()?),
            RemapTarget::Disabled => Target::Disabled,
        })
    }

    fn mods(&self) -> Mods {
        match self {
            Target::Shortcut(s) => s.mods,
            _ => Mods::NONE,
        }
    }

    fn key(&self) -> Option<u32> {
        match self {
            Target::Key(k) => Some(*k),
            Target::Shortcut(s) => Some(s.key),
            Target::Disabled => None,
        }
    }
}

/// The remaps of one profile, resolved to virtual-key codes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Remaps {
    keys: HashMap<u32, Target>,
    /// `(from, app, to)`; `app` empty applies everywhere.
    shortcuts: Vec<(Shortcut, String, Target)>,
}

impl Remaps {
    /// Entries that do not parse are skipped; the configuration sanitizer has already
    /// reported them.
    pub fn from_profile(p: &Profile) -> Remaps {
        Remaps {
            keys: p
                .key_remaps
                .iter()
                .filter_map(|r| Some((keys::parse(&r.from)?, Target::parse(&r.to)?)))
                .collect(),
            shortcuts: p
                .shortcut_remaps
                .iter()
                .filter_map(|r| Some((r.from.parse().ok()?, r.app.clone(), Target::parse(&r.to)?)))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.shortcuts.is_empty()
    }

    /// A remap of the generic `Ctrl`/`Alt`/`Shift` applies to both physical keys.
    fn key(&self, vk: u32) -> Option<Target> {
        let generic = match vk {
            0xA0 | 0xA1 => Some(VK_SHIFT),
            0xA2 | 0xA3 => Some(VK_CONTROL),
            0xA4 | 0xA5 => Some(VK_MENU),
            _ => None,
        };
        self.keys
            .get(&vk)
            .or_else(|| generic.and_then(|g| self.keys.get(&g)))
            .copied()
    }

    /// App-specific remaps win over global ones.
    fn shortcut(&self, from: Shortcut, app: &str) -> Option<Target> {
        let mut found = None;
        for (s, a, t) in &self.shortcuts {
            if *s != from {
                continue;
            }
            if !a.is_empty() && a == app {
                return Some(*t);
            }
            if a.is_empty() {
                found = Some(*t);
            }
        }
        found
    }
}

const MOD_FLAGS: [(Mods, u32); 4] = [
    (Mods::CTRL, VK_LCONTROL),
    (Mods::ALT, VK_LMENU),
    (Mods::SHIFT, VK_LSHIFT),
    (Mods::WIN, VK_LWIN),
];

fn mod_of(vk: u32) -> Mods {
    Mods::from_vk(vk).unwrap_or(Mods::NONE)
}

/// A shortcut remap whose trigger key is still down.
#[derive(Debug, Clone)]
struct Active {
    target: Target,
    /// Modifiers we pressed for the target and must release.
    pressed: Vec<u32>,
    /// Held modifiers we released for the target; pressed again afterwards if still held.
    released: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct Engine {
    remaps: Remaps,
    /// Lowercase executable name of the foreground application.
    app: String,
    /// Modifier keys the user holds, after key remapping.
    held: Vec<u32>,
    /// Physical key -> the key remap its press started.
    keys_down: HashMap<u32, Target>,
    /// Key (after key remapping) -> the shortcut remap its press started.
    shortcuts_down: HashMap<u32, Active>,
}

impl Engine {
    pub fn new(remaps: Remaps) -> Engine {
        Engine {
            remaps,
            ..Default::default()
        }
    }

    pub fn set_remaps(&mut self, remaps: Remaps) {
        self.remaps = remaps;
    }

    pub fn set_app(&mut self, exe: &str) {
        self.app = exe.to_lowercase();
    }

    pub fn handle(&mut self, vk: u32, down: bool) -> Decision {
        let (logical, remapped) = self.key_stage(vk, down);
        let mut out = Vec::new();
        for (k, d) in logical {
            self.shortcut_stage(k, d, &mut out);
        }
        if !remapped && out == [(vk, down)] {
            Decision::Pass
        } else {
            Decision::Replace(out)
        }
    }

    fn key_stage(&mut self, vk: u32, down: bool) -> (Vec<Stroke>, bool) {
        let repeat = down && self.keys_down.contains_key(&vk);
        let target = if down {
            let t = self.keys_down.get(&vk).copied().or_else(|| self.remaps.key(vk));
            if let Some(t) = t {
                self.keys_down.insert(vk, t);
            }
            t
        } else {
            self.keys_down.remove(&vk)
        };
        let Some(target) = target else {
            return (vec![(vk, down)], false);
        };
        let strokes = match target {
            Target::Disabled => Vec::new(),
            Target::Key(k) => vec![(k, down)],
            Target::Shortcut(s) => {
                let mods: Vec<u32> = MOD_FLAGS
                    .iter()
                    .filter(|(m, _)| s.mods.contains(*m))
                    .map(|(_, v)| *v)
                    .collect();
                match (down, repeat) {
                    (true, true) => vec![(s.key, true)],
                    (true, false) => mods.iter().map(|m| (*m, true)).chain([(s.key, true)]).collect(),
                    (false, _) => std::iter::once((s.key, false))
                        .chain(mods.iter().rev().map(|m| (*m, false)))
                        .collect(),
                }
            }
        };
        (strokes, true)
    }

    fn held_mods(&self) -> Mods {
        self.held.iter().fold(Mods::NONE, |acc, v| acc.union(mod_of(*v)))
    }

    fn shortcut_stage(&mut self, vk: u32, down: bool, out: &mut Vec<Stroke>) {
        if keys::is_modifier(vk) {
            if down {
                if !self.held.contains(&vk) {
                    self.held.push(vk);
                }
            } else {
                self.held.retain(|v| *v != vk);
                // Already released on the user's behalf: the system has seen it go up.
                for a in self.shortcuts_down.values_mut() {
                    if let Some(i) = a.released.iter().position(|v| *v == vk) {
                        a.released.remove(i);
                        return;
                    }
                }
            }
            out.push((vk, down));
            return;
        }

        if !down {
            match self.shortcuts_down.remove(&vk) {
                Some(a) => {
                    if let Some(k) = a.target.key() {
                        out.push((k, false));
                    }
                    out.extend(a.pressed.iter().rev().map(|m| (*m, false)));
                    out.extend(a.released.iter().filter(|m| self.held.contains(m)).map(|m| (*m, true)));
                }
                None => out.push((vk, false)),
            }
            return;
        }

        if let Some(a) = self.shortcuts_down.get(&vk) {
            // Auto-repeat of a remapped shortcut.
            if let Some(k) = a.target.key() {
                out.push((k, true));
            }
            return;
        }
        let from = Shortcut::new(self.held_mods(), vk);
        let target = if from.mods.is_empty() {
            None
        } else {
            self.remaps.shortcut(from, &self.app)
        };
        let Some(target) = target else {
            out.push((vk, true));
            return;
        };
        let mut active = Active {
            target,
            pressed: Vec::new(),
            released: Vec::new(),
        };
        if let Some(key) = target.key() {
            let want = target.mods();
            active.released = self
                .held
                .iter()
                .copied()
                .filter(|v| !want.contains(mod_of(*v)))
                .collect();
            let have = self
                .held
                .iter()
                .filter(|v| !active.released.contains(v))
                .fold(Mods::NONE, |acc, v| acc.union(mod_of(*v)));
            active.pressed = MOD_FLAGS
                .iter()
                .filter(|(m, _)| want.contains(*m) && !have.contains(*m))
                .map(|(_, v)| *v)
                .collect();
            if active
                .released
                .iter()
                .any(|v| mod_of(*v) == Mods::WIN || mod_of(*v) == Mods::ALT)
            {
                out.extend([(VK_DUMMY, true), (VK_DUMMY, false)]);
            }
            out.extend(active.released.iter().map(|m| (*m, false)));
            out.extend(active.pressed.iter().map(|m| (*m, true)));
            out.push((key, true));
        }
        self.shortcuts_down.insert(vk, active);
    }

    /// Strokes that undo every remap still in progress (keys held while remapping is
    /// switched off), leaving the engine idle.
    pub fn release_all(&mut self) -> Vec<Stroke> {
        let mut out = Vec::new();
        for (_, a) in self.shortcuts_down.drain() {
            if let Some(k) = a.target.key() {
                out.push((k, false));
            }
            out.extend(a.pressed.iter().rev().map(|m| (*m, false)));
        }
        for (_, t) in self.keys_down.drain() {
            match t {
                Target::Key(k) => out.push((k, false)),
                Target::Shortcut(s) => {
                    out.push((s.key, false));
                    out.extend(
                        MOD_FLAGS
                            .iter()
                            .filter(|(m, _)| s.mods.contains(*m))
                            .map(|(_, v)| (*v, false)),
                    );
                }
                Target::Disabled => {}
            }
        }
        self.held.clear();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{KeyRemap, ShortcutRemap};

    const A: u32 = 0x41;
    const C: u32 = 0x43;
    const J: u32 = 0x4A;
    const V: u32 = 0x56;
    const CAPS: u32 = 0x14;
    const ESC: u32 = 0x1B;
    const DOWN: u32 = 0x28;
    const RCTRL: u32 = 0xA3;

    fn key(from: &str, to: RemapTarget) -> KeyRemap {
        KeyRemap { from: from.into(), to }
    }

    fn sc(from: &str, to: RemapTarget, app: &str) -> ShortcutRemap {
        ShortcutRemap {
            from: from.into(),
            to,
            app: app.into(),
        }
    }

    fn engine(keys: Vec<KeyRemap>, shortcuts: Vec<ShortcutRemap>) -> Engine {
        Engine::new(Remaps::from_profile(&Profile {
            name: "t".into(),
            key_remaps: keys,
            shortcut_remaps: shortcuts,
        }))
    }

    fn replace(s: &[Stroke]) -> Decision {
        Decision::Replace(s.to_vec())
    }

    #[test]
    fn unmapped_keys_pass() {
        let mut e = engine(vec![key("CapsLock", RemapTarget::Key("Esc".into()))], vec![]);
        assert_eq!(e.handle(A, true), Decision::Pass);
        assert_eq!(e.handle(A, false), Decision::Pass);
        assert_eq!(e.handle(VK_LCONTROL, true), Decision::Pass);
        assert_eq!(e.handle(C, true), Decision::Pass);
    }

    #[test]
    fn key_to_key_and_disabled() {
        let mut e = engine(
            vec![
                key("CapsLock", RemapTarget::Key("Esc".into())),
                key("A", RemapTarget::Disabled),
            ],
            vec![],
        );
        assert_eq!(e.handle(CAPS, true), replace(&[(ESC, true)]));
        assert_eq!(e.handle(CAPS, true), replace(&[(ESC, true)]));
        assert_eq!(e.handle(CAPS, false), replace(&[(ESC, false)]));
        assert_eq!(e.handle(A, true), replace(&[]));
        assert_eq!(e.handle(A, false), replace(&[]));
    }

    #[test]
    fn key_to_shortcut_presses_and_releases_modifiers() {
        let mut e = engine(
            vec![key("CapsLock", RemapTarget::Shortcut("Ctrl+Shift+Esc".into()))],
            vec![],
        );
        assert_eq!(
            e.handle(CAPS, true),
            replace(&[(VK_LCONTROL, true), (VK_LSHIFT, true), (ESC, true)])
        );
        assert_eq!(e.handle(CAPS, true), replace(&[(ESC, true)]));
        assert_eq!(
            e.handle(CAPS, false),
            replace(&[(ESC, false), (VK_LSHIFT, false), (VK_LCONTROL, false)])
        );
    }

    #[test]
    fn generic_modifier_remap_covers_both_sides() {
        let mut e = engine(vec![key("Ctrl", RemapTarget::Key("CapsLock".into()))], vec![]);
        assert_eq!(e.handle(RCTRL, true), replace(&[(CAPS, true)]));
        assert_eq!(e.handle(RCTRL, false), replace(&[(CAPS, false)]));
    }

    #[test]
    fn shortcut_to_shortcut_keeps_shared_modifiers() {
        let mut e = engine(vec![], vec![sc("Ctrl+J", RemapTarget::Shortcut("Ctrl+V".into()), "")]);
        assert_eq!(e.handle(VK_LCONTROL, true), Decision::Pass);
        assert_eq!(e.handle(J, true), replace(&[(V, true)]));
        assert_eq!(e.handle(J, true), replace(&[(V, true)]));
        assert_eq!(e.handle(J, false), replace(&[(V, false)]));
        assert_eq!(e.handle(VK_LCONTROL, false), Decision::Pass);
    }

    #[test]
    fn shortcut_to_key_releases_and_restores_modifiers() {
        let mut e = engine(vec![], vec![sc("Ctrl+J", RemapTarget::Key("Down".into()), "")]);
        e.handle(VK_LCONTROL, true);
        assert_eq!(e.handle(J, true), replace(&[(VK_LCONTROL, false), (DOWN, true)]));
        // Ctrl still held: the system gets it back after the remap ends.
        assert_eq!(e.handle(J, false), replace(&[(DOWN, false), (VK_LCONTROL, true)]));
    }

    #[test]
    fn modifier_released_first_is_swallowed() {
        let mut e = engine(vec![], vec![sc("Ctrl+J", RemapTarget::Key("Down".into()), "")]);
        e.handle(VK_LCONTROL, true);
        e.handle(J, true);
        // The system already saw Ctrl go up.
        assert_eq!(e.handle(VK_LCONTROL, false), replace(&[]));
        assert_eq!(e.handle(J, false), replace(&[(DOWN, false)]));
    }

    #[test]
    fn releasing_win_taps_a_dummy_key_first() {
        let mut e = engine(vec![], vec![sc("Win+J", RemapTarget::Shortcut("Ctrl+C".into()), "")]);
        e.handle(VK_LWIN, true);
        assert_eq!(
            e.handle(J, true),
            replace(&[
                (VK_DUMMY, true),
                (VK_DUMMY, false),
                (VK_LWIN, false),
                (VK_LCONTROL, true),
                (C, true)
            ])
        );
        assert_eq!(
            e.handle(J, false),
            replace(&[(C, false), (VK_LCONTROL, false), (VK_LWIN, true)])
        );
    }

    #[test]
    fn extra_modifiers_do_not_match() {
        let mut e = engine(vec![], vec![sc("Ctrl+J", RemapTarget::Disabled, "")]);
        e.handle(VK_LCONTROL, true);
        e.handle(VK_LSHIFT, true);
        assert_eq!(e.handle(J, true), Decision::Pass);
    }

    #[test]
    fn app_specific_remaps_win_and_only_apply_in_their_app() {
        let mut e = engine(
            vec![],
            vec![
                sc("Ctrl+J", RemapTarget::Disabled, "code.exe"),
                sc("Ctrl+J", RemapTarget::Shortcut("Ctrl+V".into()), ""),
            ],
        );
        e.handle(VK_LCONTROL, true);
        assert_eq!(e.handle(J, true), replace(&[(V, true)]));
        e.handle(J, false);
        e.set_app("Code.exe");
        assert_eq!(e.handle(J, true), replace(&[]));
        assert_eq!(e.handle(J, false), replace(&[]));
    }

    #[test]
    fn key_remap_feeds_shortcut_remaps() {
        // CapsLock acts as Ctrl, and Ctrl+J is remapped.
        let mut e = engine(
            vec![key("CapsLock", RemapTarget::Key("LCtrl".into()))],
            vec![sc("Ctrl+J", RemapTarget::Key("Down".into()), "")],
        );
        assert_eq!(e.handle(CAPS, true), replace(&[(VK_LCONTROL, true)]));
        assert_eq!(e.handle(J, true), replace(&[(VK_LCONTROL, false), (DOWN, true)]));
        assert_eq!(e.handle(J, false), replace(&[(DOWN, false), (VK_LCONTROL, true)]));
        assert_eq!(e.handle(CAPS, false), replace(&[(VK_LCONTROL, false)]));
    }

    #[test]
    fn config_change_mid_press_still_releases_cleanly() {
        let mut e = engine(vec![key("CapsLock", RemapTarget::Key("Esc".into()))], vec![]);
        e.handle(CAPS, true);
        e.set_remaps(Remaps::default());
        assert_eq!(e.handle(CAPS, false), replace(&[(ESC, false)]));
        assert_eq!(e.handle(CAPS, true), Decision::Pass);
    }

    #[test]
    fn release_all_lifts_everything_in_progress() {
        let mut e = engine(
            vec![key("CapsLock", RemapTarget::Shortcut("Ctrl+Esc".into()))],
            vec![sc("Alt+J", RemapTarget::Shortcut("Ctrl+V".into()), "")],
        );
        e.handle(CAPS, true);
        let mut out = e.release_all();
        out.sort();
        assert_eq!(out, vec![(ESC, false), (VK_LCONTROL, false)]);
        e.handle(VK_LMENU, true);
        e.handle(J, true);
        assert_eq!(e.release_all(), vec![(V, false), (VK_LCONTROL, false)]);
        assert!(e.release_all().is_empty());
    }
}
