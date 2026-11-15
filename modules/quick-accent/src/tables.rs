//! Characters offered for each key, per character set. Keys are named by the character
//! they type on a US layout. An entry is either a lowercase character, whose capital is
//! derived, or `lower|upper` where the capital cannot be derived (ß, dotless ı).

use powerkit_config::ACCENT_LANGUAGES;

type Table = &'static [(char, &'static str)];

const CATALAN: Table = &[
    ('a', "à"),
    ('c', "ç"),
    ('e', "è é"),
    ('i', "í ï"),
    ('l', "·"),
    ('o', "ò ó"),
    ('u', "ú ü"),
];
const CROATIAN: Table = &[('c', "ć č"), ('d', "đ"), ('s', "š"), ('z', "ž")];
const CURRENCY: Table = &[
    ('a', "₳ ؋"),
    ('b', "฿ ₿"),
    ('c', "¢ ₡ ₵"),
    ('d', "$ ₫"),
    ('e', "€ ₠"),
    ('f', "ƒ ₣"),
    ('g', "₲ ₾"),
    ('h', "₴"),
    ('k', "₭ ₸"),
    ('l', "£ ₤ ₺"),
    ('m', "₥ ₼"),
    ('n', "₦"),
    ('p', "₱ ₽ ₰"),
    ('r', "₹ ₨ ₽"),
    ('s', "$ ₪"),
    ('t', "₮ ₸ ₺"),
    ('w', "₩"),
    ('y', "¥"),
];
const CZECH: Table = &[
    ('a', "á"),
    ('c', "č"),
    ('d', "ď"),
    ('e', "é ě"),
    ('i', "í"),
    ('n', "ň"),
    ('o', "ó"),
    ('r', "ř"),
    ('s', "š"),
    ('t', "ť"),
    ('u', "ú ů"),
    ('y', "ý"),
    ('z', "ž"),
];
const DANISH: Table = &[('a', "å æ"), ('e', "é"), ('o', "ø")];
const DUTCH: Table = &[
    ('a', "á à ä"),
    ('e', "é è ë ê"),
    ('i', "í ï"),
    ('o', "ó ö ô"),
    ('u', "ú ü û"),
];
const ESTONIAN: Table = &[('a', "ä"), ('o', "ö õ"), ('s', "š"), ('u', "ü"), ('z', "ž")];
const FINNISH: Table = &[('a', "ä å"), ('o', "ö"), ('s', "š"), ('z', "ž")];
const FRENCH: Table = &[
    ('a', "à â æ"),
    ('c', "ç"),
    ('e', "é è ê ë €"),
    ('i', "î ï"),
    ('o', "ô œ"),
    ('u', "ù û ü"),
    ('y', "ÿ"),
];
const GERMAN: Table = &[('a', "ä"), ('e', "€"), ('o', "ö"), ('s', "ß|ẞ"), ('u', "ü")];
const GREEK: Table = &[
    ('a', "α ά"),
    ('b', "β"),
    ('c', "χ"),
    ('d', "δ"),
    ('e', "ε έ η ή"),
    ('f', "φ"),
    ('g', "γ"),
    ('i', "ι ί ϊ"),
    ('k', "κ"),
    ('l', "λ"),
    ('m', "μ"),
    ('n', "ν"),
    ('o', "ο ό ω ώ"),
    ('p', "π ψ"),
    ('r', "ρ"),
    ('s', "σ ς|Σ"),
    ('t', "τ θ"),
    ('u', "υ ύ ϋ"),
    ('x', "ξ"),
    ('z', "ζ"),
];
const HUNGARIAN: Table = &[('a', "á"), ('e', "é"), ('i', "í"), ('o', "ó ö ő"), ('u', "ú ü ű")];
const ICELANDIC: Table = &[
    ('a', "á æ"),
    ('d', "ð"),
    ('e', "é"),
    ('i', "í"),
    ('o', "ó ö"),
    ('t', "þ"),
    ('u', "ú"),
    ('y', "ý"),
];
const IRISH: Table = &[('a', "á"), ('e', "é"), ('i', "í"), ('o', "ó"), ('u', "ú")];
const ITALIAN: Table = &[('a', "à"), ('e', "è é"), ('i', "ì í"), ('o', "ò ó"), ('u', "ù ú")];
const LATVIAN: Table = &[
    ('a', "ā"),
    ('c', "č"),
    ('e', "ē"),
    ('g', "ģ"),
    ('i', "ī"),
    ('k', "ķ"),
    ('l', "ļ"),
    ('n', "ņ"),
    ('s', "š"),
    ('u', "ū"),
    ('z', "ž"),
];
const LITHUANIAN: Table = &[
    ('a', "ą"),
    ('c', "č"),
    ('e', "ę ė"),
    ('i', "į"),
    ('s', "š"),
    ('u', "ų ū"),
    ('z', "ž"),
];
const MAORI: Table = &[('a', "ā"), ('e', "ē"), ('i', "ī"), ('o', "ō"), ('u', "ū")];
const NORWEGIAN: Table = &[('a', "å æ"), ('e', "é"), ('o', "ø")];
const PINYIN: Table = &[
    ('a', "ā á ǎ à"),
    ('e', "ē é ě è"),
    ('i', "ī í ǐ ì"),
    ('o', "ō ó ǒ ò"),
    ('u', "ū ú ǔ ù ü ǖ ǘ ǚ ǜ"),
    ('v', "ü ǖ ǘ ǚ ǜ"),
];
const POLISH: Table = &[
    ('a', "ą"),
    ('c', "ć"),
    ('e', "ę"),
    ('l', "ł"),
    ('n', "ń"),
    ('o', "ó"),
    ('s', "ś"),
    ('z', "ż ź"),
];
const PORTUGUESE: Table = &[
    ('a', "á à â ã"),
    ('c', "ç"),
    ('e', "é ê"),
    ('i', "í"),
    ('o', "ó ô õ"),
    ('u', "ú ü"),
];
const ROMANIAN: Table = &[('a', "ă â"), ('i', "î"), ('s', "ș"), ('t', "ț")];
const SLOVAK: Table = &[
    ('a', "á ä"),
    ('c', "č"),
    ('d', "ď"),
    ('e', "é"),
    ('i', "í"),
    ('l', "ĺ ľ"),
    ('n', "ň"),
    ('o', "ó ô"),
    ('r', "ŕ"),
    ('s', "š"),
    ('t', "ť"),
    ('u', "ú"),
    ('y', "ý"),
    ('z', "ž"),
];
const SLOVENIAN: Table = &[('c', "č"), ('s', "š"), ('z', "ž")];
const SPANISH: Table = &[
    ('a', "á"),
    ('e', "é €"),
    ('i', "í"),
    ('n', "ñ"),
    ('o', "ó"),
    ('u', "ú ü"),
    ('1', "¡"),
    ('/', "¿"),
];
const SPECIAL: Table = &[
    ('0', "⁰ ₀ ∅ °"),
    ('1', "¹ ₁ ½ ⅓ ¼"),
    ('2', "² ₂ ⅔"),
    ('3', "³ ₃ ¾"),
    ('4', "⁴ ₄"),
    ('5', "⁵ ₅"),
    ('6', "⁶ ₆"),
    ('7', "⁷ ₇"),
    ('8', "⁸ ₈ ∞"),
    ('9', "⁹ ₉"),
    ('-', "– — − ±"),
    ('=', "≠ ≈ ≤ ≥ ±"),
    ('.', "… · •"),
    (',', "„ ‚"),
    ('/', "÷ ‽"),
    ('\'', "‘ ’ “ ” « » ‹ ›"),
    ('c', "©"),
    ('m', "µ"),
    ('o', "°"),
    ('p', "¶ π"),
    ('r', "®"),
    ('s', "§ ∑"),
    ('t', "™"),
    ('x', "×"),
];
const SWEDISH: Table = &[('a', "å ä"), ('e', "é"), ('o', "ö")];
const TURKISH: Table = &[('c', "ç"), ('g', "ğ"), ('i', "ı|İ"), ('o', "ö"), ('s', "ş"), ('u', "ü")];
const VIETNAMESE: Table = &[
    ('a', "à á â ầ ấ ẩ ẫ ậ ả ã ạ ă ằ ắ ẳ ẵ ặ"),
    ('d', "đ"),
    ('e', "è é ê ề ế ể ễ ệ ẻ ẽ ẹ"),
    ('i', "ì í ỉ ĩ ị"),
    ('o', "ò ó ô ồ ố ổ ỗ ộ ỏ õ ọ ơ ờ ớ ở ỡ ợ"),
    ('u', "ù ú ủ ũ ụ ư ừ ứ ử ữ ự"),
    ('y', "ỳ ý ỷ ỹ ỵ"),
];
const WELSH: Table = &[
    ('a', "â"),
    ('e', "ê"),
    ('i', "î"),
    ('o', "ô"),
    ('u', "û"),
    ('w', "ŵ"),
    ('y', "ŷ"),
];

fn table(language: &str) -> Table {
    match language {
        "catalan" => CATALAN,
        "croatian" => CROATIAN,
        "currency" => CURRENCY,
        "czech" => CZECH,
        "danish" => DANISH,
        "dutch" => DUTCH,
        "estonian" => ESTONIAN,
        "finnish" => FINNISH,
        "french" => FRENCH,
        "german" => GERMAN,
        "greek" => GREEK,
        "hungarian" => HUNGARIAN,
        "icelandic" => ICELANDIC,
        "irish" => IRISH,
        "italian" => ITALIAN,
        "latvian" => LATVIAN,
        "lithuanian" => LITHUANIAN,
        "maori" => MAORI,
        "norwegian" => NORWEGIAN,
        "pinyin" => PINYIN,
        "polish" => POLISH,
        "portuguese" => PORTUGUESE,
        "romanian" => ROMANIAN,
        "slovak" => SLOVAK,
        "slovenian" => SLOVENIAN,
        "spanish" => SPANISH,
        "special" => SPECIAL,
        "swedish" => SWEDISH,
        "turkish" => TURKISH,
        "vietnamese" => VIETNAMESE,
        "welsh" => WELSH,
        _ => &[],
    }
}

fn capital(entry: &str) -> String {
    if let Some((_, upper)) = entry.split_once('|') {
        return upper.to_string();
    }
    let up = entry.to_uppercase();
    // A capital that is longer than one character (ŉ → ʼN) is not what the key offers.
    if up.chars().count() == entry.chars().count() {
        up
    } else {
        entry.to_string()
    }
}

/// Characters offered for `key` across `languages`, in set order without repeats.
pub fn accents(languages: &[String], key: char, upper: bool) -> Vec<String> {
    let all = languages.iter().any(|l| l == "all");
    let chosen: Vec<&str> = if all {
        ACCENT_LANGUAGES.iter().copied().filter(|l| *l != "all").collect()
    } else {
        languages.iter().map(String::as_str).collect()
    };
    let mut out: Vec<String> = Vec::new();
    for lang in chosen {
        for (_, list) in table(lang).iter().filter(|(k, _)| *k == key) {
            for entry in list.split_whitespace() {
                let c = if upper {
                    capital(entry)
                } else {
                    entry.split('|').next().unwrap_or(entry).to_string()
                };
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn langs(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_language() {
        assert_eq!(accents(&langs(&["german"]), 'a', false), ["ä"]);
        assert_eq!(accents(&langs(&["german"]), 'a', true), ["Ä"]);
        assert_eq!(accents(&langs(&["german"]), 's', true), ["ẞ"]);
        assert_eq!(accents(&langs(&["turkish"]), 'i', false), ["ı"]);
        assert_eq!(accents(&langs(&["turkish"]), 'i', true), ["İ"]);
        assert!(accents(&langs(&["german"]), 'q', false).is_empty());
    }

    #[test]
    fn languages_merge_without_repeats() {
        let a = accents(&langs(&["french", "portuguese"]), 'a', false);
        assert_eq!(a, ["à", "â", "æ", "á", "ã"]);
        let all = accents(&langs(&["all"]), 'e', false);
        assert_eq!(all.iter().filter(|c| *c == "é").count(), 1);
        assert!(all.contains(&"€".to_string()) && all.contains(&"ế".to_string()));
    }

    #[test]
    fn symbols_have_no_capitals() {
        assert_eq!(accents(&langs(&["currency"]), 'e', true), ["€", "₠"]);
        assert_eq!(accents(&langs(&["greek"]), 's', true), ["Σ"]);
    }

    #[test]
    fn every_set_has_a_table() {
        for l in ACCENT_LANGUAGES.iter().filter(|l| **l != "all") {
            assert!(!table(l).is_empty(), "{l} has no characters");
        }
    }
}
