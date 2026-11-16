//! Workspaces: save the open windows as a named set and bring them back later, starting
//! the programs that are not running and putting every window where it was.

#[cfg(windows)]
mod live;
#[cfg(windows)]
pub use live::{capture, launch};

use anyhow::{bail, Result};
use powerkit_config::{Workspace, WorkspaceApp, Workspaces as Settings};
use powerkit_core::{Event, Rect};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "workspaces";
pub const OPEN_EVENT: &str = "workspaces.open";
/// Payload `{ "name": ... }`: the workspace to launch.
pub const LAUNCH_EVENT: &str = "workspaces.launch";

/// Programs whose windows cannot be brought back by starting the program's file:
/// Store apps run inside this host.
const NOT_RELAUNCHABLE: &[&str] = &["applicationframehost.exe"];

pub fn relaunchable(path: &str) -> bool {
    let file = file_name(path).to_lowercase();
    !file.is_empty() && !NOT_RELAUNCHABLE.contains(&file.as_str())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// The program's name as shown: its file name without `.exe`.
pub fn app_name(path: &str) -> &str {
    let f = file_name(path);
    match f.len().checked_sub(4) {
        Some(i) if f.is_char_boundary(i) && f[i..].eq_ignore_ascii_case(".exe") => &f[..i],
        _ => f,
    }
}

pub fn find<'a>(workspaces: &'a [Workspace], name: &str) -> Option<&'a Workspace> {
    let name = name.trim();
    workspaces.iter().find(|w| w.name.eq_ignore_ascii_case(name))
}

/// The programs of a workspace in one line, such as `Code, msedge (2), notepad`.
pub fn summary(apps: &[WorkspaceApp]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for a in apps {
        let name = app_name(&a.path);
        match counts.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, c)) => *c += 1,
            None => counts.push((name, 1)),
        }
    }
    counts
        .iter()
        .map(|(n, c)| if *c > 1 { format!("{n} ({c})") } else { (*n).to_string() })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where `app`'s window goes on the monitors there are now, given as `(id, bounds)`
/// with the primary monitor first. On its own monitor it keeps its place even if the
/// monitor moved or changed resolution; when that monitor is gone it goes to the same
/// place on the primary one. It is kept inside the monitor either way.
pub fn target(app: &WorkspaceApp, monitors: &[(String, Rect)]) -> Option<Rect> {
    let now = monitors
        .iter()
        .find(|(id, _)| *id == app.monitor)
        .or_else(|| monitors.first())
        .map(|(_, r)| *r)?;
    let then = if app.monitor_rect.is_empty() {
        now
    } else {
        app.monitor_rect
    };
    let scale = |v: i32, from: i32, from_len: i32, to: i32, to_len: i32| {
        to + ((v - from) as i64 * to_len as i64 / from_len.max(1) as i64) as i32
    };
    let x = |v| scale(v, then.left, then.width(), now.left, now.width());
    let y = |v| scale(v, then.top, then.height(), now.top, now.height());
    let r = app.rect;
    let (w, h) = (
        (x(r.right) - x(r.left)).clamp(1, now.width()),
        (y(r.bottom) - y(r.top)).clamp(1, now.height()),
    );
    let left = x(r.left).clamp(now.left, now.right - w);
    let top = y(r.top).clamp(now.top, now.bottom - h);
    Some(Rect::from_size(left, top, w, h))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Move the open window at this index in the list of open windows.
    Move {
        app: usize,
        window: usize,
    },
    Start {
        app: usize,
    },
}

/// What to do for each app: move a window of the same program that is already open
/// (front-most first, each used once) when `move_existing` is on, otherwise start
/// the program. `open` holds the program paths of the open windows, front to back.
pub fn plan(apps: &[WorkspaceApp], open: &[String], move_existing: bool) -> Vec<Step> {
    let mut used = vec![false; open.len()];
    apps.iter()
        .enumerate()
        .map(|(app, a)| {
            let found = move_existing
                .then(|| {
                    open.iter()
                        .enumerate()
                        .position(|(i, p)| !used[i] && p.eq_ignore_ascii_case(&a.path))
                })
                .flatten();
            match found {
                Some(window) => {
                    used[window] = true;
                    Step::Move { app, window }
                }
                None => Step::Start { app },
            }
        })
        .collect()
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub moved: usize,
    pub started: usize,
    /// One line per app that could not be brought back.
    pub failed: Vec<String>,
}

impl Report {
    pub fn text(&self, name: &str) -> String {
        let mut s = format!("{name}: {} started, {} moved.", self.started, self.moved);
        if !self.failed.is_empty() {
            s.push(' ');
            s.push_str(&self.failed.join(" "));
        }
        s
    }
}

#[derive(Default)]
pub struct Workspaces {
    settings: Settings,
    running: bool,
}

impl Workspaces {
    pub fn new() -> Self {
        Self::default()
    }

    fn check(&self) -> Result<()> {
        if !(self.running && self.settings.enabled) {
            bail!("Workspaces is turned off");
        }
        Ok(())
    }

    fn open(&self, ctx: &mut Ctx) -> Result<()> {
        self.check()?;
        ctx.emit(OPEN_EVENT, Value::Null);
        Ok(())
    }

    fn launch(&self, name: &str, ctx: &mut Ctx) -> Result<()> {
        self.check()?;
        let Some(ws) = find(&self.settings.workspaces, name) else {
            bail!("there is no workspace named {name:?}");
        };
        ctx.emit(LAUNCH_EVENT, json!({ "name": ws.name }));
        Ok(())
    }
}

impl Module for Workspaces {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Workspaces".into(),
            description: "Save the open windows as a workspace and bring them back with one click.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![
                Permission::GlobalHotkeys,
                Permission::WindowManagement,
                Permission::ProcessLaunch,
            ],
            commands: vec![
                CommandSpec {
                    id: "open".into(),
                    title: "Open the Workspaces editor".into(),
                    description: String::new(),
                },
                CommandSpec {
                    id: "launch".into(),
                    title: "Launch a workspace".into(),
                    description: "Starts the programs of the workspace named `name` and puts their windows back."
                        .into(),
                },
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.workspaces.clone();
        self.running = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.workspaces.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        match action {
            "open" => self.open(ctx),
            _ => bail!("unknown action {action:?}"),
        }
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        match id {
            "open" => self.open(ctx)?,
            "launch" => self.launch(args["name"].as_str().unwrap_or_default(), ctx)?,
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        json!({
            "running": self.running && self.settings.enabled,
            "workspaces": self.settings.workspaces.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(path: &str, monitor: &str, monitor_rect: Rect, rect: Rect) -> WorkspaceApp {
        WorkspaceApp {
            path: path.into(),
            monitor: monitor.into(),
            monitor_rect,
            rect,
            ..Default::default()
        }
    }

    const FHD: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn a_window_keeps_its_place_on_its_monitor() {
        let a = app("a.exe", "D1", FHD, Rect::new(100, 50, 900, 650));
        let monitors = [("D1".to_string(), FHD)];
        assert_eq!(target(&a, &monitors), Some(Rect::new(100, 50, 900, 650)));
    }

    #[test]
    fn a_window_follows_its_monitor_when_it_moves_or_grows() {
        let a = app(
            "a.exe",
            "D2",
            Rect::new(1920, 0, 3840, 1080),
            Rect::new(2020, 100, 2980, 640),
        );
        let monitors = [
            ("D1".to_string(), FHD),
            ("D2".to_string(), Rect::new(-3840, 0, 0, 2160)),
        ];
        assert_eq!(target(&a, &monitors), Some(Rect::new(-3640, 200, -1720, 1280)));
    }

    #[test]
    fn a_window_whose_monitor_is_gone_goes_to_the_primary_one_and_stays_inside() {
        let a = app(
            "a.exe",
            "D2",
            Rect::new(1920, 0, 4480, 1440),
            Rect::new(1920, 0, 4480, 1440),
        );
        let monitors = [("D1".to_string(), FHD)];
        assert_eq!(target(&a, &monitors), Some(FHD));
        let off = app("a.exe", "D1", FHD, Rect::new(1800, 1000, 2400, 1400));
        assert_eq!(target(&off, &monitors), Some(Rect::new(1320, 680, 1920, 1080)));
        assert_eq!(target(&off, &[]), None);
    }

    #[test]
    fn open_windows_of_the_same_program_are_moved_once_each() {
        let apps = [
            app("C:\\Edge\\msedge.exe", "", FHD, FHD),
            app("C:\\Edge\\msedge.exe", "", FHD, FHD),
            app("C:\\Code.exe", "", FHD, FHD),
        ];
        let open = ["c:\\edge\\MSEDGE.exe".to_string(), "C:\\notepad.exe".into()];
        assert_eq!(
            plan(&apps, &open, true),
            [
                Step::Move { app: 0, window: 0 },
                Step::Start { app: 1 },
                Step::Start { app: 2 }
            ]
        );
        assert!(plan(&apps, &open, false)
            .iter()
            .all(|s| matches!(s, Step::Start { .. })));
    }

    #[test]
    fn workspaces_are_summed_up_by_program() {
        let apps = [
            app("C:\\Edge\\msedge.exe", "", FHD, FHD),
            app("C:\\notepad.EXE", "", FHD, FHD),
            app("C:\\Edge\\msedge.exe", "", FHD, FHD),
        ];
        assert_eq!(summary(&apps), "msedge (2), notepad");
        assert!(!relaunchable("C:\\Windows\\System32\\ApplicationFrameHost.exe"));
        assert!(relaunchable("C:\\Windows\\notepad.exe"));
    }

    #[test]
    fn workspaces_are_found_by_name_in_any_case() {
        let ws = [Workspace {
            name: "Morning".into(),
            apps: vec![],
        }];
        assert!(find(&ws, " morning ").is_some());
        assert!(find(&ws, "Evening").is_none());
    }
}
