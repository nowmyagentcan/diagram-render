//! draw.io labels.

use std::fmt::Write;

use super::Rect;
use super::style::Style;
use crate::raster::Widths;
use crate::svg::{escape, num as n};

/// Draws `text` for a cell at `at`; the box the text fills.
pub fn draw(
    out: &mut String,
    widths: &Widths,
    text: &str,
    style: &Style,
    at: Rect,
    _edge: bool,
) -> Option<Rect> {
    let size = style.num("fontSize", 12.0);
    let lines: Vec<&str> = text.split('\n').collect();
    let height = lines.len() as f64 * size * 1.2;
    let width = lines.iter().map(|l| widths.of(l, size)).fold(0.0, f64::max);
    let top = at.cy() - height / 2.0;
    let _ = write!(
        out,
        r#"<text font-family="Helvetica, Arial, Liberation Sans, sans-serif" font-size="{}" text-anchor="middle">"#,
        n(size)
    );
    for (i, line) in lines.iter().enumerate() {
        let y = top + (i as f64 + 0.79) * size * 1.2;
        let _ = write!(
            out,
            r#"<tspan x="{}" y="{}">{}</tspan>"#,
            n(at.cx()),
            n(y),
            escape(line)
        );
    }
    out.push_str("</text>\n");
    Some(Rect {
        x: at.cx() - width / 2.0,
        y: top,
        w: width,
        h: height,
    })
}
