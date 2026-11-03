//! Excalidraw scenes (`.excalidraw`, `.excalidraw.svg`, `.excalidraw.png`), drawn as Excalidraw
//! 0.18.1's `exportToSvg` draws them. Geometry is ported from its `scene/export.ts`,
//! `renderer/staticSvgScene.ts`, `scene/Shape.ts` and `element/bounds.ts` (MIT, Copyright (c)
//! 2020 Excalidraw); strokes from rough.js with each element's seed, text as glyph outlines.

mod freehand;
mod read;
mod rough;
mod text;

use std::collections::HashMap;
use std::f64::consts::PI;
use std::fmt::Write;

use crate::engine::{Checked, Rendered, Task};
use crate::error::{Error, Warning};
use crate::raster::Widths;
use crate::svg::{escape, num as n};
use read::{Element, Kind, Stroke};
use rough::{Drawable, Fill, Op, Options, P, Seg};

/// Space around the drawing, as Excalidraw exports it.
const PADDING: f64 = 10.0;
/// How frames and their names are drawn (`FRAME_STYLE`).
const FRAME_RADIUS: f64 = 8.0;
const FRAME_NAME_SIZE: f64 = 14.0;
const FRAME_NAME_LINE_HEIGHT: f64 = 1.25;
const FRAME_NAME_OFFSET: f64 = 3.0;

pub fn render(task: &Task) -> Result<Rendered, Error> {
    let scene = read::scene(&task.lines, task.name)?;
    let mut warnings = scene.warnings;
    let widths = Widths::load()?;
    let elements = with_frame_names(scene.elements, &widths);
    let mut d = Drawing {
        by_id: (elements.iter().enumerate())
            .map(|(i, e)| (e.id.as_str(), i))
            .collect(),
        elements: &elements,
        files: &scene.files,
        background: &scene.background,
        bound: HashMap::new(),
        shapes: HashMap::new(),
        body: String::new(),
        defs: String::new(),
        texts: vec![],
        fills: vec![],
        warnings: vec![],
        widths: &widths,
        symbols: HashMap::new(),
    };
    for (i, e) in elements.iter().enumerate() {
        if let Some(c) = e.container_id.as_deref().and_then(|c| d.by_id.get(c)) {
            d.bound.insert(*c, i);
        }
    }
    let svg = d.draw(task.background)?;
    warnings.append(&mut d.warnings);
    Ok(Rendered {
        svg: svg.into_bytes(),
        warnings,
        ..Rendered::default()
    })
}

/// Everything a render finds, without the drawing.
pub fn check(task: &Task) -> Result<Checked, Error> {
    Ok(Checked {
        warnings: render(task)?.warnings,
        ..Checked::default()
    })
}

/// The elements with each frame's name before it as a text element, as Excalidraw's export
/// adds them (`addFrameLabelsAsTextElements`), cut with `...` to the frame's width.
fn with_frame_names(elements: Vec<Element>, widths: &Widths) -> Vec<Element> {
    let mut out = Vec::with_capacity(elements.len());
    for e in elements {
        if e.kind.frame() {
            let title = match (&e.name, e.kind) {
                (Some(name), _) => name.clone(),
                (None, Kind::Frame) => "Frame".into(),
                (None, _) => "AI Frame".into(),
            };
            let measure = |t: &str| widths.of(t, FRAME_NAME_SIZE);
            let (mut text, mut width) = (title.clone(), measure(&title));
            if width > e.width {
                let chars: Vec<char> = title.chars().collect();
                for i in (1..=chars.len()).rev() {
                    let cut = format!("{}...", chars[..i].iter().collect::<String>());
                    if measure(&cut) <= e.width {
                        text = cut;
                        break;
                    }
                }
                width = e.width;
            }
            let height = FRAME_NAME_SIZE * FRAME_NAME_LINE_HEIGHT;
            out.push(Element {
                id: String::new(),
                kind: Kind::Text,
                x: e.x,
                y: e.y - FRAME_NAME_OFFSET - height,
                width,
                height,
                angle: 0.0,
                stroke_color: "#999999".into(),
                background_color: "transparent".into(),
                opacity: 100.0,
                roundness: None,
                frame_id: None,
                link: None,
                text,
                font_size: FRAME_NAME_SIZE,
                font_family: 2,
                text_align: "left".into(),
                line_height: FRAME_NAME_LINE_HEIGHT,
                container_id: None,
                ..e.clone()
            });
        }
        out.push(e);
    }
    out
}

/// A box: left, top, right and bottom.
type Bounds = (f64, f64, f64, f64);

fn rotate((x, y): P, (cx, cy): P, angle: f64) -> P {
    let (s, c) = angle.sin_cos();
    (
        (x - cx) * c - (y - cy) * s + cx,
        (x - cx) * s + (y - cy) * c + cy,
    )
}

fn points_box(points: &[P]) -> Bounds {
    points.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |(a, b, c, d), &(x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)),
    )
}

/// Whether the first and last of 3 or more points are within 8 px (`isPathALoop`).
fn is_loop(points: &[P]) -> bool {
    points.len() >= 3 && {
        let (a, b) = (points[0], points[points.len() - 1]);
        (a.0 - b.0).hypot(a.1 - b.1) <= 8.0
    }
}

fn transparent(color: &str) -> bool {
    let c: Vec<char> = color.chars().collect();
    (c.len() == 5 && c[4] == '0')
        || (c.len() == 9 && c[7] == '0' && c[8] == '0')
        || color == "transparent"
}

/// The corner radius for a side of `x` px (`getCornerRadius`).
fn corner_radius(x: f64, e: &Element) -> f64 {
    match e.roundness.map(|r| (r.kind, r.value)) {
        Some((1 | 2, _)) => x * 0.25,
        Some((3, value)) => {
            let fixed = value.unwrap_or(32.0);
            if x <= fixed / 0.25 { x * 0.25 } else { fixed }
        }
        _ => 0.0,
    }
}

fn adjust_roughness(e: &Element) -> f64 {
    let (max, min) = (e.width.max(e.height), e.width.min(e.height));
    let round_kind = matches!(
        e.kind,
        Kind::Rectangle
            | Kind::Iframe
            | Kind::Embeddable
            | Kind::Line
            | Kind::Diamond
            | Kind::Image
    );
    if (min >= 20.0 && max >= 50.0)
        || (min >= 15.0 && e.roundness.is_some() && round_kind)
        || (e.kind.linear() && max >= 50.0)
    {
        return e.roughness;
    }
    (e.roughness / if max < 10.0 { 3.0 } else { 2.0 }).min(2.5)
}

/// rough.js options for an element (`generateRoughOptions`).
fn rough_options(e: &Element, continuous: bool) -> Options {
    let solid = e.stroke_style == Stroke::Solid;
    let sw = e.stroke_width;
    let mut o = Options {
        seed: e.seed,
        dash: match e.stroke_style {
            Stroke::Dashed => Some((8.0, 8.0 + sw)),
            Stroke::Dotted => Some((1.5, 6.0 + sw)),
            Stroke::Solid => None,
        },
        disable_multi_stroke: !solid,
        stroke_width: if solid { sw } else { sw + 0.5 },
        fill_weight: sw / 2.0,
        hachure_gap: sw * 4.0,
        roughness: adjust_roughness(e),
        stroke: e.stroke_color.clone(),
        preserve_vertices: continuous || e.roughness < 2.0,
        ..Options::default()
    };
    match e.kind {
        Kind::Rectangle | Kind::Iframe | Kind::Embeddable | Kind::Diamond | Kind::Ellipse => {
            o.fill_style = e.fill_style;
            o.fill = (!transparent(&e.background_color)).then(|| e.background_color.clone());
            if e.kind == Kind::Ellipse {
                o.curve_fitting = 1.0;
            }
        }
        Kind::Line | Kind::Freedraw if is_loop(&e.points) => {
            o.fill_style = e.fill_style;
            o.fill = (e.background_color != "transparent").then(|| e.background_color.clone());
        }
        _ => {}
    }
    o
}

/// An embeddable or iframe as its export placeholder draws it.
fn placeholder_look(e: &Element) -> Element {
    let mut e = e.clone();
    if transparent(&e.background_color) && transparent(&e.stroke_color) {
        e.roughness = 0.0;
        e.background_color = "#d3d3d3".into();
        e.fill_style = Fill::Solid;
    } else if e.kind == Kind::Iframe {
        if transparent(&e.stroke_color) {
            e.stroke_color = "#000000".into();
        }
        if transparent(&e.background_color) {
            e.background_color = "#f4f4f6".into();
        }
    }
    e
}

fn diamond_points(e: &Element) -> [P; 4] {
    let top = ((e.width / 2.0).floor() + 1.0, 0.0);
    let right = (e.width, (e.height / 2.0).floor() + 1.0);
    [top, right, (top.0, e.height), (0.0, right.1)]
}

/// Builds a path as rough.js reads one: quadratic segments turned cubic.
struct PathData {
    segs: Vec<Seg>,
    at: P,
}

impl PathData {
    fn new(p: P) -> Self {
        Self {
            segs: vec![Seg::M(p)],
            at: p,
        }
    }

    fn l(mut self, p: P) -> Self {
        self.segs.push(Seg::L(p));
        self.at = p;
        self
    }

    fn q(mut self, c: P, p: P) -> Self {
        self.segs.push(rough::quad(self.at, c, p));
        self.at = p;
        self
    }

    fn c(mut self, a: P, b: P, p: P) -> Self {
        self.segs.push(Seg::C(a, b, p));
        self.at = p;
        self
    }
}

/// The rough.js shapes of an element (`_generateElementShape`).
fn shapes(e: &Element, background: &str) -> Vec<Drawable> {
    match e.kind {
        Kind::Rectangle | Kind::Iframe | Kind::Embeddable => {
            let look = if e.kind.iframe_like() {
                placeholder_look(e)
            } else {
                e.clone()
            };
            let (w, h) = (e.width, e.height);
            if e.roundness.is_some() {
                let r = corner_radius(w.min(h), e);
                let p = PathData::new((r, 0.0))
                    .l((w - r, 0.0))
                    .q((w, 0.0), (w, r))
                    .l((w, h - r))
                    .q((w, h), (w - r, h))
                    .l((r, h))
                    .q((0.0, h), (0.0, h - r))
                    .l((0.0, r))
                    .q((0.0, 0.0), (r, 0.0));
                vec![rough::path(&p.segs, &rough_options(&look, true))]
            } else {
                vec![rough::rectangle(
                    0.0,
                    0.0,
                    w,
                    h,
                    &rough_options(&look, false),
                )]
            }
        }
        Kind::Diamond => {
            let [t, r, b, l] = diamond_points(e);
            if e.roundness.is_some() {
                let vr = corner_radius((t.0 - l.0).abs(), e);
                let hr = corner_radius((r.1 - t.1).abs(), e);
                let p = PathData::new((t.0 + vr, t.1 + hr))
                    .l((r.0 - vr, r.1 - hr))
                    .c(r, r, (r.0 - vr, r.1 + hr))
                    .l((b.0 + vr, b.1 - hr))
                    .c(b, b, (b.0 - vr, b.1 - hr))
                    .l((l.0 + vr, l.1 + hr))
                    .c(l, l, (l.0 + vr, l.1 - hr))
                    .l((t.0 - vr, t.1 + hr))
                    .c(t, t, (t.0 + vr, t.1 + hr));
                vec![rough::path(&p.segs, &rough_options(e, true))]
            } else {
                vec![rough::polygon(&[t, r, b, l], &rough_options(e, false))]
            }
        }
        Kind::Ellipse => vec![rough::ellipse(
            e.width / 2.0,
            e.height / 2.0,
            e.width,
            e.height,
            &rough_options(e, false),
        )],
        Kind::Line | Kind::Arrow => linear_shapes(e, background),
        _ => vec![],
    }
}

/// The first shape of a line or arrow as Excalidraw measures its bounds with: a curve when
/// round, a filled polygon or a polyline otherwise.
fn linear_curve(e: &Element, o: &Options) -> Drawable {
    if e.roundness.is_some() {
        rough::curve(&e.points, o)
    } else if o.fill.is_some() {
        rough::polygon(&e.points, o)
    } else {
        rough::linear_path(&e.points, o)
    }
}

fn linear_shapes(e: &Element, background: &str) -> Vec<Drawable> {
    let mut options = rough_options(e, false);
    let mut shapes = if e.elbowed {
        if e.points.iter().any(|p| p.0.abs() > 1e6 || p.1.abs() > 1e6) {
            return vec![];
        }
        vec![rough::path(
            &elbow_path(&e.points, 16.0),
            &rough_options(e, true),
        )]
    } else {
        vec![linear_curve(e, &options)]
    };
    if e.kind == Kind::Arrow {
        for (start, head) in [(true, &e.start_arrowhead), (false, &e.end_arrowhead)] {
            if let Some(head) = head {
                let more = arrowhead(e, &shapes, start, head, &mut options, background);
                shapes.extend(more);
            }
        }
    }
    shapes
}

/// An elbow arrow's segments with their corners rounded (`generateElbowArrowShape`).
fn elbow_path(points: &[P], radius: f64) -> Vec<Seg> {
    let horizontal = |p: P, o: P| {
        let (x, y) = (p.0 - o.0, p.1 - o.1);
        x > y.abs() || x <= -y.abs()
    };
    let dist = |a: P, b: P| (a.0 - b.0).hypot(a.1 - b.1);
    let mut sub = vec![];
    for i in 1..points.len().saturating_sub(1) {
        let (prev, p, next) = (points[i - 1], points[i], points[i + 1]);
        let corner = radius.min(dist(p, next) / 2.0).min(dist(p, prev) / 2.0);
        let side = |toward: P, h: bool| match h {
            true if toward.0 < p.0 => (p.0 - corner, p.1),
            true => (p.0 + corner, p.1),
            false if toward.1 < p.1 => (p.0, p.1 - corner),
            false => (p.0, p.1 + corner),
        };
        sub.push(side(prev, horizontal(p, prev)));
        sub.push(p);
        sub.push(side(next, horizontal(next, p)));
    }
    let mut d = PathData::new(points[0]);
    for [a, b, c] in sub.as_chunks::<3>().0 {
        d = d.l(*a).q(*b, *c);
    }
    d.l(points[points.len() - 1]).segs
}

fn arrowhead_size(head: &str) -> f64 {
    match head {
        "arrow" => 25.0,
        "diamond" | "diamond_outline" => 12.0,
        "crowfoot_many" | "crowfoot_one" | "crowfoot_one_or_many" => 20.0,
        _ => 15.0,
    }
}

fn arrowhead_angle(head: &str) -> f64 {
    match head {
        "bar" => 90.0,
        "arrow" => 20.0,
        _ => 25.0,
    }
}

/// The ops of a drawable's outline: its first path set (`getCurvePathOps`).
fn curve_ops(d: &Drawable) -> &[Op] {
    let set = (d.sets.iter())
        .find(|s| s.kind == rough::Kind::Path)
        .or(d.sets.first());
    set.map_or(&[], |s| &s.ops)
}

/// Where an arrowhead's points go (`getArrowheadPoints`).
fn arrowhead_points(e: &Element, shapes: &[Drawable], start: bool, head: &str) -> Option<Vec<f64>> {
    let ops = curve_ops(shapes.first()?);
    if ops.len() < 2 {
        return None;
    }
    let index = if start { 1 } else { ops.len() - 1 };
    let Op::Curve(p1, p2, p3) = ops[index] else {
        return None;
    };
    let p0 = match ops[index - 1] {
        Op::Move(p) => p,
        Op::Curve(_, _, p) => p,
        Op::Line(_) => (0.0, 0.0),
    };
    let equation = |t: f64, i: usize| {
        let pick = |p: P| if i == 0 { p.0 } else { p.1 };
        (1.0 - t).powi(3) * pick(p3)
            + 3.0 * t * (1.0 - t).powi(2) * pick(p2)
            + 3.0 * t.powi(2) * (1.0 - t) * pick(p1)
            + pick(p0) * t.powi(3)
    };
    let (x2, y2) = if start { p0 } else { p3 };
    let (x1, y1) = (equation(0.3, 0), equation(0.3, 1));
    let distance = (x2 - x1).hypot(y2 - y1);
    if distance == 0.0 {
        return None;
    }
    let (nx, ny) = ((x2 - x1) / distance, (y2 - y1) / distance);
    let size = arrowhead_size(head);
    let pts = &e.points;
    let last = pts.len() - 1;
    let (c, p) = match start {
        true => (pts[0], if pts.len() > 1 { pts[1] } else { (0.0, 0.0) }),
        false => (
            pts[last],
            if pts.len() > 1 {
                pts[last - 1]
            } else {
                (0.0, 0.0)
            },
        ),
    };
    let length = (c.0 - p.0).hypot(c.1 - p.1);
    let multiplier = if head.starts_with("diamond") {
        0.25
    } else {
        0.5
    };
    let min_size = size.min(length * multiplier);
    let (xs, ys) = (x2 - nx * min_size, y2 - ny * min_size);
    if matches!(head, "dot" | "circle" | "circle_outline") {
        let diameter = (ys - y2).hypot(xs - x2) + e.stroke_width - 2.0;
        return Some(vec![x2, y2, diameter]);
    }
    let angle = arrowhead_angle(head).to_radians();
    if matches!(head, "crowfoot_many" | "crowfoot_one_or_many") {
        let (x3, y3) = rotate((x2, y2), (xs, ys), -angle);
        let (x4, y4) = rotate((x2, y2), (xs, ys), angle);
        return Some(vec![xs, ys, x3, y3, x4, y4]);
    }
    let (x3, y3) = rotate((xs, ys), (x2, y2), -arrowhead_angle(head) * PI / 180.0);
    let (x4, y4) = rotate((xs, ys), (x2, y2), angle);
    if head.starts_with("diamond") {
        let (ox, oy) = match start {
            true => {
                let (px, py) = if pts.len() > 1 { pts[1] } else { (0.0, 0.0) };
                rotate(
                    (x2 + min_size * 2.0, y2),
                    (x2, y2),
                    (py - y2).atan2(px - x2),
                )
            }
            false => {
                let (px, py) = if pts.len() > 1 {
                    pts[last - 1]
                } else {
                    (0.0, 0.0)
                };
                rotate(
                    (x2 - min_size * 2.0, y2),
                    (x2, y2),
                    (y2 - py).atan2(x2 - px),
                )
            }
        };
        return Some(vec![x2, y2, x3, y3, ox, oy, x4, y4]);
    }
    Some(vec![x2, y2, x3, y3, x4, y4])
}

/// An arrowhead's shapes (`getArrowheadShapes`); `options` are the arrow's, changed as
/// Excalidraw changes them for the heads that follow.
fn arrowhead(
    e: &Element,
    shapes: &[Drawable],
    start: bool,
    head: &str,
    options: &mut Options,
    background: &str,
) -> Vec<Drawable> {
    let Some(p) = arrowhead_points(e, shapes, start, head) else {
        return vec![];
    };
    let filled = |options: &Options, outline: bool, roughness: f64| Options {
        fill: Some(if outline {
            background.into()
        } else {
            e.stroke_color.clone()
        }),
        fill_style: Fill::Solid,
        stroke: e.stroke_color.clone(),
        roughness: options.roughness.min(roughness),
        ..options.clone()
    };
    match head {
        "dot" | "circle" | "circle_outline" => {
            options.dash = None;
            let o = filled(options, head == "circle_outline", 0.5);
            vec![rough::ellipse(p[0], p[1], p[2], p[2], &o)]
        }
        "triangle" | "triangle_outline" => {
            options.dash = None;
            let o = filled(options, head == "triangle_outline", 1.0);
            let pts = [(p[0], p[1]), (p[2], p[3]), (p[4], p[5]), (p[0], p[1])];
            vec![rough::polygon(&pts, &o)]
        }
        "diamond" | "diamond_outline" => {
            options.dash = None;
            let o = filled(options, head == "diamond_outline", 1.0);
            let pts = [
                (p[0], p[1]),
                (p[2], p[3]),
                (p[4], p[5]),
                (p[6], p[7]),
                (p[0], p[1]),
            ];
            vec![rough::polygon(&pts, &o)]
        }
        "crowfoot_one" => vec![rough::line((p[2], p[3]), (p[4], p[5]), options)],
        _ => {
            options.dash = match e.stroke_style {
                // A dotted arrow's head has smaller gaps, so it reads.
                Stroke::Dotted => Some((1.5, 6.0 + e.stroke_width - 1.0 - 1.0)),
                _ => None,
            };
            options.roughness = options.roughness.min(1.0);
            let tip = (p[0], p[1]);
            let mut out = vec![
                rough::line((p[2], p[3]), tip, options),
                rough::line((p[4], p[5]), tip, options),
            ];
            if head == "crowfoot_one_or_many"
                && let Some(one) = arrowhead_points(e, shapes, start, "crowfoot_one")
            {
                out.push(rough::line((one[2], one[3]), (one[4], one[5]), options));
            }
            out
        }
    }
}

/// The ends of a cubic Bézier and where it turns, for its bounds (`getCubicBezierCurveBound`).
fn bezier_bounds(p0: P, p1: P, p2: P, p3: P) -> Bounds {
    let solve = |a: f64, b: f64, c: f64, d: f64| -> Vec<f64> {
        let i = b - a;
        let j = c - b;
        let k = d - c;
        let qa = 3.0 * i - 6.0 * j + 3.0 * k;
        let qb = 6.0 * j - 6.0 * i;
        let qc = 3.0 * i;
        let sq = qb * qb - 4.0 * qa * qc;
        if sq < 0.0 {
            return vec![];
        }
        let at = |t: f64| {
            (1.0 - t).powi(3) * a
                + 3.0 * (1.0 - t).powi(2) * t * b
                + 3.0 * (1.0 - t) * t * t * c
                + t.powi(3) * d
        };
        let mut out = vec![];
        let roots = match qa {
            0.0 => [-qc / qb, -qc / qb],
            _ => [
                (-qb + sq.sqrt()) / (2.0 * qa),
                (-qb - sq.sqrt()) / (2.0 * qa),
            ],
        };
        for t in roots {
            if (0.0..=1.0).contains(&t) {
                out.push(at(t));
            }
        }
        out
    };
    let (mut x0, mut x1) = (p0.0.min(p3.0), p0.0.max(p3.0));
    for x in solve(p0.0, p1.0, p2.0, p3.0) {
        (x0, x1) = (x0.min(x), x1.max(x));
    }
    let (mut y0, mut y1) = (p0.1.min(p3.1), p0.1.max(p3.1));
    for y in solve(p0.1, p1.1, p2.1, p3.1) {
        (y0, y1) = (y0.min(y), y1.max(y));
    }
    (x0, y0, x1, y1)
}

/// The SVG being drawn, and what the elements need while drawing it.
struct Drawing<'a> {
    elements: &'a [Element],
    by_id: HashMap<&'a str, usize>,
    files: &'a HashMap<String, String>,
    background: &'a str,
    /// Container index → its bound text's index.
    bound: HashMap<usize, usize>,
    /// Shapes made so far, so bounds and drawing share them.
    shapes: HashMap<usize, Vec<Drawable>>,
    body: String,
    defs: String,
    /// Text lines to outline and their colours; the body holds `\0{index}\0` where each goes.
    texts: Vec<text::Line>,
    fills: Vec<String>,
    warnings: Vec<Warning>,
    widths: &'a Widths,
    /// Image symbols made so far: by file id and, when cropped, size.
    symbols: HashMap<(Option<String>, Option<String>), usize>,
}

impl<'a> Drawing<'a> {
    fn shapes(&mut self, i: usize) -> &[Drawable] {
        let (e, bg) = (&self.elements[i], self.background);
        self.shapes.entry(i).or_insert_with(|| shapes(e, bg))
    }

    /// Where an element's box is, before rotation: (x1, y1, x2, y2), from its points for lines
    /// and freedraws (`getElementAbsoluteCoords` with nothing cached, as a fresh export has).
    fn coords(&self, i: usize) -> Bounds {
        let e = &self.elements[i];
        match e.kind {
            Kind::Line | Kind::Arrow | Kind::Freedraw if !e.points.is_empty() => {
                let (a, b, c, d) = points_box(&e.points);
                (a + e.x, b + e.y, c + e.x, d + e.y)
            }
            Kind::Text => match self.arrow_of(i) {
                Some(a) => {
                    let (x, y) = self.text_on_arrow(a, i);
                    (x, y, x + e.width, y + e.height)
                }
                None => (e.x, e.y, e.x + e.width, e.y + e.height),
            },
            _ => (e.x, e.y, e.x + e.width, e.y + e.height),
        }
    }

    /// The arrow or line a text is bound to.
    fn arrow_of(&self, i: usize) -> Option<usize> {
        let c = *self.by_id.get(self.elements[i].container_id.as_deref()?)?;
        self.elements[c].kind.linear().then_some(c)
    }

    /// Where a text bound to a line or arrow goes: centred on its middle point, or on the
    /// middle of its middle segment (`getBoundTextElementPosition`).
    fn text_on_arrow(&self, arrow: usize, text: usize) -> P {
        let (a, t) = (&self.elements[arrow], &self.elements[text]);
        let (x1, y1, x2, y2) = self.coords(arrow);
        let centre = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let global = |p: P| rotate((a.x + p.0, a.y + p.1), centre, a.angle);
        let n = a.points.len();
        let mid = if n % 2 == 1 {
            global(a.points[n / 2])
        } else {
            let (p, q) = (global(a.points[n / 2 - 1]), global(a.points[n / 2]));
            ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0)
        };
        (mid.0 - t.width / 2.0, mid.1 - t.height / 2.0)
    }

    /// An element's box with its rotation, as the export measures the canvas
    /// (`ElementBounds.calculateBounds`).
    fn bounds(&mut self, i: usize) -> Bounds {
        let (x1, y1, x2, y2) = self.coords(i);
        let e = &self.elements[i];
        let (cx, cy) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let c = (cx, cy);
        let a = e.angle;
        let corners =
            |pts: &[P]| points_box(&pts.iter().map(|&p| rotate(p, c, a)).collect::<Vec<_>>());
        match e.kind {
            Kind::Freedraw => {
                let local = (cx - e.x, cy - e.y);
                let pts: Vec<P> = e.points.iter().map(|&p| rotate(p, local, a)).collect();
                let (a, b, c, d) = points_box(&pts);
                (a + e.x, b + e.y, c + e.x, d + e.y)
            }
            Kind::Line | Kind::Arrow => self.linear_bounds(i, c),
            Kind::Diamond => corners(&[(cx, y1), (cx, y2), (x1, cy), (x2, cy)]),
            Kind::Ellipse => {
                let (w, h) = ((x2 - x1) / 2.0, (y2 - y1) / 2.0);
                let (s, co) = a.sin_cos();
                let (ww, hh) = ((w * co).hypot(h * s), (h * co).hypot(w * s));
                (cx - ww, cy - hh, cx + ww, cy + hh)
            }
            _ => corners(&[(x1, y1), (x1, y2), (x2, y2), (x2, y1)]),
        }
    }

    fn linear_bounds(&mut self, i: usize, c: P) -> Bounds {
        let e = &self.elements[i];
        let mut b = if e.points.len() < 2 {
            let p = e.points.first().copied().unwrap_or_default();
            let (x, y) = rotate((e.x + p.0, e.y + p.1), c, e.angle);
            (x, y, x, y)
        } else {
            let curve;
            let ops = if e.elbowed {
                curve = linear_curve(e, &rough_options(e, false));
                curve_ops(&curve).to_vec()
            } else {
                curve_ops(&self.shapes(i)[0]).to_vec()
            };
            let e = &self.elements[i];
            let t = |p: P| rotate((e.x + p.0, e.y + p.1), c, e.angle);
            let mut b = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            let mut at = (0.0, 0.0);
            for op in ops {
                match op {
                    Op::Move(p) => at = p,
                    Op::Curve(p1, p2, p3) => {
                        let (x0, y0, x1, y1) = bezier_bounds(t(at), t(p1), t(p2), t(p3));
                        b = (b.0.min(x0), b.1.min(y0), b.2.max(x1), b.3.max(y1));
                        at = p3;
                    }
                    Op::Line(_) => {}
                }
            }
            b
        };
        if let Some(&text) = self.bound.get(&i) {
            b = self.with_bound_text(i, text, b);
        }
        b
    }

    /// The bounds of a line grown to hold its bound text (`getMinMaxXYWithBoundText`).
    fn with_bound_text(
        &self,
        i: usize,
        text: usize,
        (mut x1, mut y1, mut x2, mut y2): Bounds,
    ) -> Bounds {
        let (e, t) = (&self.elements[i], &self.elements[text]);
        let c = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let (tx1, ty1) = self.text_on_arrow(i, text);
        let (tx2, ty2) = (tx1 + t.width, ty1 + t.height);
        let tl = rotate((x1, y1), c, e.angle);
        let tr = rotate((x2, y1), c, e.angle);
        let back = |p: P| rotate(p, c, -e.angle);
        let (btl, btr, bbl, bbr) = (
            back((tx1, ty1)),
            back((tx2, ty1)),
            back((tx1, ty2)),
            back((tx2, ty2)),
        );
        if tl.0 < tr.0 && tl.1 >= tr.1 {
            x1 = x1.min(bbl.0);
            x2 = x2.max(btr.0.max(bbr.0));
            y1 = y1.min(btl.1);
            y2 = y2.max(bbr.1);
        } else if tl.0 >= tr.0 && tl.1 > tr.1 {
            x1 = x1.min(bbr.0);
            x2 = x2.max(btl.0.max(btr.0));
            y1 = y1.min(bbl.1);
            y2 = y2.max(btr.1);
        } else if tl.0 >= tr.0 {
            x1 = x1.min(btr.0);
            x2 = x2.max(bbl.0);
            y1 = y1.min(bbr.1);
            y2 = y2.max(btl.1);
        } else if tl.1 <= tr.1 {
            x1 = x1.min(btr.0.min(btl.0));
            x2 = x2.max(bbr.0);
            y1 = y1.min(btr.1);
            y2 = y2.max(bbl.1);
        }
        (x1, y1, x2, y2)
    }

    /// The frame an element is in, when there is one.
    fn frame_of(&self, e: &Element) -> Option<usize> {
        self.by_id.get(e.frame_id.as_deref()?).copied()
    }

    fn draw(&mut self, background: Option<&str>) -> Result<String, Error> {
        // The canvas: every element not inside a frame, and the frames.
        let mut b = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for i in 0..self.elements.len() {
            let e = &self.elements[i];
            let in_frame = self
                .frame_of(e)
                .is_some_and(|f| self.elements[f].kind.frame());
            if e.kind.frame() || !in_frame {
                let (x0, y0, x1, y1) = self.bounds(i);
                b = (b.0.min(x0), b.1.min(y0), b.2.max(x1), b.3.max(y1));
            }
        }
        if !b.0.is_finite() {
            b = (0.0, 0.0, 0.0, 0.0);
        }
        let (width, height) = (b.2 - b.0 + PADDING * 2.0, b.3 - b.1 + PADDING * 2.0);
        let offset = (-b.0 + PADDING, -b.1 + PADDING);
        for (k, e) in self.elements.iter().enumerate() {
            if e.kind.frame() {
                let centre = (e.width / 2.0, e.height / 2.0);
                let _ = writeln!(
                    self.defs,
                    r#"<clipPath id="frame-{k}"><rect{} width="{}" height="{}" rx="{r}" ry="{r}"/></clipPath>"#,
                    placed((e.x + offset.0, e.y + offset.1), e.angle, centre),
                    n(e.width),
                    n(e.height),
                    r = FRAME_RADIUS
                );
            }
        }
        let els = self.elements;
        for (i, e) in els.iter().enumerate() {
            let bound = e
                .container_id
                .as_deref()
                .is_some_and(|c| self.by_id.contains_key(c));
            if e.kind.iframe_like() || (e.kind == Kind::Text && bound) {
                continue;
            }
            self.element(i, e.x + offset.0, e.y + offset.1);
            if let Some(&t) = self.bound.get(&i) {
                self.element(t, els[t].x + offset.0, els[t].y + offset.1);
            }
        }
        for (i, e) in els.iter().enumerate() {
            if e.kind.iframe_like() {
                self.element(i, e.x + offset.0, e.y + offset.1);
            }
        }
        let body = self.outline_texts()?;
        let (w, h) = (n(width), n(height));
        let mut svg = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n"
        );
        if !self.defs.is_empty() {
            let _ = write!(svg, "<defs>\n{}</defs>\n", self.defs);
        }
        let fill = background.unwrap_or(self.background);
        if fill != "transparent" {
            let _ = writeln!(
                svg,
                r#"<rect x="0" y="0" width="{w}" height="{h}" fill="{}"/>"#,
                escape(fill)
            );
        }
        Ok(svg + &body + "</svg>\n")
    }

    /// The body with each text line's glyph outlines in place of its marker.
    fn outline_texts(&mut self) -> Result<String, Error> {
        let (paths, warning) = text::outlines(&self.texts)?;
        self.warnings.extend(warning);
        let mut out = String::with_capacity(self.body.len());
        for (k, part) in self.body.split('\0').enumerate() {
            if k % 2 == 0 {
                out += part;
                continue;
            }
            let i: usize = part.parse().unwrap_or(usize::MAX);
            if let (Some(d), Some(l), Some(fill)) =
                (paths.get(i), self.texts.get(i), self.fills.get(i))
                && !d.is_empty()
            {
                let _ = write!(
                    out,
                    r#"<path d="{d}" fill="{}" aria-label="{}"/>"#,
                    escape(fill),
                    escape(&l.text)
                );
            }
        }
        Ok(out)
    }

    /// One element, placed at `(ox, oy)` (`renderElementToSvg`).
    fn element(&mut self, i: usize, mut ox: f64, mut oy: f64) {
        let els = self.elements;
        let e = &els[i];
        let (x1, y1, x2, y2) = self.coords(i);
        let (mut cx, mut cy) = ((x2 - x1) / 2.0 - (e.x - x1), (y2 - y1) / 2.0 - (e.y - y1));
        if let Some(a) = (e.kind == Kind::Text).then(|| self.arrow_of(i)).flatten() {
            let (ax1, ay1, ax2, ay2) = self.coords(a);
            let (bx, by) = self.text_on_arrow(a, i);
            cx = (ax2 - ax1) / 2.0 - (bx - ax1);
            cy = (ay2 - ay1) / 2.0 - (by - ay1);
            ox += bx - e.x;
            oy += by - e.y;
        }
        let frame = self.frame_of(e);
        let frame_opacity = frame.map_or(100.0, |f| self.elements[f].opacity);
        let opacity = frame_opacity * e.opacity / 10000.0;
        let clip = frame.filter(|&f| self.elements[f].kind.frame());
        let place = placed((ox, oy), e.angle, (cx, cy));
        let mut out = String::new();
        match e.kind {
            Kind::Rectangle | Kind::Diamond | Kind::Ellipse => {
                let attrs = format!(r#"{} stroke-linecap="round"{place}"#, faded(opacity));
                for s in self.shapes(i) {
                    rough::svg(&mut out, s, &attrs);
                }
            }
            Kind::Embeddable | Kind::Iframe => {
                let attrs = format!(
                    r#"{} stroke-linecap="round"{place}"#,
                    faded(e.opacity / 100.0)
                );
                for s in self.shapes(i) {
                    rough::svg(&mut out, s, &attrs);
                }
                self.body += &out;
                let label = self.embed_label(e);
                let at = (label.x + ox - e.x, label.y + oy - e.y);
                let place = placed(at, label.angle, (label.width / 2.0, label.height / 2.0));
                self.text(&label, &place, 1.0);
                return;
            }
            Kind::Line | Kind::Arrow => {
                let mut group = String::from(r#"<g stroke-linecap="round""#);
                if let Some(&t) = self.bound.get(&i) {
                    let (bx, by) = self.text_on_arrow(i, t);
                    let te = &self.elements[t];
                    let _ = writeln!(
                        self.defs,
                        r##"<mask id="mask-{i}"><rect x="0" y="0" fill="#fff" width="{}" height="{}"/><rect x="{}" y="{}" fill="#000" width="{}" height="{}" opacity="1"/></mask>"##,
                        n(e.width + 100.0 + ox),
                        n(e.height + 100.0 + oy),
                        n(ox + bx - e.x),
                        n(oy + by - e.y),
                        n(te.width),
                        n(te.height)
                    );
                    let _ = write!(group, r#" mask="url(#mask-{i})""#);
                }
                group += ">\n";
                let evenodd = e.kind == Kind::Line
                    && is_loop(&e.points)
                    && e.background_color != "transparent";
                let rule = if evenodd {
                    r#" fill-rule="evenodd""#
                } else {
                    ""
                };
                let attrs = format!("{}{place}{rule}", faded(opacity));
                for s in self.shapes(i) {
                    rough::svg(&mut group, s, &attrs);
                }
                out = group + "</g>\n";
            }
            Kind::Freedraw => {
                let o = Options {
                    stroke: "none".into(),
                    ..rough_options(e, false)
                };
                let attrs = format!(r#"{}{place} stroke="none""#, faded(opacity));
                let outline =
                    freehand::path(&e.points, &e.pressures, e.simulate_pressure, e.stroke_width);
                let path = format!(
                    r#"<path fill="{}" d="{outline}"/>"#,
                    escape(&e.stroke_color)
                );
                if is_loop(&e.points) {
                    let fill = rough::curve(&rough::simplify(&e.points, 0.75), &o);
                    let mut g = String::new();
                    rough::svg(&mut g, &fill, &attrs);
                    let g = g.trim_end().strip_suffix("</g>").unwrap_or(&g).to_string();
                    out = g + &path + "</g>\n";
                } else {
                    out = format!("<g{attrs}>{path}</g>\n");
                }
            }
            Kind::Image => out = self.image(i, (ox, oy), (cx, cy), opacity),
            Kind::Frame | Kind::MagicFrame => {
                let _ = writeln!(
                    out,
                    r##"<rect{place} width="{}px" height="{}px" rx="{r}" ry="{r}" fill="none" stroke="#bbb" stroke-width="2"/>"##,
                    n(e.width),
                    n(e.height),
                    r = FRAME_RADIUS
                );
            }
            Kind::Text => {
                let mut sink = String::new();
                std::mem::swap(&mut self.body, &mut sink);
                self.text(e, &place, opacity);
                std::mem::swap(&mut self.body, &mut sink);
                out = sink;
            }
        }
        if let Some(f) = clip {
            out = format!("<g clip-path=\"url(#frame-{f})\">{out}</g>\n");
        }
        if let Some(link) = &e.link {
            out = format!("<a href=\"{}\">{out}</a>\n", escape(link));
        }
        self.body += &out;
    }

    /// A text element's lines, each to be outlined, in a group placed by `place`.
    fn text(&mut self, e: &Element, place: &str, opacity: f64) {
        let font = text::drawn_with(e.font_family);
        let swapped = text::substituted(e.font_family);
        if font != e.font_family && !self.warnings.contains(&swapped) {
            self.warnings.push(swapped);
        }
        let line_px = e.font_size * e.line_height;
        let x = match e.text_align.as_str() {
            "center" => e.width / 2.0,
            "right" => e.width,
            _ => 0.0,
        };
        let top = text::baseline(e.font_family, e.font_size, line_px);
        let rtl = text::rtl(&e.text);
        let anchor = match e.text_align.as_str() {
            "center" => "middle",
            "right" => "end",
            _ if rtl => "end",
            _ => "start",
        };
        let _ = write!(self.body, "<g{}{place}>", faded(opacity));
        for (k, line) in e
            .text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .split('\n')
            .enumerate()
        {
            let at = self.texts.len();
            self.texts.push(text::Line {
                text: line.to_string(),
                x,
                y: k as f64 * line_px + top,
                size: e.font_size,
                font,
                anchor,
                rtl,
            });
            self.fills.push(e.stroke_color.clone());
            let _ = write!(self.body, "\0{at}\0");
        }
        self.body += "</g>\n";
    }

    fn embed_label(&self, e: &Element) -> Element {
        let text = match (e.kind, &e.link) {
            (Kind::Iframe, _) => "IFrame element".to_string(),
            (_, Some(link)) => link.clone(),
            _ => "Empty Web-Embed".to_string(),
        };
        let chars = text.chars().count().max(1) as f64;
        let size = (e.width / 2.0).min(e.width / chars).max(e.width / 30.0);
        let lines = wrap(&text, self.widths, size, e.width - 20.0);
        let width = lines
            .iter()
            .map(|l| self.widths.of(l, size))
            .fold(0.0, f64::max);
        let height = size * 1.15 * lines.len() as f64;
        Element {
            id: String::new(),
            kind: Kind::Text,
            x: e.x + e.width / 2.0 - width / 2.0,
            y: e.y + e.height / 2.0 - height / 2.0,
            width,
            height,
            stroke_color: if e.stroke_color != "transparent" {
                e.stroke_color.clone()
            } else {
                "black".into()
            },
            opacity: 100.0,
            frame_id: None,
            link: None,
            text: lines.join("\n"),
            font_size: size,
            font_family: 2,
            text_align: "center".into(),
            line_height: 1.15,
            container_id: None,
            ..e.clone()
        }
    }

    fn image(&mut self, i: usize, (ox, oy): P, (cx, cy): P, opacity: f64) -> String {
        let els = self.elements;
        let e = &els[i];
        let (width, height) = (e.width.round(), e.height.round());
        let url = (e.file_id.as_ref())
            .and_then(|f| self.files.get(f))
            .filter(|u| u.starts_with("data:image/"));
        let Some(url) = url else {
            self.warnings.push(Warning::image_missing(&e.id, e.line));
            return format!(
                "<g{} opacity=\"{}\"><rect width=\"{}\" height=\"{}\" fill=\"#E7E7E7\"/></g>\n",
                placed((ox, oy), e.angle, (cx, cy)),
                n(opacity),
                n(e.width),
                n(e.height)
            );
        };
        let (uw, uh) = match e.crop {
            Some(c) => (
                e.width / (c.width / c.natural_width),
                e.height / (c.height / c.natural_height),
            ),
            None => (e.width, e.height),
        };
        let (crop_x, crop_y) = match e.crop {
            Some(c) => (c.x / (c.natural_width / uw), c.y / (c.natural_height / uh)),
            None => (0.0, 0.0),
        };
        let size = match e.crop {
            Some(_) => format!(r#"width="{}" height="{}""#, n(uw), n(uh)),
            None => r#"width="100%" height="100%""#.into(),
        };
        // One symbol per file, and per size for cropped images, as Excalidraw reuses them.
        let key = (e.file_id.clone(), e.crop.map(|_| format!("{uw}x{uh}")));
        let next = self.symbols.len();
        let symbol = *self.symbols.entry(key).or_insert(next);
        if symbol == next {
            let _ = writeln!(
                self.defs,
                r#"<symbol id="image-{symbol}"><image href="{}" preserveAspectRatio="none" {size}/></symbol>"#,
                escape(url)
            );
        }
        let (acx, acy) = (cx + crop_x, cy + crop_y);
        let flip = if e.scale != (1.0, 1.0) {
            format!(
                r#" transform="translate({} {}) scale({} {}) translate({} {})""#,
                n(acx),
                n(acy),
                n(e.scale.0),
                n(e.scale.1),
                n(-acx),
                n(-acy)
            )
        } else {
            String::new()
        };
        let mut g = format!(
            "<g{}",
            placed((ox - crop_x, oy - crop_y), e.angle, (acx, acy))
        );
        if e.crop.is_some() {
            let _ = writeln!(
                self.defs,
                r##"<mask id="mask-image-crop-{i}" fill="#fff"><rect x="{}" y="{}" width="{}" height="{}"/></mask>"##,
                n(crop_x),
                n(crop_y),
                n(width),
                n(height)
            );
            let _ = write!(g, r#" mask="url(#mask-image-crop-{i})""#);
        }
        if e.roundness.is_some() {
            let r = corner_radius(e.width.min(e.height), e);
            let _ = writeln!(
                self.defs,
                r#"<clipPath id="image-clipPath-{i}"><rect width="{}" height="{}" rx="{r}" ry="{r}"/></clipPath>"#,
                n(e.width),
                n(e.height),
                r = n(r)
            );
            let _ = write!(g, r#" clip-path="url(#image-clipPath-{i})""#);
        }
        let _ = writeln!(
            g,
            r##"><use href="#image-{symbol}" width="{}" height="{}" opacity="{}"{flip}/></g>"##,
            n(width + crop_x),
            n(height + crop_y),
            n(opacity)
        );
        g
    }
}

/// The attribute that moves something to `at` and turns it by `angle` about `centre`.
fn placed(at: P, angle: f64, centre: P) -> String {
    format!(
        r#" transform="translate({} {}) rotate({} {} {})""#,
        n(at.0),
        n(at.1),
        n(180.0 * angle / PI),
        n(centre.0),
        n(centre.1)
    )
}

/// The attributes of something drawn at `opacity`, from 0 to 1.
fn faded(opacity: f64) -> String {
    match opacity {
        1.0 => String::new(),
        o => format!(r#" stroke-opacity="{0}" fill-opacity="{0}""#, n(o)),
    }
}

/// `text` broken into lines no wider than `max` at `size` px: at spaces when it can, else
/// within words.
fn wrap(text: &str, widths: &Widths, size: f64, max: f64) -> Vec<String> {
    if !max.is_finite() || max <= 0.0 || widths.of(text, size) <= max {
        return vec![text.to_string()];
    }
    let mut lines = vec![];
    let mut line = String::new();
    for word in text.split(' ') {
        let tried = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if widths.of(&tried, size) <= max {
            line = tried;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        for c in word.chars() {
            let tried = format!("{line}{c}");
            if !line.is_empty() && widths.of(&tried, size) > max {
                lines.push(std::mem::take(&mut line));
            }
            line.push(c);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
