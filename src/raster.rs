//! PNG from SVG with resvg. Text in the families the bundled Liberation fonts stand in for is
//! always drawn with them, so those PNGs are the same on every computer; other families and
//! missing characters come from the computer's fonts, which are loaded only when needed.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::fontdb::{self, Database, Family, ID};
use resvg::usvg::{self, FontFamily, FontResolver, FontStretch, FontStyle, ImageHrefResolver};
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

use crate::engine;
use crate::error::{Error, Warning};

pub const SANS: &str = "Liberation Sans";
pub const SERIF: &str = "Liberation Serif";
const MONO: &str = "Liberation Mono";

/// The font files in `fonts/` beside the executable.
const FILES: [&str; 8] = [
    "LiberationSans-Regular.ttf",
    "LiberationSans-Bold.ttf",
    "LiberationSans-Italic.ttf",
    "LiberationSans-BoldItalic.ttf",
    "LiberationSerif-Regular.ttf",
    "LiberationSerif-Bold.ttf",
    "LiberationMono-Regular.ttf",
    "LiberationMono-Bold.ttf",
];

/// The bundled font files, each checked to be there.
pub fn fonts() -> Result<Vec<PathBuf>, Error> {
    let dir = engine::home().join("fonts");
    let files: Vec<_> = FILES.iter().map(|f| dir.join(f)).collect();
    match files.iter().find(|f| !f.is_file()) {
        Some(gone) => Err(Error::fonts_missing(&gone.display().to_string())),
        None => Ok(files),
    }
}

/// The most pixels a PNG may have on a side and in all.
const SIDE: u64 = 16384;
const AREA: u64 = 64_000_000;

/// The scale to draw a `width` × `height` CSS-pixel diagram at: `scale`, else the largest
/// scale with 2 decimals whose PNG fits the limits, with a warning; an error below 0.05.
pub fn fit(width: f64, height: f64, scale: f64) -> Result<(f64, Option<Warning>), Error> {
    let size = |s: f64| (pixels(width, s), pixels(height, s));
    let fits = |s: f64| {
        let (w, h) = size(s);
        u64::from(w) <= SIDE && u64::from(h) <= SIDE && u64::from(w) * u64::from(h) <= AREA
    };
    if fits(scale) {
        return Ok((scale, None));
    }
    let (side, area) = (SIDE as f64, AREA as f64);
    let most = (side / width)
        .min(side / height)
        .min((area / (width * height)).sqrt());
    let mut hundredths = (most * 100.0).floor();
    while hundredths >= 5.0 && !fits(hundredths / 100.0) {
        hundredths -= 1.0;
    }
    if hundredths < 5.0 {
        return Err(Error::image_too_large(
            pixels(width, 1.0),
            pixels(height, 1.0),
        ));
    }
    let (w, h) = size(scale);
    let reduced = hundredths / 100.0;
    Ok((reduced, Some(Warning::scale_reduced(w, h, reduced))))
}

/// A PNG and its size in pixels.
pub struct Png {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// `bytes` with the size its header gives, when they are a PNG.
pub fn sized(bytes: Vec<u8>) -> Option<Png> {
    let header = bytes.get(..24)?;
    if !header.starts_with(b"\x89PNG\r\n\x1a\n") || &header[12..16] != b"IHDR" {
        return None;
    }
    let side = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
    let (width, height) = (side(16), side(20));
    Some(Png {
        bytes,
        width,
        height,
    })
}

/// What the font choices found: families drawn with another font, and whether the computer's
/// fonts are loaded.
#[derive(Default)]
struct Found {
    substituted: Vec<(String, &'static str)>,
    system: bool,
}

/// Up to 5 characters of `text` that Liberation Sans doesn't have, each once, in order.
pub fn uncovered(text: &str) -> Result<Vec<char>, Error> {
    let mut missing = vec![];
    if text.is_ascii() {
        return Ok(missing);
    }
    let file = &fonts()?[0];
    let unreadable = |e: &dyn std::fmt::Display| {
        Error::internal(&format!("can't read the font {}: {e}", file.display()))
    };
    let data = fs::read(file).map_err(|e| unreadable(&e))?;
    let charmap = FontRef::new(&data).map_err(|e| unreadable(&e))?.charmap();
    for c in text.chars() {
        if !c.is_ascii() && !c.is_control() && charmap.map(c).is_none() && !missing.contains(&c) {
            missing.push(c);
            if missing.len() == 5 {
                break;
            }
        }
    }
    Ok(missing)
}

/// Text widths from Liberation Sans, which has Arial's.
pub struct Widths(Vec<u8>);

impl Widths {
    pub fn load() -> Result<Self, Error> {
        let file = &fonts()?[0];
        let unreadable = |e: &dyn std::fmt::Display| {
            Error::internal(&format!("can't read the font {}: {e}", file.display()))
        };
        let data = fs::read(file).map_err(|e| unreadable(&e))?;
        FontRef::new(&data).map_err(|e| unreadable(&e))?;
        Ok(Self(data))
    }

    /// The advance width of `text` at `size` px; a character the font lacks counts as 1 em.
    pub fn of(&self, text: &str, size: f64) -> f64 {
        let font = FontRef::new(&self.0).expect("checked when loaded");
        let (charmap, metrics) = (
            font.charmap(),
            font.glyph_metrics(Size::new(size as f32), LocationRef::default()),
        );
        let advance = |c| charmap.map(c).and_then(|g| metrics.advance_width(g));
        text.chars()
            .map(|c| advance(c).map_or(size, f64::from))
            .sum()
    }
}

/// Paints `svg` at `scale` PNG pixels per CSS pixel. A family that isn't anywhere is drawn with
/// `substitute`: the font whose widths the diagram was laid out with.
pub fn png(
    svg: &str,
    scale: f64,
    substitute: &'static str,
    warnings: &mut Vec<Warning>,
) -> Result<Png, Error> {
    let mut db = Database::new();
    for file in fonts()? {
        db.load_font_file(&file).map_err(|e| {
            Error::internal(&format!("can't read the font {}: {e}", file.display()))
        })?;
    }
    let found = Mutex::new(Found::default());
    let fallback = FontResolver::default_fallback_selector();
    let tree = usvg::Tree::from_str(
        svg,
        &usvg::Options {
            fontdb: Arc::new(db),
            font_resolver: FontResolver {
                select_font: Box::new(|font, db| select(font, db, substitute, &found)),
                select_fallback: Box::new(|c, used, db| {
                    system(db, &mut found.lock().unwrap());
                    fallback(c, used, db)
                }),
            },
            // Only images inside the SVG: nothing is read from disk or the network.
            image_href_resolver: ImageHrefResolver {
                resolve_data: ImageHrefResolver::default_data_resolver(),
                resolve_string: Box::new(|_, _| None),
            },
            ..usvg::Options::default()
        },
    )
    .map_err(|e| Error::internal(&format!("the SVG can't be drawn: {e}")))?;
    let found = found.into_inner().unwrap();
    for (family, substitute) in found.substituted {
        warnings.push(Warning::font_substituted(&family, substitute));
    }
    let missing = uncovered(svg)?;
    if !missing.is_empty() {
        warnings.push(Warning::font_fallback(&missing));
    }
    let size = tree.size();
    let (width, height) = (f64::from(size.width()), f64::from(size.height()));
    let (width, height) = (pixels(width, scale), pixels(height, scale));
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| Error::internal(&format!("no {width}×{height} image can be made")))?;
    let s = scale as f32;
    resvg::render(&tree, Transform::from_scale(s, s), &mut pixmap.as_mut());
    let bytes =
        (pixmap.encode_png()).map_err(|e| Error::internal(&format!("PNG encoding: {e}")))?;
    Ok(Png {
        bytes,
        width,
        height,
    })
}

/// A side of `css` pixels at `scale`, rounded up as the SVG's own size is.
fn pixels(css: f64, scale: f64) -> u32 {
    ((css * scale - 1e-6).ceil()).clamp(1.0, f64::from(u32::MAX)) as u32
}

/// The bundled family a name stands in for: Arial and Helvetica are Liberation Sans, Times
/// Liberation Serif, Courier Liberation Mono (each metric-compatible), and so are the generic
/// names.
fn stand_in(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "arial" | "helvetica" | "sans-serif" | "liberation sans" | "cursive" | "fantasy" => {
            Some(SANS)
        }
        "times" | "times new roman" | "serif" | "liberation serif" => Some(SERIF),
        "courier" | "courier new" | "monospace" | "liberation mono" => Some(MONO),
        _ => None,
    }
}

/// The first of the font's families that can be drawn: a bundled one, else one installed here.
/// A named family that isn't anywhere is drawn with the next one, else `substitute`.
fn select(
    font: &usvg::Font,
    db: &mut Arc<Database>,
    substitute: &'static str,
    found: &Mutex<Found>,
) -> Option<ID> {
    let mut gone = None;
    let mut bundled = None;
    for family in font.families() {
        bundled = match family {
            FontFamily::Serif => Some(SERIF),
            FontFamily::Monospace => Some(MONO),
            FontFamily::SansSerif | FontFamily::Cursive | FontFamily::Fantasy => Some(SANS),
            FontFamily::Named(name) => stand_in(name),
        };
        if let (None, FontFamily::Named(name)) = (bundled, family) {
            system(db, &mut found.lock().unwrap());
            if let Some(id) = query(db, name, font) {
                return Some(id);
            }
            gone = gone.or(Some(name));
        } else {
            break;
        }
    }
    let bundled = bundled.unwrap_or(substitute);
    let mut f = found.lock().unwrap();
    if let Some(name) = gone.filter(|n| !f.substituted.iter().any(|(s, _)| s == *n)) {
        f.substituted.push((name.clone(), bundled));
    }
    query(db, bundled, font)
}

/// Loads the computer's fonts the first time a family or character needs them.
fn system(db: &mut Arc<Database>, found: &mut Found) {
    if !found.system {
        Arc::make_mut(db).load_system_fonts();
        found.system = true;
    }
}

/// The face of `family` closest to the font's weight, style and stretch.
fn query(db: &Database, family: &str, font: &usvg::Font) -> Option<ID> {
    db.query(&fontdb::Query {
        families: &[Family::Name(family)],
        weight: fontdb::Weight(font.weight()),
        stretch: match font.stretch() {
            FontStretch::UltraCondensed => fontdb::Stretch::UltraCondensed,
            FontStretch::ExtraCondensed => fontdb::Stretch::ExtraCondensed,
            FontStretch::Condensed => fontdb::Stretch::Condensed,
            FontStretch::SemiCondensed => fontdb::Stretch::SemiCondensed,
            FontStretch::Normal => fontdb::Stretch::Normal,
            FontStretch::SemiExpanded => fontdb::Stretch::SemiExpanded,
            FontStretch::Expanded => fontdb::Stretch::Expanded,
            FontStretch::ExtraExpanded => fontdb::Stretch::ExtraExpanded,
            FontStretch::UltraExpanded => fontdb::Stretch::UltraExpanded,
        },
        style: match font.style() {
            FontStyle::Normal => fontdb::Style::Normal,
            FontStyle::Italic => fontdb::Style::Italic,
            FontStyle::Oblique => fontdb::Style::Oblique,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_families_are_drawn_with_the_bundled_fonts() {
        for (name, bundled) in [
            ("Arial", SANS),
            ("Helvetica", SANS),
            ("sans-Serif", SANS),
            ("Times New Roman", SERIF),
            ("Times", SERIF),
            ("Courier New", MONO),
            ("monospace", MONO),
        ] {
            assert_eq!(stand_in(name), Some(bundled), "{name}");
        }
        assert_eq!(stand_in("Comic Sans MS"), None);
    }

    #[test]
    fn a_png_too_large_is_drawn_at_the_largest_scale_that_fits() {
        assert_eq!(fit(1000.0, 500.0, 2.0).unwrap(), (2.0, None));
        let (s, w) = fit(20000.0, 10.0, 2.0).unwrap();
        assert_eq!(s, 0.81);
        assert_eq!(
            w.unwrap().message,
            "The PNG would be 40000×20 px; drew it at scale 0.81 to stay within 16384 px and 64 MP."
        );
        // 10000 × 10000 at 0.8 is exactly 64 MP.
        assert_eq!(fit(10000.0, 10000.0, 1.0).unwrap().0, 0.8);
        assert_eq!(fit(10000.0, 10000.0, 0.8).unwrap(), (0.8, None));
        // The area, not a side, can be the limit.
        assert_eq!(fit(16000.0, 8000.0, 1.0).unwrap().0, 0.7);
        let big = fit(400_000.0, 100.0, 1.0).unwrap_err();
        assert_eq!(
            (big.code, big.message.as_str()),
            (
                "image_too_large",
                "The diagram is 400000×100 px; a PNG would need a scale below 0.05."
            )
        );
        assert_eq!(fit(327_000.0, 100.0, 1.0).unwrap().0, 0.05);
    }

    #[test]
    fn sides_round_up_like_the_svg_size() {
        assert_eq!(pixels(62.0 * 4.0 / 3.0, 1.0), 83);
        assert_eq!(pixels(100.0, 2.0), 200);
        assert_eq!(pixels(10.2, 1.5), 16);
    }
}
