//! Window Tiling: keyboard shortcuts that put the window in front on a half, a third,
//! two thirds or a quarter of its monitor, maximize it or center it. Pressing the
//! same shortcut again steps to the next size, as on macOS window managers.

use anyhow::{bail, Result};
use powerkit_config::WindowTiling as Settings;
use powerkit_core::Rect;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, Permission, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "window_tiling";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "left" => Dir::Left,
            "right" => Dir::Right,
            "up" => Dir::Up,
            "down" => Dir::Down,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    LeftHalf,
    LeftThird,
    LeftTwoThirds,
    RightHalf,
    RightThird,
    RightTwoThirds,
    TopLeft,
    BottomLeft,
    TopRight,
    BottomRight,
    Center,
}

/// Places a window is matched against, to know which one it is in now.
pub const PLACES: [Place; 11] = [
    Place::LeftHalf,
    Place::LeftThird,
    Place::LeftTwoThirds,
    Place::RightHalf,
    Place::RightThird,
    Place::RightTwoThirds,
    Place::TopLeft,
    Place::BottomLeft,
    Place::TopRight,
    Place::BottomRight,
    Place::Center,
];

/// What a shortcut does to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    To(Place),
    Maximize,
    /// Back to its size before it was maximized.
    Restore,
}

/// The rectangle of `place` in the work area `work`, with `gap` pixels around it.
pub fn rect_of(place: Place, work: Rect, gap: i32) -> Rect {
    let (w, h) = (work.right - work.left, work.bottom - work.top);
    let x = |num: i32, den: i32| work.left + w * num / den;
    let y = |num: i32, den: i32| work.top + h * num / den;
    let r = match place {
        Place::LeftHalf => Rect::new(work.left, work.top, x(1, 2), work.bottom),
        Place::LeftThird => Rect::new(work.left, work.top, x(1, 3), work.bottom),
        Place::LeftTwoThirds => Rect::new(work.left, work.top, x(2, 3), work.bottom),
        Place::RightHalf => Rect::new(x(1, 2), work.top, work.right, work.bottom),
        Place::RightThird => Rect::new(x(2, 3), work.top, work.right, work.bottom),
        Place::RightTwoThirds => Rect::new(x(1, 3), work.top, work.right, work.bottom),
        Place::TopLeft => Rect::new(work.left, work.top, x(1, 2), y(1, 2)),
        Place::BottomLeft => Rect::new(work.left, y(1, 2), x(1, 2), work.bottom),
        Place::TopRight => Rect::new(x(1, 2), work.top, work.right, y(1, 2)),
        Place::BottomRight => Rect::new(x(1, 2), y(1, 2), work.right, work.bottom),
        Place::Center => Rect::new(x(1, 6), y(1, 6), x(5, 6), y(5, 6)),
    };
    // A gap on every side, halved where two tiled windows meet.
    let half = gap / 2;
    let edge = |at: i32, outer: i32, inward: i32| {
        if at == outer {
            at + inward * gap
        } else {
            at + inward * half
        }
    };
    Rect::new(
        edge(r.left, work.left, 1),
        edge(r.top, work.top, 1),
        edge(r.right, work.right, -1),
        edge(r.bottom, work.bottom, -1),
    )
}

/// The place `window` is in, if any, allowing for windows that round their size.
pub fn current(window: Rect, work: Rect, gap: i32) -> Option<Place> {
    const SLACK: i32 = 8;
    PLACES.into_iter().find(|&p| {
        let r = rect_of(p, work, gap);
        (r.left - window.left).abs() <= SLACK
            && (r.top - window.top).abs() <= SLACK
            && (r.right - window.right).abs() <= SLACK
            && (r.bottom - window.bottom).abs() <= SLACK
    })
}

/// Where a window in `now` (maximized or not) goes for `dir`.
pub fn next(dir: Dir, now: Option<Place>, maximized: bool) -> Move {
    use Place::*;
    let left = matches!(now, Some(LeftHalf | LeftThird | LeftTwoThirds));
    let right = matches!(now, Some(RightHalf | RightThird | RightTwoThirds));
    Move::To(match dir {
        Dir::Left => match now {
            Some(LeftHalf) => LeftThird,
            Some(LeftThird) => LeftTwoThirds,
            Some(TopRight) => TopLeft,
            Some(BottomRight) => BottomLeft,
            _ => LeftHalf,
        },
        Dir::Right => match now {
            Some(RightHalf) => RightThird,
            Some(RightThird) => RightTwoThirds,
            Some(TopLeft) => TopRight,
            Some(BottomLeft) => BottomRight,
            _ => RightHalf,
        },
        Dir::Up if maximized => return Move::Maximize,
        Dir::Up if left => TopLeft,
        Dir::Up if right => TopRight,
        Dir::Up => match now {
            Some(BottomLeft) => LeftHalf,
            Some(BottomRight) => RightHalf,
            _ => return Move::Maximize,
        },
        Dir::Down if maximized => return Move::Restore,
        Dir::Down if left => BottomLeft,
        Dir::Down if right => BottomRight,
        Dir::Down => match now {
            Some(TopLeft) => LeftHalf,
            Some(TopRight) => RightHalf,
            _ => Center,
        },
    })
}

#[derive(Default)]
pub struct WindowTiling {
    settings: Settings,
    running: bool,
    paused: bool,
}

impl WindowTiling {
    pub fn new() -> Self {
        Self::default()
    }

    fn tile(&mut self, dir: Dir) -> Result<()> {
        if !(self.running && !self.paused && self.settings.enabled) {
            bail!("Window Tiling is not running");
        }
        #[cfg(windows)]
        live::tile(dir, self.settings.gap)?;
        #[cfg(not(windows))]
        let _ = dir;
        Ok(())
    }
}

#[cfg(windows)]
mod live {
    use super::{current, next, rect_of, Dir, Move};
    use anyhow::{Context, Result};
    use powerkit_platform::{monitor, window};

    pub fn tile(dir: Dir, gap: u32) -> Result<()> {
        let Some(hwnd) = window::foreground() else {
            return Ok(());
        };
        let info = window::info(hwnd)?;
        let m = monitor::from_window(hwnd).context("the window is on no monitor")?;
        if let Err(why) = info.manageable(&m.rect) {
            tracing::debug!("not tiling {:?}: {why:?}", info.title);
            return Ok(());
        }
        let gap = m.scale(gap);
        let now = if info.maximized {
            None
        } else {
            current(window::visible_rect(hwnd)?, m.work_area, gap)
        };
        match next(dir, now, info.maximized) {
            Move::Maximize => window::maximize(hwnd),
            Move::Restore => window::restore(hwnd),
            Move::To(place) => {
                if info.maximized {
                    window::restore(hwnd);
                }
                window::set_visible_rect(hwnd, rect_of(place, m.work_area, gap))?;
            }
        }
        Ok(())
    }
}

impl Module for WindowTiling {
    fn manifest(&self) -> Manifest {
        let cmd = |id: &str, title: &str| CommandSpec {
            id: id.into(),
            title: title.into(),
            description: String::new(),
        };
        Manifest {
            id: ID.into(),
            name: "Window Tiling".into(),
            description: "Put the window in front on a half, third or quarter of the screen by keyboard.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![Permission::GlobalHotkeys, Permission::WindowManagement],
            commands: vec![
                cmd("left", "Tile the window to the left"),
                cmd("right", "Tile the window to the right"),
                cmd("up", "Tile the window up or maximize it"),
                cmd("down", "Tile the window down or center it"),
            ],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.window_tiling.clone();
        self.running = true;
        self.paused = false;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut Ctx) {
        self.running = false;
    }

    fn pause(&mut self, _ctx: &mut Ctx) {
        self.paused = true;
    }

    fn resume(&mut self, _ctx: &mut Ctx) {
        self.paused = false;
    }

    fn config_changed(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.settings = ctx.config.window_tiling.clone();
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        match Dir::parse(action) {
            Some(d) => self.tile(d),
            None => bail!("unknown action {action:?}"),
        }
    }

    fn command(&mut self, id: &str, _args: &Value, _ctx: &mut Ctx) -> Result<Value> {
        match Dir::parse(id) {
            Some(d) => self.tile(d)?,
            None => bail!("unknown command {id:?}"),
        }
        Ok(Value::Null)
    }

    fn status(&self) -> Value {
        json!({ "running": self.running && self.settings.enabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerkit_config::Config;

    const WORK: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn places_cover_their_share_of_the_work_area() {
        assert_eq!(rect_of(Place::LeftHalf, WORK, 0), Rect::new(0, 0, 960, 1040));
        assert_eq!(rect_of(Place::RightThird, WORK, 0), Rect::new(1280, 0, 1920, 1040));
        assert_eq!(rect_of(Place::BottomRight, WORK, 0), Rect::new(960, 520, 1920, 1040));
        assert_eq!(rect_of(Place::Center, WORK, 0), Rect::new(320, 173, 1600, 866));
    }

    #[test]
    fn gaps_are_full_at_the_edges_and_halved_between_windows() {
        assert_eq!(rect_of(Place::LeftHalf, WORK, 10), Rect::new(10, 10, 955, 1030));
        assert_eq!(rect_of(Place::RightHalf, WORK, 10), Rect::new(965, 10, 1910, 1030));
    }

    #[test]
    fn a_window_is_recognised_in_its_place() {
        let mut r = rect_of(Place::TopRight, WORK, 0);
        r.right -= 3;
        assert_eq!(current(r, WORK, 0), Some(Place::TopRight));
        assert_eq!(current(Rect::new(100, 100, 700, 600), WORK, 0), None);
    }

    #[test]
    fn pressing_again_steps_through_sizes_and_corners() {
        use Place::*;
        let to = Move::To;
        assert_eq!(next(Dir::Left, None, false), to(LeftHalf));
        assert_eq!(next(Dir::Left, Some(LeftHalf), false), to(LeftThird));
        assert_eq!(next(Dir::Left, Some(LeftThird), false), to(LeftTwoThirds));
        assert_eq!(next(Dir::Left, Some(LeftTwoThirds), false), to(LeftHalf));
        assert_eq!(next(Dir::Left, Some(TopRight), false), to(TopLeft));
        assert_eq!(next(Dir::Up, Some(RightHalf), false), to(TopRight));
        assert_eq!(next(Dir::Up, Some(BottomLeft), false), to(LeftHalf));
        assert_eq!(next(Dir::Up, None, false), Move::Maximize);
        assert_eq!(next(Dir::Down, None, true), Move::Restore);
        assert_eq!(next(Dir::Down, Some(LeftThird), false), to(BottomLeft));
        assert_eq!(next(Dir::Down, None, false), to(Center));
        assert_eq!(next(Dir::Right, None, true), to(RightHalf));
    }

    #[test]
    fn the_shortcuts_are_registered() {
        let c = Config::new();
        let ours: Vec<_> = c
            .shortcuts()
            .into_iter()
            .filter(|(m, _, _)| *m == ID)
            .map(|(_, a, _)| a)
            .collect();
        assert_eq!(ours, ["left", "right", "up", "down"]);
    }
}
