//! Reading .reg files: the text encodings regedit writes, and the keys and values a
//! file would create, change or delete. Problems are reported per line and the rest
//! of the file is still read, so a preview shows as much as it can.

/// Root keys a .reg file may name, as regedit spells them.
const ROOTS: [&str; 5] = [
    "HKEY_LOCAL_MACHINE",
    "HKEY_CURRENT_USER",
    "HKEY_CLASSES_ROOT",
    "HKEY_USERS",
    "HKEY_CURRENT_CONFIG",
];
const HEADER_5: &str = "Windows Registry Editor Version 5.00";
const HEADER_4: &str = "REGEDIT4";
/// Shown bytes of binary data before the rest is summarised.
const SHOWN_BYTES: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    String(String),
    Expand(String),
    Multi(Vec<String>),
    Dword(u32),
    Qword(u64),
    Binary(Vec<u8>),
    /// Any other type, by its number.
    Other(u32, Vec<u8>),
}

impl Data {
    pub fn kind(&self) -> String {
        match self {
            Data::String(_) => "REG_SZ".into(),
            Data::Expand(_) => "REG_EXPAND_SZ".into(),
            Data::Multi(_) => "REG_MULTI_SZ".into(),
            Data::Dword(_) => "REG_DWORD".into(),
            Data::Qword(_) => "REG_QWORD".into(),
            Data::Binary(_) => "REG_BINARY".into(),
            Data::Other(0, _) => "REG_NONE".into(),
            Data::Other(k, _) => format!("Type {k}"),
        }
    }

    /// The data as Registry Editor shows it.
    pub fn display(&self) -> String {
        let bytes = |b: &[u8]| {
            let mut s: Vec<String> = b.iter().take(SHOWN_BYTES).map(|x| format!("{x:02x}")).collect();
            if b.len() > SHOWN_BYTES {
                s.push(format!("… ({} bytes)", b.len()));
            }
            s.join(" ")
        };
        match self {
            Data::String(s) | Data::Expand(s) => s.clone(),
            Data::Multi(v) => v.join(" · "),
            Data::Dword(d) => format!("0x{d:08x} ({d})"),
            Data::Qword(q) => format!("0x{q:016x} ({q})"),
            Data::Binary(b) | Data::Other(_, b) => bytes(b),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `[path]` creates the key; `[-path]` deletes it with everything under it.
    Key { path: String, delete: bool, line: usize },
    /// A value of `key`; `name` is empty for the default value, `data` is `None`
    /// when the value is deleted.
    Value {
        key: String,
        name: String,
        data: Option<Data>,
        line: usize,
    },
}

impl Item {
    pub fn line(&self) -> usize {
        match self {
            Item::Key { line, .. } | Item::Value { line, .. } => *line,
        }
    }

    /// The key this item is or belongs to.
    pub fn key(&self) -> &str {
        match self {
            Item::Key { path, .. } => path,
            Item::Value { key, .. } => key,
        }
    }
}

/// What a file holds. Lines are 0-based.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub items: Vec<Item>,
    pub errors: Vec<(usize, String)>,
}

impl Parsed {
    /// Whether writing the file touches more than the current user's own keys.
    pub fn needs_admin(&self) -> bool {
        self.items
            .iter()
            .any(|i| root(i.key()).is_none_or(|r| r != "HKEY_CURRENT_USER"))
    }

    /// `[key or value, type, data]` and the line of each item, for the window.
    pub fn listing(&self) -> Vec<([String; 3], usize)> {
        self.items
            .iter()
            .map(|item| {
                let cols = match item {
                    Item::Key {
                        path, delete: false, ..
                    } => [path.clone(), "Key".into(), String::new()],
                    Item::Key { path, delete: true, .. } => {
                        [path.clone(), "Key".into(), "Deleted, with everything under it".into()]
                    }
                    Item::Value { name, data, .. } => {
                        let name = if name.is_empty() { "(Default)" } else { name };
                        let (kind, shown) = match data {
                            Some(d) => (d.kind(), d.display()),
                            None => (String::new(), "Deleted".into()),
                        };
                        [format!("    {name}"), kind, shown]
                    }
                };
                (cols, item.line())
            })
            .collect()
    }

    /// The first problem, as "Line N: …", and how many there are.
    pub fn summary(&self) -> String {
        let keys = self.items.iter().filter(|i| matches!(i, Item::Key { .. })).count();
        let values = self.items.len() - keys;
        match self.errors.first() {
            None => format!("{keys} keys, {values} values."),
            Some((line, e)) if self.errors.len() == 1 => format!("Line {}: {e}", line + 1),
            Some((line, e)) => format!("Line {}: {e} ({} problems in all)", line + 1, self.errors.len()),
        }
    }
}

fn root(path: &str) -> Option<&'static str> {
    let first = path.split('\\').next().unwrap_or("");
    ROOTS.into_iter().find(|r| r.eq_ignore_ascii_case(first))
}

/// Physical lines joined where a line ends in `\`, each with its first line number.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut open = false;
    for (n, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let piece = if open { line.trim_start() } else { line };
        let (piece, more) = match piece.trim_end().strip_suffix('\\') {
            Some(p) => (p, true),
            None => (piece, false),
        };
        match out.last_mut() {
            Some((_, text)) if open => text.push_str(piece),
            _ => out.push((n, piece.to_string())),
        }
        open = more;
    }
    out
}

/// A quoted string starting at `s[0] == '"'`; returns it unescaped and the rest.
fn quoted(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = s.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => match chars.next() {
                Some((_, e @ ('\\' | '"'))) => out.push(e),
                Some((_, e)) => {
                    out.push('\\');
                    out.push(e);
                }
                None => return None,
            },
            c => out.push(c),
        }
    }
    None
}

fn hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(',')
        .map(|b| {
            let b = b.trim();
            if b.is_empty() || b.len() > 2 {
                return Err(format!("{b:?} is not a hex byte."));
            }
            u8::from_str_radix(b, 16).map_err(|_| format!("{b:?} is not a hex byte."))
        })
        .collect()
}

fn text_of(bytes: &[u8], unicode: bool) -> Vec<String> {
    let text = if unicode {
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    };
    text.split('\0').map(str::to_string).collect()
}

fn typed(kind: u32, bytes: Vec<u8>, unicode: bool) -> Data {
    match kind {
        1 | 2 => {
            let s = text_of(&bytes, unicode).into_iter().next().unwrap_or_default();
            if kind == 1 {
                Data::String(s)
            } else {
                Data::Expand(s)
            }
        }
        7 => {
            let mut v = text_of(&bytes, unicode);
            while v.last().is_some_and(String::is_empty) {
                v.pop();
            }
            Data::Multi(v)
        }
        3 => Data::Binary(bytes),
        4 if bytes.len() == 4 => Data::Dword(u32::from_le_bytes(bytes.try_into().expect("length checked"))),
        11 if bytes.len() == 8 => Data::Qword(u64::from_le_bytes(bytes.try_into().expect("length checked"))),
        k => Data::Other(k, bytes),
    }
}

/// The data after `=`; `None` deletes the value.
fn data(s: &str, unicode: bool) -> Result<Option<Data>, String> {
    let s = s.trim();
    if s == "-" {
        return Ok(None);
    }
    if s.starts_with('"') {
        let (text, rest) = quoted(s).ok_or("The closing quote is missing.")?;
        if !rest.trim().is_empty() {
            return Err(format!("Unexpected {:?} after the closing quote.", rest.trim()));
        }
        return Ok(Some(Data::String(text)));
    }
    let lower = s.to_ascii_lowercase();
    if let Some(d) = lower.strip_prefix("dword:") {
        let d = d.trim();
        if d.is_empty() || d.len() > 8 {
            return Err("A dword needs 1 to 8 hex digits.".into());
        }
        return u32::from_str_radix(d, 16)
            .map(|v| Some(Data::Dword(v)))
            .map_err(|_| format!("{d:?} is not a hex number."));
    }
    if let Some(b) = lower.strip_prefix("hex:") {
        return Ok(Some(Data::Binary(hex_bytes(b)?)));
    }
    if let Some(rest) = lower.strip_prefix("hex(") {
        let (kind, bytes) = rest.split_once("):").ok_or("Expected hex(type):bytes.")?;
        let kind = u32::from_str_radix(kind.trim(), 16).map_err(|_| format!("{kind:?} is not a value type."))?;
        return Ok(Some(typed(kind, hex_bytes(bytes)?, unicode)));
    }
    Err(format!("{s:?} is not value data."))
}

pub fn parse(text: &str) -> Parsed {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parsed::default();
    let mut unicode = true;
    let mut header = false;
    let mut key: Option<String> = None;
    for (n, line) in logical_lines(text) {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') {
            continue;
        }
        if !header {
            header = true;
            if t == HEADER_5 {
                continue;
            }
            if t == HEADER_4 {
                unicode = false;
                continue;
            }
            p.errors.push((n, format!("The first line should be \"{HEADER_5}\".")));
        }
        if let Some(inner) = t.strip_prefix('[') {
            let Some(inner) = inner.strip_suffix(']') else {
                p.errors.push((n, "The closing ] is missing.".into()));
                key = None;
                continue;
            };
            let (path, delete) = match inner.strip_prefix('-') {
                Some(path) => (path, true),
                None => (inner, false),
            };
            if root(path).is_none() {
                p.errors.push((
                    n,
                    format!("{path:?} does not start with a root key such as HKEY_CURRENT_USER."),
                ));
                key = None;
                continue;
            }
            p.items.push(Item::Key {
                path: path.to_string(),
                delete,
                line: n,
            });
            // Values listed under a deleted key have nowhere to go.
            key = (!delete).then(|| path.to_string());
            continue;
        }
        let (name, rest) = if let Some(rest) = t.strip_prefix('@') {
            (String::new(), rest)
        } else if t.starts_with('"') {
            match quoted(t) {
                Some(q) => q,
                None => {
                    p.errors.push((n, "The value name's closing quote is missing.".into()));
                    continue;
                }
            }
        } else {
            p.errors.push((n, "Expected [key], \"name\"=data or @=data.".into()));
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            p.errors.push((n, "Expected = after the value name.".into()));
            continue;
        };
        let Some(k) = &key else {
            p.errors.push((n, "This value is not under a key.".into()));
            continue;
        };
        match data(rest, unicode) {
            Ok(data) => p.items.push(Item::Value {
                key: k.clone(),
                name,
                data,
                line: n,
            }),
            Err(e) => p.errors.push((n, e)),
        }
    }
    if !header {
        p.errors.push((0, "The file is empty.".into()));
    }
    p
}

/// How a .reg file's text is stored, kept so saving writes it back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// What regedit writes: UTF-16 little-endian with a byte order mark.
    #[default]
    Utf16,
    Utf8Bom,
    Utf8,
    /// One byte per character, as REGEDIT4 files are; read as Latin-1.
    Ansi,
}

pub fn decode(bytes: &[u8]) -> (String, Encoding) {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        return (String::from_utf16_lossy(&units), Encoding::Utf16);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), Encoding::Utf8Bom);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Encoding::Utf8),
        Err(_) => (bytes.iter().map(|&b| b as char).collect(), Encoding::Ansi),
    }
}

/// The bytes to save; an error when `encoding` cannot hold some character.
pub fn encode(text: &str, encoding: Encoding) -> Result<Vec<u8>, String> {
    Ok(match encoding {
        Encoding::Utf16 => [0xFF, 0xFE]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect(),
        Encoding::Utf8Bom => [0xEF, 0xBB, 0xBF].into_iter().chain(text.bytes()).collect(),
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Ansi => text
            .chars()
            .map(|c| u8::try_from(u32::from(c)).map_err(|_| format!("{c:?} does not fit the file's 8-bit encoding.")))
            .collect::<Result<_, _>>()?,
    })
}

/// Line breaks as CR LF, which the text box needs.
pub fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "Windows Registry Editor Version 5.00\r\n\
        \r\n\
        ; a comment\r\n\
        [HKEY_CURRENT_USER\\Software\\Demo]\r\n\
        @=\"default\"\r\n\
        \"Path\"=\"C:\\\\Tools \\\"x\\\"\"\r\n\
        \"Count\"=dword:0000002a\r\n\
        \"Big\"=hex(b):01,00,00,00,00,00,00,00\r\n\
        \"Exp\"=hex(2):25,00,54,00,25,00,00,00\r\n\
        \"Multi\"=hex(7):61,00,00,00,62,00,00,00,\\\r\n  00,00\r\n\
        \"Bin\"=hex:de,ad\r\n\
        \"Gone\"=-\r\n\
        \r\n\
        [-HKEY_LOCAL_MACHINE\\Software\\Old]\r\n";

    fn value(p: &Parsed, name: &str) -> Option<Data> {
        p.items.iter().find_map(|i| match i {
            Item::Value { name: n, data, .. } if n == name => Some(data.clone()),
            _ => None,
        })?
    }

    #[test]
    fn reads_every_kind_of_line() {
        let p = parse(FILE);
        assert_eq!(p.errors, []);
        assert_eq!(p.items.len(), 10);
        assert_eq!(
            p.items[0],
            Item::Key {
                path: "HKEY_CURRENT_USER\\Software\\Demo".into(),
                delete: false,
                line: 3
            }
        );
        assert_eq!(value(&p, ""), Some(Data::String("default".into())));
        assert_eq!(value(&p, "Path"), Some(Data::String("C:\\Tools \"x\"".into())));
        assert_eq!(value(&p, "Count"), Some(Data::Dword(42)));
        assert_eq!(value(&p, "Big"), Some(Data::Qword(1)));
        assert_eq!(value(&p, "Exp"), Some(Data::Expand("%T%".into())));
        assert_eq!(value(&p, "Multi"), Some(Data::Multi(vec!["a".into(), "b".into()])));
        assert_eq!(value(&p, "Bin"), Some(Data::Binary(vec![0xde, 0xad])));
        assert_eq!(value(&p, "Gone"), None);
        assert_eq!(p.items[6].line(), 9, "a continued value keeps its first line");
        assert_eq!(p.items[8].line(), 12);
        assert!(matches!(
            &p.items[9],
            Item::Key {
                delete: true,
                line: 14,
                ..
            }
        ));
        assert!(p.needs_admin(), "HKLM needs administrator rights");
        assert_eq!(p.summary(), "2 keys, 8 values.");

        let rows = p.listing();
        assert_eq!(
            rows[1].0,
            ["    (Default)".to_string(), "REG_SZ".into(), "default".into()]
        );
        assert_eq!(rows[3].0[2], "0x0000002a (42)");
        assert_eq!(rows[9].0[2], "Deleted, with everything under it");
    }

    #[test]
    fn problems_are_reported_by_line_and_the_rest_is_read() {
        let p = parse(
            "REGEDIT4\n\"orphan\"=\"x\"\n[HKEY_NOPE\\X]\n[HKEY_CURRENT_USER\\A]\n\"a\"=dword:xyz\n\"b\"=\"unterminated\n\"c\"=hex(2):41,42,00\n[HKEY_CURRENT_USER\\B\n",
        );
        let lines: Vec<usize> = p.errors.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, [1, 2, 4, 5, 7]);
        assert_eq!(
            value(&p, "c"),
            Some(Data::Expand("AB".into())),
            "REGEDIT4 text is 8-bit"
        );
        assert!(!p.needs_admin());
        assert!(p
            .summary()
            .starts_with("Line 2: This value is not under a key. (5 problems in all)"));
        assert_eq!(parse("").errors, [(0, "The file is empty.".to_string())]);
        assert_eq!(parse("hello\n").errors.len(), 2, "no header, and not a key or value");
    }

    #[test]
    fn values_under_a_deleted_key_are_refused() {
        let p = parse("REGEDIT4\n[-HKEY_CURRENT_USER\\A]\n\"x\"=\"y\"\n");
        assert_eq!(p.errors.len(), 1);
    }

    #[test]
    fn text_round_trips_in_its_encoding() {
        for enc in [Encoding::Utf16, Encoding::Utf8Bom, Encoding::Utf8] {
            let bytes = encode("REGEDIT4\r\n€", enc).unwrap();
            assert_eq!(decode(&bytes), ("REGEDIT4\r\n€".to_string(), enc));
        }
        let ansi = [b'a', 0xE9];
        assert_eq!(decode(&ansi), ("aé".to_string(), Encoding::Ansi));
        assert_eq!(encode("aé", Encoding::Ansi).unwrap(), ansi);
        assert!(encode("€", Encoding::Ansi).is_err());
        assert_eq!(crlf("a\nb\r\nc"), "a\r\nb\r\nc");
    }
}
