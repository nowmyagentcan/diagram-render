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

fn text(s: &str, family: i64) -> Value {
    json!({"id": "t", "type": "text", "x": 0, "y": 0, "width": 200, "height": 25,
           "text": s, "fontSize": 20, "fontFamily": family})
}
