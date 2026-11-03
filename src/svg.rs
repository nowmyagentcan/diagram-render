//! The SVGs the engines write.

use crate::detect::xml_root;

/// The SVG's size in CSS px, rounded up: from the root's `width` and `height` (a `pt` is 4/3
/// px), else from its `viewBox`.
pub fn size(svg: &str) -> (u32, u32) {
    let attrs = xml_root(svg).map(|(_, attrs)| attrs).unwrap_or_default();
    let attr = |key: &str| attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let view: Vec<f64> = (attr("viewBox").unwrap_or_default())
        .split([' ', ','])
        .filter_map(|n| n.parse().ok())
        .collect();
    let side = |key: &str, i: usize| {
        let length = attr(key).and_then(|v| match v.trim().strip_suffix("pt") {
            Some(pt) => pt.parse::<f64>().ok().map(|n| n * 4.0 / 3.0),
            None => v.trim().trim_end_matches("px").parse().ok(),
        });
        let length = length.or_else(|| view.get(i).copied()).unwrap_or(0.0);
        // Rounding up a value a float multiply left a hair above a whole number isn't wanted.
        (length - 1e-6).ceil().max(0.0) as u32
    };
    (side("width", 2), side("height", 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_come_from_width_and_height_else_the_view_box() {
        let dot = "<?xml version=\"1.0\"?>\n<!DOCTYPE svg>\n<!-- x -->\n<svg width=\"62pt\" height=\"75pt\"\n viewBox=\"0.00 0.00 62.00 116.00\">";
        assert_eq!(size(dot), (83, 100));
        let mermaid = r#"<svg id="merman" width="100%" viewBox="4.5 4 187 357.2">"#;
        assert_eq!(size(mermaid), (187, 358));
        assert_eq!(size(r#"<svg width="255" height="434px">"#), (255, 434));
        assert_eq!(size("<svg>"), (0, 0));
    }
}
