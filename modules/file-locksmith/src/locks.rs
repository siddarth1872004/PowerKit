//! Turning an Explorer selection into the files to check, and the per-item answers
//! into one list of processes.

use std::path::{Path, PathBuf};

/// One selected file or folder and the files to check for it.
#[derive(Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Expanded {
    pub items: Vec<Item>,
    /// Some files were left unchecked because there were more than the limit.
    pub truncated: bool,
    /// Folders that could not be listed, with the reason.
    pub unreadable: Vec<(PathBuf, String)>,
}

/// The files to check for each selected path: a file is itself, a folder is every
/// file inside it at any depth. Links to other folders are not followed, so a
/// junction cannot make the walk endless. At most `limit` files are taken in all.
pub fn expand(selected: &[PathBuf], limit: usize) -> Expanded {
    let mut out = Expanded::default();
    let mut taken = 0;
    for path in selected {
        let mut files = Vec::new();
        let mut folders = Vec::new();
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => folders.push(path.clone()),
            Ok(_) => files.push(path.clone()),
            Err(e) => out.unreadable.push((path.clone(), e.to_string())),
        }
        taken += files.len();
        while let Some(folder) = folders.pop() {
            let entries = match std::fs::read_dir(&folder) {
                Ok(entries) => entries,
                Err(e) => {
                    out.unreadable.push((folder, e.to_string()));
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        out.unreadable.push((folder.clone(), e.to_string()));
                        break;
                    }
                };
                match entry.file_type() {
                    Ok(t) if t.is_dir() => folders.push(entry.path()),
                    Ok(t) if t.is_symlink() => {}
                    _ if taken >= limit => out.truncated = true,
                    _ => {
                        files.push(entry.path());
                        taken += 1;
                    }
                }
            }
        }
        out.items.push(Item {
            path: path.clone(),
            files,
        });
    }
    out
}

/// A process and which selected items (by index) it is using.
#[derive(Debug, PartialEq, Eq)]
pub struct Found<P> {
    pub process: P,
    pub items: Vec<usize>,
}

/// One entry per process from the processes found for each item, in the order they
/// were first found. `same` tells whether two answers are the same process.
pub fn merge<P>(per_item: Vec<Vec<P>>, same: impl Fn(&P, &P) -> bool) -> Vec<Found<P>> {
    let mut out: Vec<Found<P>> = Vec::new();
    for (item, processes) in per_item.into_iter().enumerate() {
        for p in processes {
            match out.iter_mut().find(|f| same(&f.process, &p)) {
                Some(f) if f.items.last() != Some(&item) => f.items.push(item),
                Some(_) => {}
                None => out.push(Found {
                    process: p,
                    items: vec![item],
                }),
            }
        }
    }
    out
}

/// A short name for a selected item: its file or folder name.
pub fn label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_open_up_into_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        for f in ["a/1.txt", "a/b/2.txt", "3.txt"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let e = expand(&[d.join("a"), d.join("3.txt"), d.join("gone")], 100);
        let mut a = e.items[0].files.clone();
        a.sort();
        assert_eq!(a, [d.join("a").join("1.txt"), d.join("a").join("b").join("2.txt")]);
        assert_eq!(e.items[1].files, [d.join("3.txt")]);
        assert!(e.items[2].files.is_empty());
        assert_eq!(e.unreadable.len(), 1, "a missing path is reported");
        assert!(!e.truncated);

        let e = expand(&[d.join("3.txt"), d.join("a")], 2);
        assert_eq!(e.items.iter().map(|i| i.files.len()).sum::<usize>(), 2);
        assert!(e.truncated);
    }

    #[test]
    fn a_process_found_for_several_items_is_listed_once() {
        let found = merge(vec![vec![1, 2], vec![2, 2], vec![3, 1]], |a, b| a == b);
        let pairs: Vec<_> = found.iter().map(|f| (f.process, f.items.clone())).collect();
        assert_eq!(pairs, [(1, vec![0, 2]), (2, vec![0, 1]), (3, vec![2])]);
        assert!(merge(Vec::<Vec<u8>>::new(), |a, b| a == b).is_empty());
    }

    #[test]
    fn labels_are_names() {
        assert_eq!(label(Path::new("/x/report.docx")), "report.docx");
        assert_eq!(label(Path::new("/")), "/");
    }
}
