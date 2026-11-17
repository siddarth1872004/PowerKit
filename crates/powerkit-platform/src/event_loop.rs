//! The host thread's message loop and the event sources that feed it.
//!
//! A hidden top-level "helper" window receives hotkeys, timers, display-change and
//! clipboard notifications, tray callbacks and cross-thread wake-ups. Its window
//! procedure only translates messages into [`Event`]s and queues them; the queue is
//! drained by [`EventLoop::run`] between messages, so handlers never run re-entrantly
//! inside a window procedure.

use crate::util::{pcwstr, wide};
use crate::window::Hwnd;
use crate::{Context, Error, Result};
use powerkit_core::{Event, TrayEvent, WindowEventKind};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::marker::PhantomData;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) const WM_WAKE: u32 = WM_APP + 1;
pub(crate) const WM_TRAY: u32 = WM_APP + 2;
const CLASS_NAME: &str = "PowerKit.Host";

thread_local! {
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static HELPER: Cell<isize> = const { Cell::new(0) };
    static NEXT_TIMER: Cell<usize> = const { Cell::new(1) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

/// Queue an event for the handler. Consecutive duplicates of coalescable events are
/// dropped so bursts (display changes, wake-ups) are handled once.
pub(crate) fn push(event: Event) {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        // Location changes arrive in bursts while a window is dragged; keep one.
        let coalesce = matches!(
            event,
            Event::DisplayChanged
                | Event::Wake
                | Event::ClipboardChanged
                | Event::Window {
                    kind: WindowEventKind::LocationChange,
                    ..
                }
        );
        if coalesce && q.back() == Some(&event) {
            return;
        }
        let was_empty = q.is_empty();
        q.push_back(event);
        // Low-level hooks run inside GetMessage without making it return, so an event
        // they queue would wait for the next unrelated message.
        let helper = HELPER.with(Cell::get);
        if was_empty && helper != 0 {
            // SAFETY: posting to this thread's own helper window; WM_NULL does nothing.
            if let Err(e) = unsafe { PostMessageW(Some(HWND(helper as *mut _)), WM_NULL, WPARAM(0), LPARAM(0)) } {
                tracing::debug!(error = %e, "event loop not woken");
            }
        }
    });
}

/// The helper window of the current thread's event loop.
pub(crate) fn helper() -> Result<HWND> {
    let h = HELPER.with(Cell::get);
    if h == 0 {
        return Err(Error::Other("no event loop exists on this thread".into()));
    }
    Ok(HWND(h as *mut core::ffi::c_void))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => push(Event::Hotkey(wparam.0 as i32)),
        WM_TIMER => push(Event::Timer(wparam.0)),
        WM_WAKE => push(Event::Wake),
        WM_DISPLAYCHANGE => push(Event::DisplayChanged),
        // SPI_SETWORKAREA: the taskbar moved or resized.
        WM_SETTINGCHANGE if wparam.0 == 0x002F => push(Event::DisplayChanged),
        WM_CLIPBOARDUPDATE => push(Event::ClipboardChanged),
        // Without NOTIFYICON_VERSION_4, wparam is the id of the icon clicked.
        WM_TRAY => {
            let icon = wparam.0 as u32;
            match (lparam.0 as u32) & 0xFFFF {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => push(Event::Tray {
                    icon,
                    event: TrayEvent::Activate,
                }),
                WM_RBUTTONUP | WM_CONTEXTMENU => push(Event::Tray {
                    icon,
                    event: TrayEvent::ContextMenu,
                }),
                _ => {}
            }
        }
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION if wparam.0 != 0 => push(Event::Quit),
        WM_CLOSE => push(Event::Quit),
        m if m != 0 && m == TASKBAR_CREATED.with(Cell::get) => crate::tray::taskbar_recreated(),
        // SAFETY: forwarding the unmodified arguments of this callback.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

/// Wakes the event loop from another thread; the loop then delivers [`Event::Wake`].
#[derive(Debug, Clone, Copy)]
pub struct Waker(isize);

// SAFETY: a window handle is an opaque identifier and PostMessage is thread-safe.
unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

impl Waker {
    pub fn wake(&self) {
        // SAFETY: posting to a stale handle fails harmlessly.
        if let Err(e) = unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)) } {
            tracing::debug!(error = %e, "wake-up not delivered (event loop gone)");
        }
    }

    /// Ask the event loop to shut down (delivers [`Event::Quit`]).
    pub fn quit(&self) {
        // SAFETY: as above.
        let _ = unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

/// Owns the helper window. Create exactly one per thread, before any other platform
/// object that needs the loop (hotkeys, timers, tray).
pub struct EventLoop {
    hwnd: HWND,
    _not_send: PhantomData<*const ()>,
}

impl EventLoop {
    pub fn new() -> Result<EventLoop> {
        if HELPER.with(Cell::get) != 0 {
            return Err(Error::Other("an event loop already exists on this thread".into()));
        }
        let class = wide(CLASS_NAME);
        // SAFETY: `class` outlives the calls; `wndproc` matches the WNDPROC contract.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                ..Default::default()
            };
            // Registration fails with "class already exists" when a previous loop on
            // another thread registered it; CreateWindowEx then still succeeds.
            RegisterClassW(&wc);
            TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(windows::core::w!("TaskbarCreated"))));
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                pcwstr(&class),
                pcwstr(&class),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(helper)")?
        };
        HELPER.with(|c| c.set(hwnd.0 as isize));
        Ok(EventLoop {
            hwnd,
            _not_send: PhantomData,
        })
    }

    pub fn waker(&self) -> Waker {
        Waker(self.hwnd.0 as isize)
    }

    /// Pump messages until `handler` returns `false` or the thread receives `WM_QUIT`.
    pub fn run(&mut self, mut handler: impl FnMut(Event) -> bool) {
        let mut msg = MSG::default();
        loop {
            // SAFETY: `msg` is a valid out pointer. GetMessage blocks without consuming
            // CPU until a message arrives.
            let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if got.0 <= 0 {
                return; // WM_QUIT or failure
            }
            // SAFETY: `msg` was filled in by GetMessage.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !Self::drain(&mut handler) {
                return;
            }
        }
    }

    /// Process pending messages without blocking, then return. Used by tests.
    pub fn poll(&mut self, mut handler: impl FnMut(Event) -> bool) -> bool {
        let mut msg = MSG::default();
        // SAFETY: as in `run`.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return false;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Self::drain(&mut handler)
    }

    fn drain(handler: &mut impl FnMut(Event) -> bool) -> bool {
        // Pop one at a time: the handler may trigger window procedures that push more.
        while let Some(event) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
            if !handler(event) {
                return false;
            }
        }
        true
    }
}

impl Drop for EventLoop {
    fn drop(&mut self) {
        HELPER.with(|c| c.set(0));
        QUEUE.with(|q| q.borrow_mut().clear());
        // SAFETY: the window was created by this object on this thread.
        if let Err(e) = unsafe { DestroyWindow(self.hwnd) } {
            tracing::debug!(error = %e, "helper window already destroyed");
        }
    }
}

/// Start a repeating timer; [`Event::Timer`] carries the returned id.
pub fn set_timer(interval_ms: u32) -> Result<usize> {
    let hwnd = helper()?;
    let id = NEXT_TIMER.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1).max(1));
        id
    });
    // SAFETY: plain value arguments; no callback is supplied.
    if unsafe { SetTimer(Some(hwnd), id, interval_ms.max(10), None) } == 0 {
        return Err(Error::Os {
            context: "SetTimer".into(),
            source: windows::core::Error::from_thread(),
        });
    }
    Ok(id)
}

pub fn kill_timer(id: usize) {
    if let Ok(hwnd) = helper() {
        // SAFETY: plain value arguments. Failure means the timer is already gone.
        let _ = unsafe { KillTimer(Some(hwnd), id) };
    }
}

thread_local! {
    static CLIPBOARD_LISTENERS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Subscribe to / unsubscribe from clipboard change notifications. Counted, so
/// several modules can listen at once: each `true` needs its own `false`.
pub fn listen_clipboard(enable: bool) -> Result<()> {
    let hwnd = helper()?;
    let count = CLIPBOARD_LISTENERS.get();
    match (enable, count) {
        (false, 0) => return Ok(()),
        (true, 1..) | (false, 2..) => {}
        // SAFETY: the helper window belongs to this thread.
        (true, 0) => unsafe { AddClipboardFormatListener(hwnd) }.context("AddClipboardFormatListener")?,
        // SAFETY: as above.
        (false, 1) => unsafe { RemoveClipboardFormatListener(hwnd) }.context("RemoveClipboardFormatListener")?,
    }
    CLIPBOARD_LISTENERS.set(if enable { count + 1 } else { count - 1 });
    Ok(())
}

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MOVESIZESTART: u32 = 0x000A;
const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_DESTROY: u32 = 0x8001;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_OBJECT_HIDE: u32 = 0x8003;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const WINEVENT_SKIPOWNPROCESS: u32 = 2;

unsafe extern "system" fn winevent_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Only events about a window itself (OBJID_WINDOW, CHILDID_SELF), not its parts,
    // the caret or the cursor.
    if id_object != 0 || id_child != 0 || hwnd.is_invalid() {
        return;
    }
    let kind = match event {
        EVENT_SYSTEM_FOREGROUND => WindowEventKind::Foreground,
        EVENT_SYSTEM_MOVESIZESTART => WindowEventKind::MoveSizeStart,
        EVENT_SYSTEM_MOVESIZEEND => WindowEventKind::MoveSizeEnd,
        EVENT_SYSTEM_MINIMIZESTART => WindowEventKind::Minimized,
        EVENT_SYSTEM_MINIMIZEEND => WindowEventKind::Restored,
        EVENT_OBJECT_DESTROY => WindowEventKind::Destroyed,
        EVENT_OBJECT_SHOW => WindowEventKind::Shown,
        EVENT_OBJECT_HIDE => WindowEventKind::Hidden,
        EVENT_OBJECT_LOCATIONCHANGE => WindowEventKind::LocationChange,
        _ => return,
    };
    let event = Event::Window {
        kind,
        hwnd: hwnd.0 as isize,
    };
    // Through `push`, so the loop is woken: an event queued without waking it would
    // leave the queue non-empty and keep every later event from waking it either.
    push(event);
}

/// An installed WinEvent hook; uninstalled on drop. Events are delivered as
/// [`Event::Window`] through the event loop of the installing thread.
pub struct WinEventHook {
    hook: HWINEVENTHOOK,
    _not_send: PhantomData<*const ()>,
}

impl Drop for WinEventHook {
    fn drop(&mut self) {
        // SAFETY: the hook was installed by this thread and is removed exactly once.
        let _ = unsafe { UnhookWinEvent(self.hook) };
    }
}

fn install(min: u32, max: u32, pid: u32, tid: u32, flags: u32) -> Result<WinEventHook> {
    helper()?; // out-of-context hooks need a message loop on this thread
               // SAFETY: `winevent_proc` matches WINEVENTPROC and lives for the whole program.
    let hook = unsafe { SetWinEventHook(min, max, None, Some(winevent_proc), pid, tid, flags) };
    if hook.is_invalid() {
        return Err(Error::Other(format!("SetWinEventHook({min:#x}..{max:#x}) failed")));
    }
    Ok(WinEventHook {
        hook,
        _not_send: PhantomData,
    })
}

/// System-wide hooks for cheap, infrequent events: foreground changes, move/size
/// start/end and minimize/restore. Events from PowerKit's own windows are skipped.
pub fn hook_global_window_events() -> Result<Vec<WinEventHook>> {
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    Ok(vec![
        install(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, 0, 0, flags)?,
        install(EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND, 0, 0, flags)?,
        install(EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND, 0, 0, flags)?,
    ])
}

/// Hook the chatty per-window events (move, show, hide, destroy) for one thread only,
/// so tracking a window costs nothing while other applications are busy.
pub fn hook_thread_window_events(window: Hwnd) -> Result<WinEventHook> {
    let (pid, tid) =
        crate::window::thread_process(window).ok_or_else(|| Error::Other("the window no longer exists".into()))?;
    install(
        EVENT_OBJECT_DESTROY,
        EVENT_OBJECT_LOCATIONCHANGE,
        pid,
        tid,
        WINEVENT_OUTOFCONTEXT,
    )
}
