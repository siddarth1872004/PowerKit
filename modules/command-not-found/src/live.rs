use crate::{policy_runs_profiles, with_block, without_block, Report};
use anyhow::{Context, Result};
use powerkit_platform::shell;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Shell {
    name: &'static str,
    exe: PathBuf,
    profile: PathBuf,
}

/// The PowerShells installed: Windows PowerShell always, PowerShell 7 when found.
fn shells() -> Result<Vec<Shell>> {
    let docs = shell::documents_dir()?;
    let profile = |dir: &str| docs.join(dir).join("Microsoft.PowerShell_profile.ps1");
    let mut list = vec![Shell {
        name: "Windows PowerShell",
        exe: "powershell.exe".into(),
        profile: profile("WindowsPowerShell"),
    }];
    if let Some(exe) = find_pwsh() {
        list.push(Shell {
            name: "PowerShell 7",
            exe,
            profile: profile("PowerShell"),
        });
    }
    Ok(list)
}

fn find_pwsh() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join("pwsh.exe"));
    let installed =
        std::env::var_os("ProgramFiles").map(|p| Path::new(&p).join("PowerShell").join("7").join("pwsh.exe"));
    on_path.chain(installed).find(|p| p.is_file())
}

/// `Get-ExecutionPolicy` as that PowerShell sees it.
fn execution_policy(exe: &Path) -> Result<String> {
    let out = Command::new(exe)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-ExecutionPolicy",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("running {}", exe.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "{} could not read its execution policy: {}",
            exe.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn read(path: &Path) -> Result<String> {
    match std::fs::read(path) {
        // A profile saved by an editor may start with a UTF-8 byte order mark.
        Ok(bytes) => Ok(String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `text`, keeping a byte order mark the file already had.
fn write(path: &Path, text: &str) -> Result<()> {
    let bom = std::fs::read(path).is_ok_and(|b| b.starts_with(b"\xEF\xBB\xBF"));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut bytes = if bom { b"\xEF\xBB\xBF".to_vec() } else { Vec::new() };
    bytes.extend_from_slice(text.as_bytes());
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Add or update the block in each PowerShell profile that will run it.
pub fn install() -> Result<Report> {
    let mut report = Report::default();
    for shell in shells()? {
        let old = read(&shell.profile)?;
        let new = with_block(&old);
        if new == old {
            report.current.push(shell.name.into());
            continue;
        }
        let policy = execution_policy(&shell.exe)?;
        if !policy_runs_profiles(&policy) {
            tracing::info!("command not found: {} skipped, execution policy {policy}", shell.name);
            report.refused.push((shell.name.into(), policy));
            continue;
        }
        write(&shell.profile, &new)?;
        tracing::info!("command not found: added to {}", shell.profile.display());
        report.added.push(shell.name.into());
    }
    Ok(report)
}

/// Take the block out of every profile that has it, and return those shells. A
/// profile left empty is deleted.
pub fn uninstall() -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for shell in shells()? {
        let (rest, found) = without_block(&read(&shell.profile)?);
        if !found {
            continue;
        }
        if rest.trim().is_empty() {
            std::fs::remove_file(&shell.profile).with_context(|| format!("removing {}", shell.profile.display()))?;
        } else {
            write(&shell.profile, &rest)?;
        }
        removed.push(shell.name.into());
    }
    Ok(removed)
}
