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
    // The edge with a point to fall back on ends there, less its arrow.
    assert!(svg.contains(r#"<path d="M120 30 L293.63 30""#), "{svg}");
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

/// The fixtures that show every shape, style and container.
const FIXTURES: [&str; 6] = [
    "shapes",
    "styles",
    "containers",
    "images",
    "edges",
    "arrows",
];

#[test]
fn every_fixture_renders_to_svg_and_png_at_the_svg_size() {
    let dir = folder("drawio-fixtures");
    for name in FIXTURES {
        let file = format!("{name}.drawio");
        let source = fs::read(fixture(&file)).unwrap();
        let (code, svg) = in_dir(&dir, &file, &source, &[]);
        let quiet = if name == "images" {
            vec!["remote_image_not_fetched"]
        } else {
            vec![]
        };
        assert_eq!((code, codes(&svg)), (0, vec![]), "{name}: {svg}");
        for scale in [1.0, 2.0] {
            let s = scale.to_string();
            let (code, png) = in_dir(&dir, &file, &source, &["--to", "png", "--scale", &s]);
            assert_eq!((code, codes(&png)), (0, quiet.clone()), "{name}: {png}");
            for k in ["width", "height"] {
                let want = svg[k].as_f64().unwrap() * scale;
                assert!(
                    (png[k].as_f64().unwrap() - want).abs() <= 1.0,
                    "{name} {k}: {png}"
                );
            }
        }
    }
}

/// The `fixture` rendered to SVG with `args`.
fn svg_of(file: &str, args: &[&str]) -> String {
    let dir = folder(&format!("drawio-svg-{}", file.replace('.', "-")));
    let source = fs::read(fixture(file)).unwrap();
    let (code, v) = in_dir(&dir, file, &source, &[&["-o", "out.svg"], args].concat());
    assert_eq!(code, 0, "{v}");
    fs::read_to_string(dir.join("out.svg")).unwrap()
}

#[test]
fn styles_fill_stroke_turn_and_shade_shapes() {
    let svg = svg_of("styles.drawio", &[]);
    for want in [
        r##"fill="#dae8fc" stroke="#6c8ebf""##,
        r##"<linearGradient id="g1" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#dae8fc"/><stop offset="100%" stop-color="#7ea6e0"/>"##,
        r#"x1="0" y1="0" x2="1" y2="0""#,
        r#"stroke-width="2" stroke-dasharray="6 6""#,
        r#"stroke-dasharray="1 4""#,
        r#"filter="url(#shadow)""#,
        r#"<g opacity="0.4">"#,
        r##"fill-opacity="0.3" stroke="#9673a6" stroke-opacity="0.6""##,
        r#"transform="rotate(30 "#,
        r#"scale(-1 1)"#,
        r#"rx="12""#,
        r##"fill="#ffe6cc""##,
        r#"<a xlink:href="https://example.org/">"#,
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
}

#[test]
fn tables_draw_lines_between_rows_and_cells() {
    let svg = svg_of("containers.drawio", &[]);
    assert!(
        svg.contains(r##"<path d="M300 260 L540 260 M380 220 L380 300 M460 220 L460 300" fill="none" stroke="#000000"/>"##),
        "{svg}"
    );
    // A swimlane down the side has its header on the left and its body filled.
    assert!(
        svg.contains(r#"<path d="M330 20 L300 20 L300 180 L330 180 Z""#),
        "{svg}"
    );
    assert!(
        svg.contains(r##"fill="#f5f5f5" stroke="#000000""##),
        "{svg}"
    );
}

#[test]
fn vendor_shapes_and_stencils_are_drawn_as_dashed_boxes() {
    let dir = folder("drawio-vendor");
    let source = fs::read(fixture("vendor.drawio")).unwrap();
    let (code, v) = in_dir(&dir, "vendor.drawio", &source, &[]);
    assert_eq!(code, 0);
    assert_eq!(
        v["warnings"],
        json!([{"code": "shape_approximated", "message": "Drew 3 shapes as plain boxes: mxgraph.aws4.resourceIcon, mxgraph.aws4.bucket, stencil."}])
    );
    let svg = fs::read_to_string(dir.join("vendor.svg")).unwrap();
    assert_eq!(svg.matches(r#"stroke-dasharray="4 4""#).count(), 3, "{svg}");
}

#[test]
fn remote_images_are_links_in_svg_and_placeholders_in_png() {
    let svg = svg_of("images.drawio", &[]);
    assert!(
        svg.contains(r#"xlink:href="https://example.org/logo.png""#),
        "{svg}"
    );
    // draw.io leaves `;base64` out of data URLs in styles.
    assert!(
        svg.contains(r#"xlink:href="data:image/png;base64,iVBOR"#),
        "{svg}"
    );
    let dir = folder("drawio-remote");
    let source = fs::read(fixture("images.drawio")).unwrap();
    let (code, v) = in_dir(&dir, "images.drawio", &source, &["--to", "png"]);
    assert_eq!(code, 0);
    assert_eq!(
        v["warnings"],
        json!([{"code": "remote_image_not_fetched", "message": "Image https://example.org/logo.png isn't fetched (the tool works offline); drew a placeholder."}])
    );
}

#[test]
fn edges_route_as_draw_io_routes_them() {
    let svg = svg_of("edges.drawio", &[]);
    for want in [
        // Orthogonal: out of the side and into the top, the end cut back for its arrow.
        r#"<path d="M540 45 L710 45 L710 73.63""#,
        // Rounded corners, a waypoint the segments turn at, and a fixed exit and entry.
        r#"<path d="M120 185 L280 185 Q290 185 290 195 L290 213.63""#,
        r#"<path d="M490 210 L490 280 L600 280 L600 245 L653.63 245""#,
        r#"<path d="M490 630 L490 677.5 L653.63 677.5""#,
        // Entity relation: out, across and in, with its crow's feet.
        r#"<path d="M540 465 L570 465 L630 525 L644.5 525""#,
        // A curve through the waypoint, and lines ending on an ellipse and a rhombus.
        r#"<path d="M98.95 630 Q180 700 233.93 682.84""#,
        r#"<path d="M103.03 810.03 L143.03 845.76""#,
        r#"<path d="M303.53 801.18 L353.04 845.74""#,
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
}

#[test]
fn unknown_edge_styles_and_arrows_are_drawn_as_the_nearest_known() {
    let cells = format!(
        r#"{}{}
        <mxCell id="e1" style="edgeStyle=isometricEdgeStyle;endArrow=fancyArrow;" edge="1" parent="1" source="a" target="b">
          <mxGeometry relative="1" as="geometry" />
        </mxCell>
        <mxCell id="e2" style="edgeStyle=isometricEdgeStyle;startArrow=fancyArrow;" edge="1" parent="1" source="b" target="a">
          <mxGeometry relative="1" as="geometry" />
        </mxCell>"#,
        vertex("a", "A", 0, 0, ""),
        vertex("b", "B", 300, 100, ""),
    );
    let dir = folder("drawio-unknown-edges");
    let (code, v) = in_dir(&dir, "u.drawio", page(&cells).as_bytes(), &[]);
    assert_eq!(code, 0);
    assert_eq!(
        v["warnings"],
        json!([
            {"code": "edge_style_approximated", "message": "Drew isometricEdgeStyle as straight lines."},
            {"code": "arrow_approximated", "message": "Drew fancyArrow as classic."},
        ])
    );
}

#[test]
fn labels_are_laid_out_where_draw_io_puts_them() {
    let svg = svg_of("labels.drawio", &[]);
    for want in [
        // Wrapped to the box less its spacing, each line centred.
        r#"<tspan x="36.97" y="182.56">A long label that</tspan><tspan x="38.98" y="196.96">wraps inside its</tspan>"#,
        // Aligned to a corner, 2 px in (and 5 more from the top), or outside the cell.
        r#"<tspan x="342" y="178.36">Top left</tspan>"#,
        r#"<tspan x="553.31" y="213.96">Bottom right</tspan>"#,
        r#"<tspan x="33.66" y="338.36">Below</tspan>"#,
        r#"<tspan x="202" y="304.16">Right of it</tspan>"#,
        // List markers before text 1.75 em in, and paragraphs 1 em apart.
        r#"<tspan x="552.13" y="50.36">•  </tspan><tspan x="563" y="50.36">Apples</tspan>"#,
        r#"<tspan x="366.98" y="85.16">A paragraph</tspan>"#,
        // Inline looks, a font tag and a coloured span.
        r#"<tspan font-weight="bold">Bold</tspan>"#,
        r#"<tspan font-size="9.96" baseline-shift="sub">2</tspan>"#,
        r#"<tspan text-decoration="line-through">struck</tspan>"#,
        r##"<tspan font-family="'Courier New', Helvetica, Arial, 'Liberation Sans', sans-serif" font-size="18" fill="#0000ff">Font tag</tspan>"##,
        r#"fill="rgb(255, 0, 0)">Red 16 px</tspan>"#,
        // Edge labels on a white box, one along the route, and a lane's name read upwards.
        r#"<tspan x="198.69" y="403.81">says</tspan>"#,
        r#"<tspan x="127.59" y="492.81">near the start</tspan>"#,
        r#"transform="rotate(-90 "#,
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
}

#[test]
fn html_outside_the_subset_is_text_and_scripts_are_left_out() {
    let label = "&lt;img src=x onerror=alert(1)&gt;Safe &lt;table&gt;&lt;tr&gt;&lt;td&gt;cell&lt;/td&gt;&lt;/tr&gt;&lt;/table&gt;";
    let cells = format!(
        r#"
        <mxCell id="a" value="{label}" style="html=1;" vertex="1" parent="1">
          <mxGeometry x="0" y="0" width="120" height="60" as="geometry" />
        </mxCell>"#
    );
    let dir = folder("drawio-unsafe");
    let (code, v) = in_dir(&dir, "x.drawio", page(&cells).as_bytes(), &[]);
    assert_eq!(code, 0);
    assert_eq!(codes(&v), ["html_simplified", "unsafe_content_removed"]);
    assert_eq!(
        v["warnings"][1]["message"],
        "Removed 1 unsafe item (scripts, event handlers or javascript: links) from the SVG."
    );
    let svg = fs::read_to_string(dir.join("x.svg")).unwrap();
    assert!(svg.contains(">Safe cell</tspan>"), "{svg}");
    assert!(!svg.contains("onerror") && !svg.contains("img"), "{svg}");
}

#[test]
fn pngs_say_which_fonts_were_missing_or_came_from_the_computer() {
    let cells = format!(
        r#"{}{}"#,
        vertex("a", "Missing font", 0, 0, ""),
        vertex("b", "漢字 label", 200, 0, ""),
    )
    .replace(
        r#"style="whiteSpace=wrap;html=1;" vertex="1" parent="1">
          <mxGeometry x="0""#,
        r#"style="whiteSpace=wrap;html=1;fontFamily=No Such Font;" vertex="1" parent="1">
          <mxGeometry x="0""#,
    );
    let dir = folder("drawio-fonts");
    let (code, v) = in_dir(&dir, "f.drawio", page(&cells).as_bytes(), &["--to", "png"]);
    assert_eq!(code, 0);
    assert_eq!(codes(&v), ["font_substituted", "font_fallback"]);
    assert_eq!(
        v["warnings"][0]["message"],
        "Font No Such Font isn't available; drew its text with Liberation Sans."
    );
}

/// The characters of each `<text>` in an SVG, without spaces or list markers: the reference's
/// lines break and space where the browser's layout does.
fn texts(svg: &str) -> Vec<String> {
    let doc = roxmltree::Document::parse(svg).unwrap();
    let marker = |t: &str| {
        let t = t.trim();
        t == "•"
            || t.strip_suffix('.')
                .is_some_and(|n| n.parse::<u32>().is_ok())
    };
    (doc.descendants().filter(|n| n.has_tag_name("text")))
        .map(|t| {
            let lines = t.children().filter(|l| {
                !marker(
                    &l.descendants()
                        .filter(|d| d.is_text())
                        .filter_map(|d| d.text())
                        .collect::<String>(),
                )
            });
            let text: String = lines
                .flat_map(|l| {
                    l.descendants()
                        .filter(|d| d.is_text())
                        .filter_map(|d| d.text())
                })
                .collect();
            text.split_whitespace().collect()
        })
        .collect()
}

#[test]
fn renders_match_the_draw_io_viewer_within_four_percent() {
    // References: draw.io's viewer (viewer-static.min.js 32.6.9) in Chromium, framed by our SVG.
    let labels: Value =
        serde_json::from_str(&fs::read_to_string(fixture("refs/labels.json")).unwrap()).unwrap();
    let dir = folder("drawio-references");
    for (name, want) in labels.as_object().unwrap() {
        let file = format!("{name}.drawio");
        let source = fs::read(fixture(&file)).unwrap();
        assert_eq!(
            in_dir(&dir, &file, &source, &["-o", "out.svg"]).0,
            0,
            "{name}"
        );
        let ours = texts(&fs::read_to_string(dir.join("out.svg")).unwrap());
        for label in want.as_array().unwrap() {
            let label: String = label.as_str().unwrap().split_whitespace().collect();
            assert!(ours.contains(&label), "{name}: {label} not in {ours:?}");
        }
        let args = ["-o", "out.png", "--scale", "1"];
        assert_eq!(in_dir(&dir, &file, &source, &args).0, 0, "{name}");
        let ours = fs::read(dir.join("out.png")).unwrap();
        let reference = fs::read(fixture(&format!("refs/{name}.png"))).unwrap();
        let difference = blurred_difference(&ours, &reference, name);
        assert!(difference <= 0.04, "{name}: {:.2} %", difference * 100.0);
    }
}
