//! The PowerKit background host as a library, so its logic can be tested without a
//! tray icon or real global hotkeys.

pub mod env;
pub mod explorer_menu;
pub mod host;
pub mod hosts;
pub mod icons;
pub mod locksmith;
pub mod new_menu;
pub mod peek_view;
pub mod registry;
pub mod rename;
pub mod resizer;
pub mod settings;
pub mod update;
pub mod workspace_editor;

pub use host::{Backend, Host, Output};

use powerkit_plugin_api::Module;

/// The built-in utilities, statically registered in display order.
pub fn builtin_modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(power_rename::PowerRename::new()),
        Box::new(zoom_it::ZoomIt::new()),
        Box::new(workspaces::Workspaces::new()),
        Box::new(peek::Peek::new()),
        Box::new(always_on_top::AlwaysOnTop::new()),
        Box::new(color_picker::ColorPicker::new()),
        Box::new(fancy_zones::FancyZones::new()),
        Box::new(keyboard_manager::KeyboardManager::new()),
        Box::new(launcher::Launcher::new()),
        Box::new(awake::Awake::new()),
        Box::new(light_switch::LightSwitch::new()),
        Box::new(quick_accent::QuickAccent::new()),
        Box::new(mouse_utilities::MouseUtilities::new()),
        Box::new(crop_and_lock::CropAndLock::new()),
        Box::new(shortcut_guide::ShortcutGuide::new()),
        Box::new(grab_and_move::GrabAndMove::new()),
        Box::new(window_hopper::WindowHopper::new()),
        Box::new(hosts_editor::HostsEditor::new()),
        Box::new(environment_variables::EnvironmentVariables::new()),
        Box::new(registry_preview::RegistryPreview::new()),
        Box::new(new_plus::NewPlus::new()),
        Box::new(file_locksmith::FileLocksmith::new()),
        Box::new(image_resizer::ImageResizer::new()),
        Box::new(text_extractor::TextExtractor::new()),
        Box::new(power_display::PowerDisplay::new()),
        Box::new(screen_ruler::ScreenRuler::new()),
    ]
}
