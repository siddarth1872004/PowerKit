//! The Environment Variables window's contents. User variables are written directly;
//! system variables need administrator rights, so their changes are handed to an
//! elevated `powerkit --write-env` run on a worker thread and the result comes back
//! by wake-up.

use crate::Host;
use environment_variables::vars::{self, Change, Table, Var};
use powerkit_core::TableEvent;
use powerkit_platform::event_loop::Waker;
use powerkit_platform::table_window::{Field, TableRow, TableSpec, TableWindow};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const TABLE: &str = "env";
const USER: usize = 0;
const SYSTEM: usize = 1;

static SPEC: TableSpec = TableSpec {
    id: TABLE,
    title: "Environment Variables",
    columns: &[("Name", 220), ("Value", 540), ("Note", 90)],
    fields: &[
        Field {
            label: "Name",
            label_width: 45,
            width: 220,
        },
        Field {
            label: "Value",
            label_width: 45,
            width: 0,
        },
    ],
    checks: false,
    choices: &[
        "User variables",
        "System variables (saving asks for administrator permission)",
    ],
};

/// Write `changes` to one scope's registry key; stops at the first failure. Writing
/// the same changes again is harmless, so a partly applied save can be retried.
pub fn apply(system: bool, changes: &[Change]) -> Result<(), String> {
    use powerkit_platform::env;
    for c in changes {
        match &c.value {
            Some(v) => env::set(system, &c.name, v, c.expand),
            None => env::remove(system, &c.name),
        }
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn load(scope: usize) -> Result<Table, String> {
    let vars =
        powerkit_platform::env::list(scope == SYSTEM).map_err(|e| format!("Could not read the variables: {e}"))?;
    Ok(Table::new(
        vars.into_iter()
            .map(|v| Var {
                name: v.name,
                value: v.value,
                expand: v.expand,
            })
            .collect(),
    ))
}

fn rows(table: &Table) -> Vec<TableRow> {
    table.rows().into_iter().map(|r| (false, r.to_vec())).collect()
}

/// Apply an edit to `table`; returns the row to select afterwards.
pub fn edit(table: &mut Table, event: TableEvent) -> Result<Option<usize>, String> {
    let pair = |f: &[String]| match f {
        [name, value] => Ok((name.clone(), value.clone())),
        _ => Err("The variable's fields are missing.".to_string()),
    };
    match event {
        TableEvent::Add(f) => {
            let (name, value) = pair(&f)?;
            table.add(&name, &value).map(Some)
        }
        TableEvent::Update { row, fields } => {
            let (name, value) = pair(&fields)?;
            table.update(row, &name, &value).map(Some)
        }
        TableEvent::Delete(row) => table.delete(row),
        TableEvent::Toggle { .. } | TableEvent::Save | TableEvent::Reload | TableEvent::Choice(_) => Ok(None),
    }
}

pub struct EnvUi {
    waker: Waker,
    window: Option<TableWindow>,
    tables: [Table; 2],
    scope: usize,
    /// Where the outcome of saving the system variables arrives.
    saving: Option<Receiver<Result<u32, String>>>,
}

impl EnvUi {
    pub fn new(waker: Waker) -> EnvUi {
        EnvUi {
            waker,
            window: None,
            tables: Default::default(),
            scope: USER,
            saving: None,
        }
    }

    fn dirty(&self) -> bool {
        self.tables.iter().any(Table::is_dirty)
    }

    pub fn open(&mut self) -> Result<(), String> {
        if self.window.is_none() {
            self.window =
                Some(TableWindow::new(&SPEC).map_err(|e| format!("could not open Environment Variables: {e}"))?);
            if let Some(w) = self.window.as_mut() {
                crate::icons::apply("environment_variables", |ico| w.set_icon(ico));
            }
        }
        // Unsaved edits survive closing the window; otherwise show what is stored now.
        if !self.dirty() && self.saving.is_none() {
            let other = 1 - self.scope;
            if let Ok(t) = load(other) {
                self.tables[other] = t;
            }
            self.reload();
        }
        self.window.as_mut().expect("created above").show();
        Ok(())
    }

    fn reload(&mut self) {
        match load(self.scope) {
            Ok(t) => {
                self.tables[self.scope] = t;
                self.show(None, &self.summary());
            }
            Err(e) => {
                self.tables[self.scope] = Table::default();
                self.show(None, &e);
            }
        }
    }

    fn summary(&self) -> String {
        format!("{} variables.", self.tables[self.scope].vars().len())
    }

    fn show(&mut self, selected: Option<usize>, status: &str) {
        let title = if self.dirty() {
            "Environment Variables (not saved)"
        } else {
            "Environment Variables"
        };
        let rows = rows(&self.tables[self.scope]);
        let Some(w) = self.window.as_mut() else { return };
        w.set_rows(&rows, selected);
        w.set_status(status);
        w.set_title(title);
    }

    fn status(&mut self, text: &str) {
        if let Some(w) = self.window.as_mut() {
            w.set_status(text);
        }
    }

    pub fn on_event(&mut self, event: TableEvent, host: &Host) {
        if let TableEvent::Choice(i) = event {
            self.scope = i.min(SYSTEM);
            let summary = self.summary();
            return self.show(None, &summary);
        }
        if self.saving.is_some() {
            return self.status("Still saving; wait for it to finish.");
        }
        match event {
            TableEvent::Reload => self.reload(),
            TableEvent::Save => self.save(&host.paths().root),
            e => match edit(&mut self.tables[self.scope], e) {
                Ok(selected) => self.show(selected, "Changed. Choose Save to write the variables."),
                Err(e) => self.status(&e),
            },
        }
    }

    fn save(&mut self, data_dir: &Path) {
        if !self.dirty() {
            return self.status("Nothing to save.");
        }
        let user = self.tables[USER].changes();
        if !user.is_empty() {
            if let Err(e) = apply(false, &user) {
                return self.status(&format!("User variables not saved: {e}."));
            }
            self.tables[USER] = Table::new(self.tables[USER].vars().to_vec());
            std::thread::spawn(powerkit_platform::env::broadcast);
        }
        let system = self.tables[SYSTEM].changes();
        if system.is_empty() {
            let summary = self.summary();
            return self.show(
                None,
                &format!("Saved. Programs started from now on see the change. {summary}"),
            );
        }
        let staged = data_dir.join("environment.json");
        let prepared = serde_json::to_string(&system)
            .map_err(|e| e.to_string())
            .and_then(|json| {
                std::fs::create_dir_all(data_dir)
                    .and_then(|()| std::fs::write(&staged, json))
                    .map_err(|e| e.to_string())
            });
        if let Err(e) = prepared {
            return self.status(&format!("Could not prepare the system variables: {e}"));
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => return self.status(&format!("Could not find powerkit.exe: {e}")),
        };
        let (tx, rx) = mpsc::channel();
        let waker = self.waker;
        let args = format!("--write-env \"{}\"", staged.display());
        std::thread::spawn(move || {
            let result = powerkit_platform::shell::run_elevated(&exe, &args).map_err(|e| e.to_string());
            if tx.send(result).is_ok() {
                waker.wake();
            }
        });
        self.saving = Some(rx);
        self.status("Saving system variables; Windows asks for permission.");
    }

    /// Pick up the outcome of saving system variables, if it has arrived. Call on
    /// every wake-up.
    pub fn poll(&mut self) {
        let Some(rx) = &self.saving else { return };
        let outcome = match rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(r) => r,
            Err(TryRecvError::Disconnected) => Err("the save stopped unexpectedly".into()),
        };
        self.saving = None;
        match outcome {
            Ok(0) => {
                tracing::info!("system environment variables saved");
                self.tables[SYSTEM] = Table::new(self.tables[SYSTEM].vars().to_vec());
                let summary = self.summary();
                self.show(
                    None,
                    &format!("Saved. Programs started from now on see the change. {summary}"),
                );
            }
            Ok(code) => {
                tracing::warn!(code, "writing system environment variables failed");
                self.status(&vars::exit::message(code));
            }
            Err(e) => self.status(&format!("System variables not saved: {e}.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_edits_reach_the_table() {
        let mut t = Table::new(vec![Var {
            name: "A".into(),
            value: "1".into(),
            expand: false,
        }]);
        let add = TableEvent::Add(vec!["B".into(), "%A%".into()]);
        assert_eq!(edit(&mut t, add), Ok(Some(1)));
        let update = TableEvent::Update {
            row: 0,
            fields: vec!["A".into(), "2".into()],
        };
        assert_eq!(edit(&mut t, update), Ok(Some(0)));
        assert_eq!(rows(&t)[0], (false, vec!["A".into(), "2".into(), "Changed".into()]));
        assert!(edit(&mut t, TableEvent::Add(vec!["C".into()])).is_err());
        assert_eq!(edit(&mut t, TableEvent::Delete(1)), Ok(Some(0)));
        assert_eq!(t.changes().len(), 1);
    }
}
