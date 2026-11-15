//! The settings window's contents: every configuration section as a flat list of
//! values, edited as text and saved through `config.set` like any IPC client would.

use crate::Host;
use powerkit_core::SettingsEvent;
use powerkit_platform::settings_window::SettingsWindow;
use serde_json::{json, Map, Value};

/// A setting's place in its section: the object keys leading to it.
pub type Path = Vec<String>;

/// The leaves of `value` in order. Objects are opened up; anything else (including
/// lists of objects such as remappings) is one value.
pub fn flatten(value: &Value) -> Vec<(Path, Value)> {
    fn walk(v: &Value, path: &mut Path, out: &mut Vec<(Path, Value)>) {
        match v {
            Value::Object(map) if !map.is_empty() => {
                for (k, child) in map {
                    path.push(k.clone());
                    walk(child, path, out);
                    path.pop();
                }
            }
            _ => out.push((path.clone(), v.clone())),
        }
    }
    let mut out = Vec::new();
    walk(value, &mut Vec::new(), &mut out);
    out
}

fn is_string_list(v: &Value) -> bool {
    v.as_array().is_some_and(|a| a.iter().all(Value::is_string))
}

/// How a value reads in the list and in the edit box.
pub fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) if is_string_list(v) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        _ => v.to_string(),
    }
}

/// The value typed for a setting, read as the same kind of value it had before.
pub fn parse(original: &Value, text: &str) -> Result<Value, String> {
    let t = text.trim();
    match original {
        Value::Bool(_) => match t.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(Value::Bool(true)),
            "false" | "off" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!("{t:?} is not true or false")),
        },
        Value::Number(n) if n.is_f64() => t
            .parse::<f64>()
            .ok()
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            .ok_or_else(|| format!("{t:?} is not a number")),
        Value::Number(_) => t
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("{t:?} is not a whole number")),
        Value::String(_) => Ok(Value::String(text.to_string())),
        Value::Array(_) if is_string_list(original) && !t.starts_with('[') => Ok(Value::Array(
            t.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.into()))
                .collect(),
        )),
        _ => serde_json::from_str(t).map_err(|e| format!("not valid JSON: {e}")),
    }
}

/// Replace the value at `path` inside `root`, creating objects on the way.
pub fn set_path(root: &mut Value, path: &[String], v: Value) {
    let Some((last, parents)) = path.split_last() else {
        *root = v;
        return;
    };
    let mut cur = root;
    for key in parents {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        cur = cur
            .as_object_mut()
            .expect("made an object above")
            .entry(key.clone())
            .or_insert(Value::Null);
    }
    if !cur.is_object() {
        *cur = Value::Object(Map::new());
    }
    cur.as_object_mut()
        .expect("made an object above")
        .insert(last.clone(), v);
}

/// "find_my_mouse / shortcut" from `["find_my_mouse", "shortcut"]`, in words.
pub fn label(path: &[String]) -> String {
    path.iter().map(|k| k.replace('_', " ")).collect::<Vec<_>>().join(" / ")
}

#[derive(Default)]
pub struct SettingsUi {
    window: Option<SettingsWindow>,
    /// `(config key, display name)` for each section.
    sections: Vec<(String, String)>,
    current: usize,
    rows: Vec<(Path, Value)>,
}

impl SettingsUi {
    /// Show the window at the section with config key `page`, or where it was.
    pub fn open(&mut self, host: &mut Host, page: Option<&str>) -> Result<(), String> {
        if self.window.is_none() {
            self.window = Some(SettingsWindow::new().map_err(|e| format!("could not open settings: {e}"))?);
        }
        self.sections = vec![("general".into(), "General".into())];
        if let Some(modules) = host.status()["modules"].as_array() {
            for m in modules {
                if let (Some(id), Some(name)) = (m["id"].as_str(), m["name"].as_str()) {
                    self.sections.push((id.into(), name.into()));
                }
            }
        }
        if let Some(i) = page.and_then(|p| self.sections.iter().position(|(id, _)| id == p)) {
            self.current = i;
        }
        self.current = self.current.min(self.sections.len() - 1);
        let names: Vec<String> = self.sections.iter().map(|(_, n)| n.clone()).collect();
        let w = self.window.as_mut().expect("created above");
        w.set_sections(&names, self.current);
        w.set_status("Select a setting, type its new value below and press Enter.");
        self.refresh(host, None);
        self.window.as_mut().expect("created above").show();
        Ok(())
    }

    fn refresh(&mut self, host: &Host, selected: Option<usize>) {
        let Some(w) = self.window.as_mut() else { return };
        let Some((key, _)) = self.sections.get(self.current) else {
            return;
        };
        let config = serde_json::to_value(host.config()).unwrap_or(Value::Null);
        self.rows = flatten(&config[key.as_str()]);
        let shown: Vec<(String, String)> = self.rows.iter().map(|(p, v)| (label(p), display(v))).collect();
        w.set_rows(&shown, selected);
    }

    pub fn on_event(&mut self, event: SettingsEvent, host: &mut Host) {
        match event {
            SettingsEvent::Section(i) if i < self.sections.len() => {
                self.current = i;
                self.refresh(host, None);
            }
            SettingsEvent::Section(_) => {}
            SettingsEvent::Toggle(row) => match self.rows.get(row) {
                Some((_, Value::Bool(b))) => {
                    let text = (!b).to_string();
                    self.save(host, row, &text);
                }
                Some(_) => {
                    if let Some(w) = self.window.as_mut() {
                        w.focus_edit();
                    }
                }
                None => {}
            },
            SettingsEvent::Edit { row, text } => self.save(host, row, &text),
        }
    }

    fn save(&mut self, host: &mut Host, row: usize, text: &str) {
        let Some((path, original)) = self.rows.get(row).cloned() else {
            return;
        };
        let Some((key, _)) = self.sections.get(self.current).cloned() else {
            return;
        };
        let status = match parse(&original, text) {
            Err(e) => format!("Not saved: {e}"),
            Ok(v) => {
                let mut config = serde_json::to_value(host.config()).unwrap_or(Value::Null);
                let mut full = vec![key];
                full.extend(path.iter().cloned());
                set_path(&mut config, &full, v);
                match host.handle_request("config.set", &json!({ "config": config })) {
                    Ok(r) => {
                        let warnings: Vec<&str> = r["warnings"]
                            .as_array()
                            .map(|a| a.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        if warnings.is_empty() {
                            format!("Saved {}.", label(&path))
                        } else {
                            warnings.join(" ")
                        }
                    }
                    Err(e) => format!("Not saved: {e}"),
                }
            }
        };
        self.refresh(host, Some(row));
        if let Some(w) = self.window.as_mut() {
            w.set_status(&status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_opens_objects_only() {
        let v = json!({"a": {"b": 1, "c": [ {"x": 1} ]}, "d": {}, "e": "s"});
        let rows = flatten(&v);
        let paths: Vec<String> = rows.iter().map(|(p, _)| p.join(".")).collect();
        assert_eq!(paths, ["a.b", "a.c", "d", "e"]);
        assert_eq!(label(&rows[0].0), "a / b");
    }

    #[test]
    fn values_round_trip_through_text() {
        let apps = json!(["a.exe", "b.exe"]);
        assert_eq!(display(&apps), "a.exe, b.exe");
        assert_eq!(parse(&apps, " c.exe ,, d.exe").unwrap(), json!(["c.exe", "d.exe"]));
        assert_eq!(parse(&json!([]), "x.exe").unwrap(), json!(["x.exe"]));
        assert_eq!(parse(&json!([]), r#"[{"k": 1}]"#).unwrap(), json!([{"k": 1}]));
        assert_eq!(parse(&json!(true), "Off").unwrap(), json!(false));
        assert!(parse(&json!(true), "maybe").is_err());
        assert_eq!(parse(&json!(5), " 12 ").unwrap(), json!(12));
        assert!(parse(&json!(5), "1.5").is_err());
        assert_eq!(parse(&json!(0.5), "1.5").unwrap(), json!(1.5));
        assert_eq!(parse(&json!("Win+T"), "Alt+T").unwrap(), json!("Alt+T"));
        assert_eq!(display(&json!({"k": 1})), r#"{"k":1}"#);
    }

    #[test]
    fn set_path_replaces_nested_values() {
        let mut v = json!({"m": {"inner": {"x": 1}, "y": 2}});
        set_path(&mut v, &["m".into(), "inner".into(), "x".into()], json!(5));
        set_path(&mut v, &["m".into(), "new".into()], json!(true));
        assert_eq!(v, json!({"m": {"inner": {"x": 5}, "y": 2, "new": true}}));
    }
}
