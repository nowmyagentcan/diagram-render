//! Runs the built binary the way people and agents do.

use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn cmd(args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_diagram-render"));
    cmd.args(args).env_remove("NO_COLOR").stdin(Stdio::null());
    cmd
}

/// Exit code, stdout and stderr.
fn run(args: &[&str]) -> (i32, String, String) {
    let out = cmd(args).output().unwrap();
    let text = |b| String::from_utf8(b).unwrap();
    (
        out.status.code().unwrap(),
        text(out.stdout),
        text(out.stderr),
    )
}

/// The `--json` answer: one line on stdout, nothing on stderr.
fn json(args: &[&str]) -> (i32, Value) {
    let (code, out, err) = run(args);
    assert_eq!(err, "", "{args:?}");
    assert_eq!(out.matches('\n').count(), 1, "one line: {out}");
    (code, serde_json::from_str(&out).unwrap())
}

#[test]
fn version_names_the_tool() {
    for flag in ["--version", "-V"] {
        assert_eq!(
            run(&[flag]),
            (0, "diagram-render 0.1.0\n".into(), "".into())
        );
    }
}

#[test]
fn help_shows_the_sentence_options_and_examples() {
    let (code, out, err) = run(&["--help"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("Render Mermaid, DOT, D2, BPMN, draw.io and Excalidraw diagrams.\n"));
    assert!(out.contains("  -o, --output <FILE>"));
    assert!(out.contains("  diagram-render board.drawio --page 2 -o page2.svg\n"));
    assert_eq!(run(&["flow.mmd", "-h"]).1, out);
    assert_eq!(run(&["render", "--help"]).1, out);
    let check = run(&["check", "--help"]).1;
    assert!(check.starts_with("Check a diagram for errors without rendering it.\n"));
    assert!(!check.contains("--scale"));
}

#[test]
fn usage_errors_are_two_lines_and_exit_2() {
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &[],
            "No diagram given.",
            "Try: diagram-render flow.mmd, or diagram-render --help.",
        ),
        (
            &["a.mmd", "b.mmd"],
            "Give one diagram per run.",
            "Render the others in their own runs, for example in a shell loop.",
        ),
        (
            &["a.mmd", "--scale", "9"],
            "--scale must be between 0.1 and 8.",
            "Try --scale 2.",
        ),
        (
            &["a.mmd", "--scale", "1.234"],
            "--scale takes at most 2 decimals.",
            "Try --scale 1.25.",
        ),
        (
            &["a.mmd", "--scale"],
            "--scale needs a value.",
            "Try --scale 2.",
        ),
        (
            &["a.mmd", "-o"],
            "--output needs a value.",
            "Try -o flow.png.",
        ),
        (
            &["a.mmd", "--to", "pdf"],
            "--to must be svg or png.",
            "Try --to png.",
        ),
        (
            &["a.mmd", "--format", "plantuml"],
            "--format must be auto, mermaid, dot, d2, bpmn, drawio or excalidraw.",
            "Try --format mermaid.",
        ),
        (
            &["a.mmd", "--background", "black"],
            "--background must be auto, transparent, white, #RGB or #RRGGBB.",
            "Try --background white.",
        ),
        (
            &["a.mmd", "--timeout", "0"],
            "--timeout must be a whole number of seconds from 1 to 600.",
            "Try --timeout 120.",
        ),
        (
            &["a.mmd", "--page", ""],
            "--page needs a page number or name.",
            "Try --page 2.",
        ),
        (
            &["a.mmd", "--bogus"],
            "Unknown option --bogus.",
            "Run diagram-render --help for the options.",
        ),
        (
            &["a.mmd", "-x"],
            "Unknown option -x.",
            "Run diagram-render --help for the options.",
        ),
        (
            &["a.mmd", "--debug=yes"],
            "--debug takes no value.",
            "Write --debug on its own.",
        ),
        (
            &["a.mmd", "--to", "png", "--to", "svg"],
            "--to is given twice.",
            "Give each option once.",
        ),
        (
            &["check", "a.mmd", "-o", "a.svg"],
            "--output doesn't apply to check.",
            "Run diagram-render check --help for its options.",
        ),
        (
            &["check", "a.mmd", "--version"],
            "Unknown option --version.",
            "Run diagram-render check --help for the options.",
        ),
    ];
    for (args, message, hint) in cases {
        let (code, out, err) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(out, "", "{args:?}");
        assert_eq!(err, format!("✗ {message}\n  {hint}\n"), "{args:?}");
        // --json goes first (after the command), so options missing their value stay that way.
        let mut with_json = args.to_vec();
        with_json.insert(usize::from(args.first() == Some(&"check")), "--json");
        let (code, v) = json(&with_json);
        let code_name = if args.is_empty() {
            "no_input"
        } else if args.len() == 2 && !args[1].starts_with('-') {
            "one_input_only"
        } else {
            "invalid_option"
        };
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(
            v,
            json!({"error": {"code": code_name, "message": message, "hint": hint}})
        );
    }
}

#[test]
fn json_after_double_dash_is_an_input_not_a_flag() {
    let (code, out, err) = run(&["--", "--json", "x"]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.starts_with("✗ Give one diagram per run.\n"));
}

#[test]
fn only_an_exact_first_word_is_a_command() {
    // `./check` and a second-place `check` are inputs.
    let (_, v) = json(&["./check", "--json"]);
    assert_eq!(v["input"], "./check");
    let (code, _, err) = run(&["a.mmd", "check"]);
    assert_eq!(code, 2);
    assert!(err.starts_with("✗ Give one diagram per run.\n"));
}

/// A pipe whose reading end is already closed, so every write to it fails.
fn closed() -> Stdio {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    writer.into()
}

#[test]
fn closed_stdout_or_stderr_ends_quietly_with_the_run_s_code() {
    let cases: &[(&[&str], i32)] = &[
        (&["--version"], 0),
        (&["--help"], 0),
        (&["check", "--help"], 0),
        (&["--scale", "9"], 2),
        (&["--scale", "9", "--json"], 2),
        (&["a.mmd", "--json"], 1),
        (&["check", "a.mmd", "--json"], 1),
        (&["formats"], 0),
        (&["formats", "--json"], 0),
        (&["formats", "--debug"], 0),
    ];
    for (args, want) in cases {
        let out = cmd(args).stdout(closed()).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(*want),
            "{args:?} with stdout closed"
        );
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("panicked"),
            "{args:?}"
        );
        let out = cmd(args).stderr(closed()).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(*want),
            "{args:?} with stderr closed"
        );
    }
}

/// A fresh, empty folder for one test.
fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("diagram-render-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs in `dir` with `stdin` as input: exit code, stdout and stderr.
fn run_in(dir: &std::path::Path, args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    use std::io::Write;
    let mut child = cmd(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    let stdin = stdin.to_vec();
    let feeder = std::thread::spawn(move || {
        let _ = pipe.write_all(&stdin);
    });
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    let text = |b| String::from_utf8(b).unwrap();
    (
        out.status.code().unwrap(),
        text(out.stdout),
        text(out.stderr),
    )
}

#[test]
fn input_errors_name_the_file_and_what_to_do() {
    let dir = folder("input-errors");
    std::fs::create_dir(dir.join("sub.mmd")).unwrap();
    std::fs::write(dir.join("blank.mmd"), " \r\n\t\r\n").unwrap();
    let big = std::fs::File::create(dir.join("big.excalidraw")).unwrap();
    big.set_len((64 << 20) + 1).unwrap();
    drop(big);
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "missing.mmd",
            "file_not_found",
            "Can't find missing.mmd.",
            "Check the path; relative paths start from ",
        ),
        (
            "sub.mmd",
            "not_a_file",
            "sub.mmd is a folder.",
            "Give one diagram file.",
        ),
        (
            "blank.mmd",
            "empty_input",
            "The diagram is empty.",
            "Write some diagram source, or check the file.",
        ),
        (
            "big.excalidraw",
            "too_large",
            "big.excalidraw is 64.0 MiB; the limit is 64 MiB (4 MiB for Mermaid, DOT and D2 text).",
            "Split the diagram, or remove embedded images.",
        ),
    ];
    for (file, code, message, hint) in cases {
        for command in [&["render"][..], &["check"], &[]] {
            let args = [command, &[*file]].concat();
            let (exit, out, err) = run_in(&dir, &args, b"");
            assert_eq!((exit, out.as_str()), (1, ""), "{args:?}");
            let (head, tail) = err.split_once('\n').unwrap();
            assert_eq!(head, format!("✗ {file}: {message}"));
            assert!(tail.starts_with(&format!("  {hint}")), "{tail}");
            let (exit, out, err) = run_in(&dir, &[&args[..], &["--json"]].concat(), b"");
            assert_eq!((exit, err.as_str()), (1, ""));
            let v: Value = serde_json::from_str(&out).unwrap();
            let e = if command == ["check"] {
                &v["errors"][0]
            } else {
                &v["error"]
            };
            assert_eq!(
                (e["code"].as_str(), e["message"].as_str()),
                (Some(*code), Some(*message))
            );
            assert!(e["hint"].as_str().unwrap().starts_with(hint));
            assert_eq!(
                (&e["line"], &e["column"], &e["excerpt"]),
                (&Value::Null, &Value::Null, &Value::Null)
            );
            assert_eq!((&v["input"], &v["format"]), (&json!(file), &Value::Null));
        }
    }
}

#[test]
fn stdin_too_large_or_empty_says_stdin() {
    let dir = folder("stdin-limits");
    let (code, _, err) = run_in(&dir, &["-"], &vec![b'a'; (64 << 20) + 1]);
    assert_eq!(code, 1);
    assert!(
        err.starts_with("✗ -: stdin is over 64 MiB; the limit is 64 MiB"),
        "{err}"
    );
    let (code, out, _) = run_in(&dir, &["-", "-o", "x.svg", "--json"], b"");
    assert_eq!(code, 1);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (v["input"].as_str(), v["error"]["code"].as_str()),
        (Some("-"), Some("empty_input"))
    );
}

#[test]
fn bad_utf8_points_at_the_same_place_whatever_the_line_ends_and_bom() {
    let dir = folder("not-utf8");
    let lf = b"graph TD\n  A --> B\n  B --> \xE9t\xE9\n".to_vec();
    let crlf = [
        b"\xEF\xBB\xBF",
        &lf.iter()
            .flat_map(|&b| {
                if b == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![b]
                }
            })
            .collect::<Vec<_>>()[..],
    ]
    .concat();
    let cr: Vec<u8> = lf
        .iter()
        .map(|&b| if b == b'\n' { b'\r' } else { b })
        .collect();
    for (name, bytes) in [("lf.mmd", lf), ("crlf.mmd", crlf), ("cr.mmd", cr)] {
        std::fs::write(dir.join(name), &bytes).unwrap();
        let (code, _, err) = run_in(&dir, &[name], b"");
        assert_eq!(code, 1);
        assert_eq!(
            err,
            format!(
                "✗ {name}:3:9: Line 3 isn't valid UTF-8 text.\n    3 |   B --> \u{FFFD}t\u{FFFD}\n      |         ^\n  Save the file as UTF-8.\n"
            )
        );
        let (_, out, _) = run_in(&dir, &[name, "--json"], b"");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["error"],
            json!({"code": "not_utf8", "message": "Line 3 isn't valid UTF-8 text.", "hint": "Save the file as UTF-8.", "line": 3, "column": 9, "excerpt": "  B --> \u{FFFD}t\u{FFFD}"})
        );
    }
}

#[test]
fn an_unreadable_file_is_read_failed() {
    let dir = folder("unreadable");
    let path = dir.join("locked.mmd");
    std::fs::write(&path, "graph TD\n").unwrap();
    #[cfg(windows)]
    let _lock = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            return; // root reads anything, so there is nothing to test.
        }
    }
    let (code, _, err) = run_in(&dir, &["locked.mmd"], b"");
    assert_eq!(code, 1);
    assert!(
        err.starts_with("✗ locked.mmd: Can't read locked.mmd: "),
        "{err}"
    );
    assert!(
        err.ends_with(".\n  Check that the file isn't locked and that you can open it.\n"),
        "{err}"
    );
    assert!(!err.contains("os error"), "{err}");
}

#[test]
fn detection_reports_the_format_its_engine_and_the_guess() {
    let dir = folder("detect");
    std::fs::write(dir.join("notes.txt"), "a -> b\n").unwrap();
    std::fs::write(dir.join("flow.mmd"), "flowchart LR\n  a --> b\n").unwrap();
    let cases: &[(&[&str], &[u8], &str, &str)] = &[
        (
            &["-", "-o", "x.svg"],
            b"graph TD; A-->B",
            "mermaid",
            "flowchart",
        ),
        (&["check", "-"], b"digraph { a -> b }", "dot", "digraph"),
        (&["check", "flow.mmd"], b"", "mermaid", "flowchart"),
        (
            &["check", "-", "--format", "graphviz"],
            b"graph G { a }",
            "dot",
            "graph",
        ),
        (&["check", "-"], b"<mxfile/>", "drawio", ""),
    ];
    let engines = json!({
        "mermaid": {"name": "merman", "version": "0.9.3"},
        "dot": {"name": "graphviz", "version": "16.1.0"},
        "drawio": {"name": "diagram-render", "version": "0.1.0"},
    });
    for (args, stdin, format, kind) in cases {
        let (_, out, err) = run_in(&dir, &[*args, &["--json"]].concat(), stdin);
        assert_eq!(err, "", "{args:?}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            (&v["format"], &v["engine"]),
            (&json!(format), &engines[format]),
            "{args:?}"
        );
        assert_eq!(v["warnings"], json!([]));
        if args[0] == "check" {
            assert_eq!(
                v["diagram_type"],
                if kind.is_empty() {
                    Value::Null
                } else {
                    json!(kind)
                }
            );
        }
    }
    let (_, out, _) = run_in(&dir, &["notes.txt", "--json"], b"");
    let v: Value = serde_json::from_str(&out).unwrap();
    let guessed = "Read as D2 (no Mermaid or DOT header found); pass --format to choose.";
    assert_eq!(
        (&v["format"], &v["warnings"]),
        (
            &json!("d2"),
            &json!([{"code": "format_guessed", "message": guessed}])
        )
    );
    let (_, _, err) = run_in(&dir, &["notes.txt"], b"");
    assert!(
        err.starts_with(&format!("! notes.txt: {guessed}\n✗ notes.txt: ")),
        "{err}"
    );
}

#[test]
fn formats_the_tool_doesnt_read_are_refused_by_name() {
    let dir = folder("refused");
    std::fs::write(dir.join("README.md"), "# Notes\n").unwrap();
    std::fs::write(dir.join("page.html"), "<html><body/></html>").unwrap();
    std::fs::write(
        dir.join("logo.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    let cases = [
        (
            "README.md",
            "unsupported_input",
            "Markdown files aren't rendered as a whole.",
            "Copy one diagram into its own .mmd file, or pipe it in with --format mermaid.",
        ),
        (
            "page.html",
            "unknown_format",
            "Can't tell which diagram language this is.",
            "Pass --format mermaid, dot, d2, bpmn, drawio or excalidraw.",
        ),
        (
            "logo.svg",
            "unsupported_input",
            "logo.svg is an image with no draw.io or Excalidraw diagram inside.",
            "Nothing to render; it's already an image.",
        ),
    ];
    for (file, code, message, hint) in cases {
        let (exit, _, err) = run_in(&dir, &[file], b"");
        assert_eq!(exit, 1);
        assert_eq!(err, format!("✗ {file}: {message}\n  {hint}\n"));
        let (_, out, _) = run_in(&dir, &["check", file, "--json"], b"");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            (v["errors"][0]["code"].as_str(), &v["format"]),
            (Some(code), &Value::Null)
        );
    }
    // --format wins over the refusal.
    let (_, out, _) = run_in(&dir, &["README.md", "--format", "mermaid", "--json"], b"");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["format"], "mermaid");
}

#[test]
fn page_is_a_usage_error_for_formats_without_pages() {
    let dir = folder("pages");
    for (stdin, name) in [
        (&b"graph TD"[..], "Mermaid"),
        (b"digraph {}", "Graphviz DOT"),
        (br#"{"type":"excalidraw"}"#, "Excalidraw"),
    ] {
        let (code, out, err) = run_in(&dir, &["-", "--page", "2"], stdin);
        assert_eq!((code, out.as_str()), (2, ""));
        assert_eq!(
            err,
            format!("✗ {name} diagrams have no pages.\n  Drop --page.\n")
        );
        let (code, out, _) = run_in(&dir, &["check", "-", "--page", "x", "--json"], stdin);
        assert_eq!(code, 2);
        let message = format!("{name} diagrams have no pages.");
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({"error": {"code": "page_not_supported", "message": message, "hint": "Drop --page."}})
        );
    }
    let (_, out, _) = run_in(
        &dir,
        &["-", "--page", "2", "-o", "x.svg", "--json"],
        b"a -> b",
    );
    assert_ne!(
        serde_json::from_str::<Value>(&out).unwrap()["error"]["code"],
        "page_not_supported"
    );
}

#[test]
fn output_choices_that_cant_work_are_usage_errors() {
    let dir = folder("outputs");
    let drawio =
        r#"<svg xmlns="http://www.w3.org/2000/svg" content="&lt;mxfile&gt;&lt;/mxfile&gt;"/>"#;
    std::fs::write(dir.join("board.svg"), drawio).unwrap();
    let cases: &[(&[&str], &str, &str, &str)] = &[
        (
            &["a.mmd", "-o", "a.png", "--to", "svg"],
            "format_conflict",
            "-o says PNG but --to says SVG.",
            "Drop --to, or change the extension of -o.",
        ),
        (
            &["a.mmd", "-o", "a.jpg"],
            "bad_extension",
            "The output must end in .svg or .png.",
            "For example: -o flow.png.",
        ),
        (
            &["-", "--json"],
            "json_needs_output",
            "--json needs -o FILE when the image would go to stdout.",
            "Add -o flow.svg, or drop --json.",
        ),
        (
            &["a.mmd", "-o", "-", "--json"],
            "json_needs_output",
            "--json needs -o FILE when the image would go to stdout.",
            "Add -o flow.svg, or drop --json.",
        ),
        (
            &["board.svg"],
            "output_is_input",
            "The output would replace the input, board.svg.",
            "Pass -o with another file name.",
        ),
        (
            &["board.svg", "-o", "./board.svg"],
            "output_is_input",
            "The output would replace the input, ./board.svg.",
            "Pass -o with another file name.",
        ),
    ];
    for (args, code, message, hint) in cases {
        let (exit, out, err) = run_in(&dir, args, b"graph TD");
        assert_eq!(exit, 2, "{args:?}");
        if args.contains(&"--json") {
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(
                v,
                json!({"error": {"code": code, "message": message, "hint": hint}})
            );
        } else {
            assert_eq!(err, format!("✗ {message}\n  {hint}\n"), "{args:?}");
        }
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("board.svg")).unwrap(),
        drawio
    );
    // check writes nothing, so it has no output to refuse.
    let (_, out, _) = run_in(&dir, &["check", "-", "--json"], b"graph TD");
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["format"],
        "mermaid"
    );
}

#[test]
fn scale_with_svg_output_is_a_warning() {
    let dir = folder("scale");
    let (_, out, _) = run_in(
        &dir,
        &["-", "--scale", "3", "-o", "x.svg", "--json"],
        b"graph TD",
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["warnings"],
        json!([{"code": "scale_ignored", "message": "--scale applies to PNG only."}])
    );
    let (_, out, _) = run_in(
        &dir,
        &["-", "--scale", "3", "-o", "x.png", "--json"],
        b"graph TD",
    );
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["warnings"],
        json!([])
    );
}

/// A copy of the binary in a folder of its own, whose `engines/` holds `engines` (each a copy of
/// the binary under that name, so it answers `--version` but fails anything else).
fn installed(name: &str, engines: &[&str]) -> std::path::PathBuf {
    let dir = folder(name);
    let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let bin = dir.join(exe("diagram-render"));
    std::fs::copy(env!("CARGO_BIN_EXE_diagram-render"), &bin).unwrap();
    std::fs::create_dir(dir.join("engines")).unwrap();
    for engine in engines {
        std::fs::copy(&bin, dir.join("engines").join(exe(engine))).unwrap();
    }
    bin
}

fn run_at(bin: &std::path::Path, args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    use std::io::Write;
    let mut child = Command::new(bin)
        .args(args)
        .current_dir(bin.parent().unwrap())
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    let text = |b| String::from_utf8(b).unwrap();
    (
        out.status.code().unwrap(),
        text(out.stdout),
        text(out.stderr),
    )
}

#[test]
fn formats_lists_every_format_with_its_engine_running_here() {
    let (code, v) = json(&["formats", "--json"]);
    assert_eq!(code, 0);
    let rows = v["formats"].as_array().unwrap();
    let ids: Vec<_> = rows.iter().map(|r| r["format"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        ["mermaid", "dot", "d2", "bpmn", "drawio", "excalidraw"]
    );
    for row in rows {
        assert_eq!(
            (&row["available"], &row["problem"]),
            (&json!(true), &Value::Null),
            "{row}"
        );
    }
    assert_eq!(
        rows[4],
        json!({"format": "drawio", "name": "draw.io", "extensions": [".drawio", ".dio", ".drawio.xml", ".drawio.svg", ".drawio.png"], "engine": {"name": "diagram-render", "version": "0.1.0"}, "available": true, "problem": null})
    );
    let families = rows[0]["families"].as_array().unwrap();
    for family in ["flowchart", "sequence", "class", "zenuml"] {
        assert!(families.contains(&json!(family)), "{family}");
    }
    assert!(rows[1].get("families").is_none());
    let (code, out, err) = run(&["formats"]);
    assert_eq!((code, err.as_str()), (0, ""));
    let lines: Vec<_> = out.lines().collect();
    assert_eq!(
        lines[0],
        format!(
            "mermaid     merman 0.9.3          .mmd .mermaid{}ok",
            " ".repeat(49)
        )
    );
    assert_eq!(
        lines[5],
        "excalidraw  diagram-render 0.1.0  .excalidraw .excalidraw.json .excalidraw.svg .excalidraw.png  ok"
    );
    let (_, _, err) = run(&["formats", "--debug"]);
    assert!(
        err.contains(" -V\n") && err.contains("debug: dot - graphviz version 16.1.0"),
        "{err}"
    );
}

#[test]
fn formats_names_each_engine_that_is_missing_or_fails() {
    let bin = installed("no-engines", &["merman-cli"]);
    let (code, out, _) = run_at(&bin, &["formats", "--json"], b"");
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    let engines = bin.parent().unwrap().join("engines");
    let dot = engines.join(format!("dot{}", std::env::consts::EXE_SUFFIX));
    let problem = |i: usize| v["formats"][i]["problem"].as_str().unwrap().to_string();
    assert_eq!(v["formats"][1]["available"], false);
    assert_eq!(
        problem(1),
        format!(
            "The Graphviz engine is missing: {} doesn't exist.",
            dot.display()
        )
    );
    assert!(problem(2).starts_with("The D2 engine is missing: "));
    // This stand-in answers --version but not `capabilities --json`.
    assert_eq!(problem(0), "Mermaid stopped unexpectedly (exit code 1).");
    assert_eq!(v["formats"][3]["available"], true);
    let (_, out, _) = run_at(&bin, &["formats"], b"");
    assert!(out.lines().nth(1).unwrap().ends_with(&format!(
        "  The Graphviz engine is missing: {} doesn't exist.",
        dot.display()
    )));
    let (_, _, err) = run_at(&bin, &["formats", "--debug"], b"");
    assert!(
        err.contains("capabilities --json\ndebug: exit code 1 after "),
        "{err}"
    );
}

#[test]
fn rendering_without_its_engine_is_engine_missing() {
    let bin = installed("render-no-engine", &[]);
    for (stdin, label, file) in [
        (&b"graph TD"[..], "Mermaid", "merman-cli"),
        (b"digraph {}", "Graphviz", "dot"),
        (b"a -> b", "D2", "d2"),
    ] {
        let exe = bin
            .parent()
            .unwrap()
            .join("engines")
            .join(format!("{file}{}", std::env::consts::EXE_SUFFIX));
        let message = format!(
            "The {label} engine is missing: {} doesn't exist.",
            exe.display()
        );
        let (code, _, err) = run_at(&bin, &["-", "-o", "x.svg"], stdin);
        assert_eq!(code, 3);
        assert!(
            err.ends_with(&format!("✗ -: {message}\n  Reinstall diagram-render.\n")),
            "{err}"
        );
        let (code, out, _) = run_at(&bin, &["check", "-", "--json"], stdin);
        assert_eq!(code, 3);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            (
                v["errors"][0]["code"].as_str(),
                v["errors"][0]["message"].as_str()
            ),
            (Some("engine_missing"), Some(message.as_str()))
        );
    }
}

#[test]
fn formats_takes_only_its_own_options() {
    assert_eq!(
        run(&["formats", "x"]),
        (
            2,
            "".into(),
            "✗ formats takes no file.\n  Run diagram-render formats on its own.\n".into()
        )
    );
    let (code, _, err) = run(&["formats", "--page", "2"]);
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "✗ --page doesn't apply to formats.\n  Run diagram-render formats --help for its options.\n"
        )
    );
    assert!(
        run(&["formats", "-h"])
            .1
            .starts_with("List the formats and whether each engine runs here.\n")
    );
}
