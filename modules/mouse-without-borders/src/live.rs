//! This machine's side: watches the mouse and keyboard, and while the pointer is on
//! another machine keeps all input from this one and sends it there instead.

use crate::net::{Net, Notice};
use crate::{clamp_to, past_edge, switch_key, travel, Machine, Message, Pointer};
use anyhow::Result;
use powerkit_config::MouseWithoutBorders as Settings;
use powerkit_core::{Event, Rect};
use powerkit_platform::keyhook::{self, KeyDecision};
use powerkit_platform::mousehook::{Button, MouseInput};
use powerkit_platform::{clipboard, crypto, event_loop, input, monitor};
use powerkit_plugin_api::Ctx;
use std::collections::{BTreeSet, HashSet};

const POLL_MS: u32 = 200;
/// Longer copied text stays on this machine.
const MAX_CLIPBOARD: usize = 4 << 20;
const TITLE: &str = "Mouse Without Borders";

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// A random security key of letters and digits.
pub fn new_key() -> Result<String> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut key = String::new();
    while key.len() < 24 {
        let mut bytes = [0u8; 32];
        crypto::random(&mut bytes)?;
        // Bytes past the last whole multiple of 62 would favour some characters.
        key.extend(
            bytes
                .iter()
                .filter(|b| usize::from(**b) < 248)
                .map(|b| CHARS[usize::from(*b) % CHARS.len()] as char),
        );
    }
    key.truncate(24);
    Ok(key)
}

fn button_code(b: Button) -> u8 {
    match b {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::X1 => 3,
        Button::X2 => 4,
    }
}

fn is_ctrl(vk: u32) -> bool {
    matches!(vk, 0x11 | 0xA2 | 0xA3)
}

fn is_alt(vk: u32) -> bool {
    matches!(vk, 0x12 | 0xA4 | 0xA5)
}

/// The middle of the monitor nearest to a point.
fn monitor_center(at: (i32, i32)) -> (i32, i32) {
    monitor::from_point(at.0, at.1).map_or(at, |m| m.rect.center())
}

struct Remote {
    pointer: Pointer,
    /// Where this machine's own pointer is held; each move is measured from here.
    anchor: (i32, i32),
}

pub struct Live {
    pub settings: Settings,
    machines: Vec<Machine>,
    local: usize,
    net: Net,
    screen: Rect,
    remote: Option<Remote>,
    buttons: BTreeSet<u8>,
    ctrl: bool,
    alt: bool,
    /// Keys down on this machine, released when the keyboard goes elsewhere.
    held: BTreeSet<u32>,
    /// Keys sent down to the machine being controlled.
    sent: BTreeSet<u32>,
    /// Switch keys whose release must be swallowed as their press was.
    eaten: BTreeSet<u32>,
    /// The text last copied here or received, so it is not sent back and forth.
    clipboard: Option<String>,
    listening: bool,
    timer: Option<usize>,
    told: HashSet<String>,
}

impl Live {
    pub fn start(settings: Settings, machines: Vec<Machine>, local: usize, me: String) -> Result<Live> {
        let screen = monitor::virtual_screen();
        let net = Net::start(me, machines.clone(), &settings.security_key, settings.port, screen)?;
        let mut live = Live {
            listening: false,
            timer: None,
            settings,
            machines,
            local,
            net,
            screen,
            remote: None,
            buttons: BTreeSet::new(),
            ctrl: false,
            alt: false,
            held: BTreeSet::new(),
            sent: BTreeSet::new(),
            eaten: BTreeSet::new(),
            clipboard: None,
            told: HashSet::new(),
        };
        live.timer = Some(event_loop::set_timer(POLL_MS)?);
        if live.settings.share_clipboard {
            event_loop::listen_clipboard(true)?;
            live.listening = true;
        }
        Ok(live)
    }

    pub fn close(&mut self) {
        self.go_home(None);
        if let Some(t) = self.timer.take() {
            event_loop::kill_timer(t);
        }
        if std::mem::take(&mut self.listening) {
            if let Err(e) = event_loop::listen_clipboard(false) {
                tracing::warn!(error = %e, "Mouse Without Borders: could not stop watching the clipboard");
            }
        }
    }

    pub fn connected(&self) -> Vec<String> {
        self.net.connected()
    }

    fn send(&self, machine: usize, m: Message) -> bool {
        self.net.send(&self.machines[machine].name, m)
    }

    /// Give input back to this machine, with the pointer at `at` (or where it is held).
    fn go_home(&mut self, at: Option<(i32, i32)>) {
        let Some(r) = self.remote.take() else { return };
        for vk in std::mem::take(&mut self.sent) {
            self.send(r.pointer.machine, Message::Key { vk, down: false });
        }
        for button in std::mem::take(&mut self.buttons) {
            self.send(r.pointer.machine, Message::Button { button, down: false });
        }
        // Not on the very edge, or the next move along it would leave again.
        let s = self.screen;
        let (x, y) = at.unwrap_or(r.anchor);
        let at = (x.clamp(s.left + 2, (s.right - 3).max(s.left + 2)), y);
        if let Err(e) = input::set_cursor_pos(at.0, at.1) {
            tracing::warn!(error = %e, "Mouse Without Borders: could not place the pointer");
        }
    }

    /// Take input to another machine, the pointer at `to` there.
    fn go_to(&mut self, to: Pointer) {
        let anchor = match &self.remote {
            Some(r) => {
                if r.pointer.machine != to.machine {
                    let from = r.pointer.machine;
                    for vk in std::mem::take(&mut self.sent) {
                        self.send(from, Message::Key { vk, down: false });
                    }
                }
                r.anchor
            }
            None => {
                let here = input::cursor_pos().unwrap_or(self.screen.center());
                let anchor = monitor_center(here);
                if let Err(e) = input::set_cursor_pos(anchor.0, anchor.1) {
                    tracing::warn!(error = %e, "Mouse Without Borders: could not hold the pointer");
                }
                // Keys held here would stay down for good: their releases go elsewhere.
                let keys: Vec<_> = std::mem::take(&mut self.held)
                    .into_iter()
                    .map(|vk| (vk, false))
                    .collect();
                if let Err(e) = keyhook::send_keys(&keys) {
                    tracing::warn!(error = %e, "Mouse Without Borders: could not release keys");
                }
                anchor
            }
        };
        self.remote = Some(Remote { pointer: to, anchor });
        if !self.send(to.machine, Message::Move { x: to.at.0, y: to.at.1 }) {
            self.go_home(None);
        }
    }

    /// Ctrl+Alt+F1 to F4.
    fn switch_to(&mut self, machine: usize) {
        if machine == self.local {
            let at = monitor_center(self.screen.center());
            self.go_home(Some(at));
            return;
        }
        if let Some(Some(s)) = self.net.screens().get(machine) {
            let at = s.center();
            self.go_to(Pointer { machine, at });
        }
    }

    /// Mouse hook callback; returns whether to keep the input from this machine.
    pub fn on_mouse(&mut self, input: MouseInput) -> bool {
        let Some(r) = &self.remote else {
            match input {
                MouseInput::Down { button, .. } => {
                    self.buttons.insert(button_code(button));
                }
                MouseInput::Up { button, .. } => {
                    self.buttons.remove(&button_code(button));
                }
                MouseInput::Move { x, y } if self.buttons.is_empty() && !self.net.driven() => {
                    if let Some(past) = past_edge(self.screen, (x, y)) {
                        let to = travel(&self.net.screens(), self.local, past);
                        if to.machine != self.local {
                            self.go_to(to);
                            return true;
                        }
                    }
                }
                _ => {}
            }
            return false;
        };
        let machine = r.pointer.machine;
        let delivered = match input {
            MouseInput::Move { x, y } => {
                let (dx, dy) = (x - r.anchor.0, y - r.anchor.1);
                if (dx, dy) == (0, 0) {
                    return true;
                }
                let want = (r.pointer.at.0 + dx, r.pointer.at.1 + dy);
                let screens = self.net.screens();
                let to = match screens.get(machine).copied().flatten() {
                    // A drag stays on the machine where it started.
                    Some(s) if !self.buttons.is_empty() => Pointer {
                        machine,
                        at: clamp_to(s, want),
                    },
                    Some(_) => travel(&screens, machine, want),
                    None => {
                        self.go_home(None);
                        return true;
                    }
                };
                if to.machine == self.local {
                    self.go_home(Some(to.at));
                    return true;
                }
                if to.machine != machine {
                    self.go_to(to);
                    return true;
                }
                if let Some(r) = &mut self.remote {
                    r.pointer = to;
                }
                self.send(machine, Message::Move { x: to.at.0, y: to.at.1 })
            }
            MouseInput::Down { button, .. } => {
                let button = button_code(button);
                self.buttons.insert(button);
                self.send(machine, Message::Button { button, down: true })
            }
            MouseInput::Up { button, .. } => {
                let button = button_code(button);
                self.buttons.remove(&button);
                self.send(machine, Message::Button { button, down: false })
            }
            MouseInput::Wheel { delta, horizontal } => self.send(machine, Message::Wheel { delta, horizontal }),
        };
        if !delivered {
            self.go_home(None);
        }
        true
    }

    /// Keyboard hook callback.
    pub fn on_key(&mut self, vk: u32, down: bool) -> KeyDecision {
        if is_ctrl(vk) {
            self.ctrl = down;
        }
        if is_alt(vk) {
            self.alt = down;
        }
        if !down && self.eaten.remove(&vk) {
            return KeyDecision::Replace(Vec::new());
        }
        if down && self.settings.switch_keys && self.ctrl && self.alt {
            if let Some(machine) = switch_key(vk).filter(|m| *m < self.machines.len()) {
                self.eaten.insert(vk);
                self.switch_to(machine);
                return KeyDecision::Replace(Vec::new());
            }
        }
        let Some(r) = &self.remote else {
            if down {
                self.held.insert(vk);
            } else {
                self.held.remove(&vk);
            }
            return KeyDecision::Pass;
        };
        let machine = r.pointer.machine;
        if down {
            self.sent.insert(vk);
        } else {
            self.sent.remove(&vk);
        }
        if !self.send(machine, Message::Key { vk, down }) {
            self.go_home(None);
        }
        KeyDecision::Replace(Vec::new())
    }

    pub fn on_event(&mut self, event: &Event, ctx: &mut Ctx) {
        match event {
            Event::Timer(id) if Some(*id) == self.timer => {
                for n in self.net.take_notices() {
                    self.notice(n, ctx);
                }
                if let Some(r) = &self.remote {
                    if self.net.screens().get(r.pointer.machine).copied().flatten().is_none() {
                        self.go_home(None);
                    }
                }
            }
            Event::ClipboardChanged if self.listening => match clipboard::get_text() {
                Ok(Some(text)) if text.len() <= MAX_CLIPBOARD && self.clipboard.as_ref() != Some(&text) => {
                    self.clipboard = Some(text.clone());
                    self.net.broadcast(Message::Clipboard(text));
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "Mouse Without Borders: could not read the clipboard"),
            },
            Event::DisplayChanged => {
                self.screen = monitor::virtual_screen();
                self.net.set_screen(self.screen);
            }
            _ => {}
        }
    }

    fn notice(&mut self, n: Notice, ctx: &mut Ctx) {
        match n {
            // Said once per run: a flaky network would otherwise keep announcing it.
            Notice::Connected(name) => {
                if self.told.insert(format!("connected {}", name.to_lowercase())) {
                    ctx.notify(TITLE, format!("Connected to {name}."));
                }
            }
            Notice::Disconnected(name) => tracing::info!("Mouse Without Borders: {name} disconnected"),
            Notice::WrongKey(from) => {
                if self.told.insert(format!("key {from}")) {
                    ctx.notify(
                        TITLE,
                        format!("{from} tried to connect with a different security key. Use the same key on every PC."),
                    );
                }
            }
            Notice::Clipboard(text) if self.settings.share_clipboard => {
                self.clipboard = Some(text.clone());
                if let Err(e) = clipboard::set_text(&text) {
                    tracing::warn!(error = %e, "Mouse Without Borders: could not set the clipboard");
                }
            }
            Notice::Clipboard(_) => {}
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.close();
    }
}
