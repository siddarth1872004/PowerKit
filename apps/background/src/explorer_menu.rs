//! Puts PowerKit's entries at the top of the Windows 11 Explorer context menu.
//!
//! Windows only shows them there for a signed app package, so PowerKit makes one
//! itself: it writes the menu's COM server (built into powerkit.exe) and icons to a
//! folder, signs a sparse package naming that folder with a certificate of its own,
//! has the machine trust the certificate (one administrator approval), and registers
//! the package for the user. Removing it leaves the certificate trusted, so turning
//! the menu on again needs no approval.
//!
//! Everything here blocks for seconds, so it runs off the event loop thread.

use crate::icons;
use powerkit_menu::{ACTIVE_VALUE, DLL, ENTRIES, EXE_VALUE, KEY, LOGO, PACKAGE};
use powerkit_platform::package::{self, Certificate};
use powerkit_platform::shell;
use std::path::{Path, PathBuf};

const SERVER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/powerkit_menu.dll"));
/// Subject of the signing certificate, and so the package's publisher.
const CERT_NAME: &str = "PowerKit Explorer Menu";
/// Value under [`KEY`] naming the folder the registered package uses.
const FOLDER_VALUE: &str = "Folder";
/// powerkit.exe option the elevated helper runs with, followed by the certificate
/// in hexadecimal.
pub const TRUST_FLAG: &str = "--trust-menu-cert";

fn publisher() -> String {
    format!("CN={CERT_NAME}")
}

/// Folder name for this build's files: a new server gets a new folder, because the
/// old DLL stays loaded (and locked) in Explorer's helper process for a while.
fn folder_name() -> String {
    // FNV-1a: stable across builds and Rust versions, unlike the std hasher.
    let hash = SERVER.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// The 256-pixel PNG image inside an .ico file, if it holds one.
fn png_in_ico(ico: &[u8]) -> Option<&[u8]> {
    let count = u16::from_le_bytes(ico.get(4..6)?.try_into().ok()?) as usize;
    (0..count).find_map(|i| {
        let entry = ico.get(6 + i * 16..6 + (i + 1) * 16)?;
        let size = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
        let data = ico.get(offset..offset.checked_add(size)?)?;
        data.starts_with(b"\x89PNG").then_some(data)
    })
}

/// Package version: PowerKit's own, with the fourth part Windows requires.
fn version() -> String {
    format!("{}.0", env!("CARGO_PKG_VERSION"))
}

fn write_files(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("creating {}: {e}", folder.display()))?;
    let mut files: Vec<(String, &[u8])> = vec![(DLL.to_string(), SERVER)];
    for e in &ENTRIES {
        let ico = icons::module(e.module).ok_or_else(|| format!("no icon for {}", e.module))?;
        files.push((format!("{}.ico", e.module), ico));
    }
    for (name, bytes) in files {
        let path = folder.join(name);
        // The DLL may be loaded from a folder made earlier; same name, same content.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
            continue;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Have the machine trust `cert`, asking for administrator approval if it does not.
fn trust(cert: &Certificate, exe: &Path) -> Result<(), String> {
    if cert.trusted_by_machine().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let hex: String = cert.der().iter().map(|b| format!("{b:02x}")).collect();
    match shell::run_elevated(exe, &format!("{TRUST_FLAG} {hex}")) {
        Ok(0) => Ok(()),
        Ok(code) => Err(format!("trusting the certificate failed (exit code {code})")),
        Err(e) => Err(format!("trusting the certificate: {e}")),
    }
}

/// Entry point of the elevated helper: trust the hex-encoded certificate.
pub fn trust_elevated(hex: &str) -> Result<(), String> {
    let der = (0..hex.len())
        .step_by(2)
        .map(|i| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect::<Option<Vec<u8>>>()
        .filter(|d| !d.is_empty())
        .ok_or("the certificate is not valid hexadecimal")?;
    package::trust_for_machine(&der).map_err(|e| e.to_string())
}

fn remove_installed() -> Result<(), String> {
    for name in package::installed(PACKAGE, &publisher()).map_err(|e| e.to_string())? {
        package::remove(&name).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Delete the files of earlier builds; a folder still in use is left for next time.
fn remove_old_folders(menu: &Path, keep: &str) {
    let Ok(dir) = std::fs::read_dir(menu) else { return };
    for entry in dir.flatten() {
        if entry.file_name() != keep && entry.path().is_dir() {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                tracing::debug!(folder = %entry.path().display(), "old Explorer menu files not removed yet: {e}");
            }
        }
    }
}

fn set(name: &str, value: &str) -> Result<(), String> {
    shell::registry_set(KEY, name, value).map_err(|e| e.to_string())
}

fn set_active(active: bool) -> Result<(), String> {
    shell::registry_set_dword(KEY, ACTIVE_VALUE, active.into()).map_err(|e| e.to_string())
}

/// Make the menu show, installing or updating the package when needed. `menu` is
/// PowerKit's folder for the menu's files.
pub fn install(menu: &Path, exe: &Path) -> Result<(), String> {
    let name = folder_name();
    let folder = menu.join(&name);
    set(EXE_VALUE, &exe.to_string_lossy())?;
    let current = shell::registry_get(KEY, FOLDER_VALUE)
        .map_err(|e| e.to_string())?
        .as_deref()
        == Some(&name)
        && folder.join(DLL).is_file()
        && !package::installed(PACKAGE, &publisher())
            .map_err(|e| e.to_string())?
            .is_empty();
    if !current {
        write_files(&folder)?;
        let cert = Certificate::find_or_create(CERT_NAME).map_err(|e| e.to_string())?;
        trust(&cert, exe)?;
        let msix = menu.join(format!("{name}.msix"));
        let logo = png_in_ico(icons::APP).ok_or("the PowerKit icon holds no PNG image")?;
        package::write_package(
            &msix,
            &powerkit_menu::manifest(&publisher(), &version()),
            &[(LOGO, "image/png", logo)],
        )
        .map_err(|e| e.to_string())?;
        package::sign_package(&msix, &cert).map_err(|e| e.to_string())?;
        remove_installed()?;
        let registered = package::register_sparse(&msix, &folder).map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&msix);
        registered?;
        set(FOLDER_VALUE, &name)?;
        tracing::info!(folder = %folder.display(), "Explorer menu package registered");
    }
    set_active(true)?;
    remove_old_folders(menu, &name);
    Ok(())
}

/// Take the entries out of the top of the menu and unregister the package.
pub fn uninstall(menu: &Path) -> Result<(), String> {
    set_active(false)?;
    remove_installed()?;
    if shell::registry_get(KEY, FOLDER_VALUE)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        shell::registry_delete(KEY, FOLDER_VALUE).map_err(|e| e.to_string())?;
    }
    remove_old_folders(menu, "");
    Ok(())
}

/// Whether the registry says a package was installed, so turning the setting off has
/// something to undo. Cheap, unlike asking the package manager.
pub fn maybe_installed() -> bool {
    shell::registry_get(KEY, FOLDER_VALUE).is_ok_and(|v| v.is_some())
        || shell::registry_get_dword(KEY, ACTIVE_VALUE).is_ok_and(|v| v == Some(1))
}

/// The folder for the menu's files under PowerKit's data folder.
pub fn folder(root: &Path) -> PathBuf {
    root.join("menu")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_icon_holds_the_logo() {
        let png = png_in_ico(icons::APP).expect("a PNG entry");
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(png_in_ico(b"junk"), None);
    }

    #[test]
    fn the_package_version_has_four_parts() {
        assert_eq!(version().split('.').count(), 4);
        assert_eq!(folder_name().len(), 16);
    }

    #[test]
    fn the_trust_helper_rejects_bad_input() {
        assert!(trust_elevated("zz").is_err());
        assert!(trust_elevated("").is_err());
    }
}
