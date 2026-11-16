//! Power Display: brightness, contrast, volume, input, color preset and power state of
//! external monitors, set over DDC/CI from one window.

#[cfg(windows)]
mod live;

use anyhow::{bail, Context as _, Result};
use powerkit_config::PowerDisplay as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const ID: &str = "power_display";

/// A monitor control, by its VESA MCCS (VCP) code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feature {
    Brightness,
    Contrast,
    Volume,
    Input,
    Color,
    Power,
}

impl Feature {
    pub const ALL: [Feature; 6] = [
        Feature::Brightness,
        Feature::Contrast,
        Feature::Volume,
        Feature::Input,
        Feature::Color,
        Feature::Power,
    ];

    pub fn code(self) -> u8 {
        match self {
            Feature::Brightness => 0x10,
            Feature::Contrast => 0x12,
            Feature::Volume => 0x62,
            Feature::Input => 0x60,
            Feature::Color => 0x14,
            Feature::Power => 0xD6,
        }
    }

    /// The name used in commands and status.
    pub fn name(self) -> &'static str {
        match self {
            Feature::Brightness => "brightness",
            Feature::Contrast => "contrast",
            Feature::Volume => "volume",
            Feature::Input => "input",
            Feature::Color => "color",
            Feature::Power => "power",
        }
    }

    pub fn from_name(name: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(name))
    }

    pub fn label(self) -> &'static str {
        match self {
            Feature::Brightness => "Brightness",
            Feature::Contrast => "Contrast",
            Feature::Volume => "Volume",
            Feature::Input => "Input source",
            Feature::Color => "Color preset",
            Feature::Power => "Power",
        }
    }

    /// Levels are sliders; the rest pick one of the values the monitor lists.
    pub fn is_level(self) -> bool {
        matches!(self, Feature::Brightness | Feature::Contrast | Feature::Volume)
    }

    /// Why a change asks first; `None` for changes that are safe to make at once.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Feature::Input => Some(
                "The monitor stops showing this PC until it is switched back, which may need the monitor's own buttons.",
            ),
            Feature::Power => Some("The monitor goes dark and may only come back on with its own power button."),
            Feature::Color => Some("Some monitors reset their brightness and contrast when the preset changes."),
            _ => None,
        }
    }

    pub fn shown(self, s: &Settings) -> bool {
        match self {
            Feature::Brightness => true,
            Feature::Contrast => s.contrast,
            Feature::Volume => s.volume,
            Feature::Input => s.input_source,
            Feature::Color => s.color_preset,
            Feature::Power => s.power_state,
        }
    }

    /// The name of one of this control's values.
    pub fn value_name(self, v: u32) -> String {
        let known = match (self, v) {
            (Feature::Input, 0x01) => "VGA 1",
            (Feature::Input, 0x02) => "VGA 2",
            (Feature::Input, 0x03) => "DVI 1",
            (Feature::Input, 0x04) => "DVI 2",
            (Feature::Input, 0x05) => "Composite 1",
            (Feature::Input, 0x06) => "Composite 2",
            (Feature::Input, 0x07) => "S-Video 1",
            (Feature::Input, 0x08) => "S-Video 2",
            (Feature::Input, 0x09) => "Tuner 1",
            (Feature::Input, 0x0A) => "Tuner 2",
            (Feature::Input, 0x0B) => "Tuner 3",
            (Feature::Input, 0x0C) => "Component 1",
            (Feature::Input, 0x0D) => "Component 2",
            (Feature::Input, 0x0E) => "Component 3",
            (Feature::Input, 0x0F) => "DisplayPort 1",
            (Feature::Input, 0x10) => "DisplayPort 2",
            (Feature::Input, 0x11) => "HDMI 1",
            (Feature::Input, 0x12) => "HDMI 2",
            (Feature::Color, 0x01) => "sRGB",
            (Feature::Color, 0x02) => "Native",
            (Feature::Color, 0x03) => "4000 K",
            (Feature::Color, 0x04) => "5000 K",
            (Feature::Color, 0x05) => "6500 K",
            (Feature::Color, 0x06) => "7500 K",
            (Feature::Color, 0x07) => "8200 K",
            (Feature::Color, 0x08) => "9300 K",
            (Feature::Color, 0x09) => "10000 K",
            (Feature::Color, 0x0A) => "11500 K",
            (Feature::Color, 0x0B) => "User 1",
            (Feature::Color, 0x0C) => "User 2",
            (Feature::Color, 0x0D) => "User 3",
            (Feature::Power, 0x01) => "On",
            (Feature::Power, 0x02) => "Standby",
            (Feature::Power, 0x03) => "Suspend",
            (Feature::Power, 0x04) => "Off",
            (Feature::Power, 0x05) => "Off (power button)",
            _ => "",
        };
        if known.is_empty() {
            format!("{} {v:#04X}", self.label())
        } else {
            known.to_string()
        }
    }
}

/// The VCP codes a monitor's capabilities string lists, each with the values it lists
/// for it. `None` when the string has no `vcp(...)` part.
pub fn parse_vcp(caps: &str) -> Option<BTreeMap<u8, Vec<u8>>> {
    let start = caps.to_ascii_lowercase().find("vcp(")? + 4;
    let mut map: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
    let mut depth = 1;
    let mut last = None;
    let mut digits = String::new();
    // Values are bytes: pairs of hex digits, which some monitors run together.
    let mut flush = |digits: &mut String, depth: i32, map: &mut BTreeMap<u8, Vec<u8>>| {
        if let Ok(v) = u8::from_str_radix(digits, 16) {
            if depth == 1 {
                map.entry(v).or_default();
                last = Some(v);
            } else if let (2, Some(code)) = (depth, last) {
                map.entry(code).or_default().push(v);
            }
        }
        digits.clear();
    };
    for c in caps[start..].chars() {
        match c {
            c if c.is_ascii_hexdigit() => {
                digits.push(c);
                if digits.len() == 2 {
                    flush(&mut digits, depth, &mut map);
                }
            }
            '(' => {
                flush(&mut digits, depth, &mut map);
                depth += 1;
            }
            ')' => {
                flush(&mut digits, depth, &mut map);
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => flush(&mut digits, depth, &mut map),
        }
    }
    Some(map)
}

/// What one control on a monitor is set to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Level {
        current: u32,
        max: u32,
    },
    /// One of `options`, which always includes `current`.
    Pick {
        current: u32,
        options: Vec<u32>,
    },
}

impl Reading {
    /// A pick from the listed values, adding the current one when it is not listed.
    pub fn pick(current: u32, listed: &[u8]) -> Reading {
        let mut options: Vec<u32> = listed.iter().map(|v| *v as u32).collect();
        if !options.contains(&current) {
            options.insert(0, current);
        }
        Reading::Pick { current, options }
    }

    fn json(&self, f: Feature) -> Value {
        match self {
            Reading::Level { current, max } => json!({ "value": current, "max": max }),
            Reading::Pick { current, options } => json!({
                "value": current,
                "name": f.value_name(*current),
                "options": options.iter().map(|v| json!({ "value": v, "name": f.value_name(*v) })).collect::<Vec<_>>(),
            }),
        }
    }
}

/// One physical monitor and the controls it answered for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
    pub device: String,
    pub controls: Vec<(Feature, Reading)>,
}

impl Monitor {
    /// The controls the settings ask to show, in order.
    pub fn shown<'a>(&'a self, s: &'a Settings) -> impl Iterator<Item = &'a (Feature, Reading)> + 'a {
        self.controls.iter().filter(|(f, _)| f.shown(s))
    }

    pub fn reading(&self, f: Feature) -> Option<&Reading> {
        self.controls.iter().find(|(g, _)| *g == f).map(|(_, r)| r)
    }

    /// Record a value that was just set.
    pub fn update(&mut self, f: Feature, value: u32) {
        if let Some((_, Reading::Level { current, .. } | Reading::Pick { current, .. })) =
            self.controls.iter_mut().find(|(g, _)| *g == f)
        {
            *current = value;
        }
    }

    fn json(&self) -> Value {
        let controls: serde_json::Map<String, Value> = self
            .controls
            .iter()
            .map(|(f, r)| (f.name().to_string(), r.json(*f)))
            .collect();
        json!({ "name": self.name, "device": self.device, "controls": controls })
    }
}

/// Checks a requested value against what the monitor reported: levels up to their
/// maximum, picks only from the listed values.
pub fn validate(m: &Monitor, f: Feature, value: u32) -> Result<()> {
    match m.reading(f) {
        None => bail!("{} does not report its {}", m.name, f.label().to_lowercase()),
        Some(Reading::Level { max, .. }) if value > *max => {
            bail!("{} goes up to {max} on {}", f.label(), m.name)
        }
        Some(Reading::Pick { options, .. }) if !options.contains(&value) => {
            bail!("{} does not list {} {value:#04X}", m.name, f.label().to_lowercase())
        }
        Some(_) => Ok(()),
    }
}

/// The status line for a finished reading.
pub fn summary(monitors: &[Monitor]) -> String {
    if monitors.iter().all(|m| m.controls.is_empty()) {
        "No monitor answered over DDC/CI. Turn DDC/CI on in the monitor's own menu. \
         Built-in laptop screens are not supported."
            .into()
    } else {
        String::new()
    }
}

#[derive(Default)]
pub struct PowerDisplay {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: live::Live,
}

impl PowerDisplay {
    pub fn new() -> Self {
        Self::default()
    }

    fn check_running(&self) -> Result<()> {
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Power Display is not running");
        }
        Ok(())
    }

    fn set(&mut self, args: &Value) -> Result<()> {
        let feature = args["feature"].as_str().context("\"feature\" is required")?;
        let f = Feature::from_name(feature).with_context(|| format!("unknown feature {feature:?}"))?;
        if !f.shown(&self.settings) {
            bail!("{} is turned off in the Power Display settings", f.label());
        }
        let value = args["value"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .context("\"value\" must be a whole number")?;
        let display = args["display"]
            .as_u64()
            .context("\"display\" is the monitor's index in the status")?;
        #[cfg(windows)]
        self.live.set(display as usize, f, value)?;
        #[cfg(not(windows))]
        let _ = (display, value);
        Ok(())
    }
}

impl Module for PowerDisplay {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Power Display".into(),
            description: "Set the brightness, contrast, volume and input of your monitors.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys],
            commands: vec![
                CommandSpec {
                    id: "open".into(),
                    title: "Open Power Display".into(),
                    description: String::new(),
                },
                CommandSpec {
                    id: "refresh".into(),
                    title: "Read the monitors again".into(),
                    description: String::new(),
                },
                CommandSpec {
                    id: "set".into(),
                    title: "Set a monitor control".into(),
                    description: "{\"display\": 0, \"feature\": \"brightness\", \"value\": 60}".into(),
                },
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.power_display.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        #[cfg(windows)]
        self.live.close();
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        #[cfg(windows)]
        self.live.close();
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.power_display.clone();
        #[cfg(windows)]
        if self.settings.enabled {
            self.live.render(&self.settings)?;
        } else {
            self.live.close();
        }
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        match action {
            "open" => {
                self.check_running()?;
                #[cfg(windows)]
                self.live.toggle(&self.settings)?;
                Ok(())
            }
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        self.live.on_event(event, &self.settings, ctx)?;
        #[cfg(not(windows))]
        let _ = (event, ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        self.check_running()?;
        match id {
            #[cfg(windows)]
            "open" => self.live.open(&self.settings)?,
            #[cfg(windows)]
            "refresh" => self.live.refresh()?,
            #[cfg(not(windows))]
            "open" | "refresh" => {}
            "set" => self.set(args)?,
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let (monitors, busy, message) = (self.live.monitors(), self.live.busy(), self.live.message());
        #[cfg(not(windows))]
        let (monitors, busy, message): (&[Monitor], bool, &str) = (&[], false, "");
        json!({
            "running": self.running && self.settings.enabled,
            "busy": busy,
            "message": message,
            "monitors": monitors.iter().map(Monitor::json).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::Config;

    const ACER: &str = "(prot(monitor)type(LCD)model(ACER)cmds(01 02 03 07 0C E3 F3)vcp(04 10 12 14(05 06 08 0B) 16 18 1A 59 5A 5B 5C 5D 5E 60(01 11) 62 6C 6E 70 8D 9B 9C 9D 9E 9F A0 CC(01 02 03 04 05 06 07 08 09 0A 0C 0D 0E 14 16 1E 24) D6(01 02 04 05) E0(00 04 05) E1(00 01 02)E2(00 01 02 03 04 05 06 07 10 11 12) E3 E4 E5 E7(00 01 02) E8(00 01 02 03 04)) mswhql(1)asset_eep(40)mccs_ver(2.2))";

    #[test]
    fn capabilities_list_codes_and_values() {
        let vcp = parse_vcp(ACER).unwrap();
        assert_eq!(vcp[&0x10], Vec::<u8>::new());
        assert_eq!(vcp[&0x60], [0x01, 0x11]);
        assert_eq!(vcp[&0x14], [0x05, 0x06, 0x08, 0x0B]);
        assert_eq!(vcp[&0xD6], [0x01, 0x02, 0x04, 0x05]);
        assert_eq!(vcp[&0xE1], [0x00, 0x01, 0x02], "no space before the next code");
        assert!(vcp.contains_key(&0xE2) && vcp.contains_key(&0xE8));
        assert!(!vcp.contains_key(&0x01), "cmds(...) is not part of vcp(...)");
        assert!(!vcp.contains_key(&0x22), "mccs_ver after vcp(...) is ignored");
    }

    #[test]
    fn run_together_codes_are_split_into_bytes() {
        let vcp = parse_vcp("(vcp(021012 60(0F1112)))").unwrap();
        assert_eq!(vcp.keys().copied().collect::<Vec<_>>(), [0x02, 0x10, 0x12, 0x60]);
        assert_eq!(vcp[&0x60], [0x0F, 0x11, 0x12]);
        assert_eq!(parse_vcp("(prot(monitor)type(lcd))"), None);
    }

    #[test]
    fn values_have_names() {
        assert_eq!(Feature::Input.value_name(0x11), "HDMI 1");
        assert_eq!(Feature::Input.value_name(0x1B), "Input source 0x1B");
        assert_eq!(Feature::Color.value_name(0x05), "6500 K");
        assert_eq!(Feature::Power.value_name(0x04), "Off");
        assert_eq!(Feature::from_name("Brightness"), Some(Feature::Brightness));
        assert_eq!(Feature::from_name("gamma"), None);
    }

    fn monitor() -> Monitor {
        Monitor {
            name: "Acer".into(),
            device: r"\\.\DISPLAY1".into(),
            controls: vec![
                (Feature::Brightness, Reading::Level { current: 50, max: 100 }),
                (Feature::Volume, Reading::Level { current: 10, max: 100 }),
                (Feature::Input, Reading::pick(0x0F, &[0x01, 0x11])),
                (Feature::Power, Reading::pick(0x01, &[0x01, 0x04])),
            ],
        }
    }

    #[test]
    fn risky_controls_are_hidden_until_asked_for() {
        let m = monitor();
        let mut s = Settings::default();
        let shown = |s: &Settings| m.shown(s).map(|(f, _)| *f).collect::<Vec<_>>();
        assert_eq!(shown(&s), [Feature::Brightness, Feature::Volume]);
        s.volume = false;
        s.input_source = true;
        assert_eq!(shown(&s), [Feature::Brightness, Feature::Input]);
        assert!(Feature::Input.warning().is_some() && Feature::Power.warning().is_some());
        assert!(Feature::Brightness.warning().is_none());
    }

    #[test]
    fn the_current_input_is_always_an_option() {
        assert_eq!(
            monitor().reading(Feature::Input),
            Some(&Reading::Pick {
                current: 0x0F,
                options: vec![0x0F, 0x01, 0x11]
            })
        );
    }

    #[test]
    fn requests_are_checked_against_the_monitor() {
        let mut m = monitor();
        assert!(validate(&m, Feature::Brightness, 100).is_ok());
        assert!(validate(&m, Feature::Brightness, 101).is_err());
        assert!(validate(&m, Feature::Contrast, 10).is_err(), "not reported");
        assert!(validate(&m, Feature::Input, 0x11).is_ok());
        assert!(validate(&m, Feature::Input, 0x12).is_err(), "not listed");
        m.update(Feature::Brightness, 70);
        assert_eq!(
            m.reading(Feature::Brightness),
            Some(&Reading::Level { current: 70, max: 100 })
        );
    }

    #[test]
    fn no_answer_explains_why() {
        assert!(summary(&[]).contains("DDC/CI"));
        assert_eq!(summary(&[monitor()]), "");
    }

    #[test]
    fn the_shortcut_is_registered() {
        let mut c = Config::new();
        let ours: Vec<_> = c.shortcuts().into_iter().filter(|(m, _, _)| *m == ID).collect();
        assert_eq!(ours, [(ID, "open".to_string(), "Win+Alt+M".to_string())]);
        assert!(c.sanitize().is_empty());
    }
}
