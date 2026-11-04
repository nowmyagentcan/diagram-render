//! The command line: `diagram-render [render] <INPUT> [OPTIONS]` and `diagram-render check <INPUT>`.

mod bpmn;
mod d2;
mod detect;
mod dot;
mod drawio;
mod engine;
mod error;
mod excalidraw;
mod input;
mod mcp;
mod mermaid;
mod output;
mod raster;
mod run;
mod svg;
mod text;
mod ui;

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::Instant;

use detect::Format;
use error::Error;
use lexopt::Arg::{Long, Short, Value};
use output::{Target, To};
use run::{Choices, Known, Opened, Source, Stopped};
use serde_json::json;
use text::{Look, Made};

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

const UI_HELP: &str = "\
Open the local web page.

Usage: diagram-render ui [OPTIONS]

Options:
      --port <N>         port on 127.0.0.1, 1-65535 [default: a free one]
      --no-open          don't open the browser
      --debug            show engine command lines and full output
  -h, --help             print help
";

const MCP_HELP: &str =
    "Run the MCP server on stdio, with the tools render_diagram and check_diagram.

Usage: diagram-render mcp [OPTIONS]

Options:
      --debug            show engine command lines and full output
  -h, --help             print help
";

#[derive(Clone, Copy, PartialEq)]
enum Command {
    Render,
    Check,
    Formats,
    Ui,
    Mcp,
}

impl Command {
    fn word(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::Check => "check",
            Self::Formats => "formats",
            Self::Ui => "ui",
            Self::Mcp => "mcp",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::Render => HELP,
            Self::Check => CHECK_HELP,
            Self::Formats => FORMATS_HELP,
            Self::Ui => UI_HELP,
            Self::Mcp => MCP_HELP,
        }
    }

    /// Whether `option` belongs to this command.
    fn takes(self, option: &str) -> bool {
        let common = ["--json", "--debug", "--help"];
        let check = ["--format", "--page", "--timeout"];
        let ui = ["--port", "--no-open"];
        match self {
            Self::Render => !ui.contains(&option),
            Self::Check => common.contains(&option) || check.contains(&option),
            Self::Formats => common.contains(&option),
            Self::Ui => ui.contains(&option) || ["--debug", "--help"].contains(&option),
            Self::Mcp => ["--debug", "--help"].contains(&option),
        }
    }
}

#[derive(Default)]
struct Args {
    input: Option<OsString>,
    output: Option<OsString>,
    to: Option<To>,
    choices: Choices,
    port: u16,
    no_open: bool,
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
        Some("ui") => (Command::Ui, &args[1..]),
        Some("mcp") => (Command::Mcp, &args[1..]),
        _ => (Command::Render, &args[..]),
    };
    let code = match parse(command, rest) {
        Ok(Parsed::Print(text)) => finish(io::stdout().write_all(text.as_bytes()), 0),
        Ok(Parsed::Run(args)) if command == Command::Formats => formats(&args),
        Ok(Parsed::Run(args)) if command == Command::Ui => {
            let stop = "  (Ctrl+C to stop)";
            served(ui::serve(args.port, !args.no_open, stop, args.debug))
        }
        Ok(Parsed::Run(args)) if command == Command::Mcp => served(mcp::serve(args.debug)),
        Ok(Parsed::Run(args)) => run(command, args),
        Err(e) => usage(&e, json),
    };
    ExitCode::from(code)
}

/// The exit code of a server that ended; what stopped it, on stderr.
fn served(result: Result<(), Error>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(e) => usage(&e, false),
    }
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
            ("--allow-network", _) => args.choices.allow_network = true,
            ("--no-open", _) => args.no_open = true,
            ("--port", _) => args.port = port(&string(&mut p, name)?)?,
            ("--output", _) => args.output = Some(p.value()?),
            ("--to", _) => args.to = Some(to(&string(&mut p, name)?)?),
            ("--format", _) => args.choices.format = format(&string(&mut p, name)?)?,
            ("--scale", _) => args.choices.scale = Some(scale(&string(&mut p, name)?)?),
            ("--background", _) => args.choices.background = background(&string(&mut p, name)?)?,
            ("--page", _) => args.choices.page = Some(page(string(&mut p, name)?)?),
            ("--timeout", _) => args.choices.timeout = Some(timeout(&string(&mut p, name)?)?),
            (_, Value(_)) if ![Command::Render, Command::Check].contains(&command) => {
                let word = command.word();
                return Err(Error::invalid_option(
                    format!("{word} takes no file."),
                    format!("Run diagram-render {word} on its own."),
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
    ("--port", "--port 8080"),
    ("--no-open", ""),
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
            e => Error::invalid_option(
                format!("{e}."),
                "Run diagram-render --help for the options.",
            ),
        }
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

/// `--background`: `None` for auto, else `white`, `transparent` or `#rrggbb`.
fn background(v: &str) -> Result<Option<String>, Error> {
    let v = v.to_ascii_lowercase();
    let hex = v
        .strip_prefix('#')
        .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
    match (v.as_str(), hex.map(str::len)) {
        ("auto", _) => Ok(None),
        ("transparent" | "white", _) | (_, Some(6)) => Ok(Some(v)),
        (_, Some(3)) => Ok(Some(
            hex.unwrap()
                .chars()
                .fold("#".into(), |s, c| format!("{s}{c}{c}")),
        )),
        _ => Err(Error::invalid_option(
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

/// `--port`: 1 to 65535.
fn port(v: &str) -> Result<u16, Error> {
    match v.parse::<u16>() {
        Ok(p) if p > 0 && v.bytes().all(|b| b.is_ascii_digit()) => Ok(p),
        _ => Err(Error::invalid_option(
            "--port must be between 1 and 65535.",
            try_hint("--port"),
        )),
    }
}

fn run(command: Command, args: Args) -> u8 {
    let Some(arg) = &args.input else {
        return usage(&Error::no_input(), args.json);
    };
    let name = arg.to_string_lossy();
    match command {
        Command::Check => match check(arg, &args) {
            Ok((known, errors)) => checks(&name, &known, &errors, args.json),
            Err(stopped) => failed(command, &name, &stopped, args.json),
        },
        _ => match render(arg, &args) {
            Ok(done) => renders(&done, args.json),
            Err(stopped) => failed(command, &name, &stopped, args.json),
        },
    }
}

/// What a render left behind.
pub struct Done {
    pub known: Known,
    pub made: Made,
    /// Whether the image went to stdout, which leaves stderr for the result line.
    pub on_stdout: bool,
}

/// Where a command-line input comes from.
fn source<'a>(arg: &'a OsStr, name: &'a str) -> Source<'a> {
    let path = (arg != "-").then(|| Path::new(arg));
    let shown = if path.is_some() { name } else { "stdin" };
    Source { name, shown, path }
}

/// Prints the engine's command lines for `--debug`; a closed stderr is no reason to stop.
fn debug(log: &[String]) {
    let mut err = io::stderr().lock();
    let written = log
        .iter()
        .try_for_each(|l| writeln!(err, "debug: {}", text::escape(l)));
    let _ = written.and_then(|()| err.flush());
}

/// Renders `arg` to where `args` says.
fn render(arg: &OsStr, args: &Args) -> Result<Done, Stopped> {
    let start = Instant::now();
    let name = arg.to_string_lossy();
    let asked = args.choices.page.as_deref();
    let (target, to) = output::target(arg, args.output.as_deref(), args.to, asked)?;
    if args.json && target == Target::Stdout {
        return Err(Error::json_needs_output().into());
    }
    let from = source(arg, &name);
    let path = from.path;
    let mut opened = run::open(from, input::read(arg), &args.choices)?;
    if let (Target::File(out), Some(path)) = (&target, path)
        && output::is_input(out, path)
    {
        return Err(Error::output_is_input(&out.to_string_lossy()).into());
    }
    let made = draw(&mut opened, arg, args, target, to, start);
    if args.debug {
        debug(opened.log());
    }
    let known = std::mem::take(&mut opened.known);
    match made {
        Ok((made, on_stdout)) => Ok(Done {
            known,
            made,
            on_stdout,
        }),
        Err(error) => Err(Stopped::new(error, known)),
    }
}

fn draw(
    opened: &mut Opened,
    arg: &OsStr,
    args: &Args,
    target: Target,
    to: To,
    start: Instant,
) -> Result<(Made, bool), Error> {
    let drawn = opened.draw(to)?;
    // Beside the input, the output is named after the page found, not as --page wrote it.
    let target = match (&drawn.page, &args.choices.page, &args.output, arg != "-") {
        (Some(page), Some(_), None, true) => {
            output::target(arg, None, Some(to), Some(&page.name))?.0
        }
        _ => target,
    };
    let picture = opened.picture(drawn.svg, to)?;
    output::write(&target, &picture.bytes)?;
    let made = Made {
        input: arg.to_string_lossy().into_owned(),
        output: match &target {
            Target::Stdout => "-".to_string(),
            Target::File(p) => p.to_string_lossy().into_owned(),
        },
        to,
        width: picture.width,
        height: picture.height,
        bytes: picture.bytes.len(),
        scale: picture.scale,
        took: start.elapsed(),
        page: drawn.page,
        pages: drawn.pages,
    };
    Ok((made, target == Target::Stdout))
}

/// Checks `arg`: what it found out, and every error.
fn check(arg: &OsStr, args: &Args) -> Result<(Known, Vec<Error>), Stopped> {
    let name = arg.to_string_lossy();
    let mut opened = run::open(source(arg, &name), input::read(arg), &args.choices)?;
    let errors = opened.check();
    if args.debug {
        debug(opened.log());
    }
    let known = std::mem::take(&mut opened.known);
    match errors {
        Ok(errors) => Ok((known, errors)),
        Err(error) => Err(Stopped::new(error, known)),
    }
}

/// Reports a render that wrote its image: the result line and the warnings, or one JSON object.
fn renders(done: &Done, json: bool) -> u8 {
    let written = if json {
        let result = run::render_json(&done.known, &done.made);
        writeln!(io::stdout(), "{result}")
    } else {
        let line = match done.on_stdout {
            true => text::rendered(&mut io::stderr(), Look::stderr(), &done.made),
            false => text::rendered(&mut io::stdout(), Look::stdout(), &done.made),
        };
        let (mut err, look) = (io::stderr().lock(), Look::stderr());
        let input = &done.made.input;
        line.and_then(|()| {
            (done.known.warnings.iter()).try_for_each(|w| text::warning(&mut err, look, input, w))
        })
    };
    finish(written, 0)
}

/// Reports a check: valid, or every error, then the warnings and the counts; or one JSON object.
fn checks(input: &str, known: &Known, errors: &[Error], json: bool) -> u8 {
    let format = known.format.expect("a checked input has a format");
    let written = if json {
        let errors: Vec<_> = errors.iter().map(Error::check_json).collect();
        writeln!(io::stdout(), "{}", run::check_json(input, known, &errors))
    } else {
        let (mut err, look) = (io::stderr().lock(), Look::stderr());
        let first = match errors.is_empty() {
            true => text::valid(
                &mut io::stdout(),
                Look::stdout(),
                input,
                format.name(),
                known.diagram_type,
            ),
            false => errors
                .iter()
                .try_for_each(|e| text::error(&mut err, look, Some(input), e)),
        };
        first
            .and_then(|()| {
                (known.warnings.iter()).try_for_each(|w| text::warning(&mut err, look, input, w))
            })
            .and_then(|()| match errors.is_empty() {
                true => Ok(()),
                false => text::counts(&mut err, look, errors.len(), known.warnings.len()),
            })
    };
    finish(written, if errors.is_empty() { 0 } else { 1 })
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

/// Reports a failed render or check: the warnings and the error on stderr, or one JSON object.
fn failed(command: Command, input: &str, stopped: &Stopped, json: bool) -> u8 {
    let Stopped { error: e, known } = stopped;
    if e.is_usage() {
        return usage(e, json);
    }
    let written = if json {
        let result = match command {
            Command::Check => run::check_json(input, known, &[e.check_json()]),
            _ => stopped.json(input),
        };
        writeln!(io::stdout(), "{result}")
    } else {
        text::stopped(&mut io::stderr().lock(), Look::stderr(), input, stopped)
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
