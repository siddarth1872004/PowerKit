//! Rich Text, which the preview pane shows in a read-only RichEdit control: it selects,
//! copies and scrolls by itself.

/// Colors of the preview pane, from Explorer when it says them, otherwise from the
/// Windows app theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub background: [u8; 3],
    pub text: [u8; 3],
}

impl Theme {
    pub const LIGHT: Theme = Theme {
        background: [255, 255, 255],
        text: [30, 30, 30],
    };
    pub const DARK: Theme = Theme {
        background: [32, 32, 32],
        text: [230, 230, 230],
    };

    pub fn dark(&self) -> bool {
        let [r, g, b] = self.background.map(u32::from);
        r * 299 + g * 587 + b * 114 < 128_000
    }
}

/// Color slots in the color table every document here starts with; `\cfN` picks one.
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Color {
    Text = 1,
    Muted,
    Link,
    CodeBack,
    Keyword,
    Str,
    Comment,
    Number,
    Tag,
}

/// The document header: fonts (`\f0` Segoe UI, `\f1` Consolas) and colors.
pub fn header(theme: Theme) -> String {
    let rgb = |[r, g, b]: [u8; 3]| format!("\\red{r}\\green{g}\\blue{b};");
    let colors: [[u8; 3]; 9] = if theme.dark() {
        [
            theme.text,
            [150, 150, 150],
            [100, 170, 255],
            [50, 50, 50],
            [86, 156, 214],
            [206, 145, 120],
            [106, 153, 85],
            [181, 206, 168],
            [78, 201, 176],
        ]
    } else {
        [
            theme.text,
            [110, 110, 110],
            [0, 102, 204],
            [240, 240, 240],
            [0, 0, 255],
            [163, 21, 21],
            [0, 128, 0],
            [9, 134, 88],
            [128, 0, 0],
        ]
    };
    let table: String = colors.into_iter().map(rgb).collect();
    format!("{{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1{{\\fonttbl{{\\f0\\fswiss Segoe UI;}}{{\\f1\\fmodern Consolas;}}}}{{\\colortbl ;{table}}}\\viewkind4\\f0\\fs20\\cf1 ")
}

/// Plain text made safe for RTF: control characters escaped and anything past ASCII
/// as `\uN?`, in UTF-16 units as RTF wants.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\tab "),
            '\r' | '\n' => {}
            ' '..='~' => out.push(c),
            _ if (c as u32) < 0x20 => {}
            _ => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_braces_and_unicode() {
        assert_eq!(escape("a{b}\\c"), "a\\{b\\}\\\\c");
        assert_eq!(escape("é"), "\\u233?");
        assert_eq!(escape("😀"), "\\u-10179?\\u-8704?");
        assert_eq!(escape("a\tb\r\n\u{1}"), "a\\tab b");
    }

    #[test]
    fn the_theme_decides_dark_from_the_background() {
        assert!(Theme::DARK.dark());
        assert!(!Theme::LIGHT.dark());
        assert!(header(Theme::DARK).starts_with("{\\rtf1"));
    }
}
