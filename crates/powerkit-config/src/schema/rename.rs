use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseMode {
    #[default]
    Lower,
    Upper,
    /// Capitalize Every Word.
    Title,
    /// Capitalize only the first letter.
    Sentence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Start,
    #[default]
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    /// Time the rename is previewed.
    Now,
    #[default]
    Modified,
    Created,
    /// EXIF `DateTimeOriginal`; falls back to the modified time when absent.
    ExifTaken,
}

/// Which part of the file name text operations act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Name without extension (the extension is preserved).
    #[default]
    Stem,
    Extension,
    /// The complete name including extension.
    Full,
}

/// One step of a rename pipeline. Steps run in order on each item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RenameOp {
    /// Find and replace. With `regex`, `with` may use `$1`, `${name}` capture references.
    Replace {
        find: String,
        #[serde(default)]
        with: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
        /// Replace only the first match instead of all.
        #[serde(default)]
        first_only: bool,
    },
    /// Remove every occurrence of `text`.
    Remove {
        text: String,
        #[serde(default)]
        case_insensitive: bool,
    },
    Case {
        mode: CaseMode,
    },
    /// Add a prefix (`Start`) or suffix (`End`).
    Insert {
        text: String,
        position: Position,
    },
    /// Replace `length` characters starting at character `start` (0-based) with `with`.
    ReplaceRange {
        start: u32,
        length: u32,
        #[serde(default)]
        with: String,
    },
    ReplaceSpaces {
        with: String,
    },
    /// Sequential number per item, in list order.
    Number {
        start: i64,
        step: i64,
        /// Minimum digits (zero padded).
        padding: u32,
        position: Position,
        #[serde(default)]
        separator: String,
    },
    /// Insert a date/time. `format` supports %Y %y %m %d %H %M %S and %%.
    Date {
        source: DateSource,
        format: String,
        position: Position,
        #[serde(default)]
        separator: String,
    },
    /// Collapse runs of a repeated character. Empty `chars` collapses any repeated
    /// character; otherwise only the listed ones.
    Dedupe {
        #[serde(default)]
        chars: String,
    },
    /// Trim whitespace (or the listed characters) from both ends.
    Trim {
        #[serde(default)]
        chars: String,
    },
    /// Replace the extension (regardless of the pipeline target). Empty removes it.
    Extension {
        new: String,
    },
    /// Add the parent folder's name.
    ParentFolder {
        position: Position,
        #[serde(default)]
        separator: String,
    },
    /// Build the name from a template with `${name}`, `${ext}`, `${parent}`, `${n}`,
    /// `${n:3}`, `${size}`, `${modified:%Y-%m-%d}`, `${created:...}`, `${taken:...}`.
    Template {
        pattern: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenameOptions {
    pub ops: Vec<RenameOp>,
    pub target: Target,
    pub include_files: bool,
    pub include_folders: bool,
    /// Only items whose name matches this glob (`*.jpg;*.png`) are renamed. Empty = all.
    pub include: String,
    /// Items whose name matches this glob are skipped.
    pub exclude: String,
}

impl Default for RenameOptions {
    fn default() -> Self {
        Self {
            ops: Vec::new(),
            target: Target::Stem,
            include_files: true,
            include_folders: true,
            include: String::new(),
            exclude: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub options: RenameOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerRename {
    pub enabled: bool,
    pub presets: Vec<Preset>,
    /// Completed rename batches kept for undo.
    pub undo_limit: u32,
    /// Show "Rename with PowerKit" in the Explorer context menu (classic menu).
    pub context_menu: bool,
}

impl Default for PowerRename {
    fn default() -> Self {
        Self {
            enabled: true,
            presets: Vec::new(),
            undo_limit: 20,
            context_menu: false,
        }
    }
}

impl PowerRename {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        super::clamp(&mut self.undo_limit, 0, 200, "PowerRename undo history size", w);
        let mut seen = std::collections::HashSet::new();
        self.presets.retain_mut(|p| {
            p.name = p.name.trim().chars().take(60).collect();
            p.options.ops.truncate(50);
            let ok = !p.name.is_empty() && seen.insert(p.name.to_lowercase());
            if !ok {
                w.push(format!("PowerRename: dropped unnamed or duplicate preset {:?}", p.name));
            }
            ok
        });
        self.presets.truncate(100);
    }
}
