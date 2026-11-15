//! The runtime interface between the host and a utility.
//!
//! Lifecycle (see `powerkit_core::lifecycle`):
//!
//! | Spec step          | Call                                   |
//! |--------------------|----------------------------------------|
//! | Initialize         | the module's constructor               |
//! | Load configuration | [`Module::start`] / [`Module::config_changed`] read `ctx.config` |
//! | Start              | [`Module::start`]                      |
//! | Activate           | [`Module::on_hotkey`] / [`Module::command`] |
//! | Pause / resume     | [`Module::pause`] / [`Module::resume`] |
//! | Stop               | [`Module::stop`]                       |
//! | Cleanup            | [`Module::stop`] releases OS resources; `Drop` frees the rest |
//!
//! All methods are called on the host's main thread. A module never calls another
//! module directly: it queues an [`Action`] and the host routes it.

use crate::Manifest;
use powerkit_config::{Config, Paths};
use powerkit_core::Event;
use serde_json::Value;

/// A request from a module to the host, executed after the current callback returns.
pub enum Action {
    /// Change the configuration; the host validates, persists and redistributes it.
    UpdateConfig(Box<dyn FnOnce(&mut Config)>),
    /// Run a command of another module.
    Invoke {
        module: String,
        command: String,
        args: Value,
    },
    /// Open the settings window, optionally on a specific page.
    OpenSettings { page: Option<String> },
    /// Show a notification to the user.
    Notify { title: String, message: String },
    /// Publish an event to IPC subscribers (the settings UI).
    Emit { event: String, payload: Value },
}

/// What a module can see and request during a callback.
pub struct Ctx<'a> {
    pub config: &'a Config,
    pub paths: &'a Paths,
    pub actions: &'a mut Vec<Action>,
}

impl Ctx<'_> {
    pub fn notify(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.actions.push(Action::Notify {
            title: title.into(),
            message: message.into(),
        });
    }

    pub fn emit(&mut self, event: impl Into<String>, payload: Value) {
        self.actions.push(Action::Emit {
            event: event.into(),
            payload,
        });
    }

    pub fn update_config(&mut self, f: impl FnOnce(&mut Config) + 'static) {
        self.actions.push(Action::UpdateConfig(Box::new(f)));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    /// Passed back to [`Module::on_hotkey`].
    pub action: String,
    /// Shortcut text (`"Win+Ctrl+T"`).
    pub shortcut: String,
}

pub trait Module {
    fn manifest(&self) -> Manifest;

    /// Global shortcuts the host should register while the module runs. The default
    /// takes them from the central configuration.
    fn hotkeys(&self, config: &Config) -> Vec<HotkeyBinding> {
        let id = self.manifest().id;
        config
            .shortcuts()
            .into_iter()
            .filter(|(m, _, _)| *m == id)
            .map(|(_, action, shortcut)| HotkeyBinding { action, shortcut })
            .collect()
    }

    /// Acquire resources and begin operating. An error leaves the module `Failed`
    /// without affecting other modules.
    fn start(&mut self, ctx: &mut Ctx) -> anyhow::Result<()>;

    /// Release every OS resource and undo lasting effects on the system (unpin
    /// windows, remove hooks). Must be safe to call after a failed `start`.
    fn stop(&mut self, ctx: &mut Ctx);

    /// Become inert without releasing resources.
    fn pause(&mut self, _ctx: &mut Ctx) {}

    fn resume(&mut self, _ctx: &mut Ctx) {}

    /// The configuration changed while the module is running.
    fn config_changed(&mut self, _ctx: &mut Ctx) -> anyhow::Result<()> {
        Ok(())
    }

    fn on_hotkey(&mut self, _action: &str, _ctx: &mut Ctx) -> anyhow::Result<()> {
        Ok(())
    }

    /// Platform events (window changes, timers, display changes...). Every running
    /// module sees every event and ignores what it does not need.
    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> anyhow::Result<()> {
        Ok(())
    }

    /// Execute one of the commands listed in the manifest.
    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> anyhow::Result<Value> {
        anyhow::bail!("unknown command {id:?}")
    }

    /// Module-specific live status for the UI.
    fn status(&self) -> Value {
        Value::Null
    }
}
