//! The Peek window's contents: reads what File Explorer has selected, loads each file
//! on a worker thread, and browses, opens and closes on the window's keys.

use peek::{browse, date_time, decode, human_size, kind, looks_like_text, Kind, TEXT_LIMIT};
use powerkit_core::PeekEvent;
use powerkit_platform::event_loop::Waker;
use powerkit_platform::peek_window::{background, PeekView, PeekWindow};
use powerkit_platform::window::Hwnd;
use powerkit_platform::{explorer, image, monitor, shell, theme, time};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::SystemTime;

/// Share of the monitor's work area the window takes when it opens, in percent.
const SHARE: u32 = 65;

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn when(t: std::io::Result<SystemTime>) -> Option<String> {
    let secs = t.ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs() as i64;
    match time::to_local(secs) {
        Ok(local) => Some(date_time(local)),
        Err(e) => {
            tracing::warn!("peek: local time: {e}");
            Some(format!("{} (UTC)", date_time(secs)))
        }
    }
}

fn details(path: &Path, note: Option<String>) -> PeekView {
    let mut rows = Vec::new();
    match std::fs::metadata(path) {
        Ok(meta) => {
            if meta.is_dir() {
                rows.push(("Type".into(), "Folder".into()));
                match std::fs::read_dir(path) {
                    Ok(entries) => rows.push(("Items".into(), entries.count().to_string())),
                    Err(e) => rows.push(("Items".into(), format!("not readable: {e}"))),
                }
            } else {
                let ext = path
                    .extension()
                    .map(|e| format!("{} file", e.to_string_lossy().to_uppercase()))
                    .unwrap_or_else(|| "File".into());
                rows.push(("Type".into(), ext));
                rows.push(("Size".into(), human_size(meta.len())));
            }
            if let Some(m) = when(meta.modified()) {
                rows.push(("Modified".into(), m));
            }
            if let Some(c) = when(meta.created()) {
                rows.push(("Created".into(), c));
            }
        }
        Err(e) => rows.push(("Error".into(), e.to_string())),
    }
    rows.push((
        "Folder".into(),
        path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
    ));
    if let Some(n) = note {
        rows.push(("Preview".into(), n));
    }
    PeekView::Details {
        name: name_of(path),
        rows,
    }
}

/// `area` is the most room the picture gets; it is shrunk to fit, never enlarged.
/// Lay premultiplied pixels over the window's background, so they can be drawn opaque.
fn flatten(pixels: &mut [u32], light: bool) {
    let bg = background(light);
    for p in pixels {
        let a = *p >> 24;
        if a == 255 {
            continue;
        }
        let mix = |shift: u32| {
            let src = (*p >> shift) & 0xFF;
            let back = (bg >> shift) & 0xFF;
            (src + back * (255 - a) / 255).min(255) << shift
        };
        *p = 0xFF00_0000 | mix(16) | mix(8) | mix(0);
    }
}

/// The largest size at most `max` that keeps the shape of `w` by `h` and fits `area`.
fn fit_area(w: u32, h: u32, area: (u32, u32)) -> (u32, u32) {
    let s = (area.0 as f64 / w.max(1) as f64)
        .min(area.1 as f64 / h.max(1) as f64)
        .min(1.0);
    (((w as f64 * s) as u32).max(1), ((h as f64 * s) as u32).max(1))
}

fn shown(width: u32, height: u32, mut pixels: Vec<u32>, light: bool) -> PeekView {
    flatten(&mut pixels, light);
    PeekView::Picture { width, height, pixels }
}

/// A picture drawn by the preview renderer: premultiplied BGRA bytes.
fn rendered(img: powerkit_preview::image::Image, light: bool) -> PeekView {
    let pixels = img
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    shown(img.width, img.height, pixels, light)
}

fn picture(path: &Path, area: (u32, u32), light: bool) -> Result<(PeekView, String), String> {
    let pic = image::open(path).map_err(|e| e.to_string())?;
    let size = fit_area(pic.width, pic.height, area);
    let pixels = pic.pixels(size).map_err(|e| e.to_string())?;
    let footer = format!("{} \u{d7} {}", pic.width, pic.height);
    Ok((shown(size.0, size.1, pixels, light), footer))
}

fn svg(path: &Path, area: (u32, u32), light: bool) -> Result<(PeekView, String), String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let img = powerkit_preview::render::svg(&source, area.0.min(area.1).max(1)).map_err(|e| e.message())?;
    Ok((rendered(img, light), "SVG picture".into()))
}

/// The first page, fitted to the window.
fn pdf(path: &Path, area: (u32, u32), light: bool) -> Result<(PeekView, String), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let doc = powerkit_preview::render::Pdf::open(&bytes).map_err(|e| e.message())?;
    let &(pw, ph) = doc.pages.first().ok_or("the document has no pages")?;
    let (w, _) = fit_area(pw.max(1.0) as u32 * 3, ph.max(1.0) as u32 * 3, area);
    let img = doc.page(0, w).map_err(|e| e.message())?;
    let n = doc.pages.len();
    let footer = if n == 1 {
        "1 page".to_string()
    } else {
        format!("page 1 of {n}; Enter opens the whole document")
    };
    Ok((rendered(img, light), footer))
}

/// What Explorer would show for the file in its largest view; details when it has
/// only an icon.
fn thumbnail(path: &Path, area: (u32, u32), light: bool) -> (PeekView, String) {
    match shell::thumbnail(path, area.0.max(area.1)) {
        Ok((w, h, pixels)) if w <= area.0 && h <= area.1 => (shown(w, h, pixels, light), String::new()),
        Ok((w, h, pixels)) => {
            // Windows may hand back a larger size than asked for; shrink by sampling.
            let (fw, fh) = fit_area(w, h, area);
            let mut small = Vec::with_capacity(fw as usize * fh as usize);
            for y in 0..fh {
                let sy = (u64::from(y) * u64::from(h) / u64::from(fh)) as usize;
                for x in 0..fw {
                    let sx = (u64::from(x) * u64::from(w) / u64::from(fw)) as usize;
                    small.push(pixels[sy * w as usize + sx]);
                }
            }
            (shown(fw, fh, small, light), String::new())
        }
        Err(_) => (details(path, None), String::new()),
    }
}

/// The start of the file, and whether there is more.
fn head(path: &Path) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(TEXT_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    let more = bytes.len() > TEXT_LIMIT;
    bytes.truncate(TEXT_LIMIT);
    Ok((bytes, more))
}

fn text(path: &Path, sure: bool) -> Option<(PeekView, String)> {
    let (bytes, more) = match head(path) {
        Ok(h) => h,
        Err(e) => return Some((details(path, Some(format!("not readable: {e}"))), String::new())),
    };
    if !sure && !looks_like_text(&bytes[..bytes.len().min(8192)]) {
        return None;
    }
    let lines = decode(&bytes);
    let footer = if more {
        "only the first 512 KB is shown".to_string()
    } else {
        format!("{} lines", lines.lines().count())
    };
    Some((PeekView::Text(lines), footer))
}

/// What to show for `path`, and a note for the footer.
fn load(path: &Path, area: (u32, u32), light: bool) -> (PeekView, String) {
    shell::init_com_thread();
    if path.is_dir() {
        return thumbnail(path, area, light);
    }
    let failed = |what: &str, e: String| {
        (
            details(path, Some(format!("{what} could not be read: {e}"))),
            String::new(),
        )
    };
    match kind(path) {
        // Explorer may read formats WIC has no codec for here (HEIC without the
        // extension, for one) through another thumbnail provider.
        Kind::Picture => picture(path, area, light)
            .or_else(|e| match thumbnail(path, area, light) {
                shown @ (PeekView::Picture { .. }, _) => Ok(shown),
                _ => Err(e),
            })
            .unwrap_or_else(|e| failed("the picture", e)),
        Kind::Pdf => pdf(path, area, light).unwrap_or_else(|e| failed("the document", e)),
        Kind::Svg => svg(path, area, light)
            .or_else(|e| text(path, true).ok_or(e))
            .unwrap_or_else(|e| failed("the picture", e)),
        Kind::Text => text(path, true).unwrap_or_else(|| (details(path, None), String::new())),
        Kind::Unknown => text(path, false).unwrap_or_else(|| thumbnail(path, area, light)),
        Kind::Other => thumbnail(path, area, light),
    }
}

pub struct PeekUi {
    waker: Waker,
    window: Option<PeekWindow>,
    items: Vec<PathBuf>,
    index: usize,
    /// Replaced on each load, so a slow file that finishes late is dropped.
    loading: Option<Receiver<(PeekView, String)>>,
    light: bool,
}

impl PeekUi {
    pub fn new(waker: Waker) -> PeekUi {
        PeekUi {
            waker,
            window: None,
            items: Vec::new(),
            index: 0,
            loading: None,
            light: false,
        }
    }

    /// Show what the File Explorer window or desktop `hwnd` has selected, or close
    /// the window if it is open.
    pub fn open(&mut self, hwnd: isize) -> Result<(), String> {
        if let Some(w) = self.window.as_mut().filter(|w| w.is_visible()) {
            w.hide();
            self.close();
            return Ok(());
        }
        let sel = explorer::selection(Hwnd(hwnd))
            .map_err(|e| format!("Peek could not read File Explorer's selection: {e}"))?
            .ok_or("Peek works in File Explorer and on the desktop.")?;
        let (items, index) = browse(sel.items, &sel.selected).ok_or("Select a file to preview first.")?;
        if self.window.is_none() {
            self.light = theme::is_light(false).unwrap_or_else(|e| {
                tracing::warn!("peek: theme: {e}");
                false
            });
            let mut w = PeekWindow::new(self.light).map_err(|e| format!("could not open Peek: {e}"))?;
            crate::icons::apply(peek::ID, |ico| w.set_icon(ico));
            self.window = Some(w);
        }
        self.items = items;
        self.index = index;
        let area = match monitor::from_window(Hwnd(hwnd)) {
            Some(m) => m.work_area,
            None => monitor::enumerate()
                .ok()
                .and_then(|l| l.into_iter().find(|m| m.primary))
                .map(|m| m.work_area)
                .ok_or("Peek found no monitor to show on.")?,
        };
        self.window.as_mut().expect("created above").show(area, SHARE);
        self.load();
        Ok(())
    }

    fn footer(&self, note: &str) -> String {
        let mut s = String::new();
        if self.items.len() > 1 {
            s = format!("{} of {}  \u{b7}  ", self.index + 1, self.items.len());
        }
        if !note.is_empty() {
            s.push_str(note);
            s.push_str("  \u{b7}  ");
        }
        s.push_str("Enter opens it");
        s
    }

    fn load(&mut self) {
        let Some(path) = self.items.get(self.index).cloned() else {
            return;
        };
        let footer = self.footer("");
        let Some(w) = self.window.as_mut() else { return };
        w.set(&name_of(&path), &footer, PeekView::Message("Loading\u{2026}".into()));
        let (tx, rx) = mpsc::channel();
        let (waker, area, light) = (self.waker, w.picture_area(), self.light);
        std::thread::spawn(move || {
            if tx.send(load(&path, area, light)).is_ok() {
                waker.wake();
            }
        });
        self.loading = Some(rx);
    }

    pub fn poll(&mut self) {
        let Some(rx) = self.loading.as_ref() else { return };
        let (view, note) = match rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(done) => done,
            Err(TryRecvError::Disconnected) => {
                (PeekView::Message("Loading stopped unexpectedly.".into()), String::new())
            }
        };
        self.loading = None;
        let footer = self.footer(&note);
        let title = self.items.get(self.index).map(|p| name_of(p)).unwrap_or_default();
        if let Some(w) = self.window.as_mut() {
            w.set(&title, &footer, view);
        }
    }

    fn close(&mut self) {
        self.items.clear();
        self.loading = None;
    }

    pub fn on_event(&mut self, event: PeekEvent) -> Result<(), String> {
        let n = self.items.len();
        match event {
            PeekEvent::Previous | PeekEvent::Next if n > 1 => {
                self.index = if event == PeekEvent::Next {
                    (self.index + 1) % n
                } else {
                    (self.index + n - 1) % n
                };
                self.load();
            }
            PeekEvent::Previous | PeekEvent::Next => {}
            PeekEvent::Open => {
                let Some(path) = self.items.get(self.index) else {
                    return Ok(());
                };
                shell::open(&path.to_string_lossy(), None)
                    .map_err(|e| format!("Could not open {}: {e}", name_of(path)))?;
                if let Some(w) = self.window.as_mut() {
                    w.hide();
                }
                self.close();
            }
            PeekEvent::Closed => self.close(),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_files_are_drawn_and_binary_files_without_a_thumbnail_show_details() {
        let dir = std::env::temp_dir().join(format!("pk-peek-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg_path = dir.join("square.svg");
        std::fs::write(
            &svg_path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="10" height="10" fill="red"/></svg>"#,
        )
        .unwrap();
        let (view, _) = load(&svg_path, (200, 100), true);
        let PeekView::Picture { width, height, pixels } = view else {
            panic!("an SVG should be drawn, got {view:?}");
        };
        assert_eq!((width, height), (100, 50));
        assert_eq!(pixels[25 * 100 + 10] & 0x00FF_FFFF, 0x00FF_0000, "left half is red");

        let blob = dir.join("data.bin7");
        std::fs::write(&blob, [0u8, 1, 2, 0, 0, 0xFF]).unwrap();
        assert!(matches!(load(&blob, (200, 100), true).0, PeekView::Details { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pictures_only_shrink_to_fit() {
        assert_eq!(fit_area(400, 200, (100, 100)), (100, 50));
        assert_eq!(fit_area(40, 20, (100, 100)), (40, 20));
    }
}
