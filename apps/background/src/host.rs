//! Module host: owns the configuration and the modules, drives their lifecycle, routes
//! hotkeys/events/commands, and isolates failures (a module that errors or panics is
//! marked `Failed` and the rest keep running).

use powerkit_config::{Config, ConfigStore, LoadReport, LoadSource, LogLevel, Paths};
use powerkit_core::{keys, Event, Lifecycle, Shortcut, State, Transition};
use powerkit_plugin_api::{Action, Ctx, HotkeyBinding, Manifest, Module};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

/// Operating-system services the host needs, abstracted so tests can fake them.
pub trait Backend {
    fn register_hotkey(&mut self, id: i32, shortcut: Shortcut) -> Result<(), String>;
    fn unregister_hotkey(&mut self, id: i32);
    fn set_launch_at_startup(&mut self, enabled: bool) -> Result<(), String>;
    fn set_log_level(&mut self, level: LogLevel);
}

/// Something the host wants the application shell to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Notify { title: String, message: String },
    Event { event: String, payload: Value },
    OpenSettings { page: Option<String> },
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Bound {
    id: i32,
    action: String,
    shortcut: String,
    error: Option<String>,
}

struct Slot {
    module: Box<dyn Module>,
    manifest: Manifest,
    life: Lifecycle,
    error: Option<String>,
    hotkeys: Vec<Bound>,
}

pub struct Host {
    store: ConfigStore,
    config: Config,
    slots: Vec<Slot>,
    backend: Box<dyn Backend>,
    pending: VecDeque<Action>,
    outbox: Vec<Output>,
    warnings: Vec<String>,
    load_source: LoadSource,
    next_hotkey_id: i32,
    started: Instant,
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

impl Host {
    /// Load the configuration and take ownership of the modules. Nothing is started
    /// until [`Host::start`].
    pub fn new(paths: Paths, modules: Vec<Box<dyn Module>>, backend: Box<dyn Backend>) -> Host {
        let store = ConfigStore::new(paths);
        let (config, LoadReport { source, warnings }) = store.load();
        for w in &warnings {
            tracing::warn!("{w}");
        }
        let slots = modules
            .into_iter()
            .map(|module| Slot {
                manifest: module.manifest(),
                module,
                life: Lifecycle::default(),
                error: None,
                hotkeys: Vec::new(),
            })
            .collect();
        Host {
            store,
            config,
            slots,
            backend,
            pending: VecDeque::new(),
            outbox: Vec::new(),
            warnings,
            load_source: source,
            next_hotkey_id: 1,
            started: Instant::now(),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn paths(&self) -> &Paths {
        self.store.paths()
    }

    /// Take everything the shell should act on since the last call.
    pub fn take_outputs(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.outbox)
    }

    pub fn module_state(&self, id: &str) -> Option<State> {
        self.slot_index(id).map(|i| self.slots[i].life.state())
    }

    fn slot_index(&self, id: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.manifest.id == id)
    }

    /// Start every enabled module.
    pub fn start(&mut self) {
        if self.load_source != LoadSource::File {
            // First run or recovery: write a clean file so the state is well defined.
            if let Err(e) = self.store.save(&self.config) {
                self.warnings.push(format!("settings could not be saved: {e}"));
            }
        }
        if let Err(e) = self
            .backend
            .set_launch_at_startup(self.config.general.launch_at_startup)
        {
            tracing::warn!("startup registration: {e}");
        }
        if matches!(self.load_source, LoadSource::Backup { .. } | LoadSource::Reset { .. }) {
            self.outbox.push(Output::Notify {
                title: "PowerKit settings were damaged".into(),
                message: self.warnings.join("\n"),
            });
        }
        self.reconcile();
        self.process_actions();
    }

    /// Stop every module (restoring whatever they changed on the system).
    pub fn shutdown(&mut self) {
        for i in 0..self.slots.len() {
            if self.slots[i].life.is_active() {
                self.stop_module(i);
            }
        }
        self.pending.clear();
    }

    // ---- guarded module calls -------------------------------------------------

    /// Run `f` against module `i`, converting a panic into an `Err` and collecting
    /// the actions the module queued.
    fn guarded<R>(&mut self, i: usize, f: impl FnOnce(&mut dyn Module, &mut Ctx) -> R) -> Result<R, String> {
        let mut actions = Vec::new();
        let result = {
            let slot = &mut self.slots[i];
            let mut ctx = Ctx {
                config: &self.config,
                paths: self.store.paths(),
                actions: &mut actions,
            };
            catch_unwind(AssertUnwindSafe(|| f(slot.module.as_mut(), &mut ctx)))
        };
        self.pending.extend(actions);
        result.map_err(panic_message)
    }

    fn fail(&mut self, i: usize, error: String) {
        let id = self.slots[i].manifest.id.clone();
        tracing::error!(module = %id, "{error}");
        self.unbind_hotkeys(i);
        // Give the module a chance to release OS resources; a second panic is ignored.
        let _ = self.guarded(i, |m, ctx| m.stop(ctx));
        let slot = &mut self.slots[i];
        let _ = slot.life.apply(Transition::Fail);
        slot.error = Some(error.clone());
        self.outbox.push(Output::Notify {
            title: format!("{} stopped working", slot.manifest.name),
            message: error,
        });
        self.emit_status();
    }

    fn start_module(&mut self, i: usize) {
        match self.guarded(i, |m, ctx| m.start(ctx)) {
            Ok(Ok(())) => {
                let slot = &mut self.slots[i];
                slot.error = None;
                let _ = slot.life.apply(Transition::Start);
                tracing::info!(module = %slot.manifest.id, "started");
                self.bind_hotkeys(i);
            }
            Ok(Err(e)) => self.fail(i, format!("could not start: {e:#}")),
            Err(p) => self.fail(i, format!("crashed while starting: {p}")),
        }
    }

    fn stop_module(&mut self, i: usize) {
        self.unbind_hotkeys(i);
        if let Err(p) = self.guarded(i, |m, ctx| m.stop(ctx)) {
            tracing::error!(module = %self.slots[i].manifest.id, "crashed while stopping: {p}");
        }
        let slot = &mut self.slots[i];
        let _ = slot.life.apply(Transition::Stop);
        tracing::info!(module = %slot.manifest.id, "stopped");
    }

    fn desired_bindings(&mut self, i: usize) -> Vec<HotkeyBinding> {
        self.guarded(i, |m, ctx| m.hotkeys(ctx.config)).unwrap_or_default()
    }

    fn bind_hotkeys(&mut self, i: usize) {
        let desired = self.desired_bindings(i);
        let current: Vec<(&str, &str)> = self.slots[i]
            .hotkeys
            .iter()
            .map(|b| (b.action.as_str(), b.shortcut.as_str()))
            .collect();
        if current
            .iter()
            .copied()
            .eq(desired.iter().map(|b| (b.action.as_str(), b.shortcut.as_str())))
        {
            return;
        }
        self.unbind_hotkeys(i);
        for binding in desired {
            let id = self.next_hotkey_id;
            self.next_hotkey_id += 1;
            let error = match binding.shortcut.parse::<Shortcut>() {
                Ok(s) => self.backend.register_hotkey(id, s).err(),
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = &error {
                tracing::warn!(module = %self.slots[i].manifest.id, shortcut = %binding.shortcut, "{e}");
            }
            self.slots[i].hotkeys.push(Bound {
                id,
                action: binding.action,
                shortcut: binding.shortcut,
                error,
            });
        }
    }

    fn unbind_hotkeys(&mut self, i: usize) {
        for b in std::mem::take(&mut self.slots[i].hotkeys) {
            if b.error.is_none() {
                self.backend.unregister_hotkey(b.id);
            }
        }
    }

    /// Bring every module in line with the configuration.
    fn reconcile(&mut self) {
        for i in 0..self.slots.len() {
            let want = self.config.module_enabled(&self.slots[i].manifest.id).unwrap_or(true);
            match (want, self.slots[i].life.state()) {
                (true, State::Created | State::Stopped) => self.start_module(i),
                (true, State::Running | State::Paused) => match self.guarded(i, |m, ctx| m.config_changed(ctx)) {
                    Ok(Ok(())) => {
                        if self.slots[i].life.state() == State::Running {
                            self.bind_hotkeys(i);
                        }
                    }
                    Ok(Err(e)) => self.fail(i, format!("could not apply settings: {e:#}")),
                    Err(p) => self.fail(i, format!("crashed while applying settings: {p}")),
                },
                (false, State::Running | State::Paused) => self.stop_module(i),
                // Disabling a failed module clears the failure; enabling it again retries.
                (false, State::Failed) => {
                    self.slots[i].life = Lifecycle::default();
                    self.slots[i].error = None;
                }
                (true, State::Failed) | (false, State::Created | State::Stopped) => {}
            }
        }
    }

    // ---- configuration --------------------------------------------------------

    /// Validate, persist and apply a new configuration. Returns the repairs made.
    pub fn apply_config(&mut self, mut new: Config) -> Vec<String> {
        let warnings = new.sanitize();
        if new == self.config {
            return warnings;
        }
        if let Err(e) = self.store.save(&new) {
            tracing::error!("saving settings failed: {e}");
            self.outbox.push(Output::Notify {
                title: "PowerKit could not save settings".into(),
                message: e.to_string(),
            });
        }
        if new.general.launch_at_startup != self.config.general.launch_at_startup {
            if let Err(e) = self.backend.set_launch_at_startup(new.general.launch_at_startup) {
                self.outbox.push(Output::Notify {
                    title: "Startup setting not applied".into(),
                    message: e,
                });
            }
        }
        if new.general.log_level != self.config.general.log_level {
            self.backend.set_log_level(new.general.log_level);
        }
        self.config = new;
        self.warnings = warnings.clone();
        self.reconcile();
        self.outbox.push(Output::Event {
            event: "config.changed".into(),
            payload: serde_json::to_value(&self.config).unwrap_or(Value::Null),
        });
        self.emit_status();
        warnings
    }

    fn emit_status(&mut self) {
        let payload = self.status();
        self.outbox.push(Output::Event {
            event: "status.changed".into(),
            payload,
        });
    }

    fn touch_recent(&mut self, id: &str) {
        if self.config.general.recent_modules.first().map(String::as_str) == Some(id) {
            return;
        }
        self.config.touch_recent(id);
        if let Err(e) = self.store.save(&self.config) {
            tracing::warn!("saving settings failed: {e}");
        }
    }

    // ---- actions --------------------------------------------------------------

    fn process_actions(&mut self) {
        // A module reacting to its own actions could loop forever; bound the work.
        let mut budget = 200;
        while let Some(action) = self.pending.pop_front() {
            budget -= 1;
            if budget == 0 {
                tracing::error!("action loop detected; dropping {} queued actions", self.pending.len());
                self.pending.clear();
                return;
            }
            match action {
                Action::UpdateConfig(f) => {
                    let mut new = self.config.clone();
                    if catch_unwind(AssertUnwindSafe(|| f(&mut new))).is_ok() {
                        self.apply_config(new);
                    }
                }
                Action::Invoke { module, command, args } => {
                    if let Err(e) = self.run_command(&module, &command, &args) {
                        tracing::warn!(%module, %command, "command failed: {e}");
                        self.outbox.push(Output::Notify {
                            title: "PowerKit".into(),
                            message: e,
                        });
                    }
                }
                Action::OpenSettings { page } => self.outbox.push(Output::OpenSettings { page }),
                Action::Notify { title, message } => self.outbox.push(Output::Notify { title, message }),
                Action::Emit { event, payload } => self.outbox.push(Output::Event { event, payload }),
            }
        }
    }

    fn run_command(&mut self, module: &str, command: &str, args: &Value) -> Result<Value, String> {
        let i = self
            .slot_index(module)
            .ok_or_else(|| format!("unknown utility {module:?}"))?;
        if self.slots[i].life.state() != State::Running {
            return Err(format!("{} is not running", self.slots[i].manifest.name));
        }
        self.touch_recent(module);
        match self.guarded(i, |m, ctx| m.command(command, args, ctx)) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(format!("{e:#}")),
            Err(p) => {
                self.fail(i, format!("crashed while running {command:?}: {p}"));
                Err(format!("{} crashed", self.slots[i].manifest.name))
            }
        }
    }

    // ---- event routing --------------------------------------------------------

    /// Deliver a platform event. Hotkeys go to the module that registered them; all
    /// other events go to every running module.
    pub fn dispatch(&mut self, event: &Event) {
        if let Event::Hotkey(id) = event {
            let owner = self
                .slots
                .iter()
                .enumerate()
                .find_map(|(i, s)| s.hotkeys.iter().find(|b| b.id == *id).map(|b| (i, b.action.clone())));
            if let Some((i, action)) = owner {
                if self.slots[i].life.state() == State::Running {
                    let id = self.slots[i].manifest.id.clone();
                    self.touch_recent(&id);
                    match self.guarded(i, |m, ctx| m.on_hotkey(&action, ctx)) {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => self.outbox.push(Output::Notify {
                            title: self.slots[i].manifest.name.clone(),
                            message: format!("{e:#}"),
                        }),
                        Err(p) => self.fail(i, format!("crashed handling a shortcut: {p}")),
                    }
                }
            }
        } else {
            for i in 0..self.slots.len() {
                if self.slots[i].life.state() != State::Running {
                    continue;
                }
                match self.guarded(i, |m, ctx| m.on_event(event, ctx)) {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::warn!(module = %self.slots[i].manifest.id, "event handling failed: {e:#}"),
                    Err(p) => self.fail(i, format!("crashed handling an event: {p}")),
                }
            }
        }
        self.process_actions();
    }

    // ---- status and IPC -------------------------------------------------------

    pub fn status(&self) -> Value {
        let modules: Vec<Value> = self
            .slots
            .iter()
            .map(|s| {
                json!({
                    "id": s.manifest.id,
                    "name": s.manifest.name,
                    "description": s.manifest.description,
                    "version": s.manifest.version,
                    "permissions": s.manifest.permissions,
                    "commands": s.manifest.commands,
                    "state": s.life.state(),
                    "enabled": self.config.module_enabled(&s.manifest.id).unwrap_or(true),
                    "error": s.error,
                    "hotkeys": s.hotkeys.iter().map(|b| json!({
                        "action": b.action,
                        "shortcut": b.shortcut,
                        "registered": b.error.is_none(),
                        "error": b.error,
                    })).collect::<Vec<_>>(),
                    "status": catch_unwind(AssertUnwindSafe(|| s.module.status())).unwrap_or(Value::Null),
                })
            })
            .collect();
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": powerkit_ipc::PROTOCOL_VERSION,
            "pid": std::process::id(),
            "uptime_secs": self.started.elapsed().as_secs(),
            "data_dir": self.store.paths().root,
            "warnings": self.warnings,
            "modules": modules,
        })
    }

    /// Handle one IPC request. Every method is listed in `docs/IPC.md`.
    pub fn handle_request(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let str_param = |name: &str| -> Result<String, String> {
            params
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("missing string parameter {name:?}"))
        };
        let bool_param = |name: &str| -> Result<bool, String> {
            params
                .get(name)
                .and_then(Value::as_bool)
                .ok_or_else(|| format!("missing boolean parameter {name:?}"))
        };
        let result = match method {
            "status" => Ok(self.status()),
            "config.get" => serde_json::to_value(&self.config).map_err(|e| e.to_string()),
            "config.set" => {
                let value = params.get("config").cloned().ok_or("missing parameter \"config\"")?;
                let (new, mut warnings) = Config::from_value(value);
                warnings.extend(self.apply_config(new));
                warnings.dedup();
                Ok(json!({ "config": self.config, "warnings": warnings }))
            }
            "config.reset" => {
                let mut new = self.config.clone();
                let d = Config::new();
                match params.get("section").and_then(Value::as_str) {
                    None => new = d,
                    Some("general") => new.general = d.general,
                    Some("always_on_top") => new.always_on_top = d.always_on_top,
                    Some("color_picker") => new.color_picker = d.color_picker,
                    Some("fancy_zones") => new.fancy_zones = d.fancy_zones,
                    Some("power_rename") => new.power_rename = d.power_rename,
                    Some("keyboard_manager") => new.keyboard_manager = d.keyboard_manager,
                    Some("launcher") => new.launcher = d.launcher,
                    Some("awake") => new.awake = d.awake,
                    Some("light_switch") => new.light_switch = d.light_switch,
                    Some("quick_accent") => new.quick_accent = d.quick_accent,
                    Some("mouse_utilities") => new.mouse_utilities = d.mouse_utilities,
                    Some("crop_and_lock") => new.crop_and_lock = d.crop_and_lock,
                    Some("shortcut_guide") => new.shortcut_guide = d.shortcut_guide,
                    Some("grab_and_move") => new.grab_and_move = d.grab_and_move,
                    Some("window_hopper") => new.window_hopper = d.window_hopper,
                    Some("hosts_editor") => new.hosts_editor = d.hosts_editor,
                    Some("environment_variables") => new.environment_variables = d.environment_variables,
                    Some(other) => return Err(format!("unknown section {other:?}")),
                }
                self.apply_config(new);
                Ok(json!({ "config": self.config }))
            }
            "config.export" => {
                let path = str_param("path")?;
                self.store.export(&self.config, std::path::Path::new(&path)).map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            "config.import" => {
                let path = str_param("path")?;
                let (new, mut warnings) =
                    self.store.import(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
                warnings.extend(self.apply_config(new));
                Ok(json!({ "config": self.config, "warnings": warnings }))
            }
            "module.set_enabled" => {
                let id = str_param("id")?;
                let mut new = self.config.clone();
                if !new.set_module_enabled(&id, bool_param("enabled")?) {
                    return Err(format!("unknown utility {id:?}"));
                }
                self.apply_config(new);
                Ok(self.status())
            }
            "module.set_paused" => {
                let id = str_param("id")?;
                let paused = bool_param("paused")?;
                let i = self.slot_index(&id).ok_or_else(|| format!("unknown utility {id:?}"))?;
                match (paused, self.slots[i].life.state()) {
                    (true, State::Running) => {
                        self.unbind_hotkeys(i);
                        match self.guarded(i, |m, ctx| m.pause(ctx)) {
                            Ok(()) => {
                                let _ = self.slots[i].life.apply(Transition::Pause);
                            }
                            Err(p) => self.fail(i, format!("crashed while pausing: {p}")),
                        }
                    }
                    (false, State::Paused) => match self.guarded(i, |m, ctx| m.resume(ctx)) {
                        Ok(()) => {
                            let _ = self.slots[i].life.apply(Transition::Resume);
                            self.bind_hotkeys(i);
                        }
                        Err(p) => self.fail(i, format!("crashed while resuming: {p}")),
                    },
                    _ => {}
                }
                self.emit_status();
                Ok(self.status())
            }
            "module.restart" => {
                let id = str_param("id")?;
                let i = self.slot_index(&id).ok_or_else(|| format!("unknown utility {id:?}"))?;
                if self.slots[i].life.is_active() {
                    self.stop_module(i);
                }
                if self.config.module_enabled(&id).unwrap_or(true) {
                    self.start_module(i);
                }
                self.emit_status();
                Ok(self.status())
            }
            "module.command" => {
                let id = str_param("id")?;
                let command = str_param("command")?;
                let args = params.get("args").cloned().unwrap_or(Value::Null);
                self.run_command(&id, &command, &args)
            }
            "monitors.list" => powerkit_platform::monitor::enumerate()
                .map(|ms| {
                    Value::Array(
                        ms.iter()
                            .map(|m| json!({ "id": m.id, "rect": m.rect, "work_area": m.work_area, "dpi": m.dpi, "primary": m.primary }))
                            .collect(),
                    )
                })
                .map_err(|e| e.to_string()),
            "keys.list" => Ok(Value::Array(keys::all().into_iter().map(|(n, _)| Value::String(n)).collect())),
            "app.open_settings" => {
                let page = params.get("page").and_then(Value::as_str).map(str::to_string);
                self.outbox.push(Output::OpenSettings { page });
                Ok(Value::Null)
            }
            "app.quit" => {
                self.outbox.push(Output::Quit);
                Ok(Value::Null)
            }
            other => Err(format!("unknown method {other:?}")),
        };
        self.process_actions();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_plugin_api::API_VERSION;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct BackendLog {
        registered: Vec<(i32, String)>,
        unregistered: Vec<i32>,
        startup: Vec<bool>,
        taken: Vec<String>,
    }

    struct FakeBackend(Rc<RefCell<BackendLog>>);

    impl Backend for FakeBackend {
        fn register_hotkey(&mut self, id: i32, shortcut: Shortcut) -> Result<(), String> {
            let mut log = self.0.borrow_mut();
            if log.taken.contains(&shortcut.to_string()) {
                return Err("already used".into());
            }
            log.registered.push((id, shortcut.to_string()));
            Ok(())
        }
        fn unregister_hotkey(&mut self, id: i32) {
            self.0.borrow_mut().unregistered.push(id);
        }
        fn set_launch_at_startup(&mut self, enabled: bool) -> Result<(), String> {
            self.0.borrow_mut().startup.push(enabled);
            Ok(())
        }
        fn set_log_level(&mut self, _level: LogLevel) {}
    }

    #[derive(Default)]
    struct Behaviour {
        fail_start: bool,
        panic_on_hotkey: bool,
        panic_on_event: bool,
        calls: Vec<String>,
    }

    /// A module that impersonates one of the built-in ids so the enable flag in the
    /// real configuration applies to it.
    struct Fake {
        id: &'static str,
        b: Rc<RefCell<Behaviour>>,
    }

    impl Module for Fake {
        fn manifest(&self) -> Manifest {
            Manifest {
                id: self.id.into(),
                name: self.id.into(),
                description: String::new(),
                version: "1".into(),
                api_version: API_VERSION,
                permissions: vec![],
                commands: vec![],
                settings: vec![],
            }
        }
        fn start(&mut self, _ctx: &mut Ctx) -> anyhow::Result<()> {
            self.b.borrow_mut().calls.push("start".into());
            if self.b.borrow().fail_start {
                anyhow::bail!("boom");
            }
            Ok(())
        }
        fn stop(&mut self, _ctx: &mut Ctx) {
            self.b.borrow_mut().calls.push("stop".into());
        }
        fn pause(&mut self, _ctx: &mut Ctx) {
            self.b.borrow_mut().calls.push("pause".into());
        }
        fn resume(&mut self, _ctx: &mut Ctx) {
            self.b.borrow_mut().calls.push("resume".into());
        }
        fn config_changed(&mut self, _ctx: &mut Ctx) -> anyhow::Result<()> {
            self.b.borrow_mut().calls.push("config".into());
            Ok(())
        }
        fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> anyhow::Result<()> {
            if self.b.borrow().panic_on_hotkey {
                panic!("hotkey panic");
            }
            self.b.borrow_mut().calls.push(format!("hotkey:{action}"));
            ctx.notify("t", "m");
            Ok(())
        }
        fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> anyhow::Result<()> {
            if self.b.borrow().panic_on_event {
                panic!("event panic");
            }
            self.b.borrow_mut().calls.push("event".into());
            Ok(())
        }
        fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> anyhow::Result<Value> {
            match id {
                "echo" => Ok(args.clone()),
                "set_theme" => {
                    ctx.update_config(|c| c.general.theme = powerkit_config::Theme::Dark);
                    Ok(Value::Null)
                }
                "poke_other" => {
                    ctx.actions.push(Action::Invoke {
                        module: "color_picker".into(),
                        command: "echo".into(),
                        args: json!(1),
                    });
                    Ok(Value::Null)
                }
                _ => anyhow::bail!("unknown command"),
            }
        }
    }

    struct Fx {
        host: Host,
        log: Rc<RefCell<BackendLog>>,
        a: Rc<RefCell<Behaviour>>,
        b: Rc<RefCell<Behaviour>>,
        _dir: tempfile::TempDir,
    }

    fn fixture_with(setup: impl FnOnce(&mut BackendLog, &mut Behaviour, &mut Behaviour)) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let log = Rc::new(RefCell::new(BackendLog::default()));
        let a = Rc::new(RefCell::new(Behaviour::default()));
        let b = Rc::new(RefCell::new(Behaviour::default()));
        setup(&mut log.borrow_mut(), &mut a.borrow_mut(), &mut b.borrow_mut());
        let modules: Vec<Box<dyn Module>> = vec![
            Box::new(Fake {
                id: "always_on_top",
                b: a.clone(),
            }),
            Box::new(Fake {
                id: "color_picker",
                b: b.clone(),
            }),
        ];
        let mut host = Host::new(Paths::new(dir.path()), modules, Box::new(FakeBackend(log.clone())));
        host.start();
        Fx {
            host,
            log,
            a,
            b,
            _dir: dir,
        }
    }

    fn fixture() -> Fx {
        fixture_with(|_, _, _| {})
    }

    fn hotkey_id(fx: &Fx, shortcut: &str) -> i32 {
        fx.log
            .borrow()
            .registered
            .iter()
            .rev()
            .find(|(_, s)| s == shortcut)
            .unwrap()
            .0
    }

    #[test]
    fn starts_modules_and_registers_their_hotkeys() {
        let fx = fixture();
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Running));
        assert_eq!(fx.host.module_state("color_picker"), Some(State::Running));
        let reg: Vec<String> = fx.log.borrow().registered.iter().map(|(_, s)| s.clone()).collect();
        assert_eq!(reg, vec!["Win+Ctrl+T", "Win+Shift+C"]);
        assert!(fx.host.paths().config().exists(), "first run writes the config file");
    }

    #[test]
    fn hotkeys_route_to_the_owning_module_only() {
        let mut fx = fixture();
        let id = hotkey_id(&fx, "Win+Shift+C");
        fx.host.dispatch(&Event::Hotkey(id));
        assert!(fx.b.borrow().calls.contains(&"hotkey:pick".to_string()));
        assert!(!fx.a.borrow().calls.iter().any(|c| c.starts_with("hotkey")));
        assert!(fx.host.take_outputs().contains(&Output::Notify {
            title: "t".into(),
            message: "m".into()
        }));
        assert_eq!(fx.host.config().general.recent_modules, vec!["color_picker"]);
    }

    #[test]
    fn a_failing_start_does_not_affect_other_modules() {
        let fx = fixture_with(|_, a, _| a.fail_start = true);
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Failed));
        assert_eq!(fx.host.module_state("color_picker"), Some(State::Running));
        // Cleanup was attempted and no hotkey of the failed module remains.
        assert_eq!(fx.a.borrow().calls, vec!["start", "stop"]);
        let status = fx.host.status();
        assert!(status["modules"][0]["error"].as_str().unwrap().contains("boom"));
        assert_eq!(status["modules"][0]["hotkeys"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn a_panicking_module_is_isolated() {
        let mut fx = fixture_with(|_, a, b| {
            a.panic_on_hotkey = true;
            b.panic_on_event = true;
        });
        let id = hotkey_id(&fx, "Win+Ctrl+T");
        fx.host.dispatch(&Event::Hotkey(id));
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Failed));
        assert!(
            fx.log.borrow().unregistered.contains(&id),
            "hotkeys of a failed module are released"
        );

        fx.host.dispatch(&Event::DisplayChanged);
        assert_eq!(fx.host.module_state("color_picker"), Some(State::Failed));
        // The host itself keeps working.
        assert!(fx.host.handle_request("status", &Value::Null).is_ok());
        fx.host.dispatch(&Event::DisplayChanged);
    }

    #[test]
    fn failed_module_can_be_retried_by_toggling() {
        let mut fx = fixture_with(|_, a, _| a.fail_start = true);
        fx.a.borrow_mut().fail_start = false;
        let off = json!({"id": "always_on_top", "enabled": false});
        let on = json!({"id": "always_on_top", "enabled": true});
        fx.host.handle_request("module.set_enabled", &off).unwrap();
        fx.host.handle_request("module.set_enabled", &on).unwrap();
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Running));
    }

    #[test]
    fn disabling_stops_and_releases_hotkeys_and_persists() {
        let mut fx = fixture();
        let id = hotkey_id(&fx, "Win+Ctrl+T");
        fx.host
            .handle_request("module.set_enabled", &json!({"id": "always_on_top", "enabled": false}))
            .unwrap();
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Stopped));
        assert!(fx.log.borrow().unregistered.contains(&id));
        // Events no longer reach it.
        fx.host.dispatch(&Event::DisplayChanged);
        assert!(!fx.a.borrow().calls.contains(&"event".to_string()));
        assert!(fx.b.borrow().calls.contains(&"event".to_string()));
        // Persisted: a new host on the same directory starts with it disabled.
        let (reloaded, _) = ConfigStore::new(fx.host.paths().clone()).load();
        assert!(!reloaded.always_on_top.enabled);
    }

    #[test]
    fn changing_a_shortcut_rebinds_only_that_module() {
        let mut fx = fixture();
        let old = hotkey_id(&fx, "Win+Ctrl+T");
        let mut cfg = fx.host.config().clone();
        cfg.always_on_top.shortcut = "Ctrl+Alt+P".into();
        fx.host.handle_request("config.set", &json!({ "config": cfg })).unwrap();
        let log = fx.log.borrow();
        assert!(log.unregistered.contains(&old));
        assert_eq!(log.registered.last().unwrap().1, "Ctrl+Alt+P");
        assert_eq!(log.registered.iter().filter(|(_, s)| s == "Win+Shift+C").count(), 1);
    }

    #[test]
    fn unavailable_shortcut_is_reported_not_fatal() {
        let fx = fixture_with(|log, _, _| log.taken.push("Win+Ctrl+T".into()));
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Running));
        let hk = &fx.host.status()["modules"][0]["hotkeys"][0];
        assert_eq!(hk["registered"], json!(false));
        assert_eq!(hk["error"], json!("already used"));
    }

    #[test]
    fn pause_and_resume() {
        let mut fx = fixture();
        let id = hotkey_id(&fx, "Win+Ctrl+T");
        fx.host
            .handle_request("module.set_paused", &json!({"id": "always_on_top", "paused": true}))
            .unwrap();
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Paused));
        assert!(fx.log.borrow().unregistered.contains(&id));
        fx.host.dispatch(&Event::DisplayChanged);
        assert!(!fx.a.borrow().calls.contains(&"event".to_string()));
        fx.host
            .handle_request("module.set_paused", &json!({"id": "always_on_top", "paused": false}))
            .unwrap();
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Running));
        assert_eq!(fx.a.borrow().calls, vec!["start", "pause", "resume"]);
    }

    #[test]
    fn module_actions_update_config_and_invoke_other_modules() {
        let mut fx = fixture();
        let cmd = |c: &str| json!({"id": "always_on_top", "command": c});
        fx.host.handle_request("module.command", &cmd("set_theme")).unwrap();
        assert_eq!(fx.host.config().general.theme, powerkit_config::Theme::Dark);
        assert!(
            fx.b.borrow().calls.contains(&"config".to_string()),
            "other modules see the new config"
        );
        fx.host.handle_request("module.command", &cmd("poke_other")).unwrap();
        let echo = json!({"id": "color_picker", "command": "echo", "args": {"k": 1}});
        assert_eq!(
            fx.host.handle_request("module.command", &echo).unwrap(),
            json!({"k": 1})
        );
        assert!(fx.host.handle_request("module.command", &cmd("nope")).is_err());
        assert!(fx.host.handle_request("bogus.method", &Value::Null).is_err());
    }

    #[test]
    fn config_set_sanitizes_and_reports() {
        let mut fx = fixture();
        let mut cfg = serde_json::to_value(fx.host.config()).unwrap();
        cfg["always_on_top"]["border_thickness"] = json!(500);
        cfg["general"]["launch_at_startup"] = json!(true);
        let out = fx.host.handle_request("config.set", &json!({ "config": cfg })).unwrap();
        assert_eq!(out["config"]["always_on_top"]["border_thickness"], json!(20));
        assert_eq!(out["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(fx.log.borrow().startup, vec![false, true]);
        let outputs = fx.host.take_outputs();
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Output::Event { event, .. } if event == "config.changed")));
    }

    #[test]
    fn reset_export_import() {
        let mut fx = fixture();
        let mut cfg = fx.host.config().clone();
        cfg.always_on_top.border_thickness = 9;
        cfg.launcher.max_results = 12;
        fx.host.apply_config(cfg);
        let file = fx.host.paths().root.join("export.json");
        fx.host.handle_request("config.export", &json!({"path": file})).unwrap();
        fx.host
            .handle_request("config.reset", &json!({"section": "always_on_top"}))
            .unwrap();
        assert_eq!(fx.host.config().always_on_top.border_thickness, 3);
        assert_eq!(fx.host.config().launcher.max_results, 12);
        fx.host.handle_request("config.reset", &Value::Null).unwrap();
        assert_eq!(fx.host.config().launcher.max_results, 8);
        fx.host.handle_request("config.import", &json!({"path": file})).unwrap();
        assert_eq!(fx.host.config().always_on_top.border_thickness, 9);
        assert!(fx
            .host
            .handle_request("config.import", &json!({"path": "Z:\\nope.json"}))
            .is_err());
    }

    #[test]
    fn shutdown_stops_everything() {
        let mut fx = fixture();
        fx.host.shutdown();
        assert_eq!(fx.host.module_state("always_on_top"), Some(State::Stopped));
        assert_eq!(fx.a.borrow().calls.last().unwrap(), "stop");
        assert_eq!(fx.log.borrow().unregistered.len(), 2);
    }

    #[test]
    fn damaged_config_is_recovered_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), b"{{{{").unwrap();
        let log = Rc::new(RefCell::new(BackendLog::default()));
        let mut host = Host::new(Paths::new(dir.path()), vec![], Box::new(FakeBackend(log)));
        host.start();
        assert!(host
            .take_outputs()
            .iter()
            .any(|o| matches!(o, Output::Notify { title, .. } if title.contains("damaged"))));
        assert_eq!(host.config(), &Config::new());
        assert!(host.paths().config().exists());
    }
}
