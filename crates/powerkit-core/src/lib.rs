//! Platform-independent base types: geometry, key names, shortcuts, events and the
//! module lifecycle state machine. Nothing in here touches the operating system.

pub mod event;
pub mod geometry;
pub mod keys;
pub mod lifecycle;
pub mod shortcut;

pub use event::{Event, MouseEvent, PopupEvent, TrayEvent, WindowEventKind};
pub use geometry::Rect;
pub use lifecycle::{Lifecycle, State, Transition};
pub use shortcut::{Mods, Shortcut, ShortcutError};

/// Identifiers of the built-in utilities, in display order.
pub const MODULE_IDS: [&str; 14] = [
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
];
