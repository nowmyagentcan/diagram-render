//! The MCP server on stdio: `check_diagram` finds errors by line, and `render_diagram` writes a
//! file when asked and hands the model a PNG preview, so it can see what it drew.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use rmcp::service::ServerInitializeError;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::generate::SchemaSettings;
use schemars::transform::RecursiveTransform;
use schemars::{JsonSchema, Schema};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{Error, Warning};
use crate::input;
use crate::output::{self, Target, To};
use crate::run::{self, Opened, Options, Source, Stopped};
use crate::text::Made;

/// The most a `source` argument may hold.
const SOURCE_LIMIT: usize = 4 << 20;
/// The largest preview a model gets: its longer side, then its file size.
const PREVIEW_SIDE: f64 = 1568.0;
const PREVIEW_BYTES: usize = 1_000_000;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderArgs {
    /// The diagram's source text, at most 4 MiB; give this or path
    #[schemars(length(max = 4194304))]
    source: Option<String>,
    /// A diagram file; give this or source. Relative paths start from the server's working folder
    path: Option<String>,
    /// The diagram language
    #[schemars(extend("enum" = ["auto", "mermaid", "dot", "d2", "bpmn", "drawio", "excalidraw", null], "default" = "auto"))]
    format: Option<String>,
    /// A .svg or .png file to write, replacing an existing one; left out, nothing is written
    output: Option<String>,
    /// The image type; left out, it comes from output's extension, else svg
    #[schemars(extend("enum" = ["svg", "png", null]))]
    to: Option<String>,
    /// PNG pixels per SVG pixel
    #[schemars(range(min = 0.1, max = 8), extend("default" = 2))]
    scale: Option<f64>,
    /// auto, transparent, white, #RGB or #RRGGBB
    #[schemars(extend("default" = "auto"))]
    background: Option<String>,
    /// Page number or name (draw.io, BPMN, D2); left out, the first
    page: Option<String>,
    /// Let D2 fetch the remote images a diagram names
    #[serde(default)]
    allow_network: bool,
    /// Include a PNG preview in the result
    #[schemars(extend("default" = true))]
    preview: Option<bool>,
    /// Stop an engine after this many seconds
    #[schemars(range(min = 1, max = 600), extend("default" = 60))]
    timeout: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CheckArgs {
    /// The diagram's source text, at most 4 MiB; give this or path
    #[schemars(length(max = 4194304))]
    source: Option<String>,
    /// A diagram file; give this or source. Relative paths start from the server's working folder
    path: Option<String>,
    /// The diagram language
    #[schemars(extend("enum" = ["auto", "mermaid", "dot", "d2", "bpmn", "drawio", "excalidraw", null], "default" = "auto"))]
    format: Option<String>,
    /// Page number or name (draw.io, BPMN, D2); left out, the first
    page: Option<String>,
    /// Stop an engine after this many seconds
    #[schemars(range(min = 1, max = 600), extend("default" = 60))]
    timeout: Option<u64>,
}

#[derive(Clone)]
struct Server {
    debug: bool,
}

#[tool_router]
impl Server {
    #[tool(
        description = "Render a diagram to SVG or PNG locally, with no browser: Mermaid, Graphviz DOT, D2, BPMN 2.0 XML, draw.io (.drawio) or Excalidraw (.excalidraw). Give the diagram as source text or a file path. With output (a .svg or .png path) the file is written, replacing an existing one; without output nothing is written. Unless preview is false the result includes a PNG preview so you can see the diagram. Returns one JSON text block {input, format, output, width, height, warnings, ...}; on failure isError is true and the text holds {\"error\":{\"code\",\"message\",\"hint\",\"line\",\"column\",\"excerpt\"}}.",
        input_schema = input_schema::<RenderArgs>(),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    fn render_diagram(&self, args: JsonObject) -> CallToolResult {
        answer(parse(args).and_then(|a| render(a, self.debug)))
    }

    #[tool(
        description = "Check a diagram for errors without rendering it: Mermaid, Graphviz DOT, D2, BPMN 2.0, draw.io or Excalidraw. Returns valid, the diagram type, and every error and warning with line and column. Use it after writing or editing a diagram.",
        input_schema = input_schema::<CheckArgs>(),
        output_schema = check_schema(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    fn check_diagram(&self, args: JsonObject) -> CallToolResult {
        answer(parse(args).and_then(|a| check(a, self.debug)))
    }
}

#[tool_handler(
    name = "diagram-render",
    instructions = "Check and render diagrams locally: Mermaid, Graphviz DOT, D2, BPMN 2.0, draw.io and Excalidraw. Use check_diagram after writing or editing a diagram to get errors by line and column. Use render_diagram to save an SVG or PNG and to look at the result as an image."
)]
impl ServerHandler for Server {}

/// The error object of a call that can't start.
fn usage(e: Error) -> Value {
    json!({"error": e.json()})
}

/// The arguments, refusing names the tool doesn't define.
fn parse<T: DeserializeOwned>(args: JsonObject) -> Result<T, Value> {
    serde_json::from_value(Value::Object(args)).map_err(|e| {
        let e = Error::invalid_option(
            format!("Can't read the arguments: {e}."),
            "Check them against the tool's input schema.",
        );
        usage(e)
    })
}

/// The tool's result, or the error object as an error result. Errors carry no structured
/// content, so clients don't check them against the output schema.
fn answer(result: Result<CallToolResult, Value>) -> CallToolResult {
    result.unwrap_or_else(|e| CallToolResult::error(vec![ContentBlock::text(e.to_string())]))
}

/// The diagram a call names: its text, or the file at a path.
struct Input {
    name: String,
    file: bool,
}

impl Input {
    fn new(
        source: Option<String>,
        path: Option<String>,
    ) -> Result<(Self, Result<Vec<u8>, Error>), Error> {
        match (source, path) {
            (Some(_), Some(_)) => Err(Error::invalid_option(
                "Give source or path, not both.",
                "Pass the diagram one way.",
            )),
            (None, None) => Err(Error::invalid_option(
                "Give source or path.",
                "Pass the diagram's text as source, or its file as path.",
            )),
            (Some(source), None) if source.len() > SOURCE_LIMIT => Err(Error::invalid_option(
                "source is over 4 MiB.",
                "Write the diagram to a file and pass path.",
            )),
            (Some(source), None) => {
                let name = "-".into();
                Ok((Self { name, file: false }, Ok(source.into_bytes())))
            }
            (None, Some(name)) => {
                let bytes = input::read_file(OsStr::new(&name));
                Ok((Self { name, file: true }, bytes))
            }
        }
    }

    fn source(&self) -> Source<'_> {
        Source {
            name: &self.name,
            shown: if self.file { &self.name } else { "source" },
            path: self.file.then(|| Path::new(&self.name)),
        }
    }
}

/// Prints the engine's command lines to stderr, which MCP clients keep for logs.
fn debug(on: bool, opened: &Opened) {
    if on {
        crate::debug(opened.log());
    }
}

/// Renders as the CLI does, without a default output: a file only when `output` is given.
fn render(args: RenderArgs, debugging: bool) -> Result<CallToolResult, Value> {
    let start = Instant::now();
    let options = Options {
        format: args.format,
        to: args.to,
        scale: args.scale,
        background: args.background,
        page: args.page,
        allow_network: args.allow_network,
        timeout: args.timeout,
    };
    let (choices, to) = options.choices().map_err(usage)?;
    let target = (args.output.map(|o| output::named(o.into(), to)).transpose()).map_err(usage)?;
    let to = target.as_ref().map(|t| t.1).or(to).unwrap_or(To::Svg);
    let (input, bytes) = Input::new(args.source, args.path).map_err(usage)?;
    let source = input.source();
    if let (Some((Target::File(out), _)), Some(path)) = (&target, source.path)
        && output::is_input(out, path)
    {
        return Err(usage(Error::output_is_input(&out.to_string_lossy())));
    }
    let mut opened = run::open(source, bytes, &choices).map_err(|s| s.json(&input.name))?;
    let made = draw(
        &mut opened,
        &input.name,
        target,
        to,
        start,
        args.preview != Some(false),
    );
    debug(debugging, &opened);
    let known = std::mem::take(&mut opened.known);
    match made {
        Ok((made, image)) => {
            let mut result = run::render_json(&known, &made);
            if made.output.is_empty() {
                result["output"] = Value::Null;
            }
            let mut content = vec![ContentBlock::text(result.to_string())];
            if let Some(png) = image {
                let image = ContentBlock::image(STANDARD.encode(png), "image/png".to_string());
                content.insert(0, image);
            }
            Ok(CallToolResult::success(content))
        }
        Err(e) => Err(Stopped::new(e, known).json(&input.name)),
    }
}

/// Draws, writes the file when there is one, and makes the preview from the same SVG.
fn draw(
    opened: &mut Opened,
    name: &str,
    target: Option<(Target, To)>,
    to: To,
    start: Instant,
    preview: bool,
) -> Result<(Made, Option<Vec<u8>>), Error> {
    let (svg, drawn) = opened.draw(to)?;
    let kept = preview.then(|| svg.clone());
    let picture = opened.picture(svg, to)?;
    if let Some((target, _)) = &target {
        output::write(target, &picture.bytes)?;
    }
    let small = |p: &run::Picture| {
        p.width.max(p.height) as f64 <= PREVIEW_SIDE && p.bytes.len() <= PREVIEW_BYTES
    };
    let image = match kept {
        None => None,
        Some(_) if to == To::Png && small(&picture) => Some(picture.bytes.clone()),
        Some(svg) => match shrunk(opened, &svg, to) {
            Ok(png) => Some(png),
            // The file is written and the diagram is fine; only the look at it is missing.
            Err(e) => {
                opened.known.warnings.push(Warning::no_preview(&e.message));
                None
            }
        },
    };
    let output = match target {
        Some((Target::File(path), _)) => path.to_string_lossy().into_owned(),
        _ => String::new(),
    };
    let made = Made::new(name, output, to, &picture, drawn, start.elapsed());
    Ok((made, image))
}

/// The PNG of `svg` at the largest scale up to 2 that fits a preview's side, lowered in steps of
/// 0.75 until it also fits its size in bytes.
fn shrunk(opened: &mut Opened, svg: &str, to: To) -> Result<Vec<u8>, Error> {
    let (width, height) = opened.css_size(svg);
    let mut scale = (PREVIEW_SIDE / width.max(height)).min(2.0);
    for _ in 0..16 {
        // These are about the output; the preview's own go unreported.
        let png = opened.png(svg, to, scale, &mut vec![])?;
        if png.width.max(png.height) as f64 <= PREVIEW_SIDE && png.bytes.len() <= PREVIEW_BYTES {
            return Ok(png.bytes);
        }
        scale *= 0.75;
    }
    Err(Error::internal("the preview stays over 1,000,000 bytes"))
}

/// Checks as the CLI does. A diagram that can't be read as its format is a result, with its
/// errors; a run that couldn't look at it is an error.
fn check(args: CheckArgs, debugging: bool) -> Result<CallToolResult, Value> {
    let options = Options {
        format: args.format,
        page: args.page,
        timeout: args.timeout,
        ..Options::default()
    };
    let (choices, _) = options.choices().map_err(usage)?;
    let (input, bytes) = Input::new(args.source, args.path).map_err(usage)?;
    let opened = run::open(input.source(), bytes, &choices);
    let mut opened = opened.map_err(|s| s.json(&input.name))?;
    let errors = opened.check();
    debug(debugging, &opened);
    let errors = match errors {
        Ok(errors) => errors,
        Err(e) if e.exit_code() == 1 => vec![e],
        Err(e) => return Err(Stopped::new(e, opened.known).json(&input.name)),
    };
    let errors: Vec<_> = errors.iter().map(Error::check_json).collect();
    let report = run::check_json(&input.name, &opened.known, &errors);
    Ok(CallToolResult::structured(report))
}

/// A tool's arguments, closed to names it doesn't define.
fn input_schema<T: JsonSchema>() -> Arc<JsonObject> {
    let no_format = |s: &mut Schema| drop(s.remove("format"));
    let schema = SchemaSettings::draft2020_12()
        .with_transform(RecursiveTransform(no_format))
        .into_generator()
        .into_root_schema_for::<T>();
    let Value::Object(mut schema) = schema.to_value() else {
        unreachable!("a root schema is an object")
    };
    schema.remove("title");
    schema.remove("description");
    schema.insert("type".into(), "object".into());
    schema.insert("additionalProperties".into(), false.into());
    Arc::new(schema)
}

/// The shape of `check_diagram`'s result (`run::check_json`).
fn check_schema() -> Arc<JsonObject> {
    let text = json!({"type": ["string", "null"]});
    let number = json!({"type": ["integer", "null"]});
    let schema = json!({
        "type": "object",
        "required": ["input", "format", "diagram_type", "valid", "errors", "warnings", "engine"],
        "properties": {
            "input": {"type": "string"},
            "format": {"type": "string"},
            "diagram_type": text,
            "valid": {"type": "boolean"},
            "errors": {"type": "array", "items": {
                "type": "object",
                "required": ["code", "message", "hint"],
                "properties": {
                    "code": {"type": "string"}, "message": {"type": "string"},
                    "hint": {"type": "string"}, "line": number, "column": number,
                    "end_line": number, "end_column": number, "excerpt": text,
                },
            }},
            "warnings": {"type": "array", "items": {
                "type": "object",
                "required": ["code", "message"],
                "properties": {
                    "code": {"type": "string"}, "message": {"type": "string"},
                    "line": {"type": "integer"},
                },
            }},
            "engine": {
                "type": "object",
                "required": ["name", "version"],
                "properties": {"name": {"type": "string"}, "version": {"type": "string"}},
            },
        },
    });
    let Value::Object(schema) = schema else {
        unreachable!("the schema is an object")
    };
    Arc::new(schema)
}

/// Serves until the client closes stdin. Calls run one at a time on this thread.
pub fn serve(debug: bool) -> Result<(), Error> {
    let failed = |e: &dyn std::fmt::Display| Error::internal(&format!("MCP server: {e}"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| failed(&e))?;
    runtime.block_on(async {
        match (Server { debug }).serve(rmcp::transport::stdio()).await {
            Ok(service) => service.waiting().await.map(drop).map_err(|e| failed(&e)),
            // stdin closed before the handshake ended: the client left, which is no failure.
            Err(ServerInitializeError::ConnectionClosed(_)) => Ok(()),
            // stdout closed before the answer went out: the client left too.
            Err(ServerInitializeError::TransportError { error, .. })
                if error
                    .error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
            {
                Ok(())
            }
            Err(e) => Err(failed(&e)),
        }
    })
}
