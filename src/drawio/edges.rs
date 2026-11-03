//! draw.io's edges: where they end on their terminals and the route between, ported from
//! mxGraph's `mxGraphView`, `mxEdgeStyle`, `mxPerimeter` and `mxMarker` as draw.io v32.6.9 ships
//! them (Apache-2.0, https://github.com/jgraph/drawio, src/main/webapp/mxgraph/src).

use std::f64::consts::FRAC_1_SQRT_2;
use std::fmt::Write;

use super::read::{Geo, P};
use super::shapes::{self, Path, paint};
use super::style::Style;
use super::{Rect, State};
use crate::svg::num as n;

/// Where an edge ends: on a vertex, or at a point.
#[derive(Clone, Copy)]
pub enum End<'a> {
    Cell(&'a State),
    Point(P),
}

impl<'a> End<'a> {
    fn cell(self) -> Option<&'a State> {
        match self {
            End::Cell(s) => Some(s),
            End::Point(_) => None,
        }
    }
}

/// What the route found to approximate: the edge style or arrow and what it was drawn as.
pub type Approximated = (&'static str, String, &'static str);

const WEST: i32 = 1;
const NORTH: i32 = 2;
const SOUTH: i32 = 4;
const EAST: i32 = 8;
const ALL: i32 = 15;

/// The routing styles mxGraph has, by their draw.io names.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Orthogonal,
    Elbow,
    SideToSide,
    TopToBottom,
    Segment,
    EntityRelation,
    Loop,
}

fn kind(style: &Style, loops: bool, notes: &mut Vec<Approximated>) -> Option<Kind> {
    if loops {
        return Some(Kind::Loop);
    }
    if style.on("noEdgeStyle") {
        return None;
    }
    match style.get("edgeStyle")? {
        "orthogonalEdgeStyle" => Some(Kind::Orthogonal),
        "elbowEdgeStyle" => Some(Kind::Elbow),
        "sideToSideEdgeStyle" => Some(Kind::SideToSide),
        "topToBottomEdgeStyle" => Some(Kind::TopToBottom),
        "segmentEdgeStyle" => Some(Kind::Segment),
        "entityRelationEdgeStyle" => Some(Kind::EntityRelation),
        "loopEdgeStyle" => Some(Kind::Loop),
        "none" => None,
        other => {
            notes.push(("edge_style_approximated", other.into(), "straight lines"));
            None
        }
    }
}

/// The points from the source's end to the target's: fixed ends first, then the route the
/// edge style makes through the waypoints, then the ends left floating on the terminals'
/// perimeters, as `mxGraphView.updateEdgeState` does.
pub fn route<'a>(
    style: &Style,
    source: End<'a>,
    target: End<'a>,
    hints: &[P],
    notes: &mut Vec<Approximated>,
) -> Vec<P> {
    let (src, trg) = (source.cell(), target.cell());
    let fixed = |e: End, exit: bool| match e {
        End::Point(p) => Some(p),
        End::Cell(s) => constraint(style, exit).map(|c| connection_point(s, c)),
    };
    let (p0, pe) = (fixed(source, true), fixed(target, false));
    let same = matches!((src, trg), (Some(a), Some(b)) if std::ptr::eq(a, b));
    let loops = same && hints.len() < 2;
    let kind = kind(style, loops, notes);
    let mut pts: Vec<Option<P>> = vec![p0];
    match kind {
        Some(k) => {
            let view = View {
                style,
                src,
                trg,
                p0,
                pe,
                hints,
            };
            match k {
                Kind::Orthogonal => view.orthogonal(&mut pts),
                Kind::Segment => view.segment(&mut pts),
                Kind::Elbow => view.elbow(&mut pts),
                Kind::SideToSide => view.side_to_side(&mut pts),
                Kind::TopToBottom => view.top_to_bottom(&mut pts),
                Kind::EntityRelation => view.entity_relation(&mut pts),
                Kind::Loop => view.loop_route(&mut pts),
            }
        }
        None => pts.extend(hints.iter().map(|&p| Some(p))),
    }
    pts.push(pe);
    let orth = match style.get("orthogonal") {
        Some(v) => v == "1",
        None => kind.is_some_and(|k| k != Kind::Loop),
    };
    let count = pts.len();
    if pe.is_none()
        && let Some(t) = trg
    {
        let next = pts[count - 2].unwrap_or_else(|| center_of(source));
        pts[count - 1] = Some(floating(t, next, orth, spacing(style, false)));
    }
    if p0.is_none()
        && let Some(s) = src
    {
        let next = pts[1].unwrap_or_else(|| center_of(target));
        pts[0] = Some(floating(s, next, orth, spacing(style, true)));
    }
    let mut pts: Vec<P> = pts.into_iter().flatten().collect();
    // Orthogonal routes drop points that repeat.
    let removes = matches!(
        kind,
        Some(Kind::Elbow | Kind::SideToSide | Kind::TopToBottom | Kind::EntityRelation)
    ) || (kind == Some(Kind::Orthogonal) && hints.is_empty());
    if removes && pts.len() > 2 {
        let close = |a: P, b: P| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01;
        let mut i = pts.len() - 2;
        while i > 0 && pts.len() > 2 {
            if close(pts[i], pts[i + 1]) || close(pts[i], pts[i - 1]) {
                pts.remove(i);
            }
            i -= 1;
        }
    }
    pts
}

fn center_of(e: End) -> P {
    match e {
        End::Cell(s) => (s.rect.cx(), s.rect.cy()),
        End::Point(p) => p,
    }
}

fn spacing(style: &Style, source: bool) -> f64 {
    let own = if source {
        "sourcePerimeterSpacing"
    } else {
        "targetPerimeterSpacing"
    };
    style.num("perimeterSpacing", 0.0) + style.num(own, 0.0)
}

/// An `exitX`/`exitY` (or `entry`) point with its offsets and whether it sits on the perimeter.
struct Constraint {
    point: P,
    d: P,
    perimeter: bool,
}

fn constraint(style: &Style, exit: bool) -> Option<Constraint> {
    let side = if exit { "exit" } else { "entry" };
    let x = style.get(&format!("{side}X"))?.parse::<f64>().ok()?;
    let y = style.get(&format!("{side}Y"))?.parse::<f64>().ok()?;
    let d = |k: &str| style.num(&format!("{side}{k}"), 0.0);
    Some(Constraint {
        point: (x, y),
        d: (d("Dx"), d("Dy")),
        perimeter: style.not_off(&format!("{side}Perimeter")),
    })
}

/// mxGraph's `getConnectionPoint`.
fn connection_point(s: &State, c: Constraint) -> P {
    let st = &s.style;
    let bounds = grow(s.rect, st.num("perimeterSpacing", 0.0));
    let center = (bounds.cx(), bounds.cy());
    let direction = st.get("direction");
    let vertical = matches!(direction, Some("north" | "south"));
    let b = match vertical {
        true => Rect {
            x: center.0 - bounds.h / 2.0,
            y: center.1 - bounds.w / 2.0,
            w: bounds.h,
            h: bounds.w,
        },
        false => bounds,
    };
    let mut p = (b.x + c.point.0 * b.w + c.d.0, b.y + c.point.1 * b.h + c.d.1);
    let (mut fh, mut fv) = (st.on("flipH"), st.on("flipV"));
    if vertical {
        (fh, fv) = (fv, fh);
    }
    if fh {
        p.0 = 2.0 * b.cx() - p.0;
    }
    if fv {
        p.1 = 2.0 * b.cy() - p.1;
    }
    let turn = match (direction, st.not_off("anchorPointDirection")) {
        (Some("north"), true) => 270.0,
        (Some("west"), true) => 180.0,
        (Some("south"), true) => 90.0,
        _ => 0.0,
    };
    if turn != 0.0 {
        p = rotate(p, turn, center, true);
    }
    if c.perimeter {
        p = perimeter_point(s, p, false, 0.0);
    }
    let r = st.num("rotation", 0.0);
    if r != 0.0 {
        p = rotate(p, r, center, false);
    }
    p
}

fn grow(r: Rect, by: f64) -> Rect {
    Rect {
        x: r.x - by,
        y: r.y - by,
        w: r.w + 2.0 * by,
        h: r.h + 2.0 * by,
    }
}

/// `p` turned `degrees` clockwise around `c`; quarter turns with exact sines and cosines.
fn rotate((x, y): P, degrees: f64, (cx, cy): P, exact: bool) -> P {
    let a = degrees.to_radians();
    let (mut cos, mut sin) = (a.cos(), a.sin());
    if exact {
        (cos, sin) = (cos.round(), sin.round());
    }
    let (dx, dy) = (x - cx, y - cy);
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

/// mxGraphView's `getFloatingTerminalPoint`: on the perimeter towards `next`, with the shape's
/// rotation undone and done again.
fn floating(s: &State, next: P, orth: bool, border: f64) -> P {
    let rotation = s.style.num("rotation", 0.0);
    let center = (s.rect.cx(), s.rect.cy());
    let quarter = rotation % 90.0 == 0.0;
    let next = match rotation {
        0.0 => next,
        r => rotate(next, -r, center, quarter),
    };
    let p = perimeter_point(s, next, quarter && orth, border);
    match rotation {
        0.0 => p,
        r => rotate(p, r, center, quarter),
    }
}

/// mxGraphView's `getPerimeterPoint`: where a line towards `next` meets the vertex's perimeter.
fn perimeter_point(s: &State, next: P, orth: bool, border: f64) -> P {
    let st = &s.style;
    let b = grow(s.rect, border + st.num("perimeterSpacing", 0.0));
    let Some(kind) = st.get("perimeter") else {
        return (s.rect.cx(), s.rect.cy());
    };
    if b.w <= 0.0 && b.h <= 0.0 {
        return (s.rect.cx(), s.rect.cy());
    }
    let (fh, fv) = (st.on("flipH"), st.on("flipV"));
    let flip = |(x, y): P| {
        (
            if fh { 2.0 * b.cx() - x } else { x },
            if fv { 2.0 * b.cy() - y } else { y },
        )
    };
    let next = flip(next);
    let p = match kind {
        "ellipsePerimeter" => ellipse_perimeter(b, next, orth),
        "rhombusPerimeter"
        | "trianglePerimeter"
        | "hexagonPerimeter"
        | "hexagonPerimeter2"
        | "parallelogramPerimeter"
        | "trapezoidPerimeter"
        | "stepPerimeter" => {
            let shape = match kind {
                "rhombusPerimeter" => "rhombus",
                "trianglePerimeter" => "triangle",
                "parallelogramPerimeter" => "parallelogram",
                "trapezoidPerimeter" => "trapezoid",
                "stepPerimeter" => "step",
                _ => "hexagon",
            };
            let mut pst = st.clone();
            pst.set("shape", shape);
            pst.unset("flipH");
            pst.unset("flipV");
            match shapes::polygon(&pst, b) {
                Some(poly) => polygon_perimeter(&poly, b, next, orth),
                None => rectangle_perimeter(b, next, orth),
            }
        }
        _ => rectangle_perimeter(b, next, orth),
    };
    flip(p)
}

/// mxPerimeter's `RectanglePerimeter`.
fn rectangle_perimeter(b: Rect, next: P, orth: bool) -> P {
    let (cx, cy) = (b.cx(), b.cy());
    let alpha = (next.1 - cy).atan2(next.0 - cx);
    let pi = std::f64::consts::PI;
    let beta = pi / 2.0 - alpha;
    let t = b.h.atan2(b.w);
    let mut p = if alpha < -pi + t || alpha > pi - t {
        (b.x, cy - b.w * alpha.tan() / 2.0)
    } else if alpha < -t {
        (cx - b.h * beta.tan() / 2.0, b.y)
    } else if alpha < t {
        (b.x + b.w, cy + b.w * alpha.tan() / 2.0)
    } else {
        (cx + b.h * beta.tan() / 2.0, b.y + b.h)
    };
    if orth {
        if next.0 >= b.x && next.0 <= b.x + b.w {
            p.0 = next.0;
        } else if next.1 >= b.y && next.1 <= b.y + b.h {
            p.1 = next.1;
        }
        if next.0 < b.x {
            p.0 = b.x;
        } else if next.0 > b.x + b.w {
            p.0 = b.x + b.w;
        }
        if next.1 < b.y {
            p.1 = b.y;
        } else if next.1 > b.y + b.h {
            p.1 = b.y + b.h;
        }
    }
    p
}

/// mxPerimeter's `EllipsePerimeter`.
fn ellipse_perimeter(b: Rect, (px, py): P, orth: bool) -> P {
    let (a, bb) = (b.w / 2.0, b.h / 2.0);
    let (cx, cy) = (b.x + a, b.y + bb);
    let (dx, dy) = ((px - cx).trunc(), (py - cy).trunc());
    if dx == 0.0 && dy != 0.0 {
        return (cx, cy + bb * dy.signum());
    }
    if dx == 0.0 && dy == 0.0 {
        return (px, py);
    }
    if orth {
        if py >= b.y && py <= b.y + b.h {
            let ty = py - cy;
            let tx = (a * a * (1.0 - ty * ty / (bb * bb))).sqrt();
            let tx = if tx.is_nan() { 0.0 } else { tx };
            return (if px <= b.x { cx - tx } else { cx + tx }, py);
        }
        if px >= b.x && px <= b.x + b.w {
            let tx = px - cx;
            let ty = (bb * bb * (1.0 - tx * tx / (a * a))).sqrt();
            let ty = if ty.is_nan() { 0.0 } else { ty };
            return (px, if py <= b.y { cy - ty } else { cy + ty });
        }
    }
    let t = a * bb / (bb * bb * dx * dx + a * a * dy * dy).sqrt();
    (cx + t * dx, cy + t * dy)
}

/// Where the line from the shape towards `next` crosses a polygon's outline: from the centre,
/// or for orthogonal ends straight across from `next` when it is beside or above the shape.
fn polygon_perimeter(poly: &[P], b: Rect, next: P, orth: bool) -> P {
    let (cx, cy) = (b.cx(), b.cy());
    let from = match orth {
        true if next.0 >= b.x && next.0 <= b.x + b.w => (next.0, cy),
        true if next.1 >= b.y && next.1 <= b.y + b.h => (cx, next.1),
        _ => (cx, cy),
    };
    let (dx, dy) = (next.0 - from.0, next.1 - from.1);
    if dx == 0.0 && dy == 0.0 {
        return next;
    }
    // The first crossing along the ray from `from` through `next`.
    let mut best: Option<(f64, P)> = None;
    for i in 0..poly.len() {
        let (a, c) = (poly[i], poly[(i + 1) % poly.len()]);
        let (ex, ey) = (c.0 - a.0, c.1 - a.1);
        let den = dx * ey - dy * ex;
        if den.abs() < 1e-12 {
            continue;
        }
        let t = ((a.0 - from.0) * ey - (a.1 - from.1) * ex) / den;
        let u = ((a.0 - from.0) * dy - (a.1 - from.1) * dx) / den;
        if t >= 0.0 && (-1e-9..=1.0 + 1e-9).contains(&u) && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, (from.0 + t * dx, from.1 + t * dy)));
        }
    }
    best.map_or((cx, cy), |(_, p)| p)
}

/// A terminal as the edge styles see it: its box rounded to a tenth and its rotation.
#[derive(Clone, Copy)]
struct Term {
    r: Rect,
    rotation: f64,
}

fn tenth(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn term(s: Option<&State>) -> Option<Term> {
    s.map(|s| Term {
        r: Rect {
            x: tenth(s.rect.x),
            y: tenth(s.rect.y),
            w: tenth(s.rect.w),
            h: tenth(s.rect.h),
        },
        rotation: s.style.num("rotation", 0.0),
    })
}

/// What an edge style is given: the edge's style, its terminals, its fixed ends and its
/// waypoints.
struct View<'a> {
    style: &'a Style,
    src: Option<&'a State>,
    trg: Option<&'a State>,
    p0: Option<P>,
    pe: Option<P>,
    hints: &'a [P],
}

fn routing_center(s: &State) -> P {
    let st = &s.style;
    (
        s.rect.cx() + st.num("routingCenterX", 0.0) * s.rect.w,
        s.rect.cy() + st.num("routingCenterY", 0.0) * s.rect.h,
    )
}

/// A terminal box, or a point standing in for one.
fn at_point(p: P) -> Rect {
    Rect {
        x: p.0,
        y: p.1,
        w: 0.0,
        h: 0.0,
    }
}

fn reverse(c: i32) -> i32 {
    ((c & WEST) << 3) | ((c & NORTH) << 1) | ((c & SOUTH) >> 1) | ((c & EAST) >> 3)
}

const DIR_VECTORS: [(f64, f64); 7] = [
    (-1.0, 0.0),
    (0.0, -1.0),
    (1.0, 0.0),
    (0.0, 1.0),
    (-1.0, 0.0),
    (0.0, -1.0),
    (1.0, 0.0),
];

const ROUTE_PATTERNS: [[&[i32]; 4]; 4] = [
    [
        &[513, 2308, 2081, 2562],
        &[513, 1090, 514, 2184, 2114, 2561],
        &[513, 1090, 514, 2564, 2184, 2562],
        &[513, 2308, 2561, 1090, 514, 2568, 2308],
    ],
    [
        &[514, 1057, 513, 2308, 2081, 2562],
        &[514, 2184, 2114, 2561],
        &[514, 2184, 2562, 1057, 513, 2564, 2184],
        &[514, 1057, 513, 2568, 2308, 2561],
    ],
    [
        &[1090, 514, 1057, 513, 2308, 2081, 2562],
        &[2114, 2561],
        &[1090, 2562, 1057, 513, 2564, 2184],
        &[1090, 514, 1057, 513, 2308, 2561, 2568],
    ],
    [
        &[2081, 2562],
        &[1057, 513, 1090, 514, 2184, 2114, 2561],
        &[1057, 513, 1090, 514, 2184, 2562, 2564],
        &[1057, 2561, 1090, 514, 2568, 2308],
    ],
];

/// The bounding box of `r` turned `degrees` around its centre.
fn turned_box(r: Rect, degrees: f64) -> Rect {
    let corners = [
        (r.x, r.y),
        (r.x + r.w, r.y),
        (r.x + r.w, r.y + r.h),
        (r.x, r.y + r.h),
    ];
    let c = (r.cx(), r.cy());
    let pts: Vec<P> = corners
        .iter()
        .map(|&p| rotate(p, degrees, c, false))
        .collect();
    let (x0, y0) = pts
        .iter()
        .fold((f64::MAX, f64::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
    let (x1, y1) = pts
        .iter()
        .fold((f64::MIN, f64::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1)));
    Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

impl View<'_> {
    fn jetty(&self, source: bool) -> f64 {
        let own = if source {
            "sourceJettySize"
        } else {
            "targetJettySize"
        };
        let value = self.style.get(own).or_else(|| self.style.get("jettySize"));
        match value {
            Some("auto") => {
                let (arrow, size) = match source {
                    true => ("startArrow", "startSize"),
                    false => ("endArrow", "endSize"),
                };
                match self.style.get(arrow) {
                    Some(_) => {
                        let size = self.style.num(size, 6.0);
                        ((size + 10.0) / 10.0).ceil().max(2.0) * 10.0
                    }
                    None => 20.0,
                }
            }
            Some(v) => v.parse().unwrap_or(10.0),
            None => 10.0,
        }
    }

    /// mxEdgeStyle's `OrthConnector`.
    fn orthogonal(&self, result: &mut Vec<Option<P>>) {
        let p0 = self.p0.map(|p| (tenth(p.0), tenth(p.1)));
        let pe = self.pe.map(|p| (tenth(p.0), tenth(p.1)));
        let (source, target) = (term(self.src), term(self.trg));
        let mut sb = self.jetty(true);
        let mut tb = self.jetty(false);
        let same = matches!((self.src, self.trg), (Some(a), Some(b)) if std::ptr::eq(a, b));
        if same {
            tb = sb.max(tb);
            sb = tb;
        }
        let total = sb + tb;
        let too_short = match (p0, pe) {
            (Some(a), Some(b)) => (b.0 - a.0).powi(2) + (b.1 - a.1).powi(2) < total * total,
            _ => false,
        };
        if too_short || !self.hints.is_empty() {
            return self.segment(result);
        }
        let side = |t: Option<Term>, p: Option<P>| -> Option<[f64; 4]> {
            match t {
                Some(t) => {
                    let r = match t.rotation {
                        0.0 => t.r,
                        a => turned_box(t.r, a),
                    };
                    Some([r.x, r.y, r.w, r.h])
                }
                None => p.map(|p| [p.0, p.1, 1.0, 1.0]),
            }
        };
        let (Some(g0), Some(g1)) = (side(source, p0), side(target, pe)) else {
            return;
        };
        if g0[2] == 0.0 || g0[3] == 0.0 || g1[2] == 0.0 || g1[3] == 0.0 {
            return;
        }
        let geo = [g0, g1];
        let buffer = [sb, tb];
        let port = [ALL, ALL];
        let mut limits = [[0.0; 9]; 2];
        for i in 0..2 {
            limits[i][1] = geo[i][0] - buffer[i];
            limits[i][2] = geo[i][1] - buffer[i];
            limits[i][4] = geo[i][0] + geo[i][2] + buffer[i];
            limits[i][8] = geo[i][1] + geo[i][3] + buffer[i];
        }
        let dx = (geo[0][0] + geo[0][2] / 2.0) - (geo[1][0] + geo[1][2] / 2.0);
        let dy = (geo[0][1] + geo[0][3] / 2.0) - (geo[1][1] + geo[1][3] / 2.0);
        let quad: i32 = match (dx < 0.0, dy < 0.0) {
            (true, true) => 2,
            (true, false) => 1,
            _ if dy <= 0.0 => {
                if dx == 0.0 {
                    2
                } else {
                    3
                }
            }
            _ => 0,
        };
        let mut dir = [0, 0];
        let mut constraint = [[0.5, 0.5], [0.5, 0.5]];
        if source.is_none() {
            constraint[0] = [0.0, 0.0];
        }
        if target.is_none() {
            constraint[1] = [0.0, 0.0];
        }
        let terms = [source.and(p0), target.and(pe)];
        for i in 0..2 {
            if let Some(ct) = terms[i] {
                constraint[i][0] = (ct.0 - geo[i][0]) / geo[i][2];
                if (ct.0 - geo[i][0]).abs() <= 1.0 {
                    dir[i] = WEST;
                } else if (ct.0 - geo[i][0] - geo[i][2]).abs() <= 1.0 {
                    dir[i] = EAST;
                }
                constraint[i][1] = (ct.1 - geo[i][1]) / geo[i][3];
                if (ct.1 - geo[i][1]).abs() <= 1.0 {
                    dir[i] = NORTH;
                } else if (ct.1 - geo[i][1] - geo[i][3]).abs() <= 1.0 {
                    dir[i] = SOUTH;
                }
            }
        }
        let top = geo[0][1] - (geo[1][1] + geo[1][3]);
        let left = geo[0][0] - (geo[1][0] + geo[1][2]);
        let bottom = geo[1][1] - (geo[0][1] + geo[0][3]);
        let right = geo[1][0] - (geo[0][0] + geo[0][2]);
        let mut seps = [0.0; 5];
        seps[1] = (left - total).max(0.0);
        seps[2] = (top - total).max(0.0);
        seps[4] = (bottom - total).max(0.0);
        seps[3] = (right - total).max(0.0);
        let mut hor = [if left >= right { WEST } else { EAST }, 0];
        let mut vert = [if top >= bottom { NORTH } else { SOUTH }, 0];
        hor[1] = reverse(hor[0]);
        vert[1] = reverse(vert[0]);
        let hdist = left.max(right);
        let vdist = top.max(bottom);
        let mut pref = [[0, 0], [0, 0]];
        let mut set = false;
        for i in 0..2 {
            if dir[i] != 0 {
                continue;
            }
            if hor[i] & port[i] == 0 {
                hor[i] = reverse(hor[i]);
            }
            if vert[i] & port[i] == 0 {
                vert[i] = reverse(vert[i]);
            }
            pref[i] = [vert[i], hor[i]];
        }
        if vdist > 0.0 && hdist > 0.0 {
            if hor[0] & port[0] > 0 && vert[1] & port[1] > 0 {
                pref = [[hor[0], vert[0]], [vert[1], hor[1]]];
                set = true;
            } else if vert[0] & port[0] > 0 && hor[1] & port[1] > 0 {
                pref = [[vert[0], hor[0]], [hor[1], vert[1]]];
                set = true;
            }
        }
        if vdist > 0.0 && !set {
            pref = [[vert[0], hor[0]], [vert[1], hor[1]]];
            set = true;
        }
        if hdist > 0.0 && !set {
            pref = [[hor[0], vert[0]], [hor[1], vert[1]]];
        }
        for i in 0..2 {
            if dir[i] != 0 {
                continue;
            }
            if pref[i][0] & port[i] == 0 {
                pref[i][0] = pref[i][1];
            }
            let mut d = pref[i][0] & port[i];
            d |= (pref[i][1] & port[i]) << 8;
            d |= (pref[1 - i][i] & port[i]) << 16;
            d |= (pref[1 - i][1 - i] & port[i]) << 24;
            if d & 0xF == 0 {
                d <<= 8;
            }
            if d & 0xF00 == 0 {
                d = (d & 0xF) | (d >> 8);
            }
            if d & 0xF0000 == 0 {
                d = (d & 0xFFFF) | ((d & 0xF00_0000) >> 8);
            }
            dir[i] = d & 0xF;
            if [WEST, NORTH, EAST, SOUTH].contains(&port[i]) {
                dir[i] = port[i];
            }
        }
        let index = |d: i32| {
            let i = if d == EAST { 3 } else { d } - quad;
            if i < 1 { i + 4 } else { i }
        };
        let (si, ti) = (index(dir[0]), index(dir[1]));
        let pattern = ROUTE_PATTERNS[(si - 1) as usize][(ti - 1) as usize];
        let mut way = [[0.0_f64; 2]; 12];
        way[0] = [geo[0][0], geo[0][1]];
        match dir[0] {
            WEST => {
                way[0][0] -= sb;
                way[0][1] += constraint[0][1] * geo[0][3];
            }
            SOUTH => {
                way[0][0] += constraint[0][0] * geo[0][2];
                way[0][1] += geo[0][3] + sb;
            }
            EAST => {
                way[0][0] += geo[0][2] + sb;
                way[0][1] += constraint[0][1] * geo[0][3];
            }
            NORTH => {
                way[0][0] += constraint[0][0] * geo[0][2];
                way[0][1] -= sb;
            }
            _ => {}
        }
        let mut current = 0;
        let horizontal = |d: i32| if d & (EAST | WEST) > 0 { 0 } else { 1 };
        let mut last = horizontal(dir[0]);
        let initial = last;
        for &step in pattern {
            let next_dir = step & 0xF;
            let mut di = if next_dir == EAST { 3 } else { next_dir } + quad;
            if di > 4 {
                di -= 4;
            }
            let direction = DIR_VECTORS[(di - 1) as usize];
            let orientation = if di % 2 > 0 { 0 } else { 1 };
            if orientation != last {
                current += 1;
                way[current] = way[current - 1];
            }
            let tar = step & 2048 > 0;
            let sou = step & 1024 > 0;
            let mut side = (step & 480) >> 5;
            side <<= quad;
            if side > 0xF {
                side >>= 4;
            }
            let center = step & 512 > 0;
            if (sou || tar) && side < 9 {
                let st = if sou { 0 } else { 1 };
                let limit = if center && orientation == 0 {
                    geo[st][0] + constraint[st][0] * geo[st][2]
                } else if center {
                    geo[st][1] + constraint[st][1] * geo[st][3]
                } else {
                    limits[st][side as usize]
                };
                if orientation == 0 {
                    let delta = (limit - way[current][0]) * direction.0;
                    if delta > 0.0 {
                        way[current][0] += direction.0 * delta;
                    }
                } else {
                    let delta = (limit - way[current][1]) * direction.1;
                    if delta > 0.0 {
                        way[current][1] += direction.1 * delta;
                    }
                }
            } else if center {
                way[current][0] += direction.0 * (seps[di as usize] / 2.0).abs();
                way[current][1] += direction.1 * (seps[di as usize] / 2.0).abs();
            }
            if current > 0 && way[current][orientation] == way[current - 1][orientation] {
                current -= 1;
            } else {
                last = orientation;
            }
        }
        let start = result.len();
        for (i, w) in way.iter().enumerate().take(current + 1) {
            if i == current {
                let target_orientation = horizontal(dir[1]);
                let same_orient = if target_orientation == initial { 0 } else { 1 };
                if same_orient != (current + 1) % 2 {
                    break;
                }
            }
            result.push(Some((tenth(w[0]), tenth(w[1]))));
        }
        let mut i = start.max(1);
        while i < result.len() {
            if result[i - 1].is_some() && result[i - 1] == result[i] {
                result.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// mxEdgeStyle's `SegmentConnector`: straight segments that turn at each waypoint.
    fn segment(&self, result: &mut Vec<Option<P>>) {
        let tol = 1.0;
        let pts = [
            self.p0.map(|p| (tenth(p.0), tenth(p.1))),
            self.pe.map(|p| (tenth(p.0), tenth(p.1))),
        ];
        let (source, target) = (term(self.src).map(|t| t.r), term(self.trg).map(|t| t.r));
        let (sc, tc) = (source, target);
        let mut temp: Vec<P> = vec![];
        let mut last_pushed: Option<P> = result.first().copied().flatten();
        let mut horizontal = true;
        let hint;
        let mut pt = match (pts[0], self.src) {
            (Some(p), _) => p,
            (None, Some(s)) => routing_center(s),
            _ => return,
        };
        let in_y = |c: Option<Rect>, y: f64| c.is_some_and(|c| y >= c.y && y <= c.y + c.h);
        let in_x = |c: Option<Rect>, x: f64| c.is_some_and(|c| x >= c.x && x <= c.x + c.w);
        if !self.hints.is_empty() {
            let mut hints: Vec<P> = self.hints.to_vec();
            if let Some(&h0) = hints.first() {
                let h = &mut hints[0];
                if (h0.0 - pt.0).abs() < tol {
                    h.0 = pt.0;
                }
                if (h0.1 - pt.1).abs() < tol {
                    h.1 = pt.1;
                }
            }
            if let Some(p) = pts[1] {
                let h = hints.last_mut().unwrap();
                if (h.0 - p.0).abs() < tol {
                    h.0 = p.0;
                }
                if (h.1 - p.1).abs() < tol {
                    h.1 = p.1;
                }
            }
            let first = hints[0];
            let mut current_term = source.filter(|_| pts[0].is_none());
            let mut channel = sc;
            let mut current_pt = pts[0];
            let mut current_hint = first;
            for i in 0..2 {
                let fixed_vert = current_pt.is_some_and(|p| p.0 == current_hint.0);
                let fixed_hoz = current_pt.is_some_and(|p| p.1 == current_hint.1);
                let in_hoz = current_term.is_some() && in_y(channel, current_hint.1);
                let in_vert = current_term.is_some() && in_x(channel, current_hint.0);
                let hoz_chan = fixed_hoz || (current_pt.is_none() && in_hoz);
                let vert_chan = fixed_vert || (current_pt.is_none() && in_vert);
                if !(i == 0 && ((hoz_chan && vert_chan) || (fixed_vert && fixed_hoz))) {
                    if current_pt.is_some() && !fixed_hoz && !fixed_vert && (in_hoz || in_vert) {
                        horizontal = !in_hoz;
                        break;
                    }
                    if vert_chan || hoz_chan {
                        horizontal = hoz_chan;
                        if i == 1 {
                            horizontal = if hints.len().is_multiple_of(2) {
                                hoz_chan
                            } else {
                                vert_chan
                            };
                        }
                        break;
                    }
                }
                current_term = target.filter(|_| pts[1].is_none());
                channel = tc;
                current_pt = pts[1];
                current_hint = *hints.last().unwrap();
                if fixed_vert && fixed_hoz {
                    hints.remove(0);
                }
            }
            let Some(&h) = hints.first().or(Some(&first)) else {
                return;
            };
            let outside_y = |c: Option<Rect>, y: f64| c.is_some_and(|c| y < c.y || y > c.y + c.h);
            let outside_x = |c: Option<Rect>, x: f64| c.is_some_and(|c| x < c.x || x > c.x + c.w);
            let hh = first;
            if horizontal
                && (pts[0].is_some_and(|p| p.1 != hh.1)
                    || (pts[0].is_none() && outside_y(sc, hh.1)))
            {
                temp.push((pt.0, hh.1));
            } else if !horizontal
                && (pts[0].is_some_and(|p| p.0 != hh.0)
                    || (pts[0].is_none() && outside_x(sc, hh.0)))
            {
                temp.push((hh.0, pt.1));
            }
            if horizontal {
                pt.1 = hh.1;
            } else {
                pt.0 = hh.0;
            }
            let _ = h;
            let mut last_hint = hh;
            for &hn in &hints {
                horizontal = !horizontal;
                if horizontal {
                    pt.1 = hn.1;
                } else {
                    pt.0 = hn.0;
                }
                temp.push(pt);
                last_hint = hn;
            }
            hint = Some(last_hint);
        } else {
            hint = Some(pt);
            horizontal = true;
        }
        let end = match (pts[1], self.trg) {
            (Some(p), _) => Some(p),
            (None, Some(t)) => Some(routing_center(t)),
            _ => None,
        };
        if let (Some(end), Some(hint)) = (end, hint) {
            let outside_y = |c: Option<Rect>, y: f64| c.is_some_and(|c| y < c.y || y > c.y + c.h);
            let outside_x = |c: Option<Rect>, x: f64| c.is_some_and(|c| x < c.x || x > c.x + c.w);
            if horizontal
                && (pts[1].is_some_and(|p| p.1 != hint.1)
                    || (pts[1].is_none() && outside_y(tc, hint.1)))
            {
                temp.push((end.0, hint.1));
            } else if !horizontal
                && (pts[1].is_some_and(|p| p.0 != hint.0)
                    || (pts[1].is_none() && outside_x(tc, hint.0)))
            {
                temp.push((hint.0, end.1));
            }
        }
        let same = matches!((self.src, self.trg), (Some(a), Some(b)) if std::ptr::eq(a, b));
        if !same || self.style.get("innerLoopWaypoints") != Some("1") {
            if pts[0].is_none()
                && let Some(c) = sc
            {
                while temp.first().is_some_and(|&p| c.contains(p)) {
                    temp.remove(0);
                }
            }
            if pts[1].is_none()
                && let Some(c) = tc
            {
                while temp.last().is_some_and(|&p| c.contains(p)) {
                    temp.pop();
                }
            }
        }
        for p in temp {
            let p = (tenth(p.0), tenth(p.1));
            if last_pushed.is_none_or(|l| (l.0 - p.0).abs() >= tol || (l.1 - p.1).abs() >= tol) {
                result.push(Some(p));
                last_pushed = Some(p);
            }
        }
        if let (Some(pe), Some(Some(l))) = (pts[1], result.last().copied())
            && (pe.0 - l.0).abs() <= tol
            && (pe.1 - l.1).abs() <= tol
            && result.len() > 1
        {
            result.pop();
            if let Some(Some(l)) = result.last_mut() {
                if (l.0 - pe.0).abs() < tol {
                    l.0 = pe.0;
                }
                if (l.1 - pe.1).abs() < tol {
                    l.1 = pe.1;
                }
            }
        }
    }

    /// mxEdgeStyle's `ElbowConnector`: one bend, across or down.
    fn elbow(&self, result: &mut Vec<Option<P>>) {
        let pt = self.hints.first().copied();
        let (mut vertical, mut horizontal) = (false, false);
        if let (Some(s), Some(t)) = (self.src, self.trg) {
            let (s, t) = (s.rect, t.rect);
            match pt {
                Some(pt) => {
                    let (left, right) = (s.x.min(t.x), (s.x + s.w).max(t.x + t.w));
                    let (top, bottom) = (s.y.min(t.y), (s.y + s.h).max(t.y + t.h));
                    vertical = pt.1 < top || pt.1 > bottom;
                    horizontal = pt.0 < left || pt.0 > right;
                }
                None => {
                    let (left, right) = (s.x.max(t.x), (s.x + s.w).min(t.x + t.w));
                    vertical = left == right;
                    if !vertical {
                        let (top, bottom) = (s.y.max(t.y), (s.y + s.h).min(t.y + t.h));
                        horizontal = top == bottom;
                    }
                }
            }
        }
        if !horizontal && (vertical || self.style.get("elbow") == Some("vertical")) {
            self.top_to_bottom(result);
        } else {
            self.side_to_side(result);
        }
    }

    fn terminals(&self) -> Option<(Rect, Rect, P, P)> {
        let s = self.p0.map(at_point).or(self.src.map(|s| s.rect))?;
        let t = self.pe.map(at_point).or(self.trg.map(|t| t.rect))?;
        let center = |p: Option<P>, st: Option<&State>, r: Rect| match (p, st) {
            (None, Some(st)) => routing_center(st),
            _ => (r.cx(), r.cy()),
        };
        Some((
            s,
            t,
            center(self.p0, self.src, s),
            center(self.pe, self.trg, t),
        ))
    }

    /// mxEdgeStyle's `SideToSide`.
    fn side_to_side(&self, result: &mut Vec<Option<P>>) {
        let Some((s, t, sc, tc)) = self.terminals() else {
            return;
        };
        let pt = self.hints.first().copied();
        let (l, r) = (s.x.max(t.x), (s.x + s.w).min(t.x + t.w));
        let x = pt.map_or_else(|| (r + (l - r) / 2.0).round(), |p| p.0);
        let (mut y1, mut y2) = (sc.1, tc.1);
        if let Some(pt) = pt {
            if pt.1 >= s.y && pt.1 <= s.y + s.h {
                y1 = pt.1;
            }
            if pt.1 >= t.y && pt.1 <= t.y + t.h {
                y2 = pt.1;
            }
        }
        let start = result.len();
        let free = |p: P| !t.contains(p) && !s.contains(p);
        if free((x, y1)) {
            result.push(Some((x, y1)));
        }
        if free((x, y2)) {
            result.push(Some((x, y2)));
        }
        if result.len() - start == 1 {
            match pt {
                Some(pt) if free((x, pt.1)) => result.push(Some((x, pt.1))),
                Some(_) => {}
                None => {
                    let (top, bottom) = (s.y.max(t.y), (s.y + s.h).min(t.y + t.h));
                    result.push(Some((x, top + (bottom - top) / 2.0)));
                }
            }
        }
    }

    /// mxEdgeStyle's `TopToBottom`.
    fn top_to_bottom(&self, result: &mut Vec<Option<P>>) {
        let Some((s, t, sc, tc)) = self.terminals() else {
            return;
        };
        let pt = self.hints.first().copied();
        let (top, bottom) = (s.y.max(t.y), (s.y + s.h).min(t.y + t.h));
        let mut x = sc.0;
        if let Some(pt) = pt.filter(|p| p.0 >= s.x && p.0 <= s.x + s.w) {
            x = pt.0;
        }
        let y = pt.map_or_else(|| (bottom + (top - bottom) / 2.0).round(), |p| p.1);
        let start = result.len();
        let free = |p: P| !t.contains(p) && !s.contains(p);
        if free((x, y)) {
            result.push(Some((x, y)));
        }
        x = match pt.filter(|p| p.0 >= t.x && p.0 <= t.x + t.w) {
            Some(pt) => pt.0,
            None => tc.0,
        };
        if free((x, y)) {
            result.push(Some((x, y)));
        }
        if result.len() - start == 1 {
            match pt {
                Some(pt) if free((pt.0, y)) => result.push(Some((pt.0, y))),
                Some(_) => {}
                None => {
                    let (l, r) = (s.x.max(t.x), (s.x + s.w).min(t.x + t.w));
                    result.push(Some((l + (r - l) / 2.0, y)));
                }
            }
        }
    }

    /// mxEdgeStyle's `EntityRelation`: out of a side, across, and into a side.
    fn entity_relation(&self, result: &mut Vec<Option<P>>) {
        let segment = self.style.num("segment", 30.0);
        let (Some(s), Some(t)) = (
            self.p0.map(at_point).or(self.src.map(|s| s.rect)),
            self.pe.map(at_point).or(self.trg.map(|t| t.rect)),
        ) else {
            return;
        };
        let (p0, pe) = (self.p0, self.pe);
        let source_left = match self.src {
            Some(_) => pe.map_or(t.x + t.w, |p| p.0) < p0.map_or(s.x, |p| p.0),
            None => false,
        };
        let target_left = match self.trg {
            Some(_) => p0.map_or(s.x + s.w, |p| p.0) < pe.map_or(t.x, |p| p.0),
            None => true,
        };
        let x0 = if source_left { s.x } else { s.x + s.w };
        let y0 = match (p0, self.src) {
            (None, Some(st)) => routing_center(st).1,
            _ => s.cy(),
        };
        let xe = if target_left { t.x } else { t.x + t.w };
        let ye = match (pe, self.trg) {
            (None, Some(st)) => routing_center(st).1,
            _ => t.cy(),
        };
        let dep = (x0 + if source_left { -segment } else { segment }, y0);
        let arr = (xe + if target_left { -segment } else { segment }, ye);
        if source_left == target_left {
            let x = if source_left {
                x0.min(xe) - segment
            } else {
                x0.max(xe) + segment
            };
            result.push(Some((x, y0)));
            result.push(Some((x, ye)));
        } else if (dep.0 < arr.0) == source_left {
            let mid = y0 + (ye - y0) / 2.0;
            result.extend([Some(dep), Some((dep.0, mid)), Some((arr.0, mid)), Some(arr)]);
        } else {
            result.extend([Some(dep), Some(arr)]);
        }
    }

    /// mxEdgeStyle's `Loop`: an edge from a vertex back to itself.
    fn loop_route(&self, result: &mut Vec<Option<P>>) {
        if self.p0.is_some() && self.pe.is_some() {
            result.extend(self.hints.iter().map(|&p| Some(p)));
            return;
        }
        let Some(s) = self.src else {
            return;
        };
        let r = s.rect;
        let pt = self.hints.first().copied().filter(|&p| !r.contains(p));
        let seg = self.style.num("segment", 10.0);
        let dir = self.style.get("direction").unwrap_or("west");
        let (mut x, mut dx, mut y, mut dy) = (0.0, 0.0, 0.0, 0.0);
        if matches!(dir, "north" | "south") {
            x = routing_center(s).0;
            dx = seg;
        } else {
            y = routing_center(s).1;
            dy = seg;
        }
        match pt {
            Some(p) if p.0 >= r.x && p.0 <= r.x + r.w => {
                x = routing_center(s).0;
                dx = (x - p.0).abs().max(dy);
                y = p.1;
                dy = 0.0;
            }
            Some(p) => {
                x = p.0;
                dy = (y - p.1).abs().max(dy);
            }
            None => match dir {
                "north" => y = r.y - 2.0 * dx,
                "south" => y = r.y + r.h + 2.0 * dx,
                "east" => x = r.x - 2.0 * dy,
                _ => x = r.x + r.w + 2.0 * dy,
            },
        }
        result.push(Some((x - dx, y - dy)));
        result.push(Some((x + dx, y + dy)));
    }
}

/// The point at `geo` along a route, as mxGraphView's `getPoint` finds it for labels: `x` from
/// -1 (the start) to 1 (the end), `y` across the segment there, plus the offset.
pub fn along(points: &[P], geo: &Geo) -> P {
    let lengths: Vec<f64> = points
        .windows(2)
        .scan(0.0, |total, w| {
            *total += (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
            Some(*total)
        })
        .collect();
    let length = lengths.last().copied().unwrap_or(0.0);
    let dist = ((geo.x / 2.0 + 0.5) * length).round();
    let index = lengths
        .iter()
        .position(|&l| dist < l.round())
        .unwrap_or(lengths.len().saturating_sub(1));
    let before = if index == 0 { 0.0 } else { lengths[index - 1] };
    let (p0, pe) = (points[index], points[(index + 1).min(points.len() - 1)]);
    let segment = (pe.0 - p0.0).hypot(pe.1 - p0.1);
    let factor = if segment == 0.0 {
        0.0
    } else {
        (dist - before) / segment
    };
    let (dx, dy) = (pe.0 - p0.0, pe.1 - p0.1);
    let (nx, ny) = match segment {
        0.0 => (0.0, 0.0),
        s => (dy / s, dx / s),
    };
    (
        p0.0 + dx * factor + nx * geo.y + geo.offset.0,
        p0.1 + dy * factor - ny * geo.y + geo.offset.1,
    )
}

/// The arrows drawn as asked; others are drawn as `classic`.
const ARROWS: [&str; 24] = [
    "classic",
    "classicThin",
    "block",
    "blockThin",
    "open",
    "openThin",
    "oval",
    "diamond",
    "diamondThin",
    "box",
    "halfCircle",
    "dash",
    "cross",
    "circle",
    "circlePlus",
    "async",
    "openAsync",
    "ERone",
    "ERmandOne",
    "ERmany",
    "ERoneToMany",
    "ERzeroToOne",
    "ERzeroToMany",
    "baseDash",
];

/// Draws the edge along `points`: its line, shortened where a marker covers the end, then the
/// markers. Returns the box around what it drew.
pub fn draw(out: &mut String, style: &Style, points: &[P], notes: &mut Vec<Approximated>) {
    let mut pts = points.to_vec();
    let sw = style.num("strokeWidth", 1.0);
    let mut markers = String::new();
    let stroke_color = paint(style.get("strokeColor"), "#000000");
    for start in [true, false] {
        let (key, fill_key, size_key, color_key) = match start {
            true => ("startArrow", "startFill", "startSize", "startFillColor"),
            false => ("endArrow", "endFill", "endSize", "endFillColor"),
        };
        let Some(mut kind) = style.get(key) else {
            continue;
        };
        if !ARROWS.contains(&kind) {
            notes.push(("arrow_approximated", kind.into(), "classic"));
            kind = "classic";
        }
        let k = pts.len();
        let (from, at) = match start {
            true => (pts[1], 0),
            false => (pts[k - 2], k - 1),
        };
        let (dx, dy) = (pts[at].0 - from.0, pts[at].1 - from.1);
        let dist = dx.hypot(dy);
        if dist == 0.0 {
            continue;
        }
        let unit = (dx / dist, dy / dist);
        let size = style.num(size_key, 6.0);
        let filled = style.get(fill_key).is_none_or(|v| v != "0");
        let fill = match filled {
            true => paint(style.get(color_key).or(Some(&stroke_color)), "#000000"),
            false => "none".into(),
        };
        let shape = marker(kind, &mut pts[at], unit, size, sw, start, filled);
        if !shape.0.is_empty() {
            let fill = if shape.1 { fill.as_str() } else { "none" };
            let _ = write!(
                markers,
                r#"<path d="{}" fill="{fill}"/>"#,
                shape.0.trim_end()
            );
        }
    }
    let mut line = Path::default();
    match style.on("curved") && pts.len() > 2 {
        true => {
            line.m(pts[0]);
            let k = pts.len();
            for i in 1..k - 2 {
                let (a, b) = (pts[i], pts[i + 1]);
                line.q(a, ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
            }
            line.q(pts[k - 2], pts[k - 1]);
        }
        false => {
            let arc = style.num("arcSize", 20.0) / 2.0;
            line.points(&pts, style.on("rounded"), arc, false);
        }
    }
    let mut group = String::new();
    if let Some(o) = Some(style.num("opacity", 100.0)).filter(|o| *o != 100.0) {
        let _ = write!(group, r#" opacity="{}""#, n(o / 100.0));
    }
    let stroke = shapes::stroke(style, true);
    let width = if sw != 1.0 {
        format!(r#" stroke-width="{}""#, n(sw))
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        r#"<g{group}><path d="{}" fill="none"{stroke}/>{}</g>"#,
        line.0.trim_end(),
        if markers.is_empty() {
            String::new()
        } else {
            format!(r#"<g stroke="{stroke_color}"{width}>{markers}</g>"#)
        }
    );
}

/// The path of a marker at the end `pe` pointing along `unit`, and whether it is filled; moves
/// `pe` back to where the line should stop, as mxMarker's functions do.
fn marker(
    kind: &str,
    pe: &mut P,
    (ux, uy): P,
    size: f64,
    sw: f64,
    source: bool,
    filled: bool,
) -> (String, bool) {
    let mut p = Path::default();
    let mut fill = filled;
    let end = *pe;
    match kind {
        "classic" | "classicThin" | "block" | "blockThin" => {
            let wf = if kind.ends_with("Thin") { 3.0 } else { 2.0 };
            let (ox, oy) = (ux * sw * 1.118, uy * sw * 1.118);
            let (vx, vy) = (ux * (size + sw), uy * (size + sw));
            let pt = (end.0 - ox, end.1 - oy);
            let f = if kind.starts_with("classic") {
                0.75
            } else {
                1.0
            };
            *pe = (end.0 - vx * f - ox, end.1 - vy * f - oy);
            p.m(pt);
            p.l((pt.0 - vx - vy / wf, pt.1 - vy + vx / wf));
            if f < 1.0 {
                p.l((pt.0 - vx * 3.0 / 4.0, pt.1 - vy * 3.0 / 4.0));
            }
            p.l((pt.0 + vy / wf - vx, pt.1 - vy - vx / wf));
            p.z();
        }
        "open" | "openThin" => {
            let wf = if kind == "openThin" { 3.0 } else { 2.0 };
            let (ox, oy) = (ux * sw * 1.118, uy * sw * 1.118);
            let (vx, vy) = (ux * (size + sw), uy * (size + sw));
            let pt = (end.0 - ox, end.1 - oy);
            *pe = (end.0 - ox * 2.0, end.1 - oy * 2.0);
            p.m((pt.0 - vx - vy / wf, pt.1 - vy + vx / wf));
            p.l(pt);
            p.l((pt.0 + vy / wf - vx, pt.1 - vy - vx / wf));
            fill = false;
        }
        "oval" => {
            let a = size / 2.0;
            *pe = (end.0 - ux * a, end.1 - uy * a);
            circle(&mut p, end, a);
        }
        "diamond" | "diamondThin" => {
            let swf = if kind == "diamond" {
                FRAC_1_SQRT_2
            } else {
                0.9862
            };
            let (ox, oy) = (ux * sw * swf, uy * sw * swf);
            let (vx, vy) = (ux * (size + sw), uy * (size + sw));
            let pt = (end.0 - ox, end.1 - oy);
            *pe = (end.0 - vx - ox, end.1 - vy - oy);
            let tk = if kind == "diamond" { 2.0 } else { 3.4 };
            p.m(pt);
            p.l((pt.0 - vx / 2.0 - vy / tk, pt.1 + vx / tk - vy / 2.0));
            p.l((pt.0 - vx, pt.1 - vy));
            p.l((pt.0 - vx / 2.0 + vy / tk, pt.1 - vy / 2.0 - vx / tk));
            p.z();
        }
        "box" => {
            let (nx, ny) = (ux * (size + sw + 1.0), uy * (size + sw + 1.0));
            let (px, py) = (end.0 + nx / 2.0, end.1 + ny / 2.0);
            *pe = (end.0 - nx, end.1 - ny);
            p.m((px - nx / 2.0 - ny / 2.0, py - ny / 2.0 + nx / 2.0));
            p.l((px - nx / 2.0 + ny / 2.0, py - ny / 2.0 - nx / 2.0));
            p.l((
                px + ny / 2.0 - 3.0 * nx / 2.0,
                py - 3.0 * ny / 2.0 - nx / 2.0,
            ));
            p.l((
                px - ny / 2.0 - 3.0 * nx / 2.0,
                py - 3.0 * ny / 2.0 + nx / 2.0,
            ));
            p.z();
        }
        "halfCircle" => {
            let (nx, ny) = (ux * (size + sw + 1.0), uy * (size + sw + 1.0));
            *pe = (end.0 - nx, end.1 - ny);
            p.m((end.0 - ny, end.1 + nx));
            p.q((pe.0 - ny, pe.1 + nx), *pe);
            p.q((pe.0 + ny, pe.1 - nx), (end.0 + ny, end.1 - nx));
            fill = false;
        }
        "dash" | "baseDash" => {
            let (nx, ny) = (ux * (size + sw + 1.0), uy * (size + sw + 1.0));
            match kind {
                "dash" => {
                    p.m((end.0 - nx / 2.0 - ny / 2.0, end.1 - ny / 2.0 + nx / 2.0));
                    p.l((
                        end.0 + ny / 2.0 - 3.0 * nx / 2.0,
                        end.1 - 3.0 * ny / 2.0 - nx / 2.0,
                    ));
                }
                _ => {
                    p.m((end.0 - ny / 2.0, end.1 + nx / 2.0));
                    p.l((end.0 + ny / 2.0, end.1 - nx / 2.0));
                }
            }
            fill = false;
        }
        "cross" => {
            let (nx, ny) = (ux * (size + sw + 1.0), uy * (size + sw + 1.0));
            p.m((end.0 - nx / 2.0 - ny / 2.0, end.1 - ny / 2.0 + nx / 2.0));
            p.l((
                end.0 + ny / 2.0 - 3.0 * nx / 2.0,
                end.1 - 3.0 * ny / 2.0 - nx / 2.0,
            ));
            p.m((end.0 - nx / 2.0 + ny / 2.0, end.1 - ny / 2.0 - nx / 2.0));
            p.l((
                end.0 - ny / 2.0 - 3.0 * nx / 2.0,
                end.1 - 3.0 * ny / 2.0 + nx / 2.0,
            ));
            fill = false;
        }
        "circle" | "circlePlus" => {
            let s = size + sw;
            *pe = (end.0 - ux * (2.0 * s + sw), end.1 - uy * (2.0 * s + sw));
            let (vx, vy) = (ux * (s + sw), uy * (s + sw));
            circle(&mut p, (end.0 - vx, end.1 - vy), s);
            if kind == "circlePlus" {
                let (nx, ny) = (ux * (size + 2.0 * sw), uy * (size + 2.0 * sw));
                p.m((end.0 - vx * sw / (s + sw), end.1 - vy * sw / (s + sw)));
                p.l((end.0 - 2.0 * nx + ux * sw, end.1 - 2.0 * ny + uy * sw));
                p.m((end.0 - nx - ny + uy * sw, end.1 - ny + nx - ux * sw));
                p.l((end.0 + ny - nx - uy * sw, end.1 - ny - nx + ux * sw));
            }
        }
        "async" => {
            let (ox, oy) = (ux * sw * 1.118, uy * sw * 1.118);
            let (vx, vy) = (ux * (size + sw), uy * (size + sw));
            let pt = (end.0 - ox, end.1 - oy);
            *pe = (end.0 - vx - ox, end.1 - vy - oy);
            p.m(pt);
            match source {
                true => p.l((pt.0 - vx - vy / 2.0, pt.1 - vy + vx / 2.0)),
                false => p.l((pt.0 + vy / 2.0 - vx, pt.1 - vy - vx / 2.0)),
            }
            p.l((pt.0 - vx, pt.1 - vy));
            p.z();
        }
        "openAsync" => {
            let (vx, vy) = (ux * (size + sw), uy * (size + sw));
            p.m(end);
            match source {
                true => p.l((end.0 - vx - vy / 2.0, end.1 - vy + vx / 2.0)),
                false => p.l((end.0 + vy / 2.0 - vx, end.1 - vy - vx / 2.0)),
            }
            fill = false;
        }
        // Crow's foot notation, in our own drawing: bars across the line, a circle for zero and
        // a fork for many.
        _ => {
            let (nx, ny) = (ux * (size + sw + 1.0), uy * (size + sw + 1.0));
            let bar = |p: &mut Path, at: f64| {
                let (cx, cy) = (end.0 - nx * at, end.1 - ny * at);
                p.m((cx - ny / 2.0, cy + nx / 2.0));
                p.l((cx + ny / 2.0, cy - nx / 2.0));
            };
            let fork = |p: &mut Path| {
                p.m((end.0 + ny / 2.0, end.1 - nx / 2.0));
                p.l((end.0 - nx, end.1 - ny));
                p.l((end.0 - ny / 2.0, end.1 + nx / 2.0));
            };
            match kind {
                "ERone" => bar(&mut p, 0.5),
                "ERmandOne" => {
                    bar(&mut p, 0.5);
                    bar(&mut p, 1.0);
                }
                "ERmany" => fork(&mut p),
                "ERoneToMany" => {
                    fork(&mut p);
                    bar(&mut p, 1.0);
                }
                _ => {
                    let a = size / 2.0;
                    circle(&mut p, (end.0 - 1.5 * nx, end.1 - 1.5 * ny), a);
                    match kind {
                        "ERzeroToOne" => bar(&mut p, 0.5),
                        _ => fork(&mut p),
                    }
                    *pe = (
                        end.0 - 2.0 * nx + ux * sw / 2.0,
                        end.1 - 2.0 * ny + uy * sw / 2.0,
                    );
                    p.m(*pe);
                    p.l(end);
                    return (p.0, false);
                }
            }
            fill = false;
        }
    }
    (p.0, fill)
}

fn circle(p: &mut Path, (cx, cy): P, r: f64) {
    p.m((cx - r, cy));
    let _ = write!(
        p.0,
        "A{r} {r} 0 1 0 {} {} A{r} {r} 0 1 0 {} {} Z ",
        n(cx + r),
        n(cy),
        n(cx - r),
        n(cy),
        r = n(r)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(style: &str, x: f64, y: f64, w: f64, h: f64) -> State {
        State {
            rect: Rect { x, y, w, h },
            style: Style::parse(style, false),
        }
    }

    #[test]
    fn straight_edges_end_on_each_perimeter() {
        let a = state("", 0.0, 0.0, 100.0, 60.0);
        let b = state("ellipse", 200.0, 0.0, 60.0, 60.0);
        let e = Style::parse("", true);
        let pts = route(&e, End::Cell(&a), End::Cell(&b), &[], &mut vec![]);
        assert_eq!(pts, vec![(100.0, 30.0), (200.0, 30.0)]);
    }

    #[test]
    fn orthogonal_edges_turn_between_their_terminals() {
        let a = state("", 0.0, 0.0, 100.0, 60.0);
        let b = state("", 200.0, 100.0, 100.0, 60.0);
        let e = Style::parse("edgeStyle=orthogonalEdgeStyle", true);
        let pts = route(&e, End::Cell(&a), End::Cell(&b), &[], &mut vec![]);
        // Apart on both axes: out of one side and into the top, with one bend.
        assert_eq!(pts, vec![(100.0, 30.0), (250.0, 30.0), (250.0, 100.0)]);
        let c = state("", 200.0, 20.0, 100.0, 60.0);
        let pts = route(&e, End::Cell(&a), End::Cell(&c), &[], &mut vec![]);
        assert_eq!(
            pts,
            vec![(100.0, 30.0), (150.0, 30.0), (150.0, 50.0), (200.0, 50.0)]
        );
        // With a waypoint, segments turn there.
        let pts = route(
            &e,
            End::Cell(&a),
            End::Cell(&b),
            &[(50.0, 200.0)],
            &mut vec![],
        );
        assert_eq!(
            pts,
            vec![(50.0, 60.0), (50.0, 200.0), (250.0, 200.0), (250.0, 160.0)]
        );
    }

    #[test]
    fn labels_sit_along_the_route() {
        let pts = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)];
        assert_eq!(along(&pts, &Geo::default()), (100.0, 0.0));
        let geo = Geo {
            x: -0.5,
            y: 10.0,
            ..Geo::default()
        };
        assert_eq!(along(&pts, &geo), (50.0, -10.0));
    }
}
