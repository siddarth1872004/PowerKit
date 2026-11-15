//! Shortcut Guide: hold the Windows key (or press a shortcut) to see the keyboard
//! shortcuts of Windows, PowerKit and the focused app, with numbers on the taskbar
//! buttons that Win+1..Win+0 open.

#[cfg(windows)]
mod live;
pub mod logic;

use anyhow::{bail, Result};
use powerkit_config::ShortcutGuide as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
#[cfg(windows)]
use std::{cell::RefCell, rc::Rc};

pub const ID: &str = "shortcut_guide";

#[derive(Default)]
pub struct ShortcutGuide {
    settings: Settings,
    powerkit: Option<logic::Section>,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: Option<Rc<RefCell<live::Live>>>,
    #[cfg(windows)]
    key_hook: Option<powerkit_platform::keyhook::KeyboardHook>,
}

impl ShortcutGuide {
    pub fn new() -> Self {
        Self::default()
    }

    fn active(&self) -> bool {
        self.running && !self.paused && self.settings.enabled
    }

    fn load(&mut self, ctx: &Ctx) {
        self.settings = ctx.config.shortcut_guide.clone();
        self.powerkit = Some(logic::powerkit_section(&ctx.config.shortcuts()));
    }

    #[cfg(windows)]
    fn apply(&mut self) -> Result<()> {
        use powerkit_platform::keyhook::{KeyDecision, KeyboardHook};
        if !self.active() {
            self.key_hook = None;
            self.live = None;
            return Ok(());
        }
        let powerkit = self.powerkit.clone().unwrap_or_else(|| logic::powerkit_section(&[]));
        let live =
            Rc::clone(self.live.get_or_insert_with(|| {
                Rc::new(RefCell::new(live::Live::new(self.settings.clone(), powerkit.clone())))
            }));
        {
            let mut l = live.borrow_mut();
            l.settings = self.settings.clone();
            l.powerkit = powerkit;
        }
        // Watched even with holding the Windows key off: Esc closes a guide the shortcut
        // opened.
        if self.key_hook.is_none() {
            self.key_hook = Some(KeyboardHook::install(move |vk, down| match live.try_borrow_mut() {
                Ok(mut l) => l.on_key(vk, down),
                Err(_) => KeyDecision::Pass,
            })?);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply(&mut self) -> Result<()> {
        Ok(())
    }

    fn shown(&self) -> bool {
        #[cfg(windows)]
        return self.live.as_ref().is_some_and(|l| l.borrow().is_shown());
        #[cfg(not(windows))]
        false
    }
}

impl Module for ShortcutGuide {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Shortcut Guide".into(),
            description: "Hold the Windows key to see the shortcuts that work right now.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys, Permission::KeyboardHook],
            commands: vec![CommandSpec {
                id: "toggle".into(),
                title: "Show or hide the shortcut guide".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.load(ctx);
        self.running = true;
        self.paused = false;
        self.apply()
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "Shortcut Guide did not stop cleanly");
        }
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "Shortcut Guide did not pause cleanly");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.apply() {
            ctx.notify("Shortcut Guide", format!("Could not turn back on: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.load(ctx);
        self.apply()
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        self.command(action, &Value::Null, ctx).map(|_| ())
    }

    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        if let (Event::Timer(id), Some(l)) = (event, &self.live) {
            let mut l = l.borrow_mut();
            if l.is_timer(*id) {
                l.tick()?;
            }
        }
        #[cfg(not(windows))]
        let _ = event;
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        if id != "toggle" {
            bail!("unknown command {id:?}");
        }
        if !self.active() {
            bail!("Shortcut Guide is not running");
        }
        #[cfg(windows)]
        if let Some(l) = &self.live {
            l.borrow_mut().toggle()?;
        }
        ctx.emit("shortcut_guide.changed", self.status());
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "shown": self.shown() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};

    #[test]
    fn commands_need_a_running_module() {
        let dir = tempfile::tempdir().unwrap();
        // Disabled, so starting installs no keyboard hook (that needs an event loop).
        let mut config = Config::new();
        config.shortcut_guide.enabled = false;
        let paths = Paths::new(dir.path().join("data"));
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        let mut m = ShortcutGuide::new();
        assert!(m.command("toggle", &Value::Null, &mut ctx).is_err());
        m.start(&mut ctx).unwrap();
        assert!(m.command("nope", &Value::Null, &mut ctx).is_err());
        assert!(m.command("toggle", &Value::Null, &mut ctx).is_err(), "disabled");
        assert_eq!(m.status(), json!({ "shown": false }));
        let pk = m.powerkit.as_ref().unwrap();
        assert!(pk.items.iter().any(|i| i.what == "Pick a color"));
        assert!(!pk.items.iter().any(|i| i.what == "This guide"), "disabled");
    }

    #[test]
    fn default_shortcut_is_registered() {
        let c = Config::new();
        let ours: Vec<_> = c.shortcuts().into_iter().filter(|(m, _, _)| *m == ID).collect();
        assert_eq!(ours, [(ID, "toggle".to_string(), "Win+Shift+Slash".to_string())]);
        assert!(Config::new().sanitize().is_empty());
    }
}
