//! The connections between the machines: one TCP connection per pair, dialled by the
//! machine whose name sorts first. Both ends send a fresh random nonce; each direction
//! is then sealed with AES-GCM under its own key, an HMAC of both nonces with a master
//! key derived from the security key. Only machines with the same security key get past
//! the first message, and recorded traffic cannot be played back into a new connection.

use crate::{Machine, Message};
use anyhow::{anyhow, bail, ensure, Context, Result};
use powerkit_core::Rect;
use powerkit_platform::crypto::{self, Aead, NONCE_LEN, TAG_LEN};
use powerkit_platform::mousehook::Button;
use powerkit_platform::{input, keyhook};
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAGIC: &[u8; 8] = b"PKMWB\x01\0\0";
const SALT: &[u8] = b"PowerKit Mouse Without Borders";
const ITERATIONS: u64 = 100_000;
const MAX_FRAME: usize = 16 << 20;
/// A connection with nothing to say pings this often...
const PING: Duration = Duration::from_secs(5);
/// ...and one that hears nothing for this long is dropped.
const SILENCE: Duration = Duration::from_secs(15);
const DIAL_EVERY: Duration = Duration::from_secs(3);
const DIAL_TIMEOUT: Duration = Duration::from_secs(1);
/// Input received this recently means another machine is driving this one.
const DRIVEN: Duration = Duration::from_secs(1);
/// Connections still in their handshake; more are turned away.
const MAX_PENDING: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Connected(String),
    Disconnected(String),
    Clipboard(String),
    /// A connection from this address had a different security key.
    WrongKey(String),
}

struct Peer {
    id: u64,
    name: String,
    screen: Rect,
    tx: Sender<Message>,
    stream: TcpStream,
}

struct Shared {
    me: String,
    machines: Vec<Machine>,
    master: [u8; 32],
    screen: Mutex<Rect>,
    /// By lowercase name.
    peers: Mutex<HashMap<String, Peer>>,
    notices: Mutex<Vec<Notice>>,
    driven_at: Mutex<Option<Instant>>,
    stop: AtomicBool,
    next_id: AtomicU64,
    pending: AtomicUsize,
}

/// A lock that a panicked thread left behind still guards plain data.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn notice(&self, n: Notice) {
        lock(&self.notices).push(n);
    }

    fn known(&self, name: &str) -> bool {
        !name.eq_ignore_ascii_case(&self.me) && self.machines.iter().any(|m| m.name.eq_ignore_ascii_case(name))
    }
}

/// One direction of a connection: a key and the number of frames it has sealed, which
/// is each frame's nonce.
struct Channel {
    aead: Aead,
    count: u64,
}

impl Channel {
    fn new(key: [u8; 32]) -> Result<Channel> {
        Ok(Channel {
            aead: Aead::new(&key)?,
            count: 0,
        })
    }

    fn nonce(&mut self) -> [u8; NONCE_LEN] {
        let mut n = [0u8; NONCE_LEN];
        n[..8].copy_from_slice(&self.count.to_le_bytes());
        self.count += 1;
        n
    }

    fn write(&mut self, stream: &mut impl Write, m: &Message) -> Result<()> {
        let nonce = self.nonce();
        let sealed = self.aead.seal(&nonce, &[], &m.encode())?;
        let mut frame = Vec::with_capacity(4 + sealed.len());
        frame.extend_from_slice(&(sealed.len() as u32).to_le_bytes());
        frame.extend_from_slice(&sealed);
        stream.write_all(&frame)?;
        Ok(())
    }

    /// The next message; an error when the stream ends or the frame was not sealed with
    /// the matching key.
    fn read(&mut self, stream: &mut impl Read) -> Result<Message> {
        let mut len = [0u8; 4];
        stream.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        ensure!((TAG_LEN..=MAX_FRAME + TAG_LEN).contains(&len), "frame of {len} bytes");
        let mut sealed = vec![0u8; len];
        stream.read_exact(&mut sealed)?;
        let nonce = self.nonce();
        let plain = self.aead.open(&nonce, &[], &sealed).map_err(|_| WrongKey)?;
        Message::decode(&plain).ok_or_else(|| anyhow!("a message this version does not know"))
    }
}

#[derive(Debug)]
struct WrongKey;

impl std::fmt::Display for WrongKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the other machine has a different security key")
    }
}

impl std::error::Error for WrongKey {}

/// The keys for sending and receiving, from both ends' nonces.
fn keys(master: &[u8; 32], mine: &[u8; 32], theirs: &[u8; 32]) -> Result<([u8; 32], [u8; 32])> {
    let key = |from: &[u8; 32], to: &[u8; 32]| crypto::hmac_sha256(master, &[&b"send"[..], from, to].concat());
    Ok((key(mine, theirs)?, key(theirs, mine)?))
}

/// Exchange nonces and hellos. Returns the peer's name and screen and both channels.
fn handshake(shared: &Shared, stream: &mut TcpStream) -> Result<(String, Rect, Channel, Channel)> {
    let mut mine = [0u8; 32];
    crypto::random(&mut mine)?;
    stream.write_all(&[&MAGIC[..], &mine].concat())?;
    let mut opening = [0u8; 40];
    stream.read_exact(&mut opening)?;
    ensure!(
        &opening[..8] == MAGIC,
        "not a PowerKit Mouse Without Borders connection"
    );
    let theirs: [u8; 32] = opening[8..].try_into().expect("32 bytes");
    // The same nonce coming back means our own words are being reflected at us.
    ensure!(theirs != mine, "the other end repeated this machine's nonce");
    let (send, receive) = keys(&shared.master, &mine, &theirs)?;
    let (mut send, mut receive) = (Channel::new(send)?, Channel::new(receive)?);
    let screen = *lock(&shared.screen);
    send.write(
        stream,
        &Message::Hello {
            name: shared.me.clone(),
            screen,
        },
    )?;
    match receive.read(stream)? {
        Message::Hello { name, screen } => Ok((name, screen, send, receive)),
        other => bail!("expected a hello, got {other:?}"),
    }
}

/// Keys and buttons pressed on this machine for a peer, released if it goes away
/// before releasing them itself.
#[derive(Default)]
struct Pressed {
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u8>,
    failing: bool,
}

fn button(code: u8) -> Button {
    match code {
        0 => Button::Left,
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::X1,
        _ => Button::X2,
    }
}

impl Pressed {
    fn inject(&mut self, result: powerkit_platform::Result<()>) {
        match result {
            Ok(()) => self.failing = false,
            // The secure desktop (sign-in, UAC) and elevated windows refuse injected
            // input; say so once rather than on every move.
            Err(e) if !self.failing => {
                tracing::warn!(error = %e, "Mouse Without Borders could not pass on input");
                self.failing = true;
            }
            Err(_) => {}
        }
    }

    fn release(&mut self) {
        let keys: Vec<_> = std::mem::take(&mut self.keys)
            .into_iter()
            .map(|vk| (vk, false))
            .collect();
        self.inject(keyhook::send_keys(&keys));
        for b in std::mem::take(&mut self.buttons) {
            self.inject(input::send_button(button(b), false));
        }
    }
}

fn run_connection(shared: Arc<Shared>, mut stream: TcpStream, expected: Option<String>) {
    let from = stream
        .peer_addr()
        .map_or_else(|_| "an unknown address".into(), |a| a.ip().to_string());
    if let Err(e) = connection(&shared, &mut stream, expected) {
        if e.downcast_ref::<WrongKey>().is_some() {
            shared.notice(Notice::WrongKey(from));
        } else {
            tracing::info!("Mouse Without Borders: connection with {from} ended: {e:#}");
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn connection(shared: &Arc<Shared>, stream: &mut TcpStream, expected: Option<String>) -> Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(SILENCE))?;
    stream.set_write_timeout(Some(SILENCE))?;
    let pending = shared.pending.fetch_add(1, Ordering::SeqCst);
    let shaken = if pending < MAX_PENDING {
        handshake(shared, stream)
    } else {
        Err(anyhow!("too many connections at once"))
    };
    shared.pending.fetch_sub(1, Ordering::SeqCst);
    let (name, screen, mut send, mut receive) = shaken?;
    ensure!(shared.known(&name), "{name} is not in the list of machines");
    if let Some(want) = &expected {
        ensure!(name.eq_ignore_ascii_case(want), "{want} answered as {name}");
    }
    ensure!(!shared.stop.load(Ordering::SeqCst), "stopping");

    let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
    let key = name.to_lowercase();
    let (tx, rx) = mpsc::channel::<Message>();
    let mut writer = stream.try_clone()?;
    let peer = Peer {
        id,
        name: name.clone(),
        screen,
        tx,
        stream: stream.try_clone()?,
    };
    // A newer connection to the same machine replaces the old one, which is usually
    // dead already (the other end restarted).
    if let Some(old) = lock(&shared.peers).insert(key.clone(), peer) {
        let _ = old.stream.shutdown(Shutdown::Both);
    }
    shared.notice(Notice::Connected(name.clone()));
    tracing::info!("Mouse Without Borders: connected to {name}");

    std::thread::Builder::new()
        .name("mwb-send".into())
        .spawn(move || {
            loop {
                let m = match rx.recv_timeout(PING) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => Message::Ping,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                if let Err(e) = send.write(&mut writer, &m) {
                    tracing::info!("Mouse Without Borders: sending failed: {e:#}");
                    break;
                }
            }
            let _ = writer.shutdown(Shutdown::Both);
        })
        .context("starting the sending thread")?;

    let mut pressed = Pressed::default();
    let ended = receive_loop(shared, stream, &mut receive, &key, id, &mut pressed);
    pressed.release();
    let mut peers = lock(&shared.peers);
    if peers.get(&key).is_some_and(|p| p.id == id) {
        peers.remove(&key);
        drop(peers);
        shared.notice(Notice::Disconnected(name));
    }
    ended
}

fn receive_loop(
    shared: &Shared,
    stream: &mut TcpStream,
    receive: &mut Channel,
    key: &str,
    id: u64,
    pressed: &mut Pressed,
) -> Result<()> {
    let driven = || *lock(&shared.driven_at) = Some(Instant::now());
    loop {
        match receive.read(stream)? {
            Message::Hello { screen, .. } => {
                if let Some(p) = lock(&shared.peers).get_mut(key).filter(|p| p.id == id) {
                    p.screen = screen;
                }
            }
            Message::Move { x, y } => {
                driven();
                pressed.inject(input::send_move(x, y));
            }
            Message::Button { button: b, down } => {
                driven();
                if down {
                    pressed.buttons.insert(b);
                } else {
                    pressed.buttons.remove(&b);
                }
                pressed.inject(input::send_button(button(b), down));
            }
            Message::Wheel { delta, horizontal } => {
                driven();
                pressed.inject(input::send_wheel(delta, horizontal));
            }
            Message::Key { vk, down } => {
                driven();
                if down {
                    pressed.keys.insert(vk);
                } else {
                    pressed.keys.remove(&vk);
                }
                pressed.inject(keyhook::send_keys(&[(vk, down)]));
            }
            Message::Clipboard(text) => shared.notice(Notice::Clipboard(text)),
            Message::Ping => {}
        }
    }
}

fn listen(shared: Arc<Shared>, listener: TcpListener) {
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = stream.set_nonblocking(false) {
                    tracing::warn!(error = %e, "Mouse Without Borders: could not take a connection");
                    continue;
                }
                let shared = Arc::clone(&shared);
                if let Err(e) = std::thread::Builder::new()
                    .name("mwb-in".into())
                    .spawn(move || run_connection(shared, stream, None))
                {
                    tracing::warn!(error = %e, "Mouse Without Borders: could not start a connection thread");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                tracing::warn!(error = %e, "Mouse Without Borders: accepting failed");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn dial(shared: Arc<Shared>, port: u16) {
    let me = shared.me.to_lowercase();
    while !shared.stop.load(Ordering::SeqCst) {
        for m in &shared.machines {
            let key = m.name.to_lowercase();
            if key <= me || lock(&shared.peers).contains_key(&key) {
                continue;
            }
            // An address may name its own port ("host:port").
            let addresses = match m
                .address
                .to_socket_addrs()
                .or_else(|_| (m.address.as_str(), port).to_socket_addrs())
            {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!("Mouse Without Borders: cannot find {}: {e}", m.address);
                    continue;
                }
            };
            for address in addresses {
                match TcpStream::connect_timeout(&address, DIAL_TIMEOUT) {
                    Ok(stream) => {
                        let (shared, name) = (Arc::clone(&shared), m.name.clone());
                        if let Err(e) = std::thread::Builder::new()
                            .name("mwb-out".into())
                            .spawn(move || run_connection(shared, stream, Some(name)))
                        {
                            tracing::warn!(error = %e, "Mouse Without Borders: could not start a connection thread");
                        }
                        break;
                    }
                    Err(e) => tracing::debug!("Mouse Without Borders: {} at {address}: {e}", m.name),
                }
            }
        }
        let until = Instant::now() + DIAL_EVERY;
        while Instant::now() < until && !shared.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub struct Net {
    shared: Arc<Shared>,
    listener: Option<JoinHandle<()>>,
}

impl Net {
    pub fn start(me: String, machines: Vec<Machine>, key: &str, port: u16, screen: Rect) -> Result<Net> {
        let mut master = [0u8; 32];
        crypto::pbkdf2_sha256(key.as_bytes(), SALT, ITERATIONS, &mut master)?;
        let listener = TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("cannot listen on port {port}"))?;
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            me,
            machines,
            master,
            screen: Mutex::new(screen),
            peers: Mutex::new(HashMap::new()),
            notices: Mutex::new(Vec::new()),
            driven_at: Mutex::new(None),
            stop: AtomicBool::new(false),
            next_id: AtomicU64::new(0),
            pending: AtomicUsize::new(0),
        });
        let s = Arc::clone(&shared);
        let listener = std::thread::Builder::new()
            .name("mwb-listen".into())
            .spawn(move || listen(s, listener))?;
        let net = Net {
            shared: Arc::clone(&shared),
            listener: Some(listener),
        };
        std::thread::Builder::new()
            .name("mwb-dial".into())
            .spawn(move || dial(shared, port))?;
        Ok(net)
    }

    /// Each machine's screen in list order; `None` for one not connected.
    pub fn screens(&self) -> Vec<Option<Rect>> {
        let peers = lock(&self.shared.peers);
        let own = *lock(&self.shared.screen);
        self.shared
            .machines
            .iter()
            .map(|m| {
                if m.name.eq_ignore_ascii_case(&self.shared.me) {
                    Some(own)
                } else {
                    peers.get(&m.name.to_lowercase()).map(|p| p.screen)
                }
            })
            .collect()
    }

    /// Whether the machine is connected to take the message.
    pub fn send(&self, machine: &str, m: Message) -> bool {
        lock(&self.shared.peers)
            .get(&machine.to_lowercase())
            .is_some_and(|p| p.tx.send(m).is_ok())
    }

    pub fn broadcast(&self, m: Message) {
        for p in lock(&self.shared.peers).values() {
            // A closed channel means the connection is ending; its reader removes it.
            let _ = p.tx.send(m.clone());
        }
    }

    pub fn set_screen(&self, screen: Rect) {
        *lock(&self.shared.screen) = screen;
        self.broadcast(Message::Hello {
            name: self.shared.me.clone(),
            screen,
        });
    }

    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *lock(&self.shared.notices))
    }

    /// Whether another machine moved this one's pointer or typed on it just now.
    pub fn driven(&self) -> bool {
        lock(&self.shared.driven_at).is_some_and(|t| t.elapsed() < DRIVEN)
    }

    pub fn connected(&self) -> Vec<String> {
        let mut names: Vec<String> = lock(&self.shared.peers).values().map(|p| p.name.clone()).collect();
        names.sort();
        names
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for (_, p) in lock(&self.shared.peers).drain() {
            let _ = p.stream.shutdown(Shutdown::Both);
        }
        // The port must be free again before a restart binds it.
        if let Some(l) = self.listener.take() {
            if l.join().is_err() {
                tracing::warn!("Mouse Without Borders: the listening thread panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machines() -> Vec<Machine> {
        crate::machines(&["ALPHA 127.0.0.1:15191".into(), "BETA 127.0.0.1:15190".into()])
    }

    #[track_caller]
    fn wait_for(net: &Net, what: impl Fn(&Notice) -> bool) -> Notice {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(n) = net.take_notices().into_iter().find(&what) {
                return n;
            }
            assert!(Instant::now() < until, "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_keys_for_each_direction_differ_and_match_the_other_end() {
        let master = [7u8; 32];
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let (a_send, a_receive) = keys(&master, &a, &b).unwrap();
        let (b_send, b_receive) = keys(&master, &b, &a).unwrap();
        assert_eq!(a_send, b_receive);
        assert_eq!(b_send, a_receive);
        assert_ne!(a_send, a_receive);
    }

    #[test]
    fn frames_open_only_in_order_and_with_the_same_key() {
        let mut send = Channel::new([3; 32]).unwrap();
        let mut wire = Vec::new();
        send.write(&mut wire, &Message::Ping).unwrap();
        send.write(&mut wire, &Message::Move { x: 1, y: 2 }).unwrap();

        let mut receive = Channel::new([3; 32]).unwrap();
        let mut r = &wire[..];
        assert_eq!(receive.read(&mut r).unwrap(), Message::Ping);
        assert_eq!(receive.read(&mut r).unwrap(), Message::Move { x: 1, y: 2 });

        // The second frame alone, where the first was expected: a replay.
        let first_len = 4 + u32::from_le_bytes(wire[..4].try_into().unwrap()) as usize;
        let mut fresh = Channel::new([3; 32]).unwrap();
        assert!(fresh.read(&mut &wire[first_len..]).unwrap_err().is::<WrongKey>());
        let mut other = Channel::new([4; 32]).unwrap();
        assert!(other.read(&mut &wire[..]).unwrap_err().is::<WrongKey>());
    }

    /// Two machines on this PC: BETA listens, ALPHA (first by name) dials it.
    #[test]
    fn machines_with_the_same_key_connect_and_others_are_refused() {
        let port = 15190;
        let beta = Net::start(
            "BETA".into(),
            machines(),
            "0123456789abcdefXYZ",
            port,
            Rect::new(0, 0, 800, 600),
        )
        .unwrap();
        let alpha = {
            // Not BETA's port, which is taken on this PC.
            Net::start(
                "ALPHA".into(),
                machines(),
                "0123456789abcdefXYZ",
                port + 1,
                Rect::new(0, 0, 1920, 1080),
            )
            .unwrap()
        };
        assert_eq!(
            wait_for(&beta, |n| matches!(n, Notice::Connected(_))),
            Notice::Connected("ALPHA".into())
        );
        wait_for(&alpha, |n| matches!(n, Notice::Connected(_)));
        assert_eq!(
            alpha.screens(),
            [Some(Rect::new(0, 0, 1920, 1080)), Some(Rect::new(0, 0, 800, 600))]
        );
        assert_eq!(beta.connected(), ["ALPHA"]);

        alpha.broadcast(Message::Clipboard("copied".into()));
        assert_eq!(
            wait_for(&beta, |n| matches!(n, Notice::Clipboard(_))),
            Notice::Clipboard("copied".into())
        );
        drop(alpha);
        wait_for(&beta, |n| matches!(n, Notice::Disconnected(_)));

        let stranger = Net::start(
            "ALPHA".into(),
            machines(),
            "a different key!!",
            port + 2,
            Rect::new(0, 0, 10, 10),
        )
        .unwrap();
        assert!(matches!(
            wait_for(&beta, |n| matches!(n, Notice::WrongKey(_))),
            Notice::WrongKey(a) if a == "127.0.0.1"
        ));
        assert!(stranger.connected().is_empty());
    }
}
