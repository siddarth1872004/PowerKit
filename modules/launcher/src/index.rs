//! The searchable lists of installed applications and files, built on a background
//! thread because walking folders can take seconds.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Name matched against the query (app name, or file name with extension).
    pub name: String,
    pub path: String,
    pub dir: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Index {
    pub apps: Vec<Entry>,
    pub files: Vec<Entry>,
    /// Folders that could not be read, for diagnostics.
    pub errors: Vec<String>,
}

const APP_EXTENSIONS: [&str; 4] = ["lnk", "url", "exe", "appref-ms"];
/// Start Menu entries that are noise in a launcher.
const APP_NOISE: [&str; 4] = ["uninstall", "readme", "help", "documentation"];

/// Start Menu folders of the current user and of all users.
pub fn default_app_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for (var, rel) in [
        ("APPDATA", r"Microsoft\Windows\Start Menu\Programs"),
        ("ProgramData", r"Microsoft\Windows\Start Menu\Programs"),
    ] {
        if let Some(base) = std::env::var_os(var) {
            roots.push(Path::new(&base).join(rel));
        }
    }
    roots
}

/// The configured roots, or Desktop, Documents and Downloads when none are set.
pub fn file_roots(configured: &[String]) -> Vec<PathBuf> {
    if !configured.is_empty() {
        return configured.iter().map(PathBuf::from).collect();
    }
    let Some(home) = std::env::var_os("USERPROFILE") else {
        return Vec::new();
    };
    ["Desktop", "Documents", "Downloads"]
        .iter()
        .map(|d| Path::new(&home).join(d))
        .collect()
}

fn hidden(name: &str) -> bool {
    name.starts_with('.') || name.eq_ignore_ascii_case("node_modules") || name.eq_ignore_ascii_case("$recycle.bin")
}

/// Breadth-first walk so shallow (more likely relevant) entries are kept when `limit`
/// cuts the index short. Symbolic links are not followed, which also rules out cycles.
fn walk(roots: &[PathBuf], limit: usize, errors: &mut Vec<String>, mut visit: impl FnMut(&Path, &str, bool) -> bool) {
    let mut queue: VecDeque<PathBuf> = roots.iter().cloned().collect();
    let mut count = 0;
    while let Some(dir) = queue.pop_front() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if hidden(&name) {
                continue;
            }
            let path = entry.path();
            if visit(&path, &name, ft.is_dir()) {
                count += 1;
                if count >= limit {
                    return;
                }
            }
            if ft.is_dir() {
                queue.push_back(path);
            }
        }
    }
}

pub fn index_apps(roots: &[PathBuf], errors: &mut Vec<String>) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    walk(roots, 20_000, errors, |path, _, dir| {
        if dir {
            return false;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !APP_EXTENSIONS.contains(&ext.as_str()) {
            return false;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let lower = stem.to_lowercase();
        if APP_NOISE.iter().any(|n| lower.contains(n)) || !seen.insert(lower) {
            return false;
        }
        apps.push(Entry {
            name: stem.to_string(),
            path: path.to_string_lossy().into_owned(),
            dir: false,
        });
        true
    });
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

pub fn index_files(roots: &[PathBuf], limit: usize, errors: &mut Vec<String>) -> Vec<Entry> {
    let mut files = Vec::new();
    walk(roots, limit, errors, |path, name, dir| {
        files.push(Entry {
            name: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            dir,
        });
        true
    });
    files
}

pub fn build(app_roots: &[PathBuf], file_roots: &[PathBuf], max_files: usize) -> Index {
    let mut errors = Vec::new();
    let apps = index_apps(app_roots, &mut errors);
    let files = if max_files == 0 {
        Vec::new()
    } else {
        index_files(file_roots, max_files, &mut errors)
    };
    Index { apps, files, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"").unwrap();
    }

    #[test]
    fn apps_are_deduplicated_filtered_and_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let (user, all) = (dir.path().join("user"), dir.path().join("all"));
        touch(&user.join("Paint.lnk"));
        touch(&user.join("Tools/Zed.lnk"));
        touch(&user.join("Tools/Uninstall Paint.lnk"));
        touch(&user.join("notes.txt"));
        touch(&all.join("paint.lnk"));
        touch(&all.join("Site.url"));
        let idx = build(&[user, all, dir.path().join("missing")], &[], 0);
        let names: Vec<&str> = idx.apps.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Paint", "Site", "Zed"]);
        // The first folder wins duplicates.
        assert!(idx.apps[0].path.contains("user"));
        assert_eq!(idx.errors.len(), 1, "{:?}", idx.errors);
    }

    #[test]
    fn files_are_breadth_first_bounded_and_skip_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        touch(&root.join("a/b/c/deep.txt"));
        touch(&root.join("top.txt"));
        touch(&root.join(".git/config"));
        touch(&root.join("node_modules/x/index.js"));
        let mut errors = Vec::new();
        let all = index_files(std::slice::from_ref(&root), 1000, &mut errors);
        let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"deep.txt") && names.contains(&"top.txt"));
        assert!(!names.iter().any(|n| *n == "config" || *n == "index.js"));
        assert!(all.iter().any(|e| e.name == "a" && e.dir));

        let few = index_files(&[root], 2, &mut errors);
        assert_eq!(few.len(), 2);
        assert!(few.iter().all(|e| e.name != "deep.txt"));
    }

    #[test]
    fn configured_roots_override_defaults() {
        assert_eq!(file_roots(&["C:\\x".into()]), vec![PathBuf::from("C:\\x")]);
    }
}
