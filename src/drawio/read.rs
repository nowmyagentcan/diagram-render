//! Reading draw.io files: the pages of an `mxfile` (each plain or compressed as draw.io's
//! `Graph.compress` writes them), a bare `mxGraphModel`, or the file a `.drawio.svg` carries in
//! its `content` attribute; and the cells of one page.

use std::collections::HashMap;

use base64::Engine;
use roxmltree::{Document, Node};

use crate::input;

/// `%XX` sequences decoded as `decodeURIComponent` does; anything else is kept as it is.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            b[i],
            b.get(i + 1).and_then(|&c| hex(c)),
            b.get(i + 2).and_then(|&c| hex(c)),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a compressed page holds: base64, then raw deflate, then percent-encoded XML. `Err(None)`
/// when it unpacks to more than the limit, `Err(Some(reason))` when it isn't one.
pub fn decompress(text: &str) -> Result<String, Option<String>> {
    let packed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = (base64::engine::general_purpose::STANDARD.decode(packed))
        .map_err(|e| Some(format!("it isn't base64 ({e})")))?;
    let inflated = input::inflate(&bytes, false)
        .map_err(|e| Some(format!("it can't be unpacked ({e})")))?
        .ok_or(None)?;
    Ok(percent_decode(&String::from_utf8_lossy(&inflated)))
}

/// A point.
pub type P = (f64, f64);

/// A cell's `mxGeometry`.
#[derive(Clone, Default)]
pub struct Geo {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub relative: bool,
    pub offset: P,
    pub points: Vec<P>,
    pub source: Option<P>,
    pub target: Option<P>,
}

/// One `mxCell`, with what a `UserObject` or `object` around it adds.
pub struct Cell<'a> {
    pub id: &'a str,
    pub parent: Option<&'a str>,
    pub label: String,
    pub style: &'a str,
    pub vertex: bool,
    pub edge: bool,
    pub source: Option<&'a str>,
    pub target: Option<&'a str>,
    pub visible: bool,
    pub collapsed: bool,
    pub geo: Geo,
    pub line: usize,
}

/// The cells under a model's `<root>`, in the file's order.
pub fn cells<'a>(model: Node<'a, '_>, line: &dyn Fn(Node) -> usize) -> Vec<Cell<'a>> {
    let Some(root) = model.children().find(|n| n.has_tag_name("root")) else {
        return vec![];
    };
    (root.children().filter(Node::is_element))
        .filter_map(|n| {
            let (cell, object) = match n.tag_name().name() {
                "mxCell" => (n, None),
                _ => (n.children().find(|c| c.has_tag_name("mxCell"))?, Some(n)),
            };
            let attr = |k| cell.attribute(k);
            let label = match object {
                Some(o) => {
                    let label = o.attribute("label").unwrap_or_default();
                    match o.attribute("placeholders") == Some("1") {
                        true => placeholders(label, o),
                        false => label.to_string(),
                    }
                }
                None => attr("value").unwrap_or_default().to_string(),
            };
            Some(Cell {
                id: object.unwrap_or(cell).attribute("id").unwrap_or_default(),
                parent: attr("parent"),
                label,
                style: attr("style").unwrap_or_default(),
                vertex: attr("vertex") == Some("1"),
                edge: attr("edge") == Some("1"),
                source: attr("source"),
                target: attr("target"),
                visible: attr("visible") != Some("0"),
                collapsed: attr("collapsed") == Some("1"),
                geo: (cell.children().find(|c| c.has_tag_name("mxGeometry")))
                    .map(geometry)
                    .unwrap_or_default(),
                line: line(n),
            })
        })
        .collect()
}

/// `%name%` replaced by the object's attribute of that name, when it has one.
fn placeholders(label: &str, object: Node) -> String {
    let mut out = String::new();
    let mut rest = label;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            break;
        };
        let name = &after[..end];
        match object
            .attribute(name)
            .filter(|_| !name.is_empty() && name != "label")
        {
            Some(value) => {
                out.push_str(&rest[..start]);
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out + rest
}

fn number(n: Node, key: &str) -> f64 {
    let v = n.attribute(key).and_then(|v| v.trim().parse::<f64>().ok());
    v.filter(|v| v.is_finite()).unwrap_or(0.0)
}

fn point(n: Node) -> P {
    (number(n, "x"), number(n, "y"))
}

fn geometry(g: Node) -> Geo {
    let mut geo = Geo {
        x: number(g, "x"),
        y: number(g, "y"),
        w: number(g, "width"),
        h: number(g, "height"),
        relative: g.attribute("relative") == Some("1"),
        ..Geo::default()
    };
    for c in g.children().filter(Node::is_element) {
        match (c.tag_name().name(), c.attribute("as")) {
            ("mxPoint", Some("sourcePoint")) => geo.source = Some(point(c)),
            ("mxPoint", Some("targetPoint")) => geo.target = Some(point(c)),
            ("mxPoint", Some("offset")) => geo.offset = point(c),
            ("Array", Some("points")) => {
                geo.points = (c.children().filter(|p| p.has_tag_name("mxPoint")))
                    .map(point)
                    .collect();
            }
            _ => {}
        }
    }
    geo
}

/// Cells by id, for parents and terminals.
pub fn by_id<'a>(cells: &[Cell<'a>]) -> HashMap<&'a str, usize> {
    (cells.iter().enumerate()).map(|(i, c)| (c.id, i)).collect()
}

/// The page a `<diagram>` holds, parsed: its own `mxGraphModel`, or its compressed text.
pub enum Content<'d, 'i> {
    Model(Node<'d, 'i>),
    Packed(&'d str),
    Empty,
}

pub fn content<'d, 'i>(diagram: Node<'d, 'i>) -> Content<'d, 'i> {
    if diagram.has_tag_name("mxGraphModel") {
        return Content::Model(diagram);
    }
    if let Some(model) = diagram.children().find(|c| c.has_tag_name("mxGraphModel")) {
        return Content::Model(model);
    }
    match diagram.text().map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => Content::Packed(text),
        None => Content::Empty,
    }
}

/// The document; a file that isn't well-formed XML is a syntax error at its line and column.
pub fn parse<'a>(text: &'a str) -> Result<Document<'a>, roxmltree::Error> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    Document::parse_with_options(text, options)
}

/// roxmltree's message without its ` at r:c`.
pub fn message(e: &roxmltree::Error) -> String {
    let said = e.to_string();
    let at = format!(" at {}", e.pos());
    said.strip_suffix(&at).unwrap_or(&said).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_escapes_decode_as_in_a_browser() {
        assert_eq!(percent_decode("%3Cb%3E%C3%A9%20x"), "<b>é x");
        assert_eq!(percent_decode("100% sure %zz %4"), "100% sure %zz %4");
    }

    #[test]
    fn compressed_pages_unpack() {
        // draw.io's Graph.compress of `<mxGraphModel><root/></mxGraphModel>`.
        let packed = "UzV2zq1wL0osyPDNT0nNUTV2VTV2LsrPL1E1coNwgAw0FQA=";
        assert_eq!(
            decompress(packed).unwrap(),
            "<mxGraphModel><root/></mxGraphModel>"
        );
        assert!(decompress("not base64!").unwrap_err().is_some());
    }

    #[test]
    fn placeholders_take_the_objects_attributes() {
        let doc = Document::parse(r#"<o label="%name% (%role%) %x% 5%" name="Ann" role="lead"/>"#);
        let doc = doc.unwrap();
        let o = doc.root_element();
        let label = o.attribute("label").unwrap();
        assert_eq!(placeholders(label, o), "Ann (lead) %x% 5%");
    }
}
