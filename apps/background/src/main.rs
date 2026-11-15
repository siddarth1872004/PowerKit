//! `powerkit.exe`: the background process.
//!
//! * `powerkit`             start (or, if already running, open the settings window)
//! * `powerkit --settings`  start and open the settings window
//! * `powerkit --quit`      ask the running instance to exit
//! * `powerkit --rename P…` open PowerRename with the given paths (Explorer menu)
//! * `powerkit --version`
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use powerkit_config::{LogLevel, Paths};
use powerkit_core::{Event, Shortcut, TrayEvent};
use powerkit_host::{builtin_modules, Backend, Host, Output};
use powerkit_ipc::{Client, Request, Response};
use powerkit_logging::LogHandle;
use powerkit_platform::event_loop::{hook_global_window_events, EventLoop};
use powerkit_platform::hotkey;
use powerkit_platform::pipe::{PipeServer, Reply};
use powerkit_platform::shell::{self, SingleInstance};
use powerkit_platform::tray::{MenuItem, Tray};
use serde_json::{json, Value};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

const STARTUP_VALUE: &str = "PowerKit";
const MENU_SETTINGS: u32 = 1;
const MENU_QUIT: u32 = 2;
const MENU_MODULE_BASE: u32 = 100;

struct OsBackend {
    log: Option<LogHandle>,
}

impl Backend for OsBackend {
    fn register_hotkey(&mut self, id: i32, shortcut: Shortcut) -> Result<(), String> {
        hotkey::register(id, shortcut).map_err(|e| e.to_string())
    }

    fn unregister_hotkey(&mut self, id: i32) {
        if let Err(e) = hotkey::unregister(id) {
            tracing::debug!("unregistering hotkey {id}: {e}");
        }
    }

    fn set_launch_at_startup(&mut self, enabled: bool) -> Result<(), String> {
        let command = if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            Some(format!("\"{}\"", exe.display()))
        } else {
            None
        };
        // Avoid touching the registry when nothing would change.
        let current = shell::launch_at_startup_command(STARTUP_VALUE).map_err(|e| e.to_string())?;
        if current == command {
            return Ok(());
        }
        shell::set_launch_at_startup(STARTUP_VALUE, command.as_deref()).map_err(|e| e.to_string())
    }

    fn set_log_level(&mut self, level: LogLevel) {
        if let Some(log) = &self.log {
            log.set_level(level);
        }
    }
}

/// Start the settings application that ships next to this executable. The path is
/// fixed and arguments are passed without a shell.
fn open_settings(page: Option<&str>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let settings = exe.with_file_name("powerkit-settings.exe");
    if !settings.exists() {
        return Err(format!("{} is missing; reinstall PowerKit", settings.display()));
    }
    let mut cmd = std::process::Command::new(settings);
    if let Some(page) = page.filter(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        cmd.arg(format!("--page={page}"));
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open settings: {e}"))
}

type IpcJob = (Request, mpsc::SyncSender<Response>);

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--version") {
        println!("PowerKit {}", env!("CARGO_PKG_VERSION"));
        return 0.into();
    }
    if has("--quit") {
        return match Client::connect().and_then(|mut c| c.call("app.quit", Value::Null)) {
            Ok(_) | Err(powerkit_ipc::IpcError::NotRunning) => 0.into(),
            Err(e) => {
                eprintln!("{e}");
                1.into()
            }
        };
    }

    let rename: Vec<String> = match args.iter().position(|a| a == "--rename") {
        Some(i) => args[i + 1..].to_vec(),
        None => Vec::new(),
    };
    let rename_request =
        (!rename.is_empty()).then(|| json!({ "id": power_rename::ID, "command": "open", "args": { "paths": rename } }));

    let pipe_name = powerkit_ipc::pipe_name();
    let instance = match SingleInstance::acquire(&format!("{pipe_name}.lock")) {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            // Already running: starting PowerKit again opens the settings window.
            let forwarded =
                Client::connect_to(&pipe_name, Duration::from_secs(3)).and_then(|mut c| match &rename_request {
                    Some(params) => c.call("module.command", params.clone()),
                    None => c.call("app.open_settings", Value::Null),
                });
            return match forwarded {
                Ok(_) => 0.into(),
                Err(e) => {
                    eprintln!("{e}");
                    1.into()
                }
            };
        }
        Err(e) => {
            eprintln!("{e}");
            return 1.into();
        }
    };

    powerkit_platform::init_dpi_awareness();
    let paths = Paths::resolve();
    let log = match powerkit_logging::init(&paths.logs(), "powerkit", LogLevel::Info) {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("logging disabled: {e}");
            None
        }
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), data_dir = %paths.root.display(), "PowerKit starting");

    let code = match run(paths, log, &pipe_name, has("--settings"), rename_request) {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!("fatal: {e:#}");
            eprintln!("{e:#}");
            1
        }
    };
    drop(instance);
    tracing::info!("PowerKit exited");
    code.into()
}

fn run(
    paths: Paths,
    log: Option<LogHandle>,
    pipe_name: &str,
    show_settings: bool,
    rename_request: Option<Value>,
) -> anyhow::Result<()> {
    let mut event_loop = EventLoop::new()?;
    let waker = event_loop.waker();
    let tray = Tray::new("PowerKit")?;
    // Kept alive for the lifetime of the loop; dropped (unhooked) on exit.
    let _hooks = hook_global_window_events()?;

    let mut host = Host::new(paths, builtin_modules(), Box::new(OsBackend { log: log.clone() }));
    if let Some(log) = &log {
        log.set_level(host.config().general.log_level);
    }

    // IPC: pipe threads hand requests to the main thread and wait for the answer.
    let (tx, rx) = mpsc::channel::<IpcJob>();
    let tx = std::sync::Mutex::new(tx);
    let server = PipeServer::start(
        pipe_name,
        Arc::new(move |line: &str| {
            let request: Request = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(e) => return Reply::Line(Response::err(0, format!("invalid request: {e}")).to_line()),
            };
            let id = request.id;
            match request.method.as_str() {
                "subscribe" => return Reply::Subscribe(Response::ok(id, Value::Null).to_line()),
                "ping" => {
                    let info = json!({ "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() });
                    return Reply::Line(Response::ok(id, info).to_line());
                }
                _ => {}
            }
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            let sent = tx
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .send((request, reply_tx))
                .is_ok();
            if sent {
                waker.wake();
            }
            let response = match reply_rx.recv_timeout(Duration::from_secs(30)) {
                Ok(r) if sent => r,
                _ => Response::err(id, "PowerKit is busy or shutting down"),
            };
            Reply::Line(response.to_line())
        }),
    )?;

    host.start();
    if let Some(params) = rename_request {
        // The command opens the PowerRename page itself.
        if let Err(e) = host.handle_request("module.command", &params) {
            tracing::error!("--rename: {e}");
            let _ = tray.notify("PowerRename", &e.to_string());
        }
    } else if show_settings || !host.config().general.start_minimized {
        host.handle_request("app.open_settings", &Value::Null).ok();
    }

    let mut flush = |host: &mut Host| -> bool {
        let mut keep_running = true;
        for output in host.take_outputs() {
            match output {
                Output::Notify { title, message } => {
                    if let Err(e) = tray.notify(&title, &message) {
                        tracing::warn!("notification not shown: {e}");
                    }
                }
                Output::Event { event, payload } => {
                    let msg = powerkit_ipc::EventMessage { event, payload };
                    if let Ok(line) = serde_json::to_string(&msg) {
                        server.broadcast(&line);
                    }
                }
                Output::OpenSettings { page } => {
                    if let Err(e) = open_settings(page.as_deref()) {
                        tracing::error!("{e}");
                        let _ = tray.notify("PowerKit", &e);
                    }
                }
                Output::Quit => keep_running = false,
            }
        }
        keep_running
    };
    flush(&mut host);

    event_loop.run(|event| {
        match event {
            Event::Quit => return false,
            Event::Wake => {
                while let Ok((request, reply)) = rx.try_recv() {
                    let response = match host.handle_request(&request.method, &request.params) {
                        Ok(v) => Response::ok(request.id, v),
                        Err(e) => Response::err(request.id, e),
                    };
                    let _ = reply.send(response);
                }
            }
            Event::Tray(TrayEvent::Activate) => {
                host.handle_request("app.open_settings", &Value::Null).ok();
            }
            Event::Tray(TrayEvent::ContextMenu) => {
                let status = host.status();
                let modules = status["modules"].as_array().cloned().unwrap_or_default();
                let mut items = vec![MenuItem::new(MENU_SETTINGS, "Open settings"), MenuItem::Separator];
                for (i, m) in modules.iter().enumerate() {
                    items.push(MenuItem::checked(
                        MENU_MODULE_BASE + i as u32,
                        m["name"].as_str().unwrap_or("?"),
                        m["enabled"].as_bool().unwrap_or(false),
                    ));
                }
                items.push(MenuItem::Separator);
                items.push(MenuItem::new(MENU_QUIT, "Quit PowerKit"));
                match tray.show_menu(&items) {
                    Ok(Some(MENU_SETTINGS)) => {
                        host.handle_request("app.open_settings", &Value::Null).ok();
                    }
                    Ok(Some(MENU_QUIT)) => return false,
                    Ok(Some(id)) if id >= MENU_MODULE_BASE => {
                        if let Some(m) = modules.get((id - MENU_MODULE_BASE) as usize) {
                            let params = json!({ "id": m["id"], "enabled": !m["enabled"].as_bool().unwrap_or(false) });
                            host.handle_request("module.set_enabled", &params).ok();
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("tray menu: {e}"),
                }
            }
            other => host.dispatch(&other),
        }
        flush(&mut host)
    });

    host.shutdown();
    flush(&mut host);
    Ok(())
}
