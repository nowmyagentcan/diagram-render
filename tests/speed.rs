//! Wall times against the performance targets, on a release build:
//! `cargo test --release --test speed -- --ignored --nocapture`.

mod common;

use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use common::*;

/// The median wall time of 5 runs of `args` in `dir`.
fn median(dir: &Path, args: &[&str]) -> Duration {
    let mut times: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            let (code, _, err) = run_in(dir, args, b"");
            assert_eq!(code, 0, "{args:?}: {err}");
            start.elapsed()
        })
        .collect();
    times.sort();
    times[2]
}

/// `large.bpmn`: 3 pools with 100 flow nodes each (a start, tasks and gateways, an end), 348
/// sequence flows and 2 message flows, all laid out and named.
fn large_bpmn() -> String {
    let (mut model, mut di) = (String::new(), String::new());
    let mut collaboration = String::from(r#"<collaboration id="c">"#);
    for pool in 0..3 {
        let top = 80 + pool * 760;
        let _ = write!(
            collaboration,
            r#"<participant id="p{pool}" name="Pool {pool}" processRef="r{pool}"/>"#
        );
        let _ = write!(
            di,
            r#"<bpmndi:BPMNShape id="p{pool}_di" bpmnElement="p{pool}" isHorizontal="true"><dc:Bounds x="100" y="{top}" width="2900" height="720"/></bpmndi:BPMNShape>"#
        );
        let _ = write!(model, r#"<process id="r{pool}">"#);
        let node = |i: usize| (180 + (i % 20) * 140, top + 40 + (i / 20) * 140);
        for i in 0..100 {
            let id = format!("n{pool}_{i}");
            let (x, y) = node(i);
            let (kind, w, h) = match i {
                0 => ("startEvent", 36, 36),
                99 => ("endEvent", 36, 36),
                i if i % 7 == 3 => ("exclusiveGateway", 50, 50),
                _ => ("userTask", 100, 80),
            };
            let _ = write!(
                model,
                r#"<{kind} id="{id}" name="Step {i} of pool {pool}"/>"#
            );
            let _ = write!(
                di,
                r#"<bpmndi:BPMNShape id="{id}_di" bpmnElement="{id}"><dc:Bounds x="{x}" y="{y}" width="{w}" height="{h}"/></bpmndi:BPMNShape>"#
            );
        }
        let flows = (0..99)
            .map(|i| (i, i + 1))
            .chain((0..17).map(|k| (k * 5, k * 5 + 21)));
        for (k, (a, b)) in flows.enumerate() {
            let id = format!("f{pool}_{k}");
            let _ = write!(
                model,
                r#"<sequenceFlow id="{id}" name="go {k}" sourceRef="n{pool}_{a}" targetRef="n{pool}_{b}"/>"#
            );
            let ((ax, ay), (bx, by)) = (node(a), node(b));
            let _ = write!(
                di,
                r#"<bpmndi:BPMNEdge id="{id}_di" bpmnElement="{id}"><di:waypoint x="{}" y="{}"/><di:waypoint x="{}" y="{}"/><di:waypoint x="{}" y="{}"/></bpmndi:BPMNEdge>"#,
                ax + 50,
                ay + 40,
                ax + 50,
                by + 40,
                bx,
                by + 40
            );
        }
        model += "</process>";
    }
    for m in 0..2 {
        let _ = write!(
            collaboration,
            r#"<messageFlow id="m{m}" name="Message {m}" sourceRef="n{m}_50" targetRef="n{}_50"/>"#,
            m + 1
        );
        let (x, y) = (180 + 10 * 140 + 50, 80 + m * 760 + 40 + 2 * 140);
        let _ = write!(
            di,
            r#"<bpmndi:BPMNEdge id="m{m}_di" bpmnElement="m{m}"><di:waypoint x="{x}" y="{}"/><di:waypoint x="{x}" y="{}"/></bpmndi:BPMNEdge>"#,
            y + 80,
            y + 760
        );
    }
    collaboration += "</collaboration>";
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" xmlns:bpmndi="http://www.omg.org/spec/BPMN/20100524/DI" xmlns:dc="http://www.omg.org/spec/DD/20100524/DC" xmlns:di="http://www.omg.org/spec/DD/20100524/DI" id="large">
{collaboration}
{model}
<bpmndi:BPMNDiagram id="d"><bpmndi:BPMNPlane id="pl" bpmnElement="c">{di}</bpmndi:BPMNPlane></bpmndi:BPMNDiagram>
</definitions>
"#
    )
}

#[test]
#[ignore = "timing: run on a release build"]
fn bpmn_renders_within_its_targets() {
    let dir = folder("speed-bpmn");
    fs::copy(
        "tests/fixtures/bpmn/collaboration.bpmn",
        dir.join("small.bpmn"),
    )
    .unwrap();
    fs::write(dir.join("large.bpmn"), large_bpmn()).unwrap();
    let check = |file: &str, svg_target: u64, png_extra: u64| {
        let svg = median(&dir, &[file, "-o", "out.svg"]);
        let png = median(&dir, &[file, "-o", "out.png"]);
        println!("{file}: SVG {svg:.0?}, PNG at scale 2 {png:.0?}");
        assert!(
            svg <= Duration::from_millis(svg_target),
            "{file} SVG {svg:?}"
        );
        assert!(
            png <= svg + Duration::from_millis(png_extra),
            "{file} PNG {png:?}"
        );
    };
    check("small.bpmn", 60, 300);
    check("large.bpmn", 500, 2000);
}

/// `large.drawio`: 500 labelled vertices in a 25 × 20 grid and 600 orthogonal edges between
/// neighbours, one page.
fn large_drawio() -> String {
    let mut cells = String::new();
    for i in 0..500 {
        let (x, y) = (40 + (i % 25) * 160, 40 + (i / 25) * 120);
        let _ = write!(
            cells,
            r#"<mxCell id="v{i}" value="Step {i}" style="rounded=1;whiteSpace=wrap;html=1;" vertex="1" parent="1"><mxGeometry x="{x}" y="{y}" width="100" height="50" as="geometry"/></mxCell>"#
        );
    }
    let pairs = (0..500)
        .filter(|i| i % 25 != 24)
        .map(|i| (i, i + 1))
        .chain((0..475).map(|i| (i, i + 25)))
        .take(600);
    for (k, (a, b)) in pairs.enumerate() {
        let _ = write!(
            cells,
            r#"<mxCell id="e{k}" style="edgeStyle=orthogonalEdgeStyle;rounded=0;html=1;" edge="1" parent="1" source="v{a}" target="v{b}"><mxGeometry relative="1" as="geometry"/></mxCell>"#
        );
    }
    format!(
        r#"<mxfile><diagram name="Large"><mxGraphModel><root><mxCell id="0"/><mxCell id="1" parent="0"/>{cells}</root></mxGraphModel></diagram></mxfile>"#
    )
}

/// A PNG of noise of about 300 KB, different for each `seed`.
fn noise_png(seed: u32) -> Vec<u8> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(280, 280).unwrap();
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    for byte in pixmap.data_mut().chunks_mut(4) {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        byte.copy_from_slice(&[x as u8, (x >> 8) as u8, (x >> 16) as u8, 255]);
    }
    pixmap.encode_png().unwrap()
}

/// `large.excalidraw`: 2,000 elements of every type, in a grid, and 5 embedded 300 KB PNGs.
fn large_excalidraw() -> String {
    use base64::Engine;
    let files: Vec<String> = (0..5)
        .map(|i| {
            let png = base64::engine::general_purpose::STANDARD.encode(noise_png(i + 1));
            format!(
                r#""f{i}": {{"mimeType": "image/png", "dataURL": "data:image/png;base64,{png}"}}"#
            )
        })
        .collect();
    let mut elements = vec![];
    for i in 0..2000 {
        let (x, y) = ((i % 50) * 80, (i / 50) * 60);
        let base = format!(
            r##""id": "e{i}", "x": {x}, "y": {y}, "width": 64, "height": 48, "seed": {}, "backgroundColor": "#a5d8ff", "fillStyle": "{}""##,
            i + 1,
            ["hachure", "cross-hatch", "solid", "zigzag"][i % 4]
        );
        elements.push(match i % 8 {
            0 => format!(r#"{{"type": "rectangle", {base}}}"#),
            1 => format!(r#"{{"type": "diamond", {base}}}"#),
            2 => format!(r#"{{"type": "ellipse", {base}}}"#),
            3 => format!(r#"{{"type": "arrow", {base}, "points": [[0, 0], [32, 40], [64, 10]], "roundness": {{"type": 2}}}}"#),
            4 => format!(r#"{{"type": "line", {base}, "points": [[0, 0], [64, 48]]}}"#),
            5 => {
                let points: Vec<String> = (0..40).map(|k| format!("[{}, {}]", k, (k * 7) % 40)).collect();
                format!(r#"{{"type": "freedraw", {base}, "points": [{}], "simulatePressure": true}}"#, points.join(", "))
            }
            6 => format!(r#"{{"type": "text", {base}, "text": "Step {i}", "fontSize": 20, "fontFamily": 5}}"#),
            7 if i < 40 => format!(r#"{{"type": "image", {base}, "fileId": "f{}"}}"#, i / 8),
            _ => format!(r#"{{"type": "rectangle", {base}, "roundness": {{"type": 3}}}}"#),
        });
    }
    format!(
        r#"{{"type": "excalidraw", "elements": [{}], "files": {{{}}}}}"#,
        elements.join(",\n"),
        files.join(",")
    )
}

#[test]
#[ignore = "timing: run on a release build"]
fn excalidraw_renders_within_its_targets() {
    let dir = folder("speed-excalidraw");
    fs::copy(
        "tests/fixtures/excalidraw/small.excalidraw",
        dir.join("small.excalidraw"),
    )
    .unwrap();
    fs::write(dir.join("large.excalidraw"), large_excalidraw()).unwrap();
    for (file, svg_target, png_extra) in [
        ("small.excalidraw", 60, 300),
        ("large.excalidraw", 1500, 2000),
    ] {
        let svg = median(&dir, &[file, "-o", "out.svg"]);
        let png = median(&dir, &[file, "-o", "out.png"]);
        println!("{file}: SVG {svg:.0?}, PNG at scale 2 {png:.0?}");
        assert!(
            svg <= Duration::from_millis(svg_target),
            "{file} SVG {svg:?}"
        );
        assert!(
            png <= svg + Duration::from_millis(png_extra),
            "{file} PNG {png:?}"
        );
    }
}

#[test]
#[ignore = "timing: run on a release build"]
fn drawio_renders_within_its_targets() {
    let dir = folder("speed-drawio");
    fs::copy(
        "tests/fixtures/drawio/pages.drawio",
        dir.join("small.drawio"),
    )
    .unwrap();
    fs::write(dir.join("large.drawio"), large_drawio()).unwrap();
    for (file, svg_target, png_extra) in [("small.drawio", 60, 300), ("large.drawio", 1000, 2000)] {
        let svg = median(&dir, &[file, "-o", "out.svg"]);
        let png = median(&dir, &[file, "-o", "out.png"]);
        println!("{file}: SVG {svg:.0?}, PNG at scale 2 {png:.0?}");
        assert!(
            svg <= Duration::from_millis(svg_target),
            "{file} SVG {svg:?}"
        );
        assert!(
            png <= svg + Duration::from_millis(png_extra),
            "{file} PNG {png:?}"
        );
    }
}
