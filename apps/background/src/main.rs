//! `powerkit.exe`: the background process.
//!
//! * `powerkit`             start (or, if already running, open the settings window)
//! * `powerkit --settings`  start and open the settings window
//! * `powerkit --quit`      ask the running instance to exit
//! * `powerkit --rename P…` open PowerRename with the given paths (Explorer menu)
//! * `powerkit --hosts`     open the Hosts File Editor
//! * `powerkit --write-hosts F`  copy F over the hosts file (run elevated by the editor)
//! * `powerkit --env`       open Environment Variables
//! * `powerkit --write-env F`  apply the system variable changes in F (run elevated)
//! * `powerkit --reg [F]`   open Registry Preview, with the .reg file F if given
//! * `powerkit --new DIR`   show the New+ template menu for folder DIR (Explorer menu)
//! * `powerkit --unlock P…` show which processes use the given files (Explorer menu)
//! * `powerkit --resize P…` resize the given pictures (Explorer menu)
//! * `powerkit --workspace NAME`  launch the saved workspace NAME
//! * `powerkit --trust-menu-cert HEX`  trust the Explorer menu's signing certificate
//!   for the machine (run elevated)
//! * `powerkit --version`
//! * `powerkit --after-update`  start once the previous version has exited
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use powerkit_config::{LogLevel, Paths};
use powerkit_core::{Event, Shortcut, TrayEvent};
use powerkit_host::env::EnvUi;
use powerkit_host::explorer_menu;
use powerkit_host::hosts::HostsUi;
use powerkit_host::locksmith::LocksmithUi;
use powerkit_host::registry::RegistryUi;
use powerkit_host::rename::RenameUi;
use powerkit_host::resizer::ResizerUi;
use powerkit_host::settings::SettingsUi;
use powerkit_host::update::{self, Release};
use powerkit_host::workspace_editor::WorkspacesUi;
use powerkit_host::{builtin_modules, Backend, Host, Output};
use powerkit_ipc::{Client, Request, Response};
use powerkit_logging::LogHandle;
use powerkit_platform::event_loop::{hook_global_window_events, EventLoop};
use powerkit_platform::hotkey;
use powerkit_platform::pipe::{PipeServer, Reply};
use powerkit_platform::shell::{self, SingleInstance};
use powerkit_platform::tray::{MenuItem, Tray};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const STARTUP_VALUE: &str = "PowerKit";
const MENU_SETTINGS: u32 = 1;
const MENU_QUIT: u32 = 2;
const MENU_CHECK_UPDATES: u32 = 3;
const MENU_INSTALL_UPDATE: u32 = 4;
const MENU_HOSTS: u32 = 5;
const MENU_ENV: u32 = 6;
const MENU_REGISTRY: u32 = 7;
/// The first automatic check waits so it does not slow down signing in.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(120);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
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

type IpcJob = (Request, mpsc::SyncSender<Response>);

enum UpdateMsg {
    /// `manual`: asked for from the tray, so "up to date" and failures are reported too.
    Checked {
        manual: bool,
        result: anyhow::Result<Option<Release>>,
    },
    Installed(anyhow::Result<PathBuf>),
}

fn spawn_check(tx: mpsc::Sender<UpdateMsg>, waker: powerkit_platform::event_loop::Waker) {
    std::thread::spawn(move || {
        let result = update::check();
        if tx.send(UpdateMsg::Checked { manual: true, result }).is_ok() {
            waker.wake();
        }
    });
}

/// Checks once a day while `enabled` is set, until the main thread is gone.
fn spawn_daily_check(
    tx: mpsc::Sender<UpdateMsg>,
    waker: powerkit_platform::event_loop::Waker,
    enabled: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK_DELAY);
        loop {
            if enabled.load(Ordering::Relaxed) {
                let result = update::check();
                if tx.send(UpdateMsg::Checked { manual: false, result }).is_err() {
                    return;
                }
                waker.wake();
            }
            std::thread::sleep(CHECK_INTERVAL);
        }
    });
}

fn spawn_menu_job(
    wanted: bool,
    folder: PathBuf,
    tx: mpsc::Sender<(bool, Result<(), String>)>,
    waker: powerkit_platform::event_loop::Waker,
) {
    std::thread::spawn(move || {
        let result = if wanted {
            std::env::current_exe()
                .map_err(|e| format!("finding powerkit.exe: {e}"))
                .and_then(|exe| explorer_menu::install(&folder, &exe))
        } else {
            explorer_menu::uninstall(&folder)
        };
        if tx.send((wanted, result)).is_ok() {
            waker.wake();
        }
    });
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if let Some(i) = args.iter().position(|a| a == "--write-hosts") {
        use hosts_editor::hosts::{exit, install, system_path};
        let code = match args.get(i + 1) {
            Some(src) => install(Path::new(src), &system_path()).err().unwrap_or(exit::OK),
            None => exit::USAGE,
        };
        return code.into();
    }
    if let Some(i) = args.iter().position(|a| a == "--write-env") {
        use environment_variables::vars::{exit, read_changes};
        let code = match args.get(i + 1).map(|f| read_changes(Path::new(f))) {
            None => exit::USAGE,
            Some(Err(e)) => {
                eprintln!("{e}");
                exit::BAD_SOURCE
            }
            Some(Ok(changes)) => match powerkit_host::env::apply(true, &changes) {
                Ok(()) => {
                    powerkit_platform::env::broadcast();
                    exit::OK
                }
                Err(e) => {
                    eprintln!("{e}");
                    exit::WRITE_FAILED
                }
            },
        };
        return code.into();
    }
    if let Some(i) = args.iter().position(|a| a == explorer_menu::TRUST_FLAG) {
        return match explorer_menu::trust_elevated(args.get(i + 1).map_or("", String::as_str)) {
            Ok(()) => 0.into(),
            Err(e) => {
                eprintln!("{e}");
                1.into()
            }
        };
    }
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
    // The running instance may have another working directory.
    let absolute_after = |flag: &str| -> Vec<PathBuf> {
        match args.iter().position(|a| a == flag) {
            Some(i) => args[i + 1..]
                .iter()
                .map(|f| std::path::absolute(f).unwrap_or_else(|_| PathBuf::from(f)))
                .collect(),
            None => Vec::new(),
        }
    };
    let unlock = absolute_after("--unlock");
    let resize = absolute_after("--resize");
    // A module command asked for on the command line, run by this or the running instance.
    let request = if !rename.is_empty() {
        Some(json!({ "id": power_rename::ID, "command": "open", "args": { "paths": rename } }))
    } else if !unlock.is_empty() {
        Some(json!({ "id": file_locksmith::ID, "command": "open", "args": { "paths": unlock } }))
    } else if !resize.is_empty() {
        Some(json!({ "id": image_resizer::ID, "command": "open", "args": { "paths": resize } }))
    } else if let Some(name) = args
        .iter()
        .position(|a| a == "--workspace")
        .and_then(|i| args.get(i + 1))
    {
        Some(json!({ "id": workspaces::ID, "command": "launch", "args": { "name": name } }))
    } else if has("--hosts") {
        Some(json!({ "id": hosts_editor::ID, "command": "open" }))
    } else if has("--env") {
        Some(json!({ "id": environment_variables::ID, "command": "open" }))
    } else if let Some(i) = args.iter().position(|a| a == "--reg") {
        // The running instance may have another working directory.
        let path = args
            .get(i + 1)
            .map(|f| std::path::absolute(f).unwrap_or_else(|_| PathBuf::from(f)));
        Some(json!({ "id": registry_preview::ID, "command": "open", "args": { "path": path } }))
    } else if let Some(folder) = args.iter().position(|a| a == "--new").and_then(|i| args.get(i + 1)) {
        let folder = std::path::absolute(folder).unwrap_or_else(|_| PathBuf::from(folder));
        Some(json!({ "id": new_plus::ID, "command": "new", "args": { "folder": folder } }))
    } else {
        None
    };

    let pipe_name = powerkit_ipc::pipe_name();
    let lock = format!("{pipe_name}.lock");
    let mut acquired = SingleInstance::acquire(&lock);
    if has("--after-update") {
        // The version that started this one is still shutting down.
        let deadline = Instant::now() + Duration::from_secs(15);
        while matches!(acquired, Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            acquired = SingleInstance::acquire(&lock);
        }
    }
    let instance = match acquired {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            // Already running: starting PowerKit again opens the settings window. The
            // instance that shows it may need to come to the front.
            powerkit_platform::shell::allow_foreground();
            let forwarded = Client::connect_to(&pipe_name, Duration::from_secs(3)).and_then(|mut c| match &request {
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

    update::remove_leftover();
    let code = match run(
        paths,
        log,
        &pipe_name,
        has("--settings"),
        has("--after-update"),
        request,
    ) {
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

/// The windows the host shows, opened by module outputs and fed their own events.
struct Windows {
    settings: SettingsUi,
    rename: RenameUi,
    hosts: HostsUi,
    env: EnvUi,
    registry: RegistryUi,
    locksmith: LocksmithUi,
    resizer: ResizerUi,
    workspaces: WorkspacesUi,
}

fn run(
    paths: Paths,
    log: Option<LogHandle>,
    pipe_name: &str,
    show_settings: bool,
    after_update: bool,
    request: Option<Value>,
) -> anyhow::Result<()> {
    let mut event_loop = EventLoop::new()?;
    let waker = event_loop.waker();
    let event_loop_waker = event_loop.waker();
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
    if after_update {
        let _ = tray.notify("PowerKit", &format!("Updated to version {}.", update::CURRENT));
    }
    let (update_tx, update_rx) = mpsc::channel::<UpdateMsg>();
    let auto_check = Arc::new(AtomicBool::new(host.config().general.check_for_updates));
    spawn_daily_check(update_tx.clone(), event_loop.waker(), Arc::clone(&auto_check));
    // The Explorer menu package, installed or removed off this thread. `menu_applied`
    // is the setting last acted on; a failure is reported once, not retried.
    let (menu_tx, menu_rx) = mpsc::channel::<(bool, Result<(), String>)>();
    let menu_waker = event_loop.waker();
    let menu_folder = explorer_menu::folder(&host.paths().root);
    let mut menu_applied = None;
    let mut menu_busy = false;
    let wanted = host.config().general.explorer_menu;
    if wanted || explorer_menu::maybe_installed() {
        menu_busy = true;
        spawn_menu_job(wanted, menu_folder.clone(), menu_tx.clone(), menu_waker);
    } else {
        menu_applied = Some(false);
    }
    let mut available: Option<Release> = None;
    let mut update_busy = false;
    if let Some(params) = request {
        // The command opens its window itself.
        if let Err(e) = host.handle_request("module.command", &params) {
            tracing::error!("command line: {e}");
            let _ = tray.notify("PowerKit", &e.to_string());
        }
    } else if show_settings || !host.config().general.start_minimized {
        host.handle_request("app.open_settings", &Value::Null).ok();
    }

    let mut ui = Windows {
        settings: SettingsUi::default(),
        rename: RenameUi::default(),
        hosts: HostsUi::new(event_loop.waker()),
        env: EnvUi::new(event_loop.waker()),
        registry: RegistryUi::new(event_loop.waker()),
        locksmith: LocksmithUi::new(event_loop.waker()),
        resizer: ResizerUi::new(event_loop.waker()),
        workspaces: WorkspacesUi::new(event_loop.waker()),
    };
    let flush = |host: &mut Host, ui: &mut Windows| -> bool {
        let mut keep_running = true;
        for output in host.take_outputs() {
            match output {
                Output::Notify { title, message } => {
                    if let Err(e) = tray.notify(&title, &message) {
                        tracing::warn!("notification not shown: {e}");
                    }
                }
                Output::Event { event, payload } => {
                    let opened = match event.as_str() {
                        "power_rename.pending" => ui.rename.open(host),
                        hosts_editor::OPEN_EVENT => ui.hosts.open(),
                        environment_variables::OPEN_EVENT => ui.env.open(),
                        registry_preview::OPEN_EVENT => ui.registry.open(payload["path"].as_str().map(PathBuf::from)),
                        new_plus::OPEN_EVENT => powerkit_host::new_menu::show(
                            Path::new(payload["folder"].as_str().unwrap_or_default()),
                            &host.config().new_plus,
                            &host.paths().root,
                            &tray,
                        ),
                        file_locksmith::OPEN_EVENT => ui.locksmith.open(
                            payload["paths"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(PathBuf::from)
                                .collect(),
                        ),
                        image_resizer::OPEN_EVENT => ui.resizer.open(
                            payload["paths"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(PathBuf::from)
                                .collect(),
                            host,
                        ),
                        workspaces::OPEN_EVENT => ui.workspaces.open(host),
                        workspaces::LAUNCH_EVENT => {
                            ui.workspaces.launch(payload["name"].as_str().unwrap_or_default(), host)
                        }
                        _ => Ok(()),
                    };
                    if let Err(e) = opened {
                        tracing::error!("{e}");
                        let _ = tray.notify("PowerKit", &e);
                    }
                    let msg = powerkit_ipc::EventMessage { event, payload };
                    if let Ok(line) = serde_json::to_string(&msg) {
                        server.broadcast(&line);
                    }
                }
                Output::OpenSettings { page } => {
                    if let Err(e) = ui.settings.open(host, page.as_deref()) {
                        tracing::error!("{e}");
                        let _ = tray.notify("PowerKit", &e);
                    }
                }
                Output::Quit => keep_running = false,
            }
        }
        keep_running
    };
    flush(&mut host, &mut ui);

    event_loop.run(|event| {
        match event {
            Event::Quit => return false,
            Event::Wake => {
                ui.hosts.poll();
                ui.env.poll();
                ui.registry.poll();
                ui.locksmith.poll();
                ui.resizer.poll();
                if let Some(outcome) = ui.workspaces.poll() {
                    if let Err(e) = tray.notify("Workspaces", &outcome) {
                        tracing::warn!("notification not shown: {e}");
                    }
                }
                while let Ok((request, reply)) = rx.try_recv() {
                    let response = match host.handle_request(&request.method, &request.params) {
                        Ok(v) => Response::ok(request.id, v),
                        Err(e) => Response::err(request.id, e),
                    };
                    let _ = reply.send(response);
                }
                while let Ok((wanted, result)) = menu_rx.try_recv() {
                    menu_busy = false;
                    menu_applied = Some(wanted);
                    if let Err(e) = result {
                        tracing::error!(wanted, "Explorer menu: {e}");
                        let _ = tray.notify("PowerKit", &format!("The Windows 11 context menu entries could not be changed: {e}"));
                    }
                    host.reapply();
                }
                while let Ok(msg) = update_rx.try_recv() {
                    let note = |message: &str| {
                        if let Err(e) = tray.notify("PowerKit update", message) {
                            tracing::warn!("notification not shown: {e}");
                        }
                    };
                    match msg {
                        UpdateMsg::Checked { manual, result } => {
                            update_busy = update_busy && !manual;
                            match result {
                                Ok(Some(r)) => {
                                    let known = available.as_ref().is_some_and(|a| a.version == r.version);
                                    if manual || !known {
                                        tracing::info!(version = %r.version, page = %r.page, "update available");
                                        note(&format!(
                                            "PowerKit {} is available. Right-click the tray icon and choose Install update.",
                                            r.version
                                        ));
                                    }
                                    available = Some(r);
                                }
                                Ok(None) if manual => {
                                    note(&format!("You have the latest version ({}).", update::CURRENT));
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    tracing::warn!("update check failed: {e:#}");
                                    if manual {
                                        note(&format!("Could not check for updates: {e:#}"));
                                    }
                                }
                            }
                        }
                        UpdateMsg::Installed(Ok(exe)) => {
                            tracing::info!("update installed; restarting");
                            match std::process::Command::new(&exe).arg("--after-update").spawn() {
                                Ok(_) => return false,
                                Err(e) => {
                                    update_busy = false;
                                    note(&format!("The update is installed but PowerKit could not restart: {e}. Start it again."));
                                }
                            }
                        }
                        UpdateMsg::Installed(Err(e)) => {
                            update_busy = false;
                            tracing::error!("update failed: {e:#}");
                            note(&format!("The update failed: {e:#}"));
                        }
                    }
                }
            }
            Event::Tray(TrayEvent::Activate) => {
                host.handle_request("app.open_settings", &Value::Null).ok();
            }
            Event::Tray(TrayEvent::ContextMenu) => {
                let status = host.status();
                let modules = status["modules"].as_array().cloned().unwrap_or_default();
                let mut items = vec![MenuItem::new(MENU_SETTINGS, "Open settings")];
                if host.config().hosts_editor.enabled {
                    items.push(MenuItem::new(MENU_HOSTS, "Hosts File Editor"));
                }
                if host.config().environment_variables.enabled {
                    items.push(MenuItem::new(MENU_ENV, "Environment Variables"));
                }
                if host.config().registry_preview.enabled {
                    items.push(MenuItem::new(MENU_REGISTRY, "Registry Preview"));
                }
                items.push(match &available {
                    Some(r) => MenuItem::new(MENU_INSTALL_UPDATE, format!("Install update {}", r.version)),
                    None => MenuItem::new(MENU_CHECK_UPDATES, "Check for updates"),
                });
                items.push(MenuItem::Separator);
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
                    Ok(Some(id @ (MENU_HOSTS | MENU_ENV | MENU_REGISTRY))) => {
                        let module = match id {
                            MENU_HOSTS => hosts_editor::ID,
                            MENU_ENV => environment_variables::ID,
                            _ => registry_preview::ID,
                        };
                        let params = json!({ "id": module, "command": "open" });
                        if let Err(e) = host.handle_request("module.command", &params) {
                            let _ = tray.notify("PowerKit", &e);
                        }
                    }
                    Ok(Some(MENU_QUIT)) => return false,
                    Ok(Some(MENU_CHECK_UPDATES | MENU_INSTALL_UPDATE)) if update_busy => {
                        let _ = tray.notify("PowerKit update", "Already working on it.");
                    }
                    Ok(Some(MENU_CHECK_UPDATES)) => {
                        update_busy = true;
                        spawn_check(update_tx.clone(), event_loop_waker);
                    }
                    Ok(Some(MENU_INSTALL_UPDATE)) => {
                        if let Some(r) = available.clone() {
                            update_busy = true;
                            let _ = tray.notify("PowerKit update", &format!("Downloading PowerKit {}...", r.version));
                            let tx = update_tx.clone();
                            std::thread::spawn(move || {
                                if tx.send(UpdateMsg::Installed(update::install(&r))).is_ok() {
                                    event_loop_waker.wake();
                                }
                            });
                        }
                    }
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
            Event::Settings(e) => ui.settings.on_event(e, &mut host),
            Event::Rename(e) => ui.rename.on_event(e, &mut host),
            Event::Table { table, event } if table == powerkit_host::hosts::TABLE => ui.hosts.on_event(event, &host),
            Event::Table { table, event } if table == powerkit_host::env::TABLE => ui.env.on_event(event, &host),
            Event::Table { table, event } if table == powerkit_host::locksmith::TABLE => ui.locksmith.on_event(event),
            Event::Table { table, event } if table == powerkit_host::resizer::TABLE => ui.resizer.on_event(event, &mut host),
            Event::Table { table, event } if table == powerkit_host::workspace_editor::TABLE => {
                ui.workspaces.on_event(event, &mut host)
            }
            Event::Registry(e) => ui.registry.on_event(e, &host.paths().root),
            other => host.dispatch(&other),
        }
        auto_check.store(host.config().general.check_for_updates, Ordering::Relaxed);
        let wanted = host.config().general.explorer_menu;
        if !menu_busy && menu_applied != Some(wanted) {
            menu_busy = true;
            spawn_menu_job(wanted, menu_folder.clone(), menu_tx.clone(), menu_waker);
        }
        flush(&mut host, &mut ui)
    });

    host.shutdown();
    flush(&mut host, &mut ui);
    Ok(())
}
