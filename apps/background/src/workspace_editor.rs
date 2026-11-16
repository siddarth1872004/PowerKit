//! The Workspaces editor: capture the open windows under a name, launch or delete a
//! saved workspace. Launching waits for programs to open their windows, so it runs on
//! a worker thread and the outcome comes back by wake-up.

use crate::Host;
use powerkit_config::{Workspace, Workspaces as Settings};
use powerkit_core::TableEvent;
use powerkit_platform::event_loop::Waker;
use powerkit_platform::table_window::{Field, TableRow, TableSpec, TableWindow};
use serde_json::json;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const TABLE: &str = "workspaces";

static SPEC: TableSpec = TableSpec {
    id: TABLE,
    title: "Workspaces",
    columns: &[("Name", 220), ("Programs", 560), ("Windows", 90)],
    fields: &[Field {
        label: "Name",
        label_width: 50,
        width: 0,
        capture: false,
    }],
    checks: false,
    choices: &[],
    buttons: ["Capture", "Launch", "Delete", "", ""],
};

const HINT: &str =
    "Arrange your windows, type a name and click Capture. Choose a workspace and click Launch to bring it back.";

pub fn rows(workspaces: &[Workspace]) -> Vec<TableRow> {
    workspaces
        .iter()
        .map(|w| {
            (
                false,
                vec![w.name.clone(), workspaces::summary(&w.apps), w.apps.len().to_string()],
            )
        })
        .collect()
}

/// Save `apps` as the workspace `name`, replacing one of that name; returns its row
/// and whether it replaced one.
pub fn store(s: &mut Settings, name: &str, apps: Vec<powerkit_config::WorkspaceApp>) -> Result<(usize, bool), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name for the workspace first.".into());
    }
    if apps.is_empty() {
        return Err("There are no windows to save.".into());
    }
    let ws = Workspace {
        name: name.into(),
        apps,
    };
    match s.workspaces.iter().position(|w| w.name.eq_ignore_ascii_case(name)) {
        Some(i) => {
            s.workspaces[i] = ws;
            Ok((i, true))
        }
        None => {
            s.workspaces.push(ws);
            Ok((s.workspaces.len() - 1, false))
        }
    }
}

pub struct WorkspacesUi {
    waker: Waker,
    window: Option<TableWindow>,
    working: Option<(String, Receiver<workspaces::Report>)>,
}

impl WorkspacesUi {
    pub fn new(waker: Waker) -> WorkspacesUi {
        WorkspacesUi {
            waker,
            window: None,
            working: None,
        }
    }

    pub fn open(&mut self, host: &Host) -> Result<(), String> {
        if self.window.is_none() {
            let mut w = TableWindow::new(&SPEC).map_err(|e| format!("could not open Workspaces: {e}"))?;
            crate::icons::apply(workspaces::ID, |ico| w.set_icon(ico));
            self.window = Some(w);
        }
        let list = &host.config().workspaces.workspaces;
        let w = self.window.as_mut().expect("created above");
        w.set_rows(&rows(list), (!list.is_empty()).then_some(0));
        w.set_status(HINT);
        w.show();
        Ok(())
    }

    fn status(&mut self, text: &str) {
        if let Some(w) = self.window.as_mut() {
            w.set_status(text);
        }
    }

    fn visible(&self) -> bool {
        self.window.as_ref().is_some_and(TableWindow::is_visible)
    }

    /// Start bringing back the workspace `name`.
    pub fn launch(&mut self, name: &str, host: &Host) -> Result<(), String> {
        if self.working.is_some() {
            return Err("Still launching the last workspace.".into());
        }
        let s = &host.config().workspaces;
        let ws = workspaces::find(&s.workspaces, name).ok_or_else(|| format!("There is no workspace named {name}."))?;
        let (tx, rx) = mpsc::channel();
        let (waker, apps, move_existing) = (self.waker, ws.apps.clone(), s.move_existing);
        std::thread::spawn(move || {
            if tx.send(workspaces::launch(&apps, move_existing)).is_ok() {
                waker.wake();
            }
        });
        let text = format!("Launching {}...", ws.name);
        self.working = Some((ws.name.clone(), rx));
        self.status(&text);
        Ok(())
    }

    /// The outcome of a finished launch, to show as a notification when the editor is
    /// not open.
    pub fn poll(&mut self) -> Option<String> {
        let (name, rx) = self.working.as_ref()?;
        let text = match rx.try_recv() {
            Err(TryRecvError::Empty) => return None,
            Ok(report) => report.text(name),
            Err(TryRecvError::Disconnected) => format!("Launching {name} stopped unexpectedly."),
        };
        self.working = None;
        if self.visible() {
            self.status(&text);
            None
        } else {
            Some(text)
        }
    }

    fn save(&mut self, host: &mut Host, change: impl FnOnce(&mut Settings) -> Result<Option<usize>, String>) -> bool {
        let mut config = host.config().clone();
        let selected = match change(&mut config.workspaces) {
            Ok(row) => row,
            Err(e) => {
                self.status(&e);
                return false;
            }
        };
        if let Err(e) = host.handle_request("config.set", &json!({ "config": config })) {
            self.status(&format!("Not saved: {e}"));
            return false;
        }
        let list = &host.config().workspaces.workspaces;
        if let Some(w) = self.window.as_mut() {
            w.set_rows(&rows(list), selected.filter(|i| *i < list.len()));
        }
        true
    }

    pub fn on_event(&mut self, event: TableEvent, host: &mut Host) {
        match event {
            TableEvent::Add(fields) => {
                let name = fields.first().map_or("", |f| f.trim()).to_string();
                let apps = match workspaces::capture() {
                    Ok(a) => a,
                    Err(e) => return self.status(&format!("Could not read the open windows: {e:#}")),
                };
                let count = apps.len();
                let mut replaced = false;
                if self.save(host, |s| {
                    store(s, &name, apps).map(|(row, r)| {
                        replaced = r;
                        Some(row)
                    })
                }) {
                    let verb = if replaced { "Captured again" } else { "Captured" };
                    self.status(&format!("{verb} {name} with {count} windows."));
                }
            }
            TableEvent::Update { row, .. } => {
                let Some(name) = host.config().workspaces.workspaces.get(row).map(|w| w.name.clone()) else {
                    return;
                };
                if let Err(e) = self.launch(&name, host) {
                    self.status(&e);
                }
            }
            TableEvent::Delete(row) => {
                self.save(host, |s| {
                    if row < s.workspaces.len() {
                        s.workspaces.remove(row);
                    }
                    Ok(None)
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::WorkspaceApp;

    fn apps(n: usize) -> Vec<WorkspaceApp> {
        (0..n)
            .map(|i| WorkspaceApp {
                path: format!("C:\\app{i}.exe"),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn capturing_under_a_used_name_replaces_that_workspace() {
        let mut s = Settings::default();
        assert_eq!(store(&mut s, " Work ", apps(2)), Ok((0, false)));
        assert_eq!(store(&mut s, "Play", apps(1)), Ok((1, false)));
        assert_eq!(store(&mut s, "work", apps(3)), Ok((0, true)));
        assert_eq!(s.workspaces.len(), 2);
        assert_eq!(s.workspaces[0].apps.len(), 3);
        assert_eq!(rows(&s.workspaces)[1].1, ["Play", "app0", "1"]);
    }

    #[test]
    fn nothing_is_saved_without_a_name_or_windows() {
        let mut s = Settings::default();
        assert!(store(&mut s, "  ", apps(1)).is_err());
        assert!(store(&mut s, "Empty", vec![]).is_err());
        assert!(s.workspaces.is_empty());
    }
}
