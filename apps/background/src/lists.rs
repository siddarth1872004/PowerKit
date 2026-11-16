//! Table windows that edit the lists in the configuration which the settings page
//! cannot show as single values: Keyboard Manager remaps and profiles, PowerRename
//! presets, PowerKit Run commands and Image Resizer sizes. Every change is saved
//! straight away, and a change the configuration would not keep is refused with the
//! reason instead.

use crate::Host;
use powerkit_config::{Config, CustomCommand, KeyRemap, Preset, RemapTarget, RenameOp, ShortcutRemap};
use powerkit_core::TableEvent;
use powerkit_platform::table_window::{Field, TableRow, TableSpec, TableWindow};
use serde_json::json;

const fn field(label: &'static str, label_width: i32, width: i32, capture: bool) -> Field {
    Field {
        label,
        label_width,
        width,
        capture,
    }
}

static REMAPS: TableSpec = TableSpec {
    id: "lists.remaps",
    title: "Keyboard Manager",
    columns: &[("From", 220), ("To", 220), ("Only in", 200)],
    fields: &[
        field("From", 40, 0, true),
        field("To", 24, 0, true),
        field("Only in", 50, 160, false),
    ],
    checks: false,
    choices: &["Keys", "Shortcuts"],
    buttons: ["Add", "Update", "Delete", "", "Profiles..."],
};

static PROFILES: TableSpec = TableSpec {
    id: "lists.profiles",
    title: "Keyboard Manager profiles",
    columns: &[("Profile", 260), ("Key remaps", 110), ("Shortcut remaps", 130)],
    fields: &[field("Name", 40, 0, false)],
    checks: false,
    choices: &[],
    buttons: ["Add", "Rename", "Delete", "", ""],
};

static PRESETS: TableSpec = TableSpec {
    id: "lists.presets",
    title: "PowerRename presets",
    columns: &[
        ("Preset", 180),
        ("Search for", 200),
        ("Replace with", 200),
        ("Other steps", 260),
    ],
    fields: &[
        field("Name", 40, 140, false),
        field("Search for", 66, 0, false),
        field("Replace with", 80, 0, false),
    ],
    checks: false,
    choices: &[],
    buttons: ["Add", "Update", "Delete", "", ""],
};

static COMMANDS: TableSpec = TableSpec {
    id: "lists.commands",
    title: "PowerKit Run commands",
    columns: &[("Keyword", 110), ("Title", 180), ("Program", 300), ("Arguments", 240)],
    fields: &[
        field("Keyword", 56, 90, false),
        field("Title", 30, 140, false),
        field("Program", 56, 0, false),
        field("Arguments", 66, 160, false),
    ],
    checks: false,
    choices: &[],
    buttons: ["Add", "Update", "Delete", "", ""],
};

static SIZES: TableSpec = TableSpec {
    id: "lists.sizes",
    title: "Image Resizer sizes",
    columns: crate::resizer::COLUMNS,
    fields: crate::resizer::FIELDS,
    checks: false,
    choices: &[],
    buttons: ["Add", "Update", "Delete", "", ""],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    Remaps,
    Profiles,
    Presets,
    Commands,
    Sizes,
}

const GONE: &str = "That entry is no longer listed.";

impl List {
    fn spec(self) -> &'static TableSpec {
        match self {
            List::Remaps => &REMAPS,
            List::Profiles => &PROFILES,
            List::Presets => &PRESETS,
            List::Commands => &COMMANDS,
            List::Sizes => &SIZES,
        }
    }

    fn from_table(table: &str) -> Option<List> {
        [List::Remaps, List::Profiles, List::Presets, List::Commands, List::Sizes]
            .into_iter()
            .find(|l| l.spec().id == table)
    }
}

/// The list a settings card's button edits, by the card's dotted key.
fn for_setting(key: &str) -> Option<List> {
    match key {
        "keyboard_manager.profiles" => Some(List::Remaps),
        "power_rename.presets" => Some(List::Presets),
        "launcher.custom_commands" => Some(List::Commands),
        "image_resizer.sizes" => Some(List::Sizes),
        _ => None,
    }
}

fn target_text(t: &RemapTarget) -> String {
    match t {
        RemapTarget::Key(k) | RemapTarget::Shortcut(k) => k.clone(),
        RemapTarget::Disabled => "Disabled".into(),
    }
}

/// An empty box (or "Disabled") turns the key off; anything with a `+` is a shortcut.
fn parse_target(text: &str) -> RemapTarget {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("disabled") {
        RemapTarget::Disabled
    } else if t.len() > 1 && t.contains('+') {
        RemapTarget::Shortcut(t.into())
    } else {
        RemapTarget::Key(t.into())
    }
}

fn step_name(op: &RenameOp) -> &'static str {
    match op {
        RenameOp::Replace { .. } => "Replace",
        RenameOp::Remove { .. } => "Remove text",
        RenameOp::Case { .. } => "Change case",
        RenameOp::Insert { .. } => "Add text",
        RenameOp::ReplaceRange { .. } => "Replace characters",
        RenameOp::ReplaceSpaces { .. } => "Replace spaces",
        RenameOp::Number { .. } => "Number",
        RenameOp::Date { .. } => "Add date",
        RenameOp::Dedupe { .. } => "Remove repeats",
        RenameOp::Trim { .. } => "Trim",
        RenameOp::Extension { .. } => "Change extension",
        RenameOp::ParentFolder { .. } => "Add folder name",
        RenameOp::Template { .. } => "Template",
    }
}

fn rows(list: List, c: &Config, view: usize) -> Vec<TableRow> {
    let row = |cols: Vec<String>| (false, cols);
    match list {
        List::Remaps => {
            let Some(p) = c.keyboard_manager.active() else {
                return Vec::new();
            };
            if view == 0 {
                p.key_remaps
                    .iter()
                    .map(|r| row(vec![r.from.clone(), target_text(&r.to), String::new()]))
                    .collect()
            } else {
                p.shortcut_remaps
                    .iter()
                    .map(|r| row(vec![r.from.clone(), target_text(&r.to), r.app.clone()]))
                    .collect()
            }
        }
        List::Profiles => c
            .keyboard_manager
            .profiles
            .iter()
            .map(|p| {
                let mut name = p.name.clone();
                if name == c.keyboard_manager.active_profile {
                    name.push_str(" (in use)");
                }
                row(vec![
                    name,
                    p.key_remaps.len().to_string(),
                    p.shortcut_remaps.len().to_string(),
                ])
            })
            .collect(),
        List::Presets => c
            .power_rename
            .presets
            .iter()
            .map(|p| {
                let (search, replace, rest) = match p.options.ops.split_first() {
                    Some((RenameOp::Replace { find, with, .. }, rest)) => (find.clone(), with.clone(), rest),
                    _ => (String::new(), String::new(), &p.options.ops[..]),
                };
                let steps: Vec<&str> = rest.iter().map(step_name).collect();
                row(vec![p.name.clone(), search, replace, steps.join(", ")])
            })
            .collect(),
        List::Commands => c
            .launcher
            .custom_commands
            .iter()
            .map(|cmd| {
                row(vec![
                    cmd.keyword.clone(),
                    cmd.title.clone(),
                    cmd.program.clone(),
                    cmd.args.clone(),
                ])
            })
            .collect(),
        List::Sizes => crate::resizer::rows(&c.image_resizer.sizes),
    }
}

/// Add, change or delete the entry of `v` the table event is about; `make` builds
/// the entry from the boxes and the entry it replaces. Returns the row to select.
fn edit_vec<T>(
    v: &mut Vec<T>,
    event: TableEvent,
    make: impl Fn(&[String], Option<&T>) -> Result<T, String>,
) -> Result<Option<usize>, String> {
    match event {
        TableEvent::Add(f) => {
            v.push(make(&f, None)?);
            Ok(Some(v.len() - 1))
        }
        TableEvent::Update { row, fields } => {
            let new = make(&fields, Some(v.get(row).ok_or(GONE)?))?;
            v[row] = new;
            Ok(Some(row))
        }
        TableEvent::Delete(row) => {
            if row >= v.len() {
                return Err(GONE.into());
            }
            v.remove(row);
            Ok(row.checked_sub(1).or((!v.is_empty()).then_some(0)))
        }
        TableEvent::Toggle { .. } | TableEvent::Reload | TableEvent::Save | TableEvent::Choice(_) => Ok(None),
    }
}

fn text(f: &[String], i: usize) -> String {
    f.get(i).map_or(String::new(), |s| s.trim().to_string())
}

/// Whether another entry than `row` is already called `name`.
fn taken(names: impl Iterator<Item = String>, row: Option<usize>, name: &str) -> bool {
    names
        .enumerate()
        .any(|(i, n)| Some(i) != row && n.eq_ignore_ascii_case(name))
}

fn row_of(event: &TableEvent) -> Option<usize> {
    match event {
        TableEvent::Update { row, .. } => Some(*row),
        _ => None,
    }
}

fn edit(list: List, c: &mut Config, view: usize, event: TableEvent) -> Result<Option<usize>, String> {
    match list {
        List::Remaps => {
            let km = &mut c.keyboard_manager;
            let active = km.active_profile.clone();
            let p = km
                .profiles
                .iter_mut()
                .find(|p| p.name == active)
                .ok_or("The profile in use is missing; pick one on the settings page.")?;
            if view == 0 {
                edit_vec(&mut p.key_remaps, event, |f, _| {
                    let from = text(f, 0);
                    if from.is_empty() {
                        return Err("Click in From and press the key to remap.".into());
                    }
                    if from.len() > 1 && from.contains('+') {
                        return Err(format!("{from} is a shortcut; pick Shortcuts above to remap it."));
                    }
                    Ok(KeyRemap {
                        from,
                        to: parse_target(&text(f, 1)),
                    })
                })
            } else {
                edit_vec(&mut p.shortcut_remaps, event, |f, _| {
                    let from = text(f, 0);
                    if from.is_empty() {
                        return Err("Click in From and press the shortcut to remap.".into());
                    }
                    Ok(ShortcutRemap {
                        from,
                        to: parse_target(&text(f, 1)),
                        app: text(f, 2),
                    })
                })
            }
        }
        List::Profiles => {
            let km = &mut c.keyboard_manager;
            let names: Vec<String> = km.profiles.iter().map(|p| p.name.clone()).collect();
            let row = row_of(&event);
            let deleted = match &event {
                TableEvent::Delete(r) => names.get(*r).cloned(),
                _ => None,
            };
            let renamed = row.and_then(|r| names.get(r).cloned());
            let selected = edit_vec(&mut km.profiles, event, |f, old| {
                let name = text(f, 0);
                if name.is_empty() {
                    return Err("Type a name for the profile.".into());
                }
                if taken(names.iter().cloned(), row, &name) {
                    return Err(format!("There is already a profile called {name}."));
                }
                let mut p = old.cloned().unwrap_or_default();
                p.name = name;
                Ok(p)
            })?;
            if km.profiles.is_empty() {
                return Err("Keyboard Manager needs at least one profile.".into());
            }
            if renamed.as_ref() == Some(&km.active_profile) {
                if let Some(r) = row {
                    km.active_profile = km.profiles[r].name.clone();
                }
            }
            if deleted.as_ref() == Some(&km.active_profile) {
                km.active_profile = km.profiles[0].name.clone();
            }
            Ok(selected)
        }
        List::Presets => {
            let names: Vec<String> = c.power_rename.presets.iter().map(|p| p.name.clone()).collect();
            let row = row_of(&event);
            edit_vec(&mut c.power_rename.presets, event, |f, old| {
                let (name, search, replace) = (text(f, 0), text(f, 1), f.get(2).cloned().unwrap_or_default());
                if name.is_empty() {
                    return Err("Type a name for the preset.".into());
                }
                if taken(names.iter().cloned(), row, &name) {
                    return Err(format!("There is already a preset called {name}."));
                }
                let mut p = old.cloned().unwrap_or(Preset {
                    name: String::new(),
                    options: Default::default(),
                });
                p.name = name;
                let ops = &mut p.options.ops;
                let first_is_replace = matches!(ops.first(), Some(RenameOp::Replace { .. }));
                match (search.is_empty(), first_is_replace) {
                    (true, true) => {
                        ops.remove(0);
                    }
                    (true, false) => {}
                    (false, true) => {
                        if let Some(RenameOp::Replace { find, with, .. }) = ops.first_mut() {
                            *find = search;
                            *with = replace;
                        }
                    }
                    (false, false) => ops.insert(
                        0,
                        RenameOp::Replace {
                            find: search,
                            with: replace,
                            regex: false,
                            case_insensitive: true,
                            first_only: false,
                        },
                    ),
                }
                if ops.is_empty() {
                    return Err("Type what the preset searches for.".into());
                }
                Ok(p)
            })
        }
        List::Commands => {
            let keywords: Vec<String> = c.launcher.custom_commands.iter().map(|k| k.keyword.clone()).collect();
            let row = row_of(&event);
            edit_vec(&mut c.launcher.custom_commands, event, |f, _| {
                let keyword = text(f, 0);
                if keyword.is_empty() || keyword.contains(char::is_whitespace) || keyword.len() > 24 {
                    return Err("The keyword is the one word (up to 24 letters) typed to run the command.".into());
                }
                if taken(keywords.iter().cloned(), row, &keyword) {
                    return Err(format!("The keyword {keyword} is already used."));
                }
                let program = text(f, 2);
                if program.is_empty() {
                    return Err("Type the program, document or web address to open.".into());
                }
                Ok(CustomCommand {
                    keyword,
                    title: text(f, 1),
                    program,
                    args: text(f, 3),
                })
            })
        }
        List::Sizes => edit_vec(&mut c.image_resizer.sizes, event, |f, _| crate::resizer::parse_size(f)),
    }
}

/// The configuration with `edited` saved, or why it was not.
fn save(host: &mut Host, edited: &Config) -> Result<(), String> {
    let value = serde_json::to_value(edited).map_err(|e| e.to_string())?;
    // The checks the configuration makes on load decide what can be saved; anything
    // they would drop is refused here instead of being lost quietly.
    let (_, warnings) = Config::from_value(value.clone());
    if !warnings.is_empty() {
        return Err(warnings.join(" "));
    }
    host.handle_request("config.set", &json!({ "config": value }))
        .map(|_| ())
        .map_err(|e| format!("Not saved: {e}"))
}

#[derive(Default)]
pub struct Lists {
    windows: Vec<(List, TableWindow)>,
    /// Keys (0) or shortcuts (1) in the remaps window.
    remap_view: usize,
}

impl Lists {
    pub fn owns(table: &str) -> bool {
        List::from_table(table).is_some()
    }

    /// Whether a settings card with this key opens one of these windows.
    pub fn edits(key: &str) -> bool {
        for_setting(key).is_some()
    }

    /// Open the window that edits the setting `key`.
    pub fn open_for(&mut self, key: &str, host: &Host) -> Result<(), String> {
        let list = for_setting(key).ok_or_else(|| format!("{key} has no list editor"))?;
        self.open(list, host)
    }

    fn window(&mut self, list: List) -> Option<&mut TableWindow> {
        self.windows.iter_mut().find(|(l, _)| *l == list).map(|(_, w)| w)
    }

    fn open(&mut self, list: List, host: &Host) -> Result<(), String> {
        if self.window(list).is_none() {
            let mut w = TableWindow::new(list.spec()).map_err(|e| format!("could not open the list editor: {e}"))?;
            let module = match list {
                List::Remaps | List::Profiles => "keyboard_manager",
                List::Presets => "power_rename",
                List::Commands => "launcher",
                List::Sizes => "image_resizer",
            };
            crate::icons::apply(module, |ico| w.set_icon(ico));
            if list == List::Remaps {
                w.set_choice(self.remap_view);
            }
            self.windows.push((list, w));
        }
        self.show_rows(list, host, None);
        if let Some(w) = self.window(list) {
            w.set_status("");
            w.show();
        }
        Ok(())
    }

    fn show_rows(&mut self, list: List, host: &Host, selected: Option<usize>) {
        let view = self.remap_view;
        let rows = rows(list, host.config(), view);
        let title = (list == List::Remaps).then(|| {
            format!(
                "Keyboard Manager: profile {}",
                host.config().keyboard_manager.active_profile
            )
        });
        if let Some(w) = self.window(list) {
            w.set_rows(&rows, selected);
            if let Some(t) = title {
                w.set_title(&t);
            }
        }
    }

    /// The configuration changed elsewhere: show it in the windows that are open.
    pub fn refresh(&mut self, host: &Host) {
        let open: Vec<List> = self
            .windows
            .iter()
            .filter(|(_, w)| w.is_visible())
            .map(|(l, _)| *l)
            .collect();
        for list in open {
            self.show_rows(list, host, None);
        }
    }

    /// Handle what the user did in one of these windows; true when the configuration
    /// was saved.
    pub fn on_event(&mut self, table: &str, event: TableEvent, host: &mut Host) -> bool {
        let Some(list) = List::from_table(table) else {
            return false;
        };
        match (list, &event) {
            (List::Remaps, TableEvent::Choice(i)) => {
                self.remap_view = *i;
                self.show_rows(list, host, None);
                return false;
            }
            (List::Remaps, TableEvent::Save) => {
                if let Err(e) = self.open(List::Profiles, host) {
                    tracing::warn!("{e}");
                }
                return false;
            }
            (_, TableEvent::Add(_) | TableEvent::Update { .. } | TableEvent::Delete(_)) => {}
            _ => return false,
        }
        let mut config = host.config().clone();
        let outcome = edit(list, &mut config, self.remap_view, event).and_then(|sel| save(host, &config).map(|_| sel));
        match outcome {
            Ok(selected) => {
                self.show_rows(list, host, selected);
                // Profiles and remaps show each other's changes.
                let other = match list {
                    List::Remaps => Some(List::Profiles),
                    List::Profiles => Some(List::Remaps),
                    _ => None,
                };
                if let Some(o) = other.filter(|o| self.window(*o).is_some()) {
                    self.show_rows(o, host, None);
                }
                if let Some(w) = self.window(list) {
                    w.set_status("Saved.");
                }
                true
            }
            Err(e) => {
                if let Some(w) = self.window(list) {
                    w.set_status(&e);
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(f: &[&str]) -> Vec<String> {
        f.iter().map(|s| s.to_string()).collect()
    }

    fn saves(c: &Config) -> Vec<String> {
        Config::from_value(serde_json::to_value(c).unwrap()).1
    }

    #[test]
    fn remaps_take_keys_shortcuts_and_disabling() {
        let mut c = Config::new();
        edit(
            List::Remaps,
            &mut c,
            0,
            TableEvent::Add(fields(&["CapsLock", "Esc", ""])),
        )
        .unwrap();
        edit(List::Remaps, &mut c, 0, TableEvent::Add(fields(&["F1", "", ""]))).unwrap();
        edit(
            List::Remaps,
            &mut c,
            1,
            TableEvent::Add(fields(&["Ctrl+Q", "Alt+F4", "Code.exe"])),
        )
        .unwrap();
        let p = c.keyboard_manager.active().unwrap();
        assert_eq!(p.key_remaps[0].to, RemapTarget::Key("Esc".into()));
        assert_eq!(p.key_remaps[1].to, RemapTarget::Disabled);
        assert_eq!(p.shortcut_remaps[0].to, RemapTarget::Shortcut("Alt+F4".into()));
        assert!(saves(&c).is_empty());
        let err = edit(List::Remaps, &mut c, 0, TableEvent::Add(fields(&["Ctrl+K", "A", ""]))).unwrap_err();
        assert!(err.contains("Shortcuts"), "{err}");
        // A second remap of the same key is something saving would drop.
        edit(List::Remaps, &mut c, 0, TableEvent::Add(fields(&["CapsLock", "A", ""]))).unwrap();
        assert!(!saves(&c).is_empty());
        let rows = rows(List::Remaps, &c, 0);
        assert_eq!(rows[1].1, ["F1", "Disabled", ""]);
    }

    #[test]
    fn renaming_or_deleting_the_profile_in_use_follows_it() {
        let mut c = Config::new();
        edit(List::Profiles, &mut c, 0, TableEvent::Add(fields(&["Games"]))).unwrap();
        edit(
            List::Profiles,
            &mut c,
            0,
            TableEvent::Update {
                row: 0,
                fields: fields(&["Work"]),
            },
        )
        .unwrap();
        assert_eq!(c.keyboard_manager.active_profile, "Work");
        assert!(edit(List::Profiles, &mut c, 0, TableEvent::Add(fields(&["games"]))).is_err());
        edit(List::Profiles, &mut c, 0, TableEvent::Delete(0)).unwrap();
        assert_eq!(c.keyboard_manager.active_profile, "Games");
        assert!(edit(List::Profiles, &mut c, 0, TableEvent::Delete(0)).is_err());
    }

    #[test]
    fn presets_keep_their_other_steps() {
        let mut c = Config::new();
        c.power_rename.presets.push(Preset {
            name: "Photos".into(),
            options: powerkit_config::RenameOptions {
                ops: vec![RenameOp::Trim { chars: String::new() }],
                ..Default::default()
            },
        });
        let update = |f: &[&str]| TableEvent::Update {
            row: 0,
            fields: fields(f),
        };
        edit(List::Presets, &mut c, 0, update(&["Photos", "IMG_", "Photo "])).unwrap();
        assert_eq!(rows(List::Presets, &c, 0)[0].1, ["Photos", "IMG_", "Photo ", "Trim"]);
        edit(List::Presets, &mut c, 0, update(&["Photos", "", ""])).unwrap();
        assert_eq!(c.power_rename.presets[0].options.ops.len(), 1);
        assert!(edit(List::Presets, &mut c, 0, TableEvent::Add(fields(&["New", "", ""]))).is_err());
    }

    #[test]
    fn commands_need_a_keyword_and_a_program() {
        let mut c = Config::new();
        let add = |f: &[&str]| TableEvent::Add(fields(f));
        edit(
            List::Commands,
            &mut c,
            0,
            add(&["gh", "GitHub", "https://github.com/search", "q={query}"]),
        )
        .unwrap();
        assert!(edit(List::Commands, &mut c, 0, add(&["two words", "", "x", ""])).is_err());
        assert!(edit(List::Commands, &mut c, 0, add(&["GH", "", "x", ""])).is_err());
        assert!(edit(List::Commands, &mut c, 0, add(&["x", "", "", ""])).is_err());
        assert!(saves(&c).is_empty());
    }
}
