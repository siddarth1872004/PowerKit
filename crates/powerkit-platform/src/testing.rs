//! Helpers for integration tests that need real windows. Not used by the product.

use crate::util::{pcwstr, wide};
use crate::window::Hwnd;
use crate::{Context, Result};
use powerkit_core::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: forwarding the unmodified arguments of this callback.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// An ordinary resizable application window owned by the calling thread. It is shown
/// without being activated so tests do not steal the user's focus.
pub struct TestWindow {
    hwnd: HWND,
}

impl TestWindow {
    pub fn new(title: &str, rect: Rect) -> Result<TestWindow> {
        Self::with_style(title, rect, WS_OVERLAPPEDWINDOW.0, 0)
    }

    pub fn with_style(title: &str, rect: Rect, style: u32, ex_style: u32) -> Result<TestWindow> {
        let class = wide("PowerKit.TestWindow");
        let wtitle = wide(title);
        // SAFETY: strings outlive the calls; `wndproc` matches the WNDPROC contract.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).context("GetModuleHandle")?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: pcwstr(&class),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(ex_style),
                pcwstr(&class),
                pcwstr(&wtitle),
                WINDOW_STYLE(style),
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                None,
                None,
                Some(instance.into()),
                None,
            )
            .context("CreateWindowEx(test)")?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            hwnd
        };
        pump();
        Ok(TestWindow { hwnd })
    }

    pub fn hwnd(&self) -> Hwnd {
        Hwnd::from_raw(self.hwnd)
    }

    pub fn show_state(&self, state: ShowState) {
        let cmd = match state {
            ShowState::Maximized => SW_SHOWMAXIMIZED,
            ShowState::Minimized => SW_SHOWMINNOACTIVE,
            ShowState::Normal => SW_SHOWNOACTIVATE,
            ShowState::Hidden => SW_HIDE,
        };
        // SAFETY: own window.
        let _ = unsafe { ShowWindow(self.hwnd, cmd) };
        pump();
    }

    /// Destroy the window now (it is otherwise destroyed on drop).
    pub fn destroy(&mut self) {
        if !self.hwnd.is_invalid() {
            // SAFETY: the window belongs to this thread.
            let _ = unsafe { DestroyWindow(self.hwnd) };
            self.hwnd = HWND::default();
            pump();
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        self.destroy();
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ShowState {
    Normal,
    Maximized,
    Minimized,
    Hidden,
}

/// Dispatch all pending messages of the calling thread.
pub fn pump() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid out pointer; messages are dispatched unmodified.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
