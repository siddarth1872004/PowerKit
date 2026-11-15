//! The contract every PowerKit utility (built-in or future external) describes itself with.
//!
//! Built-in utilities are statically registered Rust modules; they still publish a
//! [`Manifest`] so the host, the settings UI and the launcher treat all utilities uniformly.
//! External plugins are *not* loaded yet (see `docs/SECURITY.md`); this crate already
//! validates manifests so that a future loader starts from a strict, fuzzed parser.

mod module;

pub use module::{Action, Ctx, HotkeyBinding, Module};

use serde::{Deserialize, Serialize};

/// Version of the host <-> utility contract. Bumped on breaking changes.
pub const API_VERSION: u32 = 1;

/// Capabilities a utility needs. Shown to the user and enforced for external plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Register system-wide shortcuts.
    GlobalHotkeys,
    /// Observe and rewrite keyboard input (low-level hook).
    KeyboardHook,
    /// Observe mouse input while a tool is active.
    MouseHook,
    /// Move, resize or restyle other applications' windows.
    WindowManagement,
    /// Read pixels from the screen.
    ScreenCapture,
    /// Read the clipboard.
    ClipboardRead,
    /// Write the clipboard.
    ClipboardWrite,
    /// Read file names and metadata.
    FilesystemRead,
    /// Rename files.
    FilesystemWrite,
    /// Start other programs.
    ProcessLaunch,
    /// Enumerate running processes.
    ProcessQuery,
}

/// A command a utility exposes to the host (tray, launcher, IPC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Stable identifier, unique within the utility (`"toggle_pin"`).
    pub id: String,
    /// Human readable title.
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// Type of a single setting, used to render generic settings UI for extensions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    Bool { default: bool },
    Integer { default: i64, min: i64, max: i64 },
    Text { default: String },
    Shortcut { default: String },
    Choice { default: String, options: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingSpec {
    pub key: String,
    pub title: String,
    #[serde(flatten)]
    pub kind: SettingKind,
}

/// Static description of a utility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Stable identifier (`"always_on_top"`): lowercase ASCII, digits and `_`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Utility version (semver string).
    pub version: String,
    /// Contract version the utility was built against.
    pub api_version: u32,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest is not valid JSON: {0}")]
    Parse(String),
    #[error("manifest is too large ({0} bytes, limit {MAX_MANIFEST_BYTES})")]
    TooLarge(usize),
    #[error("invalid id {0:?}: use 1-48 lowercase ASCII letters, digits or '_', starting with a letter")]
    InvalidId(String),
    #[error("field {0} must not be empty or longer than {1} characters")]
    InvalidField(&'static str, usize),
    #[error("utility targets API version {found}, host supports {API_VERSION}")]
    IncompatibleApi { found: u32 },
    #[error("duplicate {0} id {1:?}")]
    Duplicate(&'static str, String),
    #[error("setting {0:?} has an invalid range or default")]
    InvalidSetting(String),
}

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && id.len() <= 48
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl Manifest {
    /// Parse and validate an untrusted manifest.
    pub fn parse(json: &str) -> Result<Manifest, ManifestError> {
        if json.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge(json.len()));
        }
        let manifest: Manifest = serde_json::from_str(json).map_err(|e| ManifestError::Parse(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if !valid_id(&self.id) {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 64 {
            return Err(ManifestError::InvalidField("name", 64));
        }
        if self.version.trim().is_empty() || self.version.len() > 32 {
            return Err(ManifestError::InvalidField("version", 32));
        }
        if self.description.chars().count() > 512 {
            return Err(ManifestError::InvalidField("description", 512));
        }
        if self.api_version != API_VERSION {
            return Err(ManifestError::IncompatibleApi {
                found: self.api_version,
            });
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.commands {
            if !valid_id(&c.id) {
                return Err(ManifestError::InvalidId(c.id.clone()));
            }
            if !seen.insert(c.id.as_str()) {
                return Err(ManifestError::Duplicate("command", c.id.clone()));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for s in &self.settings {
            if !valid_id(&s.key) {
                return Err(ManifestError::InvalidId(s.key.clone()));
            }
            if !seen.insert(s.key.as_str()) {
                return Err(ManifestError::Duplicate("setting", s.key.clone()));
            }
            match &s.kind {
                SettingKind::Integer { default, min, max } if min > max || default < min || default > max => {
                    return Err(ManifestError::InvalidSetting(s.key.clone()));
                }
                SettingKind::Choice { default, options } if !options.contains(default) => {
                    return Err(ManifestError::InvalidSetting(s.key.clone()));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> Manifest {
        Manifest {
            id: "sample".into(),
            name: "Sample".into(),
            description: String::new(),
            version: "1.0.0".into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys],
            commands: vec![CommandSpec {
                id: "run".into(),
                title: "Run".into(),
                description: String::new(),
            }],
            settings: vec![SettingSpec {
                key: "size".into(),
                title: "Size".into(),
                kind: SettingKind::Integer {
                    default: 3,
                    min: 1,
                    max: 9,
                },
            }],
        }
    }

    #[test]
    fn round_trip() {
        let m = sample();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(Manifest::parse(&json).unwrap(), m);
    }

    #[test]
    fn rejects_bad_input() {
        let mut m = sample();
        m.id = "Bad Id".into();
        assert!(matches!(m.validate(), Err(ManifestError::InvalidId(_))));
        let mut m = sample();
        m.api_version = 99;
        assert_eq!(m.validate(), Err(ManifestError::IncompatibleApi { found: 99 }));
        let mut m = sample();
        m.commands.push(m.commands[0].clone());
        assert!(matches!(m.validate(), Err(ManifestError::Duplicate("command", _))));
        let mut m = sample();
        m.settings[0].kind = SettingKind::Integer {
            default: 10,
            min: 1,
            max: 9,
        };
        assert!(matches!(m.validate(), Err(ManifestError::InvalidSetting(_))));
        assert!(matches!(Manifest::parse("{"), Err(ManifestError::Parse(_))));
        let big = " ".repeat(MAX_MANIFEST_BYTES + 1);
        assert!(matches!(Manifest::parse(&big), Err(ManifestError::TooLarge(_))));
    }

    proptest! {
        #[test]
        fn parse_never_panics(s in ".{0,400}") {
            let _ = Manifest::parse(&s);
        }

        #[test]
        fn valid_ids_accepted(id in "[a-z][a-z0-9_]{0,47}") {
            let mut m = sample();
            m.id = id;
            prop_assert!(m.validate().is_ok());
        }
    }
}
