//! The SVGs the engines write: what could run script is taken out, and the size is read.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::detect::xml_root;

/// Elements removed with their content inside a `foreignObject`, where HTML lives.
const HTML_REMOVED: [&str; 9] = [
    "iframe", "object", "embed", "form", "input", "button", "link", "meta", "base",
];

/// `svg` without scripts, event handlers, `javascript:` and other links with a scheme but
/// http(s), mail and image data, and `<style>` that imports or runs anything, with every other
/// byte kept; and how many items went. `Err` when the engine's SVG isn't well-formed XML.
pub fn clean(svg: &str) -> Result<(String, usize), String> {
    let mut reader = Reader::from_str(svg);
    let (mut out, mut removed) = (String::with_capacity(svg.len()), 0);
    let mut depth = 0;
    // The depth of the removed element being skipped, of the `foreignObject` we're inside, and
    // of the `<style>` being copied with where it starts in `out`.
    let (mut skip, mut foreign, mut style) = (None, None, None);
    loop {
        let from = reader.buffer_position() as usize;
        let event = reader.read_event().map_err(|e| e.to_string())?;
        let raw = &svg[from..reader.buffer_position() as usize];
        match event {
            Event::Eof => break,
            Event::Start(tag) => {
                depth += 1;
                if skip.is_some() {
                    continue;
                }
                let name = local(&tag);
                if name == "script" || foreign.is_some() && HTML_REMOVED.contains(&name.as_str()) {
                    (skip, removed) = (Some(depth), removed + 1);
                    continue;
                }
                match name.as_str() {
                    "foreignobject" => foreign = foreign.or(Some(depth)),
                    "style" => style = Some((out.len(), depth)),
                    _ => {}
                }
                removed += copy_tag(&mut out, raw, &tag, false);
            }
            Event::Empty(tag) if skip.is_none() => {
                let name = local(&tag);
                if name == "script" || foreign.is_some() && HTML_REMOVED.contains(&name.as_str()) {
                    removed += 1;
                } else {
                    removed += copy_tag(&mut out, raw, &tag, true);
                }
            }
            Event::End(_) => {
                if skip.is_none() {
                    out.push_str(raw);
                }
                if skip == Some(depth) {
                    skip = None;
                }
                if foreign == Some(depth) {
                    foreign = None;
                }
                if let Some((at, d)) = style
                    && d == depth
                {
                    if unsafe_css(&out[at..]) {
                        out.truncate(at);
                        removed += 1;
                    }
                    style = None;
                }
                depth -= 1;
            }
            _ if skip.is_none() => out.push_str(raw),
            _ => {}
        }
    }
    Ok((out, removed))
}

/// An element's local name in lower case: `svg:script` and `SCRIPT` are `script`.
fn local(tag: &BytesStart) -> String {
    tag.local_name().as_ref().to_ascii_lowercase()
}

/// Copies a start or empty tag, rewritten without its unsafe attributes when it has any; how
/// many it had.
fn copy_tag(out: &mut String, raw: &str, tag: &BytesStart, empty: bool) -> usize {
    let mut kept = vec![];
    let mut removed = 0;
    for attr in tag.attributes().with_checks(false).flatten() {
        let value = attr.normalized_value(XmlVersion::Implicit1_0);
        let key = attr.key.local_name().as_ref().to_ascii_lowercase();
        match value {
            Ok(value) if safe(&key, &value) => kept.push(attr),
            _ => removed += 1,
        }
    }
    if removed == 0 {
        out.push_str(raw);
        return 0;
    }
    out.push('<');
    out.push_str(tag.name().as_ref());
    for attr in kept {
        let value = &attr.value;
        let quote = if value.contains('"') { '\'' } else { '"' };
        let key = attr.key.as_ref();
        out.push_str(&format!(" {key}={quote}{value}{quote}"));
    }
    out.push_str(if empty { "/>" } else { ">" });
    removed
}

/// Whether an attribute may stay: no event handler, no script URL anywhere, and links only
/// relative, to http(s) or mail, or to image data. Browsers ignore spaces and controls inside a scheme, so
/// the check does too.
fn safe(key: &str, value: &str) -> bool {
    let v: String = (value.chars())
        .filter(|c| !c.is_ascii_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    if key.starts_with("on") || v.contains("javascript:") || v.contains("vbscript:") {
        return false;
    }
    if !matches!(key, "href" | "src" | "action") {
        return true;
    }
    let image = v.strip_prefix("data:image/").is_some_and(|rest| {
        ["png", "jpeg", "gif", "webp", "svg+xml"].iter().any(|t| {
            rest.strip_prefix(t)
                .is_some_and(|r| r.starts_with([';', ',']))
        })
    });
    // A reference without a scheme (`#n`, `a.png`) points into the SVG or beside it.
    let scheme = v.split_once(':').map(|(s, _)| s).filter(|s| {
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    image || matches!(scheme, None | Some("http" | "https" | "mailto"))
}

/// Whether a `<style>` element's text loads or runs anything.
fn unsafe_css(style: &str) -> bool {
    let css = style.to_ascii_lowercase();
    css.contains("@import") || css.contains("javascript:")
}

/// The SVG's size in CSS px, rounded up: from the root's `width` and `height` (a `pt` is 4/3
/// px), else from its `viewBox`.
pub fn size(svg: &str) -> (u32, u32) {
    let attrs = xml_root(svg).map(|(_, attrs)| attrs).unwrap_or_default();
    let attr = |key: &str| attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let view: Vec<f64> = (attr("viewBox").unwrap_or_default())
        .split([' ', ','])
        .filter_map(|n| n.parse().ok())
        .collect();
    let side = |key: &str, i: usize| {
        let length = attr(key).and_then(|v| match v.trim().strip_suffix("pt") {
            Some(pt) => pt.parse::<f64>().ok().map(|n| n * 4.0 / 3.0),
            None => v.trim().trim_end_matches("px").parse().ok(),
        });
        let length = length.or_else(|| view.get(i).copied()).unwrap_or(0.0);
        // Rounding up a value a float multiply left a hair above a whole number isn't wanted.
        (length - 1e-6).ceil().max(0.0) as u32
    };
    (side("width", 2), side("height", 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_come_from_width_and_height_else_the_view_box() {
        let dot = "<?xml version=\"1.0\"?>\n<!DOCTYPE svg>\n<!-- x -->\n<svg width=\"62pt\" height=\"75pt\"\n viewBox=\"0.00 0.00 62.00 116.00\">";
        assert_eq!(size(dot), (83, 100));
        let mermaid = r#"<svg id="merman" width="100%" viewBox="4.5 4 187 357.2">"#;
        assert_eq!(size(mermaid), (187, 358));
        assert_eq!(size(r#"<svg width="255" height="434px">"#), (255, 434));
        assert_eq!(size("<svg>"), (0, 0));
    }

    #[test]
    fn a_safe_svg_is_kept_byte_for_byte() {
        let svg = "<?xml version=\"1.0\"?>\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\"\n \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n<!-- c -->\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\">\n<style>.a{fill:url(#g)}</style>\n<a xlink:href='https://x.org/?a=1&amp;b=2'><text x=\"1\">a &lt; b &#233;</text></a>\n<image href=\"data:image/png;base64,AAAA\"/><use href=\"#n\"/>\n<foreignObject><div xmlns=\"http://www.w3.org/1999/xhtml\"><a href=\"mailto:a@b.c\">m</a><br/></div></foreignObject>\n<![CDATA[x]]></svg>\n";
        assert_eq!(clean(svg).unwrap(), (svg.to_string(), 0));
    }

    #[test]
    fn scripts_handlers_and_script_links_are_removed() {
        let cases = [
            (
                "<svg><script>alert(1)</script><g/></svg>",
                "<svg><g/></svg>",
                1,
            ),
            ("<svg><SVG:Script/><g/></svg>", "<svg><g/></svg>", 1),
            (
                "<svg><a xlink:href=\"javascript:alert(1)\" xlink:title=\"t\">x</a></svg>",
                "<svg><a xlink:title=\"t\">x</a></svg>",
                1,
            ),
            (
                "<svg><g onclick=\"alert(1)\" ONLOAD='x' id=\"a\"/></svg>",
                "<svg><g id=\"a\"/></svg>",
                2,
            ),
            (
                "<svg><a href=\"&#106;ava\tscript:x\">a</a><a href=\" JAVASCRIPT:x\">b</a></svg>",
                "<svg><a>a</a><a>b</a></svg>",
                2,
            ),
            (
                "<svg><image href=\"data:text/html,x\"/><image src=\"file:///etc/passwd\"/><image href=\"C:\\a.png\"/><image href=\"a b/c:d.png\"/></svg>",
                "<svg><image/><image/><image/><image href=\"a b/c:d.png\"/></svg>",
                3,
            ),
            (
                "<svg><set attributeName=\"href\" to=\"javascript:x\"/></svg>",
                "<svg><set attributeName=\"href\"/></svg>",
                1,
            ),
            (
                "<svg><style>@import url(http://x/a.css);</style><style>.a{}</style></svg>",
                "<svg><style>.a{}</style></svg>",
                1,
            ),
            (
                "<svg><foreignObject><div><iframe src=\"https://x\"><p/></iframe><form/><p>ok</p></div></foreignObject><form/></svg>",
                "<svg><foreignObject><div><p>ok</p></div></foreignObject><form/></svg>",
                2,
            ),
            (
                "<svg><a title='say \"hi\"' onclick=\"x\">q</a></svg>",
                "<svg><a title='say \"hi\"'>q</a></svg>",
                1,
            ),
        ];
        for (svg, want, n) in cases {
            assert_eq!(clean(svg).unwrap(), (want.to_string(), n), "{svg}");
        }
        assert!(clean("<svg><g></svg>").is_err());
    }
}
