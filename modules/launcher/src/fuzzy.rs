//! Fuzzy matching: the query's characters must appear in order in the candidate.
//! Matches at word starts, consecutive runs and prefixes score higher; gaps and long
//! candidates score lower, so "vsc" ranks "Visual Studio Code" above "vsconfig.bak".

const MATCH: i32 = 16;
const WORD_START: i32 = 24;
const CONSECUTIVE: i32 = 12;
const FIRST_CHAR: i32 = 20;
const GAP: i32 = 2;
const MAX_GAP_PENALTY: i32 = 30;
/// Every query character landed on a word start: an acronym such as "vsc".
const ACRONYM: i32 = 80;

fn is_boundary(prev: Option<char>, c: char) -> bool {
    match prev {
        None => true,
        Some(p) => {
            matches!(p, ' ' | '-' | '_' | '.' | '\\' | '/' | '(' | '[' | ':')
                || (p.is_lowercase() && c.is_uppercase())
                || (!p.is_ascii_digit() && c.is_ascii_digit())
        }
    }
}

/// Score `candidate` against `query`, or `None` if it does not match. An empty query
/// matches everything with score 0.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    let q: Vec<char> = query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();

    // Prefer a whole-substring hit at a word start: it is what users usually type.
    let qs: String = query.to_lowercase();
    let cand_lower: String = lower.iter().collect();
    let substring_bonus = match cand_lower.find(qs.trim()) {
        Some(0) => 60,
        Some(i) if is_boundary(cand_lower[..i].chars().last(), chars[cand_lower[..i].chars().count()]) => 40,
        Some(_) => 15,
        None => 0,
    };

    // Greedy left-to-right that jumps ahead to a word start when one is available, which
    // gives acronym-style matches their bonus.
    let mut total = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    let mut all_word_starts = true;
    let mut i = 0;
    while i < lower.len() && qi < q.len() {
        if lower[i] != q[qi] {
            i += 1;
            continue;
        }
        let consecutive = last == Some(i.wrapping_sub(1));
        if !consecutive && !is_boundary(i.checked_sub(1).map(|p| chars[p]), chars[i]) {
            let ahead = (i + 1..lower.len()).find(|&j| lower[j] == q[qi] && is_boundary(Some(chars[j - 1]), chars[j]));
            if let Some(j) = ahead {
                if remaining_matches(&lower[j + 1..], &q[qi + 1..]) {
                    i = j;
                }
            }
        }
        let mut s = MATCH;
        if i == 0 {
            s += FIRST_CHAR;
        }
        if is_boundary(i.checked_sub(1).map(|p| chars[p]), chars[i]) {
            s += WORD_START;
        } else {
            all_word_starts = false;
        }
        if last == Some(i.wrapping_sub(1)) {
            s += CONSECUTIVE;
        } else if let Some(l) = last {
            s -= ((i - l - 1) as i32 * GAP).min(MAX_GAP_PENALTY);
        }
        total += s;
        last = Some(i);
        qi += 1;
        i += 1;
    }
    if qi < q.len() {
        return None;
    }
    let length_penalty = (chars.len() as i32 - q.len() as i32).clamp(0, 40);
    let acronym = if all_word_starts && q.len() >= 2 { ACRONYM } else { 0 };
    Some(total + substring_bonus + acronym - length_penalty)
}

fn remaining_matches(hay: &[char], needle: &[char]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|n| it.any(|h| h == n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_characters_in_order() {
        assert!(score("note", "Notepad").is_some());
        assert!(score("NOTE", "notepad").is_some());
        assert!(score("eton", "Notepad").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn prefers_prefixes_word_starts_and_short_names() {
        let s = |q, c| score(q, c).unwrap();
        assert!(s("vsc", "Visual Studio Code") > s("vsc", "vsconfig.bak"));
        assert!(s("code", "Code") > s("code", "Visual Studio Code"));
        assert!(s("calc", "Calculator") > s("calc", "Graphing calc helper"));
        assert!(s("pad", "Notepad") < s("note", "Notepad"));
        assert!(s("ws", "WindowsSettings") > s("ws", "Wordpress"));
    }

    #[test]
    fn handles_non_ascii() {
        assert!(score("über", "Über Uhr").is_some());
        assert!(score("ü", "x").is_none());
    }
}
