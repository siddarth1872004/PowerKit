//! Quick Notes: a notes window that opens and closes with one shortcut and keeps
//! what was typed. The window lives in the host process; this module asks for it.

use anyhow::{bail, Result};
use powerkit_config::QuickNotes as Settings;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "quick_notes";
/// Emitted to ask the host to show the window, or put it away when it is in front.
pub const OPEN_EVENT: &str = "quick_notes.open";
/// The notes, kept in the data folder.
pub const FILE: &str = "notes.txt";

#[derive(Default)]
pub struct QuickNotes {
    settings: Settings,
    running: bool,
}

impl QuickNotes {
    pub fn new() -> Self {
        Self::default()
    }

    fn open(&self, ctx: &mut Ctx) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Quick Notes is turned off");
        }
        ctx.emit(OPEN_EVENT, Value::Null);
        Ok(())
    }
}

impl Module for QuickNotes {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Quick Notes".into(),
            description: "Jot something down from anywhere; the notes are kept until you delete them.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::FilesystemRead,
                Permission::FilesystemWrite,
            ],
            commands: vec![CommandSpec {
                id: "open".into(),
                title: "Open Quick Notes".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.quick_notes.clone();
        self.running = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.quick_notes.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "open" => self.open(ctx),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "open" => self.open(ctx)?,
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "running": self.running && self.settings.enabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};
    use powerkit_plugin_api::Action;

    #[test]
    fn the_shortcut_asks_the_host_for_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new();
        let paths = Paths::new(dir.path());
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        let mut m = QuickNotes::new();
        assert!(m.on_hotkey("open", &mut ctx).is_err(), "not started");
        m.start(&mut ctx).unwrap();
        m.on_hotkey("open", &mut ctx).unwrap();
        assert!(matches!(&actions[..], [Action::Emit { event, .. }] if event == OPEN_EVENT));
    }
}
