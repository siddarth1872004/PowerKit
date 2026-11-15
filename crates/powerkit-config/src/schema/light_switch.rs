use super::{clamp, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightSwitchMode {
    /// Only the shortcut changes the theme.
    #[default]
    Manual,
    /// Light from `light_time`, dark from `dark_time`.
    FixedHours,
    /// Light from sunrise, dark from sunset at `latitude`/`longitude`, each shifted by
    /// its offset.
    SunsetToSunrise,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightSwitch {
    pub enabled: bool,
    pub mode: LightSwitchMode,
    /// Toggles between light and dark; the schedule takes over again at its next change.
    pub shortcut: String,
    /// Local time `HH:MM`.
    pub light_time: String,
    /// Local time `HH:MM`.
    pub dark_time: String,
    /// Degrees, -90 (south) to 90 (north).
    pub latitude: f64,
    /// Degrees, -180 (west) to 180 (east).
    pub longitude: f64,
    /// Minutes added to sunrise, -180 to 180.
    pub sunrise_offset_minutes: i32,
    /// Minutes added to sunset, -180 to 180.
    pub sunset_offset_minutes: i32,
    /// Switch the theme of applications.
    pub change_apps: bool,
    /// Switch the theme of the taskbar and Start menu.
    pub change_system: bool,
}

impl Default for LightSwitch {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: LightSwitchMode::Manual,
            shortcut: "Win+Ctrl+Shift+D".into(),
            light_time: "07:00".into(),
            dark_time: "20:00".into(),
            latitude: 0.0,
            longitude: 0.0,
            sunrise_offset_minutes: 0,
            sunset_offset_minutes: 0,
            change_apps: true,
            change_system: true,
        }
    }
}

/// Minutes after midnight for `HH:MM` (24-hour clock).
pub fn minutes_of_day(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let ok = |p: &str| (1..=2).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit());
    if !ok(h) || m.len() != 2 || !ok(m) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn fix_time(value: &mut String, default: &str, what: &str, w: &mut Vec<String>) {
    match minutes_of_day(value) {
        Some(t) => *value = format!("{:02}:{:02}", t / 60, t % 60),
        None => {
            w.push(format!("{what}: {value:?} is not a HH:MM time; restored {default}"));
            *value = default.to_string();
        }
    }
}

fn fix_degrees(value: &mut f64, limit: f64, what: &str, w: &mut Vec<String>) {
    if !value.is_finite() {
        w.push(format!("{what}: not a number; using 0"));
        *value = 0.0;
    }
    clamp(value, -limit, limit, what, w);
}

impl LightSwitch {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = Self::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "Light Switch", w);
        fix_time(&mut self.light_time, &d.light_time, "Light Switch light time", w);
        fix_time(&mut self.dark_time, &d.dark_time, "Light Switch dark time", w);
        if self.light_time == self.dark_time {
            w.push("Light Switch: light and dark times are equal; restored the defaults".into());
            self.light_time = d.light_time;
            self.dark_time = d.dark_time;
        }
        fix_degrees(&mut self.latitude, 90.0, "Light Switch latitude", w);
        fix_degrees(&mut self.longitude, 180.0, "Light Switch longitude", w);
        clamp(
            &mut self.sunrise_offset_minutes,
            -180,
            180,
            "Light Switch sunrise offset",
            w,
        );
        clamp(
            &mut self.sunset_offset_minutes,
            -180,
            180,
            "Light Switch sunset offset",
            w,
        );
        if !self.change_apps && !self.change_system {
            w.push("Light Switch: nothing selected to switch; enabled applications".into());
            self.change_apps = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(minutes_of_day("07:30"), Some(450));
        assert_eq!(minutes_of_day("7:05"), Some(425));
        assert_eq!(minutes_of_day("23:59"), Some(1439));
        for bad in ["24:00", "12:60", "12:5", "ab:cd", "1230", "-1:00", "", "+1:00"] {
            assert_eq!(minutes_of_day(bad), None, "{bad}");
        }
    }

    #[test]
    fn sanitize_repairs_values() {
        let mut s = LightSwitch {
            light_time: "7:00".into(),
            dark_time: "25:00".into(),
            latitude: f64::NAN,
            longitude: 200.0,
            sunset_offset_minutes: -500,
            change_apps: false,
            change_system: false,
            ..LightSwitch::default()
        };
        let mut w = Vec::new();
        s.sanitize(&mut w);
        assert_eq!((s.light_time.as_str(), s.dark_time.as_str()), ("07:00", "20:00"));
        assert_eq!((s.latitude, s.longitude), (0.0, 180.0));
        assert_eq!(s.sunset_offset_minutes, -180);
        assert!(s.change_apps);
        assert_eq!(w.len(), 5, "{w:?}");
    }

    #[test]
    fn equal_times_are_rejected() {
        let mut s = LightSwitch {
            light_time: "08:00".into(),
            dark_time: "8:00".into(),
            ..LightSwitch::default()
        };
        let mut w = Vec::new();
        s.sanitize(&mut w);
        assert_eq!(s.light_time, "07:00");
        assert_eq!(w.len(), 1, "{w:?}");
    }
}
