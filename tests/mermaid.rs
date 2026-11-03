//! Mermaid through the merman engine: renders, errors by line and column, and how outputs are
//! written.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use serde_json::{Value, json};

const FAMILIES: [&str; 5] = ["flowchart", "sequence", "class", "state", "er"];

/// A fresh folder holding copies of the named Mermaid fixtures.
fn fixtures(test: &str, names: &[&str]) -> std::path::PathBuf {
    let dir = folder(test);
    for name in names {
        let from = Path::new("tests/fixtures/mermaid").join(format!("{name}.mmd"));
        fs::copy(from, dir.join(format!("{name}.mmd"))).unwrap();
    }
    dir
}

#[test]
fn each_family_renders_to_svg_beside_its_source() {
    let dir = fixtures("mermaid-render", &FAMILIES);
    for family in FAMILIES {
        let input = format!("{family}.mmd");
        let (code, out, err) = run_in(&dir, &[&input, "--json"], b"");
        assert_eq!((code, err.as_str()), (0, ""), "{family}: {out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        let svg = fs::read_to_string(dir.join(format!("{family}.svg"))).unwrap();
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert_eq!(
            (&v["diagram_type"], &v["output"], &v["warnings"]),
            (&json!(family), &json!(format!("{family}.svg")), &json!([]))
        );
        assert_eq!(v["bytes"], svg.len());
        assert!(v["width"].as_u64().unwrap() > 50 && v["height"].as_u64().unwrap() > 50);
        let (code, out, _) = run_in(&dir, &["check", &input], b"");
        assert_eq!(
            (code, out),
            (0, format!("✓ {input} is valid Mermaid ({family})\n"))
        );
    }
    let (code, out, err) = run_in(&dir, &["flowchart.mmd", "-o", "out dir.svg"], b"");
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(
        out.starts_with("✓ flowchart.mmd → out dir.svg  ") && out.ends_with(" s\n"),
        "{out}"
    );
    let fields: Vec<_> = out.trim_end().split("  ").collect();
    assert_eq!(fields.len(), 4, "{out}");
    assert!(fields[2].ends_with(" KB"), "{out}");
}

#[test]
fn stdin_renders_to_stdout_with_the_line_on_stderr() {
    let dir = folder("mermaid-stdin");
    let (code, out, err) = run_in(&dir, &["-"], b"graph TD\n  A-->B\n");
    assert_eq!(code, 0);
    assert!(out.starts_with("<svg") && out.trim_end().ends_with("</svg>"));
    assert!(
        err.starts_with("✓ - → -  ") && err.ends_with(" s\n"),
        "{err}"
    );
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn background_sets_the_svg_background() {
    let dir = folder("mermaid-background");
    for (background, css) in [
        ("auto", "background-color: white;"),
        ("transparent", "background-color: transparent;"),
        ("#369", "background-color: #336699;"),
    ] {
        let args = ["-", "-o", "bg.svg", "--background", background];
        assert_eq!(run_in(&dir, &args, b"graph TD\n  A-->B\n").0, 0);
        let svg = fs::read_to_string(dir.join("bg.svg")).unwrap();
        assert!(svg[..svg.find('>').unwrap()].contains(css), "{background}");
    }
}

/// Sources with an error, the line and column it is at, and its message.
const BROKEN: [(&str, usize, usize, &str); 5] = [
    (
        "flowchart TD\n  A[\"é漢\"] --> B(()\n  B --> C\n",
        2,
        16,
        "Unterminated node label (missing `))`)",
    ),
    (
        "sequenceDiagram\n  Alice->>Bob: é漢\n  Bob-->>+ : hi\n  Bob->>Alice: ok\n",
        3,
        12,
        "unexpected text; expected num, actor",
    ),
    (
        "classDiagram\n  class A[\"é漢\"]\n  A <|-- B C\n  B : +x\n",
        3,
        12,
        "unexpected name; expected newline, label",
    ),
    (
        "stateDiagram-v2\n  [*] --> A\n  A --> \n  A --> [*]\n",
        3,
        9,
        "unexpected statement separator; expected node identifier, styled id, edge state",
    ),
    (
        "erDiagram\n  A ||--o{ B : \"é漢\"\n  B }|..|{ C\n  C { int x }\n",
        3,
        13,
        "unexpected statement separator; expected style separator, colon",
    ),
];

#[test]
fn errors_name_their_line_and_character_column_whatever_the_line_ends() {
    let dir = folder("mermaid-errors");
    for (source, line, column, message) in BROKEN {
        let crlf = format!("\u{FEFF}{}", source.replace('\n', "\r\n"));
        for text in [source.to_string(), crlf] {
            fs::write(dir.join("bad.mmd"), &text).unwrap();
            let (code, out, _) = run_in(&dir, &["check", "bad.mmd", "--json"], b"");
            assert_eq!(code, 1, "{text:?}");
            let v: Value = serde_json::from_str(&out).unwrap();
            let e = &v["errors"][0];
            assert_eq!(
                (
                    &v["valid"],
                    &e["code"],
                    &e["line"],
                    &e["column"],
                    &e["message"]
                ),
                (
                    &json!(false),
                    &json!("syntax_error"),
                    &json!(line),
                    &json!(column),
                    &json!(message)
                ),
                "{text:?}"
            );
            let excerpt = source.lines().nth(line - 1).unwrap();
            assert_eq!(e["excerpt"], excerpt);
            assert_eq!(e["hint"], format!("Fix line {line}, then run again."));
            assert!(
                e["end_line"].as_u64() >= Some(line as u64),
                "merman gives a range"
            );
            let (code, out, _) = run_in(&dir, &["bad.mmd", "--json"], b"");
            assert_eq!(code, 1);
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(
                (&v["error"]["line"], &v["error"]["column"], &v["output"]),
                (&json!(line), &json!(column), &Value::Null)
            );
            assert_eq!(
                v["error"]["hint"],
                format!(
                    "Fix line {line}, then run again. `diagram-render check bad.mmd` lists every error."
                )
            );
        }
    }
    fs::write(dir.join("bad.mmd"), BROKEN[0].0).unwrap();
    let (code, _, err) = run_in(&dir, &["check", "bad.mmd"], b"");
    assert_eq!(code, 1);
    assert_eq!(
        err,
        "✗ bad.mmd:2:16: Unterminated node label (missing `))`)\n    2 |   A[\"é漢\"] --> B(()\n      |                ^\n  Fix line 2, then run again.\n1 error · 0 warnings\n"
    );
    assert!(!dir.join("bad.svg").exists());
}

#[test]
fn an_output_is_replaced_but_never_by_a_failed_render() {
    let dir = fixtures("mermaid-replace", &["flowchart"]);
    fs::write(dir.join("old.svg"), "old").unwrap();
    assert_eq!(run_in(&dir, &["flowchart.mmd", "-o", "old.svg"], b"").0, 0);
    let svg = fs::read_to_string(dir.join("old.svg")).unwrap();
    assert!(svg.starts_with("<svg"));
    fs::write(dir.join("bad.mmd"), BROKEN[0].0).unwrap();
    assert_eq!(run_in(&dir, &["bad.mmd", "-o", "old.svg"], b"").0, 1);
    assert_eq!(fs::read_to_string(dir.join("old.svg")).unwrap(), svg);
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 3, "no temporary file is left: {names:?}");
}

#[test]
fn an_output_folder_that_doesnt_exist_is_write_failed() {
    let dir = fixtures("mermaid-write", &["flowchart"]);
    let out = Path::new("missing").join("x.svg");
    let out = out.to_str().unwrap();
    let (code, _, err) = run_in(&dir, &["flowchart.mmd", "-o", out], b"");
    assert_eq!(code, 3);
    assert!(
        err.starts_with(&format!("✗ flowchart.mmd: Can't write {out}: ")),
        "{err}"
    );
    assert!(
        err.ends_with(".\n  Check the folder exists and isn't read-only.\n"),
        "{err}"
    );
    let (code, v) = {
        let (code, out, _) = run_in(&dir, &["flowchart.mmd", "-o", out, "--json"], b"");
        (code, serde_json::from_str::<Value>(&out).unwrap())
    };
    assert_eq!((code, &v["error"]["code"]), (3, &json!("write_failed")));
}
