//! Screenshot: take a picture of a dragged-out area, the window in front or every
//! monitor, copy it and save it as a PNG; the manager lists the pictures saved.

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_config::Screenshot as Settings;
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const ID: &str = "screenshot";
/// Emitted to ask the host to show the list of screenshots; the payload's `folder`
/// is where they are saved.
pub const MANAGER_EVENT: &str = "screenshot.manager";
/// Smaller drags are taken as a click, not a selection.
pub const MIN_SIZE: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Region,
    Window,
    Screen,
}

impl Mode {
    fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "region" => Mode::Region,
            "window" => Mode::Window,
            "screen" => Mode::Screen,
            _ => return None,
        })
    }
}

/// The area between where a drag started and ended, within `bounds`.
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

/// `Screenshot 2026-11-17 093012.png`: sorts by time, and Windows' own screenshots in
/// the same folder are named `Screenshot (n).png`, so the two never collide.
pub fn file_name(date: (u16, u16, u16), time: (u16, u16, u16)) -> String {
    let ((y, mo, d), (h, mi, s)) = (date, time);
    format!("Screenshot {y:04}-{mo:02}-{d:02} {h:02}{mi:02}{s:02}.png")
}

/// `name` in `dir`, numbered when a file of that name is already there (two shots in
/// one second).
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("some number is free")
}

#[derive(Default)]
pub struct Screenshot {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: live::Live,
}

impl Screenshot {
    pub fn new() -> Self {
        Self::default()
    }

    fn take(&mut self, mode: Mode, ctx: &mut Ctx) -> Result<()> {
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Screenshot is not running");
        }
        #[cfg(windows)]
        self.live.take(mode, &self.settings, ctx)?;
        #[cfg(not(windows))]
        let _ = (mode, ctx);
        Ok(())
    }

    fn manager(&mut self, ctx: &mut Ctx) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Screenshot is turned off");
        }
        #[cfg(windows)]
        ctx.emit(MANAGER_EVENT, json!({ "folder": live::folder(&self.settings)? }));
        #[cfg(not(windows))]
        let _ = ctx;
        Ok(())
    }

    fn act(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match (action, Mode::parse(action)) {
            ("manager", _) => self.manager(ctx),
            (_, Some(mode)) => self.take(mode, ctx),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn cancel(&mut self) {
        #[cfg(windows)]
        self.live.cancel();
    }
}

impl Module for Screenshot {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        };
        Manifest {
            id: ID.into(),
            name: "Screenshot".into(),
            description: "Take a picture of an area, a window or the whole screen, copied and saved.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::MouseHook,
                Permission::ScreenCapture,
                Permission::ClipboardWrite,
                Permission::FilesystemWrite,
            ],
            commands: vec![
                cmd("region", "Take a picture of an area"),
                cmd("window", "Take a picture of the window in front"),
                cmd("screen", "Take a picture of every monitor"),
                cmd("manager", "Show the screenshots taken"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.screenshot.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        self.cancel();
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        self.cancel();
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.screenshot.clone();
        if !self.settings.enabled {
            self.cancel();
        }
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        self.act(action, ctx)
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        self.live.on_event(event, &self.settings, ctx)?;
        #[cfg(not(windows))]
        let _ = (event, ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        self.act(id, ctx)?;
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "running": self.running && self.settings.enabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::Config;

    #[test]
    fn names_sort_by_time_and_never_overwrite() {
        assert_eq!(file_name((2026, 11, 7), (9, 5, 3)), "Screenshot 2026-11-07 090503.png");
        let dir = tempfile::tempdir().unwrap();
        let name = "Screenshot 2026-11-07 090503.png";
        assert_eq!(free_path(dir.path(), name), dir.path().join(name));
        std::fs::write(dir.path().join(name), b"").unwrap();
        std::fs::write(dir.path().join("Screenshot 2026-11-07 090503 (2).png"), b"").unwrap();
        assert_eq!(
            free_path(dir.path(), name),
            dir.path().join("Screenshot 2026-11-07 090503 (3).png")
        );
    }

    #[test]
    fn a_drag_is_normalized_and_kept_on_screen() {
        let b = Rect::new(-1920, 0, 1920, 1080);
        assert_eq!(selection((10, 300), (-50, 200), b), Some(Rect::new(-50, 200, 10, 300)));
        assert_eq!(selection((5, 5), (7, 90), b), None, "a click, not a drag");
    }

    #[test]
    fn the_shortcuts_are_registered() {
        let c = Config::new();
        let ours: Vec<_> = c
            .shortcuts()
            .into_iter()
            .filter(|(m, _, _)| *m == ID)
            .map(|(_, a, _)| a)
            .collect();
        assert_eq!(
            ours,
            ["region", "window", "screen"],
            "the manager has no shortcut until one is set"
        );
    }
}
