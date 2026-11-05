//! What a broken or hostile diagram, or a client that goes away, must not do: touch other files,
//! reach the network, outlive its limits or take a server down. Tests marked `known bug` are the
//! repro of a bug still open, with the behaviour it should have.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_diagram-render");
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0.0"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

/// A graph that keeps `dot` busy for minutes.
fn slow_dot() -> String {
    let mut source = String::from("digraph {\n");
    let mut x: u32 = 1;
    for _ in 0..6000 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        source += &format!("n{} -> n{}\n", (x >> 4) % 3000, (x >> 16) % 3000);
    }
    source + "}\n"
}

/// Waits for `child` for at most `limit`; a run still going then is killed and reported as `None`.
fn within(child: &mut Child, limit: Duration) -> Option<i32> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status.code().unwrap_or(-1));
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    None
}

/// Runs `args` in `dir` with a limit of `limit`: the exit code and stdout, or `None` past it.
fn bounded(dir: &Path, args: &[&str], limit: Duration) -> Option<(i32, String)> {
    let mut child = cmd(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let code = within(&mut child, limit);
    let text = reader.join().unwrap();
    code.map(|code| (code, text))
}

/// The ids of `parent`'s child processes.
fn children(parent: u32) -> Vec<u32> {
    let out = if cfg!(windows) {
        let query = format!(
            "(Get-CimInstance Win32_Process -Filter 'ParentProcessId={parent}' | \
             Where-Object Name -ne 'conhost.exe').ProcessId"
        );
        Command::new("powershell")
            .args(["-NoProfile", "-Command", &query])
            .output()
    } else {
        Command::new("pgrep")
            .args(["-P", &parent.to_string()])
            .output()
    };
    (String::from_utf8(out.unwrap().stdout).unwrap().lines())
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn alive(pid: u32) -> bool {
    let out = if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
    } else {
        Command::new("ps").args(["-p", &pid.to_string()]).output()
    };
    String::from_utf8_lossy(&out.unwrap().stdout).contains(&pid.to_string())
}

fn kill(pid: u32) {
    let _ = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
    } else {
        Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output()
    };
}

/// A `.d2` render that takes a moment, so a test can act before the temporary file is written.
fn busy_d2() -> String {
    (0..150).map(|i| format!("a{i} -> a{}\n", i + 1)).collect()
}

#[test]
fn a_hard_link_at_the_temporary_name_keeps_the_file_it_points_to() {
    let dir = folder("hostile-link");
    let input = dir.join("flow.d2");
    fs::write(&input, busy_d2()).unwrap();
    // A folder where the output goes makes the last step fail as well.
    fs::create_dir(dir.join("out.svg")).unwrap();
    let mut child = cmd(&["flow.d2", "-o", "out.svg", "--json"])
        .current_dir(&dir)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let temp = dir.join(format!(".out.svg.{}.tmp", child.id()));
    fs::hard_link(&input, &temp).unwrap();
    child.wait().unwrap();
    assert_eq!(fs::read_to_string(&input).unwrap(), busy_d2());
}

#[test]
fn a_file_at_the_temporary_name_is_left_alone() {
    let dir = folder("hostile-temp");
    fs::write(dir.join("flow.d2"), busy_d2()).unwrap();
    let mut child = cmd(&["flow.d2", "-o", "out.svg", "--json"])
        .current_dir(&dir)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let temp = dir.join(format!(".out.svg.{}.tmp", child.id()));
    fs::write(&temp, "someone else's").unwrap();
    child.wait().unwrap();
    assert_eq!(fs::read_to_string(&temp).unwrap(), "someone else's");
    assert!(dir.join("out.svg").is_file());
}

#[cfg(unix)]
#[test]
fn links_planted_for_every_likely_pid_dont_redirect_the_output() {
    let dir = folder("hostile-symlink");
    fs::write(dir.join("flow.mmd"), "flowchart TD\n  A --> B\n").unwrap();
    fs::write(dir.join("victim"), "precious").unwrap();
    let start = std::process::id();
    for pid in start..start + 3000 {
        let link = dir.join(format!(".flow.svg.{pid}.tmp"));
        std::os::unix::fs::symlink(dir.join("victim"), link).unwrap();
    }
    let (code, _, _) = run_in(&dir, &["flow.mmd", "--json"], b"");
    assert_eq!(code, 0);
    assert_eq!(fs::read_to_string(dir.join("victim")).unwrap(), "precious");
}

/// Starts `args`, waits for the engine it runs, and kills the engine's parent: was the engine
/// gone a second later?
fn engine_dies_with_its_parent(file: &str, source: &str) {
    let dir = folder("hostile-parent");
    fs::write(dir.join(file), source).unwrap();
    let mut child = cmd(&[file, "-o", "out.svg", "--timeout", "60"])
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let engines = loop {
        let engines = children(child.id());
        if !engines.is_empty() || start.elapsed() > Duration::from_secs(20) {
            break engines;
        }
    };
    assert!(!engines.is_empty(), "{file}: no engine started");
    child.kill().unwrap();
    child.wait().unwrap();
    thread::sleep(Duration::from_secs(1));
    let left: Vec<_> = engines.into_iter().filter(|p| alive(*p)).collect();
    left.iter().copied().for_each(kill);
    assert!(
        left.is_empty(),
        "{file}: engine {left:?} outlived its parent"
    );
}

#[test]
#[ignore = "known bug: R1-002"]
fn killing_a_render_stops_its_engine() {
    engine_dies_with_its_parent("slow.dot", &slow_dot());
    let mermaid: String = (0..8100)
        .map(|i| format!("n{} --> n{}\n", i / 90, i % 90))
        .collect();
    engine_dies_with_its_parent("slow.mmd", &format!("flowchart TD\n{mermaid}"));
    let d2: String = (0..30000)
        .map(|i| format!("a{i} -> a{}\n", (i * 7) % 3000))
        .collect();
    engine_dies_with_its_parent("slow.d2", &d2);
}

#[test]
fn a_render_with_stdout_closed_ends_at_its_timeout() {
    let dir = folder("hostile-closed");
    fs::write(dir.join("slow.dot"), slow_dot()).unwrap();
    let mut child = cmd(&["slow.dot", "-o", "-", "--timeout", "1"])
        .current_dir(&dir)
        .stdout(closed())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(within(&mut child, Duration::from_secs(10)), Some(3));
}

/// `diagram-render mcp`, past the handshake.
struct Mcp {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Mcp {
    fn start(dir: &Path) -> Mcp {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = std::sync::mpsc::channel();
        thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let _ = send.send(line);
            }
        });
        let stdin = child.stdin.take();
        let mut mcp = Mcp {
            child,
            stdin,
            lines,
        };
        mcp.send(INITIALIZE);
        mcp.next(Duration::from_secs(20)).unwrap();
        mcp.send(INITIALIZED);
        mcp
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
    }

    fn call(&mut self, tool: &str, arguments: Value) {
        let message = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments},
        });
        self.send(&message.to_string());
    }

    /// The next message from the server, if one comes within `limit`.
    fn next(&mut self, limit: Duration) -> Option<Value> {
        let line = self.lines.recv_timeout(limit).ok()?;
        Some(serde_json::from_str(&line).unwrap())
    }
}

impl Drop for Mcp {
    /// Lets the server end by itself, so an engine it runs isn't left behind.
    fn drop(&mut self) {
        drop(self.stdin.take());
        within(&mut self.child, Duration::from_secs(20));
    }
}

#[test]
fn an_mcp_client_that_closes_stdin_ends_the_server_by_the_timeout() {
    let dir = folder("hostile-mcp-eof");
    let mut mcp = Mcp::start(&dir);
    mcp.call(
        "render_diagram",
        json!({"source": slow_dot(), "format": "dot", "timeout": 2, "preview": false}),
    );
    thread::sleep(Duration::from_millis(300));
    drop(mcp.stdin.take());
    assert_eq!(within(&mut mcp.child, Duration::from_secs(10)), Some(0));
}

#[test]
#[ignore = "known bug: R1-003"]
fn an_mcp_client_that_cancels_gets_its_engine_stopped() {
    let dir = folder("hostile-mcp-cancel");
    let mut mcp = Mcp::start(&dir);
    mcp.call(
        "render_diagram",
        json!({"source": slow_dot(), "format": "dot", "timeout": 8, "preview": false}),
    );
    thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    mcp.send(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}"#);
    mcp.send(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
    let answer = mcp.next(Duration::from_secs(10));
    assert!(start.elapsed() < Duration::from_secs(3), "{answer:?}");
}

/// `diagram-render ui` on a free port.
fn page() -> (Child, u16) {
    use std::io::BufRead;
    let mut child = Command::new(BIN)
        .args(["ui", "--no-open"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .strip_prefix("Open http://127.0.0.1:")
        .and_then(|rest| rest.split(' ').next())
        .unwrap()
        .parse()
        .unwrap();
    (child, port)
}

fn upload(port: u16, name: &str, body: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!(
        "POST /api/render HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-File-Name: {name}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    stream
}

#[test]
#[ignore = "known bug: R1-003"]
fn a_browser_that_hangs_up_frees_the_page_for_the_next_render() {
    let (mut child, port) = page();
    drop(upload(port, "slow.dot", slow_dot().as_bytes()));
    thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    let mut stream = upload(port, "next.dot", b"digraph { a -> b }");
    let _ = stream.read_to_end(&mut vec![]);
    let took = start.elapsed();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(took < Duration::from_secs(3), "waited {took:?}");
}

#[test]
#[ignore = "known bug: T1-03"]
fn a_page_render_that_crashes_leaves_the_server_answering() {
    let (mut child, port) = page();
    let mut stream = upload(port, "x.excalidraw", rounded_rectangle().as_bytes());
    let mut answer = vec![];
    let _ = stream.read_to_end(&mut answer);
    thread::sleep(Duration::from_millis(200));
    let up = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    child.wait().unwrap();
    assert!(up, "the server ended");
    assert!(answer.starts_with(b"HTTP/1.1 4"), "an error answer");
}

/// A scene with `element` and nothing else.
fn scene(element: Value) -> String {
    json!({"type": "excalidraw", "version": 2, "elements": [element]}).to_string()
}

/// One rounded rectangle far taller than any screen.
fn rounded_rectangle() -> String {
    scene(
        json!({"type": "rectangle", "x": 0, "y": 0, "width": 140, "height": 1e38,
                 "roundness": {"type": 3}}),
    )
}

/// A draw.io file with one vertex of `style` and `geometry`.
fn drawio(style: &str, geometry: &str) -> String {
    format!(
        r#"<mxGraphModel><root><mxCell id="0"/><mxCell id="1" parent="0"/><mxCell id="2" parent="1" vertex="1" value="v" style="{style}"><mxGeometry {geometry} as="geometry"/></mxCell></root></mxGraphModel>"#
    )
}

/// The answer to rendering `file`: it must end, within 20 s, with an exit code the CLI defines.
fn survives(file: &str, source: &[u8], extra: &[&str]) -> Value {
    let dir = folder("hostile-render");
    fs::write(dir.join(file), source).unwrap();
    let args = [&[file, "--json"], extra].concat();
    let (code, out) = bounded(&dir, &args, Duration::from_secs(20))
        .unwrap_or_else(|| panic!("{file} {extra:?}: still running after 20 s"));
    assert!((0..=3).contains(&code), "{file} {extra:?}: exit {code}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{file}: {e}: {out:?}"))
}

#[test]
#[ignore = "known bug: T1-03"]
fn a_negative_size_is_no_crash_in_any_drawio_shape() {
    for style in [
        "swimlane;",
        "shape=note;",
        "shape=card;",
        "shape=cube;",
        "shape=process;",
        "shape=parallelogram;",
    ] {
        for geometry in [r#"width="100" height="-1""#, r#"width="-100" height="50""#] {
            let v = survives("x.drawio", drawio(style, geometry).as_bytes(), &[]);
            assert_ne!(v["error"]["code"], "internal", "{style} {geometry}");
        }
    }
}

#[test]
#[ignore = "known bug: T1-03"]
fn numbers_past_any_screen_are_no_crash_in_excalidraw() {
    survives("x.excalidraw", rounded_rectangle().as_bytes(), &[]);
    let ellipse = scene(
        json!({"type": "ellipse", "x": 0, "y": 0, "width": 100, "height": 70,
                               "backgroundColor": "#ffc9c9", "roughness": 4294967296u64}),
    );
    survives("x.excalidraw", ellipse.as_bytes(), &["-o", "x.png"]);
}

#[test]
#[ignore = "known bug: T1-03"]
fn a_render_that_panics_still_gets_an_mcp_answer() {
    let dir = folder("hostile-mcp-panic");
    let mut mcp = Mcp::start(&dir);
    mcp.call(
        "render_diagram",
        json!({"source": drawio("swimlane;", r#"width="100" height="-1""#)}),
    );
    let answer = mcp.next(Duration::from_secs(10)).expect("an answer");
    assert_eq!(answer["id"], 2, "{answer}");
}

#[test]
#[ignore = "known bug: R1-004"]
fn many_filled_shapes_in_a_small_file_have_a_limit() {
    let elements: Vec<Value> = (0..32)
        .map(|i| {
            json!({"type": "rectangle", "id": format!("r{i}"), "x": 0, "y": 0,
                   "width": 1e7, "height": 1e7, "backgroundColor": "#ffff00",
                   "fillStyle": "hachure", "roughness": 1, "seed": i})
        })
        .collect();
    let source = json!({"type": "excalidraw", "version": 2, "elements": elements});
    let dir = folder("hostile-hachure");
    fs::write(dir.join("x.excalidraw"), source.to_string()).unwrap();
    let args = ["x.excalidraw", "--json", "--timeout", "1"];
    let answer = bounded(&dir, &args, Duration::from_secs(8));
    assert!(answer.is_some(), "still running 7 s past --timeout");
}

#[test]
#[ignore = "known bug: R1-004"]
fn one_filled_shape_past_any_screen_has_a_limit() {
    let dir = folder("hostile-one-shape");
    for size in [4294967296.0, 1e38] {
        let rectangle = scene(json!({"type": "rectangle", "x": 0, "y": 0, "width": size,
                                     "height": 100, "backgroundColor": "#ffff00",
                                     "fillStyle": "cross-hatch"}));
        fs::write(dir.join("x.excalidraw"), rectangle).unwrap();
        // 200 bytes of scene today take 10 s, 900 MB and write a 141 MB SVG.
        let args = ["x.excalidraw", "--json", "--timeout", "1"];
        assert!(
            bounded(&dir, &args, Duration::from_secs(5)).is_some(),
            "{size}"
        );
    }
}

/// A PNG `side` pixels square, black, 1 bit a pixel: a few hundred KB packed, gigabytes unpacked.
fn huge_png(side: u32) -> Vec<u8> {
    let mut zlib = ZlibEncoder::new(vec![], Compression::best());
    let row = vec![0u8; side.div_ceil(8) as usize + 1];
    for _ in 0..side {
        zlib.write_all(&row).unwrap();
    }
    let mut out = vec![];
    let mut encoder = png::Encoder::new(&mut out, side, side);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::One);
    let mut writer = encoder.write_header().unwrap();
    let data = zlib.finish().unwrap();
    writer.write_chunk(png::chunk::IDAT, &data).unwrap();
    drop(writer);
    out
}

#[test]
#[ignore = "known bug: T1-04"]
fn an_embedded_image_over_the_pixel_limit_is_not_unpacked() {
    use base64::Engine;
    let url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(huge_png(20000))
    );
    let scene = json!({"type": "excalidraw", "version": 2,
        "elements": [{"type": "image", "id": "i", "x": 0, "y": 0, "width": 100, "height": 100,
                      "fileId": "f", "status": "saved"}],
        "files": {"f": {"id": "f", "mimeType": "image/png", "dataURL": url}}});
    let dir = folder("hostile-image");
    fs::write(dir.join("x.excalidraw"), scene.to_string()).unwrap();
    let (code, out, _) = run_in(&dir, &["x.excalidraw", "-o", "x.png", "--json"], b"");
    // 400 MP is over the 64 MP limit: it isn't drawn, whatever else the run says.
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(code == 1 || !codes(&v).is_empty(), "{out}");
}

/// `block` statements `depth` deep: merman's layout needs memory by the square of the depth.
fn block_nest(depth: usize) -> String {
    format!(
        "block-beta\n{}a\n{}",
        "block\n".repeat(depth),
        "end\n".repeat(depth)
    )
}

#[test]
#[ignore = "known bug: T1-02"]
fn a_small_mermaid_file_cant_take_all_the_memory() {
    let dir = folder("hostile-merman");
    fs::write(dir.join("x.mmd"), block_nest(3000)).unwrap();
    let args = ["x.mmd", "-o", "x.svg", "--json", "--timeout", "3"];
    // merman's own budget refuses this in 50 ms; left running it takes 350 MB a second, so the
    // short --timeout keeps this test from taking the memory it is about.
    let answer = bounded(&dir, &args, Duration::from_secs(10));
    assert!(answer.is_some_and(|(code, _)| code == 1));
}

#[test]
#[ignore = "known bug: R1-005"]
fn only_the_first_dot_graph_is_read() {
    let v = survives(
        "x.dot",
        b"digraph first {a->b}\ndigraph second {a -> ;}\n",
        &[],
    );
    assert_eq!(codes(&v), ["extra_graphs_ignored"], "{v}");
    let source = format!("digraph first {{a->b}}\n{}", slow_dot());
    let dir = folder("hostile-second");
    fs::write(dir.join("x.dot"), source).unwrap();
    let (code, out, _) = run_in(&dir, &["x.dot", "--json", "--timeout", "2"], b"");
    assert_eq!(code, 0, "{out}");
}

#[test]
#[ignore = "known bug: R1-006"]
fn a_number_picks_a_d2_board_by_position() {
    let source = b"root -> child\nlayers: {\n  deploy: {\n    build -> ship\n  }\n}\n";
    let v = survives("x.d2", source, &["--page", "2"]);
    assert_eq!(v["page"]["index"], 2, "{v}");
}

#[test]
#[ignore = "known bug: R1-008"]
fn cr_line_ends_count_lines_like_every_other_line_end() {
    let dir = folder("hostile-cr");
    let (mut lines, mut columns) = (vec![], vec![]);
    for end in ["\n", "\r\n", "\r"] {
        let source = ["digraph {", "  a -> ;", "}", ""].join(end);
        fs::write(dir.join("x.dot"), source).unwrap();
        let (_, out, _) = run_in(&dir, &["check", "x.dot", "--json"], b"");
        let v: Value = serde_json::from_str(&out).unwrap();
        lines.push(v["errors"][0]["line"].clone());
        columns.push(v["errors"][0]["column"].clone());
    }
    assert_eq!(lines, [json!(2), json!(2), json!(2)]);
    assert_eq!(columns[0], columns[2]);
    let v = survives("x.txt", b"// comment\rdigraph {a->b}\r", &[]);
    assert_eq!(v["format"], "dot", "{v}");
}

#[cfg(windows)]
#[test]
#[ignore = "known bug: R1-009"]
fn an_input_folder_over_260_characters_works() {
    let dir = folder("hostile-long");
    let long = dir.join("x".repeat(120)).join("y".repeat(120));
    fs::create_dir_all(&long).unwrap();
    fs::write(long.join("a.dot"), "digraph { a -> b }").unwrap();
    let input = long.join("a.dot");
    let (code, out, _) = run(&[input.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0, "{out}");
}

#[test]
#[ignore = "known bug: R1-010"]
fn no_svg_holds_a_number_that_isnt_one() {
    let geometry = r#"x="1e308" y="1e308" width="1e308" height="1e308""#;
    let bpmn = br#"<?xml version="1.0"?><definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" xmlns:bpmndi="http://www.omg.org/spec/BPMN/20100524/DI" xmlns:dc="http://www.omg.org/spec/DD/20100524/DC"><process id="p"><task id="t"/></process><bpmndi:BPMNDiagram><bpmndi:BPMNPlane bpmnElement="p"><bpmndi:BPMNShape bpmnElement="t"><dc:Bounds x="1e308" y="1e308" width="1e308" height="1e308"/></bpmndi:BPMNShape></bpmndi:BPMNPlane></bpmndi:BPMNDiagram></definitions>"#;
    let excalidraw = scene(
        json!({"type": "rectangle", "x": 1e308, "y": 1e308, "width": 1e308,
                                  "height": 1e308, "angle": 1e308}),
    );
    let sources = [
        ("x.drawio", drawio("rounded=1;", geometry).into_bytes()),
        ("x.bpmn", bpmn.to_vec()),
        ("x.excalidraw", excalidraw.into_bytes()),
    ];
    for (file, source) in sources {
        let dir = folder("hostile-inf");
        fs::write(dir.join(file), source).unwrap();
        let (code, out, _) = run_in(&dir, &[file, "-o", "x.svg", "--json"], b"");
        if code == 0 {
            let svg = fs::read_to_string(dir.join("x.svg")).unwrap();
            assert!(
                !svg.contains("inf") && !svg.contains("NaN"),
                "{file}: {out}"
            );
        }
    }
}

#[test]
#[ignore = "known bug: R1-011"]
fn cells_that_parent_each_other_are_refused_or_warned_about() {
    let source = r#"<mxGraphModel><root><mxCell id="a" parent="b" vertex="1" value="LOST A"><mxGeometry width="100" height="50"/></mxCell><mxCell id="b" parent="a" vertex="1" value="LOST B"><mxGeometry width="100" height="50"/></mxCell></root></mxGraphModel>"#;
    let v = survives("x.drawio", source.as_bytes(), &[]);
    assert!(v.get("error").is_some() || !codes(&v).is_empty(), "{v}");
}

#[test]
#[ignore = "known bug: R1-013"]
fn malformed_json_gets_a_parse_error() {
    let dir = folder("hostile-rpc");
    let mut mcp = Mcp::start(&dir);
    mcp.send("{bad}");
    let answer = mcp.next(Duration::from_secs(3)).expect("an answer");
    assert_eq!(answer["error"]["code"], -32700, "{answer}");
    assert_eq!(answer["id"], Value::Null);
}

/// `n` bytes of zeros, deflated in zlib format.
fn zeros(n: usize) -> Vec<u8> {
    let mut zlib = ZlibEncoder::new(vec![], Compression::fast());
    let block = vec![0u8; 1 << 20];
    for _ in 0..n >> 20 {
        zlib.write_all(&block).unwrap();
    }
    zlib.finish().unwrap()
}

/// A PNG of one pixel holding a chunk of `kind`.
fn png_with(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    let mut encoder = png::Encoder::new(&mut out, 1, 1);
    encoder.set_color(png::ColorType::Grayscale);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_chunk(png::chunk::ChunkType(kind), body)
        .unwrap();
    writer.write_image_data(&[0]).unwrap();
    drop(writer);
    out
}

#[test]
fn a_bomb_in_any_compressed_wrapper_is_too_large() {
    let bomb = zeros(300 << 20);
    for (name, key) in [
        ("x.drawio.png", &b"mxfile"[..]),
        ("x.excalidraw.png", &b"application/vnd.excalidraw+json"[..]),
    ] {
        let ztxt = [key, &[0, 0], &bomb].concat();
        let itxt = [key, &[0, 1, 0, 0, 0], &bomb].concat();
        for (kind, body) in [(*b"zTXt", ztxt), (*b"iTXt", itxt)] {
            let v = survives(name, &png_with(kind, &body), &[]);
            assert_eq!(v["error"]["code"], "too_large", "{name} {kind:?}: {v}");
        }
    }
}

/// A listener on a free port and every request it sees, on a thread of its own.
fn canary() -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (send, seen) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut head = vec![0; 512];
            let n = stream.read(&mut head).unwrap_or(0);
            let _ = send.send(String::from_utf8_lossy(&head[..n]).into_owned());
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    });
    (url, seen)
}

#[test]
fn nothing_named_in_a_diagram_is_fetched_without_the_option() {
    let (url, seen) = canary();
    let sources = [
        (
            "a.mmd",
            format!("flowchart TD\n A[\"<img src='{url}/1.png' />\"]\n B@{{ img: \"{url}/2.png\", label: \"x\", h: 60 }}\n"),
        ),
        (
            "b.mmd",
            format!("%%{{init: {{\"themeCSS\": \"@import url({url}/3.css);\"}}}}%%\nflowchart TD\n a-->b\n"),
        ),
        (
            "c.dot",
            format!("digraph {{a [image=\"{url}/4.png\" URL=\"{url}/5\"]; graph [stylesheet=\"{url}/6.css\"]}}\n"),
        ),
        ("d.d2", format!("a: {{shape: image; icon: {url}/7.png}}\nb: {{icon: {url}/8.svg}}\n")),
        ("e.drawio", drawio(&format!("shape=image;image={url}/9.png;"), r#"width="50" height="50""#)),
        (
            "f.excalidraw",
            json!({"type": "excalidraw", "version": 2,
                   "elements": [{"type": "image", "id": "i", "x": 0, "y": 0, "width": 9, "height": 9,
                                 "fileId": "f", "status": "saved"},
                                {"type": "embeddable", "id": "e", "x": 9, "y": 0, "width": 9, "height": 9,
                                 "link": format!("{url}/10")}],
                   "files": {"f": {"id": "f", "mimeType": "image/png", "dataURL": format!("{url}/11.png")}}})
            .to_string(),
        ),
    ];
    let dir = folder("hostile-offline");
    for (file, source) in sources {
        fs::write(dir.join(file), source).unwrap();
        for out in ["x.svg", "x.png"] {
            let (code, text, _) = run_in(&dir, &[file, "-o", out, "--json"], b"");
            assert!(code <= 1, "{file} {out}: {text}");
        }
    }
    thread::sleep(Duration::from_millis(200));
    assert_eq!(seen.try_iter().collect::<Vec<_>>(), Vec::<String>::new());
}

#[cfg(windows)]
#[test]
#[ignore = "known bug: T1-01"]
fn a_network_path_in_a_diagram_is_not_opened_without_the_option() {
    // The WebDAV redirector takes `//host@port/`; it answers a local port, so the test stays here.
    let sources = [
        (
            "a.dot",
            "digraph {{a [image=\"//127.0.0.1@{port}/s/x.png\"]}}",
        ),
        (
            "b.dot",
            "digraph {{a [shapefile=\"//127.0.0.1@{port}/s/x.png\"]}}",
        ),
        (
            "c.dot",
            "digraph {{graph [imagepath=\"//127.0.0.1@{port}/s\"]; a [image=\"x.png\"]}}",
        ),
        (
            "d.d2",
            "a: {{shape: image; icon: \"//127.0.0.1@{port}/s/x.png\"}}",
        ),
    ];
    let dir = folder("hostile-unc");
    for (file, source) in sources {
        let (url, seen) = canary();
        let port = url.rsplit(':').next().unwrap();
        let source = source
            .replace("{port}", port)
            .replace("{{", "{")
            .replace("}}", "}");
        fs::write(dir.join(file), source).unwrap();
        let _ = run_in(&dir, &[file, "-o", "x.svg", "--json"], b"");
        thread::sleep(Duration::from_millis(400));
        assert_eq!(seen.try_iter().count(), 0, "{file} reached the network");
    }
}

/// Serves `head` to whoever connects.
fn answering(head: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x.png", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let _ = stream.read(&mut [0; 2048]);
            let _ = stream.write_all(head.as_bytes());
        }
    });
    url
}

#[test]
#[ignore = "known bug: T1-05"]
fn a_broken_image_url_says_so_in_svg_and_png_alike() {
    let url = answering("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let dir = folder("hostile-404");
    fs::write(
        dir.join("x.d2"),
        format!("a: {{shape: image; icon: {url}}}\n"),
    )
    .unwrap();
    for out in ["x.svg", "x.png"] {
        let args = ["x.d2", "-o", out, "--json", "--allow-network"];
        let (code, text, _) = run_in(&dir, &args, b"");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(code != 0 || !codes(&v).is_empty(), "{out}: {text}");
    }
}

#[test]
#[ignore = "known bug: T1-06"]
fn an_answer_that_is_no_image_fails_the_svg_as_it_fails_the_png() {
    let dir = folder("hostile-not-image");
    for head in [
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 6\r\nConnection: close\r\n\r\n<html>",
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ] {
        let url = answering(head);
        let source = format!("a: {{shape: image; icon: {url}}}\n");
        fs::write(dir.join("x.d2"), source).unwrap();
        let (code, text, _) = run_in(&dir, &["x.d2", "--json", "--allow-network"], b"");
        assert_eq!(code, 1, "{text}");
    }
}

#[test]
fn allow_network_follows_a_redirect_and_stops_at_a_loop() {
    let dir = folder("hostile-redirect");
    let looping = answering(
        "HTTP/1.1 302 Found\r\nLocation: /x.png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    fs::write(
        dir.join("x.d2"),
        format!("a: {{shape: image; icon: {looping}}}\n"),
    )
    .unwrap();
    let (code, out, _) = run_in(&dir, &["x.d2", "--json", "--allow-network"], b"");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("redirects"), "{out}");
    let png = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/d2/pixel.png"))
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/moved.png", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut head = [0; 2048];
            let n = stream.read(&mut head).unwrap_or(0);
            let head = String::from_utf8_lossy(&head[..n]);
            let answer = if head.starts_with("GET /moved.png") {
                b"HTTP/1.1 302 Found\r\nLocation: /real.png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            } else {
                let start = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    png.len()
                );
                [start.as_bytes(), &png].concat()
            };
            let _ = stream.write_all(&answer);
        }
    });
    fs::write(
        dir.join("y.d2"),
        format!("a: {{shape: image; icon: {url}}}\n"),
    )
    .unwrap();
    let (code, out, _) = run_in(&dir, &["y.d2", "--json", "--allow-network"], b"");
    assert_eq!(code, 0, "{out}");
    let svg = fs::read_to_string(dir.join("y.svg")).unwrap();
    assert!(svg.contains("data:image/png;base64,"), "{out}");
}

#[test]
fn the_users_proxy_is_used_only_with_the_option() {
    let (proxy, seen) = canary();
    let dir = folder("hostile-proxy");
    let source = "a: {shape: image; icon: http://canary.invalid/x.png}\n";
    fs::write(dir.join("x.d2"), source).unwrap();
    let run_with = |extra: &[&str]| {
        let mut command = cmd(&[&["x.d2", "--json"], extra].concat());
        command.env("HTTP_PROXY", &proxy).env("http_proxy", &proxy);
        feed(command, &dir, b"")
    };
    let (code, out, _) = run_with(&[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("remote_images_not_fetched"), "{out}");
    assert_eq!(seen.try_iter().count(), 0);
    let _ = run_with(&["--allow-network"]);
    let request = seen.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        request.starts_with("GET http://canary.invalid/x.png "),
        "{request}"
    );
}
