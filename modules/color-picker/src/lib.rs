//! Color Picker: press the shortcut, point at any pixel on screen with a live
//! magnifier, click to copy its color in the configured format. Picked colors are kept
//! in a history the settings window can show in every format.

pub mod color;
#[cfg(windows)]
mod session;

use anyhow::{anyhow, bail, Result};
use color::Rgb;
use powerkit_config::{ColorFormat, ColorPicker as Settings, StateFile};
#[cfg(windows)]
use powerkit_core::{Event, MouseEvent};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const ID: &str = "color_picker";
const HISTORY_FILE: &str = "color_picker_history";
#[cfg(windows)]
const VK_RETURN: u32 = 0x0D;
#[cfg(windows)]
const VK_ESCAPE: u32 = 0x1B;
/// Same bounds as the `magnifier_zoom` setting.
#[cfg(windows)]
const ZOOM_RANGE: (u32, u32) = (4, 24);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct History {
    /// `#RRGGBB`, newest first.
    colors: Vec<String>,
}

#[derive(Deserialize)]
struct ColorArgs {
    color: String,
    #[serde(default)]
    format: Option<ColorFormat>,
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))
}

fn parse_color(s: &str) -> Result<Rgb> {
    Rgb::parse(s).ok_or_else(|| anyhow!("{s:?} is not a color; use #RRGGBB"))
}

#[cfg(windows)]
fn copy(text: &str) -> Result<()> {
    session::copy(text)
}

#[cfg(not(windows))]
fn copy(_text: &str) -> Result<()> {
    bail!("the clipboard is only available on Windows")
}

#[derive(Default)]
pub struct ColorPicker {
    settings: Settings,
    #[cfg(windows)]
    session: Option<session::Session>,
}

impl ColorPicker {
    pub fn new() -> Self {
        Self::default()
    }

    fn history_file(ctx: &Ctx) -> StateFile {
        StateFile::new(ctx.paths, HISTORY_FILE)
    }

    fn history(&self, ctx: &Ctx) -> Vec<String> {
        let mut h: History = Self::history_file(ctx).load();
        h.colors.truncate(self.settings.history_limit as usize);
        h.colors
    }

    fn save_history(ctx: &Ctx, colors: Vec<String>) -> Result<()> {
        Self::history_file(ctx)
            .save(&History { colors })
            .map_err(|e| anyhow!("Color history not saved: {e}"))
    }

    fn text(&self, c: Rgb, f: ColorFormat) -> String {
        color::format(c, f, &self.settings.custom_format)
    }

    fn picking(&self) -> bool {
        #[cfg(windows)]
        {
            self.session.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    #[cfg(windows)]
    fn begin(&mut self, ctx: &mut Ctx) -> Result<()> {
        if self.session.is_none() {
            self.session = Some(session::Session::start(self.settings.magnifier_zoom)?);
            self.changed(ctx);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn begin(&mut self, _ctx: &mut Ctx) -> Result<()> {
        bail!("picking colors from the screen is only available on Windows")
    }

    #[cfg(windows)]
    fn end(&mut self, ctx: &mut Ctx) {
        if self.session.take().is_some() {
            self.changed(ctx);
        }
    }

    #[cfg(not(windows))]
    fn end(&mut self, _ctx: &mut Ctx) {}

    /// Copy a picked color and add it to the history.
    #[cfg(windows)]
    fn picked(&mut self, c: Rgb, ctx: &mut Ctx) -> Result<Value> {
        let text = self.text(c, self.settings.copy_format);
        copy(&text)?;
        let limit = self.settings.history_limit as usize;
        if limit > 0 {
            let mut colors = self.history(ctx);
            color::remember(&mut colors, &c.hex(), limit);
            Self::save_history(ctx, colors)?;
        }
        ctx.notify("Color Picker", format!("Copied {text}"));
        self.changed(ctx);
        Ok(json!({ "color": c.hex(), "text": text }))
    }

    #[cfg(windows)]
    fn pick_at(&mut self, point: Option<(i32, i32)>, ctx: &mut Ctx) -> Result<()> {
        self.end(ctx);
        let (x, y) = match point {
            Some(p) => p,
            None => session::cursor_pos()?,
        };
        let c = Rgb::from_u32(session::pixel_at(x, y)?);
        self.picked(c, ctx).map(|_| ())
    }

    #[cfg(windows)]
    fn refresh(&mut self, ctx: &mut Ctx) -> Result<()> {
        let (radius, grid, format) = (
            self.settings.magnifier_radius,
            self.settings.show_grid,
            self.settings.copy_format,
        );
        let custom = self.settings.custom_format.clone();
        let Some(s) = self.session.as_mut() else {
            return Ok(());
        };
        if let Err(e) = s.refresh(radius, grid, |v| color::format(Rgb::from_u32(v), format, &custom)) {
            self.end(ctx);
            ctx.notify("Color Picker", format!("Stopped picking: {e:#}"));
            return Err(e);
        }
        Ok(())
    }

    fn changed(&self, ctx: &mut Ctx) {
        ctx.emit("color_picker.changed", self.status());
    }
}

impl Module for ColorPicker {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "Color Picker".into(),
            description: "Pick a color anywhere on screen and copy it in the format you need.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::MouseHook,
                Permission::ScreenCapture,
                Permission::ClipboardWrite,
            ],
            commands: vec![
                cmd("pick", "Pick a color", ""),
                cmd("formats", "Show a color in every format", "Args: {color}"),
                cmd("copy", "Copy a color", "Args: {color, format?}"),
                cmd("history", "List picked colors", ""),
                cmd("remove_history", "Forget a picked color", "Args: {color}"),
                cmd("clear_history", "Forget all picked colors", ""),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.color_picker.clone();
        Ok(())
    }

    fn stop(&mut self, ctx: &mut Ctx) {
        self.end(ctx);
    }

    fn pause(&mut self, ctx: &mut Ctx) {
        self.end(ctx);
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.color_picker.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "pick" => self.begin(ctx),
            _ => bail!("unknown shortcut action {action:?}"),
        }
    }

    #[cfg(windows)]
    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        if !self.picking() {
            return Ok(());
        }
        match *event {
            Event::Timer(id) if self.session.as_ref().is_some_and(|s| s.timer() == id) => self.refresh(ctx)?,
            Event::Mouse(MouseEvent::LeftUp { x, y }) => self.pick_at(Some((x, y)), ctx)?,
            Event::Key(VK_RETURN) => self.pick_at(None, ctx)?,
            Event::Mouse(MouseEvent::RightUp { .. }) | Event::Key(VK_ESCAPE) => self.end(ctx),
            Event::Mouse(MouseEvent::Wheel { delta }) => {
                if let Some(s) = self.session.as_mut() {
                    let step: i32 = if delta > 0 { 2 } else { -2 };
                    s.zoom = (s.zoom as i32 + step).clamp(ZOOM_RANGE.0 as i32, ZOOM_RANGE.1 as i32) as u32;
                }
            }
            // Picking happens on release so the button-up never reaches another window.
            _ => {}
        }
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "pick" => {
                self.begin(ctx)?;
                Ok(Value::Null)
            }
            "formats" => {
                let a: ColorArgs = parse(args)?;
                let c = parse_color(&a.color)?;
                let all: Vec<Value> = ColorFormat::ALL
                    .iter()
                    .map(|f| json!({ "format": f, "text": self.text(c, *f) }))
                    .collect();
                Ok(Value::Array(all))
            }
            "copy" => {
                let a: ColorArgs = parse(args)?;
                let c = parse_color(&a.color)?;
                let text = self.text(c, a.format.unwrap_or(self.settings.copy_format));
                copy(&text)?;
                Ok(json!({ "text": text }))
            }
            "history" => {
                let f = self.settings.copy_format;
                let items: Vec<Value> = self
                    .history(ctx)
                    .into_iter()
                    .filter_map(|h| Rgb::parse(&h))
                    .map(|c| json!({ "color": c.hex(), "text": self.text(c, f) }))
                    .collect();
                Ok(Value::Array(items))
            }
            "remove_history" => {
                let a: ColorArgs = parse(args)?;
                let c = parse_color(&a.color)?;
                let mut colors = self.history(ctx);
                colors.retain(|h| Rgb::parse(h) != Some(c));
                Self::save_history(ctx, colors)?;
                self.changed(ctx);
                Ok(Value::Null)
            }
            "clear_history" => {
                Self::save_history(ctx, Vec::new())?;
                self.changed(ctx);
                Ok(Value::Null)
            }
            _ => bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        json!({ "picking": self.picking() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};
    use powerkit_core::{Event, MouseEvent};
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

        fn started(&mut self) -> ColorPicker {
            let mut m = ColorPicker::new();
            m.start(&mut self.ctx()).unwrap();
            m
        }

        fn run(&mut self, m: &mut ColorPicker, id: &str, args: Value) -> Result<Value> {
            m.command(id, &args, &mut self.ctx())
        }

        fn seed_history(&mut self, colors: &[&str]) {
            let ctx = self.ctx();
            ColorPicker::save_history(&ctx, colors.iter().map(|c| c.to_string()).collect()).unwrap();
        }
    }

    #[test]
    fn formats_lists_every_format_and_uses_custom_template() {
        let mut fx = Fixture::new();
        fx.config.color_picker.custom_format = "0x%X".into();
        let mut m = fx.started();
        let out = fx.run(&mut m, "formats", json!({ "color": "#ff8000" })).unwrap();
        let items = out.as_array().unwrap();
        assert_eq!(items.len(), ColorFormat::ALL.len());
        assert_eq!(items[0], json!({ "format": "hex", "text": "#FF8000" }));
        assert_eq!(
            items.last().unwrap(),
            &json!({ "format": "custom", "text": "0xFF8000" })
        );
    }

    #[test]
    fn invalid_colors_are_rejected_with_a_hint() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        let e = fx.run(&mut m, "formats", json!({ "color": "red" })).unwrap_err();
        assert!(e.to_string().contains("#RRGGBB"), "{e}");
        assert!(fx.run(&mut m, "formats", json!({})).is_err());
    }

    #[test]
    fn history_uses_copy_format_and_respects_limit() {
        let mut fx = Fixture::new();
        fx.config.color_picker.copy_format = ColorFormat::Rgb;
        fx.config.color_picker.history_limit = 2;
        fx.seed_history(&["#FF0000", "#00FF00", "#0000FF"]);
        let mut m = fx.started();
        let out = fx.run(&mut m, "history", Value::Null).unwrap();
        assert_eq!(
            out,
            json!([
                { "color": "#FF0000", "text": "rgb(255, 0, 0)" },
                { "color": "#00FF00", "text": "rgb(0, 255, 0)" },
            ])
        );
    }

    #[test]
    fn remove_and_clear_history() {
        let mut fx = Fixture::new();
        fx.seed_history(&["#FF0000", "#00FF00"]);
        let mut m = fx.started();
        fx.run(&mut m, "remove_history", json!({ "color": "#f00" })).unwrap();
        let out = fx.run(&mut m, "history", Value::Null).unwrap();
        assert_eq!(out, json!([{ "color": "#00FF00", "text": "#00FF00" }]));
        assert!(fx
            .actions
            .iter()
            .any(|a| matches!(a, Action::Emit { event, .. } if event == "color_picker.changed")));
        fx.run(&mut m, "clear_history", Value::Null).unwrap();
        assert_eq!(fx.run(&mut m, "history", Value::Null).unwrap(), json!([]));
    }

    #[test]
    fn unknown_shortcut_and_command_fail() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        assert!(m.on_hotkey("nope", &mut fx.ctx()).is_err());
        assert!(fx.run(&mut m, "nope", Value::Null).is_err());
    }

    #[test]
    fn events_are_ignored_while_not_picking() {
        let mut fx = Fixture::new();
        let mut m = fx.started();
        m.on_event(&Event::Mouse(MouseEvent::LeftUp { x: 1, y: 1 }), &mut fx.ctx())
            .unwrap();
        m.on_event(&Event::Key(0x0D), &mut fx.ctx()).unwrap();
        assert!(fx.actions.is_empty());
        assert_eq!(m.status(), json!({ "picking": false }));
    }
}
