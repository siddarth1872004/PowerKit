//! Where everything goes: the navigation pane and the page are laid out into lists
//! of nodes, each with a rectangle, what to draw there and what a click on it does.
//! Painting and hit-testing both read these lists.

use super::model::{Control, Item, NavEntry, Page, Severity};
use super::painter::{Align, Face, Measure, Rect, Style};
use std::collections::HashSet;

// Sizes in 96-DPI pixels, after the Windows App SDK settings controls.
pub(crate) const NAV_W: f32 = 280.0;
pub(crate) const NAV_COMPACT_W: f32 = 48.0;
/// Below this window width the navigation pane shows icons only.
pub(crate) const COMPACT_BELOW: f32 = 820.0;
const NAV_ROW: f32 = 36.0;
const NAV_GAP: f32 = 4.0;
const NAV_TOP: f32 = 8.0;
const NAV_INDENT: f32 = 28.0;
const PAD_X: f32 = 40.0;
const MAX_W: f32 = 1000.0;
const TITLE_TOP: f32 = 24.0;
const TITLE_H: f32 = 44.0;
const HERO_GAP: f32 = 24.0;
const HERO_MAX_W: f32 = 400.0;
const GROUP_TOP: f32 = 30.0;
const GROUP_H: f32 = 20.0;
const GROUP_GAP: f32 = 8.0;
const CARD_GAP: f32 = 4.0;
const CARD_MIN_H: f32 = 68.0;
const CARD_PAD: f32 = 16.0;
const ICON: f32 = 20.0;
const TILE_W: f32 = 148.0;
const TILE_H: f32 = 120.0;
const TILE_GAP: f32 = 12.0;
const ROW_MIN_H: f32 = 52.0;
const CHILD_INDENT: f32 = 58.0;
const CONTROL_H: f32 = 32.0;
const TEXT_GAP: f32 = 24.0;
const BOTTOM: f32 = 40.0;
pub(crate) const RADIUS: f32 = 4.0;
pub(crate) const TITLE_PX: f32 = 28.0;
pub(crate) const BODY_PX: f32 = 14.0;
pub(crate) const CAPTION_PX: f32 = 12.0;
pub(crate) const KEY_PX: f32 = 14.0;
const KEY_PAD: f32 = 10.0;
const KEY_GAP: f32 = 4.0;
pub(crate) const DIALOG_W: f32 = 460.0;

/// Which item of the page: group, item, and child row when it is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ItemRef {
    pub group: usize,
    pub item: usize,
    pub child: Option<usize>,
}

impl ItemRef {
    pub fn get<'p>(&self, page: &'p Page) -> Option<&'p Item> {
        let item = page.groups.get(self.group)?.items.get(self.item)?;
        match self.child {
            Some(c) => item.children.get(c),
            None => Some(item),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Toggle,
    Expand,
    Shortcut,
    Choice,
    Color,
    Button,
    Link,
    Field,
    /// The option at this index of a [`Control::Layouts`].
    Pick(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogButton {
    Save,
    Reset,
    Clear,
    Cancel,
}

/// What a click does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hit {
    Nav(usize),
    Item(ItemRef, Part),
    CloseInfo,
    Dialog(DialogButton),
}

/// What is drawn.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Look {
    NavRow {
        entry: usize,
        selected: bool,
        compact: bool,
    },
    /// `style.color` names a palette color: 0 disabled, 1 text, 2 secondary text.
    Text {
        text: String,
        style: Style,
    },
    Card,
    Divider,
    /// Hover feedback over a whole clickable card.
    CardHover,
    ItemIcon(ItemRef),
    Toggle {
        on: bool,
        enabled: bool,
    },
    Keys {
        keys: Vec<String>,
        enabled: bool,
        large: bool,
    },
    EditButton {
        enabled: bool,
    },
    Combo {
        label: String,
        enabled: bool,
    },
    Field(ItemRef),
    Swatch {
        color: u32,
        enabled: bool,
    },
    Button {
        label: String,
        accent: bool,
        enabled: bool,
    },
    Chevron {
        glyph: char,
        enabled: bool,
    },
    LayoutTile {
        name: String,
        zones: Vec<[f32; 4]>,
        selected: bool,
        enabled: bool,
    },
    Info {
        severity: Severity,
    },
    Dialog,
    DialogFooter,
    InfoGlyph {
        severity: Severity,
    },
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub rect: Rect,
    pub look: Look,
    pub hit: Option<Hit>,
}

/// Everything the layout depends on besides the text measurer.
pub(crate) struct Input<'a> {
    pub nav: &'a [NavEntry],
    pub folded: &'a HashSet<usize>,
    pub selected: usize,
    pub page: &'a Page,
    pub expanded: &'a HashSet<String>,
    pub info: Option<&'a (Severity, String)>,
    /// Height for a walkthrough of the given width, when the page has one.
    pub guide: Option<&'a dyn Fn(f32) -> f32>,
    pub width: f32,
    /// Pixels per 96-DPI pixel.
    pub k: f32,
}

#[derive(Debug, Default)]
pub(crate) struct Layout {
    pub nav_w: f32,
    pub nav: Vec<Node>,
    pub nav_height: f32,
    pub content: Vec<Node>,
    pub content_height: f32,
    /// Where the walkthrough goes, in content coordinates.
    pub guide: Option<Rect>,
    /// Text boxes, in content coordinates, for the edit controls.
    pub fields: Vec<(ItemRef, Rect)>,
}

pub(crate) fn style(face: Face, px: f32, color: u32) -> Style {
    Style {
        face,
        px,
        color,
        align: Align::Left,
        wrap: false,
    }
}

/// The parts of a shortcut as they appear on key caps.
pub(crate) fn key_caps(shortcut: &str) -> Vec<String> {
    shortcut
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match p {
            "Win" => "\u{229E}".to_string(),
            "Left" => "\u{2190}".to_string(),
            "Right" => "\u{2192}".to_string(),
            "Up" => "\u{2191}".to_string(),
            "Down" => "\u{2193}".to_string(),
            "Backtick" => "`".to_string(),
            "Slash" => "/".to_string(),
            "Period" => ".".to_string(),
            "Comma" => ",".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Width of key caps for `keys` laid side by side.
pub(crate) fn keys_width(m: &mut dyn Measure, keys: &[String], px: f32, k: f32) -> f32 {
    let st = style(Face::Strong, px * k, 0);
    let mut w = 0.0;
    for key in keys {
        let (tw, _) = m.measure(key, &st, 0.0);
        w += (tw + 2.0 * KEY_PAD * k).max(CONTROL_H * k) + KEY_GAP * k;
    }
    (w - KEY_GAP * k).max(0.0)
}

pub(crate) fn layout(input: &Input, m: &mut dyn Measure) -> Layout {
    let k = input.k;
    let compact = input.width < COMPACT_BELOW * k;
    let nav_w = if compact { NAV_COMPACT_W } else { NAV_W } * k;
    let mut out = Layout {
        nav_w,
        ..Default::default()
    };
    layout_nav(input, compact, nav_w, &mut out);
    layout_page(input, m, nav_w, &mut out);
    out
}

fn layout_nav(input: &Input, compact: bool, nav_w: f32, out: &mut Layout) {
    let k = input.k;
    let mut y = NAV_TOP * k;
    let mut hidden = false;
    for (i, entry) in input.nav.iter().enumerate() {
        let indent = match entry {
            NavEntry::Category { .. } => {
                hidden = false;
                0.0
            }
            NavEntry::Page { depth, .. } => {
                if hidden && *depth > 0 {
                    continue;
                }
                if compact {
                    0.0
                } else {
                    f32::from(*depth) * NAV_INDENT * k
                }
            }
        };
        if let NavEntry::Category { .. } = entry {
            hidden = input.folded.contains(&i);
            if compact {
                // Categories cannot be folded in the narrow pane; a thin rule stands in.
                out.nav.push(Node {
                    rect: Rect::new(8.0 * k, y + 2.0 * k, nav_w - 16.0 * k, k),
                    look: Look::Divider,
                    hit: None,
                });
                y += 5.0 * k;
                hidden = false;
                continue;
            }
        }
        let rect = Rect::new(4.0 * k + indent, y, nav_w - 8.0 * k - indent, NAV_ROW * k);
        out.nav.push(Node {
            rect,
            look: Look::NavRow {
                entry: i,
                selected: i == input.selected,
                compact,
            },
            hit: Some(Hit::Nav(i)),
        });
        y += (NAV_ROW + NAV_GAP) * k;
    }
    out.nav_height = y + NAV_TOP * k;
}

/// Width of the control on the right of a card, or 0 when the control sits on its
/// own line.
fn control_width(m: &mut dyn Measure, c: &Control, k: f32) -> f32 {
    let body = style(Face::Text, BODY_PX * k, 0);
    match c {
        Control::None => 0.0,
        Control::Toggle(_) => {
            let label = ["On", "Off"]
                .iter()
                .map(|s| m.measure(s, &body, 0.0).0)
                .fold(0.0, f32::max);
            label + 12.0 * k + 40.0 * k
        }
        Control::Shortcut { value, .. } => {
            let keys = key_caps(value);
            let w = if keys.is_empty() {
                m.measure("Not set", &body, 0.0).0
            } else {
                keys_width(m, &keys, KEY_PX, k)
            };
            w + 8.0 * k + CONTROL_H * k
        }
        Control::Choice { options, .. } => {
            let widest = options
                .iter()
                .map(|(_, l)| m.measure(l, &body, 0.0).0)
                .fold(0.0, f32::max);
            (widest + 12.0 * k + 36.0 * k).clamp(160.0 * k, 320.0 * k)
        }
        Control::Text { wide: true, .. } | Control::Layouts { .. } => 0.0,
        Control::Text { .. } => 240.0 * k,
        Control::Number { .. } => 140.0 * k,
        Control::Color(_) => 72.0 * k,
        Control::Button { label, .. } => (m.measure(label, &body, 0.0).0 + 24.0 * k).max(120.0 * k),
    }
}

fn parse_color(s: &str) -> u32 {
    let hex = s.trim().trim_start_matches('#');
    match u32::from_str_radix(hex, 16) {
        Ok(v) if hex.len() == 6 => 0xFF00_0000 | v,
        _ => 0,
    }
}

/// Nodes for `c` drawn in `r` (the control's box, right-aligned in its row).
fn control_nodes(c: &Control, r: Rect, at: ItemRef, enabled: bool, k: f32, out: &mut Layout) {
    let hit = |part| enabled.then_some(Hit::Item(at, part));
    let mid = r.y + (r.h - CONTROL_H * k) / 2.0;
    let boxed = Rect::new(r.x, mid, r.w, CONTROL_H * k);
    match c {
        Control::None => {}
        Control::Toggle(on) => out.content.push(Node {
            rect: boxed,
            look: Look::Toggle { on: *on, enabled },
            hit: hit(Part::Toggle),
        }),
        Control::Shortcut { value, .. } => {
            let keys = key_caps(value);
            let edit = Rect::new(r.right() - CONTROL_H * k, mid, CONTROL_H * k, CONTROL_H * k);
            let caps = Rect::new(r.x, mid, r.w - CONTROL_H * k - 8.0 * k, CONTROL_H * k);
            if keys.is_empty() {
                let mut st = style(Face::Text, BODY_PX * k, if enabled { 2 } else { 0 });
                st.align = Align::Right;
                out.content.push(Node {
                    rect: caps,
                    look: Look::Text {
                        text: "Not set".into(),
                        style: st,
                    },
                    hit: hit(Part::Shortcut),
                });
            } else {
                out.content.push(Node {
                    rect: caps,
                    look: Look::Keys {
                        keys,
                        enabled,
                        large: false,
                    },
                    hit: hit(Part::Shortcut),
                });
            }
            out.content.push(Node {
                rect: edit,
                look: Look::EditButton { enabled },
                hit: hit(Part::Shortcut),
            });
        }
        Control::Choice { options, selected } => {
            let label = options
                .iter()
                .find(|(v, _)| v == selected)
                .map(|(_, l)| l.clone())
                .unwrap_or_else(|| selected.clone());
            out.content.push(Node {
                rect: boxed,
                look: Look::Combo { label, enabled },
                hit: hit(Part::Choice),
            });
        }
        Control::Text { .. } | Control::Number { .. } => {
            // A wide box fills the space under the title; others sit in the row.
            let wide = matches!(c, Control::Text { wide: true, .. });
            let rect = if wide { r } else { boxed };
            out.content.push(Node {
                rect,
                look: Look::Field(at),
                hit: None,
            });
            out.fields.push((at, rect));
        }
        Control::Color(c) => out.content.push(Node {
            rect: boxed,
            look: Look::Swatch {
                color: parse_color(c),
                enabled,
            },
            hit: hit(Part::Color),
        }),
        Control::Button { label, accent } => out.content.push(Node {
            rect: boxed,
            look: Look::Button {
                label: label.clone(),
                accent: *accent,
                enabled,
            },
            hit: hit(Part::Button),
        }),
        Control::Layouts { options, selected } => {
            let cols = tile_columns(r.w, k);
            for (i, o) in options.iter().enumerate() {
                let (col, line) = ((i % cols) as f32, (i / cols) as f32);
                out.content.push(Node {
                    rect: Rect::new(
                        r.x + col * (TILE_W + TILE_GAP) * k,
                        r.y + line * (TILE_H + TILE_GAP) * k,
                        TILE_W * k,
                        TILE_H * k,
                    ),
                    look: Look::LayoutTile {
                        name: o.name.clone(),
                        zones: o.zones.clone(),
                        selected: o.id == *selected,
                        enabled,
                    },
                    hit: hit(Part::Pick(i)),
                });
            }
        }
    }
}

/// How many layout pictures fit across `w`.
fn tile_columns(w: f32, k: f32) -> usize {
    (((w + TILE_GAP * k) / ((TILE_W + TILE_GAP) * k)) as usize).max(1)
}

/// Lay out one row (a card's header or an expanded child) whose content starts at
/// `left` and ends at `right`; returns its height.
#[allow(clippy::too_many_arguments)]
fn row(
    item: &Item,
    at: ItemRef,
    enabled: bool,
    left: f32,
    right: f32,
    y: f32,
    min_h: f32,
    m: &mut dyn Measure,
    k: f32,
    out: &mut Layout,
) -> f32 {
    let has_icon = item.icon.is_some() || item.glyph.is_some();
    let text_x = if has_icon { left + ICON * k + 20.0 * k } else { left };
    let cw = control_width(m, &item.control, k);
    let text_w = (right - text_x - if cw > 0.0 { cw + TEXT_GAP * k } else { 0.0 }).max(40.0 * k);
    let title = style(Face::Text, BODY_PX * k, 0);
    let desc = Style {
        wrap: true,
        ..style(Face::Text, CAPTION_PX * k, 0)
    };
    let (_, th) = m.measure(&item.title, &title, text_w);
    let th = th.max(20.0 * k);
    let dh = if item.description.is_empty() {
        0.0
    } else {
        m.measure(&item.description, &desc, text_w).1
    };
    let wide = matches!(item.control, Control::Text { wide: true, .. } | Control::Layouts { .. });
    let wide_h = match &item.control {
        Control::Text { lines, wide: true, .. } => (f32::from((*lines).max(1)) * 20.0 + 12.0) * k,
        Control::Layouts { options, .. } => {
            let lines = options.len().div_ceil(tile_columns(right - text_x, k)).max(1) as f32;
            lines * (TILE_H + TILE_GAP) * k - TILE_GAP * k
        }
        _ => 0.0,
    };
    let text_h = th + dh;
    let pad_y = 12.0 * k;
    let head_h = (text_h + 2.0 * pad_y).max(min_h);
    let h = if wide {
        pad_y + text_h + 8.0 * k + wide_h + pad_y
    } else {
        head_h
    };
    let text_top = if wide { y + pad_y } else { y + (head_h - text_h) / 2.0 };
    if has_icon {
        out.content.push(Node {
            rect: Rect::new(left, text_top + (th - ICON * k) / 2.0, ICON * k, ICON * k),
            look: Look::ItemIcon(at),
            hit: None,
        });
    }
    let ink = if enabled { 1 } else { 0 };
    out.content.push(Node {
        rect: Rect::new(text_x, text_top, text_w, th),
        look: Look::Text {
            text: item.title.clone(),
            style: Style { color: ink, ..title },
        },
        hit: None,
    });
    if dh > 0.0 {
        out.content.push(Node {
            rect: Rect::new(text_x, text_top + th, text_w, dh),
            look: Look::Text {
                text: item.description.clone(),
                style: Style { color: 2 * ink, ..desc },
            },
            hit: None,
        });
    }
    if wide {
        let fy = text_top + text_h + 8.0 * k;
        control_nodes(
            &item.control,
            Rect::new(text_x, fy, right - text_x, wide_h),
            at,
            enabled,
            k,
            out,
        );
    } else if cw > 0.0 {
        control_nodes(&item.control, Rect::new(right - cw, y, cw, head_h), at, enabled, k, out);
    }
    h
}

fn layout_page(input: &Input, m: &mut dyn Measure, nav_w: f32, out: &mut Layout) {
    let k = input.k;
    let page = input.page;
    let avail = input.width - nav_w;
    let w = (avail - 2.0 * PAD_X * k).min(MAX_W * k).max(200.0 * k);
    let x0 = nav_w + PAD_X * k;
    let mut y = TITLE_TOP * k;
    out.content.push(Node {
        rect: Rect::new(x0, y, w, TITLE_H * k),
        look: Look::Text {
            text: page.title.clone(),
            style: style(Face::Title, TITLE_PX * k, 1),
        },
        hit: None,
    });
    y += TITLE_H * k + 12.0 * k;

    // The walkthrough with the description beside it, or under it when narrow.
    let desc_style = Style {
        wrap: true,
        ..style(Face::Text, BODY_PX * k, 2)
    };
    match input.guide {
        Some(height_for) => {
            let side = w >= 640.0 * k;
            let gw = if side {
                (w * 0.5).min(HERO_MAX_W * k)
            } else {
                w.min(HERO_MAX_W * k)
            };
            let gh = height_for(gw);
            out.guide = Some(Rect::new(x0, y, gw, gh));
            let (dx, dy, dw) = if side {
                (x0 + gw + HERO_GAP * k, y, w - gw - HERO_GAP * k)
            } else {
                (x0, y + gh + 16.0 * k, w)
            };
            let (_, dh) = m.measure(&page.description, &desc_style, dw);
            let mut st = desc_style;
            st.color = 1;
            out.content.push(Node {
                rect: Rect::new(dx, dy, dw, dh),
                look: Look::Text {
                    text: page.description.clone(),
                    style: st,
                },
                hit: None,
            });
            y = if side { (y + gh).max(dy + dh) } else { dy + dh };
        }
        None if !page.description.is_empty() => {
            let (_, dh) = m.measure(&page.description, &desc_style, w);
            out.content.push(Node {
                rect: Rect::new(x0, y, w, dh),
                look: Look::Text {
                    text: page.description.clone(),
                    style: desc_style,
                },
                hit: None,
            });
            y += dh;
        }
        None => {}
    }

    if let Some((severity, text)) = input.info {
        y += 16.0 * k;
        let text_x = x0 + 48.0 * k;
        let text_w = w - 48.0 * k - 48.0 * k;
        let st = Style {
            wrap: true,
            ..style(Face::Text, BODY_PX * k, 1)
        };
        let (_, th) = m.measure(text, &st, text_w);
        let h = (th + 28.0 * k).max(48.0 * k);
        out.content.push(Node {
            rect: Rect::new(x0, y, w, h),
            look: Look::Info { severity: *severity },
            hit: None,
        });
        out.content.push(Node {
            rect: Rect::new(x0 + 16.0 * k, y, 20.0 * k, 48.0 * k),
            look: Look::InfoGlyph { severity: *severity },
            hit: None,
        });
        out.content.push(Node {
            rect: Rect::new(text_x, y + (h - th) / 2.0, text_w, th),
            look: Look::Text {
                text: text.clone(),
                style: st,
            },
            hit: None,
        });
        out.content.push(Node {
            rect: Rect::new(x0 + w - 40.0 * k, y + 8.0 * k, 32.0 * k, 32.0 * k),
            look: Look::Close,
            hit: Some(Hit::CloseInfo),
        });
        y += h;
    }

    for (gi, group) in page.groups.iter().enumerate() {
        if !group.title.is_empty() {
            y += GROUP_TOP * k;
            out.content.push(Node {
                rect: Rect::new(x0 + k, y, w, GROUP_H * k),
                look: Look::Text {
                    text: group.title.clone(),
                    style: style(Face::Strong, BODY_PX * k, 1),
                },
                hit: None,
            });
            y += GROUP_H * k + GROUP_GAP * k;
        } else {
            y += if gi == 0 { 24.0 } else { CARD_GAP } * k;
        }
        for (ii, item) in group.items.iter().enumerate() {
            if ii > 0 {
                y += CARD_GAP * k;
            }
            let at = ItemRef {
                group: gi,
                item: ii,
                child: None,
            };
            y += card(input, m, item, at, x0, w, y, out);
        }
    }
    out.content_height = y + BOTTOM * k;
}

/// Lay out a card and, when it is expanded, its children; returns its height.
#[allow(clippy::too_many_arguments)]
fn card(input: &Input, m: &mut dyn Measure, item: &Item, at: ItemRef, x: f32, w: f32, y: f32, out: &mut Layout) -> f32 {
    let k = input.k;
    let expandable = !item.children.is_empty();
    let expanded = expandable && input.expanded.contains(&item.key);
    let card_at = out.content.len();
    out.content.push(Node {
        rect: Rect::default(),
        look: Look::Card,
        hit: None,
    });
    if item.link && item.enabled {
        out.content.push(Node {
            rect: Rect::default(),
            look: Look::CardHover,
            hit: Some(Hit::Item(at, Part::Link)),
        });
    }
    let left = x + CARD_PAD * k;
    let mut right = x + w - CARD_PAD * k;
    if expandable || item.link {
        let r = Rect::new(right - CONTROL_H * k, y, CONTROL_H * k, CARD_MIN_H * k);
        right = r.x - 8.0 * k;
        let (glyph, part) = match (item.link, expanded) {
            (true, _) => ('\u{E76C}', Part::Link),
            (false, true) => ('\u{E70E}', Part::Expand),
            (false, false) => ('\u{E70D}', Part::Expand),
        };
        out.content.push(Node {
            rect: Rect::new(
                r.x,
                y + (CARD_MIN_H * k - CONTROL_H * k) / 2.0,
                CONTROL_H * k,
                CONTROL_H * k,
            ),
            look: Look::Chevron {
                glyph,
                enabled: item.enabled,
            },
            hit: None,
        });
        if expandable {
            out.content.push(Node {
                rect: Rect::default(),
                look: Look::CardHover,
                hit: Some(Hit::Item(at, part)),
            });
        }
    }
    let head = row(item, at, item.enabled, left, right, y, CARD_MIN_H * k, m, k, out);
    let mut h = head;
    // The header's chevron and hover area take the header's final height.
    for n in &mut out.content[card_at + 1..] {
        if let Look::Chevron { .. } = n.look {
            n.rect.y = y + (head - CONTROL_H * k) / 2.0;
        }
        if let Look::CardHover = n.look {
            n.rect = Rect::new(x, y, w, head);
        }
    }
    if expanded {
        let child_left = x + CHILD_INDENT * k;
        let child_right = x + w - 44.0 * k - 8.0 * k;
        for (ci, child) in item.children.iter().enumerate() {
            out.content.push(Node {
                rect: Rect::new(x, y + h, w, k),
                look: Look::Divider,
                hit: None,
            });
            h += k;
            let cat = ItemRef { child: Some(ci), ..at };
            let enabled = item.enabled && child.enabled;
            h += row(
                child,
                cat,
                enabled,
                child_left,
                child_right,
                y + h,
                ROW_MIN_H * k,
                m,
                k,
                out,
            );
        }
    }
    out.content[card_at].rect = Rect::new(x, y, w, h);
    h
}

/// The node under `(x, y)` that does something, searching from the top-most.
pub(crate) fn hit_test(nodes: &[Node], x: f32, y: f32) -> Option<Hit> {
    nodes
        .iter()
        .rev()
        .find(|n| n.hit.is_some() && n.rect.contains(x, y))
        .and_then(|n| n.hit)
}

/// The shortcut dialog's nodes, centered in a `width` x `height` window.
pub(crate) fn dialog(m: &mut dyn Measure, keys: &[String], valid: bool, width: f32, height: f32, k: f32) -> Vec<Node> {
    let w = (DIALOG_W * k).min(width - 32.0 * k);
    let pad = 24.0 * k;
    let footer = 80.0 * k;
    let body = Style {
        wrap: true,
        ..style(Face::Text, BODY_PX * k, 1)
    };
    let prompt = "Press a combination of keys to change this shortcut.";
    let (_, ph) = m.measure(prompt, &body, w - 2.0 * pad);
    let warn = "Only shortcuts that start with Windows key, Ctrl, Alt or Shift are valid.";
    let (_, wh) = m.measure(warn, &body, w - 2.0 * pad - 48.0 * k);
    let warn_h = if valid {
        0.0
    } else {
        (wh + 28.0 * k).max(48.0 * k) + 16.0 * k
    };
    let keys_h = 56.0 * k;
    let h = pad + 28.0 * k + 12.0 * k + ph + 24.0 * k + keys_h + 24.0 * k + warn_h + footer;
    let x = (width - w) / 2.0;
    let y = ((height - h) / 2.0).max(8.0 * k);
    let mut out = vec![Node {
        rect: Rect::new(x, y, w, h),
        look: Look::Dialog,
        hit: None,
    }];
    let mut cy = y + pad;
    out.push(Node {
        rect: Rect::new(x + pad, cy, w - 2.0 * pad, 28.0 * k),
        look: Look::Text {
            text: "Activation shortcut".into(),
            style: style(Face::Strong, 20.0 * k, 1),
        },
        hit: None,
    });
    cy += 40.0 * k;
    out.push(Node {
        rect: Rect::new(x + pad, cy, w - 2.0 * pad, ph),
        look: Look::Text {
            text: prompt.into(),
            style: body,
        },
        hit: None,
    });
    cy += ph + 24.0 * k;
    let kw = keys_width(m, keys, 18.0, k).max(0.0);
    out.push(Node {
        rect: Rect::new(x + (w - kw) / 2.0, cy, kw, keys_h),
        look: Look::Keys {
            keys: keys.to_vec(),
            enabled: true,
            large: true,
        },
        hit: None,
    });
    cy += keys_h + 24.0 * k;
    if !valid {
        let r = Rect::new(x + pad, cy, w - 2.0 * pad, warn_h - 16.0 * k);
        out.push(Node {
            rect: r,
            look: Look::Info {
                severity: Severity::Warning,
            },
            hit: None,
        });
        out.push(Node {
            rect: Rect::new(r.x + 16.0 * k, r.y, 20.0 * k, 48.0 * k),
            look: Look::InfoGlyph {
                severity: Severity::Warning,
            },
            hit: None,
        });
        out.push(Node {
            rect: Rect::new(r.x + 48.0 * k, r.y + (r.h - wh) / 2.0, r.w - 64.0 * k, wh),
            look: Look::Text {
                text: warn.into(),
                style: body,
            },
            hit: None,
        });
    }
    let fy = y + h - footer;
    out.push(Node {
        rect: Rect::new(x, fy, w, footer),
        look: Look::DialogFooter,
        hit: None,
    });
    let buttons = [
        ("Save", DialogButton::Save, true),
        ("Reset", DialogButton::Reset, false),
        ("Clear", DialogButton::Clear, false),
        ("Cancel", DialogButton::Cancel, false),
    ];
    let gap = 8.0 * k;
    let bw = (w - 2.0 * pad - gap * 3.0) / 4.0;
    for (i, (label, b, accent)) in buttons.into_iter().enumerate() {
        let enabled = b != DialogButton::Save || valid;
        out.push(Node {
            rect: Rect::new(x + pad + i as f32 * (bw + gap), fy + pad, bw, CONTROL_H * k),
            look: Look::Button {
                label: label.into(),
                accent,
                enabled,
            },
            hit: enabled.then_some(Hit::Dialog(b)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character is half the font size wide; text wraps by words.
    struct Fixed;
    impl Measure for Fixed {
        fn measure(&mut self, s: &str, st: &Style, width: f32) -> (f32, f32) {
            let full = s.chars().count() as f32 * st.px / 2.0;
            let line = st.px * 1.4;
            if st.wrap && width > 0.0 && full > width {
                (width, (full / width).ceil() * line)
            } else {
                (full, line)
            }
        }
    }

    fn page() -> Page {
        let mut expander = Item::new("fm", "Find My Mouse", Control::Toggle(true));
        expander.children = vec![
            Item::new("fm.radius", "Radius", Control::Number { value: "100".into() }),
            Item::new("fm.color", "Color", Control::Color("#000000".into())),
        ];
        Page {
            id: "mouse".into(),
            title: "Mouse utilities".into(),
            description: "Tools for the pointer.".into(),
            groups: vec![super::super::model::Group {
                title: "Find My Mouse".into(),
                items: vec![
                    Item::new("enabled", "Enable", Control::Toggle(true)),
                    expander,
                    Item::new(
                        "apps",
                        "Excluded apps",
                        Control::Text {
                            value: String::new(),
                            placeholder: String::new(),
                            lines: 4,
                            wide: true,
                        },
                    ),
                ],
            }],
        }
    }

    fn run(page: &Page, expanded: &[&str], width: f32) -> Layout {
        let nav = vec![
            NavEntry::Page {
                title: "Home".into(),
                icon: None,
                glyph: Some('\u{E80F}'),
                depth: 0,
            },
            NavEntry::Category {
                title: "Input".into(),
                glyph: '\u{E765}',
            },
            NavEntry::Page {
                title: "Mouse".into(),
                icon: None,
                glyph: None,
                depth: 1,
            },
        ];
        let folded = HashSet::new();
        let expanded: HashSet<String> = expanded.iter().map(|s| s.to_string()).collect();
        let guide = |w: f32| w * 0.6;
        let input = Input {
            nav: &nav,
            folded: &folded,
            selected: 2,
            page,
            expanded: &expanded,
            info: None,
            guide: Some(&guide),
            width,
            k: 1.0,
        };
        layout(&input, &mut Fixed)
    }

    fn hits(l: &Layout) -> Vec<Hit> {
        l.content.iter().filter_map(|n| n.hit).collect()
    }

    #[test]
    fn collapsed_expanders_hide_their_rows() {
        let p = page();
        let closed = run(&p, &[], 1200.0);
        let open = run(&p, &["fm"], 1200.0);
        assert!(open.content_height > closed.content_height);
        let child_hits = |l: &Layout| {
            hits(l)
                .into_iter()
                .filter(|h| matches!(h, Hit::Item(r, _) if r.child.is_some()))
                .count()
        };
        assert_eq!(child_hits(&closed), 0);
        assert_eq!(child_hits(&open), 1, "the color button; the number is a text box");
        assert_eq!(open.fields.len(), 2, "radius and excluded apps");
    }

    #[test]
    fn clicks_land_on_the_control_under_the_pointer() {
        let p = page();
        let l = run(&p, &["fm"], 1200.0);
        let toggle = l
            .content
            .iter()
            .find(|n| {
                n.hit
                    == Some(Hit::Item(
                        ItemRef {
                            group: 0,
                            item: 0,
                            child: None,
                        },
                        Part::Toggle,
                    ))
            })
            .expect("the enable toggle");
        let (cx, cy) = (toggle.rect.x + toggle.rect.w / 2.0, toggle.rect.y + toggle.rect.h / 2.0);
        assert_eq!(hit_test(&l.content, cx, cy), toggle.hit);
        // The expander's header is clickable everywhere, but its toggle wins.
        let exp = ItemRef {
            group: 0,
            item: 1,
            child: None,
        };
        let card = l
            .content
            .iter()
            .find(|n| n.hit == Some(Hit::Item(exp, Part::Expand)))
            .unwrap();
        assert_eq!(
            hit_test(&l.content, card.rect.x + 5.0, card.rect.y + 5.0),
            Some(Hit::Item(exp, Part::Expand))
        );
        let t = l
            .content
            .iter()
            .find(|n| n.hit == Some(Hit::Item(exp, Part::Toggle)))
            .unwrap();
        assert_eq!(
            hit_test(&l.content, t.rect.x + t.rect.w / 2.0, t.rect.y + t.rect.h / 2.0),
            Some(Hit::Item(exp, Part::Toggle))
        );
    }

    #[test]
    fn narrow_windows_get_a_compact_pane_and_stacked_header() {
        let p = page();
        let wide = run(&p, &[], 1200.0);
        let narrow = run(&p, &[], 700.0);
        assert_eq!(wide.nav_w, NAV_W);
        assert_eq!(narrow.nav_w, NAV_COMPACT_W);
        let g = narrow.guide.unwrap();
        let desc = narrow
            .content
            .iter()
            .find(|n| matches!(&n.look, Look::Text { text, .. } if text == "Tools for the pointer."))
            .unwrap();
        assert!(desc.rect.y >= g.bottom(), "description goes under the walkthrough");
        assert!(narrow.content.iter().all(|n| n.rect.right() <= 700.0 + 0.5));
    }

    #[test]
    fn folded_categories_hide_their_pages() {
        let p = page();
        let nav = vec![
            NavEntry::Category {
                title: "Input".into(),
                glyph: 'x',
            },
            NavEntry::Page {
                title: "Mouse".into(),
                icon: None,
                glyph: None,
                depth: 1,
            },
        ];
        let mut folded = HashSet::new();
        folded.insert(0);
        let expanded = HashSet::new();
        let input = Input {
            nav: &nav,
            folded: &folded,
            selected: 1,
            page: &p,
            expanded: &expanded,
            info: None,
            guide: None,
            width: 1200.0,
            k: 1.0,
        };
        let l = layout(&input, &mut Fixed);
        assert_eq!(l.nav.len(), 1);
    }

    #[test]
    fn key_caps_use_symbols() {
        assert_eq!(key_caps("Win+Ctrl+Alt+Right"), ["\u{229E}", "Ctrl", "Alt", "\u{2192}"]);
        assert_eq!(key_caps(""), Vec::<String>::new());
    }

    #[test]
    fn the_dialog_disables_save_for_invalid_shortcuts() {
        let n = dialog(&mut Fixed, &["Q".into()], false, 1000.0, 700.0, 1.0);
        assert!(!n.iter().any(|n| n.hit == Some(Hit::Dialog(DialogButton::Save))));
        let n = dialog(&mut Fixed, &["Ctrl".into(), "Q".into()], true, 1000.0, 700.0, 1.0);
        assert!(n.iter().any(|n| n.hit == Some(Hit::Dialog(DialogButton::Save))));
    }
}
