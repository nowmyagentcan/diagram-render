//! Excalidraw text as glyph outlines: its fonts (the WOFF2 files of `@excalidraw/excalidraw`
//! 0.18.1, decoded in memory) shaped by usvg, so the SVG looks the same everywhere and needs no
//! font files. Metrics are Excalidraw 0.18.1's (`fonts/FontMetadata.ts`, MIT).

use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::sync::{Arc, Mutex};

use resvg::tiny_skia::PathSegment;
use resvg::usvg::fontdb::{Database, ID, Source};
use resvg::usvg::{self, FontFamily, FontResolver, Node};

use crate::engine;
use crate::error::{Error, Warning};
use crate::raster;
use crate::svg::{escape, num as n};

/// A font id's family and the files (by name prefix in `fonts/excalidraw/`) that hold it;
/// Helvetica and Liberation Sans are the bundled Liberation Sans.
fn family(id: i64) -> Option<(&'static str, &'static str)> {
    Some(match id {
        1 => ("Virgil", "Virgil-"),
        2 | 9 => ("Liberation Sans", ""),
        3 => ("Cascadia", "CascadiaCode-"),
        5 => ("Excalifont", "Excalifont-"),
        6 => ("Nunito", "Nunito-"),
        7 => ("Lilita One", "Lilita-"),
        8 => ("Comic Shanns", "ComicShanns-"),
        _ => return None,
    })
}

/// The font id text is drawn with: its own, or Excalifont for an id Excalidraw doesn't have.
pub fn drawn_with(id: i64) -> i64 {
    if family(id).is_some() { id } else { 5 }
}

/// Units per em, ascender, descender and line height of a font id; Virgil's for other ids, as
/// Excalidraw falls back to them.
pub fn metrics(id: i64) -> (f64, f64, f64, f64) {
    match id {
        2 => (2048.0, 1577.0, -471.0, 1.15),
        3 => (2048.0, 1900.0, -480.0, 1.2),
        6 => (1000.0, 1011.0, -353.0, 1.35),
        7 => (1000.0, 923.0, -220.0, 1.15),
        8 => (1000.0, 750.0, -250.0, 1.25),
        9 => (2048.0, 1854.0, -434.0, 1.15),
        _ => (1000.0, 886.0, -374.0, 1.25),
    }
}

/// Where the first baseline sits below the top of a line box, as Excalidraw's
/// `getVerticalOffset` puts it.
pub fn baseline(id: i64, size: f64, line_px: f64) -> f64 {
    let (em, ascender, descender, _) = metrics(id);
    let em = size / em;
    let gap = (line_px - em * ascender + em * descender) / 2.0;
    em * ascender + gap
}

/// A warning for a font id that isn't one of Excalidraw's.
pub fn substituted(id: i64) -> Warning {
    Warning::font_substituted(&format!("#{id}"), "Excalifont")
}

/// One line of text to outline: where its anchor is, how it's anchored, and its font.
pub struct Line {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub font: i64,
    pub anchor: &'static str,
    pub rtl: bool,
}

/// Whether the first strongly directional character is right-to-left, as Excalidraw's `isRTL`
/// checks.
pub fn rtl(text: &str) -> bool {
    let ltr = |c: char| {
        c.is_ascii_alphabetic()
            || matches!(c, '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2B8}'
                | '\u{300}'..='\u{590}' | '\u{800}'..='\u{1FFF}' | '\u{2C00}'..='\u{FB1C}'
                | '\u{FDFE}'..='\u{FE6F}' | '\u{FEFD}'..='\u{FFFF}')
    };
    let rtl = |c: char| matches!(c, '\u{591}'..='\u{7FF}' | '\u{FB1D}'..='\u{FDFD}' | '\u{FE70}'..='\u{FEFC}');
    text.chars().find(|&c| ltr(c) || rtl(c)).is_some_and(rtl)
}

/// The path data of each line's glyphs, in the coordinates the line was placed in, and the
/// warning when the computer's fonts drew some characters.
pub fn outlines(lines: &[Line]) -> Result<(Vec<String>, Option<Warning>), Error> {
    let mut paths = vec![String::new(); lines.len()];
    if lines.iter().all(|l| l.text.trim().is_empty()) {
        return Ok((paths, None));
    }
    let mut db = Database::new();
    // Liberation Sans first: it draws ids 2 and 9, and any Latin character a subset lacks.
    let bundled = raster::fonts()?;
    let sans = db.load_font_source(Source::Binary(Arc::new(read(&bundled[0])?)));
    let mut faces: HashMap<i64, Vec<ID>> = HashMap::new();
    let dir = engine::home().join("fonts").join("excalidraw");
    let mut used: Vec<i64> = lines.iter().map(|l| l.font).collect();
    used.sort_unstable();
    used.dedup();
    for id in used {
        let (_, prefix) = family(id).expect("drawn_with gives a known id");
        if prefix.is_empty() {
            faces.insert(id, sans.to_vec());
            continue;
        }
        let mut files: Vec<_> = (fs::read_dir(&dir).into_iter().flatten().flatten())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                name.starts_with(prefix) && name.ends_with(".woff2")
            })
            .collect();
        files.sort();
        if files.is_empty() {
            let want = dir.join(format!("{prefix}*.woff2"));
            return Err(Error::fonts_missing(&want.display().to_string()));
        }
        let mut ids = vec![];
        for file in files {
            let woff2 = read(&file)?;
            let ttf = wuff::decompress_woff2(&woff2).map_err(|e| {
                Error::internal(&format!("can't read the font {}: {e:?}", file.display()))
            })?;
            ids.extend(db.load_font_source(Source::Binary(Arc::new(ttf))));
        }
        faces.insert(id, ids);
    }
    let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg">"#);
    for (i, l) in lines.iter().enumerate() {
        let dir = if l.rtl { "rtl" } else { "ltr" };
        let _ = write!(
            svg,
            r#"<text id="t{i}" x="{}" y="{}" font-family="f{}" font-size="{}" text-anchor="{}" direction="{dir}" xml:space="preserve">{}</text>"#,
            n(l.x),
            n(l.y),
            l.font,
            l.size,
            l.anchor,
            escape(&l.text)
        );
    }
    svg += "</svg>";
    let db = Arc::new(db);
    // Whether the computer's fonts are loaded.
    let system = Mutex::new(false);
    let groups = &faces;
    let tree = usvg::Tree::from_str(
        &svg,
        &usvg::Options {
            fontdb: db.clone(),
            font_resolver: FontResolver {
                select_font: Box::new(|font, _| {
                    let FontFamily::Named(name) = font.families().first()? else {
                        return None;
                    };
                    let id: i64 = name.strip_prefix('f')?.parse().ok()?;
                    groups.get(&id)?.first().copied()
                }),
                select_fallback: Box::new(|c, tried, db| {
                    // Another file of the same family, then Liberation Sans, then the computer's.
                    let base = *tried.first()?;
                    let own = groups.values().find(|ids| ids.contains(&base));
                    let mut next = own.into_iter().flatten().chain(sans.iter());
                    if let Some(id) = next.find(|id| !tried.contains(id) && has_char(db, **id, c)) {
                        return Some(*id);
                    }
                    let mut loaded = system.lock().unwrap();
                    if !*loaded {
                        Arc::make_mut(db).load_system_fonts();
                        *loaded = true;
                    }
                    (db.faces())
                        .find(|f| !tried.contains(&f.id) && has_char(db, f.id, c))
                        .map(|f| f.id)
                }),
            },
            ..usvg::Options::default()
        },
    )
    .map_err(|e| Error::internal(&format!("text can't be laid out: {e}")))?;
    collect(tree.root(), &mut paths);
    // Up to 5 characters no bundled font has, each once, in order.
    let mut missing = vec![];
    if system.into_inner().unwrap() {
        for l in lines {
            let own = &faces[&l.font];
            for c in l
                .text
                .chars()
                .filter(|c| !c.is_whitespace() && !c.is_control())
            {
                let bundled = own
                    .iter()
                    .chain(sans.iter())
                    .any(|&id| has_char(&db, id, c));
                if !bundled && !missing.contains(&c) && missing.len() < 5 {
                    missing.push(c);
                }
            }
        }
    }
    let warning = (!missing.is_empty()).then(|| Warning::font_fallback(&missing));
    Ok((paths, warning))
}

fn has_char(db: &Database, id: ID, c: char) -> bool {
    use skrifa::MetadataProvider;
    db.with_face_data(id, |data, index| {
        let font = skrifa::FontRef::from_index(data, index).ok()?;
        font.charmap().map(c)
    })
    .flatten()
    .is_some()
}

fn read(path: &std::path::Path) -> Result<Vec<u8>, Error> {
    fs::read(path)
        .map_err(|e| Error::internal(&format!("can't read the font {}: {e}", path.display())))
}

/// Writes the glyph outlines of every text `t{i}` under `group` into `paths[i]`.
fn collect(group: &usvg::Group, paths: &mut [String]) {
    for node in group.children() {
        match node {
            Node::Group(g) => collect(g, paths),
            Node::Text(t) => {
                let Some(i) = t
                    .id()
                    .strip_prefix('t')
                    .and_then(|i| i.parse::<usize>().ok())
                else {
                    continue;
                };
                let mut d = String::new();
                outline(t.flattened(), &mut d);
                if let Some(slot) = paths.get_mut(i) {
                    *slot = d.trim_end().to_string();
                }
            }
            _ => {}
        }
    }
}

fn outline(group: &usvg::Group, d: &mut String) {
    for node in group.children() {
        match node {
            Node::Group(g) => outline(g, d),
            Node::Path(p) => {
                let Some(data) = p.data().clone().transform(p.abs_transform()) else {
                    continue;
                };
                for seg in data.segments() {
                    let _ = match seg {
                        PathSegment::MoveTo(a) => {
                            write!(d, "M{} {} ", n(a.x.into()), n(a.y.into()))
                        }
                        PathSegment::LineTo(a) => {
                            write!(d, "L{} {} ", n(a.x.into()), n(a.y.into()))
                        }
                        PathSegment::QuadTo(a, b) => write!(
                            d,
                            "Q{} {} {} {} ",
                            n(a.x.into()),
                            n(a.y.into()),
                            n(b.x.into()),
                            n(b.y.into())
                        ),
                        PathSegment::CubicTo(a, b, c) => write!(
                            d,
                            "C{} {} {} {} {} {} ",
                            n(a.x.into()),
                            n(a.y.into()),
                            n(b.x.into()),
                            n(b.y.into()),
                            n(c.x.into()),
                            n(c.y.into())
                        ),
                        PathSegment::Close => write!(d, "Z "),
                    };
                }
            }
            _ => {}
        }
    }
}
