//! What the guide lists and how it flows into columns. No Windows calls here.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Keys joined by '+', e.g. "Win+Shift+S".
    pub keys: String,
    pub what: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub items: Vec<Item>,
}

fn section(title: &str, items: &[(&str, &str)]) -> Section {
    Section {
        title: title.into(),
        items: items
            .iter()
            .map(|(k, w)| Item {
                keys: (*k).into(),
                what: (*w).into(),
            })
            .collect(),
    }
}

/// Shortcuts Windows itself provides.
pub fn windows_sections() -> Vec<Section> {
    vec![
        section(
            "Windows",
            &[
                ("Win+E", "File Explorer"),
                ("Win+I", "Settings"),
                ("Win+R", "Run"),
                ("Win+S", "Search"),
                ("Win+A", "Quick Settings"),
                ("Win+N", "Notifications and calendar"),
                ("Win+V", "Clipboard history"),
                ("Win+Period", "Emoji and symbols"),
                ("Win+Shift+S", "Screenshot a region"),
                ("Win+L", "Lock the PC"),
                ("Win+X", "Quick Link menu"),
                ("Win+P", "Project to a display"),
                ("Win+K", "Cast to a device"),
                ("Win+H", "Voice typing"),
                ("Win+G", "Game Bar"),
                ("Win+B", "Focus the system tray"),
                ("Ctrl+Shift+Esc", "Task Manager"),
            ],
        ),
        section(
            "Windows and desktops",
            &[
                ("Win+D", "Show or hide the desktop"),
                ("Win+M", "Minimize all windows"),
                ("Win+Home", "Minimize all but this window"),
                ("Win+Up", "Maximize"),
                ("Win+Down", "Restore or minimize"),
                ("Win+Left", "Snap to the left"),
                ("Win+Right", "Snap to the right"),
                ("Win+Z", "Snap layouts"),
                ("Win+Shift+Up", "Stretch to top and bottom"),
                ("Win+Shift+Left", "Move to the left monitor"),
                ("Win+Shift+Right", "Move to the right monitor"),
                ("Alt+Tab", "Switch windows"),
                ("Alt+F4", "Close the window"),
                ("Win+Tab", "Task view"),
                ("Win+Ctrl+D", "New virtual desktop"),
                ("Win+Ctrl+Left", "Previous virtual desktop"),
                ("Win+Ctrl+Right", "Next virtual desktop"),
                ("Win+Ctrl+F4", "Close the virtual desktop"),
            ],
        ),
        section(
            "Taskbar",
            &[
                ("Win+1-9", "Open or switch to that app"),
                ("Win+Shift+1-9", "Open another instance"),
                ("Win+Ctrl+1-9", "Last window of that app"),
                ("Win+Alt+1-9", "Jump list of that app"),
                ("Win+T", "Cycle through taskbar apps"),
            ],
        ),
    ]
}

const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
];

/// Shortcuts of the focused application, when it is one PowerKit knows.
pub fn app_section(exe: &str) -> Option<Section> {
    Some(match exe {
        "explorer.exe" => section(
            "File Explorer",
            &[
                ("Ctrl+N", "New window"),
                ("Ctrl+T", "New tab"),
                ("Ctrl+W", "Close tab"),
                ("Ctrl+Shift+N", "New folder"),
                ("Alt+Up", "Up one folder"),
                ("Alt+Left", "Back"),
                ("Alt+Right", "Forward"),
                ("Ctrl+L", "Address bar"),
                ("Ctrl+F", "Search"),
                ("F2", "Rename"),
                ("Alt+Enter", "Properties"),
                ("Alt+P", "Preview pane"),
                ("Shift+Delete", "Delete permanently"),
            ],
        ),
        "notepad.exe" => section(
            "Notepad",
            &[
                ("Ctrl+N", "New tab"),
                ("Ctrl+O", "Open"),
                ("Ctrl+S", "Save"),
                ("Ctrl+Shift+S", "Save as"),
                ("Ctrl+F", "Find"),
                ("Ctrl+H", "Replace"),
                ("Ctrl+G", "Go to line"),
                ("F5", "Insert time and date"),
            ],
        ),
        "code.exe" => section(
            "Visual Studio Code",
            &[
                ("Ctrl+Shift+P", "Command palette"),
                ("Ctrl+P", "Go to file"),
                ("Ctrl+Backtick", "Terminal"),
                ("Ctrl+B", "Toggle the side bar"),
                ("Ctrl+Slash", "Toggle comment"),
                ("Ctrl+D", "Select the next match"),
                ("Alt+Up", "Move the line up"),
                ("Ctrl+Shift+K", "Delete the line"),
                ("Shift+Alt+F", "Format the document"),
                ("F12", "Go to definition"),
                ("F2", "Rename symbol"),
            ],
        ),
        "windowsterminal.exe" => section(
            "Terminal",
            &[
                ("Ctrl+Shift+T", "New tab"),
                ("Ctrl+Shift+W", "Close the pane"),
                ("Alt+Shift+Plus", "Split the pane"),
                ("Ctrl+Shift+F", "Find"),
                ("Ctrl+Comma", "Settings"),
            ],
        ),
        b if BROWSERS.contains(&b) => {
            let private = if b == "firefox.exe" {
                "Ctrl+Shift+P"
            } else {
                "Ctrl+Shift+N"
            };
            section(
                "Browser",
                &[
                    ("Ctrl+T", "New tab"),
                    ("Ctrl+W", "Close tab"),
                    ("Ctrl+Shift+T", "Reopen the closed tab"),
                    ("Ctrl+Tab", "Next tab"),
                    ("Ctrl+Shift+Tab", "Previous tab"),
                    ("Ctrl+L", "Address bar"),
                    ("Ctrl+F", "Find on the page"),
                    ("Ctrl+D", "Bookmark the page"),
                    ("Ctrl+H", "History"),
                    ("Ctrl+J", "Downloads"),
                    (private, "Private window"),
                    ("F5", "Reload"),
                    ("F11", "Full screen"),
                ],
            )
        }
        _ => return None,
    })
}

fn describe(module: &str, action: &str) -> String {
    let known = match (module, action) {
        ("always_on_top", "toggle_pin") => "Pin the window on top",
        ("color_picker", "pick") => "Pick a color",
        ("launcher", "toggle") => "Launcher",
        ("light_switch", "toggle") => "Switch light and dark",
        ("mouse_utilities", "find_my_mouse") => "Find my mouse",
        ("mouse_utilities", "highlighter") => "Highlight mouse clicks",
        ("mouse_utilities", "crosshairs") => "Mouse crosshairs",
        ("mouse_utilities", "glide") => "Gliding cursor",
        ("mouse_utilities", "jump") => "Mouse Jump",
        ("crop_and_lock", "thumbnail") => "Crop into a live copy",
        ("crop_and_lock", "reparent") => "Crop the window itself",
        ("fancy_zones", "next_zone") => "Move to the next zone",
        ("fancy_zones", "prev_zone") => "Move to the previous zone",
        ("fancy_zones", "cycle_layout") => "Next zone layout",
        ("fancy_zones", "toggle_overlay") => "Show the zones",
        ("window_hopper", "next") => "Next window of this app",
        ("window_hopper", "previous") => "Previous window of this app",
        ("quick_notes", "open") => "Quick Notes",
        ("clipboard_history", "open") => "Clipboard History",
        ("screenshot", "region") => "Screenshot of an area",
        ("screenshot", "window") => "Screenshot of the window",
        ("screenshot", "screen") => "Screenshot of the screen",
        ("screenshot", "manager") => "Screenshots",
        ("screen_recorder", "record") => "Record the screen",
        ("window_tiling", "left") => "Tile the window left",
        ("window_tiling", "right") => "Tile the window right",
        ("window_tiling", "up") => "Tile up or maximize",
        ("window_tiling", "down") => "Tile down or restore",
        ("hosts_editor", "open") => "Hosts File Editor",
        ("environment_variables", "open") => "Environment Variables",
        ("registry_preview", "open") => "Registry Preview",
        ("shortcut_guide", "toggle") => "This guide",
        _ => return format!("{module} {action}").replace('_', " "),
    };
    known.into()
}

/// PowerKit's own shortcuts, from `Config::shortcuts()`. The nine zone-number shortcuts
/// share a line.
pub fn powerkit_section(shortcuts: &[(&str, String, String)]) -> Section {
    let mut items = Vec::new();
    for (module, action, keys) in shortcuts {
        if let Some(n) = action.strip_prefix("zone_") {
            if n == "1" {
                let mods = keys.strip_suffix("+1").unwrap_or(keys);
                items.push(Item {
                    keys: format!("{mods}+1-9"),
                    what: "Move to that zone".into(),
                });
            }
            continue;
        }
        items.push(Item {
            keys: keys.clone(),
            what: describe(module, action),
        });
    }
    Section {
        title: "PowerKit".into(),
        items,
    }
}

/// How a key name is shown on its key cap.
pub fn cap_label(key: &str) -> &str {
    match key {
        "Slash" => "/",
        "Backslash" => "\\",
        "Period" => ".",
        "Comma" => ",",
        "Semicolon" => ";",
        "Minus" => "-",
        "Equals" | "Plus" => "+",
        "Backtick" => "`",
        "Quote" => "'",
        "LBracket" => "[",
        "RBracket" => "]",
        "Left" => "\u{2190}",
        "Up" => "\u{2191}",
        "Right" => "\u{2192}",
        "Down" => "\u{2193}",
        k => k,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    Header(&'a str),
    Item(&'a Item),
}

/// Lay sections out in columns of at most `rows` lines. A section never starts on a
/// column's last line, and one that continues into the next column repeats its header.
pub fn flow(sections: &[Section], rows: usize) -> Vec<Vec<Line<'_>>> {
    let rows = rows.max(2);
    let mut cols: Vec<Vec<Line>> = vec![Vec::new()];
    for s in sections.iter().filter(|s| !s.items.is_empty()) {
        let col = cols.last_mut().expect("never empty");
        if !col.is_empty() && rows - col.len() < 2 {
            cols.push(Vec::new());
        }
        cols.last_mut().expect("never empty").push(Line::Header(&s.title));
        for item in &s.items {
            if cols.last().expect("never empty").len() == rows {
                cols.push(vec![Line::Header(&s.title)]);
            }
            cols.last_mut().expect("never empty").push(Line::Item(item));
        }
    }
    cols.retain(|c| !c.is_empty());
    cols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_keeps_headers_with_their_items() {
        let s = vec![
            section("A", &[("1", "a"), ("2", "b"), ("3", "c")]),
            section("B", &[("4", "d"), ("5", "e")]),
            section("Empty", &[]),
        ];
        let cols = flow(&s, 5);
        // A takes 4 lines; B cannot start on the last line, so it opens column 2.
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].len(), 4);
        assert_eq!(cols[1][0], Line::Header("B"));
        assert_eq!(cols[1].len(), 3);

        let cols = flow(&s, 3);
        assert!(cols.iter().all(|c| c.len() <= 3 && matches!(c[0], Line::Header(_))));
        let items: usize = cols.iter().flatten().filter(|l| matches!(l, Line::Item(_))).count();
        assert_eq!(items, 5);
        assert_eq!(cols[1][0], Line::Header("A"), "continued section repeats its header");
    }

    #[test]
    fn powerkit_section_merges_zone_numbers() {
        let sc = vec![
            ("color_picker", "pick".to_string(), "Win+Shift+C".to_string()),
            ("fancy_zones", "zone_1".to_string(), "Ctrl+Win+Alt+1".to_string()),
            ("fancy_zones", "zone_2".to_string(), "Ctrl+Win+Alt+2".to_string()),
            ("someone", "do_it".to_string(), "Ctrl+F9".to_string()),
        ];
        let s = powerkit_section(&sc);
        let got: Vec<_> = s.items.iter().map(|i| (i.keys.as_str(), i.what.as_str())).collect();
        assert_eq!(
            got,
            [
                ("Win+Shift+C", "Pick a color"),
                ("Ctrl+Win+Alt+1-9", "Move to that zone"),
                ("Ctrl+F9", "someone do it"),
            ]
        );
    }

    #[test]
    fn apps_and_caps() {
        assert_eq!(app_section("firefox.exe").unwrap().items[10].keys, "Ctrl+Shift+P");
        assert_eq!(app_section("msedge.exe").unwrap().items[10].keys, "Ctrl+Shift+N");
        assert!(app_section("unknown.exe").is_none());
        assert_eq!(cap_label("Slash"), "/");
        assert_eq!(cap_label("Win"), "Win");
    }
}
