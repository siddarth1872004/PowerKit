//! The Screenshot manager: the pictures in the screenshot folder, newest first, to
//! open or move to the Recycle Bin.

use powerkit_core::TableEvent;
use powerkit_platform::shell;
use powerkit_platform::table_window::{TableRow, TableSpec, TableWindow};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const TABLE: &str = "screenshots";

static SPEC: TableSpec = TableSpec {
    id: TABLE,
    title: "Screenshots",
    columns: &[("Name", 300), ("Taken", 140), ("Size", 90)],
    fields: &[],
    checks: false,
    choices: &[],
    buttons: ["", "Open", "Delete", "Refresh", "Open folder"],
};

struct Shot {
    path: PathBuf,
    modified: i64,
    bytes: u64,
}

const PICTURES: [&str; 4] = ["png", "jpg", "jpeg", "bmp"];

fn list(dir: &Path) -> std::io::Result<Vec<Shot>> {
    let mut shots = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if !PICTURES.contains(&ext.as_str()) {
            continue;
        }
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        shots.push(Shot {
            path,
            modified,
            bytes: meta.len(),
        });
    }
    shots.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.path.cmp(&a.path)));
    Ok(shots)
}

/// Seconds since 1970 as `YYYY-MM-DD HH:MM`.
fn date_time(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days to a civil date, from Howard Hinnant's chrono-compatible algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60
    )
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1 << 20)),
        b => format!("{} KB", b.div_ceil(1024)),
    }
}

#[derive(Default)]
pub struct ScreenshotsUi {
    window: Option<TableWindow>,
    folder: PathBuf,
    shown: Vec<PathBuf>,
}

impl ScreenshotsUi {
    pub fn open(&mut self, folder: PathBuf) -> Result<(), String> {
        if self.window.is_none() {
            let w = TableWindow::new(&SPEC).map_err(|e| format!("could not open the screenshots: {e}"))?;
            self.window = Some(w);
            if let Some(w) = self.window.as_mut() {
                crate::icons::apply(screenshot::ID, |ico| w.set_icon(ico));
            }
        }
        self.folder = folder;
        self.refresh();
        if let Some(w) = self.window.as_mut() {
            w.show();
        }
        Ok(())
    }

    fn refresh(&mut self) {
        let Some(w) = self.window.as_mut() else { return };
        let shots = match list(&self.folder) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                w.set_status(&format!("Could not read {}: {e}", self.folder.display()));
                return;
            }
        };
        let rows: Vec<TableRow> = shots
            .iter()
            .map(|s| {
                let name = s.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let local = powerkit_platform::time::to_local(s.modified).unwrap_or(s.modified);
                (false, vec![name, date_time(local), size(s.bytes)])
            })
            .collect();
        w.set_rows(&rows, None);
        w.set_status(&match shots.len() {
            0 => format!("No screenshots in {} yet.", self.folder.display()),
            n => format!("{n} in {}", self.folder.display()),
        });
        self.shown = shots.into_iter().map(|s| s.path).collect();
    }

    pub fn on_event(&mut self, event: TableEvent) {
        let result = match event {
            TableEvent::Reload => {
                self.refresh();
                return;
            }
            TableEvent::Update { row, .. } => match self.shown.get(row) {
                Some(p) => shell::open(&p.to_string_lossy(), None).map_err(|e| format!("Could not open it: {e}")),
                None => return,
            },
            TableEvent::Delete(row) => match self.shown.get(row) {
                Some(p) => shell::recycle(p).map_err(|e| format!("Could not delete it: {e}")),
                None => return,
            },
            TableEvent::Save => std::fs::create_dir_all(&self.folder)
                .map_err(|e| e.to_string())
                .and_then(|()| shell::open(&self.folder.to_string_lossy(), None).map_err(|e| e.to_string()))
                .map_err(|e| format!("Could not open the folder: {e}")),
            _ => return,
        };
        match result {
            Ok(()) => self.refresh(),
            Err(e) => {
                if let Some(w) = self.window.as_mut() {
                    w.set_status(&e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_pictures_first_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "b.PNG", "notes.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let old = std::fs::File::options()
            .write(true)
            .open(dir.path().join("a.png"))
            .unwrap();
        old.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_000_000))
            .unwrap();
        let names: Vec<_> = list(dir.path())
            .unwrap()
            .iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["b.PNG", "a.png"]);
    }

    #[test]
    fn dates_and_sizes_read_plainly() {
        assert_eq!(date_time(0), "1970-01-01 00:00");
        assert_eq!(date_time(1_795_000_000), "2026-11-18 11:06");
        assert_eq!(size(1), "1 KB");
        assert_eq!(size(3 << 20), "3.0 MB");
    }
}
