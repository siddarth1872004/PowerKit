//! Awake: keep the computer from sleeping without changing the power plan, either
//! indefinitely, for a number of minutes, or until a point in time.

use anyhow::{anyhow, bail, Result};
use powerkit_config::{Awake as Settings, AwakeMode};
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};

pub const ID: &str = "awake";

/// How often a running session checks whether it has ended.
#[cfg_attr(not(windows), allow(dead_code))]
const CHECK_MS: u32 = 15_000;

/// What the module should be doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub awake: bool,
    pub display: bool,
    /// Unix seconds at which the session ends, for timed and "until" sessions.
    pub ends_at: Option<i64>,
}

impl Plan {
    const OFF: Plan = Plan {
        awake: false,
        display: false,
        ends_at: None,
    };
}

/// `timed_since` is when the current timed session began. An ended session plans Off.
pub fn plan(s: &Settings, timed_since: i64, now: i64) -> Plan {
    let ends_at = match s.mode {
        AwakeMode::Off => return Plan::OFF,
        AwakeMode::Indefinite => None,
        AwakeMode::Timed => Some(timed_since + i64::from(s.duration_minutes) * 60),
        AwakeMode::Until => Some(s.expire_at),
    };
    if ends_at.is_some_and(|end| now >= end) {
        return Plan::OFF;
    }
    Plan {
        awake: true,
        display: s.keep_display_on,
        ends_at,
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    mode: AwakeMode,
    #[serde(default)]
    minutes: Option<u32>,
    /// Unix seconds.
    #[serde(default)]
    until: Option<i64>,
    #[serde(default)]
    display: Option<bool>,
}

pub struct Awake {
    settings: Settings,
    paused: bool,
    running: bool,
    /// Start of the current timed session; reset whenever a timed session is chosen.
    timed_since: i64,
    applied: Plan,
    timer: Option<usize>,
    /// Set once an expired session has asked the host to switch the mode off.
    expiry_reported: bool,
    clock: fn() -> i64,
    /// Whether to touch the OS; off in tests.
    live: bool,
}

impl Default for Awake {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            paused: false,
            running: false,
            timed_since: 0,
            applied: Plan::OFF,
            timer: None,
            expiry_reported: false,
            clock: unix_now,
            live: true,
        }
    }
}

impl Awake {
    pub fn new() -> Self {
        Self::default()
    }

    fn current(&self) -> Plan {
        if !self.running || self.paused {
            return Plan::OFF;
        }
        plan(&self.settings, self.timed_since, (self.clock)())
    }

    fn take_settings(&mut self, new: &Settings) {
        let restart_timer = new.mode == AwakeMode::Timed
            && (self.settings.mode != AwakeMode::Timed
                || self.settings.duration_minutes != new.duration_minutes
                || !self.running);
        if restart_timer {
            self.timed_since = (self.clock)();
        }
        if new.mode != self.settings.mode || new.expire_at != self.settings.expire_at || restart_timer {
            self.expiry_reported = false;
        }
        self.settings = new.clone();
    }

    /// Bring the OS request and the check timer in line with the plan, and switch the
    /// configured mode off once a session has ended.
    fn apply(&mut self, ctx: &mut Ctx) -> Result<()> {
        let p = self.current();
        if p != self.applied {
            self.set_os(p)?;
            self.applied = p;
            ctx.emit("awake.changed", self.status());
        }
        self.set_timer(p.ends_at.is_some())?;
        let ended = self.running && !self.paused && self.settings.mode != AwakeMode::Off && !p.awake;
        if ended && !self.expiry_reported {
            self.expiry_reported = true;
            ctx.update_config(|c| c.awake.mode = AwakeMode::Off);
            ctx.notify("Awake", "The keep-awake session ended; the power plan applies again.");
        }
        Ok(())
    }

    fn set_os(&self, p: Plan) -> Result<()> {
        if !self.live {
            return Ok(());
        }
        #[cfg(windows)]
        {
            use powerkit_platform::power;
            if p.awake {
                power::keep_awake(p.display)?;
            } else {
                power::release()?;
            }
        }
        #[cfg(not(windows))]
        let _ = p;
        Ok(())
    }

    fn set_timer(&mut self, want: bool) -> Result<()> {
        match (want, self.timer) {
            (true, None) => self.timer = Some(self.start_timer()?),
            (false, Some(id)) => {
                #[cfg(windows)]
                if self.live {
                    powerkit_platform::event_loop::kill_timer(id);
                }
                let _ = id;
                self.timer = None;
            }
            _ => {}
        }
        Ok(())
    }

    fn start_timer(&self) -> Result<usize> {
        #[cfg(windows)]
        if self.live {
            return Ok(powerkit_platform::event_loop::set_timer(CHECK_MS)?);
        }
        // Without an event loop (tests, other platforms) no timer fires; the id only
        // keeps the bookkeeping identical.
        Ok(usize::MAX)
    }
}

impl Module for Awake {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "Awake".into(),
            description: "Keep the computer awake without changing the power plan.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![],
            commands: vec![
                cmd(
                    "set",
                    "Choose how to stay awake",
                    "Args: {mode: off|indefinite|timed|until, minutes?, until? (Unix seconds), display?}",
                ),
                cmd("off", "Let the computer sleep normally", ""),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.running = true;
        self.paused = false;
        let s = ctx.config.awake.clone();
        self.take_settings(&s);
        self.apply(ctx)
    }

    fn stop(&mut self, ctx: &mut Ctx) {
        self.running = false;
        if let Err(e) = self.apply(ctx) {
            tracing::warn!(error = %e, "could not hand sleep control back to Windows");
        }
    }

    fn pause(&mut self, ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply(ctx) {
            tracing::warn!(error = %e, "could not hand sleep control back to Windows");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.apply(ctx) {
            ctx.notify("Awake", format!("Could not keep the computer awake: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        let s = ctx.config.awake.clone();
        self.take_settings(&s);
        self.apply(ctx)
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        match event {
            Event::Timer(id) if Some(*id) == self.timer => self.apply(ctx),
            _ => Ok(()),
        }
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "set" => {
                let a: SetArgs = serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))?;
                match a.mode {
                    AwakeMode::Timed if a.minutes.is_some_and(|m| !(1..=7 * 24 * 60).contains(&m)) => {
                        bail!("minutes must be between 1 and 10080")
                    }
                    AwakeMode::Until => match a.until {
                        None => bail!("\"until\" mode needs an end time"),
                        Some(t) if t <= (self.clock)() => bail!("the end time is in the past"),
                        Some(_) => {}
                    },
                    _ => {}
                }
                ctx.update_config(move |c| {
                    let w = &mut c.awake;
                    w.mode = a.mode;
                    if let Some(m) = a.minutes {
                        w.duration_minutes = m;
                    }
                    if let Some(t) = a.until {
                        w.expire_at = t;
                    }
                    if let Some(d) = a.display {
                        w.keep_display_on = d;
                    }
                });
                Ok(Value::Null)
            }
            "off" => {
                ctx.update_config(|c| c.awake.mode = AwakeMode::Off);
                Ok(Value::Null)
            }
            _ => bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        let p = self.applied;
        json!({
            "mode": self.settings.mode,
            "awake": p.awake,
            "display": p.display,
            "ends_at": p.ends_at,
            "remaining_seconds": p.ends_at.map(|e| (e - (self.clock)()).max(0)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};
    use powerkit_plugin_api::Action;
    use std::cell::Cell;

    thread_local! {
        static NOW: Cell<i64> = const { Cell::new(1_000_000) };
    }

    fn fake_now() -> i64 {
        NOW.with(Cell::get)
    }

    fn advance(secs: i64) {
        NOW.with(|n| n.set(n.get() + secs));
    }

    fn settings(mode: AwakeMode) -> Settings {
        Settings {
            mode,
            ..Settings::default()
        }
    }

    #[test]
    fn plan_by_mode() {
        assert_eq!(plan(&settings(AwakeMode::Off), 0, 100), Plan::OFF);
        let mut s = settings(AwakeMode::Indefinite);
        s.keep_display_on = true;
        assert_eq!(
            plan(&s, 0, 100),
            Plan {
                awake: true,
                display: true,
                ends_at: None
            }
        );
        let mut t = settings(AwakeMode::Timed);
        t.duration_minutes = 2;
        assert_eq!(plan(&t, 100, 219).ends_at, Some(220));
        assert_eq!(plan(&t, 100, 220), Plan::OFF);
        let mut u = settings(AwakeMode::Until);
        u.expire_at = 500;
        assert!(plan(&u, 0, 499).awake);
        assert_eq!(plan(&u, 0, 500), Plan::OFF);
    }

    struct Fixture {
        config: Config,
        paths: Paths,
        actions: Vec<Action>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            Self {
                config: Config::new(),
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

        /// Apply queued config updates the way the host does, then notify the module.
        fn flush(&mut self, m: &mut Awake) {
            let mut changed = false;
            for a in std::mem::take(&mut self.actions) {
                if let Action::UpdateConfig(f) = a {
                    f(&mut self.config);
                    changed = true;
                }
            }
            if changed {
                assert!(self.config.sanitize().is_empty());
                m.config_changed(&mut self.ctx()).unwrap();
            }
        }

        fn notified(&self) -> bool {
            self.actions.iter().any(|a| matches!(a, Action::Notify { .. }))
        }
    }

    fn module() -> Awake {
        Awake {
            clock: fake_now,
            live: false,
            ..Awake::default()
        }
    }

    #[test]
    fn timed_session_ends_and_switches_off() {
        let mut fx = Fixture::new();
        let mut m = module();
        m.start(&mut fx.ctx()).unwrap();
        assert_eq!(m.status()["awake"], json!(false));
        assert!(m.timer.is_none());

        m.command("set", &json!({ "mode": "timed", "minutes": 1 }), &mut fx.ctx())
            .unwrap();
        fx.flush(&mut m);
        assert_eq!(m.status()["awake"], json!(true));
        assert_eq!(m.status()["remaining_seconds"], json!(60));
        let timer = m.timer.expect("a timed session checks for its end");

        advance(30);
        m.on_event(&Event::Timer(timer), &mut fx.ctx()).unwrap();
        assert_eq!(m.status()["remaining_seconds"], json!(30));
        assert!(!fx.notified());

        advance(30);
        m.on_event(&Event::Timer(timer), &mut fx.ctx()).unwrap();
        assert!(fx.notified());
        assert_eq!(m.status()["awake"], json!(false));
        assert!(m.timer.is_none());
        fx.flush(&mut m);
        assert_eq!(fx.config.awake.mode, AwakeMode::Off);
    }

    #[test]
    fn changing_the_duration_restarts_the_timer() {
        let mut fx = Fixture::new();
        fx.config.awake.mode = AwakeMode::Timed;
        fx.config.awake.duration_minutes = 10;
        let mut m = module();
        m.start(&mut fx.ctx()).unwrap();
        advance(300);
        fx.config.awake.keep_display_on = true;
        m.config_changed(&mut fx.ctx()).unwrap();
        assert_eq!(
            m.status()["remaining_seconds"],
            json!(300),
            "unrelated change keeps the deadline"
        );
        fx.config.awake.duration_minutes = 20;
        m.config_changed(&mut fx.ctx()).unwrap();
        assert_eq!(m.status()["remaining_seconds"], json!(1200));
        assert_eq!(m.status()["display"], json!(true));
    }

    #[test]
    fn pause_and_stop_release() {
        let mut fx = Fixture::new();
        fx.config.awake.mode = AwakeMode::Indefinite;
        let mut m = module();
        m.start(&mut fx.ctx()).unwrap();
        assert_eq!(m.status()["awake"], json!(true));
        assert!(m.timer.is_none());
        m.pause(&mut fx.ctx());
        assert_eq!(m.status()["awake"], json!(false));
        m.resume(&mut fx.ctx());
        assert_eq!(m.status()["awake"], json!(true));
        m.stop(&mut fx.ctx());
        assert_eq!(m.status()["awake"], json!(false));
        assert!(!fx.notified(), "pausing is not the end of a session");
    }

    #[test]
    fn expired_until_at_start_switches_off_once() {
        let mut fx = Fixture::new();
        fx.config.awake.mode = AwakeMode::Until;
        fx.config.awake.expire_at = fake_now() - 1;
        let mut m = module();
        m.start(&mut fx.ctx()).unwrap();
        m.on_event(&Event::Timer(0), &mut fx.ctx()).unwrap();
        let updates = fx
            .actions
            .iter()
            .filter(|a| matches!(a, Action::UpdateConfig(_)))
            .count();
        assert_eq!(updates, 1);
    }

    #[test]
    fn set_validates_arguments() {
        let mut fx = Fixture::new();
        let mut m = module();
        let mut set = |args: Value| m.command("set", &args, &mut fx.ctx());
        assert!(set(json!({ "mode": "until" })).is_err());
        assert!(set(json!({ "mode": "until", "until": fake_now() })).is_err());
        assert!(set(json!({ "mode": "timed", "minutes": 0 })).is_err());
        assert!(set(json!({ "mode": "sometimes" })).is_err());
        assert!(set(json!({ "mode": "indefinite", "extra": 1 })).is_err());
        set(json!({ "mode": "until", "until": fake_now() + 60, "display": true })).unwrap();
        fx.flush(&mut m);
        assert_eq!(fx.config.awake.mode, AwakeMode::Until);
        assert!(fx.config.awake.keep_display_on);
        assert!(m.command("nope", &Value::Null, &mut fx.ctx()).is_err());
    }
}
