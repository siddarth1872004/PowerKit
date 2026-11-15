//! Registry Preview: show what a .reg file would write before it is written, edit
//! it, and apply it. The window lives in the host process; this module opens it and
//! holds the file format logic.

pub mod reg;

use anyhow::{bail, Result};
use powerkit_config::RegistryPreview as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "registry_preview";
/// Emitted to ask the host to show the window; the payload's `path` is a file to
/// open, or null.
pub const OPEN_EVENT: &str = "registry_preview.open";

#[derive(Default)]
pub struct RegistryPreview {
    settings: Settings,
    running: bool,
}

impl RegistryPreview {
    pub fn new() -> Self {
        Self::default()
    }

    fn open(&self, path: Option<&str>, ctx: &mut Ctx) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Registry Preview is turned off");
        }
        ctx.emit(OPEN_EVENT, json!({ "path": path }));
        Ok(())
    }
}

/// The Explorer menu entry for .reg files exists only at the Windows 11 top level;
/// without it, .reg files open from the tray or `powerkit --reg`.
fn sync_menu(shown: bool) {
    #[cfg(windows)]
    if let Err(e) = powerkit_menu::show(ID, shown) {
        tracing::warn!(error = %e, "Explorer context menu not updated");
    }
    #[cfg(not(windows))]
    let _ = shown;
}

impl Module for RegistryPreview {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Registry Preview".into(),
            description: "See what a .reg file changes before writing it to the registry.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::FilesystemRead,
                Permission::FilesystemWrite,
                Permission::Registry,
            ],
            commands: vec![CommandSpec {
                id: "open".into(),
                title: "Open Registry Preview".into(),
                description: "args: { \"path\": optional .reg file }".into(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.registry_preview.clone();
        self.running = true;
        sync_menu(self.settings.enabled);
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        // The menu entry launches PowerKit, so it must not outlive the module.
        sync_menu(false);
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.registry_preview.clone();
        sync_menu(self.settings.enabled);
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "open" => self.open(None, ctx),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "open" => self.open(args["path"].as_str(), ctx)?,
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
        let mut m = RegistryPreview::new();
        assert!(m.command("open", &Value::Null, &mut ctx).is_err(), "not started");
        m.start(&mut ctx).unwrap();
        m.command("open", &json!({ "path": "C:\\a.reg" }), &mut ctx).unwrap();
        assert!(matches!(&actions[..], [Action::Emit { event, payload }]
            if event == OPEN_EVENT && payload["path"] == "C:\\a.reg"));
        assert!(Config::new().sanitize().is_empty());
    }
}
