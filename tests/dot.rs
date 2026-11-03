//! Graphviz DOT through the dot engine: every layout, errors by line and column, the first of
//! several graphs, unsafe links and the time limit.

mod common;

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use serde_json::{Value, json};

/// Renders the fixture `name` from a fresh copy in a folder of its own (tests run at once): the
/// JSON result and the SVG.
fn render(name: &str, extra: &[&str]) -> (Value, String) {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let run = RUNS.fetch_add(1, Ordering::Relaxed);
    let dir = folder(&format!("dot-{name}-{run}"));
    let file = format!("{name}.dot");
    fs::copy(format!("tests/fixtures/dot/{file}"), dir.join(&file)).unwrap();
    let (code, out, err) = run_in(&dir, &[&[file.as_str(), "--json"], extra].concat(), b"");
    assert_eq!((code, err.as_str()), (0, ""), "{name}: {out}");
    let svg = fs::read_to_string(dir.join(format!("{name}.svg"))).unwrap();
    (serde_json::from_str(&out).unwrap(), svg)
}

#[test]
fn every_layout_and_feature_renders() {
    let layouts = [
        "dot",
        "neato",
        "fdp",
        "sfdp",
        "circo",
        "twopi",
        "osage",
        "patchwork",
    ];
    let names = ["clusters", "records", "html", "bgcolor"]
        .map(String::from)
        .into_iter()
        .chain(layouts.map(|l| format!("layout-{l}")));
    for name in names {
        let (v, svg) = render(&name, &[]);
        assert!(
            svg.starts_with("<?xml") && svg.trim_end().ends_with("</svg>"),
            "{name}"
        );
        assert_eq!(svg.matches("<svg").count(), 1, "{name}");
        assert!(v["width"].as_u64().unwrap() > 50, "{name}");
        let warnings = match name.as_str() {
            "layout-sfdp" => {
                json!([{"code": "engine_warning", "message": "Graphviz: sfdp laid this out without removing node overlaps (this build has no triangulation library), so nodes may overlap."}])
            }
            _ => json!([]),
        };
        assert_eq!(v["warnings"], warnings, "{name}");
        assert_eq!(
            v["diagram_type"],
            if name.starts_with("layout") {
                "graph"
            } else {
                "digraph"
            }
        );
    }
    let (_, svg) = render("bgcolor", &[]);
    assert!(svg.contains("<polygon fill=\"#fdf6e3\" stroke=\"none\""));
    let (_, svg) = render("html", &[]);
    assert!(svg.contains(">merman</text>") && svg.contains("font-weight=\"bold\""));
}

#[test]
fn a_size_in_points_is_given_in_css_pixels() {
    let (v, svg) = render("clusters", &[]);
    let pt = |key: &str| {
        let at = svg.find(&format!("{key}=\"")).unwrap() + key.len() + 2;
        svg[at..]
            .split("pt")
            .next()
            .unwrap()
            .parse::<f64>()
            .unwrap()
    };
    assert_eq!(v["width"], (pt("width") * 4.0 / 3.0).ceil() as u64);
    assert_eq!(v["height"], (pt("height") * 4.0 / 3.0).ceil() as u64);
}

#[test]
fn background_overrides_the_graphs_own() {
    for (background, polygon) in [
        ("white", Some("white")),
        ("#369", Some("#336699")),
        ("transparent", None),
    ] {
        let (_, svg) = render("bgcolor", &["--background", background]);
        assert!(!svg.contains("#fdf6e3"), "{background}");
        if let Some(fill) = polygon {
            assert!(svg.contains(&format!("<polygon fill=\"{fill}\" stroke=\"none\"")));
        }
    }
}

#[test]
fn only_the_first_of_several_graphs_is_rendered() {
    let (v, svg) = render("two-graphs", &[]);
    assert_eq!(
        v["warnings"],
        json!([{"code": "extra_graphs_ignored", "message": "The file has 2 graphs; rendered the first."}])
    );
    assert!(svg.contains("<title>first</title>") && !svg.contains("second"));
    assert_eq!(svg.matches("<?xml").count(), 1);
}

#[test]
fn a_syntax_error_names_its_line_and_the_token_s_column() {
    let dir = folder("dot-errors");
    let source = "digraph {\n  a\n  b -> ;\n}\n";
    let crlf = format!("\u{FEFF}{}", source.replace('\n', "\r\n"));
    for text in [source.to_string(), crlf] {
        fs::write(dir.join("bad.dot"), &text).unwrap();
        for args in [&["check", "bad.dot", "--json"][..], &["bad.dot", "--json"]] {
            let (code, out, _) = run_in(&dir, args, b"");
            assert_eq!(code, 1, "{args:?}");
            let v: Value = serde_json::from_str(&out).unwrap();
            let e = if args[0] == "check" {
                &v["errors"][0]
            } else {
                &v["error"]
            };
            assert_eq!(
                (
                    &e["code"],
                    &e["message"],
                    &e["line"],
                    &e["column"],
                    &e["excerpt"]
                ),
                (
                    &json!("syntax_error"),
                    &json!("syntax error in line 3 near ';'"),
                    &json!(3),
                    &json!(8),
                    &json!("  b -> ;")
                ),
                "{args:?} {text:?}"
            );
            assert_eq!(e["hint"], "Fix line 3, then run again.");
        }
    }
    let source = "digraph {\n  a [label=\"é漢\"] -> b\n}\n";
    let (_, out, _) = run_in(&dir, &["check", "-", "--json"], source.as_bytes());
    let e = &serde_json::from_str::<Value>(&out).unwrap()["errors"][0];
    assert_eq!(
        (&e["line"], &e["column"]),
        (&json!(2), &json!(18)),
        "characters, not bytes"
    );
    fs::write(
        dir.join("label.dot"),
        "digraph {\n  a\n  c [label=<x<br>>]\n}\n",
    )
    .unwrap();
    let (code, _, err) = run_in(&dir, &["label.dot"], b"");
    assert_eq!(code, 1);
    assert_eq!(
        err,
        "✗ label.dot: Graphviz couldn't render this diagram: mismatched tag in line 1.\n  Run diagram-render check label.dot, or --debug for the engine's full output.\n"
    );
    let (code, out, err) = run_in(&dir, &["check", "-"], b"digraph { a [color=nosuch] }");
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "✓ - is valid Graphviz DOT (digraph)\n",
            "! -: Graphviz: nosuch is not a known color.\n"
        )
    );
}

#[test]
fn script_links_are_removed_from_the_svg() {
    let dir = folder("dot-unsafe");
    let source =
        b"digraph { a [URL=\"javascript:alert(1)\"]; b [href=\"https://example.org/\"]; a -> b }";
    let (code, out, _) = run_in(&dir, &["-", "-o", "x.svg", "--json"], source);
    assert_eq!(code, 0);
    let svg = fs::read_to_string(dir.join("x.svg")).unwrap();
    assert!(!svg.contains("javascript:"));
    assert!(svg.contains("xlink:href=\"https://example.org/\""));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["warnings"][0]["code"], "unsafe_content_removed");
}

#[test]
fn an_image_is_linked_beside_the_svg_and_a_missing_one_is_warned_about() {
    let dir = folder("dot-image");
    // A 1x1 PNG.
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDATx\xdac\xfc\xcf\xc0\xf0\x1f\0\x05\x05\x02\x00\x5f\xc8\xf1\xd2\0\0\0\0IEND\xaeB`\x82";
    fs::write(dir.join("a.png"), png).unwrap();
    let (code, out, _) = run_in(
        &dir,
        &["-", "-o", "x.svg", "--json"],
        b"digraph { a [image=\"a.png\"] }",
    );
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["warnings"],
        json!([])
    );
    assert!(
        fs::read_to_string(dir.join("x.svg"))
            .unwrap()
            .contains("<image xlink:href=\"a.png\"")
    );
    let (code, out, _) = run_in(
        &dir,
        &["-", "-o", "x.svg", "--json"],
        b"digraph { a [image=\"gone.png\"] }",
    );
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["warnings"][1]["message"],
        "Graphviz: No or improper image=\"gone.png\" for node \"a\""
    );
}

/// A dense random graph that dot needs minutes to lay out.
fn slow_graph() -> String {
    let mut source = String::from("digraph {\n");
    let mut x: u32 = 1;
    for _ in 0..6000 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        source += &format!("n{} -> n{}\n", (x >> 4) % 3000, (x >> 16) % 3000);
    }
    source + "}\n"
}

/// Whether process `pid` still runs.
fn running(pid: &str) -> bool {
    let out = match cfg!(windows) {
        true => Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output(),
        false => Command::new("kill").args(["-0", pid]).output(),
    };
    let out = out.unwrap();
    match cfg!(windows) {
        true => String::from_utf8_lossy(&out.stdout).contains(pid),
        false => out.status.success(),
    }
}

#[test]
fn a_render_past_its_time_limit_stops_its_engine() {
    let dir = folder("dot-timeout");
    fs::write(dir.join("old.svg"), "old").unwrap();
    let args = [
        "-",
        "--format",
        "dot",
        "-o",
        "old.svg",
        "--timeout",
        "1",
        "--debug",
    ];
    let (code, _, err) = run_in(&dir, &args, slow_graph().as_bytes());
    assert_eq!(code, 3);
    assert!(
        err.ends_with("✗ -: Rendering took longer than 1 s and was stopped.\n  Simplify the diagram, or raise --timeout (up to 600).\n"),
        "{err}"
    );
    let pid = err.split("stopped after 1 s (pid ").nth(1).unwrap();
    let pid = &pid[..pid.find(')').unwrap()];
    assert!(!running(pid), "dot (pid {pid}) is gone");
    assert_eq!(fs::read_to_string(dir.join("old.svg")).unwrap(), "old");
    let (code, out, _) = run_in(
        &dir,
        &["check", "-", "--timeout", "1", "--json"],
        slow_graph().as_bytes(),
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!((code, &v["errors"][0]["code"]), (3, &json!("timeout")));
}
