//! Text Extractor: press the shortcut, drag a box over any part of the screen, and
//! the text in it is read by the OCR engine built into Windows and copied.

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_config::TextExtractor as Settings;
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "text_extractor";
/// Smaller drags are taken as a click and cancel.
pub const MIN_SIZE: i32 = 6;

/// The rectangle between two drag points, kept inside `bounds`; `None` when too small.
pub fn selection(start: (i32, i32), end: (i32, i32), bounds: Rect) -> Option<Rect> {
    let r = Rect::new(
        start.0.min(end.0),
        start.1.min(end.1),
        start.0.max(end.0),
        start.1.max(end.1),
    );
    let r = r.intersect(&bounds)?;
    (r.width() >= MIN_SIZE && r.height() >= MIN_SIZE).then_some(r)
}

/// The text to copy from the recognised lines.
pub fn join(lines: &[String], single_line: bool) -> String {
    let lines = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty());
    if single_line {
        lines.collect::<Vec<_>>().join(" ")
    } else {
        lines.collect::<Vec<_>>().join("\r\n")
    }
}

/// The size to read a `w` x `h` area at: unchanged unless a side is over `max`.
pub fn read_size(w: i32, h: i32, max: u32) -> (i32, i32) {
    let max = max.max(1) as f64;
    let scale = (max / w.max(1) as f64).min(max / h.max(1) as f64).min(1.0);
    (
        ((w as f64 * scale).floor() as i32).max(1),
        ((h as f64 * scale).floor() as i32).max(1),
    )
}

#[derive(Default)]
pub struct TextExtractor {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: live::Live,
}

impl TextExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    fn begin(&mut self) -> Result<()> {
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Text Extractor is not running");
        }
        #[cfg(windows)]
        self.live.begin()?;
        Ok(())
    }
}

impl Module for TextExtractor {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Text Extractor".into(),
            description: "Copy the text in any part of the screen.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::MouseHook,
                Permission::ClipboardWrite,
            ],
            commands: vec![CommandSpec {
                id: "extract".into(),
                title: "Select an area and copy its text".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.text_extractor.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        #[cfg(windows)]
        self.live.cancel();
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        #[cfg(windows)]
        self.live.cancel();
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.text_extractor.clone();
        #[cfg(windows)]
        if !self.settings.enabled {
            self.live.cancel();
        }
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        match action {
            "extract" => self.begin(),
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

    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        match id {
            "extract" => self.begin()?,
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let busy = self.live.busy();
        #[cfg(not(windows))]
        let busy = false;
        json!({ "running": self.running && self.settings.enabled, "busy": busy })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::Config;

    #[test]
    fn selection_is_normalized_and_clamped() {
        let b = Rect::new(-1920, 0, 1920, 1080);
        assert_eq!(selection((10, 300), (-50, 200), b), Some(Rect::new(-50, 200, 10, 300)));
        assert_eq!(selection((200, 200), (203, 260), b), None, "too narrow");
        assert_eq!(selection((0, 2000), (50, 2100), b), None, "outside");
    }

    #[test]
    fn lines_are_joined_as_asked() {
        let lines = vec!["  Hello ".to_string(), String::new(), "world".into()];
        assert_eq!(join(&lines, false), "Hello\r\nworld");
        assert_eq!(join(&lines, true), "Hello world");
        assert_eq!(join(&[], true), "");
    }

    #[test]
    fn big_areas_are_read_smaller() {
        assert_eq!(read_size(800, 600, 2600), (800, 600));
        assert_eq!(read_size(5200, 1300, 2600), (2600, 650));
        assert_eq!(read_size(1000, 5200, 2600), (500, 2600));
    }

    #[test]
    fn the_shortcut_is_registered() {
        let mut c = Config::new();
        let ours: Vec<_> = c.shortcuts().into_iter().filter(|(m, _, _)| *m == ID).collect();
        assert_eq!(ours, [(ID, "extract".to_string(), "Win+Shift+T".to_string())]);
        assert!(c.sanitize().is_empty());
    }
}
