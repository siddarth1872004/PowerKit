//! New+: create files and folders from templates the user keeps in a folder, picked
//! from Explorer's right-click menu. The menu itself is shown by the host process;
//! this module registers the Explorer entry and holds the template logic.

#[cfg(windows)]
mod shell_menu;
pub mod templates;

use anyhow::{bail, Result};
use powerkit_config::NewPlus as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "new_plus";
/// Emitted to ask the host for the template menu; the payload's `folder` is where
/// the copy goes.
pub const OPEN_EVENT: &str = "new_plus.open";

#[derive(Default)]
pub struct NewPlus {
    settings: Settings,
    running: bool,
}

impl NewPlus {
    pub fn new() -> Self {
        Self::default()
    }

    fn apply_settings(&mut self, ctx: &Ctx) {
        self.settings = ctx.config.new_plus.clone();
        #[cfg(windows)]
        if let Err(e) = shell_menu::sync(self.settings.enabled) {
            tracing::warn!(error = %e, "Explorer context menu not updated");
        }
    }
}

impl Module for NewPlus {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "New+".into(),
            description: "Create files and folders from your own templates.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::FilesystemRead,
                Permission::FilesystemWrite,
                Permission::Registry,
            ],
            commands: vec![CommandSpec {
                id: "new".into(),
                title: "New from template".into(),
                description: "args: { \"folder\": where to create it }".into(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.apply_settings(ctx);
        self.running = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
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

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        bail!("unknown action {action:?}")
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "new" => {
                if !(self.running && self.settings.enabled) {
                    bail!("New+ is turned off");
                }
                let Some(folder) = args["folder"].as_str().filter(|f| !f.is_empty()) else {
                    bail!("no folder given");
                };
                ctx.emit(OPEN_EVENT, json!({ "folder": folder }));
            }
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
    fn new_asks_the_host_for_the_menu() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::new();
        // Keep the test from touching the real Explorer menu.
        config.new_plus.enabled = false;
        let paths = Paths::new(dir.path());
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        let mut m = NewPlus::new();
        m.start(&mut ctx).unwrap();
        assert!(
            m.command("new", &json!({ "folder": "C:\\x" }), &mut ctx).is_err(),
            "turned off"
        );
        m.settings.enabled = true;
        assert!(m.command("new", &Value::Null, &mut ctx).is_err(), "no folder");
        m.command("new", &json!({ "folder": "C:\\x" }), &mut ctx).unwrap();
        assert!(matches!(&actions[..], [Action::Emit { event, payload }]
            if event == OPEN_EVENT && payload["folder"] == "C:\\x"));
    }
}
