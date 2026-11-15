//! Turns a query into ranked results from every enabled provider.

use crate::index::Entry;
use crate::{calc, fuzzy, units};
use powerkit_config::{CustomCommand, Launcher as Settings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Typed before a query to list running processes to end.
pub const KILL_PREFIX: &str = "kill ";
/// Typed before a query to search the clipboard history.
pub const CLIP_PREFIX: &str = "clip ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Apps,
    Files,
    Calculator,
    Units,
    Settings,
    Clipboard,
    Processes,
    Commands,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Open {
        target: String,
        args: Option<String>,
    },
    Copy {
        text: String,
    },
    Kill {
        pid: u32,
        name: String,
    },
    /// A page of the PowerKit settings window.
    Settings {
        page: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// Stable identity used for the usage history.
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub provider: Provider,
    pub action: Action,
    pub score: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
}

/// Everything the engine searches besides the settings.
#[derive(Default)]
pub struct Sources<'a> {
    pub apps: &'a [Entry],
    pub files: &'a [Entry],
    pub clipboard: &'a [String],
    pub processes: &'a [Process],
    /// PowerKit utilities as `(id, name)`.
    pub modules: &'a [(&'a str, &'a str)],
}

/// How often each result was chosen.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub counts: HashMap<String, u32>,
}

const HISTORY_LIMIT: usize = 500;

impl History {
    pub fn record(&mut self, key: &str) {
        *self.counts.entry(key.to_string()).or_default() += 1;
        if self.counts.len() > HISTORY_LIMIT {
            let mut v: Vec<(String, u32)> = self.counts.drain().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            v.truncate(HISTORY_LIMIT * 4 / 5);
            self.counts = v.into_iter().collect();
        }
    }

    fn boost(&self, key: &str) -> i32 {
        self.counts
            .get(key)
            .map_or(0, |&n| (24.0 * (1.0 + n as f64).ln()) as i32)
    }
}

/// Windows Settings pages, opened through `ms-settings:` URIs.
#[rustfmt::skip]
const WINDOWS_SETTINGS: &[(&str, &str)] = &[
    ("Display", "display"), ("Sound", "sound"), ("Notifications", "notifications"),
    ("Power & battery", "powersleep"), ("Storage", "storagesense"), ("Multitasking", "multitasking"),
    ("Bluetooth & devices", "bluetooth"), ("Printers & scanners", "printers"), ("Mouse", "mousetouchpad"),
    ("Network & internet", "network-status"), ("Wi-Fi", "network-wifi"), ("VPN", "network-vpn"),
    ("Proxy", "network-proxy"), ("Personalization", "personalization"), ("Background", "personalization-background"),
    ("Colors", "personalization-colors"), ("Themes", "themes"), ("Taskbar", "taskbar"),
    ("Installed apps", "appsfeatures"), ("Default apps", "defaultapps"), ("Startup apps", "startupapps"),
    ("Accounts", "yourinfo"), ("Sign-in options", "signinoptions"), ("Date & time", "dateandtime"),
    ("Language & region", "regionlanguage"), ("Gaming", "gaming-gamebar"), ("Accessibility", "easeofaccess"),
    ("Privacy & security", "privacy"), ("Windows Update", "windowsupdate"), ("About this PC", "about"),
    ("Clipboard settings", "clipboard"), ("Night light", "nightlight"), ("Focus", "quiethours"),
    ("Keyboard", "typing"), ("Fonts", "fonts"), ("Developer settings", "developers"),
];

const MAX_CLIP_TITLE: usize = 80;

/// Whether `query` selects a custom command: its first word is the keyword.
fn custom_command<'c>(query: &str, commands: &'c [CustomCommand]) -> Option<(&'c CustomCommand, String)> {
    let (word, rest) = query.split_once(char::is_whitespace).unwrap_or((query, ""));
    let word = word.to_lowercase();
    commands
        .iter()
        .find(|c| c.keyword == word)
        .map(|c| (c, rest.trim().to_string()))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The action that runs `command` with `query` substituted for `{query}`. In a URL
/// program the query is percent-encoded so it cannot change the URL's structure.
pub fn command_action(command: &CustomCommand, query: &str) -> Action {
    let lower = command.program.to_ascii_lowercase();
    let in_program = if lower.starts_with("http://") || lower.starts_with("https://") {
        percent_encode(query)
    } else {
        query.to_string()
    };
    let target = command.program.replace("{query}", &in_program);
    let args = command.args.replace("{query}", query);
    Action::Open {
        target,
        args: (!args.trim().is_empty()).then_some(args),
    }
}

fn one_line(s: &str, max: usize) -> String {
    let line = s
        .split(['\r', '\n'])
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut t: String = line.chars().take(max).collect();
    if line.chars().count() > max || s.trim().lines().count() > 1 {
        t.push('…');
    }
    t
}

/// Search every enabled provider. Results are sorted best first, without duplicate
/// keys, and cut to `max_results`.
pub fn search(query: &str, settings: &Settings, src: &Sources, history: &History) -> Vec<Item> {
    let q = query.trim();
    let p = &settings.providers;
    let mut r: Vec<Item> = Vec::new();

    if let Some((cmd, rest)) = p
        .commands
        .then(|| custom_command(q, &settings.custom_commands))
        .flatten()
    {
        r.push(Item {
            key: format!("cmd:{}", cmd.keyword),
            title: if rest.is_empty() {
                cmd.title.clone()
            } else {
                format!("{}: {rest}", cmd.title)
            },
            subtitle: cmd.program.clone(),
            provider: Provider::Commands,
            action: command_action(cmd, &rest),
            score: 10_000,
        });
        return finish(r, settings, history);
    }

    // ASCII lowering keeps byte offsets valid for slicing `q`.
    let lower = q.to_ascii_lowercase();
    if p.processes && lower.starts_with(KILL_PREFIX.trim_end()) && (lower.len() == 4 || lower.starts_with(KILL_PREFIX))
    {
        let needle = q[4..].trim();
        for proc in src.processes {
            if let Some(s) = fuzzy::score(needle, &proc.name) {
                r.push(Item {
                    key: format!("kill:{}", proc.name.to_lowercase()),
                    title: format!("End {}", proc.name),
                    subtitle: format!("Process {}", proc.pid),
                    provider: Provider::Processes,
                    action: Action::Kill {
                        pid: proc.pid,
                        name: proc.name.clone(),
                    },
                    score: s,
                });
            }
        }
        return finish(r, settings, history);
    }
    if p.clipboard && lower.starts_with(CLIP_PREFIX.trim_end()) && (lower.len() == 4 || lower.starts_with(CLIP_PREFIX))
    {
        let needle = q[4..].trim().to_lowercase();
        for (i, text) in src.clipboard.iter().enumerate() {
            if needle.is_empty() || text.to_lowercase().contains(&needle) {
                r.push(Item {
                    key: format!("clip:{i}"),
                    title: one_line(text, MAX_CLIP_TITLE),
                    subtitle: "Copy from clipboard history".into(),
                    provider: Provider::Clipboard,
                    action: Action::Copy { text: text.clone() },
                    // Keep the history's newest-first order.
                    score: 1000 - i as i32,
                });
            }
        }
        // Clipboard entries are positional; usage history would reorder them wrongly.
        return finish(r, settings, &History::default());
    }
    if q.is_empty() {
        return Vec::new();
    }

    if p.calculator {
        if let Some(v) = calc::eval(q) {
            let text = calc::format(v);
            r.push(Item {
                key: "calc".into(),
                title: format!("= {text}"),
                subtitle: "Copy result".into(),
                provider: Provider::Calculator,
                action: Action::Copy { text },
                score: 5_000,
            });
        }
    }
    if p.units {
        if let Some(c) = units::convert(q) {
            let text = format!("{} {}", calc::format(c.value), c.symbol);
            r.push(Item {
                key: "units".into(),
                title: format!("{} = {text}", c.input),
                subtitle: "Copy result".into(),
                provider: Provider::Units,
                action: Action::Copy { text },
                score: 5_000,
            });
        }
    }
    if p.apps {
        for e in src.apps {
            if let Some(s) = fuzzy::score(q, &e.name) {
                r.push(Item {
                    key: format!("app:{}", e.path.to_lowercase()),
                    title: e.name.clone(),
                    subtitle: "Application".into(),
                    provider: Provider::Apps,
                    action: Action::Open {
                        target: e.path.clone(),
                        args: None,
                    },
                    score: s + 40,
                });
            }
        }
    }
    if p.files && q.chars().count() >= 2 {
        for e in src.files {
            if let Some(s) = fuzzy::score(q, &e.name) {
                r.push(Item {
                    key: format!("file:{}", e.path.to_lowercase()),
                    title: e.name.clone(),
                    subtitle: e.path.clone(),
                    provider: Provider::Files,
                    action: Action::Open {
                        target: e.path.clone(),
                        args: None,
                    },
                    score: s - if e.dir { 5 } else { 10 },
                });
            }
        }
    }
    if p.settings {
        for (id, name) in src.modules {
            let title = format!("PowerKit: {name}");
            if let Some(s) = fuzzy::score(q, name).or_else(|| fuzzy::score(q, &title)) {
                r.push(Item {
                    key: format!("pk:{id}"),
                    title,
                    subtitle: "Open PowerKit settings".into(),
                    provider: Provider::Settings,
                    action: Action::Settings { page: id.to_string() },
                    score: s,
                });
            }
        }
        for (name, uri) in WINDOWS_SETTINGS {
            if let Some(s) = fuzzy::score(q, name) {
                r.push(Item {
                    key: format!("ms:{uri}"),
                    title: name.to_string(),
                    subtitle: "Windows Settings".into(),
                    provider: Provider::Settings,
                    action: Action::Open {
                        target: format!("ms-settings:{uri}"),
                        args: None,
                    },
                    score: s,
                });
            }
        }
    }
    if p.commands {
        for c in &settings.custom_commands {
            if let Some(s) = fuzzy::score(q, &c.keyword).max(fuzzy::score(q, &c.title)) {
                r.push(Item {
                    key: format!("cmd:{}", c.keyword),
                    title: c.title.clone(),
                    subtitle: format!("Type \"{} …\" to run with text", c.keyword),
                    provider: Provider::Commands,
                    action: command_action(c, ""),
                    score: s,
                });
            }
        }
    }
    finish(r, settings, history)
}

fn finish(mut items: Vec<Item>, settings: &Settings, history: &History) -> Vec<Item> {
    if settings.remember_history {
        for it in &mut items {
            it.score += history.boost(&it.key);
        }
    }
    items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
    let mut seen = std::collections::HashSet::new();
    items.retain(|it| seen.insert(it.key.clone()));
    items.truncate(settings.max_results as usize);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str) -> Entry {
        Entry {
            name: name.into(),
            path: path.into(),
            dir: false,
        }
    }

    fn titles(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.title.as_str()).collect()
    }

    struct Data {
        apps: Vec<Entry>,
        files: Vec<Entry>,
        clipboard: Vec<String>,
        processes: Vec<Process>,
    }

    impl Data {
        fn new() -> Self {
            Data {
                apps: vec![
                    entry("Visual Studio Code", r"C:\sm\Visual Studio Code.lnk"),
                    entry("Notepad", r"C:\sm\Notepad.lnk"),
                    entry("Calculator", r"C:\sm\Calculator.lnk"),
                ],
                files: vec![entry("notes.txt", r"C:\Users\u\Documents\notes.txt")],
                clipboard: vec!["second copy".into(), "first\nline two".into()],
                processes: vec![
                    Process {
                        pid: 10,
                        name: "notepad.exe".into(),
                    },
                    Process {
                        pid: 11,
                        name: "chrome.exe".into(),
                    },
                ],
            }
        }

        fn search(&self, q: &str, s: &Settings, h: &History) -> Vec<Item> {
            let src = Sources {
                apps: &self.apps,
                files: &self.files,
                clipboard: &self.clipboard,
                processes: &self.processes,
                modules: &[("color_picker", "Color Picker")],
            };
            search(q, s, &src, h)
        }
    }

    #[test]
    fn apps_rank_above_files_and_empty_query_is_empty() {
        let d = Data::new();
        let s = Settings::default();
        let out = d.search("note", &s, &History::default());
        assert_eq!(titles(&out)[..2], ["Notepad", "notes.txt"]);
        assert_eq!(
            out[0].action,
            Action::Open {
                target: r"C:\sm\Notepad.lnk".into(),
                args: None
            }
        );
        assert!(d.search("  ", &s, &History::default()).is_empty());
    }

    #[test]
    fn calculator_and_units_lead() {
        let d = Data::new();
        let s = Settings::default();
        let out = d.search("2*21", &s, &History::default());
        assert_eq!(out[0].title, "= 42");
        assert_eq!(out[0].action, Action::Copy { text: "42".into() });
        let out = d.search("5 km to m", &s, &History::default());
        assert_eq!(out[0].title, "5 km = 5000 m");
    }

    #[test]
    fn history_boosts_frequent_choices() {
        let d = Data::new();
        let s = Settings::default();
        let mut h = History::default();
        let first = d.search("c", &s, &h)[0].title.clone();
        let other = d.search("c", &s, &h).into_iter().find(|i| i.title != first).unwrap();
        for _ in 0..20 {
            h.record(&other.key);
        }
        assert_eq!(d.search("c", &s, &h)[0].key, other.key);
        let mut off = s.clone();
        off.remember_history = false;
        assert_eq!(d.search("c", &off, &h)[0].title, first);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::default();
        for i in 0..HISTORY_LIMIT + 1 {
            h.record(&format!("k{i}"));
        }
        h.record("k0");
        assert!(h.counts.len() <= HISTORY_LIMIT);
        assert_eq!(h.counts.get("k0"), Some(&2));
    }

    #[test]
    fn custom_commands_substitute_query() {
        let d = Data::new();
        let s = Settings {
            custom_commands: vec![
                CustomCommand {
                    keyword: "g".into(),
                    title: "Search the web".into(),
                    program: "https://www.google.com/search?q={query}".into(),
                    args: String::new(),
                },
                CustomCommand {
                    keyword: "edit".into(),
                    title: "Edit file".into(),
                    program: "notepad.exe".into(),
                    args: "\"{query}\"".into(),
                },
            ],
            ..Settings::default()
        };
        let out = d.search("g rust & tauri", &s, &History::default());
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].action,
            Action::Open {
                target: "https://www.google.com/search?q=rust%20%26%20tauri".into(),
                args: None
            }
        );
        let out = d.search("EDIT a b.txt", &s, &History::default());
        assert_eq!(
            out[0].action,
            Action::Open {
                target: "notepad.exe".into(),
                args: Some("\"a b.txt\"".into())
            }
        );
        // Without arguments the command is still found by name.
        assert!(titles(&d.search("edi", &s, &History::default())).contains(&"Edit file"));
    }

    #[test]
    fn prefixes_select_processes_and_clipboard() {
        let d = Data::new();
        let s = Settings::default();
        let out = d.search("kill note", &s, &History::default());
        assert_eq!(titles(&out), ["End notepad.exe"]);
        assert_eq!(
            out[0].action,
            Action::Kill {
                pid: 10,
                name: "notepad.exe".into()
            }
        );
        assert_eq!(d.search("kill", &s, &History::default()).len(), 2);

        let out = d.search("clip", &s, &History::default());
        assert_eq!(titles(&out), ["second copy", "first…"]);
        let out = d.search("clip TWO", &s, &History::default());
        assert_eq!(
            out[0].action,
            Action::Copy {
                text: "first\nline two".into()
            }
        );
        // "killer" and "clipper" are ordinary queries.
        assert!(d
            .search("killer", &s, &History::default())
            .iter()
            .all(|i| i.provider != Provider::Processes));
    }

    #[test]
    fn settings_pages_and_disabled_providers() {
        let d = Data::new();
        let mut s = Settings::default();
        let out = d.search("color pick", &s, &History::default());
        assert_eq!(
            out[0].action,
            Action::Settings {
                page: "color_picker".into()
            }
        );
        assert!(d.search("night", &s, &History::default()).iter().any(|i| i.action
            == Action::Open {
                target: "ms-settings:nightlight".into(),
                args: None
            }));
        s.providers.apps = false;
        s.providers.calculator = false;
        assert!(d
            .search("notepad", &s, &History::default())
            .iter()
            .all(|i| i.provider != Provider::Apps));
        assert!(d
            .search("1+1", &s, &History::default())
            .iter()
            .all(|i| i.provider != Provider::Calculator));
    }

    #[test]
    fn results_are_capped() {
        let d = Data::new();
        let s = Settings {
            max_results: 3,
            ..Settings::default()
        };
        assert!(d.search("e", &s, &History::default()).len() <= 3);
    }
}
