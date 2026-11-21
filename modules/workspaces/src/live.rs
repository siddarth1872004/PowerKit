//! Reading the open windows and putting a workspace's windows back. Both block, so
//! launching runs on a worker thread.

use crate::{app_name, plan, relaunchable, target, Report, Step};
use anyhow::{Context, Result};
use powerkit_config::{WindowState, WorkspaceApp};
use powerkit_core::Rect;
use powerkit_platform::monitor;
use powerkit_platform::window::{self, Hwnd, WindowInfo};
use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// How long a started program has to open its window.
const WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(250);
/// Some programs move their window to where they last had it right after showing it.
const SETTLE: Duration = Duration::from_millis(700);

struct Open {
    hwnd: Hwnd,
    info: WindowInfo,
    path: String,
}

/// Ordinary windows of programs that can be started again, minimized ones too, front
/// to back, leaving out those of process `skip_pid`. The titles of ordinary windows
/// whose program cannot be started again go to `left_out`.
fn open_windows(skip_pid: u32, mut left_out: Option<&mut Vec<String>>) -> Result<Vec<Open>> {
    let mut out = Vec::new();
    for hwnd in window::enumerate()? {
        let Ok(info) = window::info(hwnd) else { continue };
        if info.pid == skip_pid {
            continue;
        }
        let Some(m) = monitor::from_window(hwnd) else { continue };
        let probe = WindowInfo {
            minimized: false,
            ..info.clone()
        };
        if probe.manageable(&m.rect).is_err() {
            continue;
        }
        let Some(path) = window::process_path(info.pid).filter(|p| relaunchable(p)) else {
            if let Some(list) = left_out.as_deref_mut() {
                list.push(info.title);
            }
            continue;
        };
        out.push(Open { hwnd, info, path });
    }
    Ok(out)
}

/// The windows open now, back to front so that launching them in order leaves the
/// front-most on top, and the titles of windows that could not be kept. PowerKit's
/// own windows are left out.
pub fn capture() -> Result<(Vec<WorkspaceApp>, Vec<String>)> {
    let mut apps = Vec::new();
    let mut left_out = Vec::new();
    for w in open_windows(std::process::id(), Some(&mut left_out))?.into_iter().rev() {
        let Some(m) = monitor::from_window(w.hwnd) else {
            continue;
        };
        let state = if w.info.minimized {
            WindowState::Minimized
        } else if w.info.maximized {
            WindowState::Maximized
        } else {
            WindowState::Normal
        };
        let rect = match state {
            WindowState::Normal => w.info.rect,
            _ => match window::restored_rect(w.hwnd) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("workspaces: skipped {:?}: {e}", w.info.title);
                    left_out.push(w.info.title);
                    continue;
                }
            },
        };
        apps.push(WorkspaceApp {
            path: w.path,
            args: String::new(),
            title: w.info.title,
            monitor: m.id,
            monitor_rect: m.rect,
            rect,
            state,
        });
    }
    Ok((apps, left_out))
}

fn monitors() -> Result<Vec<(String, Rect)>> {
    let mut list = monitor::enumerate().context("listing the monitors")?;
    list.sort_by_key(|m| !m.primary);
    Ok(list.into_iter().map(|m| (m.id, m.rect)).collect())
}

fn place(hwnd: Hwnd, app: &WorkspaceApp, monitors: &[(String, Rect)]) -> Result<(), String> {
    let r = target(app, monitors).ok_or("there is no monitor")?;
    window::set_visible_rect(hwnd, r).map_err(|e| e.to_string())?;
    match app.state {
        WindowState::Normal => {}
        WindowState::Maximized => window::maximize(hwnd),
        WindowState::Minimized => window::minimize(hwnd),
    }
    Ok(())
}

fn start(app: &WorkspaceApp) -> std::io::Result<u32> {
    let mut cmd = Command::new(&app.path);
    if !app.args.trim().is_empty() {
        cmd.raw_arg(&app.args);
    }
    if let Some(dir) = Path::new(&app.path).parent() {
        cmd.current_dir(dir);
    }
    Ok(cmd.spawn()?.id())
}

/// Bring back `apps`: move open windows of their programs if `move_existing`, start
/// the rest and place each new window as it appears.
pub fn launch(apps: &[WorkspaceApp], move_existing: bool) -> Report {
    let mut report = Report::default();
    let (monitors, before) = match (monitors(), open_windows(std::process::id(), None)) {
        (Ok(m), Ok(o)) => (m, o),
        (Err(e), _) | (_, Err(e)) => {
            report.failed.push(format!("Nothing was done: {e:#}."));
            return report;
        }
    };
    let paths: Vec<String> = before.iter().map(|w| w.path.clone()).collect();
    let mut taken: HashSet<Hwnd> = before.iter().map(|w| w.hwnd).collect();
    let mut waiting: Vec<(usize, u32)> = Vec::new();
    for step in plan(apps, &paths, move_existing) {
        match step {
            Step::Move { app, window } => match place(before[window].hwnd, &apps[app], &monitors) {
                Ok(()) => report.moved += 1,
                Err(e) => report
                    .failed
                    .push(format!("{} could not be moved: {e}.", app_name(&apps[app].path))),
            },
            Step::Start { app } => match start(&apps[app]) {
                Ok(pid) => waiting.push((app, pid)),
                Err(e) => report
                    .failed
                    .push(format!("{} could not be started: {e}.", app_name(&apps[app].path))),
            },
        }
    }

    let deadline = Instant::now() + WAIT;
    let mut shown: Vec<(usize, Hwnd)> = Vec::new();
    while !waiting.is_empty() && Instant::now() < deadline {
        std::thread::sleep(POLL);
        let Ok(now) = open_windows(std::process::id(), None) else {
            continue;
        };
        waiting.retain(|&(app, pid)| {
            let a = &apps[app];
            // Programs that hand over to a copy already running open the window from
            // another process, so a new window of the same program counts too.
            let found = now
                .iter()
                .filter(|w| !taken.contains(&w.hwnd))
                .find(|w| w.info.pid == pid)
                .or_else(|| {
                    now.iter()
                        .find(|w| !taken.contains(&w.hwnd) && w.path.eq_ignore_ascii_case(&a.path))
                });
            match found {
                Some(w) => {
                    taken.insert(w.hwnd);
                    shown.push((app, w.hwnd));
                    false
                }
                None => true,
            }
        });
    }
    for (app, _) in &waiting {
        report
            .failed
            .push(format!("{} did not open a window in time.", app_name(&apps[*app].path)));
    }
    if !shown.is_empty() {
        std::thread::sleep(SETTLE);
    }
    for (app, hwnd) in shown {
        match place(hwnd, &apps[app], &monitors) {
            Ok(()) => report.started += 1,
            Err(e) => report
                .failed
                .push(format!("{} could not be placed: {e}.", app_name(&apps[app].path))),
        }
    }
    report
}
