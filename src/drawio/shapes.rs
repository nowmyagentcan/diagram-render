//! draw.io's vertex shapes.

use std::fmt::Write;

use super::{State, color};
use crate::svg::num as n;

/// Draws the vertex; the name of a shape drawn as a plain box instead of its own.
pub fn draw(out: &mut String, s: &State) -> Option<String> {
    let st = &s.style;
    let fill = paint(st.get("fillColor"), "#ffffff");
    let stroke = paint(st.get("strokeColor"), "#000000");
    if fill == "none" && stroke == "none" {
        return None;
    }
    let r = s.rect;
    let _ = writeln!(
        out,
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}" stroke="{stroke}"/>"#,
        n(r.x),
        n(r.y),
        n(r.w),
        n(r.h)
    );
    None
}

/// A colour, its `default` the light theme's `light`, absent `none`.
pub fn paint(value: Option<&str>, light: &str) -> String {
    match value {
        None => "none".into(),
        Some("default") => light.into(),
        Some(v) => color(v),
    }
}
