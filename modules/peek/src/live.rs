use powerkit_platform::explorer::is_explorer_class;
use powerkit_platform::window::{self, Hwnd};

/// Whether `hwnd` is a File Explorer window or the desktop.
pub fn is_explorer(hwnd: isize) -> bool {
    window::info(Hwnd(hwnd)).is_ok_and(|i| is_explorer_class(&i.class))
}
