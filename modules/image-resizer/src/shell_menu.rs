//! "Resize with PowerKit" in the classic Explorer context menu of pictures,
//! registered per user under HKCU so no elevation is needed.

use anyhow::{anyhow, Result};
use powerkit_platform::shell;

const VERB: &str = "PowerKitResize";
const TITLE: &str = "Resize with PowerKit";
/// Every file type Windows counts as a picture.
const CLASS: &str = r"Software\Classes\SystemFileAssociations\image\shell";

/// Add or remove the menu entry. Removal checks first so a disabled feature never
/// writes to the registry.
pub fn sync(enabled: bool) -> Result<()> {
    // The Windows 11 top-level entry replaces the classic one when it is in use.
    let top = powerkit_menu::show(crate::ID, enabled).map_err(|e| anyhow!("Explorer menu: {e}"))?;
    let enabled = enabled && !top;
    let key = format!(r"{CLASS}\{VERB}");
    if enabled {
        let exe = std::env::current_exe().map_err(|e| anyhow!("locating powerkit.exe: {e}"))?;
        let command = format!("\"{}\" --resize \"%1\"", exe.display());
        shell::registry_set(&key, "", TITLE).map_err(|e| anyhow!(e))?;
        shell::registry_set(&key, "Icon", &format!("\"{}\"", exe.display())).map_err(|e| anyhow!(e))?;
        // Without it Explorer hides the entry when more than 15 pictures are selected.
        shell::registry_set(&key, "MultiSelectModel", "Player").map_err(|e| anyhow!(e))?;
        shell::registry_set(&format!(r"{key}\command"), "", &command).map_err(|e| anyhow!(e))?;
    } else if shell::registry_get(&key, "").map_err(|e| anyhow!(e))?.is_some() {
        shell::registry_delete_tree(&key).map_err(|e| anyhow!(e))?;
    }
    Ok(())
}
