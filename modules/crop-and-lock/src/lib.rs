//! Crop and Lock: drag a rectangle over the focused window and keep only that part,
//! either as a live read-only copy (thumbnail) or by moving the window itself into a
//! smaller container that still takes input (reparent).

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_config::CropAndLock as Settings;
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "crop_and_lock";
/// Smaller drags are taken as a click and cancel.
pub const MIN_SIZE: i32 = 8;

/// The rectangle between two drag points, kept inside `bounds`; `None` when too small.
pub fn selection(start: (i32, i32), end: (i32, i32), bounds: Rect) -> Option<Rect> {
    let r = Rect::new(
        start.0.min(end.0),
        start.1.min(end.1),
        start.0.max(end.0),
        start.1.max(end.1),
    );
    let r = r.intersect(&bounds)?;
    (r.width() >= MIN_SIZE && r.height() >= MIN_SIZE).then_some(r)
}

#[derive(Default)]
pub struct CropAndLock {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: live::Live,
}

impl CropAndLock {
    pub fn new() -> Self {
        Self::default()
    }

    fn act(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Crop and Lock is not running");
        }
        #[cfg(windows)]
        let mode = match action {
            "thumbnail" => powerkit_platform::crop::CropMode::Thumbnail,
            "reparent" => powerkit_platform::crop::CropMode::Reparent,
            _ => bail!("unknown action {action:?}"),
        };
        #[cfg(not(windows))]
        if !matches!(action, "thumbnail" | "reparent") {
            bail!("unknown action {action:?}");
        }
        #[cfg(windows)]
        self.live.begin(mode)?;
        ctx.emit("crop_and_lock.changed", self.status());
        Ok(())
    }
}

impl Module for CropAndLock {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        };
        Manifest {
            id: ID.into(),
            name: "Crop and Lock".into(),
            description: "Keep only part of a window, as a live copy or as the window itself.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::WindowManagement,
                Permission::MouseHook,
            ],
            commands: vec![
                cmd("thumbnail", "Crop the focused window into a live copy"),
                cmd("reparent", "Crop the focused window itself"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.crop_and_lock.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        // Reparented windows must go back to the desktop before PowerKit exits.
        #[cfg(windows)]
        self.live.close_all();
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        #[cfg(windows)]
        self.live.cancel();
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.crop_and_lock.clone();
        #[cfg(windows)]
        if !self.settings.enabled {
            self.live.close_all();
        }
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        self.act(action, ctx)
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        if self.live.on_event(event)? {
            ctx.emit("crop_and_lock.changed", self.status());
        }
        #[cfg(not(windows))]
        let _ = (event, ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        self.act(id, ctx)?;
        Ok(self.status())
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let (selecting, windows) = (self.live.selecting(), self.live.open_count());
        #[cfg(not(windows))]
        let (selecting, windows) = (false, 0);
        json!({ "selecting": selecting, "windows": windows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};

    #[test]
    fn selection_is_normalized_and_clamped() {
        let b = Rect::new(100, 100, 500, 400);
        assert_eq!(selection((300, 300), (50, 200), b), Some(Rect::new(100, 200, 300, 300)));
        assert_eq!(selection((200, 200), (204, 260), b), None, "too narrow");
        assert_eq!(selection((0, 0), (50, 50), b), None, "outside");
    }

    #[test]
    fn actions_need_a_running_module() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new();
        let paths = Paths::new(dir.path().join("data"));
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        let mut m = CropAndLock::new();
        assert!(m.command("thumbnail", &Value::Null, &mut ctx).is_err());
        m.start(&mut ctx).unwrap();
        assert!(m.command("nope", &Value::Null, &mut ctx).is_err());
        assert_eq!(m.status(), json!({ "selecting": false, "windows": 0 }));
    }

    #[test]
    fn default_shortcuts_are_registered() {
        let c = Config::new();
        let ours: Vec<_> = c.shortcuts().into_iter().filter(|(m, _, _)| *m == ID).collect();
        assert_eq!(ours.len(), 2);
        assert!(Config::new().sanitize().is_empty());
    }
}
