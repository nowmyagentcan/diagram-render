//! draw.io diagrams (`.drawio`, `.drawio.svg`, `.drawio.png`), drawn from the positions the file
//! stores, in the order draw.io paints them: each cell after its parent, children in file order.

mod edges;
mod label;
mod read;
mod shapes;
mod style;

use std::fmt::Write;

use roxmltree::Node;

use crate::engine::{Checked, Page, Rendered, Task};
use crate::error::{Error, Warning};
use crate::input::Lines;
use crate::raster::Widths;
use crate::svg::{escape, num as n};
use read::{Cell, Content, P};
use style::Style;

/// The most cells one page may have.
const LIMIT: usize = 50_000;

/// A box: left, top, width and height.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }

    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
}

/// A vertex as drawn: where it is and its style.
pub struct State {
    pub rect: Rect,
    pub style: Style,
}

pub fn render(task: &Task) -> Result<Rendered, Error> {
    // XML reads every line end as LF, and so lines are counted as everywhere else. A PNG's chunk
    // holds the file percent-encoded.
    let mut text = task.lines.text().replace("\r\n", "\n").replace('\r', "\n");
    if text.trim_start().starts_with('%') {
        text = read::percent_decode(text.trim());
    }
    let lines = Lines::new(&text);
    let doc = read::parse(&text).map_err(|e| {
        let (row, col) = (e.pos().row as usize, e.pos().col as usize);
        lines.place(
            Error::syntax_error(&read::message(&e), row, None),
            row,
            Some(col),
        )
    })?;
    let line = |n: Node| lines.locate(n.range().start).0;
    let at_line =
        |said: &str, row: usize| lines.place(Error::syntax_error(said, row, None), row, None);
    // A `.drawio.svg` carries the file in its root's `content`.
    let carried;
    let inner;
    let mut file = doc.root_element();
    // Lines inside the file an SVG carries are counted as the SVG's root element's.
    let mut carried_row = None;
    if file.has_tag_name("svg") {
        let row = line(file);
        carried_row = Some(row);
        carried = file.attribute("content").unwrap_or_default().to_string();
        inner = read::parse(&carried).map_err(|e| {
            at_line(
                &format!("{} in the diagram the SVG carries", read::message(&e)),
                row,
            )
        })?;
        file = inner.root_element();
    }
    let row_of = |n: Node| carried_row.unwrap_or_else(|| line(n));
    let diagrams: Vec<Node> = match file.tag_name().name() {
        "mxfile" => (file.children())
            .filter(|n| n.has_tag_name("diagram"))
            .collect(),
        "mxGraphModel" => vec![file],
        other => {
            let said = format!("this isn't a draw.io file: its root element is <{other}>");
            return Err(at_line(&said, row_of(file)));
        }
    };
    let pages: Vec<Page> = (diagrams.iter().enumerate())
        .map(|(i, d)| Page {
            index: Some(i + 1),
            name: (d.attribute("name").filter(|_| d.has_tag_name("diagram")))
                .map_or_else(|| format!("Page-{}", i + 1), Into::into),
        })
        .collect();
    let at = match task.page {
        None => 0,
        Some(p) => find(&diagrams, &pages, p).ok_or_else(|| {
            let names: Vec<&str> = pages.iter().map(|p| p.name.as_str()).collect();
            Error::page_not_found(task.name, p, &names)
        })?,
    };
    let mut warnings = vec![];
    if pages.len() > 1 && task.page.is_none() {
        warnings.push(Warning::pages_more(task.name, pages.len(), &pages[0].name));
    }
    let unpacked;
    let unpacked_doc;
    let mut packed_row = None;
    let model = match diagrams.get(at).map(|d| (d, read::content(*d))) {
        Some((_, Content::Model(m))) => Some(m),
        Some((d, Content::Packed(t))) => {
            let (row, name) = (row_of(*d), &pages[at].name);
            unpacked = read::decompress(t).map_err(|e| match e {
                None => Error::too_large_unpacked(task.name),
                Some(why) => at_line(&format!("page \"{name}\" can't be read: {why}"), row),
            })?;
            unpacked_doc = read::parse(&unpacked)
                .map_err(|e| at_line(&format!("{} in page \"{name}\"", read::message(&e)), row))?;
            packed_row = Some(row);
            Some(unpacked_doc.root_element())
        }
        _ => None,
    };
    let cells = match model {
        Some(m) => read::cells(m, &|n| packed_row.unwrap_or_else(|| row_of(n))),
        None => vec![],
    };
    if cells.len() > LIMIT {
        return Err(Error::too_many_elements(cells.len(), LIMIT));
    }
    let background = (model.and_then(|m| m.attribute("background")))
        .filter(|b| !b.is_empty() && *b != "none")
        .map(color);
    let mut d = Drawing {
        body: String::new(),
        extent: None,
        widths: Widths::load()?,
        warnings,
        approximated: vec![],
        removed: 0,
    };
    d.draw(&cells);
    if !d.approximated.is_empty() {
        let names: Vec<&str> = d.approximated.iter().take(5).map(String::as_str).collect();
        let count = d.approximated.len();
        d.warnings.push(Warning::shape_approximated(count, &names));
    }
    let page = Page {
        index: Some(at + 1),
        name: pages.get(at).map_or("Page-1".into(), |p| p.name.clone()),
    };
    let fill = task.background.map(str::to_string).or(background);
    let (warnings, removed) = (std::mem::take(&mut d.warnings), d.removed);
    Ok(Rendered {
        svg: d.finish(fill.as_deref()).into_bytes(),
        warnings,
        page: Some(page),
        pages,
        diagram_type: None,
        removed,
    })
}

/// Everything a render finds, without the drawing; `pages_more` belongs to a render only.
pub fn check(task: &Task) -> Result<Checked, Error> {
    let r = render(task)?;
    Ok(Checked {
        warnings: (r.warnings.into_iter())
            .filter(|w| w.code != "pages_more")
            .collect(),
        ..Checked::default()
    })
}

/// The page `--page` names: its number from 1, else its name or id.
fn find(diagrams: &[Node], pages: &[Page], wanted: &str) -> Option<usize> {
    if wanted.bytes().all(|b| b.is_ascii_digit()) {
        let n: usize = wanted.parse().ok()?;
        return (1..=pages.len().max(1)).contains(&n).then(|| n - 1);
    }
    let named = |(d, p): (&Node, &Page)| p.name == wanted || d.attribute("id") == Some(wanted);
    diagrams.iter().zip(pages).position(named)
}

/// A colour as SVG reads it: draw.io's `default` colours are its light theme's.
pub fn color(value: &str) -> String {
    let v = value.trim();
    let v = match v.strip_prefix("light-dark(") {
        Some(pair) => pair.split(',').next().unwrap_or_default().trim(),
        None => v,
    };
    escape(v)
}

/// The SVG being drawn, the box around everything in it, and what was found on the way.
struct Drawing {
    body: String,
    extent: Option<(P, P)>,
    widths: Widths,
    warnings: Vec<Warning>,
    /// The shapes drawn as plain boxes, each once.
    approximated: Vec<String>,
    /// Unsafe items left out of labels.
    removed: usize,
}

impl Drawing {
    /// Grows the drawing's box to hold the point.
    fn include(&mut self, (x, y): P) {
        let ((x0, y0), (x1, y1)) = self.extent.unwrap_or(((x, y), (x, y)));
        self.extent = Some(((x0.min(x), y0.min(y)), (x1.max(x), y1.max(y))));
    }

    fn include_rect(&mut self, r: Rect) {
        self.include((r.x, r.y));
        self.include((r.x + r.w, r.y + r.h));
    }

    /// Draws the page's cells: vertices placed first, as edges end on them, then everything in
    /// the order draw.io paints it.
    fn draw(&mut self, cells: &[Cell]) {
        let ids = read::by_id(cells);
        let mut parent = vec![None; cells.len()];
        let mut children = vec![vec![]; cells.len()];
        let mut roots = vec![];
        for (i, c) in cells.iter().enumerate() {
            match c.parent.map(|p| (p, ids.get(p).copied())) {
                None => roots.push(i),
                Some((_, Some(j))) if j != i => {
                    parent[i] = Some(j);
                    children[j].push(i);
                }
                Some((p, _)) => {
                    self.warnings.push(Warning::parent_missing(c.id, p, c.line));
                    roots.push(i);
                }
            }
        }
        // Cells in painting order, each shown or hidden by itself and its ancestors.
        let mut order = vec![];
        let mut pending: Vec<(usize, bool)> = roots.iter().rev().map(|&i| (i, true)).collect();
        let mut shown = vec![false; cells.len()];
        while let Some((i, visible)) = pending.pop() {
            let c = &cells[i];
            shown[i] = visible && c.visible;
            order.push(i);
            let open = shown[i] && !(c.collapsed && c.vertex);
            pending.extend(children[i].iter().rev().map(|&k| (k, open)));
        }
        let mut states: Vec<Option<State>> = (0..cells.len()).map(|_| None).collect();
        let mut origins = vec![(0.0, 0.0); cells.len()];
        for &i in &order {
            let c = &cells[i];
            let p = parent[i];
            if p.is_some_and(|p| cells[p].edge) {
                continue;
            }
            let base = p.map_or((0.0, 0.0), |p| origins[p]);
            let at = match (c.geo.relative, p.and_then(|p| states[p].as_ref())) {
                (true, Some(ps)) if c.vertex => (
                    base.0 + c.geo.x * ps.rect.w + c.geo.offset.0,
                    base.1 + c.geo.y * ps.rect.h + c.geo.offset.1,
                ),
                _ => (base.0 + c.geo.x, base.1 + c.geo.y),
            };
            origins[i] = at;
            if c.vertex {
                let style = self.style(&states, p, c);
                let rect = Rect {
                    x: at.0,
                    y: at.1,
                    w: c.geo.w,
                    h: c.geo.h,
                };
                states[i] = Some(State { rect, style });
            }
        }
        let mut routes: Vec<Option<Vec<P>>> = (0..cells.len()).map(|_| None).collect();
        for &i in &order {
            let c = &cells[i];
            if !shown[i] {
                continue;
            }
            if c.edge {
                let style = self.style(&states, parent[i], c);
                let Some(points) =
                    self.route(cells, &ids, &parent, &shown, &states, i, &style, origins[i])
                else {
                    continue;
                };
                self.edge(c, &style, &points);
                routes[i] = Some(points);
            } else if let Some(p) = parent[i].filter(|&p| cells[p].edge && c.vertex) {
                // A label on an edge: placed along the edge's route.
                let Some(route) = &routes[p] else {
                    continue;
                };
                let (x, y) = edges::along(route, &c.geo);
                let style = self.style(&states, Some(p), c);
                let rect = Rect {
                    x,
                    y,
                    w: c.geo.w,
                    h: c.geo.h,
                };
                self.vertex(c, &State { rect, style });
            } else if let Some(state) = &states[i] {
                self.vertex(c, state);
            }
        }
    }

    /// A cell's style, with `inherit` colours taken from its parent's.
    fn style(&self, states: &[Option<State>], parent: Option<usize>, c: &Cell) -> Style {
        let mut style = Style::parse(c.style, c.edge);
        let from = parent.and_then(|p| states[p].as_ref().map(|s| &s.style));
        for key in ["strokeColor", "fillColor", "fontColor"] {
            if style.get(key) == Some("inherit") {
                match from.and_then(|s| s.get(key)) {
                    Some(v) => style.set(key, v),
                    None => style.unset(key),
                }
            }
        }
        style
    }

    /// The points an edge is drawn through, or `None` when it can't be drawn.
    #[allow(clippy::too_many_arguments)]
    fn route(
        &mut self,
        cells: &[Cell],
        ids: &std::collections::HashMap<&str, usize>,
        parent: &[Option<usize>],
        shown: &[bool],
        states: &[Option<State>],
        i: usize,
        style: &Style,
        origin: P,
    ) -> Option<Vec<P>> {
        let c = &cells[i];
        let mut ends = [None, None];
        for (k, (terminal, point)) in [(c.source, c.geo.source), (c.target, c.geo.target)]
            .into_iter()
            .enumerate()
        {
            match terminal.map(|t| (t, ids.get(t).copied())) {
                Some((_, Some(t))) => {
                    if !shown[t] {
                        return None;
                    }
                    // A terminal inside a collapsed cell ends on that cell.
                    let mut best = t;
                    let mut up = parent[t];
                    while let Some(a) = up {
                        if cells[a].collapsed && cells[a].vertex {
                            best = a;
                        }
                        up = parent[a];
                    }
                    ends[k] = Some(edges::End::Cell(states[best].as_ref()?));
                }
                Some((id, None)) if point.is_none() => {
                    let kind = if k == 0 { "source" } else { "target" };
                    let reason = format!("its {kind} {id} isn't on the page");
                    self.warnings.push(Warning::skipped(
                        "edge_skipped",
                        "edge",
                        c.id,
                        &reason,
                        c.line,
                    ));
                    return None;
                }
                _ => match point {
                    Some((x, y)) => ends[k] = Some(edges::End::Point((origin.0 + x, origin.1 + y))),
                    None => {
                        let reason = "it has no source or target";
                        self.warnings.push(Warning::skipped(
                            "edge_skipped",
                            "edge",
                            c.id,
                            reason,
                            c.line,
                        ));
                        return None;
                    }
                },
            }
        }
        let [Some(source), Some(target)] = ends else {
            return None;
        };
        let points: Vec<P> = c
            .geo
            .points
            .iter()
            .map(|p| (p.0 + origin.0, p.1 + origin.1))
            .collect();
        Some(edges::route(style, source, target, &points))
    }

    fn edge(&mut self, c: &Cell, style: &Style, points: &[P]) {
        for &p in points {
            self.include(p);
        }
        let mut out = String::new();
        edges::draw(&mut out, style, points);
        self.body += &out;
        if !c.label.trim().is_empty() {
            let anchor = edges::along(points, &read::Geo::default());
            let at = Rect {
                x: anchor.0,
                y: anchor.1,
                w: 0.0,
                h: 0.0,
            };
            self.label(&c.label, style, at, true);
        }
    }

    fn vertex(&mut self, c: &Cell, state: &State) {
        let mut out = String::new();
        if let Some(name) = shapes::draw(&mut out, state)
            && !self.approximated.contains(&name)
        {
            self.approximated.push(name);
        }
        if !out.is_empty() {
            self.include_rect(state.rect);
        }
        self.body += &out;
        if !c.label.trim().is_empty() {
            self.label(&c.label, &state.style, state.rect, false);
        }
    }

    fn label(&mut self, text: &str, style: &Style, at: Rect, edge: bool) {
        let mut out = String::new();
        let drawn = label::draw(&mut out, &self.widths, text, style, at, edge);
        if let Some(b) = drawn {
            self.include_rect(b);
        }
        self.body += &out;
    }

    /// The whole SVG: everything drawn plus 1 px, on the page background when there is one.
    fn finish(self, background: Option<&str>) -> String {
        let ((x, y), (x1, y1)) = self.extent.unwrap_or_default();
        let (x, y) = ((x - 1.0).floor(), (y - 1.0).floor());
        let (w, h) = ((x1 + 1.0).ceil() - x, (y1 + 1.0).ceil() - y);
        let (x, y, w, h) = (n(x), n(y), n(w), n(h));
        let mut svg = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"{x} {y} {w} {h}\" version=\"1.1\">\n"
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
