//! Peek: press the shortcut in File Explorer or on the desktop to see the selected file
//! (a picture, text, or its details) without opening it, and browse with the arrows.

#[cfg(windows)]
mod live;
#[cfg(windows)]
pub use live::is_explorer;

use anyhow::{bail, Result};
use powerkit_config::{Config, Peek as Settings};
use powerkit_core::{Event, WindowEventKind};
use powerkit_plugin_api::{CommandSpec, Ctx, HotkeyBinding, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const ID: &str = "peek";
/// Payload `{ "hwnd": ... }`: the File Explorer window or desktop to preview from.
pub const OPEN_EVENT: &str = "peek.open";

/// Text files are shown up to this many bytes.
pub const TEXT_LIMIT: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Text,
    Pdf,
    /// Drawn as a picture.
    Svg,
    /// Not known by its extension; text if its first bytes look like text.
    Unknown,
    /// Shown as its Windows thumbnail (video frame, document page, album art) when
    /// it has one, otherwise as details.
    Other,
}

const PICTURES: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "gif", "bmp", "dib", "tif", "tiff", "ico", "webp", "heic", "heif", "avif",
    "jxr", "wdp",
];
const TEXT: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonc",
    "xml",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "rs",
    "py",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "c",
    "h",
    "cpp",
    "hpp",
    "cc",
    "cs",
    "java",
    "kt",
    "go",
    "rb",
    "php",
    "lua",
    "sql",
    "html",
    "htm",
    "css",
    "scss",
    "sh",
    "bash",
    "ps1",
    "psm1",
    "bat",
    "cmd",
    "reg",
    "gitignore",
    "editorconfig",
    "env",
];
/// Files that are never text, so their first bytes are not read.
const BINARY: &[&str] = &[
    "exe", "dll", "sys", "msi", "zip", "7z", "rar", "gz", "tar", "iso", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "mp3", "mp4", "mkv", "avi", "mov", "wav", "flac", "ogg", "psd", "lnk",
];

pub fn kind(path: &Path) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if PICTURES.contains(&ext.as_str()) {
        Kind::Picture
    } else if ext == "pdf" {
        Kind::Pdf
    } else if ext == "svg" {
        Kind::Svg
    } else if TEXT.contains(&ext.as_str()) {
        Kind::Text
    } else if BINARY.contains(&ext.as_str()) {
        Kind::Other
    } else {
        Kind::Unknown
    }
}

/// Whether the start of a file reads as text: a Unicode byte-order mark, or UTF-8
/// without NUL bytes (the end may be cut in the middle of a character).
pub fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return true;
    }
    if bytes.contains(&0) {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && bytes.len() - e.valid_up_to() < 4,
    }
}

/// Text from a file's bytes, following a UTF-8 or UTF-16 byte-order mark.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if be {
                    u16::from_be_bytes(c)
                } else {
                    u16::from_le_bytes(c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    let shown = if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
    format!("{shown} {} ({bytes} bytes)", UNITS[unit])
}

/// `YYYY-MM-DD HH:MM` for seconds counted from 1970 (already in local time).
pub fn date_time(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// The files to browse and where to start: with several selected, just those; with
/// one, everything in the view starting at it. `None` when nothing is selected.
pub fn browse(items: Vec<PathBuf>, selected: &[usize]) -> Option<(Vec<PathBuf>, usize)> {
    match selected {
        [] => None,
        [one] => (*one < items.len()).then_some((items, *one)),
        many => {
            let picked: Vec<PathBuf> = many.iter().filter_map(|&i| items.get(i).cloned()).collect();
            (!picked.is_empty()).then_some((picked, 0))
        }
    }
}

#[derive(Default)]
pub struct Peek {
    settings: Settings,
    running: bool,
    /// The File Explorer window or desktop last in front, while it is in front.
    explorer: Option<isize>,
}

impl Peek {
    pub fn new() -> Self {
        Self::default()
    }

    fn open(&self, ctx: &mut Ctx) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Peek is turned off");
        }
        let Some(hwnd) = self.explorer else {
            bail!("Select a file in File Explorer first");
        };
        ctx.emit(OPEN_EVENT, json!({ "hwnd": hwnd }));
        Ok(())
    }
}

#[cfg(windows)]
fn explorer_in_front(hwnd: isize) -> bool {
    is_explorer(hwnd)
}

#[cfg(not(windows))]
fn explorer_in_front(_hwnd: isize) -> bool {
    false
}

impl Module for Peek {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Peek".into(),
            description: "Preview the file selected in File Explorer with a shortcut.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys, Permission::WindowManagement],
            commands: vec![CommandSpec {
                id: "open".into(),
                title: "Preview the selected file".into(),
                description: "Works while File Explorer or the desktop is in front.".into(),
            }],
            settings: vec![],
        }
    }

    /// The shortcut is held only while File Explorer or the desktop is in front, so a
    /// common one like Ctrl+Space keeps working everywhere else.
    fn hotkeys(&self, config: &Config) -> Vec<HotkeyBinding> {
        if self.explorer.is_none() {
            return Vec::new();
        }
        config
            .shortcuts()
            .into_iter()
            .filter(|(m, _, _)| *m == ID)
            .map(|(_, action, shortcut)| HotkeyBinding { action, shortcut })
            .collect()
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.peek.clone();
        self.running = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.peek.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "open" => self.open(ctx),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) -> Result<()> {
        // PowerKit's own windows are not reported, so the Peek window keeps the
        // shortcut for the Explorer window it was opened from.
        if let Event::Window {
            kind: WindowEventKind::Foreground,
            hwnd,
        } = event
        {
            self.explorer = explorer_in_front(*hwnd).then_some(*hwnd);
        }
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "open" => self.open(ctx)?,
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
    fn files_are_sorted_by_extension() {
        assert_eq!(kind(Path::new("a.PNG")), Kind::Picture);
        assert_eq!(kind(Path::new("notes.md")), Kind::Text);
        assert_eq!(kind(Path::new("setup.exe")), Kind::Other);
        assert_eq!(kind(Path::new("README")), Kind::Unknown);
        assert_eq!(kind(Path::new("Report.PDF")), Kind::Pdf);
        assert_eq!(kind(Path::new("logo.svg")), Kind::Svg);
    }

    #[test]
    fn text_is_told_apart_from_binary() {
        assert!(looks_like_text(b"hello\nworld"));
        assert!(looks_like_text("caf\u{e9}".as_bytes()));
        // Cut in the middle of the two-byte é.
        assert!(looks_like_text(&"caf\u{e9}".as_bytes()[..4]));
        assert!(!looks_like_text(b"MZ\x90\x00\x03"));
        assert!(!looks_like_text(&[0xC3, 0x28, b'a', b'b', b'c']));
        assert!(looks_like_text(&[0xFF, 0xFE, b'h', 0]));
    }

    #[test]
    fn byte_order_marks_are_followed() {
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'h', b'i']), "hi");
        assert_eq!(decode(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode(&[0xFE, 0xFF, 0, b'h', 0, b'i']), "hi");
        assert_eq!(decode(b"plain"), "plain");
    }

    #[test]
    fn sizes_and_dates_read_naturally() {
        assert_eq!(human_size(512), "512 bytes");
        assert_eq!(human_size(1536), "1.5 KB (1536 bytes)");
        assert_eq!(human_size(50 * 1024 * 1024), "50 MB (52428800 bytes)");
        assert_eq!(date_time(0), "1970-01-01 00:00");
        assert_eq!(date_time(1_709_210_096), "2024-02-29 12:34");
    }

    #[test]
    fn one_selected_browses_the_folder_several_browse_the_selection() {
        let items: Vec<PathBuf> = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
        assert_eq!(browse(items.clone(), &[2]), Some((items.clone(), 2)));
        assert_eq!(
            browse(items.clone(), &[3, 1]),
            Some((vec![PathBuf::from("d"), PathBuf::from("b")], 0))
        );
        assert_eq!(browse(items, &[]), None);
    }
}
