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
    buttons: ["Capture", "Launch", "Delete", "Desktop shortcut", "Rename"],
};

const HINT: &str = "Arrange your windows, type a name and click Capture. Choose a workspace and click Launch to \
bring it back, or Desktop shortcut to launch it from the desktop. To rename one, choose it, change the name and click \
Rename.";

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
    let name = checked_name(name)?;
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

/// The trimmed name, if it can be a workspace name. A quote would end the name early
/// on a desktop shortcut's command line.
fn checked_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name for the workspace first.".into());
    }
    if name.contains('"') {
        return Err("A workspace name cannot contain a quote (\").".into());
    }
    Ok(name)
}

/// Rename the workspace at `row` to `name`, unless another one already has it.
pub fn rename(s: &mut Settings, row: usize, name: &str) -> Result<usize, String> {
    let name = checked_name(name)?;
    if row >= s.workspaces.len() {
        return Err("Choose a workspace to rename first.".into());
    }
    if let Some(other) = s
        .workspaces
        .iter()
        .enumerate()
        .find(|(i, w)| *i != row && w.name.eq_ignore_ascii_case(name))
    {
        return Err(format!("There is already a workspace named {}.", other.1.name));
    }
    s.workspaces[row].name = name.into();
    Ok(row)
}

/// The file name for a workspace's desktop shortcut: characters Windows does not
/// allow in file names become dashes.
pub fn shortcut_file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("{} (workspace)", clean.trim_end_matches(['.', ' ']))
}

/// The command line that launches the workspace `name`. Backslashes before the
/// closing quote are doubled so that they do not escape it.
pub fn shortcut_args(name: &str) -> String {
    let tail = name.len() - name.trim_end_matches('\\').len();
    format!("--workspace \"{name}{}\"", "\\".repeat(tail))
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
                let (apps, left_out) = match workspaces::capture() {
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
                    let mut text = format!("{verb} {name} with {count} windows.");
                    if !left_out.is_empty() {
                        text.push_str(&format!(
                            " Left out, as their programs cannot be started again: {}.",
                            left_out.join(", ")
                        ));
                    }
                    self.status(&text);
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
            TableEvent::Reload => self.desktop_shortcut(host),
            TableEvent::Save => {
                let Some(row) = self.window.as_ref().and_then(TableWindow::selected) else {
                    return self.status("Choose a workspace to rename first.");
                };
                let name = self.fields().first().cloned().unwrap_or_default();
                if self.save(host, |s| rename(s, row, &name).map(Some)) {
                    self.status(&format!("Renamed to {}.", name.trim()));
                }
            }
            _ => {}
        }
    }

    fn fields(&self) -> Vec<String> {
        self.window.as_ref().map(TableWindow::fields).unwrap_or_default()
    }

    fn desktop_shortcut(&mut self, host: &Host) {
        let list = &host.config().workspaces.workspaces;
        let Some(ws) = self
            .window
            .as_ref()
            .and_then(TableWindow::selected)
            .and_then(|i| list.get(i))
        else {
            return self.status("Choose a workspace first.");
        };
        let name = ws.name.clone();
        let made = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
            powerkit_platform::shell::create_desktop_shortcut(&shortcut_file_name(&name), &exe, &shortcut_args(&name))
                .map_err(|e| e.to_string())
        });
        match made {
            Ok(path) => self.status(&format!("Put {} on the desktop.", path.display())),
            Err(e) => self.status(&format!("Could not make the shortcut: {e}")),
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
    fn renaming_keeps_names_unique_and_quote_free() {
        let mut s = Settings::default();
        store(&mut s, "Work", apps(1)).unwrap();
        store(&mut s, "Play", apps(1)).unwrap();
        assert!(rename(&mut s, 1, "WORK").is_err());
        assert!(rename(&mut s, 1, "say \"hi\"").is_err());
        assert!(rename(&mut s, 5, "Games").is_err());
        assert_eq!(rename(&mut s, 1, " Games "), Ok(1));
        assert_eq!(rename(&mut s, 0, "work"), Ok(0));
        assert_eq!(s.workspaces[0].name, "work");
        assert_eq!(s.workspaces[1].name, "Games");
    }

    #[test]
    fn shortcuts_name_files_safely_and_quote_the_workspace() {
        assert_eq!(shortcut_file_name("A/B: C?"), "A-B- C- (workspace)");
        assert_eq!(shortcut_file_name("Dots..."), "Dots (workspace)");
        assert_eq!(shortcut_args("My work"), "--workspace \"My work\"");
        assert_eq!(shortcut_args("ends\\"), "--workspace \"ends\\\\\"");
    }

    #[test]
    fn nothing_is_saved_without_a_name_or_windows() {
        let mut s = Settings::default();
        assert!(store(&mut s, "  ", apps(1)).is_err());
        assert!(store(&mut s, "Empty", vec![]).is_err());
        assert!(s.workspaces.is_empty());
    }
}
