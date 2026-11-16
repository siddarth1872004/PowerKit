use crate::{Choice, Plan, PowerState};
use anyhow::{Context, Result};
use powerkit_platform::power_scheme;

/// The plans and what is on now. A power mode that cannot be read (Windows before
/// 10 1709) is left out rather than failing the whole menu.
pub fn state() -> Result<PowerState> {
    let plans = power_scheme::plans()
        .context("listing the power plans")?
        .into_iter()
        .map(|p| Plan {
            guid: p.guid,
            name: p.name,
        })
        .collect();
    let active = power_scheme::active_plan().context("reading the power plan")?;
    let mode = match power_scheme::active_mode() {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!("power plan: no power mode: {e}");
            None
        }
    };
    Ok(PowerState { plans, active, mode })
}

/// Switch, and return what is on afterwards.
pub fn apply(choice: &Choice) -> Result<PowerState> {
    match choice {
        Choice::Plan(g) => power_scheme::set_plan(g).context("switching the power plan")?,
        Choice::Mode(g) => power_scheme::set_mode(g).context("switching the power mode")?,
    }
    state()
}

pub fn next() -> Result<PowerState> {
    let now = state()?;
    let Some(plan) = now.next_plan() else {
        anyhow::bail!("there are no power plans");
    };
    apply(&Choice::Plan(plan.guid.clone()))
}
