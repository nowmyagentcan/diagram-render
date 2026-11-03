//! PNG output: sizes from the scale, the bundled fonts and the warnings about fonts.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use resvg::tiny_skia::Pixmap;
use serde_json::Value;

/// Renders `source` as `file` in `dir` with `args`: the JSON result and the bytes it wrote.
fn render(dir: &Path, file: &str, source: &[u8], args: &[&str]) -> (Value, Vec<u8>) {
    fs::write(dir.join(file), source).unwrap();
    let (code, out, err) = run_in(dir, &[&[file, "--json"], args].concat(), b"");
    assert_eq!((code, err.as_str()), (0, ""), "{file} {args:?}: {out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let bytes = fs::read(dir.join(v["output"].as_str().unwrap())).unwrap();
    (v, bytes)
}

fn warnings(v: &Value) -> Vec<&str> {
    let all = v["warnings"].as_array().unwrap();
    all.iter().map(|w| w["code"].as_str().unwrap()).collect()
}

const DOT: &[u8] = b"digraph { node [shape=box]; a [label=\"Start\"]; a -> b -> c; a -> c }";

#[test]
fn a_dot_png_is_the_svg_size_times_the_scale() {
    let dir = folder("png-dot-size");
    let (svg, _) = render(&dir, "g.dot", DOT, &[]);
    let side = |v: &Value, k: &str| v[k].as_u64().unwrap();
    for scale in ["1", "2", "0.5"] {
        let (v, png) = render(&dir, "g.dot", DOT, &["--to", "png", "--scale", scale]);
        let s: f64 = scale.parse().unwrap();
        assert_eq!(
            (v["to"].as_str(), v["scale"].as_f64()),
            (Some("png"), Some(s))
        );
        assert_eq!(v["scale"].is_u64(), scale != "0.5", "{v}");
        assert_eq!(v["bytes"].as_u64(), Some(png.len() as u64));
        for k in ["width", "height"] {
            let want = side(&svg, k) as f64 * s;
            assert!(
                (side(&v, k) as f64 - want).abs() <= 1.0,
                "{k} at {scale}: {v}"
            );
        }
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(
            (u64::from(pixmap.width()), u64::from(pixmap.height())),
            (side(&v, "width"), side(&v, "height"))
        );
        assert_eq!(warnings(&v), Vec::<&str>::new());
    }
    // The default scale is 2, and the PNG goes beside the input.
    let (v, _) = render(&dir, "g.dot", DOT, &["--to", "png"]);
    assert_eq!(
        (v["scale"].as_f64(), v["output"].as_str()),
        (Some(2.0), Some("g.png"))
    );
}

#[test]
fn a_png_is_8_bit_rgba_with_no_metadata() {
    let dir = folder("png-dot-chunks");
    let (_, png) = render(&dir, "g.dot", DOT, &["-o", "g.png"]);
    // Bit depth 8, colour type 6 (RGBA) in IHDR.
    assert_eq!(&png[24..26], &[8, 6]);
    let mut chunks = vec![];
    let mut at = 8;
    while at < png.len() {
        let n = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        chunks.push(String::from_utf8_lossy(&png[at + 4..at + 8]).into_owned());
        at += 12 + n;
    }
    chunks.dedup();
    assert_eq!(chunks, ["IHDR", "IDAT", "IEND"]);
}

#[test]
fn a_font_that_isnt_installed_is_drawn_with_the_font_graphviz_measured() {
    let dir = folder("png-dot-font");
    let source = b"digraph { a [fontname=\"No Such Font Pro\" label=\"Hello\"] }";
    let (v, _) = render(&dir, "f.dot", source, &["--to", "png"]);
    assert_eq!(
        v["warnings"],
        serde_json::json!([
            {"code": "engine_warning", "message": "Graphviz: no hard-coded metrics for 'No Such Font Pro'.  Falling back to 'Times' metrics"},
            {"code": "font_substituted", "message": "Font No Such Font Pro isn't available; drew its text with Liberation Serif."},
        ])
    );
    // Graphviz's own families are the bundled ones: no warning.
    let source =
        b"digraph { a [fontname=Helvetica]; b [fontname=Courier]; c [fontname=\"Times-Roman\"] }";
    let (v, _) = render(&dir, "g.dot", source, &["--to", "png"]);
    assert_eq!(warnings(&v), Vec::<&str>::new());
}

#[test]
fn characters_outside_the_bundled_fonts_say_where_they_came_from() {
    let dir = folder("png-dot-cjk");
    let source = "digraph { a [label=\"漢字 ok\"] }";
    let (v, _) = render(&dir, "c.dot", source.as_bytes(), &["--to", "png"]);
    let fallback = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == "font_fallback");
    assert_eq!(
        fallback.unwrap()["message"],
        "Some characters aren't in the bundled fonts (漢字); this computer's fonts drew them, so the PNG can differ between computers."
    );
}

#[test]
fn a_png_over_the_limits_is_drawn_smaller_and_one_far_over_is_refused() {
    let dir = folder("png-dot-limits");
    let wide = b"digraph { a [shape=box width=300 height=0.1 fixedsize=true label=\"\"] }";
    let (v, png) = render(&dir, "wide.dot", wide, &["--to", "png"]);
    assert_eq!(v["scale"], 0.56);
    assert_eq!(
        v["warnings"],
        serde_json::json!([{"code": "scale_reduced", "message": "The PNG would be 57622×40 px; drew it at scale 0.56 to stay within 16384 px and 64 MP."}])
    );
    assert_eq!(Pixmap::decode_png(&png).unwrap().width(), 16134);
    let huge = b"digraph { a [shape=box width=5000 height=0.1 fixedsize=true label=\"\"] }";
    fs::write(dir.join("huge.dot"), huge).unwrap();
    let (code, v) = json(&[
        dir.join("huge.dot").to_str().unwrap(),
        "--to",
        "png",
        "--json",
    ]);
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "image_too_large");
    assert_eq!(
        v["error"]["message"],
        "The diagram is 480011×20 px; a PNG would need a scale below 0.05."
    );
    assert!(!dir.join("huge.png").exists());
}

/// The colour of the top-left pixel, as straight RGBA.
fn corner(png: &[u8]) -> [u8; 4] {
    let c = Pixmap::decode_png(png)
        .unwrap()
        .pixel(0, 0)
        .unwrap()
        .demultiply();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

#[test]
fn the_background_is_the_diagrams_unless_another_is_asked_for() {
    let dir = folder("png-dot-background");
    let graph = b"digraph { a -> b }";
    let cases: [(&[&str], [u8; 4]); 4] = [
        (&[], [255, 255, 255, 255]),
        (&["--background", "transparent"], [0, 0, 0, 0]),
        (&["--background", "#336699"], [0x33, 0x66, 0x99, 255]),
        (&["--background", "#369"], [0x33, 0x66, 0x99, 255]),
    ];
    for (args, want) in cases {
        let (_, png) = render(&dir, "g.dot", graph, &[&["--to", "png"], args].concat());
        assert_eq!(corner(&png), want, "{args:?}");
    }
    let tinted = b"digraph { bgcolor=\"#fdf6e3\"; a -> b }";
    let (_, png) = render(&dir, "t.dot", tinted, &["--to", "png"]);
    assert_eq!(corner(&png), [0xfd, 0xf6, 0xe3, 255]);
}
