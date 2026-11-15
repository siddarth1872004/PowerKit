use powerkit_core::{keys, Shortcut};
use serde::{Deserialize, Serialize};

pub const MAX_PROFILES: usize = 16;
pub const MAX_REMAPS: usize = 200;

/// What a remapped key or shortcut produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RemapTarget {
    /// A single key name (`"Esc"`).
    Key(String),
    /// A shortcut (`"Ctrl+C"`).
    Shortcut(String),
    /// Swallow the input.
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRemap {
    /// Key name, see `powerkit_core::keys`.
    pub from: String,
    pub to: RemapTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutRemap {
    pub from: String,
    pub to: RemapTarget,
    /// Executable name the remap is limited to (`"code.exe"`); empty applies everywhere.
    #[serde(default)]
    pub app: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub key_remaps: Vec<KeyRemap>,
    pub shortcut_remaps: Vec<ShortcutRemap>,
}

impl Profile {
    pub fn is_empty(&self) -> bool {
        self.key_remaps.is_empty() && self.shortcut_remaps.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardManager {
    pub enabled: bool,
    pub profiles: Vec<Profile>,
    /// Name of the profile in effect.
    pub active_profile: String,
}

impl Default for KeyboardManager {
    fn default() -> Self {
        Self {
            enabled: true,
            profiles: vec![Profile {
                name: "Default".into(),
                ..Default::default()
            }],
            active_profile: "Default".into(),
        }
    }
}

fn fix_target(to: &mut RemapTarget) -> Result<(), String> {
    match to {
        RemapTarget::Disabled => Ok(()),
        RemapTarget::Key(k) => {
            let vk = keys::parse(k).ok_or_else(|| format!("unknown key {k:?}"))?;
            *k = keys::name(vk).unwrap_or_default();
            Ok(())
        }
        RemapTarget::Shortcut(s) => {
            let parsed: Shortcut = s.parse().map_err(|e| format!("{e}"))?;
            if parsed.mods.is_empty() {
                // A shortcut without modifiers is just a key.
                *to = RemapTarget::Key(keys::name(parsed.key).unwrap_or_default());
            } else {
                *s = parsed.to_string();
            }
            Ok(())
        }
    }
}

impl KeyboardManager {
    pub fn active(&self) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == self.active_profile)
    }

    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        if self.profiles.len() > MAX_PROFILES {
            w.push(format!(
                "Keyboard Manager: only the first {MAX_PROFILES} profiles are kept"
            ));
            self.profiles.truncate(MAX_PROFILES);
        }
        if self.profiles.is_empty() {
            self.profiles.push(Profile {
                name: "Default".into(),
                ..Default::default()
            });
        }
        let mut names = std::collections::HashSet::new();
        for (i, p) in self.profiles.iter_mut().enumerate() {
            p.name = p.name.trim().chars().take(40).collect();
            if p.name.is_empty() {
                p.name = format!("Profile {}", i + 1);
            }
            let base = p.name.clone();
            let mut n = 2;
            while !names.insert(p.name.clone()) {
                p.name = format!("{base} ({n})");
                n += 1;
            }
            let pname = p.name.clone();

            let mut seen = std::collections::HashSet::new();
            let mut kept = 0usize;
            p.key_remaps.retain_mut(|r| {
                let res = (|| {
                    let from = keys::parse(&r.from).ok_or_else(|| format!("unknown key {:?}", r.from))?;
                    r.from = keys::name(from).unwrap_or_default();
                    fix_target(&mut r.to)?;
                    if r.to == RemapTarget::Key(r.from.clone()) {
                        return Err(format!("{} is mapped to itself", r.from));
                    }
                    if !seen.insert(from) {
                        return Err(format!("{} is mapped more than once", r.from));
                    }
                    kept += 1;
                    if kept > MAX_REMAPS {
                        return Err(format!("more than {MAX_REMAPS} key remaps"));
                    }
                    Ok(())
                })();
                if let Err(e) = &res {
                    w.push(format!("Keyboard Manager profile {pname:?}: dropped key remap ({e})"));
                }
                res.is_ok()
            });

            let mut seen = std::collections::HashSet::new();
            let mut kept = 0usize;
            p.shortcut_remaps.retain_mut(|r| {
                let res = (|| {
                    let from: Shortcut = r.from.parse().map_err(|e| format!("{e}"))?;
                    if from.mods.is_empty() {
                        return Err(format!("{} has no modifier; use a key remap instead", r.from));
                    }
                    r.from = from.to_string();
                    r.app = r.app.trim().to_lowercase();
                    fix_target(&mut r.to)?;
                    if r.to == RemapTarget::Shortcut(r.from.clone()) {
                        return Err(format!("{} is mapped to itself", r.from));
                    }
                    if !seen.insert((from, r.app.clone())) {
                        return Err(format!("{} is mapped more than once", r.from));
                    }
                    kept += 1;
                    if kept > MAX_REMAPS {
                        return Err(format!("more than {MAX_REMAPS} shortcut remaps"));
                    }
                    Ok(())
                })();
                if let Err(e) = &res {
                    w.push(format!(
                        "Keyboard Manager profile {pname:?}: dropped shortcut remap ({e})"
                    ));
                }
                res.is_ok()
            });
        }
        if self.active().is_none() {
            self.active_profile = self.profiles[0].name.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_invalid_remaps() {
        let mut km = KeyboardManager::default();
        km.profiles[0].key_remaps = vec![
            KeyRemap {
                from: "capslock".into(),
                to: RemapTarget::Key("escape".into()),
            },
            KeyRemap {
                from: "CapsLock".into(),
                to: RemapTarget::Disabled,
            },
            KeyRemap {
                from: "A".into(),
                to: RemapTarget::Key("a".into()),
            },
            KeyRemap {
                from: "Nope".into(),
                to: RemapTarget::Disabled,
            },
            KeyRemap {
                from: "B".into(),
                to: RemapTarget::Shortcut("C".into()),
            },
        ];
        km.profiles[0].shortcut_remaps = vec![
            ShortcutRemap {
                from: "ctrl+j".into(),
                to: RemapTarget::Key("Down".into()),
                app: " Code.EXE".into(),
            },
            ShortcutRemap {
                from: "J".into(),
                to: RemapTarget::Disabled,
                app: String::new(),
            },
            ShortcutRemap {
                from: "Ctrl+J".into(),
                to: RemapTarget::Disabled,
                app: "code.exe".into(),
            },
            ShortcutRemap {
                from: "Ctrl+J".into(),
                to: RemapTarget::Disabled,
                app: String::new(),
            },
        ];
        km.active_profile = "gone".into();
        let mut w = Vec::new();
        km.sanitize(&mut w);
        let p = &km.profiles[0];
        assert_eq!(
            p.key_remaps,
            vec![
                KeyRemap {
                    from: "CapsLock".into(),
                    to: RemapTarget::Key("Esc".into())
                },
                KeyRemap {
                    from: "B".into(),
                    to: RemapTarget::Key("C".into())
                },
            ]
        );
        assert_eq!(p.shortcut_remaps.len(), 2);
        assert_eq!(p.shortcut_remaps[0].app, "code.exe");
        assert_eq!(w.len(), 5, "{w:?}");
        assert_eq!(km.active_profile, "Default");
    }

    #[test]
    fn profile_names_are_unique() {
        let mut km = KeyboardManager::default();
        km.profiles.push(Profile {
            name: "Default".into(),
            ..Default::default()
        });
        km.profiles.push(Profile::default());
        km.sanitize(&mut Vec::new());
        let names: Vec<&str> = km.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Default", "Default (2)", "Profile 3"]);
    }
}
