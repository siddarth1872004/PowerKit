//! Screen Recorder: drag a box over part of the screen to record it as an MP4 or an
//! animated GIF; the same shortcut stops the recording.

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_config::{RecordingFormat, ScreenRecorder as Settings};
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "screen_recorder";

/// `Screen recording 2026-11-17 093012.mp4`, sorting by time.
pub fn file_name(date: (u16, u16, u16), time: (u16, u16, u16), format: RecordingFormat) -> String {
    let ((y, mo, d), (h, mi, s)) = (date, time);
    let ext = match format {
        RecordingFormat::Mp4 => "mp4",
        RecordingFormat::Gif => "gif",
    };
    format!("Screen recording {y:04}-{mo:02}-{d:02} {h:02}{mi:02}{s:02}.{ext}")
}

#[derive(Default)]
pub struct ScreenRecorder {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: live::Live,
}

impl ScreenRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&mut self, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        if self.live.recording() {
            // Stopping is always allowed, even after the utility was paused.
            self.live.stop(ctx);
            return Ok(());
        }
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Screen Recorder is not running");
        }
        #[cfg(windows)]
        self.live.begin()?;
        #[cfg(not(windows))]
        let _ = ctx;
        Ok(())
    }

    fn end(&mut self, ctx: &mut Ctx) {
        #[cfg(windows)]
        {
            self.live.cancel();
            self.live.stop(ctx);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }
}

impl Module for ScreenRecorder {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Screen Recorder".into(),
            description: "Record part of the screen as an MP4 video or an animated GIF.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::MouseHook,
                Permission::ScreenCapture,
                Permission::FilesystemWrite,
            ],
            commands: vec![CommandSpec {
                id: "record".into(),
                title: "Start or stop recording".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.screen_recorder.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, ctx: &mut Ctx) {
        self.running = false;
        self.end(ctx);
    }

    fn pause(&mut self, ctx: &mut Ctx) {
        self.paused = true;
        self.end(ctx);
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.screen_recorder.clone();
        if !self.settings.enabled {
            self.end(ctx);
        }
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "record" => self.record(ctx),
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

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        self.on_hotkey(id, ctx)?;
        Ok(self.status())
    }

    fn status(&self) -> Value {
        #[cfg(windows)]
        let recording = self.live.recording();
        #[cfg(not(windows))]
        let recording = false;
        json!({ "running": self.running && self.settings.enabled, "recording": recording })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::Config;

    #[test]
    fn names_sort_by_time_and_carry_the_format() {
        assert_eq!(
            file_name((2026, 1, 2), (3, 4, 5), RecordingFormat::Mp4),
            "Screen recording 2026-01-02 030405.mp4"
        );
        assert!(file_name((2026, 1, 2), (3, 4, 5), RecordingFormat::Gif).ends_with(".gif"));
    }

    #[test]
    fn one_shortcut_starts_and_stops() {
        let c = Config::new();
        let ours: Vec<_> = c
            .shortcuts()
            .into_iter()
            .filter(|(m, _, _)| *m == ID)
            .map(|(_, a, _)| a)
            .collect();
        assert_eq!(ours, ["record"]);
    }
}
