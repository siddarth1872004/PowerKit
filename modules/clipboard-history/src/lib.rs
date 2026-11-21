//! Clipboard History: text and pictures copied anywhere are kept, newest first; a
//! shortcut opens a searchable list, and choosing an entry pastes it where the user was
//! typing. Copies an app marks as private (password managers do) are not kept.

use anyhow::{bail, Result};
use powerkit_config::{ClipboardHistory as Settings, StateFile};
use powerkit_core::{Event, PopupEvent};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

pub const ID: &str = "clipboard_history";
const STATE: &str = "clipboard_history";
/// Larger texts are not kept: the list shows one line of each anyway.
const MAX_BYTES: usize = 64 * 1024;
/// Pictures are kept in memory only, so their number and size are capped well below
/// the text limit: a 4K screenshot is 33 MB.
const MAX_PICTURES: usize = 20;
const MAX_PICTURE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Text(String),
    /// A packed DIB as the clipboard holds it.
    Picture(Rc<Vec<u8>>),
}

impl Entry {
    /// The list row: the first line of a text and how much more there is, or a
    /// picture's size.
    pub fn row(&self) -> (String, String) {
        match self {
            Entry::Text(t) => row_text(t),
            Entry::Picture(dib) => (
                "Picture".into(),
                match dib_size(dib) {
                    Some((w, h)) => format!("{w} x {h} pixels"),
                    None => "Picture".into(),
                },
            ),
        }
    }
}

/// Width and height from a DIB's BITMAPINFOHEADER; height is negative for top-down ones.
fn dib_size(dib: &[u8]) -> Option<(u32, u32)> {
    let field = |at: usize| Some(i32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?));
    Some((field(4)?.unsigned_abs(), field(8)?.unsigned_abs()))
}

/// Writes the history on a thread of its own: a long history is megabytes of JSON and
/// writing it on every copy held up the event loop. Only the newest pending write is
/// done; `None` removes the file.
struct Saver {
    tx: Option<Sender<Option<Vec<String>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Saver {
    fn new(state: StateFile) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Option<Vec<String>>>();
        let thread = std::thread::Builder::new()
            .name("clipboard-history-save".into())
            .spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    while let Ok(next) = rx.try_recv() {
                        job = next;
                    }
                    match job {
                        Some(texts) => {
                            if let Err(e) = state.save(&texts) {
                                tracing::warn!("clipboard history not saved: {e}");
                            }
                        }
                        None => {
                            if let Err(e) = std::fs::remove_file(state.path()) {
                                if e.kind() != std::io::ErrorKind::NotFound {
                                    tracing::warn!("saved clipboard history not removed: {e}");
                                }
                            }
                        }
                    }
                }
            });
        match thread {
            Ok(thread) => Some(Self {
                tx: Some(tx),
                thread: Some(thread),
            }),
            Err(e) => {
                tracing::warn!("clipboard history will not be saved: {e}");
                None
            }
        }
    }

    fn send(&self, job: Option<Vec<String>>) {
        if let Some(tx) = &self.tx {
            if tx.send(job).is_err() {
                tracing::warn!("clipboard history not saved: its writer stopped");
            }
        }
    }
}

impl Drop for Saver {
    /// Finish the last write, so nothing copied before quitting is lost.
    fn drop(&mut self) {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::warn!("clipboard history writer failed");
            }
        }
    }
}

#[derive(Default)]
pub struct ClipboardHistory {
    settings: Settings,
    running: bool,
    paused: bool,
    /// Newest first.
    entries: Vec<Entry>,
    /// The entries listed for the current search, as indexes into `entries`.
    shown: Vec<usize>,
    saver: Option<Saver>,
    #[cfg(windows)]
    os: live::Os,
}

impl ClipboardHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put `entry` at the top, once.
    fn remember(&mut self, entry: Entry) {
        match &entry {
            Entry::Text(t) if t.trim().is_empty() || t.len() > MAX_BYTES => return,
            Entry::Picture(dib) if dib.len() > MAX_PICTURE_BYTES => return,
            _ => {}
        }
        let text = matches!(entry, Entry::Text(_));
        self.entries.retain(|e| *e != entry);
        self.entries.insert(0, entry);
        self.entries.truncate(self.settings.max_items as usize);
        let mut pictures = 0;
        self.entries.retain(|e| {
            pictures += usize::from(matches!(e, Entry::Picture(_)));
            matches!(e, Entry::Text(_)) || pictures <= MAX_PICTURES
        });
        if text {
            self.save();
        }
    }

    /// Only texts are written: pictures would make the file hundreds of megabytes.
    fn save(&self) {
        let Some(saver) = self.saver.as_ref().filter(|_| self.settings.remember) else {
            return;
        };
        let texts = self
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Text(t) => Some(t.clone()),
                Entry::Picture(_) => None,
            })
            .collect();
        saver.send(Some(texts));
    }

    /// Forget the saved copy when remembering is turned off, so it does not stay on disk.
    fn sync_saved(&self) {
        let Some(saver) = &self.saver else { return };
        if self.settings.remember {
            self.save();
        } else {
            saver.send(None);
        }
    }

    fn search(&mut self, query: &str) {
        let q = query.trim().to_lowercase();
        self.shown = (0..self.entries.len())
            .filter(|&i| {
                q.is_empty()
                    || match &self.entries[i] {
                        Entry::Text(t) => t.to_lowercase().contains(&q),
                        Entry::Picture(_) => "picture".contains(&q),
                    }
            })
            .collect();
    }

    fn active(&self) -> bool {
        self.running && !self.paused && self.settings.enabled
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.shown.clear();
        self.save();
    }
}

/// The first line of `text`, shortened, and what else there is.
pub fn row_text(text: &str) -> (String, String) {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().trim();
    let title: String = first.chars().take(120).collect();
    let more = lines.count();
    let chars = text.chars().count();
    let subtitle = match more {
        0 => format!("{chars} characters"),
        1 => format!("{chars} characters, 1 more line"),
        n => format!("{chars} characters, {n} more lines"),
    };
    (title, subtitle)
}

#[cfg(windows)]
mod live {
    use super::Entry;
    use anyhow::{Context, Result};
    use powerkit_platform::popup::{Popup, Row};
    use powerkit_platform::window::{self, Hwnd};
    use powerkit_platform::{clipboard, event_loop, keyhook};

    #[derive(Default)]
    pub struct Os {
        popup: Option<Popup>,
        listening: bool,
        /// The window in front before the list opened, where the choice is pasted.
        target: Option<Hwnd>,
    }

    impl Os {
        pub fn listen(&mut self, on: bool) {
            if on == self.listening {
                return;
            }
            match event_loop::listen_clipboard(on) {
                Ok(()) => self.listening = on,
                Err(e) => tracing::warn!("clipboard history: {e}"),
            }
        }

        /// What was just copied, unless there is nothing or the copier marked it
        /// private. Text wins when both are there: a spreadsheet copies its cells as a
        /// picture too.
        pub fn copied(&self) -> Result<Option<Entry>> {
            if !self.listening || clipboard::is_private().context("reading the clipboard")? {
                return Ok(None);
            }
            if let Some(text) = clipboard::get_text().context("reading the clipboard")? {
                return Ok(Some(Entry::Text(text)));
            }
            let dib = clipboard::get_dib().context("reading the clipboard")?;
            Ok(dib.map(|d| Entry::Picture(d.into())))
        }

        pub fn visible(&self) -> bool {
            self.popup.as_ref().is_some_and(Popup::is_visible)
        }

        pub fn show(&mut self) -> Result<()> {
            self.target = window::foreground();
            let popup = match &mut self.popup {
                Some(p) => p,
                None => self.popup.insert(Popup::new(super::ID)?),
            };
            popup.show()?;
            Ok(())
        }

        pub fn hide(&mut self) {
            if let Some(p) = self.popup.as_mut() {
                p.hide();
            }
        }

        pub fn set_rows(&mut self, entries: &[&Entry]) -> Result<()> {
            let rows = entries
                .iter()
                .map(|e| {
                    let (title, subtitle) = e.row();
                    Row { title, subtitle }
                })
                .collect();
            if let Some(p) = self.popup.as_mut() {
                p.set_rows(rows)?;
            }
            Ok(())
        }

        pub fn paste(&mut self, entry: &Entry) -> Result<()> {
            match entry {
                Entry::Text(t) => clipboard::set_text(t),
                Entry::Picture(dib) => clipboard::set_dib(dib.to_vec()),
            }
            .context("writing the clipboard")?;
            if let Some(hwnd) = self.target.take() {
                window::activate(hwnd).context("it is on the clipboard; press Ctrl+V to paste it")?;
            }
            keyhook::press_paste().context("pressing Ctrl+V")?;
            Ok(())
        }

        pub fn close(&mut self) {
            self.popup = None;
            self.listen(false);
        }
    }
}

impl Module for ClipboardHistory {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        };
        Manifest {
            id: ID.into(),
            name: "Clipboard History".into(),
            description: "Paste any of the texts and pictures you copied lately, found by typing part of it.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::ClipboardRead,
                Permission::ClipboardWrite,
                Permission::FilesystemWrite,
            ],
            commands: vec![
                cmd("open", "Open Clipboard History"),
                cmd("clear", "Forget everything copied"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.clipboard_history.clone();
        let state = StateFile::new(ctx.paths, STATE);
        if self.settings.remember {
            let texts: Vec<String> = state.load();
            self.entries = texts.into_iter().map(Entry::Text).collect();
            self.entries.truncate(self.settings.max_items as usize);
        }
        self.saver = Saver::new(state);
        self.running = true;
        self.paused = false;
        #[cfg(windows)]
        self.os.listen(self.settings.enabled);
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
        self.entries.clear();
        self.shown.clear();
        #[cfg(windows)]
        self.os.close();
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
        #[cfg(windows)]
        {
            self.os.hide();
            self.os.listen(false);
        }
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
        #[cfg(windows)]
        self.os.listen(self.settings.enabled);
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        let before = std::mem::replace(&mut self.settings, ctx.config.clipboard_history.clone());
        self.entries.truncate(self.settings.max_items as usize);
        if before.remember != self.settings.remember || before.max_items != self.settings.max_items {
            self.sync_saved();
        }
        #[cfg(windows)]
        self.os.listen(self.active());
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        match action {
            "open" => self.open(),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) -> Result<()> {
        match event {
            #[cfg(windows)]
            Event::ClipboardChanged if self.active() => {
                if let Some(entry) = self.os.copied()? {
                    self.remember(entry);
                }
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Query(q),
            } => {
                self.search(q);
                #[cfg(windows)]
                {
                    let entries: Vec<&Entry> = self.shown.iter().map(|&i| &self.entries[i]).collect();
                    self.os.set_rows(&entries)?;
                }
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Activate(row),
            } => {
                let Some(entry) = self.shown.get(*row).and_then(|&i| self.entries.get(i)).cloned() else {
                    bail!("there is no entry {row}");
                };
                self.shown.clear();
                #[cfg(windows)]
                self.os.paste(&entry)?;
                #[cfg(not(windows))]
                drop(entry);
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Dismiss,
            } => self.shown.clear(),
            _ => {}
        }
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        match id {
            "open" => self.open()?,
            "clear" => self.clear(),
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({ "running": self.active(), "entries": self.entries.len() })
    }
}

impl ClipboardHistory {
    fn open(&mut self) -> Result<()> {
        if !self.active() {
            bail!("Clipboard History is not running");
        }
        #[cfg(windows)]
        {
            if self.os.visible() {
                self.os.hide();
                return Ok(());
            }
            self.os.show()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};

    fn module(dir: &std::path::Path, remember: bool) -> ClipboardHistory {
        let mut config = Config::new();
        config.clipboard_history.remember = remember;
        config.clipboard_history.max_items = 3;
        let paths = Paths::new(dir);
        let mut actions = Vec::new();
        let mut m = ClipboardHistory::new();
        m.start(&mut Ctx {
            config: &config,
            paths: &paths,
            actions: &mut actions,
        })
        .unwrap();
        m
    }

    fn text(t: &str) -> Entry {
        Entry::Text(t.into())
    }

    fn picture(w: i32, h: i32) -> Entry {
        let mut dib = vec![40, 0, 0, 0];
        dib.extend_from_slice(&w.to_le_bytes());
        dib.extend_from_slice(&h.to_le_bytes());
        Entry::Picture(dib.into())
    }

    #[test]
    fn newest_first_without_repeats_and_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = module(dir.path(), false);
        for t in ["a", "b", "a", "c", "d", "  "] {
            m.remember(text(t));
        }
        assert_eq!(m.entries, [text("d"), text("c"), text("a")]);
        m.search("C");
        assert_eq!(m.shown, [1]);
    }

    #[test]
    fn kept_across_restarts_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        module(dir.path(), false).remember(text("secret"));
        assert!(module(dir.path(), false).entries.is_empty());
        assert!(
            module(dir.path(), true).entries.is_empty(),
            "nothing was written while off"
        );
        let mut m = module(dir.path(), true);
        m.remember(text("kept"));
        m.remember(picture(2, 2));
        drop(m);
        assert_eq!(
            module(dir.path(), true).entries,
            [text("kept")],
            "pictures stay in memory"
        );
    }

    #[test]
    fn pictures_are_listed_by_size_and_capped() {
        assert_eq!(
            picture(1920, -1080).row(),
            ("Picture".into(), "1920 x 1080 pixels".into())
        );
        let dir = tempfile::tempdir().unwrap();
        let mut m = module(dir.path(), false);
        m.settings.max_items = 500;
        for i in 0..MAX_PICTURES as i32 + 5 {
            m.remember(picture(i, 1));
        }
        m.remember(text("t"));
        assert_eq!(m.entries.len(), MAX_PICTURES + 1);
        assert_eq!(m.entries[1], picture(MAX_PICTURES as i32 + 4, 1));
        m.search("pict");
        assert_eq!(m.shown.len(), MAX_PICTURES);
    }

    #[test]
    fn rows_show_the_first_line_and_how_much_more() {
        assert_eq!(
            row_text("one\ntwo\nthree"),
            ("one".into(), "13 characters, 2 more lines".into())
        );
        assert_eq!(row_text("hi"), ("hi".into(), "2 characters".into()));
    }
}
