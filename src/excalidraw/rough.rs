//! Hand-drawn strokes and fills, ported from rough.js 4.6.4 (MIT, github.com/rough-stuff/rough,
//! `src/renderer.ts`, `generator.ts`, `svg.ts`, `fillers/`) with hachure-fill 0.5.2,
//! points-on-curve 0.2.0 and points-on-path 0.2.1 (MIT, same author). Every number is computed
//! in the same order with the same seeded generator, so a seed wobbles as in Excalidraw's export.

use std::cell::Cell;
use std::f64::consts::PI;
use std::fmt::Write;
use std::rc::Rc;

use crate::svg::{escape, num as n};

pub type P = (f64, f64);

/// rough.js defaults that Excalidraw never changes.
const MAX_RANDOMNESS_OFFSET: f64 = 2.0;
const CURVE_STEP_COUNT: f64 = 9.0;
const FILL_SHAPE_ROUGHNESS_GAIN: f64 = 0.8;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fill {
    Hachure,
    CrossHatch,
    Solid,
    Zigzag,
}

#[derive(Clone)]
pub struct Options {
    pub roughness: f64,
    /// `none` draws no outline.
    pub stroke: String,
    pub stroke_width: f64,
    pub curve_fitting: f64,
    pub fill_style: Fill,
    pub fill: Option<String>,
    pub fill_weight: f64,
    pub hachure_angle: f64,
    pub hachure_gap: f64,
    pub seed: i32,
    pub disable_multi_stroke: bool,
    pub preserve_vertices: bool,
    pub dash: Option<(f64, f64)>,
    /// The random state, shared by copies made while drawing one shape (as rough.js's
    /// `Object.assign` copies share its randomizer).
    pub(super) rng: Rc<Cell<Option<i32>>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            roughness: 1.0,
            stroke: "#000".into(),
            stroke_width: 1.0,
            curve_fitting: 0.95,
            fill_style: Fill::Hachure,
            fill: None,
            fill_weight: -1.0,
            hachure_angle: -41.0,
            hachure_gap: -1.0,
            seed: 0,
            disable_multi_stroke: false,
            preserve_vertices: false,
            dash: None,
            rng: Rc::default(),
        }
    }
}

impl Options {
    /// A copy with its own random state, started again from the seed.
    fn fresh(&self) -> Self {
        Self {
            rng: Rc::default(),
            ..self.clone()
        }
    }

    /// rough.js's `Random`: Park–Miller with 48271, as 32-bit integers. Seed 0 means
    /// `Math.random()` there; here it is the middle of the range, so nothing wobbles.
    fn random(&self) -> f64 {
        let Some(s) = self.rng.get().or(Some(self.seed)).filter(|&s| s != 0) else {
            return 0.5;
        };
        let s = s.wrapping_mul(48271);
        self.rng.set(Some(s));
        f64::from(s & 0x7fff_ffff) / 2_147_483_648.0
    }

    fn offset(&self, min: f64, max: f64, gain: f64) -> f64 {
        self.roughness * gain * (self.random() * (max - min) + min)
    }

    fn offset_opt(&self, x: f64, gain: f64) -> f64 {
        self.offset(-x, x, gain)
    }

    fn alter_seed(&self) -> Self {
        let mut o = self.fresh();
        if self.seed != 0 {
            o.seed = self.seed.wrapping_add(1);
        }
        o
    }

    /// A copy for a fill drawn as a shape: one stroke, rougher, the same random state.
    fn for_fill_shape(&self) -> Self {
        let mut o = self.clone();
        o.disable_multi_stroke = true;
        if o.roughness != 0.0 {
            o.roughness += FILL_SHAPE_ROUGHNESS_GAIN;
        }
        o
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Op {
    Move(P),
    Curve(P, P, P),
    Line(P),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Path,
    FillPath,
    FillSketch,
}

pub struct Set {
    pub kind: Kind,
    pub ops: Vec<Op>,
}

/// What a generator call makes: its paths and the options they were made with.
pub struct Drawable {
    /// Whether rough.js fills it even-odd (curves and polygons).
    evenodd: bool,
    pub sets: Vec<Set>,
    pub o: Options,
}

/// A path as rough.js's `path()` reads it after normalising: M, L and C (Excalidraw closes none).
#[derive(Clone, Copy)]
pub enum Seg {
    M(P),
    L(P),
    C(P, P, P),
}

/// A quadratic segment from `from` as the cubic path-data-parser turns it into.
pub fn quad(from: P, c: P, to: P) -> Seg {
    Seg::C(
        (
            from.0 + 2.0 * (c.0 - from.0) / 3.0,
            from.1 + 2.0 * (c.1 - from.1) / 3.0,
        ),
        (
            to.0 + 2.0 * (c.0 - to.0) / 3.0,
            to.1 + 2.0 * (c.1 - to.1) / 3.0,
        ),
        to,
    )
}

// The generator.

fn drawable(evenodd: bool, sets: Vec<Set>, o: Options) -> Drawable {
    Drawable { evenodd, sets, o }
}

fn stroked(o: &Options) -> bool {
    o.stroke != "none"
}

pub fn line(a: P, b: P, o: &Options) -> Drawable {
    let o = o.fresh();
    let set = path_set(double_line(a, b, &o, false));
    drawable(false, vec![set], o)
}

pub fn rectangle(x: f64, y: f64, w: f64, h: f64, o: &Options) -> Drawable {
    let o = o.fresh();
    let points = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    let outline = linear_path_set(&points, true, &o);
    let mut sets = vec![];
    if o.fill.is_some() {
        sets.push(fill_polygons(&[points.to_vec()], &o));
    }
    if stroked(&o) {
        sets.push(outline);
    }
    drawable(false, sets, o)
}

pub fn ellipse(x: f64, y: f64, w: f64, h: f64, o: &Options) -> Drawable {
    let o = o.fresh();
    let params = ellipse_params(w, h, &o);
    let (estimated, outline) = ellipse_with_params(x, y, &o, params);
    let mut sets = vec![];
    if o.fill.is_some() {
        if o.fill_style == Fill::Solid {
            let (_, mut shape) = ellipse_with_params(x, y, &o, params);
            shape.kind = Kind::FillPath;
            sets.push(shape);
        } else {
            sets.push(pattern_fill(&[estimated], &o));
        }
    }
    if stroked(&o) {
        sets.push(outline);
    }
    drawable(false, sets, o)
}

pub fn linear_path(points: &[P], o: &Options) -> Drawable {
    let o = o.fresh();
    let set = linear_path_set(points, false, &o);
    drawable(false, vec![set], o)
}

pub fn polygon(points: &[P], o: &Options) -> Drawable {
    let o = o.fresh();
    let outline = linear_path_set(points, true, &o);
    let mut sets = vec![];
    if o.fill.is_some() {
        sets.push(fill_polygons(&[points.to_vec()], &o));
    }
    if stroked(&o) {
        sets.push(outline);
    }
    drawable(true, sets, o)
}

pub fn curve(points: &[P], o: &Options) -> Drawable {
    let o = o.fresh();
    let outline = curve_set(points, &o);
    let mut sets = vec![];
    if o.fill.as_deref().is_some_and(|f| f != "none") && points.len() >= 3 {
        if o.fill_style == Fill::Solid {
            let shape = curve_set(points, &o.for_fill_shape());
            sets.push(Set {
                kind: Kind::FillPath,
                ops: merged(shape.ops),
            });
        } else {
            let bezier = curve_to_bezier(points);
            let poly = points_on_bezier_curves(&bezier, 10.0, (1.0 + o.roughness) / 2.0);
            sets.push(pattern_fill(&[poly], &o));
        }
    }
    if stroked(&o) {
        sets.push(outline);
    }
    drawable(true, sets, o)
}

pub fn path(segs: &[Seg], o: &Options) -> Drawable {
    let o = o.fresh();
    let filled = o
        .fill
        .as_deref()
        .is_some_and(|f| f != "transparent" && f != "none");
    let points = points_on_path(segs, 1.0, (1.0 + o.roughness) / 2.0);
    let shape = svg_path(segs, &o);
    let mut sets = vec![];
    if filled {
        if o.fill_style == Fill::Solid {
            if points.len() == 1 {
                let shape = svg_path(segs, &o.for_fill_shape());
                sets.push(Set {
                    kind: Kind::FillPath,
                    ops: merged(shape.ops),
                });
            } else {
                sets.push(solid_fill(&points, &o));
            }
        } else {
            sets.push(pattern_fill(&points, &o));
        }
    }
    if stroked(&o) {
        sets.push(shape);
    }
    drawable(false, sets, o)
}

/// Drops every move but the first, so a fill is one shape.
fn merged(ops: Vec<Op>) -> Vec<Op> {
    (ops.into_iter().enumerate())
        .filter(|(i, op)| *i == 0 || !matches!(op, Op::Move(_)))
        .map(|(_, op)| op)
        .collect()
}

fn fill_polygons(polygons: &[Vec<P>], o: &Options) -> Set {
    match o.fill_style {
        Fill::Solid => solid_fill(polygons, o),
        _ => pattern_fill(polygons, o),
    }
}

// The renderer.

fn path_set(ops: Vec<Op>) -> Set {
    Set {
        kind: Kind::Path,
        ops,
    }
}

fn linear_path_set(points: &[P], close: bool, o: &Options) -> Set {
    let len = points.len();
    if len > 2 {
        let mut ops = vec![];
        for w in points.windows(2) {
            ops.extend(double_line(w[0], w[1], o, false));
        }
        if close {
            ops.extend(double_line(points[len - 1], points[0], o, false));
        }
        return path_set(ops);
    }
    if len == 2 {
        return path_set(double_line(points[0], points[1], o, false));
    }
    path_set(vec![])
}

fn curve_set(points: &[P], o: &Options) -> Set {
    let mut ops = curve_with_offset(points, 1.0 + o.roughness * 0.2, o);
    if !o.disable_multi_stroke {
        let other = o.alter_seed();
        ops.extend(curve_with_offset(
            points,
            1.5 * (1.0 + o.roughness * 0.22),
            &other,
        ));
    }
    path_set(ops)
}

#[derive(Clone, Copy)]
struct EllipseParams {
    increment: f64,
    rx: f64,
    ry: f64,
}

fn ellipse_params(w: f64, h: f64, o: &Options) -> EllipseParams {
    let psq = (PI * 2.0 * (((w / 2.0).powi(2) + (h / 2.0).powi(2)) / 2.0).sqrt()).sqrt();
    let steps = CURVE_STEP_COUNT
        .max((CURVE_STEP_COUNT / 200f64.sqrt()) * psq)
        .ceil();
    let increment = (PI * 2.0) / steps;
    let (mut rx, mut ry) = ((w / 2.0).abs(), (h / 2.0).abs());
    let randomness = 1.0 - o.curve_fitting;
    rx += o.offset_opt(rx * randomness, 1.0);
    ry += o.offset_opt(ry * randomness, 1.0);
    EllipseParams { increment, rx, ry }
}

fn ellipse_with_params(x: f64, y: f64, o: &Options, e: EllipseParams) -> (Vec<P>, Set) {
    let overlap = e.increment * o.offset(0.1, o.offset(0.4, 1.0, 1.0), 1.0);
    let (all, core) = ellipse_points(e, x, y, 1.0, overlap, o);
    let mut ops = bezier_curve(&all, o);
    if !o.disable_multi_stroke && o.roughness != 0.0 {
        let (all, _) = ellipse_points(e, x, y, 1.5, 0.0, o);
        ops.extend(bezier_curve(&all, o));
    }
    (core, path_set(ops))
}

fn svg_path(segs: &[Seg], o: &Options) -> Set {
    let mut ops = vec![];
    let mut current = (0.0, 0.0);
    for seg in segs {
        match *seg {
            Seg::M(p) => current = p,
            Seg::L(p) => {
                ops.extend(double_line(current, p, o, false));
                current = p;
            }
            Seg::C(c1, c2, p) => {
                ops.extend(bezier_to(c1, c2, p, current, o));
                current = p;
            }
        }
    }
    path_set(ops)
}

fn solid_fill(polygons: &[Vec<P>], o: &Options) -> Set {
    let mut ops = vec![];
    for points in polygons.iter().filter(|p| p.len() > 2) {
        for (i, p) in points.iter().enumerate() {
            let at = (
                p.0 + o.offset_opt(MAX_RANDOMNESS_OFFSET, 1.0),
                p.1 + o.offset_opt(MAX_RANDOMNESS_OFFSET, 1.0),
            );
            ops.push(if i == 0 { Op::Move(at) } else { Op::Line(at) });
        }
    }
    Set {
        kind: Kind::FillPath,
        ops,
    }
}

/// Two wobbly strokes from `a` to `b`, or one when `o` says so (fills always draw two).
fn double_line(a: P, b: P, o: &Options, filling: bool) -> Vec<Op> {
    let mut ops = line_ops(a, b, o, false);
    if filling || !o.disable_multi_stroke {
        ops.extend(line_ops(a, b, o, true));
    }
    ops
}

fn line_ops((x1, y1): P, (x2, y2): P, o: &Options, overlay: bool) -> Vec<Op> {
    let length_sq = (x1 - x2).powi(2) + (y1 - y2).powi(2);
    let length = length_sq.sqrt();
    let gain = if length < 200.0 {
        1.0
    } else if length > 500.0 {
        0.4
    } else {
        -0.0016668 * length + 1.233334
    };
    let mut offset = MAX_RANDOMNESS_OFFSET;
    if offset * offset * 100.0 > length_sq {
        offset = length / 10.0;
    }
    let half = offset / 2.0;
    let diverge = 0.2 + o.random() * 0.2;
    let mid_x = MAX_RANDOMNESS_OFFSET * (y2 - y1) / 200.0;
    let mid_y = MAX_RANDOMNESS_OFFSET * (x1 - x2) / 200.0;
    let mid_x = o.offset_opt(mid_x, gain);
    let mid_y = o.offset_opt(mid_y, gain);
    let keep = o.preserve_vertices;
    let r = |size: f64| o.offset_opt(size, gain);
    let size = if overlay { half } else { offset };
    let start = (
        x1 + if keep { 0.0 } else { r(size) },
        y1 + if keep { 0.0 } else { r(size) },
    );
    let c1x = mid_x + x1 + (x2 - x1) * diverge + r(size);
    let c1y = mid_y + y1 + (y2 - y1) * diverge + r(size);
    let c2x = mid_x + x1 + 2.0 * (x2 - x1) * diverge + r(size);
    let c2y = mid_y + y1 + 2.0 * (y2 - y1) * diverge + r(size);
    let ex = x2 + if keep { 0.0 } else { r(size) };
    let ey = y2 + if keep { 0.0 } else { r(size) };
    vec![Op::Move(start), Op::Curve((c1x, c1y), (c2x, c2y), (ex, ey))]
}

fn curve_with_offset(points: &[P], offset: f64, o: &Options) -> Vec<Op> {
    let moved = |p: P| {
        (
            p.0 + o.offset_opt(offset, 1.0),
            p.1 + o.offset_opt(offset, 1.0),
        )
    };
    let mut ps = vec![moved(points[0]), moved(points[0])];
    for (i, &p) in points.iter().enumerate().skip(1) {
        ps.push(moved(p));
        if i == points.len() - 1 {
            ps.push(moved(p));
        }
    }
    bezier_curve(&ps, o)
}

/// rough.js's `_curve`: a Catmull-Rom spline through the points as cubic Béziers.
fn bezier_curve(points: &[P], o: &Options) -> Vec<Op> {
    let len = points.len();
    let mut ops = vec![];
    if len > 3 {
        ops.push(Op::Move(points[1]));
        for i in 1..len - 2 {
            let (a, p, b, c) = (points[i - 1], points[i], points[i + 1], points[i + 2]);
            ops.push(Op::Curve(
                (p.0 + (b.0 - a.0) / 6.0, p.1 + (b.1 - a.1) / 6.0),
                (b.0 + (p.0 - c.0) / 6.0, b.1 + (p.1 - c.1) / 6.0),
                b,
            ));
        }
    } else if len == 3 {
        ops.push(Op::Move(points[1]));
        ops.push(Op::Curve(points[1], points[2], points[2]));
    } else if len == 2 {
        ops.extend(double_line(points[0], points[1], o, false));
    }
    ops
}

fn ellipse_points(
    e: EllipseParams,
    cx: f64,
    cy: f64,
    offset: f64,
    overlap: f64,
    o: &Options,
) -> (Vec<P>, Vec<P>) {
    let (rx, ry) = (e.rx, e.ry);
    let mut core = vec![];
    let mut all = vec![];
    if o.roughness == 0.0 {
        let increment = e.increment / 4.0;
        all.push((cx + rx * (-increment).cos(), cy + ry * (-increment).sin()));
        let mut angle = 0.0;
        while angle <= PI * 2.0 {
            let p = (cx + rx * angle.cos(), cy + ry * angle.sin());
            core.push(p);
            all.push(p);
            angle += increment;
        }
        all.push((cx + rx, cy));
        all.push((cx + rx * increment.cos(), cy + ry * increment.sin()));
    } else {
        let r = |x: f64| o.offset_opt(x, 1.0);
        let rad = r(0.5) - PI / 2.0;
        let at = |scale: f64, angle: f64| {
            let x = r(offset) + cx + scale * rx * angle.cos();
            (x, r(offset) + cy + scale * ry * angle.sin())
        };
        all.push(at(0.9, rad - e.increment));
        let end = PI * 2.0 + rad - 0.01;
        let mut angle = rad;
        while angle < end {
            let p = at(1.0, angle);
            core.push(p);
            all.push(p);
            angle += e.increment;
        }
        all.push(at(1.0, rad + PI * 2.0 + overlap * 0.5));
        all.push(at(0.98, rad + overlap));
        all.push(at(0.9, rad + overlap * 0.5));
    }
    (all, core)
}

fn bezier_to(c1: P, c2: P, to: P, current: P, o: &Options) -> Vec<Op> {
    let ros = [MAX_RANDOMNESS_OFFSET, MAX_RANDOMNESS_OFFSET + 0.3];
    let keep = o.preserve_vertices;
    let r = |x: f64| o.offset_opt(x, 1.0);
    let mut ops = vec![];
    for (i, &ro) in ros
        .iter()
        .enumerate()
        .take(if o.disable_multi_stroke { 1 } else { 2 })
    {
        if i == 0 || keep {
            ops.push(Op::Move(current));
        } else {
            ops.push(Op::Move((current.0 + r(ros[0]), current.1 + r(ros[0]))));
        }
        let end = if keep {
            to
        } else {
            (to.0 + r(ro), to.1 + r(ro))
        };
        let a = (c1.0 + r(ro), c1.1 + r(ro));
        let b = (c2.0 + r(ro), c2.1 + r(ro));
        ops.push(Op::Curve(a, b, end));
    }
    ops
}

// The fillers.

fn pattern_fill(polygons: &[Vec<P>], o: &Options) -> Set {
    let ops = match o.fill_style {
        Fill::Zigzag => {
            let gap = gap(o);
            let mut o2 = o.clone();
            o2.hachure_gap = gap;
            let angle = PI / 180.0 * o.hachure_angle;
            let (dx, dy) = (gap * 0.5 * angle.cos(), gap * 0.5 * angle.sin());
            let mut lines = vec![];
            for (p1, p2) in hachure(polygons, &o2) {
                if (p1.0 - p2.0).hypot(p1.1 - p2.1) != 0.0 {
                    lines.push(((p1.0 - dx, p1.1 + dy), p2));
                    lines.push(((p1.0 + dx, p1.1 - dy), p2));
                }
            }
            fill_lines(&lines, o)
        }
        Fill::CrossHatch => {
            let mut ops = fill_lines(&hachure(polygons, o), o);
            let mut o2 = o.clone();
            o2.hachure_angle += 90.0;
            ops.extend(fill_lines(&hachure(polygons, &o2), &o2));
            ops
        }
        _ => fill_lines(&hachure(polygons, o), o),
    };
    Set {
        kind: Kind::FillSketch,
        ops,
    }
}

fn fill_lines(lines: &[(P, P)], o: &Options) -> Vec<Op> {
    (lines.iter())
        .flat_map(|&(a, b)| double_line(a, b, o, true))
        .collect()
}

fn gap(o: &Options) -> f64 {
    let gap = if o.hachure_gap < 0.0 {
        o.stroke_width * 4.0
    } else {
        o.hachure_gap
    };
    gap.max(0.1)
}

/// The hachure lines across the polygons: scan lines `gap` apart, turned by the angle.
fn hachure(polygons: &[Vec<P>], o: &Options) -> Vec<(P, P)> {
    let angle = o.hachure_angle + 90.0;
    let gap = gap(o);
    let mut step = 1.0;
    // The randomizer exists by now: the outline was drawn first.
    if o.roughness >= 1.0 && o.rng.get().is_some() && o.random() > 0.7 {
        step = gap;
    }
    let turn = |p: P, degrees: f64| {
        let (s, c) = (PI / 180.0 * degrees).sin_cos();
        (p.0 * c - p.1 * s, p.0 * s + p.1 * c)
    };
    let turned: Vec<Vec<P>> = (polygons.iter())
        .map(|poly| poly.iter().map(|&p| turn(p, angle)).collect())
        .collect();
    (scan_lines(&turned, gap, step).into_iter())
        .map(|(a, b)| (turn(a, -angle), turn(b, -angle)))
        .collect()
}

struct Edge {
    ymin: f64,
    ymax: f64,
    x: f64,
    islope: f64,
}

/// The most scan lines one fill may take, so a huge shape can't stall a render.
const SCAN_LIMIT: usize = 4_000_000;

fn scan_lines(polygons: &[Vec<P>], gap: f64, step: f64) -> Vec<(P, P)> {
    let mut edges = vec![];
    for poly in polygons {
        let mut v = poly.clone();
        if v.first() != v.last() {
            v.push(v[0]);
        }
        if v.len() <= 2 || v.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
            continue;
        }
        for w in v.windows(2) {
            let (p1, p2) = (w[0], w[1]);
            if p1.1 != p2.1 {
                let ymin = p1.1.min(p2.1);
                edges.push(Edge {
                    ymin,
                    ymax: p1.1.max(p2.1),
                    x: if ymin == p1.1 { p1.0 } else { p2.0 },
                    islope: (p2.0 - p1.0) / (p2.1 - p1.1),
                });
            }
        }
    }
    edges.sort_by(|a, b| {
        a.ymin
            .partial_cmp(&b.ymin)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
            .then(
                a.ymax
                    .partial_cmp(&b.ymax)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let mut lines = vec![];
    let Some(first) = edges.first() else {
        return lines;
    };
    let mut y = first.ymin;
    let mut edges = edges.into_iter().peekable();
    let mut active: Vec<Edge> = vec![];
    let mut iteration = 0.0;
    let mut scans = 0;
    while !active.is_empty() || edges.peek().is_some() {
        while let Some(e) = edges.next_if(|e| e.ymin <= y) {
            active.push(e);
        }
        active.retain(|e| e.ymax > y);
        active.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        if (step != 1.0 || iteration % gap == 0.0) && active.len() > 1 {
            for [a, b] in active.as_chunks::<2>().0 {
                lines.push(((js_round(a.x), y), (js_round(b.x), y)));
            }
        }
        y += step;
        for e in &mut active {
            e.x += step * e.islope;
        }
        iteration += 1.0;
        scans += 1;
        if scans > SCAN_LIMIT {
            break;
        }
    }
    lines
}

/// `Math.round`: halves go up.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

// points-on-curve and points-on-path.

fn lerp(a: P, b: P, t: f64) -> P {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

fn distance_sq(a: P, b: P) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

fn flatness(p: &[P]) -> f64 {
    let (p1, p2, p3, p4) = (p[0], p[1], p[2], p[3]);
    let ux = (3.0 * p2.0 - 2.0 * p1.0 - p4.0).powi(2);
    let uy = (3.0 * p2.1 - 2.0 * p1.1 - p4.1).powi(2);
    let vx = (3.0 * p3.0 - 2.0 * p4.0 - p1.0).powi(2);
    let vy = (3.0 * p3.1 - 2.0 * p4.1 - p1.1).powi(2);
    ux.max(vx) + uy.max(vy)
}

fn bezier_points(p: &[P], tolerance: f64, out: &mut Vec<P>) {
    if flatness(p) < tolerance {
        if out
            .last()
            .is_none_or(|&last| distance_sq(last, p[0]).sqrt() > 1.0)
        {
            out.push(p[0]);
        }
        out.push(p[3]);
    } else {
        let (q1, q2, q3) = (
            lerp(p[0], p[1], 0.5),
            lerp(p[1], p[2], 0.5),
            lerp(p[2], p[3], 0.5),
        );
        let (r1, r2) = (lerp(q1, q2, 0.5), lerp(q2, q3, 0.5));
        let red = lerp(r1, r2, 0.5);
        bezier_points(&[p[0], q1, r1, red], tolerance, out);
        bezier_points(&[red, r2, q3, p[3]], tolerance, out);
    }
}

fn points_on_bezier_curves(points: &[P], tolerance: f64, distance: f64) -> Vec<P> {
    let mut out = vec![];
    for i in 0..(points.len() - 1) / 3 {
        bezier_points(&points[i * 3..i * 3 + 4], tolerance, &mut out);
    }
    if distance > 0.0 {
        return simplify(&out, distance);
    }
    out
}

fn segment_distance_sq(p: P, v: P, w: P) -> f64 {
    let l2 = distance_sq(v, w);
    if l2 == 0.0 {
        return distance_sq(p, v);
    }
    let t = ((p.0 - v.0) * (w.0 - v.0) + (p.1 - v.1) * (w.1 - v.1)) / l2;
    distance_sq(p, lerp(v, w, t.clamp(0.0, 1.0)))
}

/// Ramer–Douglas–Peucker, as points-on-curve does it.
pub fn simplify(points: &[P], epsilon: f64) -> Vec<P> {
    fn go(points: &[P], start: usize, end: usize, epsilon: f64, out: &mut Vec<P>) {
        let (s, e) = (points[start], points[end - 1]);
        let (mut max, mut at) = (0.0, 1);
        for (i, &p) in points.iter().enumerate().take(end - 1).skip(start + 1) {
            let d = segment_distance_sq(p, s, e);
            if d > max {
                max = d;
                at = i;
            }
        }
        if max.sqrt() > epsilon {
            go(points, start, at + 1, epsilon, out);
            go(points, at, end, epsilon, out);
        } else {
            if out.is_empty() {
                out.push(s);
            }
            out.push(e);
        }
    }
    let mut out = vec![];
    if !points.is_empty() {
        go(points, 0, points.len(), epsilon, &mut out);
    }
    out
}

fn curve_to_bezier(points: &[P]) -> Vec<P> {
    if points.len() == 3 {
        return vec![points[0], points[1], points[2], points[2]];
    }
    let mut ps = vec![points[0], points[0]];
    ps.extend_from_slice(&points[1..]);
    ps.push(points[points.len() - 1]);
    let mut out = vec![ps[0]];
    for i in 1..ps.len() - 2 {
        let (a, p, b, c) = (ps[i - 1], ps[i], ps[i + 1], ps[i + 2]);
        out.push((p.0 + (b.0 - a.0) / 6.0, p.1 + (b.1 - a.1) / 6.0));
        out.push((b.0 + (p.0 - c.0) / 6.0, b.1 + (p.1 - c.1) / 6.0));
        out.push(b);
    }
    out
}

fn points_on_path(segs: &[Seg], tolerance: f64, distance: f64) -> Vec<Vec<P>> {
    let mut sets = vec![];
    let mut current: Vec<P> = vec![];
    let mut start = (0.0, 0.0);
    let mut pending: Vec<P> = vec![];
    let flush_curve = |pending: &mut Vec<P>, current: &mut Vec<P>| {
        if pending.len() >= 4 {
            current.extend(points_on_bezier_curves(pending, tolerance, 0.0));
        }
        pending.clear();
    };
    for seg in segs {
        match *seg {
            Seg::M(p) => {
                flush_curve(&mut pending, &mut current);
                if !current.is_empty() {
                    sets.push(std::mem::take(&mut current));
                }
                start = p;
                current.push(p);
            }
            Seg::L(p) => {
                flush_curve(&mut pending, &mut current);
                current.push(p);
            }
            Seg::C(a, b, p) => {
                if pending.is_empty() {
                    pending.push(current.last().copied().unwrap_or(start));
                }
                pending.extend([a, b, p]);
            }
        }
    }
    flush_curve(&mut pending, &mut current);
    if !current.is_empty() {
        sets.push(current);
    }
    (sets.iter())
        .map(|set| simplify(set, distance))
        .filter(|s| !s.is_empty())
        .collect()
}

// SVG.

/// The ops as path data, numbers to 2 decimals as Excalidraw exports them.
pub fn ops_to_path(ops: &[Op]) -> String {
    let mut d = String::new();
    for op in ops {
        let _ = match *op {
            Op::Move(p) => write!(d, "M{} {} ", n(p.0), n(p.1)),
            Op::Curve(a, b, p) => write!(
                d,
                "C{} {}, {} {}, {} {} ",
                n(a.0),
                n(a.1),
                n(b.0),
                n(b.1),
                n(p.0),
                n(p.1)
            ),
            Op::Line(p) => write!(d, "L{} {} ", n(p.0), n(p.1)),
        };
    }
    d.truncate(d.trim_end().len());
    d
}

/// The drawable as rough.js's SVG renderer writes it: one `<g>` (with `attrs`) of paths.
pub fn svg(out: &mut String, d: &Drawable, attrs: &str) {
    let o = &d.o;
    let _ = write!(out, "<g{attrs}>");
    for set in &d.sets {
        let data = ops_to_path(&set.ops);
        let _ = match set.kind {
            Kind::Path => {
                let dash = o.dash.map_or_else(String::new, |(a, b)| {
                    format!(r#" stroke-dasharray="{} {}""#, n(a), n(b))
                });
                write!(
                    out,
                    r#"<path d="{data}" stroke="{}" stroke-width="{}" fill="none"{dash}/>"#,
                    escape(&o.stroke),
                    n(o.stroke_width)
                )
            }
            Kind::FillPath => {
                let rule = if d.evenodd {
                    r#" fill-rule="evenodd""#
                } else {
                    ""
                };
                write!(
                    out,
                    r#"<path d="{data}" stroke="none" stroke-width="0" fill="{}"{rule}/>"#,
                    escape(o.fill.as_deref().unwrap_or_default())
                )
            }
            Kind::FillSketch => {
                let weight = if o.fill_weight < 0.0 {
                    o.stroke_width / 2.0
                } else {
                    o.fill_weight
                };
                write!(
                    out,
                    r#"<path d="{data}" stroke="{}" stroke-width="{}" fill="none"/>"#,
                    escape(o.fill.as_deref().unwrap_or_default()),
                    n(weight)
                )
            }
        };
    }
    out.push_str("</g>\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seeded_generator_matches_rough_js() {
        // new Random(1).next() three times in rough.js 4.6.4.
        let o = Options {
            seed: 1,
            ..Options::default()
        };
        let got: Vec<f64> = (0..3).map(|_| o.random()).collect();
        assert_eq!(
            got,
            [
                48271.0 / 2_147_483_648.0,
                f64::from(48271_i32.wrapping_mul(48271) & 0x7fff_ffff) / 2_147_483_648.0,
                f64::from(48271_i32.wrapping_mul(48271).wrapping_mul(48271) & 0x7fff_ffff)
                    / 2_147_483_648.0
            ]
        );
    }
}
