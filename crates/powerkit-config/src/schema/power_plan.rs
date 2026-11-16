use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerPlan {
    pub enabled: bool,
    /// Shows the power plans and power modes at the cursor.
    pub shortcut: String,
    /// Switches straight to the next power plan. Empty for none.
    pub next_shortcut: String,
    /// Say which plan is on after switching.
    pub notify: bool,
}

impl Default for PowerPlan {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Ctrl+Alt+P".into(),
            next_shortcut: String::new(),
            notify: true,
        }
    }
}

impl PowerPlan {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = PowerPlan::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "Power Plan Switcher", w);
        fix_shortcut(&mut self.next_shortcut, &d.next_shortcut, "Power Plan Switcher", w);
    }
}
