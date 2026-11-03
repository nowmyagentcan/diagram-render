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
