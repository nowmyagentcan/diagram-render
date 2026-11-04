//! One render or check, the same for every face: read the input, find its format and engine, draw
//! the SVG, then make the picture. What each step learns is kept for the report, also when a later
//! step fails.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::detect::{self, Format};
use crate::engine::{self, Engine, Job, Page, Task};
use crate::error::{Error, Warning};
use crate::input::Lines;
use crate::output::To;
use crate::raster::{self, Png};
use crate::svg;
use crate::text::Made;

/// What may be asked of a render or a check, already checked.
#[derive(Default)]
pub struct Choices {
    pub format: Option<Format>,
    pub scale: Option<f64>,
    pub background: Option<String>,
    pub page: Option<String>,
    pub timeout: Option<u64>,
    pub allow_network: bool,
}

/// The same options as plain values, as the page and the MCP tools send them.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub format: Option<String>,
    pub to: Option<String>,
    pub scale: Option<f64>,
    pub background: Option<String>,
    pub page: Option<String>,
    #[serde(default)]
    pub allow_network: bool,
    pub timeout: Option<u64>,
}

impl Options {
    /// Checks every value as the command line does.
    pub fn choices(self) -> Result<(Choices, Option<To>), Error> {
        let to = self.to.as_deref().map(crate::to).transpose()?;
        let format = self.format.as_deref().map(crate::format).transpose()?;
        let background = self.background.as_deref().map(crate::background);
        let scale = self.scale.map(|s| crate::scale(&s.to_string()));
        let timeout = self.timeout.map(|t| crate::timeout(&t.to_string()));
        let choices = Choices {
            format: format.flatten(),
            scale: scale.transpose()?,
            background: background.transpose()?.flatten(),
            page: self.page.map(crate::page).transpose()?,
            timeout: timeout.transpose()?,
            allow_network: self.allow_network,
        };
        Ok((choices, to))
    }
}

/// Where a diagram came from.
pub struct Source<'a> {
    /// The input in reports: as given, `-` when it has no file.
    pub name: &'a str,
    /// The input in sentences.
    pub shown: &'a str,
    /// Its file, whose name says the format and whose folder relative links start from.
    pub path: Option<&'a Path>,
}

/// What a run learned about its input.
#[derive(Default)]
pub struct Known {
    pub format: Option<Format>,
    pub diagram_type: Option<&'static str>,
    pub warnings: Vec<Warning>,
}

/// A run that stopped, and what it knew by then.
pub struct Stopped {
    pub error: Error,
    pub known: Box<Known>,
}

impl From<Error> for Stopped {
    fn from(error: Error) -> Self {
        Self::new(error, Known::default())
    }
}

impl Stopped {
    pub fn new(error: Error, known: Known) -> Self {
        let known = Box::new(known);
        Self { error, known }
    }

    /// The report of a failed render, or of a usage error.
    pub fn json(&self, input: &str) -> Value {
        match self.error.is_usage() {
            true => json!({"error": self.error.json()}),
            false => json!({
                "input": input, "format": self.known.format.map(Format::id), "output": null,
                "error": self.error.json(), "warnings": warnings(&self.known),
                "engine": self.known.format.map(engine_json),
            }),
        }
    }
}

/// A diagram whose format and engine are known.
pub struct Opened<'a> {
    pub known: Known,
    format: Format,
    name: &'a str,
    dir: PathBuf,
    text: String,
    choices: &'a Choices,
    engine: Option<Engine>,
}

/// The SVG of a diagram, safe to show, and its pages.
pub struct Drawn {
    pub svg: String,
    pub page: Option<Page>,
    pub pages: Vec<Page>,
}

/// The finished image.
pub struct Picture {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// PNG pixels per CSS pixel; `None` for SVG.
    pub scale: Option<f64>,
}

/// Reads what `bytes` holds and finds its engine.
pub fn open<'a>(
    source: Source<'a>,
    bytes: Result<Vec<u8>, Error>,
    choices: &'a Choices,
) -> Result<Opened<'a>, Stopped> {
    let mut known = Known {
        format: choices.format,
        ..Known::default()
    };
    let detected =
        bytes.and_then(|b| detect::detect(source.path, source.shown, &b, choices.format));
    let diagram = match detected {
        Ok(diagram) => diagram,
        Err(error) => return Err(Stopped::new(error, known)),
    };
    if choices.page.is_some() && !diagram.format.has_pages() {
        return Err(Error::page_not_supported(diagram.format.name()).into());
    }
    known = Known {
        format: Some(diagram.format),
        diagram_type: diagram.diagram_type,
        warnings: diagram.warnings,
    };
    let engine = match engine::locate(diagram.format).transpose() {
        Ok(engine) => engine,
        Err(error) => return Err(Stopped::new(error, known)),
    };
    // The engine's folder for relative imports and images: the input's, else the current one.
    let dir = source.path.and_then(Path::parent);
    let dir = dir.filter(|d| !d.as_os_str().is_empty());
    Ok(Opened {
        known,
        format: diagram.format,
        name: source.name,
        dir: dir.unwrap_or(Path::new(".")).to_path_buf(),
        text: diagram.text.unwrap_or_default(),
        choices,
        engine,
    })
}

impl Opened<'_> {
    fn task(&self, to: To) -> Task<'_> {
        Task {
            name: self.name,
            lines: Lines::new(&self.text),
            job: Job {
                stdin: self.text.as_bytes(),
                dir: &self.dir,
                env: vec![],
                limit: Duration::from_secs(self.choices.timeout.unwrap_or(60)),
            },
            background: self.choices.background.as_deref(),
            page: self.choices.page.as_deref(),
            allow_network: self.choices.allow_network,
            to,
        }
    }

    /// Every error found; the warnings go to `known`.
    pub fn check(&mut self) -> Result<Vec<Error>, Error> {
        let mut engine = self.engine.take();
        let checked = engine::check(self.format, engine.as_mut(), &self.task(To::Svg));
        self.engine = engine;
        let checked = checked?;
        self.known.warnings.extend(checked.warnings);
        self.known.diagram_type = checked.diagram_type.or(self.known.diagram_type);
        Ok(checked.errors)
    }

    /// The diagram's safe SVG, made with `to` in mind; the warnings go to `known`.
    pub fn draw(&mut self, to: To) -> Result<Drawn, Error> {
        if self.choices.scale.is_some() && to == To::Svg {
            self.known.warnings.push(Warning::scale_ignored());
        }
        let mut engine = self.engine.take();
        let rendered = engine::render(self.format, engine.as_mut(), &self.task(to));
        self.engine = engine;
        let r = rendered?;
        self.known.diagram_type = r.diagram_type.or(self.known.diagram_type);
        let (svg, removed) = svg::clean(&String::from_utf8_lossy(&r.svg))
            .map_err(|e| Error::internal(&format!("the engine's SVG isn't valid XML: {e}")))?;
        self.known.warnings.extend(r.warnings);
        if removed + r.removed > 0 {
            let removed = Warning::unsafe_content_removed(removed + r.removed);
            self.known.warnings.push(removed);
        }
        // d2 can't list its boards, so a board has a name but no number.
        let asked = self.choices.page.clone();
        Ok(Drawn {
            svg,
            page: r.page.or(asked.map(|name| Page { index: None, name })),
            pages: r.pages,
        })
    }

    /// The image of `svg` as `to`: the SVG itself, or a PNG within the size limits.
    pub fn picture(&mut self, svg: String, to: To) -> Result<Picture, Error> {
        if to == To::Svg {
            let (width, height) = svg::size(&svg);
            let bytes = svg.into_bytes();
            let scale = None;
            return Ok(Picture {
                bytes,
                width,
                height,
                scale,
            });
        }
        let (width, height) = self.css_size(&svg);
        let (scale, reduced) = raster::fit(width, height, self.choices.scale.unwrap_or(2.0))?;
        self.known.warnings.extend(reduced);
        let mut warnings = vec![];
        let png = self.png(&svg, to, scale, &mut warnings)?;
        self.known.warnings.extend(warnings);
        Ok(Picture {
            bytes: png.bytes,
            width: png.width,
            height: png.height,
            scale: Some(scale),
        })
    }

    /// The size a PNG of `svg` starts from. d2 sizes its PNG by the diagram's own size, which its
    /// SVG rounds.
    pub fn css_size(&self, svg: &str) -> (f64, f64) {
        let pad = if self.format == Format::D2 { 1.0 } else { 0.0 };
        let (width, height) = svg::css_size(svg);
        (width + pad, height + pad)
    }

    /// The PNG of `svg` at exactly `scale`, for a run that makes `to`.
    pub fn png(
        &mut self,
        svg: &str,
        to: To,
        scale: f64,
        warnings: &mut Vec<Warning>,
    ) -> Result<Png, Error> {
        let mut engine = self.engine.take();
        let task = self.task(to);
        let png = engine::png(self.format, engine.as_mut(), &task, svg, scale, warnings);
        self.engine = engine;
        png
    }

    /// The `--debug` lines of the engine's runs.
    pub fn log(&self) -> &[String] {
        self.engine.as_ref().map_or(&[], |e| &e.log)
    }
}

fn engine_json(format: Format) -> Value {
    let (name, version) = format.engine();
    json!({"name": name, "version": version})
}

fn warnings(known: &Known) -> Vec<Value> {
    known.warnings.iter().map(Warning::json).collect()
}

/// The report of a finished render.
pub fn render_json(known: &Known, made: &Made) -> Value {
    json!({
        "input": made.input, "format": known.format.map(Format::id),
        "diagram_type": known.diagram_type, "output": made.output, "to": made.to.ext(),
        "width": made.width, "height": made.height, "bytes": made.bytes,
        // A whole scale reads as one: 2, not 2.0.
        "scale": made.scale.map(|s| if s.fract() == 0.0 { json!(s as u64) } else { json!(s) }),
        "page": made.page.as_ref().map(Page::json),
        "pages": made.pages.iter().map(Page::json).collect::<Vec<_>>(),
        "warnings": warnings(known), "engine": known.format.map(engine_json),
        "elapsed_ms": made.took.as_millis() as u64,
    })
}

/// The report of a check, whose errors are `Error::check_json` objects.
pub fn check_json(input: &str, known: &Known, errors: &[Value]) -> Value {
    json!({
        "input": input, "format": known.format.map(Format::id),
        "diagram_type": known.diagram_type, "valid": errors.is_empty(), "errors": errors,
        "warnings": warnings(known), "engine": known.format.map(engine_json),
    })
}
