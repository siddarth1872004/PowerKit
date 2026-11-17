//! Tray icons of their own for the utilities used from the notification area, as
//! PowerToys gives Awake one: Awake, Power Plan Switcher and Light Switch, each shown
//! while its utility is on.

use crate::host::Host;
use crate::icons;
use powerkit_config::AwakeMode;
use powerkit_core::TrayEvent;
use powerkit_platform::tray::{MenuItem, Tray};
use serde_json::{json, Value};

pub const AWAKE: u32 = 2;
pub const POWER_PLAN: u32 = 3;
pub const LIGHT_SWITCH: u32 = 4;

/// Lengths offered for a timed Awake session, in minutes.
const TIMED: [u32; 3] = [30, 60, 120];

const AWAKE_OFF: u32 = 1;
const AWAKE_INDEFINITE: u32 = 2;
const AWAKE_UNTIL: u32 = 3;
const AWAKE_TIMED_BASE: u32 = 10;
const AWAKE_DISPLAY: u32 = 50;
const SETTINGS: u32 = 60;
const LIGHT: u32 = 1;
const DARK: u32 = 2;

/// What the icons show; they are redone when it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    awake: bool,
    power_plan: bool,
    light_switch: bool,
    awake_mode: AwakeMode,
    awake_minutes: u32,
    awake_until: i64,
}

#[derive(Default)]
pub struct ToolTrays {
    awake: Option<Tray>,
    power_plan: Option<Tray>,
    light_switch: Option<Tray>,
    shown: Option<Shown>,
}

impl ToolTrays {
    /// Add or remove the icons as the utilities are turned on or off, and keep the
    /// Awake icon's text current. Cheap when nothing changed.
    pub fn sync(&mut self, host: &Host) {
        let c = host.config();
        let now = Shown {
            awake: c.awake.enabled,
            power_plan: c.power_plan.enabled,
            light_switch: c.light_switch.enabled,
            awake_mode: c.awake.mode,
            awake_minutes: c.awake.duration_minutes,
            awake_until: c.awake.expire_at,
        };
        if self.shown.as_ref() == Some(&now) {
            return;
        }
        let awake_text = now.awake.then(|| awake_tooltip(host));
        show(
            &mut self.awake,
            now.awake,
            AWAKE,
            "awake",
            awake_text.as_deref().unwrap_or("Awake"),
        );
        if let (Some(tray), Some(text)) = (&self.awake, &awake_text) {
            if let Err(e) = tray.set_tooltip(text) {
                tracing::warn!("Awake tray icon: {e}");
            }
        }
        show(
            &mut self.power_plan,
            now.power_plan,
            POWER_PLAN,
            "power_plan",
            "Power plan",
        );
        show(
            &mut self.light_switch,
            now.light_switch,
            LIGHT_SWITCH,
            "light_switch",
            "Light Switch: click to switch between light and dark",
        );
        self.shown = Some(now);
    }

    /// A click on one of these icons; `None` when `icon` is not one of them.
    pub fn on_click(&self, icon: u32, event: TrayEvent, host: &mut Host) -> Option<Result<(), String>> {
        Some(match icon {
            AWAKE => awake_menu(self.awake.as_ref()?, host),
            POWER_PLAN => power_plan_menu(self.power_plan.as_ref()?, host.config().power_plan.notify),
            LIGHT_SWITCH if event == TrayEvent::Activate => command(host, "light_switch", "toggle", Value::Null),
            LIGHT_SWITCH => light_switch_menu(self.light_switch.as_ref()?, host),
            _ => return None,
        })
    }
}

fn show(slot: &mut Option<Tray>, wanted: bool, id: u32, module: &str, tooltip: &str) {
    match (wanted, slot.is_some()) {
        (true, false) => {
            let Some(ico) = icons::module(module) else { return };
            match Tray::with_icon(id, tooltip, ico) {
                Ok(t) => *slot = Some(t),
                Err(e) => tracing::warn!("{module} tray icon not added: {e}"),
            }
        }
        (false, true) => *slot = None,
        _ => {}
    }
}

fn command(host: &mut Host, id: &str, command: &str, args: Value) -> Result<(), String> {
    host.handle_request("module.command", &json!({ "id": id, "command": command, "args": args }))
        .map(drop)
}

fn open_settings(host: &mut Host, page: &str) -> Result<(), String> {
    host.handle_request("app.open_settings", &json!({ "page": page }))
        .map(drop)
}

/// `utc_secs` as local `HH:MM`.
fn clock(utc_secs: i64) -> Option<String> {
    let local = powerkit_platform::time::to_local(utc_secs).ok()?;
    let minutes = local.rem_euclid(86_400) / 60;
    Some(format!("{:02}:{:02}", minutes / 60, minutes % 60))
}

fn awake_tooltip(host: &Host) -> String {
    let c = &host.config().awake;
    match c.mode {
        AwakeMode::Off => "Awake: off, the power plan decides".into(),
        AwakeMode::Indefinite => "Awake: kept awake".into(),
        AwakeMode::Timed | AwakeMode::Until => {
            let status = host.status();
            let ends = status["modules"]
                .as_array()
                .and_then(|m| m.iter().find(|m| m["id"] == awake::ID))
                .and_then(|m| m["status"]["ends_at"].as_i64())
                .or((c.mode == AwakeMode::Until).then_some(c.expire_at));
            match ends.and_then(clock) {
                Some(at) => format!("Awake: kept awake until {at}"),
                None => "Awake: kept awake".into(),
            }
        }
    }
}

fn minutes_label(m: u32) -> String {
    match m {
        60 => "1 hour".into(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} minutes"),
    }
}

fn awake_menu(tray: &Tray, host: &mut Host) -> Result<(), String> {
    let c = host.config().awake.clone();
    let mut timed: Vec<u32> = TIMED.to_vec();
    if !timed.contains(&c.duration_minutes) {
        timed.push(c.duration_minutes);
        timed.sort_unstable();
    }
    let mut items = vec![
        MenuItem::checked(AWAKE_OFF, "Off (the power plan decides)", c.mode == AwakeMode::Off),
        MenuItem::checked(
            AWAKE_INDEFINITE,
            "Keep awake indefinitely",
            c.mode == AwakeMode::Indefinite,
        ),
    ];
    items.extend(timed.iter().enumerate().map(|(i, &m)| {
        MenuItem::checked(
            AWAKE_TIMED_BASE + i as u32,
            format!("Keep awake for {}", minutes_label(m)),
            c.mode == AwakeMode::Timed && c.duration_minutes == m,
        )
    }));
    if c.mode == AwakeMode::Until {
        if let Some(at) = clock(c.expire_at) {
            items.push(MenuItem::Item {
                id: AWAKE_UNTIL,
                label: format!("Keep awake until {at}"),
                checked: true,
                enabled: false,
            });
        }
    }
    items.push(MenuItem::Separator);
    items.push(MenuItem::checked(AWAKE_DISPLAY, "Keep screen on", c.keep_display_on));
    items.push(MenuItem::new(SETTINGS, "Awake settings"));
    let set = |host: &mut Host, args: Value| command(host, awake::ID, "set", args);
    match tray.show_menu(&items).map_err(|e| format!("Awake: {e}"))? {
        Some(AWAKE_OFF) => command(host, awake::ID, "off", Value::Null),
        Some(AWAKE_INDEFINITE) => set(host, json!({ "mode": AwakeMode::Indefinite })),
        Some(id) if (AWAKE_TIMED_BASE..AWAKE_DISPLAY).contains(&id) => {
            match timed.get((id - AWAKE_TIMED_BASE) as usize) {
                Some(&m) => set(host, json!({ "mode": AwakeMode::Timed, "minutes": m })),
                None => Ok(()),
            }
        }
        Some(AWAKE_DISPLAY) => {
            // Through the whole config, not "set": that would restart a timed session.
            let mut config = host.config().clone();
            config.awake.keep_display_on = !c.keep_display_on;
            let value = serde_json::to_value(&config).map_err(|e| e.to_string())?;
            host.handle_request("config.set", &json!({ "config": value })).map(drop)
        }
        Some(SETTINGS) => open_settings(host, awake::ID),
        _ => Ok(()),
    }
}

fn light_switch_menu(tray: &Tray, host: &mut Host) -> Result<(), String> {
    let status = host.status();
    let theme = status["modules"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["id"] == light_switch::ID))
        .and_then(|m| m["status"]["theme"].as_str().map(str::to_string));
    let items = [
        MenuItem::checked(LIGHT, "Light", theme.as_deref() == Some("light")),
        MenuItem::checked(DARK, "Dark", theme.as_deref() == Some("dark")),
        MenuItem::Separator,
        MenuItem::new(SETTINGS, "Light Switch settings"),
    ];
    match tray.show_menu(&items).map_err(|e| format!("Light Switch: {e}"))? {
        Some(id @ (LIGHT | DARK)) => command(host, light_switch::ID, "set", json!({ "light": id == LIGHT })),
        Some(SETTINGS) => open_settings(host, light_switch::ID),
        _ => Ok(()),
    }
}

/// Show the power plans (and power modes) at the cursor and switch to the one chosen.
pub fn power_plan_menu(tray: &Tray, notify: bool) -> Result<(), String> {
    let state = power_plan::state().map_err(|e| format!("Power Plan Switcher: {e:#}"))?;
    let (plans, modes) = state.menu();
    let mut items: Vec<MenuItem> = plans
        .iter()
        .enumerate()
        .map(|(i, e)| MenuItem::checked(1 + i as u32, &e.label, e.checked))
        .collect();
    if !modes.is_empty() {
        items.push(MenuItem::Separator);
    }
    items.extend(
        modes
            .iter()
            .enumerate()
            .map(|(i, e)| MenuItem::checked(1000 + i as u32, &e.label, e.checked)),
    );
    let chosen = match tray
        .show_menu(&items)
        .map_err(|e| format!("Power Plan Switcher: {e}"))?
    {
        Some(id) if id >= 1000 => modes.get((id - 1000) as usize),
        Some(id) => plans.get(id as usize - 1),
        None => None,
    };
    let Some(entry) = chosen else { return Ok(()) };
    if entry.checked {
        return Ok(());
    }
    let now = power_plan::apply(&entry.choice).map_err(|e| format!("Power Plan Switcher: {e:#}"))?;
    if notify {
        if let Err(e) = tray.notify("Power plan", &now.describe()) {
            tracing::warn!("notification not shown: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::minutes_label;

    #[test]
    fn timed_lengths_read_as_hours_when_whole() {
        assert_eq!(minutes_label(30), "30 minutes");
        assert_eq!(minutes_label(60), "1 hour");
        assert_eq!(minutes_label(120), "2 hours");
        assert_eq!(minutes_label(90), "90 minutes");
    }
}
