//! The Quick Notes window and the file its text is kept in.

use powerkit_platform::notes_window::NotesWindow;
use std::path::Path;

#[derive(Default)]
pub struct NotesUi {
    window: Option<NotesWindow>,
}

/// The text box wants CR LF line ends; the file may have been edited elsewhere.
fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

impl NotesUi {
    /// Show the notes, or put them away when they are already in front.
    pub fn toggle(&mut self, data_dir: &Path, on_top: bool) -> Result<(), String> {
        if let Some(w) = self.window.as_mut() {
            if w.is_visible() && w.is_foreground() {
                w.hide();
                return Ok(());
            }
        } else {
            let path = data_dir.join(quick_notes::FILE);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => crlf(&t),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(format!("could not read {}: {e}", path.display())),
            };
            let mut w = NotesWindow::new(&text).map_err(|e| format!("could not open Quick Notes: {e}"))?;
            crate::icons::apply(quick_notes::ID, |ico| w.set_icon(ico));
            self.window = Some(w);
        }
        if let Some(w) = self.window.as_mut() {
            w.set_topmost(on_top);
            w.show();
        }
        Ok(())
    }

    pub fn save(&self, text: &str, data_dir: &Path) -> Result<(), String> {
        let path = data_dir.join(quick_notes::FILE);
        powerkit_config::write_atomic(&path, text.as_bytes())
            .map_err(|e| format!("Quick Notes not saved to {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_written_elsewhere_get_windows_line_ends() {
        assert_eq!(crlf("a\nb\r\nc"), "a\r\nb\r\nc");
    }

    #[test]
    fn notes_are_saved_in_the_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        NotesUi::default().save("one\r\ntwo", dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(quick_notes::FILE)).unwrap(),
            "one\r\ntwo"
        );
    }
}
