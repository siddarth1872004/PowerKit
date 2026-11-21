//! Quick Accent: hold a letter and press Space or an arrow key to pick an accented
//! version of it (or a symbol) from a bar; releasing the letter types the choice.

pub mod engine;
pub mod tables;

use anyhow::{anyhow, bail, Result};
use engine::Engine;
use powerkit_config::QuickAccent as Settings;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

pub const ID: &str = "quick_accent";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccentsArgs {
    /// The character the key types on a US layout, such as "a" or "-".
    key: String,
    #[serde(default)]
    upper: bool,
}

#[cfg(windows)]
mod ui {
    use super::engine::{Effect, ERASE};
    use powerkit_platform::accent_bar::{AccentBar, AccentView};
    use powerkit_platform::{keyhook, monitor, window};

    #[derive(Default)]
    pub struct Ui {
        bar: Option<AccentBar>,
        view: Option<AccentView>,
    }

    fn target_monitor() -> Option<monitor::Monitor> {
        if let Some(m) = window::foreground().and_then(monitor::from_window) {
            return Some(m);
        }
        let (x, y) = powerkit_platform::input::cursor_pos().ok()?;
        monitor::from_point(x, y)
    }

    impl Ui {
        pub fn apply(&mut self, effect: Effect) -> powerkit_platform::Result<()> {
            match effect {
                Effect::None => Ok(()),
                Effect::Show { chars, selected } => {
                    let Some(m) = target_monitor() else {
                        return Err(powerkit_platform::Error::Other("no monitor to show the bar on".into()));
                    };
                    self.view = Some(AccentView {
                        chars,
                        selected,
                        area: m.work_area,
                        dpi: m.dpi,
                    });
                    self.draw()
                }
                Effect::Select(i) => {
                    if let Some(v) = &mut self.view {
                        v.selected = i;
                    }
                    self.draw()
                }
                Effect::Commit(text) => {
                    self.hide();
                    keyhook::send_keys(&ERASE)?;
                    keyhook::send_text(&text)
                }
                Effect::Close => {
                    self.hide();
                    Ok(())
                }
            }
        }

        fn draw(&mut self) -> powerkit_platform::Result<()> {
            let Some(view) = &self.view else { return Ok(()) };
            if self.bar.is_none() {
                self.bar = Some(AccentBar::new()?);
            }
            self.bar.as_mut().map_or(Ok(()), |b| b.show(view))
        }

        pub fn hide(&mut self) {
            self.view = None;
            if let Some(b) = &mut self.bar {
                b.hide();
            }
        }
    }
}

#[derive(Default)]
pub struct QuickAccent {
    settings: Settings,
    running: bool,
    paused: bool,
    engine: Rc<RefCell<Engine>>,
    #[cfg(windows)]
    excluded: Rc<RefCell<Vec<String>>>,
    #[cfg(windows)]
    skip_in_game_mode: Rc<std::cell::Cell<bool>>,
    #[cfg(windows)]
    ui: Rc<RefCell<ui::Ui>>,
    #[cfg(windows)]
    hook: Option<powerkit_platform::keyhook::KeyboardHook>,
}

impl QuickAccent {
    pub fn new() -> Self {
        Self::default()
    }

    fn take_settings(&mut self, s: &Settings) {
        self.settings = s.clone();
        if let Ok(mut e) = self.engine.try_borrow_mut() {
            e.trigger = s.trigger;
            e.delay_ms = u64::from(s.input_delay_ms);
            e.languages = s.languages.clone();
        }
        #[cfg(windows)]
        if let Ok(mut x) = self.excluded.try_borrow_mut() {
            x.clone_from(&s.excluded_apps);
        }
        #[cfg(windows)]
        self.skip_in_game_mode.set(s.skip_in_game_mode);
    }

    /// Hook the keyboard only while running and not paused.
    #[cfg(windows)]
    fn apply(&mut self) -> Result<()> {
        use powerkit_platform::keyhook::{KeyDecision, KeyboardHook};
        let want = self.running && !self.paused;
        if !want {
            if self.hook.take().is_some() {
                self.engine.borrow_mut().reset();
                self.ui.borrow_mut().hide();
            }
            return Ok(());
        }
        if self.hook.is_none() {
            let (engine, ui, excluded) = (Rc::clone(&self.engine), Rc::clone(&self.ui), Rc::clone(&self.excluded));
            let skip_in_game_mode = Rc::clone(&self.skip_in_game_mode);
            let epoch = std::time::Instant::now();
            self.hook = Some(KeyboardHook::install(move |vk, down| {
                let now = epoch.elapsed().as_millis() as u64;
                let outcome = match engine.try_borrow_mut() {
                    Ok(mut e) => e.handle(
                        vk,
                        down,
                        now,
                        |shift| shift ^ powerkit_platform::input::caps_lock_on(),
                        || !foreground_excluded(&excluded.borrow()) && !(skip_in_game_mode.get() && game_in_front()),
                    ),
                    Err(_) => return KeyDecision::Pass,
                };
                if let Ok(mut ui) = ui.try_borrow_mut() {
                    if let Err(e) = ui.apply(outcome.effect) {
                        tracing::warn!(error = %e, "Quick Accent could not update the bar");
                    }
                }
                if outcome.swallow {
                    KeyDecision::Replace(Vec::new())
                } else {
                    KeyDecision::Pass
                }
            })?);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
fn foreground_excluded(excluded: &[String]) -> bool {
    use powerkit_platform::window;
    if excluded.is_empty() {
        return false;
    }
    let exe = window::foreground()
        .and_then(window::thread_process)
        .and_then(|(pid, _)| window::process_exe(pid));
    exe.is_some_and(|exe| excluded.contains(&exe.to_lowercase()))
}

/// Games often run in a borderless window covering the screen rather than in the
/// exclusive full-screen mode Windows reports, so both count.
#[cfg(windows)]
fn game_in_front() -> bool {
    use powerkit_platform::window::{self, Unmanageable};
    use powerkit_platform::{monitor, shell};
    if shell::game_running() {
        return true;
    }
    let Some(fg) = window::foreground() else { return false };
    let (Ok(info), Some(mon)) = (window::info(fg), monitor::from_window(fg)) else {
        return false;
    };
    info.manageable(&mon.rect) == Err(Unmanageable::FullScreen)
}

impl Module for QuickAccent {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Quick Accent".into(),
            description: "Hold a letter and press Space or an arrow key to type an accented character.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::KeyboardHook],
            commands: vec![CommandSpec {
                id: "accents".into(),
                title: "List the characters offered for a key".into(),
                description: "Args: {key, upper?}".into(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        let s = ctx.config.quick_accent.clone();
        self.take_settings(&s);
        self.running = true;
        self.paused = false;
        self.apply()
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "could not remove the Quick Accent hook");
        }
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply() {
            tracing::warn!(error = %e, "could not remove the Quick Accent hook");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.apply() {
            ctx.notify("Quick Accent", format!("Could not listen to the keyboard again: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        let s = ctx.config.quick_accent.clone();
        self.take_settings(&s);
        self.apply()
    }

    fn command(&mut self, id: &str, args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        match id {
            "accents" => {
                let a: AccentsArgs =
                    serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))?;
                let mut chars = a.key.chars();
                let (Some(key), None) = (chars.next(), chars.next()) else {
                    bail!("key must be a single character");
                };
                let list = tables::accents(&self.settings.languages, key.to_ascii_lowercase(), a.upper);
                Ok(list.into_iter().map(Value::String).collect())
            }
            _ => bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let listening = self.hook.is_some();
        #[cfg(not(windows))]
        let listening = false;
        json!({ "listening": listening, "open": self.engine.borrow().is_open() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};

    #[test]
    fn accents_command_uses_configured_languages() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::new();
        config.quick_accent.languages = vec!["german".into()];
        let paths = Paths::new(dir.path().join("data"));
        let mut actions = Vec::new();
        let mut ctx = Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        };
        // Not started: starting installs a real hook on Windows.
        let mut m = QuickAccent::new();
        m.take_settings(&config.quick_accent);
        assert_eq!(
            m.command("accents", &json!({"key": "O"}), &mut ctx).unwrap(),
            json!(["ö"])
        );
        assert_eq!(
            m.command("accents", &json!({"key": "s", "upper": true}), &mut ctx)
                .unwrap(),
            json!(["ẞ"])
        );
        assert!(m.command("accents", &json!({"key": "ab"}), &mut ctx).is_err());
        assert!(m.command("nope", &Value::Null, &mut ctx).is_err());
    }
}
