//! The PowerKit background host as a library, so its logic can be tested without a
//! tray icon or real global hotkeys.

pub mod host;

pub use host::{Backend, Host, Output};

use powerkit_plugin_api::Module;

/// The built-in utilities, statically registered in display order.
pub fn builtin_modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(power_rename::PowerRename::new()),
        Box::new(always_on_top::AlwaysOnTop::new()),
        Box::new(color_picker::ColorPicker::new()),
        Box::new(fancy_zones::FancyZones::new()),
        Box::new(keyboard_manager::KeyboardManager::new()),
        Box::new(launcher::Launcher::new()),
        Box::new(awake::Awake::new()),
        Box::new(light_switch::LightSwitch::new()),
        Box::new(quick_accent::QuickAccent::new()),
    ]
}
