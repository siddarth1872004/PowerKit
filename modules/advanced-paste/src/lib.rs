//! Advanced Paste: paste what is on the clipboard in another format, from a menu at
//! the cursor or straight from a shortcut.

pub mod convert;
#[cfg(windows)]
mod live;
#[cfg(windows)]
pub use live::{paste, read, Reading};

use anyhow::{anyhow, bail, Result};
use powerkit_config::AdvancedPaste as Settings;
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "advanced_paste";
pub const OPEN_EVENT: &str = "advanced_paste.open";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Plain,
    Markdown,
    Json,
    TextFile,
    HtmlFile,
    PngFile,
    ImageText,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Plain,
        Format::Markdown,
        Format::Json,
        Format::ImageText,
        Format::TextFile,
        Format::HtmlFile,
        Format::PngFile,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Format::Plain => "plain",
            Format::Markdown => "markdown",
            Format::Json => "json",
            Format::TextFile => "txt_file",
            Format::HtmlFile => "html_file",
            Format::PngFile => "png_file",
            Format::ImageText => "image_text",
        }
    }

    pub fn from_id(id: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Plain => "Paste as plain text",
            Format::Markdown => "Paste as Markdown",
            Format::Json => "Paste as JSON",
            Format::TextFile => "Paste as .txt file",
            Format::HtmlFile => "Paste as .html file",
            Format::PngFile => "Paste as .png file",
            Format::ImageText => "Paste the text in the picture",
        }
    }

    /// Whether the format makes a file, which pastes into File Explorer.
    pub fn is_file(self) -> bool {
        matches!(self, Format::TextFile | Format::HtmlFile | Format::PngFile)
    }
}

/// What the clipboard holds, as read once when pasting starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Clip {
    pub text: Option<String>,
    /// The CF_HTML text, header included.
    pub html: Option<String>,
    /// A packed DIB.
    pub picture: Option<Vec<u8>>,
}

impl Clip {
    fn text(&self) -> Option<&str> {
        self.text.as_deref().filter(|t| !t.is_empty())
    }

    /// The formats this clipboard can be pasted as, in menu order.
    pub fn choices(&self) -> Vec<Format> {
        Format::ALL
            .into_iter()
            .filter(|&f| match f {
                Format::Plain | Format::TextFile => self.text().is_some(),
                Format::Markdown | Format::HtmlFile => self.html.is_some(),
                Format::Json => self.text().is_some_and(|t| convert::to_json(t).is_some()),
                Format::PngFile | Format::ImageText => self.picture.is_some(),
            })
            .collect()
    }

    /// The text to paste for a text format: plain text, Markdown, JSON, or the
    /// contents of a .txt or .html file.
    pub fn convert_text(&self, format: Format) -> Result<String> {
        let text = || self.text().ok_or_else(|| anyhow!("There is no text on the clipboard."));
        let html = || {
            self.html
                .as_deref()
                .map(convert::html_fragment)
                .ok_or_else(|| anyhow!("There is no formatted text (HTML) on the clipboard."))
        };
        match format {
            Format::Plain | Format::TextFile => text().map(str::to_string),
            Format::Markdown => html().map(convert::html_to_markdown),
            Format::HtmlFile => html().map(|h| {
                format!("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n</head>\n<body>\n{h}\n</body>\n</html>\n")
            }),
            Format::Json => convert::to_json(text()?)
                .ok_or_else(|| anyhow!("The clipboard text is not JSON or a comma, semicolon or tab separated table.")),
            Format::PngFile | Format::ImageText => bail!("{} is not a text format", format.label()),
        }
    }
}

/// The name of a pasted file: when it was made, so pasting twice into one folder
/// never asks to replace the first.
pub fn file_name(date: (u16, u16, u16), time: (u16, u16, u16), extension: &str) -> String {
    let ((y, mo, d), (h, mi, s)) = (date, time);
    format!("Pasted {y:04}-{mo:02}-{d:02} {h:02}.{mi:02}.{s:02}.{extension}")
}

#[derive(Default)]
pub struct AdvancedPaste {
    settings: Settings,
    running: bool,
    #[cfg(windows)]
    reading: Option<live::Reading>,
}

impl AdvancedPaste {
    pub fn new() -> Self {
        Self::default()
    }

    fn check(&self) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Advanced Paste is turned off");
        }
        Ok(())
    }

    /// Paste into `target`, or the window in front when `None`.
    #[cfg(windows)]
    fn paste_now(&mut self, format: Format, target: Option<isize>) -> Result<()> {
        self.reading = paste(format, &read()?, target, &self.settings.ocr_language)?;
        Ok(())
    }

    #[cfg(not(windows))]
    fn paste_now(&mut self, _format: Format, _target: Option<isize>) -> Result<()> {
        bail!("Advanced Paste works only on Windows")
    }
}

impl Module for AdvancedPaste {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Advanced Paste".into(),
            description: "Paste the clipboard as plain text, Markdown, JSON, a file, or the text in a picture.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys],
            commands: vec![
                CommandSpec {
                    id: "open".into(),
                    title: "Show the ways to paste".into(),
                    description: String::new(),
                },
                CommandSpec {
                    id: "paste".into(),
                    title: "Paste in a format".into(),
                    description: "Takes `format`: plain, markdown, json, image_text, txt_file, html_file or png_file; and optionally `target`, the window to paste into."
                        .into(),
                },
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.advanced_paste.clone();
        self.running = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.advanced_paste.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        self.check()?;
        match action {
            "open" => {
                ctx.emit(OPEN_EVENT, Value::Null);
                Ok(())
            }
            other => {
                let format = Format::from_id(other).ok_or_else(|| anyhow!("unknown action {other:?}"))?;
                self.paste_now(format, None)
            }
        }
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        if let (Event::Timer(id), Some(r)) = (event, &self.reading) {
            if *id == r.timer() {
                if let Some(done) = r.finish() {
                    self.reading = None;
                    if let Err(e) = done {
                        ctx.notify("Advanced Paste", format!("{e:#}"));
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (event, ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        self.check()?;
        match id {
            "open" => ctx.emit(OPEN_EVENT, Value::Null),
            "paste" => {
                let name = args["format"].as_str().unwrap_or_default();
                let format = Format::from_id(name).ok_or_else(|| anyhow!("unknown format {name:?}"))?;
                self.paste_now(format, args["target"].as_i64().map(|t| t as isize))?;
            }
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "running": self.running && self.settings.enabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_follow_what_the_clipboard_holds() {
        let empty = Clip::default();
        assert!(empty.choices().is_empty());
        let words = Clip {
            text: Some("hello".into()),
            ..Default::default()
        };
        assert_eq!(words.choices(), vec![Format::Plain, Format::TextFile]);
        let table = Clip {
            text: Some("a,b\n1,2".into()),
            html: Some("<table></table>".into()),
            ..Default::default()
        };
        assert_eq!(
            table.choices(),
            vec![
                Format::Plain,
                Format::Markdown,
                Format::Json,
                Format::TextFile,
                Format::HtmlFile
            ]
        );
        let picture = Clip {
            picture: Some(vec![0]),
            ..Default::default()
        };
        assert_eq!(picture.choices(), vec![Format::ImageText, Format::PngFile]);
    }

    #[test]
    fn text_formats_convert_or_say_why_not() {
        let clip = Clip {
            text: Some("Bold".into()),
            html: Some("<!--StartFragment--><b>Bold</b><!--EndFragment-->".into()),
            ..Default::default()
        };
        assert_eq!(clip.convert_text(Format::Plain).unwrap(), "Bold");
        assert_eq!(clip.convert_text(Format::Markdown).unwrap(), "**Bold**");
        assert!(clip
            .convert_text(Format::HtmlFile)
            .unwrap()
            .contains("<body>\n<b>Bold</b>\n</body>"));
        let err = clip.convert_text(Format::Json).unwrap_err().to_string();
        assert!(err.contains("not JSON"), "{err}");
        let err = Clip::default().convert_text(Format::Markdown).unwrap_err().to_string();
        assert!(err.contains("no formatted text"), "{err}");
    }

    #[test]
    fn formats_round_trip_their_ids() {
        for f in Format::ALL {
            assert_eq!(Format::from_id(f.id()), Some(f));
        }
        assert_eq!(Format::from_id("open"), None);
    }

    #[test]
    fn pasted_files_are_named_by_time() {
        assert_eq!(
            file_name((2026, 1, 2), (3, 4, 5), "png"),
            "Pasted 2026-01-02 03.04.05.png"
        );
    }
}
