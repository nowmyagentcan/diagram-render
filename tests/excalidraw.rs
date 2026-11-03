//! Excalidraw scenes drawn from their JSON, or from the scene a PNG or SVG carries.

mod common;

use std::fs;
use std::io::Write;

use common::*;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use serde_json::{Value, json};

fn fixture(file: &str) -> String {
    format!("tests/fixtures/excalidraw/{file}")
}

fn scene(elements: Value) -> Vec<u8> {
    let scene = json!({"type": "excalidraw", "elements": elements});
    serde_json::to_vec_pretty(&scene).unwrap()
}

fn rectangle(id: &str, x: u32) -> Value {
    json!({"id": id, "type": "rectangle", "x": x, "y": 0, "width": 100, "height": 50, "seed": 1})
}

fn svg_of(dir: &std::path::Path, source: &[u8], args: &[&str]) -> (Value, String) {
    let (code, v) = in_dir(
        dir,
        "s.excalidraw",
        source,
        &[&["-o", "out.svg"], args].concat(),
    );
    assert_eq!(code, 0, "{v}");
    (v, fs::read_to_string(dir.join("out.svg")).unwrap())
}

#[test]
fn every_fixture_renders_to_svg_and_png_with_no_warnings() {
    let dir = folder("excalidraw-fixtures");
    for name in ["shapes", "arrows", "text", "freedraw", "frames", "small"] {
        let file = format!("{name}.excalidraw");
        let source = fs::read(fixture(&file)).unwrap();
        for out in ["out.svg", "out.png"] {
            let (code, v) = in_dir(&dir, &file, &source, &["-o", out]);
            assert_eq!(code, 0, "{name}: {v}");
            assert_eq!(codes(&v), Vec::<&str>::new(), "{name}");
            assert_eq!(v["format"], "excalidraw");
            assert_eq!(v["engine"]["name"], "diagram-render");
        }
    }
}

#[test]
fn a_seed_always_draws_the_same_strokes() {
    let dir = folder("excalidraw-seed");
    let (_, one) = svg_of(&dir, &scene(json!([rectangle("a", 0)])), &[]);
    let (_, again) = svg_of(&dir, &scene(json!([rectangle("a", 0)])), &[]);
    assert_eq!(one, again);
    let mut other = rectangle("a", 0);
    other["seed"] = 2.into();
    assert_ne!(one, svg_of(&dir, &scene(json!([other])), &[]).1);
}

#[test]
fn the_canvas_is_the_elements_box_and_10_px_around_it() {
    let dir = folder("excalidraw-canvas");
    let (v, svg) = svg_of(
        &dir,
        &scene(json!([rectangle("a", 0), rectangle("b", 200)])),
        &[],
    );
    assert_eq!((&v["width"], &v["height"]), (&json!(320), &json!(70)));
    assert!(svg.contains(r#"viewBox="0 0 320 70""#), "{svg}");
    // Deleted elements, empty text and a line of one point count for nothing.
    let mut gone = rectangle("c", 900);
    gone["isDeleted"] = true.into();
    let empty =
        json!({"type": "text", "id": "t", "x": 900, "y": 0, "width": 10, "height": 10, "text": ""});
    let dot = json!({"type": "line", "id": "l", "x": 900, "y": 0, "width": 0, "height": 0, "points": [[0, 0]]});
    let (v, _) = svg_of(
        &dir,
        &scene(json!([rectangle("a", 0), gone, empty, dot])),
        &[],
    );
    assert_eq!(v["width"], 120);
}

#[test]
fn json_errors_point_to_their_line_and_column_with_any_line_ends() {
    let dir = folder("excalidraw-syntax");
    let lf = "{\n  \"type\": \"excalidraw\",\n  \"elements\": [}\n}\n";
    for (name, source) in [
        ("lf", lf.to_string()),
        ("crlf", lf.replace('\n', "\r\n")),
        ("bom", format!("\u{feff}{}", lf.replace('\n', "\r\n"))),
        ("cr", lf.replace('\n', "\r")),
    ] {
        let (code, v) = in_dir(&dir, "bad.excalidraw", source.as_bytes(), &[]);
        assert_eq!(code, 1, "{name}");
        let e = &v["error"];
        assert_eq!(e["code"], "syntax_error", "{name}");
        assert_eq!(
            (&e["line"], &e["column"]),
            (&json!(3), &json!(16)),
            "{name}: {e}"
        );
        assert_eq!(e["hint"], "Fix line 3, then run again.");
        assert_eq!(e["excerpt"], "  \"elements\": [}", "{name}");
    }
    // Columns count characters, not bytes.
    let source = "{\"é漢\": 1,\n\"é漢\": ]}";
    let (_, v) = in_dir(&dir, "bad.excalidraw", source.as_bytes(), &[]);
    assert_eq!(
        (&v["error"]["line"], &v["error"]["column"]),
        (&json!(2), &json!(7))
    );
}

#[test]
fn json_that_isnt_a_scene_says_why() {
    let dir = folder("excalidraw-invalid");
    for (source, reason) in [
        (r#"[1]"#, "the file isn't a JSON object"),
        (r#"{"elements": []}"#, "it has no \"type\": \"excalidraw\""),
        (
            r#"{"type": "excalidrawlib", "elements": []}"#,
            "its \"type\" is \"excalidrawlib\", not \"excalidraw\"",
        ),
        (r#"{"type": "excalidraw"}"#, "it has no \"elements\" list"),
        (
            r#"{"type": "excalidraw", "elements": 3}"#,
            "it has no \"elements\" list",
        ),
    ] {
        let (code, v) = in_dir(&dir, "x.excalidraw", source.as_bytes(), &[]);
        assert_eq!(code, 1, "{source}");
        assert_eq!(v["error"]["code"], "invalid_scene", "{source}");
        assert_eq!(
            v["error"]["message"],
            format!("This isn't an Excalidraw scene: {reason}.")
        );
        assert_eq!(
            v["error"]["hint"],
            "Open it in Excalidraw and save it again."
        );
    }
}

#[test]
fn elements_that_cant_be_drawn_are_skipped_with_the_line_of_their_id() {
    let dir = folder("excalidraw-skipped");
    let source = r#"{
  "type": "excalidraw",
  "elements": [
    {"type": "rectangle", "x": 0, "y": 0, "width": 10, "height": 10,
     "boundElements": [{"id": "odd", "type": "text"}], "id": "ok"},
    {
      "id": "odd",
      "type": "hexagon", "x": 0, "y": 0, "width": 10, "height": 10
    },
    {
      "id": "nox", "type": "ellipse", "y": 0, "width": 10, "height": 10
    },
    {"x": 1},
    7
  ]
}"#;
    let (code, v) = in_dir(&dir, "s.excalidraw", source.as_bytes(), &["-o", "out.svg"]);
    assert_eq!(code, 0, "{v}");
    let found: Vec<(&str, u64)> = (v["warnings"].as_array().unwrap().iter())
        .map(|w| (w["message"].as_str().unwrap(), w["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        found,
        [
            (
                "Skipped element odd: its type \"hexagon\" isn't one Excalidraw draws.",
                7
            ),
            ("Skipped element nox: it has no x.", 11),
            ("Skipped element #4: it has no type.", 13),
            ("Skipped element #5: it isn't a JSON object.", 14),
        ]
    );
}

#[test]
fn a_scene_over_the_element_limit_is_refused() {
    let dir = folder("excalidraw-limit");
    let source = format!(
        r#"{{"type": "excalidraw", "elements": [{}0]}}"#,
        "0,".repeat(100_000)
    );
    let (code, v) = in_dir(&dir, "s.excalidraw", source.as_bytes(), &[]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &json!("too_many_elements"))
    );
    assert_eq!(
        v["error"]["message"],
        "This diagram has 100001 elements; the limit is 100000."
    );
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut z = ZlibEncoder::new(vec![], Compression::fast());
    z.write_all(data).unwrap();
    z.finish().unwrap()
}

/// The JSON Excalidraw puts around a scene in a PNG or SVG.
fn wrapper(text: &[u8], compressed: bool) -> Vec<u8> {
    let data = if compressed {
        zlib(text)
    } else {
        text.to_vec()
    };
    let encoded: String = data.iter().map(|&b| char::from(b)).collect();
    let json = json!({"version": "1", "encoding": "bstring", "compressed": compressed, "encoded": encoded});
    // A byte string: every character is one byte in the chunk or the payload.
    (json.to_string().chars()).map(|c| c as u8).collect()
}

/// A 1×1 PNG with `text` in a tEXt chunk under Excalidraw's keyword.
fn png_with(text: &[u8]) -> Vec<u8> {
    let chunk = |kind: &[u8], body: &[u8]| {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(body);
        // The reader doesn't check CRCs.
        out.extend(0_u32.to_be_bytes());
        out
    };
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
    let body = [b"application/vnd.excalidraw+json\0".as_slice(), text].concat();
    png.extend(chunk(b"tEXt", &body));
    png.extend(chunk(b"IEND", b""));
    png
}

fn svg_with(payload: &[u8], version: u32) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><metadata><!-- payload-type:application/vnd.excalidraw+json --><!-- payload-version:{version} --><!-- payload-start -->{b64}<!-- payload-end --></metadata></svg>"
    )
}

#[test]
fn png_and_svg_files_carrying_a_scene_render_it_again() {
    let dir = folder("excalidraw-carried");
    let want = {
        let source = fs::read(fixture("small.excalidraw")).unwrap();
        svg_of(&dir, &source, &[]).1
    };
    for file in ["small.excalidraw.png", "small.excalidraw.svg"] {
        let source = fs::read(fixture(file)).unwrap();
        let (code, v) = in_dir(&dir, file, &source, &["-o", "got.svg"]);
        assert_eq!(code, 0, "{file}: {v}");
        assert_eq!(
            fs::read_to_string(dir.join("got.svg")).unwrap(),
            want,
            "{file}"
        );
    }
    // Default names drop the whole suffix, and the source is never replaced.
    let source = fs::read(fixture("small.excalidraw.png")).unwrap();
    let (code, v) = in_dir(&dir, "small.excalidraw.png", &source, &["--to", "png"]);
    assert_eq!((code, &v["output"]), (0, &json!("small.png")));
    assert_eq!(fs::read(dir.join("small.excalidraw.png")).unwrap(), source);

    // The other ways Excalidraw has stored it: not compressed, plain JSON, payload version 1.
    let scene = scene(json!([rectangle("a", 0)]));
    let want = svg_of(&dir, &scene, &[]).1;
    for (file, source) in [
        ("a.excalidraw.png", png_with(&wrapper(&scene, false))),
        ("b.excalidraw.png", png_with(&scene)),
        (
            "c.excalidraw.svg",
            svg_with(&wrapper(&scene, true), 2).into_bytes(),
        ),
        ("d.excalidraw.svg", svg_with(&scene, 1).into_bytes()),
    ] {
        let (code, v) = in_dir(&dir, file, &source, &["-o", "got.svg"]);
        assert_eq!(code, 0, "{file}: {v}");
        assert_eq!(
            fs::read_to_string(dir.join("got.svg")).unwrap(),
            want,
            "{file}"
        );
    }
}

#[test]
fn a_carried_scene_that_cant_be_read_says_so() {
    let dir = folder("excalidraw-carried-bad");
    for (file, source, code, text) in [
        (
            "a.excalidraw.svg",
            b"<svg><metadata><!-- payload-type:application/vnd.excalidraw+json --></metadata></svg>".to_vec(),
            "invalid_scene",
            "This isn't an Excalidraw scene: the SVG holds no scene between payload-start and payload-end.",
        ),
        (
            "b.excalidraw.svg",
            b"<svg><!-- payload-type:application/vnd.excalidraw+json --><!-- payload-start -->@@<!-- payload-end --></svg>".to_vec(),
            "invalid_scene",
            "",
        ),
        ("c.excalidraw.png", png_with(br#"{"encoding": "bstring", "compressed": true, "encoded": "xx"}"#), "invalid_scene", ""),
        ("d.excalidraw.png", png_with(&wrapper(b"{\n  nope", true)), "syntax_error", ""),
    ] {
        let (status, v) = in_dir(&dir, file, &source, &[]);
        assert_eq!((status, v["error"]["code"].as_str().unwrap()), (1, code), "{file}: {v}");
        if !text.is_empty() {
            assert_eq!(v["error"]["message"], text);
        }
    }
    let (_, v) = in_dir(
        &dir,
        "d.excalidraw.png",
        &png_with(&wrapper(b"{\n  nope", true)),
        &[],
    );
    assert_eq!(v["error"]["line"], 2);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .ends_with("in the scene the PNG carries"),
        "{v}"
    );
    // More than 256 MiB unpacked.
    let bomb = png_with(&wrapper(&vec![b' '; (256 << 20) + 1], true));
    let (status, v) = in_dir(&dir, "e.excalidraw.png", &bomb, &[]);
    assert_eq!((status, &v["error"]["code"]), (1, &json!("too_large")));
}

#[test]
fn images_over_the_data_limit_are_refused() {
    let dir = folder("excalidraw-image-data");
    let url = format!("data:image/png;base64,{}", "A".repeat(65 << 20));
    let source = json!({"type": "excalidraw", "elements": [], "files": {"f": {"dataURL": url}}});
    let png = png_with(&wrapper(source.to_string().as_bytes(), true));
    let (status, v) = in_dir(&dir, "a.excalidraw.png", &png, &[]);
    assert_eq!((status, &v["error"]["code"]), (1, &json!("too_large")));
    assert_eq!(
        v["error"]["message"],
        "a.excalidraw.png holds 65.0 MiB of image data; the limit is 64 MiB."
    );
}

#[test]
fn an_image_without_its_data_is_a_grey_box_and_a_warning() {
    let dir = folder("excalidraw-image");
    let image = |id: &str, file: &str| json!({"id": id, "type": "image", "x": 0, "y": 0, "width": 80, "height": 60, "fileId": file});
    let source = json!({
        "type": "excalidraw",
        "elements": [image("gone", "nope"), image("text", "html")],
        "files": {"html": {"dataURL": "data:text/html;base64,PGI+"}}
    });
    let source = serde_json::to_vec_pretty(&source).unwrap();
    let (v, svg) = svg_of(&dir, &source, &[]);
    assert_eq!(codes(&v), ["image_missing", "image_missing"]);
    let w = &v["warnings"][0];
    assert_eq!(
        w["message"],
        "Image gone has no image data in the file; drew a placeholder."
    );
    assert_eq!(w["line"], 5);
    assert!(svg.contains(r##"fill="#E7E7E7""##), "{svg}");
    assert!(!svg.contains("data:text"), "{svg}");
}

fn text(s: &str, family: i64) -> Value {
    json!({"id": "t", "type": "text", "x": 0, "y": 0, "width": 200, "height": 25,
           "text": s, "fontSize": 20, "fontFamily": family})
}

#[test]
fn text_is_drawn_as_outlines_with_the_text_kept_as_a_label() {
    let dir = folder("excalidraw-text");
    let (_, svg) = svg_of(&dir, &scene(json!([text("a <b> & c\nnext", 5)])), &[]);
    assert!(!svg.contains("<text"), "{svg}");
    assert!(svg.contains(r#"aria-label="a &lt;b&gt; &amp; c""#), "{svg}");
    assert!(svg.contains(r#"aria-label="next""#), "{svg}");
    assert_eq!(svg.matches("aria-label").count(), 2);
}

#[test]
fn a_font_id_excalidraw_doesnt_have_is_drawn_with_excalifont() {
    let dir = folder("excalidraw-font");
    let (v, with_id) = svg_of(&dir, &scene(json!([text("Hello", 4)])), &[]);
    assert_eq!(codes(&v), ["font_substituted"]);
    assert_eq!(
        v["warnings"][0]["message"],
        "Font #4 isn't available; drew its text with Excalifont."
    );
    let (v, excalifont) = svg_of(&dir, &scene(json!([text("Hello", 5)])), &[]);
    assert_eq!(codes(&v), Vec::<&str>::new());
    assert_eq!(with_id, excalifont);
}

#[test]
fn characters_outside_the_bundled_fonts_come_from_the_computer() {
    let dir = folder("excalidraw-cjk");
    let (v, _) = svg_of(&dir, &scene(json!([text("漢字 ok", 5)])), &[]);
    assert_eq!(codes(&v), ["font_fallback"]);
    assert!(
        v["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("漢字"),
        "{v}"
    );
}

#[test]
fn a_missing_font_file_is_an_install_fault() {
    let bin = installed("excalidraw-no-fonts", &[]);
    let dir = bin.parent().unwrap();
    fs::create_dir(dir.join("fonts")).unwrap();
    for entry in fs::read_dir(env!("CARGO_BIN_EXE_diagram-render").replace(
        &format!("diagram-render{}", std::env::consts::EXE_SUFFIX),
        "fonts",
    ))
    .unwrap()
    {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::copy(&path, dir.join("fonts").join(path.file_name().unwrap())).unwrap();
        }
    }
    fs::write(dir.join("s.excalidraw"), scene(json!([text("Hi", 5)]))).unwrap();
    let (code, _, err) = run_at(&bin, &["s.excalidraw", "-o", "out.svg"], b"");
    assert_eq!(code, 3, "{err}");
    assert!(
        err.contains("The fonts PNGs are drawn with are missing"),
        "{err}"
    );
}

#[test]
fn the_background_is_the_scenes_unless_another_is_asked_for() {
    let dir = folder("excalidraw-background");
    let source = serde_json::to_vec(&json!({
        "type": "excalidraw",
        "elements": [rectangle("a", 0)],
        "appState": {"viewBackgroundColor": "#fff3bf"}
    }))
    .unwrap();
    assert!(svg_of(&dir, &source, &[]).1.contains(r##"fill="#fff3bf""##));
    let (_, svg) = svg_of(&dir, &source, &["--background", "#336699"]);
    assert!(
        svg.contains(r##"fill="#336699""##) && !svg.contains("#fff3bf"),
        "{svg}"
    );
    let (_, svg) = svg_of(&dir, &source, &["--background", "transparent"]);
    assert!(!svg.contains("<rect"), "{svg}");
}

#[test]
fn a_scene_too_big_for_a_png_comes_out_smaller() {
    let dir = folder("excalidraw-big");
    let source = json!([{"id": "a", "type": "rectangle", "x": 0, "y": 0, "width": 20000, "height": 50, "seed": 1}]);
    let (code, v) = in_dir(&dir, "s.excalidraw", &scene(source), &["-o", "out.png"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(codes(&v), ["scale_reduced"]);
}

#[test]
fn links_stay_links_and_scripts_dont() {
    let dir = folder("excalidraw-links");
    let mut a = rectangle("a", 0);
    a["link"] = "https://example.com/?a=1&b=2".into();
    let mut b = rectangle("b", 200);
    b["link"] = "javascript:alert(1)".into();
    let (v, svg) = svg_of(&dir, &scene(json!([a, b])), &[]);
    assert!(
        svg.contains(r#"href="https://example.com/?a=1&amp;b=2""#),
        "{svg}"
    );
    assert!(!svg.contains("javascript"), "{svg}");
    assert_eq!(codes(&v), ["unsafe_content_removed"]);
}

#[test]
fn colours_cant_break_out_of_their_attributes() {
    let dir = folder("excalidraw-escape");
    let mut a = rectangle("a", 0);
    a["strokeColor"] = r#""/><script>alert(1)</script>"#.into();
    a["backgroundColor"] = "<x>".into();
    let (_, svg) = svg_of(&dir, &scene(json!([a])), &[]);
    assert!(!svg.contains("<script") && !svg.contains("<x>"), "{svg}");
}

#[test]
fn check_finds_what_a_render_finds_and_writes_nothing() {
    let dir = folder("excalidraw-check");
    fs::write(
        dir.join("s.excalidraw"),
        scene(json!([rectangle("a", 0), {"id": "q", "type": "blob"}])),
    )
    .unwrap();
    let (code, out, _) = run_in(&dir, &["check", "s.excalidraw", "--json"], b"");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!((code, &v["valid"]), (0, &json!(true)), "{v}");
    assert_eq!(codes(&v), ["element_skipped"]);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    let (code, v) = in_dir(&dir, "s.excalidraw", b"{", &[]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("syntax_error")));
}

#[test]
fn scenes_have_no_pages() {
    let dir = folder("excalidraw-page");
    let (code, v) = in_dir(&dir, "s.excalidraw", &scene(json!([])), &["--page", "2"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (2, &json!("page_not_supported"))
    );
}

#[test]
fn renders_match_export_to_svg() {
    // References: exportToSvg of @excalidraw/excalidraw 0.18.1 in Chromium, as greyscale PNGs. The
    // bar is 5 %; ours are far under it, as strokes are the same, so a slip shows at once.
    let labels: Value =
        serde_json::from_str(&fs::read_to_string(fixture("refs/labels.json")).unwrap()).unwrap();
    let dir = folder("excalidraw-references");
    for (name, want) in labels.as_object().unwrap() {
        let file = format!("{name}.excalidraw");
        let source = fs::read(fixture(&file)).unwrap();
        assert_eq!(
            in_dir(&dir, &file, &source, &["-o", "out.svg"]).0,
            0,
            "{name}"
        );
        let svg = fs::read_to_string(dir.join("out.svg")).unwrap();
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let ours: Vec<&str> = doc
            .descendants()
            .filter_map(|n| n.attribute("aria-label"))
            .collect();
        for label in want.as_array().unwrap() {
            let label = label.as_str().unwrap();
            assert!(ours.contains(&label), "{name}: {label} not in {ours:?}");
        }
        assert_eq!(
            in_dir(&dir, &file, &source, &["-o", "out.png", "--scale", "1"]).0,
            0
        );
        let ours = fs::read(dir.join("out.png")).unwrap();
        let reference = fs::read(fixture(&format!("refs/{name}.png"))).unwrap();
        let difference = blurred_difference(&ours, &reference, name, 4);
        assert!(difference <= 0.01, "{name}: {:.2} %", difference * 100.0);
    }
}
