use super::fix_apps;
use serde::{Deserialize, Serialize};

/// The key held while dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrabModifier {
    /// Few applications use Windows+drag, so this is the safe default.
    #[default]
    Win,
    /// Alt+click is used by some editors (multiple cursors, column selection).
    Alt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrabAndMove {
    pub enabled: bool,
    pub modifier: GrabModifier,
    /// Modifier + right-drag resizes from the nearest edge or corner.
    pub resize: bool,
    /// Bring the window to the front when it is grabbed.
    pub activate: bool,
    /// Executable names (e.g. "blender.exe") whose windows are left alone.
    pub excluded_apps: Vec<String>,
}

impl Default for GrabAndMove {
    fn default() -> Self {
        Self {
            enabled: true,
            modifier: GrabModifier::Win,
            resize: true,
            activate: true,
            excluded_apps: Vec::new(),
        }
    }
}

impl GrabAndMove {
    pub(crate) fn sanitize(&mut self, _w: &mut [String]) {
        fix_apps(&mut self.excluded_apps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_normalizes_apps() {
        let mut c = GrabAndMove {
            excluded_apps: vec![" Blender.exe".into(), "blender.exe".into(), "".into()],
            ..GrabAndMove::default()
        };
        c.sanitize(&mut []);
        assert_eq!(c.excluded_apps, ["blender.exe"]);
        let v: GrabAndMove = serde_json::from_str(r#"{"modifier":"alt"}"#).unwrap();
        assert_eq!(v.modifier, GrabModifier::Alt);
    }
}
