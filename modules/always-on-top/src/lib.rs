//! Always On Top: toggle the real `WS_EX_TOPMOST` flag of application windows, show a
//! frame around pinned windows, and put everything back when unpinned or stopped.

use anyhow::{anyhow, bail, Result};
use powerkit_config::AlwaysOnTop as Settings;
use powerkit_core::{Event, Rect, WindowEventKind};
use powerkit_platform::event_loop::{hook_thread_window_events, WinEventHook};
use powerkit_platform::frame::FrameWindow;
use powerkit_platform::monitor;
use powerkit_platform::window::{self, Hwnd, OpacityState, Unmanageable};
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const ID: &str = "always_on_top";

struct Pinned {
    hwnd: Hwnd,
    /// Process that owned the handle when it was pinned; used to detect handle reuse.
    pid: u32,
    title: String,
    exe: String,
    was_topmost: bool,
    opacity: Option<OpacityState>,
    frame: Option<FrameWindow>,
    _hook: Option<WinEventHook>,
}

#[derive(Default)]
pub struct AlwaysOnTop {
    pinned: Vec<Pinned>,
    /// Windows already auto-pinned once, so unpinning them manually sticks.
    auto_handled: HashSet<isize>,
    settings: Settings,
}

/// Where the indicator frame goes: just outside the window, but never off its monitor
/// (a maximized or screen-edge window gets the frame inside its bounds instead).
pub fn frame_rect(window: Rect, monitor: Rect, thickness: i32) -> Rect {
    let grown = window.inset(-thickness);
    Rect {
        left: grown.left.max(monitor.left.min(window.left)),
        top: grown.top.max(monitor.top.min(window.top)),
        right: grown.right.min(monitor.right.max(window.right)),
        bottom: grown.bottom.min(monitor.bottom.max(window.bottom)),
    }
}

fn friendly(e: powerkit_platform::Error) -> anyhow::Error {
    if e.is_access_denied() {
        anyhow!(
            "That window belongs to an application running as administrator. \
             PowerKit can only manage it when PowerKit itself runs as administrator."
        )
    } else {
        anyhow!(e)
    }
}

impl AlwaysOnTop {
    pub fn new() -> Self {
        Self::default()
    }

    fn index_of(&self, hwnd: Hwnd) -> Option<usize> {
        self.pinned.iter().position(|p| p.hwnd == hwnd)
    }

    pub fn is_pinned(&self, hwnd: Hwnd) -> bool {
        self.index_of(hwnd).is_some()
    }

    pub fn pinned_count(&self) -> usize {
        self.pinned.len()
    }

    /// Drop entries whose window is gone or whose handle now belongs to another
    /// process. Such windows are not touched: they are not "ours" any more.
    fn prune(&mut self) -> bool {
        let before = self.pinned.len();
        self.pinned
            .retain(|p| window::is_window(p.hwnd) && window::thread_process(p.hwnd).map(|(pid, _)| pid) == Some(p.pid));
        self.auto_handled.retain(|h| window::is_window(Hwnd(*h)));
        before != self.pinned.len()
    }

    fn pin(&mut self, hwnd: Hwnd) -> Result<()> {
        if self.is_pinned(hwnd) {
            return Ok(());
        }
        let info = window::info(hwnd).map_err(friendly)?;
        if info.pid == std::process::id() && !cfg!(test) {
            bail!("PowerKit's own windows cannot be pinned");
        }
        let mon = monitor::from_window(hwnd).map(|m| m.rect).unwrap_or_default();
        match info.manageable(&mon) {
            Err(
                reason @ (Unmanageable::NotVisible
                | Unmanageable::Cloaked
                | Unmanageable::ChildWindow
                | Unmanageable::ShellWindow),
            ) => bail!("This window cannot be pinned ({reason:?})"),
            _ => {}
        }
        if self.settings.excluded_apps.contains(&info.exe) {
            bail!("{} is on the Always On Top exclusion list", info.exe);
        }
        let was_topmost = info.is_topmost();
        window::set_topmost(hwnd, true).map_err(friendly)?;
        let mut entry = Pinned {
            hwnd,
            pid: info.pid,
            title: info.title,
            exe: info.exe,
            was_topmost,
            opacity: None,
            frame: None,
            _hook: None,
        };
        self.decorate(&mut entry);
        self.pinned.push(entry);
        Ok(())
    }

    /// Apply the optional extras (opacity, frame). Failures here are not fatal: the
    /// window is pinned either way.
    fn decorate(&self, p: &mut Pinned) {
        let want_alpha =
            (self.settings.opacity_percent < 100).then(|| (self.settings.opacity_percent * 255 / 100) as u8);
        match (want_alpha, p.opacity) {
            (Some(alpha), existing) => match window::set_opacity(p.hwnd, alpha) {
                // Keep the *original* state if opacity was already applied once.
                Ok(state) => p.opacity = existing.or(Some(state)),
                Err(e) => tracing::warn!(error = %e, "could not change window opacity"),
            },
            (None, Some(state)) => {
                if let Err(e) = window::restore_opacity(p.hwnd, state) {
                    tracing::warn!(error = %e, "could not restore window opacity");
                }
                p.opacity = None;
            }
            (None, None) => {}
        }
        if self.settings.show_border {
            if p.frame.is_none() {
                match FrameWindow::new(&self.settings.border_color) {
                    Ok(f) => p.frame = Some(f),
                    Err(e) => tracing::warn!(error = %e, "could not create pin indicator"),
                }
            }
            if let Some(f) = &p.frame {
                f.set_color(&self.settings.border_color);
            }
            if p._hook.is_none() {
                match hook_thread_window_events(p.hwnd) {
                    Ok(h) => p._hook = Some(h),
                    Err(e) => tracing::warn!(error = %e, "pin indicator will not follow the window"),
                }
            }
        } else {
            p.frame = None;
            p._hook = None;
        }
        Self::place_frame(p, self.settings.border_thickness);
    }

    fn place_frame(p: &mut Pinned, thickness96: u32) {
        let Some(frame) = p.frame.as_mut() else { return };
        let shown = window::info(p.hwnd)
            .ok()
            .filter(|i| i.visible && !i.minimized && !i.cloaked)
            .and_then(|i| monitor::from_window(p.hwnd).map(|m| (i, m)));
        match shown {
            Some((info, mon)) => {
                let t = mon.scale(thickness96).max(1);
                if let Err(e) = frame.show(frame_rect(info.rect, mon.rect, t), t) {
                    tracing::debug!(error = %e, "pin indicator update failed");
                }
            }
            None => frame.hide(),
        }
    }

    fn unpin_at(&mut self, index: usize) {
        let mut p = self.pinned.remove(index);
        // Undo in the reverse order of `pin`: the frame (itself topmost) and its hook go
        // first, or clearing the window's topmost state sometimes does not stick.
        p.frame = None;
        p._hook = None;
        let ours = window::thread_process(p.hwnd).map(|(pid, _)| pid) == Some(p.pid);
        if !ours {
            return;
        }
        if let Some(state) = p.opacity {
            if let Err(e) = window::restore_opacity(p.hwnd, state) {
                tracing::warn!(error = %e, "could not restore window opacity");
            }
        }
        if !p.was_topmost {
            if let Err(e) = window::set_topmost(p.hwnd, false) {
                tracing::warn!(error = %e, "could not clear topmost state");
            }
        }
    }

    fn unpin_all(&mut self) {
        while !self.pinned.is_empty() {
            self.unpin_at(self.pinned.len() - 1);
        }
    }

    /// Returns the new pinned state.
    fn toggle(&mut self, hwnd: Hwnd) -> Result<bool> {
        self.prune();
        match self.index_of(hwnd) {
            Some(i) => {
                self.unpin_at(i);
                Ok(false)
            }
            None => self.pin(hwnd).map(|_| true),
        }
    }

    fn target(args: &Value) -> Result<Hwnd> {
        match args.get("hwnd").and_then(Value::as_i64) {
            Some(h) => Ok(Hwnd(h as isize)),
            None => window::foreground().ok_or_else(|| anyhow!("There is no active window")),
        }
    }

    fn changed(&self, ctx: &mut Ctx) {
        ctx.emit("always_on_top.changed", self.status());
    }
}

impl Module for AlwaysOnTop {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str, description: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: description.into(),
        };
        Manifest {
            id: ID.into(),
            name: "Always On Top".into(),
            description: "Pin a window so it stays above all others.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys, Permission::WindowManagement],
            commands: vec![
                cmd("toggle_pin", "Pin or unpin the active window", ""),
                cmd("pin_app", "Pin all windows of the active application", ""),
                cmd("unpin_all", "Unpin all windows", ""),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.always_on_top.clone();
        Ok(())
    }

    fn stop(&mut self, ctx: &mut Ctx) {
        self.prune();
        self.unpin_all();
        self.auto_handled.clear();
        self.changed(ctx);
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.always_on_top.clone();
        self.prune();
        let mut pinned = std::mem::take(&mut self.pinned);
        for p in &mut pinned {
            self.decorate(p);
        }
        self.pinned = pinned;
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, ctx: &mut Ctx) -> Result<()> {
        if action == "toggle_pin" {
            self.command("toggle_pin", &Value::Null, ctx)?;
        }
        Ok(())
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) -> Result<()> {
        let Event::Window { kind, hwnd } = *event else {
            return Ok(());
        };
        let hwnd = Hwnd(hwnd);
        match kind {
            WindowEventKind::Destroyed => {
                if self.is_pinned(hwnd) && self.prune() {
                    self.changed(ctx);
                }
            }
            WindowEventKind::Foreground => {
                if !self.settings.auto_pin_apps.is_empty()
                    && !self.is_pinned(hwnd)
                    && !self.auto_handled.contains(&hwnd.0)
                {
                    let exe = window::thread_process(hwnd)
                        .and_then(|(pid, _)| window::process_exe(pid))
                        .unwrap_or_default();
                    if self.settings.auto_pin_apps.contains(&exe) {
                        self.auto_handled.insert(hwnd.0);
                        match self.pin(hwnd) {
                            Ok(()) => self.changed(ctx),
                            Err(e) => tracing::info!(error = %e, "auto-pin skipped"),
                        }
                    }
                }
            }
            WindowEventKind::LocationChange
            | WindowEventKind::Shown
            | WindowEventKind::Hidden
            | WindowEventKind::Minimized
            | WindowEventKind::Restored
            | WindowEventKind::MoveSizeEnd => {
                let thickness = self.settings.border_thickness;
                if let Some(i) = self.index_of(hwnd) {
                    Self::place_frame(&mut self.pinned[i], thickness);
                }
            }
            WindowEventKind::MoveSizeStart => {}
        }
        Ok(())
    }

    fn command(&mut self, id: &str, args: &Value, ctx: &mut Ctx) -> Result<Value> {
        let result = match id {
            "toggle_pin" => json!({ "pinned": self.toggle(Self::target(args)?)? }),
            "pin" => {
                self.prune();
                self.pin(Self::target(args)?)?;
                json!({ "pinned": true })
            }
            "unpin" => {
                let hwnd = Self::target(args)?;
                self.prune();
                if let Some(i) = self.index_of(hwnd) {
                    self.unpin_at(i);
                }
                json!({ "pinned": false })
            }
            "unpin_all" => {
                self.prune();
                self.unpin_all();
                Value::Null
            }
            "pin_app" => {
                let hwnd = Self::target(args)?;
                let (pid, _) = window::thread_process(hwnd).ok_or_else(|| anyhow!("The window no longer exists"))?;
                self.prune();
                let mut count = 0;
                for h in window::enumerate()? {
                    if window::thread_process(h).map(|(p, _)| p) != Some(pid) {
                        continue;
                    }
                    let mon = monitor::from_window(h).map(|m| m.rect).unwrap_or_default();
                    let ordinary = window::info(h).map(|i| i.manageable(&mon).is_ok()).unwrap_or(false);
                    if ordinary && self.pin(h).is_ok() {
                        count += 1;
                    }
                }
                json!({ "pinned": count })
            }
            "list" => return Ok(self.status()),
            other => bail!("unknown command {other:?}"),
        };
        self.changed(ctx);
        Ok(result)
    }

    fn status(&self) -> Value {
        json!({
            "pinned": self.pinned.iter().map(|p| json!({
                "hwnd": p.hwnd.0,
                "title": p.title,
                "exe": p.exe,
            })).collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::{Config, Paths};
    use powerkit_platform::event_loop::EventLoop;
    use powerkit_platform::testing::{pump, ShowState, TestWindow};
    use powerkit_plugin_api::Action;
    use proptest::prelude::*;

    struct Fixture {
        config: Config,
        paths: Paths,
        actions: Vec<Action>,
        _dir: tempfile::TempDir,
        event_loop: EventLoop,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            Self {
                config: Config::new(),
                paths: Paths::new(dir.path()),
                actions: Vec::new(),
                _dir: dir,
                event_loop: EventLoop::new().unwrap(),
            }
        }

        fn ctx(&mut self) -> Ctx<'_> {
            Ctx {
                config: &self.config,
                paths: &self.paths,
                actions: &mut self.actions,
            }
        }

        /// Deliver queued platform events to the module, like the host does.
        fn deliver(&mut self, m: &mut AlwaysOnTop) {
            let mut events = Vec::new();
            self.event_loop.poll(|e| {
                events.push(e);
                true
            });
            for e in events {
                m.on_event(&e, &mut self.ctx()).unwrap();
            }
        }
    }

    fn rect() -> Rect {
        Rect::from_size(120, 120, 400, 300)
    }

    #[test]
    fn pin_and_unpin_restore_topmost_state() {
        let mut fx = Fixture::new();
        let mut m = AlwaysOnTop::new();
        m.start(&mut fx.ctx()).unwrap();
        let w = TestWindow::new("pk-aot-1", rect()).unwrap();
        let args = json!({ "hwnd": w.hwnd().0 });

        assert!(!window::is_topmost(w.hwnd()));
        assert_eq!(
            m.command("toggle_pin", &args, &mut fx.ctx()).unwrap(),
            json!({"pinned": true})
        );
        assert!(window::is_topmost(w.hwnd()));
        assert_eq!(m.status()["pinned"][0]["title"], "pk-aot-1");
        assert_eq!(
            m.command("toggle_pin", &args, &mut fx.ctx()).unwrap(),
            json!({"pinned": false})
        );
        assert!(!window::is_topmost(w.hwnd()));
        assert_eq!(m.pinned_count(), 0);
        assert!(fx
            .actions
            .iter()
            .any(|a| matches!(a, Action::Emit { event, .. } if event == "always_on_top.changed")));
    }

    #[test]
    fn window_that_was_already_topmost_stays_topmost() {
        let mut fx = Fixture::new();
        let mut m = AlwaysOnTop::new();
        m.start(&mut fx.ctx()).unwrap();
        let w = TestWindow::new("pk-aot-2", rect()).unwrap();
        window::set_topmost(w.hwnd(), true).unwrap();
        let args = json!({ "hwnd": w.hwnd().0 });
        m.command("pin", &args, &mut fx.ctx()).unwrap();
        m.command("unpin", &args, &mut fx.ctx()).unwrap();
        assert!(window::is_topmost(w.hwnd()), "original topmost state must be preserved");
    }

    #[test]
    fn stop_unpins_everything_and_restores_opacity() {
        let mut fx = Fixture::new();
        fx.config.always_on_top.opacity_percent = 60;
        let mut m = AlwaysOnTop::new();
        m.start(&mut fx.ctx()).unwrap();
        let a = TestWindow::new("pk-aot-3a", rect()).unwrap();
        let b = TestWindow::new("pk-aot-3b", rect().offset(40, 40)).unwrap();
        for w in [&a, &b] {
            m.command("pin", &json!({ "hwnd": w.hwnd().0 }), &mut fx.ctx()).unwrap();
            let info = window::info(w.hwnd()).unwrap();
            assert!(info.is_topmost() && info.has_ex_style(window::WS_EX_LAYERED_BITS));
        }
        assert_eq!(m.pinned_count(), 2);
        m.stop(&mut fx.ctx());
        for w in [&a, &b] {
            let info = window::info(w.hwnd()).unwrap();
            assert!(!info.is_topmost());
            assert!(
                !info.has_ex_style(window::WS_EX_LAYERED_BITS),
                "layered style must be removed again"
            );
        }
    }

    #[test]
    fn destroyed_window_is_forgotten() {
        let mut fx = Fixture::new();
        let mut m = AlwaysOnTop::new();
        m.start(&mut fx.ctx()).unwrap();
        let mut w = TestWindow::new("pk-aot-4", rect()).unwrap();
        let hwnd = w.hwnd();
        m.command("pin", &json!({ "hwnd": hwnd.0 }), &mut fx.ctx()).unwrap();
        w.destroy();
        fx.deliver(&mut m);
        assert_eq!(m.pinned_count(), 0, "destroy event must remove the entry");
        // A stale handle is rejected cleanly.
        assert!(m.command("pin", &json!({ "hwnd": hwnd.0 }), &mut fx.ctx()).is_err());
    }

    #[test]
    fn frame_follows_minimize_and_restore() {
        let mut fx = Fixture::new();
        let mut m = AlwaysOnTop::new();
        m.start(&mut fx.ctx()).unwrap();
        let w = TestWindow::new("pk-aot-5", rect()).unwrap();
        m.command("pin", &json!({ "hwnd": w.hwnd().0 }), &mut fx.ctx()).unwrap();
        let visible = |m: &AlwaysOnTop| m.pinned[0].frame.as_ref().unwrap().is_visible();
        assert!(visible(&m));
        w.show_state(ShowState::Minimized);
        fx.deliver(&mut m);
        assert!(!visible(&m), "frame must hide with a minimized window");
        w.show_state(ShowState::Normal);
        fx.deliver(&mut m);
        assert!(visible(&m));
        assert!(window::is_topmost(w.hwnd()), "pin survives minimize/restore");
        m.stop(&mut fx.ctx());
    }

    #[test]
    fn excluded_apps_and_unsuitable_windows_are_refused() {
        let mut fx = Fixture::new();
        let me = window::process_exe(std::process::id()).unwrap();
        fx.config.always_on_top.excluded_apps = vec![me];
        let mut m = AlwaysOnTop::new();
        m.start(&mut fx.ctx()).unwrap();
        let w = TestWindow::new("pk-aot-6", rect()).unwrap();
        let err = m
            .command("pin", &json!({ "hwnd": w.hwnd().0 }), &mut fx.ctx())
            .unwrap_err();
        assert!(err.to_string().contains("exclusion list"), "{err}");
        assert!(!window::is_topmost(w.hwnd()));

        fx.config.always_on_top.excluded_apps.clear();
        m.config_changed(&mut fx.ctx()).unwrap();
        w.show_state(ShowState::Hidden);
        assert!(m.command("pin", &json!({ "hwnd": w.hwnd().0 }), &mut fx.ctx()).is_err());
        pump();
    }

    #[test]
    fn settings_changes_apply_to_pinned_windows() {
        let mut fx = Fixture::new();
        let mut m = AlwaysOnTop::new();
        m.start(&mut fx.ctx()).unwrap();
        let w = TestWindow::new("pk-aot-7", rect()).unwrap();
        m.command("pin", &json!({ "hwnd": w.hwnd().0 }), &mut fx.ctx()).unwrap();
        assert!(m.pinned[0].frame.is_some());
        fx.config.always_on_top.show_border = false;
        fx.config.always_on_top.opacity_percent = 50;
        m.config_changed(&mut fx.ctx()).unwrap();
        assert!(m.pinned[0].frame.is_none());
        assert!(window::info(w.hwnd()).unwrap().has_ex_style(window::WS_EX_LAYERED_BITS));
        fx.config.always_on_top.opacity_percent = 100;
        m.config_changed(&mut fx.ctx()).unwrap();
        assert!(!window::info(w.hwnd()).unwrap().has_ex_style(window::WS_EX_LAYERED_BITS));
        m.stop(&mut fx.ctx());
    }

    proptest! {
        #[test]
        fn frame_never_leaves_the_monitor_more_than_the_window_does(
            l in -3000i32..3000, t in -3000i32..3000, w in 50i32..3000, h in 50i32..3000, th in 1i32..20
        ) {
            let mon = Rect::new(-1920, 0, 0, 1080);
            let win = Rect::from_size(l, t, w, h);
            let f = frame_rect(win, mon, th);
            // The frame contains the window...
            prop_assert!(f.left <= win.left && f.top <= win.top && f.right >= win.right && f.bottom >= win.bottom);
            // ...extends at most `th` beyond it...
            prop_assert!(win.left - f.left <= th && f.right - win.right <= th);
            // ...and never crosses a monitor edge the window itself is inside of.
            if win.left >= mon.left { prop_assert!(f.left >= mon.left); }
            if win.right <= mon.right { prop_assert!(f.right <= mon.right); }
            if win.top >= mon.top { prop_assert!(f.top >= mon.top); }
            if win.bottom <= mon.bottom { prop_assert!(f.bottom <= mon.bottom); }
        }
    }
}
