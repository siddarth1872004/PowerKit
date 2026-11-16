//! Source code as colored Rich Text. A small tokenizer per language family colors
//! comments, strings, numbers, keywords and names (tags, keys, variables); it does not
//! parse, so unusual code may be colored loosely but is never changed.

use crate::rtf::{escape, header, Color, Theme};

#[derive(Debug)]
pub struct Lang {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static str,
    keywords: &'static [&'static str],
    ignore_case: bool,
    /// `<tag>` names are colored (HTML, XML).
    markup: bool,
    /// A name or string before `=` or `:` is a key; a line in `[...]` a section.
    keys: bool,
    /// `$name` is a variable.
    dollar: bool,
}

const PLAIN: Lang = Lang {
    line: &[],
    block: None,
    quotes: "",
    keywords: &[],
    ignore_case: false,
    markup: false,
    keys: false,
    dollar: false,
};

const CLIKE_WORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "crate",
    "def",
    "default",
    "delete",
    "do",
    "double",
    "dyn",
    "else",
    "enum",
    "export",
    "extends",
    "extern",
    "f32",
    "f64",
    "false",
    "final",
    "finally",
    "float",
    "fn",
    "for",
    "foreach",
    "from",
    "fun",
    "func",
    "function",
    "go",
    "i16",
    "i32",
    "i64",
    "i8",
    "if",
    "impl",
    "implements",
    "import",
    "in",
    "instanceof",
    "int",
    "interface",
    "internal",
    "isize",
    "let",
    "long",
    "loop",
    "match",
    "mod",
    "module",
    "move",
    "mut",
    "namespace",
    "new",
    "nil",
    "null",
    "object",
    "operator",
    "override",
    "package",
    "private",
    "protected",
    "pub",
    "public",
    "readonly",
    "ref",
    "return",
    "self",
    "Self",
    "short",
    "signed",
    "sizeof",
    "static",
    "str",
    "string",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "throws",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "u16",
    "u32",
    "u64",
    "u8",
    "uint",
    "undefined",
    "union",
    "unsafe",
    "unsigned",
    "use",
    "using",
    "usize",
    "val",
    "var",
    "virtual",
    "void",
    "volatile",
    "when",
    "where",
    "while",
    "yield",
];

const CLIKE: Lang = Lang {
    line: &["//"],
    block: Some(("/*", "*/")),
    quotes: "\"'`",
    keywords: CLIKE_WORDS,
    ..PLAIN
};

fn lang(name: &str) -> Lang {
    match name.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "c++" | "cs" | "csharp" | "c#" | "java" | "kt" | "kts"
        | "kotlin" | "go" | "golang" | "js" | "mjs" | "cjs" | "jsx" | "javascript" | "ts" | "tsx" | "typescript"
        | "rs" | "rust" | "swift" | "dart" | "scala" | "gradle" | "groovy" => CLIKE,
        "php" => Lang {
            line: &["//", "#"],
            dollar: true,
            ..CLIKE
        },
        "json" | "jsonc" | "json5" => Lang {
            keywords: &["true", "false", "null"],
            keys: true,
            ..CLIKE
        },
        "css" | "scss" | "less" => Lang { keywords: &[], ..CLIKE },
        "py" | "pyw" | "python" => Lang {
            line: &["#"],
            quotes: "\"'",
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
                "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None",
                "nonlocal", "not", "or", "pass", "raise", "return", "self", "True", "try", "while", "with", "yield",
            ],
            ..PLAIN
        },
        "sh" | "bash" | "zsh" | "shell" | "console" => Lang {
            line: &["#"],
            quotes: "\"'",
            keywords: &[
                "case", "do", "done", "echo", "elif", "else", "esac", "exit", "export", "false", "fi", "for",
                "function", "if", "in", "local", "readonly", "return", "then", "true", "until", "while",
            ],
            dollar: true,
            ..PLAIN
        },
        "ps1" | "psm1" | "psd1" | "powershell" | "pwsh" => Lang {
            line: &["#"],
            block: Some(("<#", "#>")),
            quotes: "\"'",
            keywords: &[
                "begin", "break", "catch", "class", "continue", "do", "else", "elseif", "end", "exit", "filter",
                "finally", "for", "foreach", "function", "if", "in", "param", "process", "return", "switch", "throw",
                "trap", "try", "until", "using", "while",
            ],
            ignore_case: true,
            dollar: true,
            ..PLAIN
        },
        "bat" | "cmd" | "batch" => Lang {
            line: &["::", "rem ", "REM ", "@rem "],
            quotes: "\"",
            keywords: &[
                "call",
                "defined",
                "do",
                "echo",
                "else",
                "endlocal",
                "errorlevel",
                "exist",
                "exit",
                "for",
                "goto",
                "if",
                "in",
                "not",
                "pause",
                "set",
                "setlocal",
            ],
            ignore_case: true,
            ..PLAIN
        },
        "sql" => Lang {
            line: &["--"],
            block: Some(("/*", "*/")),
            quotes: "'\"",
            keywords: &[
                "add",
                "all",
                "alter",
                "and",
                "as",
                "asc",
                "begin",
                "between",
                "by",
                "case",
                "commit",
                "create",
                "default",
                "delete",
                "desc",
                "distinct",
                "drop",
                "else",
                "end",
                "exists",
                "foreign",
                "from",
                "group",
                "having",
                "in",
                "index",
                "inner",
                "insert",
                "into",
                "is",
                "join",
                "key",
                "left",
                "like",
                "limit",
                "not",
                "null",
                "offset",
                "on",
                "or",
                "order",
                "outer",
                "primary",
                "references",
                "right",
                "rollback",
                "select",
                "set",
                "table",
                "then",
                "union",
                "unique",
                "update",
                "values",
                "view",
                "when",
                "where",
            ],
            ignore_case: true,
            ..PLAIN
        },
        "lua" => Lang {
            line: &["--"],
            block: Some(("--[[", "]]")),
            quotes: "\"'",
            keywords: &[
                "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local",
                "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
            ],
            ..PLAIN
        },
        "rb" | "ruby" => Lang {
            line: &["#"],
            quotes: "\"'",
            keywords: &[
                "alias", "and", "begin", "break", "case", "class", "def", "do", "else", "elsif", "end", "ensure",
                "false", "for", "if", "in", "module", "next", "nil", "not", "or", "redo", "require", "rescue", "retry",
                "return", "self", "super", "then", "true", "undef", "unless", "until", "when", "while", "yield",
            ],
            ..PLAIN
        },
        "r" => Lang {
            line: &["#"],
            quotes: "\"'",
            keywords: &[
                "break", "else", "FALSE", "for", "function", "if", "in", "Inf", "library", "NA", "NaN", "next", "NULL",
                "repeat", "return", "TRUE", "while",
            ],
            ..PLAIN
        },
        "toml" | "ini" | "cfg" | "conf" | "yaml" | "yml" | "properties" | "editorconfig" => Lang {
            line: &["#", ";"],
            quotes: "\"'",
            keywords: &["true", "false", "null", "yes", "no", "on", "off"],
            ignore_case: true,
            keys: true,
            ..PLAIN
        },
        "html" | "htm" | "xml" | "xaml" | "vue" | "svg" | "csproj" | "props" | "targets" => Lang {
            block: Some(("<!--", "-->")),
            quotes: "\"'",
            markup: true,
            ..PLAIN
        },
        _ => PLAIN,
    }
}

/// Whether a fenced block's info string or a file extension names a language colored here.
pub fn known(name: &str) -> bool {
    let l = lang(name);
    !(l.line.is_empty() && l.block.is_none() && l.quotes.is_empty())
}

/// The lines of `src`, each split into runs with the color they get (`None` is text).
pub fn spans(src: &str, name: &str) -> Vec<Vec<(Option<Color>, String)>> {
    let lang = lang(name);
    let mut open: Option<Open> = None;
    src.lines().map(|line| line_spans(line, &lang, &mut open)).collect()
}

/// What carries on into the next line.
#[derive(Clone, Copy)]
enum Open {
    Comment(&'static str),
    Backtick,
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn line_spans(line: &str, lang: &Lang, open: &mut Option<Open>) -> Vec<(Option<Color>, String)> {
    let mut out: Vec<(Option<Color>, String)> = Vec::new();
    let mut push = |color: Option<Color>, text: &str| {
        if text.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((c, t)) if c.map(|c| c as u8) == color.map(|c| c as u8) => t.push_str(text),
            _ => out.push((color, text.to_string())),
        }
    };
    let mut i = 0;
    let mut tag_next = false;
    let trimmed = line.trim();
    if lang.keys && trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.contains('=') {
        push(Some(Color::Keyword), line);
        return out;
    }
    let key_end = lang.keys.then(|| {
        line.find(['=', ':'])
            .filter(|&e| {
                let k = line[..e].trim().trim_start_matches("- ");
                !k.is_empty() && !k.contains(['"', '\'', '#', ';', ' ']) || k.starts_with('"') && k.ends_with('"')
            })
            .unwrap_or(0)
    });
    while i < line.len() {
        let rest = &line[i..];
        match *open {
            Some(Open::Comment(end)) => {
                let stop = rest.find(end).map_or(rest.len(), |p| p + end.len());
                push(Some(Color::Comment), &rest[..stop]);
                i += stop;
                if stop < rest.len() || rest.ends_with(end) {
                    *open = None;
                }
                continue;
            }
            Some(Open::Backtick) => {
                let stop = string_end(rest, '`').unwrap_or(rest.len());
                push(Some(Color::Str), &rest[..stop]);
                i += stop;
                if stop < rest.len() || rest.ends_with('`') {
                    *open = None;
                }
                continue;
            }
            None => {}
        }
        if let Some(k) = key_end.filter(|&k| i < k) {
            let lead = line[..k].len() - line[..k].trim_start().len();
            push(None, &line[i..lead.max(i)]);
            push(Some(Color::Tag), line[lead.max(i)..k].trim_end());
            push(None, &line[line[..k].trim_end().len().max(i)..k]);
            i = k;
            continue;
        }
        if lang.line.iter().any(|m| rest.starts_with(m)) {
            push(Some(Color::Comment), rest);
            break;
        }
        if let Some((start, end)) = lang.block.filter(|(s, _)| rest.starts_with(s)) {
            *open = Some(Open::Comment(end));
            push(Some(Color::Comment), start);
            i += start.len();
            continue;
        }
        let c = rest.chars().next().expect("i is inside the line");
        if lang.quotes.contains(c) {
            let stop = string_end(&rest[1..], c).map(|e| e + 1);
            if stop.is_none() && c == '`' {
                *open = Some(Open::Backtick);
            }
            let stop = stop.unwrap_or(rest.len());
            push(Some(Color::Str), &rest[..stop]);
            i += stop;
            continue;
        }
        if c.is_ascii_digit() && !line[..i].ends_with(word_char) {
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_'))
                .unwrap_or(rest.len());
            push(Some(Color::Number), &rest[..len]);
            i += len;
            continue;
        }
        if word_char(c) || (lang.dollar && c == '$') {
            let body = &rest[c.len_utf8()..];
            let len = c.len_utf8()
                + body
                    .find(|c: char| !(word_char(c) || c == '-' && lang.markup))
                    .unwrap_or(body.len());
            let word = &rest[..len];
            let color = if lang.dollar && c == '$' || tag_next {
                Some(Color::Tag)
            } else if lang.keywords.iter().any(|k| {
                if lang.ignore_case {
                    k.eq_ignore_ascii_case(word)
                } else {
                    *k == word
                }
            }) {
                Some(Color::Keyword)
            } else {
                None
            };
            push(color, word);
            tag_next = false;
            i += len;
            continue;
        }
        tag_next = lang.markup && (c == '<' || c == '/' && tag_next);
        push(None, &rest[..c.len_utf8()]);
        i += c.len_utf8();
    }
    out
}

/// Where a string that started just before `s` ends, closing quote included.
fn string_end(s: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return Some(i + c.len_utf8());
        }
    }
    None
}

/// The runs of one line as RTF, without a paragraph end.
pub fn line_rtf(runs: &[(Option<Color>, String)]) -> String {
    runs.iter()
        .map(|(color, text)| match color {
            Some(c) => format!("{{\\cf{} {}}}", *c as u8, escape(text)),
            None => escape(text),
        })
        .collect()
}

/// A whole file as a document; `cut` says the file was longer than what was read.
pub fn to_rtf(src: &str, name: &str, theme: Theme, cut: bool) -> String {
    let mut out = header(theme);
    out.push_str("\\f1\\fs19 ");
    for runs in spans(src, name) {
        out.push_str(&line_rtf(&runs));
        out.push_str("\\par\n");
    }
    if cut {
        out.push_str(&format!(
            "\\f0{{\\cf{} \\i Only the first {} KB are shown.}}\\par\n",
            Color::Muted as u8,
            crate::Format::Code.read_limit() / 1024
        ));
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(line: &str, name: &str) -> Vec<(u8, String)> {
        spans(line, name)
            .remove(0)
            .into_iter()
            .map(|(c, t)| (c.map_or(0, |c| c as u8), t))
            .collect()
    }

    #[test]
    fn colors_keywords_strings_numbers_and_comments() {
        let k = Color::Keyword as u8;
        let s = Color::Str as u8;
        let n = Color::Number as u8;
        let c = Color::Comment as u8;
        assert_eq!(
            colors("let x1 = \"a\\\"b\" + 42; // done", "rs"),
            vec![
                (k, "let".into()),
                (0, " x1 = ".into()),
                (s, "\"a\\\"b\"".into()),
                (0, " + ".into()),
                (n, "42".into()),
                (0, "; ".into()),
                (c, "// done".into()),
            ]
        );
    }

    #[test]
    fn block_comments_and_template_strings_span_lines() {
        let lines = spans("a /* one\ntwo */ b\nx = `p\nq` y", "js");
        let c = |i: usize| {
            lines[i]
                .iter()
                .map(|(c, t)| (c.map_or(0, |c| c as u8), t.as_str()))
                .collect::<Vec<_>>()
        };
        let (cm, st) = (Color::Comment as u8, Color::Str as u8);
        assert_eq!(c(0), vec![(0, "a "), (cm, "/* one")]);
        assert_eq!(c(1), vec![(cm, "two */"), (0, " b")]);
        assert_eq!(c(2), vec![(0, "x = "), (st, "`p")]);
        assert_eq!(c(3), vec![(st, "q`"), (0, " y")]);
    }

    #[test]
    fn config_keys_sections_and_markup_tags() {
        let tag = Color::Tag as u8;
        assert_eq!(
            colors("[package]", "toml"),
            vec![(Color::Keyword as u8, "[package]".into())]
        );
        assert_eq!(
            colors("  name = \"x\" # c", "toml"),
            vec![
                (0, "  ".into()),
                (tag, "name".into()),
                (0, " = ".into()),
                (Color::Str as u8, "\"x\"".into()),
                (0, " ".into()),
                (Color::Comment as u8, "# c".into())
            ]
        );
        assert_eq!(
            colors("<div class=\"a\"></div>", "html"),
            vec![
                (0, "<".into()),
                (tag, "div".into()),
                (0, " class=".into()),
                (Color::Str as u8, "\"a\"".into()),
                (0, "></".into()),
                (tag, "div".into()),
                (0, ">".into()),
            ]
        );
        assert_eq!(colors("$Path", "ps1"), vec![(tag, "$Path".into())]);
        assert_eq!(
            colors("IF exist", "bat"),
            vec![
                (Color::Keyword as u8, "IF".into()),
                (0, " ".into()),
                (Color::Keyword as u8, "exist".into())
            ]
        );
    }

    #[test]
    fn unknown_languages_are_plain_and_documents_are_closed() {
        assert!(!known("brainfuck"));
        assert!(known("rust") && known(".toml"));
        let doc = to_rtf("a{b}\n", "zz", Theme::LIGHT, true);
        assert!(doc.contains("a\\{b\\}\\par"));
        assert!(doc.contains("Only the first 1024 KB"));
        assert!(doc.ends_with('}'));
    }
}
