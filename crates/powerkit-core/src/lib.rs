//! Platform-independent base types: geometry, key names, shortcuts, events and the
//! module lifecycle state machine. Nothing in here touches the operating system.

pub mod event;
pub mod geometry;
pub mod keys;
pub mod lifecycle;
pub mod shortcut;
pub mod zone_layout;

pub use event::{
    Event, MouseEvent, PeekEvent, PopupEvent, PowerDisplayEvent, RegistryEvent, RenameEvent, RenameForm, SettingValue,
    SettingsEvent, TableEvent, TrayEvent, WindowEventKind,
};
pub use geometry::Rect;
pub use lifecycle::{Lifecycle, State, Transition};
pub use shortcut::{Mods, Shortcut, ShortcutError};
pub use zone_layout::ZoneLayout;

/// Identifiers of the built-in utilities, in display order.
pub const MODULE_IDS: [&str; 36] = [
    "fancy_zones",
    "power_rename",
    "color_picker",
    "keyboard_manager",
    "always_on_top",
    "launcher",
    "awake",
    "light_switch",
    "quick_accent",
    "mouse_utilities",
    "crop_and_lock",
    "shortcut_guide",
    "grab_and_move",
    "window_hopper",
    "hosts_editor",
    "environment_variables",
    "registry_preview",
    "new_plus",
    "file_locksmith",
    "image_resizer",
    "text_extractor",
    "window_tiling",
    "quick_notes",
    "clipboard_history",
    "screenshot",
    "screen_recorder",
    "power_display",
    "screen_ruler",
    "zoom_it",
    "workspaces",
    "peek",
    "power_plan",
    "advanced_paste",
    "command_not_found",
    "file_explorer",
    "mouse_without_borders",
];
