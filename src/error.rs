//! Every failure the tool reports: a stable code, a sentence saying what happened and a sentence
//! saying what to do.

use serde_json::{Value, json};

/// A failure: what went wrong and how to fix it.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
}

const USAGE: &[&str] = &["invalid_option", "one_input_only", "no_input"];

impl Error {
    fn new(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.into(),
        }
    }

    /// 2 for a misused command line, 3 when the machine or the tool failed, 1 when the diagram
    /// or the file is at fault.
    pub fn exit_code(&self) -> u8 {
        match self.code {
            c if USAGE.contains(&c) => 2,
            "internal" => 3,
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
            e["line"] = Value::Null;
            e["column"] = Value::Null;
            e["excerpt"] = Value::Null;
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

    pub fn internal(detail: &str) -> Self {
        Self::new(
            "internal",
            format!("Something went wrong inside diagram-render: {detail}."),
            "Please report it with --debug output.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_who_is_at_fault() {
        assert_eq!(Error::no_input().exit_code(), 2);
        assert_eq!(Error::one_input_only().exit_code(), 2);
        assert_eq!(Error::internal("x").exit_code(), 3);
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
