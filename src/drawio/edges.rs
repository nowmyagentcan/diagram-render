//! draw.io's edges: where they end on their terminals and the route between.

use std::fmt::Write;

use super::State;
use super::read::{Geo, P};
use super::shapes::paint;
use super::style::Style;
use crate::svg::num as n;

/// Where an edge ends: on a vertex, or at a point.
pub enum End<'a> {
    Cell(&'a State),
    Point(P),
}

impl End<'_> {
    fn center(&self) -> P {
        match self {
            End::Cell(s) => (s.rect.cx(), s.rect.cy()),
            End::Point(p) => *p,
        }
    }
}

/// The points from the source's end to the target's.
pub fn route(_style: &Style, source: End, target: End, points: &[P]) -> Vec<P> {
    let mut route = vec![source.center()];
    route.extend_from_slice(points);
    route.push(target.center());
    route
}

pub fn draw(out: &mut String, style: &Style, points: &[P]) {
    let stroke = paint(style.get("strokeColor"), "#000000");
    let d: Vec<String> = points
        .iter()
        .map(|&(x, y)| format!("{},{}", n(x), n(y)))
        .collect();
    let _ = writeln!(
        out,
        r#"<path d="M{}" fill="none" stroke="{stroke}"/>"#,
        d.join(" ")
    );
}

/// The point a label with `geo` sits at along the route.
pub fn along(points: &[P], _geo: &Geo) -> P {
    let (a, b) = (points[0], points[points.len() - 1]);
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}
