//! BPMN: read from the file's diagram section and drawn by this binary.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use serde_json::{Value, json};

/// `tests/fixtures/bpmn/{file}`.
fn fixture(file: &str) -> String {
    Path::new("tests/fixtures/bpmn")
        .join(file)
        .to_string_lossy()
        .into_owned()
}

/// A process with one shape and one edge, and `extra` inside the plane.
fn process(extra: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" xmlns:bpmndi="http://www.omg.org/spec/BPMN/20100524/DI" xmlns:dc="http://www.omg.org/spec/DD/20100524/DC" xmlns:di="http://www.omg.org/spec/DD/20100524/DI" id="d">
  <process id="p">
    <task id="t" name="Task" />
    <endEvent id="e" />
    <sequenceFlow id="f" sourceRef="t" targetRef="e" />
  </process>
  <bpmndi:BPMNDiagram id="diagram">
    <bpmndi:BPMNPlane id="plane" bpmnElement="p">
      <bpmndi:BPMNShape id="t_di" bpmnElement="t">
        <dc:Bounds x="100" y="100" width="100" height="80" />
      </bpmndi:BPMNShape>
      <bpmndi:BPMNShape id="e_di" bpmnElement="e">
        <dc:Bounds x="250" y="122" width="36" height="36" />
      </bpmndi:BPMNShape>
      <bpmndi:BPMNEdge id="f_di" bpmnElement="f">
        <di:waypoint x="200" y="140" />
        <di:waypoint x="250" y="140" />
      </bpmndi:BPMNEdge>{extra}
    </bpmndi:BPMNPlane>
  </bpmndi:BPMNDiagram>
</definitions>
"#
    )
}

#[test]
fn a_process_renders_to_svg_framed_like_bpmn_js() {
    let dir = folder("bpmn-process");
    let (code, v) = in_dir(&dir, "p.bpmn", process("").as_bytes(), &[]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        (&v["format"], &v["diagram_type"], &v["output"]),
        (&json!("bpmn"), &json!("process"), &json!("p.svg"))
    );
    assert_eq!(
        v["engine"],
        json!({"name": "diagram-render", "version": "0.1.0"})
    );
    assert_eq!(v["page"], json!({"index": 1, "name": "diagram"}));
    assert_eq!(v["pages"], json!([{"index": 1, "name": "diagram"}]));
    // Shapes from x 100 to 286, y 100 to 180, plus 5 px.
    assert_eq!((&v["width"], &v["height"]), (&json!(196), &json!(90)));
    let svg = fs::read_to_string(dir.join("p.svg")).unwrap();
    assert!(svg.contains(r#"viewBox="95 95 196 90""#), "{svg}");
    let (code, v) = json(&["check", &fixture("pages.bpmn"), "--json"]);
    assert_eq!((code, &v["valid"], codes(&v)), (0, &json!(true), vec![]));
}

#[test]
fn a_file_with_two_diagrams_renders_the_first_or_the_one_asked_for() {
    let dir = folder("bpmn-pages");
    let source = fs::read(fixture("pages.bpmn")).unwrap();
    let (code, v) = in_dir(&dir, "pages.bpmn", &source, &[]);
    assert_eq!(
        (code, &v["page"]),
        (0, &json!({"index": 1, "name": "Order"}))
    );
    assert_eq!(
        v["pages"],
        json!([{"index": 1, "name": "Order"}, {"index": 2, "name": "Shipping"}])
    );
    assert_eq!(
        v["warnings"],
        json!([{"code": "pages_more", "message": "pages.bpmn has 2 pages; rendered page 1 (Order). Use --page for another."}])
    );
    // A number, a name or an id; beside the input, the page's name goes in the file name.
    for page in ["2", "Shipping", "ship_diagram"] {
        let (code, v) = in_dir(&dir, "pages.bpmn", &source, &["--page", page]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["page"], json!({"index": 2, "name": "Shipping"}));
        assert_eq!(
            (&v["output"], codes(&v)),
            (&json!("pages-Shipping.svg"), vec![])
        );
    }
    let (code, v) = in_dir(
        &dir,
        "pages.bpmn",
        &source,
        &["--page", "2", "-o", "two.png"],
    );
    assert_eq!((code, &v["output"]), (0, &json!("two.png")));
    let (code, v) = in_dir(&dir, "pages.bpmn", &source, &["--page", "Billing"]);
    assert_eq!(code, 1);
    assert_eq!(
        v["error"],
        json!({"code": "page_not_found", "message": "pages.bpmn has no page Billing; it has: Order, Shipping.", "hint": "Pass --page with one of those, or leave it out for the first.", "line": null, "column": null, "excerpt": null})
    );
    let (code, v) = json(&["check", &fixture("pages.bpmn"), "--page", "3", "--json"]);
    assert_eq!(
        (code, &v["errors"][0]["code"]),
        (1, &json!("page_not_found"))
    );
}

#[test]
fn xml_errors_point_to_their_line_and_column_with_any_line_ends() {
    let lf = fs::read_to_string(fixture("broken.bpmn")).unwrap();
    let crlf = format!("\u{FEFF}{}", lf.replace('\n', "\r\n"));
    let dir = folder("bpmn-broken");
    for (file, source) in [("lf.bpmn", lf), ("crlf.bpmn", crlf)] {
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
            assert_eq!(e["message"], "expected 'bpmn:task' tag, not 'bpmn:tsk'");
            // Column 39 counts characters: é and 漢 are one each.
            assert_eq!(
                (&e["line"], &e["column"]),
                (&json!(4), &json!(39)),
                "{file}"
            );
            assert_eq!(
                e["excerpt"],
                "    <bpmn:task id=\"t1\" name=\"Café 漢字\"></bpmn:tsk>"
            );
        }
    }
    let (code, out, err) = run(&["check", &fixture("broken.bpmn")]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.contains(":4:39: expected 'bpmn:task' tag"), "{err}");
}

#[test]
fn entities_from_outside_or_without_end_fail_cleanly() {
    let (code, v) = json(&[&fixture("xxe.bpmn"), "-o", "never.svg", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(
        (
            &v["error"]["code"],
            &v["error"]["message"],
            &v["error"]["line"]
        ),
        (
            &json!("syntax_error"),
            &json!("unknown entity reference 'secret'"),
            &json!(7)
        )
    );
    let (code, v) = json(&[&fixture("laughs.bpmn"), "-o", "never.svg", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(
        (&v["error"]["code"], &v["error"]["message"]),
        (
            &json!("syntax_error"),
            &json!("a possible entity reference loop is detected")
        )
    );
    assert!(!Path::new("never.svg").exists());
}

#[test]
fn a_file_without_a_diagram_section_isnt_laid_out() {
    let source = r#"<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL"><process id="p"><task id="t"/></process></definitions>"#;
    let dir = folder("bpmn-no-layout");
    let (code, v) = in_dir(&dir, "n.bpmn", source.as_bytes(), &[]);
    assert_eq!(code, 1);
    assert_eq!(
        (
            &v["error"]["code"],
            &v["error"]["message"],
            &v["error"]["hint"]
        ),
        (
            &json!("no_diagram_layout"),
            &json!("This BPMN file has no diagram layout (no BPMNDiagram section)."),
            &json!("Open it in a BPMN modeler (such as Camunda Modeler) and save it to add one.")
        )
    );
    let (code, _, err) = run_in(&dir, &["n.bpmn"], b"");
    assert_eq!(code, 1);
    assert_eq!(
        err,
        "✗ n.bpmn: This BPMN file has no diagram layout (no BPMNDiagram section).\n  Open it in a BPMN modeler (such as Camunda Modeler) and save it to add one.\n"
    );
}

#[test]
fn diagram_items_that_cant_be_drawn_are_skipped_with_their_line() {
    let extra = r#"
      <bpmndi:BPMNShape id="ghost_di" bpmnElement="ghost">
        <dc:Bounds x="0" y="0" width="10" height="10" />
      </bpmndi:BPMNShape>
      <bpmndi:BPMNEdge id="short_di" bpmnElement="f">
        <di:waypoint x="200" y="140" />
      </bpmndi:BPMNEdge>
      <bpmndi:BPMNShape id="bare_di" bpmnElement="t" />"#;
    let dir = folder("bpmn-skipped");
    let source = process(extra);
    for command in [&["check"][..], &[]] {
        fs::write(dir.join("s.bpmn"), &source).unwrap();
        let (code, out, _) = run_in(&dir, &[command, &["s.bpmn", "--json"]].concat(), b"");
        assert_eq!(code, 0, "{out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["warnings"],
            json!([
                {"code": "di_reference_missing", "message": "Diagram shape ghost_di points to ghost, which isn't in the model.", "line": 20},
                {"code": "edge_skipped", "message": "Skipped edge short_di: it has fewer than 2 waypoints.", "line": 23},
                {"code": "element_skipped", "message": "Skipped shape bare_di: it has no bounds.", "line": 26},
            ])
        );
    }
    let (_, _, err) = run_in(&dir, &["s.bpmn"], b"");
    assert!(
        err.contains("! s.bpmn:20: Diagram shape ghost_di points to ghost"),
        "{err}"
    );
}

#[test]
fn a_diagram_over_the_element_limit_is_refused() {
    let shapes: String = (0..20_001)
        .map(|i| format!(r#"<bpmndi:BPMNShape id="s{i}" bpmnElement="t"><dc:Bounds x="0" y="0" width="1" height="1"/></bpmndi:BPMNShape>"#))
        .collect();
    let dir = folder("bpmn-limit");
    let (code, v) = in_dir(&dir, "big.bpmn", process(&shapes).as_bytes(), &[]);
    assert_eq!(code, 1);
    assert_eq!(
        (&v["error"]["code"], &v["error"]["message"]),
        (
            &json!("too_many_elements"),
            &json!("This diagram has 20004 elements; the limit is 20000.")
        )
    );
}

#[test]
fn backgrounds_replace_the_white_one() {
    let dir = folder("bpmn-background");
    let source = process("");
    for (args, want) in [
        (&[][..], Some("white")),
        (&["--background", "#369"][..], Some("#336699")),
        (&["--background", "transparent"][..], None),
    ] {
        let (code, _) = in_dir(&dir, "b.bpmn", source.as_bytes(), args);
        assert_eq!(code, 0);
        let svg = fs::read_to_string(dir.join("b.svg")).unwrap();
        let first = svg.lines().nth(2).unwrap();
        match want {
            Some(fill) => assert_eq!(
                first,
                format!(r#"<rect x="95" y="95" width="196" height="90" fill="{fill}"/>"#)
            ),
            None => assert!(!first.contains("95"), "{first}"),
        }
    }
}

/// The fixtures that show every element, marker and edge kind.
const FIXTURES: [&str; 6] = [
    "activities",
    "events",
    "gateways-data",
    "collaboration",
    "vertical",
    "colors",
];

#[test]
fn every_fixture_renders_to_svg_and_png_at_the_svg_size() {
    let dir = folder("bpmn-fixtures");
    for name in FIXTURES {
        let file = format!("{name}.bpmn");
        let source = fs::read(fixture(&file)).unwrap();
        let (code, svg) = in_dir(&dir, &file, &source, &[]);
        assert_eq!((code, codes(&svg)), (0, vec![]), "{name}: {svg}");
        for scale in [1.0, 2.0] {
            let s = scale.to_string();
            let (code, png) = in_dir(&dir, &file, &source, &["--to", "png", "--scale", &s]);
            assert_eq!((code, codes(&png)), (0, vec![]), "{name}: {png}");
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
    let dir = folder(&format!("bpmn-svg-{}", file.replace('.', "-")));
    let source = fs::read(fixture(file)).unwrap();
    let (code, v) = in_dir(&dir, file, &source, &[&["-o", "out.svg"], args].concat());
    assert_eq!(code, 0, "{v}");
    fs::read_to_string(dir.join("out.svg")).unwrap()
}

#[test]
fn labels_are_laid_out_as_bpmn_js_lays_them_out() {
    let svg = svg_of("activities.bpmn", &[]);
    // Centred in the box less 7 px padding, with Arial's widths; the first baseline is 3/4 of a
    // 1.2 line height below the top of the text.
    assert!(
        svg.contains(r#"<tspan x="183.99" y="123.6">Plain task</tspan>"#),
        "{svg}"
    );
    assert!(svg.contains(">Approve the</tspan><tspan"), "{svg}");
    assert!(svg.contains(">Repeated sub-</tspan><tspan"), "{svg}");
    // An outside label wraps within its bounds, which grow to hold it, and frame the drawing.
    let svg = svg_of("pages.bpmn", &["--page", "2"]);
    assert!(svg.contains(r#"viewBox="130 75 456 102""#), "{svg}");
    assert!(svg.contains(">Order</tspan><tspan"), "{svg}");
}

#[test]
fn pools_and_lanes_show_their_names_in_their_headers() {
    let svg = svg_of("collaboration.bpmn", &[]);
    let text = |name: &str| {
        let end = svg.find(&format!(">{name}</tspan>")).unwrap();
        svg[..end].rsplit("<text").next().unwrap().to_string()
    };
    assert!(text("Sales").contains("rotate(-90)"), "{svg}");
    assert!(text("Shop").contains("rotate(-90)"));
    // A collapsed pool's name lies across its middle.
    assert!(!text("Customers").contains("rotate"));
    let svg = svg_of("vertical.bpmn", &[]);
    assert!(!svg.contains("rotate"), "{svg}");
}

#[test]
fn colours_from_either_extension_replace_bpmn_js_ink() {
    let svg = svg_of("colors.bpmn", &[]);
    for want in [
        r##"fill="#bbdefb" stroke="#0d4372""##,
        r##"fill="#e1bee7" stroke="#5b2a6e""##,
        r##"fill="#205022">"##,
        r##"stroke="#6b3c00""##,
    ] {
        assert!(svg.contains(want), "{want}: {svg}");
    }
    let plain = svg_of("pages.bpmn", &[]);
    assert!(plain.contains(r##"fill="white" stroke="#22242a""##));
}

#[test]
fn unknown_elements_are_drawn_as_named_boxes() {
    let source = process(
        r#"
      <bpmndi:BPMNShape id="w_di" bpmnElement="w">
        <dc:Bounds x="350" y="100" width="100" height="80" />
      </bpmndi:BPMNShape>"#,
    )
    .replace(
        r#"<endEvent id="e" />"#,
        r#"<endEvent id="e" /><choreographyTask id="w" name="Widget" />"#,
    );
    let dir = folder("bpmn-unknown");
    let (code, v) = in_dir(&dir, "u.bpmn", source.as_bytes(), &[]);
    assert_eq!(code, 0);
    assert_eq!(
        v["warnings"],
        json!([{"code": "element_approximated", "message": "Drew 1 BPMN elements as plain boxes: choreographyTask."}])
    );
    let svg = fs::read_to_string(dir.join("u.svg")).unwrap();
    assert!(svg.contains(">Widget</tspan>"), "{svg}");
}

/// The text of each `<text>` in an SVG, its lines joined by spaces.
fn texts(svg: &str) -> Vec<String> {
    let doc = roxmltree::Document::parse(svg).unwrap();
    (doc.descendants().filter(|n| n.has_tag_name("text")))
        .map(|t| {
            let lines: Vec<&str> = (t.children().filter_map(|s| s.text())).collect();
            lines
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[test]
fn renders_match_bpmn_js_within_three_percent() {
    // References: bpmn-js 18.31.1 `saveSVG` drawn at scale 1 on white in Chromium, as grey.
    let labels: Value =
        serde_json::from_str(&fs::read_to_string(fixture("refs/labels.json")).unwrap()).unwrap();
    let dir = folder("bpmn-references");
    for (name, want) in labels.as_object().unwrap() {
        let (file, page) = match name.split_once("-") {
            Some(("pages", n)) => ("pages.bpmn".to_string(), vec!["--page", n]),
            _ => (format!("{name}.bpmn"), vec![]),
        };
        let source = fs::read(fixture(&file)).unwrap();
        let args = [&["-o", "out.svg"], &page[..]].concat();
        assert_eq!(in_dir(&dir, &file, &source, &args).0, 0, "{name}");
        let ours = texts(&fs::read_to_string(dir.join("out.svg")).unwrap());
        for label in want.as_array().unwrap() {
            assert!(
                ours.contains(&label.as_str().unwrap().to_string()),
                "{name}: {label}"
            );
        }
        let args = [&["-o", "out.png", "--scale", "1"], &page[..]].concat();
        assert_eq!(in_dir(&dir, &file, &source, &args).0, 0, "{name}");
        let ours = fs::read(dir.join("out.png")).unwrap();
        let reference = fs::read(fixture(&format!("refs/{name}.png"))).unwrap();
        let difference = blurred_difference(&ours, &reference, name, 2);
        assert!(difference <= 0.03, "{name}: {:.2} %", difference * 100.0);
    }
}
