//! Structured logging to `<data dir>\logs\<name>.log`.
//!
//! * The file is size-bounded: when it exceeds [`MAX_LOG_BYTES`] it is rotated to
//!   `<name>.old.log` (one generation), so logs can never fill the disk.
//! * The level can be changed at runtime from the settings UI.
//! * Policy: log *what happened*, never user content. No typed keys, clipboard text,
//!   file contents or search queries are logged anywhere in PowerKit.

use powerkit_config::LogLevel;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload;

pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

struct Inner {
    path: PathBuf,
    file: Option<File>,
    written: u64,
    limit: u64,
}

impl Inner {
    fn open(&mut self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.written = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file = None; // close before renaming (required on Windows)
        let old = self.path.with_extension("old.log");
        let _ = fs::remove_file(&old);
        fs::rename(&self.path, &old)?;
        self.open()
    }
}

/// A cloneable, thread-safe, size-bounded log file.
#[derive(Clone)]
pub struct RotatingFile(Arc<Mutex<Inner>>);

impl RotatingFile {
    pub fn new(path: impl Into<PathBuf>, limit: u64) -> io::Result<Self> {
        let mut inner = Inner {
            path: path.into(),
            file: None,
            written: 0,
            limit,
        };
        inner.open()?;
        Ok(Self(Arc::new(Mutex::new(inner))))
    }

    pub fn path(&self) -> PathBuf {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).path.clone()
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner.written + buf.len() as u64 > inner.limit && inner.written > 0 {
            inner.rotate()?;
        }
        match inner.file.as_mut() {
            Some(f) => {
                f.write_all(buf)?;
                inner.written += buf.len() as u64;
                Ok(buf.len())
            }
            None => Err(io::Error::other("log file is closed")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0.lock().unwrap_or_else(|p| p.into_inner()).file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for RotatingFile {
    type Writer = RotatingFile;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
    }
}

/// Handle for changing the log level after initialization.
#[derive(Clone)]
pub struct LogHandle {
    reload: reload::Handle<LevelFilter, tracing_subscriber::Registry>,
    path: PathBuf,
}

impl LogHandle {
    pub fn set_level(&self, level: LogLevel) {
        // Fails only if the subscriber was dropped, in which case nothing logs anyway.
        let _ = self.reload.modify(|f| *f = filter(level));
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Install the global logger writing to `<logs_dir>\<name>.log`. Call once per process.
/// On failure (for example a read-only data directory) the process keeps running
/// without file logging and the error is returned for display.
pub fn init(logs_dir: &Path, name: &str, level: LogLevel) -> io::Result<LogHandle> {
    let file = RotatingFile::new(logs_dir.join(format!("{name}.log")), MAX_LOG_BYTES)?;
    let path = file.path();
    let (level_layer, reload) = reload::Layer::new(filter(level));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(file)
        .with_ansi(false)
        .with_target(true);
    tracing_subscriber::registry()
        .with(level_layer)
        .with(fmt)
        .try_init()
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(LogHandle { reload, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_bounds_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("t.log");
        let mut f = RotatingFile::new(&path, 1000).unwrap();
        for i in 0..100 {
            writeln!(f, "line {i:04} {}", "x".repeat(40)).unwrap();
        }
        f.flush().unwrap();
        let cur = fs::metadata(&path).unwrap().len();
        let old = fs::metadata(path.with_extension("old.log")).unwrap().len();
        assert!(cur <= 1000 && old <= 1000, "{cur} {old}");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("line 0099"));
    }

    #[test]
    fn appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        fs::write(&path, "first\n").unwrap();
        let mut f = RotatingFile::new(&path, 1000).unwrap();
        writeln!(f, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
    }
}
