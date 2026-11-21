use crate::{convert, file_name, Clip, Format};
use anyhow::{anyhow, bail, Context, Result};
use powerkit_platform::window::{self, Hwnd};
use powerkit_platform::{clipboard, event_loop, image, keyhook, ocr, time};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

const POLL_MS: u32 = 50;

/// Text being read from a picture on a worker thread: recognition can take a second,
/// and the event loop also runs the keyboard hook, which Windows drops when it stalls.
pub struct Reading {
    rx: Receiver<Result<String, String>>,
    timer: usize,
    target: Option<isize>,
}

impl Drop for Reading {
    fn drop(&mut self) {
        event_loop::kill_timer(self.timer);
    }
}

impl Reading {
    pub fn timer(&self) -> usize {
        self.timer
    }

    /// `None` while still reading; then the text is pasted, or why it was not.
    pub fn finish(&self) -> Option<Result<()>> {
        let text = match self.rx.try_recv() {
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => return Some(Err(anyhow!("text recognition stopped unexpectedly"))),
            Ok(r) => r,
        };
        Some(text.map_err(anyhow::Error::msg).and_then(|text| {
            clipboard::set_text(&text).context("writing the clipboard")?;
            put_back(self.target)
        }))
    }
}

/// Read every format Advanced Paste can use from the clipboard.
pub fn read() -> Result<Clip> {
    Ok(Clip {
        text: clipboard::get_text().context("reading the clipboard")?,
        html: clipboard::get_html().context("reading the clipboard")?,
        picture: clipboard::get_dib().context("reading the clipboard")?,
    })
}

/// Put `clip` on the clipboard in `format`, bring back `target` (the window that was in
/// front before a menu took focus) and press Ctrl+V there. The converted content stays
/// on the clipboard. Text in a picture is read on a worker: the paste happens when the
/// returned reading finishes.
pub fn paste(format: Format, clip: &Clip, target: Option<isize>, language: &str) -> Result<Option<Reading>> {
    match format {
        Format::Plain | Format::Markdown | Format::Json => {
            clipboard::set_text(&clip.convert_text(format)?).context("writing the clipboard")?
        }
        Format::TextFile | Format::HtmlFile => {
            let ext = if format == Format::TextFile { "txt" } else { "html" };
            let path = new_file(ext)?;
            std::fs::write(&path, clip.convert_text(format)?).with_context(|| format!("writing {}", path.display()))?;
            clipboard::set_files(&[path]).context("writing the clipboard")?;
        }
        Format::PngFile => {
            let picture = picture(clip)?;
            let path = new_file("png")?;
            image::save_png(&picture.pixels, picture.width, picture.height, &path)
                .with_context(|| format!("writing {}", path.display()))?;
            clipboard::set_files(&[path]).context("writing the clipboard")?;
        }
        Format::ImageText => {
            let picture = picture(clip)?;
            let max = ocr::max_dimension();
            if picture.width > max || picture.height > max {
                bail!("The picture is too big to read text from (over {max} pixels across).");
            }
            let language = language.to_string();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let text = ocr::recognize(&picture.pixels, picture.width, picture.height, &language)
                    .map_err(|e| e.to_string())
                    .and_then(|lines| match lines.is_empty() {
                        true => Err("No text was found in the picture.".to_string()),
                        false => Ok(lines.join("\r\n")),
                    });
                // The receiver is gone only when the paste was replaced or PowerKit quits.
                let _ = tx.send(text);
            });
            return Ok(Some(Reading {
                rx,
                timer: event_loop::set_timer(POLL_MS)?,
                target,
            }));
        }
    }
    put_back(target)?;
    Ok(None)
}

fn put_back(target: Option<isize>) -> Result<()> {
    if let Some(hwnd) = target {
        window::activate(Hwnd(hwnd)).context("the clipboard holds the converted content; press Ctrl+V to paste it")?;
    }
    keyhook::press_paste().context("pressing Ctrl+V")
}

fn picture(clip: &Clip) -> Result<convert::Picture> {
    let Some(dib) = &clip.picture else {
        bail!("There is no picture on the clipboard.");
    };
    convert::dib_picture(dib).map_err(anyhow::Error::msg)
}

/// A path for a new pasted file in PowerKit's own temporary folder. Files from earlier
/// pastes are removed first: File Explorer copied them when they were pasted.
fn new_file(extension: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("PowerKit Advanced Paste");
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!("advanced paste: old files not removed: {e}");
        }
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let t = time::local_now();
    Ok(dir.join(file_name(
        (t.year, t.month, t.day),
        (t.hour, t.minute, t.second),
        extension,
    )))
}
