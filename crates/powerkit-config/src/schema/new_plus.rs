use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewPlus {
    pub enabled: bool,
    /// Folder holding the templates. Empty: `NewPlus\Templates` in PowerKit's data folder.
    pub templates: String,
    /// Show file templates without their extension in the menu.
    pub hide_extension: bool,
    /// Drop leading digits, spaces and dots (used to order the menu) from the menu
    /// and from the created name.
    pub hide_starting_digits: bool,
    /// Replace `$YYYY`, `$MM`, `$PARENT_FOLDER_NAME`, `%USERNAME%` and the like in
    /// created names.
    pub replace_variables: bool,
}

impl Default for NewPlus {
    fn default() -> Self {
        Self {
            enabled: true,
            templates: String::new(),
            hide_extension: false,
            hide_starting_digits: true,
            replace_variables: false,
        }
    }
}

impl NewPlus {
    pub(crate) fn sanitize(&mut self, _w: &mut Vec<String>) {
        self.templates = self.templates.trim().to_string();
    }
}
