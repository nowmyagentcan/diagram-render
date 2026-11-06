//! D2 through the d2 engine: the same SVG as d2 itself, boards, errors by character column,
//! no network unless asked, and nothing from the user's environment.

mod common;

use std::fs;
#[cfg(not(windows))]
use std::io::Read;
use std::io::Write;
#[cfg(not(windows))]
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use serde_json::{Value, json};

const FIXTURES: [&str; 11] = [
    "containers",
    "sql_table",
    "class",
    "sequence",
    "grid",
    "sketch",
    "layout-dagre",
    "layout-elk",
    "layout-tala",
    "image",
    "layers",
];

/// A fresh folder with every D2 fixture and the image one of them shows.
fn fixtures(test: &str) -> PathBuf {
    let dir = folder(test);
    for entry in fs::read_dir("tests/fixtures/d2").unwrap() {
        let path = entry.unwrap().path();
        fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
    }
    dir
}

fn d2() -> PathBuf {
    let bin = Path::new(env!("CARGO_BIN_EXE_diagram-render"));
    let exe = format!("d2{}", std::env::consts::EXE_SUFFIX);
    bin.parent().unwrap().join("engines").join(exe)
}

/// d2 run by hand in `dir` on `source` with `args`, with only what the system needs.
fn by_hand(dir: &Path, args: &[&str], source: &[u8]) -> Vec<u8> {
    let mut cmd = Command::new(d2());
    cmd.args(args).env_clear().current_dir(dir);
    for key in ["SystemRoot", "TEMP", "TMP"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    let mut child = (cmd.stdin(std::process::Stdio::piped()))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(source).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    out.stdout
}

#[test]
fn every_fixture_renders_as_d2_itself_renders_it() {
    let dir = fixtures("d2-render");
    let flags = [
        "--target=",
        "--scale",
        "1",
        "--timeout",
        "65",
        "--bundle=false",
        "-",
        "-",
    ];
    // On Windows a D2 image takes --allow-network.
    for name in FIXTURES.iter().filter(|n| !cfg!(windows) || **n != "image") {
        let input = format!("{name}.d2");
        let (code, out, err) = run_in(&dir, &[&input, "--json"], b"");
        assert_eq!((code, err.as_str()), (0, ""), "{name}: {out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            (&v["warnings"], &v["diagram_type"], &v["page"]),
            (&json!([]), &Value::Null, &Value::Null),
            "{name}"
        );
        let ours = fs::read(dir.join(format!("{name}.svg"))).unwrap();
        let source = fs::read(dir.join(&input)).unwrap();
        assert!(
            ours == by_hand(&dir, &flags, &source),
            "{name} differs from d2's own"
        );
        let text = String::from_utf8(ours).unwrap();
        let root = &text[text.find("<svg").unwrap()..];
        let root = &root[..root.find('>').unwrap()];
        assert!(
            root.contains(&format!(" width=\"{}\"", v["width"])),
            "{name}: {root}"
        );
    }
    if cfg!(windows) {
        let (code, out, _) = run_in(&dir, &["image.d2", "--json", "--allow-network"], b"");
        assert_eq!(code, 0, "{out}");
    }
    let svg = fs::read_to_string(dir.join("image.svg")).unwrap();
    assert!(
        svg.contains("<image href=\"data:image/png;base64,"),
        "local images are embedded"
    );
}

#[test]
fn a_board_is_chosen_with_page_and_named_in_the_output() {
    let dir = fixtures("d2-page");
    let (code, out, _) = run_in(
        &dir,
        &["layers.d2", "--page", "layers.deploy", "--json"],
        b"",
    );
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (&v["output"], &v["page"], &v["pages"]),
        (
            &json!("layers-layers.deploy.svg"),
            &json!({"index": null, "name": "layers.deploy"}),
            &json!([])
        )
    );
    let svg = fs::read_to_string(dir.join("layers-layers.deploy.svg")).unwrap();
    assert!(svg.contains(">build<") && !svg.contains(">root<"));
    let (code, _, err) = run_in(&dir, &["layers.d2", "--page", "nope"], b"");
    assert_eq!(
        (code, err.as_str()),
        (
            1,
            "✗ layers.d2: D2 found no board nope.\n  Pass --page with a board's path, such as layers.deploy, or leave it out for the root board.\n"
        )
    );
    let (code, out, _) = run_in(
        &dir,
        &["check", "layers.d2", "--page", "nope", "--json"],
        b"",
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (code, &v["errors"][0]["code"]),
        (1, &json!("page_not_found"))
    );
}

#[test]
fn check_lists_every_error_at_its_character_column() {
    let dir = folder("d2-errors");
    let source = "a.shape: nosuch\nb -> c\n\"é漢\".style.opacity: 7\n";
    let crlf = format!("\u{FEFF}{}", source.replace('\n', "\r\n"));
    for text in [source.to_string(), crlf] {
        fs::write(dir.join("bad.d2"), &text).unwrap();
        let (code, out, _) = run_in(&dir, &["check", "bad.d2", "--json"], b"");
        assert_eq!(code, 1);
        let v: Value = serde_json::from_str(&out).unwrap();
        let places: Vec<_> = (v["errors"].as_array().unwrap().iter())
            .map(|e| (e["line"].clone(), e["column"].clone(), e["message"].clone()))
            .collect();
        assert_eq!(
            places,
            [
                (json!(1), json!(10), json!("unknown shape \"nosuch\"")),
                (
                    json!(3),
                    json!(21),
                    json!("expected \"opacity\" to be a number between 0.0 and 1.0")
                ),
            ],
            "{text:?}"
        );
        let (code, out, _) = run_in(&dir, &["bad.d2", "--json"], b"");
        let e = &serde_json::from_str::<Value>(&out).unwrap()["error"];
        assert_eq!((code, &e["line"], &e["column"]), (1, &json!(1), &json!(10)));
        assert_eq!(
            e["hint"],
            "Fix line 1, then run again. `diagram-render check bad.d2` lists every error."
        );
    }
    let (code, _, err) = run_in(&dir, &["check", "bad.d2"], b"");
    assert_eq!(code, 1);
    assert!(
        err.ends_with("  Fix line 3, then run again.\n2 errors · 0 warnings\n"),
        "{err}"
    );
    let (code, out, _) = run_in(&dir, &["check", "-", "--format", "d2"], b"a -> b\n");
    assert_eq!((code, out.as_str()), (0, "✓ - is valid D2\n"));
}

#[test]
fn the_users_environment_and_plugins_never_reach_d2() {
    let dir = fixtures("d2-isolation");
    let plugins = dir.join("plugins");
    fs::create_dir(&plugins).unwrap();
    let marker = plugins.join("ran.txt");
    plant_plugin(&plugins);
    let path = std::env::join_paths(
        [plugins.clone()]
            .into_iter()
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let hostile = [
        ("D2_LAYOUT", "evil".into()),
        ("SCALE", "3".into()),
        ("PATH", path),
    ];
    // d2 itself, given these, runs the plugin.
    let mut own = Command::new(d2());
    own.args(["--target=", "-", "-"])
        .current_dir(&dir)
        .envs(hostile.clone());
    let _ = feed(own, &dir, b"a -> b");
    assert!(marker.exists(), "the plugin is one d2 would run");
    fs::remove_file(&marker).unwrap();
    let (code, ..) = run_in(&dir, &["containers.d2", "-o", "plain.svg"], b"");
    assert_eq!(code, 0);
    let mut cmd = cmd(&["containers.d2", "-o", "hostile.svg"]);
    cmd.envs(hostile);
    let (code, _, err) = feed(cmd, &dir, b"");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        fs::read(dir.join("hostile.svg")).unwrap(),
        fs::read(dir.join("plain.svg")).unwrap()
    );
    assert!(!marker.exists(), "the plugin never ran");
}

/// A `d2plugin-evil` in `dir` that leaves `ran.txt` beside itself when run.
#[cfg(windows)]
fn plant_plugin(dir: &Path) {
    let script = "@echo off
echo ran> \"%~dp0ran.txt\"
exit /b 1
";
    fs::write(dir.join("d2plugin-evil.cmd"), script).unwrap();
}

#[cfg(unix)]
fn plant_plugin(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let plugin = dir.join("d2plugin-evil");
    let script = "#!/bin/sh
echo ran > \"$(dirname \"$0\")/ran.txt\"
exit 1
";
    fs::write(&plugin, script).unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A listener on a free local port, and the URL of an image on it.
#[cfg(not(windows))]
fn image_server() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/pixel.png", listener.local_addr().unwrap());
    (listener, url)
}

#[cfg(not(windows))]
#[test]
fn remote_images_stay_links_offline_and_are_fetched_with_allow_network() {
    let dir = fixtures("d2-network");
    let (listener, url) = image_server();
    listener.set_nonblocking(true).unwrap();
    let source = format!("logo: {{\n  shape: image\n  icon: {url}\n}}\n");
    let (code, out, _) = run_in(
        &dir,
        &["-", "--format", "d2", "-o", "x.svg", "--json"],
        source.as_bytes(),
    );
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["warnings"],
        json!([{"code": "remote_images_not_fetched", "message": "Left 1 remote image as links; a browser loads them when it shows the SVG. Use --allow-network to embed them."}])
    );
    assert!(
        fs::read_to_string(dir.join("x.svg"))
            .unwrap()
            .contains(&format!("href=\"{url}\""))
    );
    assert!(listener.accept().is_err(), "no request reached the server");
    listener.set_nonblocking(false).unwrap();
    let png = fs::read(dir.join("pixel.png")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut request = vec![0; 4096];
        let n = conn.read(&mut request).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            png.len()
        );
        conn.write_all(&[head.as_bytes(), &png].concat()).unwrap();
        String::from_utf8_lossy(&request[..n]).into_owned()
    });
    let args = [
        "-",
        "--format",
        "d2",
        "-o",
        "y.svg",
        "--json",
        "--allow-network",
    ];
    let (code, out, _) = run_in(&dir, &args, source.as_bytes());
    assert_eq!(code, 0, "{out}");
    assert!(server.join().unwrap().starts_with("GET /pixel.png "));
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["warnings"],
        json!([])
    );
    let svg = fs::read_to_string(dir.join("y.svg")).unwrap();
    assert!(svg.contains("<image href=\"data:image/png;base64,") && !svg.contains(&url));
}

#[test]
fn script_links_go_and_background_is_d2_s_own() {
    let dir = folder("d2-unsafe");
    let source = b"a: {link: \"javascript:alert(1)\"}\nb: {link: https://example.org}\na -> b\n";
    let args = [
        "-",
        "--format",
        "d2",
        "-o",
        "x.svg",
        "--json",
        "--background",
        "white",
    ];
    let (code, out, _) = run_in(&dir, &args, source);
    assert_eq!(code, 0);
    let svg = fs::read_to_string(dir.join("x.svg")).unwrap();
    assert!(!svg.contains("javascript:") && svg.contains("href=\"https://example.org\""));
    let codes: Vec<_> = (serde_json::from_str::<Value>(&out).unwrap()["warnings"]
        .as_array()
        .unwrap())
    .iter()
    .map(|w| w["code"].clone())
    .collect();
    assert_eq!(
        codes,
        [json!("background_ignored"), json!("unsafe_content_removed")]
    );
}
