//! Light Switch: move Windows between the light and dark theme on a schedule (fixed
//! hours, or sunrise and sunset at a location) or with a shortcut.
//!
//! The schedule only acts at its transitions, so a theme the user picks in Windows
//! Settings stays until the next one. Stopping the module leaves the theme as it is.

pub mod sun;

use anyhow::{anyhow, bail, Result};
use powerkit_config::{minutes_of_day, LightSwitch as Settings, LightSwitchMode};
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use sun::{day_of_year, sun_day, SunDay};

pub const ID: &str = "light_switch";

#[cfg_attr(not(windows), allow(dead_code))]
const CHECK_MS: u32 = 30_000;

/// Light periods as local minutes after midnight: light from `.0`, dark from `.1`.
/// `None` for manual mode; `Some(Err(light))` when the sun neither rises nor sets.
fn periods(s: &Settings, local_secs: i64, utc_offset_min: i32) -> Option<Result<(u32, u32), bool>> {
    match s.mode {
        LightSwitchMode::Manual => None,
        LightSwitchMode::FixedHours => Some(Ok((
            minutes_of_day(&s.light_time).unwrap_or(7 * 60),
            minutes_of_day(&s.dark_time).unwrap_or(20 * 60),
        ))),
        LightSwitchMode::SunsetToSunrise => {
            let doy = day_of_year(local_secs.div_euclid(86_400));
            let shift = |t: u32, by: i32| (i64::from(t) + i64::from(by)).rem_euclid(1440) as u32;
            Some(match sun_day(doy, s.latitude, s.longitude, utc_offset_min) {
                SunDay::Times { rise, set } => Ok((
                    shift(rise, s.sunrise_offset_minutes),
                    shift(set, s.sunset_offset_minutes),
                )),
                SunDay::PolarDay => Err(true),
                SunDay::PolarNight => Err(false),
            })
        }
    }
}

/// Whether the schedule asks for the light theme at `local_secs` (local wall-clock
/// seconds since the epoch); `None` in manual mode.
pub fn scheduled_light(s: &Settings, local_secs: i64, utc_offset_min: i32) -> Option<bool> {
    let t = local_secs.rem_euclid(86_400) as u32 / 60;
    Some(match periods(s, local_secs, utc_offset_min)? {
        Ok((light, dark)) if light == dark => true,
        Ok((light, dark)) if light < dark => (light..dark).contains(&t),
        Ok((light, dark)) => t >= light || t < dark,
        Err(light) => light,
    })
}

fn hhmm(t: u32) -> String {
    format!("{:02}:{:02}", t / 60, t % 60)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn utc_offset_min(utc: i64) -> i32 {
    match powerkit_platform::time::to_local(utc) {
        Ok(local) => ((local - utc) / 60) as i32,
        Err(e) => {
            tracing::warn!(error = %e, "local time unavailable; using UTC");
            0
        }
    }
}

#[cfg(not(windows))]
fn utc_offset_min(_utc: i64) -> i32 {
    0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    light: bool,
}

pub struct LightSwitch {
    settings: Settings,
    running: bool,
    paused: bool,
    /// The schedule's answer at the last check; a change is a transition.
    last_scheduled: Option<bool>,
    /// Theme chosen with the shortcut or a command, kept until the next transition.
    manual: Option<bool>,
    /// The theme this module last decided on; it only writes when this changes.
    decided: Option<bool>,
    /// The theme as last written or read, standing in for Windows when not live.
    applied: Option<bool>,
    timer: Option<usize>,
    clock: fn() -> i64,
    offset: fn(i64) -> i32,
    /// Whether to touch the OS; off in tests.
    #[cfg_attr(not(windows), allow(dead_code))]
    live: bool,
}

impl Default for LightSwitch {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            running: false,
            paused: false,
            last_scheduled: None,
            manual: None,
            decided: None,
            applied: None,
            timer: None,
            clock: unix_now,
            offset: utc_offset_min,
            live: true,
        }
    }
}

impl LightSwitch {
    pub fn new() -> Self {
        Self::default()
    }

    fn local_now(&self) -> (i64, i32) {
        let utc = (self.clock)();
        let off = (self.offset)(utc);
        (utc + i64::from(off) * 60, off)
    }

    fn active(&self) -> bool {
        self.running && !self.paused
    }

    /// The current Windows theme, judged by the part this module switches.
    fn current_light(&mut self) -> Result<Option<bool>> {
        #[cfg(windows)]
        if self.live {
            let light = powerkit_platform::theme::is_light(!self.settings.change_apps)?;
            self.applied = Some(light);
        }
        Ok(self.applied)
    }

    fn write(&mut self, light: bool, ctx: &mut Ctx) -> Result<()> {
        if self.current_light()? == Some(light) {
            return Ok(());
        }
        #[cfg(windows)]
        if self.live {
            let s = &self.settings;
            powerkit_platform::theme::set_light(light, s.change_apps, s.change_system)?;
        }
        self.applied = Some(light);
        ctx.emit("light_switch.changed", self.status());
        Ok(())
    }

    /// Follow the schedule: at a transition drop the manual choice and apply the
    /// scheduled theme.
    fn check(&mut self, ctx: &mut Ctx) -> Result<()> {
        let scheduled = if self.active() {
            let (local, off) = self.local_now();
            scheduled_light(&self.settings, local, off)
        } else {
            None
        };
        if scheduled != self.last_scheduled {
            self.last_scheduled = scheduled;
            self.manual = None;
        }
        let want = self.manual.or(scheduled);
        if want.is_some() && want != self.decided {
            self.decided = want;
            if let Some(light) = want {
                self.write(light, ctx)?;
            }
        }
        let timer = self.active() && self.settings.mode != LightSwitchMode::Manual;
        self.set_timer(timer)
    }

    fn choose(&mut self, light: bool, ctx: &mut Ctx) -> Result<()> {
        self.manual = Some(light);
        self.decided = Some(light);
        self.write(light, ctx)
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

    /// Re-evaluate from scratch, as after a settings change or a resume.
    fn restart(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.last_scheduled = None;
        self.decided = None;
        self.check(ctx)
    }
}

impl Module for LightSwitch {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "Light Switch".into(),
            description: "Switch between the light and dark theme on a schedule or with a shortcut.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys],
            commands: vec![
                cmd("toggle", "Toggle light and dark", ""),
                cmd(
                    "set",
                    "Use the light or dark theme until the next scheduled change",
                    "Args: {light}",
                ),
                cmd("schedule", "Today's light and dark times", ""),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.light_switch.clone();
        self.running = true;
        self.paused = false;
        self.manual = None;
        self.restart(ctx)
    }

    fn stop(&mut self, ctx: &mut Ctx) {
        self.running = false;
        if let Err(e) = self.check(ctx) {
            tracing::warn!(error = %e, "Light Switch did not stop cleanly");
        }
    }

    fn pause(&mut self, ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.check(ctx) {
            tracing::warn!(error = %e, "Light Switch did not pause cleanly");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.restart(ctx) {
            ctx.notify("Light Switch", format!("Could not change the theme: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.light_switch.clone();
        self.restart(ctx)
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "toggle" => {
                self.command("toggle", &Value::Null, ctx)?;
                Ok(())
            }
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        match event {
            Event::Timer(id) if Some(*id) == self.timer => self.check(ctx),
            _ => Ok(()),
        }
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "toggle" => {
                let light = !self.current_light()?.unwrap_or(true);
                self.choose(light, ctx)?;
                Ok(json!({ "light": light }))
            }
            "set" => {
                let a: SetArgs = serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))?;
                self.choose(a.light, ctx)?;
                Ok(Value::Null)
            }
            "schedule" => {
                let (local, off) = self.local_now();
                Ok(match periods(&self.settings, local, off) {
                    None => json!({ "mode": "manual" }),
                    Some(Ok((light, dark))) => json!({ "light_at": hhmm(light), "dark_at": hhmm(dark) }),
                    Some(Err(true)) => json!({ "all_day": "light" }),
                    Some(Err(false)) => json!({ "all_day": "dark" }),
                })
            }
            _ => bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        json!({
            "mode": self.settings.mode,
            "theme": self.applied.map(|l| if l { "light" } else { "dark" }),
            "manual": self.manual.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};
    use powerkit_plugin_api::Action;
    use std::cell::Cell;

    const DAY: i64 = 86_400;
    const HOUR: i64 = 3_600;

    thread_local! {
        // 2026-06-21 00:00 UTC.
        static NOW: Cell<i64> = const { Cell::new(20_625 * DAY) };
    }

    fn fake_now() -> i64 {
        NOW.with(Cell::get)
    }

    fn set_now(t: i64) {
        NOW.with(|n| n.set(t));
    }

    fn fixed(light: &str, dark: &str) -> Settings {
        Settings {
            mode: LightSwitchMode::FixedHours,
            light_time: light.into(),
            dark_time: dark.into(),
            ..Settings::default()
        }
    }

    #[test]
    fn fixed_hours_including_overnight_light() {
        let s = fixed("07:00", "20:00");
        let at = |h: i64, m: i64| scheduled_light(&s, 5 * DAY + h * HOUR + m * 60, 0);
        assert_eq!(at(6, 59), Some(false));
        assert_eq!(at(7, 0), Some(true));
        assert_eq!(at(19, 59), Some(true));
        assert_eq!(at(20, 0), Some(false));
        let night_owl = fixed("22:00", "06:00");
        let at = |h: i64| scheduled_light(&night_owl, h * HOUR, 0);
        assert_eq!((at(23), at(3), at(12)), (Some(true), Some(true), Some(false)));
        assert_eq!(scheduled_light(&Settings::default(), 0, 0), None);
    }

    #[test]
    fn sun_schedule_with_offsets_and_polar_day() {
        let mut s = Settings {
            mode: LightSwitchMode::SunsetToSunrise,
            latitude: 51.5,
            longitude: -0.13,
            sunset_offset_minutes: -60,
            ..Settings::default()
        };
        let local = 20_625 * DAY; // midnight 2026-06-21, London summer time
        let at = |s: &Settings, h: i64| scheduled_light(s, local + h * HOUR, 60);
        assert_eq!(at(&s, 4), Some(false)); // before 04:43 sunrise
        assert_eq!(at(&s, 5), Some(true));
        assert_eq!(at(&s, 20), Some(true));
        assert_eq!(at(&s, 21), Some(false), "sunset 21:21 moved an hour earlier");
        s.latitude = 69.6;
        assert_eq!(at(&s, 0), Some(true));
    }

    struct Fixture {
        config: Config,
        paths: Paths,
        actions: Vec<Action>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(settings: Settings) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut config = Config::new();
            config.light_switch = settings;
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
    }

    fn module(theme: Option<bool>) -> LightSwitch {
        LightSwitch {
            clock: fake_now,
            offset: |_| 0,
            live: false,
            applied: theme,
            ..LightSwitch::default()
        }
    }

    fn tick(m: &mut LightSwitch, fx: &mut Fixture, at: i64) {
        set_now(at);
        let timer = m.timer.expect("a schedule keeps a timer");
        m.on_event(&Event::Timer(timer), &mut fx.ctx()).unwrap();
    }

    #[test]
    fn manual_choice_lasts_until_the_next_transition() {
        let mut fx = Fixture::new(fixed("07:00", "20:00"));
        let mut m = module(Some(true));
        let day = 20_625 * DAY;
        set_now(day + 21 * HOUR);
        m.start(&mut fx.ctx()).unwrap();
        assert_eq!(m.status()["theme"], json!("dark"));

        m.on_hotkey("toggle", &mut fx.ctx()).unwrap();
        assert_eq!(m.status()["theme"], json!("light"));
        assert_eq!(m.status()["manual"], json!(true));
        tick(&mut m, &mut fx, day + 23 * HOUR);
        assert_eq!(m.status()["theme"], json!("light"), "the shortcut wins until morning");

        tick(&mut m, &mut fx, day + DAY + 7 * HOUR);
        assert_eq!(m.status()["manual"], json!(false));
        tick(&mut m, &mut fx, day + DAY + 20 * HOUR);
        assert_eq!(m.status()["theme"], json!("dark"));
    }

    #[test]
    fn theme_changed_outside_powerkit_is_left_alone_between_transitions() {
        let mut fx = Fixture::new(fixed("07:00", "20:00"));
        let mut m = module(Some(true));
        let day = 20_625 * DAY;
        set_now(day + 9 * HOUR);
        m.start(&mut fx.ctx()).unwrap();
        assert!(fx.actions.is_empty(), "already light; nothing to write");
        m.applied = Some(false); // the user picked dark in Windows Settings
        tick(&mut m, &mut fx, day + 10 * HOUR);
        assert_eq!(m.status()["theme"], json!("dark"));
        tick(&mut m, &mut fx, day + 21 * HOUR);
        tick(&mut m, &mut fx, day + DAY + 7 * HOUR);
        assert_eq!(m.status()["theme"], json!("light"));
    }

    #[test]
    fn manual_mode_has_no_timer_and_pause_stops_checking() {
        let mut fx = Fixture::new(Settings::default());
        let mut m = module(None);
        m.start(&mut fx.ctx()).unwrap();
        assert!(m.timer.is_none());
        assert_eq!(m.status()["theme"], Value::Null);
        m.command("set", &json!({ "light": false }), &mut fx.ctx()).unwrap();
        assert_eq!(m.status()["theme"], json!("dark"));

        fx.config.light_switch = fixed("07:00", "20:00");
        m.config_changed(&mut fx.ctx()).unwrap();
        assert!(m.timer.is_some());
        m.pause(&mut fx.ctx());
        assert!(m.timer.is_none());
    }

    #[test]
    fn commands() {
        let mut fx = Fixture::new(fixed("06:30", "19:15"));
        let mut m = module(Some(true));
        m.start(&mut fx.ctx()).unwrap();
        assert_eq!(
            m.command("schedule", &Value::Null, &mut fx.ctx()).unwrap(),
            json!({ "light_at": "06:30", "dark_at": "19:15" })
        );
        assert!(m.command("set", &json!({}), &mut fx.ctx()).is_err());
        assert!(m.command("nope", &Value::Null, &mut fx.ctx()).is_err());
        assert!(m.on_hotkey("nope", &mut fx.ctx()).is_err());
    }
}
