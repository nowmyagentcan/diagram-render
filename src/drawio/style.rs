//! draw.io styles: `name;key=value;…` laid over the default vertex or edge style. The default
//! and named styles are the values of draw.io's `styles/default.xml` (Apache-2.0,
//! https://github.com/jgraph/drawio), read as mxGraph's `mxStylesheet.getCellStyle` reads them.

use std::collections::HashMap;

type Entries = &'static [(&'static str, &'static str)];

const VERTEX: Entries = &[
    ("shape", "label"),
    ("perimeter", "rectanglePerimeter"),
    ("fontSize", "12"),
    ("fontFamily", "Helvetica"),
    ("align", "center"),
    ("verticalAlign", "middle"),
    ("fillColor", "default"),
    ("strokeColor", "default"),
    ("fontColor", "default"),
];

const EDGE: Entries = &[
    ("shape", "connector"),
    ("labelBackgroundColor", "default"),
    ("endArrow", "classic"),
    ("fontSize", "11"),
    ("fontFamily", "Helvetica"),
    ("align", "center"),
    ("verticalAlign", "middle"),
    ("rounded", "1"),
    ("strokeColor", "default"),
    ("fontColor", "default"),
];

const TEXT: Entries = &[
    ("fillColor", "none"),
    ("gradientColor", "none"),
    ("strokeColor", "none"),
    ("align", "left"),
    ("verticalAlign", "top"),
];

const LABEL: Entries = &[
    ("fontStyle", "1"),
    ("align", "left"),
    ("verticalAlign", "middle"),
    ("spacing", "2"),
    ("spacingLeft", "52"),
    ("imageWidth", "42"),
    ("imageHeight", "42"),
    ("rounded", "1"),
];

/// The named styles: each a list of the styles it extends, then its own entries.
fn named(name: &str) -> &'static [Entries] {
    match name {
        "text" => &[TEXT],
        "edgeLabel" => &[
            TEXT,
            &[("labelBackgroundColor", "default"), ("fontSize", "11")],
        ],
        "label" => &[LABEL],
        "icon" => &[
            LABEL,
            &[
                ("align", "center"),
                ("imageAlign", "center"),
                ("verticalLabelPosition", "bottom"),
                ("verticalAlign", "top"),
                ("labelBackgroundColor", "default"),
                ("spacing", "0"),
                ("spacingLeft", "0"),
                ("spacingTop", "6"),
                ("fontStyle", "0"),
                ("imageWidth", "48"),
                ("imageHeight", "48"),
            ],
        ],
        "swimlane" => &[&[
            ("shape", "swimlane"),
            ("fontSize", "12"),
            ("fontStyle", "1"),
            ("startSize", "23"),
        ]],
        "group" => &[&[
            ("verticalAlign", "top"),
            ("fillColor", "none"),
            ("strokeColor", "none"),
            ("gradientColor", "none"),
        ]],
        "ellipse" => &[&[("shape", "ellipse"), ("perimeter", "ellipsePerimeter")]],
        "rhombus" => &[&[("shape", "rhombus"), ("perimeter", "rhombusPerimeter")]],
        "triangle" => &[&[("shape", "triangle"), ("perimeter", "trianglePerimeter")]],
        "line" => &[&[
            ("shape", "line"),
            ("strokeWidth", "4"),
            ("labelBackgroundColor", "default"),
            ("verticalAlign", "top"),
            ("spacingTop", "8"),
        ]],
        "image" => &[IMAGE],
        "roundImage" => &[IMAGE, &[("perimeter", "ellipsePerimeter")]],
        "rhombusImage" => &[IMAGE, &[("perimeter", "rhombusPerimeter")]],
        _ => &[],
    }
}

const IMAGE: Entries = &[
    ("shape", "image"),
    ("labelBackgroundColor", "default"),
    ("verticalAlign", "top"),
    ("verticalLabelPosition", "bottom"),
];

/// A cell's style with every default and named style applied; a key set to `none` is unset.
#[derive(Clone, Default)]
pub struct Style(HashMap<String, String>);

impl Style {
    pub fn parse(style: &str, edge: bool) -> Self {
        let mut map = HashMap::new();
        // A style starting with `;` doesn't take the defaults.
        if !style.starts_with(';') {
            let base = if edge { EDGE } else { VERTEX };
            map.extend(base.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        }
        for part in style.split(';') {
            match part.split_once('=') {
                Some((key, "none")) => {
                    map.remove(key);
                }
                Some((key, value)) => {
                    map.insert(key.to_string(), value.to_string());
                }
                None => {
                    for entries in named(part) {
                        map.extend(entries.iter().map(|(k, v)| (k.to_string(), v.to_string())));
                    }
                }
            }
        }
        Self(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// A number as JavaScript's `parseFloat` reads it, else `default`.
    pub fn num(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(lead_number).unwrap_or(default)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.into(), value.into());
    }

    pub fn unset(&mut self, key: &str) {
        self.0.remove(key);
    }
}

/// The number a value starts with, as `parseFloat` reads it.
fn lead_number(v: &str) -> Option<f64> {
    let v = v.trim_start();
    let end = (v.char_indices())
        .take_while(|&(i, c)| {
            c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+')) || c == 'e'
        })
        .map(|(i, c)| i + c.len_utf8())
        .last()?;
    (1..=end)
        .rev()
        .find_map(|e| v[..e].parse::<f64>().ok())
        .filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_styles_and_keys_lay_over_the_defaults() {
        let s = Style::parse(
            "ellipse;whiteSpace=wrap;fillColor=#dae8fc;strokeColor=none",
            false,
        );
        assert_eq!(s.get("shape"), Some("ellipse"));
        assert_eq!(s.get("perimeter"), Some("ellipsePerimeter"));
        assert_eq!(s.get("fillColor"), Some("#dae8fc"));
        assert_eq!(s.get("strokeColor"), None);
        assert_eq!(s.get("fontSize"), Some("12"));
        let e = Style::parse("edgeStyle=orthogonalEdgeStyle;endArrow=none", true);
        assert_eq!((e.get("endArrow"), e.get("rounded")), (None, Some("1")));
        assert_eq!(Style::parse(";fontSize=9", false).get("shape"), None);
    }

    #[test]
    fn numbers_read_as_parse_float_reads_them() {
        let s = Style::parse("a=12px;b=.5;c=-3e2x;d=x;e=1", false);
        assert_eq!(s.num("a", 0.0), 12.0);
        assert_eq!(s.num("b", 0.0), 0.5);
        assert_eq!(s.num("c", 0.0), -300.0);
        assert_eq!(s.num("d", 7.0), 7.0);
    }
}
