//! Power Plan Switcher: pick the Windows power plan, and the power mode while the
//! Balanced plan is on, from a menu at the cursor or the tray, or step to the next
//! plan with a shortcut.

#[cfg(windows)]
mod live;
#[cfg(windows)]
pub use live::{apply, next, state};

use anyhow::{bail, Result};
use powerkit_config::PowerPlan as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "power_plan";
pub const OPEN_EVENT: &str = "power_plan.open";

/// The Balanced plan Windows ships; only it has power modes.
pub const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";

/// The power modes of Windows 11's slider, from slowest to fastest.
pub const MODES: [(&str, &str); 3] = [
    ("961cc777-2547-4f9d-8174-7d86181b8a7a", "Best power efficiency"),
    ("00000000-0000-0000-0000-000000000000", "Balanced"),
    ("ded574b5-45a0-4f42-8737-46345c09c238", "Best performance"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub guid: String,
    pub name: String,
}

/// The plans there are, the one that is on, and the power mode if it can be read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerState {
    pub plans: Vec<Plan>,
    pub active: String,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Plan(String),
    Mode(String),
}

/// One line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub checked: bool,
    pub choice: Choice,
}

impl PowerState {
    pub fn plan_name(&self, guid: &str) -> Option<&str> {
        self.plans
            .iter()
            .find(|p| p.guid.eq_ignore_ascii_case(guid))
            .map(|p| p.name.as_str())
    }

    /// The plans, then the power modes while the Balanced plan is on.
    pub fn menu(&self) -> (Vec<Entry>, Vec<Entry>) {
        let plans = self
            .plans
            .iter()
            .map(|p| Entry {
                label: p.name.clone(),
                checked: p.guid.eq_ignore_ascii_case(&self.active),
                choice: Choice::Plan(p.guid.clone()),
            })
            .collect();
        let modes = match &self.mode {
            Some(mode) if self.active.eq_ignore_ascii_case(BALANCED) => MODES
                .iter()
                .map(|(g, name)| Entry {
                    label: format!("Power mode: {name}"),
                    checked: g.eq_ignore_ascii_case(mode),
                    choice: Choice::Mode((*g).into()),
                })
                .collect(),
            _ => Vec::new(),
        };
        (plans, modes)
    }

    /// The plan after the one that is on, going round to the first.
    pub fn next_plan(&self) -> Option<&Plan> {
        let at = self
            .plans
            .iter()
            .position(|p| p.guid.eq_ignore_ascii_case(&self.active));
        let n = self.plans.len();
        (n > 0).then(|| &self.plans[at.map_or(0, |i| (i + 1) % n)])
    }

    /// What is on now, in a few words.
    pub fn describe(&self) -> String {
        let plan = self.plan_name(&self.active).unwrap_or("An unknown plan");
        let mode = self
            .mode
            .as_deref()
            .filter(|_| self.active.eq_ignore_ascii_case(BALANCED))
            .and_then(|m| MODES.iter().find(|(g, _)| g.eq_ignore_ascii_case(m)))
            .map(|(_, name)| format!(", power mode {name}"))
            .unwrap_or_default();
        format!("{plan} is on{mode}.")
    }
}

#[derive(Default)]
pub struct PowerPlan {
    settings: Settings,
    running: bool,
}

impl PowerPlan {
    pub fn new() -> Self {
        Self::default()
    }

    fn check(&self) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Power Plan Switcher is turned off");
        }
        Ok(())
    }

    #[cfg(windows)]
    fn step(&self, ctx: &mut Ctx) -> Result<()> {
        let now = next()?;
        if self.settings.notify {
            ctx.notify("Power plan", now.describe());
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn step(&self, _ctx: &mut Ctx) -> Result<()> {
        bail!("power plans exist only on Windows")
    }

    #[cfg(windows)]
    fn choose(&self, choice: Choice) -> Result<Value> {
        Ok(json!({ "state": apply(&choice)?.describe() }))
    }

    #[cfg(not(windows))]
    fn choose(&self, _choice: Choice) -> Result<Value> {
        bail!("power plans exist only on Windows")
    }
}

impl Module for PowerPlan {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Power Plan Switcher".into(),
            description: "Switch the power plan and power mode from a shortcut or the tray.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys],
            commands: vec![
                CommandSpec {
                    id: "open".into(),
                    title: "Show the power plans".into(),
                    description: String::new(),
                },
                CommandSpec {
                    id: "next".into(),
                    title: "Switch to the next power plan".into(),
                    description: String::new(),
                },
                CommandSpec {
                    id: "set".into(),
                    title: "Switch the power plan or mode".into(),
                    description: "Takes `plan` or `mode`, a GUID.".into(),
                },
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.power_plan.clone();
        self.running = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.power_plan.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        self.check()?;
        match action {
            "open" => {
                ctx.emit(OPEN_EVENT, Value::Null);
                Ok(())
            }
            "next" => self.step(ctx),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        self.check()?;
        match id {
            "open" => ctx.emit(OPEN_EVENT, Value::Null),
            "next" => self.step(ctx)?,
            "set" => {
                let choice = match (args["plan"].as_str(), args["mode"].as_str()) {
                    (Some(p), _) => Choice::Plan(p.into()),
                    (None, Some(m)) => Choice::Mode(m.into()),
                    (None, None) => bail!("give `plan` or `mode`"),
                };
                return self.choose(choice);
            }
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "running": self.running && self.settings.enabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(active: &str, mode: Option<&str>) -> PowerState {
        PowerState {
            plans: vec![
                Plan {
                    guid: BALANCED.into(),
                    name: "Balanced".into(),
                },
                Plan {
                    guid: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into(),
                    name: "High performance".into(),
                },
            ],
            active: active.into(),
            mode: mode.map(Into::into),
        }
    }

    #[test]
    fn power_modes_are_offered_only_with_the_balanced_plan() {
        let (plans, modes) = state(BALANCED, Some(MODES[2].0)).menu();
        assert_eq!(plans.iter().filter(|e| e.checked).count(), 1);
        assert!(plans[0].checked);
        assert_eq!(modes.len(), 3);
        assert!(modes[2].checked && !modes[0].checked);
        let (_, modes) = state("8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635C", Some(MODES[1].0)).menu();
        assert!(modes.is_empty());
        let (_, modes) = state(BALANCED, None).menu();
        assert!(modes.is_empty());
    }

    #[test]
    fn the_next_plan_goes_round() {
        let s = state(BALANCED, None);
        assert_eq!(s.next_plan().unwrap().name, "High performance");
        let s = state("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c", None);
        assert_eq!(s.next_plan().unwrap().name, "Balanced");
        let s = state("unknown", None);
        assert_eq!(s.next_plan().unwrap().name, "Balanced");
        assert!(PowerState::default().next_plan().is_none());
    }

    #[test]
    fn the_state_is_described_in_words() {
        assert_eq!(
            state(BALANCED, Some(MODES[0].0)).describe(),
            "Balanced is on, power mode Best power efficiency."
        );
        assert_eq!(
            state("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c", Some(MODES[0].0)).describe(),
            "High performance is on."
        );
    }
}
