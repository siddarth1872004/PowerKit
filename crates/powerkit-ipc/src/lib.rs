//! IPC between the background host (`powerkit.exe`) and its clients (settings UI,
//! second instances, scripts).
//!
//! Transport: a per-user named pipe carrying newline-delimited JSON.
//! * RPC connection: the client writes a [`Request`] line and reads one [`Response`]
//!   line, any number of times.
//! * Event connection: the client sends the `subscribe` method once; afterwards the
//!   host pushes [`EventMessage`] lines and the client only reads.
//!
//! The full method list is documented in `docs/IPC.md`. There is intentionally no
//! method that runs an arbitrary program or shell command.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(error.into()),
        }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("Response serialization cannot fail")
    }
}

/// Pushed from the host to subscribed clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventMessage {
    pub event: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("PowerKit is not running")]
    NotRunning,
    #[error("could not talk to PowerKit: {0}")]
    Io(#[from] std::io::Error),
    #[error("PowerKit sent an invalid reply: {0}")]
    Protocol(String),
    /// The host handled the request and reported a failure.
    #[error("{0}")]
    Remote(String),
}

/// Name of the host's pipe for the current user (without the `\\.\pipe\` prefix).
/// `POWERKIT_PIPE` overrides it so tests can run an isolated host.
pub fn pipe_name() -> String {
    if let Ok(name) = std::env::var("POWERKIT_PIPE") {
        if !name.is_empty() {
            return name;
        }
    }
    match powerkit_platform::pipe::current_user_sid() {
        Ok(sid) => format!("PowerKit.{sid}"),
        Err(_) => "PowerKit.default".to_string(),
    }
}

fn open_pipe(name: &str, timeout: Duration) -> Result<File, IpcError> {
    let path = format!(r"\\.\pipe\{name}");
    let deadline = Instant::now() + timeout;
    loop {
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => return Ok(f),
            Err(e) => {
                // ERROR_PIPE_BUSY (231): all instances are busy; the server creates a
                // new one right after accepting, so retry briefly.
                let busy = e.raw_os_error() == Some(231);
                let missing = e.kind() == std::io::ErrorKind::NotFound;
                if Instant::now() >= deadline {
                    return Err(if missing { IpcError::NotRunning } else { IpcError::Io(e) });
                }
                if !busy && !missing {
                    return Err(IpcError::Io(e));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// A blocking RPC connection to the host.
pub struct Client {
    writer: File,
    reader: BufReader<File>,
    next_id: u64,
}

impl Client {
    /// Connect to the running host, failing fast with [`IpcError::NotRunning`].
    pub fn connect() -> Result<Client, IpcError> {
        Self::connect_to(&pipe_name(), Duration::from_millis(300))
    }

    /// Connect, waiting up to `timeout` for the host to appear (used right after
    /// starting it).
    pub fn connect_to(name: &str, timeout: Duration) -> Result<Client, IpcError> {
        let file = open_pipe(name, timeout)?;
        let reader = BufReader::new(file.try_clone()?);
        Ok(Client {
            writer: file,
            reader,
            next_id: 1,
        })
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, IpcError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&request).expect("Request serialization cannot fail");
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply)? == 0 {
            return Err(IpcError::Protocol("connection closed".into()));
        }
        let response: Response =
            serde_json::from_str(reply.trim_end()).map_err(|e| IpcError::Protocol(e.to_string()))?;
        if response.id != id {
            return Err(IpcError::Protocol(format!(
                "reply id {} does not match request {id}",
                response.id
            )));
        }
        match (response.result, response.error) {
            (_, Some(e)) => Err(IpcError::Remote(e)),
            (Some(v), None) => Ok(v),
            (None, None) => Ok(Value::Null),
        }
    }

    /// Turn this connection into an event stream.
    pub fn subscribe(mut self) -> Result<EventStream, IpcError> {
        self.call("subscribe", Value::Null)?;
        Ok(EventStream { reader: self.reader })
    }
}

/// Blocking iterator over host events; ends when the host exits.
pub struct EventStream {
    reader: BufReader<File>,
}

impl Iterator for EventStream {
    type Item = EventMessage;

    fn next(&mut self) -> Option<EventMessage> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {
                    if let Ok(msg) = serde_json::from_str(line.trim_end()) {
                        return Some(msg);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_platform::pipe::{PipeServer, Reply};
    use serde_json::json;
    use std::sync::Arc;

    fn server(name: &str) -> PipeServer {
        PipeServer::start(
            name,
            Arc::new(|line: &str| {
                let req: Request = match serde_json::from_str(line) {
                    Ok(r) => r,
                    Err(e) => return Reply::Line(Response::err(0, e.to_string()).to_line()),
                };
                match req.method.as_str() {
                    "subscribe" => Reply::Subscribe(Response::ok(req.id, Value::Null).to_line()),
                    "echo" => Reply::Line(Response::ok(req.id, req.params).to_line()),
                    "fail" => Reply::Line(Response::err(req.id, "nope").to_line()),
                    _ => Reply::Line(Response::err(req.id, "unknown method").to_line()),
                }
            }),
        )
        .unwrap()
    }

    #[test]
    fn rpc_and_events() {
        let name = format!("PowerKit.Test.ipc.{}", std::process::id());
        let srv = server(&name);
        let mut c = Client::connect_to(&name, Duration::from_secs(2)).unwrap();
        assert_eq!(c.call("echo", json!({"a": [1, "ü"]})).unwrap(), json!({"a": [1, "ü"]}));
        assert!(matches!(c.call("fail", Value::Null), Err(IpcError::Remote(m)) if m == "nope"));
        assert_eq!(c.call("echo", json!(2)).unwrap(), json!(2));

        let mut events = Client::connect_to(&name, Duration::from_secs(2))
            .unwrap()
            .subscribe()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while srv.subscriber_count() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let msg = EventMessage {
            event: "config_changed".into(),
            payload: json!({"x": 1}),
        };
        srv.broadcast(&serde_json::to_string(&msg).unwrap());
        assert_eq!(events.next(), Some(msg));
        drop(srv);
    }

    #[test]
    fn not_running_is_reported() {
        let err = Client::connect_to("PowerKit.Test.absent", Duration::from_millis(50))
            .err()
            .unwrap();
        assert!(matches!(err, IpcError::NotRunning));
    }
}
