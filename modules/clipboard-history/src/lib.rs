//! Clipboard History: text copied anywhere is kept, newest first; a shortcut opens a
//! searchable list, and choosing an entry pastes it where the user was typing.
//! Copies an app marks as private (password managers do) are not kept.

use anyhow::{bail, Result};
use powerkit_config::{ClipboardHistory as Settings, StateFile};
use powerkit_core::{Event, PopupEvent};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "clipboard_history";
const STATE: &str = "clipboard_history";
/// Larger texts are not kept: the list shows one line of each anyway.
const MAX_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct ClipboardHistory {
    settings: Settings,
    running: bool,
    paused: bool,
    /// Newest first.
    entries: Vec<String>,
    /// The entries listed for the current search, as indexes into `entries`.
    shown: Vec<usize>,
    state: Option<StateFile>,
    #[cfg(windows)]
    os: live::Os,
}

impl ClipboardHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put `text` at the top, once.
    fn remember(&mut self, text: String) {
        if text.trim().is_empty() || text.len() > MAX_BYTES {
            return;
        }
        self.entries.retain(|t| *t != text);
        self.entries.insert(0, text);
        self.entries.truncate(self.settings.max_items as usize);
        self.save();
    }

    fn save(&self) {
        let Some(state) = self.state.as_ref().filter(|_| self.settings.remember) else {
            return;
        };
        if let Err(e) = state.save(&self.entries) {
            tracing::warn!("clipboard history not saved: {e}");
        }
    }

    /// Forget the saved copy when remembering is turned off, so it does not stay on disk.
    fn sync_saved(&self) {
        let Some(state) = &self.state else { return };
        if self.settings.remember {
            self.save();
        } else if let Err(e) = std::fs::remove_file(state.path()) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("saved clipboard history not removed: {e}");
            }
        }
    }

    fn search(&mut self, query: &str) {
        let q = query.trim().to_lowercase();
        self.shown = (0..self.entries.len())
            .filter(|&i| q.is_empty() || self.entries[i].to_lowercase().contains(&q))
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
    use super::row_text;
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

        /// The text just copied, unless there is none or the copier marked it private.
        pub fn copied(&self) -> Result<Option<String>> {
            if !self.listening || clipboard::is_private().context("reading the clipboard")? {
                return Ok(None);
            }
            clipboard::get_text().context("reading the clipboard")
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

        pub fn set_rows(&mut self, texts: &[&str]) -> Result<()> {
            let rows = texts
                .iter()
                .map(|t| {
                    let (title, subtitle) = row_text(t);
                    Row { title, subtitle }
                })
                .collect();
            if let Some(p) = self.popup.as_mut() {
                p.set_rows(rows)?;
            }
            Ok(())
        }

        pub fn paste(&mut self, text: &str) -> Result<()> {
            clipboard::set_text(text).context("writing the clipboard")?;
            if let Some(hwnd) = self.target.take() {
                window::activate(hwnd).context("the text is on the clipboard; press Ctrl+V to paste it")?;
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
            description: "Paste any of the texts you copied lately, found by typing part of it.".into(),
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
            self.entries = state.load();
            self.entries.truncate(self.settings.max_items as usize);
        }
        self.state = Some(state);
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
                if let Some(text) = self.os.copied()? {
                    self.remember(text);
                }
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Query(q),
            } => {
                self.search(q);
                #[cfg(windows)]
                {
                    let texts: Vec<&str> = self.shown.iter().map(|&i| self.entries[i].as_str()).collect();
                    self.os.set_rows(&texts)?;
                }
            }
            Event::Popup {
                owner: ID,
                event: PopupEvent::Activate(row),
            } => {
                let Some(text) = self.shown.get(*row).and_then(|&i| self.entries.get(i)).cloned() else {
                    bail!("there is no entry {row}");
                };
                self.shown.clear();
                #[cfg(windows)]
                self.os.paste(&text)?;
                #[cfg(not(windows))]
                drop(text);
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

    #[test]
    fn newest_first_without_repeats_and_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = module(dir.path(), false);
        for t in ["a", "b", "a", "c", "d", "  "] {
            m.remember(t.into());
        }
        assert_eq!(m.entries, ["d", "c", "a"]);
        m.search("C");
        assert_eq!(m.shown, [1]);
    }

    #[test]
    fn kept_across_restarts_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        module(dir.path(), false).remember("secret".into());
        assert!(module(dir.path(), false).entries.is_empty());
        assert!(
            module(dir.path(), true).entries.is_empty(),
            "nothing was written while off"
        );
        module(dir.path(), true).remember("kept".into());
        assert_eq!(module(dir.path(), true).entries, ["kept"]);
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
