//! The configuration schema. Every struct is `#[serde(default)]` so documents written by
//! older versions (missing fields) and newer versions (unknown fields) both load.

mod awake;
mod color_picker;
mod fancy_zones;
mod keyboard;
mod launcher;
mod rename;

pub use awake::*;
pub use color_picker::*;
pub use fancy_zones::*;
pub use keyboard::*;
pub use launcher::*;
pub use rename::*;

use powerkit_core::Shortcut;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version written by this build. Increase when a migration is added to [`MIGRATIONS`].
pub const CONFIG_VERSION: u32 = 1;

/// Migration steps; entry `i` upgrades a document from version `i + 1` to `i + 2`.
pub const MIGRATIONS: &[fn(&mut Value)] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub launch_at_startup: bool,
    /// Closing the settings window keeps PowerKit running in the tray.
    pub minimize_to_tray: bool,
    /// Do not open the settings window when PowerKit starts.
    pub start_minimized: bool,
    pub theme: Theme,
    pub log_level: LogLevel,
    /// Module ids, most recently used first (bounded).
    pub recent_modules: Vec<String>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            launch_at_startup: false,
            minimize_to_tray: true,
            start_minimized: true,
            theme: Theme::System,
            log_level: LogLevel::Info,
            recent_modules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlwaysOnTop {
    pub enabled: bool,
    pub shortcut: String,
    pub show_border: bool,
    /// `#RRGGBB`.
    pub border_color: String,
    /// Border thickness in pixels (1-20).
    pub border_thickness: u32,
    /// Opacity applied to pinned windows, 20-100. 100 leaves windows untouched.
    pub opacity_percent: u32,
    /// Executable names (`notepad.exe`) pinned automatically when they come to the front.
    pub auto_pin_apps: Vec<String>,
    /// Executable names that are never pinned.
    pub excluded_apps: Vec<String>,
}

impl Default for AlwaysOnTop {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Ctrl+T".into(),
            show_border: true,
            border_color: "#0F7BFF".into(),
            border_thickness: 3,
            opacity_percent: 100,
            auto_pin_apps: Vec::new(),
            excluded_apps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub general: General,
    pub always_on_top: AlwaysOnTop,
    pub color_picker: ColorPicker,
    pub fancy_zones: FancyZones,
    pub power_rename: PowerRename,
    pub keyboard_manager: KeyboardManager,
    pub launcher: Launcher,
    pub awake: Awake,
}

pub(crate) fn fix_shortcut(value: &mut String, default: &str, what: &str, warnings: &mut Vec<String>) {
    if value.trim().is_empty() {
        value.clear(); // empty means "no shortcut"
        return;
    }
    match value.parse::<Shortcut>() {
        Ok(s) if s.is_valid_global() => *value = s.to_string(),
        Ok(_) => {
            warnings.push(format!(
                "{what}: shortcut {value:?} needs a modifier key; restored {default:?}"
            ));
            *value = default.to_string();
        }
        Err(e) => {
            warnings.push(format!("{what}: {e}; restored {default:?}"));
            *value = default.to_string();
        }
    }
}

pub(crate) fn fix_color(value: &mut String, default: &str, what: &str, warnings: &mut Vec<String>) {
    let ok = value.len() == 7 && value.starts_with('#') && value[1..].chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        *value = value.to_ascii_uppercase();
    } else {
        warnings.push(format!("{what}: {value:?} is not a #RRGGBB color; restored {default}"));
        *value = default.to_string();
    }
}

pub(crate) fn clamp<T: PartialOrd + Copy + std::fmt::Display>(
    value: &mut T,
    min: T,
    max: T,
    what: &str,
    warnings: &mut Vec<String>,
) {
    if *value < min || *value > max {
        let fixed = if *value < min { min } else { max };
        warnings.push(format!("{what}: {value} is outside {min}..={max}; using {fixed}"));
        *value = fixed;
    }
}

/// Normalize a list of executable names: trimmed, lowercase, unique, non-empty.
pub(crate) fn fix_apps(list: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    list.retain_mut(|a| {
        *a = a.trim().to_lowercase();
        !a.is_empty() && seen.insert(a.clone())
    });
}

impl Config {
    pub fn new() -> Self {
        Self {
            version: CONFIG_VERSION,
            ..Default::default()
        }
    }

    /// Repair out-of-range or malformed values in place and describe every repair.
    /// After this returns the configuration is safe for every module to consume.
    pub fn sanitize(&mut self) -> Vec<String> {
        let mut w = Vec::new();
        self.version = CONFIG_VERSION;

        self.general
            .recent_modules
            .retain(|m| powerkit_core::MODULE_IDS.contains(&m.as_str()));
        self.general.recent_modules.dedup();
        self.general.recent_modules.truncate(6);

        let d = AlwaysOnTop::default();
        let a = &mut self.always_on_top;
        fix_shortcut(&mut a.shortcut, &d.shortcut, "Always On Top", &mut w);
        fix_color(&mut a.border_color, &d.border_color, "Always On Top border", &mut w);
        clamp(&mut a.border_thickness, 1, 20, "Always On Top border thickness", &mut w);
        clamp(&mut a.opacity_percent, 20, 100, "Always On Top opacity", &mut w);
        fix_apps(&mut a.auto_pin_apps);
        fix_apps(&mut a.excluded_apps);

        self.color_picker.sanitize(&mut w);
        self.fancy_zones.sanitize(&mut w);
        self.power_rename.sanitize(&mut w);
        self.keyboard_manager.sanitize(&mut w);
        self.launcher.sanitize(&mut w);
        self.awake.sanitize(&mut w);
        self.report_shortcut_conflicts(&mut w);
        w
    }

    /// Every global shortcut in the configuration as `(module id, action, shortcut)`.
    pub fn shortcuts(&self) -> Vec<(&'static str, String, String)> {
        let mut out = Vec::new();
        let mut push = |m: &'static str, action: &str, s: &str| {
            if !s.is_empty() {
                out.push((m, action.to_string(), s.to_string()));
            }
        };
        push("always_on_top", "toggle_pin", &self.always_on_top.shortcut);
        push("color_picker", "pick", &self.color_picker.shortcut);
        push("launcher", "toggle", &self.launcher.shortcut);
        let fz = &self.fancy_zones;
        push("fancy_zones", "next_zone", &fz.next_zone_shortcut);
        push("fancy_zones", "prev_zone", &fz.prev_zone_shortcut);
        push("fancy_zones", "cycle_layout", &fz.cycle_layout_shortcut);
        push("fancy_zones", "toggle_overlay", &fz.toggle_overlay_shortcut);
        if !fz.zone_number_modifiers.is_empty() {
            for n in 1..=9 {
                push(
                    "fancy_zones",
                    &format!("zone_{n}"),
                    &format!("{}+{n}", fz.zone_number_modifiers),
                );
            }
        }
        out
    }

    fn report_shortcut_conflicts(&self, warnings: &mut Vec<String>) {
        let all = self.shortcuts();
        for (i, (m1, a1, s1)) in all.iter().enumerate() {
            for (m2, a2, s2) in &all[i + 1..] {
                if s1 == s2 {
                    warnings.push(format!("shortcut {s1} is assigned to both {m1}/{a1} and {m2}/{a2}"));
                }
            }
        }
    }

    pub fn module_enabled(&self, id: &str) -> Option<bool> {
        Some(match id {
            "always_on_top" => self.always_on_top.enabled,
            "color_picker" => self.color_picker.enabled,
            "fancy_zones" => self.fancy_zones.enabled,
            "power_rename" => self.power_rename.enabled,
            "keyboard_manager" => self.keyboard_manager.enabled,
            "launcher" => self.launcher.enabled,
            "awake" => self.awake.enabled,
            _ => return None,
        })
    }

    pub fn set_module_enabled(&mut self, id: &str, enabled: bool) -> bool {
        let slot = match id {
            "always_on_top" => &mut self.always_on_top.enabled,
            "color_picker" => &mut self.color_picker.enabled,
            "fancy_zones" => &mut self.fancy_zones.enabled,
            "power_rename" => &mut self.power_rename.enabled,
            "keyboard_manager" => &mut self.keyboard_manager.enabled,
            "launcher" => &mut self.launcher.enabled,
            "awake" => &mut self.awake.enabled,
            _ => return false,
        };
        *slot = enabled;
        true
    }

    /// Record that a module was used (for the "recently used" list on the home page).
    pub fn touch_recent(&mut self, id: &str) {
        let r = &mut self.general.recent_modules;
        r.retain(|m| m != id);
        r.insert(0, id.to_string());
        r.truncate(6);
    }

    /// Build a configuration from an untrusted JSON value: migrate, then deserialize each
    /// section independently so one damaged section cannot discard the others.
    pub fn from_value(mut value: Value) -> (Config, Vec<String>) {
        let mut warnings = Vec::new();
        let found = value.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
        if found > CONFIG_VERSION {
            warnings.push(format!(
                "configuration was written by a newer PowerKit (version {found}); unknown settings are ignored"
            ));
        }
        for (i, step) in MIGRATIONS.iter().enumerate() {
            if found as usize <= i + 1 {
                step(&mut value);
            }
        }
        let mut config = Config::new();
        let Value::Object(mut map) = value else {
            warnings.push("configuration root is not an object; using defaults".into());
            return (config, warnings);
        };
        fn section<T: serde::de::DeserializeOwned + Default>(
            map: &mut serde_json::Map<String, Value>,
            key: &str,
            warnings: &mut Vec<String>,
        ) -> T {
            match map.remove(key) {
                None => T::default(),
                Some(v) => serde_json::from_value(v).unwrap_or_else(|e| {
                    warnings.push(format!("section {key:?} is damaged ({e}); restored defaults for it"));
                    T::default()
                }),
            }
        }
        config.general = section(&mut map, "general", &mut warnings);
        config.always_on_top = section(&mut map, "always_on_top", &mut warnings);
        config.color_picker = section(&mut map, "color_picker", &mut warnings);
        config.fancy_zones = section(&mut map, "fancy_zones", &mut warnings);
        config.power_rename = section(&mut map, "power_rename", &mut warnings);
        config.keyboard_manager = section(&mut map, "keyboard_manager", &mut warnings);
        config.launcher = section(&mut map, "launcher", &mut warnings);
        config.awake = section(&mut map, "awake", &mut warnings);
        warnings.extend(config.sanitize());
        (config, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn defaults_are_clean() {
        let mut c = Config::new();
        let before = c.clone();
        assert_eq!(c.sanitize(), Vec::<String>::new());
        assert_eq!(c, before);
    }

    #[test]
    fn damaged_section_does_not_lose_others() {
        let (c, w) = Config::from_value(json!({
            "version": 1,
            "general": { "theme": "dark" },
            "always_on_top": "garbage",
            "launcher": { "max_results": 5 }
        }));
        assert_eq!(c.general.theme, Theme::Dark);
        assert_eq!(c.always_on_top, AlwaysOnTop::default());
        assert_eq!(c.launcher.max_results, 5);
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn sanitize_repairs_values() {
        let mut c = Config::new();
        c.always_on_top.shortcut = "T".into();
        c.always_on_top.border_color = "blue".into();
        c.always_on_top.opacity_percent = 3;
        c.always_on_top.excluded_apps = vec![" Foo.EXE ".into(), "foo.exe".into(), "".into()];
        c.color_picker.shortcut = "win+shift+c".into();
        let w = c.sanitize();
        assert_eq!(c.always_on_top.shortcut, "Win+Ctrl+T");
        assert_eq!(c.always_on_top.border_color, "#0F7BFF");
        assert_eq!(c.always_on_top.opacity_percent, 20);
        assert_eq!(c.always_on_top.excluded_apps, vec!["foo.exe"]);
        assert_eq!(c.color_picker.shortcut, "Win+Shift+C");
        assert_eq!(w.len(), 3, "{w:?}");
    }

    #[test]
    fn conflicting_shortcuts_are_reported() {
        let mut c = Config::new();
        c.launcher.shortcut = c.always_on_top.shortcut.clone();
        let w = c.sanitize();
        assert!(w.iter().any(|m| m.contains("assigned to both")), "{w:?}");
    }

    #[test]
    fn non_object_root() {
        let (c, w) = Config::from_value(json!([1, 2]));
        assert_eq!(c, Config::new());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn recent_modules() {
        let mut c = Config::new();
        c.touch_recent("launcher");
        c.touch_recent("color_picker");
        c.touch_recent("launcher");
        assert_eq!(c.general.recent_modules, vec!["launcher", "color_picker"]);
    }

    fn arb_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::from),
            any::<i64>().prop_map(Value::from),
            ".{0,12}".prop_map(Value::from),
        ];
        leaf.prop_recursive(3, 24, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::from),
                proptest::collection::vec(
                    (
                        prop_oneof![
                            Just("general".to_string()),
                            Just("fancy_zones".to_string()),
                            Just("keyboard_manager".to_string()),
                            Just("layouts".to_string()),
                            Just("zones".to_string()),
                            Just("shortcut".to_string()),
                            Just("profiles".to_string()),
                            "[a-z_]{1,10}",
                        ],
                        inner
                    ),
                    0..5
                )
                .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        })
    }

    proptest! {
        /// Whatever JSON arrives, loading yields a configuration that is already
        /// sanitized (idempotent) and survives a serialization round trip.
        #[test]
        fn any_json_loads_to_a_stable_config(v in arb_json()) {
            let (c, _) = Config::from_value(v);
            let mut again = c.clone();
            prop_assert_eq!(again.sanitize(), Vec::<String>::new());
            prop_assert_eq!(&again, &c);
            let text = serde_json::to_string(&c).unwrap();
            let (back, w) = Config::from_value(serde_json::from_str(&text).unwrap());
            prop_assert_eq!(back, c);
            prop_assert!(w.is_empty());
        }
    }
}
