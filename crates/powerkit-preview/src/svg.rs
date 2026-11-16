//! Readies an SVG for drawing at any size: Direct2D draws the root `<svg>` at its own
//! width and height, so those are taken out and a `viewBox` made from them if there is
//! none, which makes the picture fill the size asked for.

/// The SVG with its root tag rewritten, and the picture's width and height.
pub fn normalize(svg: &str) -> Option<(String, f32, f32)> {
    let start = root_start(svg)?;
    let end = start + tag_end(&svg[start..])?;
    let tag = &svg[start..end];
    let attrs = attributes(&tag[4..])?;
    let get = |name: &str| attrs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
    let length = |name: &str| get(name).and_then(parse_length);
    let viewbox = get("viewBox").and_then(|v| {
        let n: Vec<f32> = v
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        match n[..] {
            [x, y, w, h] if w > 0.0 && h > 0.0 => Some((x, y, w, h)),
            _ => None,
        }
    });
    let (w, h) = match (viewbox, length("width"), length("height")) {
        (_, Some(w), Some(h)) => (w, h),
        (Some((_, _, vw, vh)), Some(w), None) => (w, w * vh / vw),
        (Some((_, _, vw, vh)), None, Some(h)) => (h * vw / vh, h),
        (Some((_, _, vw, vh)), None, None) => (vw, vh),
        (None, w, h) => (w.unwrap_or(300.0), h.unwrap_or(150.0)),
    };
    let mut out = String::with_capacity(svg.len() + 40);
    out.push_str(&svg[..start]);
    out.push_str("<svg");
    for (name, value) in &attrs {
        if *name != "width" && *name != "height" {
            let quote = if value.contains('"') { '\'' } else { '"' };
            out.push_str(&format!(" {name}={quote}{value}{quote}"));
        }
    }
    if viewbox.is_none() {
        out.push_str(&format!(" viewBox=\"0 0 {w} {h}\""));
    }
    out.push_str(if tag.ends_with("/>") { "/>" } else { ">" });
    out.push_str(&svg[end..]);
    Some((out, w, h))
}

/// Where the root `<svg` tag starts, past the XML declaration, comments and doctype.
fn root_start(svg: &str) -> Option<usize> {
    let mut at = 0;
    loop {
        let i = at + svg[at..].find('<')?;
        let rest = &svg[i..];
        if rest.starts_with("<!--") {
            at = i + rest.find("-->")? + 3;
        } else if rest.starts_with("<svg") && rest[4..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/')
        {
            return Some(i);
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            at = i + 2;
        } else {
            return None;
        }
    }
}

/// Length of the tag at the start of `s`, `>` included, skipping `>` in quoted values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

fn attributes(s: &str) -> Option<Vec<(&str, &str)>> {
    let mut attrs = Vec::new();
    let mut rest = s.trim_end_matches('>').trim_end_matches('/');
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Some(attrs);
        }
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let close = after[1..].find(quote)?;
        attrs.push((name, &after[1..1 + close]));
        rest = &after[close + 2..];
    }
}

/// A length in pixels; percentages give `None` (they need a container).
fn parse_length(v: &str) -> Option<f32> {
    let v = v.trim();
    let split = v.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(v.len());
    let n: f32 = v[..split].trim().parse().ok()?;
    let px = match &v[split..] {
        "" | "px" => n,
        "pt" => n * 4.0 / 3.0,
        "pc" => n * 16.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "em" => n * 16.0,
        _ => return None,
    };
    (px > 0.0).then_some(px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_comes_from_width_and_height_which_are_removed() {
        let (out, w, h) =
            normalize("<?xml version=\"1.0\"?>\n<!-- a > b --><svg width=\"2in\" height='50' xmlns=\"x\"><g/></svg>")
                .unwrap();
        assert_eq!((w, h), (192.0, 50.0));
        assert_eq!(
            out,
            "<?xml version=\"1.0\"?>\n<!-- a > b --><svg xmlns=\"x\" viewBox=\"0 0 192 50\"><g/></svg>"
        );
    }

    #[test]
    fn a_view_box_is_kept_and_gives_the_aspect() {
        let (out, w, h) = normalize("<svg viewBox=\"0,0,40,20\" width=\"100%\" data-x=\"a>b\">").unwrap();
        assert_eq!((w, h), (40.0, 20.0));
        assert_eq!(out, "<svg viewBox=\"0,0,40,20\" data-x=\"a>b\">");
        let (_, w, h) = normalize("<svg viewBox=\"0 0 40 20\" width=\"80\"/>").unwrap();
        assert_eq!((w, h), (80.0, 40.0));
    }

    #[test]
    fn no_size_at_all_uses_the_html_default() {
        let (out, w, h) = normalize("<!DOCTYPE svg><svg></svg>").unwrap();
        assert_eq!((w, h), (300.0, 150.0));
        assert!(out.contains("viewBox=\"0 0 300 150\""));
        assert!(normalize("<html><svg/></html>").is_none());
    }
}
