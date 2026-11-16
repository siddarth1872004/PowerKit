//! One walkthrough per utility. Each draws the whole frame for time `t` (seconds
//! since the loop started) and sets the caption for that moment.

use crate::palette::*;
use crate::*;
use powerkit_config::{AccentTrigger, Config, FindMouseActivation, GrabModifier, RemapTarget};

type Draw = fn(&mut Canvas, f32, &Config);

pub(crate) const ALL: &[(&str, f32, Draw)] = &[
    ("always_on_top", 9.0, always_on_top),
    ("color_picker", 9.0, color_picker),
    ("fancy_zones", 9.0, fancy_zones),
    ("power_rename", 10.0, power_rename),
    ("keyboard_manager", 7.0, keyboard_manager),
    ("launcher", 9.0, launcher),
    ("awake", 10.0, awake),
    ("light_switch", 9.0, light_switch),
    ("quick_accent", 7.0, quick_accent),
    ("mouse_utilities", 12.0, mouse_utilities),
    ("crop_and_lock", 9.0, crop_and_lock),
    ("shortcut_guide", 8.0, shortcut_guide),
    ("grab_and_move", 9.0, grab_and_move),
    ("window_hopper", 8.0, window_hopper),
    ("hosts_editor", 10.0, hosts_editor),
    ("environment_variables", 9.0, environment_variables),
    ("registry_preview", 10.0, registry_preview),
    ("new_plus", 10.0, new_plus),
    ("file_locksmith", 9.0, file_locksmith),
    ("image_resizer", 9.0, image_resizer),
    ("text_extractor", 9.0, text_extractor),
    ("power_display", 9.0, power_display),
    ("screen_ruler", 9.0, screen_ruler),
    ("zoom_it", 10.0, zoom_it),
];

const KEYS_Y: f32 = 216.0;
const BLACK: Color = Color::rgb(0, 0, 0);

/// The key caps for `shortcut`, shown from just before `a` to just after `b` and
/// held down between them.
fn press(c: &mut Canvas, t: f32, a: f32, b: f32, shortcut: &str) {
    if t < a - 0.5 || t > b + 0.5 {
        return;
    }
    if shortcut.trim().is_empty() {
        c.text_box(0.0, KEYS_Y, WIDTH, 22.0, "No shortcut set", 9.0, WHITE, Align::Center);
    } else {
        c.keys(WIDTH / 2.0, KEYS_Y, shortcut, t >= a && t <= b);
    }
}

/// A shortcut as written in a caption.
fn named(shortcut: &str) -> String {
    if shortcut.trim().is_empty() {
        return "its shortcut (none is set)".into();
    }
    shortcut.split('+').map(key_label).collect::<Vec<_>>().join("+")
}

/// A ripple for a click at `at` seconds.
fn click(c: &mut Canvas, t: f32, at: f32, x: f32, y: f32) {
    c.click(x, y, span(t, at, at + 0.5));
}

/// `#RRGGBB`, or `fallback` when it is not one.
fn hex(s: &str, fallback: Color) -> Color {
    let s = s.trim().trim_start_matches('#');
    match u32::from_str_radix(s, 16) {
        Ok(v) if s.len() == 6 => Color(0xFF00_0000 | v),
        _ => fallback,
    }
}

/// Grey bars standing in for text.
fn scribble(c: &mut Canvas, x: f32, y: f32, w: f32, rows: usize, color: Color) {
    for i in 0..rows {
        let len = w * [0.92, 0.7, 0.84, 0.55, 0.78][i % 5];
        c.rect(x, y + i as f32 * 11.0, len, 4.0, 2.0, color);
    }
}

fn file_icon(c: &mut Canvas, x: f32, y: f32, folder: bool) {
    if folder {
        c.rect(x, y, 6.0, 4.0, 1.0, Color::rgb(230, 170, 40));
        c.rect(x, y + 2.0, 12.0, 9.0, 1.5, YELLOW);
    } else {
        c.rect(x + 1.0, y, 10.0, 12.0, 1.5, WHITE);
        c.outline(x + 1.0, y, 10.0, 12.0, 1.5, MUTED, 1.0);
        c.line(x + 3.0, y + 4.0, x + 9.0, y + 4.0, LINE, 1.0);
        c.line(x + 3.0, y + 7.0, x + 9.0, y + 7.0, LINE, 1.0);
    }
}

/// A row of a file list.
fn file_row(c: &mut Canvas, x: f32, y: f32, w: f32, name: &str, folder: bool, selected: bool) {
    if selected {
        c.rect(x, y, w, 18.0, 3.0, ACCENT_SOFT);
    }
    file_icon(c, x + 6.0, y + 3.0, folder);
    c.text(x + 24.0, y + 9.0, name, 7.5, TEXT);
}

/// A table with a header row; `cols` are the left edges, relative to `x`.
#[allow(clippy::too_many_arguments)]
fn table(
    c: &mut Canvas,
    x: f32,
    y: f32,
    w: f32,
    cols: &[f32],
    head: &[&str],
    rows: &[Vec<String>],
    selected: Option<usize>,
) {
    c.rect(x, y, w, 16.0, 0.0, TITLE);
    for (cx, h) in cols.iter().zip(head) {
        c.bold(x + cx + 4.0, y + 8.0, h, 7.0, MUTED);
    }
    for (i, row) in rows.iter().enumerate() {
        let ry = y + 16.0 + i as f32 * 16.0;
        if selected == Some(i) {
            c.rect(x, ry, w, 16.0, 0.0, ACCENT_SOFT);
        }
        for (cx, cell) in cols.iter().zip(row) {
            c.text(x + cx + 4.0, ry + 8.0, cell, 7.0, TEXT);
        }
        c.line(x, ry + 16.0, x + w, ry + 16.0, LINE, 0.5);
    }
}

fn always_on_top(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.always_on_top;
    c.desktop();
    let pinned = (2.2..6.8).contains(&t);
    let notes_front = (1.0..7.4).contains(&t);
    let border = hex(&s.border_color, ACCENT);
    let draw_notes = |c: &mut Canvas| {
        let top = c.window(50.0, 36.0, 200.0, 140.0, "Notes");
        scribble(c, 62.0, top + 12.0, 170.0, 8, LINE);
        if pinned && s.show_border {
            c.outline(48.0, 34.0, 204.0, 144.0, 7.0, border, 3.0);
        }
    };
    let draw_browser = |c: &mut Canvas| {
        let top = c.window(170.0, 66.0, 260.0, 160.0, "Browser");
        c.rect(180.0, top + 8.0, 240.0, 12.0, 6.0, TITLE);
        c.rect(180.0, top + 28.0, 110.0, 60.0, 4.0, ACCENT_SOFT);
        scribble(c, 300.0, top + 30.0, 110.0, 6, LINE);
        scribble(c, 180.0, top + 98.0, 230.0, 3, LINE);
    };
    if notes_front {
        draw_browser(c);
        draw_notes(c);
    } else {
        draw_notes(c);
        draw_browser(c);
    }
    press(c, t, 1.6, 2.2, &s.shortcut);
    press(c, t, 6.2, 6.8, &s.shortcut);
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 200.0),
            (0.8, 110.0, 44.0),
            (2.6, 110.0, 44.0),
            (3.8, 350.0, 180.0),
            (6.8, 350.0, 180.0),
            (7.2, 380.0, 76.0),
        ],
    );
    click(c, t, 0.9, 110.0, 44.0);
    click(c, t, 4.0, 350.0, 180.0);
    click(c, t, 7.3, 380.0, 76.0);
    c.cursor(x, y);
    let pin = format!("Press {} to pin the active window", named(&s.shortcut));
    let unpin = format!("Press {} again to unpin it", named(&s.shortcut));
    c.steps(
        t,
        &[
            (0.0, "Click the window you want to keep in view"),
            (1.4, &pin),
            (3.4, "Click another window: the pinned one stays on top"),
            (5.8, &unpin),
            (7.0, "Unpinned windows stack as usual again"),
        ],
    );
}

const SWATCHES: [Color; 18] = [
    Color::rgb(232, 85, 61),
    Color::rgb(245, 146, 51),
    Color::rgb(250, 204, 70),
    Color::rgb(150, 205, 90),
    Color::rgb(58, 170, 120),
    Color::rgb(40, 160, 190),
    Color::rgb(46, 116, 236),
    Color::rgb(98, 84, 210),
    Color::rgb(160, 80, 200),
    Color::rgb(222, 80, 150),
    Color::rgb(120, 72, 52),
    Color::rgb(60, 64, 72),
    Color::rgb(255, 230, 200),
    Color::rgb(200, 236, 250),
    Color::rgb(220, 250, 220),
    Color::rgb(250, 220, 236),
    Color::rgb(180, 186, 196),
    Color::rgb(250, 250, 250),
];

/// Draw the swatch grid with its top-left at `(x, y)`; returns the swatch at `at`.
fn swatches(c: &mut Canvas, x: f32, y: f32, at: (f32, f32)) -> Option<Color> {
    let mut hit = None;
    for (i, col) in SWATCHES.iter().enumerate() {
        let sx = x + (i % 6) as f32 * 62.0;
        let sy = y + (i / 6) as f32 * 52.0;
        c.rect(sx, sy, 58.0, 48.0, 4.0, *col);
        if (sx..sx + 58.0).contains(&at.0) && (sy..sy + 48.0).contains(&at.1) {
            hit = Some(*col);
        }
    }
    hit
}

fn color_picker(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.color_picker;
    c.desktop();
    let top = c.window(40.0, 26.0, 400.0, 190.0, "Photos");
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 230.0),
            (1.6, 300.0, 230.0),
            (2.6, 100.0, top + 30.0),
            (3.6, 230.0, top + 80.0),
            (4.6, 160.0, top + 130.0),
            (5.6, 90.0, top + 30.0),
        ],
    );
    let hit = swatches(c, 56.0, top + 8.0, (x, y));
    press(c, t, 0.6, 1.4, &s.shortcut);
    let picking = (1.5..6.2).contains(&t);
    let picked = SWATCHES[0];
    if picking {
        let col = hit.unwrap_or(WHITE);
        let (mx, my) = (x + 34.0, y - 34.0);
        c.circle(mx + 1.0, my + 2.0, 22.0, SHADOW);
        c.circle(mx, my, 22.0, col);
        c.ring(mx, my, 22.0, WHITE, 3.0);
        let label = format!("#{:06X}", col.0 & 0xFF_FFFF);
        c.rect(mx - 26.0, my + 26.0, 52.0, 14.0, 7.0, Color::rgb(30, 32, 38));
        c.text_box(mx - 26.0, my + 26.0, 52.0, 14.0, &label, 7.0, WHITE, Align::Center);
    }
    click(c, t, 5.9, x, y);
    c.toast(&format!("#{:06X} copied", picked.0 & 0xFF_FFFF), span(t, 6.2, 8.8));
    c.cursor(x, y);
    let start = format!("Press {} to start picking", named(&s.shortcut));
    c.steps(
        t,
        &[
            (0.0, &start),
            (1.6, "Move over any colour on screen; the magnifier shows its value"),
            (5.8, "Click to copy it, in the format chosen in settings"),
        ],
    );
}

fn fancy_zones(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.fancy_zones;
    c.desktop();
    let zones: [(f32, f32, f32, f32); 3] = [
        (8.0, 8.0, 150.0, 236.0),
        (164.0, 8.0, 150.0, 236.0),
        (320.0, 8.0, 152.0, 236.0),
    ];
    let highlight = hex(&s.highlight_color, ACCENT);
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 210.0),
            (0.8, 120.0, 69.0),
            (1.3, 120.0, 69.0),
            (2.8, 260.0, 110.0),
            (4.3, 400.0, 120.0),
        ],
    );
    let dragging = (1.3..4.6).contains(&t);
    if dragging {
        for z in zones {
            let hot = (z.0..z.0 + z.2).contains(&x);
            c.rect(
                z.0,
                z.1,
                z.2,
                z.3,
                6.0,
                if hot { highlight.alpha(0.55) } else { WHITE.alpha(0.18) },
            );
            c.outline(z.0, z.1, z.2, z.3, 6.0, WHITE.alpha(0.7), 1.5);
        }
    }
    let free = (x - 60.0, y - 9.0, 200.0, 130.0);
    let lerp_rect = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32), p: f32| {
        (
            lerp(a.0, b.0, p),
            lerp(a.1, b.1, p),
            lerp(a.2, b.2, p),
            lerp(a.3, b.3, p),
        )
    };
    let r = if t < 4.6 {
        free
    } else if t < 7.0 {
        lerp_rect(free, zones[2], ease(span(t, 4.6, 4.9)))
    } else {
        lerp_rect(zones[2], zones[1], ease(span(t, 7.0, 7.4)))
    };
    let top = c.window(r.0, r.1, r.2, r.3, "Mail");
    scribble(
        c,
        r.0 + 10.0,
        top + 10.0,
        r.2 - 20.0,
        ((r.3 - 30.0) / 11.0) as usize,
        LINE,
    );
    if dragging {
        c.key_caps(WIDTH / 2.0, KEYS_Y, &["Shift".into()], true);
    }
    press(c, t, 6.4, 7.0, &s.prev_zone_shortcut);
    c.cursor(x, y);
    let keys = format!("{} moves the window between zones", named(&s.prev_zone_shortcut));
    c.steps(
        t,
        &[
            (0.0, "Drag a window by its title bar"),
            (1.3, "Hold Shift while dragging to show the zones"),
            (4.6, "Let go over a zone and the window fills it"),
            (6.0, &keys),
        ],
    );
}

fn power_rename(c: &mut Canvas, t: f32, _: &Config) {
    c.desktop();
    let names = ["IMG_0412.jpg", "IMG_0413.jpg", "IMG_0414.jpg", "IMG_0415.jpg"];
    let top = c.window(24.0, 20.0, 230.0, 160.0, "Pictures");
    let replace = typed("Trip_", t, 4.6, 0.18);
    let done = t >= 7.4;
    for (i, n) in names.iter().enumerate() {
        let shown = if done { n.replace("IMG_", "Trip_") } else { (*n).into() };
        file_row(c, 30.0, top + 6.0 + i as f32 * 20.0, 218.0, &shown, false, t >= 1.1);
    }
    let menu_at = (130.0, top + 58.0);
    if (1.8..3.4).contains(&t) {
        let hot = (t >= 2.6).then_some(1);
        c.menu(
            menu_at.0,
            menu_at.1,
            130.0,
            &["Open", "Rename with PowerKit", "", "Properties"],
            hot,
        );
    }
    if (3.4..7.4).contains(&t) {
        let wt = c.window(190.0, 44.0, 270.0, 176.0, "PowerRename");
        c.text(200.0, wt + 12.0, "Search for", 7.0, MUTED);
        c.field(200.0, wt + 20.0, 120.0, typed("IMG_", t, 3.6, 0.12), t < 4.4);
        c.text(332.0, wt + 12.0, "Replace with", 7.0, MUTED);
        c.field(332.0, wt + 20.0, 118.0, replace, t >= 4.4);
        c.bold(200.0, wt + 50.0, "Preview", 7.0, MUTED);
        for (i, n) in names.iter().enumerate() {
            let ry = wt + 64.0 + i as f32 * 16.0;
            c.text(200.0, ry, n, 7.0, TEXT);
            c.text(300.0, ry, "\u{2192}", 7.0, MUTED);
            let to = if t >= 3.6 {
                n.replace("IMG_", replace)
            } else {
                (*n).into()
            };
            c.text(316.0, ry, &to, 7.0, ACCENT);
        }
        c.button(392.0, wt + 136.0, 58.0, "Rename", (6.8..7.4).contains(&t));
    }
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 230.0),
            (0.6, 140.0, top + 40.0),
            (2.0, 140.0, top + 40.0),
            (2.6, 180.0, menu_at.1 + 3.0 + MENU_ROW * 1.5),
            (3.4, 180.0, menu_at.1 + 3.0 + MENU_ROW * 1.5),
            (6.0, 420.0, 196.0),
        ],
    );
    press(c, t, 0.7, 1.1, "Ctrl+A");
    click(c, t, 1.7, 140.0, top + 40.0);
    click(c, t, 3.2, x, y);
    click(c, t, 6.9, x, y);
    c.cursor(x, y);
    c.steps(
        t,
        &[
            (0.0, "Select the files to rename"),
            (1.6, "Right-click and choose Rename with PowerKit"),
            (3.4, "Type what to find and what to put instead"),
            (5.6, "The preview shows every new name before anything changes"),
            (7.4, "Rename, and Undo is there if you change your mind"),
        ],
    );
}

fn keyboard_manager(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.keyboard_manager;
    let profile = s
        .profiles
        .iter()
        .find(|p| p.name == s.active_profile)
        .or(s.profiles.first());
    let (from, to) = profile
        .and_then(|p| {
            p.key_remaps.iter().find_map(|r| match &r.to {
                RemapTarget::Key(k) | RemapTarget::Shortcut(k) => Some((r.from.clone(), k.clone())),
                RemapTarget::Disabled => None,
            })
        })
        .unwrap_or_else(|| ("CapsLock".into(), "Ctrl".into()));
    c.desktop();
    let top = c.window(70.0, 110.0, 340.0, 90.0, "Notes");
    scribble(c, 82.0, top + 10.0, 310.0, 5, LINE);
    let shown = span(t, 0.4, 1.2);
    c.rect(110.0, 30.0, 260.0, 64.0, 8.0, WHITE.alpha(shown));
    if shown > 0.5 {
        let down = (2.6..3.3).contains(&t) || (4.4..5.1).contains(&t);
        let after = t >= 1.6;
        c.key_caps(180.0, 50.0, &[key_label(&from)], down);
        if after {
            c.line(222.0, 62.0, 258.0, 62.0, MUTED, 2.0);
            c.polygon(vec![(262.0, 62.0), (254.0, 57.0), (254.0, 67.0)], MUTED, None);
            c.key_caps(300.0, 50.0, &[key_label(&to)], down);
        }
    }
    let sends = format!("Press {}: programs receive {}", key_label(&from), key_label(&to));
    c.steps(
        t,
        &[
            (0.0, "Keyboard Manager turns one key or shortcut into another"),
            (1.6, "Add a remap in settings and it applies at once"),
            (2.6, &sends),
        ],
    );
}

const APPS: [&str; 5] = ["Calculator", "Calendar", "Camera", "Clock", "Control Panel"];

fn launcher(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.launcher;
    c.desktop();
    let top = c.window(30.0, 40.0, 250.0, 160.0, "Documents");
    scribble(c, 42.0, top + 12.0, 220.0, 9, LINE);
    press(c, t, 0.5, 1.2, &s.shortcut);
    if (1.3..5.3).contains(&t) {
        let query = typed("calc", t, 1.8, 0.3);
        c.rect(102.0, 53.0, 280.0, 28.0, 8.0, SHADOW);
        c.rect(100.0, 50.0, 280.0, 28.0, 8.0, WINDOW);
        c.outline(100.0, 50.0, 280.0, 28.0, 8.0, ACCENT, 1.5);
        c.ring(116.0, 63.0, 5.0, MUTED, 1.5);
        c.line(120.0, 67.0, 124.0, 71.0, MUTED, 1.5);
        c.field_text(130.0, 64.0, query);
        if !query.is_empty() {
            let hits: Vec<&str> = APPS
                .iter()
                .copied()
                .filter(|a| a.to_lowercase().starts_with(query))
                .collect();
            c.rect(100.0, 82.0, 280.0, hits.len() as f32 * 22.0 + 6.0, 8.0, WINDOW);
            for (i, a) in hits.iter().enumerate() {
                let ry = 85.0 + i as f32 * 22.0;
                if i == 0 {
                    c.rect(103.0, ry, 274.0, 22.0, 5.0, ACCENT_SOFT);
                }
                c.rect(110.0, ry + 4.0, 14.0, 14.0, 3.0, ACCENT);
                c.text(132.0, ry + 11.0, a, 8.0, TEXT);
                c.text(330.0, ry + 11.0, "App", 7.0, MUTED);
            }
        }
    }
    press(c, t, 4.8, 5.2, "Enter");
    if t >= 5.3 {
        let wt = c.window(300.0, 30.0, 140.0, 190.0, "Calculator");
        c.text_box(306.0, wt + 4.0, 126.0, 26.0, "0", 14.0, TEXT, Align::Left);
        for i in 0..20 {
            let bx = 306.0 + (i % 4) as f32 * 32.0;
            let by = wt + 36.0 + (i / 4) as f32 * 26.0;
            c.rect(bx, by, 29.0, 23.0, 3.0, if i % 4 == 3 { ACCENT_SOFT } else { TITLE });
        }
    }
    let open = format!("Press {} anywhere", named(&s.shortcut));
    c.steps(
        t,
        &[
            (0.0, &open),
            (1.4, "Type a few letters of an app, file, or a sum like 2+2"),
            (4.6, "Enter opens the top result"),
        ],
    );
}

impl Canvas {
    /// Text typed into a box, with the caret after it.
    fn field_text(&mut self, x: f32, cy: f32, text: &str) {
        self.text(x, cy, text, 9.0, TEXT);
        let cx = x + 1.0 + text.chars().count() as f32 * 5.0;
        self.line(cx, cy - 6.0, cx, cy + 6.0, TEXT, 1.0);
    }
}

fn tray_icon_cup(c: &mut Canvas, x: f32, y: f32, color: Color) {
    c.rect(x, y, 8.0, 8.0, 2.0, color);
    c.ring(x + 9.0, y + 3.5, 2.2, color, 1.2);
}

fn awake(c: &mut Canvas, t: f32, _: &Config) {
    c.desktop();
    let top = c.window(60.0, 30.0, 300.0, 170.0, "Download");
    let p = if t < 4.0 {
        span(t, 0.0, 4.0) * 0.4
    } else {
        0.4 + span(t, 6.0, 10.0) * 0.55
    };
    c.text(72.0, top + 16.0, "Downloading backup.zip", 8.0, TEXT);
    c.rect(72.0, top + 30.0, 276.0, 8.0, 4.0, TITLE);
    c.rect(72.0, top + 30.0, 276.0 * p, 8.0, 4.0, ACCENT);
    scribble(c, 72.0, top + 52.0, 260.0, 6, LINE);
    let on = t >= 5.6;
    tray_icon_cup(c, 410.0, HEIGHT - 13.0, if on { YELLOW } else { WHITE.alpha(0.6) });
    if t < 4.4 {
        c.rect(0.0, 0.0, WIDTH, HEIGHT, 0.0, BLACK.alpha(span(t, 1.6, 3.6) * 0.92));
    }
    if (4.4..5.8).contains(&t) {
        c.menu(
            330.0,
            166.0,
            130.0,
            &["Keep awake", "Keep awake for 1 hour", "", "Off"],
            (t >= 5.0).then_some(0),
        );
    }
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 200.0),
            (4.4, 300.0, 200.0),
            (5.0, 380.0, 178.0),
            (6.4, 380.0, 178.0),
            (7.0, 300.0, 150.0),
        ],
    );
    click(c, t, 5.5, 380.0, 178.0);
    c.cursor(x, y);
    let idle = format!("Idle for {} min", ((t - 6.0).max(0.0) * 3.0) as u32);
    if t >= 6.0 {
        c.toast(&idle, span(t, 6.0, 10.0).mul_add(0.7, 0.15));
    }
    c.steps(
        t,
        &[
            (0.0, "Left alone, the PC dims and sleeps, even mid-download"),
            (4.4, "Choose Keep awake from PowerKit's tray menu"),
            (6.0, "The screen and the PC stay on until you turn it off"),
        ],
    );
}

fn light_switch(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.light_switch;
    let dark = ease(span(t, 2.6, 3.6)) * (1.0 - ease(span(t, 6.6, 7.4)));
    c.desktop();
    c.rect(
        0.0,
        0.0,
        WIDTH,
        HEIGHT - 18.0,
        0.0,
        Color::rgb(10, 14, 40).alpha(0.55 * dark),
    );
    let (sx, sy) = path(t, &[(0.0, 400.0, 40.0), (3.0, 440.0, 150.0)]);
    let sun = 1.0 - dark;
    c.circle(sx, sy, 16.0, YELLOW.alpha(sun));
    c.circle(410.0, 46.0, 14.0, WHITE.alpha(dark * 0.9));
    c.circle(
        416.0,
        41.0,
        12.0,
        WALL_TOP.mix(Color::rgb(10, 14, 40), 0.55).alpha(dark),
    );
    let top = c.window_toned(40.0, 36.0, 300.0, 170.0, "Files", dark);
    let bar = LINE.mix(Color::rgb(70, 74, 84), dark);
    scribble(c, 54.0, top + 12.0, 270.0, 10, bar);
    press(c, t, 6.0, 6.6, &s.shortcut);
    let when = format!("At sunset, or at {}, apps and Windows switch to dark", s.dark_time);
    let flip = format!("{} switches by hand at any time", named(&s.shortcut));
    c.steps(
        t,
        &[
            (0.0, "Light Switch follows the time of day"),
            (2.4, &when),
            (5.6, &flip),
        ],
    );
}

fn quick_accent(c: &mut Canvas, t: f32, cfg: &Config) {
    let trigger = match cfg.quick_accent.trigger {
        AccentTrigger::Arrows => "\u{2192}",
        AccentTrigger::Space | AccentTrigger::Both => "Space",
    };
    c.desktop();
    let top = c.window(70.0, 80.0, 340.0, 110.0, "Message");
    let text = if t >= 4.4 { "café" } else { typed("caf", t, 0.3, 0.3) };
    c.rect(82.0, top + 14.0, 316.0, 26.0, 4.0, WHITE);
    c.outline(82.0, top + 14.0, 316.0, 26.0, 4.0, ACCENT, 1.0);
    c.field_text(90.0, top + 27.0, text);
    scribble(c, 82.0, top + 52.0, 300.0, 3, LINE);
    let accents = ["é", "è", "ê", "ë", "ē", "ė"];
    if (2.2..4.4).contains(&t) {
        let sel = if t >= 3.0 { 1 } else { 0 };
        let sel = if t >= 3.6 { 0 } else { sel };
        let w = accents.len() as f32 * 26.0 + 8.0;
        let x0 = (WIDTH - w) / 2.0;
        c.rect(x0, 40.0, w, 34.0, 8.0, Color::rgb(30, 32, 38));
        for (i, a) in accents.iter().enumerate() {
            let ax = x0 + 4.0 + i as f32 * 26.0;
            if i == sel {
                c.rect(ax, 44.0, 24.0, 26.0, 5.0, ACCENT);
            }
            c.text_box(ax, 44.0, 24.0, 26.0, a, 12.0, WHITE, Align::Center);
        }
    }
    if (1.6..4.4).contains(&t) {
        let tap = (2.2..2.5).contains(&t) || (3.0..3.3).contains(&t) || (3.6..3.9).contains(&t);
        let key = if (3.6..3.9).contains(&t) && trigger == "\u{2192}" {
            "\u{2190}"
        } else {
            trigger
        };
        c.key_caps(WIDTH / 2.0, KEYS_Y, &["e".into(), key.into()], tap);
    }
    let hold = format!("Hold the letter and tap {trigger}");
    c.steps(
        t,
        &[
            (0.0, "Type as usual"),
            (1.6, &hold),
            (2.2, "Pick the accent you need from the bar"),
            (4.4, "Let go: the accented letter is typed"),
        ],
    );
}

fn mouse_utilities(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.mouse_utilities;
    c.desktop();
    let top = c.window(40.0, 26.0, 400.0, 190.0, "Spreadsheet");
    for r in 0..9 {
        c.line(
            48.0,
            top + 14.0 + r as f32 * 18.0,
            432.0,
            top + 14.0 + r as f32 * 18.0,
            LINE,
            0.8,
        );
    }
    for col in 0..6 {
        c.line(
            48.0 + col as f32 * 64.0,
            top + 14.0,
            48.0 + col as f32 * 64.0,
            top + 158.0,
            LINE,
            0.8,
        );
    }
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 140.0),
            (4.4, 300.0, 140.0),
            (5.0, 150.0, 90.0),
            (5.8, 250.0, 160.0),
            (6.6, 360.0, 80.0),
            (8.4, 360.0, 80.0),
            (10.0, 200.0, 120.0),
            (11.5, 280.0, 170.0),
        ],
    );
    // Find My Mouse: everything but a circle round the pointer goes dark.
    if (1.3..3.9).contains(&t) {
        let r = lerp(220.0, 34.0, ease(span(t, 1.3, 1.9)));
        let fade = 1.0 - span(t, 3.5, 3.9);
        c.ring(x, y, r + 300.0, BLACK.alpha(0.75 * fade), 600.0);
        c.ring(x, y, r, WHITE.alpha(fade), 2.0);
    }
    let find = match s.find_my_mouse.activation {
        FindMouseActivation::DoubleCtrl => {
            if (0.4..0.6).contains(&t) || (0.8..1.0).contains(&t) {
                c.key_caps(WIDTH / 2.0, KEYS_Y, &["Ctrl".into()], true);
            } else if (0.0..1.3).contains(&t) {
                c.key_caps(WIDTH / 2.0, KEYS_Y, &["Ctrl".into()], false);
            }
            "Lost the pointer? Press Ctrl twice".to_string()
        }
        FindMouseActivation::Shake => "Lost the pointer? Shake the mouse".to_string(),
        FindMouseActivation::ShortcutOnly => {
            press(c, t, 0.4, 1.0, &s.find_my_mouse.shortcut);
            format!("Lost the pointer? Press {}", named(&s.find_my_mouse.shortcut))
        }
    };
    press(c, t, 4.2, 4.6, &s.highlighter.shortcut);
    let left = hex(&s.highlighter.left_color, YELLOW);
    let right = hex(&s.highlighter.right_color, ACCENT);
    for (at, px, py, col) in [
        (5.0, 150.0, 90.0, left),
        (5.8, 250.0, 160.0, left),
        (6.6, 360.0, 80.0, right),
    ] {
        if (at..8.0).contains(&t) {
            let a = 1.0 - span(t, at + 0.6, at + 1.2);
            c.circle(px, py, 13.0, col.alpha(0.55 * a));
        }
    }
    press(c, t, 8.4, 8.8, &s.crosshairs.shortcut);
    if t >= 8.8 {
        let col = hex(&s.crosshairs.color, Color::rgb(255, 0, 0));
        let gap = 8.0;
        c.line(0.0, y, x - gap, y, col, 3.0);
        c.line(x + gap, y, WIDTH, y, col, 3.0);
        c.line(x, 0.0, x, y - gap, col, 3.0);
        c.line(x, y + gap, x, HEIGHT, col, 3.0);
    }
    c.cursor(x, y);
    let mark = format!("{} marks every click", named(&s.highlighter.shortcut));
    let cross = format!("{} draws crosshairs through the pointer", named(&s.crosshairs.shortcut));
    c.steps(
        t,
        &[
            (0.0, &find),
            (1.3, "A spotlight shows where it is"),
            (4.0, &mark),
            (8.2, &cross),
        ],
    );
}

/// Picture tiles in the 280 x 160 area at `(x, y)`, cut to the part that starts
/// at `src` and is `size` big, scaled by `scale`.
fn picture(c: &mut Canvas, x: f32, y: f32, src: (f32, f32), size: (f32, f32), scale: f32) {
    for i in 0..18 {
        let tx = (i % 6) as f32 * 47.0;
        let ty = (i / 6) as f32 * 54.0;
        let (l, r) = (tx.max(src.0), (tx + 44.0).min(src.0 + size.0));
        let (u, d) = (ty.max(src.1), (ty + 50.0).min(src.1 + size.1));
        if l < r && u < d {
            let col = SWATCHES[(i * 7) % SWATCHES.len()];
            c.rect(
                x + (l - src.0) * scale,
                y + (u - src.1) * scale,
                (r - l) * scale,
                (d - u) * scale,
                0.0,
                col,
            );
        }
    }
}

fn crop_and_lock(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.crop_and_lock;
    c.desktop();
    let top = c.window(20.0, 20.0, 296.0, 190.0, "Dashboard");
    let origin = (28.0, top + 6.0);
    picture(c, origin.0, origin.1, (0.0, 0.0), (282.0, 160.0), 1.0);
    press(c, t, 0.5, 1.1, &s.thumbnail_shortcut);
    let a = (110.0, 70.0);
    let (bx, by) = path(t, &[(0.0, 110.0, 70.0), (1.8, 110.0, 70.0), (3.6, 220.0, 150.0)]);
    if (1.2..4.0).contains(&t) {
        c.rect(0.0, 0.0, WIDTH, HEIGHT, 0.0, BLACK.alpha(0.25));
        if t >= 1.8 {
            c.rect(a.0, a.1, bx - a.0, by - a.1, 0.0, WHITE.alpha(0.2));
            c.outline(a.0, a.1, bx - a.0, by - a.1, 0.0, WHITE, 1.5);
        }
    }
    if t >= 4.0 {
        let (w, h) = ((220.0 - 110.0) * 1.2, (150.0 - 70.0) * 1.2);
        let wt = c.window(316.0, 100.0, w + 12.0, h + TITLE_H + 12.0, "Dashboard (cropped)");
        picture(c, 322.0, wt + 6.0, (a.0 - origin.0, a.1 - origin.1), (110.0, 80.0), 1.2);
    }
    let (x, y) = if t < 1.8 {
        path(t, &[(0.0, 300.0, 230.0), (1.4, a.0, a.1)])
    } else {
        (bx, by)
    };
    c.cursor(x, y);
    let start = format!("Press {} on the window to crop", named(&s.thumbnail_shortcut));
    let other = format!("{} crops the live window itself instead", named(&s.reparent_shortcut));
    c.steps(
        t,
        &[
            (0.0, &start),
            (1.6, "Drag round the part you want to keep"),
            (4.0, "It becomes a small window of its own that stays up to date"),
            (6.8, &other),
        ],
    );
}

fn shortcut_guide(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.shortcut_guide;
    c.desktop();
    let top = c.window(70.0, 40.0, 300.0, 160.0, "Files");
    scribble(c, 82.0, top + 12.0, 270.0, 10, LINE);
    let delay = s.press_time_ms as f32 / 1000.0;
    let open_at = if s.hold_win { 0.5 + delay } else { 1.2 };
    let shown = (open_at..6.4).contains(&t);
    if shown {
        let a = span(t, open_at, open_at + 0.25);
        c.rect(40.0, 20.0, 400.0, 186.0, 10.0, Color::rgb(28, 30, 36).alpha(0.94 * a));
        c.bold(56.0, 38.0, "Shortcuts", 9.0, WHITE.alpha(a));
        let mut rows: Vec<(String, &str)> = vec![
            ("Win+E".into(), "File Explorer"),
            ("Win+D".into(), "Show the desktop"),
            ("Win+V".into(), "Clipboard history"),
            ("Win+Shift+S".into(), "Snip part of the screen"),
            ("Win+L".into(), "Lock"),
        ];
        for (sc, what) in [
            (&cfg.color_picker.shortcut, "Color Picker"),
            (&cfg.always_on_top.shortcut, "Pin a window on top"),
            (&cfg.launcher.shortcut, "Launcher"),
            (&cfg.crop_and_lock.thumbnail_shortcut, "Crop and Lock"),
            (&cfg.light_switch.shortcut, "Light or dark"),
        ] {
            if !sc.is_empty() {
                rows.push((named(sc), what));
            }
        }
        for (i, (sc, what)) in rows.iter().take(10).enumerate() {
            let x = 56.0 + (i / 5) as f32 * 192.0;
            let y = 60.0 + (i % 5) as f32 * 26.0;
            let w = sc.chars().count() as f32 * 4.4 + 12.0;
            c.rect(x, y - 8.0, w, 16.0, 4.0, WHITE.alpha(0.14 * a));
            c.text_box(x, y - 8.0, w, 16.0, sc, 7.0, WHITE.alpha(a), Align::Center);
            c.text(x + w + 8.0, y, what, 7.5, Color::rgb(200, 204, 214).alpha(a));
        }
    }
    let how = if s.hold_win {
        if (0.5..6.4).contains(&t) {
            c.key_caps(WIDTH / 2.0, KEYS_Y, &["Win".into()], true);
        }
        format!("Hold the Windows key for {:.1} seconds", delay)
    } else {
        press(c, t, 0.6, 1.2, &s.shortcut);
        format!("Press {}", named(&s.shortcut))
    };
    c.steps(
        t,
        &[
            (0.0, &how),
            (open_at, "Every shortcut you can use, PowerKit's included"),
            (6.4, "Let go to close it"),
        ],
    );
}

fn grab_and_move(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.grab_and_move;
    let modifier = match s.modifier {
        GrabModifier::Win => "Win",
        GrabModifier::Alt => "Alt",
    };
    c.desktop();
    let mv = ease(span(t, 1.8, 3.4));
    let grow = if s.resize { ease(span(t, 4.2, 6.0)) } else { 0.0 };
    let (wx, wy) = (50.0 + 150.0 * mv, 40.0 + 40.0 * mv);
    let (ww, wh) = (200.0 + 70.0 * grow, 120.0 + 50.0 * grow);
    let top = c.window(wx, wy, ww, wh, "Music");
    c.rect(wx + 10.0, top + 10.0, 60.0, 60.0, 6.0, Color::rgb(222, 80, 150));
    scribble(c, wx + 80.0, top + 14.0, ww - 96.0, ((wh - 40.0) / 11.0) as usize, LINE);
    if (1.2..6.6).contains(&t) {
        c.key_caps(WIDTH / 2.0, KEYS_Y, &[modifier.into()], true);
    }
    let (x, y) = if t < 4.0 {
        path(
            t,
            &[
                (0.0, 300.0, 230.0),
                (1.0, 150.0, 120.0),
                (1.8, 150.0, 120.0),
                (3.4, 300.0, 160.0),
            ],
        )
    } else {
        (wx + ww - 30.0, wy + wh - 30.0)
    };
    c.cursor(x, y);
    let hold = format!("Hold {modifier} and drag anywhere inside a window to move it");
    let resize = format!("{modifier} and the right button resizes it");
    c.steps(
        t,
        &[
            (0.0, &hold),
            (if s.resize { 3.8 } else { 99.0 }, &resize),
            (6.6, "No need to find the title bar or the edges"),
        ],
    );
}

fn window_hopper(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.window_hopper;
    c.desktop();
    let presses = [1.4, 3.4, 5.4];
    let front = presses.iter().filter(|&&p| t >= p + 0.3).count() % 3;
    let names = ["Notes - plan.txt", "Notes - todo.txt", "Notes - ideas.txt"];
    let at = |i: usize| (40.0 + i as f32 * 80.0, 26.0 + i as f32 * 26.0);
    for i in (0..3).filter(|&i| i != front).chain([front]) {
        let (x, y) = at(i);
        let top = c.window(x, y, 240.0, 150.0, names[i]);
        scribble(c, x + 12.0, top + 12.0, 210.0, 9, if i == front { MUTED } else { LINE });
        if i == front {
            c.outline(x, y, 240.0, 150.0, 6.0, ACCENT, 1.5);
        }
    }
    for p in presses {
        press(c, t, p, p + 0.3, &s.next_shortcut);
    }
    let next = format!("{} brings up the next window of the same app", named(&s.next_shortcut));
    let back = format!("{} goes the other way", named(&s.previous_shortcut));
    c.steps(
        t,
        &[(0.0, "Several windows of one app are open"), (1.0, &next), (5.0, &back)],
    );
}

fn uac(c: &mut Canvas, t: f32, a: f32, yes_at: f32) {
    if !(a..yes_at + 0.3).contains(&t) {
        return;
    }
    c.rect(0.0, 0.0, WIDTH, HEIGHT, 0.0, BLACK.alpha(0.5));
    c.rect(140.0, 70.0, 200.0, 110.0, 6.0, WINDOW);
    c.rect(140.0, 70.0, 200.0, 24.0, 6.0, Color::rgb(30, 60, 120));
    c.text(150.0, 82.0, "User Account Control", 7.5, WHITE);
    c.text(150.0, 108.0, "Do you want to allow this app", 7.5, TEXT);
    c.text(150.0, 120.0, "to make changes to your device?", 7.5, TEXT);
    c.button(196.0, 154.0, 64.0, "Yes", t >= yes_at);
    c.button(268.0, 154.0, 64.0, "No", false);
}

fn hosts_editor(c: &mut Canvas, t: f32, _: &Config) {
    c.desktop();
    let top = c.window(30.0, 16.0, 420.0, 226.0, "Hosts File Editor");
    let mut rows = vec![
        vec!["127.0.0.1".to_string(), "localhost".into(), "".into()],
        vec!["::1".into(), "localhost".into(), "".into()],
    ];
    let added = t >= 3.8;
    if added {
        rows.push(vec!["127.0.0.1".into(), "dev.local".into(), "My project".into()]);
    }
    table(
        c,
        38.0,
        top + 8.0,
        404.0,
        &[0.0, 120.0, 260.0],
        &["Address", "Host", "Comment"],
        &rows,
        added.then_some(2),
    );
    let fy = top + 150.0;
    let fresh = t < 3.8;
    c.field(
        38.0,
        fy,
        110.0,
        if fresh { typed("127.0.0.1", t, 1.0, 0.09) } else { "" },
        fresh && t < 2.0,
    );
    c.field(
        154.0,
        fy,
        120.0,
        if fresh { typed("dev.local", t, 2.1, 0.1) } else { "" },
        fresh && t >= 2.0,
    );
    c.button(282.0, fy, 50.0, "Add", (3.5..3.8).contains(&t));
    c.button(386.0, fy + 28.0, 56.0, "Save", (5.0..5.3).contains(&t));
    uac(c, t, 5.4, 6.6);
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 230.0),
            (2.8, 300.0, 230.0),
            (3.4, 307.0, fy + 8.0),
            (4.4, 307.0, fy + 8.0),
            (4.9, 414.0, fy + 36.0),
            (5.6, 414.0, fy + 36.0),
            (6.3, 228.0, 162.0),
        ],
    );
    click(c, t, 3.5, 307.0, fy + 8.0);
    click(c, t, 5.0, 414.0, fy + 36.0);
    click(c, t, 6.6, 228.0, 162.0);
    c.cursor(x, y);
    c.toast("Saved", span(t, 7.0, 9.6));
    c.steps(
        t,
        &[
            (0.0, "Type an address and a host name"),
            (3.4, "Add puts it in the list; switch entries off and on with a click"),
            (4.8, "Save, and Windows asks for permission once"),
            (7.0, "The hosts file is updated; no Notepad as administrator"),
        ],
    );
}

fn environment_variables(c: &mut Canvas, t: f32, _: &Config) {
    c.desktop();
    let top = c.window(30.0, 16.0, 420.0, 226.0, "Environment Variables");
    let changed = t >= 3.6;
    let saved = t >= 5.3;
    let editor = if changed { "code" } else { "vim" };
    let note = if changed && !saved { "Changed" } else { "" };
    let rows = vec![
        vec!["EDITOR".to_string(), editor.into(), note.into()],
        vec!["GOPATH".into(), "%USERPROFILE%\\go".into(), "".into()],
        vec!["Path".into(), "C:\\tools;C:\\bin".into(), "".into()],
        vec!["TEMP".into(), "%USERPROFILE%\\Temp".into(), "".into()],
    ];
    c.button(38.0, top + 6.0, 50.0, "User", true);
    c.button(92.0, top + 6.0, 50.0, "System", false);
    table(
        c,
        38.0,
        top + 28.0,
        404.0,
        &[0.0, 110.0, 330.0],
        &["Name", "Value", ""],
        &rows,
        (t >= 1.0).then_some(0),
    );
    let fy = top + 150.0;
    let sel = t >= 1.0;
    let value = if t < 2.0 { "vim" } else { typed("code", t, 2.2, 0.2) };
    c.field(38.0, fy, 100.0, if sel { "EDITOR" } else { "" }, false);
    c.field(144.0, fy, 160.0, if sel { value } else { "" }, (2.0..3.6).contains(&t));
    c.button(312.0, fy, 56.0, "Update", (3.4..3.6).contains(&t));
    c.button(386.0, fy + 28.0, 56.0, "Save", (5.0..5.3).contains(&t));
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 230.0),
            (0.8, 70.0, top + 52.0),
            (1.4, 70.0, top + 52.0),
            (1.9, 200.0, fy + 8.0),
            (2.8, 200.0, fy + 8.0),
            (3.3, 340.0, fy + 8.0),
            (4.4, 340.0, fy + 8.0),
            (4.9, 414.0, fy + 36.0),
        ],
    );
    click(c, t, 1.0, 70.0, top + 52.0);
    click(c, t, 3.4, 340.0, fy + 8.0);
    click(c, t, 5.0, 414.0, fy + 36.0);
    c.cursor(x, y);
    c.toast("Saved; programs started from now on see it", span(t, 5.3, 8.8));
    c.steps(
        t,
        &[
            (0.0, "Pick a variable for your account or the whole system"),
            (1.8, "Change its value; long values like Path are easy to read"),
            (3.6, "Edits are marked until you save"),
            (5.0, "Save writes them all at once"),
        ],
    );
}

fn registry_preview(c: &mut Canvas, t: f32, _: &Config) {
    c.desktop();
    let top = c.window(16.0, 12.0, 448.0, 232.0, "Registry Preview - theme.reg");
    let lines = [
        (0.2, "Windows Registry Editor Version 5.00"),
        (1.6, "[HKEY_CURRENT_USER\\Software\\Demo]"),
        (3.0, "\"Theme\"=\"dark\""),
        (4.0, "\"Size\"=dword:00000010"),
    ];
    c.rect(24.0, top + 6.0, 206.0, 170.0, 3.0, WHITE);
    c.outline(24.0, top + 6.0, 206.0, 170.0, 3.0, LINE, 1.0);
    for (i, (at, line)) in lines.iter().enumerate() {
        let shown = if i == 0 { line } else { typed(line, t, *at, 0.04) };
        let ly = top + 16.0 + i as f32 * 14.0 + if i > 0 { 14.0 } else { 0.0 };
        c.text(30.0, ly, shown, 6.0, TEXT);
    }
    let complete = |i: usize| t >= lines[i].0 + lines[i].1.chars().count() as f32 * 0.04;
    let mut rows: Vec<Vec<String>> = Vec::new();
    if complete(1) {
        rows.push(vec!["HKCU\\Software\\Demo".into(), "".into(), "".into()]);
    }
    if complete(2) {
        rows.push(vec!["  Theme".into(), "REG_SZ".into(), "dark".into()]);
    }
    if complete(3) {
        rows.push(vec!["  Size".into(), "REG_DWORD".into(), "16".into()]);
    }
    table(
        c,
        238.0,
        top + 6.0,
        218.0,
        &[0.0, 100.0, 164.0],
        &["Key / value", "Type", "Data"],
        &rows,
        None,
    );
    c.button(24.0, top + 186.0, 50.0, "Open", false);
    c.button(80.0, top + 186.0, 50.0, "Save", false);
    c.button(396.0, top + 186.0, 60.0, "Write", (6.4..6.7).contains(&t));
    let (x, y) = path(
        t,
        &[(0.0, 300.0, 240.0), (5.4, 300.0, 240.0), (6.2, 426.0, top + 194.0)],
    );
    click(c, t, 6.4, 426.0, top + 194.0);
    c.cursor(x, y);
    c.toast("Written to the registry", span(t, 6.8, 9.6));
    c.steps(
        t,
        &[
            (0.0, "Open a .reg file, or type one"),
            (1.6, "Each key and value shows up as it would be written"),
            (5.4, "Mistakes are pointed out before anything is written"),
            (
                6.4,
                "Write imports it; Windows asks first if it touches the whole system",
            ),
        ],
    );
}

fn new_plus(c: &mut Canvas, t: f32, _: &Config) {
    c.desktop();
    let top = c.window(24.0, 20.0, 280.0, 196.0, "Documents");
    file_row(c, 30.0, top + 6.0, 268.0, "Invoices", true, false);
    file_row(c, 30.0, top + 26.0, 268.0, "notes.txt", false, false);
    if t >= 3.7 {
        let name = if t < 4.2 {
            "Folder"
        } else {
            typed("Projects", t, 4.2, 0.18)
        };
        let editing = t < 6.2;
        let ry = top + 46.0;
        file_row(c, 30.0, ry, 268.0, "", true, !editing);
        if editing {
            c.field(52.0, ry + 1.0, 120.0, name, true);
            if t < 4.2 {
                c.rect(57.0, ry + 4.0, 26.0, 10.0, 1.0, ACCENT.alpha(0.35));
            }
        } else {
            c.text(54.0, ry + 9.0, name, 7.5, TEXT);
        }
    }
    let at = (150.0, top + 90.0);
    let items = ["View", "Sort by", "", "New+", "", "Properties"];
    let sub_y = at.1 + 3.0 + 3.0 * MENU_ROW - 3.0;
    if (1.3..3.6).contains(&t) {
        c.menu(at.0, at.1, 110.0, &items, (t >= 2.0).then_some(3));
        if t >= 2.4 {
            c.menu(
                at.0 + 108.0,
                sub_y,
                110.0,
                &["Folder", "Text document", "", "Open templates"],
                (t >= 3.0).then_some(0),
            );
        }
    }
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 230.0),
            (1.0, at.0, at.1),
            (1.4, at.0, at.1),
            (2.0, at.0 + 40.0, at.1 + 3.0 + 3.5 * MENU_ROW),
            (2.6, at.0 + 100.0, at.1 + 3.0 + 3.5 * MENU_ROW),
            (3.0, at.0 + 150.0, sub_y + 3.0 + 0.5 * MENU_ROW),
            (3.8, 300.0, 200.0),
        ],
    );
    click(c, t, 1.2, at.0, at.1);
    click(c, t, 3.4, at.0 + 150.0, sub_y + 3.0 + 0.5 * MENU_ROW);
    press(c, t, 6.0, 6.2, "Enter");
    c.cursor(x, y);
    c.steps(
        t,
        &[
            (0.0, "Right-click inside a folder"),
            (2.0, "New+ lists your templates: files and whole folder trees"),
            (3.7, "The copy is made and ready to rename"),
            (6.2, "Add your own templates with Open templates"),
        ],
    );
}

fn file_locksmith(c: &mut Canvas, t: f32, _: &Config) {
    c.desktop();
    let top = c.window(24.0, 20.0, 250.0, 196.0, "Documents");
    let file = (30.0, top + 26.0);
    file_row(c, 30.0, top + 6.0, 238.0, "Invoices", true, false);
    file_row(c, file.0, file.1, 238.0, "report.docx", false, (1.0..3.0).contains(&t));
    let at = (110.0, file.1 + 10.0);
    let unlock = (at.0 + 50.0, at.1 + 3.0 + 2.5 * MENU_ROW);
    if (1.3..2.9).contains(&t) {
        let items = ["Open", "Copy", "Unlock with PowerKit", "", "Properties"];
        c.menu(at.0, at.1, 130.0, &items, (t >= 2.0).then_some(2));
    }
    let (wx, wy, ww) = (196.0, 48.0, 264.0);
    let end = (wx + 46.0, wy + 136.0);
    let second = (wx + 60.0, wy + 70.0);
    if t >= 2.9 {
        let inner = c.window(wx, wy, ww, 170.0, "File Locksmith - report.docx");
        let mut rows = Vec::new();
        if t < 4.9 {
            rows.push(vec!["Word".to_string(), "4120".into(), "report.docx".into()]);
        }
        if t < 6.9 {
            rows.push(vec!["Backup".to_string(), "2208".into(), "report.docx".into()]);
        }
        let selected = ((3.8..4.9).contains(&t) || (5.8..6.9).contains(&t)).then_some(0);
        table(
            c,
            wx + 6.0,
            inner + 6.0,
            ww - 12.0,
            &[0.0, 90.0, 140.0],
            &["Process", "PID", "Using"],
            &rows,
            selected,
        );
        c.button(
            end.0 - 30.0,
            end.1 - 8.0,
            60.0,
            "End task",
            (4.6..4.9).contains(&t) || (6.6..6.9).contains(&t),
        );
        c.button(wx + ww - 66.0, end.1 - 8.0, 60.0, "Refresh", false);
        let status = if t >= 6.9 {
            "No process is using report.docx."
        } else if t >= 4.9 {
            "1 process is using report.docx."
        } else {
            "2 processes are using report.docx."
        };
        c.text(wx + 8.0, end.1 + 18.0, status, 7.0, MUTED);
    }
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 230.0),
            (1.0, at.0, at.1),
            (1.4, at.0, at.1),
            (2.2, unlock.0, unlock.1),
            (2.8, unlock.0, unlock.1),
            (3.6, wx + 40.0, wy + 54.0),
            (4.2, end.0, end.1),
            (4.8, end.0, end.1),
            (5.6, second.0, wy + 54.0),
            (6.2, end.0, end.1),
            (7.0, end.0, end.1),
            (7.8, 380.0, 240.0),
        ],
    );
    click(c, t, 1.1, at.0, at.1);
    click(c, t, 2.6, unlock.0, unlock.1);
    click(c, t, 3.7, wx + 40.0, wy + 54.0);
    click(c, t, 4.6, end.0, end.1);
    click(c, t, 5.6, second.0, wy + 54.0);
    click(c, t, 6.6, end.0, end.1);
    c.cursor(x, y);
    c.steps(
        t,
        &[
            (0.0, "A file will not delete or move because something has it open"),
            (1.3, "Right-click it, or a folder, and choose Unlock with PowerKit"),
            (2.9, "Every process using it is listed"),
            (3.7, "Select one and choose End task; unsaved work in it is lost"),
            (6.9, "The file is free"),
        ],
    );
}

fn image_resizer(c: &mut Canvas, t: f32, cfg: &Config) {
    let r = &cfg.image_resizer;
    let num = |v: f64| {
        if v > 0.0 {
            format!("{}", (v * 100.0).round() / 100.0)
        } else {
            "auto".into()
        }
    };
    let rows: Vec<Vec<String>> = r
        .sizes
        .iter()
        .take(4)
        .map(|s| vec![s.name.clone(), num(s.width), num(s.height)])
        .collect();
    // The second size is picked, or the first when only one is set up.
    let pick = rows.len().min(2).saturating_sub(1);
    let picked = rows.get(pick).map_or("Medium".to_string(), |r| r[0].clone());
    c.desktop();
    let top = c.window(24.0, 20.0, 250.0, 196.0, "Pictures");
    let selected = (0.6..3.0).contains(&t);
    file_row(c, 30.0, top + 6.0, 238.0, "beach.jpg", false, selected);
    file_row(c, 30.0, top + 26.0, 238.0, "garden.png", false, selected);
    if t >= 5.6 && !r.replace_originals {
        let saved = |name: &str, ext: &str| {
            let stem = r.file_name.replace("%1", name).replace("%2", &picked);
            format!("{stem}.{ext}")
        };
        file_row(c, 30.0, top + 46.0, 238.0, &saved("beach", "jpg"), false, false);
        file_row(c, 30.0, top + 66.0, 238.0, &saved("garden", "png"), false, false);
    }
    let at = (110.0, top + 36.0);
    let resize = (at.0 + 50.0, at.1 + 3.0 + 2.5 * MENU_ROW);
    if (1.3..2.9).contains(&t) {
        let items = ["Open", "Copy", "Resize with PowerKit", "", "Properties"];
        c.menu(at.0, at.1, 130.0, &items, (t >= 2.0).then_some(2));
    }
    let (wx, wy, ww) = (200.0, 44.0, 260.0);
    let inner = wy + TITLE_H;
    let row = (wx + 50.0, inner + 6.0 + 16.0 + 16.0 * pick as f32 + 8.0);
    let go = (wx + ww - 40.0, wy + 156.0);
    if (2.9..6.6).contains(&t) {
        c.window(wx, wy, ww, 180.0, "Image Resizer - 2 pictures");
        let chosen = if t >= 3.8 { pick } else { 0 };
        table(
            c,
            wx + 6.0,
            inner + 6.0,
            ww - 12.0,
            &[0.0, 120.0, 180.0],
            &["Size", "Width", "Height"],
            &rows,
            (!rows.is_empty()).then_some(chosen),
        );
        c.button(wx + 8.0, go.1 - 8.0, 60.0, "Add size", false);
        c.button(go.0 - 30.0, go.1 - 8.0, 60.0, "Resize", (4.6..4.9).contains(&t));
        let status = if t >= 5.6 {
            "Resized 2 pictures."
        } else if t >= 4.9 {
            "Resizing 2 pictures..."
        } else {
            "Choose a size, then click Resize."
        };
        c.text(wx + 8.0, go.1 + 18.0, status, 7.0, MUTED);
    }
    let (x, y) = path(
        t,
        &[
            (0.0, 300.0, 230.0),
            (1.0, at.0, at.1),
            (1.4, at.0, at.1),
            (2.2, resize.0, resize.1),
            (2.8, resize.0, resize.1),
            (3.6, row.0, row.1),
            (4.2, go.0, go.1),
            (5.0, go.0, go.1),
            (6.0, 150.0, top + 90.0),
            (8.0, 150.0, top + 90.0),
        ],
    );
    click(c, t, 1.1, at.0, at.1);
    click(c, t, 2.6, resize.0, resize.1);
    click(c, t, 3.7, row.0, row.1);
    click(c, t, 4.6, go.0, go.1);
    c.cursor(x, y);
    c.steps(
        t,
        &[
            (0.0, "Select pictures in Explorer"),
            (1.3, "Right-click them and choose Resize with PowerKit"),
            (2.9, "Pick a size, or type your own in the boxes under the list"),
            (4.6, "Click Resize"),
            (
                5.6,
                if r.replace_originals {
                    "The pictures are replaced by the resized ones"
                } else {
                    "The resized copies are saved next to the originals"
                },
            ),
        ],
    );
}

fn text_extractor(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.text_extractor;
    let lines = ["Meeting moved to Thursday", "Room 4B, 10:30 am", "Bring the Q3 numbers"];
    c.desktop();
    let top = c.window(20.0, 20.0, 260.0, 150.0, "Photo of the whiteboard");
    for (i, l) in lines.iter().enumerate() {
        c.text(40.0, top + 30.0 + i as f32 * 22.0, l, 9.0, TEXT);
    }
    press(c, t, 0.5, 1.1, &s.shortcut);
    let a = (32.0, top + 14.0);
    let b = (250.0, top + 90.0);
    let (bx, by) = path(t, &[(0.0, a.0, a.1), (1.8, a.0, a.1), (3.4, b.0, b.1)]);
    if (1.2..3.8).contains(&t) {
        c.rect(0.0, 0.0, WIDTH, HEIGHT, 0.0, BLACK.alpha(0.25));
        if t >= 1.8 {
            c.rect(a.0, a.1, bx - a.0, by - a.1, 0.0, WHITE.alpha(0.2));
            c.outline(a.0, a.1, bx - a.0, by - a.1, 0.0, WHITE, 1.5);
        }
    }
    let notes = (240.0, 110.0);
    if t >= 4.4 {
        let wt = c.window(notes.0, notes.1, 220.0, 110.0, "Notes");
        if t >= 5.6 {
            if s.single_line {
                c.text(notes.0 + 8.0, wt + 14.0, &lines.join(" "), 6.0, TEXT);
            } else {
                for (i, l) in lines.iter().enumerate() {
                    c.text(notes.0 + 8.0, wt + 14.0 + i as f32 * 14.0, l, 7.0, TEXT);
                }
            }
        }
    }
    press(c, t, 5.0, 5.5, "Ctrl+V");
    let (x, y) = if t < 1.8 {
        path(t, &[(0.0, 300.0, 230.0), (1.4, a.0, a.1)])
    } else if t < 4.0 {
        (bx, by)
    } else {
        path(t, &[(4.0, bx, by), (4.8, notes.0 + 60.0, notes.1 + 50.0)])
    };
    c.cursor(x, y);
    let start = format!("Press {}", named(&s.shortcut));
    c.steps(
        t,
        &[
            (0.0, &start),
            (1.6, "Drag a box around the text you want"),
            (3.8, "Windows reads the text and it is copied"),
            (5.0, "Paste it anywhere"),
        ],
    );
}

fn power_display(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.power_display;
    let level = 0.35 + 0.5 * ease(span(t, 3.4, 5.4));
    c.desktop();
    let top = c.window(20.0, 20.0, 230.0, 130.0, "Photos");
    scribble(c, 34.0, top + 12.0, 200.0, 5, LINE);
    // The whole screen stands in for the monitor whose brightness changes.
    c.rect(0.0, 0.0, WIDTH, HEIGHT - 18.0, 0.0, BLACK.alpha(0.5 * (1.0 - level)));
    press(c, t, 0.5, 1.1, &s.shortcut);
    let mut rows = vec![("Brightness", level)];
    if s.contrast {
        rows.push(("Contrast", 0.75));
    }
    if s.volume {
        rows.push(("Volume", 0.3));
    }
    let (x, w) = (262.0, 200.0);
    let h = 44.0 + rows.len() as f32 * 24.0;
    let y = HEIGHT - 26.0 - h;
    let thumb = (x + 70.0 + 96.0 * level, y + TITLE_H + 30.5);
    if t >= 1.2 {
        let wt = c.window(x, y, w, h, "Power Display");
        c.bold(x + 10.0, wt + 8.0, "Acer EK221Q", 8.0, TEXT);
        for (i, (label, v)) in rows.iter().enumerate() {
            let ry = wt + 30.0 + i as f32 * 24.0;
            c.text(x + 10.0, ry - 4.0, label, 7.0, TEXT);
            let (bx, bw) = (x + 70.0, 96.0);
            c.rect(bx, ry - 1.0, bw, 3.0, 1.5, LINE);
            c.rect(bx, ry - 1.0, bw * v, 3.0, 1.5, ACCENT);
            c.circle(bx + bw * v, ry + 0.5, 5.0, ACCENT);
            c.text(x + w - 24.0, ry - 4.0, &format!("{}", (v * 100.0).round()), 7.0, MUTED);
        }
    }
    let (cx, cy) = if t < 3.0 {
        path(t, &[(0.0, 200.0, 200.0), (2.6, thumb.0, thumb.1)])
    } else {
        thumb
    };
    c.cursor(cx, cy);
    let start = format!("Press {}", named(&s.shortcut));
    c.steps(
        t,
        &[
            (0.0, &start),
            (1.4, "Each monitor gets its own sliders"),
            (3.2, "Drag to set the monitor's own brightness"),
            (6.0, "Input and power can be added in the settings"),
        ],
    );
}

fn screen_ruler(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.screen_ruler;
    let color = hex(&s.line_color, Color::rgb(255, 69, 0));
    c.desktop();
    let top = c.window(20.0, 20.0, 280.0, 160.0, "Design mockup");
    let (bx, by, bw, bh) = (70.0, top + 24.0, 150.0, 70.0);
    c.rect(bx, by, bw, bh, 0.0, Color::rgb(120, 170, 230));
    press(c, t, 0.5, 1.1, &s.shortcut);
    let (x, y) = path(t, &[(0.0, 330.0, 220.0), (1.4, 150.0, by + 40.0)]);
    // The mode the wheel turned to at 4.2 seconds.
    let both = t < 4.2;
    if (1.4..7.6).contains(&t) {
        let foot = if s.draw_feet { 4.0 } else { 0.0 };
        c.line(bx, y, bx + bw, y, color, 1.5);
        if foot > 0.0 {
            c.line(bx, y - foot, bx, y + foot, color, 1.5);
            c.line(bx + bw, y - foot, bx + bw, y + foot, color, 1.5);
        }
        if both {
            c.line(x, by, x, by + bh, color, 1.5);
            if foot > 0.0 {
                c.line(x - foot, by, x + foot, by, color, 1.5);
                c.line(x - foot, by + bh, x + foot, by + bh, color, 1.5);
            }
        }
        let label = if both { "150 \u{00D7} 70 px" } else { "150 px" };
        c.rect(x + 10.0, y + 10.0, 78.0, 20.0, 3.0, Color::rgb(32, 32, 32));
        c.text(x + 16.0, y + 15.0, label, 8.0, WHITE);
    }
    click(c, t, 5.4, x, y);
    c.cursor(x, y);
    let start = format!("Press {}", named(&s.shortcut));
    c.steps(
        t,
        &[
            (0.0, &start),
            (1.6, "The lines stop where the color changes"),
            (3.6, "Turn the wheel to measure only across or down"),
            (5.2, "Click to copy the size; Escape stops"),
        ],
    );
}

fn zoom_it(c: &mut Canvas, t: f32, cfg: &Config) {
    let s = &cfg.zoom_it;
    let pen = hex(&s.pen_color, Color::rgb(255, 0, 0));
    let zoom = 1.0 + (s.zoom_percent as f32 / 100.0 - 1.0) * ease(span(t, 1.2, 2.2));
    c.desktop();
    // Zooming enlarges the window around the point the cursor rests on.
    let (fx, fy) = (150.0, 90.0);
    let at = |x: f32, y: f32| (fx + (x - fx) * zoom, fy + (y - fy) * zoom);
    let (wx, wy) = at(40.0, 40.0);
    let top = c.window(wx, wy, 220.0 * zoom, 120.0 * zoom, "Quarterly results");
    let bar = |c: &mut Canvas, x: f32, h: f32, col: Color| {
        let (bx, by) = at(x, 150.0 - h);
        c.rect(bx, by.max(top), 18.0 * zoom, h * zoom, 0.0, col);
    };
    bar(c, 70.0, 40.0, Color::rgb(120, 170, 230));
    bar(c, 100.0, 60.0, Color::rgb(120, 170, 230));
    bar(c, 130.0, 80.0, Color::rgb(120, 170, 230));
    bar(c, 160.0, 55.0, Color::rgb(230, 140, 120));
    press(c, t, 0.5, 1.0, &s.zoom_shortcut);
    // Circling the odd bar once drawing starts.
    let (cx, cy) = at(169.0, 120.0);
    let r = 22.0;
    let p = span(t, 4.0, 6.0);
    if t >= 3.4 {
        let steps = (p * 32.0) as usize;
        for i in 0..steps {
            let (a0, a1) = (
                i as f32 / 32.0 * std::f32::consts::TAU,
                (i + 1) as f32 / 32.0 * std::f32::consts::TAU,
            );
            c.line(
                cx + r * a0.cos(),
                cy + r * a0.sin(),
                cx + r * a1.cos(),
                cy + r * a1.sin(),
                pen,
                3.0,
            );
        }
    }
    let (x, y) = if t < 4.0 {
        path(
            t,
            &[(0.0, 300.0, 220.0), (1.0, fx, fy), (3.0, fx, fy), (3.8, cx + r, cy)],
        )
    } else {
        let a = p * std::f32::consts::TAU;
        (cx + r * a.cos(), cy + r * a.sin())
    };
    click(c, t, 3.2, x, y);
    c.cursor(x, y);
    let start = format!("Press {}", named(&s.zoom_shortcut));
    c.steps(
        t,
        &[
            (0.0, &start),
            (1.4, "Turn the wheel to zoom further in or out"),
            (3.0, "Click to draw on what is shown"),
            (6.4, "Right-click to zoom again; Escape ends"),
        ],
    );
}
