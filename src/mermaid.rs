//! Mermaid through merman-cli: `render` for the SVG and the PNG, `lint` for every error with its
//! place. Text is measured and drawn with the bundled Liberation Sans, so the SVG and the PNG
//! have the same layout on every computer.

use std::time::Duration;

use crate::engine::{Checked, Engine, Rendered, Task};
use crate::error::{Error, Warning};
use crate::raster::{self, Png};

pub fn render(engine: &mut Engine, task: &Task) -> Result<Rendered, Error> {
    let (fonts, _) = fonts(task)?;
    // Past merman's default budgets for spec's large.mmd, short of what a hostile nest needs.
    let args = [
        "--format",
        "svg",
        "--resource-limit",
        "max_layout_work_units=2000000000",
        "--resource-limit",
        "max_scheduling_weight_bytes=1000000000000",
    ];
    let svg = run(engine, task, &args, &fonts)?;
    Ok(Rendered {
        svg,
        ..Rendered::default()
    })
}

/// The PNG at `scale`, laid out as the SVG `render` made.
pub fn png(
    engine: &mut Engine,
    task: &Task,
    scale: f64,
    warnings: &mut Vec<Warning>,
) -> Result<Png, Error> {
    let (fonts, missing) = fonts(task)?;
    let scale = scale.to_string();
    // merman's own default limits are 4096 px; ours are checked already. Its filter budget can't
    // be raised by name, and the SVG run has passed the budgets that matter.
    let args = [
        "--resource-profile",
        "unbounded-for-trusted-input",
        "--format",
        "png",
        "--scale",
        &scale,
        "--raster-max-width",
        "16384",
        "--raster-max-height",
        "16384",
        "--raster-max-pixels",
        "64000000",
    ];
    let bytes = run(engine, task, &args, &fonts)?;
    if !missing.is_empty() {
        warnings.push(Warning::font_fallback(&missing));
    }
    raster::sized(bytes).ok_or_else(|| {
        Error::engine_failed("Mermaid", "exit code 0", Some("its PNG has no header"))
    })
}

/// `--font-file` for each Liberation Sans face, and the computer's fonts only when the source
/// has characters those don't (which are returned).
fn fonts(task: &Task) -> Result<(Vec<String>, Vec<char>), Error> {
    let source = String::from_utf8_lossy(task.job.stdin);
    let missing = raster::uncovered(&source)?;
    let files = raster::fonts()?.into_iter().take(4);
    let mut args: Vec<String> = files
        .flat_map(|f| ["--font-file".into(), f.display().to_string()])
        .collect();
    let system = if missing.is_empty() { "off" } else { "on" };
    args.extend(["--font-system".into(), system.into()]);
    Ok((args, missing))
}

/// A `render` run with `args` and `fonts`: its output, or the diagram's first error.
fn run(
    engine: &mut Engine,
    task: &Task,
    args: &[&str],
    fonts: &[String],
) -> Result<Vec<u8>, Error> {
    // merman's own limit sits past ours, so ours stops it and says why.
    let ms = (task.job.limit + Duration::from_secs(5))
        .as_millis()
        .to_string();
    let background = task.background.unwrap_or("white");
    let common = [
        "render",
        "-",
        "--output",
        "-",
        "--background",
        background,
        "--operation-timeout-ms",
        &ms,
        "--quiet",
    ];
    let fonts = fonts.iter().map(String::as_str);
    let args: Vec<&str> = common
        .into_iter()
        .chain(args.iter().copied())
        .chain(fonts)
        .collect();
    let ran = engine.go(&args, task)?;
    let stderr = String::from_utf8_lossy(&ran.stderr);
    let said = stderr.lines().next().unwrap_or("no message");
    match ran.code {
        Some(0) => Ok(ran.stdout),
        // A source past merman's budgets is no syntax error, and lint can take minutes on it.
        Some(1) if said.starts_with("resource limit") => {
            Err(Error::render_failed("Mermaid", said, task.name))
        }
        Some(1) => Err(
            match lint(engine, task, Some(task.name))?
                .errors
                .into_iter()
                .next()
            {
                Some(e) => e,
                None => Error::render_failed("Mermaid", said, task.name),
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
