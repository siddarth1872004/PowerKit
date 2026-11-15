//! The templates folder: what it offers, the names shown for it, and making a copy
//! of a template under the name it should get.

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

/// Characters Windows does not allow in a file name.
const INVALID: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Files Explorer keeps in folders for itself.
const IGNORED: &[&str] = &["desktop.ini", "thumbs.db"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub hide_extension: bool,
    pub hide_starting_digits: bool,
    pub replace_variables: bool,
}

impl From<&powerkit_config::NewPlus> for Options {
    fn from(s: &powerkit_config::NewPlus) -> Options {
        Options {
            hide_extension: s.hide_extension,
            hide_starting_digits: s.hide_starting_digits,
            replace_variables: s.replace_variables,
        }
    }
}

/// Local date and time, for the date variables.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    /// 0 is Sunday.
    pub weekday: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub path: PathBuf,
    pub is_dir: bool,
}

impl Template {
    fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    /// What the menu shows.
    pub fn label(&self, opts: Options) -> String {
        let name = self.name();
        let name = if opts.hide_starting_digits {
            strip_digits(&name)
        } else {
            &name
        };
        if opts.hide_extension && !self.is_dir {
            split_extension(name).0.to_string()
        } else {
            name.to_string()
        }
    }

    /// The name a new copy gets in `folder`, before making it unique.
    pub fn new_name(&self, opts: Options, folder: &Path, now: Stamp, env: &dyn Fn(&str) -> Option<String>) -> String {
        let name = self.name();
        let base = if opts.hide_starting_digits {
            strip_digits(&name)
        } else {
            &name
        };
        if !opts.replace_variables {
            return base.to_string();
        }
        let parent = folder
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let replaced = valid_name(&replace(base, now, &parent, env));
        if replaced.is_empty() {
            base.to_string()
        } else {
            replaced
        }
    }
}

/// The templates folder: `setting` when set, otherwise `NewPlus\Templates` in `data_dir`.
pub fn folder(setting: &str, data_dir: &Path) -> PathBuf {
    if setting.is_empty() {
        data_dir.join("NewPlus").join("Templates")
    } else {
        PathBuf::from(setting)
    }
}

/// Create the templates folder with two examples if it does not exist yet, so the
/// menu shows how it works. An existing folder, even an empty one, is left alone.
pub fn seed(dir: &Path) -> io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir.join("1. Folder"))?;
    std::fs::write(dir.join("2. Text document.txt"), "")
}

/// The templates in `dir`, in the order Explorer would sort them.
pub fn list(dir: &Path) -> io::Result<Vec<Template>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if IGNORED.contains(&name.as_str()) || hidden(&meta) {
            continue;
        }
        out.push(Template {
            path: entry.path(),
            is_dir: meta.is_dir(),
        });
    }
    out.sort_by(|a, b| natural(&a.name(), &b.name()));
    Ok(out)
}

#[cfg(windows)]
fn hidden(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
    meta.file_attributes() & HIDDEN_OR_SYSTEM != 0
}

#[cfg(not(windows))]
fn hidden(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Copy `template` into `folder`; returns the path of the copy.
pub fn create(
    template: &Template,
    folder: &Path,
    opts: Options,
    now: Stamp,
    env: &dyn Fn(&str) -> Option<String>,
) -> io::Result<PathBuf> {
    let name = template.new_name(opts, folder, now, env);
    let target = unique(folder, &name, template.is_dir);
    if template.is_dir {
        copy_tree(&template.path, &target)?;
    } else {
        std::fs::copy(&template.path, &target)?;
    }
    Ok(target)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

/// `name` in `folder`, or `name (2)`, `name (3)`… (before the extension for files)
/// when that is taken.
pub fn unique(folder: &Path, name: &str, is_dir: bool) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = if is_dir { (name, "") } else { split_extension(name) };
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some number is free")
}

/// `("report", ".txt")`; a name that only starts with a dot has no extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// Without the leading number (and the spaces and dots after it) used to order
/// templates, unless nothing would be left. A name that does not start with a digit,
/// such as ".gitignore", is kept whole.
fn strip_digits(name: &str) -> &str {
    if !name.starts_with(|c: char| c.is_ascii_digit()) {
        return name;
    }
    let rest = name.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ' || c == '.');
    if rest.is_empty() {
        name
    } else {
        rest
    }
}

fn valid_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| !INVALID.contains(c) && !c.is_control())
        .collect();
    kept.trim_end_matches(['.', ' ']).trim_start().to_string()
}

fn replace(name: &str, now: Stamp, parent: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let month = MONTHS[usize::from(now.month.clamp(1, 12)) - 1];
    let day = DAYS[usize::from(now.weekday % 7)];
    // Longest first, so `$MMMM` is not read as `$MM` twice.
    let vars: [(&str, String); 20] = [
        ("$PARENT_FOLDER_NAME", parent.to_string()),
        ("$YYYY", format!("{:04}", now.year)),
        ("$MMMM", month.into()),
        ("$DDDD", day.into()),
        ("$fff", format!("{:03}", now.millis)),
        ("$MMM", month[..3].into()),
        ("$DDD", day[..3].into()),
        ("$YY", format!("{:02}", now.year % 100)),
        ("$MM", format!("{:02}", now.month)),
        ("$DD", format!("{:02}", now.day)),
        ("$hh", format!("{:02}", now.hour)),
        ("$mm", format!("{:02}", now.minute)),
        ("$ss", format!("{:02}", now.second)),
        ("$ff", format!("{:02}", now.millis / 10)),
        ("$M", now.month.to_string()),
        ("$D", now.day.to_string()),
        ("$h", now.hour.to_string()),
        ("$m", now.minute.to_string()),
        ("$s", now.second.to_string()),
        ("$f", (now.millis / 100).to_string()),
    ];
    let mut out = String::new();
    let mut rest = name;
    while let Some(c) = rest.chars().next() {
        if c == '$' {
            if let Some((token, value)) = vars.iter().find(|(t, _)| rest.starts_with(t)) {
                out.push_str(value);
                rest = &rest[token.len()..];
                continue;
            }
        } else if c == '%' {
            if let Some(end) = rest[1..].find('%') {
                let var = &rest[1..=end];
                if let Some(value) = (!var.is_empty()).then(|| env(var)).flatten() {
                    out.push_str(&value);
                    rest = &rest[end + 2..];
                    continue;
                }
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Case-insensitive, with runs of digits compared as numbers: "2" before "10".
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(d) = it.next_if(char::is_ascii_digit) {
                        n.push(d);
                    }
                    n
                };
                let (x, y) = (take(&mut a), take(&mut b));
                let (xt, yt) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = xt.len().cmp(&yt.len()).then_with(|| xt.cmp(yt));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Stamp = Stamp {
        year: 2026,
        month: 3,
        day: 7,
        weekday: 6,
        hour: 9,
        minute: 5,
        second: 4,
        millis: 321,
    };

    fn env(name: &str) -> Option<String> {
        (name == "USERNAME").then(|| "sid".into())
    }

    fn file(name: &str) -> Template {
        Template {
            path: PathBuf::from("t").join(name),
            is_dir: false,
        }
    }

    #[test]
    fn labels_follow_the_options() {
        let t = file("01. Report.docx");
        assert_eq!(t.label(Options::default()), "01. Report.docx");
        let hide = Options {
            hide_extension: true,
            hide_starting_digits: true,
            replace_variables: false,
        };
        assert_eq!(t.label(hide), "Report");
        assert_eq!(file("2024").label(hide), "2024", "never strip the whole name");
        assert_eq!(file(".gitignore").label(hide), ".gitignore");
        let dir = Template {
            path: PathBuf::from("v1.2 notes"),
            is_dir: true,
        };
        assert_eq!(dir.label(hide), "v1.2 notes", "folders have no extension");
    }

    #[test]
    fn variables_fill_in_the_new_name() {
        let opts = Options {
            hide_extension: true,
            hide_starting_digits: true,
            replace_variables: true,
        };
        let t = file("3 $YYYY-$MM-$DD $DDDD $MMM $h$mm$ss.$fff %USERNAME% %NOPE% $PARENT_FOLDER_NAME $X.md");
        assert_eq!(
            t.new_name(opts, &Path::new("work").join("notes"), NOW, &env),
            "2026-03-07 Saturday Mar 90504.321 sid %NOPE% notes $X.md"
        );
        let unsafe_value = |_: &str| Some("a:b\\c".to_string());
        assert_eq!(
            file("%P%.txt").new_name(opts, Path::new("x"), NOW, &unsafe_value),
            "abc.txt"
        );
        let off = Options {
            replace_variables: false,
            ..opts
        };
        assert_eq!(file("$YYYY.txt").new_name(off, Path::new("x"), NOW, &env), "$YYYY.txt");
    }

    #[test]
    fn copies_get_free_names() {
        let dir = tempfile::tempdir().unwrap();
        let templates = dir.path().join("templates");
        seed(&templates).unwrap();
        std::fs::create_dir(templates.join("10. Project")).unwrap();
        std::fs::write(templates.join("10. Project").join("README.md"), "hi").unwrap();
        std::fs::write(templates.join("desktop.ini"), "").unwrap();
        let found = list(&templates).unwrap();
        let labels: Vec<String> = found.iter().map(|t| t.label(Options::default())).collect();
        assert_eq!(labels, ["1. Folder", "2. Text document.txt", "10. Project"]);

        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let opts = Options {
            hide_starting_digits: true,
            ..Options::default()
        };
        let a = create(&found[1], &out, opts, NOW, &env).unwrap();
        let b = create(&found[1], &out, opts, NOW, &env).unwrap();
        assert_eq!(
            (a.file_name().unwrap(), b.file_name().unwrap()),
            ("Text document.txt".as_ref(), "Text document (2).txt".as_ref())
        );
        let p = create(&found[2], &out, opts, NOW, &env).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("README.md")).unwrap(), "hi");
        let p2 = create(&found[2], &out, opts, NOW, &env).unwrap();
        assert_eq!(p2.file_name().unwrap(), "Project (2)");

        std::fs::remove_dir_all(&templates).unwrap();
        std::fs::create_dir(&templates).unwrap();
        seed(&templates).unwrap();
        assert!(list(&templates).unwrap().is_empty(), "an emptied folder stays empty");
    }
}
