use crate::{convert, file_name, Clip, Format};
use anyhow::{bail, Context, Result};
use powerkit_platform::window::{self, Hwnd};
use powerkit_platform::{clipboard, image, input, keyhook, ocr, time};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Win, Shift, Ctrl and Alt, left and right.
const MODIFIERS: [u32; 8] = [0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5];
/// An unassigned key: pressing it between Win or Alt going down and up keeps their
/// release from opening Start or a menu bar.
const VK_NONE: u32 = 0xE8;
const VK_CONTROL: u32 = 0x11;
const VK_V: u32 = 0x56;

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
/// on the clipboard.
pub fn paste(format: Format, clip: &Clip, target: Option<isize>, language: &str) -> Result<()> {
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
            let lines = ocr::recognize(&picture.pixels, picture.width, picture.height, language)?;
            if lines.is_empty() {
                bail!("No text was found in the picture.");
            }
            clipboard::set_text(&lines.join("\r\n")).context("writing the clipboard")?;
        }
    }
    if let Some(hwnd) = target {
        window::activate(Hwnd(hwnd)).context("the clipboard holds the converted content; press Ctrl+V to paste it")?;
    }
    press_paste()
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

/// Press Ctrl+V. Modifiers still held from the shortcut would turn it into another
/// shortcut, so give the user a moment to let go, then release any still down.
fn press_paste() -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(500);
    while MODIFIERS.iter().any(|&vk| input::key_down(vk)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut strokes = vec![(VK_NONE, true), (VK_NONE, false)];
    strokes.extend(
        MODIFIERS
            .iter()
            .filter(|&&vk| input::key_down(vk))
            .map(|&vk| (vk, false)),
    );
    strokes.extend([(VK_CONTROL, true), (VK_V, true), (VK_V, false), (VK_CONTROL, false)]);
    keyhook::send_keys(&strokes).context("pressing Ctrl+V")
}
