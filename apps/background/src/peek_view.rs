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
fn picture(path: &Path, area: (u32, u32), light: bool) -> Result<(PeekView, String), String> {
    let pic = image::open(path).map_err(|e| e.to_string())?;
    let s = (area.0 as f64 / pic.width.max(1) as f64)
        .min(area.1 as f64 / pic.height.max(1) as f64)
        .min(1.0);
    let size = (
        ((pic.width as f64 * s) as u32).max(1),
        ((pic.height as f64 * s) as u32).max(1),
    );
    let mut pixels = pic.pixels(size).map_err(|e| e.to_string())?;
    // The pixels are premultiplied; lay them over the window's background.
    let bg = background(light);
    for p in &mut pixels {
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
    let footer = format!("{} \u{d7} {}", pic.width, pic.height);
    Ok((
        PeekView::Picture {
            width: size.0,
            height: size.1,
            pixels,
        },
        footer,
    ))
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
    if path.is_dir() {
        return (details(path, None), String::new());
    }
    match kind(path) {
        Kind::Picture => picture(path, area, light).unwrap_or_else(|e| {
            (
                details(path, Some(format!("the picture could not be read: {e}"))),
                String::new(),
            )
        }),
        Kind::Text => text(path, true).unwrap_or_else(|| (details(path, None), String::new())),
        Kind::Unknown => text(path, false).unwrap_or_else(|| (details(path, None), String::new())),
        Kind::Other => (details(path, None), String::new()),
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
