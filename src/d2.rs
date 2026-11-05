//! D2 through `d2`, offline unless asked: remote images stay links, and the proxy it would
//! reach the network through is a closed local port.

use std::env;
use std::ffi::OsString;

use resvg::tiny_skia::Pixmap;

use crate::engine::{Checked, Engine, Job, Rendered, Task};
use crate::error::{Error, Warning};
use crate::output::To;
use crate::raster::Png;

pub fn render(engine: &mut Engine, task: &Task) -> Result<Rendered, Error> {
    let said = run(engine, task, Some(task.name), None)?;
    let Rendered {
        svg, mut warnings, ..
    } = said.map_err(|errors| errors.into_iter().next().unwrap())?;
    if task.background.is_some() {
        warnings.push(Warning::background_ignored());
    }
    let remote = remote_images(&String::from_utf8_lossy(&svg));
    if remote > 0 && !task.allow_network && task.to == To::Svg {
        warnings.push(Warning::remote_images_not_fetched(remote));
    }
    Ok(Rendered {
        svg,
        warnings,
        ..Rendered::default()
    })
}

/// d2's own PNG at `scale` of the diagram `svg` (made by `render`), as 8-bit RGBA. A PNG
/// can't link the remote images the SVG does, so offline it needs none.
pub fn png(engine: &mut Engine, task: &Task, svg: &str, scale: f64) -> Result<Png, Error> {
    if remote_images(svg) > 0 && !task.allow_network {
        return Err(Error::network_needed());
    }
    let said = run(engine, task, Some(task.name), Some(scale))?;
    let rendered = said.map_err(|errors| errors.into_iter().next().unwrap())?;
    // d2 writes RGB with an Exif chunk; encoded again it is RGBA with nothing else.
    let pixmap = Pixmap::decode_png(&rendered.svg)
        .map_err(|_| Error::engine_failed("D2", "exit code 0", Some("its PNG can't be read")))?;
    let bytes = pixmap.encode_png();
    Ok(Png {
        bytes: bytes.map_err(|e| Error::internal(&format!("PNG encoding: {e}")))?,
        width: pixmap.width(),
        height: pixmap.height(),
    })
}

/// How many `<image>` elements link an http(s) URL.
fn remote_images(svg: &str) -> usize {
    (svg.split("<image").skip(1))
        .filter(|tag| {
            let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
            tag.contains("href=\"http://") || tag.contains("href=\"https://")
        })
        .count()
}

/// A render with its output dropped: `d2 validate` only parses, so it calls an unknown shape
/// or a bad style value valid.
pub fn check(engine: &mut Engine, task: &Task) -> Result<Checked, Error> {
    Ok(match run(engine, task, None, None)? {
        Ok(Rendered { warnings, .. }) => Checked {
            warnings,
            ..Checked::default()
        },
        Err(errors) => Checked {
            errors,
            ..Checked::default()
        },
    })
}

/// Runs d2 on the page (the root board unless one is given), with every image it can embed
/// without the network: its SVG (or PNG at `png`'s scale) and warnings, or the diagram's
/// errors. `more` names the input in a hint that `check` lists every error.
fn run(
    engine: &mut Engine,
    task: &Task,
    more: Option<&str>,
    png: Option<f64>,
) -> Result<Result<Rendered, Vec<Error>>, Error> {
    let target = format!("--target={}", task.page.unwrap_or(""));
    // d2's own limit sits past ours, so ours stops it and says why.
    let timeout = (task.job.limit.as_secs() + 5).to_string();
    // d2 paints 2 PNG pixels per SVG pixel at scale 1.
    let scale = png.map_or("1".into(), |s| (s / 2.0).to_string());
    let mut args = vec![
        target.as_str(),
        "--scale",
        &scale,
        "--timeout",
        timeout.as_str(),
    ];
    if png.is_some() {
        args.extend(["--stdout-format", "png"]);
    }
    // Local images are embedded either way; remote ones stay links. With the network, the SVG
    // a PNG is sized from fetches them too: d2's own PNG hides an image it can't get.
    if !task.allow_network {
        args.push("--bundle=false");
    }
    args.extend(["-", "-"]);
    let job = Job {
        env: proxies(task.allow_network),
        ..task.job.clone()
    };
    let ran = engine.run(&args, &job)?;
    let ran = ran.ok_or_else(|| Error::timeout(job.limit.as_secs()))?;
    let stderr = String::from_utf8_lossy(&ran.stderr);
    let (mut errors, mut warnings) = (vec![], vec![]);
    for line in stderr.lines().map(str::trim_end) {
        if let Some(text) = line.strip_prefix("err: ") {
            errors.push(error(task, text, more)?);
        } else if let Some(text) = line.strip_prefix("warn: ") {
            warnings.push(Warning::engine("D2", text, None));
        }
    }
    match ran.code {
        Some(0) => Ok(Ok(Rendered {
            svg: ran.stdout,
            warnings,
            ..Rendered::default()
        })),
        Some(1) if !errors.is_empty() => Ok(Err(errors)),
        _ => Err(engine.failed(&ran)),
    }
}

/// Offline, a proxy no one listens on; with the network, the user's own proxy settings.
fn proxies(allow_network: bool) -> Vec<(&'static str, OsString)> {
    let keys = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
    ];
    match allow_network {
        true => (keys.into_iter())
            .filter_map(|k| env::var_os(k).map(|v| (k, v)))
            .collect(),
        false => vec![
            ("HTTP_PROXY", "http://127.0.0.1:9".into()),
            ("HTTPS_PROXY", "http://127.0.0.1:9".into()),
            ("NO_PROXY", "".into()),
        ],
    }
}

/// One `err:` line: `-:3:9: message` is a syntax error at line 3, byte 9 of that line (shown
/// as a character column). A board that isn't there fails the run as `page_not_found`.
fn error(task: &Task, text: &str, more: Option<&str>) -> Result<Error, Error> {
    let text = [
        "failed to compile -: ",
        "failed to fully compile (partial render written) -: ",
    ]
    .iter()
    .find_map(|p| text.strip_prefix(p))
    .unwrap_or(text);
    if let (Some(page), true) = (task.page, text.starts_with("render target ")) {
        return Err(Error::board_not_found(page));
    }
    let place = text.strip_prefix("-:").and_then(|rest| {
        let mut parts = rest.splitn(3, ':');
        let line: usize = parts.next()?.parse().ok()?;
        let byte: usize = parts.next()?.parse().ok()?;
        Some((line, byte, parts.next()?.trim_start()))
    });
    let Some((line, byte, message)) = place else {
        return Ok(Error::render_failed("D2", text, task.name));
    };
    let source = task.lines.line(line);
    let mut end = byte.saturating_sub(1).min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let column = source[..end].chars().count() + 1;
    let e = Error::syntax_error(message, line, more);
    Ok(task.lines.place(e, line, Some(column)))
}
