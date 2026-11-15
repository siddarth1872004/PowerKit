//! The PowerRename window's contents: the items handed over from Explorer, previewed
//! with the options on the form and renamed through PowerRename's module commands.

use crate::Host;
use power_rename::engine::{Planned, Status};
use powerkit_config::{CaseMode, Position, RenameOp, RenameOptions, Target};
use powerkit_core::{RenameEvent, RenameForm};
use powerkit_platform::rename_window::RenameWindow;
use serde_json::{json, Value};
use std::path::PathBuf;

/// The rename pipeline the form describes.
pub fn options(form: &RenameForm) -> RenameOptions {
    let mut ops = Vec::new();
    if !form.search.is_empty() {
        ops.push(RenameOp::Replace {
            find: form.search.clone(),
            with: form.replace.clone(),
            regex: form.regex,
            case_insensitive: !form.match_case,
            first_only: !form.all_occurrences,
        });
    }
    let mode = match form.case {
        1 => Some(CaseMode::Lower),
        2 => Some(CaseMode::Upper),
        3 => Some(CaseMode::Title),
        4 => Some(CaseMode::Sentence),
        _ => None,
    };
    if let Some(mode) = mode {
        ops.push(RenameOp::Case { mode });
    }
    if form.enumerate {
        ops.push(RenameOp::Number {
            start: 1,
            step: 1,
            padding: 0,
            position: Position::End,
            separator: " ".into(),
        });
    }
    RenameOptions {
        ops,
        target: match form.target {
            1 => Target::Extension,
            2 => Target::Full,
            _ => Target::Stem,
        },
        include_files: form.files,
        include_folders: form.folders,
        ..RenameOptions::default()
    }
}

/// `[original, renamed, status]` for each previewed item.
pub fn rows(items: &[Planned]) -> Vec<[String; 3]> {
    items
        .iter()
        .map(|p| {
            let renamed = match p.status {
                Status::Unchanged | Status::Skipped | Status::Missing => String::new(),
                _ => p.new_name.clone(),
            };
            let status = match p.status {
                Status::Rename => "",
                Status::Unchanged => "No change",
                Status::Skipped => "Skipped",
                Status::Conflict => "Name already taken",
                Status::Invalid => "Invalid name",
                Status::Missing => "Not found",
            };
            let status = match &p.message {
                Some(m) if p.status != Status::Rename => format!("{status}: {m}"),
                _ => status.to_string(),
            };
            [p.name.clone(), renamed, status]
        })
        .collect()
}

/// Point each path that was moved from `from` to `to` at its new place.
pub fn follow(paths: &mut [PathBuf], moves: &[(PathBuf, PathBuf)]) {
    for p in paths.iter_mut() {
        if let Some((_, to)) = moves.iter().find(|(from, _)| from == p) {
            *p = to.clone();
        }
    }
}

#[derive(Default)]
pub struct RenameUi {
    window: Option<RenameWindow>,
    form: RenameForm,
    paths: Vec<PathBuf>,
    /// The last rename from this window, `(before, after)`, to follow back on undo.
    last: Vec<(PathBuf, PathBuf)>,
}

fn command(host: &mut Host, command: &str, args: Value) -> Result<Value, String> {
    let params = json!({ "id": power_rename::ID, "command": command, "args": args });
    host.handle_request("module.command", &params)
}

impl RenameUi {
    /// Take the items waiting in PowerRename and show them, with any already listed.
    pub fn open(&mut self, host: &mut Host) -> Result<(), String> {
        let pending = command(host, "pending", Value::Null)?;
        let taken: Vec<PathBuf> = serde_json::from_value(pending["paths"].clone()).map_err(|e| e.to_string())?;
        if self.window.is_none() {
            self.window = Some(RenameWindow::new(&self.form).map_err(|e| format!("could not open PowerRename: {e}"))?);
            if let Some(w) = self.window.as_mut() {
                crate::icons::apply("power_rename", |ico| w.set_icon(ico));
            }
        }
        for p in taken {
            if !self.paths.contains(&p) {
                self.paths.push(p);
            }
        }
        let shown = self.window.as_ref().is_some_and(RenameWindow::is_visible);
        self.preview(host, None);
        if !shown {
            self.window.as_mut().expect("created above").show();
        }
        Ok(())
    }

    pub fn on_event(&mut self, event: RenameEvent, host: &mut Host) {
        match event {
            RenameEvent::Changed(form) => {
                self.form = form;
                self.preview(host, None);
            }
            RenameEvent::Apply => self.apply(host),
            RenameEvent::Undo => self.undo(host),
            RenameEvent::Closed => {
                self.paths.clear();
                self.last.clear();
            }
        }
    }

    fn args(&self) -> Value {
        json!({ "paths": self.paths, "options": options(&self.form) })
    }

    /// Show what the form would do; `done` replaces the summary in the status line.
    fn preview(&mut self, host: &mut Host, done: Option<String>) {
        let result = if self.paths.is_empty() {
            Ok(Vec::new())
        } else {
            command(host, "preview", self.args())
                .and_then(|r| serde_json::from_value::<Vec<Planned>>(r["items"].clone()).map_err(|e| e.to_string()))
        };
        let Some(w) = self.window.as_mut() else { return };
        match result {
            Ok(items) => {
                w.set_items(&rows(&items));
                let n = items.iter().filter(|p| p.status == Status::Rename).count();
                let summary = format!("{n} of {} items will be renamed.", items.len());
                w.set_status(&done.map_or(summary.clone(), |d| format!("{d} {summary}")));
            }
            Err(e) => {
                let names: Vec<[String; 3]> = self
                    .paths
                    .iter()
                    .map(|p| {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
                        [name.unwrap_or_default(), String::new(), String::new()]
                    })
                    .collect();
                w.set_items(&names);
                w.set_status(&e);
            }
        }
    }

    fn apply(&mut self, host: &mut Host) {
        if self.paths.is_empty() {
            return;
        }
        match command(host, "apply", self.args()) {
            Ok(r) => {
                let items: Vec<Planned> = match serde_json::from_value(r["items"].clone()) {
                    Ok(items) => items,
                    Err(e) => {
                        // The files were renamed but cannot be followed; start over.
                        self.paths.clear();
                        self.preview(host, Some(format!("Renamed, but the list was lost: {e}.")));
                        return;
                    }
                };
                self.last = items
                    .iter()
                    .filter(|p| p.status == Status::Rename)
                    .map(|p| (p.path.clone(), p.target()))
                    .collect();
                follow(&mut self.paths, &self.last);
                let renamed = r["renamed"].as_u64().unwrap_or(0);
                self.preview(host, Some(format!("Renamed {renamed} items.")));
            }
            Err(e) => self.preview(host, Some(format!("Rename failed: {e}."))),
        }
    }

    fn undo(&mut self, host: &mut Host) {
        match command(host, "undo", Value::Null) {
            Ok(r) => {
                let back: Vec<(PathBuf, PathBuf)> = std::mem::take(&mut self.last)
                    .into_iter()
                    .map(|(before, after)| (after, before))
                    .collect();
                follow(&mut self.paths, &back);
                let restored = r["restored"].as_u64().unwrap_or(0);
                self.preview(host, Some(format!("Restored {restored} items.")));
            }
            Err(e) => self.preview(host, Some(format!("Undo failed: {e}."))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_becomes_a_pipeline() {
        let form = RenameForm {
            search: "IMG_".into(),
            replace: "Photo ".into(),
            case: 1,
            enumerate: true,
            target: 2,
            folders: false,
            ..RenameForm::default()
        };
        let o = options(&form);
        assert_eq!(o.target, Target::Full);
        assert!(o.include_files && !o.include_folders);
        assert_eq!(o.ops.len(), 3);
        assert!(matches!(
            &o.ops[0],
            RenameOp::Replace { find, with, case_insensitive: true, first_only: false, regex: false }
                if find == "IMG_" && with == "Photo "
        ));
        assert_eq!(o.ops[1], RenameOp::Case { mode: CaseMode::Lower });
        assert!(matches!(o.ops[2], RenameOp::Number { start: 1, .. }));
        assert!(options(&RenameForm::default()).ops.is_empty());
    }

    #[test]
    fn rows_show_new_names_only_where_they_apply() {
        let item = |name: &str, new: &str, status, message: Option<&str>| Planned {
            path: PathBuf::from(name),
            name: name.into(),
            new_name: new.into(),
            status,
            message: message.map(Into::into),
        };
        let r = rows(&[
            item("a.txt", "b.txt", Status::Rename, None),
            item("c.txt", "c.txt", Status::Unchanged, None),
            item("d.txt", "e.txt", Status::Conflict, Some("e.txt exists")),
        ]);
        assert_eq!(r[0], ["a.txt", "b.txt", ""]);
        assert_eq!(r[1], ["c.txt", "", "No change"]);
        assert_eq!(r[2], ["d.txt", "e.txt", "Name already taken: e.txt exists"]);
    }

    #[test]
    fn paths_follow_renames_and_back() {
        let mut paths = vec![PathBuf::from("a"), PathBuf::from("b")];
        let moves = vec![(PathBuf::from("a"), PathBuf::from("x"))];
        follow(&mut paths, &moves);
        assert_eq!(paths, [PathBuf::from("x"), PathBuf::from("b")]);
        let back: Vec<_> = moves.into_iter().map(|(a, b)| (b, a)).collect();
        follow(&mut paths, &back);
        assert_eq!(paths, [PathBuf::from("a"), PathBuf::from("b")]);
    }
}
