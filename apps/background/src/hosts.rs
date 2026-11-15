//! The Hosts File Editor window's contents. The file is read directly; saving needs
//! administrator rights, so the new text is handed to an elevated
//! `powerkit --write-hosts` run on a worker thread and the result comes back by wake-up.

use crate::Host;
use hosts_editor::hosts::{self, Entry, HostsFile, Line};
use powerkit_core::TableEvent;
use powerkit_platform::event_loop::Waker;
use powerkit_platform::table_window::{Field, TableRow, TableSpec, TableWindow};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const TABLE: &str = "hosts";

static SPEC: TableSpec = TableSpec {
    id: TABLE,
    title: "Hosts File Editor",
    columns: &[("Address", 180), ("Hosts", 330), ("Comment", 220), ("Note", 110)],
    fields: &[
        Field {
            label: "Address",
            label_width: 60,
            width: 170,
        },
        Field {
            label: "Hosts",
            label_width: 45,
            width: 0,
        },
        Field {
            label: "Comment",
            label_width: 70,
            width: 200,
        },
    ],
    checks: true,
    choices: &[],
};

/// The listed rows: one per entry, in file order.
pub fn rows(file: &HostsFile) -> Vec<TableRow> {
    let dupes = file.duplicates();
    file.entry_lines()
        .into_iter()
        .filter_map(|i| file.entry(i))
        .zip(dupes)
        .map(|(e, dupe)| {
            let note = if dupe { "Duplicate" } else { "" };
            (
                e.active,
                vec![e.address.clone(), e.hosts.join(" "), e.comment.clone(), note.into()],
            )
        })
        .collect()
}

/// Apply an edit to `file`; returns the row to select afterwards.
pub fn edit(file: &mut HostsFile, event: TableEvent) -> Result<Option<usize>, String> {
    let lines = file.entry_lines();
    let line = |row: usize| lines.get(row).copied().ok_or("That entry is no longer listed.");
    let entry = |f: &[String], active: bool| -> Result<Entry, String> {
        let [address, names, comment] = f else {
            return Err("The entry's fields are missing.".into());
        };
        let (address, names) = hosts::validate(address, names)?;
        Ok(Entry::new(active, &address, names, comment))
    };
    match event {
        TableEvent::Toggle { row, on } => {
            let i = line(row)?;
            file.entry_mut(i)
                .ok_or("That entry is no longer listed.")?
                .set_active(on);
            Ok(Some(row))
        }
        TableEvent::Add(f) => {
            file.lines.push(Line::Entry(entry(&f, true)?));
            Ok(Some(lines.len()))
        }
        TableEvent::Update { row, fields } => {
            let i = line(row)?;
            let active = file.entry(i).is_none_or(|e| e.active);
            file.lines[i] = Line::Entry(entry(&fields, active)?);
            Ok(Some(row))
        }
        TableEvent::Delete(row) => {
            file.lines.remove(line(row)?);
            Ok(row.checked_sub(1).or(if lines.len() > 1 { Some(0) } else { None }))
        }
        TableEvent::Save | TableEvent::Reload | TableEvent::Choice(_) => Ok(None),
    }
}

fn read(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| "The hosts file is not UTF-8 text, so PowerKit leaves it alone.".to_string())
}

pub struct HostsUi {
    waker: Waker,
    path: PathBuf,
    window: Option<TableWindow>,
    file: HostsFile,
    /// The file's text as last read or saved, to notice changes made by others.
    loaded: String,
    dirty: bool,
    /// A save in progress: the text being written, and where its outcome arrives.
    saving: Option<(String, Receiver<Result<u32, String>>)>,
}

impl HostsUi {
    pub fn new(waker: Waker) -> HostsUi {
        HostsUi {
            waker,
            path: hosts::system_path(),
            window: None,
            file: HostsFile::default(),
            loaded: String::new(),
            dirty: false,
            saving: None,
        }
    }

    pub fn open(&mut self) -> Result<(), String> {
        if self.window.is_none() {
            self.window =
                Some(TableWindow::new(&SPEC).map_err(|e| format!("could not open the Hosts File Editor: {e}"))?);
            if let Some(w) = self.window.as_mut() {
                crate::icons::apply("hosts_editor", |ico| w.set_icon(ico));
            }
        }
        // Unsaved edits survive closing the window; otherwise show the file as it is now.
        if !self.dirty && self.saving.is_none() {
            self.reload();
        }
        self.window.as_mut().expect("created above").show();
        Ok(())
    }

    fn reload(&mut self) {
        match read(&self.path) {
            Ok(text) => {
                self.file = HostsFile::parse(&text);
                self.loaded = text;
                self.dirty = false;
                self.show(None, &format!("{} entries.", self.file.entry_lines().len()));
            }
            Err(e) => {
                self.file = HostsFile::default();
                self.loaded.clear();
                self.dirty = false;
                self.show(None, &e);
            }
        }
    }

    fn show(&mut self, selected: Option<usize>, status: &str) {
        let Some(w) = self.window.as_mut() else { return };
        w.set_rows(&rows(&self.file), selected);
        w.set_status(status);
        w.set_title(if self.dirty {
            "Hosts File Editor (not saved)"
        } else {
            "Hosts File Editor"
        });
    }

    pub fn on_event(&mut self, event: TableEvent, host: &Host) {
        if self.saving.is_some() {
            if let Some(w) = self.window.as_mut() {
                w.set_status("Still saving; wait for it to finish.");
            }
            return;
        }
        match event {
            TableEvent::Reload => self.reload(),
            TableEvent::Save => self.save(&host.paths().root),
            e => match edit(&mut self.file, e) {
                Ok(selected) => {
                    self.dirty = true;
                    self.show(selected, "Changed. Choose Save to write the hosts file.");
                }
                Err(e) => {
                    if let Some(w) = self.window.as_mut() {
                        w.set_status(&e);
                    }
                }
            },
        }
    }

    fn save(&mut self, data_dir: &Path) {
        let status = |ui: &mut Self, text: &str| {
            if let Some(w) = ui.window.as_mut() {
                w.set_status(text);
            }
        };
        if !self.dirty {
            return status(self, "Nothing to save.");
        }
        match read(&self.path) {
            Ok(now) if now == self.loaded => {}
            Ok(_) => {
                return status(
                    self,
                    "Another program changed the hosts file. Choose Reload to see it (your changes are dropped).",
                );
            }
            Err(e) => return status(self, &e),
        }
        let text = self.file.render();
        let staged = data_dir.join("hosts.new");
        let prepared = std::fs::create_dir_all(data_dir)
            .and_then(|()| std::fs::write(data_dir.join("hosts.backup"), &self.loaded))
            .and_then(|()| std::fs::write(&staged, &text));
        if let Err(e) = prepared {
            return status(self, &format!("Could not prepare the new hosts file: {e}"));
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => return status(self, &format!("Could not find powerkit.exe: {e}")),
        };
        let (tx, rx) = mpsc::channel();
        let waker = self.waker;
        let args = format!("--write-hosts \"{}\"", staged.display());
        std::thread::spawn(move || {
            let result = powerkit_platform::shell::run_elevated(&exe, &args).map_err(|e| e.to_string());
            if tx.send(result).is_ok() {
                waker.wake();
            }
        });
        self.saving = Some((text, rx));
        status(self, "Saving; Windows asks for permission.");
    }

    /// Pick up the outcome of a save, if one has arrived. Call on every wake-up.
    pub fn poll(&mut self) {
        let Some((text, rx)) = &self.saving else { return };
        let outcome = match rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(r) => r,
            Err(TryRecvError::Disconnected) => Err("The save stopped unexpectedly.".into()),
        };
        let text = text.clone();
        self.saving = None;
        match outcome {
            Ok(0) => {
                tracing::info!("hosts file saved");
                self.file = HostsFile::parse(&text);
                self.loaded = text;
                self.dirty = false;
                self.show(
                    None,
                    "Saved. The previous version is in hosts.backup in PowerKit's data folder.",
                );
            }
            Ok(code) => {
                tracing::warn!(code, "writing the hosts file failed");
                let message = hosts::exit::message(code);
                if let Some(w) = self.window.as_mut() {
                    w.set_status(&message);
                }
            }
            Err(e) => {
                if let Some(w) = self.window.as_mut() {
                    w.set_status(&format!("Not saved: {e}."));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(a: &str, h: &str) -> Vec<String> {
        vec![a.into(), h.into(), String::new()]
    }

    #[test]
    fn edits_change_the_listed_entries() {
        let mut f = HostsFile::parse("# header\r\n1.1.1.1 a\r\n2.2.2.2 b\r\n");
        assert_eq!(edit(&mut f, TableEvent::Add(fields("3.3.3.3", "a"))), Ok(Some(2)));
        let r = rows(&f);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].1[3], "Duplicate");
        assert_eq!(r[2].1[3], "Duplicate");

        assert_eq!(edit(&mut f, TableEvent::Toggle { row: 0, on: false }), Ok(Some(0)));
        assert!(!rows(&f)[0].0);
        assert_eq!(rows(&f)[2].1[3], "", "a disabled entry is no duplicate");

        let update = TableEvent::Update {
            row: 0,
            fields: fields("9.9.9.9", "z"),
        };
        assert_eq!(edit(&mut f, update), Ok(Some(0)));
        assert!(!rows(&f)[0].0, "updating keeps the entry off");
        assert_eq!(rows(&f)[0].1[0], "9.9.9.9");

        assert_eq!(edit(&mut f, TableEvent::Delete(1)), Ok(Some(0)));
        assert_eq!(f.render(), "# header\r\n# 9.9.9.9\tz\r\n3.3.3.3\ta\r\n");
    }

    #[test]
    fn bad_input_changes_nothing() {
        let mut f = HostsFile::parse("1.1.1.1 a\n");
        assert!(edit(&mut f, TableEvent::Add(fields("nope", "a"))).is_err());
        assert!(edit(&mut f, TableEvent::Delete(5)).is_err());
        assert_eq!(f.render(), "1.1.1.1 a\n");
        assert_eq!(edit(&mut f, TableEvent::Delete(0)), Ok(None));
    }
}
