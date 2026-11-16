//! Turning what is on the clipboard into another format. Pure functions, so they are
//! tested on any platform.

use serde_json::Value;

/// The HTML a CF_HTML clipboard text carries: the part between its
/// `StartFragment`/`EndFragment` byte offsets, else between the fragment comments,
/// else all of it.
pub fn html_fragment(clip: &str) -> &str {
    let offset = |key: &str| -> Option<usize> {
        let at = clip.find(key)? + key.len();
        clip[at..]
            .split(['\r', '\n'])
            .next()
            .and_then(|n| n.trim().parse().ok())
    };
    if let (Some(start), Some(end)) = (offset("StartFragment:"), offset("EndFragment:")) {
        if start <= end && clip.is_char_boundary(start) && clip.is_char_boundary(end) && end <= clip.len() {
            return &clip[start..end];
        }
    }
    let (open, close) = ("<!--StartFragment-->", "<!--EndFragment-->");
    match (clip.find(open), clip.find(close)) {
        (Some(a), Some(b)) if a + open.len() <= b => &clip[a + open.len()..b],
        _ => clip,
    }
}

const SKIPPED: [&str; 6] = ["script", "style", "head", "title", "noscript", "template"];

enum Token<'a> {
    Text(String),
    Open { name: String, attrs: &'a str },
    Close(String),
}

/// Split HTML into text and tags; comments, doctypes and processing instructions are
/// dropped.
fn tokens(html: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            out.push(Token::Text(decode(rest)));
            break;
        };
        if lt > 0 {
            out.push(Token::Text(decode(&rest[..lt])));
        }
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<!--") {
            rest = body.find("-->").map_or("", |i| &body[i + 3..]);
            continue;
        }
        let Some(tag_end) = tag_end(rest) else {
            // A lone "<" with no tag after it.
            out.push(Token::Text(decode(rest)));
            break;
        };
        let inner = &rest[1..tag_end - 1];
        rest = &rest[tag_end..];
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        let (closing, inner) = match inner.strip_prefix('/') {
            Some(i) => (true, i),
            None => (false, inner),
        };
        let name_len = inner.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(inner.len());
        if name_len == 0 {
            // Not a tag after all, such as "a < b".
            out.push(Token::Text(decode(&format!("<{inner}>"))));
            continue;
        }
        let name = inner[..name_len].to_ascii_lowercase();
        if closing {
            out.push(Token::Close(name));
        } else {
            out.push(Token::Open {
                name,
                attrs: &inner[name_len..],
            });
        }
    }
    out
}

/// Where the tag starting at `s[0] == '<'` ends (just past its `>`), skipping `>`
/// inside quoted attribute values; `None` when it never ends.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices().skip(1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// The value of attribute `name` in a tag's attribute text.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let value = attrs[attrs.len() - after.len() + 1..].trim_start();
        let raw = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or(""),
        };
        return Some(decode(raw));
    }
    None
}

/// Replace HTML character references; unknown ones are kept as written.
pub fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| entity(&rest[1..e]));
        match (end, decoded) {
            (Some(e), Some(c)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let n = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(n);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "hellip" => '\u{2026}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "bull" => '\u{2022}',
        "middot" => '\u{b7}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "euro" => '\u{20ac}',
        "pound" => '\u{a3}',
        "times" => '\u{d7}',
        _ => return None,
    })
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
}

struct List {
    ordered: bool,
    next: usize,
    /// Width of this level's marker, which nested lines are indented by.
    width: usize,
}

/// Writes Markdown, adding blank lines, quote marks and list indents lazily so that
/// empty elements leave nothing behind.
#[derive(Default)]
struct Markdown {
    /// Output, with one extra buffer per table cell being read.
    bufs: Vec<String>,
    /// Newlines owed before the next text: 1 ends the line, 2 leaves a blank one.
    need: usize,
    line_start: bool,
    space: bool,
    quote: usize,
    lists: Vec<List>,
    marker: Option<String>,
    pre: usize,
    skip: usize,
    tables: Vec<Table>,
    links: Vec<Option<String>>,
    /// Each open inline marker, the output length before and after it, and whether a
    /// space was pending before it.
    inline: Vec<(String, usize, usize, bool)>,
}

impl Markdown {
    fn out(&mut self) -> &mut String {
        self.bufs.last_mut().expect("the output buffer")
    }

    fn in_cell(&self) -> bool {
        self.bufs.len() > 1
    }

    fn block(&mut self) {
        let n = if self.lists.is_empty() { 2 } else { 1 };
        self.need = self.need.max(n);
        self.space = false;
    }

    /// Emit owed newlines and the line's prefix before content.
    fn begin(&mut self) {
        if self.in_cell() {
            if self.need > 0 && !self.out().is_empty() {
                self.out().push(' ');
            }
            self.need = 0;
            return;
        }
        if self.need > 0 && !self.out().is_empty() {
            for i in 0..self.need {
                self.out().push('\n');
                if i + 1 < self.need && self.quote > 0 {
                    let marks = ">".repeat(self.quote);
                    self.out().push_str(&marks);
                    self.out().push('\n');
                    // The quote mark line already holds the blank line.
                    break;
                }
            }
            self.line_start = true;
        }
        self.need = 0;
        if self.out().is_empty() {
            self.line_start = true;
        }
        if self.line_start {
            let mut prefix = "> ".repeat(self.quote);
            let depth = self.lists.len();
            let indent: usize = match &self.marker {
                Some(_) => self.lists[..depth.saturating_sub(1)].iter().map(|l| l.width).sum(),
                None => self.lists.iter().map(|l| l.width).sum(),
            };
            prefix.push_str(&" ".repeat(indent));
            if let Some(m) = self.marker.take() {
                prefix.push_str(&m);
            }
            self.out().push_str(&prefix);
            self.line_start = false;
            self.space = false;
        }
    }

    fn raw(&mut self, s: &str) {
        self.begin();
        self.out().push_str(s);
    }

    /// Write an opening marker: a pending space goes before it.
    fn open_mark(&mut self, s: &str) {
        if self.space && !self.line_start && self.need == 0 {
            self.raw(" ");
        }
        self.space = false;
        self.raw(s);
    }

    fn text(&mut self, s: &str) {
        if self.skip > 0 {
            return;
        }
        if self.pre > 0 {
            for (i, line) in s.split('\n').enumerate() {
                if i > 0 {
                    self.need = 1;
                }
                if !line.is_empty() {
                    self.raw(line.trim_end_matches('\r'));
                }
            }
            return;
        }
        for c in s.chars() {
            if c.is_whitespace() {
                self.space = true;
                continue;
            }
            if self.space && !self.line_start && self.need == 0 && !self.out().is_empty() {
                self.out().push(' ');
            }
            self.space = false;
            self.begin();
            self.out().push(c);
        }
    }

    fn open_inline(&mut self, mark: &str) {
        let (before, space) = (self.out().len(), self.space);
        self.open_mark(mark);
        let after = self.out().len();
        self.inline.push((mark.into(), before, after, space));
    }

    fn close_inline(&mut self, mark: &str) {
        let Some(at) = self.inline.iter().rposition(|(m, ..)| m == mark) else {
            return;
        };
        let (_, before, after, space) = self.inline.remove(at);
        if self.out().len() == after {
            // Nothing inside: drop the opening mark, and the space written before it
            // goes back to pending.
            let written = &self.bufs.last().map_or("", |b| &b[before..]);
            if *written == mark || written.strip_prefix(' ') == Some(mark) {
                self.out().truncate(before);
                self.space |= space;
            } else {
                self.out().truncate(after - mark.len());
            }
        } else {
            self.out().push_str(mark);
        }
    }

    fn open(&mut self, name: &str, attrs: &str) {
        if SKIPPED.contains(&name) {
            self.skip += 1;
            return;
        }
        if self.skip > 0 {
            return;
        }
        match name {
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside" | "figure" | "dl"
            | "dt" | "dd" | "address" => self.block(),
            "br" => {
                if self.in_cell() {
                    self.space = true;
                } else {
                    self.need = self.need.max(1);
                }
            }
            "hr" => {
                self.block();
                self.raw("---");
                self.block();
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block();
                let level = name[1..].parse().unwrap_or(1);
                self.raw(&format!("{} ", "#".repeat(level)));
            }
            "blockquote" => {
                self.block();
                self.quote += 1;
            }
            "pre" => {
                self.block();
                self.raw("```");
                self.need = 1;
                self.pre += 1;
            }
            "ul" | "ol" => {
                if self.lists.is_empty() {
                    self.block();
                }
                let start = attr(attrs, "start").and_then(|s| s.parse().ok()).unwrap_or(1);
                self.lists.push(List {
                    ordered: name == "ol",
                    next: start,
                    width: 2,
                });
            }
            "li" => {
                self.need = self.need.max(1);
                self.space = false;
                let marker = match self.lists.last_mut() {
                    Some(l) if l.ordered => {
                        let m = format!("{}. ", l.next);
                        l.next += 1;
                        l.width = m.len();
                        m
                    }
                    _ => "- ".into(),
                };
                self.marker = Some(marker);
                self.line_start = true;
            }
            "b" | "strong" => self.open_inline("**"),
            "i" | "em" => self.open_inline("*"),
            "s" | "del" | "strike" => self.open_inline("~~"),
            "code" if self.pre == 0 => self.open_inline("`"),
            "a" => {
                let href = attr(attrs, "href").filter(|h| !h.is_empty() && !h.starts_with("javascript:"));
                if href.is_some() {
                    self.open_mark("[");
                }
                self.links.push(href);
            }
            "img" => {
                if let Some(src) = attr(attrs, "src") {
                    let alt = attr(attrs, "alt").unwrap_or_default();
                    self.open_mark(&format!("![{alt}]({src})"));
                }
            }
            "table" => {
                self.block();
                self.tables.push(Table::default());
            }
            "td" | "th" if !self.tables.is_empty() => {
                self.bufs.push(String::new());
                self.space = false;
                self.need = 0;
            }
            _ => {}
        }
    }

    fn close(&mut self, name: &str) {
        if SKIPPED.contains(&name) {
            self.skip = self.skip.saturating_sub(1);
            return;
        }
        if self.skip > 0 {
            return;
        }
        match name {
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside" | "figure" | "dl"
            | "dt" | "dd" | "address" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.block(),
            "blockquote" => {
                self.quote = self.quote.saturating_sub(1);
                self.block();
            }
            "pre" if self.pre > 0 => {
                self.pre -= 1;
                self.need = 1;
                self.raw("```");
                self.block();
            }
            "ul" | "ol" => {
                self.lists.pop();
                self.marker = None;
                self.block();
            }
            "li" => {
                self.marker = None;
                self.need = self.need.max(1);
            }
            "b" | "strong" => self.close_inline("**"),
            "i" | "em" => self.close_inline("*"),
            "s" | "del" | "strike" => self.close_inline("~~"),
            "code" if self.pre == 0 => self.close_inline("`"),
            "a" => {
                if let Some(Some(href)) = self.links.pop() {
                    self.out().push_str(&format!("]({href})"));
                }
            }
            "td" | "th" if self.in_cell() => {
                let cell = self.bufs.pop().unwrap_or_default();
                let cell = cell.trim().replace('|', "\\|");
                if let Some(t) = self.tables.last_mut() {
                    t.row.push(cell);
                }
                self.space = false;
            }
            "tr" => {
                if let Some(t) = self.tables.last_mut() {
                    if !t.row.is_empty() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
            }
            "table" => {
                if let Some(mut t) = self.tables.pop() {
                    if !t.row.is_empty() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                    self.table(t.rows);
                }
            }
            _ => {}
        }
    }

    fn table(&mut self, mut rows: Vec<Vec<String>>) {
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        for row in &mut rows {
            row.resize(cols, String::new());
        }
        let line = |cells: &[String]| format!("| {} |", cells.join(" | "));
        let rule = vec!["---".to_string(); cols];
        self.block();
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                self.need = 1;
            }
            self.raw(&line(row));
            if i == 0 {
                self.need = 1;
                self.raw(&line(&rule));
            }
        }
        self.block();
    }
}

/// HTML as Markdown: headings, paragraphs, emphasis, links, pictures, lists, quotes,
/// code and tables. Other tags keep only their text.
pub fn html_to_markdown(html: &str) -> String {
    let mut md = Markdown {
        bufs: vec![String::new()],
        line_start: true,
        ..Default::default()
    };
    for token in tokens(html) {
        match token {
            Token::Text(t) => md.text(&t),
            Token::Open { name, attrs } => md.open(&name, attrs),
            Token::Close(name) => md.close(&name),
        }
    }
    while md.bufs.len() > 1 {
        let cell = md.bufs.pop().unwrap_or_default();
        md.out().push_str(&cell);
    }
    let out = md.bufs.pop().unwrap_or_default();
    out.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .replace('\u{a0}', " ")
}

/// Text as JSON: JSON itself, tidied, or a table (comma, semicolon or tab separated,
/// first line the headings, at least two columns and one row) as a list of objects.
/// `None` when it is neither.
pub fn to_json(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if value.is_object() || value.is_array() {
            return serde_json::to_string_pretty(&value).ok();
        }
    }
    let first = trimmed.lines().next()?;
    let sep = ['\t', ',', ';'].into_iter().find(|&c| first.contains(c))?;
    let rows = parse_table(trimmed, sep)?;
    let (head, body) = rows.split_first()?;
    if head.len() < 2 || body.is_empty() || body.iter().any(|r| r.len() != head.len()) {
        return None;
    }
    // Written by hand so the keys keep the table's column order.
    let objects: Vec<String> = body
        .iter()
        .map(|row| {
            let fields: Vec<String> = head
                .iter()
                .zip(row)
                .map(|(k, v)| format!("    {}: {}", Value::String(k.trim().into()), scalar(v)))
                .collect();
            format!("  {{\n{}\n  }}", fields.join(",\n"))
        })
        .collect();
    Some(format!("[\n{}\n]", objects.join(",\n")))
}

/// A table cell as a JSON number or boolean when it plainly is one, else a string.
fn scalar(cell: &str) -> Value {
    let t = cell.trim();
    let leading_zero = t.len() > 1 && t.starts_with('0') && !t.starts_with("0.");
    if !leading_zero {
        if let Ok(n) = t.parse::<i64>() {
            return n.into();
        }
        if let Some(n) = t
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && t.contains('.'))
            .and_then(serde_json::Number::from_f64)
        {
            return Value::Number(n);
        }
    }
    match t {
        "true" | "TRUE" | "True" => true.into(),
        "false" | "FALSE" | "False" => false.into(),
        _ => Value::String(cell.to_string()),
    }
}

/// Rows of a delimited table, with `"quoted"` cells that may hold separators, line
/// breaks and doubled quotes. `None` if a quote is never closed.
fn parse_table(text: &str, sep: char) -> Option<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                }
            }
            '"' if cell.trim().is_empty() => {
                cell.clear();
                quoted = true;
            }
            c if c == sep && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if quoted {
        return None;
    }
    row.push(cell);
    rows.push(row);
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    Some(rows)
}

/// A picture read from a clipboard DIB: `0xAARRGGBB` pixels, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// Read a packed DIB (BITMAPINFOHEADER or a later header, then masks or palette, then
/// pixels) with 1, 4, 8, 16, 24 or 32 bits per pixel, uncompressed or with bit masks.
pub fn dib_picture(dib: &[u8]) -> Result<Picture, String> {
    let u32_at = |at: usize| -> Option<u32> { Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?)) };
    let u16_at = |at: usize| -> Option<u16> { Some(u16::from_le_bytes(dib.get(at..at + 2)?.try_into().ok()?)) };
    let short = || "the picture on the clipboard is cut short".to_string();
    let header = u32_at(0).ok_or_else(short)? as usize;
    if header < 40 {
        return Err("the picture on the clipboard is in an old format".into());
    }
    let width = u32_at(4).ok_or_else(short)? as i32;
    let height = u32_at(8).ok_or_else(short)? as i32;
    let bits = u16_at(14).ok_or_else(short)? as u32;
    let compression = u32_at(16).ok_or_else(short)?;
    let used = u32_at(32).ok_or_else(short)? as usize;
    const RGB: u32 = 0;
    const BITFIELDS: u32 = 3;
    if width <= 0 || height == 0 {
        return Err("the picture on the clipboard is empty".into());
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    if w.saturating_mul(h) > 1 << 28 {
        return Err("the picture on the clipboard is too big".into());
    }
    let mut at = header;
    let masks = match (compression, bits) {
        (BITFIELDS, 16 | 32) => {
            // Version 4 and 5 headers hold the masks themselves; version 1 is followed
            // by them.
            let m = if header >= 52 { 40 } else { header };
            if header < 52 {
                at += 12;
            }
            Some([u32_at(m), u32_at(m + 4), u32_at(m + 8)].map(|v| v.unwrap_or(0)))
        }
        (RGB, 16) => Some([0x7C00, 0x03E0, 0x001F]),
        (RGB, 1 | 4 | 8 | 24 | 32) => None,
        _ => return Err("the picture on the clipboard is compressed in a way PowerKit cannot read".into()),
    };
    let palette: Vec<u32> = if bits <= 8 {
        let n = if used == 0 { 1 << bits } else { used };
        let p = (0..n)
            .map(|i| u32_at(at + i * 4).map(|c| c & 0xFF_FFFF | 0xFF00_0000))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(short)?;
        at += n * 4;
        p
    } else {
        Vec::new()
    };
    let stride = (w * bits as usize).div_ceil(32) * 4;
    if dib.len() < at + stride * h {
        return Err(short());
    }
    let channel = |px: u32, mask: u32| -> u32 {
        if mask == 0 {
            return 0;
        }
        let v = ((px & mask) >> mask.trailing_zeros()) as u64;
        let max = (mask >> mask.trailing_zeros()) as u64;
        (v * 255 / max) as u32
    };
    let mut pixels = Vec::with_capacity(w * h);
    let mut any_alpha = false;
    for y in 0..h {
        let src_row = if height > 0 { h - 1 - y } else { y };
        let row = &dib[at + src_row * stride..at + (src_row + 1) * stride];
        for x in 0..w {
            let px = match bits {
                1 | 4 | 8 => {
                    let bit = x * bits as usize;
                    let byte = row[bit / 8];
                    let shift = 8 - bits as usize - bit % 8;
                    let index = (byte >> shift) as usize & ((1 << bits) - 1);
                    *palette.get(index).ok_or("the picture's colours are missing")?
                }
                16 => {
                    let v = u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]) as u32;
                    let [r, g, b] = masks.unwrap_or_default();
                    0xFF00_0000 | channel(v, r) << 16 | channel(v, g) << 8 | channel(v, b)
                }
                24 => {
                    let p = &row[x * 3..x * 3 + 3];
                    0xFF00_0000 | (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32
                }
                _ => {
                    let v = u32::from_le_bytes(row[x * 4..x * 4 + 4].try_into().unwrap_or_default());
                    let rgb = match masks {
                        Some([r, g, b]) => channel(v, r) << 16 | channel(v, g) << 8 | channel(v, b),
                        None => v & 0xFF_FFFF,
                    };
                    any_alpha |= v >> 24 != 0;
                    v & 0xFF00_0000 | rgb
                }
            };
            pixels.push(px);
        }
    }
    // Most programs leave the fourth byte of 32-bit pictures at zero: that means no
    // transparency, not a fully transparent picture.
    if bits == 32 && !any_alpha {
        for p in &mut pixels {
            *p |= 0xFF00_0000;
        }
    }
    Ok(Picture {
        width: w as u32,
        height: h as u32,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fragment_is_taken_from_the_clipboard_header() {
        let body = "<html><body><!--StartFragment--><b>Hi</b><!--EndFragment--></body></html>";
        let header_len = 105;
        let start = header_len + body.find("<b>").unwrap();
        let end = header_len + body.find("<!--End").unwrap();
        let header = format!(
            "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n",
            header_len,
            header_len + body.len(),
            start,
            end
        );
        assert_eq!(header.len(), header_len);
        assert_eq!(html_fragment(&format!("{header}{body}")), "<b>Hi</b>");
        assert_eq!(html_fragment(body), "<b>Hi</b>");
        assert_eq!(html_fragment("<p>x</p>"), "<p>x</p>");
    }

    #[test]
    fn html_becomes_markdown() {
        let html = "<h1>Title</h1><p>Some <b>bold</b> and <em>slanted</em> text with a \
                    <a href=\"https://example.com/?a=1&amp;b=2\">link</a>.</p>\
                    <p>Line<br>break &amp; <code>x &lt; y</code></p>";
        assert_eq!(
            html_to_markdown(html),
            "# Title\n\nSome **bold** and *slanted* text with a [link](https://example.com/?a=1&b=2).\n\n\
             Line\nbreak & `x < y`"
        );
    }

    #[test]
    fn lists_nest_and_count() {
        let html = "<ul><li>One</li><li>Two<ol start=\"3\"><li>Three</li><li>Four</li></ol></li></ul><p>After</p>";
        assert_eq!(html_to_markdown(html), "- One\n- Two\n  3. Three\n  4. Four\n\nAfter");
    }

    #[test]
    fn whitespace_collapses_and_scripts_are_dropped() {
        let html = "<style>p{}</style><p>  lots\n of   space <b> </b>here </p><script>x()</script>";
        assert_eq!(html_to_markdown(html), "lots of space here");
    }

    #[test]
    fn quotes_code_and_pictures() {
        let html = "<blockquote><p>Said</p><p>twice</p></blockquote><pre><code>fn main() {\n    go();\n}</code></pre>\
                    <img src=\"a.png\" alt=\"A\">";
        assert_eq!(
            html_to_markdown(html),
            "> Said\n>\n> twice\n\n```\nfn main() {\n    go();\n}\n```\n\n![A](a.png)"
        );
    }

    #[test]
    fn tables_become_pipe_tables() {
        let html = "<table><tr><th>Name</th><th>Note</th></tr><tr><td>a|b</td><td><p>x</p><p>y</p></td></tr>\
                    <tr><td>only</td></tr></table>";
        assert_eq!(
            html_to_markdown(html),
            "| Name | Note |\n| --- | --- |\n| a\\|b | x y |\n| only |  |"
        );
    }

    #[test]
    fn character_references_decode() {
        assert_eq!(decode("&lt;&#65;&#x42;&nbsp;&bogus; & a"), "<AB\u{a0}&bogus; & a");
        assert_eq!(html_to_markdown("a < b and c &gt; d"), "a < b and c > d");
    }

    #[test]
    fn json_is_tidied() {
        assert_eq!(
            to_json(" {\"a\":[1,2]} ").unwrap(),
            "{\n  \"a\": [\n    1,\n    2\n  ]\n}"
        );
        assert!(to_json("42").is_none());
        assert!(to_json("just words").is_none());
    }

    #[test]
    fn tables_become_lists_of_objects() {
        let csv = "name,age,zip,ok\r\n\"Doe, Jane\",41,02134,true\nBob,7.5,,no\n";
        assert_eq!(
            to_json(csv).unwrap(),
            "[\n  {\n    \"name\": \"Doe, Jane\",\n    \"age\": 41,\n    \"zip\": \"02134\",\n    \"ok\": true\n  },\n  \
             {\n    \"name\": \"Bob\",\n    \"age\": 7.5,\n    \"zip\": \"\",\n    \"ok\": \"no\"\n  }\n]"
        );
        assert!(to_json("a\tb\n1\t2").unwrap().contains("\"b\": 2"));
        assert!(to_json("a,b\n1,2,3").is_none(), "ragged rows");
        assert!(to_json("a,b").is_none(), "headings only");
        assert!(to_json("a,\"b\n1,2").is_none(), "unclosed quote");
    }

    fn dib(bits: u16, height: i32, compression: u32, extra: &[u8], pixels: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(40u32.to_le_bytes());
        d.extend(2i32.to_le_bytes());
        d.extend(height.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(bits.to_le_bytes());
        d.extend(compression.to_le_bytes());
        d.extend([0u8; 20]);
        d.extend(extra);
        d.extend(pixels);
        d
    }

    #[test]
    fn bottom_up_24_bit_pictures_are_read() {
        // Two rows of two pixels, each row padded to 8 bytes; the bottom row comes first.
        let px = [0, 0, 255, 0, 255, 0, 0, 0, 255, 0, 0, 255, 255, 255, 0, 0];
        let p = dib_picture(&dib(24, 2, 0, &[], &px)).unwrap();
        assert_eq!((p.width, p.height), (2, 2));
        assert_eq!(p.pixels, vec![0xFF0000FF, 0xFFFFFFFF, 0xFFFF0000, 0xFF00FF00]);
    }

    #[test]
    fn top_down_32_bit_pictures_keep_or_add_alpha() {
        let px = [1, 2, 3, 0, 4, 5, 6, 0];
        let p = dib_picture(&dib(32, -1, 0, &[], &px)).unwrap();
        assert_eq!(p.pixels, vec![0xFF030201, 0xFF060504]);
        let px = [1, 2, 3, 0x80, 4, 5, 6, 0];
        let masks: Vec<u8> = [0xFF0000u32, 0xFF00, 0xFF]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        let p = dib_picture(&dib(32, -1, 3, &masks, &px)).unwrap();
        assert_eq!(p.pixels, vec![0x80030201, 0x00060504]);
    }

    #[test]
    fn paletted_pictures_and_bad_input() {
        let palette: Vec<u8> = [0x000000u32, 0xFFFFFF].iter().flat_map(|c| c.to_le_bytes()).collect();
        let p = dib_picture(&dib(1, -1, 0, &palette, &[0b0100_0000, 0, 0, 0])).unwrap();
        assert_eq!(p.pixels, vec![0xFF000000, 0xFFFFFFFF]);
        assert!(dib_picture(&dib(24, 2, 0, &[], &[0; 4])).is_err());
        assert!(dib_picture(&dib(24, 1, 1, &[], &[0; 8])).is_err());
        assert!(dib_picture(&[1, 2]).is_err());
    }
}
