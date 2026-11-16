//! File Explorer add-ons: preview pane handlers for Markdown, source code, SVG, PDF,
//! QOI and G-code, and thumbnails for SVG, PDF, QOI, G-code and STL files. They live in
//! powerkit_preview.dll, which Explorer loads by itself, so they keep working while
//! PowerKit is closed; turning a handler or the utility off unregisters it.

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_config::Config;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, API_VERSION};
use powerkit_preview::{Handler, HANDLERS};
use serde_json::{json, Value};

pub const ID: &str = "file_explorer";

/// Whether the configuration turns `handler` on.
pub fn handler_on(config: &Config, handler: &Handler) -> bool {
    config.file_explorer.enabled && config.file_explorer.handler_on(handler.setting).unwrap_or(false)
}

#[derive(Default)]
pub struct FileExplorer {
    running: bool,
    /// The handlers registered by the last sync.
    registered: Vec<&'static str>,
}

impl FileExplorer {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(windows)]
    fn sync(&mut self, ctx: &mut Ctx) -> Result<()> {
        let config = ctx.config;
        live::sync(&ctx.paths.root, |h| handler_on(config, h))?;
        self.registered = HANDLERS
            .iter()
            .filter(|h| handler_on(config, h))
            .map(|h| h.name)
            .collect();
        Ok(())
    }

    #[cfg(not(windows))]
    fn sync(&mut self, _ctx: &mut Ctx) -> Result<()> {
        bail!("File Explorer add-ons work only on Windows")
    }
}

impl Module for FileExplorer {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "File Explorer add-ons".into(),
            description: "Previews in the preview pane and thumbnails for Markdown, source code, SVG, PDF, QOI, G-code and STL files."
                .into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![],
            commands: vec![CommandSpec {
                id: "register".into(),
                title: "Register the previews and thumbnails again".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.sync(ctx)?;
        self.running = true;
        Ok(())
    }

    /// Called both when the utility is turned off and when PowerKit exits; only the
    /// first unregisters, so previews keep working while PowerKit is closed.
    fn stop(&mut self, ctx: &mut Ctx) {
        self.running = false;
        if ctx.config.file_explorer.enabled {
            return;
        }
        if let Err(e) = self.sync(ctx) {
            ctx.notify("File Explorer add-ons", format!("Could not remove the previews: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        if self.running {
            self.sync(ctx)?;
        }
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        bail!("unknown action {action:?}")
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        if !self.running {
            bail!("File Explorer add-ons are turned off");
        }
        match id {
            "register" => self.sync(ctx)?,
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "running": self.running, "registered": self.registered })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_handler_has_a_setting() {
        let config = Config::default();
        for h in &HANDLERS {
            assert!(config.file_explorer.handler_on(h.setting).is_some(), "{}", h.setting);
        }
        let on: Vec<_> = HANDLERS
            .iter()
            .filter(|h| handler_on(&config, h))
            .map(|h| h.setting)
            .collect();
        assert!(
            on.contains(&"markdown_preview") && !on.contains(&"pdf_preview"),
            "{on:?}"
        );
        let mut off = config.clone();
        off.file_explorer.enabled = false;
        assert!(!HANDLERS.iter().any(|h| handler_on(&off, h)));
    }
}
