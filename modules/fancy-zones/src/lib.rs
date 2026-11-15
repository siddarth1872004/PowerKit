//! FancyZones: each monitor gets a layout of zones. Hold the drag modifier while
//! moving a window to see the zones and drop the window into one (hold the span key as
//! well to cover several), or move the active window between zones with shortcuts.

pub mod engine;
#[cfg(windows)]
mod snap;

use anyhow::{anyhow, bail, Result};
use powerkit_config::{FancyZones as Settings, Layout};
#[cfg(windows)]
use powerkit_core::Event;
use powerkit_core::Rect;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};

pub const ID: &str = "fancy_zones";

#[derive(Deserialize)]
struct PreviewArgs {
    layout: String,
    width: i32,
    height: i32,
}

#[derive(Deserialize)]
struct SetLayoutArgs {
    monitor: String,
    layout: String,
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))
}

/// The layout after `current` in the configured order, wrapping around.
pub fn next_layout<'a>(settings: &'a Settings, current: &str) -> &'a Layout {
    let i = settings.layouts.iter().position(|l| l.id == current);
    let next = i.map_or(0, |i| (i + 1) % settings.layouts.len());
    &settings.layouts[next]
}

/// Which hotkey action this is: a direction, a zone number (0-based) or something else.
#[derive(Debug, PartialEq, Eq)]
enum Shortcut {
    Step { forward: bool },
    Zone(usize),
    CycleLayout,
    ToggleOverlay,
}

fn shortcut(action: &str) -> Result<Shortcut> {
    Ok(match action {
        "next_zone" => Shortcut::Step { forward: true },
        "prev_zone" => Shortcut::Step { forward: false },
        "cycle_layout" => Shortcut::CycleLayout,
        "toggle_overlay" => Shortcut::ToggleOverlay,
        _ => match action.strip_prefix("zone_").and_then(|n| n.parse::<usize>().ok()) {
            Some(n @ 1..=9) => Shortcut::Zone(n - 1),
            _ => bail!("unknown shortcut action {action:?}"),
        },
    })
}

fn rect_json(r: &Rect) -> Value {
    json!({ "x": r.left, "y": r.top, "width": r.width(), "height": r.height() })
}

#[derive(Default)]
pub struct FancyZones {
    settings: Settings,
    paused: bool,
    #[cfg(windows)]
    snap: Option<snap::Snapper>,
}

impl FancyZones {
    pub fn new() -> Self {
        Self::default()
    }

    fn layout(&self, id: &str) -> Result<&Layout> {
        self.settings
            .layout(id)
            .ok_or_else(|| anyhow!("there is no layout {id:?}"))
    }

    #[cfg(windows)]
    fn snapper(&mut self) -> &mut snap::Snapper {
        self.snap.get_or_insert_with(snap::Snapper::default)
    }

    #[cfg(windows)]
    fn halt(&mut self) {
        if let Some(s) = self.snap.as_mut() {
            s.cancel();
        }
    }

    #[cfg(not(windows))]
    fn halt(&mut self) {}

    #[cfg(windows)]
    fn run_shortcut(&mut self, s: Shortcut, ctx: &mut Ctx) -> Result<()> {
        let settings = self.settings.clone();
        let snap = self.snapper();
        match s {
            Shortcut::Step { forward } => snap.step_foreground(&settings, forward),
            Shortcut::Zone(i) => snap.foreground_to_zone(&settings, i),
            Shortcut::ToggleOverlay => snap.toggle_overlay(&settings),
            Shortcut::CycleLayout => {
                let (monitor, layout) = snap.cycle_layout(&settings)?;
                let name = self.layout(&layout)?.name.clone();
                self.settings.monitor_layouts.insert(monitor.clone(), layout.clone());
                ctx.update_config(move |c| {
                    c.fancy_zones.monitor_layouts.insert(monitor, layout);
                });
                ctx.notify("FancyZones", format!("Layout: {name}"));
                Ok(())
            }
        }
    }

    #[cfg(not(windows))]
    fn run_shortcut(&mut self, _s: Shortcut, _ctx: &mut Ctx) -> Result<()> {
        bail!("moving windows into zones is only available on Windows")
    }

    #[cfg(windows)]
    fn monitors(&self) -> Result<Value> {
        let list = powerkit_platform::monitor::enumerate()?;
        Ok(list
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "primary": m.primary,
                    "work_area": rect_json(&m.work_area),
                    "dpi": m.dpi,
                    "layout": self.settings.layout_for_monitor(&m.id).id,
                })
            })
            .collect())
    }

    #[cfg(not(windows))]
    fn monitors(&self) -> Result<Value> {
        bail!("listing monitors is only available on Windows")
    }
}

impl Module for FancyZones {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "FancyZones".into(),
            description: "Arrange windows into zones by dragging them or with shortcuts.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys, Permission::WindowManagement],
            commands: vec![
                cmd("layouts", "List layouts", ""),
                cmd(
                    "preview",
                    "Zone rectangles of a layout",
                    "Args: {layout, width, height}; pixels at 96 DPI",
                ),
                cmd("monitors", "List monitors and their layouts", ""),
                cmd("set_layout", "Assign a layout to a monitor", "Args: {monitor, layout}"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.fancy_zones.clone();
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        #[cfg(windows)]
        {
            self.snap = None;
        }
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        self.halt();
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.fancy_zones.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        let s = shortcut(action)?;
        if self.paused {
            return Ok(());
        }
        self.run_shortcut(s, ctx)
    }

    #[cfg(windows)]
    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) -> Result<()> {
        if self.paused {
            return Ok(());
        }
        let settings = self.settings.clone();
        match self.snap.as_mut() {
            Some(s) => s.on_event(event, &settings),
            None if snap::starts_drag(event) => self.snapper().on_event(event, &settings),
            None => Ok(()),
        }
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "layouts" => Ok(self
                .settings
                .layouts
                .iter()
                .map(|l| {
                    json!({
                        "id": l.id,
                        "name": l.name,
                        "kind": l.kind,
                        "zones": l.zones.len(),
                        "default": l.id == self.settings.default_layout,
                    })
                })
                .collect()),
            "preview" => {
                let a: PreviewArgs = parse(args)?;
                if !(1..=20_000).contains(&a.width) || !(1..=20_000).contains(&a.height) {
                    bail!("width and height must be between 1 and 20000");
                }
                let l = self.layout(&a.layout)?;
                let area = Rect::from_size(0, 0, a.width, a.height);
                let rects = engine::zone_rects(l, area, l.margin as i32, l.spacing as i32);
                Ok(rects.iter().map(rect_json).collect())
            }
            "monitors" => self.monitors(),
            "set_layout" => {
                let a: SetLayoutArgs = parse(args)?;
                self.layout(&a.layout)?;
                if a.monitor.is_empty() {
                    bail!("monitor id is empty");
                }
                self.settings
                    .monitor_layouts
                    .insert(a.monitor.clone(), a.layout.clone());
                ctx.update_config(move |c| {
                    c.fancy_zones.monitor_layouts.insert(a.monitor, a.layout);
                });
                Ok(Value::Null)
            }
            _ => bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        if let Some(s) = self.snap.as_ref() {
            return s.status();
        }
        json!({ "dragging": false, "overlay": false, "snapped": 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};
    use powerkit_plugin_api::Action;

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

        fn started(&mut self) -> FancyZones {
            let mut m = FancyZones::new();
            m.start(&mut self.ctx()).unwrap();
            m
        }

        fn run(&mut self, m: &mut FancyZones, id: &str, args: Value) -> Result<Value> {
            m.command(id, &args, &mut self.ctx())
        }

        /// Apply queued config updates, as the host would.
        fn apply_updates(&mut self) {
            for a in self.actions.drain(..) {
                if let Action::UpdateConfig(f) = a {
                    f(&mut self.config);
                }
            }
        }
    }

    #[test]
    fn lists_layouts_and_marks_the_default() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        let out = fx.run(&mut m, "layouts", Value::Null).unwrap();
        let items = out.as_array().unwrap();
        assert_eq!(items.len(), Layout::builtin().len());
        assert_eq!(
            items[0],
            json!({ "id": "priority-3", "name": "Priority (3 zones)", "kind": "priority", "zones": 3, "default": true })
        );
        assert!(items[1..].iter().all(|l| l["default"] == json!(false)));
    }

    #[test]
    fn preview_uses_layout_spacing() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        let out = fx
            .run(
                &mut m,
                "preview",
                json!({ "layout": "columns-2", "width": 1000, "height": 500 }),
            )
            .unwrap();
        assert_eq!(
            out,
            json!([
                { "x": 8, "y": 8, "width": 488, "height": 484 },
                { "x": 504, "y": 8, "width": 488, "height": 484 },
            ])
        );
        assert!(fx
            .run(
                &mut m,
                "preview",
                json!({ "layout": "nope", "width": 10, "height": 10 })
            )
            .is_err());
        assert!(fx
            .run(
                &mut m,
                "preview",
                json!({ "layout": "columns-2", "width": 0, "height": 10 })
            )
            .is_err());
    }

    #[test]
    fn set_layout_updates_config_and_rejects_unknown_layouts() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        let e = fx
            .run(
                &mut m,
                "set_layout",
                json!({ "monitor": "\\\\.\\DISPLAY1", "layout": "nope" }),
            )
            .unwrap_err();
        assert!(e.to_string().contains("nope"), "{e}");
        fx.run(
            &mut m,
            "set_layout",
            json!({ "monitor": "\\\\.\\DISPLAY1", "layout": "grid-2x2" }),
        )
        .unwrap();
        fx.apply_updates();
        assert_eq!(
            fx.config.fancy_zones.layout_for_monitor("\\\\.\\DISPLAY1").id,
            "grid-2x2"
        );
    }

    #[test]
    fn next_layout_wraps_and_recovers_from_unknown_ids() {
        let s = Settings::default();
        assert_eq!(next_layout(&s, "priority-3").id, "columns-2");
        assert_eq!(next_layout(&s, "priority-4").id, "priority-3");
        assert_eq!(next_layout(&s, "gone").id, "priority-3");
    }

    #[test]
    fn shortcut_actions() {
        assert_eq!(shortcut("next_zone").unwrap(), Shortcut::Step { forward: true });
        assert_eq!(shortcut("zone_1").unwrap(), Shortcut::Zone(0));
        assert_eq!(shortcut("zone_9").unwrap(), Shortcut::Zone(8));
        assert!(shortcut("zone_0").is_err());
        assert!(shortcut("zone_10").is_err());
        assert!(shortcut("nope").is_err());
    }

    #[test]
    fn hotkeys_come_from_config() {
        let mut fx = Fixture::new();
        let m = fx.started();
        let keys = m.hotkeys(&fx.config);
        assert_eq!(keys.len(), 4 + 9);
        assert!(keys
            .iter()
            .any(|k| k.action == "zone_3" && k.shortcut == "Win+Ctrl+Alt+3"));
        let mut m2 = fx.started();
        assert!(m2.on_hotkey("nope", &mut fx.ctx()).is_err());
    }

    #[test]
    fn paused_module_ignores_shortcuts() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        m.pause(&mut fx.ctx());
        m.on_hotkey("next_zone", &mut fx.ctx()).unwrap();
        assert!(fx.actions.is_empty());
        assert_eq!(m.status(), json!({ "dragging": false, "overlay": false, "snapped": 0 }));
    }
}
