use super::fix_shortcut;
use powerkit_core::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    #[default]
    Normal,
    Maximized,
    Minimized,
}

/// One window of a workspace and the program that opens it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceApp {
    /// Full path of the program.
    pub path: String,
    /// Command-line arguments passed to it as written. Capturing leaves them empty.
    pub args: String,
    /// The window's title when it was captured, to tell the windows apart.
    pub title: String,
    /// The monitor it was on (`\\.\DISPLAY2`) and that monitor's bounds then, so the
    /// window can follow the monitor if it moved or go to another one if it is gone.
    pub monitor: String,
    pub monitor_rect: Rect,
    /// Where the window was, or where it returns to when restored if it was
    /// maximized or minimized.
    pub rect: Rect,
    pub state: WindowState,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub name: String,
    pub apps: Vec<WorkspaceApp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspaces {
    pub enabled: bool,
    /// Opens the editor.
    pub shortcut: String,
    /// Move windows of the workspace's programs that are already open instead of
    /// starting the programs again.
    pub move_existing: bool,
    pub workspaces: Vec<Workspace>,
}

impl Default for Workspaces {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Ctrl+Backtick".into(),
            move_existing: true,
            workspaces: Vec::new(),
        }
    }
}

impl Workspaces {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        let d = Workspaces::default();
        fix_shortcut(&mut self.shortcut, &d.shortcut, "Workspaces", w);
        let before = self.workspaces.len();
        let mut seen: Vec<String> = Vec::new();
        self.workspaces.retain_mut(|ws| {
            ws.name = ws.name.trim().to_string();
            let key = ws.name.to_lowercase();
            if ws.name.is_empty() || seen.contains(&key) {
                return false;
            }
            seen.push(key);
            ws.apps.retain(|a| !a.path.trim().is_empty());
            true
        });
        if self.workspaces.len() < before {
            w.push(format!(
                "workspaces.workspaces: removed {} without a name or with a name used before",
                before - self.workspaces.len()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, paths: &[&str]) -> Workspace {
        Workspace {
            name: name.into(),
            apps: paths
                .iter()
                .map(|p| WorkspaceApp {
                    path: (*p).into(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn unnamed_and_repeated_workspaces_and_pathless_apps_are_dropped() {
        let mut s = Workspaces {
            workspaces: vec![
                named(" Code ", &["C:\\a.exe", " "]),
                named("", &["C:\\b.exe"]),
                named("code", &["C:\\c.exe"]),
                named("Mail", &[]),
            ],
            ..Default::default()
        };
        let mut w = Vec::new();
        s.sanitize(&mut w);
        let names: Vec<_> = s.workspaces.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Code", "Mail"]);
        assert_eq!(s.workspaces[0].apps.len(), 1);
        assert_eq!(w.len(), 1);
    }
}
