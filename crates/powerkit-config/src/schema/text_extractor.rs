use super::fix_shortcut;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextExtractor {
    pub enabled: bool,
    /// Starts a selection; the text in the selected area is copied.
    pub shortcut: String,
    /// Language tag of the text to read (`en-US`); empty uses the Windows display
    /// languages.
    pub language: String,
    /// Copy all the text as one line, joined with spaces.
    pub single_line: bool,
}

impl Default for TextExtractor {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Win+Shift+T".into(),
            language: String::new(),
            single_line: false,
        }
    }
}

impl TextExtractor {
    pub(crate) fn sanitize(&mut self, w: &mut Vec<String>) {
        fix_shortcut(
            &mut self.shortcut,
            &TextExtractor::default().shortcut,
            "Text Extractor",
            w,
        );
        let tag = self.language.trim();
        if !tag.is_empty() && !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            w.push(format!(
                "text_extractor.language {:?} is not a language tag such as en-US; using the Windows languages",
                self.language
            ));
            self.language.clear();
        } else if tag.len() != self.language.len() {
            self.language = tag.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_language_tags_fall_back() {
        let mut t = TextExtractor {
            language: "en US!".into(),
            ..TextExtractor::default()
        };
        let mut w = Vec::new();
        t.sanitize(&mut w);
        assert_eq!((t.language.as_str(), w.len()), ("", 1));
        t.language = " de-DE ".into();
        t.sanitize(&mut w);
        assert_eq!((t.language.as_str(), w.len()), ("de-DE", 1));
    }
}
