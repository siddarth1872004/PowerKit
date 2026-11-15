//! Listing and ending processes.

use crate::util::from_wide;
use crate::{Context, Error, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, TerminateProcess, PROCESS_TERMINATE};

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this module and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Running processes as `(pid, executable name)`, excluding the system idle process.
pub fn list() -> Result<Vec<(u32, String)>> {
    // SAFETY: the snapshot handle is closed by `Handle`; the entry's `dwSize` is set as
    // the API requires before the first call.
    unsafe {
        let snap = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).context("CreateToolhelp32Snapshot")?);
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut more = Process32FirstW(snap.0, &mut entry).is_ok();
        while more {
            if entry.th32ProcessID != 0 {
                out.push((entry.th32ProcessID, from_wide(&entry.szExeFile)));
            }
            more = Process32NextW(snap.0, &mut entry).is_ok();
        }
        Ok(out)
    }
}

/// End a process immediately. PowerKit refuses to end itself.
pub fn terminate(pid: u32) -> Result<()> {
    // SAFETY: no pointers involved; the process handle is closed by `Handle`.
    unsafe {
        if pid == GetCurrentProcessId() {
            return Err(Error::Other("PowerKit cannot end itself this way".into()));
        }
        let h = Handle(OpenProcess(PROCESS_TERMINATE, false, pid).context(format!("opening process {pid}"))?);
        TerminateProcess(h.0, 1).context(format!("ending process {pid}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_this_process_and_refuses_to_end_it() {
        let me = std::process::id();
        let all = list().unwrap();
        assert!(all
            .iter()
            .any(|(pid, name)| *pid == me && name.to_lowercase().ends_with(".exe")));
        assert!(terminate(me).is_err());
    }
}
