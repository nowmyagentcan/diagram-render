//! Mermaid through merman-cli: `render` for the SVG, `lint` for every error with its place.

use std::time::Duration;

use crate::engine::{Checked, Engine, Rendered, Task};
use crate::error::{Error, Warning};

pub fn render(engine: &mut Engine, task: &Task) -> Result<Rendered, Error> {
    // merman's own limit sits past ours, so ours stops it and says why.
    let ms = (task.job.limit + Duration::from_secs(5))
        .as_millis()
        .to_string();
    let background = task.background.unwrap_or("white");
    let args = [
        "render",
        "-",
        "--format",
        "svg",
        "--output",
        "-",
        "--background",
        background,
        "--operation-timeout-ms",
        &ms,
        "--quiet",
    ];
    let ran = engine.go(&args, task)?;
    match ran.code {
        Some(0) => Ok(Rendered {
            svg: ran.stdout,
            warnings: vec![],
        }),
        Some(1) => Err(
            match lint(engine, task, Some(task.name))?
                .errors
                .into_iter()
                .next()
            {
                Some(e) => e,
                None => {
                    let stderr = String::from_utf8_lossy(&ran.stderr);
                    let said = stderr.lines().next().unwrap_or("no message");
                    Error::render_failed("Mermaid", said, task.name)
                }
            },
        ),
        _ => Err(engine.failed(&ran)),
    }
}

pub fn check(engine: &mut Engine, task: &Task) -> Result<Checked, Error> {
    lint(engine, task, None)
}

/// merman's diagnostics: errors placed by their byte offsets in the source, and the rest as
/// warnings. `more` names the input in a hint that `check` lists every error.
fn lint(engine: &mut Engine, task: &Task, more: Option<&str>) -> Result<Checked, Error> {
    let ran = engine.go(&["lint", "-", "--format", "json"], task)?;
    let report = match ran.code {
        Some(0 | 1) => serde_json::from_slice::<serde_json::Value>(&ran.stdout).ok(),
        _ => None,
    };
    let report = report.ok_or_else(|| engine.failed(&ran))?;
    let mut checked = Checked::default();
    for d in report["diagnostics"].as_array().into_iter().flatten() {
        let message = d["message"].as_str().unwrap_or_default();
        let at = |key: &str| {
            let offset = d["span"][key].as_u64()?;
            Some(task.lines.locate(usize::try_from(offset).ok()?))
        };
        let start = at("byte_start");
        // This one only says the parser went on after an error it already reported.
        if d["id"] == "merman.parse.recovered_editor_facts" {
            continue;
        }
        if d["severity"] != "error" {
            let line = start.map(|(line, _)| line);
            checked
                .warnings
                .push(Warning::engine("Mermaid", message, line));
            continue;
        }
        checked.errors.push(match start {
            Some((line, column)) => {
                let e = Error::syntax_error(message, line, more);
                let mut e = task.lines.place(e, line, Some(column));
                if let Some(x) = e.excerpt.as_mut() {
                    x.end = at("byte_end");
                }
                e
            }
            None => Error::render_failed("Mermaid", message, task.name),
        });
    }
    Ok(checked)
}
