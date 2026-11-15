//! The Windows light/dark theme (Settings > Personalization > Colors).

use crate::shell::{registry_get_dword, registry_set_dword};
use crate::util::{pcwstr, wide};
use crate::Result;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const APPS: &str = "AppsUseLightTheme";
const SYSTEM: &str = "SystemUsesLightTheme";

/// Whether applications (`system == false`) or the taskbar and Start menu
/// (`system == true`) use the light theme. A missing value means light, as in Windows.
pub fn is_light(system: bool) -> Result<bool> {
    Ok(registry_get_dword(KEY, if system { SYSTEM } else { APPS })?.unwrap_or(1) != 0)
}

/// Switch the selected parts to light or dark and tell running applications.
pub fn set_light(light: bool, apps: bool, system: bool) -> Result<()> {
    let v = u32::from(light);
    if apps {
        registry_set_dword(KEY, APPS, v)?;
    }
    if system {
        registry_set_dword(KEY, SYSTEM, v)?;
    }
    // The broadcast waits on every top-level window in turn, so it runs off the event
    // loop thread; the registry already holds the new theme, so a failed broadcast only
    // delays applications noticing it.
    std::thread::spawn(|| {
        let area = wide("ImmersiveColorSet");
        // SAFETY: `area` is NUL-terminated and outlives the call; the message is sent
        // synchronously, so the pointer is not retained.
        let sent = unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(pcwstr(&area).0 as isize),
                SMTO_ABORTIFHUNG,
                2000,
                None,
            )
        };
        if sent.0 == 0 {
            tracing::warn!("theme change broadcast did not complete");
        }
    });
    Ok(())
}
