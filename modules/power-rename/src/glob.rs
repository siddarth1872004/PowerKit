//! Name filters like `*.jpg;*.png`: `;`-separated patterns, `*` and `?` wildcards,
//! case-insensitive because Windows file names are.

#[derive(Debug, Clone, Default)]
pub struct Glob {
    patterns: Vec<Vec<char>>,
}

impl Glob {
    pub fn new(spec: &str) -> Glob {
        let patterns = spec
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| p.chars().flat_map(char::to_lowercase).collect())
            .collect();
        Glob { patterns }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        let name: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
        self.patterns.iter().any(|p| wildcard(p, &name))
    }
}

/// Iterative matcher with single-star backtracking: linear in practice, never exponential.
fn wildcard(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching() {
        let g = Glob::new("*.jpg; *.PNG ;");
        assert!(g.matches("Photo.JPG"));
        assert!(g.matches("a.png"));
        assert!(!g.matches("a.png.txt"));
        assert!(!g.matches("jpg"));
        assert!(Glob::new("IMG_????.*").matches("img_0001.heic"));
        assert!(!Glob::new("IMG_????.*").matches("img_001.heic"));
        assert!(Glob::new("*a*b*").matches("xxaxxbxx"));
        assert!(!Glob::new("*a*b").matches("xxaxxbxx"));
        assert!(Glob::new("").is_empty());
        assert!(Glob::new("*").matches(""));
    }
}
