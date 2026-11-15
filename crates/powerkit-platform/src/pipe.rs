//! Named-pipe server for local IPC.
//!
//! Security boundaries:
//! * the pipe's DACL grants access to the current user only;
//! * remote clients are rejected (`PIPE_REJECT_REMOTE_CLIENTS`);
//! * the first instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so another
//!   process cannot pre-create ("squat") the name and impersonate the server;
//! * requests are size-limited lines and the number of concurrent clients is bounded.

use crate::util::{from_wide, pcwstr, wide};
use crate::{Context, Error, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Longest accepted request line.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CLIENTS: usize = 16;
const BUFFER: u32 = 64 * 1024;

/// String SID of the user running this process (`S-1-5-21-...`).
pub fn current_user_sid() -> Result<String> {
    // SAFETY: the token handle is closed; the TOKEN_USER buffer is sized by the first
    // call and outlives the SID pointer derived from it; the string SID is freed with
    // LocalFree as documented.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).context("OpenProcessToken")?;
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        let mut buf = vec![0u8; needed as usize];
        let res = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), needed, &mut needed);
        let _ = CloseHandle(token);
        res.context("GetTokenInformation")?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid_str = windows::core::PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid_str).context("ConvertSidToStringSid")?;
        let len = (0..).take_while(|i| *sid_str.0.add(*i) != 0).count();
        let out = from_wide(std::slice::from_raw_parts(sid_str.0, len));
        let _ = LocalFree(Some(HLOCAL(sid_str.0.cast())));
        Ok(out)
    }
}

/// A security descriptor allowing only the current user (and SYSTEM).
struct UserOnlySecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

// SAFETY: the descriptor is an immutable heap block; it may be used and freed from
// any thread.
unsafe impl Send for UserOnlySecurity {}

impl UserOnlySecurity {
    fn new() -> Result<Self> {
        let sddl = wide(&format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", current_user_sid()?));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated; the descriptor is freed in Drop.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(pcwstr(&sddl), SDDL_REVISION_1, &mut descriptor, None)
        }
        .context("building pipe security descriptor")?;
        Ok(Self { descriptor })
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for UserOnlySecurity {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        let _ = unsafe { LocalFree(Some(HLOCAL(self.descriptor.0))) };
    }
}

fn create_instance(name: &[u16], security: &UserOnlySecurity, first: bool) -> Result<HANDLE> {
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let sa = security.attributes();
    // SAFETY: `name` is NUL-terminated and `sa` outlives the call.
    let handle = unsafe {
        CreateNamedPipeW(
            pcwstr(name),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            Some(&sa),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(Error::Os {
            context: "CreateNamedPipe".into(),
            source: windows::core::Error::from_thread(),
        });
    }
    Ok(handle)
}

/// What the server does with a connection after handling a line.
pub enum Reply {
    /// Send this line back and keep reading requests.
    Line(String),
    /// Send this line, then turn the connection into a one-way event stream that
    /// receives everything passed to [`PipeServer::broadcast`].
    Subscribe(String),
}

type Handler = dyn Fn(&str) -> Reply + Send + Sync;

struct Shared {
    subscribers: Mutex<Vec<File>>,
    clients: AtomicUsize,
    stopping: AtomicBool,
}

/// A running pipe server. Each client is served on its own thread; `handler` is called
/// for every request line and must be quick or delegate.
pub struct PipeServer {
    shared: Arc<Shared>,
    name: String,
}

impl PipeServer {
    /// Start serving on `\\.\pipe\<name>`. Fails if another process owns the name.
    pub fn start(name: &str, handler: Arc<Handler>) -> Result<PipeServer> {
        let full = format!(r"\\.\pipe\{name}");
        let wname = wide(&full);
        let security = UserOnlySecurity::new()?;
        let first = create_instance(&wname, &security, true)?;
        let shared = Arc::new(Shared {
            subscribers: Mutex::new(Vec::new()),
            clients: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
        });
        let accept_shared = shared.clone();
        let first = first.0 as isize;
        std::thread::Builder::new()
            .name("pk-ipc-accept".into())
            .spawn(move || {
                let mut pending = HANDLE(first as *mut _);
                loop {
                    // SAFETY: `pending` is a valid pipe instance owned by this thread.
                    let connected = unsafe { ConnectNamedPipe(pending, None) };
                    // ERROR_PIPE_CONNECTED: the client connected before we waited.
                    let ok = match connected {
                        Ok(()) => true,
                        Err(e) => e.code().0 as u32 == 0x8007_0217,
                    };
                    if accept_shared.stopping.load(Ordering::SeqCst) {
                        // SAFETY: closing our own handle.
                        let _ = unsafe { CloseHandle(pending) };
                        return;
                    }
                    if ok {
                        Self::spawn_client(pending, accept_shared.clone(), handler.clone());
                    } else {
                        // SAFETY: closing our own handle.
                        let _ = unsafe { CloseHandle(pending) };
                    }
                    pending = match create_instance(&wname, &security, false) {
                        Ok(h) => h,
                        Err(e) => {
                            tracing::error!(error = %e, "IPC server cannot accept further clients");
                            return;
                        }
                    };
                }
            })
            .map_err(|e| Error::Other(format!("could not start IPC thread: {e}")))?;
        Ok(PipeServer { shared, name: full })
    }

    fn spawn_client(handle: HANDLE, shared: Arc<Shared>, handler: Arc<Handler>) {
        if shared.clients.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
            shared.clients.fetch_sub(1, Ordering::SeqCst);
            tracing::warn!("too many IPC clients; connection refused");
            // SAFETY: we own the handle and nobody else uses it.
            unsafe {
                let _ = DisconnectNamedPipe(handle);
                let _ = CloseHandle(handle);
            }
            return;
        }
        let raw = handle.0 as isize;
        let spawned = std::thread::Builder::new().name("pk-ipc-client".into()).spawn(move || {
            // SAFETY: ownership of the handle moves into the File, which closes it.
            let file = unsafe { File::from_raw_handle(raw as *mut _) };
            if let Err(e) = Self::serve_client(file, &shared, &*handler) {
                tracing::debug!(error = %e, "IPC client disconnected");
            }
            shared.clients.fetch_sub(1, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start IPC client thread");
        }
    }

    fn serve_client(file: File, shared: &Shared, handler: &Handler) -> std::io::Result<()> {
        let mut writer = file.try_clone()?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = (&mut reader)
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if n == 0 {
                return Ok(());
            }
            if line.len() > MAX_LINE_BYTES {
                return Err(std::io::Error::other("request too large"));
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\r', '\n']);
            if text.is_empty() {
                continue;
            }
            match handler(text) {
                Reply::Line(resp) => {
                    writer.write_all(resp.as_bytes())?;
                    writer.write_all(b"\n")?;
                    writer.flush()?;
                }
                Reply::Subscribe(resp) => {
                    writer.write_all(resp.as_bytes())?;
                    writer.write_all(b"\n")?;
                    writer.flush()?;
                    // From here on the connection is write-only: synchronous pipe
                    // handles serialize I/O, so we must not block in a read.
                    shared
                        .subscribers
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(writer);
                    return Ok(());
                }
            }
        }
    }

    /// Send a line to every subscribed client; clients that went away are dropped.
    pub fn broadcast(&self, line: &str) {
        let mut subs = self.shared.subscribers.lock().unwrap_or_else(|p| p.into_inner());
        subs.retain_mut(|s| {
            s.write_all(line.as_bytes())
                .and_then(|_| s.write_all(b"\n"))
                .and_then(|_| s.flush())
                .is_ok()
        });
    }

    pub fn subscriber_count(&self) -> usize {
        self.shared.subscribers.lock().unwrap_or_else(|p| p.into_inner()).len()
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Unblock the accept thread with a throwaway connection.
        let _ = std::fs::OpenOptions::new().read(true).write(true).open(&self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(name: &str) -> File {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!(r"\\.\pipe\{name}"))
            .unwrap()
    }

    fn unique(tag: &str) -> String {
        format!("PowerKit.Test.{tag}.{}", std::process::id())
    }

    #[test]
    fn sid_looks_like_a_sid() {
        assert!(current_user_sid().unwrap().starts_with("S-1-"));
    }

    #[test]
    fn request_response_and_events() {
        let name = unique("rr");
        let server = PipeServer::start(
            &name,
            Arc::new(|line: &str| {
                if line == "subscribe" {
                    Reply::Subscribe("ok".into())
                } else {
                    Reply::Line(format!("echo:{line}"))
                }
            }),
        )
        .unwrap();

        let mut c = connect(&name);
        let mut r = BufReader::new(c.try_clone().unwrap());
        for msg in ["one", "two"] {
            writeln!(c, "{msg}").unwrap();
            let mut resp = String::new();
            r.read_line(&mut resp).unwrap();
            assert_eq!(resp.trim_end(), format!("echo:{msg}"));
        }

        let mut s = connect(&name);
        writeln!(s, "subscribe").unwrap();
        let mut sr = BufReader::new(s);
        let mut resp = String::new();
        sr.read_line(&mut resp).unwrap();
        assert_eq!(resp.trim_end(), "ok");
        for _ in 0..200 {
            if server.subscriber_count() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        server.broadcast("event-1");
        resp.clear();
        sr.read_line(&mut resp).unwrap();
        assert_eq!(resp.trim_end(), "event-1");

        // A dropped subscriber is pruned on the next broadcasts.
        drop(sr);
        server.broadcast("x");
        server.broadcast("y");
        assert_eq!(server.subscriber_count(), 0);
    }

    #[test]
    fn name_cannot_be_served_twice() {
        let name = unique("dup");
        let _a = PipeServer::start(&name, Arc::new(|_: &str| Reply::Line(String::new()))).unwrap();
        assert!(PipeServer::start(&name, Arc::new(|_: &str| Reply::Line(String::new()))).is_err());
    }

    #[test]
    fn oversized_request_closes_connection() {
        let name = unique("big");
        let _server = PipeServer::start(&name, Arc::new(|_: &str| Reply::Line("ok".into()))).unwrap();
        let mut c = connect(&name);
        let chunk = vec![b'a'; 1024 * 1024];
        let mut failed = false;
        for _ in 0..6 {
            if c.write_all(&chunk).is_err() {
                failed = true;
                break;
            }
        }
        let mut buf = [0u8; 8];
        // Either the write failed or the server hung up without answering.
        assert!(failed || !matches!(c.read(&mut buf), Ok(n) if n > 0));
    }
}
