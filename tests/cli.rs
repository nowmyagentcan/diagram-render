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
        (&["a.mmd", "--json"], 3),
        (&["check", "a.mmd", "--json"], 3),
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
