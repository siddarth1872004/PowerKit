//! Calculator: arithmetic expressions with `+ - * / % ^`, parentheses, constants
//! (`pi`, `e`) and common functions. Evaluated in f64; no variables, no side effects.

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    Open,
    Close,
    Comma,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '0'..='9' | '.' => {
                if c == '0' && matches!(chars.get(i + 1), Some('x' | 'X')) {
                    let start = i + 2;
                    let mut j = start;
                    while j < chars.len() && chars[j].is_ascii_hexdigit() {
                        j += 1;
                    }
                    let digits: String = chars[start..j].iter().collect();
                    out.push(Tok::Num(u64::from_str_radix(&digits, 16).ok()? as f64));
                    i = j;
                    continue;
                }
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_') {
                    i += 1;
                }
                if i < chars.len() && matches!(chars[i], 'e' | 'E') {
                    let mut j = i + 1;
                    if matches!(chars.get(j), Some('+' | '-')) {
                        j += 1;
                    }
                    if chars.get(j).is_some_and(|d| d.is_ascii_digit()) {
                        i = j;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
                out.push(Tok::Num(text.parse().ok()?));
            }
            'a'..='z' | 'A'..='Z' | 'π' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == 'π') {
                    i += 1;
                }
                let name = chars[start..i].iter().collect::<String>().to_lowercase();
                out.push(if name == "x" { Tok::Op('*') } else { Tok::Ident(name) });
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                // `**` is a common alternative spelling of `^`.
                if c == '*' && chars.get(i + 1) == Some(&'*') {
                    out.push(Tok::Op('^'));
                    i += 2;
                    continue;
                }
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: u32,
}

const MAX_DEPTH: u32 = 64;

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect(&mut self, t: Tok) -> Option<()> {
        (self.next()? == t).then_some(())
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            match self.peek().cloned() {
                Some(Tok::Op(op @ ('*' | '/' | '%'))) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    v = match op {
                        '*' => v * r,
                        '/' => v / r,
                        _ => v % r,
                    };
                }
                // Implicit multiplication: `2pi`, `3(4+1)`, `2 sqrt(2)`.
                Some(Tok::Open | Tok::Ident(_) | Tok::Num(_)) => v *= self.unary()?,
                _ => return Some(v),
            }
        }
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return None;
            }
            // Right associative, and binds tighter than a leading minus: -2^2 = -4.
            let exp = self.unary()?;
            self.depth -= 1;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn atom(&mut self) -> Option<f64> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        let v = match self.next()? {
            Tok::Num(n) => n,
            Tok::Open => {
                let v = self.expr()?;
                self.expect(Tok::Close)?;
                v
            }
            Tok::Ident(name) => match name.as_str() {
                "pi" | "π" => std::f64::consts::PI,
                "e" => std::f64::consts::E,
                "tau" => std::f64::consts::TAU,
                _ => {
                    self.expect(Tok::Open)?;
                    let mut args = vec![self.expr()?];
                    while self.peek() == Some(&Tok::Comma) {
                        self.pos += 1;
                        args.push(self.expr()?);
                    }
                    self.expect(Tok::Close)?;
                    call(&name, &args)?
                }
            },
            _ => return None,
        };
        self.depth -= 1;
        Some(v)
    }
}

fn call(name: &str, a: &[f64]) -> Option<f64> {
    let one = |f: fn(f64) -> f64| (a.len() == 1).then(|| f(a[0]));
    match name {
        "sqrt" => one(f64::sqrt),
        "cbrt" => one(f64::cbrt),
        "abs" => one(f64::abs),
        "ln" => one(f64::ln),
        "log" | "log10" => one(f64::log10),
        "log2" => one(f64::log2),
        "exp" => one(f64::exp),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "round" => one(f64::round),
        "min" if !a.is_empty() => Some(a.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" if !a.is_empty() => Some(a.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        "pow" if a.len() == 2 => Some(a[0].powf(a[1])),
        _ => None,
    }
}

/// Evaluate `input`. Returns `None` for anything that is not a complete, finite
/// expression, and for a bare number (nothing to calculate).
pub fn eval(input: &str) -> Option<f64> {
    let input = input.trim().trim_start_matches('=');
    if input.len() > 512 {
        return None;
    }
    let toks = tokenize(input)?;
    let has_work = toks.len() > 1 || matches!(toks.first(), Some(Tok::Ident(_)));
    if !has_work {
        return None;
    }
    let mut p = Parser { toks, pos: 0, depth: 0 };
    let v = p.expr()?;
    (p.pos == p.toks.len() && v.is_finite()).then_some(v)
}

/// Format with up to 12 significant digits and no trailing zeros.
pub fn format(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let mag = v.abs().log10().floor() as i32;
    if !(-6..15).contains(&mag) {
        let s = format!("{v:.11e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        return format!("{m}e{e}");
    }
    let decimals = (11 - mag).clamp(0, 15) as usize;
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(s: &str) -> String {
        eval(s).map(format).unwrap_or_else(|| "none".into())
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(calc("1+2*3"), "7");
        assert_eq!(calc("(1+2)*3"), "9");
        assert_eq!(calc("2^3^2"), "512");
        assert_eq!(calc("-2^2"), "-4");
        assert_eq!(calc("10 % 4"), "2");
        assert_eq!(calc("7/2"), "3.5");
        assert_eq!(calc("0.1+0.2"), "0.3");
        assert_eq!(calc("2 ** 10"), "1024");
        assert_eq!(calc("3 x 4"), "12");
        assert_eq!(calc("=1_000*3"), "3000");
        assert_eq!(calc("0xff + 1"), "256");
    }

    #[test]
    fn functions_constants_and_implicit_multiplication() {
        assert_eq!(calc("sqrt(16)"), "4");
        assert_eq!(calc("2pi"), format(std::f64::consts::TAU));
        assert_eq!(calc("max(1, 5, 3)"), "5");
        assert_eq!(calc("3(4+1)"), "15");
        assert_eq!(calc("1e3 / 4"), "250");
        assert_eq!(calc("pi"), format(std::f64::consts::PI));
    }

    #[test]
    fn rejects_non_expressions() {
        for s in [
            "",
            "42",
            "notepad",
            "1+",
            "(1",
            "1/0",
            "sqrt(-1)",
            "foo(1)",
            "1 2 +",
            "chrome.exe",
        ] {
            assert_eq!(calc(s), "none", "{s:?}");
        }
        assert_eq!(calc(&"(".repeat(500)), "none");
        assert_eq!(calc(&"2^".repeat(500)), "none");
    }

    #[test]
    fn formatting() {
        assert_eq!(format(1e20), "1e20");
        assert_eq!(format(1.5e-9), "1.5e-9");
        assert_eq!(format(-0.0), "0");
        assert_eq!(format(1.0 / 3.0), "0.333333333333");
    }
}
