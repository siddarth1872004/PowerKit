use super::clamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwakeMode {
    /// Follow the Windows power plan.
    #[default]
    Off,
    /// Keep the computer awake until the mode is changed.
    Indefinite,
    /// Keep awake for `duration_minutes` from the moment the mode was chosen.
    Timed,
    /// Keep awake until the instant `expire_at`.
    Until,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Awake {
    pub enabled: bool,
    pub mode: AwakeMode,
    /// Also keep the screen on, not only the system.
    pub keep_display_on: bool,
    /// Length of a timed session, 1 minute to 7 days.
    pub duration_minutes: u32,
    /// End of an `until` session as Unix seconds (UTC).
    pub expire_at: i64,
}

impl Default for Awake {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: AwakeMode::Off,
            keep_display_on: false,
            duration_minutes: 60,
            expire_at: 0,
        }
    }
}

impl Awake {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        clamp(&mut self.duration_minutes, 1, 7 * 24 * 60, "Awake duration", w);
        if self.expire_at < 0 {
            w.push(format!("Awake: end time {} is invalid; cleared it", self.expire_at));
            self.expire_at = 0;
        }
        if self.mode == AwakeMode::Until && self.expire_at == 0 {
            w.push("Awake: \"until\" mode has no end time; switched Awake off".into());
            self.mode = AwakeMode::Off;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repairs_values() {
        let mut a = Awake {
            mode: AwakeMode::Until,
            duration_minutes: 0,
            expire_at: -5,
            ..Awake::default()
        };
        let mut w = Vec::new();
        a.sanitize(&mut w);
        assert_eq!(a.duration_minutes, 1);
        assert_eq!(a.expire_at, 0);
        assert_eq!(a.mode, AwakeMode::Off);
        assert_eq!(w.len(), 3, "{w:?}");
    }
}
