//! Interactive mode: `diagram-render` with no arguments in a terminal. Each line names a file and
//! optionally the image type; the answer is what a one-shot run prints, indented under the prompt.

use std::io::{self, BufRead, Write};
use std::mem;
use std::path::Path;

use crate::output::To;
use crate::run::Stopped;
use crate::text::{self, BOLD, DIM, Look};
use crate::{Done, SENTENCE};

const HELP: &str = "\
Type a diagram file path, then optionally svg or png:
  flow.mmd                  writes flow.svg beside it
  \"my board.drawio\" png     writes my board.png
Files can be Mermaid (.mmd), Graphviz (.dot, .gv), D2 (.d2), BPMN (.bpmn),
draw.io (.drawio, .drawio.svg, .drawio.png) or Excalidraw (.excalidraw).
For pages, scale, background and checking, run diagram-render --help.
/quit        leave (Ctrl+C works too)
";

/// A writer that starts every line two spaces in.
struct Indented<W> {
    w: W,
    fresh: bool,
}

impl<W: Write> Indented<W> {
    fn new(w: W) -> Self {
        let fresh = true;
        Self { w, fresh }
    }
}

impl<W: Write> Write for Indented<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for line in buf.split_inclusive(|&b| b == b'\n') {
            if mem::replace(&mut self.fresh, line.ends_with(b"\n")) {
                self.w.write_all(b"  ")?;
            }
            self.w.write_all(line)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// Shows the banner, then hands each line's file to `render` until `/quit` or the end of `input`.
pub fn run(
    mut input: impl BufRead,
    out: &mut impl Write,
    err: &mut impl Write,
    looks: (Look, Look),
    mut render: impl FnMut(&str, To) -> Result<Done, Stopped>,
) -> io::Result<()> {
    let (look, err_look) = looks;
    banner(out, look)?;
    let prompt = look.paint(BOLD, look.pick("›", ">"));
    let mut line = String::new();
    loop {
        write!(out, "{prompt} ")?;
        out.flush()?;
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return writeln!(out);
        }
        let words = split(&line, cfg!(not(windows)));
        let mut answer = Indented::new(&mut *out);
        let mut complaint = Indented::new(&mut *err);
        match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            [] => continue,
            ["/quit"] => return Ok(()),
            ["/help"] => answer.write_all(HELP.as_bytes())?,
            [word] if is_command(word) => {
                let message = format!("Unknown command '{word}'.");
                text::plain_error(&mut complaint, err_look, &message, "Try /help.")?;
            }
            [file] => answered(
                file,
                To::Svg,
                &mut answer,
                &mut complaint,
                looks,
                &mut render,
            )?,
            [file, to] if To::parse(to).is_some() => {
                let to = To::parse(to).unwrap_or(To::Svg);
                answered(file, to, &mut answer, &mut complaint, looks, &mut render)?;
            }
            _ => text::plain_error(
                &mut complaint,
                err_look,
                "Give one file, then svg or png.",
                "For example: flow.mmd png",
            )?,
        }
        writeln!(out)?;
    }
}

/// Renders `file` and prints what a one-shot run prints.
fn answered(
    file: &str,
    to: To,
    out: &mut impl Write,
    err: &mut impl Write,
    (look, err_look): (Look, Look),
    render: &mut impl FnMut(&str, To) -> Result<Done, Stopped>,
) -> io::Result<()> {
    match text::spin(err_look, file, || render(file, to)) {
        Ok(done) => {
            text::rendered(out, look, &done.made)?;
            let mut warnings = done.known.warnings.iter();
            warnings.try_for_each(|w| text::warning(err, err_look, file, w))
        }
        Err(stopped) => text::stopped(err, err_look, file, &stopped),
    }
}

/// A word like `/x` that isn't an existing path.
fn is_command(word: &str) -> bool {
    word.strip_prefix('/')
        .is_some_and(|name| name.chars().all(|c| c.is_ascii_alphabetic()))
        && !Path::new(word).exists()
}

fn banner(out: &mut impl Write, look: Look) -> io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let name = format!("{} diagram-render", look.pick("✻", "*"));
    let rows = [
        (
            format!("{name} {version}"),
            format!("{} {version}", look.paint(BOLD, &name)),
        ),
        (format!("  {SENTENCE}"), format!("  {SENTENCE}")),
    ];
    let width = rows.iter().map(|(plain, _)| plain.chars().count()).max();
    let width = width.unwrap_or_default();
    let [top_left, top_right, bottom_left, bottom_right, across, side] = if look.ascii {
        ["+", "+", "+", "+", "-", "|"]
    } else {
        ["╭", "╮", "╰", "╯", "─", "│"]
    };
    let rule = across.repeat(width + 2);
    let side = look.paint(DIM, side);
    writeln!(
        out,
        "{}",
        look.paint(DIM, &format!("{top_left}{rule}{top_right}"))
    )?;
    for (plain, styled) in rows {
        let pad = " ".repeat(width - plain.chars().count());
        writeln!(out, "{side} {styled}{pad} {side}")?;
    }
    writeln!(
        out,
        "{}",
        look.paint(DIM, &format!("{bottom_left}{rule}{bottom_right}"))
    )?;
    let dot = look.pick("·", "-");
    writeln!(
        out,
        "  Type or drop a diagram file {dot} add png for a PNG {dot} /help {dot} /quit\n"
    )
}

/// The words of a line, split on spaces outside quotes. With `posix` as a POSIX shell reads a
/// dropped file (backslash escapes, single and double quotes), otherwise as Windows quotes
/// paths (double quotes only, backslashes kept).
fn split(line: &str, posix: bool) -> Vec<String> {
    let (mut words, mut word, mut quote, mut started) = (vec![], String::new(), None, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_ascii_whitespace() => {
                if mem::take(&mut started) {
                    words.push(mem::take(&mut word));
                }
                continue;
            }
            (None, '\\') if posix => word.extend(chars.next()),
            (None, '"' | '\'') if posix || c == '"' => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') if posix => match chars.next() {
                Some(escaped @ ('"' | '\\' | '$' | '`')) => word.push(escaped),
                other => {
                    word.push('\\');
                    word.extend(other);
                }
            },
            _ => word.push(c),
        }
        started = true;
    }
    if started {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::error::{Error, Warning};
    use crate::run::Known;
    use crate::text::Made;

    fn done(file: &str, to: To) -> Result<Done, Stopped> {
        let made = Made {
            input: file.into(),
            output: format!("flow.{}", to.ext()),
            to,
            width: 10,
            height: 20,
            bytes: 2048,
            scale: None,
            took: Duration::from_millis(210),
            page: None,
            pages: vec![],
        };
        let known = Known {
            warnings: vec![Warning::format_guessed()],
            ..Known::default()
        };
        Ok(Done {
            known,
            made,
            on_stdout: false,
        })
    }

    /// What a session of `lines` prints on stdout and stderr, and the files rendered.
    fn session(lines: &str, look: Look) -> (String, String, Vec<String>) {
        let (mut out, mut err, mut asked) = (vec![], vec![], vec![]);
        let render = |file: &str, to: To| {
            asked.push(format!("{file} {}", to.ext()));
            match file {
                "missing.mmd" => Err(Error::file_not_found(file).into()),
                _ => done(file, to),
            }
        };
        run(lines.as_bytes(), &mut out, &mut err, (look, look), render).unwrap();
        let text = |b| String::from_utf8(b).unwrap();
        (text(out), text(err), asked)
    }

    #[test]
    fn the_banner_is_a_box_as_wide_as_the_sentence() {
        let (out, ..) = session("", Look::default());
        let want = [
            "╭───────────────────────────────────────────────────────────────────╮",
            "│ ✻ diagram-render 0.1.0                                            │",
            "│   Render Mermaid, DOT, D2, BPMN, draw.io and Excalidraw diagrams. │",
            "╰───────────────────────────────────────────────────────────────────╯\n",
        ]
        .join("\n");
        assert!(out.starts_with(&want), "{out}");
        assert!(
            out.contains("  Type or drop a diagram file · add png for a PNG · /help · /quit\n\n› ")
        );
    }

    #[test]
    fn plain_ascii_replaces_the_box_and_the_prompt() {
        let look = Look {
            ascii: true,
            ..Look::default()
        };
        let (out, ..) = session("", look);
        assert!(out.starts_with("+---"), "{out}");
        assert!(out.contains("| * diagram-render "));
        assert!(out.contains("file - add png for a PNG - /help - /quit\n\n> "));
    }

    #[test]
    fn each_line_renders_a_file_and_answers_indented() {
        let (out, err, asked) = session("flow.mmd\n\"a b.dot\" PNG\n", Look::default());
        assert_eq!(asked, ["flow.mmd svg", "a b.dot png"]);
        assert!(
            out.contains("›   ✓ flow.mmd → flow.svg  10×20  2 KB  0.21 s\n\n"),
            "{out}"
        );
        assert!(out.contains("  ✓ a b.dot → flow.png"), "{out}");
        assert!(err.starts_with("  ! flow.mmd: Read as D2"), "{err}");
    }

    #[test]
    fn errors_print_as_one_shot_errors_and_the_session_goes_on() {
        let (out, err, asked) = session("missing.mmd\nflow.mmd\n", Look::default());
        assert_eq!(asked.len(), 2);
        assert!(err.starts_with("  ✗ missing.mmd: Can't find missing.mmd.\n    Check the path"));
        assert!(out.contains("✓ flow.mmd"));
    }

    #[test]
    fn commands_and_bad_lines_answer_without_rendering() {
        let (out, err, asked) = session(
            "/help\n/nope\na.mmd svg png\na.mmd b.mmd\n\n/quit\nnever.mmd\n",
            Look::default(),
        );
        assert!(asked.is_empty());
        assert!(out.contains("  Type a diagram file path, then optionally svg or png:\n"));
        assert!(out.contains("    \"my board.drawio\" png     writes my board.png\n"));
        assert!(out.contains("  /quit        leave (Ctrl+C works too)\n"));
        assert_eq!(
            err.matches("✗ Give one file, then svg or png.\n    For example: flow.mmd png\n")
                .count(),
            2
        );
        assert!(err.contains("  ✗ Unknown command '/nope'.\n    Try /help.\n"));
    }

    #[test]
    fn posix_lines_split_like_a_shell() {
        let split = |line| split(line, true);
        assert_eq!(split("/tmp/a\\ b.mmd  png\n"), ["/tmp/a b.mmd", "png"]);
        assert_eq!(split("'/x y/it''s.dot'"), ["/x y/its.dot"]);
        assert_eq!(split(r#""a \"b\" \\c\d" e"#), [r#"a "b" \c\d"#, "e"]);
        assert_eq!(split(r#"'it'\''s' "#), ["it's"]);
        assert!(split(" \t\n").is_empty());
    }

    #[test]
    fn windows_lines_keep_backslashes_and_quote_with_double_quotes() {
        let split = |line| split(line, false);
        assert_eq!(
            split("\"C:\\my files\\a.mmd\" png\r\n"),
            ["C:\\my files\\a.mmd", "png"]
        );
        assert_eq!(split("C:\\a\\it's.d2"), ["C:\\a\\it's.d2"]);
    }
}
