//! "Unlock with PowerKit" in the classic Explorer context menu of files and folders,
//! registered per user under HKCU so no elevation is needed.

use anyhow::{anyhow, Result};
use powerkit_platform::shell;

const VERB: &str = "PowerKitUnlock";
const TITLE: &str = "Unlock with PowerKit";

const CLASSES: [&str; 2] = [r"Software\Classes\*\shell", r"Software\Classes\Directory\shell"];

/// Add or remove the menu entries; `extended` puts them in the Shift + right-click
/// menu only. Removal checks first so a disabled feature never writes to the
/// registry.
pub fn sync(enabled: bool, extended: bool) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| anyhow!("locating powerkit.exe: {e}"))?;
    for class in CLASSES {
        let key = format!(r"{class}\{VERB}");
        if enabled {
            let command = format!("\"{}\" --unlock \"%1\"", exe.display());
            shell::registry_set(&key, "", TITLE).map_err(|e| anyhow!(e))?;
            shell::registry_set(&key, "Icon", &format!("\"{}\"", exe.display())).map_err(|e| anyhow!(e))?;
            if extended {
                shell::registry_set(&key, "Extended", "").map_err(|e| anyhow!(e))?;
            } else {
                shell::registry_delete(&key, "Extended").map_err(|e| anyhow!(e))?;
            }
            shell::registry_set(&format!(r"{key}\command"), "", &command).map_err(|e| anyhow!(e))?;
        } else if shell::registry_get(&key, "").map_err(|e| anyhow!(e))?.is_some() {
            shell::registry_delete_tree(&key).map_err(|e| anyhow!(e))?;
        }
    }
    Ok(())
}
