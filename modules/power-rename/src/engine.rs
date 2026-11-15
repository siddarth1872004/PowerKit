//! The rename pipeline: compute new names, flag every problem before anything is
//! touched, and apply or undo a batch without ever overwriting an existing item.

use crate::date::DateTime;
use crate::glob::Glob;
use anyhow::{anyhow, bail, Context as _, Result};
use powerkit_config::{CaseMode, DateSource, Position, RenameOp, RenameOptions, Target};
use regex::{NoExpand, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Time source for date steps. `to_local` maps UTC Unix seconds to local wall-clock
/// seconds; it is injected so previews are reproducible in tests.
pub struct Clock {
    pub now: i64,
    pub to_local: fn(i64) -> Result<i64>,
}

/// Facts about one input item, read once per preview.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<i64>,
    pub created: Option<i64>,
    /// EXIF capture time. Cameras record local wall-clock time without a zone.
    pub taken: Option<DateTime>,
}

fn unix(t: std::io::Result<SystemTime>) -> Option<i64> {
    let t = t.ok()?;
    Some(match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    })
}

impl Entry {
    pub fn load(path: &Path, read_exif: bool) -> std::io::Result<Entry> {
        // Not following links: renaming a link renames the link itself.
        let meta = path.symlink_metadata()?;
        let is_dir = meta.is_dir();
        let taken = if read_exif && !is_dir {
            match crate::exif::date_taken(path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::debug!(path = %path.display(), error = %e, "EXIF not readable; using modified time");
                    None
                }
            }
        } else {
            None
        };
        Ok(Entry {
            path: path.to_path_buf(),
            is_dir,
            size: meta.len(),
            modified: unix(meta.modified()),
            created: unix(meta.created()),
            taken,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Rename,
    Unchanged,
    /// Filtered out by the include/exclude or file/folder options.
    Skipped,
    Conflict,
    Invalid,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Planned {
    pub path: PathBuf,
    pub name: String,
    pub new_name: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Planned {
    pub fn target(&self) -> PathBuf {
        self.path.with_file_name(&self.new_name)
    }

    fn mark(&mut self, status: Status, message: impl Into<String>) {
        self.status = status;
        self.message = Some(message.into());
    }
}

enum Token {
    Lit(String),
    Name,
    Ext,
    Parent,
    Counter(usize),
    Size,
    Time(DateSource, String),
}

enum Step {
    Regex {
        re: Regex,
        with: String,
        expand: bool,
        first_only: bool,
    },
    Case(CaseMode),
    Insert(Position, String),
    Range {
        start: usize,
        len: usize,
        with: String,
    },
    Number {
        start: i64,
        step: i64,
        padding: usize,
        position: Position,
        separator: String,
    },
    Date {
        source: DateSource,
        format: String,
        position: Position,
        separator: String,
    },
    Dedupe(Vec<char>),
    Trim(Vec<char>),
    Extension(String),
    Parent {
        position: Position,
        separator: String,
    },
    Template(Vec<Token>),
}

pub struct Pipeline {
    steps: Vec<Step>,
    target: Target,
    include_files: bool,
    include_folders: bool,
    include: Glob,
    exclude: Glob,
    needs_exif: bool,
}

fn literal(text: &str, case_insensitive: bool) -> Result<Regex> {
    if text.is_empty() {
        bail!("The text to find is empty");
    }
    Ok(RegexBuilder::new(&regex::escape(text))
        .case_insensitive(case_insensitive)
        .build()?)
}

fn parse_template(pattern: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut lit = String::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('$') {
        lit.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('$') {
            lit.push('$');
            rest = r;
            continue;
        }
        let Some(r) = rest.strip_prefix('{') else {
            lit.push('$');
            continue;
        };
        let end = r.find('}').ok_or_else(|| anyhow!("Unclosed \"${{\" in template"))?;
        let field = &r[..end];
        rest = &r[end + 1..];
        let (key, arg) = match field.split_once(':') {
            Some((k, a)) => (k, Some(a)),
            None => (field, None),
        };
        let date = |source| Token::Time(source, arg.unwrap_or("%Y-%m-%d").to_string());
        let token = match (key, arg) {
            ("name", None) => Token::Name,
            ("ext", None) => Token::Ext,
            ("parent", None) => Token::Parent,
            ("size", None) => Token::Size,
            ("n", None) => Token::Counter(1),
            ("n", Some(w)) => match w.parse::<usize>() {
                Ok(w) if (1..=10).contains(&w) => Token::Counter(w),
                _ => bail!("${{n:{w}}}: the width must be a number from 1 to 10"),
            },
            ("modified", _) => date(DateSource::Modified),
            ("created", _) => date(DateSource::Created),
            ("taken", _) => date(DateSource::ExifTaken),
            _ => bail!("Unknown template field ${{{field}}}"),
        };
        if !lit.is_empty() {
            tokens.push(Token::Lit(std::mem::take(&mut lit)));
        }
        tokens.push(token);
    }
    lit.push_str(rest);
    if !lit.is_empty() {
        tokens.push(Token::Lit(lit));
    }
    Ok(tokens)
}

/// Split a file name into stem and extension. Folders and names like `.gitignore` or
/// `name.` have no extension.
fn split(name: &str, is_dir: bool) -> (String, String) {
    match name.rfind('.') {
        Some(i) if !is_dir && i > 0 && i + 1 < name.len() => (name[..i].to_string(), name[i + 1..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

fn join(stem: &str, ext: &str) -> String {
    if ext.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{ext}")
    }
}

fn place(text: &str, value: &str, position: Position, separator: &str) -> String {
    let sep = if text.is_empty() || value.is_empty() {
        ""
    } else {
        separator
    };
    match position {
        Position::Start => format!("{value}{sep}{text}"),
        Position::End => format!("{text}{sep}{value}"),
    }
}

fn change_case(text: &str, mode: CaseMode) -> String {
    match mode {
        CaseMode::Lower => text.to_lowercase(),
        CaseMode::Upper => text.to_uppercase(),
        CaseMode::Title => {
            let mut out = String::with_capacity(text.len());
            let mut word_start = true;
            for c in text.chars() {
                if word_start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                // Apostrophes keep "don't" as one word.
                word_start = !(c.is_alphanumeric() || c == '\'');
            }
            out
        }
        CaseMode::Sentence => {
            let lower = text.to_lowercase();
            match lower.char_indices().find(|(_, c)| c.is_alphabetic()) {
                Some((i, c)) => format!("{}{}{}", &lower[..i], c.to_uppercase(), &lower[i + c.len_utf8()..]),
                None => lower,
            }
        }
    }
}

fn dedupe(text: &str, only: &[char]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev = None;
    for c in text.chars() {
        if prev == Some(c) && (only.is_empty() || only.contains(&c)) {
            continue;
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

impl Pipeline {
    pub fn compile(options: &RenameOptions) -> Result<Pipeline> {
        let mut needs_exif = false;
        let mut steps = Vec::with_capacity(options.ops.len());
        for (i, op) in options.ops.iter().enumerate() {
            let step = match op {
                RenameOp::Replace {
                    find,
                    with,
                    regex,
                    case_insensitive,
                    first_only,
                } => {
                    let re = if *regex {
                        if find.is_empty() {
                            bail!("The regular expression is empty");
                        }
                        RegexBuilder::new(find)
                            .case_insensitive(*case_insensitive)
                            .build()
                            .map_err(|e| anyhow!("Invalid regular expression: {e}"))?
                    } else {
                        literal(find, *case_insensitive)?
                    };
                    Step::Regex {
                        re,
                        with: with.clone(),
                        expand: *regex,
                        first_only: *first_only,
                    }
                }
                RenameOp::Remove { text, case_insensitive } => Step::Regex {
                    re: literal(text, *case_insensitive)?,
                    with: String::new(),
                    expand: false,
                    first_only: false,
                },
                RenameOp::Case { mode } => Step::Case(*mode),
                RenameOp::Insert { text, position } => Step::Insert(*position, text.clone()),
                RenameOp::ReplaceRange { start, length, with } => Step::Range {
                    start: *start as usize,
                    len: *length as usize,
                    with: with.clone(),
                },
                RenameOp::ReplaceSpaces { with } => Step::Regex {
                    re: literal(" ", false)?,
                    with: with.clone(),
                    expand: false,
                    first_only: false,
                },
                RenameOp::Number {
                    start,
                    step,
                    padding,
                    position,
                    separator,
                } => Step::Number {
                    start: *start,
                    step: *step,
                    padding: (*padding).min(20) as usize,
                    position: *position,
                    separator: separator.clone(),
                },
                RenameOp::Date {
                    source,
                    format,
                    position,
                    separator,
                } => {
                    needs_exif |= *source == DateSource::ExifTaken;
                    Step::Date {
                        source: *source,
                        format: format.clone(),
                        position: *position,
                        separator: separator.clone(),
                    }
                }
                RenameOp::Dedupe { chars } => Step::Dedupe(chars.chars().collect()),
                RenameOp::Trim { chars } => Step::Trim(chars.chars().collect()),
                RenameOp::Extension { new } => Step::Extension(new.trim_start_matches('.').to_string()),
                RenameOp::ParentFolder { position, separator } => Step::Parent {
                    position: *position,
                    separator: separator.clone(),
                },
                RenameOp::Template { pattern } => {
                    let tokens = parse_template(pattern).with_context(|| format!("Step {}", i + 1))?;
                    needs_exif |= tokens
                        .iter()
                        .any(|t| matches!(t, Token::Time(DateSource::ExifTaken, _)));
                    Step::Template(tokens)
                }
            };
            steps.push(step);
        }
        Ok(Pipeline {
            steps,
            target: options.target,
            include_files: options.include_files,
            include_folders: options.include_folders,
            include: Glob::new(&options.include),
            exclude: Glob::new(&options.exclude),
            needs_exif,
        })
    }

    fn included(&self, entry: &Entry, name: &str) -> bool {
        let kind_ok = if entry.is_dir {
            self.include_folders
        } else {
            self.include_files
        };
        kind_ok && (self.include.is_empty() || self.include.matches(name)) && !self.exclude.matches(name)
    }

    fn time(entry: &Entry, source: DateSource, clock: &Clock) -> Result<DateTime> {
        let utc = match source {
            DateSource::Now => clock.now,
            DateSource::Modified => entry
                .modified
                .ok_or_else(|| anyhow!("The modification time is unavailable"))?,
            DateSource::Created => entry
                .created
                .ok_or_else(|| anyhow!("The creation time is unavailable"))?,
            DateSource::ExifTaken => match entry.taken {
                Some(t) => return Ok(t),
                None => return Self::time(entry, DateSource::Modified, clock),
            },
        };
        Ok(DateTime::from_unix((clock.to_local)(utc)?))
    }

    fn run(
        &self,
        step: &Step,
        text: &str,
        entry: &Entry,
        original: &str,
        index: usize,
        clock: &Clock,
    ) -> Result<String> {
        Ok(match step {
            Step::Regex {
                re,
                with,
                expand,
                first_only,
            } => {
                let limit = if *first_only { 1 } else { 0 };
                if *expand {
                    re.replacen(text, limit, with.as_str()).into_owned()
                } else {
                    re.replacen(text, limit, NoExpand(with)).into_owned()
                }
            }
            Step::Case(mode) => change_case(text, *mode),
            Step::Insert(position, value) => place(text, value, *position, ""),
            Step::Range { start, len, with } => {
                let chars: Vec<char> = text.chars().collect();
                let a = (*start).min(chars.len());
                let b = a.saturating_add(*len).min(chars.len());
                let mut out: String = chars[..a].iter().collect();
                out.push_str(with);
                out.extend(&chars[b..]);
                out
            }
            Step::Number {
                start,
                step,
                padding,
                position,
                separator,
            } => {
                let n = start.saturating_add(step.saturating_mul(index as i64));
                place(text, &format!("{n:0width$}", width = *padding), *position, separator)
            }
            Step::Date {
                source,
                format,
                position,
                separator,
            } => place(
                text,
                &Self::time(entry, *source, clock)?.format(format),
                *position,
                separator,
            ),
            Step::Dedupe(only) => dedupe(text, only),
            Step::Trim(chars) if chars.is_empty() => text.trim().to_string(),
            Step::Trim(chars) => text.trim_matches(|c| chars.contains(&c)).to_string(),
            Step::Extension(_) => unreachable!("handled by apply"),
            Step::Parent { position, separator } => match parent_name(&entry.path) {
                Some(parent) => place(text, &parent, *position, separator),
                None => text.to_string(),
            },
            Step::Template(tokens) => {
                let (stem, ext) = split(original, entry.is_dir);
                let mut out = String::new();
                for t in tokens {
                    match t {
                        Token::Lit(s) => out.push_str(s),
                        Token::Name => out.push_str(&stem),
                        Token::Ext => out.push_str(&ext),
                        Token::Parent => out.push_str(&parent_name(&entry.path).unwrap_or_default()),
                        Token::Counter(w) => out.push_str(&format!("{:0width$}", index + 1, width = *w)),
                        Token::Size => out.push_str(&entry.size.to_string()),
                        Token::Time(source, fmt) => out.push_str(&Self::time(entry, *source, clock)?.format(fmt)),
                    }
                }
                out
            }
        })
    }

    /// The new name for `entry`; `index` counts only included items.
    fn apply(&self, entry: &Entry, name: &str, index: usize, clock: &Clock) -> Result<String> {
        if entry.is_dir && self.target == Target::Extension {
            return Ok(name.to_string());
        }
        let (mut stem, mut ext) = split(name, entry.is_dir);
        for step in &self.steps {
            if let Step::Extension(new) = step {
                if !entry.is_dir {
                    ext = new.clone();
                }
                continue;
            }
            let text = match self.target {
                Target::Stem => stem.clone(),
                Target::Extension => ext.clone(),
                Target::Full => join(&stem, &ext),
            };
            let out = self.run(step, &text, entry, name, index, clock)?;
            match self.target {
                Target::Stem => stem = out,
                Target::Extension => ext = out,
                Target::Full => (stem, ext) = split(&out, entry.is_dir),
            }
        }
        Ok(join(&stem, &ext))
    }
}

fn parent_name(path: &Path) -> Option<String> {
    Some(path.parent()?.file_name()?.to_string_lossy().into_owned())
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Windows file name rules. Checked on every platform so previews behave the same.
pub fn validate_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("The new name is empty".into());
    }
    if name == "." || name == ".." {
        return Err(format!("{name:?} is not a valid name"));
    }
    if name.encode_utf16().count() > 255 {
        return Err("The new name is longer than 255 characters".into());
    }
    if let Some(c) = name.chars().find(|&c| c < ' ' || "<>:\"/\\|?*".contains(c)) {
        return Err(format!("Names cannot contain {c:?}"));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("Names cannot end with a dot or a space".into());
    }
    let base = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    if RESERVED.contains(&base.as_str()) {
        return Err(format!("{base} is a reserved device name on Windows"));
    }
    Ok(())
}

/// Windows paths compare case-insensitively.
fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// Compute the outcome for every input path, in input order. Fails only when the
/// options themselves are invalid; per-item problems are reported in the result.
pub fn preview(paths: &[PathBuf], options: &RenameOptions, clock: &Clock) -> Result<Vec<Planned>> {
    let pipeline = Pipeline::compile(options)?;
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(paths.len());
    let mut index = 0;
    for path in paths {
        if !seen.insert(key(path)) {
            continue;
        }
        let shown = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut p = Planned {
            path: path.clone(),
            name: shown.clone(),
            new_name: shown,
            status: Status::Unchanged,
            message: None,
        };
        let name = match path.file_name().map(|n| n.to_str()) {
            None => {
                p.mark(Status::Invalid, "Drives and root folders cannot be renamed");
                out.push(p);
                continue;
            }
            Some(None) => {
                p.mark(Status::Invalid, "The current name is not valid Unicode");
                out.push(p);
                continue;
            }
            Some(Some(n)) => n.to_string(),
        };
        let entry = match Entry::load(path, pipeline.needs_exif) {
            Ok(e) => e,
            Err(e) => {
                p.mark(Status::Missing, e.to_string());
                out.push(p);
                continue;
            }
        };
        if !pipeline.included(&entry, &name) {
            p.status = Status::Skipped;
            out.push(p);
            continue;
        }
        match pipeline.apply(&entry, &name, index, clock) {
            Ok(new) => {
                p.new_name = new;
                if p.new_name != name {
                    match validate_name(&p.new_name) {
                        Ok(()) => p.status = Status::Rename,
                        Err(m) => p.mark(Status::Invalid, m),
                    }
                }
            }
            Err(e) => p.mark(Status::Invalid, format!("{e:#}")),
        }
        index += 1;
        out.push(p);
    }
    mark_conflicts(&mut out);
    Ok(out)
}

fn mark_conflicts(items: &mut [Planned]) {
    let mut targets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, p) in items.iter().enumerate().filter(|(_, p)| p.status == Status::Rename) {
        targets.entry(key(&p.target())).or_default().push(i);
    }
    for indices in targets.values().filter(|v| v.len() > 1) {
        for &i in indices {
            items[i].mark(Status::Conflict, "Another item would get the same name");
        }
    }
    // A target may be occupied by another selected item only if that item moves away.
    // Marking one conflict can invalidate another, so repeat until stable.
    loop {
        let sources: HashMap<String, bool> = items
            .iter()
            .map(|p| (key(&p.path), p.status == Status::Rename))
            .collect();
        let mut changed = false;
        for p in items.iter_mut().filter(|p| p.status == Status::Rename) {
            let target = p.target();
            let k = key(&target);
            if k == key(&p.path) {
                continue;
            }
            let blocked = match sources.get(&k) {
                Some(moves_away) => !moves_away,
                None => target.symlink_metadata().is_ok(),
            };
            if blocked {
                p.mark(Status::Conflict, "An item with this name already exists");
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[cfg(windows)]
fn move_no_replace(from: &Path, to: &Path) -> Result<()> {
    powerkit_platform::fs::move_no_replace(from, to).map_err(|e| anyhow!(e))
}

/// Non-Windows builds exist only to run the engine tests; the check-then-rename race
/// does not matter there.
#[cfg(not(windows))]
fn move_no_replace(from: &Path, to: &Path) -> Result<()> {
    if to.symlink_metadata().is_ok() {
        bail!("{} already exists", to.display());
    }
    std::fs::rename(from, to).with_context(|| format!("renaming {} to {}", from.display(), to.display()))
}

fn depth(path: &Path) -> usize {
    path.components().count()
}

fn perform(groups: &[Vec<&Move>], done: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for group in groups {
        // Every item first gets a unique temporary name, so swaps (a->b, b->a) and
        // case-only changes work without ever needing to replace anything.
        let mut temps = Vec::with_capacity(group.len());
        for (i, m) in group.iter().enumerate() {
            let tmp = m
                .from
                .with_file_name(format!(".powerkit-rename-{}-{stamp}-{i}.tmp", std::process::id()));
            move_no_replace(&m.from, &tmp)?;
            done.push((m.from.clone(), tmp.clone()));
            temps.push(tmp);
        }
        for (m, tmp) in group.iter().zip(temps) {
            move_no_replace(&tmp, &m.to)?;
            done.push((tmp, m.to.clone()));
        }
    }
    Ok(())
}

/// Execute moves as one batch: all succeed, or everything done so far is reverted.
/// Children are renamed before their parents (`deepest_first`); undo runs the
/// opposite way because recorded child paths use the parent's original name.
fn run_moves(moves: &[Move], deepest_first: bool) -> Result<()> {
    let mut by_depth: BTreeMap<usize, Vec<&Move>> = BTreeMap::new();
    for m in moves {
        by_depth.entry(depth(&m.from)).or_default().push(m);
    }
    let mut groups: Vec<Vec<&Move>> = by_depth.into_values().collect();
    if deepest_first {
        groups.reverse();
    }
    let mut done = Vec::new();
    let Err(e) = perform(&groups, &mut done) else {
        return Ok(());
    };
    let mut stuck = Vec::new();
    for (a, b) in done.iter().rev() {
        if let Err(re) = move_no_replace(b, a) {
            tracing::error!(error = %re, "rollback step failed");
            stuck.push(format!("{} (should be {})", b.display(), a.display()));
        }
    }
    if stuck.is_empty() {
        Err(e.context("Nothing was renamed"))
    } else {
        Err(e.context(format!(
            "Undoing the partial rename also failed; check these items: {}",
            stuck.join("; ")
        )))
    }
}

/// Rename every `Rename` item of a preview. Returns the moves for the undo journal.
pub fn apply(plan: &[Planned]) -> Result<Vec<Move>> {
    let moves: Vec<Move> = plan
        .iter()
        .filter(|p| p.status == Status::Rename)
        .map(|p| Move {
            from: p.path.clone(),
            to: p.target(),
        })
        .collect();
    run_moves(&moves, true)?;
    Ok(moves)
}

pub fn undo(moves: &[Move]) -> Result<()> {
    let reversed: Vec<Move> = moves
        .iter()
        .map(|m| Move {
            from: m.to.clone(),
            to: m.from.clone(),
        })
        .collect();
    run_moves(&reversed, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::fs;

    fn clock() -> Clock {
        // 2024-02-29 13:45:07 UTC, shown one hour ahead.
        Clock {
            now: 1_709_214_307,
            to_local: |t| Ok(t + 3600),
        }
    }

    fn opts(ops: Vec<RenameOp>) -> RenameOptions {
        RenameOptions {
            ops,
            ..RenameOptions::default()
        }
    }

    fn new_names(paths: &[PathBuf], options: &RenameOptions) -> Vec<(String, Status)> {
        preview(paths, options, &clock())
            .unwrap()
            .into_iter()
            .map(|p| (p.new_name, p.status))
            .collect()
    }

    struct Dir(tempfile::TempDir);

    impl Dir {
        fn new(files: &[&str]) -> Dir {
            let d = tempfile::tempdir().unwrap();
            for f in files {
                let p = d.path().join(f);
                if let Some(stripped) = f.strip_suffix('/') {
                    fs::create_dir_all(d.path().join(stripped)).unwrap();
                } else {
                    fs::create_dir_all(p.parent().unwrap()).unwrap();
                    fs::write(&p, f.as_bytes()).unwrap();
                }
            }
            Dir(d)
        }

        fn p(&self, name: &str) -> PathBuf {
            self.0.path().join(name)
        }

        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(self.0.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    fn replace(find: &str, with: &str) -> RenameOp {
        RenameOp::Replace {
            find: find.into(),
            with: with.into(),
            regex: false,
            case_insensitive: false,
            first_only: false,
        }
    }

    #[test]
    fn literal_and_regex_replace() {
        let d = Dir::new(&["a.b.txt", "IMG_001.JPG"]);
        let paths = [d.p("a.b.txt"), d.p("IMG_001.JPG")];
        let got = new_names(&paths, &opts(vec![replace(".", "$1")]));
        assert_eq!(
            got[0],
            ("a$1b.txt".into(), Status::Rename),
            "literal text is not expanded"
        );
        let re = RenameOp::Replace {
            find: r"img_(\d+)".into(),
            with: "photo-${1}".into(),
            regex: true,
            case_insensitive: true,
            first_only: false,
        };
        assert_eq!(new_names(&paths, &opts(vec![re]))[1].0, "photo-001.JPG");
        let bad = RenameOp::Replace {
            find: "(".into(),
            with: String::new(),
            regex: true,
            case_insensitive: false,
            first_only: false,
        };
        assert!(preview(&paths, &opts(vec![bad]), &clock()).is_err());
        assert!(preview(&paths, &opts(vec![replace("", "x")]), &clock()).is_err());
    }

    #[test]
    fn targets_and_extension() {
        let d = Dir::new(&["Report.final.TXT", ".gitignore", "folder.v1/"]);
        let paths = [d.p("Report.final.TXT"), d.p(".gitignore"), d.p("folder.v1")];
        let lower = vec![RenameOp::Case { mode: CaseMode::Lower }];
        let got = new_names(&paths, &opts(lower.clone()));
        assert_eq!(got[0].0, "report.final.TXT");
        assert_eq!(got[1].0, ".gitignore");
        let ext = RenameOptions {
            target: Target::Extension,
            ..opts(lower)
        };
        let got = new_names(&paths, &ext);
        assert_eq!(got[0].0, "Report.final.txt");
        assert_eq!(
            got[2],
            ("folder.v1".into(), Status::Unchanged),
            "folders have no extension"
        );
        let full = RenameOptions {
            target: Target::Full,
            ..opts(vec![replace(".", "_")])
        };
        assert_eq!(new_names(&paths, &full)[0].0, "Report_final_TXT");
        let set_ext = opts(vec![RenameOp::Extension { new: ".md".into() }]);
        let got = new_names(&paths, &set_ext);
        assert_eq!(
            (got[0].0.as_str(), got[1].0.as_str(), got[2].0.as_str()),
            ("Report.final.md", ".gitignore.md", "folder.v1")
        );
    }

    #[test]
    fn case_modes() {
        assert_eq!(
            change_case("hello WORLD-foo bar's", CaseMode::Title),
            "Hello World-Foo Bar's"
        );
        assert_eq!(change_case("  hELLO World", CaseMode::Sentence), "  Hello world");
        assert_eq!(change_case("straße", CaseMode::Upper), "STRASSE");
    }

    #[test]
    fn numbering_counts_only_included_items() {
        let d = Dir::new(&["a.jpg", "b.txt", "c.jpg"]);
        let paths = [d.p("a.jpg"), d.p("b.txt"), d.p("c.jpg")];
        let options = RenameOptions {
            include: "*.JPG".into(),
            ..opts(vec![RenameOp::Number {
                start: 9,
                step: 1,
                padding: 3,
                position: Position::Start,
                separator: " - ".into(),
            }])
        };
        let got = new_names(&paths, &options);
        assert_eq!(got[0].0, "009 - a.jpg");
        assert_eq!(got[1], ("b.txt".into(), Status::Skipped));
        assert_eq!(got[2].0, "010 - c.jpg");
    }

    #[test]
    fn templates_and_dates() {
        let d = Dir::new(&["trip/beach.png"]);
        let paths = [d.p("trip/beach.png")];
        let t = |pattern: &str| {
            opts(vec![RenameOp::Template {
                pattern: pattern.into(),
            }])
        };
        assert_eq!(
            new_names(&paths, &t("${parent}_${n:2}_${name}$$")),
            [("trip_01_beach$.png".into(), Status::Rename)]
        );
        assert_eq!(new_names(&paths, &t("${size}"))[0].0, "14.png");
        assert!(preview(&paths, &t("${bogus}"), &clock()).is_err());
        assert!(preview(&paths, &t("${name"), &clock()).is_err());
        assert!(preview(&paths, &t("${n:0}"), &clock()).is_err());
        let now = opts(vec![RenameOp::Date {
            source: DateSource::Now,
            format: "%Y%m%d-%H%M".into(),
            position: Position::Start,
            separator: "_".into(),
        }]);
        assert_eq!(
            new_names(&paths, &now)[0].0,
            "20240229-1445_beach.png",
            "local time is used"
        );
    }

    #[test]
    fn exif_date_with_fallback() {
        let d = Dir::new(&[]);
        let photo = d.p("photo.jpg");
        fs::write(&photo, crate::exif::tests::jpeg_with_date("2019:12:31 23:59:58")).unwrap();
        let plain = d.p("plain.jpg");
        fs::write(&plain, b"not a jpeg").unwrap();
        let options = opts(vec![RenameOp::Template {
            pattern: "${taken:%Y-%m-%d}".into(),
        }]);
        let got = preview(&[photo, plain], &options, &clock()).unwrap();
        assert_eq!(got[0].new_name, "2019-12-31.jpg");
        assert_eq!(got[1].status, Status::Rename, "falls back to the modified time");
    }

    #[test]
    fn invalid_names_are_reported() {
        let d = Dir::new(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        let set = |n: &str| opts(vec![RenameOp::Template { pattern: n.into() }]);
        for bad in ["con", "x:y", "trail.", "tab\t", "lpt1 "] {
            let got = preview(
                &[d.p("a.txt")],
                &RenameOptions {
                    target: Target::Full,
                    ..set(bad)
                },
                &clock(),
            )
            .unwrap();
            assert_eq!(got[0].status, Status::Invalid, "{bad:?}");
        }
        let empty = preview(
            &[d.p("a.txt")],
            &RenameOptions {
                target: Target::Full,
                ..set("")
            },
            &clock(),
        )
        .unwrap();
        assert_eq!(empty[0].status, Status::Invalid);
        assert!(validate_name(&"x".repeat(255)).is_ok());
        assert!(validate_name(&"x".repeat(256)).is_err());
        assert!(validate_name("console.txt").is_ok());
    }

    #[test]
    fn conflicts() {
        let d = Dir::new(&["a.txt", "b.txt", "c.txt", "keep.txt"]);
        let to = |name: &str| opts(vec![RenameOp::Template { pattern: name.into() }]);
        // Two items to the same name.
        let got = new_names(&[d.p("a.txt"), d.p("b.txt")], &to("same"));
        assert!(got.iter().all(|(_, s)| *s == Status::Conflict));
        // Onto an existing file that is not part of the batch.
        assert_eq!(new_names(&[d.p("a.txt")], &to("keep"))[0].1, Status::Conflict);
        // Onto a selected item that is skipped (so it stays put).
        let options = RenameOptions {
            exclude: "keep*".into(),
            ..to("keep")
        };
        let got = new_names(&[d.p("a.txt"), d.p("keep.txt")], &options);
        assert_eq!(got[0].1, Status::Conflict);
        // Case-only change of the same item is fine.
        let upper = opts(vec![RenameOp::Case { mode: CaseMode::Upper }]);
        assert_eq!(new_names(&[d.p("a.txt")], &upper)[0], ("A.txt".into(), Status::Rename));
        // Missing input.
        assert_eq!(new_names(&[d.p("nope.txt")], &upper)[0].1, Status::Missing);
    }

    #[test]
    fn swap_and_chain_apply_and_undo() {
        let d = Dir::new(&["a.txt", "b.txt"]);
        let paths = [d.p("a.txt"), d.p("b.txt")];
        let swap = opts(vec![RenameOp::Template { pattern: "x".into() }]);
        // Build a swap plan by hand: a->b and b->a, which a preview accepts since both move.
        let mut plan = preview(&paths, &swap, &clock()).unwrap();
        plan[0].new_name = "b.txt".into();
        plan[1].new_name = "a.txt".into();
        for p in &mut plan {
            p.status = Status::Rename;
        }
        mark_conflicts(&mut plan);
        assert!(plan.iter().all(|p| p.status == Status::Rename));
        let moves = apply(&plan).unwrap();
        assert_eq!(fs::read_to_string(d.p("a.txt")).unwrap(), "b.txt");
        assert_eq!(fs::read_to_string(d.p("b.txt")).unwrap(), "a.txt");
        undo(&moves).unwrap();
        assert_eq!(fs::read_to_string(d.p("a.txt")).unwrap(), "a.txt");
        assert_eq!(d.names(), ["a.txt", "b.txt"]);
    }

    #[test]
    fn folder_and_children_renamed_together() {
        let d = Dir::new(&["dir/inner.txt"]);
        let paths = [d.p("dir"), d.p("dir/inner.txt")];
        let options = opts(vec![RenameOp::Insert {
            text: "new_".into(),
            position: Position::Start,
        }]);
        let plan = preview(&paths, &options, &clock()).unwrap();
        let moves = apply(&plan).unwrap();
        assert!(d.p("new_dir/new_inner.txt").exists());
        undo(&moves).unwrap();
        assert!(d.p("dir/inner.txt").exists());
        assert_eq!(d.names(), ["dir"]);
    }

    #[test]
    fn failed_batch_is_rolled_back() {
        let d = Dir::new(&["a.txt", "b.txt"]);
        let options = opts(vec![RenameOp::Insert {
            text: "z".into(),
            position: Position::End,
        }]);
        let plan = preview(&[d.p("a.txt"), d.p("b.txt")], &options, &clock()).unwrap();
        // Something appears at a target after the preview.
        fs::write(d.p("bz.txt"), "intruder").unwrap();
        let err = apply(&plan).unwrap_err();
        assert!(format!("{err:#}").contains("Nothing was renamed"), "{err:#}");
        assert_eq!(d.names(), ["a.txt", "b.txt", "bz.txt"]);
        assert_eq!(fs::read_to_string(d.p("bz.txt")).unwrap(), "intruder");
    }

    #[test]
    fn small_ops() {
        let d = Dir::new(&["  my  file--name  .txt"]);
        let p = [d.p("  my  file--name  .txt")];
        let run = |ops| new_names(&p, &opts(ops))[0].0.clone();
        assert_eq!(run(vec![RenameOp::Trim { chars: String::new() }]), "my  file--name.txt");
        assert_eq!(
            run(vec![RenameOp::Dedupe { chars: "-".into() }]),
            "  my  file-name  .txt"
        );
        assert_eq!(
            run(vec![
                RenameOp::Dedupe { chars: String::new() },
                RenameOp::Trim { chars: String::new() }
            ]),
            "my file-name.txt"
        );
        assert_eq!(
            run(vec![RenameOp::ReplaceSpaces { with: "_".into() }]),
            "__my__file--name__.txt"
        );
        assert_eq!(
            run(vec![RenameOp::ReplaceRange {
                start: 2,
                length: 4,
                with: "X".into()
            }]),
            "  Xfile--name  .txt"
        );
        assert_eq!(
            run(vec![RenameOp::ReplaceRange {
                start: 100,
                length: 4,
                with: "!".into()
            }]),
            "  my  file--name  !.txt"
        );
        assert_eq!(
            run(vec![RenameOp::Remove {
                text: "FILE".into(),
                case_insensitive: true
            }]),
            "  my  --name  .txt"
        );
    }

    proptest! {
        #[test]
        fn templates_never_panic(pattern in ".{0,40}") {
            let _ = parse_template(&pattern);
        }

        #[test]
        fn validated_names_have_no_separators(name in ".{0,40}") {
            if validate_name(&name).is_ok() {
                prop_assert!(!name.contains('/') && !name.contains('\\') && !name.is_empty());
            }
        }

        #[test]
        fn split_join_roundtrip(name in "[a-z.]{1,12}", is_dir: bool) {
            let (stem, ext) = split(&name, is_dir);
            prop_assert_eq!(join(&stem, &ext), name);
        }
    }
}
