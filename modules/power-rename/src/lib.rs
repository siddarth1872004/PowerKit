//! PowerRename: bulk rename files and folders with a live preview, conflict detection,
//! all-or-nothing batches and undo.

mod date;
pub mod engine;
mod exif;
mod glob;
#[cfg(windows)]
mod shell_menu;

use anyhow::{anyhow, bail, Result};
use engine::{Clock, Move, Status};
use powerkit_config::{PowerRename as Settings, Preset, RenameOptions, StateFile};
use powerkit_plugin_api::{Action, CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::SystemTime;

pub const ID: &str = "power_rename";
const HISTORY_FILE: &str = "power_rename_history";
/// Bounds one request so a huge selection cannot stall the host.
const MAX_ITEMS: usize = 20_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Batch {
    /// Unix seconds when the batch was applied.
    pub time: i64,
    pub moves: Vec<Move>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct History {
    batches: Vec<Batch>,
}

#[derive(Deserialize)]
struct BatchArgs {
    paths: Vec<PathBuf>,
    #[serde(default)]
    options: RenameOptions,
}

#[derive(Default)]
pub struct PowerRename {
    settings: Settings,
    /// Paths handed over by Explorer, waiting for the settings window to pick them up.
    pending: Vec<PathBuf>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn to_local(utc: i64) -> Result<i64> {
    powerkit_platform::time::to_local(utc).map_err(|e| anyhow!(e))
}

/// Non-Windows builds only run tests; they format times as UTC.
#[cfg(not(windows))]
fn to_local(utc: i64) -> Result<i64> {
    Ok(utc)
}

fn clock() -> Clock {
    Clock { now: now(), to_local }
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))
}

fn batch_args(args: &Value) -> Result<BatchArgs> {
    let a: BatchArgs = parse(args)?;
    if a.paths.len() > MAX_ITEMS {
        bail!("Too many items selected ({}); the limit is {MAX_ITEMS}", a.paths.len());
    }
    Ok(a)
}

fn summary(plan: &[engine::Planned]) -> Value {
    let count = |s: Status| plan.iter().filter(|p| p.status == s).count();
    json!({
        "rename": count(Status::Rename),
        "unchanged": count(Status::Unchanged),
        "skipped": count(Status::Skipped),
        "conflict": count(Status::Conflict),
        "invalid": count(Status::Invalid),
        "missing": count(Status::Missing),
    })
}

impl PowerRename {
    pub fn new() -> Self {
        Self::default()
    }

    fn history(ctx: &Ctx) -> StateFile {
        StateFile::new(ctx.paths, HISTORY_FILE)
    }

    fn record(&self, ctx: &Ctx, moves: Vec<Move>) -> Result<()> {
        let file = Self::history(ctx);
        let mut history: History = file.load();
        history.batches.push(Batch { time: now(), moves });
        let limit = self.settings.undo_limit as usize;
        let excess = history.batches.len().saturating_sub(limit);
        history.batches.drain(..excess);
        file.save(&history)
            .map_err(|e| anyhow!("The rename worked, but undo information was not saved: {e}"))
    }

    fn undo_last(&self, ctx: &Ctx) -> Result<usize> {
        let file = Self::history(ctx);
        let mut history: History = file.load();
        let Some(batch) = history.batches.last() else {
            bail!("There is nothing to undo")
        };
        engine::undo(&batch.moves)?;
        let count = batch.moves.len();
        history.batches.pop();
        file.save(&history)
            .map_err(|e| anyhow!("Undo worked, but the history was not updated: {e}"))?;
        Ok(count)
    }

    fn apply_settings(&mut self, ctx: &Ctx) {
        self.settings = ctx.config.power_rename.clone();
        #[cfg(windows)]
        if let Err(e) = shell_menu::sync(self.settings.context_menu) {
            tracing::warn!(error = %e, "Explorer context menu not updated");
        }
    }

    fn changed(&self, ctx: &mut Ctx) {
        ctx.emit("power_rename.changed", self.status());
    }
}

impl Module for PowerRename {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "PowerRename".into(),
            description: "Rename many files and folders at once, with preview and undo.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::FilesystemRead, Permission::FilesystemWrite],
            commands: vec![
                cmd("preview", "Preview a rename", "Args: {paths, options}"),
                cmd("apply", "Rename", "Args: {paths, options}"),
                cmd("undo", "Undo the last rename", ""),
                cmd("history", "List renames that can be undone", ""),
                cmd("open", "Open PowerRename with files", "Args: {paths}"),
                cmd("pending", "Take the files handed over from Explorer", ""),
                cmd("save_preset", "Save options as a preset", "Args: {name, options}"),
                cmd("delete_preset", "Delete a preset", "Args: {name}"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.apply_settings(ctx);
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.pending.clear();
        // The menu entry launches PowerKit, so it must not outlive the module.
        #[cfg(windows)]
        if let Err(e) = shell_menu::sync(false) {
            tracing::warn!(error = %e, "Explorer context menu not removed");
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.apply_settings(ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "preview" => {
                let a = batch_args(args)?;
                let plan = engine::preview(&a.paths, &a.options, &clock())?;
                Ok(json!({ "summary": summary(&plan), "items": plan }))
            }
            "apply" => {
                let a = batch_args(args)?;
                let plan = engine::preview(&a.paths, &a.options, &clock())?;
                let moves = engine::apply(&plan)?;
                let renamed = moves.len();
                if renamed > 0 && self.settings.undo_limit > 0 {
                    self.record(ctx, moves)?;
                }
                self.changed(ctx);
                Ok(json!({ "renamed": renamed, "summary": summary(&plan), "items": plan }))
            }
            "undo" => {
                let restored = self.undo_last(ctx)?;
                self.changed(ctx);
                Ok(json!({ "restored": restored }))
            }
            "history" => {
                let history: History = Self::history(ctx).load();
                let batches: Vec<Value> = history
                    .batches
                    .iter()
                    .rev()
                    .map(|b| {
                        let sample: Vec<Value> = b
                            .moves
                            .iter()
                            .take(5)
                            .map(|m| json!({ "from": m.from, "to": m.to }))
                            .collect();
                        json!({ "time": b.time, "count": b.moves.len(), "sample": sample })
                    })
                    .collect();
                Ok(Value::Array(batches))
            }
            "open" => {
                #[derive(Deserialize)]
                struct OpenArgs {
                    paths: Vec<PathBuf>,
                }
                let a: OpenArgs = parse(args)?;
                // Explorer starts one process per selected item, so these calls arrive in a
                // burst; only the first one opens the window, the rest join the list.
                let first = self.pending.is_empty();
                for p in a.paths {
                    if self.pending.len() < MAX_ITEMS && !self.pending.contains(&p) {
                        self.pending.push(p);
                    }
                }
                if first {
                    ctx.actions.push(Action::OpenSettings { page: Some(ID.into()) });
                }
                ctx.emit("power_rename.pending", json!({ "count": self.pending.len() }));
                Ok(json!({ "pending": self.pending.len() }))
            }
            "pending" => Ok(json!({ "paths": std::mem::take(&mut self.pending) })),
            "save_preset" => {
                #[derive(Deserialize)]
                struct PresetArgs {
                    name: String,
                    options: RenameOptions,
                }
                let a: PresetArgs = parse(args)?;
                let name = a.name.trim().to_string();
                if name.is_empty() {
                    bail!("The preset needs a name");
                }
                engine::Pipeline::compile(&a.options)?;
                ctx.update_config(move |c| {
                    let presets = &mut c.power_rename.presets;
                    let preset = Preset {
                        name: name.clone(),
                        options: a.options,
                    };
                    match presets.iter_mut().find(|p| p.name.eq_ignore_ascii_case(&name)) {
                        Some(existing) => *existing = preset,
                        None => presets.push(preset),
                    }
                });
                Ok(Value::Null)
            }
            "delete_preset" => {
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("missing \"name\""))?;
                let name = name.trim().to_string();
                ctx.update_config(move |c| c.power_rename.presets.retain(|p| !p.name.eq_ignore_ascii_case(&name)));
                Ok(Value::Null)
            }
            other => bail!("unknown command {other:?}"),
        }
    }

    fn status(&self) -> Value {
        json!({ "pending": self.pending.len() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths, RenameOp};

    struct Fixture {
        config: Config,
        paths: Paths,
        actions: Vec<Action>,
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            Self {
                config: Config::new(),
                paths: Paths::new(dir.path().join("data")),
                actions: Vec::new(),
                dir,
            }
        }

        fn ctx(&mut self) -> Ctx<'_> {
            Ctx {
                config: &self.config,
                paths: &self.paths,
                actions: &mut self.actions,
            }
        }

        fn file(&self, name: &str) -> PathBuf {
            let p = self.dir.path().join(name);
            std::fs::write(&p, name).unwrap();
            p
        }

        /// Run queued config updates, as the host would.
        fn settle(&mut self) {
            for action in std::mem::take(&mut self.actions) {
                if let Action::UpdateConfig(f) = action {
                    f(&mut self.config);
                }
            }
        }
    }

    fn suffix_args(paths: &[PathBuf]) -> Value {
        let options = RenameOptions {
            ops: vec![RenameOp::Insert {
                text: "-x".into(),
                position: powerkit_config::Position::End,
            }],
            ..RenameOptions::default()
        };
        json!({ "paths": paths, "options": options })
    }

    #[test]
    fn apply_then_undo_through_commands() {
        let mut fx = Fixture::new();
        let mut m = PowerRename::new();
        m.start(&mut fx.ctx()).unwrap();
        let a = fx.file("a.txt");
        let b = fx.file("b.txt");
        let args = suffix_args(&[a.clone(), b.clone()]);

        let preview = m.command("preview", &args, &mut fx.ctx()).unwrap();
        assert_eq!(preview["summary"]["rename"], 2);
        assert_eq!(preview["items"][0]["new_name"], "a-x.txt");
        assert!(a.exists(), "preview does not touch files");

        let applied = m.command("apply", &args, &mut fx.ctx()).unwrap();
        assert_eq!(applied["renamed"], 2);
        assert!(fx.dir.path().join("a-x.txt").exists() && !a.exists());
        assert_eq!(
            m.command("history", &Value::Null, &mut fx.ctx()).unwrap()[0]["count"],
            2
        );

        assert_eq!(m.command("undo", &Value::Null, &mut fx.ctx()).unwrap()["restored"], 2);
        assert!(a.exists() && b.exists());
        assert!(
            m.command("undo", &Value::Null, &mut fx.ctx()).is_err(),
            "history is now empty"
        );
    }

    #[test]
    fn history_respects_limit() {
        let mut fx = Fixture::new();
        fx.config.power_rename.undo_limit = 2;
        let mut m = PowerRename::new();
        m.start(&mut fx.ctx()).unwrap();
        let mut path = fx.file("f.txt");
        for _ in 0..3 {
            let out = m
                .command("apply", &suffix_args(&[path.clone()]), &mut fx.ctx())
                .unwrap();
            path = path.with_file_name(out["items"][0]["new_name"].as_str().unwrap());
        }
        let history = m.command("history", &Value::Null, &mut fx.ctx()).unwrap();
        assert_eq!(history.as_array().unwrap().len(), 2);
    }

    #[test]
    fn open_collects_paths_and_opens_window_once() {
        let mut fx = Fixture::new();
        let mut m = PowerRename::new();
        m.start(&mut fx.ctx()).unwrap();
        m.command("open", &json!({ "paths": ["C:\\a.txt"] }), &mut fx.ctx())
            .unwrap();
        m.command("open", &json!({ "paths": ["C:\\b.txt", "C:\\a.txt"] }), &mut fx.ctx())
            .unwrap();
        let opens = fx
            .actions
            .iter()
            .filter(|a| matches!(a, Action::OpenSettings { .. }))
            .count();
        assert_eq!(opens, 1);
        let pending = m.command("pending", &Value::Null, &mut fx.ctx()).unwrap();
        assert_eq!(pending["paths"].as_array().unwrap().len(), 2);
        assert_eq!(
            m.command("pending", &Value::Null, &mut fx.ctx()).unwrap()["paths"],
            json!([])
        );
    }

    #[test]
    fn presets_are_saved_and_deleted() {
        let mut fx = Fixture::new();
        let mut m = PowerRename::new();
        m.start(&mut fx.ctx()).unwrap();
        let options = json!({ "ops": [{ "op": "case", "mode": "upper" }] });
        m.command(
            "save_preset",
            &json!({ "name": "Shout", "options": options }),
            &mut fx.ctx(),
        )
        .unwrap();
        fx.settle();
        assert_eq!(fx.config.power_rename.presets[0].name, "Shout");
        let bad = json!({ "ops": [{ "op": "replace", "find": "(", "regex": true }] });
        assert!(m
            .command("save_preset", &json!({ "name": "Bad", "options": bad }), &mut fx.ctx())
            .is_err());
        m.command("delete_preset", &json!({ "name": "shout" }), &mut fx.ctx())
            .unwrap();
        fx.settle();
        assert!(fx.config.power_rename.presets.is_empty());
    }

    #[test]
    fn bad_arguments_are_errors() {
        let mut fx = Fixture::new();
        let mut m = PowerRename::new();
        m.start(&mut fx.ctx()).unwrap();
        assert!(m.command("preview", &json!({}), &mut fx.ctx()).is_err());
        assert!(m.command("nope", &Value::Null, &mut fx.ctx()).is_err());
    }
}
