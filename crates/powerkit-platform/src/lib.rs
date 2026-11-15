//! Safe wrappers around the Win32 APIs PowerKit uses.
//!
//! This crate is the only place in the workspace that contains `unsafe` code. Each
//! wrapper validates its inputs, converts failures into [`Error`] (never ignoring them
//! silently) and documents the invariants its `unsafe` blocks rely on.
//!
//! Threading model: everything that creates windows, hooks, timers or hotkeys must be
//! called on the thread that runs [`event_loop::EventLoop`]. Types that are tied to that
//! thread are deliberately `!Send`.
#![cfg(windows)]

pub mod accent_bar;
pub mod canvas;
pub mod clipboard;
pub mod crop;
pub mod dialog;
pub mod env;
pub mod event_loop;
pub mod frame;
pub mod fs;
pub mod hotkey;
pub mod http;
pub mod input;
pub mod keyhook;
pub mod magnifier;
pub mod monitor;
pub mod mousehook;
pub mod overlay;
pub mod pipe;
pub mod popup;
pub mod power;
pub mod process;
pub mod registry_window;
pub mod rename_window;
pub mod settings_window;
pub mod shell;
pub mod table_window;
pub mod taskbar;
pub mod testing;
pub mod theme;
pub mod time;
pub mod tray;
pub mod window;
pub mod zones;

mod util;

pub use window::Hwnd;

/// A failed operating-system call, with the operation that failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}: {source}")]
    Os {
        context: String,
        #[source]
        source: windows::core::Error,
    },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Whether the OS refused the call for lack of privilege (for example when the
    /// target window belongs to an elevated process).
    pub fn is_access_denied(&self) -> bool {
        matches!(self, Error::Os { source, .. }
            if source.code() == windows::Win32::Foundation::E_ACCESSDENIED)
    }
}

pub(crate) trait Context<T> {
    fn context(self, context: impl Into<String>) -> Result<T>;
}

impl<T> Context<T> for windows::core::Result<T> {
    fn context(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|source| Error::Os {
            context: context.into(),
            source,
        })
    }
}

/// Opt the process into per-monitor DPI awareness so every coordinate PowerKit sees is
/// in physical pixels. Must run before any window is created.
pub fn init_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    // SAFETY: no pointers involved. Fails harmlessly if awareness was already set
    // (for example by an application manifest).
    if let Err(e) = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        tracing::debug!(error = %e, "DPI awareness already configured");
    }
}
