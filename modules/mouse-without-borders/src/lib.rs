//! Mouse Without Borders: the PCs listed side by side share one mouse and keyboard.
//! Pushing the pointer off the left or right edge of the screen carries it, and the
//! keyboard, to the neighbouring PC; copied text goes to every PC. Machines talk over
//! TCP, encrypted with a key derived from the security key they share.

#[cfg(windows)]
mod live;
#[cfg(windows)]
mod net;

use anyhow::Result;
use powerkit_config::MouseWithoutBorders as Settings;
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
#[cfg(windows)]
use std::{cell::RefCell, rc::Rc};

pub const ID: &str = "mouse_without_borders";
pub const MIN_KEY_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub name: String,
    /// Where to connect: the address given after the name, otherwise the name.
    pub address: String,
}

/// The machines from the settings, in order.
pub fn machines(entries: &[String]) -> Vec<Machine> {
    entries
        .iter()
        .filter_map(|e| {
            let mut parts = e.split_whitespace();
            let name = parts.next()?.to_string();
            let address = parts.next().map_or_else(|| name.clone(), str::to_string);
            Some(Machine { name, address })
        })
        .collect()
}

/// What one machine sends another. Pointer positions are in the receiver's
/// physical virtual-desktop pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// First message of every connection, and again when the screens change.
    Hello {
        name: String,
        screen: Rect,
    },
    Move {
        x: i32,
        y: i32,
    },
    /// 0 left, 1 right, 2 middle, 3 and 4 the side buttons.
    Button {
        button: u8,
        down: bool,
    },
    Wheel {
        delta: i32,
        horizontal: bool,
    },
    Key {
        vk: u32,
        down: bool,
    },
    Clipboard(String),
    /// Keeps a quiet connection from timing out.
    Ping,
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let int = |out: &mut Vec<u8>, v: i32| out.extend_from_slice(&v.to_le_bytes());
        match self {
            Message::Hello { name, screen } => {
                out.push(1);
                for v in [screen.left, screen.top, screen.right, screen.bottom] {
                    int(&mut out, v);
                }
                out.extend_from_slice(name.as_bytes());
            }
            Message::Move { x, y } => {
                out.push(2);
                int(&mut out, *x);
                int(&mut out, *y);
            }
            Message::Button { button, down } => out.extend_from_slice(&[3, *button, u8::from(*down)]),
            Message::Wheel { delta, horizontal } => {
                out.push(4);
                int(&mut out, *delta);
                out.push(u8::from(*horizontal));
            }
            Message::Key { vk, down } => {
                out.push(5);
                out.extend_from_slice(&vk.to_le_bytes());
                out.push(u8::from(*down));
            }
            Message::Clipboard(text) => {
                out.push(6);
                out.extend_from_slice(text.as_bytes());
            }
            Message::Ping => out.push(7),
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Message> {
        let (&kind, rest) = bytes.split_first()?;
        let int = |at: usize| Some(i32::from_le_bytes(rest.get(at..at + 4)?.try_into().ok()?));
        let flag = |at: usize| match rest.get(at)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        let exact = |len: usize| (rest.len() == len).then_some(());
        Some(match kind {
            1 => {
                let screen = Rect::new(int(0)?, int(4)?, int(8)?, int(12)?);
                let name = std::str::from_utf8(rest.get(16..)?).ok()?.to_string();
                Message::Hello { name, screen }
            }
            2 => {
                exact(8)?;
                Message::Move { x: int(0)?, y: int(4)? }
            }
            3 => {
                exact(2)?;
                let button = *rest.first().filter(|b| **b <= 4)?;
                Message::Button { button, down: flag(1)? }
            }
            4 => {
                exact(5)?;
                Message::Wheel {
                    delta: int(0)?,
                    horizontal: flag(4)?,
                }
            }
            5 => {
                exact(5)?;
                Message::Key {
                    vk: int(0)? as u32,
                    down: flag(4)?,
                }
            }
            6 => Message::Clipboard(std::str::from_utf8(rest).ok()?.to_string()),
            7 => {
                exact(0)?;
                Message::Ping
            }
            _ => return None,
        })
    }
}

/// Where the pointer is: on which machine, at a point in that machine's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    pub machine: usize,
    pub at: (i32, i32),
}

/// `p` moved inside `r`.
pub fn clamp_to(r: Rect, (x, y): (i32, i32)) -> (i32, i32) {
    (x.clamp(r.left, r.right - 1), y.clamp(r.top, r.bottom - 1))
}

/// Where the pointer goes when it moves to `to`, a point in `machine`'s pixels that may
/// lie past its left or right edge. `screens` holds each machine's screen in order,
/// `None` for one that is not connected. Past an edge with a connected neighbour the
/// pointer carries on onto the neighbour, at the same height relative to the screen;
/// otherwise it stops at the edge.
pub fn travel(screens: &[Option<Rect>], machine: usize, to: (i32, i32)) -> Pointer {
    let Some(r) = screens.get(machine).copied().flatten() else {
        return Pointer { machine, at: to };
    };
    let next = if to.0 >= r.right {
        Some((machine + 1, to.0 - r.right))
    } else if to.0 < r.left {
        machine.checked_sub(1).map(|m| (m, to.0 - r.left))
    } else {
        None
    };
    if let Some((m, past)) = next {
        if let Some(s) = screens.get(m).copied().flatten() {
            let y = to.1.clamp(r.top, r.bottom - 1) - r.top;
            let y = s.top + (i64::from(y) * i64::from(s.height()) / i64::from(r.height().max(1))) as i32;
            let x = if past >= 0 { s.left + past } else { s.right + past };
            return Pointer {
                machine: m,
                at: clamp_to(s, (x, y)),
            };
        }
    }
    Pointer {
        machine,
        at: clamp_to(r, to),
    }
}

/// The point just past the edge the local pointer is touching, if it touches the left
/// or right edge of the whole desktop.
pub fn past_edge(screen: Rect, (x, y): (i32, i32)) -> Option<(i32, i32)> {
    if x >= screen.right - 1 {
        Some((screen.right, y))
    } else if x <= screen.left {
        Some((screen.left - 1, y))
    } else {
        None
    }
}

/// Which machine Ctrl+Alt with this key goes to: F1 the first, up to F4.
pub fn switch_key(vk: u32) -> Option<usize> {
    (0x70..=0x73).contains(&vk).then(|| (vk - 0x70) as usize)
}

#[derive(Default)]
pub struct MouseWithoutBorders {
    settings: Settings,
    running: bool,
    paused: bool,
    #[cfg(windows)]
    live: Option<Rc<RefCell<live::Live>>>,
    #[cfg(windows)]
    hooks: Option<(
        powerkit_platform::mousehook::MouseHook,
        powerkit_platform::keyhook::KeyboardHook,
    )>,
}

impl MouseWithoutBorders {
    pub fn new() -> Self {
        Self::default()
    }

    fn active(&self) -> bool {
        self.running && !self.paused && self.settings.enabled
    }

    /// Start, restart or stop the connections and hooks to match the settings.
    #[cfg(windows)]
    fn apply(&mut self, ctx: &mut Ctx) -> Result<()> {
        use powerkit_platform::keyhook::{KeyDecision, KeyboardHook};
        use powerkit_platform::mousehook::MouseHook;
        if let Some(l) = &self.live {
            if self.active() && l.borrow().settings == self.settings {
                return Ok(());
            }
        }
        self.hooks = None;
        if let Some(l) = self.live.take() {
            l.borrow_mut().close();
        }
        if !self.active() {
            return Ok(());
        }
        if self.settings.security_key.is_empty() {
            let key = live::new_key()?;
            self.settings.security_key = key.clone();
            ctx.update_config(move |c| c.mouse_without_borders.security_key = key);
        }
        if self.settings.security_key.chars().count() < MIN_KEY_LEN {
            anyhow::bail!("the security key needs at least {MIN_KEY_LEN} characters");
        }
        let me = live::computer_name();
        let list = machines(&self.settings.machines);
        if list.len() < 2 {
            ctx.notify(
                "Mouse Without Borders",
                format!("List the PCs in Settings from left to right, this one ({me}) included, with the same security key on each."),
            );
            return Ok(());
        }
        let Some(local) = list.iter().position(|m| m.name.eq_ignore_ascii_case(&me)) else {
            anyhow::bail!("this PC ({me}) is not in the list of machines");
        };
        let live = Rc::new(RefCell::new(live::Live::start(self.settings.clone(), list, local, me)?));
        let (m, k) = (Rc::clone(&live), Rc::clone(&live));
        let mouse = MouseHook::install_filter(move |input| match m.try_borrow_mut() {
            Ok(mut l) => l.on_mouse(input),
            Err(_) => false,
        })?;
        let keyboard = KeyboardHook::install(move |vk, down| match k.try_borrow_mut() {
            Ok(mut l) => l.on_key(vk, down),
            Err(_) => KeyDecision::Pass,
        })?;
        self.hooks = Some((mouse, keyboard));
        self.live = Some(live);
        Ok(())
    }

    #[cfg(not(windows))]
    fn apply(&mut self, _ctx: &mut Ctx) -> Result<()> {
        if self.active() {
            anyhow::bail!("Mouse Without Borders runs only on Windows");
        }
        Ok(())
    }

    fn connected(&self) -> Vec<String> {
        #[cfg(windows)]
        return self.live.as_ref().map(|l| l.borrow().connected()).unwrap_or_default();
        #[cfg(not(windows))]
        Vec::new()
    }
}

impl Module for MouseWithoutBorders {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Mouse Without Borders".into(),
            description: "Use one mouse and keyboard across several PCs on the network.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::MouseHook,
                Permission::KeyboardHook,
                Permission::ClipboardRead,
                Permission::ClipboardWrite,
            ],
            commands: vec![CommandSpec {
                id: "reconnect".into(),
                title: "Reconnect to the other PCs".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.mouse_without_borders.clone();
        self.running = true;
        self.paused = false;
        self.apply(ctx)
    }

    fn stop(&mut self, ctx: &mut Ctx) {
        self.running = false;
        if let Err(e) = self.apply(ctx) {
            tracing::warn!(error = %e, "Mouse Without Borders did not stop cleanly");
        }
    }

    fn pause(&mut self, ctx: &mut Ctx) {
        self.paused = true;
        if let Err(e) = self.apply(ctx) {
            tracing::warn!(error = %e, "Mouse Without Borders did not pause cleanly");
        }
    }

    fn resume(&mut self, ctx: &mut Ctx) {
        self.paused = false;
        if let Err(e) = self.apply(ctx) {
            ctx.notify("Mouse Without Borders", format!("Could not turn back on: {e:#}"));
        }
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.mouse_without_borders.clone();
        self.apply(ctx)
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        #[cfg(windows)]
        if let Some(l) = &self.live {
            l.borrow_mut().on_event(event, ctx);
        }
        #[cfg(not(windows))]
        let _ = (event, ctx);
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "reconnect" => {
                if !self.active() {
                    anyhow::bail!("Mouse Without Borders is turned off");
                }
                #[cfg(windows)]
                {
                    self.hooks = None;
                    if let Some(l) = self.live.take() {
                        l.borrow_mut().close();
                    }
                }
                self.apply(ctx)?;
                Ok(self.status())
            }
            _ => anyhow::bail!("unknown command {id:?}"),
        }
    }

    fn status(&self) -> Value {
        json!({ "running": self.active(), "connected": self.connected() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machines_take_an_optional_address() {
        assert_eq!(
            machines(&["DESK".into(), "laptop 192.168.1.20".into()]),
            [
                Machine {
                    name: "DESK".into(),
                    address: "DESK".into()
                },
                Machine {
                    name: "laptop".into(),
                    address: "192.168.1.20".into()
                }
            ]
        );
    }

    #[test]
    fn messages_survive_the_wire_and_garbage_is_refused() {
        let all = [
            Message::Hello {
                name: "DESK".into(),
                screen: Rect::new(-1920, 0, 2560, 1440),
            },
            Message::Move { x: -5, y: 1439 },
            Message::Button { button: 4, down: true },
            Message::Wheel {
                delta: -120,
                horizontal: true,
            },
            Message::Key { vk: 0xA2, down: false },
            Message::Clipboard("héllo\r\n".into()),
            Message::Ping,
        ];
        for m in all {
            assert_eq!(Message::decode(&m.encode()), Some(m));
        }
        for bad in [
            &[][..],
            &[0],
            &[2, 1, 2, 3],
            &[3, 5, 1],
            &[3, 0, 2],
            &[7, 0],
            &[6, 0xFF],
            &[9],
        ] {
            assert_eq!(Message::decode(bad), None, "{bad:?}");
        }
    }

    const LEFT: Rect = Rect::new(-1920, 0, 0, 1080);
    const MIDDLE: Rect = Rect::new(0, 0, 2560, 1440);
    const RIGHT: Rect = Rect::new(0, 0, 1366, 768);

    #[test]
    fn the_pointer_crosses_to_a_connected_neighbour_at_the_same_height() {
        let screens = [Some(LEFT), Some(MIDDLE), Some(RIGHT)];
        assert_eq!(
            travel(&screens, 1, (2560 + 3, 720)),
            Pointer {
                machine: 2,
                at: (3, 384)
            }
        );
        assert_eq!(
            travel(&screens, 1, (-1, 1439)),
            Pointer {
                machine: 0,
                at: (-1, 1079)
            }
        );
        assert_eq!(
            travel(&screens, 2, (-10, 0)),
            Pointer {
                machine: 1,
                at: (2550, 0)
            }
        );
        assert_eq!(
            travel(&screens, 1, (100, 2000)),
            Pointer {
                machine: 1,
                at: (100, 1439)
            }
        );
    }

    #[test]
    fn the_pointer_stops_at_an_edge_with_nobody_behind_it() {
        let screens = [Some(LEFT), Some(MIDDLE), None];
        assert_eq!(
            travel(&screens, 1, (2600, 10)),
            Pointer {
                machine: 1,
                at: (2559, 10)
            }
        );
        assert_eq!(
            travel(&screens, 0, (-3000, 10)),
            Pointer {
                machine: 0,
                at: (-1920, 10)
            }
        );
    }

    #[test]
    fn only_the_outer_edges_count_and_f1_to_f4_pick_machines() {
        assert_eq!(past_edge(MIDDLE, (2559, 5)), Some((2560, 5)));
        assert_eq!(past_edge(MIDDLE, (0, 5)), Some((-1, 5)));
        assert_eq!(past_edge(MIDDLE, (1, 5)), None);
        assert_eq!(switch_key(0x70), Some(0));
        assert_eq!(switch_key(0x73), Some(3));
        assert_eq!(switch_key(0x74), None);
    }
}
