use super::{clamp, fix_apps};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccentTrigger {
    /// Left/Right arrow while a letter is held.
    Arrows,
    /// Space while a letter is held; Shift+Space goes backwards.
    Space,
    #[default]
    Both,
}

/// Character sets the bar offers. `all` is every set together.
pub const ACCENT_LANGUAGES: &[&str] = &[
    "all",
    "catalan",
    "croatian",
    "currency",
    "czech",
    "danish",
    "dutch",
    "estonian",
    "finnish",
    "french",
    "german",
    "greek",
    "hungarian",
    "icelandic",
    "irish",
    "italian",
    "latvian",
    "lithuanian",
    "maori",
    "norwegian",
    "pinyin",
    "polish",
    "portuguese",
    "romanian",
    "slovak",
    "slovenian",
    "spanish",
    "special",
    "swedish",
    "turkish",
    "vietnamese",
    "welsh",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickAccent {
    pub enabled: bool,
    pub trigger: AccentTrigger,
    /// How long the letter must be held before the trigger opens the bar, 0-1000 ms, so
    /// fast typing such as "a " is never caught.
    pub input_delay_ms: u32,
    /// Ids from [`ACCENT_LANGUAGES`].
    pub languages: Vec<String>,
    /// Lowercase executable names where the bar never opens.
    pub excluded_apps: Vec<String>,
}

impl Default for QuickAccent {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger: AccentTrigger::Both,
            input_delay_ms: 300,
            languages: vec!["all".into()],
            excluded_apps: Vec::new(),
        }
    }
}

impl QuickAccent {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        clamp(&mut self.input_delay_ms, 0, 1000, "Quick Accent input delay", w);
        let mut seen = std::collections::HashSet::new();
        self.languages.retain_mut(|l| {
            *l = l.trim().to_lowercase();
            if !ACCENT_LANGUAGES.contains(&l.as_str()) {
                w.push(format!("Quick Accent: unknown character set {l:?} removed"));
                return false;
            }
            seen.insert(l.clone())
        });
        if self.languages.is_empty() {
            w.push("Quick Accent: no character set chosen; using all".into());
            self.languages.push("all".into());
        }
        fix_apps(&mut self.excluded_apps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repairs_values() {
        let mut q = QuickAccent {
            input_delay_ms: 5000,
            languages: vec![" French ".into(), "french".into(), "klingon".into()],
            excluded_apps: vec![" Game.EXE".into()],
            ..QuickAccent::default()
        };
        let mut w = Vec::new();
        q.sanitize(&mut w);
        assert_eq!(q.input_delay_ms, 1000);
        assert_eq!(q.languages, vec!["french"]);
        assert_eq!(q.excluded_apps, vec!["game.exe"]);
        assert_eq!(w.len(), 2, "{w:?}");

        let mut q = QuickAccent {
            languages: vec![],
            ..QuickAccent::default()
        };
        let mut w = Vec::new();
        q.sanitize(&mut w);
        assert_eq!(q.languages, vec!["all"]);
        assert_eq!(w.len(), 1);
    }
}
