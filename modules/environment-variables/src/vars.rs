//! Editing one scope's environment variables and working out the registry writes
//! that turn the variables as loaded into the variables as edited. Names are
//! compared without regard to case, as Windows does.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Windows' limit on one variable's length, in UTF-16 units.
const MAX_LEN: usize = 32_767;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Var {
    pub name: String,
    pub value: String,
    /// Stored so that `%OTHER%` references expand when a program reads it.
    pub expand: bool,
}

/// One registry write: set `name`, or delete it when `value` is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub name: String,
    pub value: Option<String>,
    #[serde(default)]
    pub expand: bool,
}

fn key(name: &str) -> String {
    name.to_uppercase()
}

/// Check what the user typed; returns the name without surrounding spaces.
pub fn validate(name: &str, value: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name.".into());
    }
    if name.contains('=') {
        return Err("A name cannot contain '='.".into());
    }
    if name.contains('\0') || value.contains('\0') {
        return Err("Names and values cannot contain NUL characters.".into());
    }
    if value.is_empty() {
        return Err("Enter a value; to remove a variable, choose Delete.".into());
    }
    if name.encode_utf16().count() + value.encode_utf16().count() + 2 > MAX_LEN {
        return Err(format!("Too long; Windows allows {MAX_LEN} characters per variable."));
    }
    Ok(name.to_string())
}

/// The variables of one scope as loaded and as edited, sorted by name.
#[derive(Debug, Clone, Default)]
pub struct Table {
    original: Vec<Var>,
    vars: Vec<Var>,
}

impl Table {
    pub fn new(mut vars: Vec<Var>) -> Table {
        vars.sort_by_key(|v| key(&v.name));
        Table {
            original: vars.clone(),
            vars,
        }
    }

    pub fn vars(&self) -> &[Var] {
        &self.vars
    }

    fn find(vars: &[Var], name: &str) -> Option<usize> {
        let k = key(name);
        vars.iter().position(|v| key(&v.name) == k)
    }

    /// `[name, value, note]` per variable; the note says what is not saved yet.
    pub fn rows(&self) -> Vec<[String; 3]> {
        self.vars
            .iter()
            .map(|v| {
                let note = match Self::find(&self.original, &v.name) {
                    None => "New",
                    Some(i) if self.original[i] != *v => "Changed",
                    Some(_) => "",
                };
                [v.name.clone(), v.value.clone(), note.into()]
            })
            .collect()
    }

    /// Put `var` in name order; returns its row.
    fn insert(&mut self, var: Var) -> usize {
        let k = key(&var.name);
        let at = self.vars.partition_point(|v| key(&v.name) < k);
        self.vars.insert(at, var);
        at
    }

    pub fn add(&mut self, name: &str, value: &str) -> Result<usize, String> {
        let name = validate(name, value)?;
        if Self::find(&self.vars, &name).is_some() {
            return Err(format!("{name} already exists; select it and choose Update."));
        }
        Ok(self.insert(Var {
            name,
            value: value.into(),
            expand: value.contains('%'),
        }))
    }

    /// Change the variable on `row`, which may also be renamed.
    pub fn update(&mut self, row: usize, name: &str, value: &str) -> Result<usize, String> {
        let name = validate(name, value)?;
        let old = self.vars.get(row).ok_or("That variable is no longer listed.")?;
        if Self::find(&self.vars, &name).is_some_and(|i| i != row) {
            return Err(format!("{name} already exists."));
        }
        let expand = old.expand || value.contains('%');
        self.vars.remove(row);
        Ok(self.insert(Var {
            name,
            value: value.into(),
            expand,
        }))
    }

    /// Returns the row to select afterwards.
    pub fn delete(&mut self, row: usize) -> Result<Option<usize>, String> {
        if row >= self.vars.len() {
            return Err("That variable is no longer listed.".into());
        }
        self.vars.remove(row);
        Ok(row
            .min(self.vars.len())
            .checked_sub(1)
            .or(if self.vars.is_empty() { None } else { Some(0) }))
    }

    /// The writes that save the edits: deletions first, so that a variable renamed
    /// only in case is deleted before it is written under its new spelling.
    pub fn changes(&self) -> Vec<Change> {
        let mut out = Vec::new();
        for o in &self.original {
            match Self::find(&self.vars, &o.name) {
                Some(i) if self.vars[i].name == o.name => {}
                _ => out.push(Change {
                    name: o.name.clone(),
                    value: None,
                    expand: false,
                }),
            }
        }
        for v in &self.vars {
            if !self.original.contains(v) {
                out.push(Change {
                    name: v.name.clone(),
                    value: Some(v.value.clone()),
                    expand: v.expand,
                });
            }
        }
        out
    }

    pub fn is_dirty(&self) -> bool {
        self.original != self.vars
    }
}

/// Exit codes of `powerkit --write-env`, which saves system variables elevated.
pub mod exit {
    pub const OK: u8 = 0;
    pub const USAGE: u8 = 2;
    pub const BAD_SOURCE: u8 = 3;
    pub const WRITE_FAILED: u8 = 4;

    pub fn message(code: u32) -> String {
        match code {
            2 | 3 => "Not saved: the changes could not be handed over.".into(),
            4 => "Not saved: Windows refused some of the changes. Choose Reload to see what was written.".into(),
            c => format!("Not saved (error {c})."),
        }
    }
}

/// Read the changes `powerkit --write-env` was handed.
pub fn read_changes(path: &Path) -> Result<Vec<Change>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, value: &str, expand: bool) -> Var {
        Var {
            name: name.into(),
            value: value.into(),
            expand,
        }
    }

    fn set(name: &str, value: &str, expand: bool) -> Change {
        Change {
            name: name.into(),
            value: Some(value.into()),
            expand,
        }
    }

    fn delete(name: &str) -> Change {
        Change {
            name: name.into(),
            value: None,
            expand: false,
        }
    }

    #[test]
    fn edits_become_the_writes_that_save_them() {
        let mut t = Table::new(vec![
            var("TEMP", "%USERPROFILE%\\Temp", true),
            var("Path", "C:\\bin", true),
            var("EDITOR", "vim", false),
        ]);
        assert_eq!(t.rows()[0], ["EDITOR", "vim", ""]);
        assert!(!t.is_dirty() && t.changes().is_empty());

        assert_eq!(t.add("GOPATH", "%USERPROFILE%\\go"), Ok(1));
        assert_eq!(t.rows()[1][2], "New");
        assert_eq!(t.update(0, "EDITOR", "code"), Ok(0));
        assert_eq!(t.rows()[0][2], "Changed");
        // Renamed in case only, and kept expandable though it no longer refers to anything.
        assert_eq!(t.update(2, "PATH", "C:\\tools"), Ok(2));
        assert_eq!(t.delete(3), Ok(Some(2)));

        assert_eq!(
            t.changes(),
            [
                delete("Path"),
                delete("TEMP"),
                set("EDITOR", "code", false),
                set("GOPATH", "%USERPROFILE%\\go", true),
                set("PATH", "C:\\tools", true),
            ]
        );
        assert!(t.is_dirty());
    }

    #[test]
    fn bad_input_changes_nothing() {
        let mut t = Table::new(vec![var("A", "1", false), var("B", "2", false)]);
        assert!(t.add("a", "x").is_err(), "names ignore case");
        assert!(t.add("X=Y", "x").is_err());
        assert!(t.add("  ", "x").is_err());
        assert!(t.add("X", "").is_err());
        assert!(t.update(0, "b", "x").is_err(), "cannot rename onto another variable");
        assert!(t.update(9, "C", "x").is_err());
        assert!(t.delete(9).is_err());
        assert!(t.add("X", &"v".repeat(MAX_LEN)).is_err());
        assert!(!t.is_dirty());
        assert_eq!(t.add("  C ", "3"), Ok(2), "names are trimmed");
        assert_eq!(t.vars()[2].name, "C");
    }

    #[test]
    fn deleting_selects_a_neighbour() {
        let mut t = Table::new(vec![var("A", "1", false), var("B", "2", false)]);
        assert_eq!(t.delete(0), Ok(Some(0)));
        assert_eq!(t.delete(0), Ok(None));
    }

    #[test]
    fn changes_survive_the_handover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("env.json");
        let changes = vec![delete("A"), set("B", "%X%", true)];
        std::fs::write(&path, serde_json::to_string(&changes).unwrap()).unwrap();
        assert_eq!(read_changes(&path), Ok(changes));
        std::fs::write(&path, "nope").unwrap();
        assert!(read_changes(&path).is_err());
    }
}
