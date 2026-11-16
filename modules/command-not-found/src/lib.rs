//! Command Not Found: while it is on, PowerShell suggests the winget package that
//! provides a command it cannot find. It works through a marked block in the
//! PowerShell profiles, added when the utility is turned on and removed when it is
//! turned off.

#[cfg(windows)]
mod live;

use anyhow::{bail, Result};
use powerkit_core::Event;
use powerkit_plugin_api::{CommandSpec, Ctx, Manifest, Module, API_VERSION};
use serde_json::{json, Value};

pub const ID: &str = "command_not_found";

const BEGIN: &str = "# >>> PowerKit Command Not Found >>>";
const END: &str = "# <<< PowerKit Command Not Found <<<";

/// The profile block. Plain ASCII, because Windows PowerShell reads a profile without
/// a byte order mark in the ANSI code page.
const BLOCK: &str = r#"# >>> PowerKit Command Not Found >>>
# Added by PowerKit: suggests a winget package for a command that is not installed.
# Turning Command Not Found off in PowerKit removes this block.
if (Get-Command winget.exe -ErrorAction SilentlyContinue) {
    $ExecutionContext.InvokeCommand.CommandNotFoundAction = {
        param([string]$Name, $Lookup)
        if ($Lookup.CommandOrigin -ne 'Runspace' -or $Name -like 'get-*' -or $Name -match '[\\/:*?]') { return }
        try {
            Write-Host "Looking for a winget package with '$Name'..." -ForegroundColor DarkGray
            $lines = @(winget.exe search --command $Name --exact --source winget --accept-source-agreements --disable-interactivity 2>$null |
                ForEach-Object { "$_" -replace '^.*\r', '' })
            $rule = -1
            for ($i = 1; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^-{8,}\s*$') { $rule = $i; break } }
            if ($rule -lt 1) { return }
            $starts = @([regex]::Matches($lines[$rule - 1], '(?<=\s)\S') | ForEach-Object { $_.Index })
            if ($starts.Count -lt 1) { return }
            $at = $starts[0]
            $width = if ($starts.Count -gt 1) { $starts[1] - $at } else { [int]::MaxValue }
            $found = @(for ($i = $rule + 1; $i -lt $lines.Count; $i++) {
                $row = $lines[$i].TrimEnd()
                if ($row.Length -le $at -or $row -notmatch "\W$([regex]::Escape($Name))$") { continue }
                [pscustomobject]@{
                    Name = $row.Substring(0, $at).Trim()
                    Id = $row.Substring($at, [Math]::Min($width, $row.Length - $at)).Trim()
                }
            })
            if ($found.Count -eq 0) { return }
            Write-Host "'$Name' is not installed. To install it:" -ForegroundColor Yellow
            foreach ($p in $found | Select-Object -First 3) { Write-Host "    winget install --id $($p.Id) -e    # $($p.Name)" }
        } catch {
            Write-Host "PowerKit could not search winget: $_" -ForegroundColor DarkGray
        }
    }
}
# <<< PowerKit Command Not Found <<<"#;

/// The profile text with PowerKit's block taken out, and whether one was there.
pub fn without_block(profile: &str) -> (String, bool) {
    let (Some(start), Some(end)) = (profile.find(BEGIN), profile.find(END)) else {
        return (profile.to_string(), false);
    };
    if end < start {
        return (profile.to_string(), false);
    }
    let mut after = &profile[end + END.len()..];
    after = after
        .strip_prefix("\r\n")
        .or_else(|| after.strip_prefix('\n'))
        .unwrap_or(after);
    let before = profile[..start].trim_end_matches([' ', '\t']);
    // The blank line put in front of the block goes with it.
    let before = before
        .strip_suffix("\r\n\r\n")
        .map(|b| format!("{b}\r\n"))
        .or_else(|| before.strip_suffix("\n\n").map(|b| format!("{b}\n")))
        .unwrap_or_else(|| before.to_string());
    (format!("{before}{after}"), true)
}

/// The profile text with PowerKit's block, current version, at the end.
pub fn with_block(profile: &str) -> String {
    let (mut text, _) = without_block(profile);
    // Keep a file's own Unix line ends; anything else gets Windows ones.
    let newline = if text.contains('\n') && !text.contains("\r\n") {
        "\n"
    } else {
        "\r\n"
    };
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push_str(newline);
        }
        text.push_str(newline);
    }
    text.push_str(&BLOCK.replace('\n', newline));
    text.push_str(newline);
    text
}

/// Whether PowerShell runs profile scripts under this execution policy (the output of
/// `Get-ExecutionPolicy`). Restricted runs none and AllSigned would refuse the
/// unsigned block with an error in every new window.
pub fn policy_runs_profiles(policy: &str) -> bool {
    matches!(
        policy.trim().to_ascii_lowercase().as_str(),
        "unrestricted" | "remotesigned" | "bypass"
    )
}

/// What turning the utility on did to each PowerShell.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub added: Vec<String>,
    pub current: Vec<String>,
    /// Shells left alone, with their execution policy.
    pub refused: Vec<(String, String)>,
}

impl Report {
    /// A notification for the user, or `None` when nothing changed.
    pub fn message(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!(
                "{} will suggest winget packages for missing commands in new windows.",
                self.added.join(" and ")
            ));
        }
        for (shell, policy) in &self.refused {
            parts.push(format!(
                "{shell} was left alone: its execution policy ({policy}) does not run profile scripts. \
                 Run `Set-ExecutionPolicy RemoteSigned -Scope CurrentUser` in it, then turn Command Not Found off and on."
            ));
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

#[derive(Default)]
pub struct CommandNotFound {
    running: bool,
    report: Report,
}

impl CommandNotFound {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(windows)]
    fn install(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.report = live::install()?;
        if let Some(message) = self.report.message() {
            ctx.notify("Command Not Found", message);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn install(&mut self, _ctx: &mut Ctx) -> Result<()> {
        bail!("Command Not Found works only on Windows")
    }
}

impl Module for CommandNotFound {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.into(),
            name: "Command Not Found".into(),
            description: "PowerShell suggests the winget package for a command that is not installed.".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            permissions: vec![],
            commands: vec![CommandSpec {
                id: "install".into(),
                title: "Add the suggestions to the PowerShell profiles again".into(),
                description: String::new(),
            }],
            settings: vec![],
        }
    }

    fn start(&mut self, ctx: &mut Ctx) -> Result<()> {
        self.install(ctx)?;
        self.running = true;
        Ok(())
    }

    /// Called both when the utility is turned off and when PowerKit exits; only the
    /// first takes the block out, so suggestions keep working while PowerKit is closed.
    fn stop(&mut self, ctx: &mut Ctx) {
        self.running = false;
        if ctx.config.command_not_found.enabled {
            return;
        }
        #[cfg(windows)]
        match live::uninstall() {
            Ok(removed) if !removed.is_empty() => {
                ctx.notify(
                    "Command Not Found",
                    format!(
                        "Removed from {}; new windows no longer suggest packages.",
                        removed.join(" and ")
                    ),
                );
            }
            Ok(_) => {}
            Err(e) => ctx.notify(
                "Command Not Found",
                format!("Could not remove it from the profiles: {e:#}"),
            ),
        }
    }

    fn config_changed(&mut self, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn on_hotkey(&mut self, action: &str, _ctx: &mut Ctx) -> Result<()> {
        bail!("unknown action {action:?}")
    }

    fn on_event(&mut self, _event: &Event, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }

    fn command(&mut self, id: &str, _args: &Value, ctx: &mut Ctx) -> Result<Value> {
        if !self.running {
            bail!("Command Not Found is turned off");
        }
        match id {
            "install" => self.install(ctx)?,
            _ => bail!("unknown command {id:?}"),
        }
        Ok(self.status())
    }

    fn status(&self) -> Value {
        let mut shells = self.report.added.clone();
        shells.extend(self.report.current.iter().cloned());
        json!({
            "running": self.running,
            "shells": shells,
            "refused": self.report.refused.iter().map(|(s, p)| json!({ "shell": s, "policy": p })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_added_once_and_replaced() {
        let user = "Set-Alias ll ls\r\n";
        let once = with_block(user);
        assert!(once.starts_with("Set-Alias ll ls\r\n\r\n# >>> PowerKit"));
        assert!(once.ends_with("<<<\r\n"));
        assert!(
            !once.replace("\r\n", "").contains('\n'),
            "keeps the file's CRLF line ends"
        );
        assert_eq!(with_block(&once), once);
        let stale = once.replace("Looking for", "Searching for");
        assert_eq!(with_block(&stale), once);
        assert_eq!(with_block("").matches(BEGIN).count(), 1);
        assert!(with_block("x = 1").starts_with("x = 1\r\n\r\n# >>>"));
        assert!(with_block("a\nb\n").starts_with("a\nb\n\n# >>>"));
    }

    #[test]
    fn removing_the_block_restores_the_profile() {
        for user in ["Set-Alias ll ls\r\n", "a\nb\n", "", "# mine\r\n\r\n"] {
            let (back, found) = without_block(&with_block(user));
            assert!(found);
            assert_eq!(back, user);
        }
        let around = format!("first\n{BLOCK}\nlast\n");
        assert_eq!(without_block(&around), ("first\nlast\n".to_string(), true));
        assert_eq!(without_block("nothing here"), ("nothing here".to_string(), false));
        let backwards = format!("{END}\n{BEGIN}\n");
        assert!(!without_block(&backwards).1);
    }

    #[test]
    fn only_policies_that_run_profiles_are_used() {
        assert!(policy_runs_profiles("RemoteSigned\r\n"));
        assert!(policy_runs_profiles("Unrestricted"));
        assert!(policy_runs_profiles("Bypass"));
        assert!(!policy_runs_profiles("Restricted"));
        assert!(!policy_runs_profiles("AllSigned"));
        assert!(!policy_runs_profiles(""));
    }

    #[test]
    fn the_report_says_what_happened() {
        assert_eq!(Report::default().message(), None);
        let r = Report {
            current: vec!["PowerShell 7".into()],
            ..Default::default()
        };
        assert_eq!(r.message(), None);
        let r = Report {
            added: vec!["PowerShell 7".into()],
            current: vec![],
            refused: vec![("Windows PowerShell".into(), "Restricted".into())],
        };
        let m = r.message().unwrap();
        assert!(m.starts_with("PowerShell 7 will suggest"), "{m}");
        assert!(
            m.contains("Windows PowerShell was left alone: its execution policy (Restricted)"),
            "{m}"
        );
    }
}
