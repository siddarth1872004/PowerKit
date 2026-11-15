//! The File Locksmith window's contents. Looking for the processes that use the
//! files can take a while for big folders, so it runs on a worker thread and the
//! answer comes back by wake-up.

use file_locksmith::locks::{self, Found};
use powerkit_core::TableEvent;
use powerkit_platform::event_loop::Waker;
use powerkit_platform::process::{self, Locker};
use powerkit_platform::table_window::{TableRow, TableSpec, TableWindow};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub const TABLE: &str = "file_locksmith";
/// Files checked at most per look, so a huge folder cannot stall it for minutes.
const LIMIT: usize = 20_000;
/// Explorer starts PowerKit once per selected item; requests this close together
/// are one selection.
const SAME_SELECTION: Duration = Duration::from_secs(2);

static SPEC: TableSpec = TableSpec {
    id: TABLE,
    title: "File Locksmith",
    columns: &[("Process", 220), ("PID", 70), ("Using", 220), ("Program", 360)],
    fields: &[],
    checks: false,
    choices: &[],
    buttons: ["", "", "End task", "Refresh", ""],
};

/// What one look found.
#[derive(Default)]
pub struct Scan {
    pub items: Vec<PathBuf>,
    pub found: Vec<Found<Locker>>,
    pub truncated: bool,
    /// Folders that could not be looked into, and lookups that failed.
    pub problems: Vec<String>,
}

fn scan(items: Vec<PathBuf>) -> Scan {
    let expanded = locks::expand(&items, LIMIT);
    let mut problems: Vec<String> = expanded
        .unreadable
        .iter()
        .map(|(p, e)| format!("Could not look in {}: {e}", p.display()))
        .collect();
    let per_item = expanded
        .items
        .iter()
        .map(|item| {
            let files: Vec<_> = item.files.iter().map(PathBuf::as_path).collect();
            process::lockers(&files).unwrap_or_else(|e| {
                problems.push(format!("Could not check {}: {e}", item.path.display()));
                Vec::new()
            })
        })
        .collect();
    Scan {
        found: locks::merge(per_item, |a, b| a.pid == b.pid && a.started == b.started),
        items,
        truncated: expanded.truncated,
        problems,
    }
}

pub fn rows(scan: &Scan) -> Vec<TableRow> {
    scan.found
        .iter()
        .map(|f| {
            let p = &f.process;
            let name = match &p.service {
                Some(s) => format!("{} (service {s})", p.name),
                None => p.name.clone(),
            };
            let using: Vec<String> = f
                .items
                .iter()
                .filter_map(|&i| scan.items.get(i))
                .map(|p| locks::label(p))
                .collect();
            (
                false,
                vec![
                    name,
                    p.pid.to_string(),
                    using.join(", "),
                    p.path.clone().unwrap_or_default(),
                ],
            )
        })
        .collect()
}

fn what(items: &[PathBuf]) -> String {
    match items {
        [one] => locks::label(one),
        _ => format!("these {} items", items.len()),
    }
}

pub fn summary(scan: &Scan) -> String {
    let mut text = match scan.found.len() {
        0 => format!("No process is using {}.", what(&scan.items)),
        n => format!(
            "{n} {} using {}. Ending one loses its unsaved work.",
            if n == 1 { "process is" } else { "processes are" },
            what(&scan.items)
        ),
    };
    if scan.truncated {
        text += &format!(" Only the first {LIMIT} files were checked.");
    }
    for p in &scan.problems {
        text += " ";
        text += p;
    }
    text
}

pub struct LocksmithUi {
    waker: Waker,
    window: Option<TableWindow>,
    items: Vec<PathBuf>,
    opened: Option<Instant>,
    shown: Scan,
    looking: Option<Receiver<Scan>>,
    /// The items changed while a look was running, so another is due.
    again: bool,
}

impl LocksmithUi {
    pub fn new(waker: Waker) -> LocksmithUi {
        LocksmithUi {
            waker,
            window: None,
            items: Vec::new(),
            opened: None,
            shown: Scan::default(),
            looking: None,
            again: false,
        }
    }

    pub fn open(&mut self, paths: Vec<PathBuf>) -> Result<(), String> {
        if self.window.is_none() {
            let w = TableWindow::new(&SPEC).map_err(|e| format!("could not open File Locksmith: {e}"))?;
            self.window = Some(w);
            if let Some(w) = self.window.as_mut() {
                crate::icons::apply(file_locksmith::ID, |ico| w.set_icon(ico));
            }
        }
        let same = self.opened.is_some_and(|t| t.elapsed() < SAME_SELECTION);
        if !same {
            self.items.clear();
        }
        for p in paths {
            if !self.items.contains(&p) {
                self.items.push(p);
            }
        }
        self.opened = Some(Instant::now());
        let w = self.window.as_mut().expect("created above");
        w.set_title(&format!("File Locksmith - {}", what(&self.items)));
        w.show();
        self.look();
        Ok(())
    }

    fn look(&mut self) {
        if self.looking.is_some() {
            self.again = true;
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (waker, items) = (self.waker, self.items.clone());
        std::thread::spawn(move || {
            if tx.send(scan(items)).is_ok() {
                waker.wake();
            }
        });
        self.looking = Some(rx);
        if let Some(w) = self.window.as_mut() {
            w.set_status("Looking for processes using the files...");
        }
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.looking else { return };
        let outcome = match rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(scan) => Ok(scan),
            Err(TryRecvError::Disconnected) => Err("The search stopped unexpectedly."),
        };
        self.looking = None;
        if std::mem::take(&mut self.again) {
            return self.look();
        }
        let Some(w) = self.window.as_mut() else { return };
        match outcome {
            Ok(scan) => {
                w.set_rows(&rows(&scan), None);
                w.set_status(&summary(&scan));
                self.shown = scan;
            }
            Err(e) => w.set_status(e),
        }
    }

    pub fn on_event(&mut self, event: TableEvent) {
        match event {
            TableEvent::Reload => self.look(),
            TableEvent::Delete(row) => {
                let Some(found) = self.shown.found.get(row) else { return };
                let p = &found.process;
                match process::terminate_started(p.pid, p.started) {
                    Ok(()) => {
                        tracing::info!(pid = p.pid, name = %p.name, "ended a process holding files");
                        self.look();
                    }
                    Err(e) => {
                        let text = if e.is_access_denied() {
                            format!(
                                "Windows did not allow ending {}: it runs as another user or as administrator.",
                                p.name
                            )
                        } else {
                            format!("Could not end {}: {e}", p.name)
                        };
                        if let Some(w) = self.window.as_mut() {
                            w.set_status(&text);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locker(pid: u32, name: &str, service: Option<&str>) -> Locker {
        Locker {
            pid,
            started: 1,
            name: name.into(),
            service: service.map(Into::into),
            path: Some(format!("C:\\{name}.exe")),
        }
    }

    #[test]
    fn rows_name_the_process_and_what_it_uses() {
        let scan = Scan {
            items: vec![PathBuf::from("C:\\docs\\a.txt"), PathBuf::from("C:\\docs\\b")],
            found: vec![
                Found {
                    process: locker(7, "Notepad", None),
                    items: vec![0, 1],
                },
                Found {
                    process: locker(9, "Indexer", Some("WSearch")),
                    items: vec![1],
                },
            ],
            ..Scan::default()
        };
        let r = rows(&scan);
        assert_eq!(r[0].1, ["Notepad", "7", "a.txt, b", "C:\\Notepad.exe"]);
        assert_eq!(r[1].1[0], "Indexer (service WSearch)");
        assert!(summary(&scan).starts_with("2 processes are using these 2 items."));
    }

    #[test]
    fn the_summary_says_when_nothing_holds_the_file_and_what_went_wrong() {
        let scan = Scan {
            items: vec![PathBuf::from("C:\\a.txt")],
            truncated: true,
            problems: vec!["Could not look in C:\\x: denied".into()],
            ..Scan::default()
        };
        assert_eq!(
            summary(&scan),
            "No process is using a.txt. Only the first 20000 files were checked. Could not look in C:\\x: denied"
        );
    }
}
