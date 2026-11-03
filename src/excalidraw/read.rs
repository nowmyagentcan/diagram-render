//! Excalidraw scenes: the JSON of a `.excalidraw` file, or the one a `.excalidraw.png` or
//! `.excalidraw.svg` carries, read into elements with the defaults Excalidraw 0.18.1's
//! `restoreElements` (`data/restore.ts`) gives missing properties.

use std::collections::HashMap;

use base64::Engine as _;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use super::rough::{Fill, P};
use crate::error::{Error, Warning};
use crate::input::{self, Lines};

/// The most elements one scene may have, and the most image data its files may hold.
const LIMIT: usize = 100_000;
const IMAGE_DATA: usize = 64 << 20;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Rectangle,
    Diamond,
    Ellipse,
    Line,
    Arrow,
    Freedraw,
    Text,
    Image,
    Frame,
    MagicFrame,
    Embeddable,
    Iframe,
}

impl Kind {
    fn parse(t: &str) -> Option<Self> {
        Some(match t {
            "rectangle" => Self::Rectangle,
            "diamond" => Self::Diamond,
            "ellipse" => Self::Ellipse,
            // Excalidraw reads its old `draw` elements as lines.
            "line" | "draw" => Self::Line,
            "arrow" => Self::Arrow,
            "freedraw" => Self::Freedraw,
            "text" => Self::Text,
            "image" => Self::Image,
            "frame" => Self::Frame,
            "magicframe" => Self::MagicFrame,
            "embeddable" => Self::Embeddable,
            "iframe" => Self::Iframe,
            _ => return None,
        })
    }

    pub fn linear(self) -> bool {
        matches!(self, Self::Line | Self::Arrow)
    }

    pub fn frame(self) -> bool {
        matches!(self, Self::Frame | Self::MagicFrame)
    }

    pub fn iframe_like(self) -> bool {
        matches!(self, Self::Embeddable | Self::Iframe)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stroke {
    Solid,
    Dashed,
    Dotted,
}

/// How round the corners are: Excalidraw's `roundness.type` (1 legacy, 2 proportional,
/// 3 adaptive) and its `value`.
#[derive(Clone, Copy, Debug)]
pub struct Roundness {
    pub kind: i64,
    pub value: Option<f64>,
}

/// Part of an image's natural pixels drawn.
#[derive(Clone, Copy, Debug)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub natural_width: f64,
    pub natural_height: f64,
}

#[derive(Clone, Debug)]
pub struct Element {
    pub id: String,
    pub kind: Kind,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub angle: f64,
    pub stroke_color: String,
    pub background_color: String,
    pub fill_style: Fill,
    pub stroke_width: f64,
    pub stroke_style: Stroke,
    pub roughness: f64,
    pub opacity: f64,
    pub seed: i32,
    pub roundness: Option<Roundness>,
    pub frame_id: Option<String>,
    pub link: Option<String>,
    pub points: Vec<P>,
    pub start_arrowhead: Option<String>,
    pub end_arrowhead: Option<String>,
    pub elbowed: bool,
    pub pressures: Vec<f64>,
    pub simulate_pressure: bool,
    pub text: String,
    pub font_size: f64,
    pub font_family: i64,
    pub text_align: String,
    pub line_height: f64,
    pub container_id: Option<String>,
    pub file_id: Option<String>,
    pub scale: (f64, f64),
    pub crop: Option<Crop>,
    pub name: Option<String>,
    /// The line of its `"id"` in the source.
    pub line: usize,
}

/// A scene: its elements (deleted ones left out), background and image files.
pub struct Scene {
    pub elements: Vec<Element>,
    pub background: String,
    /// File id → data URL.
    pub files: HashMap<String, String>,
    pub warnings: Vec<Warning>,
}

/// The scene in `lines`: a scene's JSON, the wrapper a PNG carries, or an SVG with the scene in
/// its metadata.
pub fn scene(lines: &Lines, name: &str) -> Result<Scene, Error> {
    let text = lines.text();
    if text.trim_start().starts_with('<') {
        let json = from_svg(text)?;
        return read(
            &carried(&json, "SVG", name)?,
            name,
            " in the scene the SVG carries",
        );
    }
    // A PNG's chunk holds the wrapper; a scene's own file is the scene.
    if let Ok(wrapper) = serde_json::from_str::<Value>(text)
        && wrapper.get("encoded").is_some()
        && wrapper.get("type").is_none()
    {
        let inner = unwrap(&wrapper, name, "PNG")?;
        return read(&inner, name, " in the scene the PNG carries");
    }
    read(text, name, "")
}

/// The JSON in an SVG's metadata, between `payload-start` and `payload-end`.
fn from_svg(svg: &str) -> Result<String, Error> {
    let missing =
        || Error::invalid_scene("the SVG holds no scene between payload-start and payload-end");
    let start = svg.find("<!-- payload-start -->").ok_or_else(missing)?;
    let rest = &svg[start + "<!-- payload-start -->".len()..];
    let end = rest.find("<!-- payload-end -->").ok_or_else(missing)?;
    let base64: String = rest[..end].split_whitespace().collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .map_err(|e| {
            Error::invalid_scene(&format!("the scene the SVG carries isn't base64 ({e})"))
        })?;
    // From version 2 on, the payload is a byte string; version 1 is UTF-8.
    let version = (svg.split_once("<!-- payload-version:"))
        .and_then(|(_, v)| v.split_once(" -->"))
        .map_or("1", |(v, _)| v.trim());
    Ok(match version {
        "1" => String::from_utf8_lossy(&bytes).into_owned(),
        _ => bytes.iter().map(|&b| char::from(b)).collect(),
    })
}

/// The scene JSON a carried payload holds: the scene itself, or Excalidraw's wrapper.
fn carried(json: &str, what: &str, name: &str) -> Result<String, Error> {
    let lines = Lines::new(json);
    let value = parse(json, &lines, &format!(" in the scene the {what} carries"))?;
    match value.get("encoded") {
        Some(_) => unwrap(&value, name, what),
        None => Ok(json.to_string()),
    }
}

/// `{"encoding": "bstring", "compressed": …, "encoded": …}` unpacked to the scene's text.
fn unwrap(wrapper: &Value, name: &str, what: &str) -> Result<String, Error> {
    let bad = |why: &str| Error::invalid_scene(&format!("the scene the {what} carries {why}"));
    if wrapper["encoding"] != "bstring" {
        return Err(bad("has an unknown encoding"));
    }
    let encoded = wrapper["encoded"]
        .as_str()
        .ok_or_else(|| bad("has no data"))?;
    let bytes: Option<Vec<u8>> = encoded.chars().map(|c| u8::try_from(c).ok()).collect();
    let bytes = bytes.ok_or_else(|| bad("isn't a byte string"))?;
    if wrapper["compressed"] != true {
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    match input::inflate(&bytes, true) {
        Ok(Some(text)) => Ok(String::from_utf8_lossy(&text).into_owned()),
        Ok(None) => Err(Error::too_large_unpacked(name)),
        Err(e) => Err(bad(&format!("can't be unpacked ({e})"))),
    }
}

/// The scene in `text`; a syntax error's message ends with `suffix`. Lines are counted as
/// everywhere else, whatever the line ends.
fn read(text: &str, name: &str, suffix: &str) -> Result<Scene, Error> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines = Lines::new(&text);
    let value = parse(&text, &lines, suffix)?;
    from_value(value, &text, &lines, name)
}

/// The JSON, or `syntax_error` at serde_json's line and (character) column.
fn parse(text: &str, lines: &Lines, suffix: &str) -> Result<Value, Error> {
    serde_json::from_str(text).map_err(|e| {
        let (line, bytes) = (e.line().max(1), e.column());
        let said = e.to_string();
        let said = said.split(" at line ").next().unwrap_or_default();
        let row = lines.line(line);
        let mut cut = bytes.min(row.len());
        while !row.is_char_boundary(cut) {
            cut -= 1;
        }
        let column = row[..cut].chars().count().max(1);
        lines.place(
            Error::syntax_error(&format!("{said}{suffix}"), line, None),
            line,
            Some(column),
        )
    })
}

fn from_value(value: Value, text: &str, lines: &Lines, name: &str) -> Result<Scene, Error> {
    let Value::Object(top) = value else {
        return Err(Error::invalid_scene("the file isn't a JSON object"));
    };
    match top.get("type").and_then(Value::as_str) {
        Some("excalidraw") => {}
        Some(other) => {
            let reason = format!("its \"type\" is \"{other}\", not \"excalidraw\"");
            return Err(Error::invalid_scene(&reason));
        }
        None => return Err(Error::invalid_scene("it has no \"type\": \"excalidraw\"")),
    }
    let Some(Value::Array(all)) = top.get("elements") else {
        return Err(Error::invalid_scene("it has no \"elements\" list"));
    };
    if all.len() > LIMIT {
        return Err(Error::too_many_elements(all.len(), LIMIT));
    }
    let at_lines = element_lines(text, lines);
    let line = |i: usize| at_lines.get(i).copied().unwrap_or(1);
    let mut warnings = vec![];
    let mut elements = vec![];
    for (i, v) in all.iter().enumerate() {
        let Value::Object(e) = v else {
            let at = format!("#{}", i + 1);
            let reason = "it isn't a JSON object";
            warnings.push(Warning::skipped(
                "element_skipped",
                "element",
                &at,
                reason,
                line(i),
            ));
            continue;
        };
        if e.get("isDeleted") == Some(&Value::Bool(true)) {
            continue;
        }
        match element(e) {
            Ok(Some(mut el)) => {
                el.line = line(i);
                elements.push(el);
            }
            Ok(None) => {}
            Err(reason) => {
                let id = e.get("id").and_then(Value::as_str);
                let shown = id.map_or_else(|| format!("#{}", i + 1), str::to_string);
                warnings.push(Warning::skipped(
                    "element_skipped",
                    "element",
                    &shown,
                    &reason,
                    line(i),
                ));
            }
        }
    }
    let app = top.get("appState");
    let background = (app.and_then(|a| a.get("viewBackgroundColor")))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .unwrap_or("#ffffff")
        .to_string();
    let mut files = HashMap::new();
    let mut size = 0;
    if let Some(Value::Object(all)) = top.get("files") {
        for (id, f) in all {
            if let Some(url) = f.get("dataURL").and_then(Value::as_str) {
                size += url.len();
                files.insert(id.clone(), url.to_string());
            }
        }
    }
    if size > IMAGE_DATA {
        return Err(Error::too_much_image_data(name, &input::mib(size as u64)));
    }
    Ok(Scene {
        elements,
        background,
        files,
        warnings,
    })
}

/// The line of each element's `"id"` (else of its start), found in the elements' own text.
fn element_lines(text: &str, lines: &Lines) -> Vec<usize> {
    let top: HashMap<String, &RawValue> = serde_json::from_str(text).unwrap_or_default();
    let all: Vec<&RawValue> = (top.get("elements"))
        .and_then(|r| serde_json::from_str(r.get()).ok())
        .unwrap_or_default();
    (all.iter())
        .map(|r| {
            let s = r.get();
            let start = s.as_ptr() as usize - text.as_ptr() as usize;
            lines.locate(start + s.find("\"id\"").unwrap_or(0)).0
        })
        .collect()
}

fn num(e: &Map<String, Value>, key: &str) -> Option<f64> {
    e.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn string(e: &Map<String, Value>, key: &str) -> Option<String> {
    e.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(Into::into)
}

/// JavaScript's ToInt32, as `Math.imul` reads a seed.
fn int32(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    (v.trunc().rem_euclid(4_294_967_296.0) as u32) as i32
}

fn points(e: &Map<String, Value>) -> Option<Vec<P>> {
    let all = e.get("points")?.as_array()?;
    all.iter()
        .map(|p| {
            let p = p.as_array()?;
            let (x, y) = (p.first()?.as_f64()?, p.get(1)?.as_f64()?);
            (x.is_finite() && y.is_finite()).then_some((x, y))
        })
        .collect()
}

/// An element with Excalidraw's defaults, `None` for one Excalidraw drops (empty text), or why
/// it can't be drawn.
fn element(e: &Map<String, Value>) -> Result<Option<Element>, String> {
    let kind = match e.get("type").and_then(Value::as_str) {
        None => return Err("it has no type".into()),
        Some(t) => {
            Kind::parse(t).ok_or_else(|| format!("its type \"{t}\" isn't one Excalidraw draws"))?
        }
    };
    let mut at = [0.0; 4];
    for (slot, key) in at.iter_mut().zip(["x", "y", "width", "height"]) {
        *slot = num(e, key).ok_or_else(|| format!("it has no {key}"))?;
    }
    let [mut x, mut y, mut width, mut height] = at;
    if width < 0.0 {
        width = -width;
        x -= width;
    }
    if height < 0.0 {
        height = -height;
        y -= height;
    }
    let fill_style = match e.get("fillStyle").and_then(Value::as_str) {
        Some("hachure") => Fill::Hachure,
        Some("cross-hatch") => Fill::CrossHatch,
        Some("zigzag") => Fill::Zigzag,
        Some("solid") | None | Some("") => Fill::Solid,
        // rough.js draws a fill style it doesn't know as hachure.
        Some(_) => Fill::Hachure,
    };
    let stroke_style = match e.get("strokeStyle").and_then(Value::as_str) {
        Some("dashed") => Stroke::Dashed,
        Some("dotted") => Stroke::Dotted,
        _ => Stroke::Solid,
    };
    let adaptive = matches!(
        kind,
        Kind::Rectangle | Kind::Embeddable | Kind::Iframe | Kind::Image
    );
    let roundness = match e.get("roundness") {
        Some(Value::Object(r)) => Some(Roundness {
            kind: r.get("type").and_then(Value::as_i64).unwrap_or(0),
            value: num(r, "value"),
        }),
        _ if e.get("strokeSharpness").and_then(Value::as_str) == Some("round") => Some(Roundness {
            kind: if adaptive { 1 } else { 2 },
            value: None,
        }),
        _ => None,
    };
    // Excalidraw drops what can't be seen.
    let few_points = points(e).is_some_and(|p| p.len() < 2);
    if match kind {
        Kind::Line | Kind::Arrow | Kind::Freedraw => few_points,
        _ => width == 0.0 && height == 0.0,
    } {
        return Ok(None);
    }
    let mut el = Element {
        id: string(e, "id").unwrap_or_default(),
        kind,
        x,
        y,
        width,
        height,
        angle: num(e, "angle").unwrap_or(0.0),
        stroke_color: string(e, "strokeColor").unwrap_or_else(|| "#1e1e1e".into()),
        background_color: string(e, "backgroundColor").unwrap_or_else(|| "transparent".into()),
        fill_style,
        stroke_width: num(e, "strokeWidth").filter(|&w| w != 0.0).unwrap_or(2.0),
        stroke_style,
        roughness: num(e, "roughness").unwrap_or(1.0),
        opacity: num(e, "opacity").unwrap_or(100.0),
        seed: num(e, "seed").map_or(1, int32),
        roundness,
        frame_id: string(e, "frameId"),
        link: string(e, "link"),
        points: vec![],
        start_arrowhead: None,
        end_arrowhead: None,
        elbowed: false,
        pressures: vec![],
        simulate_pressure: false,
        text: String::new(),
        font_size: 20.0,
        font_family: 5,
        text_align: "left".into(),
        line_height: 1.25,
        container_id: None,
        file_id: None,
        scale: (1.0, 1.0),
        crop: None,
        name: None,
        line: 1,
    };
    let head = |key: &str, default: Option<&str>| match e.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => None,
        None => default.map(Into::into),
    };
    match kind {
        Kind::Line | Kind::Arrow => {
            let mut pts = points(e).unwrap_or_else(|| vec![(0.0, 0.0), (width, height)]);
            // Points start at the element's position.
            let (dx, dy) = pts[0];
            if (dx, dy) != (0.0, 0.0) {
                el.x += dx;
                el.y += dy;
                for p in &mut pts {
                    *p = (p.0 - dx, p.1 - dy);
                }
            }
            let (xs, ys) = (pts.iter().map(|p| p.0), pts.iter().map(|p| p.1));
            el.width = xs.clone().fold(f64::MIN, f64::max) - xs.fold(f64::MAX, f64::min);
            el.height = ys.clone().fold(f64::MIN, f64::max) - ys.fold(f64::MAX, f64::min);
            el.points = pts;
            let end = if kind == Kind::Arrow {
                Some("arrow")
            } else {
                None
            };
            el.start_arrowhead = head("startArrowhead", None);
            el.end_arrowhead = head("endArrowhead", end);
            el.elbowed = kind == Kind::Arrow && e.get("elbowed") == Some(&Value::Bool(true));
        }
        Kind::Freedraw => {
            el.points = points(e).unwrap_or_default();
            el.pressures = (e.get("pressures").and_then(Value::as_array))
                .map(|p| p.iter().filter_map(Value::as_f64).collect())
                .unwrap_or_default();
            el.simulate_pressure = e.get("simulatePressure") == Some(&Value::Bool(true));
        }
        Kind::Text => {
            el.text = e
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();
            if el.text.is_empty() {
                return Ok(None);
            }
            el.font_size = num(e, "fontSize").unwrap_or(20.0);
            el.font_family = e.get("fontFamily").and_then(Value::as_i64).unwrap_or(5);
            el.text_align = string(e, "textAlign").unwrap_or_else(|| "left".into());
            let lines = el
                .text
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .split('\n')
                .count() as f64;
            el.line_height =
                num(e, "lineHeight")
                    .filter(|&h| h != 0.0)
                    .unwrap_or(match el.height {
                        0.0 => super::text::metrics(el.font_family).3,
                        h => h / lines / el.font_size,
                    });
            el.container_id = string(e, "containerId");
        }
        Kind::Image => {
            el.file_id = string(e, "fileId");
            if let Some(Value::Array(s)) = e.get("scale")
                && let [Some(sx), Some(sy)] =
                    [s.first(), s.get(1)].map(|v| v.and_then(Value::as_f64))
            {
                el.scale = (sx, sy);
            }
            if let Some(Value::Object(c)) = e.get("crop") {
                let get = |k| num(c, k).unwrap_or(0.0);
                el.crop = Some(Crop {
                    x: get("x"),
                    y: get("y"),
                    width: get("width"),
                    height: get("height"),
                    natural_width: get("naturalWidth"),
                    natural_height: get("naturalHeight"),
                })
                .filter(|c| {
                    c.natural_width > 0.0
                        && c.natural_height > 0.0
                        && c.width > 0.0
                        && c.height > 0.0
                });
            }
        }
        Kind::Frame | Kind::MagicFrame => {
            el.name = e.get("name").and_then(Value::as_str).map(Into::into);
        }
        _ => {}
    }
    Ok(Some(el))
}
