//! File operations `std` does not offer on Windows.

use crate::{Context, Result};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Rename a file or folder, failing if `to` exists. `std::fs::rename` silently
/// replaces an existing file on Windows, which a rename tool must never do.
pub fn move_no_replace(from: &Path, to: &Path) -> Result<()> {
    let wfrom = wide_path(from);
    let wto = wide_path(to);
    // SAFETY: both buffers are NUL-terminated and outlive the call. Flags 0 means
    // same-volume rename only, never replacing the destination.
    unsafe { MoveFileExW(PCWSTR(wfrom.as_ptr()), PCWSTR(wto.as_ptr()), MOVE_FILE_FLAGS(0)) }.context(format!(
        "renaming {} to {}",
        from.display(),
        to.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_replace() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        assert!(move_no_replace(&a, &b).is_err());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        let c = dir.path().join("c.txt");
        move_no_replace(&a, &c).unwrap();
        assert!(!a.exists() && c.exists());
    }
}
