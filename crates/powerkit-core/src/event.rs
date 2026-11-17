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
    /// A page was chosen in the navigation pane, by its index there.
    Page(usize),
    /// The setting with this key was changed to `value`.
    Set { key: String, value: SettingValue },
    /// The button or link with this key was clicked.
    Action(String),
}

/// A value entered in the settings window. Numbers arrive as the text typed, so
/// the owner can say what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Bool(bool),
    Text(String),
}

/// What is filled in on the PowerRename window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameForm {
    pub search: String,
    pub replace: String,
    pub regex: bool,
    pub match_case: bool,
    pub all_occurrences: bool,
    /// 0 the name without its extension, 1 only the extension, 2 both.
    pub target: usize,
    pub files: bool,
    pub folders: bool,
    /// 0 unchanged, then lowercase, UPPERCASE, Title Case and Sentence case.
    pub case: usize,
    pub enumerate: bool,
}

impl Default for RenameForm {
    fn default() -> Self {
        Self {
            search: String::new(),
            replace: String::new(),
            regex: false,
            match_case: false,
            all_occurrences: true,
            target: 0,
            files: true,
            folders: true,
            case: 0,
            enumerate: false,
        }
    }
}

/// What the user did in the PowerRename window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameEvent {
    Changed(RenameForm),
    Apply,
    Undo,
    Closed,
}

/// What the user did in a table editor window; rows count the listed rows and
/// fields hold the edit boxes' text in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableEvent {
    Toggle {
        row: usize,
        on: bool,
    },
    Add(Vec<String>),
    Update {
        row: usize,
        fields: Vec<String>,
    },
    Delete(usize),
    Save,
    Reload,
    /// The selector above the list changed to this entry.
    Choice(usize),
}

/// What the user did in the Peek window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekEvent {
    Previous,
    Next,
    /// Open the file shown with its program.
    Open,
    /// The window was closed.
    Closed,
}

/// What the user did in Registry Preview; rows count the listed keys and values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryEvent {
    /// The text was edited; carries all of it.
    Changed(String),
    Open,
    Save,
    SaveAs,
    Write,
    /// Show the key of this row in Registry Editor.
    OpenKey(usize),
}

/// What the user did in the Power Display window; `section` and `row` index what its
/// owner last showed. A choice that asks first is reported only once confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerDisplayEvent {
    Slider { section: usize, row: usize, value: u32 },
    Choice { section: usize, row: usize, index: usize },
    Refresh,
    Closed,
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
    /// A click on the tray icon with this id.
    Tray {
        icon: u32,
        event: TrayEvent,
    },
    Popup(PopupEvent),
    Settings(SettingsEvent),
    Rename(RenameEvent),
    /// From the table editor window whose spec has this id.
    Table {
        table: &'static str,
        event: TableEvent,
    },
    Registry(RegistryEvent),
    Peek(PeekEvent),
    PowerDisplay(PowerDisplayEvent),
    /// Save was chosen in the FancyZones layout editor; these are all the layouts.
    ZoneLayouts(Vec<crate::ZoneLayout>),
    /// Another thread asked the main loop to run (IPC request pending).
    Wake,
    /// The session is ending or the process was asked to exit.
    Quit,
}
