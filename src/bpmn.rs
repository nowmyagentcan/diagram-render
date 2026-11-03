//! BPMN 2.0 files, drawn from their diagram section (BPMN DI): every shape at its bounds and
//! every edge along its waypoints, with labels laid out as bpmn-js lays them out. Files without
//! a diagram section aren't laid out.

use std::collections::HashMap;
use std::fmt::Write;

use roxmltree::{Document, Node, ParsingOptions};

use crate::engine::{Checked, Page, Rendered, Task};
use crate::error::{Error, Warning};
use crate::input::Lines;
use crate::raster::Widths;
use crate::svg::{escape, num as n};

mod shapes;

use shapes::Paint;

const MODEL: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";
const BPMNDI: &str = "http://www.omg.org/spec/BPMN/20100524/DI";
const DC: &str = "http://www.omg.org/spec/DD/20100524/DC";
const DI: &str = "http://www.omg.org/spec/DD/20100524/DI";
const BIOC: &str = "http://bpmn.io/schema/bpmn/biocolor/1.0";
const COLOR: &str = "http://www.omg.org/spec/BPMN/non-normative/color/1.0";

/// The most shapes and edges one diagram may have.
const LIMIT: usize = 20_000;
/// Space around the drawing, as bpmn-js leaves when it saves an SVG.
const PAD: f64 = 5.0;
/// bpmn-js's ink.
const INK: &str = "#22242a";

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
    // What can be drawn, warned about in the file's order.
    let mut drawn = vec![];
    for item in items {
        let id = item.attribute("id").unwrap_or("?");
        let reference = item.attribute("bpmnElement").unwrap_or_default();
        let Some(&el) = model.get(reference) else {
            warnings.push(Warning::di_reference_missing(id, reference, line(item)));
            continue;
        };
        let place = match item.tag_name().name() {
            "BPMNShape" => bounds(item).map(Place::Shape).ok_or("shape"),
            _ => match waypoints(item) {
                points if points.len() > 1 => Ok(Place::Edge(points)),
                _ => Err("edge"),
            },
        };
        match place {
            Ok(place) => drawn.push((item, el, place)),
            Err("shape") => warnings.push(Warning::skipped(
                "element_skipped",
                "shape",
                id,
                "it has no bounds",
                line(item),
            )),
            Err(_) => warnings.push(Warning::skipped(
                "edge_skipped",
                "edge",
                id,
                "it has fewer than 2 waypoints",
                line(item),
            )),
        }
    }
    // Pools, then lanes, then the other shapes, then the edges, as bpmn-js layers them.
    drawn.sort_by_key(|(_, el, place)| match (place, el.tag_name().name()) {
        (Place::Shape(_), "participant") => 0,
        (Place::Shape(_), "lane") => 1,
        (Place::Shape(_), _) => 2,
        (Place::Edge(_), _) => 3,
    });
    let mut d = Drawing {
        svg: Svg::default(),
        widths: Widths::load()?,
        labels: vec![],
        approximated: vec![],
    };
    for (di, el, place) in &drawn {
        d.item(&model, *el, *di, place);
    }
    for (text, b, color) in std::mem::take(&mut d.labels) {
        let style = Style {
            size: 11.0,
            pad: 0.0,
            align: Align::Top,
            color: &color,
        };
        d.text(&text, Frame::Straight(b.x, b.y), b.w, b.h, &style);
    }
    if !d.approximated.is_empty() {
        let mut types: Vec<&str> = d.approximated.iter().map(String::as_str).collect();
        types.sort_unstable();
        types.dedup();
        warnings.push(Warning::element_approximated(d.approximated.len(), &types));
    }
    let page = Page {
        index: Some(at + 1),
        name: pages[at].name.clone(),
    };
    Ok(Rendered {
        svg: d.svg.finish(task.background).into_bytes(),
        warnings,
        page: Some(page),
        pages,
        diagram_type: Some(diagram_type),
        removed: 0,
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

/// Where the diagram section puts an item.
enum Place {
    Shape(Rect),
    Edge(Vec<(f64, f64)>),
}

/// The drawing so far, the labels drawn last (above everything), and the types of elements drawn
/// as plain boxes.
struct Drawing {
    svg: Svg,
    widths: Widths,
    labels: Vec<(String, Rect, String)>,
    approximated: Vec<String>,
}

impl Drawing {
    fn item(&mut self, model: &HashMap<&str, Node>, el: Node, di: Node, place: &Place) {
        self.svg.body.push('\n');
        let attr = |ns: &str, key: &str| di.attribute((ns, key)).map(escape);
        let fill = attr(COLOR, "background-color").or_else(|| attr(BIOC, "fill"));
        let stroke = attr(COLOR, "border-color").or_else(|| attr(BIOC, "stroke"));
        let (fill, stroke) = (fill.unwrap_or("white".into()), stroke.unwrap_or(INK.into()));
        let label = di.children().find(|c| is(*c, BPMNDI, "BPMNLabel"));
        let color = (label
            .and_then(|l| l.attribute((COLOR, "color")))
            .map(escape))
        .unwrap_or_else(|| stroke.clone());
        let p = Paint {
            fill: &fill,
            stroke: &stroke,
        };
        let kind = el.tag_name().name();
        let name = el.attribute("name").unwrap_or_default();
        let inner = |size| Style {
            size,
            pad: 7.0,
            align: Align::Middle,
            color: &color,
        };
        match place {
            Place::Edge(points) => {
                for &(x, y) in points {
                    self.svg.include(x, y);
                }
                if !shapes::edge(&mut self.svg.body, model, el, points, &p) {
                    self.approximated.push(kind.into());
                    let line = format!("M{}", path(points));
                    shapes::path(&mut self.svg.body, &line, "none", p.stroke, 2.0, "");
                }
                if matches!(kind, "sequenceFlow" | "messageFlow") {
                    self.external(name, label, flow_middle(points), color);
                }
            }
            Place::Shape(b) => {
                let b = *b;
                self.svg.include_box(b);
                match kind {
                    "participant" | "lane" => {
                        let across = di.attribute("isHorizontal") != Some("false");
                        let expanded = kind == "lane"
                            || el.attribute("processRef").is_some()
                                && di.attribute("isExpanded") != Some("false");
                        shapes::swimlane(&mut self.svg.body, el, b, &p, expanded, across);
                        let header = Style {
                            pad: 0.0,
                            ..inner(12.0)
                        };
                        // An expanded pool's or a lane's name is in its 30 px header, read upwards
                        // along a horizontal one; a collapsed pool's fills it.
                        let turned = Frame::Turned(b.x, b.y + b.h);
                        let straight = Frame::Straight(b.x, b.y);
                        match (expanded, across) {
                            (true, true) => self.text(name, turned, b.h, 30.0, &header),
                            (true, false) => self.text(name, straight, b.w, 30.0, &header),
                            (false, true) => self.text(name, straight, b.w, b.h, &header),
                            (false, false) => self.text(name, turned, b.h, b.w, &header),
                        }
                    }
                    "textAnnotation" => {
                        shapes::annotation(&mut self.svg.body, b, &p);
                        let text = el.children().find(|c| c.tag_name().name() == "text");
                        let style = Style {
                            align: Align::TopLeft,
                            ..inner(12.0)
                        };
                        let text = text.and_then(|t| t.text()).unwrap_or_default();
                        self.text(text, Frame::Straight(b.x, b.y), b.w, b.h, &style);
                    }
                    "group" => {
                        shapes::group(&mut self.svg.body, b, &p);
                        let value = (el.attribute("categoryValueRef"))
                            .and_then(|r| model.get(r)?.attribute("value"));
                        let top = (b.x + b.w / 2.0, b.y + 10.0);
                        self.external(value.unwrap_or_default(), label, top, color);
                    }
                    _ if shapes::flow_node(&mut self.svg.body, model, el, di, b, &p) => {
                        if shapes::is_activity(kind) {
                            let expanded = di.attribute("isExpanded") == Some("true")
                                && kind != "callActivity";
                            let style = Style {
                                align: if expanded { Align::Top } else { Align::Middle },
                                ..inner(12.0)
                            };
                            self.text(name, Frame::Straight(b.x, b.y), b.w, b.h, &style);
                        } else {
                            let below = (b.x + b.w / 2.0, b.y + b.h + 10.0);
                            self.external(name, label, below, color);
                        }
                    }
                    _ => {
                        self.approximated.push(kind.into());
                        shapes::rect(&mut self.svg.body, b, 10.0, p.fill, p.stroke, 2.0, "");
                        let style = inner(12.0);
                        self.text(name, Frame::Straight(b.x, b.y), b.w, b.h, &style);
                    }
                }
            }
        }
    }

    /// Keeps an outside label to draw last, in the box bpmn-js gives it: its `BPMNLabel` bounds
    /// made as tall as the text, else the text's own size centred on `middle`.
    fn external(&mut self, text: &str, label: Option<Node>, middle: (f64, f64), color: String) {
        if text.trim().is_empty() {
            return;
        }
        let given = label.and_then(bounds).filter(|b| b.w > 0.0);
        let lines = layout(&self.widths, text, 11.0, given.map_or(90.0, |b| b.w));
        let height = (lines.len() as f64 * 13.2).ceil();
        let b = match given {
            Some(b) => Rect {
                x: b.x.round(),
                y: b.y.round(),
                w: b.w.round(),
                h: height,
            },
            None => {
                let width = lines.iter().map(|l| l.1).fold(0.0, f64::max);
                Rect {
                    x: (middle.0 - width / 2.0).round(),
                    y: (middle.1 - 10.0).round(),
                    w: width.ceil(),
                    h: height,
                }
            }
        };
        self.svg.include_box(b);
        self.labels.push((text.into(), b, color));
    }

    /// Writes `text` laid out in a `w` × `h` box placed by `frame`, and counts its lines in the
    /// drawing's box as SVG's `getBBox` measures text: from the font's ascent to its descent.
    fn text(&mut self, text: &str, frame: Frame, w: f64, h: f64, s: &Style) {
        if text.trim().is_empty() {
            return;
        }
        let lines = layout(&self.widths, text, s.size, w - 2.0 * s.pad);
        let height = 1.2 * s.size;
        let pad_top = if s.align == Align::Middle { 0.0 } else { s.pad };
        let mut y = pad_top - height / 4.0;
        if s.align == Align::Middle {
            y += (h - lines.len() as f64 * height) / 2.0;
        }
        let (transform, at) = match frame {
            Frame::Straight(x, y) => (String::new(), (x, y)),
            Frame::Turned(x, y) => (
                format!(r#" transform="translate({} {}) rotate(-90)""#, n(x), n(y)),
                (0.0, 0.0),
            ),
        };
        let _ = write!(
            self.svg.body,
            r#"<text font-family="Arial, Liberation Sans, sans-serif" font-size="{}" fill="{}"{transform}>"#,
            n(s.size),
            s.color
        );
        for (line, width) in &lines {
            y += height;
            let x = match s.align {
                Align::TopLeft => s.pad,
                _ => ((w - 2.0 * s.pad - width) / 2.0 + s.pad).max(0.0),
            };
            let (lx, ly) = (at.0 + x, at.1 + y);
            // Browsers round the font's ascent and descent to whole pixels.
            let (ascent, descent) = ((0.905 * s.size).round(), (0.212 * s.size).round());
            for (cx, cy) in [(lx, ly - ascent), (lx + width, ly + descent)] {
                let (px, py) = match frame {
                    Frame::Straight(..) => (cx, cy),
                    Frame::Turned(fx, fy) => (fx + cy, fy - cx),
                };
                self.svg.include(px, py);
            }
            let _ = write!(
                self.svg.body,
                r#"<tspan x="{}" y="{}">{}</tspan>"#,
                n(lx),
                n(ly),
                escape(line)
            );
        }
        self.svg.body.push_str("</text>\n");
    }
}

/// How text sits in its box.
#[derive(Clone, Copy, PartialEq)]
enum Align {
    TopLeft,
    Top,
    Middle,
}

/// A label's look: font size, padding on each side, alignment and colour.
struct Style<'a> {
    size: f64,
    pad: f64,
    align: Align,
    color: &'a str,
}

/// Where a text box's top-left corner is: at a point, or for a name read upwards along a pool
/// or lane, at the bottom-left of its header.
#[derive(Clone, Copy)]
enum Frame {
    Straight(f64, f64),
    Turned(f64, f64),
}

/// Where a flow's label goes without bounds: beside the middle of its middle segment.
fn flow_middle(points: &[(f64, f64)]) -> (f64, f64) {
    let mid = points.len() as f64 / 2.0 - 1.0;
    let (a, b) = (
        points[mid.floor() as usize],
        points[(mid + 0.01).ceil() as usize],
    );
    let (x, y) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    match (b.1 - a.1).abs() <= (b.0 - a.0).abs() {
        true => (x, y - 15.0),
        false => (x + 15.0, y),
    }
}

/// `text` broken into lines no wider than `max` at `size` px, with each line's width: at line
/// breaks, else at the last space or hyphen that fits, else inside the word.
fn layout(widths: &Widths, text: &str, size: f64, max: f64) -> Vec<(String, f64)> {
    let mut pending: Vec<String> = text.split('\n').map(Into::into).collect();
    pending.reverse();
    let mut lines = vec![];
    while let Some(original) = pending.pop() {
        let mut fit = original.clone();
        loop {
            let width = widths.of(fit.trim_end(), size);
            let chars = fit.chars().count();
            if fit.trim().is_empty() || width <= max || chars < 2 {
                if fit.len() < original.len() {
                    pending.push(original[fit.len()..].trim().into());
                }
                lines.push((fit.trim_end().to_string(), width));
                break;
            }
            fit = shorten(&fit, width, max);
        }
    }
    lines
}

/// The start of `line` (`width` px wide) that should fit in `max`, by the share of its
/// characters that fit: up to a space or hyphen when there is one, else cut.
fn shorten(line: &str, width: f64, max: f64) -> String {
    let keep = (line.chars().count() as f64 * max / width).max(1.0);
    let mut parts = vec![];
    let mut part = String::new();
    for c in line.chars() {
        if c.is_whitespace() || c == '-' {
            parts.push(std::mem::take(&mut part));
            parts.push(c.to_string());
        } else {
            part.push(c);
        }
    }
    parts.push(part);
    let mut kept = String::new();
    let mut length = 0;
    if parts.len() > 1 {
        let mut last = "";
        for part in &parts {
            let n = part.chars().count();
            if part.is_empty() || (n + length) as f64 >= keep {
                // A hyphen stays with the part before it.
                if part == "-" {
                    kept.truncate(kept.len() - last.len());
                }
                break;
            }
            kept.push_str(part);
            length += n;
            last = part;
        }
    }
    if kept.is_empty() {
        let n = ((keep - 1.0).round() as usize).max(1);
        kept = line.chars().take(n).collect();
    }
    kept
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

/// A number attribute, when it is one.
fn number(n: Node, key: &str) -> Option<f64> {
    let v = n.attribute(key)?.trim().parse::<f64>().ok();
    v.filter(|v| v.is_finite())
}

/// The `dc:Bounds` of a shape or label.
fn bounds(n: Node) -> Option<Rect> {
    let b = n.children().find(|c| is(*c, DC, "Bounds"))?;
    Some(Rect {
        x: number(b, "x")?,
        y: number(b, "y")?,
        w: number(b, "width")?.max(0.0),
        h: number(b, "height")?.max(0.0),
    })
}

/// An edge's `di:waypoint`s.
fn waypoints(n: Node) -> Vec<(f64, f64)> {
    (n.children().filter(|c| is(*c, DI, "waypoint")))
        .filter_map(|w| Some((number(w, "x")?, number(w, "y")?)))
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

    fn include_box(&mut self, b: Rect) {
        self.include(b.x, b.y);
        self.include(b.x + b.w, b.y + b.h);
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
        svg += "<g stroke-linecap=\"round\" stroke-linejoin=\"round\">\n";
        svg + &self.body + "</g>\n</svg>\n"
    }
}

/// `x,y x,y …`.
fn path(points: &[(f64, f64)]) -> String {
    let all: Vec<String> = (points.iter())
        .map(|&(x, y)| format!("{},{}", n(x), n(y)))
        .collect();
    all.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
