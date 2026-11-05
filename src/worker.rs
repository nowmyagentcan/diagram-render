//! This binary's own renderers and its PNG painter, each run in a process of their own: the time
//! limit, the memory limit and the kill that stop an engine stop them too, and a crash on a
//! hostile file ends that process, never a server.
//!
//! The request is one argument and the diagram comes on stdin. The answer is a line of JSON, then
//! the image's bytes.

use std::io::{self, Read, Write};
use std::mem::take;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::detect::Format;
use crate::engine::{self, Checked, Engine, Job, Rendered, Task};
use crate::error::{Error, Warning};
use crate::input::Lines;
use crate::output::To;
use crate::raster::{self, Png};
use crate::{bpmn, drawio, excalidraw};

/// The first argument of a worker. Nothing a person types: it isn't in `--help`.
pub const FLAG: &str = "--worker";

#[derive(Serialize, Deserialize)]
enum Ask {
    Render(Format),
    Check(Format),
    /// Paints the SVG on stdin at a scale; a missing font is drawn serif when asked, else sans.
    Png(f64, bool),
}

#[derive(Serialize, Deserialize)]
struct Request {
    ask: Ask,
    name: String,
    background: Option<String>,
    page: Option<String>,
    to: To,
}

impl Request {
    fn new(ask: Ask, task: &Task) -> Self {
        Self {
            ask,
            name: task.name.into(),
            background: task.background.map(Into::into),
            page: task.page.map(Into::into),
            to: task.to,
        }
    }

    /// Sends this to a worker: what it answered, and the bytes after that. Its folder is the
    /// install folder, which is short: an input's may be too long for a working folder, and
    /// nothing here reads beside the input.
    fn send<T: DeserializeOwned>(
        &self,
        engine: &mut Engine,
        stdin: &[u8],
        task: &Task,
    ) -> Result<(T, Vec<u8>), Error> {
        let request = serde_json::to_string(self)
            .map_err(|e| Error::internal(&format!("the worker's request: {e}")))?;
        let job = Job {
            stdin,
            dir: &engine::home(),
            ..task.job.clone()
        };
        let ran = engine.run(&[FLAG, &request], &job)?;
        let ran = ran.ok_or_else(|| Error::timeout(job.limit.as_secs()))?;
        if ran.code != Some(0) {
            return Err(engine.failed(&ran));
        }
        let end = ran.stdout.iter().position(|&b| b == b'\n');
        let (head, bytes) = ran.stdout.split_at(end.unwrap_or(0));
        let said: Result<T, Error> = serde_json::from_slice(head)
            .map_err(|e| Error::internal(&format!("the worker's answer: {e}")))?;
        said.map(|said| (said, bytes[1..].to_vec()))
    }
}

/// `task`'s SVG, drawn in a worker.
pub fn render(engine: &mut Engine, format: Format, task: &Task) -> Result<Rendered, Error> {
    let request = Request::new(Ask::Render(format), task);
    let (rendered, svg) = request.send(engine, task.job.stdin, task)?;
    Ok(Rendered { svg, ..rendered })
}

/// `task` checked in a worker.
pub fn check(engine: &mut Engine, format: Format, task: &Task) -> Result<Checked, Error> {
    let request = Request::new(Ask::Check(format), task);
    Ok(request.send(engine, task.job.stdin, task)?.0)
}

/// `svg` painted at `scale` in a worker, whose lines go in `engine`'s log.
pub fn png(
    engine: &mut Engine,
    task: &Task,
    svg: &str,
    scale: f64,
    serif: bool,
    warnings: &mut Vec<Warning>,
) -> Result<Png, Error> {
    let mut ours = Engine::ours();
    let request = Request::new(Ask::Png(scale, serif), task);
    let sent = request.send(&mut ours, svg.as_bytes(), task);
    engine.log.append(&mut ours.log);
    let ((png, said), bytes): ((Png, Vec<Warning>), _) = sent?;
    warnings.extend(said);
    Ok(Png { bytes, ..png })
}

/// Answers `request`, with the diagram or SVG on stdin. A panic or a stack overflow ends this
/// process, and the one that asked reports it.
pub fn serve(request: &str) -> ExitCode {
    let request: Request = serde_json::from_str(request).expect("a request from this binary");
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .expect("text from this binary");
    let task = Task {
        name: &request.name,
        lines: Lines::new(&text),
        job: Job {
            stdin: text.as_bytes(),
            dir: Path::new("."),
            env: vec![],
            limit: Duration::MAX,
            memory: u64::MAX,
        },
        background: request.background.as_deref(),
        page: request.page.as_deref(),
        allow_network: false,
        to: request.to,
    };
    let written = match &request.ask {
        Ask::Render(format) => {
            let rendered = match format {
                Format::Bpmn => bpmn::render(&task),
                Format::Drawio => drawio::render(&task),
                Format::Excalidraw => excalidraw::render(&task),
                _ => Err(Error::internal("no renderer here")),
            };
            answer(rendered.map(|mut r| {
                let svg = take(&mut r.svg);
                (r, svg)
            }))
        }
        Ask::Check(format) => {
            let checked = match format {
                Format::Bpmn => bpmn::check(&task),
                Format::Drawio => drawio::check(&task),
                Format::Excalidraw => excalidraw::check(&task),
                _ => Err(Error::internal("no checker here")),
            };
            answer(checked.map(|c| (c, vec![])))
        }
        Ask::Png(scale, serif) => {
            let substitute = if *serif { raster::SERIF } else { raster::SANS };
            let mut warnings = vec![];
            let painted = raster::png(&text, *scale, substitute, &mut warnings);
            answer(painted.map(|mut p| {
                let bytes = take(&mut p.bytes);
                ((p, warnings), bytes)
            }))
        }
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        // The one that asked went away, and nobody is left to tell.
        Err(_) => ExitCode::FAILURE,
    }
}

/// Writes the answer's line, then its bytes.
fn answer<T: Serialize>(done: Result<(T, Vec<u8>), Error>) -> io::Result<()> {
    let (said, bytes) = match done {
        Ok((said, bytes)) => (Ok(said), bytes),
        Err(e) => (Err(e), vec![]),
    };
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &said)?;
    out.write_all(b"\n")?;
    out.write_all(&bytes)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_that_dies_is_an_error_for_the_one_that_asked() {
        // The test binary stands in for the worker, and it knows no such flag.
        let task = Task {
            name: "x",
            lines: Lines::new("x"),
            job: Job {
                stdin: b"x",
                dir: Path::new("."),
                env: vec![],
                limit: Duration::from_secs(30),
                memory: engine::MEMORY_LIMIT,
            },
            background: None,
            page: None,
            allow_network: false,
            to: To::Svg,
        };
        let e = render(&mut Engine::ours(), Format::Drawio, &task).err();
        assert_eq!(e.unwrap().code, "engine_failed");
    }
}
