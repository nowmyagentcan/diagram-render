//! Talks to `diagram-render mcp` over stdio the way MCP clients do.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::*;
use resvg::tiny_skia::Pixmap;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_diagram-render");
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0.0"}}}"#;
const FLOW: &str = "flowchart LR\n  A[Write] --> B{Valid?}\n";

/// Sends `messages` after the handshake from `dir`, closes stdin, and returns the responses by
/// id. The server ends with 0 and writes nothing to stderr.
fn session(dir: &Path, messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(BIN)
        .arg("mcp")
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{INITIALIZE}").unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    for m in messages {
        writeln!(stdin, "{m}").unwrap();
    }
    // A client stays until it has its answers: rmcp drops what is unsent at the end of stdin.
    let asked = 1 + messages.iter().filter(|m| m.get("id").is_some()).count();
    let lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut responses: Vec<Value> = lines
        .take(asked)
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(out.status.code(), Some(0));
    responses.sort_by_key(|r| r["id"].as_u64());
    responses
}

/// The result of calling `tool` once with `arguments` in `dir`.
fn call(dir: &Path, tool: &str, arguments: Value) -> Value {
    let message = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    });
    session(dir, &[message]).pop().unwrap()["result"].clone()
}

#[cfg(windows)]
#[test]
fn a_network_path_is_refused_for_input_and_output() {
    let dir = folder("mcp-network");
    let (url, seen) = canary();
    let host = format!("127.0.0.1@{}", url.rsplit(':').next().unwrap());
    for path in [
        format!("//{host}/s/x.dot"),
        format!(r"\\{host}\s\x.dot"),
        format!(r"/\{host}\s\x.dot"),
        format!(r"\\?\UNC\{host}\s\x.dot"),
        format!(r"\\.\UNC\{host}\s\x.dot"),
        format!(r"\??\UNC\{host}\s\x.dot"),
    ] {
        let output = path.replace(".dot", ".svg");
        for (tool, arguments) in [
            ("render_diagram", json!({"path": path})),
            ("check_diagram", json!({"path": path})),
            (
                "render_diagram",
                json!({"source": "digraph {a}", "format": "dot", "output": output}),
            ),
        ] {
            let result = call(&dir, tool, arguments);
            assert_eq!(result["isError"], true, "{path}: {result}");
            assert_eq!(text(&result)["error"]["code"], "network_path", "{path}");
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(seen.try_iter().count(), 0, "a path reached the network");
    fs::write(dir.join("x.dot"), "digraph {a}").unwrap();
    let verbatim = format!(r"\\?\{}", dir.join("x.dot").display());
    let result = call(&dir, "check_diagram", json!({"path": verbatim}));
    assert_eq!(text(&result)["valid"], true, "{result}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_running_server_keeps_its_worker_after_an_upgrade_by_rename() {
    let bin = installed("mcp-upgrade", &[]);
    let dir = bin.parent().unwrap();
    std::os::unix::fs::symlink(Path::new(BIN).with_file_name("fonts"), dir.join("fonts")).unwrap();
    let mut command = Command::new(&bin);
    command
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A fresh copy is busy while a sibling test's fork holds its write handle.
    let mut child = loop {
        match command.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            spawned => break spawned.unwrap(),
        }
    };
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    writeln!(stdin, "{INITIALIZE}").unwrap();
    lines.next().unwrap().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    // What both installers do: a new file beside the old one, renamed over it.
    let next = dir.join("next");
    fs::copy(BIN, &next).unwrap();
    fs::rename(&next, &bin).unwrap();
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drawio/pages.drawio");
    let source = fs::read_to_string(file).unwrap();
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "render_diagram",
                   "arguments": {"source": source, "format": "drawio", "preview": false}},
    });
    writeln!(stdin, "{call}").unwrap();
    let answer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    drop(stdin);
    child.wait().unwrap();
    let result = &answer["result"];
    assert!(result.get("isError").is_none_or(|e| e == false), "{answer}");
}

/// The JSON of a result's text block.
fn text(result: &Value) -> Value {
    let content = result["content"].as_array().unwrap();
    let block = content.iter().find(|c| c["type"] == "text").unwrap();
    serde_json::from_str(block["text"].as_str().unwrap()).unwrap()
}

/// The preview a result carries, decoded.
fn image(result: &Value) -> Option<Vec<u8>> {
    let content = result["content"].as_array().unwrap();
    let block = content.iter().find(|c| c["type"] == "image")?;
    assert_eq!(block["mimeType"], "image/png");
    assert!(block.get("annotations").is_none());
    Some(STANDARD.decode(block["data"].as_str().unwrap()).unwrap())
}

/// An error result: its error object, with no structured content.
fn refused(result: &Value) -> Value {
    assert_eq!(result["isError"], true, "{result}");
    assert!(result.get("structuredContent").is_none());
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    text(result)["error"].clone()
}

#[test]
fn the_server_introduces_itself_and_both_tools() {
    let dir = folder("mcp-list");
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    let responses = session(&dir, &[list]);
    let info = &responses[0]["result"];
    assert_eq!(info["serverInfo"]["name"], "diagram-render");
    assert_eq!(info["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        info["instructions"]
            .as_str()
            .unwrap()
            .starts_with("Check and render diagrams locally:")
    );
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let tool = |name| tools.iter().find(|t| t["name"] == name).unwrap();
    assert_eq!(tools.len(), 2);
    let (render, check) = (tool("render_diagram"), tool("check_diagram"));
    for t in [render, check] {
        assert_eq!(t["inputSchema"]["additionalProperties"], false);
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(t["inputSchema"].get("required").is_none());
        let formats = &t["inputSchema"]["properties"]["format"]["enum"];
        assert!(formats.as_array().unwrap().contains(&Value::Null));
    }
    let keys = |t: &Value| {
        let mut k: Vec<_> = (t["inputSchema"]["properties"].as_object().unwrap().keys())
            .cloned()
            .collect();
        k.sort();
        k.join(" ")
    };
    assert_eq!(
        keys(render),
        "allow_network background format output page path preview scale source timeout to"
    );
    assert_eq!(keys(check), "format page path source timeout");
    assert!(render.get("outputSchema").is_none());
    assert_eq!(check["outputSchema"]["type"], "object");
    assert_eq!(render["annotations"]["readOnlyHint"], false);
    assert_eq!(render["annotations"]["idempotentHint"], true);
    assert_eq!(check["annotations"]["readOnlyHint"], true);
    assert_eq!(check["annotations"]["openWorldHint"], false);
    assert!(
        render["description"]
            .as_str()
            .unwrap()
            .starts_with("Render a diagram to SVG or PNG locally, with no browser:")
    );
}

#[test]
fn check_gives_structured_content_and_the_same_json_as_text() {
    let dir = folder("mcp-check");
    let result = call(&dir, "check_diagram", json!({"source": FLOW}));
    assert!(result.get("isError").is_none_or(|e| e == false));
    assert_eq!(result["structuredContent"], text(&result));
    let report = &result["structuredContent"];
    assert_eq!(report["input"], "-");
    assert_eq!(report["format"], "mermaid");
    assert_eq!(report["diagram_type"], "flowchart");
    assert_eq!(report["valid"], true);
    assert_eq!(report["engine"]["name"], "merman");
}

#[test]
fn check_lists_every_error_of_a_broken_diagram() {
    let dir = folder("mcp-broken");
    let source = "a -> b: {x\n\nc\n}\n";
    let arguments = json!({"source": source, "format": "d2"});
    let result = call(&dir, "check_diagram", arguments);
    assert!(result.get("isError").is_none_or(|e| e == false));
    let report = &result["structuredContent"];
    assert_eq!(report["valid"], false);
    assert!(report["errors"][0]["line"].as_u64().is_some());
    assert_eq!(report["errors"][0]["code"], "syntax_error");
    // A file that isn't there is no finding about a diagram.
    let result = call(&dir, "check_diagram", json!({"path": "missing.mmd"}));
    assert_eq!(refused(&result)["code"], "file_not_found");
}

#[test]
fn render_returns_a_preview_and_writes_nothing_without_output() {
    let dir = folder("mcp-render");
    let result = call(&dir, "render_diagram", json!({"source": FLOW}));
    assert!(result.get("structuredContent").is_none());
    assert_eq!(result["content"][0]["type"], "image");
    assert_eq!(result["content"][1]["type"], "text");
    let report = text(&result);
    assert_eq!(report["output"], Value::Null);
    assert_eq!(report["to"], "svg");
    assert_eq!(report["format"], "mermaid");
    assert_eq!(report["input"], "-");
    let png = Pixmap::decode_png(&image(&result).unwrap()).unwrap();
    assert!(png.width() as f64 >= report["width"].as_f64().unwrap() * 1.9);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn render_writes_output_and_takes_its_type_from_the_extension() {
    let dir = folder("mcp-output");
    let flow = dir.join("flow.mmd");
    fs::write(&flow, FLOW).unwrap();
    let arguments =
        json!({"path": "flow.mmd", "output": "out.png", "scale": 1.5, "preview": false});
    let result = call(&dir, "render_diagram", arguments);
    let report = text(&result);
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert_eq!(report["output"], "out.png");
    assert_eq!(report["to"], "png");
    assert_eq!(report["scale"], 1.5);
    assert_eq!(report["input"], "flow.mmd");
    let png = Pixmap::load_png(dir.join("out.png")).unwrap();
    assert_eq!(json!(png.width()), report["width"]);
    let arguments = json!({"path": "flow.mmd", "output": "flow.svg", "background": "#336699"});
    let result = call(&dir, "render_diagram", arguments);
    assert_eq!(text(&result)["to"], "svg");
    assert!(image(&result).is_some());
    assert!(
        fs::read_to_string(dir.join("flow.svg"))
            .unwrap()
            .contains("<svg")
    );
}

#[test]
fn a_page_is_chosen_like_on_the_command_line() {
    let dir = folder("mcp-page");
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drawio/pages.drawio");
    let arguments = json!({"path": file, "page": "2", "preview": false});
    let report = text(&call(&dir, "render_diagram", arguments));
    assert_eq!(report["page"]["index"], 2);
    assert_eq!(report["pages"].as_array().unwrap().len(), 3);
    let arguments = json!({"path": file, "page": "nine"});
    let result = call(&dir, "render_diagram", arguments);
    assert_eq!(refused(&result)["code"], "page_not_found");
}

#[test]
fn a_preview_keeps_within_1568_px() {
    let dir = folder("mcp-side");
    let names: Vec<_> = (0..120).map(|i| format!("node{i}")).collect();
    let source = format!("digraph {{ rankdir=LR; {} }}", names.join(" -> "));
    let result = call(&dir, "render_diagram", json!({"source": source}));
    let report = text(&result);
    assert!(report["width"].as_u64().unwrap() > 1568, "{report}");
    let png = Pixmap::decode_png(&image(&result).unwrap()).unwrap();
    assert!(png.width() <= 1568 && png.width() > 1400, "{}", png.width());
}

#[test]
fn a_preview_keeps_within_a_million_bytes() {
    let dir = folder("mcp-bytes");
    // Noise doesn't compress, so this image fills the preview's bytes at any size near its own.
    let (side, mut seed) = (500_u32, 1_u32);
    let mut noise = Pixmap::new(side, side).unwrap();
    for pixel in noise.pixels_mut() {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let [r, g, b, _] = seed.to_be_bytes();
        *pixel = resvg::tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, 255).unwrap();
    }
    let data = STANDARD.encode(noise.encode_png().unwrap());
    let scene = json!({
        "type": "excalidraw", "version": 2,
        "elements": [{
            "id": "i", "type": "image", "x": 0, "y": 0, "width": 500, "height": 500,
            "fileId": "f", "status": "saved", "scale": [1, 1], "angle": 0, "opacity": 100,
        }],
        "files": {"f": {"id": "f", "mimeType": "image/png", "dataURL": format!("data:image/png;base64,{data}")}},
    });
    fs::write(dir.join("noise.excalidraw"), scene.to_string()).unwrap();
    let result = call(&dir, "render_diagram", json!({"path": "noise.excalidraw"}));
    let png = image(&result).unwrap();
    assert!(png.len() <= 1_000_000, "{}", png.len());
    assert!(png.len() >= 200_000, "{}", png.len());
}

#[cfg(not(windows))]
#[test]
fn a_d2_diagram_that_needs_the_network_renders_without_a_preview() {
    let dir = folder("mcp-remote");
    let source = "a: {shape: image; icon: https://example.invalid/a.png}\n";
    let result = call(
        &dir,
        "render_diagram",
        json!({"source": source, "format": "d2"}),
    );
    assert!(image(&result).is_none());
    let report = text(&result);
    let warnings: Vec<_> = (report["warnings"].as_array().unwrap().iter())
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert_eq!(warnings, ["remote_images_not_fetched", "no_preview"]);
}

#[test]
fn errors_are_error_results_with_the_json_as_text() {
    let dir = folder("mcp-errors");
    let result = call(
        &dir,
        "render_diagram",
        json!({"source": "flowchart TD\n  A --> B --\n"}),
    );
    let e = refused(&result);
    assert_eq!(e["code"], "syntax_error");
    assert_eq!(e["line"], 3);
    assert!(e["hint"].as_str().unwrap().starts_with("Fix line 3"));
    assert_eq!(text(&result)["format"], "mermaid");
    assert_eq!(text(&result)["output"], Value::Null);
}

#[test]
fn bad_arguments_are_refused() {
    let dir = folder("mcp-bad");
    let flow = "graph TD; A-->B";
    let cases = [
        (json!({}), "invalid_option", "Give source or path."),
        (
            json!({"source": flow, "path": "x.mmd"}),
            "invalid_option",
            "Give source or path, not both.",
        ),
        (
            json!({"source": flow, "scale": 9}),
            "invalid_option",
            "--scale must be between 0.1 and 8.",
        ),
        (
            json!({"source": flow, "timeout": 0}),
            "invalid_option",
            "--timeout must be a whole number of seconds from 1 to 600.",
        ),
        (
            json!({"source": flow, "output": "-"}),
            "bad_extension",
            "The output must end in .svg or .png.",
        ),
        (
            json!({"source": flow, "output": "a.png", "to": "svg"}),
            "format_conflict",
            "-o says PNG but --to says SVG.",
        ),
        (
            json!({"source": "x".repeat((4 << 20) + 1)}),
            "invalid_option",
            "source is over 4 MiB.",
        ),
    ];
    for (arguments, code, message) in cases {
        let e = refused(&call(&dir, "render_diagram", arguments.clone()));
        assert_eq!(e["code"], code, "{arguments:.80}");
        assert_eq!(e["message"], message);
    }
    for tool in ["render_diagram", "check_diagram"] {
        let e = refused(&call(&dir, tool, json!({"source": flow, "colour": "red"})));
        assert_eq!(e["code"], "invalid_option");
        assert!(e["message"].as_str().unwrap().contains("colour"), "{e}");
    }
    let arguments = json!({"source": flow, "output": "o.svg"});
    let e = refused(&call(&dir, "check_diagram", arguments));
    assert!(e["message"].as_str().unwrap().contains("output"), "{e}");
}

#[test]
fn a_render_never_replaces_its_input_or_reads_the_protocol() {
    let dir = folder("mcp-input");
    let svg = dir.join("flow.drawio.svg");
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drawio/board.drawio.svg");
    fs::copy(file, &svg).unwrap();
    let arguments = json!({"path": "flow.drawio.svg", "output": "flow.drawio.svg"});
    let e = refused(&call(&dir, "render_diagram", arguments));
    assert_eq!(e["code"], "output_is_input");
    // A file named `-` is a file, and stdin stays the client's.
    let e = refused(&call(&dir, "render_diagram", json!({"path": "-"})));
    assert_eq!(e["code"], "file_not_found");
}

#[test]
fn a_client_that_leaves_ends_the_server_quietly() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = Command::new(BIN)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{INITIALIZE}").unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
    let out = cmd(&["mcp"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "stdin closed from the start");
    assert_eq!(
        cmd(&["mcp", "--help"]).output().unwrap().status.code(),
        Some(0)
    );
    let (code, _, err) = run(&["mcp", "x.mmd"]);
    assert_eq!(code, 2);
    assert!(err.contains("mcp takes no file."), "{err}");
}

#[test]
fn every_file_name_checks_and_renders_with_a_preview_in_its_limits() {
    let dir = folder("mcp-variants");
    let mut messages = vec![];
    for (i, (name, _, path)) in VARIANTS.iter().enumerate() {
        fs::write(dir.join(name), fixture(path)).unwrap();
        let id = 2 * i as u64 + 2;
        for (id, tool) in [(id, "check_diagram"), (id + 1, "render_diagram")] {
            messages.push(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": tool, "arguments": {"path": name}},
            }));
        }
    }
    let responses = session(&dir, &messages);
    for (i, (name, format, _)) in VARIANTS.iter().enumerate() {
        let check = &responses[2 * i + 1]["result"];
        assert_eq!(check["structuredContent"]["format"], *format, "{name}");
        assert_eq!(check["structuredContent"]["valid"], true, "{name}: {check}");
        let render = &responses[2 * i + 2]["result"];
        assert_eq!(text(render)["format"], *format, "{name}");
        let png = image(render).unwrap_or_else(|| panic!("{name}: no preview: {render}"));
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert!(pixmap.width().max(pixmap.height()) <= 1568, "{name}");
        assert!(png.len() <= 1_000_000, "{name}");
    }
}
