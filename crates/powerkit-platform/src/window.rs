//! Top-level window inspection and manipulation.

use crate::util::from_wide;
use crate::{Context, Error, Result};
use powerkit_core::Rect;
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// A window handle value. Handles are plain identifiers: they can be stale (the window
/// was destroyed) or reused, so every operation re-validates and reports failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hwnd(pub isize);

impl Hwnd {
    pub(crate) fn raw(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }

    pub(crate) fn from_raw(h: HWND) -> Self {
        Hwnd(h.0 as isize)
    }

    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

pub const WS_CAPTION_BITS: u32 = WS_CAPTION.0;
pub const WS_THICKFRAME_BITS: u32 = WS_THICKFRAME.0;
pub const WS_POPUP_BITS: u32 = WS_POPUP.0;
pub const WS_CHILD_BITS: u32 = WS_CHILD.0;
pub const WS_EX_TOOLWINDOW_BITS: u32 = WS_EX_TOOLWINDOW.0;
pub const WS_EX_APPWINDOW_BITS: u32 = WS_EX_APPWINDOW.0;
pub const WS_EX_TOPMOST_BITS: u32 = WS_EX_TOPMOST.0;
pub const WS_EX_NOACTIVATE_BITS: u32 = WS_EX_NOACTIVATE.0;
pub const WS_EX_LAYERED_BITS: u32 = WS_EX_LAYERED.0;

/// Snapshot of a top-level window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: Hwnd,
    pub title: String,
    pub class: String,
    pub pid: u32,
    pub tid: u32,
    /// Lowercase executable file name (`notepad.exe`); empty when it cannot be queried.
    pub exe: String,
    /// Visible bounds (excludes the invisible resize borders Windows 10+ adds).
    pub rect: Rect,
    pub style: u32,
    pub ex_style: u32,
    pub visible: bool,
    pub minimized: bool,
    pub maximized: bool,
    /// Hidden by the shell (other virtual desktop, suspended UWP host...).
    pub cloaked: bool,
    pub has_owner: bool,
}

/// Why PowerKit will not move, resize or pin a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmanageable {
    NotVisible,
    Minimized,
    Cloaked,
    ChildWindow,
    ToolWindow,
    /// Owned dialogs and popups follow their owner.
    OwnedPopup,
    /// Desktop, taskbar and other shell surfaces.
    ShellWindow,
    /// Borderless window covering its whole monitor (games, video, presentations).
    FullScreen,
    /// No caption and no resize border: splash screens, menus, tooltips.
    NoFrame,
}

const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "MultitaskingViewFrame",
    "ForegroundStaging",
    "ApplicationManager_DesktopShellWindow",
    "Shell_InputSwitchTopLevelWindow",
    "Windows.UI.Input.InputSite.WindowClass",
];

impl WindowInfo {
    pub fn has_style(&self, bits: u32) -> bool {
        self.style & bits == bits
    }

    pub fn has_ex_style(&self, bits: u32) -> bool {
        self.ex_style & bits == bits
    }

    pub fn is_topmost(&self) -> bool {
        self.has_ex_style(WS_EX_TOPMOST_BITS)
    }

    /// Decide whether this is an ordinary application window that is safe to manage.
    /// `monitor` is the full rectangle of the monitor the window is on. This is a pure
    /// function of the snapshot so the policy can be unit tested.
    pub fn manageable(&self, monitor: &Rect) -> std::result::Result<(), Unmanageable> {
        if !self.visible {
            return Err(Unmanageable::NotVisible);
        }
        if self.minimized {
            return Err(Unmanageable::Minimized);
        }
        if self.cloaked {
            return Err(Unmanageable::Cloaked);
        }
        if self.has_style(WS_CHILD_BITS) {
            return Err(Unmanageable::ChildWindow);
        }
        if SHELL_CLASSES.contains(&self.class.as_str()) {
            return Err(Unmanageable::ShellWindow);
        }
        let app_window = self.has_ex_style(WS_EX_APPWINDOW_BITS);
        if self.has_ex_style(WS_EX_TOOLWINDOW_BITS) && !app_window {
            return Err(Unmanageable::ToolWindow);
        }
        if self.has_owner && !app_window {
            return Err(Unmanageable::OwnedPopup);
        }
        let framed = self.has_style(WS_CAPTION_BITS) || self.has_style(WS_THICKFRAME_BITS);
        if !framed {
            let covers = self.rect.left <= monitor.left
                && self.rect.top <= monitor.top
                && self.rect.right >= monitor.right
                && self.rect.bottom >= monitor.bottom;
            return Err(if covers {
                Unmanageable::FullScreen
            } else {
                Unmanageable::NoFrame
            });
        }
        Ok(())
    }

    pub fn is_resizable(&self) -> bool {
        self.has_style(WS_THICKFRAME_BITS)
    }
}

fn to_rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

/// Whether the handle currently refers to an existing window.
pub fn is_window(hwnd: Hwnd) -> bool {
    // SAFETY: IsWindow accepts any handle value, including stale ones.
    !hwnd.is_null() && unsafe { IsWindow(Some(hwnd.raw())) }.as_bool()
}

/// The window the user is working in, if any.
pub fn foreground() -> Option<Hwnd> {
    // SAFETY: no arguments.
    let h = unsafe { GetForegroundWindow() };
    (!h.is_invalid()).then(|| Hwnd::from_raw(h))
}

/// `(process id, thread id)` owning the window.
pub fn thread_process(hwnd: Hwnd) -> Option<(u32, u32)> {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer for the duration of the call.
    let tid = unsafe { GetWindowThreadProcessId(hwnd.raw(), Some(&mut pid)) };
    (tid != 0).then_some((pid, tid))
}

/// Lowercase executable file name of a process, or `None` if it cannot be opened.
pub fn process_exe(pid: u32) -> Option<String> {
    process_path(pid).and_then(|p| p.rsplit(['\\', '/']).next().map(str::to_lowercase))
}

/// Full image path of a process.
pub fn process_path(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed below; the buffer outlives the call and its
    // capacity is passed in `len`.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        res.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Outer window rectangle including invisible resize borders.
fn raw_rect(hwnd: Hwnd) -> Result<Rect> {
    let mut r = RECT::default();
    // SAFETY: `r` is a valid out pointer.
    unsafe { GetWindowRect(hwnd.raw(), &mut r) }.context("GetWindowRect")?;
    Ok(to_rect(r))
}

/// Visible bounds of a window (DWM extended frame bounds, falling back to the window
/// rectangle when composition data is unavailable).
pub fn visible_rect(hwnd: Hwnd) -> Result<Rect> {
    let mut r = RECT::default();
    // SAFETY: the out pointer and size describe `r` exactly.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd.raw(),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut r as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    };
    match ok {
        Ok(()) if r.right > r.left && r.bottom > r.top => Ok(to_rect(r)),
        _ => raw_rect(hwnd),
    }
}

fn is_cloaked(hwnd: Hwnd) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the out pointer and size describe `cloaked` exactly.
    let ok = unsafe { DwmGetWindowAttribute(hwnd.raw(), DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4) };
    ok.is_ok() && cloaked != 0
}

fn text_of(hwnd: Hwnd) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: the slice bounds the write.
    let n = unsafe { GetWindowTextW(hwnd.raw(), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn class_of(hwnd: Hwnd) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the slice bounds the write.
    let n = unsafe { GetClassNameW(hwnd.raw(), &mut buf) };
    from_wide(&buf[..n.max(0) as usize])
}

/// Take a snapshot of a window. Fails if the window no longer exists.
pub fn info(hwnd: Hwnd) -> Result<WindowInfo> {
    if !is_window(hwnd) {
        return Err(Error::Other("the window no longer exists".into()));
    }
    let (pid, tid) = thread_process(hwnd).unwrap_or((0, 0));
    // SAFETY: plain value-returning queries on a handle that was just validated; a
    // window destroyed concurrently makes them return zero, which is handled.
    let (style, ex_style, visible, minimized, maximized, owner) = unsafe {
        (
            GetWindowLongPtrW(hwnd.raw(), GWL_STYLE) as u32,
            GetWindowLongPtrW(hwnd.raw(), GWL_EXSTYLE) as u32,
            IsWindowVisible(hwnd.raw()).as_bool(),
            IsIconic(hwnd.raw()).as_bool(),
            IsZoomed(hwnd.raw()).as_bool(),
            GetWindow(hwnd.raw(), GW_OWNER).ok().filter(|h| !h.is_invalid()),
        )
    };
    Ok(WindowInfo {
        hwnd,
        title: text_of(hwnd),
        class: class_of(hwnd),
        pid,
        tid,
        exe: process_exe(pid).unwrap_or_default(),
        rect: visible_rect(hwnd)?,
        style,
        ex_style,
        visible,
        minimized,
        maximized,
        cloaked: is_cloaked(hwnd),
        has_owner: owner.is_some(),
    })
}

/// Handles of all top-level windows, front to back.
pub fn enumerate() -> Result<Vec<Hwnd>> {
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the pointer to the Vec passed by `enumerate`, which
        // outlives the synchronous EnumWindows call.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<Hwnd>) };
        list.push(Hwnd::from_raw(hwnd));
        true.into()
    }
    let mut list: Vec<Hwnd> = Vec::with_capacity(128);
    // SAFETY: the callback only runs during this call and `list` outlives it.
    unsafe { EnumWindows(Some(callback), LPARAM(&mut list as *mut _ as isize)) }.context("EnumWindows")?;
    Ok(list)
}

/// Whether the window's thread has stopped processing messages. Positioning calls on a
/// hung window would block the caller, so they are refused instead.
pub fn is_hung(hwnd: Hwnd) -> bool {
    // SAFETY: accepts any handle value.
    unsafe { IsHungAppWindow(hwnd.raw()) }.as_bool()
}

/// Move/resize a window so its *visible* bounds equal `target`. Maximized windows are
/// restored first. Does not activate the window or change its z-order.
pub fn set_visible_rect(hwnd: Hwnd, target: Rect) -> Result<()> {
    if !is_window(hwnd) {
        return Err(Error::Other("the window no longer exists".into()));
    }
    if is_hung(hwnd) {
        return Err(Error::Other("the window is not responding".into()));
    }
    if target.is_empty() {
        return Err(Error::Other("refusing to give a window an empty size".into()));
    }
    // SAFETY: handle validated above; a concurrent destroy makes the calls fail cleanly.
    unsafe {
        if IsZoomed(hwnd.raw()).as_bool() || IsIconic(hwnd.raw()).as_bool() {
            // Return value is the previous visibility, not an error indicator.
            let _ = ShowWindow(hwnd.raw(), SW_SHOWNOACTIVATE);
        }
    }
    // Invisible borders: difference between the outer rectangle and the visible frame.
    let outer = raw_rect(hwnd)?;
    let visible = visible_rect(hwnd)?;
    let (dl, dt, dr, db) = (
        visible.left - outer.left,
        visible.top - outer.top,
        outer.right - visible.right,
        outer.bottom - visible.bottom,
    );
    // Guard against nonsensical deltas from windows that draw their own frames.
    let sane = |d: i32| if (0..=64).contains(&d) { d } else { 0 };
    let (dl, dt, dr, db) = (sane(dl), sane(dt), sane(dr), sane(db));
    // SAFETY: plain value arguments.
    unsafe {
        SetWindowPos(
            hwnd.raw(),
            None,
            target.left - dl,
            target.top - dt,
            target.width() + dl + dr,
            target.height() + dt + db,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    }
    .context("SetWindowPos")
}

/// Set or clear the always-on-top flag (`WS_EX_TOPMOST`) without moving or activating.
pub fn set_topmost(hwnd: Hwnd, topmost: bool) -> Result<()> {
    if !is_window(hwnd) {
        return Err(Error::Other("the window no longer exists".into()));
    }
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    // SAFETY: plain value arguments.
    unsafe {
        SetWindowPos(
            hwnd.raw(),
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    }
    .context("SetWindowPos(topmost)")
}

pub fn is_topmost(hwnd: Hwnd) -> bool {
    // SAFETY: value-returning query; zero for stale handles.
    let ex = unsafe { GetWindowLongPtrW(hwnd.raw(), GWL_EXSTYLE) } as u32;
    ex & WS_EX_TOPMOST_BITS != 0
}

/// Previous layering state of a window, captured by [`set_opacity`] so it can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpacityState {
    was_layered: bool,
    alpha: Option<u8>,
}

/// Make a window translucent (`alpha` 1-255). Returns the state to pass to
/// [`restore_opacity`].
pub fn set_opacity(hwnd: Hwnd, alpha: u8) -> Result<OpacityState> {
    if !is_window(hwnd) {
        return Err(Error::Other("the window no longer exists".into()));
    }
    // SAFETY: out pointers are valid locals; handle validated above.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd.raw(), GWL_EXSTYLE) as u32;
        let was_layered = ex & WS_EX_LAYERED_BITS != 0;
        let mut old_alpha = 0u8;
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
        let had_alpha = was_layered
            && GetLayeredWindowAttributes(hwnd.raw(), None, Some(&mut old_alpha), Some(&mut flags)).is_ok()
            && flags.contains(LWA_ALPHA);
        if !was_layered {
            windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
            let prev = SetWindowLongPtrW(hwnd.raw(), GWL_EXSTYLE, (ex | WS_EX_LAYERED_BITS) as isize);
            if prev == 0 {
                let err = windows::core::Error::from_thread();
                if err.code().is_err() {
                    return Err(Error::Os {
                        context: "SetWindowLongPtr(WS_EX_LAYERED)".into(),
                        source: err,
                    });
                }
            }
        }
        SetLayeredWindowAttributes(
            hwnd.raw(),
            windows::Win32::Foundation::COLORREF(0),
            alpha.max(1),
            LWA_ALPHA,
        )
        .context("SetLayeredWindowAttributes")?;
        Ok(OpacityState {
            was_layered,
            alpha: had_alpha.then_some(old_alpha),
        })
    }
}

/// Undo [`set_opacity`].
pub fn restore_opacity(hwnd: Hwnd, state: OpacityState) -> Result<()> {
    if !is_window(hwnd) {
        return Ok(()); // nothing left to restore
    }
    // SAFETY: handle validated above; plain value arguments.
    unsafe {
        if state.was_layered {
            SetLayeredWindowAttributes(
                hwnd.raw(),
                windows::Win32::Foundation::COLORREF(0),
                state.alpha.unwrap_or(255),
                LWA_ALPHA,
            )
            .context("SetLayeredWindowAttributes")
        } else {
            let ex = GetWindowLongPtrW(hwnd.raw(), GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(hwnd.raw(), GWL_EXSTYLE, (ex & !WS_EX_LAYERED_BITS) as isize);
            Ok(())
        }
    }
}

/// Bring a window to the foreground (restoring it if minimized). Windows may refuse
/// focus changes from background processes; that is reported as an error.
pub fn activate(hwnd: Hwnd) -> Result<()> {
    if !is_window(hwnd) {
        return Err(Error::Other("the window no longer exists".into()));
    }
    // SAFETY: handle validated above.
    unsafe {
        if IsIconic(hwnd.raw()).as_bool() {
            let _ = ShowWindow(hwnd.raw(), SW_RESTORE);
        }
        if SetForegroundWindow(hwnd.raw()).as_bool() {
            Ok(())
        } else {
            Err(Error::Other("Windows did not allow the window to take focus".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WindowInfo {
        WindowInfo {
            hwnd: Hwnd(1),
            title: "Doc".into(),
            class: "Notepad".into(),
            pid: 1,
            tid: 1,
            exe: "notepad.exe".into(),
            rect: Rect::new(100, 100, 900, 700),
            style: WS_CAPTION_BITS | WS_THICKFRAME_BITS,
            ex_style: 0,
            visible: true,
            minimized: false,
            maximized: false,
            cloaked: false,
            has_owner: false,
        }
    }

    const MON: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn ordinary_window_is_manageable() {
        assert_eq!(sample().manageable(&MON), Ok(()));
    }

    #[test]
    fn policy_rejections() {
        let cases: Vec<(Box<dyn Fn(&mut WindowInfo)>, Unmanageable)> = vec![
            (Box::new(|w| w.visible = false), Unmanageable::NotVisible),
            (Box::new(|w| w.minimized = true), Unmanageable::Minimized),
            (Box::new(|w| w.cloaked = true), Unmanageable::Cloaked),
            (Box::new(|w| w.style |= WS_CHILD_BITS), Unmanageable::ChildWindow),
            (
                Box::new(|w| w.class = "Shell_TrayWnd".into()),
                Unmanageable::ShellWindow,
            ),
            (
                Box::new(|w| w.ex_style |= WS_EX_TOOLWINDOW_BITS),
                Unmanageable::ToolWindow,
            ),
            (Box::new(|w| w.has_owner = true), Unmanageable::OwnedPopup),
            (
                Box::new(|w| {
                    w.style = WS_POPUP_BITS;
                    w.rect = MON;
                }),
                Unmanageable::FullScreen,
            ),
            (Box::new(|w| w.style = WS_POPUP_BITS), Unmanageable::NoFrame),
        ];
        for (mutate, expected) in cases {
            let mut w = sample();
            mutate(&mut w);
            assert_eq!(w.manageable(&MON), Err(expected));
        }
    }

    #[test]
    fn app_window_flag_overrides_owner_and_toolwindow() {
        let mut w = sample();
        w.has_owner = true;
        w.ex_style = WS_EX_APPWINDOW_BITS | WS_EX_TOOLWINDOW_BITS;
        assert_eq!(w.manageable(&MON), Ok(()));
    }

    #[test]
    fn stale_handles_are_rejected() {
        let bogus = Hwnd(0x7FFF_FFF0);
        assert!(!is_window(bogus));
        assert!(info(bogus).is_err());
        assert!(set_topmost(bogus, true).is_err());
        assert!(set_visible_rect(bogus, Rect::new(0, 0, 10, 10)).is_err());
        assert!(!is_window(Hwnd(0)));
    }

    #[test]
    fn enumerate_finds_windows() {
        let all = enumerate().unwrap();
        assert!(!all.is_empty());
        // Every enumerated handle can be inspected (or has vanished meanwhile).
        for h in all.into_iter().take(40) {
            if let Ok(i) = info(h) {
                assert_eq!(i.hwnd, h);
            }
        }
    }
}
