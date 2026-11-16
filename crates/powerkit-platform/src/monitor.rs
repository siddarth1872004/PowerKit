//! Monitor discovery.

use crate::util::from_wide;
use crate::window::Hwnd;
use crate::{Context, Result};
use powerkit_core::Rect;
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

/// One display. All rectangles are physical pixels in virtual-desktop coordinates, so
/// monitors left of or above the primary have negative coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// Stable identifier used to key per-monitor settings (GDI device name such as
    /// `\\.\DISPLAY2`). It survives resolution/DPI changes and reconnection to the same
    /// output.
    pub id: String,
    pub rect: Rect,
    /// `rect` minus taskbar and docked app bars.
    pub work_area: Rect,
    /// Effective DPI (96 = 100% scaling).
    pub dpi: u32,
    pub primary: bool,
    handle: isize,
}

impl Monitor {
    /// Scale a length given in 96-DPI pixels to this monitor.
    pub fn scale(&self, px96: u32) -> i32 {
        ((px96 as u64 * self.dpi as u64 + 48) / 96) as i32
    }

    /// Build a monitor description by hand (tests and simulations).
    pub fn synthetic(id: &str, rect: Rect, work_area: Rect, dpi: u32, primary: bool) -> Monitor {
        Monitor {
            id: id.into(),
            rect,
            work_area,
            dpi,
            primary,
            handle: 0,
        }
    }

    pub(crate) fn raw(&self) -> HMONITOR {
        HMONITOR(self.handle as *mut _)
    }
}

fn describe(handle: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `info` is a MONITORINFOEXW whose cbSize says so; the API accepts a pointer
    // to its MONITORINFO prefix.
    let ok = unsafe { GetMonitorInfoW(handle, (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>()) };
    if !ok.as_bool() {
        return None;
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: valid out pointers.
    if unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_err() {
        dx = 96;
    }
    let r = |r: RECT| Rect::new(r.left, r.top, r.right, r.bottom);
    Some(Monitor {
        id: from_wide(&info.szDevice),
        rect: r(info.monitorInfo.rcMonitor),
        work_area: r(info.monitorInfo.rcWork),
        dpi: dx.max(48),
        primary: info.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
        handle: handle.0 as isize,
    })
}

/// All active monitors, primary first, then left-to-right, top-to-bottom.
pub fn enumerate() -> Result<Vec<Monitor>> {
    unsafe extern "system" fn callback(h: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the Vec passed by `enumerate`, alive for the whole call.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HMONITOR>) };
        list.push(h);
        true.into()
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only runs during this call and `handles` outlives it.
    let ok = unsafe { EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut handles as *mut _ as isize)) };
    ok.ok().context("EnumDisplayMonitors")?;
    let mut monitors: Vec<Monitor> = handles.into_iter().filter_map(describe).collect();
    monitors.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    Ok(monitors)
}

/// Monitor containing (or nearest to) a point.
pub fn from_point(x: i32, y: i32) -> Option<Monitor> {
    // SAFETY: plain value arguments.
    describe(unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) })
}

/// Monitor a window is mostly on.
pub fn from_window(hwnd: Hwnd) -> Option<Monitor> {
    // SAFETY: accepts any handle; falls back to the nearest monitor.
    describe(unsafe { MonitorFromWindow(hwnd.raw(), MONITOR_DEFAULTTONEAREST) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_at_least_one_monitor() {
        let monitors = enumerate().unwrap();
        assert!(!monitors.is_empty());
        assert_eq!(monitors.iter().filter(|m| m.primary).count(), 1);
        assert!(monitors[0].primary);
        for m in &monitors {
            assert!(!m.rect.is_empty() && !m.work_area.is_empty());
            assert!(m.rect.intersect(&m.work_area) == Some(m.work_area));
            assert!(!m.id.is_empty());
            assert!(m.dpi >= 96);
        }
        let p = &monitors[0];
        let (cx, cy) = p.rect.center();
        assert_eq!(from_point(cx, cy).unwrap().id, p.id);
    }

    #[test]
    fn dpi_scaling_rounds() {
        let m = Monitor::synthetic("x", Rect::new(0, 0, 10, 10), Rect::new(0, 0, 10, 10), 144, true);
        assert_eq!(m.scale(8), 12);
        assert_eq!(m.scale(1), 2);
        assert_eq!(m.scale(0), 0);
    }
}
