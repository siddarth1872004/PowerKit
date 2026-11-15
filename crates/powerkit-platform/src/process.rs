//! Listing and ending processes.

use crate::util::{from_wide, wide};
use crate::{Context, Error, Result};
use std::path::Path;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME, HANDLE, WIN32_ERROR};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmService, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessTimes, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE,
};

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

/// A process that has a file open, as the Restart Manager reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locker {
    pub pid: u32,
    /// When the process started (FILETIME ticks). Together with the pid it names one
    /// process, so a reused pid is never mistaken for it.
    pub started: u64,
    /// The name the program gives itself, such as "Notepad".
    pub name: String,
    /// The service's short name when the process is a service.
    pub service: Option<String>,
    /// Full path of the program, when this user may query it.
    pub path: Option<String>,
}

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

fn check(code: WIN32_ERROR, what: &str) -> Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(Error::Os {
            context: what.into(),
            source: windows::core::Error::from_hresult(code.to_hresult()),
        })
    }
}

struct Session(u32);

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session was started by `lockers` and is ended once.
        let _ = unsafe { RmEndSession(self.0) };
    }
}

/// The processes that have any of `files` open. Folders are not looked into: pass
/// the files inside them.
pub fn lockers(files: &[&Path]) -> Result<Vec<Locker>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<Vec<u16>> = files.iter().map(|f| wide(&f.to_string_lossy())).collect();
    let names: Vec<PCWSTR> = names.iter().map(|n| PCWSTR(n.as_ptr())).collect();
    // SAFETY: the key buffer has the documented size; `names` points into strings
    // that outlive the session's registration call; the info buffer's length is
    // passed with it.
    unsafe {
        let mut handle = 0;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        check(
            RmStartSession(&mut handle, None, PWSTR(key.as_mut_ptr())),
            "RmStartSession",
        )?;
        let session = Session(handle);
        check(
            RmRegisterResources(session.0, Some(&names), None, None),
            "RmRegisterResources",
        )?;
        let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
        loop {
            let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
            let code = RmGetList(
                session.0,
                &mut needed,
                &mut count,
                Some(infos.as_mut_ptr()),
                &mut reasons,
            );
            if code == ERROR_MORE_DATA {
                // Processes can open the files between calls, so ask for some spare.
                infos.resize(needed as usize + 4, RM_PROCESS_INFO::default());
                continue;
            }
            check(code, "RmGetList")?;
            infos.truncate(count as usize);
            break;
        }
        Ok(infos
            .iter()
            .map(|i| {
                let pid = i.Process.dwProcessId;
                let service = from_wide(&i.strServiceShortName);
                Locker {
                    pid,
                    started: ticks(i.Process.ProcessStartTime),
                    name: from_wide(&i.strAppName),
                    service: (i.ApplicationType == RmService && !service.is_empty()).then_some(service),
                    path: crate::window::process_path(pid),
                }
            })
            .collect())
    }
}

/// End the process `pid` if it is still the one that started at `started`.
pub fn terminate_started(pid: u32, started: u64) -> Result<()> {
    // SAFETY: out pointers are valid locals; the handle is closed by `Handle`.
    unsafe {
        if pid == GetCurrentProcessId() {
            return Err(Error::Other("PowerKit cannot end itself this way".into()));
        }
        let h = Handle(
            OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .context(format!("opening process {pid}"))?,
        );
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        GetProcessTimes(h.0, created, exited, kernel, user).context(format!("reading process {pid}"))?;
        if ticks(times[0]) != started {
            return Err(Error::Other(format!("process {pid} has already ended")));
        }
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

    #[test]
    fn finds_and_ends_the_process_holding_a_file_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("held.txt");
        // A child whose output goes to the file keeps it open until it ends.
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::fs::File::create(&path).unwrap())
            .spawn()
            .unwrap();
        let found = lockers(&[&path]).unwrap();
        let held = found.iter().find(|l| l.pid == child.id()).expect("the child holds it");
        assert!(held
            .path
            .as_deref()
            .is_some_and(|p| p.to_lowercase().ends_with("cmd.exe")));
        assert!(
            terminate_started(held.pid, held.started + 1).is_err(),
            "a different start time is a different process"
        );
        terminate_started(held.pid, held.started).unwrap();
        child.wait().unwrap();
        assert!(lockers(&[&path]).unwrap().is_empty());
        assert!(lockers(&[]).unwrap().is_empty());
    }
}
