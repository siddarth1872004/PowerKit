//! What the settings window shows: the navigation pane and one page of setting
//! cards. The owner builds these; the window lays them out and draws them.

/// One row of the navigation pane.
#[derive(Debug, Clone, PartialEq)]
pub enum NavEntry {
    /// A page. `depth` 1 puts it under the category above it.
    Page {
        title: String,
        icon: Option<&'static [u8]>,
        glyph: Option<char>,
        depth: u8,
    },
    /// A category heading that folds the pages under it.
    Category { title: String, glyph: char },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// Identifies the page; the scroll position is kept while it stays the same.
    pub id: String,
    pub title: String,
    /// Shown beside the walkthrough at the top of the page.
    pub description: String,
    pub groups: Vec<Group>,
}

/// Cards under a heading. An empty title puts the cards straight under the one before.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub title: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Reported back in events for this item.
    pub key: String,
    pub title: String,
    pub description: String,
    pub icon: Option<&'static [u8]>,
    pub glyph: Option<char>,
    pub control: Control,
    /// Rows shown when the card is expanded. A card with children has a chevron.
    pub children: Vec<Item>,
    pub enabled: bool,
    /// Clicking the card itself sends [`Action`](powerkit_core::SettingsEvent::Action)
    /// with the key, and a chevron points right.
    pub link: bool,
}

impl Item {
    pub fn new(key: impl Into<String>, title: impl Into<String>, control: Control) -> Item {
        Item {
            key: key.into(),
            title: title.into(),
            description: String::new(),
            icon: None,
            glyph: None,
            control,
            children: Vec::new(),
            enabled: true,
            link: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    None,
    Toggle(bool),
    /// `value` is a shortcut in configuration form ("Win+Shift+C"), or empty for none.
    Shortcut {
        value: String,
        default: String,
    },
    /// `(value, label)` pairs; the value of the chosen one.
    Choice {
        options: Vec<(String, String)>,
        selected: String,
    },
    /// A text box; `wide` puts it on its own line under the title, and `lines` above 1
    /// makes it a multi-line box.
    Text {
        value: String,
        placeholder: String,
        lines: u8,
        wide: bool,
    },
    /// A number box; what is typed is checked by the owner.
    Number {
        value: String,
    },
    /// `#RRGGBB`.
    Color(String),
    /// A button that sends an action; `accent` fills it with the accent color.
    Button {
        label: String,
        accent: bool,
    },
    /// Pictures of zone layouts on their own lines under the title; clicking one sends
    /// its id.
    Layouts {
        options: Vec<LayoutChoice>,
        selected: String,
    },
}

/// A layout shown by [`Control::Layouts`].
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutChoice {
    pub id: String,
    pub name: String,
    /// Zones as `[x, y, w, h]`, fractions of the screen.
    pub zones: Vec<[f32; 4]>,
}

/// A message shown under the page title until it is replaced or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}
