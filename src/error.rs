//! Every failure and warning the tool reports: a stable code, a sentence saying what happened, a
//! sentence saying what to do, and where in the source it is when that is known.

use serde_json::{Value, json};

/// A failure: what went wrong and how to fix it, with its place in the source when known.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub excerpt: Option<Box<Excerpt>>,
}

/// A source line as shown under an error, cut to 200 characters, and the character (from 1)
/// the caret points at in that cut text.
#[derive(Debug, Clone, PartialEq)]
pub struct Excerpt {
    pub text: String,
    pub caret: Option<usize>,
}

/// Something worth knowing about a run that still succeeded.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
    pub line: Option<usize>,
}

impl Warning {
    pub fn format_guessed() -> Self {
        Self {
            code: "format_guessed",
            message: "Read as D2 (no Mermaid or DOT header found); pass --format to choose.".into(),
            line: None,
        }
    }

    pub fn scale_ignored() -> Self {
        Self {
            code: "scale_ignored",
            message: "--scale applies to PNG only.".into(),
            line: None,
        }
    }

    pub fn json(&self) -> Value {
        let mut w = json!({"code": self.code, "message": self.message});
        if let Some(line) = self.line {
            w["line"] = line.into();
        }
        w
    }
}

const USAGE: &[&str] = &[
    "invalid_option",
    "one_input_only",
    "no_input",
    "page_not_supported",
    "format_conflict",
    "bad_extension",
    "json_needs_output",
    "output_is_input",
];

impl Error {
    fn new(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.into(),
            line: None,
            column: None,
            excerpt: None,
        }
    }

    /// 2 for a misused command line, 3 when the machine or the tool failed, 1 when the diagram
    /// or the file is at fault.
    pub fn exit_code(&self) -> u8 {
        match self.code {
            c if USAGE.contains(&c) => 2,
            "engine_missing" | "engine_failed" | "internal" => 3,
            _ => 1,
        }
    }

    pub fn is_usage(&self) -> bool {
        self.exit_code() == 2
    }

    /// The `error` object of a render result, or of a usage error (which has no place).
    pub fn json(&self) -> Value {
        let mut e = json!({"code": self.code, "message": self.message, "hint": self.hint});
        if !self.is_usage() {
            e["line"] = self.line.into();
            e["column"] = self.column.into();
            e["excerpt"] = self.excerpt.as_ref().map(|x| x.text.as_str()).into();
        }
        e
    }

    /// One entry of a check result's `errors`.
    pub fn check_json(&self) -> Value {
        let e = self.json();
        json!({
            "code": e["code"], "message": e["message"], "hint": e["hint"],
            "line": e["line"], "column": e["column"], "end_line": null, "end_column": null,
            "excerpt": e["excerpt"],
        })
    }

    pub fn invalid_option(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new("invalid_option", message, hint)
    }

    pub fn one_input_only() -> Self {
        Self::new(
            "one_input_only",
            "Give one diagram per run.",
            "Render the others in their own runs, for example in a shell loop.",
        )
    }

    pub fn no_input() -> Self {
        Self::new(
            "no_input",
            "No diagram given.",
            "Try: diagram-render flow.mmd, or diagram-render --help.",
        )
    }

    pub fn file_not_found(path: &str) -> Self {
        let cwd = std::env::current_dir().map_or_else(|_| ".".into(), |d| d.display().to_string());
        Self::new(
            "file_not_found",
            format!("Can't find {path}."),
            format!("Check the path; relative paths start from {cwd}."),
        )
    }

    pub fn not_a_file(path: &str) -> Self {
        Self::new(
            "not_a_file",
            format!("{path} is a folder."),
            "Give one diagram file.",
        )
    }

    pub fn read_failed(path: &str, e: &std::io::Error) -> Self {
        Self::new(
            "read_failed",
            format!("Can't read {path}: {}.", reason(e)),
            "Check that the file isn't locked and that you can open it.",
        )
    }

    pub fn too_large(path: &str, size: &str) -> Self {
        Self::new(
            "too_large",
            format!("{path} is {size}; the limit is 64 MiB (4 MiB for Mermaid, DOT and D2 text)."),
            "Split the diagram, or remove embedded images.",
        )
    }

    pub fn empty_input() -> Self {
        Self::new(
            "empty_input",
            "The diagram is empty.",
            "Write some diagram source, or check the file.",
        )
    }

    pub fn not_utf8(line: usize) -> Self {
        Self::new(
            "not_utf8",
            format!("Line {line} isn't valid UTF-8 text."),
            "Save the file as UTF-8.",
        )
    }

    pub fn unknown_format() -> Self {
        Self::new(
            "unknown_format",
            "Can't tell which diagram language this is.",
            "Pass --format mermaid, dot, d2, bpmn, drawio or excalidraw.",
        )
    }

    pub fn unsupported_markdown() -> Self {
        Self::new(
            "unsupported_input",
            "Markdown files aren't rendered as a whole.",
            "Copy one diagram into its own .mmd file, or pipe it in with --format mermaid.",
        )
    }

    pub fn unsupported_plantuml() -> Self {
        Self::new(
            "unsupported_input",
            "PlantUML isn't supported.",
            "Export it from its own app as SVG or PNG.",
        )
    }

    pub fn unsupported_visio() -> Self {
        Self::new(
            "unsupported_input",
            "Visio files aren't supported.",
            "Export it from its own app as SVG or PNG.",
        )
    }

    pub fn unsupported_image(name: &str) -> Self {
        Self::new(
            "unsupported_input",
            format!("{name} is an image with no draw.io or Excalidraw diagram inside."),
            "Nothing to render; it's already an image.",
        )
    }

    pub fn page_not_supported(format: &str) -> Self {
        Self::new(
            "page_not_supported",
            format!("{format} diagrams have no pages."),
            "Drop --page.",
        )
    }

    pub fn format_conflict(output: &str, to: &str) -> Self {
        let (output, to) = (output.to_uppercase(), to.to_uppercase());
        Self::new(
            "format_conflict",
            format!("-o says {output} but --to says {to}."),
            "Drop --to, or change the extension of -o.",
        )
    }

    pub fn bad_extension() -> Self {
        Self::new(
            "bad_extension",
            "The output must end in .svg or .png.",
            "For example: -o flow.png.",
        )
    }

    pub fn json_needs_output() -> Self {
        Self::new(
            "json_needs_output",
            "--json needs -o FILE when the image would go to stdout.",
            "Add -o flow.svg, or drop --json.",
        )
    }

    pub fn output_is_input(path: &str) -> Self {
        Self::new(
            "output_is_input",
            format!("The output would replace the input, {path}."),
            "Pass -o with another file name.",
        )
    }

    pub fn engine_missing(engine: &str, path: &str) -> Self {
        Self::new(
            "engine_missing",
            format!("The {engine} engine is missing: {path} doesn't exist."),
            "Reinstall diagram-render.",
        )
    }

    /// `code` is `exit code 2` or `killed by a signal`; `first` the first line it printed.
    pub fn engine_failed(engine: &str, code: &str, first: Option<&str>) -> Self {
        let said = first.map(|l| format!(": {}", l.trim())).unwrap_or_default();
        Self::new(
            "engine_failed",
            format!("{engine} stopped unexpectedly ({code}){said}."),
            "Run with --debug for the full output.",
        )
    }

    pub fn engine_unstartable(engine: &str, e: &std::io::Error) -> Self {
        Self::new(
            "engine_failed",
            format!("The {engine} engine couldn't start: {}.", reason(e)),
            "Reinstall diagram-render.",
        )
    }

    pub fn engine_slow(engine: &str, seconds: u64) -> Self {
        Self::new(
            "engine_failed",
            format!("The {engine} engine didn't answer within {seconds} s."),
            "Run diagram-render formats again; if it stays slow, reinstall diagram-render.",
        )
    }

    pub fn internal(detail: &str) -> Self {
        Self::new(
            "internal",
            format!("Something went wrong inside diagram-render: {detail}."),
            "Please report it with --debug output.",
        )
    }
}

/// The operating system's reason as a clause: "Access is denied. (os error 5)" -> "access is
/// denied".
fn reason(e: &std::io::Error) -> String {
    let text = e.to_string();
    let text = text.split(" (os error").next().unwrap_or_default();
    let text = text.trim_end_matches('.');
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_lowercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_who_is_at_fault() {
        assert_eq!(Error::no_input().exit_code(), 2);
        assert_eq!(Error::one_input_only().exit_code(), 2);
        assert_eq!(Error::internal("x").exit_code(), 3);
        assert_eq!(Error::engine_missing("D2", "x").exit_code(), 3);
        assert_eq!(
            Error::engine_failed("D2", "exit code 2", None).exit_code(),
            3
        );
        let failed = Error::engine_failed("D2", "exit code 2", Some(" err: bad flag "));
        assert_eq!(
            failed.message,
            "D2 stopped unexpectedly (exit code 2): err: bad flag."
        );
    }

    #[test]
    fn os_reasons_read_as_a_clause() {
        let e = std::io::Error::from_raw_os_error(2);
        assert!(!reason(&e).contains("os error"));
        assert!(reason(&e).starts_with(char::is_lowercase));
        assert_eq!(reason(&std::io::Error::other("Locked.")), "locked");
    }

    #[test]
    fn warnings_carry_a_line_only_when_they_have_one() {
        let mut w = Warning::format_guessed();
        assert_eq!(w.json().as_object().unwrap().len(), 2);
        w.line = Some(4);
        assert_eq!(w.json()["line"], 4);
    }

    #[test]
    fn usage_errors_have_no_place_in_json() {
        assert_eq!(
            Error::no_input().json(),
            json!({"code": "no_input", "message": "No diagram given.", "hint": "Try: diagram-render flow.mmd, or diagram-render --help."})
        );
        let e = Error::internal("x").check_json();
        let keys: Vec<_> = e.as_object().unwrap().keys().cloned().collect();
        let want = [
            "code",
            "message",
            "hint",
            "line",
            "column",
            "end_line",
            "end_column",
            "excerpt",
        ];
        assert_eq!(keys, want);
    }
}
