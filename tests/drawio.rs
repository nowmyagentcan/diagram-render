//! draw.io: read from `.drawio`, `.drawio.svg` and `.drawio.png` files and drawn by this binary.

mod common;

use std::fs;
use std::io::Write;
use std::path::Path;

use common::*;
use serde_json::{Value, json};

/// `tests/fixtures/drawio/{file}`.
fn fixture(file: &str) -> String {
    Path::new("tests/fixtures/drawio")
        .join(file)
        .to_string_lossy()
        .into_owned()
}

/// Copies `source` as `file` into `dir` and runs `args` on it there with `--json`.
fn in_dir(dir: &Path, file: &str, source: &[u8], args: &[&str]) -> (i32, Value) {
    fs::write(dir.join(file), source).unwrap();
    let (code, out, err) = run_in(dir, &[&[file, "--json"], args].concat(), b"");
    assert_eq!(err, "", "{file} {args:?}");
    (code, serde_json::from_str(&out).unwrap())
}

fn codes(v: &Value) -> Vec<&str> {
    let all = v["warnings"].as_array().unwrap();
    all.iter().map(|w| w["code"].as_str().unwrap()).collect()
}

/// A one-page file with `cells` after the root and the default layer.
fn page(cells: &str) -> String {
    format!(
        r#"<mxfile>
  <diagram name="Page-1">
    <mxGraphModel>
      <root>
        <mxCell id="0" />
        <mxCell id="1" parent="0" />{cells}
      </root>
    </mxGraphModel>
  </diagram>
</mxfile>
"#
    )
}

/// A vertex with a label at `x`, `y`.
fn vertex(id: &str, label: &str, x: u32, y: u32, extra: &str) -> String {
    format!(
        r#"
        <mxCell id="{id}" value="{label}" style="whiteSpace=wrap;html=1;" vertex="1" parent="1"{extra}>
          <mxGeometry x="{x}" y="{y}" width="120" height="60" as="geometry" />
        </mxCell>"#
    )
}

#[test]
fn a_file_with_three_pages_renders_the_first_or_the_one_asked_for() {
    let dir = folder("drawio-pages");
    let source = fs::read(fixture("pages.drawio")).unwrap();
    let (code, v) = in_dir(&dir, "pages.drawio", &source, &[]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        (&v["format"], &v["diagram_type"], &v["output"]),
        (&json!("drawio"), &Value::Null, &json!("pages.svg"))
    );
    assert_eq!(
        v["engine"],
        json!({"name": "diagram-render", "version": "0.1.0"})
    );
    assert_eq!(v["page"], json!({"index": 1, "name": "Plan"}));
    assert_eq!(
        v["pages"],
        json!([{"index": 1, "name": "Plan"}, {"index": 2, "name": "Build"}, {"index": 3, "name": "Ship"}])
    );
    assert_eq!(
        v["warnings"],
        json!([{"code": "pages_more", "message": "pages.drawio has 3 pages; rendered page 1 (Plan). Use --page for another."}])
    );
    // A number, a name or an id; the second page is compressed.
    for page in ["2", "Build", "p2"] {
        let (code, v) = in_dir(&dir, "pages.drawio", &source, &["--page", page]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["page"], json!({"index": 2, "name": "Build"}));
        assert_eq!(
            (&v["output"], codes(&v)),
            (&json!("pages-Build.svg"), vec![])
        );
    }
    let svg = fs::read_to_string(dir.join("pages-Build.svg")).unwrap();
    assert!(svg.contains(">Build</tspan>") && svg.contains(">Test</tspan>"));
    let (code, v) = in_dir(&dir, "pages.drawio", &source, &["--page", "Deploy"]);
    assert_eq!(code, 1);
    assert_eq!(
        (&v["error"]["code"], &v["error"]["message"]),
        (
            &json!("page_not_found"),
            &json!("pages.drawio has no page Deploy; it has: Plan, Build, Ship.")
        )
    );
    let (code, v) = json(&["check", &fixture("pages.drawio"), "--page", "4", "--json"]);
    assert_eq!(
        (code, &v["errors"][0]["code"]),
        (1, &json!("page_not_found"))
    );
    let (code, v) = json(&["check", &fixture("pages.drawio"), "--json"]);
    assert_eq!((code, &v["valid"], codes(&v)), (0, &json!(true), vec![]));
}

#[test]
fn svg_and_png_files_carrying_a_diagram_render_it_again() {
    let dir = folder("drawio-carried");
    let source = fs::read(fixture("pages.drawio")).unwrap();
    in_dir(&dir, "pages.drawio", &source, &["-o", "plain.svg"]);
    let want = fs::read_to_string(dir.join("plain.svg")).unwrap();
    for file in ["board.drawio.svg", "board.drawio.png", "zipped.drawio.png"] {
        let source = fs::read(fixture(file)).unwrap();
        let (code, v) = in_dir(&dir, file, &source, &["--page", "Build"]);
        assert_eq!((code, &v["format"]), (0, &json!("drawio")), "{file}: {v}");
        assert_eq!(
            v["output"],
            "board-Build.svg".replace("board", file.split('.').next().unwrap())
        );
        let (_, v) = in_dir(&dir, file, &source, &["-o", "out.svg"]);
        assert_eq!(v["page"], json!({"index": 1, "name": "Plan"}), "{file}");
        assert_eq!(
            fs::read_to_string(dir.join("out.svg")).unwrap(),
            want,
            "{file}"
        );
    }
    // Beside the input, `.drawio.png` gives way to `.svg`.
    let source = fs::read(fixture("board.drawio.png")).unwrap();
    let (_, v) = in_dir(&dir, "board.drawio.png", &source, &[]);
    assert_eq!(v["output"], "board.svg");
    // A PNG with no diagram in it is only an image.
    let pixel = fs::read("tests/fixtures/d2/pixel.png").unwrap();
    let (code, v) = in_dir(&dir, "plain.drawio.png", &pixel, &[]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &json!("unsupported_input"))
    );
}

#[test]
fn xml_errors_point_to_their_line_and_column_with_any_line_ends() {
    let lf = fs::read_to_string(fixture("broken.drawio")).unwrap();
    let crlf = format!("\u{FEFF}{}", lf.replace('\n', "\r\n"));
    let dir = folder("drawio-broken");
    for (file, source) in [("lf.drawio", lf), ("crlf.drawio", crlf)] {
        for command in [&["check"][..], &[]] {
            fs::write(dir.join(file), &source).unwrap();
            let (code, out, _) = run_in(&dir, &[command, &[file, "--json"]].concat(), b"");
            assert_eq!(code, 1, "{out}");
            let v: Value = serde_json::from_str(&out).unwrap();
            let e = match command.is_empty() {
                true => &v["error"],
                false => &v["errors"][0],
            };
            assert_eq!(e["code"], "syntax_error");
            assert_eq!(e["message"], "expected 'mxCell' tag, not 'mxCel'");
            // Column 60 counts characters: é and 漢 are one each.
            assert_eq!(
                (&e["line"], &e["column"]),
                (&json!(4), &json!(60)),
                "{file}"
            );
        }
    }
    // Inside a compressed page, the error is at the page's line, naming it.
    let (code, v) = json(&[
        "check",
        &fixture("broken-packed.drawio"),
        "--page",
        "Broken",
        "--json",
    ]);
    assert_eq!(code, 1);
    let e = &v["errors"][0];
    assert_eq!(
        (&e["code"], &e["message"], &e["line"], &e["column"]),
        (
            &json!("syntax_error"),
            &json!("expected 'mxCell' tag, not 'root' in page \"Broken\""),
            &json!(5),
            &Value::Null
        )
    );
    let (code, v) = json(&["check", &fixture("broken-packed.drawio"), "--json"]);
    assert_eq!((code, &v["valid"]), (0, &json!(true)));
}

#[test]
fn hidden_layers_cells_and_collapsed_children_arent_drawn() {
    let cells = format!(
        r#"{}{}
        <mxCell id="hidden" parent="0" visible="0" />
        <mxCell id="h1" value="In a hidden layer" vertex="1" parent="hidden">
          <mxGeometry x="0" y="200" width="80" height="40" as="geometry" />
        </mxCell>
        <mxCell id="group" value="Folded" style="swimlane;" vertex="1" collapsed="1" parent="1">
          <mxGeometry x="0" y="100" width="120" height="30" as="geometry" />
        </mxCell>
        <mxCell id="g1" value="Inside" vertex="1" parent="group">
          <mxGeometry x="10" y="40" width="80" height="40" as="geometry" />
        </mxCell>
        <mxCell id="e1" edge="1" parent="1" source="a" target="g1">
          <mxGeometry relative="1" as="geometry" />
        </mxCell>"#,
        vertex("a", "Shown", 0, 0, ""),
        vertex("b", "Gone", 200, 0, r#" visible="0""#),
    );
    let dir = folder("drawio-hidden");
    let (code, v) = in_dir(&dir, "h.drawio", page(&cells).as_bytes(), &[]);
    assert_eq!((code, codes(&v)), (0, vec![]), "{v}");
    let svg = fs::read_to_string(dir.join("h.svg")).unwrap();
    assert!(svg.contains(">Shown<") && svg.contains(">Folded<"), "{svg}");
    for gone in ["Gone", "hidden layer", "Inside"] {
        assert!(!svg.contains(gone), "{gone}: {svg}");
    }
    // The edge to a folded cell's child ends on the folded cell.
    assert_eq!((&v["width"], &v["height"]), (&json!(122), &json!(132)));
}

#[test]
fn cells_with_missing_parents_or_terminals_are_reported_with_their_line() {
    let cells = format!(
        r#"{}
        <mxCell id="orphan" value="Orphan" vertex="1" parent="nowhere">
          <mxGeometry x="0" y="100" width="80" height="40" as="geometry" />
        </mxCell>
        <mxCell id="dangling" edge="1" parent="1" source="a" target="gone">
          <mxGeometry relative="1" as="geometry" />
        </mxCell>
        <mxCell id="loose" edge="1" parent="1" source="a" target="gone">
          <mxGeometry relative="1" as="geometry">
            <mxPoint x="300" y="30" as="targetPoint" />
          </mxGeometry>
        </mxCell>"#,
        vertex("a", "A", 0, 0, ""),
    );
    let dir = folder("drawio-missing");
    for command in [&["check"][..], &[]] {
        fs::write(dir.join("m.drawio"), page(&cells)).unwrap();
        let (code, out, _) = run_in(&dir, &[command, &["m.drawio", "--json"]].concat(), b"");
        assert_eq!(code, 0, "{out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["warnings"],
            json!([
                {"code": "parent_missing", "message": "Cell orphan points to a missing parent nowhere; drew it at the top level.", "line": 10},
                {"code": "edge_skipped", "message": "Skipped edge dangling: its target gone isn't on the page.", "line": 13},
            ])
        );
    }
    let svg = fs::read_to_string(dir.join("m.svg")).unwrap();
    assert!(svg.contains(">Orphan<"), "{svg}");
    // The edge with a point to fall back on ends there.
    assert!(svg.contains(",300,30") || svg.contains(" 300,30"), "{svg}");
}

#[test]
fn a_page_over_the_cell_limit_is_refused() {
    let cells: String = (0..50_000)
        .map(|i| format!(r#"<mxCell id="c{i}" vertex="1" parent="1"/>"#))
        .collect();
    let dir = folder("drawio-limit");
    let (code, v) = in_dir(&dir, "big.drawio", page(&cells).as_bytes(), &[]);
    assert_eq!(code, 1);
    assert_eq!(
        (&v["error"]["code"], &v["error"]["message"]),
        (
            &json!("too_many_elements"),
            &json!("This diagram has 50002 elements; the limit is 50000.")
        )
    );
}

/// `data` compressed with raw deflate or zlib.
fn deflate(data: &[u8], zlib: bool) -> Vec<u8> {
    let level = flate2::Compression::best();
    match zlib {
        true => {
            let mut e = flate2::write::ZlibEncoder::new(vec![], level);
            e.write_all(data).unwrap();
            e.finish().unwrap()
        }
        false => {
            let mut e = flate2::write::DeflateEncoder::new(vec![], level);
            e.write_all(data).unwrap();
            e.finish().unwrap()
        }
    }
}

#[test]
fn a_compressed_page_or_png_chunk_that_unpacks_too_far_is_refused() {
    use base64::Engine;
    let bomb = vec![b'A'; (256 << 20) + 1];
    let dir = folder("drawio-bomb");
    let packed = base64::engine::general_purpose::STANDARD.encode(deflate(&bomb, false));
    let source = format!(r#"<mxfile><diagram name="Bomb">{packed}</diagram></mxfile>"#);
    let (code, v) = in_dir(&dir, "bomb.drawio", source.as_bytes(), &[]);
    assert_eq!(code, 1);
    let want = json!({"code": "too_large", "message": "bomb.drawio unpacks to over 256 MiB; the limit is 256 MiB.", "hint": "Split the diagram, or remove embedded images.", "line": null, "column": null, "excerpt": null});
    assert_eq!(v["error"], want);
    // A zTXt chunk in a PNG.
    let png = fs::read(fixture("zipped.drawio.png")).unwrap();
    let at = png.windows(4).position(|w| w == b"zTXt").unwrap() - 4;
    let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
    let mut data = b"mxfile\0\0".to_vec();
    data.extend(deflate(&bomb, true));
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    chunk.extend(b"zTXt");
    chunk.extend(&data);
    chunk.extend([0; 4]);
    let png = [&png[..at], &chunk, &png[at + len + 12..]].concat();
    let (code, v) = in_dir(&dir, "bomb.drawio.png", &png, &[]);
    assert_eq!(code, 1);
    assert_eq!(
        v["error"]["message"],
        "bomb.drawio.png unpacks to over 256 MiB; the limit is 256 MiB."
    );
}

#[test]
fn the_page_background_fills_the_drawing_unless_another_is_asked_for() {
    let source = page(&vertex("a", "A", 0, 0, ""))
        .replace("<mxGraphModel>", r##"<mxGraphModel background="#FFF2CC">"##);
    let dir = folder("drawio-background");
    for (args, want) in [
        (&[][..], Some("#FFF2CC")),
        (&["--background", "#369"][..], Some("#336699")),
        (&["--background", "transparent"][..], None),
    ] {
        let (code, _) = in_dir(&dir, "b.drawio", source.as_bytes(), args);
        assert_eq!(code, 0);
        let svg = fs::read_to_string(dir.join("b.svg")).unwrap();
        let first = svg.lines().nth(2).unwrap();
        match want {
            Some(fill) => assert_eq!(
                first,
                format!(r#"<rect x="-1" y="-1" width="122" height="62" fill="{fill}"/>"#)
            ),
            None => assert!(!first.contains("122"), "{first}"),
        }
    }
    let (_, v) = in_dir(
        &dir,
        "w.drawio",
        page(&vertex("a", "A", 0, 0, "")).as_bytes(),
        &[],
    );
    let svg = fs::read_to_string(dir.join("w.svg")).unwrap();
    assert!(
        svg.lines().nth(2).unwrap().contains(r#"fill="white""#),
        "{v}"
    );
}
