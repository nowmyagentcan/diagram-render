//! The command line: `diagram-render [render] <INPUT> [OPTIONS]` and `diagram-render check <INPUT>`.

mod detect;
mod engine;
mod error;
mod input;
mod output;
mod text;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::thread;

use detect::Format;
use error::{Error, Warning};
use lexopt::Arg::{Long, Short, Value};
use output::{Target, To};
use serde_json::json;
use text::Look;

const HELP: &str = "\
Render Mermaid, DOT, D2, BPMN, draw.io and Excalidraw diagrams.

Usage: diagram-render <INPUT> [OPTIONS]
       diagram-render <COMMAND>

Commands:
  render   Render a diagram to SVG or PNG (the default)
  check    Check a diagram for errors without rendering it
  formats  List the formats and whether each engine runs here
  ui       Open the local web page
  mcp      Run the MCP server on stdio

Options:
  -o, --output <FILE>    .svg or .png path, or - for stdout [default: beside the input]
      --to <svg|png>     output format [default: from -o, else svg]
      --format <F>       auto, mermaid, dot, d2, bpmn, drawio, excalidraw [default: auto]
      --scale <N>        PNG pixels per SVG pixel, 0.1-8 [default: 2]
      --background <B>   auto, transparent, white, #RGB or #RRGGBB [default: auto]
      --page <P>         page number or name (draw.io, BPMN, D2) [default: the first]
      --allow-network    let D2 fetch remote images
      --timeout <S>      stop an engine after S seconds, 1-600 [default: 60]
      --json             print one JSON object
      --debug            show engine command lines and full output
  -h, --help             print help
  -V, --version          print version

Examples:
  diagram-render flow.mmd                          # writes flow.svg
  diagram-render flow.mmd -o flow.png
  diagram-render graph.dot --to png --scale 3
  diagram-render arch.d2 -o arch.png --allow-network
  diagram-render order.bpmn --to png
  diagram-render board.drawio --page 2 -o page2.svg
  diagram-render sketch.excalidraw --to png --background transparent
  echo \"graph TD; A-->B\" | diagram-render - --format mermaid -o ab.png --json
  diagram-render check flow.mmd
  diagram-render formats
";

const CHECK_HELP: &str = "\
Check a diagram for errors without rendering it.

Usage: diagram-render check <INPUT> [OPTIONS]

Options:
      --format <F>       auto, mermaid, dot, d2, bpmn, drawio, excalidraw [default: auto]
      --page <P>         page number or name (draw.io, BPMN, D2) [default: the first]
      --timeout <S>      stop an engine after S seconds, 1-600 [default: 60]
      --json             print one JSON object
      --debug            show engine command lines and full output
  -h, --help             print help

Examples:
  diagram-render check flow.mmd
  diagram-render check broken.d2 --json
";

const FORMATS_HELP: &str = "\
List the formats and whether each engine runs here.

Usage: diagram-render formats [OPTIONS]

Options:
      --json             print one JSON object
      --debug            show engine command lines and full output
  -h, --help             print help
";

#[derive(Clone, Copy, PartialEq)]
enum Command {
    Render,
    Check,
    Formats,
}

impl Command {
    fn word(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::Check => "check",
            Self::Formats => "formats",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::Render => HELP,
            Self::Check => CHECK_HELP,
            Self::Formats => FORMATS_HELP,
        }
    }

    /// Whether `option` belongs to this command.
    fn takes(self, option: &str) -> bool {
        let common = ["--json", "--debug", "--help"];
        let check = ["--format", "--page", "--timeout"];
        match self {
            Self::Render => true,
            Self::Check => common.contains(&option) || check.contains(&option),
            Self::Formats => common.contains(&option),
        }
    }
}

#[derive(Default)]
struct Args {
    input: Option<OsString>,
    output: Option<OsString>,
    to: Option<To>,
    scale: Option<f64>,
    format: Option<Format>,
    page: Option<String>,
    json: bool,
    debug: bool,
}

enum Parsed {
    Run(Args),
    Print(String),
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    // A usage error is JSON when --json comes before any `--`, wherever the error is.
    let json = args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
    // Only an exact first argument is a command: `./check` is a file.
    let (command, rest) = match args.first().and_then(|a| a.to_str()) {
        Some("render") => (Command::Render, &args[1..]),
        Some("check") => (Command::Check, &args[1..]),
        Some("formats") => (Command::Formats, &args[1..]),
        _ => (Command::Render, &args[..]),
    };
    let code = match parse(command, rest) {
        Ok(Parsed::Print(text)) => finish(io::stdout().write_all(text.as_bytes()), 0),
        Ok(Parsed::Run(args)) if command == Command::Formats => formats(&args),
        Ok(Parsed::Run(args)) => run(command, args),
        Err(e) => usage(&e, json),
    };
    ExitCode::from(code)
}

/// The run's exit code once its output is written: a reader that hung up ends it quietly with
/// the code it would have had, any other failure to write is 3.
fn finish(written: io::Result<()>, code: u8) -> u8 {
    match written {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => 3,
        _ => code,
    }
}

fn parse(command: Command, args: &[OsString]) -> Result<Parsed, Error> {
    let mut p = lexopt::Parser::from_args(args.iter().cloned());
    let mut args = Args::default();
    let mut seen: Vec<&'static str> = vec![];
    while let Some(arg) = p.next()? {
        let name = match arg {
            Short('o') => "--output",
            Short('h') => "--help",
            Short('V') => "--version",
            Long(long) => OPTIONS
                .iter()
                .map(|(o, _)| *o)
                .find(|o| o[2..] == *long)
                .unwrap_or(""),
            _ => "",
        };
        if !name.is_empty() && seen.contains(&name) {
            return Err(Error::invalid_option(
                format!("{name} is given twice."),
                "Give each option once.",
            ));
        }
        if !name.is_empty() && !command.takes(name) && name != "--version" {
            let word = command.word();
            return Err(Error::invalid_option(
                format!("{name} doesn't apply to {word}."),
                format!("Run diagram-render {word} --help for its options."),
            ));
        }
        seen.push(name);
        match (name, arg) {
            ("--help", _) => return Ok(Parsed::Print(command.help().into())),
            ("--version", _) if command == Command::Render => {
                return Ok(Parsed::Print(format!(
                    "diagram-render {}\n",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            ("--json", _) => args.json = true,
            ("--debug", _) => args.debug = true,
            ("--allow-network", _) => {}
            ("--output", _) => args.output = Some(p.value()?),
            ("--to", _) => args.to = Some(to(&string(&mut p, name)?)?),
            ("--format", _) => args.format = format(&string(&mut p, name)?)?,
            ("--scale", _) => args.scale = Some(scale(&string(&mut p, name)?)?),
            ("--background", _) => background(&string(&mut p, name)?)?,
            ("--page", _) => args.page = Some(page(string(&mut p, name)?)?),
            ("--timeout", _) => drop(timeout(&string(&mut p, name)?)?),
            (_, Value(_)) if command == Command::Formats => {
                return Err(Error::invalid_option(
                    "formats takes no file.",
                    "Run diagram-render formats on its own.",
                ));
            }
            (_, Value(input)) if args.input.is_none() => args.input = Some(input),
            (_, Value(_)) => return Err(Error::one_input_only()),
            (_, arg) => {
                let option = match arg {
                    Short(c) => format!("-{c}"),
                    Long(l) => format!("--{l}"),
                    Value(_) => unreachable!("values are matched above"),
                };
                let help = match command {
                    Command::Render => String::new(),
                    c => format!(" {}", c.word()),
                };
                return Err(Error::invalid_option(
                    format!("Unknown option {option}."),
                    format!("Run diagram-render{help} --help for the options."),
                ));
            }
        }
    }
    Ok(Parsed::Run(args))
}

/// Every long option, with an example of its value for hints.
const OPTIONS: &[(&str, &str)] = &[
    ("--output", "-o flow.png"),
    ("--to", "--to png"),
    ("--format", "--format mermaid"),
    ("--scale", "--scale 2"),
    ("--background", "--background white"),
    ("--page", "--page 2"),
    ("--allow-network", ""),
    ("--timeout", "--timeout 120"),
    ("--json", ""),
    ("--debug", ""),
    ("--help", ""),
    ("--version", ""),
];

/// `Try --scale 2.` for an option that takes a value.
fn try_hint(option: &str) -> String {
    let example = OPTIONS
        .iter()
        .find(|(o, _)| *o == option)
        .map_or("", |o| o.1);
    format!("Try {example}.")
}

impl From<lexopt::Error> for Error {
    fn from(e: lexopt::Error) -> Self {
        lexopt_error(e)
    }
}

fn lexopt_error(e: lexopt::Error) -> Error {
    match e {
        lexopt::Error::MissingValue {
            option: Some(option),
        } => {
            let option = if option == "-o" {
                "--output".into()
            } else {
                option
            };
            Error::invalid_option(format!("{option} needs a value."), try_hint(&option))
        }
        lexopt::Error::UnexpectedValue { option, .. } => Error::invalid_option(
            format!("{option} takes no value."),
            format!("Write {option} on its own."),
        ),
        lexopt::Error::NonUnicodeValue(_) => Error::invalid_option(
            "An option's value isn't valid text.",
            "Use plain text for option values.",
        ),
        e => Error::invalid_option(
            format!("{e}."),
            "Run diagram-render --help for the options.",
        ),
    }
}

/// The value of `option` as text.
fn string(p: &mut lexopt::Parser, option: &str) -> Result<String, Error> {
    p.value()?
        .into_string()
        .map_err(|_| Error::invalid_option(format!("{option} isn't valid text."), try_hint(option)))
}

fn to(v: &str) -> Result<To, Error> {
    To::parse(v).ok_or_else(|| Error::invalid_option("--to must be svg or png.", try_hint("--to")))
}

fn format(v: &str) -> Result<Option<Format>, Error> {
    Format::parse(v).ok_or_else(|| {
        Error::invalid_option(
            "--format must be auto, mermaid, dot, d2, bpmn, drawio or excalidraw.",
            try_hint("--format"),
        )
    })
}

/// `--scale`: 0.1 to 8 with at most 2 decimals.
fn scale(v: &str) -> Result<f64, Error> {
    let (whole, decimals) = v.split_once('.').unwrap_or((v, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !digits(decimals) || whole.len() + decimals.len() == 0 {
        return Err(Error::invalid_option(
            "--scale must be between 0.1 and 8.",
            try_hint("--scale"),
        ));
    }
    if decimals.len() > 2 {
        return Err(Error::invalid_option(
            "--scale takes at most 2 decimals.",
            "Try --scale 1.25.",
        ));
    }
    match v.parse::<f64>() {
        Ok(s) if (0.1..=8.0).contains(&s) => Ok(s),
        _ => Err(Error::invalid_option(
            "--scale must be between 0.1 and 8.",
            try_hint("--scale"),
        )),
    }
}

fn background(v: &str) -> Result<(), Error> {
    let hex = v
        .strip_prefix('#')
        .is_some_and(|h| matches!(h.len(), 3 | 6) && h.bytes().all(|b| b.is_ascii_hexdigit()));
    match hex || ["auto", "transparent", "white"].contains(&v.to_ascii_lowercase().as_str()) {
        true => Ok(()),
        false => Err(Error::invalid_option(
            "--background must be auto, transparent, white, #RGB or #RRGGBB.",
            try_hint("--background"),
        )),
    }
}

fn page(v: String) -> Result<String, Error> {
    match v.trim().is_empty() {
        true => Err(Error::invalid_option(
            "--page needs a page number or name.",
            try_hint("--page"),
        )),
        false => Ok(v),
    }
}

/// `--timeout`: whole seconds from 1 to 600.
fn timeout(v: &str) -> Result<u64, Error> {
    match v.parse::<u64>() {
        Ok(s) if v.bytes().all(|b| b.is_ascii_digit()) && (1..=600).contains(&s) => Ok(s),
        _ => Err(Error::invalid_option(
            "--timeout must be a whole number of seconds from 1 to 600.",
            try_hint("--timeout"),
        )),
    }
}

fn run(command: Command, args: Args) -> u8 {
    let Some(arg) = args.input else {
        return usage(&Error::no_input(), args.json);
    };
    let name = arg.to_string_lossy().into_owned();
    let path = (arg != "-").then(|| Path::new(&arg));
    let shown = if path.is_some() {
        name.as_str()
    } else {
        "stdin"
    };
    let mut warnings = vec![];
    let mut target = None;
    if command == Command::Render {
        let (output, to) = match output::target(&arg, args.output.as_deref(), args.to) {
            Ok(target) => target,
            Err(e) => return usage(&e, args.json),
        };
        if args.json && output == Target::Stdout {
            return usage(&Error::json_needs_output(), args.json);
        }
        if args.scale.is_some() && to == To::Svg {
            warnings.push(Warning::scale_ignored());
        }
        target = Some(output);
    }
    let mut known = Known {
        format: args.format,
        ..Known::default()
    };
    let read = input::read(&arg);
    let diagram = match read.and_then(|bytes| detect::detect(path, shown, &bytes, args.format)) {
        Ok(diagram) => diagram,
        Err(e) => return failed(command, &name, &known, &e, args.json),
    };
    if args.page.is_some() && !diagram.format.has_pages() {
        return usage(&Error::page_not_supported(diagram.format.name()), args.json);
    }
    // With --page the name holds the page's, so it can't be the input's.
    if let (Some(Target::File(out)), Some(path), None) = (&target, path, &args.page)
        && output::is_input(out, path)
    {
        return usage(&Error::output_is_input(&out.to_string_lossy()), args.json);
    }
    known = Known {
        format: Some(diagram.format),
        diagram_type: diagram.diagram_type,
        warnings: [diagram.warnings, warnings].concat(),
    };
    let e = match engine::locate(diagram.format) {
        Some(Err(missing)) => missing,
        _ => Error::internal("no renderer is built in yet"),
    };
    failed(command, &name, &known, &e, args.json)
}

/// `diagram-render formats`: each format, its engine, and whether that engine runs here.
fn formats(args: &Args) -> u8 {
    let probes: Vec<_> = thread::scope(|s| {
        let probes: Vec<_> = (Format::ALL.iter())
            .map(|&f| s.spawn(move || engine::probe(f)))
            .collect();
        probes.into_iter().map(|p| p.join().unwrap()).collect()
    });
    let mut err = io::stderr().lock();
    let mut debug = probes.iter().flat_map(|p| &p.log).filter(|_| args.debug);
    let written = debug.try_for_each(|l| writeln!(err, "debug: {}", text::escape(l)));
    let rows: Vec<_> = Format::ALL.iter().zip(&probes).collect();
    let written = written.and_then(|()| {
        let mut out = io::stdout().lock();
        if !args.json {
            return text::formats(&mut out, Look::stdout(), &rows);
        }
        let rows = rows.iter().map(|(f, probe)| {
            let (name, version) = f.engine();
            let mut row = json!({
                "format": f.id(), "name": f.name(), "extensions": f.extensions(),
                "engine": {"name": name, "version": version},
                "available": probe.problem.is_none(),
                "problem": probe.problem.as_ref().map(|e| &e.message),
            });
            if **f == Format::Mermaid {
                row["families"] = json!(probe.families);
            }
            row
        });
        writeln!(out, "{}", json!({"formats": rows.collect::<Vec<_>>()}))
    });
    finish(written, 0)
}

/// What a run learned about its input before it stopped.
#[derive(Default)]
struct Known {
    format: Option<Format>,
    diagram_type: Option<&'static str>,
    warnings: Vec<Warning>,
}

/// Reports a failed render or check: the warnings and the error on stderr, or one JSON object.
fn failed(command: Command, input: &str, known: &Known, e: &Error, json: bool) -> u8 {
    let written = if json {
        let warnings: Vec<_> = known.warnings.iter().map(Warning::json).collect();
        let engine = known.format.map(|f| {
            let (name, version) = f.engine();
            json!({"name": name, "version": version})
        });
        let format = known.format.map(Format::id);
        let result = match command {
            Command::Render | Command::Formats => json!({
                "input": input, "format": format, "output": null, "error": e.json(),
                "warnings": warnings, "engine": engine,
            }),
            Command::Check => json!({
                "input": input, "format": format, "diagram_type": known.diagram_type,
                "valid": false, "errors": [e.check_json()], "warnings": warnings, "engine": engine,
            }),
        };
        writeln!(io::stdout(), "{result}")
    } else {
        let (mut err, look) = (io::stderr().lock(), Look::stderr());
        (known.warnings.iter())
            .try_for_each(|w| text::warning(&mut err, look, input, w))
            .and_then(|()| text::error(&mut err, look, Some(input), e))
    };
    finish(written, e.exit_code())
}

/// Reports a usage error, as JSON on stdout or as two lines on stderr.
fn usage(e: &Error, json: bool) -> u8 {
    let written = match json {
        true => writeln!(io::stdout(), "{}", json!({"error": e.json()})),
        false => text::error(&mut io::stderr(), Look::stderr(), None, e),
    };
    finish(written, e.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_take_two_decimals_between_a_tenth_and_eight() {
        assert_eq!(scale("2").unwrap(), 2.0);
        assert_eq!(scale("0.1").unwrap(), 0.1);
        assert_eq!(scale("8.00").unwrap(), 8.0);
        assert_eq!(scale(".5").unwrap(), 0.5);
        for bad in ["0.09", "8.01", "", ".", "-1", "1e1", "inf", "2,5", " 2"] {
            assert_eq!(
                scale(bad).unwrap_err().message,
                "--scale must be between 0.1 and 8.",
                "{bad}"
            );
        }
        assert_eq!(
            scale("1.125").unwrap_err().message,
            "--scale takes at most 2 decimals."
        );
    }

    #[test]
    fn timeouts_are_whole_seconds_up_to_ten_minutes() {
        assert_eq!(timeout("1").unwrap(), 1);
        assert_eq!(timeout("600").unwrap(), 600);
        for bad in ["0", "601", "1.5", "+5", "", "-1"] {
            assert!(timeout(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn backgrounds_are_named_or_hex() {
        for good in ["auto", "Transparent", "white", "#fff", "#336699", "#ABCDEF"] {
            assert!(background(good).is_ok(), "{good}");
        }
        for bad in ["black", "#ff", "#12345", "#ggg", "336699", ""] {
            assert!(background(bad).is_err(), "{bad}");
        }
    }
}
