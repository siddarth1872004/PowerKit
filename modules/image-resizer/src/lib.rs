//! Image Resizer: resize pictures chosen in Explorer to a saved or typed size. The
//! window and the resizing live in the host process; this module registers the
//! Explorer entry and holds the size and naming logic.

pub mod plan;
#[cfg(windows)]
mod shell_menu;

use anyhow::{bail, Result};
use powerkit_config::ImageResizer as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "image_resizer";
/// Emitted to ask the host for the window; the payload's `paths` are the pictures.
pub const OPEN_EVENT: &str = "image_resizer.open";

#[derive(Default)]
pub struct ImageResizer {
    settings: Settings,
    running: bool,
}

impl ImageResizer {
    pub fn new() -> Self {
        Self::default()
    }

    fn apply_settings(&mut self, ctx: &Ctx) {
        self.settings = ctx.config.image_resizer.clone();
        #[cfg(windows)]
        if let Err(e) = shell_menu::sync(self.settings.enabled) {
            tracing::warn!(error = %e, "Explorer context menu not updated");
        }
    }
}

impl Module for ImageResizer {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Image Resizer".into(),
            description: "Resize pictures from the Explorer menu.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::FilesystemRead, Permission::Registry],
            commands: vec![CommandSpec {
                id: "open".into(),
                title: "Resize pictures".into(),
                description: "args: { \"paths\": pictures to resize }".into(),
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
            "open" => {
                if !(self.running && self.settings.enabled) {
                    bail!("Image Resizer is turned off");
                }
                let paths: Vec<&str> = args["paths"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).filter(|p| !p.is_empty()).collect())
                    .unwrap_or_default();
                if paths.is_empty() {
                    bail!("no pictures given");
                }
                ctx.emit(OPEN_EVENT, json!({ "paths": paths }));
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
    fn open_asks_the_host_for_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::new();
        // Switched off so the test leaves the real Explorer menu alone.
        config.image_resizer.enabled = false;
        let paths = Paths::new(dir.path());
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        let mut m = ImageResizer::new();
        m.start(&mut ctx).unwrap();
        let args = json!({ "paths": ["C:\\a.jpg", "", "C:\\b.png"] });
        assert!(m.command("open", &args, &mut ctx).is_err(), "turned off");
        m.settings.enabled = true;
        assert!(m.command("open", &json!({ "paths": [] }), &mut ctx).is_err());
        m.command("open", &args, &mut ctx).unwrap();
        assert!(matches!(&actions[..], [Action::Emit { event, payload }]
            if event == OPEN_EVENT && payload["paths"] == json!(["C:\\a.jpg", "C:\\b.png"])));
    }
}
