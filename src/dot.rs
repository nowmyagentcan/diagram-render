//! Graphviz DOT through `dot`: `-Tsvg` for the first graph's SVG, `-Tcanon` to check.

use crate::engine::{Checked, Engine, Ran, Rendered, Task};
use crate::error::{Error, Warning};

/// What this build's sfdp prints, with exit code 1, after writing a good SVG: it has no
/// triangulation library to remove node overlaps with.
const SFDP: &str = "Error: remove_overlap: Graphviz not built with triangulation library";

pub fn render(engine: &mut Engine, task: &Task) -> Result<Rendered, Error> {
    let background = task.background.map(|b| format!("-Gbgcolor={b}"));
    let args: Vec<&str> = ["-Tsvg"].into_iter().chain(background.as_deref()).collect();
    let ran = engine.go(&args, task)?;
    let mut said = read(engine, task, &ran)?;
    if let Some(e) = said.errors.into_iter().next() {
        return Err(e);
    }
    let svg = String::from_utf8_lossy(&ran.stdout);
    // Graphviz writes one document per graph, each with its own declaration.
    let starts: Vec<_> = svg.match_indices("<?xml ").map(|(i, _)| i).collect();
    if starts.len() > 1 {
        said.warnings
            .push(Warning::extra_graphs_ignored(starts.len()));
    }
    let first = starts.get(1).map_or(svg.as_ref(), |&end| &svg[..end]);
    Ok(Rendered {
        svg: first.as_bytes().to_vec(),
        warnings: said.warnings,
    })
}

pub fn check(engine: &mut Engine, task: &Task) -> Result<Checked, Error> {
    let ran = engine.go(&["-Tcanon"], task)?;
    read(engine, task, &ran)
}

/// Graphviz's messages: errors (a syntax error placed at the token it names) and warnings.
/// Any exit but 0 is a failure, except sfdp's overlap line, which is only a warning.
fn read(engine: &Engine, task: &Task, ran: &Ran) -> Result<Checked, Error> {
    let stderr = String::from_utf8_lossy(&ran.stderr);
    let (mut said, mut sfdp) = (Checked::default(), false);
    for line in stderr.lines().map(str::trim_end) {
        if line == SFDP {
            sfdp = true;
            said.warnings.push(Warning::sfdp_overlaps());
        } else if let Some(text) = line.strip_prefix("Error: ") {
            said.errors.push(error(task, text));
        } else if line.starts_with("Warning: ") {
            let warning = Warning::engine("Graphviz", line, at_line(line));
            if !said.warnings.contains(&warning) {
                said.warnings.push(warning);
            }
        }
    }
    match ran.code {
        Some(0) => Ok(said),
        Some(1) if !said.errors.is_empty() || sfdp && !ran.stdout.is_empty() => Ok(said),
        _ => Err(engine.failed(ran)),
    }
}

/// `<stdin>: syntax error in line 3 near ';'` is a syntax error at the first `;` of line 3;
/// any other error fails the render, at its line when it names one of the file's.
fn error(task: &Task, text: &str) -> Error {
    let Some(line) = at_line(text) else {
        return Error::render_failed("Graphviz", text, task.name);
    };
    let text = text.strip_prefix("<stdin>: ").unwrap_or(text);
    if !text.starts_with("syntax error in line ") {
        let e = Error::render_failed("Graphviz", text, task.name);
        return task.lines.place(e, line, None);
    }
    let near = text
        .split_once(" near '")
        .map(|(_, n)| n.trim_end_matches('\''));
    let column = near.and_then(|near| {
        let source = task.lines.line(line);
        let at = source.find(near).filter(|_| !near.is_empty())?;
        Some(source[..at].chars().count() + 1)
    });
    task.lines
        .place(Error::syntax_error(text, line, None), line, column)
}

/// The source line a message names: `in line N` of the input (`<stdin>`), not of a label.
fn at_line(text: &str) -> Option<usize> {
    if !text.contains("<stdin>") {
        return None;
    }
    let (_, rest) = text.split_once("in line ")?;
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..digits].parse().ok()
}
