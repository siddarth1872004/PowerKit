use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Screenshot {
    pub enabled: bool,
    /// Drag a box over the part of the screen to take.
    pub region_shortcut: String,
    /// The window in front.
    pub window_shortcut: String,
    /// Every monitor.
    pub screen_shortcut: String,
    /// Opens the list of screenshots taken.
    pub manager_shortcut: String,
    /// Where pictures are saved; empty is Pictures\Screenshots.
    pub folder: String,
    pub copy_to_clipboard: bool,
    pub save_to_folder: bool,
}

impl Default for Screenshot {
    fn default() -> Self {
        Self {
            enabled: true,
            region_shortcut: "Win+Alt+S".into(),
            window_shortcut: "Win+Alt+W".into(),
            screen_shortcut: "Win+Alt+F".into(),
            manager_shortcut: String::new(),
            folder: String::new(),
            copy_to_clipboard: true,
            save_to_folder: true,
        }
    }
}

impl Screenshot {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = Screenshot::default();
        fix_shortcut(&mut self.region_shortcut, &d.region_shortcut, "Screenshot", w);
        fix_shortcut(&mut self.window_shortcut, &d.window_shortcut, "Screenshot", w);
        fix_shortcut(&mut self.screen_shortcut, &d.screen_shortcut, "Screenshot", w);
        fix_shortcut(&mut self.manager_shortcut, &d.manager_shortcut, "Screenshot", w);
        self.folder = self.folder.trim().to_string();
        if !self.copy_to_clipboard && !self.save_to_folder {
            w.push("screenshot: a screenshot must be copied or saved; saving to the folder".into());
            self.save_to_folder = true;
        }
    }
}
