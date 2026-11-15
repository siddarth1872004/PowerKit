//! Window Hopper: switch between the windows of the focused app with one shortcut,
//! without going through Alt+Tab or the taskbar.

use anyhow::{bail, Result};
use powerkit_config::WindowHopper as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "window_hopper";

/// The window after (or before) `current` among the app's `windows`, wrapping around.
/// Windows are ordered by handle rather than by stacking order: the stacking order
/// changes with every switch, which would only ever flip between the two front windows.
pub fn hop(windows: &[isize], current: isize, forward: bool) -> Option<isize> {
    let mut w: Vec<isize> = windows.iter().copied().filter(|&h| h != current).collect();
    w.sort_unstable();
    w.dedup();
    if forward {
        w.iter().find(|&&h| h > current).or(w.first()).copied()
    } else {
        w.iter().rev().find(|&&h| h < current).or(w.last()).copied()
    }
}

#[derive(Default)]
pub struct WindowHopper {
    settings: Settings,
    running: bool,
    paused: bool,
}

impl WindowHopper {
    pub fn new() -> Self {
        Self::default()
    }

    fn act(&mut self, action: &str) -> Result<()> {
        let forward = match action {
            "next" => true,
            "previous" => false,
            _ => bail!("unknown action {action:?}"),
        };
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Window Hopper is not running");
        }
        #[cfg(windows)]
        switch(&self.settings, forward)?;
        #[cfg(not(windows))]
        let _ = forward;
        Ok(())
    }
}

#[cfg(windows)]
fn switch(settings: &Settings, forward: bool) -> Result<()> {
    use anyhow::anyhow;
    use powerkit_platform::monitor;
    use powerkit_platform::window::{self, Hwnd, Unmanageable, WindowInfo};

    let fg = window::foreground().ok_or_else(|| anyhow!("no window is focused"))?;
    let app = window::info(fg)?.exe;
    if app.is_empty() || settings.excluded_apps.contains(&app) {
        return Ok(());
    }
    let mut windows = Vec::new();
    for h in window::enumerate()? {
        let Ok(i) = window::info(h) else { continue };
        if h == fg || i.exe != app || (i.minimized && !settings.include_minimized) {
            continue;
        }
        let mon = monitor::from_window(h).map(|m| m.rect).unwrap_or_default();
        // Judged as if restored, so minimized windows pass the other checks.
        let probe = WindowInfo { minimized: false, ..i };
        if matches!(probe.manageable(&mon), Ok(()) | Err(Unmanageable::FullScreen)) {
            windows.push(h.0);
        }
    }
    if let Some(target) = hop(&windows, fg.0, forward) {
        window::activate(Hwnd(target))?;
    }
    Ok(())
}

impl Module for WindowHopper {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        };
        Manifest {
            id: ID.into(),
            name: "Window Hopper".into(),
            description: "Switch between the windows of the focused app.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys, Permission::WindowManagement],
            commands: vec![
                cmd("next", "Switch to the app's next window"),
                cmd("previous", "Switch to the app's previous window"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.window_hopper.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.window_hopper.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        self.act(action)
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        self.act(id)?;
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "running": self.running && !self.paused && self.settings.enabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};

    #[test]
    fn hop_cycles_in_handle_order() {
        let w = [30, 10, 20];
        assert_eq!(hop(&w, 10, true), Some(20));
        assert_eq!(hop(&w, 30, true), Some(10), "wraps");
        assert_eq!(hop(&w, 10, false), Some(30), "wraps back");
        assert_eq!(hop(&w, 20, false), Some(10));
        assert_eq!(hop(&[10], 10, true), None, "only window");
        assert_eq!(hop(&[10, 20], 15, true), Some(20), "current not listed");
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
        let mut m = WindowHopper::new();
        assert!(m.command("next", &Value::Null, &mut ctx).is_err());
        m.start(&mut ctx).unwrap();
        assert!(m.command("nope", &Value::Null, &mut ctx).is_err());
        assert_eq!(m.status(), json!({ "running": true }));
    }

    #[test]
    fn default_shortcuts_are_registered() {
        let c = Config::new();
        let ours: Vec<_> = c.shortcuts().into_iter().filter(|(m, _, _)| *m == ID).collect();
        assert_eq!(ours.len(), 2);
        assert!(Config::new().sanitize().is_empty());
    }
}
