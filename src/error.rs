//! Every failure and warning the tool reports: a stable code, a sentence saying what happened, a
//! sentence saying what to do, and where in the source it is when that is known.

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

/// `text` for as long as the program runs, kept once however often it comes: the worker's answers
/// carry the same few codes.
fn kept(text: String) -> &'static str {
    static KEPT: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut all = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    match all.get(text.as_str()) {
        Some(old) => old,
        None => {
            let new = text.leak();
            all.insert(new);
            new
        }
    }
}

/// A stable code. Named, because serde would otherwise borrow a `&str` field from its input.
pub type Code = &'static str;

fn kept_code<'de, D: Deserializer<'de>>(d: D) -> Result<&'static str, D::Error> {
    String::deserialize(d).map(kept)
}

/// Reads an optional code or name from the worker.
pub fn kept_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<&'static str>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.map(kept))
}

/// A failure: what went wrong and how to fix it, with its place in the source when known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Error {
    #[serde(deserialize_with = "kept_code")]
    pub code: Code,
    pub message: String,
    pub hint: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub excerpt: Option<Box<Excerpt>>,
}

/// A source line as shown under an error, cut to 200 characters, and the character (from 1)
/// the caret points at in that cut text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Excerpt {
    pub text: String,
    pub caret: Option<usize>,
    /// Where the problem ends (line, column), when the engine gives a range.
    pub end: Option<(usize, usize)>,
}

/// Something worth knowing about a run that still succeeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    #[serde(deserialize_with = "kept_code")]
    pub code: Code,
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

    /// An engine's own warning, as `{Engine}: {text}` without the engine's `Warning:` label.
    pub fn engine(engine: &str, text: &str, line: Option<usize>) -> Self {
        Self {
            code: "engine_warning",
            message: format!("{engine}: {}", unlabelled(text)),
            line,
        }
    }

    pub fn unsafe_content_removed(n: usize) -> Self {
        let items = if n == 1 { "item" } else { "items" };
        Self {
            code: "unsafe_content_removed",
            message: format!(
                "Removed {n} unsafe {items} (scripts, event handlers or javascript: links) from the SVG."
            ),
            line: None,
        }
    }

    /// sfdp in this Graphviz build, which says `Error:` but writes a good SVG.
    pub fn sfdp_overlaps() -> Self {
        Self {
            code: "engine_warning",
            message: "Graphviz: sfdp laid this out without removing node overlaps (this build has no triangulation library), so nodes may overlap.".into(),
            line: None,
        }
    }

    /// An MCP render whose PNG preview failed, though its output didn't.
    pub fn no_preview(reason: &str) -> Self {
        Self {
            code: "no_preview",
            message: format!("No preview image: {reason}"),
            line: None,
        }
    }

    pub fn extra_graphs_ignored(n: usize) -> Self {
        Self {
            code: "extra_graphs_ignored",
            message: format!("The file has {n} graphs; rendered the first."),
            line: None,
        }
    }

    pub fn background_ignored() -> Self {
        Self {
            code: "background_ignored",
            message: "D2 uses its theme's background; --background was ignored.".into(),
            line: None,
        }
    }

    pub fn remote_images_not_fetched(n: usize) -> Self {
        let images = if n == 1 { "image" } else { "images" };
        Self {
            code: "remote_images_not_fetched",
            message: format!(
                "Left {n} remote {images} as links; a browser loads them when it shows the SVG. Use --allow-network to embed them."
            ),
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

    pub fn scale_reduced(width: u32, height: u32, scale: f64) -> Self {
        Self {
            code: "scale_reduced",
            message: format!(
                "The PNG would be {width}×{height} px; drew it at scale {scale} to stay within 16384 px and 64 MP."
            ),
            line: None,
        }
    }

    pub fn font_substituted(family: &str, substitute: &str) -> Self {
        Self {
            code: "font_substituted",
            message: format!("Font {family} isn't available; drew its text with {substitute}."),
            line: None,
        }
    }

    /// `missing`: some of the characters, in the order met.
    pub fn font_fallback(missing: &[char]) -> Self {
        let sample: String = missing.iter().collect();
        Self {
            code: "font_fallback",
            message: format!(
                "Some characters aren't in the bundled fonts ({sample}); this computer's fonts drew them, so the PNG can differ between computers."
            ),
            line: None,
        }
    }

    pub fn pages_more(input: &str, n: usize, first: &str) -> Self {
        Self {
            code: "pages_more",
            message: format!(
                "{input} has {n} pages; rendered page 1 ({first}). Use --page for another."
            ),
            line: None,
        }
    }

    pub fn di_reference_missing(id: &str, reference: &str, line: usize) -> Self {
        Self {
            code: "di_reference_missing",
            message: format!("Diagram shape {id} points to {reference}, which isn't in the model."),
            line: Some(line),
        }
    }

    pub fn element_approximated(n: usize, types: &[&str]) -> Self {
        Self {
            code: "element_approximated",
            message: format!(
                "Drew {n} BPMN elements as plain boxes: {}.",
                types.join(", ")
            ),
            line: None,
        }
    }

    pub fn parent_missing(id: &str, parent: &str, line: usize) -> Self {
        Self {
            code: "parent_missing",
            message: format!(
                "Cell {id} points to a missing parent {parent}; drew it at the top level."
            ),
            line: Some(line),
        }
    }

    /// `names`: up to 5 of the shapes, each once.
    pub fn shape_approximated(n: usize, names: &[&str]) -> Self {
        Self {
            code: "shape_approximated",
            message: format!("Drew {n} shapes as plain boxes: {}.", names.join(", ")),
            line: None,
        }
    }

    /// `edge_style_approximated` or `arrow_approximated`.
    pub fn drawn_as(code: &'static str, name: &str, fallback: &str) -> Self {
        Self {
            code,
            message: format!("Drew {name} as {fallback}."),
            line: None,
        }
    }

    pub fn image_missing(id: &str, line: usize) -> Self {
        Self {
            code: "image_missing",
            message: format!("Image {id} has no image data in the file; drew a placeholder."),
            line: Some(line),
        }
    }

    pub fn html_simplified() -> Self {
        Self {
            code: "html_simplified",
            message: "Some HTML in labels was reduced to plain text.".into(),
            line: None,
        }
    }

    pub fn remote_image_not_fetched(url: &str) -> Self {
        let cut: String = url.chars().take(80).collect();
        let cut = if cut.len() < url.len() {
            cut + "…"
        } else {
            cut
        };
        Self {
            code: "remote_image_not_fetched",
            message: format!(
                "Image {cut} isn't fetched (the tool works offline); drew a placeholder."
            ),
            line: None,
        }
    }

    /// `element_skipped` or `edge_skipped`: `kind` is what the item is, such as `edge`.
    pub fn image_over_limit(width: u32, height: u32) -> Self {
        Self {
            code: "element_skipped",
            message: format!(
                "Skipped an embedded image: {width}×{height} px is over the limit of 16384 px and 64 MP."
            ),
            line: None,
        }
    }

    pub fn skipped(code: &'static str, kind: &str, id: &str, reason: &str, line: usize) -> Self {
        Self {
            code,
            message: format!("Skipped {kind} {id}: {reason}."),
            line: Some(line),
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
            "engine_missing" | "engine_failed" | "timeout" | "write_failed" | "port_in_use"
            | "internal" => 3,
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
        let (end_line, end_column) = self.excerpt.as_ref().and_then(|x| x.end).unzip();
        json!({
            "code": e["code"], "message": e["message"], "hint": e["hint"],
            "line": e["line"], "column": e["column"], "end_line": end_line, "end_column": end_column,
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

    /// A compressed page or embedded diagram that unpacks past the limit.
    pub fn too_large_unpacked(path: &str) -> Self {
        Self::new(
            "too_large",
            format!("{path} unpacks to over 256 MiB; the limit is 256 MiB."),
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
    /// The bundled fonts are part of the install, like the engines.
    pub fn fonts_missing(path: &str) -> Self {
        Self::new(
            "engine_missing",
            format!("The fonts PNGs are drawn with are missing: {path} doesn't exist."),
            "Reinstall diagram-render.",
        )
    }

    pub fn engine_failed(engine: &str, code: &str, first: Option<&str>) -> Self {
        let said = first
            .map(|l| format!(": {}", clause(l)))
            .unwrap_or_default();
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

    /// A diagram error at a line; `input` adds that `check` lists them all, for engines that
    /// can find more than one.
    pub fn syntax_error(message: &str, line: usize, input: Option<&str>) -> Self {
        let more = input.map_or_else(String::new, |i| {
            format!(" `diagram-render check {i}` lists every error.")
        });
        Self::new(
            "syntax_error",
            unlabelled(message),
            format!("Fix line {line}, then run again.{more}"),
        )
    }

    pub fn render_failed(engine: &str, said: &str, input: &str) -> Self {
        Self::new(
            "render_failed",
            format!("{engine} couldn't render this diagram: {}.", clause(said)),
            format!("Run diagram-render check {input}, or --debug for the engine's full output."),
        )
    }

    /// `names`: every page's, of which the first 10 are listed.
    pub fn page_not_found(input: &str, page: &str, names: &[&str]) -> Self {
        let more = if names.len() > 10 { ", …" } else { "" };
        let list = names[..names.len().min(10)].join(", ");
        Self::new(
            "page_not_found",
            format!("{input} has no page {page}; it has: {list}{more}."),
            "Pass --page with one of those, or leave it out for the first.",
        )
    }

    pub fn invalid_scene(reason: &str) -> Self {
        Self::new(
            "invalid_scene",
            format!("This isn't an Excalidraw scene: {reason}."),
            "Open it in Excalidraw and save it again.",
        )
    }

    /// A scene whose embedded images add up to more than the limit.
    pub fn too_much_image_data(path: &str, size: &str) -> Self {
        Self::new(
            "too_large",
            format!("{path} holds {size} of image data; the limit is 64 MiB."),
            "Split the diagram, or remove embedded images.",
        )
    }

    pub fn no_diagram_layout() -> Self {
        Self::new(
            "no_diagram_layout",
            "This BPMN file has no diagram layout (no BPMNDiagram section).",
            "Open it in a BPMN modeler (such as Camunda Modeler) and save it to add one.",
        )
    }

    pub fn too_many_elements(n: usize, limit: usize) -> Self {
        Self::new(
            "too_many_elements",
            format!("This diagram has {n} elements; the limit is {limit}."),
            "Split it into smaller diagrams.",
        )
    }

    /// D2 can't list its boards, so the message can't either.
    pub fn board_not_found(page: &str) -> Self {
        Self::new(
            "page_not_found",
            format!("D2 found no board {page}."),
            "Pass --page with a board's path, such as layers.deploy, or leave it out for the root board.",
        )
    }

    pub fn network_needed() -> Self {
        Self::new(
            "network_needed",
            "This D2 diagram uses remote images, and the network is off.",
            "Rerun with --allow-network, or render SVG (which links them).",
        )
    }

    pub fn d2_files_need_network() -> Self {
        Self::new(
            "network_needed",
            "On Windows D2 imports and images could reach the network, so they are off.",
            "Rerun with --allow-network if you trust this diagram.",
        )
    }

    /// A run stopped at the memory limit.
    pub fn out_of_memory(mib: u64) -> Self {
        Self::new(
            "too_large",
            format!("Rendering took over {mib} MiB of memory and was stopped."),
            "Simplify the diagram or split it into smaller ones.",
        )
    }

    /// `path`, cut to 80 characters.
    pub fn network_path(path: &str) -> Self {
        let cut: String = path.chars().take(80).collect();
        Self::new(
            "network_path",
            format!("The network path {cut} isn't opened."),
            "Copy the file to this computer and use a local path.",
        )
    }

    pub fn image_too_large(width: u32, height: u32) -> Self {
        Self::new(
            "image_too_large",
            format!("The diagram is {width}×{height} px; a PNG would need a scale below 0.05."),
            "Render SVG instead.",
        )
    }

    pub fn timeout(seconds: u64) -> Self {
        Self::new(
            "timeout",
            format!("Rendering took longer than {seconds} s and was stopped."),
            "Simplify the diagram, or raise --timeout (up to 600).",
        )
    }

    pub fn write_failed(path: &str, e: &std::io::Error) -> Self {
        Self::new(
            "write_failed",
            format!("Can't write {path}: {}.", reason(e)),
            "Check the folder exists and isn't read-only.",
        )
    }

    pub fn port_in_use(port: u16) -> Self {
        Self::new(
            "port_in_use",
            format!("Port {port} is in use."),
            "Leave out --port to use a free one.",
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

/// An engine's message without its own leading `Error:` or `Warning:` labels (Graphviz can
/// print two).
fn unlabelled(text: &str) -> String {
    let mut text = text.trim();
    while let Some(rest) = ["Error:", "Warning:", "error:", "warning:"]
        .iter()
        .find_map(|label| text.strip_prefix(label))
    {
        text = rest.trim_start();
    }
    text.into()
}

/// An engine's message, ready to end a sentence: unlabelled, without its final full stop.
fn clause(text: &str) -> String {
    unlabelled(text).trim_end_matches('.').into()
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
    fn engine_messages_lose_their_own_labels() {
        let w = Warning::engine(
            "Graphviz",
            "Warning: Warning: no value for width\r",
            Some(2),
        );
        assert_eq!(w.message, "Graphviz: no value for width");
        let e = Error::render_failed("Graphviz", "Error: mismatched tag in line 1 ", "a.dot");
        assert_eq!(
            e.message,
            "Graphviz couldn't render this diagram: mismatched tag in line 1."
        );
        let e = Error::syntax_error("bad", 3, Some("a.mmd"));
        assert_eq!(
            e.hint,
            "Fix line 3, then run again. `diagram-render check a.mmd` lists every error."
        );
        assert_eq!(Error::timeout(5).exit_code(), 3);
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
