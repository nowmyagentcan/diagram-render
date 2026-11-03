//! Which diagram language an input is: `--format` first, then the file extension, then the
//! content (XML root, JSON type, PNG text chunks, the Mermaid keyword or DOT header), else D2.

use std::ffi::OsString;
use std::path::Path;

use crate::error::{Error, Warning};
use crate::input;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mermaid,
    Dot,
    D2,
    Bpmn,
    Drawio,
    Excalidraw,
}

impl Format {
    pub const ALL: [Self; 6] = [
        Self::Mermaid,
        Self::Dot,
        Self::D2,
        Self::Bpmn,
        Self::Drawio,
        Self::Excalidraw,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Mermaid => "mermaid",
            Self::Dot => "dot",
            Self::D2 => "d2",
            Self::Bpmn => "bpmn",
            Self::Drawio => "drawio",
            Self::Excalidraw => "excalidraw",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mermaid => "Mermaid",
            Self::Dot => "Graphviz DOT",
            Self::D2 => "D2",
            Self::Bpmn => "BPMN 2.0",
            Self::Drawio => "draw.io",
            Self::Excalidraw => "Excalidraw",
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Mermaid => &[".mmd", ".mermaid"],
            Self::Dot => &[".dot", ".gv"],
            Self::D2 => &[".d2"],
            Self::Bpmn => &[".bpmn", ".bpmn2"],
            Self::Drawio => &[
                ".drawio",
                ".dio",
                ".drawio.xml",
                ".drawio.svg",
                ".drawio.png",
            ],
            Self::Excalidraw => &[
                ".excalidraw",
                ".excalidraw.json",
                ".excalidraw.svg",
                ".excalidraw.png",
            ],
        }
    }

    /// The engine that renders it, and its version.
    pub fn engine(self) -> (&'static str, &'static str) {
        match self {
            Self::Mermaid => ("merman", "0.9.3"),
            Self::Dot => ("graphviz", "16.1.0"),
            Self::D2 => ("d2", "0.9.0"),
            _ => ("diagram-render", env!("CARGO_PKG_VERSION")),
        }
    }

    /// `--format`'s value: a format, or `None` for `auto`.
    pub fn parse(v: &str) -> Option<Option<Self>> {
        Some(Some(match v.to_ascii_lowercase().as_str() {
            "auto" => return Some(None),
            "mermaid" => Self::Mermaid,
            "dot" | "graphviz" | "gv" => Self::Dot,
            "d2" => Self::D2,
            "bpmn" => Self::Bpmn,
            "drawio" => Self::Drawio,
            "excalidraw" => Self::Excalidraw,
            _ => return None,
        }))
    }

    /// Mermaid, DOT and D2 are plain text, limited to 4 MiB.
    fn is_text(self) -> bool {
        matches!(self, Self::Mermaid | Self::Dot | Self::D2)
    }

    pub fn has_pages(self) -> bool {
        matches!(self, Self::D2 | Self::Bpmn | Self::Drawio)
    }
}

/// The format a lowercase file name's extension names, and that extension; the longest match
/// wins, so `.drawio.png` beats `.png`.
fn extension(name: &str) -> Option<(Format, &'static str)> {
    let all = Format::ALL
        .iter()
        .flat_map(|&f| f.extensions().iter().map(move |&e| (f, e)));
    all.filter(|(_, ext)| name.ends_with(ext))
        .max_by_key(|(_, ext)| ext.len())
}

/// Text sources over this are refused.
const TEXT_LIMIT: usize = 4 << 20;

const BPMN_NS: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// An input whose format is known.
pub struct Diagram {
    pub format: Format,
    /// The Mermaid family, or `graph`/`digraph` for DOT.
    pub diagram_type: Option<&'static str>,
    /// The source, except for PNG files that carry a diagram.
    pub text: Option<String>,
    pub warnings: Vec<Warning>,
}

/// The format of `bytes`, read from `path` (`None` for stdin), unless `forced`. `shown` names
/// the input in messages.
pub fn detect(
    path: Option<&Path>,
    shown: &str,
    bytes: &[u8],
    forced: Option<Format>,
) -> Result<Diagram, Error> {
    let named = match forced {
        Some(f) => Some(f),
        None => path.map(by_extension).transpose()?.flatten(),
    };
    let mut diagram = Diagram {
        format: Format::D2,
        diagram_type: None,
        text: None,
        warnings: vec![],
    };
    if bytes.starts_with(PNG) && !named.is_some_and(Format::is_text) {
        let format = named.or_else(|| embedded_in_png(bytes));
        diagram.format = format.ok_or_else(|| Error::unsupported_image(shown))?;
        return Ok(diagram);
    }
    let text = input::text(bytes)?;
    diagram.format = match named {
        Some(f) => f,
        None => by_content(&text, shown)?.unwrap_or_else(|| {
            diagram.warnings.push(Warning::format_guessed());
            Format::D2
        }),
    };
    if diagram.format.is_text() && bytes.len() > TEXT_LIMIT {
        return Err(Error::too_large(shown, &input::mib(bytes.len() as u64)));
    }
    diagram.diagram_type = match diagram.format {
        Format::Mermaid => mermaid_type(&text),
        Format::Dot => dot_type(&text),
        _ => None,
    };
    diagram.text = Some(text);
    Ok(diagram)
}

/// The format a file name says, or the refusal of a format the tool doesn't read.
fn by_extension(path: &Path) -> Result<Option<Format>, Error> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let name = name.to_ascii_lowercase();
    if let Some((format, _)) = extension(&name) {
        return Ok(Some(format));
    }
    match name.rsplit_once('.').map_or("", |(_, ext)| ext) {
        "md" => Err(Error::unsupported_markdown()),
        "puml" | "plantuml" | "pu" => Err(Error::unsupported_plantuml()),
        "vsdx" | "vsd" => Err(Error::unsupported_visio()),
        _ => Ok(None),
    }
}

/// The file name of `path` without its diagram extension (or else its last one), as given: never
/// transcoded, so any name the system allows survives.
pub fn stem(path: &Path) -> OsString {
    let name = path.file_name().unwrap_or_default();
    let lower = name.to_string_lossy().to_ascii_lowercase();
    let dots = extension(&lower).map_or(1, |(_, ext)| ext.matches('.').count());
    let mut stem = Path::new(name);
    for _ in 0..dots {
        stem = Path::new(stem.file_stem().unwrap_or(stem.as_os_str()));
    }
    stem.as_os_str().to_owned()
}

/// The format a PNG's text chunks embed.
fn embedded_in_png(bytes: &[u8]) -> Option<Format> {
    let mut at = PNG.len();
    while let Some(head) = bytes.get(at..at + 8) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let data = bytes.get(at + 8..(at + 8).checked_add(len)?)?;
        if matches!(&head[4..], b"tEXt" | b"zTXt" | b"iTXt") {
            match data.split(|&b| b == 0).next() {
                Some(b"mxfile" | b"mxGraphModel") => return Some(Format::Drawio),
                Some(b"application/vnd.excalidraw+json") => return Some(Format::Excalidraw),
                _ => {}
            }
        }
        if &head[4..] == b"IEND" {
            break;
        }
        at += len + 12;
    }
    None
}

/// The format the content says; `None` when it says nothing, so it's read as D2.
fn by_content(text: &str, shown: &str) -> Result<Option<Format>, Error> {
    let start = text.trim_start();
    if let Some((name, attrs)) = start.strip_prefix('<').and_then(|_| xml_root(start)) {
        let (prefix, local) = name.split_once(':').unwrap_or(("", name));
        let attr = |key: &str| attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        let xmlns = match prefix {
            "" => "xmlns".to_string(),
            p => format!("xmlns:{p}"),
        };
        let content = attr("content").unwrap_or_default().trim_start();
        return match local {
            "mxfile" | "mxGraphModel" => Ok(Some(Format::Drawio)),
            "definitions" if attr(&xmlns) == Some(BPMN_NS) => Ok(Some(Format::Bpmn)),
            "svg" if content.starts_with("&lt;mxfile") || content.starts_with("<mxfile") => {
                Ok(Some(Format::Drawio))
            }
            "svg" if text.contains("payload-type:application/vnd.excalidraw+json") => {
                Ok(Some(Format::Excalidraw))
            }
            "svg" => Err(Error::unsupported_image(shown)),
            _ => Err(Error::unknown_format()),
        };
    }
    if start.starts_with('{')
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(text)
    {
        return match json["type"] == "excalidraw" {
            true => Ok(Some(Format::Excalidraw)),
            false => Err(Error::unknown_format()),
        };
    }
    if mermaid_type(text).is_some() {
        return Ok(Some(Format::Mermaid));
    }
    Ok(dot_type(text).map(|_| Format::Dot))
}

/// The root element's name and attributes (values as written), after any XML declaration,
/// comments, processing instructions and doctype.
fn xml_root(mut s: &str) -> Option<(&str, Vec<(&str, &str)>)> {
    loop {
        s = s.trim_start();
        if let Some(r) = s.strip_prefix("<?") {
            s = &r[r.find("?>")? + 2..];
        } else if let Some(r) = s.strip_prefix("<!--") {
            s = &r[r.find("-->")? + 3..];
        } else if let Some(r) = s.strip_prefix("<!") {
            // A doctype's internal subset may hold `>`, so skip past its `]` first.
            let from = match (r.find('['), r.find('>')) {
                (Some(open), Some(close)) if open < close => open + r[open..].find(']')?,
                _ => 0,
            };
            s = &r[from + r[from..].find('>')? + 1..];
        } else {
            break;
        }
    }
    let s = s.strip_prefix('<')?;
    let end = s.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
    let (name, mut s) = s.split_at(end);
    let mut attrs = vec![];
    loop {
        s = s.trim_start();
        let Some(eq) = s.find('=').filter(|_| !s.starts_with(['>', '/'])) else {
            break;
        };
        let key = s[..eq].trim_end();
        let r = s[eq + 1..].trim_start();
        let Some(quote) = r.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            break;
        };
        let Some(close) = r[1..].find(quote) else {
            break;
        };
        attrs.push((key, &r[1..1 + close]));
        s = &r[close + 2..];
    }
    (!name.is_empty()).then_some((name, attrs))
}

/// Mermaid's first keywords and the `diagram_type` each gives.
const KEYWORDS: &[(&str, &str)] = &[
    ("flowchart", "flowchart"),
    ("flowchart-elk", "flowchart"),
    ("sequenceDiagram", "sequence"),
    ("classDiagram", "class"),
    ("classDiagram-v2", "class"),
    ("stateDiagram", "state"),
    ("stateDiagram-v2", "state"),
    ("erDiagram", "er"),
    ("gantt", "gantt"),
    ("pie", "pie"),
    ("journey", "journey"),
    ("mindmap", "mindmap"),
    ("timeline", "timeline"),
    ("kanban", "kanban"),
    ("gitGraph", "gitgraph"),
    ("quadrantChart", "quadrant"),
    ("xychart", "xychart"),
    ("xychart-beta", "xychart"),
    ("sankey", "sankey"),
    ("sankey-beta", "sankey"),
    ("block", "block"),
    ("block-beta", "block"),
    ("architecture", "architecture"),
    ("architecture-beta", "architecture"),
    ("packet", "packet"),
    ("packet-beta", "packet"),
    ("radar", "radar"),
    ("radar-beta", "radar"),
    ("treemap", "treemap"),
    ("treemap-beta", "treemap"),
    ("requirementDiagram", "requirement"),
    ("requirement", "requirement"),
    ("C4Context", "c4"),
    ("C4Container", "c4"),
    ("C4Component", "c4"),
    ("C4Dynamic", "c4"),
    ("C4Deployment", "c4"),
    ("zenuml", "zenuml"),
    ("info", "info"),
    ("agentflow-beta", "agentflow"),
    ("cynefin-beta", "cynefin"),
    ("eventmodeling", "eventmodeling"),
    ("ishikawa", "ishikawa"),
    ("ishikawa-beta", "ishikawa"),
    ("railroad-beta", "railroad"),
    ("swimlane-beta", "swimlane"),
    ("treeView-beta", "treeview"),
    ("usecase-beta", "usecase"),
    ("venn-beta", "venn"),
    ("wardley-beta", "wardley"),
];

/// The Mermaid family of the first meaningful line (after blank lines, `%%` comments and
/// directives, and a `---` front-matter block).
fn mermaid_type(text: &str) -> Option<&'static str> {
    let mut lines = text.split(['\r', '\n']).map(str::trim);
    let mut first = true;
    let line = loop {
        let line = lines.next()?;
        if line.is_empty() {
            continue;
        }
        if first && line == "---" {
            lines.by_ref().find(|l| *l == "---")?;
        } else if line.starts_with("%%{") && !line.contains("}%%") {
            lines.by_ref().find(|l| l.contains("}%%"))?;
        } else if !line.starts_with("%%") {
            break line;
        }
        first = false;
    };
    let end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(line.len());
    let (word, rest) = line.split_at(end);
    let rest = rest.trim_start();
    if word == "graph" {
        // `graph TD` has a direction, DOT's `graph G {` never does.
        let direction = rest.split([' ', '\t', ';']).next().unwrap_or_default();
        let after = rest[direction.len()..].trim_start();
        let directions = ["TB", "TD", "BT", "RL", "LR"];
        let ok = (direction.is_empty() && !rest.starts_with('{'))
            || (directions.iter().any(|d| d.eq_ignore_ascii_case(direction))
                && !after.starts_with('{'));
        return ok.then_some("flowchart");
    }
    // D2 lines such as `pie -> slice` or `block: {` start with a word too.
    let d2 = ["->", "<-", "--", ":", "{", "."]
        .iter()
        .any(|p| rest.starts_with(p));
    let family = KEYWORDS.iter().find(|(k, _)| *k == word)?.1;
    (!d2 || word == "gitGraph" && rest.starts_with(':')).then_some(family)
}

/// `graph` or `digraph` when the text starts with `[strict] (graph|digraph) [ID] {` (after
/// comments).
fn dot_type(text: &str) -> Option<&'static str> {
    let mut s = skip_comments(text);
    let mut word = take_id(&mut s)?;
    if word.eq_ignore_ascii_case("strict") {
        s = skip_comments(s);
        word = take_id(&mut s)?;
    }
    let kind = ["graph", "digraph"]
        .into_iter()
        .find(|k| k.eq_ignore_ascii_case(word))?;
    s = skip_comments(s);
    if !s.starts_with('{') {
        take_id(&mut s)?;
        s = skip_comments(s);
    }
    s.starts_with('{').then_some(kind)
}

/// `s` after leading white space and `//`, `/* */` and `#` comments.
fn skip_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if s.starts_with("//") || s.starts_with('#') {
            s = s.find('\n').map_or("", |n| &s[n..]);
        } else if let Some(r) = s.strip_prefix("/*") {
            s = r.find("*/").map_or("", |n| &r[n + 2..]);
        } else {
            return s;
        }
    }
}

/// Takes a DOT ID (a word, a number, a quoted string or an `<html>` string) off the front of `s`.
fn take_id<'a>(s: &mut &'a str) -> Option<&'a str> {
    let end = if let Some(r) = s.strip_prefix('"') {
        let mut escaped = false;
        1 + r.find(|c| {
            let close = c == '"' && !escaped;
            escaped = c == '\\' && !escaped;
            close
        })? + 1
    } else if s.starts_with('<') {
        let mut depth = 0usize;
        1 + s.find(|c| {
            depth = match c {
                '<' => depth + 1,
                '>' => depth - 1,
                _ => depth,
            };
            depth == 0
        })?
    } else {
        s.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '-') || c > '\u{7F}'))
            .unwrap_or(s.len())
    };
    let (id, rest) = s.split_at(end);
    *s = rest;
    (!id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guess(text: &str) -> (Format, Option<&'static str>, Vec<&'static str>) {
        let d = detect(None, "stdin", text.as_bytes(), None).unwrap();
        (
            d.format,
            d.diagram_type,
            d.warnings.iter().map(|w| w.code).collect(),
        )
    }

    #[test]
    fn extensions_choose_the_format_longest_first() {
        let cases = [
            ("flow.MMD", Format::Mermaid),
            ("a.mermaid", Format::Mermaid),
            ("g.gv", Format::Dot),
            ("g.dot", Format::Dot),
            ("x.d2", Format::D2),
            ("p.bpmn2", Format::Bpmn),
            ("b.drawio.png", Format::Drawio),
            ("b.drawio.svg", Format::Drawio),
            ("b.drawio.xml", Format::Drawio),
            ("b.dio", Format::Drawio),
            ("s.excalidraw.png", Format::Excalidraw),
            ("s.Excalidraw.JSON", Format::Excalidraw),
        ];
        for (name, want) in cases {
            assert_eq!(by_extension(Path::new(name)).unwrap(), Some(want), "{name}");
        }
        for name in ["notes.txt", "x.svg", "x.png", "x.json", "x.xml", "noext"] {
            assert_eq!(by_extension(Path::new(name)).unwrap(), None, "{name}");
        }
        let refused = |name| by_extension(Path::new(name)).unwrap_err().message;
        assert_eq!(
            refused("README.md"),
            "Markdown files aren't rendered as a whole."
        );
        assert_eq!(refused("a.puml"), "PlantUML isn't supported.");
        assert_eq!(refused("a.PU"), "PlantUML isn't supported.");
        assert_eq!(refused("a.vsdx"), "Visio files aren't supported.");
    }

    #[test]
    fn an_extension_beats_the_content_and_format_beats_both() {
        let mmd = detect(Some(Path::new("x.mmd")), "x.mmd", b"a -> b", None).unwrap();
        assert_eq!((mmd.format, mmd.diagram_type), (Format::Mermaid, None));
        assert!(mmd.warnings.is_empty());
        let forced = detect(
            Some(Path::new("a.md")),
            "a.md",
            b"graph TD\n",
            Some(Format::Mermaid),
        );
        assert_eq!(forced.unwrap().diagram_type, Some("flowchart"));
        let d2 = detect(None, "-", b"graph TD\n", Some(Format::D2)).unwrap();
        assert_eq!((d2.format, d2.diagram_type), (Format::D2, None));
    }

    #[test]
    fn mermaid_is_known_by_its_first_keyword() {
        let cases = [
            ("graph TD; A-->B", "flowchart"),
            ("graph", "flowchart"),
            ("graph lr\n", "flowchart"),
            ("flowchart LR\n  a --> b", "flowchart"),
            ("\n\n%% note\nsequenceDiagram\n  A->>B: hi", "sequence"),
            ("---\ntitle: x\nconfig: {}\n---\nclassDiagram-v2\n", "class"),
            (
                "%%{init: {\n 'theme': 'dark'\n}}%%\nstateDiagram-v2",
                "state",
            ),
            ("%%{init: {'theme':'dark'}}%%\r\nerDiagram\r\n", "er"),
            ("  pie title Pets\n", "pie"),
            ("gitGraph:\n commit", "gitgraph"),
            ("C4Deployment\n", "c4"),
            ("xychart-beta\n", "xychart"),
            ("zenuml\n  A->B: hi", "zenuml"),
            ("venn-beta\n", "venn"),
            ("treeView-beta\n", "treeview"),
        ];
        for (text, family) in cases {
            assert_eq!(
                guess(text),
                (Format::Mermaid, Some(family), vec![]),
                "{text:?}"
            );
        }
    }

    #[test]
    fn dot_is_known_by_its_graph_header() {
        let cases = [
            ("digraph { a -> b }", "digraph"),
            ("graph {\n a -- b\n}", "graph"),
            ("strict digraph G {", "digraph"),
            ("STRICT DiGraph \"my graph\" {", "digraph"),
            (
                "// c\n/* block\n comment */\n# line\ndigraph g\n{\n}",
                "digraph",
            ),
            ("graph <<b>x</b>> {", "graph"),
            ("digraph \"a\\\"b\" {", "digraph"),
            ("digraph G{", "digraph"),
        ];
        for (text, kind) in cases {
            assert_eq!(guess(text), (Format::Dot, Some(kind), vec![]), "{text:?}");
        }
    }

    #[test]
    fn any_other_text_is_read_as_d2_with_a_warning() {
        for text in [
            "a -> b",
            "graph -> b",
            "pie -> slice",
            "block: {\n}",
            "digraph",
            "graph G",
        ] {
            assert_eq!(
                guess(text),
                (Format::D2, None, vec!["format_guessed"]),
                "{text:?}"
            );
        }
    }

    #[test]
    fn xml_json_and_svg_are_known_by_their_root() {
        let drawio = r#"<?xml version="1.0"?><!-- x --><mxfile host="x"><diagram/></mxfile>"#;
        assert_eq!(guess(drawio).0, Format::Drawio);
        assert_eq!(
            guess("<mxGraphModel><root/></mxGraphModel>").0,
            Format::Drawio
        );
        let bpmn = r#"<?xml version="1.0"?>
<!DOCTYPE x [ <!ENTITY a "b>"> ]>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL" id="d"/>"#;
        assert_eq!(guess(bpmn).0, Format::Bpmn);
        let bpmn = r#"<definitions xmlns='http://www.omg.org/spec/BPMN/20100524/MODEL'>"#;
        assert_eq!(guess(bpmn).0, Format::Bpmn);
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" content="&lt;mxfile host=&quot;x&quot;&gt;"/>"#;
        assert_eq!(guess(svg).0, Format::Drawio);
        let svg =
            "<svg><metadata><!-- payload-type:application/vnd.excalidraw+json --></metadata></svg>";
        assert_eq!(guess(svg).0, Format::Excalidraw);
        assert_eq!(
            guess(r#"{"type": "excalidraw", "elements": []}"#).0,
            Format::Excalidraw
        );
        let failed = |text: &str| detect(None, "stdin", text.as_bytes(), None).err().unwrap();
        assert_eq!(
            failed("<svg width='2'/>").message,
            "stdin is an image with no draw.io or Excalidraw diagram inside."
        );
        assert_eq!(
            failed("<definitions xmlns='urn:other'/>").code,
            "unknown_format"
        );
        assert_eq!(failed("<html><body/></html>").code, "unknown_format");
        assert_eq!(failed(r#"{"type": "other"}"#).code, "unknown_format");
    }

    /// A PNG with one text chunk `key` (CRCs aren't checked by detection).
    fn png(key: &[u8]) -> Vec<u8> {
        let mut png = PNG.to_vec();
        for (kind, data) in [
            (&b"IHDR"[..], &[0u8; 13][..]),
            (b"tEXt", &[key, b"\0x"].concat()),
            (b"IEND", b""),
        ] {
            png.extend((data.len() as u32).to_be_bytes());
            png.extend(kind);
            png.extend(data);
            png.extend([0; 4]);
        }
        png
    }

    #[test]
    fn pngs_are_known_by_their_text_chunks() {
        let found =
            |bytes: &[u8]| detect(Some(Path::new("x.png")), "x.png", bytes, None).map(|d| d.format);
        assert_eq!(found(&png(b"mxfile")), Ok(Format::Drawio));
        assert_eq!(found(&png(b"mxGraphModel")), Ok(Format::Drawio));
        assert_eq!(
            found(&png(b"application/vnd.excalidraw+json")),
            Ok(Format::Excalidraw)
        );
        let plain = found(&png(b"Software")).unwrap_err();
        assert_eq!(
            plain.message,
            "x.png is an image with no draw.io or Excalidraw diagram inside."
        );
        assert_eq!(plain.hint, "Nothing to render; it's already an image.");
        assert_eq!(
            found(&png(b"mxfile")[..20]).unwrap_err().code,
            "unsupported_input"
        );
        let named = detect(
            Some(Path::new("x.drawio.png")),
            "x.drawio.png",
            &png(b"x"),
            None,
        );
        assert!(named.unwrap().text.is_none());
    }

    #[test]
    fn text_formats_stop_at_4_mib() {
        let big = format!("graph TD\n{}", "%".repeat(4 << 20));
        let e = detect(None, "stdin", big.as_bytes(), None).err().unwrap();
        assert_eq!(
            e.message,
            "stdin is 4.0 MiB; the limit is 64 MiB (4 MiB for Mermaid, DOT and D2 text)."
        );
        let bpmn = format!("<mxfile>{}</mxfile>", " ".repeat(4 << 20));
        assert!(detect(None, "stdin", bpmn.as_bytes(), None).is_ok());
    }

    #[test]
    fn stems_drop_the_diagram_extension_or_the_last_one() {
        let cases = [
            ("flow.mmd", "flow"),
            ("dir/Board.DRAWIO.PNG", "Board"),
            ("scene.excalidraw.png", "scene"),
            ("a.b.excalidraw.json", "a.b"),
            ("notes.v2.txt", "notes.v2"),
            ("noext", "noext"),
            (".mmd", ".mmd"),
            ("été.d2", "été"),
        ];
        for (path, want) in cases {
            assert_eq!(stem(Path::new(path)), want, "{path}");
        }
    }
}
