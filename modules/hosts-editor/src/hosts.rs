//! Reading and writing the hosts file. Lines that are not entries (comments, blank
//! lines) and entries the user did not touch are written back exactly as they were.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// False for an entry that is commented out with `#`.
    pub active: bool,
    pub address: String,
    pub hosts: Vec<String>,
    pub comment: String,
    /// The line as read, kept while the entry is unchanged.
    raw: Option<String>,
}

impl Entry {
    pub fn new(active: bool, address: &str, hosts: Vec<String>, comment: &str) -> Entry {
        Entry {
            active,
            address: address.to_string(),
            hosts,
            comment: comment.trim().to_string(),
            raw: None,
        }
    }

    /// Turn the entry on or off, keeping the rest of its line as it was.
    pub fn set_active(&mut self, on: bool) {
        if on == self.active {
            return;
        }
        self.active = on;
        if let Some(raw) = &mut self.raw {
            *raw = if on {
                raw.trim_start().trim_start_matches('#').trim_start().to_string()
            } else {
                format!("# {raw}")
            };
        }
    }

    fn parse(line: &str) -> Option<Entry> {
        let body = line.trim_start();
        let (active, body) = match body.strip_prefix('#') {
            Some(rest) => (false, rest.trim_start_matches('#')),
            None => (true, body),
        };
        let (body, comment) = body.split_once('#').unwrap_or((body, ""));
        let mut words = body.split_whitespace();
        let address = words.next()?;
        if !is_address(address) {
            return None;
        }
        let hosts: Vec<String> = words.map(str::to_string).collect();
        // A commented line counts as a disabled entry only if it reads like one.
        if hosts.is_empty() || (!active && !hosts.iter().all(|h| is_host(h))) {
            return None;
        }
        Some(Entry {
            raw: Some(line.to_string()),
            ..Entry::new(active, address, hosts, comment)
        })
    }

    fn render(&self) -> String {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut line = format!(
            "{}{}\t{}",
            if self.active { "" } else { "# " },
            self.address,
            self.hosts.join(" ")
        );
        if !self.comment.is_empty() {
            line.push_str("\t# ");
            line.push_str(&self.comment);
        }
        line
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Entry(Entry),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostsFile {
    pub lines: Vec<Line>,
    crlf: bool,
    final_newline: bool,
    bom: bool,
}

impl HostsFile {
    pub fn parse(text: &str) -> HostsFile {
        let (bom, text) = match text.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let crlf = text.contains("\r\n") || text.is_empty();
        let final_newline = text.ends_with('\n') || text.is_empty();
        let body = text.strip_suffix('\n').unwrap_or(text);
        let lines = if text.is_empty() {
            Vec::new()
        } else {
            body.split('\n')
                .map(|l| {
                    let l = l.strip_suffix('\r').unwrap_or(l);
                    Entry::parse(l).map_or_else(|| Line::Other(l.to_string()), Line::Entry)
                })
                .collect()
        };
        HostsFile {
            lines,
            crlf,
            final_newline,
            bom,
        }
    }

    pub fn render(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut out = self
            .lines
            .iter()
            .map(|l| match l {
                Line::Entry(e) => e.render(),
                Line::Other(s) => s.clone(),
            })
            .collect::<Vec<_>>()
            .join(eol);
        if self.final_newline && !self.lines.is_empty() {
            out.push_str(eol);
        }
        if self.bom {
            out.insert(0, '\u{feff}');
        }
        out
    }

    /// The position in `lines` of each entry, in order.
    pub fn entry_lines(&self) -> Vec<usize> {
        (0..self.lines.len())
            .filter(|&i| matches!(self.lines[i], Line::Entry(_)))
            .collect()
    }

    pub fn entry(&self, line: usize) -> Option<&Entry> {
        match self.lines.get(line) {
            Some(Line::Entry(e)) => Some(e),
            _ => None,
        }
    }

    pub fn entry_mut(&mut self, line: usize) -> Option<&mut Entry> {
        match self.lines.get_mut(line) {
            Some(Line::Entry(e)) => Some(e),
            _ => None,
        }
    }

    /// For each entry (as in [`HostsFile::entry_lines`]), whether another active entry
    /// maps one of its host names for the same kind of address (IPv4 or IPv6).
    pub fn duplicates(&self) -> Vec<bool> {
        let key = |e: &Entry, h: &str| (h.to_ascii_lowercase(), e.address.contains(':'));
        let mut seen: HashMap<(String, bool), usize> = HashMap::new();
        let entries: Vec<&Entry> = self.entry_lines().into_iter().filter_map(|i| self.entry(i)).collect();
        for e in entries.iter().filter(|e| e.active) {
            for h in &e.hosts {
                *seen.entry(key(e, h)).or_default() += 1;
            }
        }
        entries
            .iter()
            .map(|e| e.active && e.hosts.iter().any(|h| seen.get(&key(e, h)).is_some_and(|&n| n > 1)))
            .collect()
    }
}

fn is_address(s: &str) -> bool {
    // IPv6 link-local addresses may carry a zone (`fe80::1%3`).
    let ip = s.split_once('%').map_or(s, |(ip, _)| ip);
    ip.parse::<IpAddr>().is_ok()
}

fn is_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with(['-', '.'])
        && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// Where Windows keeps the hosts file.
pub fn system_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&root).join(r"System32\drivers\etc\hosts")
}

/// Largest hosts file the editor writes; ad-blocking lists run to a few megabytes.
const MAX_SIZE: u64 = 64 << 20;

/// Exit codes of the elevated `--write-hosts` step, which copies a prepared file
/// over the hosts file.
pub mod exit {
    pub const OK: u8 = 0;
    pub const USAGE: u8 = 2;
    pub const BAD_SOURCE: u8 = 3;
    pub const WRITE_FAILED: u8 = 4;

    pub fn message(code: u32) -> String {
        match code {
            0 => "Saved.".into(),
            2 | 3 => "The new hosts file could not be handed over for saving.".into(),
            4 => "Windows did not let PowerKit write the hosts file. Security software may be \
                  blocking it."
                .into(),
            other => format!("Saving stopped with code {other}."),
        }
    }
}

/// Copy the text in `src` over `dest`. Run elevated; the text must be UTF-8.
pub fn install(src: &Path, dest: &Path) -> Result<(), u8> {
    let small = std::fs::metadata(src).is_ok_and(|m| m.len() <= MAX_SIZE);
    let text = std::fs::read(src)
        .ok()
        .filter(|_| small)
        .and_then(|b| String::from_utf8(b).ok());
    let Some(text) = text else {
        return Err(exit::BAD_SOURCE);
    };
    // Written in place rather than replaced, so the file keeps its permissions.
    std::fs::write(dest, text).map_err(|_| exit::WRITE_FAILED)
}

/// Check what was typed for an entry; returns the address and host names.
pub fn validate(address: &str, hosts: &str) -> Result<(String, Vec<String>), String> {
    let address = address.trim();
    if !is_address(address) {
        return Err(format!("{address:?} is not an IP address"));
    }
    let hosts: Vec<String> = hosts
        .split([' ', ',', ';', '\t'])
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect();
    if hosts.is_empty() {
        return Err("Type at least one host name".into());
    }
    if let Some(bad) = hosts.iter().find(|h| !is_host(h)) {
        return Err(format!("{bad:?} is not a valid host name"));
    }
    Ok((address.to_string(), hosts))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Copyright (c) 1993-2009 Microsoft Corp.\r\n#\r\n#      102.54.94.97     rhino.acme.com          # source server\r\n#\t127.0.0.1       localhost\r\n\r\n127.0.0.1   dev.local api.local   # work\r\n::1 dev.local\r\n";

    #[test]
    fn untouched_files_are_written_back_exactly() {
        for text in [SAMPLE, "", "1.2.3.4 a", "1.2.3.4 a\n\n# x\n", "junk line\r\n"] {
            assert_eq!(HostsFile::parse(text).render(), text, "{text:?}");
        }
    }

    #[test]
    fn entries_and_comments_are_told_apart() {
        let f = HostsFile::parse(SAMPLE);
        let entries: Vec<&Entry> = f.entry_lines().into_iter().filter_map(|i| f.entry(i)).collect();
        assert_eq!(entries.len(), 4);
        assert!(!entries[0].active);
        assert_eq!(entries[0].address, "102.54.94.97");
        assert_eq!(entries[0].hosts, ["rhino.acme.com"]);
        assert_eq!(entries[0].comment, "source server");
        assert!(entries[2].active);
        assert_eq!(entries[2].hosts, ["dev.local", "api.local"]);
        assert_eq!(entries[2].comment, "work");
        assert_eq!(entries[3].address, "::1");
        assert!(matches!(f.lines[0], Line::Other(_)), "prose comment is not an entry");
    }

    #[test]
    fn toggling_keeps_the_line_and_edits_are_formatted() {
        let mut f = HostsFile::parse(SAMPLE);
        let lines = f.entry_lines();
        f.entry_mut(lines[1]).unwrap().set_active(true);
        f.entry_mut(lines[2]).unwrap().set_active(false);
        f.lines[lines[3]] = Line::Entry(Entry::new(true, "::1", vec!["x.local".into()], " note "));
        f.lines.push(Line::Entry(Entry::new(
            false,
            "10.0.0.1",
            vec!["a".into(), "b".into()],
            "",
        )));
        let out = f.render();
        assert!(out.contains("\r\n127.0.0.1       localhost\r\n"), "{out}");
        assert!(
            out.contains("\r\n# 127.0.0.1   dev.local api.local   # work\r\n"),
            "{out}"
        );
        assert!(
            out.ends_with("\r\n::1\tx.local\t# note\r\n# 10.0.0.1\ta b\r\n"),
            "{out}"
        );
        let again = HostsFile::parse(&out);
        assert_eq!(again.entry_lines().len(), 5);
    }

    #[test]
    fn duplicates_count_active_entries_per_address_family() {
        let f = HostsFile::parse("1.1.1.1 a b\n2.2.2.2 A\n::1 a\n# 3.3.3.3 b\n4.4.4.4 c\n");
        assert_eq!(f.duplicates(), [true, true, false, false, false]);
    }

    #[test]
    fn byte_order_mark_is_kept_out_of_the_first_entry() {
        let f = HostsFile::parse("\u{feff}1.1.1.1 a\n");
        assert_eq!(f.entry_lines(), [0]);
        assert_eq!(f.render(), "\u{feff}1.1.1.1 a\n");
    }

    #[test]
    fn install_copies_text_only() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dest) = (dir.path().join("new"), dir.path().join("hosts"));
        std::fs::write(&dest, "old").unwrap();
        assert_eq!(install(&src, &dest), Err(exit::BAD_SOURCE), "missing source");
        std::fs::write(&src, [0xff, 0xfe]).unwrap();
        assert_eq!(install(&src, &dest), Err(exit::BAD_SOURCE), "not UTF-8");
        std::fs::write(&src, "1.1.1.1 a\r\n").unwrap();
        assert_eq!(install(&src, &dest), Ok(()));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "1.1.1.1 a\r\n");
    }

    #[test]
    fn typed_entries_are_checked() {
        assert_eq!(
            validate(" 10.0.0.1 ", "a.local, b.local").unwrap(),
            (
                "10.0.0.1".to_string(),
                vec!["a.local".to_string(), "b.local".to_string()]
            )
        );
        assert!(validate("fe80::1%3", "x").is_ok());
        assert!(validate("10.0.0", "x").is_err());
        assert!(validate("10.0.0.1", "  ").is_err());
        assert!(validate("10.0.0.1", "bad/host").is_err());
    }
}
