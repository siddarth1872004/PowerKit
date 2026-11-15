//! On-disk storage: atomic writes, backup, corruption recovery, import/export.

use crate::schema::Config;
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid JSON: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("file is too large ({0} bytes)")]
    TooLarge(u64),
}

fn io<'a>(action: &'static str, path: &'a Path) -> impl FnOnce(std::io::Error) -> StoreError + 'a {
    move |source| StoreError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

/// Largest configuration/import file accepted.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Locations of PowerKit's per-user data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve the data directory:
    /// 1. `POWERKIT_DATA_DIR` (tests, custom deployments);
    /// 2. `<exe dir>\data` when a `portable` marker file sits next to the executable;
    /// 3. `%APPDATA%\PowerKit`.
    pub fn resolve() -> Paths {
        if let Some(dir) = std::env::var_os("POWERKIT_DATA_DIR").filter(|d| !d.is_empty()) {
            return Paths::new(dir);
        }
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            if dir.join("portable").exists() {
                return Paths::new(dir.join("data"));
            }
        }
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Paths::new(base.join("PowerKit"))
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn backup(&self) -> PathBuf {
        self.root.join("config.backup.json")
    }

    pub fn state(&self, name: &str) -> PathBuf {
        self.root.join("state").join(format!("{name}.json"))
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
}

static TMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Write `bytes` to `path` so that readers observe either the old or the new content,
/// never a partial file: write a sibling temp file, flush it to disk, then rename over
/// the destination (an atomic replace on NTFS and POSIX filesystems).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(io("create directory", dir))?;
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = fs::File::create(&tmp).map_err(io("create", &tmp))?;
        f.write_all(bytes).map_err(io("write", &tmp))?;
        f.sync_all().map_err(io("flush", &tmp))?;
        drop(f);
        fs::rename(&tmp, path).map_err(io("replace", path))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp); // best effort: the original error is what matters
    }
    result
}

fn read_limited(path: &Path) -> Result<Vec<u8>, StoreError> {
    let len = fs::metadata(path).map_err(io("inspect", path))?.len();
    if len > MAX_FILE_BYTES {
        return Err(StoreError::TooLarge(len));
    }
    fs::read(path).map_err(io("read", path))
}

fn read_json(path: &Path) -> Result<serde_json::Value, StoreError> {
    let bytes = read_limited(path)?;
    // Tolerate a UTF-8 byte order mark left by some editors.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    serde_json::from_slice(bytes).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadSource {
    /// No configuration existed; defaults were used.
    Defaults,
    /// `config.json` loaded normally.
    File,
    /// `config.json` was unreadable; the last good backup was used.
    Backup { quarantined: Option<PathBuf> },
    /// Both files were unusable; defaults were used.
    Reset { quarantined: Option<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadReport {
    pub source: LoadSource,
    pub warnings: Vec<String>,
}

/// Owns the configuration file. A damaged file can never prevent startup: loading falls
/// back to the backup and then to defaults, keeping the damaged file for inspection.
#[derive(Debug)]
pub struct ConfigStore {
    paths: Paths,
}

impl ConfigStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn load(&self) -> (Config, LoadReport) {
        let path = self.paths.config();
        if !path.exists() && !self.paths.backup().exists() {
            return (
                Config::new(),
                LoadReport {
                    source: LoadSource::Defaults,
                    warnings: Vec::new(),
                },
            );
        }
        let primary_error = match read_json(&path) {
            Ok(value) => {
                let (config, warnings) = Config::from_value(value);
                return (
                    config,
                    LoadReport {
                        source: LoadSource::File,
                        warnings,
                    },
                );
            }
            Err(e) => e,
        };
        let mut warnings = vec![format!("configuration could not be loaded: {primary_error}")];
        let quarantined = self.quarantine(&path, &mut warnings);
        match read_json(&self.paths.backup()) {
            Ok(value) => {
                let (config, more) = Config::from_value(value);
                warnings.push("restored settings from the last good backup".into());
                warnings.extend(more);
                (
                    config,
                    LoadReport {
                        source: LoadSource::Backup { quarantined },
                        warnings,
                    },
                )
            }
            Err(e) => {
                warnings.push(format!(
                    "backup could not be loaded either ({e}); using default settings"
                ));
                (
                    Config::new(),
                    LoadReport {
                        source: LoadSource::Reset { quarantined },
                        warnings,
                    },
                )
            }
        }
    }

    /// Move a damaged file aside so it is neither lost nor read again.
    fn quarantine(&self, path: &Path, warnings: &mut Vec<String>) -> Option<PathBuf> {
        if !path.exists() {
            return None;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let target = self.paths.root.join(format!("config.corrupt-{stamp}.json"));
        match fs::rename(path, &target) {
            Ok(()) => {
                warnings.push(format!("the damaged file was kept as {}", target.display()));
                Some(target)
            }
            Err(e) => {
                warnings.push(format!("could not move the damaged file aside: {e}"));
                None
            }
        }
    }

    /// Persist the configuration atomically, keeping the previous good file as backup.
    pub fn save(&self, config: &Config) -> Result<(), StoreError> {
        let path = self.paths.config();
        let bytes = serde_json::to_vec_pretty(config).expect("Config serialization cannot fail");
        if path.exists() && read_json(&path).is_ok() {
            let backup = self.paths.backup();
            if let Err(e) = fs::copy(&path, &backup) {
                tracing::warn!(error = %e, "could not refresh configuration backup");
            }
        }
        write_atomic(&path, &bytes)
    }

    pub fn export(&self, config: &Config, to: &Path) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec_pretty(config).expect("Config serialization cannot fail");
        write_atomic(to, &bytes)
    }

    /// Read a configuration exported earlier. The result is sanitized but not saved.
    pub fn import(&self, from: &Path) -> Result<(Config, Vec<String>), StoreError> {
        Ok(Config::from_value(read_json(from)?))
    }
}

/// A small JSON document for non-settings data (history, journals). Loading never fails:
/// a missing or damaged file yields `T::default()`.
#[derive(Debug, Clone)]
pub struct StateFile {
    path: PathBuf,
}

impl StateFile {
    pub fn new(paths: &Paths, name: &str) -> Self {
        Self {
            path: paths.state(name),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load<T: DeserializeOwned + Default>(&self) -> T {
        if !self.path.exists() {
            return T::default();
        }
        match read_json(&self.path).and_then(|v| {
            serde_json::from_value(v).map_err(|e| StoreError::Parse {
                path: self.path.clone(),
                message: e.to_string(),
            })
        }) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "state file unusable; starting empty");
                T::default()
            }
        }
    }

    pub fn save<T: Serialize>(&self, value: &T) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(value).expect("state serialization cannot fail");
        write_atomic(&self.path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Theme;

    fn store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(Paths::new(dir.path().join("PowerKit")));
        (dir, store)
    }

    #[test]
    fn first_run_uses_defaults() {
        let (_d, s) = store();
        let (c, r) = s.load();
        assert_eq!(c, Config::new());
        assert_eq!(r.source, LoadSource::Defaults);
    }

    #[test]
    fn save_and_reload() {
        let (_d, s) = store();
        let mut c = Config::new();
        c.general.theme = Theme::Dark;
        c.always_on_top.border_thickness = 7;
        s.save(&c).unwrap();
        let (loaded, r) = s.load();
        assert_eq!(loaded, c);
        assert_eq!(
            r,
            LoadReport {
                source: LoadSource::File,
                warnings: vec![]
            }
        );
        // No temp files are left behind.
        let leftovers: Vec<_> = fs::read_dir(&s.paths().root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn corrupt_file_falls_back_to_backup() {
        let (_d, s) = store();
        let mut c = Config::new();
        c.general.theme = Theme::Light;
        s.save(&c).unwrap();
        c.general.theme = Theme::Dark;
        s.save(&c).unwrap(); // backup now holds the Light version
        fs::write(s.paths().config(), b"{ \"general\": ").unwrap();
        let (loaded, r) = s.load();
        assert_eq!(loaded.general.theme, Theme::Light);
        let LoadSource::Backup { quarantined: Some(q) } = r.source else {
            panic!("{r:?}")
        };
        assert!(q.exists());
        assert!(!s.paths().config().exists());
        // Startup after recovery works and saving restores a normal state.
        s.save(&loaded).unwrap();
        assert_eq!(s.load().1.source, LoadSource::File);
    }

    #[test]
    fn corrupt_file_without_backup_resets() {
        let (_d, s) = store();
        fs::create_dir_all(&s.paths().root).unwrap();
        fs::write(s.paths().config(), [0xFFu8, 0x00, 0x12]).unwrap();
        let (loaded, r) = s.load();
        assert_eq!(loaded, Config::new());
        assert!(matches!(r.source, LoadSource::Reset { quarantined: Some(_) }));
        assert!(!r.warnings.is_empty());
    }

    #[test]
    fn corrupt_save_does_not_poison_backup() {
        let (_d, s) = store();
        let c = Config::new();
        s.save(&c).unwrap();
        s.save(&c).unwrap();
        fs::write(s.paths().config(), b"garbage").unwrap();
        s.save(&c).unwrap(); // must not copy garbage over the good backup
        let backup: serde_json::Value = serde_json::from_slice(&fs::read(s.paths().backup()).unwrap()).unwrap();
        assert!(backup.is_object());
    }

    #[test]
    fn bom_is_tolerated() {
        let (_d, s) = store();
        fs::create_dir_all(&s.paths().root).unwrap();
        fs::write(s.paths().config(), b"\xEF\xBB\xBF{\"general\":{\"theme\":\"dark\"}}").unwrap();
        let (c, r) = s.load();
        assert_eq!(c.general.theme, Theme::Dark);
        assert_eq!(r.source, LoadSource::File);
    }

    #[test]
    fn export_import_round_trip() {
        let (d, s) = store();
        let mut c = Config::new();
        c.launcher.max_results = 12;
        let file = d.path().join("export.json");
        s.export(&c, &file).unwrap();
        let (back, w) = s.import(&file).unwrap();
        assert_eq!(back, c);
        assert!(w.is_empty());
        assert!(s.import(&d.path().join("missing.json")).is_err());
        fs::write(&file, b"nope").unwrap();
        assert!(matches!(s.import(&file), Err(StoreError::Parse { .. })));
    }

    #[test]
    fn state_file_recovers() {
        let (_d, s) = store();
        let f = StateFile::new(s.paths(), "history");
        assert_eq!(f.load::<Vec<u32>>(), Vec::<u32>::new());
        f.save(&vec![1u32, 2, 3]).unwrap();
        assert_eq!(f.load::<Vec<u32>>(), vec![1, 2, 3]);
        fs::write(f.path(), b"[1, 2").unwrap();
        assert_eq!(f.load::<Vec<u32>>(), Vec::<u32>::new());
    }
}
