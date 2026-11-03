//! draw.io's vertex shapes, with their geometry written from mxGraph's shape classes and
//! draw.io's `Shapes.js` (Apache-2.0, https://github.com/jgraph/drawio v32.6.9), and the fill,
//! stroke, opacity, shadow, gradient, rotation and flips their styles ask for.

use std::fmt::Write;

use super::read::P;
use super::style::Style;
use super::{Rect, State, color};
use crate::svg::{escape, num as n};

/// mxGraph's default corner radius for lines and polygons, and its rounding factor for boxes.
const LINE_ARCSIZE: f64 = 20.0;
const ROUNDING: f64 = 15.0;

/// An SVG path being written.
#[derive(Default)]
pub struct Path(pub String);

impl Path {
    pub fn m(&mut self, (x, y): P) {
        let _ = write!(self.0, "M{} {} ", n(x), n(y));
    }

    pub fn l(&mut self, (x, y): P) {
        let _ = write!(self.0, "L{} {} ", n(x), n(y));
    }

    pub fn q(&mut self, (cx, cy): P, (x, y): P) {
        let _ = write!(self.0, "Q{} {} {} {} ", n(cx), n(cy), n(x), n(y));
    }

    pub fn c(&mut self, (ax, ay): P, (bx, by): P, (x, y): P) {
        let _ = write!(
            self.0,
            "C{} {} {} {} {} {} ",
            n(ax),
            n(ay),
            n(bx),
            n(by),
            n(x),
            n(y)
        );
    }

    /// An elliptical arc as SVG's `A`.
    fn a(&mut self, (rx, ry): P, sweep: bool, (x, y): P) {
        let _ = write!(
            self.0,
            "A{} {} 0 0 {} {} {} ",
            n(rx),
            n(ry),
            u8::from(sweep),
            n(x),
            n(y)
        );
    }

    pub fn z(&mut self) {
        self.0.push_str("Z ");
    }

    /// mxShape's `addPoints`: lines through `pts`, each corner cut by up to `arc` and joined by a
    /// quadratic curve when `rounded`.
    pub fn points(&mut self, pts: &[P], rounded: bool, arc: f64, close: bool) {
        let Some(&last) = pts.last() else {
            return;
        };
        let mut pts = pts.to_vec();
        if close && rounded {
            pts.insert(0, ((last.0 + pts[0].0) / 2.0, (last.1 + pts[0].1) / 2.0));
        }
        let mut pt = pts[0];
        self.m(pt);
        let len = pts.len();
        let end = if close { len } else { len - 1 };
        let mut i = 1;
        while i < end {
            let mut tmp = pts[i % len];
            let (dx, dy) = (pt.0 - tmp.0, pt.1 - tmp.1);
            if rounded && (dx != 0.0 || dy != 0.0) {
                let dist = dx.hypot(dy);
                let cut = arc.min(dist / 2.0) / dist;
                self.l((tmp.0 + dx * cut, tmp.1 + dy * cut));
                let mut next = pts[(i + 1) % len];
                while i < len - 2
                    && (next.0 - tmp.0).round() == 0.0
                    && (next.1 - tmp.1).round() == 0.0
                {
                    next = pts[(i + 2) % len];
                    i += 1;
                }
                let (dx, dy) = (next.0 - tmp.0, next.1 - tmp.1);
                let dist = dx.hypot(dy).max(1.0);
                let cut = arc.min(dist / 2.0) / dist;
                let to = (tmp.0 + dx * cut, tmp.1 + dy * cut);
                self.q(tmp, to);
                tmp = to;
            } else {
                self.l(tmp);
            }
            pt = tmp;
            i += 1;
        }
        if close {
            self.z();
        } else {
            self.l(last);
        }
    }
}

/// Something a vertex couldn't be drawn as asked.
pub enum Note {
    /// Drawn as a plain box: the shape's name.
    Approximated(String),
    /// A remote image left as a link, or as a placeholder in a PNG.
    Remote(String),
}

/// What drawing needs besides the vertex: definitions to add to, and whether the SVG becomes a
/// PNG (which can't load remote images).
pub struct Context<'a> {
    pub defs: &'a mut Defs,
    pub png: bool,
}

/// Gradients and shadow filters, each made once.
#[derive(Default)]
pub struct Defs {
    pub svg: String,
    count: usize,
    shadow: Option<String>,
}

impl Defs {
    fn gradient(&mut self, from: &str, to: &str, direction: &str) -> String {
        self.count += 1;
        let id = format!("g{}", self.count);
        let (x1, y1, x2, y2) = match direction {
            "north" => (0, 1, 0, 0),
            "east" => (0, 0, 1, 0),
            "west" => (1, 0, 0, 0),
            _ => (0, 0, 0, 1),
        };
        let stops = format!(
            r#"<stop offset="0%" stop-color="{from}"/><stop offset="100%" stop-color="{to}"/>"#
        );
        let _ = match direction {
            "radial" => writeln!(
                self.svg,
                r#"<radialGradient id="{id}">{stops}</radialGradient>"#
            ),
            _ => writeln!(
                self.svg,
                r#"<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{stops}</linearGradient>"#
            ),
        };
        format!("url(#{id})")
    }

    /// draw.io's drop shadow: 2 px right, 3 px down, blurred 2 px, black at 25 %.
    fn shadow(&mut self) -> String {
        if self.shadow.is_none() {
            self.svg += "<filter id=\"shadow\" x=\"-10%\" y=\"-10%\" width=\"130%\" height=\"130%\"><feDropShadow dx=\"2\" dy=\"3\" stdDeviation=\"1\" flood-color=\"#000000\" flood-opacity=\"0.25\"/></filter>\n";
            self.shadow = Some("url(#shadow)".into());
        }
        self.shadow.clone().unwrap_or_default()
    }
}

/// A colour, its `default` the light theme's `light`, absent `none`.
pub fn paint(value: Option<&str>, light: &str) -> String {
    match value {
        None => "none".into(),
        Some("default") => light.into(),
        Some(v) => color(v),
    }
}

/// The stroke attributes of a style: colour, width and dashes.
pub fn stroke(st: &Style, width_attr: bool) -> String {
    let stroke = paint(st.get("strokeColor"), "#000000");
    let w = st.num("strokeWidth", 1.0);
    let mut attrs = format!(r#" stroke="{stroke}""#);
    if width_attr && w != 1.0 {
        let _ = write!(attrs, r#" stroke-width="{}""#, n(w));
    }
    if st.on("dashed") {
        let pattern = st.get("dashPattern").unwrap_or("3 3");
        let scale = if st.on("fixDash") { 1.0 } else { w };
        let dashes: Vec<String> = (pattern.split([' ', ',']))
            .filter_map(|d| d.parse::<f64>().ok())
            .map(|d| n(d * scale))
            .collect();
        if !dashes.is_empty() {
            let _ = write!(attrs, r#" stroke-dasharray="{}""#, dashes.join(" "));
        }
    }
    match st.num("strokeOpacity", 100.0) {
        100.0 => {}
        o => {
            let _ = write!(attrs, r#" stroke-opacity="{}""#, n(o / 100.0));
        }
    }
    attrs
}

/// The shape a style names, in its outline polygon when it has one: for edges to end on.
pub fn polygon(st: &Style, r: Rect) -> Option<Vec<P>> {
    let shape = st.get("shape").unwrap_or("label");
    let local = turned(st, r);
    let (w, h) = (local.w, local.h);
    let size = |rel: f64, fixed: f64, max: f64| match st.get("fixedSize").is_some_and(|v| v != "0")
    {
        true => st.num("size", fixed).clamp(0.0, max),
        false => w * st.num("size", rel).clamp(0.0, max / w.max(1e-9)),
    };
    let pts: Vec<P> = match shape {
        "rhombus" => vec![(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)],
        "triangle" => vec![(0.0, 0.0), (w, h / 2.0), (0.0, h)],
        "hexagon" => {
            let s = size(0.25, 20.0, w * 0.5);
            vec![
                (s, 0.0),
                (w - s, 0.0),
                (w, h / 2.0),
                (w - s, h),
                (s, h),
                (0.0, h / 2.0),
            ]
        }
        "parallelogram" => {
            let dx = size(0.2, 20.0, w);
            vec![(0.0, h), (dx, 0.0), (w, 0.0), (w - dx, h)]
        }
        "trapezoid" => {
            let dx = size(0.2, 20.0, w * 0.5);
            vec![(0.0, h), (dx, 0.0), (w - dx, 0.0), (w, h)]
        }
        "step" => {
            let s = size(0.2, 20.0, w);
            vec![
                (0.0, 0.0),
                (w - s, 0.0),
                (w, h / 2.0),
                (w - s, h),
                (0.0, h),
                (s, h / 2.0),
            ]
        }
        "card" => {
            let s = st.num("size", 30.0).clamp(0.0, w.min(h));
            vec![(s, 0.0), (w, 0.0), (w, h), (0.0, h), (0.0, s)]
        }
        "note" => {
            let s = st.num("size", 30.0).clamp(0.0, w.min(h));
            vec![(0.0, 0.0), (w - s, 0.0), (w, s), (w, h), (0.0, h)]
        }
        _ => return None,
    };
    Some(pts.into_iter().map(|p| place(st, r, local, p)).collect())
}

/// The box a shape is painted in before its direction turns it: north and south swap the sides.
fn turned(st: &Style, r: Rect) -> Rect {
    match st.get("direction") {
        Some("north" | "south") => Rect {
            x: r.cx() - r.h / 2.0,
            y: r.cy() - r.w / 2.0,
            w: r.h,
            h: r.w,
        },
        _ => r,
    }
}

/// The angle a direction turns a shape by.
fn direction_angle(st: &Style) -> f64 {
    match st.get("direction") {
        Some("north") => 270.0,
        Some("west") => 180.0,
        Some("south") => 90.0,
        _ => 0.0,
    }
}

/// Whether the shape is flipped across and down; a turned shape swaps the two.
fn flips(st: &Style) -> (bool, bool) {
    let (h, v) = (st.on("flipH"), st.on("flipV"));
    match st.get("direction") {
        Some("north" | "south") => (v, h),
        _ => (h, v),
    }
}

/// A point of the painted box `local` (from its corner) on the page: turned by the shape's
/// direction and flipped, as the drawing's transform does.
fn place(st: &Style, r: Rect, local: Rect, (x, y): P) -> P {
    let (cx, cy) = (r.cx(), r.cy());
    let (mut px, mut py) = (local.x + x - cx, local.y + y - cy);
    let a = direction_angle(st).to_radians();
    (px, py) = (px * a.cos() - py * a.sin(), px * a.sin() + py * a.cos());
    let (fh, fv) = flips(st);
    if fh {
        px = -px;
    }
    if fv {
        py = -py;
    }
    (cx + px, cy + py)
}

/// The transform for a shape's direction, rotation and flips, around its centre.
fn transform(st: &Style, r: Rect) -> String {
    let (cx, cy) = (n(r.cx()), n(r.cy()));
    let (fh, fv) = flips(st);
    let mut angle = st.num("rotation", 0.0) + direction_angle(st);
    let mut t = String::new();
    if fh || fv {
        let (sx, sy) = (if fh { -1 } else { 1 }, if fv { -1 } else { 1 });
        let _ = write!(
            t,
            "translate({cx} {cy}) scale({sx} {sy}) translate(-{cx} -{cy})"
        );
        if fh != fv {
            angle = -angle;
        }
    }
    if angle % 360.0 != 0.0 {
        let _ = write!(t, " rotate({} {cx} {cy})", n(angle));
    }
    t.trim().to_string()
}

/// The box a vertex covers on the page, turned with its rotation.
pub fn extent(s: &State) -> Rect {
    let r = s.rect;
    let a = (s.style.num("rotation", 0.0) + direction_angle(&s.style)).to_radians();
    if a.sin().abs() < 1e-9 && s.style.get("direction").is_none() {
        return r;
    }
    let local = turned(&s.style, r);
    let (hw, hh) = (local.w / 2.0, local.h / 2.0);
    let (w, h) = (
        hw * a.cos().abs() + hh * a.sin().abs(),
        hw * a.sin().abs() + hh * a.cos().abs(),
    );
    Rect {
        x: r.cx() - w,
        y: r.cy() - h,
        w: 2.0 * w,
        h: 2.0 * h,
    }
}

/// Draws the vertex; what couldn't be drawn as asked.
pub fn draw(out: &mut String, s: &State, cx: &mut Context) -> Option<Note> {
    let st = &s.style;
    let shape = st.get("shape").unwrap_or("label");
    let mut note = None;
    let fill_color = paint(st.get("fillColor"), "#ffffff");
    let fill = match st.get("gradientColor").filter(|_| fill_color != "none") {
        Some(to) => {
            let to = paint(Some(to), "#ffffff");
            let direction = st.get("gradientDirection").unwrap_or("south");
            cx.defs.gradient(&fill_color, &to, direction)
        }
        None => fill_color,
    };
    let fill_opacity = match st.num("fillOpacity", 100.0) {
        100.0 => String::new(),
        o => format!(r#" fill-opacity="{}""#, n(o / 100.0)),
    };
    let lines = stroke(st, true);
    let paint_attrs = format!(r#"fill="{fill}"{fill_opacity}{lines}"#);
    let line_only = format!(r#"fill="none"{lines}"#);
    let r = s.rect;
    let local = turned(st, r);
    let (x, y, w, h) = (local.x, local.y, local.w, local.h);
    let rounded = st.on("rounded");
    let arc = st.num("arcSize", LINE_ARCSIZE) / 2.0;
    let mut body = String::new();
    let polygon_path = |pts: &[P]| {
        let mut p = Path::default();
        let pts: Vec<P> = pts.iter().map(|&(px, py)| (x + px, y + py)).collect();
        p.points(&pts, rounded, arc, true);
        p.0
    };
    let path = |body: &mut String, d: &str, attrs: &str| {
        let _ = write!(body, r#"<path d="{}" {attrs}/>"#, d.trim_end());
    };
    match shape {
        "label" | "rectangle" | "rect" | "process" | "internalStorage" => {
            let radius = if rounded { box_arc(st, w, h) } else { 0.0 };
            let unseen = fill == "none" && lines.starts_with(r#" stroke="none""#);
            if !unseen {
                let _ = write!(
                    body,
                    r#"<rect x="{}" y="{}" width="{}" height="{}"{} {paint_attrs}/>"#,
                    n(x),
                    n(y),
                    n(w),
                    n(h),
                    if radius > 0.0 {
                        format!(r#" rx="{}""#, n(radius))
                    } else {
                        String::new()
                    }
                );
            }
            let mut p = Path::default();
            if shape == "process" {
                let mut inset = match st.get("fixedSize").is_some_and(|v| v != "0") {
                    true => st.num("size", 0.1).clamp(0.0, w),
                    false => w * st.num("size", 0.1).clamp(0.0, 1.0),
                };
                if rounded {
                    let f = st.num("arcSize", ROUNDING) / 100.0;
                    inset = inset.max((w * f).min(h * f));
                }
                let inset = inset.round();
                for ix in [x + inset, x + w - inset] {
                    p.m((ix, y));
                    p.l((ix, y + h));
                }
            } else if shape == "internalStorage" {
                let mut inset = 0.0_f64;
                if rounded {
                    let f = st.num("arcSize", ROUNDING) / 100.0;
                    inset = (w * f).min(h * f);
                }
                let dx = st.num("dx", 20.0).clamp(0.0, w).max(inset);
                let dy = st.num("dy", 20.0).clamp(0.0, h).max(inset);
                p.m((x, y + dy));
                p.l((x + w, y + dy));
                p.m((x + dx, y));
                p.l((x + dx, y + h));
            }
            if !p.0.is_empty() {
                path(&mut body, &p.0, &line_only);
            }
            if let Some(image) = st.get("image").filter(|_| shape == "label") {
                let iw = st.num("imageWidth", 24.0);
                let ih = st.num("imageHeight", 24.0);
                let spacing = st.num("spacing", 2.0) + 5.0;
                let ix = match st.get("imageAlign") {
                    Some("center") => x + (w - iw) / 2.0,
                    Some("right") => x + w - iw - spacing,
                    _ => x + spacing,
                };
                let iy = match st.get("imageVerticalAlign") {
                    Some("top") => y + spacing,
                    Some("bottom") => y + h - ih - spacing,
                    _ => y + (h - ih) / 2.0,
                };
                let b = Rect {
                    x: ix,
                    y: iy,
                    w: iw,
                    h: ih,
                };
                note = picture(&mut body, image, b, st, cx);
            }
        }
        "ellipse" | "doubleEllipse" => {
            let e = |body: &mut String, b: Rect, attrs: &str| {
                let _ = write!(
                    body,
                    r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" {attrs}/>"#,
                    n(b.cx()),
                    n(b.cy()),
                    n(b.w / 2.0),
                    n(b.h / 2.0)
                );
            };
            e(&mut body, local, &paint_attrs);
            if shape == "doubleEllipse" {
                let sw = st.num("strokeWidth", 1.0);
                let m = st.num("margin", (3.0 + sw).min((w / 5.0).min(h / 5.0)));
                let inner = Rect {
                    x: x + m,
                    y: y + m,
                    w: w - 2.0 * m,
                    h: h - 2.0 * m,
                };
                if inner.w > 0.0 && inner.h > 0.0 {
                    e(&mut body, inner, &line_only);
                }
            }
        }
        "rhombus" | "triangle" | "hexagon" | "parallelogram" | "trapezoid" | "step" | "card"
        | "note" => {
            let pts = polygon(
                &Style::default_for(shape, st),
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                },
            );
            let pts = pts.unwrap_or_default();
            // `note` keeps its corners square, and folds one over.
            let d = match shape {
                "note" => {
                    let mut p = Path::default();
                    p.points(
                        &pts.iter()
                            .map(|&(px, py)| (x + px, y + py))
                            .collect::<Vec<_>>(),
                        false,
                        0.0,
                        true,
                    );
                    p.0
                }
                _ => polygon_path(&pts),
            };
            path(&mut body, &d, &paint_attrs);
            if shape == "note" {
                let s = st.num("size", 30.0).clamp(0.0, w.min(h));
                let mut p = Path::default();
                p.m((x + w - s, y));
                p.l((x + w - s, y + s));
                p.l((x + w, y + s));
                path(&mut body, &p.0, &line_only);
            }
        }
        "cylinder" => {
            let dy = match st.get("size") {
                Some(_) => h * st.num("size", 0.0).clamp(0.0, 1.0),
                None => 40f64.min((h / 5.0).round()),
            };
            let mut p = Path::default();
            p.m((x, y + dy));
            p.c((x, y - dy / 3.0), (x + w, y - dy / 3.0), (x + w, y + dy));
            p.l((x + w, y + h - dy));
            p.c(
                (x + w, y + h + dy / 3.0),
                (x, y + h + dy / 3.0),
                (x, y + h - dy),
            );
            p.z();
            path(&mut body, &p.0, &paint_attrs);
            let mut lid = Path::default();
            lid.m((x, y + dy));
            lid.c((x, y + 2.0 * dy), (x + w, y + 2.0 * dy), (x + w, y + dy));
            path(&mut body, &lid.0, &line_only);
        }
        "cylinder3" => {
            let s = st.num("size", 15.0).clamp(0.0, h * 0.5);
            let lid = st.not_off("lid");
            let mut p = Path::default();
            let radii = (w * 0.5, s);
            if lid {
                p.m((x, y + s));
                p.a(radii, true, (x + w * 0.5, y));
                p.a(radii, true, (x + w, y + s));
            } else {
                p.m((x, y));
                p.a(radii, false, (x + w * 0.5, y + s));
                p.a(radii, false, (x + w, y));
            }
            p.l((x + w, y + h - s));
            p.a(radii, true, (x + w * 0.5, y + h));
            p.a(radii, true, (x, y + h - s));
            p.z();
            path(&mut body, &p.0, &paint_attrs);
            if lid && s > 0.0 {
                let mut top = Path::default();
                top.m((x + w, y + s));
                top.a(radii, true, (x + w * 0.5, y + 2.0 * s));
                top.a(radii, true, (x, y + s));
                path(&mut body, &top.0, &line_only);
            }
        }
        "document" => {
            let dy = h * st.num("size", 0.3).clamp(0.0, 1.0);
            let fy = 1.4;
            let mut p = Path::default();
            p.m((x, y));
            p.l((x + w, y));
            p.l((x + w, y + h - dy / 2.0));
            p.q(
                (x + w * 3.0 / 4.0, y + h - dy * fy),
                (x + w / 2.0, y + h - dy / 2.0),
            );
            p.q(
                (x + w / 4.0, y + h - dy * (1.0 - fy)),
                (x, y + h - dy / 2.0),
            );
            p.l((x, y + dy / 2.0));
            p.z();
            path(&mut body, &p.0, &paint_attrs);
        }
        "cloud" => {
            let at = |fx: f64, fy: f64| (x + fx * w, y + fy * h);
            let mut p = Path::default();
            p.m(at(0.25, 0.25));
            p.c(at(0.05, 0.25), at(0.0, 0.5), at(0.16, 0.55));
            p.c(at(0.0, 0.66), at(0.18, 0.9), at(0.31, 0.8));
            p.c(at(0.4, 1.0), at(0.7, 1.0), at(0.8, 0.8));
            p.c(at(1.0, 0.8), at(1.0, 0.6), at(0.875, 0.5));
            p.c(at(1.0, 0.3), at(0.8, 0.1), at(0.625, 0.2));
            p.c(at(0.5, 0.05), at(0.3, 0.05), at(0.25, 0.25));
            p.z();
            path(&mut body, &p.0, &paint_attrs);
        }
        "actor" => {
            let third = w / 3.0;
            let mut p = Path::default();
            p.m((x, y + h));
            p.c(
                (x, y + 3.0 * h / 5.0),
                (x, y + 2.0 * h / 5.0),
                (x + w / 2.0, y + 2.0 * h / 5.0),
            );
            p.c(
                (x + w / 2.0 - third, y + 2.0 * h / 5.0),
                (x + w / 2.0 - third, y),
                (x + w / 2.0, y),
            );
            p.c(
                (x + w / 2.0 + third, y),
                (x + w / 2.0 + third, y + 2.0 * h / 5.0),
                (x + w / 2.0, y + 2.0 * h / 5.0),
            );
            p.c(
                (x + w, y + 2.0 * h / 5.0),
                (x + w, y + 3.0 * h / 5.0),
                (x + w, y + h),
            );
            p.z();
            path(&mut body, &p.0, &paint_attrs);
        }
        "umlActor" => {
            let _ = write!(
                body,
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" {paint_attrs}/>"#,
                n(x + w / 2.0),
                n(y + h / 8.0),
                n(w / 4.0),
                n(h / 8.0)
            );
            let mut p = Path::default();
            p.m((x + w / 2.0, y + h / 4.0));
            p.l((x + w / 2.0, y + 2.0 * h / 3.0));
            p.m((x, y + h / 3.0));
            p.l((x + w, y + h / 3.0));
            p.m((x + w / 2.0, y + 2.0 * h / 3.0));
            p.l((x, y + h));
            p.m((x + w / 2.0, y + 2.0 * h / 3.0));
            p.l((x + w, y + h));
            path(&mut body, &p.0, &line_only);
        }
        "swimlane" | "table" => swimlane(&mut body, st, local, &paint_attrs, &line_only),
        "tableRow" => {}
        "partialRectangle" => {
            if fill != "none" {
                let _ = write!(
                    body,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}"{fill_opacity} stroke="none"/>"#,
                    n(x),
                    n(y),
                    n(w),
                    n(h)
                );
            }
            let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)];
            let mut p = Path::default();
            p.m(corners[0]);
            for (side, &to) in ["top", "right", "bottom", "left"].iter().zip(&corners[1..]) {
                match st.not_off(side) {
                    true => p.l(to),
                    false => p.m(to),
                }
            }
            if p.0.contains('L') {
                path(
                    &mut body,
                    &p.0,
                    &format!(r#"{line_only} stroke-linecap="square""#),
                );
            }
        }
        "line" => {
            let mut p = Path::default();
            p.m((x, y + h / 2.0));
            p.l((x + w, y + h / 2.0));
            path(&mut body, &p.0, &line_only);
        }
        "image" => {
            if let Some(image) = st.get("image") {
                note = picture(&mut body, image, local, st, cx);
            }
            if paint(st.get("imageBorder"), "#000000") != "none" {
                let border = paint(st.get("imageBorder"), "#000000");
                let _ = write!(
                    body,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{border}"/>"#,
                    n(x),
                    n(y),
                    n(w),
                    n(h)
                );
            }
        }
        other => {
            // Vendor libraries and stencils: a box with the label, its outline dashed.
            let stroke = paint(st.get("strokeColor"), "#000000");
            let stroke = if stroke == "none" {
                "#000000".to_string()
            } else {
                stroke
            };
            let fill = if fill == "none" {
                "#ffffff".to_string()
            } else {
                fill.clone()
            };
            let _ = write!(
                body,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}" stroke="{stroke}" stroke-dasharray="4 4"/>"#,
                n(x),
                n(y),
                n(w),
                n(h)
            );
            let name = match other.starts_with("stencil(") {
                true => "stencil",
                false => other,
            };
            note = Some(Note::Approximated(name.to_string()));
        }
    }
    if body.is_empty() {
        return note;
    }
    let mut group = String::new();
    let t = transform(st, r);
    if !t.is_empty() {
        let _ = write!(group, r#" transform="{t}""#);
    }
    if let Some(o) = Some(st.num("opacity", 100.0)).filter(|o| *o != 100.0) {
        let _ = write!(group, r#" opacity="{}""#, n(o / 100.0));
    }
    if st.on("shadow") {
        let _ = write!(group, r#" filter="{}""#, cx.defs.shadow());
    }
    let link = st
        .get("link")
        .map(|l| format!(r#"<a xlink:href="{}">"#, escape(l)));
    let _ = writeln!(
        out,
        "{}<g{group}>{body}</g>{}",
        link.as_deref().unwrap_or_default(),
        if link.is_some() { "</a>" } else { "" }
    );
    note
}

impl Style {
    /// The style with `shape` set: for drawing a polygon in its own painted box.
    fn default_for(shape: &str, st: &Style) -> Style {
        let mut s = st.clone();
        s.set("shape", shape);
        s.unset("direction");
        s.unset("flipH");
        s.unset("flipV");
        s
    }
}

/// The corner radius of a rounded box.
fn box_arc(st: &Style, w: f64, h: f64) -> f64 {
    match st.on("absoluteArcSize") {
        true => (w / 2.0)
            .min(h / 2.0)
            .min(st.num("arcSize", LINE_ARCSIZE) / 2.0),
        false => {
            let f = st.num("arcSize", ROUNDING) / 100.0;
            (w * f).min(h * f)
        }
    }
}

/// A swimlane or table: a header of `startSize` and a body below (or beside it when not
/// `horizontal`), the body filled with `swimlaneFillColor`.
fn swimlane(body: &mut String, st: &Style, r: Rect, paint_attrs: &str, line_only: &str) {
    let table = st.get("shape") == Some("table");
    let horizontal = st.not_off("horizontal");
    let side = if horizontal { r.h } else { r.w };
    let start = st
        .num("startSize", if table { 0.0 } else { 40.0 })
        .clamp(0.0, side);
    let (x, y, w, h) = (r.x, r.y, r.w, r.h);
    let lane = paint(st.get("swimlaneFillColor"), "#ffffff");
    let lane_attrs = line_only.replacen(r#"fill="none""#, &format!(r#"fill="{lane}""#), 1);
    if start == 0.0 {
        let _ = write!(
            body,
            r#"<rect x="{}" y="{}" width="{}" height="{}" {}/>"#,
            n(x),
            n(y),
            n(w),
            n(h),
            if table { paint_attrs } else { &lane_attrs }
        );
        return;
    }
    let mut head = Path::default();
    let mut rest = Path::default();
    match horizontal {
        true => {
            head.m((x, y + start));
            head.l((x, y));
            head.l((x + w, y));
            head.l((x + w, y + start));
            rest.m((x, y + start));
            rest.l((x, y + h));
            rest.l((x + w, y + h));
            rest.l((x + w, y + start));
        }
        false => {
            head.m((x + start, y));
            head.l((x, y));
            head.l((x, y + h));
            head.l((x + start, y + h));
            rest.m((x + start, y));
            rest.l((x + w, y));
            rest.l((x + w, y + h));
            rest.l((x + start, y + h));
        }
    }
    let _ = write!(body, r#"<path d="{}Z" {paint_attrs}/>"#, head.0);
    if start < side {
        let _ = write!(body, r#"<path d="{}" {lane_attrs}/>"#, rest.0.trim_end());
    }
}

/// An image cell's picture: drawn from a data URL; a remote one kept as a link in an SVG and drawn
/// as a grey box in a PNG.
fn picture(body: &mut String, image: &str, b: Rect, st: &Style, cx: &mut Context) -> Option<Note> {
    let image = image.trim();
    let remote = image.starts_with("http://") || image.starts_with("https://");
    let href = match image.split_once(',') {
        // draw.io leaves `;base64` out of data URLs, as `;` ends a style value.
        Some((head, data)) if head.starts_with("data:") && !head.contains(";base64") => {
            let looks_base64 = !data.contains(['<', ' ', '%']);
            match looks_base64 {
                true => format!("{head};base64,{data}"),
                false => image.to_string(),
            }
        }
        _ => image.to_string(),
    };
    if remote && cx.png {
        let _ = write!(
            body,
            r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#e0e0e0" stroke="#9e9e9e" stroke-dasharray="4 4"/>"##,
            n(b.x),
            n(b.y),
            n(b.w),
            n(b.h)
        );
        return Some(Note::Remote(image.to_string()));
    }
    let aspect = match st.not_off("imageAspect") {
        true => "xMidYMid meet",
        false => "none",
    };
    let _ = write!(
        body,
        r#"<image x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="{aspect}" xlink:href="{}"/>"#,
        n(b.x),
        n(b.y),
        n(b.w),
        n(b.h),
        escape(&href)
    );
    remote.then(|| Note::Remote(image.to_string()))
}

/// Grid lines between a table's rows and between the cells of its rows.
pub fn table_lines(out: &mut String, st: &Style, lines: &[(P, P)]) {
    if lines.is_empty() {
        return;
    }
    let mut p = Path::default();
    for &(a, b) in lines {
        p.m(a);
        p.l(b);
    }
    let _ = writeln!(
        out,
        r#"<path d="{}" fill="none"{}/>"#,
        p.0.trim_end(),
        stroke(st, true)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_points_cut_each_corner() {
        let mut p = Path::default();
        p.points(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)],
            true,
            10.0,
            false,
        );
        assert_eq!(p.0, "M0 0 L90 0 Q100 0 100 10 L100 100 ");
        let mut p = Path::default();
        p.points(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false, 10.0, true);
        assert_eq!(p.0, "M0 0 L10 0 L10 10 Z ");
    }

    #[test]
    fn turned_shapes_put_their_points_where_the_transform_does() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
        };
        let east = polygon(&Style::parse("triangle", false), r).unwrap();
        assert_eq!(east, vec![(0.0, 0.0), (100.0, 25.0), (0.0, 50.0)]);
        // Pointing up: the apex at the top middle.
        let north = polygon(&Style::parse("triangle;direction=north", false), r).unwrap();
        let apex = north[1];
        assert!(
            (apex.0 - 50.0).abs() < 1e-9 && apex.1.abs() < 1e-9,
            "{north:?}"
        );
    }
}
