//! Keyboard Manager: remap single keys and shortcuts, everywhere or only in one
//! application, organised in profiles of which one is active.

pub mod engine;

use anyhow::{anyhow, bail, Result};
use engine::{Decision, Engine, Remaps, Stroke};
use powerkit_config::KeyboardManager as Settings;
use powerkit_core::{keys, Shortcut};
#[cfg(windows)]
use powerkit_core::{Event, WindowEventKind};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
#[cfg(windows)]
use std::{cell::RefCell, rc::Rc};

pub const ID: &str = "keyboard_manager";

#[derive(Deserialize)]
struct ProfileArgs {
    name: String,
}

#[derive(Deserialize)]
struct SimulateArgs {
    /// A key or shortcut typed as `Ctrl+J`.
    keys: String,
    #[serde(default)]
    app: String,
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))
}

fn key_name(vk: u32) -> String {
    keys::name(vk).unwrap_or_else(|| format!("0x{vk:02X}"))
}

/// Press the modifiers (left-hand keys) and the key of `typed`, release in reverse, and
/// return what applications would receive.
fn simulate(remaps: Remaps, typed: &str, app: &str) -> Result<Vec<Stroke>> {
    let (mods, key) = match typed.parse::<Shortcut>() {
        Ok(s) => (s.mods, s.key),
        Err(_) => (
            powerkit_core::Mods::NONE,
            keys::parse(typed).ok_or_else(|| anyhow!("{typed:?} is not a key or shortcut"))?,
        ),
    };
    let mod_keys: Vec<u32> = [
        (powerkit_core::Mods::CTRL, keys::VK_LCONTROL),
        (powerkit_core::Mods::ALT, keys::VK_LMENU),
        (powerkit_core::Mods::SHIFT, keys::VK_LSHIFT),
        (powerkit_core::Mods::WIN, keys::VK_LWIN),
    ]
    .iter()
    .filter(|(m, _)| mods.contains(*m))
    .map(|(_, v)| *v)
    .collect();
    let presses: Vec<Stroke> = mod_keys
        .iter()
        .map(|m| (*m, true))
        .chain([(key, true), (key, false)])
        .chain(mod_keys.iter().rev().map(|m| (*m, false)))
        .collect();
    let mut e = Engine::new(remaps);
    e.set_app(app);
    let mut out = Vec::new();
    for (vk, down) in presses {
        match e.handle(vk, down) {
            Decision::Pass => out.push((vk, down)),
            Decision::Replace(s) => out.extend(s),
        }
    }
    Ok(out)
}

#[derive(Default)]
pub struct KeyboardManager {
    settings: Settings,
    paused: bool,
    #[cfg(windows)]
    engine: Rc<RefCell<Engine>>,
    #[cfg(windows)]
    hook: Option<powerkit_platform::keyhook::KeyboardHook>,
}

impl KeyboardManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn remaps(&self) -> Remaps {
        self.settings.active().map(Remaps::from_profile).unwrap_or_default()
    }

    /// Bring the hook in line with the settings: installed only while running with a
    /// non-empty active profile.
    #[cfg(windows)]
    fn apply(&mut self) -> Result<()> {
        use powerkit_platform::keyhook::{send_keys, KeyDecision, KeyboardHook};
        let remaps = self.remaps();
        let want = !self.paused && !remaps.is_empty();
        self.engine.borrow_mut().set_remaps(remaps);
        if !want {
            if self.hook.take().is_some() {
                let up = self.engine.borrow_mut().release_all();
                send_keys(&up)?;
            }
            return Ok(());
        }
        if self.hook.is_none() {
            if let Some(exe) = foreground_exe() {
                self.engine.borrow_mut().set_app(&exe);
            }
            let engine = Rc::clone(&self.engine);
            self.hook = Some(KeyboardHook::install(move |vk, down| {
                // Busy means we were re-entered while the module itself holds the
                // engine; letting the key through is the safe answer.
                match engine.try_borrow_mut() {
                    Ok(mut e) => match e.handle(vk, down) {
                        Decision::Pass => KeyDecision::Pass,
                        Decision::Replace(s) => KeyDecision::Replace(s),
                    },
                    Err(_) => KeyDecision::Pass,
                }
            })?);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply(&mut self) -> Result<()> {
        Ok(())
    }

    fn changed(&self, ctx: &mut Ctx) {
        ctx.emit("keyboard_manager.changed", self.status());
    }
}

#[cfg(windows)]
fn foreground_exe() -> Option<String> {
    use powerkit_platform::window;
    let hwnd = window::foreground()?;
    let (pid, _) = window::thread_process(hwnd)?;
    window::process_exe(pid)
}

impl Module for KeyboardManager {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "Keyboard Manager".into(),
            description: "Remap keys and shortcuts, everywhere or in one application.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::KeyboardHook],
            commands: vec![
                cmd("profiles", "List profiles", ""),
                cmd("set_profile", "Switch the active profile", "Args: {name}"),
                cmd("keys", "List key names", ""),
                cmd(
                    "simulate",
                    "Show what a key or shortcut turns into",
                    "Args: {keys, app?}",
                ),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.keyboard_manager.clone();
        self.paused = false;
        self.apply()
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "could not release remapped keys");
        }
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "could not release remapped keys");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.apply() {
            ctx.notify(
                "Keyboard Manager",
                format!("Remapping could not be turned back on: {e:#}"),
            );
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.keyboard_manager.clone();
        self.apply()?;
        self.changed(ctx);
        Ok(())
    }

    #[cfg(windows)]
    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) -> Result<()> {
        if let Event::Window {
            kind: WindowEventKind::Foreground,
            ..
        } = event
        {
            if self.hook.is_some() {
                let exe = foreground_exe().unwrap_or_default();
                if let Ok(mut e) = self.engine.try_borrow_mut() {
                    e.set_app(&exe);
                }
            }
        }
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "profiles" => Ok(self
                .settings
                .profiles
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "active": p.name == self.settings.active_profile,
                        "key_remaps": p.key_remaps.len(),
                        "shortcut_remaps": p.shortcut_remaps.len(),
                    })
                })
                .collect()),
            "set_profile" => {
                let a: ProfileArgs = parse(args)?;
                if !self.settings.profiles.iter().any(|p| p.name == a.name) {
                    bail!("there is no profile {:?}", a.name);
                }
                ctx.update_config(move |c| c.keyboard_manager.active_profile = a.name);
                Ok(Value::Null)
            }
            "keys" => Ok(keys::all().into_iter().map(|(n, _)| Value::String(n)).collect()),
            "simulate" => {
                let a: SimulateArgs = parse(args)?;
                let out = simulate(self.remaps(), &a.keys, &a.app)?;
                Ok(out
                    .into_iter()
                    .map(|(vk, down)| json!({ "key": key_name(vk), "down": down }))
                    .collect())
            }
            _ => bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let active = self.hook.is_some();
        #[cfg(not(windows))]
        let active = false;
        json!({ "profile": self.settings.active_profile, "remapping": active })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, KeyRemap, Paths, Profile, RemapTarget, ShortcutRemap};
    use powerkit_plugin_api::Action;

    struct Fixture {
        config: Config,
        paths: Paths,
        actions: Vec<Action>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut config = Config::new();
            config.keyboard_manager.profiles[0].shortcut_remaps.push(ShortcutRemap {
                from: "Ctrl+J".into(),
                to: RemapTarget::Key("Down".into()),
                app: "code.exe".into(),
            });
            config.keyboard_manager.profiles.push(Profile {
                name: "Gaming".into(),
                key_remaps: vec![KeyRemap {
                    from: "LWin".into(),
                    to: RemapTarget::Disabled,
                }],
                shortcut_remaps: vec![],
            });
            Self {
                config,
                paths: Paths::new(dir.path().join("data")),
                actions: Vec::new(),
                _dir: dir,
            }
        }

        fn ctx(&mut self) -> Ctx<'_> {
            Ctx {
                config: &self.config,
                paths: &self.paths,
                actions: &mut self.actions,
            }
        }

        /// Not started: starting installs a real hook on Windows.
        fn module(&mut self) -> KeyboardManager {
            let mut m = KeyboardManager::new();
            m.settings = self.config.keyboard_manager.clone();
            m
        }

        fn run(&mut self, m: &mut KeyboardManager, id: &str, args: Value) -> Result<Value> {
            m.command(id, &args, &mut self.ctx())
        }
    }

    #[test]
    fn lists_profiles() {
        let mut fx = Fixture::new();
        let mut m = fx.module();
        assert_eq!(
            fx.run(&mut m, "profiles", Value::Null).unwrap(),
            json!([
                { "name": "Default", "active": true, "key_remaps": 0, "shortcut_remaps": 1 },
                { "name": "Gaming", "active": false, "key_remaps": 1, "shortcut_remaps": 0 },
            ])
        );
    }

    #[test]
    fn set_profile_updates_config() {
        let mut fx = Fixture::new();
        let mut m = fx.module();
        assert!(fx.run(&mut m, "set_profile", json!({ "name": "nope" })).is_err());
        fx.run(&mut m, "set_profile", json!({ "name": "Gaming" })).unwrap();
        for a in fx.actions.drain(..) {
            if let Action::UpdateConfig(f) = a {
                f(&mut fx.config);
            }
        }
        assert_eq!(fx.config.keyboard_manager.active_profile, "Gaming");
    }

    #[test]
    fn simulate_respects_the_app_filter() {
        let mut fx = Fixture::new();
        let mut m = fx.module();
        let plain = fx.run(&mut m, "simulate", json!({ "keys": "Ctrl+J" })).unwrap();
        assert_eq!(
            plain,
            json!([
                { "key": "LCtrl", "down": true },
                { "key": "J", "down": true },
                { "key": "J", "down": false },
                { "key": "LCtrl", "down": false },
            ])
        );
        let in_code = fx
            .run(&mut m, "simulate", json!({ "keys": "Ctrl+J", "app": "code.exe" }))
            .unwrap();
        assert_eq!(
            in_code,
            json!([
                { "key": "LCtrl", "down": true },
                { "key": "LCtrl", "down": false },
                { "key": "Down", "down": true },
                { "key": "Down", "down": false },
                { "key": "LCtrl", "down": true },
                { "key": "LCtrl", "down": false },
            ])
        );
        assert!(fx.run(&mut m, "simulate", json!({ "keys": "Nope" })).is_err());
    }

    #[test]
    fn simulate_single_key_uses_the_active_profile() {
        let mut fx = Fixture::new();
        fx.config.keyboard_manager.active_profile = "Gaming".into();
        let mut m = fx.module();
        assert_eq!(
            fx.run(&mut m, "simulate", json!({ "keys": "LWin" })).unwrap(),
            json!([])
        );
        assert_eq!(
            fx.run(&mut m, "simulate", json!({ "keys": "A" })).unwrap(),
            json!([{ "key": "A", "down": true }, { "key": "A", "down": false }])
        );
    }

    #[test]
    fn keys_and_unknown_command() {
        let mut fx = Fixture::new();
        let mut m = fx.module();
        let keys = fx.run(&mut m, "keys", Value::Null).unwrap();
        assert!(keys.as_array().unwrap().contains(&json!("CapsLock")));
        assert!(fx.run(&mut m, "nope", Value::Null).is_err());
        assert_eq!(m.status(), json!({ "profile": "Default", "remapping": false }));
    }
}
