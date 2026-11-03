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

/// Copies the fixture `tests/fixtures/{kind}/{file}` into `dir`; its source.
fn fixture(dir: &Path, kind: &str, file: &str) -> Vec<u8> {
    let source = fs::read(Path::new("tests/fixtures").join(kind).join(file)).unwrap();
    fs::write(dir.join(file), &source).unwrap();
    source
}

/// Renders `file` to SVG and to PNG at scales 1 and 2: the PNG sizes are the SVG's (±1 px)
/// and twice it, and the files are 8-bit RGBA.
fn png_sizes_follow_the_svg(dir: &Path, file: &str, source: &[u8]) {
    let (svg, _) = render(dir, file, source, &[]);
    for scale in [1.0, 2.0] {
        let s = scale.to_string();
        let (v, png) = render(dir, file, source, &["--to", "png", "--scale", &s]);
        assert_eq!(v["scale"], scale, "{file}");
        for k in ["width", "height"] {
            let want = svg[k].as_f64().unwrap() * scale;
            assert!(
                (v[k].as_f64().unwrap() - want).abs() <= 1.0,
                "{file} {k} at {s}: {v}"
            );
        }
        assert_eq!(&png[24..26], &[8, 6], "{file}");
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(
            (pixmap.width(), pixmap.height()),
            (
                v["width"].as_u64().unwrap() as u32,
                v["height"].as_u64().unwrap() as u32
            )
        );
    }
}

#[test]
fn mermaid_pngs_are_drawn_by_merman_at_the_svg_size() {
    let dir = folder("png-mermaid");
    for family in ["flowchart", "sequence", "class", "state", "er"] {
        let file = format!("{family}.mmd");
        let source = fixture(&dir, "mermaid", &file);
        png_sizes_follow_the_svg(&dir, &file, &source);
    }
    let (v, png) = render(
        &dir,
        "f.mmd",
        b"graph TD\n  A --> B\n",
        &["--to", "png", "--background", "transparent"],
    );
    assert_eq!((corner(&png), warnings(&v)), ([0, 0, 0, 0], vec![]));
    let (_, png) = render(
        &dir,
        "f.mmd",
        b"graph TD\n  A --> B\n",
        &["--to", "png", "--background", "#336699"],
    );
    assert_eq!(corner(&png), [0x33, 0x66, 0x99, 255]);
    let (_, png) = render(&dir, "f.mmd", b"graph TD\n  A --> B\n", &["--to", "png"]);
    assert_eq!(corner(&png), [255, 255, 255, 255]);
}

#[test]
fn a_mermaid_label_outside_the_bundled_fonts_uses_the_computers() {
    let dir = folder("png-mermaid-cjk");
    let (v, _) = render(
        &dir,
        "c.mmd",
        "graph TD\n  A[漢字 ok] --> B\n".as_bytes(),
        &["--to", "png"],
    );
    assert_eq!(
        v["warnings"],
        serde_json::json!([{"code": "font_fallback", "message": "Some characters aren't in the bundled fonts (漢字); this computer's fonts drew them, so the PNG can differ between computers."}])
    );
    // The SVG names the families; the viewer's fonts draw them.
    let (v, _) = render(
        &dir,
        "c.mmd",
        "graph TD\n  A[漢字 ok] --> B\n".as_bytes(),
        &[],
    );
    assert_eq!(warnings(&v), Vec::<&str>::new());
}

#[test]
fn d2_pngs_are_drawn_by_d2_at_the_svg_size() {
    let dir = folder("png-d2");
    for file in ["containers.d2", "sql_table.d2", "sequence.d2", "sketch.d2"] {
        let source = fixture(&dir, "d2", file);
        png_sizes_follow_the_svg(&dir, file, &source);
    }
    // d2 keeps its theme's background, so the PNG is opaque.
    let (v, png) = render(
        &dir,
        "a.d2",
        b"a -> b",
        &["--to", "png", "--background", "transparent"],
    );
    assert_eq!(warnings(&v), ["background_ignored"]);
    assert_eq!(corner(&png)[3], 255);
    // Its own fonts have CJK, and are the same everywhere: no warning.
    let (v, _) = render(&dir, "c.d2", "a: 漢字 ok".as_bytes(), &["--to", "png"]);
    assert_eq!(warnings(&v), Vec::<&str>::new());
}

#[test]
fn a_d2_png_needs_the_network_for_remote_images() {
    let dir = folder("png-d2-network");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/pixel.png", listener.local_addr().unwrap());
    let source = format!("logo: {{\n  shape: image\n  icon: {url}\n}}\n");
    fs::write(dir.join("r.d2"), &source).unwrap();
    let (code, out, _) = run_in(&dir, &["r.d2", "--to", "png", "--json"], b"");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("network_needed")),
        "{out}"
    );
    assert_eq!(
        v["error"]["message"],
        "This D2 diagram uses remote images, and the network is off."
    );
    assert!(listener.accept().is_err(), "no request reached the server");
    assert!(!dir.join("r.png").exists());
    // With the network, d2 fetches the image for the PNG, and only for it.
    listener.set_nonblocking(false).unwrap();
    let pixel = fs::read("tests/fixtures/d2/pixel.png").unwrap();
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut conn, _) = listener.accept().unwrap();
        let mut request = vec![0; 4096];
        let n = conn.read(&mut request).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            pixel.len()
        );
        conn.write_all(&[head.as_bytes(), &pixel].concat()).unwrap();
        listener.set_nonblocking(true).unwrap();
        (
            String::from_utf8_lossy(&request[..n]).into_owned(),
            listener.accept().is_err(),
        )
    });
    let (v, _) = render(
        &dir,
        "r.d2",
        source.as_bytes(),
        &["--to", "png", "--allow-network"],
    );
    assert_eq!(warnings(&v), Vec::<&str>::new());
    let (request, once) = server.join().unwrap();
    assert!(request.starts_with("GET /pixel.png "), "{request}");
    assert!(once, "one request");
}

#[test]
fn mermaid_and_d2_pngs_over_the_limits_are_drawn_smaller() {
    let dir = folder("png-engine-limits");
    let chain: Vec<String> = (0..40).map(|i| format!("n{i}")).collect();
    let mermaid = format!("graph LR\n  {}\n", chain.join(" --> "));
    let d2 = format!("direction: right\n{}\n", chain.join(" -> "));
    for (file, source) in [("w.mmd", mermaid), ("w.d2", d2)] {
        let (svg, _) = render(&dir, file, source.as_bytes(), &[]);
        let (v, _) = render(
            &dir,
            file,
            source.as_bytes(),
            &["--to", "png", "--scale", "8"],
        );
        assert_eq!(warnings(&v), ["scale_reduced"], "{file}");
        let scale = v["scale"].as_f64().unwrap();
        assert!(
            scale < 8.0 && v["width"].as_u64().unwrap() <= 16384,
            "{file}: {v}"
        );
        // d2's PNG has the diagram's own size, which its SVG rounds to a whole pixel.
        let slack = if file.ends_with("d2") {
            1.0 + scale
        } else {
            1.0
        };
        let want = svg["width"].as_f64().unwrap() * scale;
        assert!(
            (v["width"].as_f64().unwrap() - want).abs() <= slack,
            "{file}: {v}"
        );
    }
}

/// The mean difference of two images' channels, out of 255.
fn mean_difference(a: &Pixmap, b: &Pixmap) -> f64 {
    let pairs = a.data().iter().zip(b.data());
    let sum: u64 = pairs.map(|(x, y)| u64::from(x.abs_diff(*y))).sum();
    sum as f64 / a.data().len() as f64
}

#[test]
fn pngs_with_the_bundled_fonts_match_on_every_computer() {
    // Made on Windows x64. resvg draws the same pixels on any x86-64; other CPUs may round a
    // few edge pixels differently.
    let dir = folder("png-reference");
    for (kind, name, ext) in [
        ("dot", "clusters", "dot"),
        ("dot", "records", "dot"),
        ("mermaid", "flowchart", "mmd"),
        ("mermaid", "sequence", "mmd"),
    ] {
        let file = format!("{name}.{ext}");
        let source = fixture(&dir, kind, &file);
        let (_, png) = render(&dir, &file, &source, &["--to", "png", "--scale", "1"]);
        let got = Pixmap::decode_png(&png).unwrap();
        let want = Pixmap::decode_png(&fs::read(format!("tests/fixtures/png/{name}.png")).unwrap());
        let want = want.unwrap();
        assert_eq!(
            (got.width(), got.height()),
            (want.width(), want.height()),
            "{name}"
        );
        let difference = mean_difference(&got, &want);
        assert!(difference <= 0.5, "{name}: {difference}");
        if kind == "dot" && cfg!(target_arch = "x86_64") {
            assert!(got.data() == want.data(), "{name} differs: {difference}");
        }
    }
}
