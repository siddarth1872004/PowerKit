//! Markdown as Rich Text: headings, paragraphs, emphasis, code (colored when the fence
//! names a language), quotes, lists, task lists, tables, rules and links. Pictures show
//! as their description; raw HTML shows as text.

use crate::code;
use crate::rtf::{escape, header, Color, Theme};

pub struct Document {
    pub rtf: String,
    /// Each link's text as shown and where it points, for opening a clicked link.
    pub links: Vec<(String, String)>,
}

pub fn to_rtf(md: &str, theme: Theme) -> Document {
    let mut doc = Document {
        rtf: header(theme),
        links: Vec::new(),
    };
    let lines: Vec<&str> = md.lines().collect();
    blocks(&lines, 0, &mut doc);
    doc.rtf.push('}');
    doc
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn fence(line: &str) -> Option<(char, usize, &str)> {
    let t = line.trim_start();
    if indent_of(line) > 3 {
        return None;
    }
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.len() - t.trim_start_matches(c).len();
    (n >= 3).then(|| (c, n, t[n..].trim()))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let level = t.len() - t.trim_start_matches('#').len();
    let rest = &t[level..];
    ((1..=6).contains(&level) && indent_of(line) <= 3 && (rest.is_empty() || rest.starts_with(' '))).then(|| {
        let text = rest.trim();
        (level, text.trim_end_matches('#').trim_end())
    })
}

fn rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && ["-", "*", "_"].iter().any(|c| t.chars().all(|x| x.to_string() == *c))
}

/// A list item: its indent, marker as shown and the text after it.
fn item(line: &str) -> Option<(usize, String, &str)> {
    let indent = indent_of(line);
    let t = &line[indent..];
    if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| t.strip_prefix(m)) {
        return Some((indent, String::new(), rest));
    }
    let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let after = &t[digits..];
    (1..=9)
        .contains(&digits)
        .then(|| after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")))
        .flatten()
        .map(|rest| (indent, t[..digits + 1].to_string(), rest))
}

fn quote(line: &str) -> Option<&str> {
    let t = line.trim_start();
    (indent_of(line) <= 3)
        .then(|| t.strip_prefix('>'))
        .flatten()
        .map(|r| r.strip_prefix(' ').unwrap_or(r))
}

fn table_row(line: &str) -> Vec<&str> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(str::trim).collect()
}

fn table_divider(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.contains(['|', '-']) && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn starts_block(lines: &[&str], i: usize) -> bool {
    let line = lines[i];
    line.trim().is_empty()
        || fence(line).is_some()
        || heading(line).is_some()
        || rule(line)
        || quote(line).is_some()
        || item(line).is_some()
        || line.contains('|') && lines.get(i + 1).is_some_and(|n| table_divider(n))
}

fn blocks(lines: &[&str], indent: u32, doc: &mut Document) {
    let muted = if indent > 0 { "\\cf2 " } else { "" };
    let pard = |extra: &str| format!("\\pard\\li{indent}\\sa120{extra}{muted}");
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
        } else if let Some((c, n, info)) = fence(line) {
            let lang = info.split_whitespace().next().unwrap_or("");
            let end = lines[i + 1..]
                .iter()
                .position(|l| fence(l).is_some_and(|(c2, n2, rest)| c2 == c && n2 >= n && rest.is_empty()))
                .map_or(lines.len(), |p| i + 1 + p);
            let body = lines[i + 1..end].join("\n");
            let runs = if code::known(lang) {
                code::spans(&body, lang)
            } else {
                body.lines().map(|l| vec![(None, l.to_string())]).collect()
            };
            let count = runs.len().max(1);
            for (k, line) in runs.iter().chain(runs.is_empty().then_some(&Vec::new())).enumerate() {
                let space = if k + 1 == count { 120 } else { 0 };
                doc.rtf.push_str(&format!(
                    "\\pard\\li{}\\sa{space}{{\\f1\\fs18\\highlight{} {}}}\\par\n",
                    indent + 240,
                    Color::CodeBack as u8,
                    code::line_rtf(line)
                ));
            }
            i = end + 1;
        } else if let Some((level, text)) = heading(line) {
            push_heading(doc, indent, level, text);
            i += 1;
        } else if i + 1 < lines.len()
            && !lines[i + 1].trim().is_empty()
            && (lines[i + 1].trim().chars().all(|c| c == '=') || lines[i + 1].trim().chars().all(|c| c == '-'))
            && item(line).is_none()
            && quote(line).is_none()
        {
            let level = if lines[i + 1].trim().starts_with('=') { 1 } else { 2 };
            push_heading(doc, indent, level, line.trim());
            i += 2;
        } else if rule(line) {
            doc.rtf
                .push_str(&format!("{}\\cf2 {}\\cf1\\par\n", pard(""), "\\u9472?".repeat(40)));
            i += 1;
        } else if quote(line).is_some() {
            let mut inner = Vec::new();
            while let Some(q) = lines.get(i).and_then(|l| quote(l)) {
                inner.push(q);
                i += 1;
            }
            blocks(&inner, indent + 360, doc);
        } else if let Some((at, marker, first)) = item(line) {
            let mut text = first.trim().to_string();
            i += 1;
            while i < lines.len() && !starts_block(lines, i) && indent_of(lines[i]) > at {
                text.push(' ');
                text.push_str(lines[i].trim());
                i += 1;
            }
            let (mark, text) = if let Some(rest) = text.strip_prefix("[ ] ") {
                ("\\u9744?".to_string(), rest.to_string())
            } else if let Some(rest) = text.strip_prefix("[x] ").or_else(|| text.strip_prefix("[X] ")) {
                ("\\u9745?".to_string(), rest.to_string())
            } else if marker.is_empty() {
                (
                    (if (at / 2) % 2 == 0 { "\\u8226?" } else { "\\u9702?" }).to_string(),
                    text,
                )
            } else {
                (escape(&marker), text)
            };
            let left = indent + 360 * (at as u32 / 2 + 1);
            let text = inline(&text, doc);
            doc.rtf.push_str(&format!(
                "\\pard\\li{left}\\fi-300\\tx{left}\\sa40{muted}{mark}\\tab {text}\\par\n"
            ));
        } else if line.contains('|') && lines.get(i + 1).is_some_and(|n| table_divider(n)) {
            let mut rows = vec![table_row(line)];
            i += 2;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(table_row(lines[i]));
                i += 1;
            }
            push_table(doc, indent, &rows);
        } else {
            let mut text = String::new();
            while i < lines.len() && (text.is_empty() || !starts_block(lines, i)) {
                let l = lines[i];
                if !text.is_empty() {
                    text.push(if text.ends_with("  ") || text.ends_with('\\') {
                        '\n'
                    } else {
                        ' '
                    });
                }
                text.push_str(l.trim_start());
                i += 1;
                // A setext underline belongs to the heading test above.
                if lines
                    .get(i + 1)
                    .is_some_and(|n| !n.trim().is_empty() && n.trim().chars().all(|c| c == '='))
                {
                    break;
                }
            }
            let parts: Vec<String> = text
                .split('\n')
                .map(|p| inline(p.trim_end().trim_end_matches('\\').trim_end(), doc))
                .collect();
            doc.rtf
                .push_str(&format!("{}{}\\par\n", pard(""), parts.join("\\line ")));
        }
    }
}

fn push_heading(doc: &mut Document, indent: u32, level: usize, text: &str) {
    const SIZES: [u32; 6] = [40, 32, 28, 24, 22, 20];
    let size = SIZES[level - 1];
    let before = if level <= 2 { 240 } else { 160 };
    let body = inline(text, doc);
    doc.rtf.push_str(&format!(
        "\\pard\\li{indent}\\sb{before}\\sa120\\keepn{{\\b\\fs{size} {body}}}\\par\n"
    ));
}

fn push_table(doc: &mut Document, indent: u32, rows: &[Vec<&str>]) {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(1);
    let widths: Vec<u32> = (0..cols)
        .map(|c| {
            let chars = rows
                .iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0);
            (chars as u32 * 110 + 300).clamp(800, 4000)
        })
        .collect();
    const BORDER: &str =
        "\\clbrdrt\\brdrs\\brdrw10\\clbrdrl\\brdrs\\brdrw10\\clbrdrb\\brdrs\\brdrw10\\clbrdrr\\brdrs\\brdrw10";
    for (r, row) in rows.iter().enumerate() {
        let mut def = format!("\\trowd\\trgaph100\\trleft{indent}");
        let mut x = indent;
        for w in &widths {
            x += w;
            def.push_str(&format!("{BORDER}\\cellx{x}"));
        }
        doc.rtf.push_str(&def);
        for c in 0..cols {
            let cell = inline(row.get(c).copied().unwrap_or(""), doc);
            let cell = if r == 0 { format!("{{\\b {cell}}}") } else { cell };
            doc.rtf.push_str(&format!("\\pard\\intbl {cell}\\cell"));
        }
        doc.rtf.push_str("\\row\n");
    }
    doc.rtf.push_str("\\pard\\sa120\\par\n");
}

/// Text without its Markdown marks, as a link shows it.
fn plain(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '~')).collect()
}

/// `[text](url "title")` starting at `s[0] == '['`: the text, the url and the length.
fn link_at(s: &str) -> Option<(&str, &str, usize)> {
    let mut depth = 0;
    let close = s.char_indices().find_map(|(i, c)| {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        None
    })?;
    let rest = s[close + 1..].strip_prefix('(')?;
    let end = rest.find(')')?;
    let target = rest[..end].trim();
    let url = target.split_once(char::is_whitespace).map_or(target, |(u, _)| u);
    let url = url.trim_start_matches('<').trim_end_matches('>');
    Some((&s[1..close], url, close + 2 + end + 1))
}

fn field(url: &str, shown: &str) -> String {
    format!(
        "{{\\field{{\\*\\fldinst{{HYPERLINK \"{}\"}}}}{{\\fldrslt{{\\ul\\cf{} {shown}}}}}}}",
        escape(url).replace('"', "%22"),
        Color::Link as u8
    )
}

/// Inline Markdown as RTF; links found are added to `doc.links`.
fn inline(s: &str, doc: &mut Document) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().expect("i is inside s");
        let prev = s[..i].chars().next_back();
        if c == '\\' && rest[1..].starts_with(|n: char| n.is_ascii_punctuation()) {
            out.push_str(&escape(&rest[1..2]));
            i += 2;
            continue;
        }
        if c == '`' {
            let n = rest.len() - rest.trim_start_matches('`').len();
            let marks = &rest[..n];
            if let Some(end) = rest[n..].find(marks) {
                let code = &rest[n..n + end];
                let code = if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') {
                    &code[1..code.len() - 1]
                } else {
                    code
                };
                out.push_str(&format!(
                    "{{\\f1\\fs18\\highlight{} {}}}",
                    Color::CodeBack as u8,
                    escape(code)
                ));
                i += n + end + n;
                continue;
            }
            out.push_str(marks);
            i += n;
            continue;
        }
        if c == '!' {
            if let Some((alt, _, len)) = rest[1..].starts_with('[').then(|| link_at(&rest[1..])).flatten() {
                out.push_str(&format!("{{\\cf{} [{}]}}", Color::Muted as u8, escape(&plain(alt))));
                i += 1 + len;
                continue;
            }
        }
        if c == '[' {
            if let Some((text, url, len)) = link_at(rest) {
                let shown = inline(text, doc);
                doc.links.push((plain(text), url.to_string()));
                out.push_str(&field(url, &shown));
                i += len;
                continue;
            }
        }
        if c == '<' {
            if let Some(end) = rest.find('>') {
                let url = &rest[1..end];
                if ["http://", "https://", "mailto:"].iter().any(|p| url.starts_with(p)) && !url.contains(' ') {
                    doc.links.push((url.to_string(), url.to_string()));
                    out.push_str(&field(url, &escape(url)));
                    i += end + 1;
                    continue;
                }
            }
        }
        if matches!(c, '*' | '_' | '~') {
            let run = rest.len() - rest.trim_start_matches(c).len();
            let word_inside = c == '_' && prev.is_some_and(char::is_alphanumeric);
            let n = if run >= 2 { 2 } else { 1 };
            if !word_inside && !(c == '~' && n == 1) {
                if let Some((inner, len)) = emphasis(rest, c, n) {
                    let control = match (c, n) {
                        ('~', _) => "\\strike",
                        (_, 2) => "\\b",
                        _ => "\\i",
                    };
                    out.push_str(&format!("{{{control} {}}}", inline(inner, doc)));
                    i += len;
                    continue;
                }
            }
            out.push_str(&escape(&rest[..run]));
            i += run;
            continue;
        }
        out.push_str(&escape(&rest[..c.len_utf8()]));
        i += c.len_utf8();
    }
    out
}

/// Emphasis opened by `n` of `c` at the start of `s`: the text inside and the length
/// with both marks.
fn emphasis(s: &str, c: char, n: usize) -> Option<(&str, usize)> {
    let marks = &s[..n];
    let body = &s[n..];
    if body.starts_with(char::is_whitespace) {
        return None;
    }
    let mut from = 0;
    while let Some(p) = body[from..].find(marks) {
        let at = from + p;
        let run = body[at..].len() - body[at..].trim_start_matches(c).len();
        // `**` closes at the end of a longer run, so `***x***` is bold around italic;
        // a single mark does not close at a double one.
        let (close, fits) = if n == 2 { (at + run - 2, true) } else { (at, run == 1) };
        let before = body[..close].chars().next_back();
        let after = body[close + n..].chars().next();
        let ok = close > 0
            && fits
            && before.is_some_and(|b| !b.is_whitespace())
            && !(c == '_' && after.is_some_and(char::is_alphanumeric));
        if ok {
            return Some((&body[..close], close + 2 * n));
        }
        from = at + run;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(md: &str) -> String {
        let doc = to_rtf(md, Theme::LIGHT);
        let start = doc.rtf.find("\\cf1 ").unwrap() + 5;
        doc.rtf[start..doc.rtf.len() - 1].to_string()
    }

    #[test]
    fn inline_marks_become_formatting() {
        let mut doc = to_rtf("", Theme::LIGHT);
        assert_eq!(
            inline("a **b** *c* _d_ ~~e~~", &mut doc),
            "a {\\b b} {\\i c} {\\i d} {\\strike e}"
        );
        assert_eq!(
            inline("snake_case_name and 2 * 3 * 4", &mut doc),
            "snake_case_name and 2 * 3 * 4"
        );
        assert_eq!(inline("***both***", &mut doc), "{\\b {\\i both}}");
        assert_eq!(
            inline("`a{b}` \\*x\\*", &mut doc),
            "{\\f1\\fs18\\highlight4 a\\{b\\}} *x*"
        );
        assert_eq!(inline("![logo](a.png)", &mut doc), "{\\cf2 [logo]}");
    }

    #[test]
    fn links_are_fields_and_remembered() {
        let mut doc = to_rtf("", Theme::LIGHT);
        let out = inline("see [the **docs**](https://x.org/a \"t\") or <https://y.org>", &mut doc);
        assert!(out.starts_with(
            "see {\\field{\\*\\fldinst{HYPERLINK \"https://x.org/a\"}}{\\fldrslt{\\ul\\cf3 the {\\b docs}}}}"
        ));
        assert_eq!(
            doc.links,
            vec![
                ("the docs".into(), "https://x.org/a".into()),
                ("https://y.org".into(), "https://y.org".into())
            ]
        );
    }

    #[test]
    fn blocks_are_laid_out() {
        let md = "# Title #\n\nSome text\ncontinued  \nnext line\n\n- one\n  more\n  - two\n1. first\n- [x] done\n\n> quoted\n\n---\n\n```rust\nlet a = 1;\n```\n\n| A | B |\n|---|:-:|\n| 1 | 2 |\n\nSub\n===\n";
        let b = body(md);
        assert!(b.contains("{\\b\\fs40 Title}"), "{b}");
        assert!(b.contains("Some text continued\\line next line\\par"), "{b}");
        assert!(
            b.contains("\\li360\\fi-300\\tx360\\sa40\\u8226?\\tab one more\\par"),
            "{b}"
        );
        assert!(b.contains("\\li720\\fi-300\\tx720\\sa40\\u9702?\\tab two\\par"), "{b}");
        assert!(b.contains("1.\\tab first"), "{b}");
        assert!(b.contains("\\u9745?\\tab done"), "{b}");
        assert!(b.contains("\\li360\\sa120\\cf2 quoted\\par"), "{b}");
        assert!(b.contains("\\u9472?\\u9472?"), "{b}");
        assert!(b.contains("{\\cf5 let} a = {\\cf8 1};"), "{b}");
        assert!(
            b.contains("\\pard\\intbl {\\b A}\\cell\\pard\\intbl {\\b B}\\cell\\row"),
            "{b}"
        );
        assert!(b.contains("\\pard\\intbl 1\\cell\\pard\\intbl 2\\cell\\row"), "{b}");
        assert!(b.contains("{\\b\\fs40 Sub}"), "{b}");
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        let b = body("```\nraw *text*\n");
        assert!(b.contains("raw *text*"), "{b}");
    }
}
