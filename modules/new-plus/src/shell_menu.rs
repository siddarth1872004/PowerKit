//! "New+" in the classic Explorer context menu of folders and folder backgrounds,
//! registered per user under HKCU so no elevation is needed.

use anyhow::{anyhow, Result};
use powerkit_platform::shell;

const VERB: &str = "PowerKitNewPlus";
const TITLE: &str = "New+";

/// (class key, argument placeholder): a folder, and a folder's background.
const CLASSES: [(&str, &str); 2] = [
    (r"Software\Classes\Directory\shell", "%1"),
    (r"Software\Classes\Directory\Background\shell", "%V"),
];

/// Add or remove the menu entries. Removal checks first so a disabled feature never
/// writes to the registry.
pub fn sync(enabled: bool) -> Result<()> {
    // The Windows 11 top-level entry replaces the classic one when it is in use.
    let top = powerkit_menu::show(crate::ID, enabled).map_err(|e| anyhow!("Explorer menu: {e}"))?;
    let enabled = enabled && !top;
    let exe = std::env::current_exe().map_err(|e| anyhow!("locating powerkit.exe: {e}"))?;
    for (class, placeholder) in CLASSES {
        let key = format!(r"{class}\{VERB}");
        if enabled {
            let command = format!("\"{}\" --new \"{placeholder}\"", exe.display());
            shell::registry_set(&key, "", TITLE).map_err(|e| anyhow!(e))?;
            shell::registry_set(&key, "Icon", &format!("\"{}\"", exe.display())).map_err(|e| anyhow!(e))?;
            shell::registry_set(&format!(r"{key}\command"), "", &command).map_err(|e| anyhow!(e))?;
        } else if shell::registry_get(&key, "").map_err(|e| anyhow!(e))?.is_some() {
            shell::registry_delete_tree(&key).map_err(|e| anyhow!(e))?;
        }
    }
    Ok(())
}
