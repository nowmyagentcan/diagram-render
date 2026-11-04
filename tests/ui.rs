//! The local page: the HTML works offline, and `diagram-render ui` renders uploads over HTTP on
//! 127.0.0.1 exactly as the CLI renders files.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};

use common::*;
use serde_json::{Value, json};

const PAGE: &str = include_str!("../src/ui.html");
const BIN: &str = env!("CARGO_BIN_EXE_diagram-render");
const FLOW: &[u8] = b"flowchart LR\n  A[Write] --> B{Valid?}\n";

#[test]
fn page_loads_nothing_from_another_host() {
    let footer = [
        "https://nowmyagentcan.com",
        "https://github.com/nowmyagentcan/diagram-render",
        "https://x.com/chayp05",
    ];
    for pat in ["src=", "href=", "url(", "@import", "fetch(", "action="] {
        for (i, _) in PAGE.match_indices(pat) {
            let rest = PAGE[i + pat.len()..].trim_start_matches(['"', '\'', ' ']);
            let target = &rest[..rest.find(['"', '\'', ' ', ')', '>']).unwrap_or(rest.len())];
            assert!(
                target.starts_with("data:") || !target.contains("://") || footer.contains(&target),
                "{pat} loads {target}"
            );
        }
    }
    assert!(PAGE.contains(r#"fetch("/api/render""#));
}

#[test]
fn page_keeps_its_title_and_button() {
    assert!(PAGE.contains("<title>Render a diagram</title>"));
    assert!(PAGE.contains(">Render</button>"));
}

/// Percent-decodes `s` as `decodeURIComponent` does.
fn unescape(s: &str) -> String {
    let mut bytes = vec![];
    let mut rest = s.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        if b == b'%' {
            bytes.push(u8::from_str_radix(std::str::from_utf8(&tail[..2]).unwrap(), 16).unwrap());
            rest = &tail[2..];
        } else {
            bytes.push(b);
            rest = tail;
        }
    }
    String::from_utf8(bytes).unwrap()
}

/// `diagram-render ui` on a free port, stopped when dropped.
struct Server {
    child: Child,
    port: u16,
}

/// A response: status, headers and body.
type Answer = (u16, String, Vec<u8>);

impl Server {
    fn start() -> Server {
        let mut child = Command::new(BIN)
            .args(["ui", "--no-open"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let port = line
            .strip_prefix("Open http://127.0.0.1:")
            .and_then(|rest| rest.strip_suffix("  (Ctrl+C to stop)\n"))
            .unwrap_or_else(|| panic!("{line:?}"))
            .parse()
            .unwrap();
        Server { child, port }
    }

    /// Sends `head` (request line and headers, `{port}` filled in) and `body`.
    fn send(&self, head: &str, body: &[u8]) -> Answer {
        let head = head.replace("{port}", &self.port.to_string());
        self.raw(format!("{head}\r\n\r\n").as_bytes(), body)
    }

    fn raw(&self, head: &[u8], body: &[u8]) -> Answer {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.write_all(head).unwrap();
        // A server that answers before reading the whole body may close early.
        let _ = stream.write_all(body);
        let _ = stream.shutdown(Shutdown::Write);
        let mut response = vec![];
        stream.read_to_end(&mut response).unwrap();
        let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = String::from_utf8(response[..end].to_vec()).unwrap();
        let status = headers[9..12].parse().unwrap();
        (status, headers, response[end + 4..].to_vec())
    }

    /// Uploads `body` as the page does: the status, the report (from `X-Result` on 200) or error
    /// JSON, and the image.
    fn render(&self, name: &str, options: &str, body: &[u8]) -> (u16, Value, Vec<u8>) {
        let head = format!(
            "POST /api/render HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nOrigin: http://127.0.0.1:{{port}}\r\n\
             X-File-Name: {name}\r\nX-Options: {options}\r\nContent-Length: {}",
            body.len()
        );
        let (status, headers, body) = self.send(&head, body);
        let result = headers
            .lines()
            .find_map(|l| l.strip_prefix("X-Result: "))
            .map(unescape);
        match result {
            Some(report) => (status, serde_json::from_str(&report).unwrap(), body),
            None => (status, serde_json::from_slice(&body).unwrap(), vec![]),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

#[test]
fn serves_the_page_and_renders_like_the_cli() {
    let server = Server::start();
    let (status, headers, body) = server.send("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}", b"");
    assert_eq!((status, body.as_slice()), (200, PAGE.as_bytes()));
    assert!(headers.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(headers.contains("Content-Security-Policy: default-src 'none';"));
    assert!(headers.contains("X-Content-Type-Options: nosniff\r\n"));

    let dir = folder("ui-cli");
    fs::write(dir.join("flow.mmd"), FLOW).unwrap();
    let (code, want, _) = run_in(&dir, &["flow.mmd", "--to", "png", "--json"], b"");
    assert_eq!(code, 0);
    let want: Value = serde_json::from_str(&want).unwrap();
    let (status, report, png) = server.render("flow.mmd", r#"{"to":"png"}"#, FLOW);
    assert_eq!(status, 200);
    for key in [
        "input",
        "format",
        "diagram_type",
        "to",
        "width",
        "height",
        "scale",
        "warnings",
    ] {
        assert_eq!(report[key], want[key], "{key}");
    }
    assert_eq!(report["output"], "flow.png");
    assert_eq!(png, fs::read(dir.join("flow.png")).unwrap());
    assert_eq!(report["bytes"], json!(png.len()));
}

#[test]
fn pasted_text_has_no_name_and_detects_by_content() {
    let server = Server::start();
    let (status, report, svg) = server.render("", r#"{"to":"svg"}"#, FLOW);
    assert_eq!(status, 200);
    assert_eq!(report["input"], "-");
    assert_eq!(report["output"], "diagram.svg");
    assert_eq!(report["format"], "mermaid");
    assert!(String::from_utf8(svg).unwrap().contains("<svg"));
    // Every format, as the Format select sends it, with its own bytes.
    for (file, format) in [
        ("dot/clusters.dot", "dot"),
        ("d2/containers.d2", "d2"),
        ("bpmn/pages.bpmn", "bpmn"),
        ("drawio/shapes.drawio", "drawio"),
        ("excalidraw/shapes.excalidraw", "excalidraw"),
    ] {
        let options = format!(r#"{{"to":"svg","format":"{format}"}}"#);
        let (status, report, _) = server.render("", &options, &fixture(file));
        assert_eq!(
            (status, &report["format"]),
            (200, &json!(format)),
            "{report}"
        );
    }
}

#[test]
fn a_dropped_png_file_and_its_pages_render() {
    let server = Server::start();
    let png = fixture("drawio/board.drawio.png");
    let (status, report, _) = server.render("board.drawio.png", r#"{"to":"svg"}"#, &png);
    assert_eq!((status, &report["format"]), (200, &json!("drawio")));
    assert_eq!(report["output"], "board.svg");
    let pages = fixture("drawio/pages.drawio");
    let (status, report, _) = server.render("pages.drawio", r#"{"to":"svg"}"#, &pages);
    assert_eq!(status, 200);
    assert_eq!(report["pages"].as_array().unwrap().len(), 3);
    assert_eq!(report["page"]["index"], 1);
    let (status, second, _) = server.render("pages.drawio", r#"{"to":"svg","page":"2"}"#, &pages);
    assert_eq!(status, 200);
    assert_eq!(second["page"]["index"], 2);
    let name = second["page"]["name"].as_str().unwrap();
    assert_eq!(second["output"], format!("pages-{name}.svg"));
}

#[test]
fn a_unicode_name_comes_back_in_the_report() {
    let server = Server::start();
    let (status, report, _) = server.render("Gr%C3%BC%C3%9Fe%20flow.mmd", "{}", FLOW);
    assert_eq!(status, 200);
    assert_eq!(report["input"], "Grüße flow.mmd");
    assert_eq!(report["output"], "Grüße flow.svg");
    // Only the file's name counts; a path in it reaches nothing.
    let (status, report, _) = server.render("..%2F..%2Fsecret%2Fflow.mmd", "{}", FLOW);
    assert_eq!((status, &report["input"]), (200, &json!("flow.mmd")));
}

#[test]
fn diagram_and_option_errors_are_422() {
    let server = Server::start();
    let (status, report, _) = server.render("", "{}", b"");
    assert_eq!(
        (status, &report["error"]["code"]),
        (422, &json!("empty_input"))
    );
    let broken = b"digraph {\n  a -> ;\n}\n";
    let (status, report, _) = server.render("broken.dot", "{}", broken);
    assert_eq!(status, 422);
    assert_eq!(report["error"]["code"], "syntax_error");
    assert_eq!(report["error"]["line"], 2);
    assert_eq!(report["format"], "dot");
    assert_eq!(report["error"]["excerpt"], "  a -> ;");
    let (status, report, _) = server.render("", r#"{"scale":9}"#, FLOW);
    assert_eq!(status, 422);
    assert_eq!(
        report["error"]["message"],
        "--scale must be between 0.1 and 8."
    );
    assert!(report["error"].get("line").is_none());
    let (status, report, _) = server.render("plain.png", "{}", &fixture("d2/pixel.png"));
    assert_eq!(
        (status, &report["error"]["code"]),
        (422, &json!("unsupported_input"))
    );
}

#[test]
fn an_svg_comes_back_cleaned() {
    let server = Server::start();
    let dot = br#"digraph { a [URL="javascript:alert(1)"] }"#;
    let (status, report, svg) = server.render("x.dot", "{}", dot);
    assert_eq!(status, 200);
    assert_eq!(report["warnings"][0]["code"], "unsafe_content_removed");
    assert!(!String::from_utf8(svg).unwrap().contains("javascript:"));
}

#[test]
fn only_this_computers_page_is_answered() {
    let server = Server::start();
    let get = |headers: &str| server.send(&format!("GET / HTTP/1.1\r\n{headers}"), b"").0;
    assert_eq!(get("Host: 127.0.0.1:{port}"), 200);
    assert_eq!(get("Host: localhost:{port}"), 200);
    assert_eq!(
        get("Host: LOCALHOST:{port}\r\nOrigin: http://localhost:{port}"),
        200
    );
    for headers in [
        "Host: evil.example",
        "Host: 127.0.0.1",
        "Host: 127.0.0.1:1",
        "Host: 127.0.0.1:{port}\r\nOrigin: https://127.0.0.1:{port}",
        "Host: 127.0.0.1:{port}\r\nOrigin: http://evil.example",
        "Host: 127.0.0.1:{port}\r\nOrigin: null",
        "X-Other: 1",
    ] {
        assert_eq!(get(headers), 403, "{headers}");
    }
    let host = "Host: 127.0.0.1:{port}";
    for head in [
        "GET /other HTTP/1.1",
        "GET /api/render HTTP/1.1",
        "POST / HTTP/1.1",
        "PUT /api/render HTTP/1.1",
    ] {
        let (status, ..) = server.send(&format!("{head}\r\n{host}"), b"");
        assert_eq!(status, 404, "{head}");
    }
}

#[test]
fn malformed_requests_are_400_and_oversized_ones_413() {
    let server = Server::start();
    let post = "POST /api/render HTTP/1.1\r\nHost: 127.0.0.1:{port}";
    for (head, body) in [
        (
            format!("{post}\r\nHost: 127.0.0.1:{{port}}\r\nContent-Length: 0"),
            "",
        ),
        (
            format!(
                "{post}\r\nOrigin: http://127.0.0.1:{{port}}\r\nOrigin: http://127.0.0.1:{{port}}"
            ),
            "",
        ),
        (
            format!("{post}\r\nContent-Length: 1\r\nContent-Length: 1"),
            "a",
        ),
        (format!("{post}\r\nTransfer-Encoding: chunked"), ""),
        (format!("{post}\r\nContent-Length: +1"), "a"),
        (format!("{post}\r\nContent-Length: x"), ""),
        (format!("{post}\r\nContent-Length: 10"), "short"),
        (format!("{post}\r\nno colon here"), ""),
        (format!("{post}\r\nBad Name: x"), ""),
        (
            format!("{post}\r\nX-File-Name: %zz\r\nContent-Length: 1"),
            "a",
        ),
        (
            format!("{post}\r\nX-File-Name: %C3\r\nContent-Length: 1"),
            "a",
        ),
        (
            format!("{post}\r\nX-Options: nope\r\nContent-Length: 1"),
            "a",
        ),
        (
            format!("{post}\r\nX-Options: {{\"colour\":1}}\r\nContent-Length: 1"),
            "a",
        ),
        ("GET / HTTP/2\r\nHost: 127.0.0.1:{port}".to_string(), ""),
        ("GET  / HTTP/1.1\r\nHost: 127.0.0.1:{port}".to_string(), ""),
        ("nonsense\r\nHost: 127.0.0.1:{port}".to_string(), ""),
    ] {
        let (status, _, answer) = server.send(&head, body.as_bytes());
        assert_eq!(status, 400, "{head}");
        let report: Value = serde_json::from_slice(&answer).unwrap();
        assert_eq!(report["error"]["code"], "bad_request");
    }
    let port = server.port;
    let bytes = format!("GET /\u{e9} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    assert_eq!(server.raw(bytes.as_bytes(), b"").0, 400);
    let bytes = [
        format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Options: ").as_bytes(),
        b"\xff\r\n\r\n",
    ]
    .concat();
    assert_eq!(server.raw(&bytes, b"").0, 400);
    let (status, ..) = server.send(&format!("{post}\r\nContent-Length: 67108865"), b"");
    assert_eq!(status, 413);
    let long = format!("{post}\r\nX-Long: {}\r\n", "x".repeat(20_000));
    assert_eq!(server.send(&long, b"").0, 413);
}

#[test]
fn a_taken_port_is_a_runtime_error() {
    let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let (code, _, err) = run(&["ui", "--no-open", "--port", &port]);
    assert_eq!(code, 3);
    assert!(err.contains(&format!("Port {port} is in use.")), "{err}");
}

#[test]
fn options_of_ui_belong_to_ui() {
    let (code, _, err) = run(&["ui", "--port", "0"]);
    assert_eq!(code, 2);
    assert!(err.contains("--port must be between 1 and 65535."), "{err}");
    let (code, _, err) = run(&["flow.mmd", "--port", "8080"]);
    assert_eq!(code, 2);
    assert!(err.contains("--port doesn't apply to render."), "{err}");
    let (code, out, _) = run(&["ui", "--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("--no-open"));
    let (code, out, _) = run(&["ui", "--json"]);
    assert_eq!(code, 2);
    assert!(out.contains("--json doesn't apply to ui."), "{out}");
}

#[test]
fn a_closed_stdout_ends_the_server_quietly() {
    let out = cmd(&["ui", "--no-open"]).stdout(closed()).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
}

/// Started from Explorer it has a console to itself, so it opens the page. The `cmd.exe` beside
/// the copy stands in for the one that would open a browser.
#[cfg(windows)]
#[test]
fn a_double_click_on_windows_starts_the_page() {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let bin = installed("double-click", &[]);
    fs::copy(&bin, bin.with_file_name("cmd.exe")).unwrap();
    let mut child = Command::new(&bin)
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(line.starts_with("Open http://127.0.0.1:"), "{line:?}");
    assert!(
        line.ends_with(" · close this window to stop.\n"),
        "{line:?}"
    );
}

#[test]
fn every_file_name_renders_through_the_page() {
    let server = Server::start();
    for (name, format, path) in VARIANTS {
        for to in ["svg", "png"] {
            let options = format!(r#"{{"to":"{to}"}}"#);
            let (status, report, image) = server.render(name, &options, &fixture(path));
            assert_eq!(status, 200, "{name} {to}: {report}");
            assert_eq!(report["format"], format, "{name} {to}");
            assert!(!image.is_empty());
        }
    }
}
