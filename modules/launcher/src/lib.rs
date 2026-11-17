//! PowerKit Run: press the shortcut, type, press Enter. Finds installed apps, files,
//! PowerKit and Windows settings pages, evaluates calculations and unit conversions,
//! runs user-defined commands, ends processes (`kill …`) and pastes from the clipboard
//! history (`clip …`). Frequently chosen results rank higher.

pub mod calc;
pub mod engine;
pub mod fuzzy;
pub mod index;
pub mod units;

use anyhow::{anyhow, bail, Result};
use engine::{History, Item, Process, Provider, Sources};
use index::Index;
use powerkit_config::{Launcher as Settings, StateFile};
use powerkit_core::{Event, PopupEvent};
use powerkit_plugin_api::{Action as HostAction, CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Instant;

pub const ID: &str = "launcher";
const HISTORY_FILE: &str = "launcher_history";
#[cfg(windows)]
/// The index is rebuilt in the background when the popup opens and it is older.
const REINDEX_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);
#[cfg_attr(not(windows), allow(dead_code))]
/// Larger clipboard texts are not kept in the history.
const MAX_CLIP_BYTES: usize = 64 * 1024;

const MODULE_NAMES: [(&str, &str); 6] = [
    ("fancy_zones", "FancyZones"),
    ("power_rename", "PowerRename"),
    ("color_picker", "Color Picker"),
    ("keyboard_manager", "Keyboard Manager"),
    ("always_on_top", "Always On Top"),
    ("launcher", "PowerKit Run"),
];

#[cfg(windows)]
mod os {
    use anyhow::{anyhow, Result};
    use powerkit_platform::{clipboard, event_loop, process, shell};

    pub fn open(target: &str, args: Option<&str>) -> Result<()> {
        shell::open(target, args).map_err(|e| anyhow!(e))
    }
    pub fn copy(text: &str) -> Result<()> {
        clipboard::set_text(text).map_err(|e| anyhow!(e))
    }
    pub fn kill(pid: u32) -> Result<()> {
        process::terminate(pid).map_err(|e| anyhow!(e))
    }
    pub fn processes() -> Result<Vec<(u32, String)>> {
        process::list().map_err(|e| anyhow!(e))
    }
    pub fn clipboard_text() -> Result<Option<String>> {
        clipboard::get_text().map_err(|e| anyhow!(e))
    }
    pub fn listen_clipboard(on: bool) -> Result<()> {
        event_loop::listen_clipboard(on).map_err(|e| anyhow!(e))
    }
}

#[cfg(not(windows))]
mod os {
    use anyhow::{bail, Result};

    pub fn open(_target: &str, _args: Option<&str>) -> Result<()> {
        bail!("opening programs is only available on Windows")
    }
    pub fn copy(_text: &str) -> Result<()> {
        bail!("the clipboard is only available on Windows")
    }
    pub fn kill(_pid: u32) -> Result<()> {
        bail!("ending processes is only available on Windows")
    }
    pub fn processes() -> Result<Vec<(u32, String)>> {
        Ok(Vec::new())
    }
}

#[derive(Deserialize)]
struct QueryArgs {
    query: String,
    #[serde(default)]
    index: usize,
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))
}

#[derive(Default)]
pub struct Launcher {
    settings: Settings,
    history: History,
    /// Newest first.
    clipboard: Vec<String>,
    index: Index,
    pending: Option<mpsc::Receiver<Index>>,
    indexed_at: Option<Instant>,
    processes: Vec<Process>,
    results: Vec<Item>,
    #[cfg(windows)]
    popup: Option<powerkit_platform::popup::Popup>,
    #[cfg(windows)]
    listening: bool,
}

impl Launcher {
    pub fn new() -> Self {
        Self::default()
    }

    fn history_file(ctx: &Ctx) -> StateFile {
        StateFile::new(ctx.paths, HISTORY_FILE)
    }

    /// Rebuild the app and file index on a background thread.
    fn reindex(&mut self) {
        let p = &self.settings.providers;
        let app_roots = if p.apps { index::default_app_roots() } else { Vec::new() };
        let (file_roots, limit) = if p.files {
            (
                index::file_roots(&self.settings.file_roots),
                self.settings.max_indexed_files as usize,
            )
        } else {
            (Vec::new(), 0)
        };
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("launcher-index".into())
            .spawn(move || {
                let _ = tx.send(index::build(&app_roots, &file_roots, limit));
            });
        match spawned {
            Ok(_) => {
                self.pending = Some(rx);
                self.indexed_at = Some(Instant::now());
            }
            Err(e) => tracing::error!("could not start the launcher index thread: {e}"),
        }
    }

    /// Take a finished background index, if any.
    fn collect_index(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok(idx) => {
                for e in idx.errors.iter().take(5) {
                    tracing::debug!("launcher index: {e}");
                }
                self.index = idx;
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                tracing::error!("the launcher index thread stopped without a result");
                self.pending = None;
            }
        }
    }

    fn refresh_processes(&mut self) {
        match os::processes() {
            Ok(list) => {
                let mut procs: Vec<Process> = list.into_iter().map(|(pid, name)| Process { pid, name }).collect();
                procs.sort_by(|a, b| {
                    a.name
                        .to_lowercase()
                        .cmp(&b.name.to_lowercase())
                        .then(a.pid.cmp(&b.pid))
                });
                self.processes = procs;
            }
            Err(e) => tracing::warn!("listing processes failed: {e:#}"),
        }
    }

    fn search(&mut self, query: &str) -> &[Item] {
        self.collect_index();
        if self.settings.providers.processes && query.trim_start().to_ascii_lowercase().starts_with("kill") {
            self.refresh_processes();
        }
        let src = Sources {
            apps: &self.index.apps,
            files: &self.index.files,
            clipboard: &self.clipboard,
            processes: &self.processes,
            modules: &MODULE_NAMES,
        };
        self.results = engine::search(query, &self.settings, &src, &self.history);
        &self.results
    }

    fn run(&mut self, item: &Item, ctx: &mut Ctx) -> Result<()> {
        match &item.action {
            engine::Action::Open { target, args } => os::open(target, args.as_deref())?,
            engine::Action::Copy { text } => os::copy(text)?,
            engine::Action::Kill { pid, name } => {
                os::kill(*pid).map_err(|e| anyhow!("Could not end {name}: {e:#}"))?;
                self.processes.retain(|p| p.pid != *pid);
            }
            engine::Action::Settings { page } => ctx.actions.push(HostAction::OpenSettings {
                page: Some(page.clone()),
            }),
        }
        if self.settings.remember_history && item.provider != Provider::Clipboard {
            self.history.record(&item.key);
            if let Err(e) = Self::history_file(ctx).save(&self.history) {
                tracing::warn!("launcher history not saved: {e}");
            }
        }
        Ok(())
    }

    fn run_result(&mut self, index: usize, ctx: &mut Ctx) -> Result<()> {
        let item = self
            .results
            .get(index)
            .cloned()
            .ok_or_else(|| anyhow!("there is no result {index}"))?;
        self.results.clear();
        self.run(&item, ctx)
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn remember_clipboard(&mut self, text: String) {
        let limit = self.settings.clipboard_history_size as usize;
        if limit == 0 || text.trim().is_empty() || text.len() > MAX_CLIP_BYTES {
            return;
        }
        self.clipboard.retain(|t| *t != text);
        self.clipboard.insert(0, text);
        self.clipboard.truncate(limit);
    }

    fn wants_clipboard(&self) -> bool {
        self.settings.providers.clipboard && self.settings.clipboard_history_size > 0
    }

    #[cfg(windows)]
    fn sync_clipboard_listener(&mut self) {
        let want = self.wants_clipboard();
        if want != self.listening {
            match os::listen_clipboard(want) {
                Ok(()) => self.listening = want,
                Err(e) => tracing::warn!("clipboard history: {e:#}"),
            }
        }
        if !want {
            self.clipboard.clear();
        }
    }

    #[cfg(not(windows))]
    fn sync_clipboard_listener(&mut self) {
        if !self.wants_clipboard() {
            self.clipboard.clear();
        }
    }

    fn apply_settings(&mut self, ctx: &Ctx) {
        let old = std::mem::replace(&mut self.settings, ctx.config.launcher.clone());
        let s = &self.settings;
        let reindex = old.file_roots != s.file_roots
            || old.max_indexed_files != s.max_indexed_files
            || old.providers.apps != s.providers.apps
            || old.providers.files != s.providers.files;
        self.clipboard.truncate(s.clipboard_history_size as usize);
        if reindex {
            self.reindex();
        }
        self.sync_clipboard_listener();
    }

    #[cfg(windows)]
    fn visible(&self) -> bool {
        self.popup.as_ref().is_some_and(|p| p.is_visible())
    }

    #[cfg(not(windows))]
    fn visible(&self) -> bool {
        false
    }

    #[cfg(windows)]
    fn toggle(&mut self) -> Result<()> {
        if self.visible() {
            self.hide();
            return Ok(());
        }
        if self.indexed_at.is_none_or(|t| t.elapsed() > REINDEX_AFTER) && self.pending.is_none() {
            self.reindex();
        }
        if self.popup.is_none() {
            self.popup = Some(powerkit_platform::popup::Popup::new(ID).map_err(|e| anyhow!(e))?);
        }
        self.results.clear();
        self.popup
            .as_mut()
            .map(|p| p.show())
            .transpose()
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    #[cfg(not(windows))]
    fn toggle(&mut self) -> Result<()> {
        bail!("the launcher window is only available on Windows")
    }

    fn hide(&mut self) {
        #[cfg(windows)]
        if let Some(p) = self.popup.as_mut() {
            p.hide();
        }
        self.results.clear();
    }

    #[cfg(windows)]
    fn show_results(&mut self) -> Result<()> {
        use powerkit_platform::popup::Row;
        let rows = self
            .results
            .iter()
            .map(|i| Row {
                title: i.title.clone(),
                subtitle: i.subtitle.clone(),
            })
            .collect();
        match self.popup.as_mut() {
            Some(p) => p.set_rows(rows).map_err(|e| anyhow!(e)),
            None => Ok(()),
        }
    }

    #[cfg(not(windows))]
    fn show_results(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Module for Launcher {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "PowerKit Run".into(),
            description: "Launch apps, find files, calculate and run commands from one search box.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::FilesystemRead,
                Permission::ProcessLaunch,
                Permission::ProcessQuery,
                Permission::ClipboardRead,
                Permission::ClipboardWrite,
            ],
            commands: vec![
                cmd("toggle", "Open or close the launcher", ""),
                cmd("query", "Search", "Args: {query}"),
                cmd("run", "Run a search result", "Args: {query, index?}"),
                cmd("reindex", "Rebuild the app and file index", ""),
                cmd("clear_history", "Forget which results were chosen", ""),
                cmd("clear_clipboard", "Forget the clipboard history", ""),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.launcher.clone();
        self.history = Self::history_file(ctx).load();
        self.reindex();
        self.sync_clipboard_listener();
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.hide();
        #[cfg(windows)]
        {
            self.popup = None;
            if std::mem::take(&mut self.listening) {
                if let Err(e) = os::listen_clipboard(false) {
                    tracing::warn!("clipboard history: {e:#}");
                }
            }
        }
        self.pending = None;
        self.clipboard.clear();
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.hide();
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.apply_settings(ctx);
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        match action {
            "toggle" => self.toggle(),
            _ => bail!("unknown shortcut action {action:?}"),
        }
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        match event {
            #[cfg(windows)]
            Event::ClipboardChanged if self.listening => {
                if let Some(text) = os::clipboard_text()? {
                    self.remember_clipboard(text);
                }
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Query(q),
            } => {
                self.search(q);
                self.show_results()?;
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Activate(i),
            } => {
                if let Err(e) = self.run_result(*i, ctx) {
                    ctx.notify("PowerKit Run", format!("{e:#}"));
                    return Err(e);
                }
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Dismiss,
            } => self.results.clear(),
            _ => {}
        }
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "toggle" => {
                self.toggle()?;
                Ok(Value::Null)
            }
            "query" => {
                let a: QueryArgs = parse(args)?;
                Ok(serde_json::to_value(self.search(&a.query))?)
            }
            "run" => {
                let a: QueryArgs = parse(args)?;
                self.search(&a.query);
                self.run_result(a.index, ctx)?;
                Ok(Value::Null)
            }
            "reindex" => {
                self.reindex();
                Ok(Value::Null)
            }
            "clear_history" => {
                self.history = History::default();
                Self::history_file(ctx)
                    .save(&self.history)
                    .map_err(|e| anyhow!("Launcher history not saved: {e}"))?;
                Ok(Value::Null)
            }
            "clear_clipboard" => {
                self.clipboard.clear();
                Ok(Value::Null)
            }
            _ => bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        json!({
            "visible": self.visible(),
            "indexing": self.pending.is_some(),
            "apps": self.index.apps.len(),
            "files": self.index.files.len(),
            "clipboard": self.clipboard.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, CustomCommand, Paths};

    struct Fixture {
        config: Config,
        paths: Paths,
        actions: Vec<HostAction>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut config = Config::new();
            // Keep tests off the real Start Menu and user folders.
            config.launcher.providers.apps = false;
            config.launcher.providers.files = false;
            Self {
                config,
                paths: Paths::new(dir.path().join("data")),
                actions: Vec::new(),
                _dir: dir,
            }
        }

        fn ctx(&mut self) -> Ctx<'_> {
            Ctx {
                config: &self.config,
                paths: &self.paths,
                actions: &mut self.actions,
            }
        }

        fn started(&mut self) -> Launcher {
            let mut m = Launcher::new();
            m.start(&mut self.ctx()).unwrap();
            m
        }

        fn run(&mut self, m: &mut Launcher, id: &str, args: Value) -> Result<Value> {
            m.command(id, &args, &mut self.ctx())
        }
    }

    fn wait_for_index(m: &mut Launcher) {
        for _ in 0..500 {
            m.collect_index();
            if m.pending.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("index did not finish");
    }

    #[test]
    fn query_returns_ranked_results() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        let out = fx.run(&mut m, "query", json!({ "query": "6*7" })).unwrap();
        assert_eq!(out[0]["title"], "= 42");
        assert_eq!(out[0]["action"], json!({ "type": "copy", "text": "42" }));
        assert_eq!(out[0]["provider"], "calculator");
    }

    #[test]
    fn settings_results_open_settings_and_are_remembered() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        fx.run(&mut m, "run", json!({ "query": "color picker" })).unwrap();
        assert!(fx
            .actions
            .iter()
            .any(|a| matches!(a, HostAction::OpenSettings { page: Some(p) } if p == "color_picker")));
        // The choice survives a restart through the history file.
        let mut again = fx.started();
        assert_eq!(again.history.counts.get("pk:color_picker"), Some(&1));
        fx.run(&mut again, "clear_history", Value::Null).unwrap();
        assert!(fx.started().history.counts.is_empty());
    }

    #[test]
    fn running_a_missing_result_fails() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        let e = fx.run(&mut m, "run", json!({ "query": "", "index": 0 })).unwrap_err();
        assert!(e.to_string().contains("no result"), "{e}");
        assert!(fx.run(&mut m, "query", json!({})).is_err());
        assert!(fx.run(&mut m, "nope", Value::Null).is_err());
        assert!(m.on_hotkey("nope", &mut fx.ctx()).is_err());
    }

    #[test]
    fn popup_events_drive_search_and_activation() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        m.on_event(
            &Event::Popup {
                owner: ID,
                event: PopupEvent::Query("power run".into()),
            },
            &mut fx.ctx(),
        )
        .unwrap();
        assert_eq!(m.results[0].key, "pk:launcher");
        m.on_event(
            &Event::Popup {
                owner: ID,
                event: PopupEvent::Activate(0),
            },
            &mut fx.ctx(),
        )
        .unwrap();
        assert!(m.results.is_empty());
        assert!(fx
            .actions
            .iter()
            .any(|a| matches!(a, HostAction::OpenSettings { page: Some(p) } if p == "launcher")));
        // Activating after the results were consumed is reported, not ignored.
        let e = m.on_event(
            &Event::Popup {
                owner: ID,
                event: PopupEvent::Activate(0),
            },
            &mut fx.ctx(),
        );
        assert!(e.is_err());
        assert!(fx.actions.iter().any(|a| matches!(a, HostAction::Notify { .. })));
    }

    #[test]
    fn clipboard_history_is_deduplicated_and_bounded() {
        let mut fx = Fixture::new();
        fx.config.launcher.clipboard_history_size = 2;
        let mut m = fx.started();
        for t in ["a", "b", "a", " ", "c"] {
            m.remember_clipboard(t.into());
        }
        m.remember_clipboard("x".repeat(MAX_CLIP_BYTES + 1));
        assert_eq!(m.clipboard, ["c", "a"]);
        let out = fx.run(&mut m, "query", json!({ "query": "clip" })).unwrap();
        assert_eq!(out.as_array().unwrap().len(), 2);
        fx.config.launcher.providers.clipboard = false;
        m.config_changed(&mut fx.ctx()).unwrap();
        assert!(m.clipboard.is_empty());
    }

    #[test]
    fn files_are_indexed_in_the_background_and_reindexed_on_root_change() {
        let mut fx = Fixture::new();
        let root = fx._dir.path().join("docs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("quarterly report.xlsx"), b"").unwrap();
        fx.config.launcher.providers.files = true;
        fx.config.launcher.file_roots = vec![root.to_string_lossy().into_owned()];
        let mut m = fx.started();
        wait_for_index(&mut m);
        let out = fx.run(&mut m, "query", json!({ "query": "quart" })).unwrap();
        assert_eq!(out[0]["title"], "quarterly report.xlsx");
        assert_eq!(m.status()["files"], 1);

        let other = fx._dir.path().join("empty");
        std::fs::create_dir_all(&other).unwrap();
        fx.config.launcher.file_roots = vec![other.to_string_lossy().into_owned()];
        m.config_changed(&mut fx.ctx()).unwrap();
        wait_for_index(&mut m);
        assert_eq!(m.status()["files"], 0);
    }

    #[test]
    fn custom_command_from_config() {
        let mut fx = Fixture::new();
        fx.config.launcher.custom_commands = vec![CustomCommand {
            keyword: "w".into(),
            title: "Wiki".into(),
            program: "https://en.wikipedia.org/wiki/{query}".into(),
            args: String::new(),
        }];
        let mut m = fx.started();
        let out = fx.run(&mut m, "query", json!({ "query": "w Rust lang" })).unwrap();
        assert_eq!(out[0]["action"]["target"], "https://en.wikipedia.org/wiki/Rust%20lang");
    }
}
