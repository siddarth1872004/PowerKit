use super::{clamp, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    pub apps: bool,
    pub files: bool,
    pub calculator: bool,
    pub units: bool,
    pub settings: bool,
    pub clipboard: bool,
    pub processes: bool,
    pub commands: bool,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            apps: true,
            files: true,
            calculator: true,
            units: true,
            settings: true,
            clipboard: true,
            processes: true,
            commands: true,
        }
    }
}

/// A user-defined launcher entry. The program is started directly (never through a
/// shell), so arguments are not subject to shell interpretation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCommand {
    /// Word typed in the launcher to select this command.
    pub keyword: String,
    pub title: String,
    /// Executable, document or URL to open.
    pub program: String,
    /// Arguments; `{query}` is replaced with the text typed after the keyword.
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Launcher {
    pub enabled: bool,
    pub shortcut: String,
    /// Result rows shown at once (3-20).
    pub max_results: u32,
    pub providers: Providers,
    /// Folders indexed for file search. Empty selects Desktop, Documents and Downloads.
    pub file_roots: Vec<String>,
    /// Upper bound on indexed file-system entries (memory bound).
    pub max_indexed_files: u32,
    /// Clipboard text entries remembered in memory (0 disables clipboard history).
    pub clipboard_history_size: u32,
    /// Rank frequently chosen results higher.
    pub remember_history: bool,
    pub custom_commands: Vec<CustomCommand>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Alt+Space".into(),
            max_results: 8,
            providers: Providers::default(),
            file_roots: Vec::new(),
            max_indexed_files: 150_000,
            clipboard_history_size: 25,
            remember_history: true,
            custom_commands: Vec::new(),
        }
    }
}

impl Launcher {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = Self::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "PowerKit Run", w);
        clamp(&mut self.max_results, 3, 20, "PowerKit Run result count", w);
        clamp(
            &mut self.max_indexed_files,
            1_000,
            1_000_000,
            "PowerKit Run file index limit",
            w,
        );
        clamp(
            &mut self.clipboard_history_size,
            0,
            200,
            "PowerKit Run clipboard history size",
            w,
        );
        self.file_roots.retain_mut(|r| {
            *r = r.trim().to_string();
            !r.is_empty() && r.len() <= 1024
        });
        self.file_roots.dedup();
        self.file_roots.truncate(32);
        let mut seen = std::collections::HashSet::new();
        self.custom_commands.retain_mut(|c| {
            c.keyword = c.keyword.trim().to_lowercase();
            c.program = c.program.trim().to_string();
            c.title = c.title.trim().chars().take(80).collect();
            let ok = !c.keyword.is_empty()
                && c.keyword.len() <= 24
                && !c.keyword.contains(char::is_whitespace)
                && !c.program.is_empty()
                && c.program.len() <= 2048
                && c.args.len() <= 2048
                && seen.insert(c.keyword.clone());
            if !ok {
                w.push(format!(
                    "PowerKit Run: dropped invalid or duplicate command {:?}",
                    c.keyword
                ));
            } else if c.title.is_empty() {
                c.title = c.keyword.clone();
            }
            ok
        });
        self.custom_commands.truncate(100);
    }
}
