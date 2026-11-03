//! draw.io labels: plain text, or the HTML subset draw.io's labels use, laid out where
//! mxGraph's `mxCellRenderer` and `mxText` put them (spacing, label positions, alignment,
//! wrapping) with Liberation Sans widths and draw.io's 1.2 line height.

use std::fmt::Write;

use super::Rect;
use super::shapes::paint;
use super::style::Style;
use crate::raster::Widths;
use crate::svg::{escape, num as n};

/// Where in a line its baseline is, as a share of the font size: half the leading of a 1.2 line
/// height over Arial's ascent and descent, then the ascent.
const BASELINE: f64 = (1.2 - 0.905 - 0.212) / 2.0 + 0.905;

/// What a label's HTML held that was left out.
#[derive(Default)]
pub struct Dropped {
    /// Tags outside the subset, kept as their text.
    pub simplified: bool,
    /// Scripts, event handlers and script links.
    pub unsafe_items: usize,
}

/// A piece of text in one look.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    text: String,
    font: Font,
}

#[derive(Clone, Debug, PartialEq)]
struct Font {
    size: f64,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: String,
    family: String,
    /// -1 subscript, 1 superscript.
    shift: i8,
}

/// A line of runs: a list item's text starts `indent` in and has its marker before it, and a
/// paragraph's or list's margin can leave a gap above.
#[derive(Default, Debug)]
struct Line {
    runs: Vec<Run>,
    indent: f64,
    marker: Option<Run>,
    gap: f64,
}

/// Draws `text` for a cell in `at` (for an edge, at its point); the box the text covers.
pub fn draw(
    out: &mut String,
    widths: &Widths,
    text: &str,
    style: &Style,
    at: Rect,
    edge: bool,
    dropped: &mut Dropped,
) -> Option<Rect> {
    if style.on("noLabel") {
        return None;
    }
    let bits = style.num("fontStyle", 0.0) as u32;
    let base = Font {
        size: style.num("fontSize", 11.0),
        bold: bits & 1 != 0,
        italic: bits & 2 != 0,
        underline: bits & 4 != 0,
        strike: bits & 8 != 0,
        color: paint(style.get("fontColor"), "#000000"),
        family: style.get("fontFamily").unwrap_or("Helvetica").to_string(),
        shift: 0,
    };
    let mut lines = match style.on("html") {
        true => html(text, &base, dropped),
        false => (text.split('\n'))
            .map(|l| Line {
                runs: vec![Run {
                    text: l.to_string(),
                    font: base.clone(),
                }],
                ..Line::default()
            })
            .collect(),
    };
    let blank = |l: &Line| l.runs.iter().all(|r| r.text.trim().is_empty()) && l.marker.is_none();
    while lines.len() > 1 && lines.last().is_some_and(|l| blank(l) && l.gap == 0.0) {
        lines.pop();
    }
    if lines.iter().all(blank) {
        return None;
    }
    let spacing = style.num("spacing", 2.0);
    let side = |k: &str| spacing + style.num(k, 0.0);
    let (st, sr, sb, sl) = (
        side("spacingTop"),
        side("spacingRight"),
        side("spacingBottom"),
        side("spacingLeft"),
    );
    let align = style.get("align").unwrap_or("center");
    let valign = style.get("verticalAlign").unwrap_or("middle");
    let horizontal = style.not_off("horizontal");
    // The label's box: the cell, moved by the label position, cut to the shape's text area.
    let mut b = at;
    if !edge {
        match style.get("labelPosition") {
            Some("left") => b.x -= at.w,
            Some("right") => b.x += at.w,
            _ => {}
        }
        match style.get("verticalLabelPosition") {
            Some("top") => b.y -= at.h,
            Some("bottom") => b.y += at.h,
            _ => {}
        }
        let centred = !matches!(style.get("labelPosition"), Some("left" | "right"))
            && !matches!(style.get("verticalLabelPosition"), Some("top" | "bottom"));
        if centred {
            b = text_area(style, b);
        }
    }
    // A label read upwards is laid out across a box turned a quarter.
    let turn = !horizontal && !edge;
    if turn {
        b = Rect {
            x: b.cx() - b.h / 2.0,
            y: b.cy() - b.w / 2.0,
            w: b.h,
            h: b.w,
        };
    }
    let ax = match align {
        "left" => b.x + sl,
        "right" => b.x + b.w - sr,
        _ => b.x + b.w / 2.0 + (sl - sr) / 2.0,
    };
    let ay = match valign {
        "top" => b.y + st + 5.0,
        "bottom" => b.y + b.h - sb - 1.0,
        _ => b.y + b.h / 2.0 + (st - sb) / 2.0,
    };
    let wrap = style.get("whiteSpace") == Some("wrap") && !edge && b.w > 0.0;
    if wrap {
        let max = (b.w - sl - sr + 2.0).round();
        lines = lines
            .into_iter()
            .flat_map(|l| fold(widths, l, max))
            .collect();
    }
    let width = |l: &Line| l.indent + l.runs.iter().map(|r| measure(widths, r)).sum::<f64>();
    let size_of = |l: &Line| {
        l.runs
            .iter()
            .chain(&l.marker)
            .map(|r| r.font.size)
            .fold(0.0, f64::max)
    };
    let height = |l: &Line| l.gap + 1.2 * size_of(l);
    let total: f64 = lines.iter().map(height).sum();
    let widest = lines.iter().map(width).fold(0.0, f64::max);
    let mut y = match valign {
        "top" => ay,
        "bottom" => ay - total,
        _ => ay - total / 2.0,
    };
    let left_of = |w: f64| match align {
        "left" => ax,
        "right" => ax - w,
        _ => ax - w / 2.0,
    };
    let block = Rect {
        x: left_of(widest),
        y,
        w: widest,
        h: total,
    };
    let mut body = String::new();
    let bg = paint(style.get("labelBackgroundColor"), "#ffffff");
    let border = paint(style.get("labelBorderColor"), "#000000");
    if bg != "none" || border != "none" {
        let _ = write!(
            body,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{bg}" stroke="{border}"/>"#,
            n(block.x),
            n(block.y),
            n(block.w),
            n(block.h)
        );
    }
    let _ = write!(
        body,
        r#"<text font-family="{}" font-size="{}" fill="{}"{}{}>"#,
        families(&base.family),
        n(base.size),
        base.color,
        if base.bold {
            r#" font-weight="bold""#
        } else {
            ""
        },
        if base.italic {
            r#" font-style="italic""#
        } else {
            ""
        }
    );
    for line in &lines {
        if line.runs.is_empty() && line.marker.is_none() {
            y += height(line);
            continue;
        }
        let size = size_of(line);
        let x = left_of(width(line)) + line.indent;
        let baseline = y + line.gap + BASELINE * size;
        if let Some(m) = &line.marker {
            let at = x - measure(widths, m);
            let _ = write!(
                body,
                r#"<tspan x="{}" y="{}">{}</tspan>"#,
                n(at),
                n(baseline),
                escape(&m.text)
            );
        }
        let _ = write!(body, r#"<tspan x="{}" y="{}">"#, n(x), n(baseline));
        for run in &line.runs {
            let text = escape(&run.text);
            match run.font == base {
                true => body.push_str(&text),
                false => {
                    let _ = write!(body, "<tspan{}>{text}</tspan>", attrs(&run.font, &base));
                }
            }
        }
        body.push_str("</tspan>");
        y += height(line);
    }
    body.push_str("</text>");
    let opacity = match style.num("textOpacity", 100.0) {
        100.0 => String::new(),
        o => format!(r#" opacity="{}""#, n(o / 100.0)),
    };
    let rotate = match turn {
        true => format!(r#" transform="rotate(-90 {} {})""#, n(b.cx()), n(b.cy())),
        false => String::new(),
    };
    let _ = writeln!(out, "<g{opacity}{rotate}>{body}</g>");
    Some(match turn {
        true => Rect {
            x: b.cx() - (block.y + block.h - b.cy()),
            y: b.cy() - (block.x + block.w - b.cx()),
            w: block.h,
            h: block.w,
        },
        false => block,
    })
}

/// The part of a centred label's box a shape keeps text in: a swimlane's header, inside a
/// double ellipse or a process's bars, and the body of shapes with `boundedLbl`.
fn text_area(style: &Style, b: Rect) -> Rect {
    let shape = style.get("shape").unwrap_or("label");
    let mut r = b;
    match shape {
        "swimlane" | "table" => {
            let start = style.num("startSize", if shape == "table" { 0.0 } else { 40.0 });
            if start > 0.0 {
                match style.not_off("horizontal") {
                    true => r.h = r.h.min(start),
                    false => r.w = r.w.min(start),
                }
            }
        }
        "doubleEllipse" => {
            let sw = style.num("strokeWidth", 1.0);
            let m = style.num("margin", (3.0 + sw).min((b.w / 5.0).min(b.h / 5.0)));
            r = Rect {
                x: b.x + m,
                y: b.y + m,
                w: b.w - 2.0 * m,
                h: b.h - 2.0 * m,
            };
        }
        "process" => {
            let inset = match style.get("fixedSize").is_some_and(|v| v != "0") {
                true => style.num("size", 0.1).clamp(0.0, b.w),
                false => b.w * style.num("size", 0.1).clamp(0.0, 1.0),
            };
            r.x += inset.round();
            r.w -= (2.0 * inset).round();
        }
        "document" if style.on("boundedLbl") => {
            r.h -= style.num("size", 0.3) * b.h;
        }
        "cylinder3" if style.on("boundedLbl") => {
            let size = style.num("size", 15.0);
            let top = (size * 2.0).min(b.h);
            r.y += top;
            r.h -= top + size * 0.3;
        }
        _ => {}
    }
    r
}

/// SVG's font family list: the style's, then Helvetica's stand-ins.
fn families(family: &str) -> String {
    let mut all = vec![];
    for f in family
        .split(',')
        .map(|f| f.trim().trim_matches(['\'', '"']))
    {
        if !f.is_empty() && !all.contains(&f) {
            all.push(f);
        }
    }
    for f in ["Helvetica", "Arial", "Liberation Sans", "sans-serif"] {
        if !all.contains(&f) {
            all.push(f);
        }
    }
    let quoted: Vec<String> = (all.iter())
        .map(|f| match f.contains(' ') {
            true => format!("'{f}'"),
            false => f.to_string(),
        })
        .collect();
    escape(&quoted.join(", "))
}

/// The attributes a run needs beyond the label's own look.
fn attrs(f: &Font, base: &Font) -> String {
    let mut a = String::new();
    if f.family != base.family {
        let _ = write!(a, r#" font-family="{}""#, families(&f.family));
    }
    if f.size != base.size || f.shift != 0 {
        let size = if f.shift != 0 { f.size * 0.83 } else { f.size };
        let _ = write!(a, r#" font-size="{}""#, n(size));
    }
    if f.shift != 0 {
        let _ = write!(
            a,
            r#" baseline-shift="{}""#,
            if f.shift < 0 { "sub" } else { "super" }
        );
    }
    if f.bold != base.bold {
        let _ = write!(
            a,
            r#" font-weight="{}""#,
            if f.bold { "bold" } else { "normal" }
        );
    }
    if f.italic != base.italic {
        let _ = write!(
            a,
            r#" font-style="{}""#,
            if f.italic { "italic" } else { "normal" }
        );
    }
    let deco: Vec<&str> = [(f.underline, "underline"), (f.strike, "line-through")]
        .into_iter()
        .filter_map(|(on, d)| on.then_some(d))
        .collect();
    if !deco.is_empty() {
        let _ = write!(a, r#" text-decoration="{}""#, deco.join(" "));
    }
    if f.color != base.color {
        let _ = write!(a, r#" fill="{}""#, f.color);
    }
    a
}

fn measure(widths: &Widths, r: &Run) -> f64 {
    let size = if r.font.shift != 0 {
        r.font.size * 0.83
    } else {
        r.font.size
    };
    widths.styled(&r.text, size, r.font.bold)
}

/// `line` broken at spaces (or inside a word too long for it) into lines no wider than `max`.
fn fold(widths: &Widths, line: Line, max: f64) -> Vec<Line> {
    let mut out = vec![];
    let mut current = Line {
        indent: line.indent,
        marker: line.marker,
        gap: line.gap,
        ..Line::default()
    };
    let mut used = line.indent;
    for run in line.runs {
        // Words with the spaces after them, so a line breaks after a space.
        let mut words: Vec<String> = vec![];
        for c in run.text.chars() {
            match words.last_mut() {
                Some(w) if !(w.ends_with(' ') && c != ' ') => w.push(c),
                _ => words.push(c.to_string()),
            }
        }
        for word in words {
            let piece = Run {
                text: word,
                font: run.font.clone(),
            };
            let w = measure(
                widths,
                &Run {
                    text: piece.text.trim_end().into(),
                    font: run.font.clone(),
                },
            );
            if used + w > max && used > line.indent {
                trim_end(&mut current);
                out.push(std::mem::replace(
                    &mut current,
                    Line {
                        indent: line.indent,
                        ..Line::default()
                    },
                ));
                used = line.indent;
            }
            let mut piece = piece;
            // A word wider than the whole line is cut where it fills it.
            while measure(widths, &piece) > max - used
                && piece.text.chars().count() > 1
                && used == line.indent
            {
                let mut cut = String::new();
                for c in piece.text.chars() {
                    let mut next = cut.clone();
                    next.push(c);
                    if measure(
                        widths,
                        &Run {
                            text: next.clone(),
                            font: piece.font.clone(),
                        },
                    ) > max - used
                        && !cut.is_empty()
                    {
                        break;
                    }
                    cut = next;
                }
                if cut.len() == piece.text.len() {
                    break;
                }
                let rest = piece.text[cut.len()..].to_string();
                push(
                    &mut current,
                    Run {
                        text: cut,
                        font: piece.font.clone(),
                    },
                );
                out.push(std::mem::replace(
                    &mut current,
                    Line {
                        indent: line.indent,
                        ..Line::default()
                    },
                ));
                piece.text = rest;
            }
            used += measure(widths, &piece);
            push(&mut current, piece);
        }
    }
    trim_end(&mut current);
    out.push(current);
    out
}

/// Adds a run, joining it to the last one when they look the same.
fn push(line: &mut Line, run: Run) {
    match line.runs.last_mut() {
        Some(last) if last.font == run.font => last.text.push_str(&run.text),
        _ => line.runs.push(run),
    }
}

fn trim_end(line: &mut Line) {
    if let Some(last) = line.runs.last_mut() {
        let kept = last.text.trim_end().len();
        last.text.truncate(kept);
    }
}

/// The lines of an HTML label, in the subset draw.io writes: inline looks, line breaks, blocks
/// and lists; other tags keep only their text, and scripts and handlers are left out.
fn html(text: &str, base: &Font, dropped: &mut Dropped) -> Vec<Line> {
    let mut lines = vec![Line::default()];
    let mut stack: Vec<(String, Font)> = vec![];
    let mut lists: Vec<(bool, usize)> = vec![];
    let mut font = base.clone();
    let mut rest = text.replace('\n', "<br>");
    let mut skip: Option<String> = None;
    let mut space = false;
    // The margin waiting above the next line: a paragraph's or outer list's, 1 em.
    let mut pending: f64 = 0.0;
    while !rest.is_empty() {
        let (chunk, after) = match rest.find('<') {
            Some(0) => {
                let end = rest.find('>').map_or(rest.len(), |e| e + 1);
                (rest[..end].to_string(), rest[end..].to_string())
            }
            Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
            None => (rest.clone(), String::new()),
        };
        rest = after;
        if !chunk.starts_with('<') {
            if skip.is_none() {
                let mut t = String::new();
                for c in entities(&chunk).chars() {
                    match c {
                        '\u{a0}' => {
                            t.push(' ');
                            space = false;
                        }
                        c if c.is_ascii_whitespace() => {
                            if !space {
                                t.push(' ');
                            }
                            space = true;
                        }
                        c => {
                            t.push(c);
                            space = false;
                        }
                    }
                }
                let line = lines.last_mut().unwrap();
                if line.runs.is_empty() {
                    t = t.trim_start().to_string();
                }
                if !t.is_empty() && line.runs.is_empty() && line.marker.is_none() {
                    line.gap += std::mem::take(&mut pending);
                }
                if !t.is_empty() {
                    push(
                        line,
                        Run {
                            text: t,
                            font: font.clone(),
                        },
                    );
                }
            }
            continue;
        }
        let inner = chunk
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim_end_matches('/');
        let closing = inner.starts_with('/');
        let inner = inner.trim_start_matches('/');
        let name_end = inner
            .find(|c: char| c.is_whitespace())
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        let attrs = attributes(&inner[name_end..]);
        if let Some(open) = &skip {
            if closing && *open == name {
                skip = None;
            }
            continue;
        }
        for (k, v) in &attrs {
            let v = v
                .to_ascii_lowercase()
                .replace(|c: char| c.is_whitespace(), "");
            if k.starts_with("on") || v.contains("javascript:") {
                dropped.unsafe_items += 1;
            }
        }
        let block = matches!(name.as_str(), "div" | "p" | "li" | "ul" | "ol" | "br");
        if block {
            let line = lines.last().unwrap();
            let open_li = name == "li" && !closing;
            if name == "br" || !line.runs.is_empty() || open_li {
                if !line.runs.is_empty() || name == "br" {
                    lines.push(Line::default());
                }
                space = false;
            }
        }
        let outer_list =
            matches!(name.as_str(), "ul" | "ol") && lists.len() == usize::from(closing);
        if name == "p" || outer_list {
            pending = pending.max(font.size);
        }
        match (name.as_str(), closing) {
            ("script" | "style" | "iframe" | "object" | "embed", false) => {
                if name == "script" || name != "style" {
                    dropped.unsafe_items += 1;
                }
                dropped.simplified = true;
                if !chunk.ends_with("/>") {
                    skip = Some(name);
                }
            }
            ("ul" | "ol", false) => lists.push((name == "ol", 0)),
            ("ul" | "ol", true) => {
                lists.pop();
            }
            ("li", false) => {
                let depth = lists.len().max(1) as f64;
                let marker = match lists.last_mut() {
                    Some((true, k)) => {
                        *k += 1;
                        format!("{k}. ")
                    }
                    _ => "•  ".to_string(),
                };
                let line = lines.last_mut().unwrap();
                line.indent = 1.75 * font.size * depth;
                line.gap += std::mem::take(&mut pending);
                line.marker = Some(Run {
                    text: marker,
                    font: font.clone(),
                });
            }
            ("br" | "div" | "p" | "li", _) => {}
            (_, true) => {
                if let Some(at) = stack.iter().rposition(|(n, _)| *n == name) {
                    font = stack[at].1.clone();
                    stack.truncate(at);
                }
            }
            (tag, false) => {
                let before = font.clone();
                match tag {
                    "b" | "strong" => font.bold = true,
                    "i" | "em" => font.italic = true,
                    "u" | "a" => font.underline = true,
                    "s" | "strike" | "del" => font.strike = true,
                    "sub" => font.shift = -1,
                    "sup" => font.shift = 1,
                    "span" | "font" => {}
                    _ => dropped.simplified = true,
                }
                for (k, v) in &attrs {
                    match k.as_str() {
                        "color" if tag == "font" => font.color = super::color(v),
                        "face" if tag == "font" => font.family = v.clone(),
                        "size" if tag == "font" => {
                            let sizes = [10.0, 13.0, 16.0, 18.0, 24.0, 32.0, 48.0];
                            if let Ok(k) = v.trim().parse::<usize>() {
                                font.size = sizes[k.clamp(1, 7) - 1];
                            }
                        }
                        "style" => css(v, &mut font),
                        _ => {}
                    }
                }
                let void = chunk.ends_with("/>")
                    || matches!(tag, "img" | "hr" | "input" | "meta" | "link" | "wbr");
                if !void {
                    stack.push((name.clone(), before));
                }
            }
        }
    }
    while lines.len() > 1
        && lines
            .last()
            .is_some_and(|l| l.runs.is_empty() && l.marker.is_none())
    {
        lines.pop();
    }
    if pending > 0.0 {
        lines.push(Line {
            gap: pending,
            ..Line::default()
        });
    }
    lines
}

/// An inline `style` attribute's colour, size, family, weight, style and decoration.
fn css(style: &str, font: &mut Font) {
    for decl in style.split(';') {
        let Some((k, v)) = decl.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "color" => font.color = super::color(v),
            "font-size" => {
                let num = v.trim_end_matches(|c: char| c.is_ascii_alphabetic());
                if let Ok(s) = num.trim().parse::<f64>() {
                    font.size = if v.ends_with("pt") { s * 4.0 / 3.0 } else { s };
                }
            }
            "font-family" => font.family = v.to_string(),
            "font-weight" => font.bold = v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600),
            "font-style" => font.italic = v == "italic" || v == "oblique",
            "text-decoration" | "text-decoration-line" => {
                font.underline = v.contains("underline");
                font.strike = v.contains("line-through");
            }
            _ => {}
        }
    }
}

/// A tag's attributes, values unquoted and their entities decoded.
fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut rest = s.trim();
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let key = rest[..end].to_ascii_lowercase();
        rest = rest[end..].trim_start();
        let mut value = String::new();
        if let Some(r) = rest.strip_prefix('=') {
            let r = r.trim_start();
            let (v, after) = match r.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let close = r[1..].find(q).map_or(r.len(), |i| i + 1);
                    (&r[1..close], r.get(close + 1..).unwrap_or_default())
                }
                _ => {
                    let e = r.find(char::is_whitespace).unwrap_or(r.len());
                    (&r[..e], &r[e..])
                }
            };
            value = entities(v);
            rest = after.trim_start();
        }
        if !key.is_empty() {
            out.push((key, value));
        }
    }
    out
}

/// Text with its character references decoded: the named ones labels use, and numbers.
fn entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let name = &after[..e];
            let c = match name {
                "nbsp" => '\u{a0}',
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => match name.strip_prefix('#') {
                    Some(h) if h.starts_with(['x', 'X']) => {
                        char::from_u32(u32::from_str_radix(&h[1..], 16).ok()?)?
                    }
                    Some(d) => char::from_u32(d.parse().ok()?)?,
                    None => return None,
                },
            };
            Some((c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &after[e + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out + rest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Font {
        Font {
            size: 12.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            color: "#000000".into(),
            family: "Helvetica".into(),
            shift: 0,
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        (lines.iter())
            .filter(|l| !l.runs.is_empty())
            .map(|l| {
                l.marker
                    .iter()
                    .chain(&l.runs)
                    .map(|r| r.text.as_str())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn html_labels_keep_their_looks_and_breaks() {
        let mut d = Dropped::default();
        let lines = html(
            "<b>Bold</b> and <i>it</i><br>next&nbsp;line<div>block</div><font color=\"#ff0000\" size=\"4\">red</font>",
            &base(),
            &mut d,
        );
        assert_eq!(texts(&lines), ["Bold and it", "next line", "block", "red"]);
        assert!(lines[0].runs[0].font.bold && lines[0].runs[2].font.italic);
        assert_eq!(
            (
                lines[3].runs[0].font.color.as_str(),
                lines[3].runs[0].font.size
            ),
            ("#ff0000", 18.0)
        );
        assert!(!d.simplified && d.unsafe_items == 0);
    }

    #[test]
    fn lists_number_or_bullet_their_items() {
        let mut d = Dropped::default();
        let lines = html(
            "<ul><li>one</li><li>two</li></ul><ol><li>first</li><li>second</li></ol>",
            &base(),
            &mut d,
        );
        assert_eq!(texts(&lines), ["•  one", "•  two", "1. first", "2. second"]);
        // Text 1.75 em in, the outer lists 1 em apart and from what is around them.
        assert_eq!(
            (lines[0].indent, lines[0].gap, lines[2].gap, lines[4].gap),
            (21.0, 12.0, 12.0, 12.0)
        );
    }

    #[test]
    fn other_tags_keep_their_text_and_scripts_go() {
        let mut d = Dropped::default();
        let lines = html(
            r#"<img src="x" onerror="alert(1)">Hi <table><tr><td>cell</td></tr></table><script>alert(2)</script><a href="javascript:x">go</a>"#,
            &base(),
            &mut d,
        );
        assert_eq!(texts(&lines), ["Hi cellgo"]);
        assert!(d.simplified);
        assert_eq!(d.unsafe_items, 3);
    }

    #[test]
    fn entities_and_css_decode() {
        assert_eq!(
            entities("a &amp; b &#233; &#x6F22; &bogus; &"),
            "a & b é 漢 &bogus; &"
        );
        let mut f = base();
        css(
            "font-size: 9pt; color: rgb(0, 0, 255); font-weight: 700; text-decoration: underline",
            &mut f,
        );
        assert_eq!(
            (f.size, f.color.as_str(), f.bold, f.underline),
            (12.0, "rgb(0, 0, 255)", true, true)
        );
    }
}
