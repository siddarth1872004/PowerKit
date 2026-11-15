//! The Registry Preview window's contents: a .reg file read and shown as the keys
//! and values it would write, edited, saved, and written with `reg import`, run as
//! administrator only when the file touches more than the current user's keys.

use powerkit_core::RegistryEvent;
use powerkit_platform::event_loop::Waker;
use powerkit_platform::registry_window::{RegistryRow, RegistryWindow};
use registry_preview::reg::{self, Encoding, Parsed};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

const NEW_FILE: &str = "Windows Registry Editor Version 5.00\r\n\r\n";
/// Where Registry Editor keeps the key it shows when it starts.
const REGEDIT_STATE: &str = r"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn rows(parsed: &Parsed) -> Vec<RegistryRow> {
    parsed
        .listing()
        .into_iter()
        .map(|(cols, line)| RegistryRow { cols, line })
        .collect()
}

fn reg_exe() -> Result<PathBuf, String> {
    let root = std::env::var_os("SystemRoot").ok_or("SystemRoot is not set, so reg.exe cannot be found.")?;
    Ok(Path::new(&root).join("System32").join("reg.exe"))
}

/// Run `reg import file`, as administrator when `admin`. Blocks; returns a message
/// when it did not succeed.
fn import(file: &Path, admin: bool) -> Result<(), String> {
    let exe = reg_exe()?;
    if admin {
        let code = powerkit_platform::shell::run_elevated(&exe, &format!("import \"{}\"", file.display()))
            .map_err(|e| e.to_string())?;
        return match code {
            0 => Ok(()),
            c => Err(format!("reg import stopped with code {c}")),
        };
    }
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(&exe)
        .arg("import")
        .arg(file)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run reg.exe: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.trim().trim_start_matches("ERROR:").trim();
    Err(if said.is_empty() {
        format!("reg import stopped with {}", out.status)
    } else {
        said.to_string()
    })
}

pub struct RegistryUi {
    waker: Waker,
    window: Option<RegistryWindow>,
    path: Option<PathBuf>,
    encoding: Encoding,
    /// The text as last loaded or saved, to tell whether there are unsaved edits.
    saved: String,
    text: String,
    parsed: Parsed,
    /// The user was warned that opening another file drops unsaved edits.
    discard: bool,
    writing: Option<Receiver<Result<(), String>>>,
}

impl RegistryUi {
    pub fn new(waker: Waker) -> RegistryUi {
        RegistryUi {
            waker,
            window: None,
            path: None,
            encoding: Encoding::Utf16,
            saved: NEW_FILE.into(),
            text: NEW_FILE.into(),
            parsed: reg::parse(NEW_FILE),
            discard: false,
            writing: None,
        }
    }

    fn dirty(&self) -> bool {
        self.text != self.saved
    }

    /// Show the window, opening `path` in it unless that would drop unsaved edits.
    pub fn open(&mut self, path: Option<PathBuf>) -> Result<(), String> {
        let fresh = self.window.is_none();
        if fresh {
            self.window = Some(RegistryWindow::new().map_err(|e| format!("could not open Registry Preview: {e}"))?);
        }
        if let Some(w) = self.window.as_mut() {
            w.show();
        }
        match path {
            Some(p) if self.dirty() => self.status(&format!(
                "Not opened {}: there are unsaved changes. Save them, or choose Open to drop them.",
                p.display()
            )),
            Some(p) => self.load(&p),
            None if fresh => self.refresh(),
            None => {}
        }
        Ok(())
    }

    fn load(&mut self, path: &Path) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return self.status(&format!("Could not read {}: {e}", path.display())),
        };
        let (text, encoding) = reg::decode(&bytes);
        let text = reg::crlf(&text);
        self.path = Some(path.to_path_buf());
        self.encoding = encoding;
        self.saved = text.clone();
        self.text = text;
        self.discard = false;
        if let Some(w) = self.window.as_mut() {
            w.set_text(&self.text);
        }
        self.refresh();
    }

    /// Re-read the text and show the result.
    fn refresh(&mut self) {
        self.parsed = reg::parse(&self.text);
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or("New file".into(), |n| n.to_string_lossy().into_owned());
        let title = format!(
            "Registry Preview - {name}{}",
            if self.dirty() { " (not saved)" } else { "" }
        );
        let rows = rows(&self.parsed);
        let summary = self.parsed.summary();
        let Some(w) = self.window.as_mut() else { return };
        if self.path.is_none() && self.text == NEW_FILE {
            w.set_text(&self.text);
        }
        w.set_rows(&rows);
        w.set_title(&title);
        w.set_status(&summary);
    }

    fn status(&mut self, text: &str) {
        if let Some(w) = self.window.as_mut() {
            w.set_status(text);
        }
    }

    /// Take the text as it is in the window now; edits are otherwise reported only
    /// once typing pauses.
    fn sync(&mut self) {
        let Some(now) = self.window.as_ref().map(RegistryWindow::text) else {
            return;
        };
        if now != self.text {
            self.text = now;
            self.refresh();
        }
    }

    pub fn on_event(&mut self, event: RegistryEvent, data_dir: &Path) {
        match event {
            RegistryEvent::Changed(text) => {
                self.text = text;
                self.discard = false;
                self.refresh();
            }
            RegistryEvent::Open => {
                self.sync();
                if self.dirty() && !self.discard {
                    self.discard = true;
                    return self.status("There are unsaved changes. Choose Open again to drop them, or Save first.");
                }
                let picked = self.window.as_ref().map(RegistryWindow::pick_open);
                match picked {
                    Some(Ok(Some(p))) => self.load(&p),
                    Some(Ok(None)) | None => {}
                    Some(Err(e)) => self.status(&format!("Could not show the Open dialog: {e}")),
                }
            }
            RegistryEvent::Save => {
                self.sync();
                match self.path.clone() {
                    Some(p) => self.save(&p),
                    None => self.save_as(),
                }
            }
            RegistryEvent::SaveAs => {
                self.sync();
                self.save_as();
            }
            RegistryEvent::Write => {
                self.sync();
                self.write(data_dir);
            }
            RegistryEvent::OpenKey(row) => self.open_key(row),
        }
    }

    fn save_as(&mut self) {
        let picked = self.window.as_ref().map(|w| w.pick_save(self.path.as_deref()));
        match picked {
            Some(Ok(Some(p))) => self.save(&p),
            Some(Ok(None)) | None => {}
            Some(Err(e)) => self.status(&format!("Could not show the Save dialog: {e}")),
        }
    }

    /// The text in the file's own encoding, or as UTF-16 (what regedit writes) when
    /// that cannot hold it; the second part says so.
    fn bytes(&self) -> (Vec<u8>, Option<String>) {
        match reg::encode(&self.text, self.encoding) {
            Ok(b) => (b, None),
            Err(e) => {
                let b = reg::encode(&self.text, Encoding::Utf16).expect("UTF-16 holds any text");
                (b, Some(format!("{e} Saved as UTF-16 instead.")))
            }
        }
    }

    fn save(&mut self, path: &Path) {
        let (bytes, note) = self.bytes();
        if let Err(e) = std::fs::write(path, bytes) {
            return self.status(&format!("Not saved: {e}"));
        }
        if note.is_some() {
            self.encoding = Encoding::Utf16;
        }
        self.path = Some(path.to_path_buf());
        self.saved = self.text.clone();
        self.discard = false;
        self.refresh();
        self.status(&format!("Saved {}. {}", path.display(), note.unwrap_or_default()));
    }

    fn write(&mut self, data_dir: &Path) {
        if self.writing.is_some() {
            return self.status("Still writing; wait for it to finish.");
        }
        if let Some((line, e)) = self.parsed.errors.first() {
            return self.status(&format!("Not written: line {} has a problem: {e}", line + 1));
        }
        if self.parsed.items.is_empty() {
            return self.status("Nothing to write.");
        }
        let staged = data_dir.join("registry-preview.reg");
        let (bytes, _) = self.bytes();
        if let Err(e) = std::fs::create_dir_all(data_dir).and_then(|()| std::fs::write(&staged, bytes)) {
            return self.status(&format!("Could not prepare the file for writing: {e}"));
        }
        let admin = self.parsed.needs_admin();
        let (tx, rx) = mpsc::channel();
        let waker = self.waker;
        std::thread::spawn(move || {
            if tx.send(import(&staged, admin)).is_ok() {
                waker.wake();
            }
        });
        self.writing = Some(rx);
        self.status(if admin {
            "Writing; Windows asks for permission because the file changes keys outside HKEY_CURRENT_USER."
        } else {
            "Writing…"
        });
    }

    /// Pick up the outcome of a write, if it has arrived. Call on every wake-up.
    pub fn poll(&mut self) {
        let Some(rx) = &self.writing else { return };
        let outcome = match rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(r) => r,
            Err(TryRecvError::Disconnected) => Err("the write stopped unexpectedly".into()),
        };
        self.writing = None;
        match outcome {
            Ok(()) => {
                tracing::info!("registry file imported");
                self.status("Written to the registry.");
            }
            Err(e) => {
                tracing::warn!("registry import failed: {e}");
                self.status(&format!("Not written: {e}."));
            }
        }
    }

    fn open_key(&mut self, row: usize) {
        let Some(key) = self.parsed.items.get(row).map(|i| i.key().to_string()) else {
            return self.status("Select a key or value first.");
        };
        if let Err(e) = powerkit_platform::shell::registry_set(REGEDIT_STATE, "LastKey", &format!("Computer\\{key}")) {
            return self.status(&format!("Could not point Registry Editor at the key: {e}"));
        }
        // Registry Editor asks for administrator permission, and the shell waits for
        // that answer; -m lets it open beside a copy that is already running.
        std::thread::spawn(|| {
            if let Err(e) = powerkit_platform::shell::open("regedit.exe", Some("-m")) {
                tracing::warn!("Registry Editor did not start: {e}");
            }
        });
        self.status(&format!("Opening {key} in Registry Editor."));
    }
}
