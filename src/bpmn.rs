//! BPMN 2.0 files, drawn from their diagram section (BPMN DI): every shape at its bounds and
//! every edge along its waypoints. Files without one aren't laid out.

use std::collections::HashMap;
use std::fmt::Write;

use roxmltree::{Document, Node, ParsingOptions};

use crate::engine::{Checked, Page, Rendered, Task};
use crate::error::{Error, Warning};
use crate::input::Lines;

const MODEL: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";
const BPMNDI: &str = "http://www.omg.org/spec/BPMN/20100524/DI";
const DC: &str = "http://www.omg.org/spec/DD/20100524/DC";
const DI: &str = "http://www.omg.org/spec/DD/20100524/DI";

/// The most shapes and edges one diagram may have.
const LIMIT: usize = 20_000;
/// Space around the drawing, as bpmn-js leaves when it saves an SVG.
const PAD: f64 = 5.0;

pub fn render(task: &Task) -> Result<Rendered, Error> {
    // XML reads every line end as LF, and so lines are counted as everywhere else.
    let text = task.lines.text().replace("\r\n", "\n").replace('\r', "\n");
    let doc = parse(&text, &task.lines)?;
    let line = |n: Node| doc.text_pos_at(n.range().start).row as usize;
    let model: HashMap<&str, Node> = (doc.descendants())
        .filter(|n| n.tag_name().namespace() == Some(MODEL))
        .filter_map(|n| Some((n.attribute("id")?, n)))
        .collect();
    let diagrams: Vec<Node> = (doc.descendants())
        .filter(|n| is(*n, BPMNDI, "BPMNDiagram"))
        .collect();
    if diagrams.is_empty() {
        return Err(Error::no_diagram_layout());
    }
    let pages: Vec<Page> = (diagrams.iter().enumerate())
        .map(|(i, d)| Page {
            index: Some(i + 1),
            name: (d.attribute("name").or(d.attribute("id")))
                .map_or_else(|| format!("Diagram {}", i + 1), Into::into),
        })
        .collect();
    let at = match task.page {
        None => 0,
        Some(p) => find(&diagrams, p).ok_or_else(|| {
            let names: Vec<&str> = pages.iter().map(|p| p.name.as_str()).collect();
            Error::page_not_found(task.name, p, &names)
        })?,
    };
    let mut warnings = vec![];
    if pages.len() > 1 && task.page.is_none() {
        warnings.push(Warning::pages_more(task.name, pages.len(), &pages[0].name));
    }
    let plane = diagrams[at]
        .children()
        .find(|n| is(*n, BPMNDI, "BPMNPlane"));
    let shown = plane.and_then(|p| model.get(p.attribute("bpmnElement")?));
    let diagram_type = match shown.map(|n| n.tag_name().name()) {
        Some("collaboration") => "collaboration",
        _ => "process",
    };
    let items: Vec<Node> = (plane.into_iter().flat_map(|p| p.descendants()))
        .filter(|n| is(*n, BPMNDI, "BPMNShape") || is(*n, BPMNDI, "BPMNEdge"))
        .collect();
    if items.len() > LIMIT {
        return Err(Error::too_many_elements(items.len(), LIMIT));
    }
    let mut svg = Svg::default();
    for item in items {
        let id = item.attribute("id").unwrap_or("?");
        let reference = item.attribute("bpmnElement").unwrap_or_default();
        if !model.contains_key(reference) {
            warnings.push(Warning::di_reference_missing(id, reference, line(item)));
            continue;
        }
        if item.tag_name().name() == "BPMNShape" {
            match bounds(item) {
                Some(b) => svg.rect(b),
                None => warnings.push(Warning::skipped(
                    "element_skipped",
                    "shape",
                    id,
                    "it has no bounds",
                    line(item),
                )),
            }
        } else {
            let points = waypoints(item);
            match points.len() {
                0 | 1 => warnings.push(Warning::skipped(
                    "edge_skipped",
                    "edge",
                    id,
                    "it has fewer than 2 waypoints",
                    line(item),
                )),
                _ => svg.line(&points),
            }
        }
    }
    let page = Page {
        index: Some(at + 1),
        name: pages[at].name.clone(),
    };
    Ok(Rendered {
        svg: svg.finish(task.background).into_bytes(),
        warnings,
        page: Some(page),
        pages,
        diagram_type: Some(diagram_type),
    })
}

/// Everything a render finds, without the drawing; `pages_more` belongs to a render only.
pub fn check(task: &Task) -> Result<Checked, Error> {
    let r = render(task)?;
    Ok(Checked {
        warnings: (r.warnings.into_iter())
            .filter(|w| w.code != "pages_more")
            .collect(),
        diagram_type: r.diagram_type,
        ..Checked::default()
    })
}

/// The document; a file that isn't well-formed XML is a syntax error at its line and column.
/// External entities are never read, and entities that expand without end are refused.
fn parse<'a>(text: &'a str, lines: &Lines) -> Result<Document<'a>, Error> {
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, options).map_err(|e| {
        let pos = e.pos();
        let said = e.to_string();
        let said = said.strip_suffix(&format!(" at {pos}")).unwrap_or(&said);
        let (row, col) = (pos.row as usize, pos.col as usize);
        lines.place(Error::syntax_error(said, row, None), row, Some(col))
    })
}

/// The diagram `--page` names: its number from 1, else its name or id.
fn find(diagrams: &[Node], wanted: &str) -> Option<usize> {
    if wanted.bytes().all(|b| b.is_ascii_digit()) {
        let n: usize = wanted.parse().ok()?;
        return (1..=diagrams.len()).contains(&n).then(|| n - 1);
    }
    let named = |d: &Node| [d.attribute("name"), d.attribute("id")].contains(&Some(wanted));
    diagrams.iter().position(named)
}

fn is(n: Node, namespace: &str, name: &str) -> bool {
    n.tag_name().namespace() == Some(namespace) && n.tag_name().name() == name
}

/// A box: left, top, width and height.
#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// The `dc:Bounds` of a shape or label.
fn bounds(n: Node) -> Option<Rect> {
    let b = n.children().find(|c| is(*c, DC, "Bounds"))?;
    let get = |k: &str| {
        b.attribute(k)?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    };
    Some(Rect {
        x: get("x")?,
        y: get("y")?,
        w: get("width")?.max(0.0),
        h: get("height")?.max(0.0),
    })
}

/// An edge's `di:waypoint`s.
fn waypoints(n: Node) -> Vec<(f64, f64)> {
    let point = |w: Node| {
        let get = |k: &str| {
            w.attribute(k)?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
        };
        Some((get("x")?, get("y")?))
    };
    (n.children().filter(|c| is(*c, DI, "waypoint")))
        .filter_map(point)
        .collect()
}

/// The SVG being drawn, and the box around everything in it.
#[derive(Default)]
struct Svg {
    body: String,
    /// The top-left and bottom-right corners of everything drawn.
    extent: Option<((f64, f64), (f64, f64))>,
}

impl Svg {
    /// Grows the drawing's box to hold the point.
    fn include(&mut self, x: f64, y: f64) {
        let ((x0, y0), (x1, y1)) = self.extent.unwrap_or(((x, y), (x, y)));
        self.extent = Some(((x0.min(x), y0.min(y)), (x1.max(x), y1.max(y))));
    }

    fn rect(&mut self, b: Rect) {
        self.include(b.x, b.y);
        self.include(b.x + b.w, b.y + b.h);
        let _ = writeln!(
            self.body,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="white" stroke="black" stroke-width="2"/>"#,
            n(b.x),
            n(b.y),
            n(b.w),
            n(b.h)
        );
    }

    fn line(&mut self, points: &[(f64, f64)]) {
        for &(x, y) in points {
            self.include(x, y);
        }
        let _ = writeln!(
            self.body,
            r#"<polyline points="{}" fill="none" stroke="black" stroke-width="2"/>"#,
            path(points)
        );
    }

    /// The whole SVG, framed as bpmn-js frames its export: everything drawn plus 5 px.
    fn finish(self, background: Option<&str>) -> String {
        let ((x, y), (x1, y1)) = self.extent.unwrap_or_default();
        let (x, y) = (x - PAD, y - PAD);
        let (w, h) = (x1 - x + PAD, y1 - y + PAD);
        let (x, y, w, h) = (n(x), n(y), n(w), n(h));
        let mut svg = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{x} {y} {w} {h}\" version=\"1.1\">\n"
        );
        match background.unwrap_or("white") {
            "transparent" => {}
            fill => {
                let _ = writeln!(
                    svg,
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{fill}"/>"#
                );
            }
        }
        svg + &self.body + "</svg>\n"
    }
}

/// `x,y x,y …`.
fn path(points: &[(f64, f64)]) -> String {
    let all: Vec<String> = (points.iter())
        .map(|&(x, y)| format!("{},{}", n(x), n(y)))
        .collect();
    all.join(" ")
}

/// A number with at most 2 decimals and no trailing zeros.
fn n(v: f64) -> String {
    format!("{}", (v * 100.0).round() / 100.0 + 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_keep_two_decimals() {
        assert_eq!(n(12.0), "12");
        assert_eq!(n(-0.001), "0");
        assert_eq!(n(1.005_1), "1.01");
        assert_eq!(n(-3.5), "-3.5");
    }

    #[test]
    fn pages_are_found_by_number_name_or_id() {
        let doc = Document::parse(r#"<a><d id="d1" name="Order"/><d id="d2" name="Ship"/></a>"#);
        let doc = doc.unwrap();
        let diagrams: Vec<Node> = doc.root_element().children().collect();
        assert_eq!(find(&diagrams, "2"), Some(1));
        assert_eq!(find(&diagrams, "Ship"), Some(1));
        assert_eq!(find(&diagrams, "d1"), Some(0));
        assert_eq!(find(&diagrams, "3"), None);
        assert_eq!(find(&diagrams, "0"), None);
        assert_eq!(find(&diagrams, "ship"), None);
    }
}
