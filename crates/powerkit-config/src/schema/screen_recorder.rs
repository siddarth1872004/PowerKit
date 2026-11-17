use super::{clamp, fix_shortcut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFormat {
    /// H.264 video, small and smooth.
    #[default]
    Mp4,
    /// An animated picture that plays anywhere, for short clips.
    Gif,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenRecorder {
    pub enabled: bool,
    /// Drag a box to start recording it; pressed again, stops.
    pub shortcut: String,
    pub format: RecordingFormat,
    /// Frames each second.
    pub fps: u32,
    /// Where recordings are saved; empty is Videos\Screen recordings.
    pub folder: String,
    pub show_cursor: bool,
}

impl Default for ScreenRecorder {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Ctrl+Alt+R".into(),
            format: RecordingFormat::Mp4,
            fps: 20,
            folder: String::new(),
            show_cursor: true,
        }
    }
}

impl ScreenRecorder {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(
            &mut self.shortcut,
            &ScreenRecorder::default().shortcut,
            "Screen Recorder",
            w,
        );
        clamp(&mut self.fps, 1, 30, "screen_recorder.fps", w);
        self.folder = self.folder.trim().to_string();
    }
}
