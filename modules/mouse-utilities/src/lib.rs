//! Mouse utilities: Find My Mouse (a spotlight on the pointer), Mouse Highlighter
//! (circles on clicks), Crosshairs (lines through the pointer) and Mouse Jump (click a
//! small picture of every screen to move the pointer there).

#[cfg(windows)]
mod live;
pub mod logic;

use anyhow::{bail, Result};
use powerkit_config::MouseUtilities as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
#[cfg(windows)]
use std::{cell::RefCell, rc::Rc};

pub const ID: &str = "mouse_utilities";

#[derive(Default)]
pub struct MouseUtilities {
    settings: Settings,
    running: bool,
    paused: bool,
    /// Turned on and off by shortcut, for this session only.
    highlighter_on: bool,
    crosshairs_on: bool,
    #[cfg(windows)]
    live: Option<Rc<RefCell<live::Live>>>,
    #[cfg(windows)]
    key_hook: Option<powerkit_platform::keyhook::KeyboardHook>,
    #[cfg(windows)]
    mouse_hook: Option<powerkit_platform::mousehook::MouseHook>,
}

impl MouseUtilities {
    pub fn new() -> Self {
        Self::default()
    }

    fn active(&self) -> bool {
        self.running && !self.paused && self.settings.enabled
    }

    /// Bring hooks and overlays in line with the settings and toggles.
    #[cfg(windows)]
    fn apply(&mut self) -> Result<()> {
        use powerkit_platform::keyhook::{KeyDecision, KeyboardHook};
        use powerkit_platform::mousehook::MouseHook;
        if !self.active() {
            self.key_hook = None;
            self.mouse_hook = None;
            if let Some(l) = &self.live {
                l.borrow_mut().clear();
            }
            return Ok(());
        }
        let s = &self.settings;
        let live = Rc::clone(
            self.live
                .get_or_insert_with(|| Rc::new(RefCell::new(live::Live::new(s.clone())))),
        );
        {
            let mut l = live.borrow_mut();
            l.settings = s.clone();
            if !s.find_my_mouse.enabled && l.spotlight_on() {
                l.toggle_spotlight()?;
            }
            if !s.jump.enabled && l.jump_open() {
                l.toggle_jump()?;
            }
            l.set_highlighter(self.highlighter_on && s.highlighter.enabled);
            l.set_crosshairs(self.crosshairs_on && s.crosshairs.enabled)?;
        }

        let want_keys = s.find_my_mouse.enabled;
        let want_mouse = s.find_my_mouse.enabled || (s.highlighter.enabled && self.highlighter_on);
        if !want_keys {
            self.key_hook = None;
        } else if self.key_hook.is_none() {
            let l = Rc::clone(&live);
            self.key_hook = Some(KeyboardHook::install(move |vk, down| {
                if let Ok(mut l) = l.try_borrow_mut() {
                    if let Err(e) = l.on_key(vk, down) {
                        tracing::warn!(error = %e, "Find My Mouse failed");
                    }
                }
                KeyDecision::Pass
            })?);
        }
        if !want_mouse {
            self.mouse_hook = None;
        } else if self.mouse_hook.is_none() {
            let l = Rc::clone(&live);
            self.mouse_hook = Some(MouseHook::install(move |input| {
                if let Ok(mut l) = l.try_borrow_mut() {
                    if let Err(e) = l.on_mouse(input) {
                        tracing::warn!(error = %e, "mouse utilities failed");
                    }
                }
            })?);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply(&mut self) -> Result<()> {
        Ok(())
    }

    /// Run one of the four tools; `jump` and `find_my_mouse` are one-shot, the others
    /// switch on or off.
    fn act(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        if !self.active() {
            bail!("mouse utilities are not running");
        }
        let s = &self.settings;
        let (enabled, name) = match action {
            "find_my_mouse" => (s.find_my_mouse.enabled, "Find My Mouse"),
            "highlighter" => (s.highlighter.enabled, "Mouse Highlighter"),
            "crosshairs" => (s.crosshairs.enabled, "Crosshairs"),
            "jump" => (s.jump.enabled, "Mouse Jump"),
            _ => bail!("unknown action {action:?}"),
        };
        if !enabled {
            bail!("{name} is turned off");
        }
        match action {
            "highlighter" => self.highlighter_on = !self.highlighter_on,
            "crosshairs" => self.crosshairs_on = !self.crosshairs_on,
            _ => {}
        }
        #[cfg(windows)]
        if let Some(l) = &self.live {
            match action {
                "find_my_mouse" => l.borrow_mut().toggle_spotlight()?,
                "jump" => l.borrow_mut().toggle_jump()?,
                _ => {}
            }
        }
        self.apply()?;
        ctx.emit("mouse_utilities.changed", self.status());
        Ok(())
    }
}

impl Module for MouseUtilities {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        };
        Manifest {
            id: ID.into(),
            name: "Mouse utilities".into(),
            description: "Find, highlight and aim the mouse pointer, or jump it across screens.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::KeyboardHook],
            commands: vec![
                cmd("find_my_mouse", "Show or hide the spotlight on the pointer"),
                cmd("highlighter", "Turn click highlighting on or off"),
                cmd("crosshairs", "Turn the crosshairs on or off"),
                cmd("jump", "Pick a spot on any screen to move the pointer to"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.mouse_utilities.clone();
        self.running = true;
        self.paused = false;
        self.apply()
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "could not stop the mouse utilities");
        }
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "could not pause the mouse utilities");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.apply() {
            ctx.notify("Mouse utilities", format!("Could not turn back on: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.mouse_utilities.clone();
        self.apply()?;
        ctx.emit("mouse_utilities.changed", self.status());
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        self.act(action, ctx)
    }

    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        if let Some(l) = &self.live {
            let mut l = l.borrow_mut();
            match event {
                Event::Timer(id) if l.is_timer(*id) => l.tick()?,
                Event::DisplayChanged => l.display_changed(),
                Event::Mouse(_) | Event::Key(_) if l.jump_open() => l.on_capture(event)?,
                _ => {}
            }
        }
        #[cfg(not(windows))]
        let _ = event;
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        self.act(id, ctx)?;
        Ok(self.status())
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let (spotlight, jump) = self.live.as_ref().map_or((false, false), |l| {
            let l = l.borrow();
            (l.spotlight_on(), l.jump_open())
        });
        #[cfg(not(windows))]
        let (spotlight, jump) = (false, false);
        json!({
            "highlighter": self.highlighter_on,
            "crosshairs": self.crosshairs_on,
            "spotlight": spotlight,
            "jump": jump,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};

    #[test]
    fn actions_need_a_running_enabled_tool() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::new();
        config.mouse_utilities.jump.enabled = false;
        let paths = Paths::new(dir.path().join("data"));
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        let mut m = MouseUtilities::new();
        assert!(m.command("crosshairs", &Value::Null, &mut ctx).is_err(), "not running");

        // Running without start(), which would install real hooks on Windows.
        m.settings = config.mouse_utilities.clone();
        m.running = true;
        let err = m.command("jump", &Value::Null, &mut ctx).unwrap_err();
        assert_eq!(err.to_string(), "Mouse Jump is turned off");
        assert!(m.command("nope", &Value::Null, &mut ctx).is_err());
        assert_eq!(m.status()["highlighter"], json!(false));
    }

    #[test]
    fn shortcuts_follow_enabled_tools() {
        let mut config = Config::new();
        let ours = |c: &Config| -> Vec<String> {
            c.shortcuts()
                .into_iter()
                .filter(|(m, _, _)| *m == ID)
                .map(|(_, a, _)| a)
                .collect()
        };
        assert_eq!(ours(&config), ["highlighter", "crosshairs", "jump"]);
        config.mouse_utilities.crosshairs.enabled = false;
        config.mouse_utilities.find_my_mouse.shortcut = "Win+Shift+F".into();
        assert_eq!(ours(&config), ["find_my_mouse", "highlighter", "jump"]);
        assert!(Config::new().sanitize().is_empty(), "default shortcuts must not clash");
    }
}
