use super::clamp;
use serde::{Deserialize, Serialize};

pub const MAX_MACHINES: usize = 4;

/// One mouse and keyboard for up to four PCs side by side on the same network. Off by
/// default because it listens on a network port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseWithoutBorders {
    pub enabled: bool,
    /// Shared by every machine; empty until the module first starts and makes one.
    pub security_key: String,
    /// The PCs from left to right, this one included: a computer name, optionally
    /// followed by a space and the address to reach it at.
    pub machines: Vec<String>,
    pub port: u16,
    pub share_clipboard: bool,
    /// Ctrl+Alt+F1 to F4 go to the first to fourth machine.
    pub switch_keys: bool,
}

impl Default for MouseWithoutBorders {
    fn default() -> Self {
        Self {
            enabled: false,
            security_key: String::new(),
            machines: Vec::new(),
            port: 15110,
            share_clipboard: true,
            switch_keys: true,
        }
    }
}

impl MouseWithoutBorders {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        clamp(&mut self.port, 1024, 65535, "Mouse Without Borders port", w);
        self.security_key = self.security_key.trim().to_string();
        let mut seen = std::collections::HashSet::new();
        self.machines.retain_mut(|m| {
            *m = m.split_whitespace().collect::<Vec<_>>().join(" ");
            let name = m.split(' ').next().unwrap_or_default().to_lowercase();
            !name.is_empty() && seen.insert(name)
        });
        if self.machines.len() > MAX_MACHINES {
            w.push(format!(
                "Mouse Without Borders: only the first {MAX_MACHINES} machines are used"
            ));
            self.machines.truncate(MAX_MACHINES);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machines_are_tidied_and_limited() {
        let mut c = MouseWithoutBorders {
            machines: vec![
                "  DESK ".into(),
                "".into(),
                "laptop   192.168.1.20".into(),
                "desk".into(),
                "a".into(),
                "b".into(),
                "c".into(),
            ],
            port: 80,
            ..Default::default()
        };
        let mut w = Vec::new();
        c.sanitize(&mut w);
        assert_eq!(c.machines, ["DESK", "laptop 192.168.1.20", "a", "b"]);
        assert_eq!(c.port, 1024);
        assert_eq!(w.len(), 2);
    }
}
