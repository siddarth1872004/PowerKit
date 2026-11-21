//! PowerKit's entry in Settings > Apps > Installed apps, and taking PowerKit off the PC.

use powerkit_platform::shell;
use std::path::Path;

const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PowerKit";
pub const FLAG: &str = "--uninstall";

/// List PowerKit in Installed apps, uninstalled by running `exe` with [`FLAG`].
/// Leaves the entry alone when it already says the same.
pub fn register(exe: &Path) -> Result<(), String> {
    let command = format!("\"{}\" {FLAG}", exe.display());
    let current = (
        shell::registry_get(KEY, "UninstallString").map_err(|e| e.to_string())?,
        shell::registry_get(KEY, "DisplayVersion").map_err(|e| e.to_string())?,
    );
    if current == (Some(command.clone()), Some(crate::update::CURRENT.to_string())) {
        return Ok(());
    }
    let folder = exe.parent().unwrap_or(exe);
    let size_kb = std::fs::metadata(exe).map_or(0, |m| (m.len() / 1024) as u32);
    let strings = [
        ("DisplayName", "PowerKit".to_string()),
        ("DisplayVersion", crate::update::CURRENT.to_string()),
        ("DisplayIcon", exe.display().to_string()),
        ("InstallLocation", folder.display().to_string()),
        (
            "URLInfoAbout",
            "https://github.com/siddarth1872004/PowerKit".to_string(),
        ),
        ("UninstallString", command),
    ];
    for (name, value) in strings {
        shell::registry_set(KEY, name, &value).map_err(|e| e.to_string())?;
    }
    for (name, value) in [("NoModify", 1), ("NoRepair", 1), ("EstimatedSize", size_kb)] {
        shell::registry_set_dword(KEY, name, value).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Take PowerKit out of Installed apps.
pub fn unregister() -> Result<(), String> {
    shell::registry_delete_tree(KEY).map_err(|e| e.to_string())
}
