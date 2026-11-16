//! The settings window's contents: a dashboard, the general settings and one page
//! per utility, built from the configuration and saved through `config.set` like
//! any IPC client would.

use crate::Host;
use powerkit_config::Config;
use powerkit_core::{SettingValue, SettingsEvent};
use powerkit_platform::guide_view::Scene;
use powerkit_platform::settings_window::{Control, Group, Item, NavEntry, Page, SettingsWindow, Severity};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

/// How a setting is edited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Toggle,
    Shortcut,
    /// `(value, label)` pairs.
    Choice(&'static [(&'static str, &'static str)]),
    Number,
    Color,
    /// A one-line text box with a placeholder.
    Text(&'static str),
    /// A list of strings, one per line.
    Lines(&'static str),
    /// A value with structure of its own, edited as JSON.
    Json,
}

/// One setting of a utility: `key` is its path under the utility's section, with
/// dots between object keys.
pub struct Spec {
    key: &'static str,
    title: &'static str,
    description: &'static str,
    kind: Kind,
    /// Settings shown when the card is expanded; the card's own control stays in
    /// its header.
    children: Vec<Spec>,
}

fn s(key: &'static str, title: &'static str, description: &'static str, kind: Kind) -> Spec {
    Spec {
        key,
        title,
        description,
        kind,
        children: Vec::new(),
    }
}

impl Spec {
    fn with(mut self, children: Vec<Spec>) -> Spec {
        self.children = children;
        self
    }
}

const THEMES: &[(&str, &str)] = &[("system", "Use system setting"), ("light", "Light"), ("dark", "Dark")];
const APPS: &str = "Program names such as notepad.exe, one per line";

use Kind::*;

/// The general settings, under the "general" section.
fn general_spec() -> Vec<(&'static str, Vec<Spec>)> {
    vec![
        (
            "Appearance & behavior",
            vec![
                s(
                    "theme",
                    "App theme",
                    "Light, dark, or the same as Windows",
                    Choice(THEMES),
                ),
                s(
                    "launch_at_startup",
                    "Run at startup",
                    "Start PowerKit when you sign in to Windows",
                    Toggle,
                ),
                s(
                    "start_minimized",
                    "Start in the background",
                    "Open only the tray icon when PowerKit starts",
                    Toggle,
                ),
                s(
                    "minimize_to_tray",
                    "Keep running in the tray",
                    "Closing windows leaves PowerKit running",
                    Toggle,
                ),
            ],
        ),
        (
            "Explorer",
            vec![s(
                "explorer_menu",
                "Show in the Windows 11 context menu",
                "Put Rename, Resize, New+, Unlock and .reg Preview at the top of the right-click menu. Windows asks for administrator approval the first time",
                Toggle,
            )],
        ),
        (
            "Updates",
            vec![s(
                "check_for_updates",
                "Check for updates automatically",
                "Look for a new version once a day and offer to install it",
                Toggle,
            )],
        ),
        (
            "Troubleshooting",
            vec![s(
                "log_level",
                "Log detail",
                "How much PowerKit writes to its log file",
                Choice(&[
                    ("error", "Errors only"),
                    ("warn", "Warnings"),
                    ("info", "Information"),
                    ("debug", "Debug"),
                ]),
            )],
        ),
    ]
}

/// The settings of the utility `id`, in groups; its `enabled` flag is added on top
/// of every page.
pub fn module_spec(id: &str) -> Vec<(&'static str, Vec<Spec>)> {
    match id {
        "always_on_top" => vec![
            (
                "Activation",
                vec![s(
                    "shortcut",
                    "Activation shortcut",
                    "Pin or unpin the active window",
                    Shortcut,
                )],
            ),
            (
                "Appearance",
                vec![
                    s("show_border", "Show a border around the pinned window", "", Toggle).with(vec![
                        s("border_color", "Color", "", Color),
                        s("border_thickness", "Thickness (px)", "1 to 20", Number),
                    ]),
                    s("opacity_percent", "Opacity of pinned windows (%)", "20 to 100", Number),
                ],
            ),
            (
                "Apps",
                vec![
                    s(
                        "auto_pin_apps",
                        "Pin automatically",
                        "These apps are pinned when they open",
                        Lines(APPS),
                    ),
                    s(
                        "excluded_apps",
                        "Excluded apps",
                        "These apps are never pinned",
                        Lines(APPS),
                    ),
                ],
            ),
        ],
        "color_picker" => vec![
            (
                "Activation",
                vec![s("shortcut", "Activation shortcut", "Open the color picker", Shortcut)],
            ),
            (
                "Picker behavior",
                vec![
                    s(
                        "copy_format",
                        "Copied color format",
                        "The format put on the clipboard when you pick a color",
                        Choice(&[
                            ("hex", "HEX"),
                            ("hex_alpha", "HEX with alpha"),
                            ("rgb", "RGB"),
                            ("rgba", "RGBA"),
                            ("hsl", "HSL"),
                            ("hsv", "HSV"),
                            ("cmyk", "CMYK"),
                            ("css_rgb", "CSS rgb()"),
                            ("css_hsl", "CSS hsl()"),
                            ("custom", "Custom"),
                        ]),
                    ),
                    s(
                        "custom_format",
                        "Custom format",
                        "%R %G %B for red, green and blue; used when the format is Custom",
                        Text("%R, %G, %B"),
                    ),
                    s("history_limit", "Colors kept in history", "0 to 500", Number),
                ],
            ),
            (
                "Magnifier",
                vec![
                    s(
                        "magnifier_radius",
                        "Radius (pixels around the pointer)",
                        "2 to 15",
                        Number,
                    ),
                    s("magnifier_zoom", "Zoom", "4 to 24", Number),
                    s("show_grid", "Show pixel grid", "", Toggle),
                ],
            ),
        ],
        "fancy_zones" => vec![
            (
                "Layouts",
                vec![
                    s(
                        "default_layout",
                        "Default layout",
                        "The id of the layout used on monitors without one",
                        Text(""),
                    ),
                    s("layouts", "Layouts", "Each layout's zones, as JSON", Json),
                    s(
                        "monitor_layouts",
                        "Layouts per monitor",
                        "Monitor name to layout id, as JSON",
                        Json,
                    ),
                ],
            ),
            (
                "Zone behavior",
                vec![
                    s(
                        "drag_modifier",
                        "Hold while dragging to show zones",
                        "",
                        Choice(&[
                            ("shift", "Shift"),
                            ("ctrl", "Ctrl"),
                            ("alt", "Alt"),
                            ("none", "Always show zones"),
                        ]),
                    ),
                    s(
                        "restore_size_on_unsnap",
                        "Restore the original size when a window leaves a zone",
                        "",
                        Toggle,
                    ),
                    s(
                        "move_across_monitors",
                        "Move windows between zones across all monitors",
                        "",
                        Toggle,
                    ),
                    s(
                        "zone_number_modifiers",
                        "Snap to zone by number",
                        "Hold these keys and press a zone's number",
                        Text("Win+Ctrl+Alt"),
                    ),
                ],
            ),
            (
                "Shortcuts",
                vec![
                    s("next_zone_shortcut", "Move to the next zone", "", Shortcut),
                    s("prev_zone_shortcut", "Move to the previous zone", "", Shortcut),
                    s("cycle_layout_shortcut", "Switch layout", "", Shortcut),
                    s("toggle_overlay_shortcut", "Show zones", "", Shortcut),
                ],
            ),
            (
                "Appearance",
                vec![
                    s("zone_color", "Zone color", "", Color),
                    s("highlight_color", "Highlighted zone color", "", Color),
                    s("overlay_opacity", "Zone opacity (%)", "10 to 90", Number),
                ],
            ),
            (
                "Apps",
                vec![s(
                    "excluded_apps",
                    "Excluded apps",
                    "These apps are never snapped to zones",
                    Lines(APPS),
                )],
            ),
        ],
        "power_rename" => vec![(
            "Behavior",
            vec![
                s(
                    "context_menu",
                    "Show in the Explorer menu",
                    "Rename selected files from their right-click menu",
                    Toggle,
                ),
                s("undo_limit", "Renames kept for undo", "0 to 200", Number),
                s("presets", "Saved presets", "Search and replace presets, as JSON", Json),
            ],
        )],
        "keyboard_manager" => vec![(
            "Remapping",
            vec![
                s(
                    "active_profile",
                    "Active profile",
                    "The name of the profile in use",
                    Text("Default"),
                ),
                s(
                    "profiles",
                    "Profiles",
                    "Key and shortcut remappings of each profile, as JSON",
                    Json,
                ),
            ],
        )],
        "launcher" => vec![
            (
                "Activation",
                vec![s("shortcut", "Activation shortcut", "Open PowerKit Run", Shortcut)],
            ),
            (
                "Results",
                vec![
                    s("max_results", "Number of results shown", "3 to 20", Number),
                    s("remember_history", "Rank results you choose often higher", "", Toggle),
                ],
            ),
            (
                "Plugins",
                vec![
                    s(
                        "providers.apps",
                        "Programs",
                        "Installed apps and Start menu shortcuts",
                        Toggle,
                    ),
                    s("providers.files", "Files", "Files in the folders below", Toggle).with(vec![
                        s(
                            "file_roots",
                            "Folders to search",
                            "One folder per line; empty for your user folders",
                            Lines("C:\\Users\\me\\Documents"),
                        ),
                        s("max_indexed_files", "Most files indexed", "", Number),
                    ]),
                    s(
                        "providers.calculator",
                        "Calculator",
                        "Type a sum such as 2*(3+4)",
                        Toggle,
                    ),
                    s(
                        "providers.units",
                        "Unit converter",
                        "Type a conversion such as 10 km to mi",
                        Toggle,
                    ),
                    s(
                        "providers.settings",
                        "Windows settings",
                        "Open Windows settings pages by name",
                        Toggle,
                    ),
                    s(
                        "providers.clipboard",
                        "Clipboard history",
                        "Find text you copied earlier",
                        Toggle,
                    )
                    .with(vec![s("clipboard_history_size", "Entries kept", "", Number)]),
                    s(
                        "providers.processes",
                        "Running processes",
                        "Find and end running programs",
                        Toggle,
                    ),
                    s("providers.commands", "Custom commands", "Commands of your own", Toggle).with(vec![s(
                        "custom_commands",
                        "Commands",
                        "Name, command line and arguments of each, as JSON",
                        Json,
                    )]),
                ],
            ),
        ],
        "awake" => vec![(
            "Behavior",
            vec![
                s(
                    "mode",
                    "Mode",
                    "Keep the computer awake, and for how long",
                    Choice(&[
                        ("off", "Off (follow the power plan)"),
                        ("indefinite", "Keep awake indefinitely"),
                        ("timed", "Keep awake for a time"),
                        ("until", "Keep awake until a set time"),
                    ]),
                ),
                s(
                    "duration_minutes",
                    "Time to keep awake (minutes)",
                    "Used in the timed mode; 1 to 10080",
                    Number,
                ),
                s(
                    "expire_at",
                    "Keep awake until",
                    "Used in the until mode; seconds since 1970 (UTC), as set from the tray",
                    Number,
                ),
                s("keep_display_on", "Keep the screen on", "", Toggle),
            ],
        )],
        "light_switch" => vec![
            (
                "Activation",
                vec![s("shortcut", "Switch now", "Toggle between light and dark", Shortcut)],
            ),
            (
                "Schedule",
                vec![
                    s(
                        "mode",
                        "Mode",
                        "",
                        Choice(&[
                            ("manual", "Manual (shortcut only)"),
                            ("fixed_hours", "Fixed hours"),
                            ("sunset_to_sunrise", "Sunset to sunrise"),
                        ]),
                    ),
                    s(
                        "light_time",
                        "Light from",
                        "In the fixed hours mode; HH:MM",
                        Text("07:00"),
                    ),
                    s(
                        "dark_time",
                        "Dark from",
                        "In the fixed hours mode; HH:MM",
                        Text("20:00"),
                    ),
                    s(
                        "latitude",
                        "Latitude",
                        "For sunset to sunrise; north is positive",
                        Number,
                    ),
                    s(
                        "longitude",
                        "Longitude",
                        "For sunset to sunrise; east is positive",
                        Number,
                    ),
                    s("sunrise_offset_minutes", "Sunrise offset (minutes)", "", Number),
                    s("sunset_offset_minutes", "Sunset offset (minutes)", "", Number),
                ],
            ),
            (
                "What changes",
                vec![
                    s("change_apps", "Apps", "", Toggle),
                    s("change_system", "Windows (taskbar and Start)", "", Toggle),
                ],
            ),
        ],
        "quick_accent" => vec![
            (
                "Activation",
                vec![
                    s(
                        "trigger",
                        "Activation key",
                        "Hold a letter and press this to pick an accent",
                        Choice(&[
                            ("arrows", "Left or right arrow"),
                            ("space", "Space"),
                            ("both", "Arrows or space"),
                        ]),
                    ),
                    s(
                        "input_delay_ms",
                        "Delay before the toolbar appears (ms)",
                        "0 to 1000",
                        Number,
                    ),
                ],
            ),
            (
                "Characters",
                vec![s(
                    "languages",
                    "Languages",
                    "Language codes such as fr or de, one per line; all for every language",
                    Lines("all"),
                )],
            ),
            ("Apps", vec![s("excluded_apps", "Excluded apps", "", Lines(APPS))]),
        ],
        "mouse_utilities" => vec![
            (
                "Find My Mouse",
                vec![s(
                    "find_my_mouse.enabled",
                    "Enable Find My Mouse",
                    "Spotlight the pointer",
                    Toggle,
                )
                .with(vec![
                    s(
                        "find_my_mouse.activation",
                        "Activation",
                        "",
                        Choice(&[
                            ("double_ctrl", "Press Left Ctrl twice"),
                            ("shake", "Shake the mouse"),
                            ("shortcut_only", "Shortcut only"),
                        ]),
                    ),
                    s("find_my_mouse.shortcut", "Shortcut", "", Shortcut),
                    s("find_my_mouse.background_color", "Background color", "", Color),
                    s(
                        "find_my_mouse.opacity_percent",
                        "Background opacity (%)",
                        "10 to 100",
                        Number,
                    ),
                    s("find_my_mouse.radius", "Spotlight radius (px)", "20 to 500", Number),
                    s("find_my_mouse.initial_zoom", "Initial zoom", "1 to 15", Number),
                    s(
                        "find_my_mouse.animation_ms",
                        "Animation duration (ms)",
                        "0 to 5000",
                        Number,
                    ),
                    s(
                        "find_my_mouse.skip_in_game_mode",
                        "Do not activate in Game Mode",
                        "",
                        Toggle,
                    ),
                    s("find_my_mouse.excluded_apps", "Excluded apps", "", Lines(APPS)),
                ])],
            ),
            (
                "Mouse Highlighter",
                vec![
                    s("highlighter.enabled", "Enable Mouse Highlighter", "Show clicks", Toggle).with(vec![
                        s("highlighter.shortcut", "Shortcut", "", Shortcut),
                        s("highlighter.left_color", "Left button color", "", Color),
                        s("highlighter.right_color", "Right button color", "", Color),
                        s("highlighter.opacity_percent", "Opacity (%)", "10 to 100", Number),
                        s("highlighter.radius", "Radius (px)", "5 to 100", Number),
                        s("highlighter.fade_delay_ms", "Fade delay (ms)", "0 to 5000", Number),
                        s("highlighter.fade_ms", "Fade duration (ms)", "0 to 5000", Number),
                        s(
                            "highlighter.always_highlight",
                            "Always highlight the pointer",
                            "",
                            Toggle,
                        ),
                        s("highlighter.always_color", "Pointer highlight color", "", Color),
                        s(
                            "highlighter.spotlight_mode",
                            "Spotlight mode",
                            "Dim the screen around the pointer",
                            Toggle,
                        ),
                    ]),
                ],
            ),
            (
                "Mouse Pointer Crosshairs",
                vec![s(
                    "crosshairs.enabled",
                    "Enable Mouse Pointer Crosshairs",
                    "Lines through the pointer",
                    Toggle,
                )
                .with(vec![
                    s("crosshairs.shortcut", "Shortcut", "", Shortcut),
                    s("crosshairs.color", "Color", "", Color),
                    s("crosshairs.opacity_percent", "Opacity (%)", "10 to 100", Number),
                    s("crosshairs.thickness", "Thickness (px)", "1 to 50", Number),
                    s(
                        "crosshairs.center_gap",
                        "Gap around the pointer (px)",
                        "0 to 500",
                        Number,
                    ),
                    s("crosshairs.length", "Length (px)", "0 for the whole screen", Number),
                    s("crosshairs.border_color", "Border color", "", Color),
                    s("crosshairs.border_size", "Border size (px)", "0 to 50", Number),
                    s(
                        "crosshairs.orientation",
                        "Orientation",
                        "",
                        Choice(&[("both", "Both"), ("horizontal", "Horizontal"), ("vertical", "Vertical")]),
                    ),
                    s(
                        "crosshairs.hide_with_pointer",
                        "Hide when the pointer is hidden",
                        "",
                        Toggle,
                    ),
                    s("crosshairs.gliding_shortcut", "Gliding cursor shortcut", "", Shortcut),
                    s("crosshairs.gliding_fast_speed", "Gliding fast speed", "1 to 60", Number),
                    s("crosshairs.gliding_slow_speed", "Gliding slow speed", "1 to 20", Number),
                ])],
            ),
            (
                "Mouse Jump",
                vec![
                    s("jump.enabled", "Enable Mouse Jump", "Jump across large screens", Toggle).with(vec![
                        s("jump.shortcut", "Shortcut", "", Shortcut),
                        s("jump.max_width", "Preview width (px)", "160 to 4000", Number),
                        s("jump.max_height", "Preview height (px)", "160 to 4000", Number),
                    ]),
                ],
            ),
        ],
        "crop_and_lock" => vec![(
            "Activation",
            vec![
                s(
                    "thumbnail_shortcut",
                    "Thumbnail shortcut",
                    "A live view of part of a window",
                    Shortcut,
                ),
                s(
                    "reparent_shortcut",
                    "Reparent shortcut",
                    "Keep only part of a window",
                    Shortcut,
                ),
            ],
        )],
        "shortcut_guide" => vec![
            (
                "Activation",
                vec![
                    s("shortcut", "Activation shortcut", "", Shortcut),
                    s("hold_win", "Hold the Windows key to show the guide", "", Toggle),
                    s("press_time_ms", "Hold time (ms)", "100 to 10000", Number),
                ],
            ),
            (
                "Appearance",
                vec![
                    s("theme", "Theme", "", Choice(THEMES)),
                    s("opacity_percent", "Background opacity (%)", "0 to 100", Number),
                    s("show_taskbar_numbers", "Show taskbar icon shortcuts", "", Toggle),
                ],
            ),
            ("Apps", vec![s("excluded_apps", "Excluded apps", "", Lines(APPS))]),
        ],
        "grab_and_move" => vec![
            (
                "Behavior",
                vec![
                    s(
                        "modifier",
                        "Hold to grab",
                        "Drag a window from anywhere while holding this key",
                        Choice(&[("win", "Windows key"), ("alt", "Alt")]),
                    ),
                    s("resize", "Resize with the right mouse button", "", Toggle),
                    s("activate", "Bring the window to the front", "", Toggle),
                ],
            ),
            ("Apps", vec![s("excluded_apps", "Excluded apps", "", Lines(APPS))]),
        ],
        "window_hopper" => vec![
            (
                "Activation",
                vec![
                    s("next_shortcut", "Next window of the app", "", Shortcut),
                    s("previous_shortcut", "Previous window of the app", "", Shortcut),
                ],
            ),
            (
                "Behavior",
                vec![
                    s("include_minimized", "Include minimized windows", "", Toggle),
                    s("excluded_apps", "Excluded apps", "", Lines(APPS)),
                ],
            ),
        ],
        "hosts_editor" | "environment_variables" | "registry_preview" => vec![(
            "Activation",
            vec![s("shortcut", "Activation shortcut", "Open the editor", Shortcut)],
        )],
        "new_plus" => vec![(
            "Templates",
            vec![
                s(
                    "templates",
                    "Templates folder",
                    "Files and folders in it are offered in the New menu",
                    Text("Default folder"),
                ),
                s(
                    "hide_starting_digits",
                    "Hide leading digits",
                    "Templates can be ordered as 01 Name",
                    Toggle,
                ),
                s("hide_extension", "Hide file extensions", "", Toggle),
                s(
                    "replace_variables",
                    "Replace variables in names",
                    "$YYYY, $MM, $DD and $PARENT_FOLDER_NAME in template names",
                    Toggle,
                ),
            ],
        )],
        "file_locksmith" => vec![(
            "Shell integration",
            vec![s(
                "extended_menu_only",
                "Extended menu only",
                "Show \"Unlock with PowerKit\" only when Shift is held while right-clicking",
                Toggle,
            )],
        )],
        "image_resizer" => vec![
            (
                "Sizes",
                vec![
                    s(
                        "sizes",
                        "Sizes",
                        "Offered in the Image Resizer window, as JSON; a width or height of 0 follows the other",
                        Json,
                    ),
                    s(
                        "shrink_only",
                        "Never enlarge pictures",
                        "Leave pictures that are already smaller than the size as they are",
                        Toggle,
                    ),
                    s(
                        "ignore_orientation",
                        "Ignore the orientation of pictures",
                        "Swap a size's width and height for pictures turned the other way",
                        Toggle,
                    ),
                ],
            ),
            (
                "Saving",
                vec![
                    s(
                        "replace_originals",
                        "Replace the originals",
                        "Overwrite the pictures instead of saving resized copies",
                        Toggle,
                    ),
                    s(
                        "file_name",
                        "File name",
                        "%1 original name, %2 size name, %3 and %4 size width and height, %5 and %6 new width and height",
                        Text("%1 (%2)"),
                    ),
                    s(
                        "fallback_format",
                        "Fallback format",
                        "Used for pictures Windows can read but not write, such as HEIC and WebP",
                        Choice(&[
                            ("jpeg", "JPEG"),
                            ("png", "PNG"),
                            ("bmp", "BMP"),
                            ("gif", "GIF"),
                            ("tiff", "TIFF"),
                        ]),
                    ),
                    s("jpeg_quality", "JPEG quality", "1 to 100", Number),
                    s("remove_metadata", "Remove metadata", "Drop camera details and other metadata", Toggle),
                    s(
                        "keep_date_modified",
                        "Keep the date modified",
                        "Give resized pictures the date of the originals",
                        Toggle,
                    ),
                ],
            ),
        ],
        "text_extractor" => vec![(
            "Activation",
            vec![
                s("shortcut", "Activation shortcut", "Then drag a box around the text to copy", Shortcut),
                s(
                    "language",
                    "Language",
                    "A language tag such as en-US or de-DE; empty uses the Windows display languages",
                    Text(""),
                ),
                s("single_line", "Copy as one line", "Join the lines with spaces", Toggle),
            ],
        )],
        "power_display" => vec![
            (
                "Activation",
                vec![s("shortcut", "Open Power Display", "Shows a slider for each monitor", Shortcut)],
            ),
            (
                "Controls",
                vec![
                    s("contrast", "Contrast", "", Toggle),
                    s("volume", "Volume", "For monitors with speakers", Toggle),
                    s(
                        "input_source",
                        "Input source",
                        "Switching away blanks this PC's picture; asks first",
                        Toggle,
                    ),
                    s("color_preset", "Color preset", "Asks first", Toggle),
                    s(
                        "power_state",
                        "Power",
                        "Turning a monitor off may need its own button to undo; asks first",
                        Toggle,
                    ),
                ],
            ),
        ],
        "screen_ruler" => vec![
            (
                "Activation",
                vec![s(
                    "shortcut",
                    "Activation shortcut",
                    "The wheel or 1-4 switches what is measured; a click copies it; Escape stops",
                    Shortcut,
                )],
            ),
            (
                "Measuring",
                vec![
                    s(
                        "tolerance",
                        "Edge tolerance",
                        "0 to 255; how much a color may change before it counts as an edge",
                        Number,
                    ),
                    s("draw_feet", "Draw feet", "Short bars across the ends of each line", Toggle),
                    s("line_color", "Line color", "", Color),
                    s(
                        "extra_unit",
                        "Also show",
                        "Worked out from the Windows scaling, not the panel's real size",
                        Choice(&[
                            ("none", "Pixels only"),
                            ("inches", "Inches"),
                            ("centimeters", "Centimeters"),
                            ("millimeters", "Millimeters"),
                        ]),
                    ),
                ],
            ),
        ],
        "workspaces" => vec![
            (
                "Activation",
                vec![s("shortcut", "Activation shortcut", "Open the editor", Shortcut)],
            ),
            (
                "Behavior",
                vec![s(
                    "move_existing",
                    "Move windows that are already open",
                    "Instead of starting their programs again",
                    Toggle,
                )],
            ),
        ],
        "peek" => vec![(
            "Activation",
            vec![s(
                "shortcut",
                "Activation shortcut",
                "Taken only while File Explorer or the desktop is in front",
                Shortcut,
            )],
        )],
        "advanced_paste" => vec![
            (
                "Shortcuts",
                vec![
                    s(
                        "shortcut",
                        "Show the ways to paste",
                        "A menu at the cursor of what the clipboard can be pasted as",
                        Shortcut,
                    ),
                    s("plain_shortcut", "Paste as plain text", "Without the formatting", Shortcut),
                    s("markdown_shortcut", "Paste as Markdown", "From formatted text (HTML)", Shortcut),
                    s(
                        "json_shortcut",
                        "Paste as JSON",
                        "From JSON, or a comma, semicolon or tab separated table",
                        Shortcut,
                    ),
                ],
            ),
            (
                "Text in pictures",
                vec![s(
                    "ocr_language",
                    "Language",
                    "A language tag such as en-US or de-DE; empty uses the Windows display languages",
                    Text(""),
                )],
            ),
        ],
        "power_plan" => vec![
            (
                "Shortcuts",
                vec![
                    s("shortcut", "Show the power plans", "A menu at the cursor; also in the tray menu", Shortcut),
                    s("next_shortcut", "Next power plan", "Switches without a menu", Shortcut),
                ],
            ),
            (
                "Behavior",
                vec![s("notify", "Say which plan is on after switching", "", Toggle)],
            ),
        ],
        "zoom_it" => vec![
            (
                "Shortcuts",
                vec![
                    s(
                        "zoom_shortcut",
                        "Zoom",
                        "The wheel zooms; a click draws on what is shown; Escape stops",
                        Shortcut,
                    ),
                    s("draw_shortcut", "Draw", "Draw on the screen as it is", Shortcut),
                    s("break_shortcut", "Break timer", "A countdown across the screen", Shortcut),
                    s(
                        "live_zoom_shortcut",
                        "Live zoom",
                        "Zoom while the screen keeps changing underneath",
                        Shortcut,
                    ),
                ],
            ),
            (
                "Zoom",
                vec![s("zoom_percent", "First zoom", "125 to 800 percent", Number)],
            ),
            (
                "Draw",
                vec![
                    s("pen_color", "Pen color", "R, G, B, Y, O and P change it while drawing", Color),
                    s(
                        "pen_width",
                        "Pen width",
                        "1 to 40 pixels; the wheel changes it while drawing",
                        Number,
                    ),
                ],
            ),
            (
                "Break",
                vec![s("break_minutes", "Break length", "1 to 99 minutes", Number)],
            ),
        ],
        _ => Vec::new(),
    }
}

/// Where each utility goes in the navigation pane, after Dashboard and General.
const CATEGORIES: &[(&str, char, &[&str])] = &[
    (
        "System Tools",
        '\u{E770}',
        &[
            "awake",
            "color_picker",
            "light_switch",
            "launcher",
            "power_display",
            "screen_ruler",
            "shortcut_guide",
            "text_extractor",
            "zoom_it",
            "peek",
            "power_plan",
        ],
    ),
    (
        "Windowing & Layouts",
        '\u{E737}',
        &[
            "always_on_top",
            "crop_and_lock",
            "fancy_zones",
            "grab_and_move",
            "window_hopper",
            "workspaces",
        ],
    ),
    (
        "Input / Output",
        '\u{E765}',
        &["keyboard_manager", "mouse_utilities", "quick_accent", "advanced_paste"],
    ),
    (
        "File Management",
        '\u{E8B7}',
        &["file_locksmith", "image_resizer", "new_plus", "power_rename"],
    ),
    (
        "Advanced",
        '\u{E9F5}',
        &["environment_variables", "hosts_editor", "registry_preview"],
    ),
];

/// Configuration kept by PowerKit itself or edited in its own window, not shown.
const HIDDEN: &[&str] = &["general.recent_modules", "workspaces.workspaces"];

/// The value at the dotted `key` inside `root`.
fn at<'a>(root: &'a Value, key: &str) -> &'a Value {
    key.split('.').fold(root, |v, k| &v[k])
}

/// The leaves of `value` in order, as dotted paths. Objects are opened up;
/// anything else (including lists of objects such as remappings) is one value.
pub fn leaves(value: &Value, prefix: &str) -> Vec<String> {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(map) if !map.is_empty() => {
                for (k, child) in map {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(child, &p, out);
                }
            }
            _ => out.push(path.to_string()),
        }
    }
    let mut out = Vec::new();
    walk(value, prefix, &mut out);
    out
}

fn is_string_list(v: &Value) -> bool {
    v.as_array().is_some_and(|a| a.iter().all(Value::is_string))
}

/// How an unlisted value is edited, from its type.
fn guess(v: &Value) -> Kind {
    match v {
        Value::Bool(_) => Toggle,
        Value::Number(_) => Number,
        Value::String(_) => Text(""),
        _ if is_string_list(v) => Lines(""),
        _ => Json,
    }
}

/// How a value reads in its text box.
pub fn display(v: &Value, kind: Kind) -> String {
    match (v, kind) {
        (Value::String(s), _) => s.clone(),
        (Value::Array(a), Lines(_)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        (_, Json) => serde_json::to_string_pretty(v).unwrap_or_default(),
        (Value::Null, _) => String::new(),
        _ => v.to_string(),
    }
}

/// The value typed for a setting, read as the same kind of value it had before.
pub fn parse(original: &Value, kind: Kind, text: &str) -> Result<Value, String> {
    let t = text.trim();
    match (original, kind) {
        (_, Lines(_)) => Ok(Value::Array(
            t.lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.into()))
                .collect(),
        )),
        (Value::Bool(_), _) => match t.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(Value::Bool(true)),
            "false" | "off" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!("{t:?} is not on or off")),
        },
        (Value::Number(n), _) if n.is_f64() => t
            .parse::<f64>()
            .ok()
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            .ok_or_else(|| format!("{t:?} is not a number")),
        (Value::Number(_), _) => t
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("{t:?} is not a whole number")),
        (Value::String(_), _) => Ok(Value::String(text.to_string())),
        _ => serde_json::from_str(t).map_err(|e| format!("not valid JSON: {e}")),
    }
}

/// Replace the value at the dotted `key` inside `root`, creating objects on the way.
pub fn set_at(root: &mut Value, key: &str, v: Value) {
    let mut cur = root;
    for k in key.split('.') {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        cur = cur
            .as_object_mut()
            .expect("made an object above")
            .entry(k.to_string())
            .or_insert(Value::Null);
    }
    *cur = v;
}

/// The control editing `v`, a setting whose default is `default`.
fn control(kind: Kind, v: &Value, default: &Value) -> Control {
    match kind {
        Toggle => Control::Toggle(v.as_bool().unwrap_or(false)),
        Shortcut => Control::Shortcut {
            value: v.as_str().unwrap_or_default().to_string(),
            default: default.as_str().unwrap_or_default().to_string(),
        },
        Choice(options) => Control::Choice {
            options: options.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(),
            selected: v.as_str().unwrap_or_default().to_string(),
        },
        Number => Control::Number {
            value: display(v, kind),
        },
        Color => Control::Color(v.as_str().unwrap_or("#000000").to_string()),
        Text(placeholder) => Control::Text {
            value: display(v, kind),
            placeholder: placeholder.into(),
            lines: 1,
            wide: false,
        },
        Lines(placeholder) => Control::Text {
            value: display(v, kind),
            placeholder: placeholder.into(),
            lines: 4,
            wide: true,
        },
        Json => Control::Text {
            value: display(v, kind),
            placeholder: String::new(),
            lines: 8,
            wide: true,
        },
    }
}

/// The card for `spec` in the section `section` of `config`.
fn item(
    section: &str,
    spec: &Spec,
    config: &Value,
    defaults: &Value,
    enabled: bool,
    kinds: &mut HashMap<String, Kind>,
) -> Item {
    let key = format!("{section}.{}", spec.key);
    let v = at(config, &key);
    kinds.insert(key.clone(), spec.kind);
    let control = control(spec.kind, v, at(defaults, &key));
    let mut it = Item::new(key, spec.title, control);
    it.description = spec.description.into();
    it.enabled = enabled;
    // A card whose header is a switch turns its own settings off with it.
    let inner = enabled && (spec.kind != Toggle || v.as_bool().unwrap_or(true));
    it.children = spec
        .children
        .iter()
        .map(|c| item(section, c, config, defaults, inner, kinds))
        .collect();
    it
}

fn covered(specs: &[Spec], section: &str, out: &mut HashSet<String>) {
    for sp in specs {
        out.insert(format!("{section}.{}", sp.key));
        covered(&sp.children, section, out);
    }
}

/// The groups of a page for `section`, plus "Other" for any setting not in `spec`.
fn groups(
    section: &str,
    spec: Vec<(&'static str, Vec<Spec>)>,
    config: &Value,
    defaults: &Value,
    enabled: bool,
    kinds: &mut HashMap<String, Kind>,
) -> Vec<Group> {
    let mut seen: HashSet<String> = HIDDEN.iter().map(|s| s.to_string()).collect();
    seen.insert(format!("{section}.enabled"));
    let mut out = Vec::new();
    for (title, specs) in &spec {
        covered(specs, section, &mut seen);
        out.push(Group {
            title: title.to_string(),
            items: specs
                .iter()
                .map(|sp| item(section, sp, config, defaults, enabled, kinds))
                .collect(),
        });
    }
    let mut other = Vec::new();
    for key in leaves(&config[section], section) {
        if seen.contains(&key) {
            continue;
        }
        let v = at(config, &key);
        let kind = guess(v);
        kinds.insert(key.clone(), kind);
        let title = key[section.len() + 1..].replace(['_', '.'], " ");
        let mut it = Item::new(key.clone(), title, control(kind, v, at(defaults, &key)));
        it.enabled = enabled;
        other.push(it);
    }
    if !other.is_empty() {
        out.push(Group {
            title: "Other".into(),
            items: other,
        });
    }
    out
}

/// A utility from the host's status.
#[derive(Debug, Clone)]
struct Module {
    id: String,
    name: String,
    description: String,
    enabled: bool,
}

/// What a row of the navigation pane opens.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    Dashboard,
    General,
    Module(usize),
    Category,
}

/// The navigation pane: dashboard, general, then each category's utilities.
fn nav(modules: &[Module]) -> (Vec<NavEntry>, Vec<Target>) {
    let mut entries = vec![
        NavEntry::Page {
            title: "Dashboard".into(),
            icon: None,
            glyph: Some('\u{E80F}'),
            depth: 0,
        },
        NavEntry::Page {
            title: "General".into(),
            icon: None,
            glyph: Some('\u{E713}'),
            depth: 0,
        },
    ];
    let mut targets = vec![Target::Dashboard, Target::General];
    let mut placed = HashSet::new();
    let page = |m: &Module, depth| NavEntry::Page {
        title: m.name.clone(),
        icon: crate::icons::module(&m.id),
        glyph: None,
        depth,
    };
    for (title, glyph, ids) in CATEGORIES {
        let mut members: Vec<usize> = ids
            .iter()
            .filter_map(|id| modules.iter().position(|m| m.id == *id))
            .collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by(|&a, &b| modules[a].name.cmp(&modules[b].name));
        entries.push(NavEntry::Category {
            title: title.to_string(),
            glyph: *glyph,
        });
        targets.push(Target::Category);
        for i in members {
            entries.push(page(&modules[i], 1));
            targets.push(Target::Module(i));
            placed.insert(i);
        }
    }
    for (i, m) in modules.iter().enumerate() {
        if !placed.contains(&i) {
            entries.push(page(m, 0));
            targets.push(Target::Module(i));
        }
    }
    (entries, targets)
}

fn dashboard(modules: &[Module]) -> Page {
    let mut items: Vec<Item> = modules
        .iter()
        .map(|m| {
            // The switch sends the `enabled` setting; clicking the card sends the
            // same key as an action, which opens the utility's page.
            let mut it = Item::new(format!("{}.enabled", m.id), &m.name, Control::Toggle(m.enabled));
            it.description = m.description.clone();
            it.icon = crate::icons::module(&m.id);
            it.link = true;
            it
        })
        .collect();
    items.sort_by(|a, b| a.title.cmp(&b.title));
    Page {
        id: "dashboard".into(),
        title: "Dashboard".into(),
        description: String::new(),
        groups: vec![Group {
            title: "Utilities".into(),
            items,
        }],
    }
}

#[derive(Default)]
pub struct SettingsUi {
    window: Option<SettingsWindow>,
    modules: Vec<Module>,
    targets: Vec<Target>,
    /// The navigation row shown.
    current: usize,
    /// How each setting on the page is edited, by dotted key.
    kinds: HashMap<String, Kind>,
}

impl SettingsUi {
    /// Show the window at the page of the utility (or "general") `page`, or where it was.
    pub fn open(&mut self, host: &mut Host, page: Option<&str>) -> Result<(), String> {
        if self.window.is_none() {
            let mut w = SettingsWindow::new().map_err(|e| format!("could not open settings: {e}"))?;
            if let Err(e) = w.set_icon(crate::icons::APP) {
                tracing::warn!("could not set the settings window icon: {e}");
            }
            self.window = Some(w);
            self.current = 0;
        }
        self.read_modules(host);
        let wanted = match page {
            Some("general") => Some(Target::General),
            Some("dashboard") => Some(Target::Dashboard),
            Some(id) => self.modules.iter().position(|m| m.id == id).map(Target::Module),
            None => None,
        };
        let (entries, targets) = nav(&self.modules);
        self.targets = targets;
        if let Some(i) = wanted.and_then(|t| self.targets.iter().position(|x| *x == t)) {
            self.current = i;
        }
        if self.targets.get(self.current).is_none_or(|t| *t == Target::Category) {
            self.current = 0;
        }
        if let Some(w) = self.window.as_mut() {
            w.set_nav(entries, self.current);
        }
        self.refresh(host);
        if let Some(w) = self.window.as_mut() {
            w.show();
        }
        Ok(())
    }

    fn read_modules(&mut self, host: &Host) {
        self.modules = host.status()["modules"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        Some(Module {
                            id: m["id"].as_str()?.into(),
                            name: m["name"].as_str()?.into(),
                            description: m["description"].as_str().unwrap_or_default().into(),
                            enabled: m["enabled"].as_bool().unwrap_or(true),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    /// Rebuild the page shown from the current configuration.
    fn refresh(&mut self, host: &Host) {
        let Some(w) = self.window.as_mut() else { return };
        let config = serde_json::to_value(host.config()).unwrap_or(Value::Null);
        let defaults = serde_json::to_value(Config::new()).unwrap_or(Value::Null);
        w.set_dark(match config["general"]["theme"].as_str() {
            Some("light") => Some(false),
            Some("dark") => Some(true),
            _ => None,
        });
        self.kinds.clear();
        let (page, scene) = match self.targets.get(self.current) {
            Some(Target::General) => {
                let mut groups = groups("general", general_spec(), &config, &defaults, true, &mut self.kinds);
                let mut about = Item::new("about", "PowerKit", Control::None);
                about.description = format!("Version {}", host.status()["version"].as_str().unwrap_or("?"));
                about.icon = Some(crate::icons::APP);
                groups.push(Group {
                    title: "About".into(),
                    items: vec![about],
                });
                let page = Page {
                    id: "general".into(),
                    title: "General".into(),
                    description: String::new(),
                    groups,
                };
                (page, None)
            }
            Some(Target::Module(i)) => {
                let m = &self.modules[*i];
                let enabled = at(&config, &format!("{}.enabled", m.id)).as_bool().unwrap_or(m.enabled);
                let mut toggle = Item::new(
                    format!("{}.enabled", m.id),
                    format!("Enable {}", m.name),
                    Control::Toggle(enabled),
                );
                toggle.icon = crate::icons::module(&m.id);
                let mut groups = vec![Group {
                    title: String::new(),
                    items: vec![toggle],
                }];
                groups.extend(self::groups(
                    &m.id,
                    module_spec(&m.id),
                    &config,
                    &defaults,
                    enabled,
                    &mut self.kinds,
                ));
                // Restarted after every save too, so a changed shortcut shows straight away.
                let scene = powerkit_guide::walkthrough(&m.id).map(|walk| {
                    let config = host.config().clone();
                    Scene {
                        duration: walk.duration,
                        frame: Box::new(move |t| walk.frame(t, &config)),
                    }
                });
                let page = Page {
                    id: m.id.clone(),
                    title: m.name.clone(),
                    description: m.description.clone(),
                    groups,
                };
                (page, scene)
            }
            _ => (dashboard(&self.modules), None),
        };
        w.set_page(page, scene);
    }

    pub fn on_event(&mut self, event: SettingsEvent, host: &mut Host) {
        match event {
            SettingsEvent::Page(i) if self.targets.get(i).is_some_and(|t| *t != Target::Category) => {
                self.current = i;
                self.refresh(host);
            }
            SettingsEvent::Page(_) => {}
            SettingsEvent::Action(key) => {
                // Dashboard cards: open the utility's page.
                let id = key.strip_suffix(".enabled").unwrap_or(&key);
                let target = self.modules.iter().position(|m| m.id == id).map(Target::Module);
                if let Some(i) = target.and_then(|t| self.targets.iter().position(|x| *x == t)) {
                    self.current = i;
                    let entries = nav(&self.modules).0;
                    if let Some(w) = self.window.as_mut() {
                        w.set_nav(entries, i);
                    }
                    self.refresh(host);
                }
            }
            SettingsEvent::Set { key, value } => self.save(host, &key, value),
        }
    }

    fn save(&mut self, host: &mut Host, key: &str, value: SettingValue) {
        let mut config = serde_json::to_value(host.config()).unwrap_or(Value::Null);
        let original = at(&config, key).clone();
        let kind = self.kinds.get(key).copied().unwrap_or_else(|| guess(&original));
        let parsed = match value {
            SettingValue::Bool(b) => Ok(Value::Bool(b)),
            SettingValue::Text(t) => parse(&original, kind, &t),
        };
        let info = match parsed {
            Err(e) => Some((Severity::Error, format!("Not saved: {e}"))),
            Ok(v) => {
                set_at(&mut config, key, v);
                match host.handle_request("config.set", &json!({ "config": config })) {
                    Ok(r) => {
                        let warnings: Vec<&str> = r["warnings"]
                            .as_array()
                            .map(|a| a.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        (!warnings.is_empty()).then(|| (Severity::Warning, warnings.join(" ")))
                    }
                    Err(e) => Some((Severity::Error, format!("Not saved: {e}"))),
                }
            }
        };
        if key.ends_with(".enabled") {
            self.read_modules(host);
        }
        self.refresh(host);
        if let Some(w) = self.window.as_mut() {
            w.set_info(info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_a_card() {
        let config = serde_json::to_value(Config::new()).unwrap();
        let mut kinds = HashMap::new();
        let mut sections: Vec<&String> = config.as_object().unwrap().keys().filter(|k| *k != "version").collect();
        sections.sort();
        for section in sections {
            let spec = if section == "general" {
                general_spec()
            } else {
                assert!(
                    !module_spec(section).is_empty() || leaves(&config[section], section).len() <= 1,
                    "{section}"
                );
                module_spec(section)
            };
            let groups = groups(section, spec, &config, &config, true, &mut kinds);
            let other: Vec<&str> = groups
                .iter()
                .filter(|g| g.title == "Other")
                .flat_map(|g| g.items.iter().map(|i| i.key.as_str()))
                .collect();
            assert!(other.is_empty(), "not in the spec: {other:?}");
        }
        for (key, kind) in &kinds {
            let v = at(&config, key);
            assert!(!v.is_null(), "{key} is not a setting");
            match kind {
                Toggle => assert!(v.is_boolean(), "{key}"),
                Number => assert!(v.is_number(), "{key}"),
                Shortcut | Color | Text(_) | Choice(_) => assert!(v.is_string(), "{key}"),
                Lines(_) => assert!(is_string_list(v), "{key}"),
                Json => {}
            }
            if let Choice(options) = kind {
                let current = v.as_str().unwrap();
                assert!(options.iter().any(|(o, _)| *o == current), "{key}: {current}");
            }
        }
    }

    #[test]
    fn unlisted_settings_go_under_other() {
        let config = json!({"m": {"enabled": true, "a": 1, "b": {"c": "x"}}});
        let mut kinds = HashMap::new();
        let g = groups("m", Vec::new(), &config, &config, true, &mut kinds);
        assert_eq!(g.len(), 1);
        let keys: Vec<&str> = g[0].items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["m.a", "m.b.c"]);
        assert_eq!(kinds["m.a"], Number);
    }

    #[test]
    fn switched_off_cards_disable_their_settings() {
        let config = json!({"m": {"x": {"enabled": false, "n": 1}}});
        let mut kinds = HashMap::new();
        let spec = vec![(
            "G",
            vec![s("x.enabled", "X", "", Toggle).with(vec![s("x.n", "N", "", Number)])],
        )];
        let g = groups("m", spec, &config, &config, true, &mut kinds);
        let card = &g[0].items[0];
        assert!(card.enabled);
        assert!(!card.children[0].enabled);
        let g = groups("m", Vec::new(), &config, &config, false, &mut kinds);
        assert!(g[0].items.iter().all(|i| !i.enabled));
    }

    #[test]
    fn values_round_trip_through_text() {
        let apps = json!(["a.exe", "b.exe"]);
        assert_eq!(display(&apps, Lines("")), "a.exe\nb.exe");
        assert_eq!(
            parse(&apps, Lines(""), " c.exe \n\n d.exe\r\n").unwrap(),
            json!(["c.exe", "d.exe"])
        );
        assert_eq!(parse(&json!([]), Json, r#"[{"k": 1}]"#).unwrap(), json!([{"k": 1}]));
        assert!(parse(&json!([]), Json, "[").is_err());
        assert_eq!(parse(&json!(true), Toggle, "Off").unwrap(), json!(false));
        assert_eq!(parse(&json!(5), Number, " 12 ").unwrap(), json!(12));
        assert!(parse(&json!(5), Number, "1.5").is_err());
        assert_eq!(parse(&json!(0.5), Number, "1.5").unwrap(), json!(1.5));
        assert_eq!(parse(&json!("Win+T"), Shortcut, "Alt+T").unwrap(), json!("Alt+T"));
        assert_eq!(display(&json!({"k": 1}), Json), "{\n  \"k\": 1\n}");
    }

    #[test]
    fn set_at_replaces_nested_values() {
        let mut v = json!({"m": {"inner": {"x": 1}, "y": 2}});
        set_at(&mut v, "m.inner.x", json!(5));
        set_at(&mut v, "m.new", json!(true));
        assert_eq!(v, json!({"m": {"inner": {"x": 5}, "y": 2, "new": true}}));
    }

    #[test]
    fn nav_puts_utilities_under_their_category() {
        let m = |id: &str, name: &str| Module {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            enabled: true,
        };
        let modules = vec![
            m("color_picker", "Color Picker"),
            m("awake", "Awake"),
            m("mystery", "Mystery"),
        ];
        let (entries, targets) = nav(&modules);
        let titles: Vec<&str> = entries
            .iter()
            .map(|e| match e {
                NavEntry::Page { title, .. } | NavEntry::Category { title, .. } => title.as_str(),
            })
            .collect();
        assert_eq!(
            titles,
            [
                "Dashboard",
                "General",
                "System Tools",
                "Awake",
                "Color Picker",
                "Mystery"
            ]
        );
        assert_eq!(targets[3], Target::Module(1));
        assert_eq!(targets[2], Target::Category);
    }
}
