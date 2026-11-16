//! PowerKit's icon and each utility's, built into the program.

pub const APP: &[u8] = include_bytes!("../../../assets/icon.ico");

/// The icon (an .ico file) of the utility with id `id`.
pub fn module(id: &str) -> Option<&'static [u8]> {
    Some(match id {
        "always_on_top" => include_bytes!("../../../assets/modules/always_on_top.ico"),
        "color_picker" => include_bytes!("../../../assets/modules/color_picker.ico"),
        "fancy_zones" => include_bytes!("../../../assets/modules/fancy_zones.ico"),
        "power_rename" => include_bytes!("../../../assets/modules/power_rename.ico"),
        "keyboard_manager" => include_bytes!("../../../assets/modules/keyboard_manager.ico"),
        "launcher" => include_bytes!("../../../assets/modules/launcher.ico"),
        "awake" => include_bytes!("../../../assets/modules/awake.ico"),
        "light_switch" => include_bytes!("../../../assets/modules/light_switch.ico"),
        "quick_accent" => include_bytes!("../../../assets/modules/quick_accent.ico"),
        "mouse_utilities" => include_bytes!("../../../assets/modules/mouse_utilities.ico"),
        "crop_and_lock" => include_bytes!("../../../assets/modules/crop_and_lock.ico"),
        "shortcut_guide" => include_bytes!("../../../assets/modules/shortcut_guide.ico"),
        "grab_and_move" => include_bytes!("../../../assets/modules/grab_and_move.ico"),
        "window_hopper" => include_bytes!("../../../assets/modules/window_hopper.ico"),
        "hosts_editor" => include_bytes!("../../../assets/modules/hosts_editor.ico"),
        "environment_variables" => include_bytes!("../../../assets/modules/environment_variables.ico"),
        "registry_preview" => include_bytes!("../../../assets/modules/registry_preview.ico"),
        "new_plus" => include_bytes!("../../../assets/modules/new_plus.ico"),
        "file_locksmith" => include_bytes!("../../../assets/modules/file_locksmith.ico"),
        "image_resizer" => include_bytes!("../../../assets/modules/image_resizer.ico"),
        "text_extractor" => include_bytes!("../../../assets/modules/text_extractor.ico"),
        "power_display" => include_bytes!("../../../assets/modules/power_display.ico"),
        "screen_ruler" => include_bytes!("../../../assets/modules/screen_ruler.ico"),
        "zoom_it" => include_bytes!("../../../assets/modules/zoom_it.ico"),
        "workspaces" => include_bytes!("../../../assets/modules/workspaces.ico"),
        "peek" => include_bytes!("../../../assets/modules/peek.ico"),
        _ => return None,
    })
}

/// Give a utility's window its icon; a window without one still works, so a
/// failure is only logged.
pub fn apply(id: &str, set: impl FnOnce(&'static [u8]) -> powerkit_platform::Result<()>) {
    let Some(ico) = module(id) else {
        return tracing::warn!("no icon for {id}");
    };
    if let Err(e) = set(ico) {
        tracing::warn!("could not set the {id} window icon: {e}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_utility_has_an_icon() {
        for id in powerkit_core::MODULE_IDS {
            let ico = super::module(id).unwrap_or_else(|| panic!("{id} has no icon"));
            assert_eq!(&ico[..4], [0, 0, 1, 0], "{id}");
        }
    }
}
