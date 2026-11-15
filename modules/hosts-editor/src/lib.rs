//! Hosts File Editor: view, add, change and turn off entries of the Windows hosts
//! file. The window lives in the host process; this module opens it and holds the
//! file format logic.

pub mod hosts;

use anyhow::{bail, Result};
use powerkit_config::HostsEditor as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "hosts_editor";
/// Emitted to ask the host to show the editor window.
pub const OPEN_EVENT: &str = "hosts_editor.open";

#[derive(Default)]
pub struct HostsEditor {
    settings: Settings,
    running: bool,
}

impl HostsEditor {
    pub fn new() -> Self {
        Self::default()
    }

    fn open(&self, ctx: &mut Ctx) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Hosts File Editor is turned off");
        }
        ctx.emit(OPEN_EVENT, Value::Null);
        Ok(())
    }
}

impl Module for HostsEditor {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Hosts File Editor".into(),
            description: "Edit the hosts file that maps host names to IP addresses.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::FilesystemRead,
                Permission::FilesystemWrite,
            ],
            commands: vec![CommandSpec {
                id: "open".into(),
                title: "Open the Hosts File Editor".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.hosts_editor.clone();
        self.running = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.hosts_editor.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "open" => self.open(ctx),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
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
    fn open_asks_the_host_for_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new();
        let paths = Paths::new(dir.path());
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        let mut m = HostsEditor::new();
        assert!(m.command("open", &Value::Null, &mut ctx).is_err(), "not started");
        m.start(&mut ctx).unwrap();
        m.command("open", &Value::Null, &mut ctx).unwrap();
        assert!(matches!(&actions[..], [Action::Emit { event, .. }] if event == OPEN_EVENT));
        assert!(Config::new().sanitize().is_empty());
    }
}
