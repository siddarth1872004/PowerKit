//! Events delivered by the platform layer to the background host and its modules.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEventKind {
    /// A different window became the foreground window.
    Foreground,
    /// The user started dragging or resizing a window.
    MoveSizeStart,
    /// The user finished dragging or resizing a window.
    MoveSizeEnd,
    /// A top-level window moved or changed size.
    LocationChange,
    Destroyed,
    Minimized,
    Restored,
    Shown,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    LeftDown { x: i32, y: i32 },
    LeftUp { x: i32, y: i32 },
    RightDown { x: i32, y: i32 },
    RightUp { x: i32, y: i32 },
    Wheel { delta: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Icon double-clicked / default action.
    Activate,
    /// The user asked for the context menu (right click).
    ContextMenu,
}

/// Input from the launcher popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupEvent {
    /// The search text changed.
    Query(String),
    /// The user chose a result row (Enter or click).
    Activate(usize),
    /// The popup closed without a choice (Esc or focus lost).
    Dismiss,
}

/// What the user did in the settings window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEvent {
    /// A section was chosen in the list on the left.
    Section(usize),
    /// A setting's value was edited and applied (Enter or the Apply button).
    Edit { row: usize, text: String },
    /// A setting was double-clicked.
    Toggle(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A registered global hotkey fired (identifier assigned at registration).
    Hotkey(i32),
    /// A timer created through the platform layer elapsed.
    Timer(usize),
    /// Something happened to a top-level window (`hwnd` is the raw handle value).
    Window {
        kind: WindowEventKind,
        hwnd: isize,
    },
    /// Monitors were added, removed, rearranged, or changed resolution/DPI.
    DisplayChanged,
    ClipboardChanged,
    /// Mouse input captured while a mouse hook is active.
    Mouse(MouseEvent),
    /// A key pressed while an input capture is active (virtual-key code).
    Key(u32),
    Tray(TrayEvent),
    Popup(PopupEvent),
    Settings(SettingsEvent),
    /// Another thread asked the main loop to run (IPC request pending).
    Wake,
    /// The session is ending or the process was asked to exit.
    Quit,
}
