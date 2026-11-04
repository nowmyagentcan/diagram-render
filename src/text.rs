//! Text output for people: colour only on a terminal without `NO_COLOR`, ASCII marks on a
//! Windows console whose output code page isn't UTF-8, and control characters shown as
//! `<U+XXXX>` so a file name or a message can't rewrite the terminal.

use std::env;
use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::detect::Format;
use crate::engine::{Page, Probe};
use crate::error::{Error, Warning};
use crate::output::To;
use crate::run::Stopped;

pub const DIM: &str = "2";
pub const BOLD: &str = "1";
const RED: &str = "31";
const YELLOW: &str = "33";
const GREEN: &str = "32";

/// How long a render runs before the spinner shows, and how long each frame stays.
const SPIN_AFTER: Duration = Duration::from_millis(150);
const FRAME: Duration = Duration::from_millis(80);

/// How one stream shows text.
#[derive(Debug, Clone, Copy, Default)]
pub struct Look {
    pub colour: bool,
    pub ascii: bool,
}

impl Look {
    pub fn stdout() -> Self {
        Self::new(io::stdout().is_terminal(), console::STDOUT)
    }

    pub fn stderr() -> Self {
        Self::new(io::stderr().is_terminal(), console::STDERR)
    }

    fn new(terminal: bool, stream: console::Stream) -> Self {
        let ansi = terminal && console::enable_ansi(stream);
        Self {
            colour: ansi && env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
            ascii: terminal && !console::utf8(),
        }
    }

    /// `s` in the SGR style `sgr` when colour is on.
    pub fn paint(self, sgr: &str, s: &str) -> String {
        if self.colour {
            format!("\x1b[{sgr}m{s}\x1b[0m")
        } else {
            s.into()
        }
    }

    /// `unicode`, or `ascii` where the console can't show it.
    pub fn pick(self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.ascii { ascii } else { unicode }
    }
}

#[cfg(windows)]
mod console {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleOutputCP,
        GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    pub type Stream = STD_HANDLE;
    pub const STDOUT: Stream = STD_OUTPUT_HANDLE;
    pub const STDERR: Stream = STD_ERROR_HANDLE;

    /// Turns on ANSI sequences for the console behind `stream`.
    pub fn enable_ansi(stream: Stream) -> bool {
        let mut mode = CONSOLE_MODE::default();
        // SAFETY: plain console calls on a standard handle; `mode` outlives the call.
        unsafe {
            GetStdHandle(stream).is_ok_and(|handle| {
                GetConsoleMode(handle, &mut mode).is_ok()
                    && SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING).is_ok()
            })
        }
    }

    pub fn utf8() -> bool {
        // SAFETY: no arguments; returns 0 without a console.
        unsafe { GetConsoleOutputCP() == 65001 }
    }
}

#[cfg(not(windows))]
mod console {
    pub type Stream = ();
    pub const STDOUT: Stream = ();
    pub const STDERR: Stream = ();

    pub fn enable_ansi(_: Stream) -> bool {
        true
    }

    pub fn utf8() -> bool {
        true
    }
}

/// `s` safe for a terminal: C0 controls but tab, DEL, C1 controls and the bidi controls that
/// can disguise text, as `<U+202E>` and the like.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => out.push(c),
            '\0'..='\x1F'
            | '\x7F'..='\u{9F}'
            | '\u{61C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => out += &format!("<U+{:04X}>", c as u32),
            c => out.push(c),
        }
    }
    out
}

/// `✗ [input[:line[:column]]: ]message`, the source line with a caret under the column when the
/// error has a place, then the hint.
pub fn error(w: &mut impl Write, look: Look, input: Option<&str>, e: &Error) -> io::Result<()> {
    let mark = look.paint(RED, look.pick("✗", "x"));
    let place = input.map_or_else(String::new, |i| {
        let line = e.line.map(|l| format!(":{l}")).unwrap_or_default();
        let column = e.line.and(e.column).map(|c| format!(":{c}"));
        format!("{}{line}{}: ", escape(i), column.unwrap_or_default())
    });
    let message = look.paint(BOLD, &escape(&e.message));
    writeln!(w, "{mark} {place}{message}")?;
    if let (Some(line), Some(x)) = (e.line, &e.excerpt) {
        let shown = |c: char| match c {
            '…' if look.ascii => "...".to_string(),
            c => escape(c.encode_utf8(&mut [0; 4])),
        };
        let n = line.to_string();
        let text: String = x.text.chars().map(shown).collect();
        writeln!(w, "    {} {text}", look.paint(DIM, &format!("{n} |")))?;
        if let Some(caret) = x.caret {
            // Tabs stay tabs, so the caret lines up however wide the terminal draws them.
            let pad: String = (x.text.chars().take(caret - 1))
                .map(|c| match c {
                    '\t' => "\t".into(),
                    c => " ".repeat(shown(c).chars().count()),
                })
                .collect();
            let gutter = format!("{:w$} |", "", w = n.len());
            let caret = look.paint(DIM, "^");
            writeln!(w, "    {} {pad}{caret}", look.paint(DIM, &gutter))?;
        }
    }
    writeln!(w, "  {}", look.paint(DIM, &escape(&e.hint)))
}

/// `✗ message` and its hint, for what isn't about any file.
pub fn plain_error(w: &mut impl Write, look: Look, message: &str, hint: &str) -> io::Result<()> {
    let mark = look.paint(RED, look.pick("✗", "x"));
    let message = look.paint(BOLD, &escape(message));
    writeln!(w, "{mark} {message}")?;
    writeln!(w, "  {}", look.paint(DIM, hint))
}

/// `! input[:line]: message`.
pub fn warning(w: &mut impl Write, look: Look, input: &str, warning: &Warning) -> io::Result<()> {
    let mark = look.paint(YELLOW, "!");
    let line = warning.line.map(|l| format!(":{l}")).unwrap_or_default();
    let message = escape(&warning.message);
    writeln!(w, "{mark} {}{line}: {message}", escape(input))
}

/// Runs `work`; when it takes over 150 ms and stderr is a terminal, `⠋ Rendering {file}…` spins on
/// stderr meanwhile and is erased before this returns.
pub fn spin<T>(look: Look, file: &str, work: impl FnOnce() -> T) -> T {
    if !io::stderr().is_terminal() {
        return work();
    }
    let (stop, stopped) = mpsc::channel();
    let line = format!("Rendering {}{}", escape(file), look.pick("…", "..."));
    thread::scope(|s| {
        s.spawn(|| spinner(io::stderr(), look, &line, stopped));
        let result = work();
        drop(stop);
        result
    })
}

/// Draws `line` behind a turning frame until `stopped` ends, then erases it.
fn spinner(mut w: impl Write, look: Look, line: &str, stopped: Receiver<()>) {
    let frames: &[&str] = if look.ascii {
        &["-", "\\", "|", "/"]
    } else {
        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
    };
    let mut drawn = false;
    for (i, frame) in frames.iter().cycle().enumerate() {
        let wait = if i == 0 { SPIN_AFTER } else { FRAME };
        if stopped.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
            break;
        }
        let text = look.paint(DIM, &format!("  {frame} {line}"));
        if write!(w, "\r{text}").and_then(|()| w.flush()).is_err() {
            return;
        }
        drawn = true;
    }
    if drawn {
        let _ = write!(w, "\r\x1b[K");
    }
}

/// The warnings of a stopped run, then its error.
pub fn stopped(w: &mut impl Write, look: Look, input: &str, s: &Stopped) -> io::Result<()> {
    (s.known.warnings.iter()).try_for_each(|x| warning(w, look, input, x))?;
    error(w, look, Some(input), &s.error)
}

/// What a render made.
pub struct Made {
    pub input: String,
    pub output: String,
    pub to: To,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    /// PNG pixels per CSS pixel; `None` for SVG.
    pub scale: Option<f64>,
    pub took: Duration,
    pub page: Option<Page>,
    pub pages: Vec<Page>,
}

/// `✓ flow.mmd → flow.svg  1180×640  18 KB  0.21 s`.
pub fn rendered(w: &mut impl Write, look: Look, m: &Made) -> io::Result<()> {
    let mark = look.paint(GREEN, look.pick("✓", "ok"));
    let arrow = look.pick("→", "->");
    let output = look.paint(BOLD, &escape(&m.output));
    let times = look.pick("×", "x");
    let (input, w_, h) = (escape(&m.input), m.width, m.height);
    let (size, secs) = (size(m.bytes), seconds(m.took));
    writeln!(
        w,
        "{mark} {input} {arrow} {output}  {w_}{times}{h}  {size}  {secs} s"
    )
}

/// `✓ flow.mmd is valid Mermaid (flowchart)`.
pub fn valid(
    w: &mut impl Write,
    look: Look,
    input: &str,
    name: &str,
    kind: Option<&str>,
) -> io::Result<()> {
    let mark = look.paint(GREEN, look.pick("✓", "ok"));
    let kind = kind.map(|k| format!(" ({k})")).unwrap_or_default();
    writeln!(w, "{mark} {} is valid {name}{kind}", escape(input))
}

/// `2 errors · 1 warning`.
pub fn counts(w: &mut impl Write, look: Look, errors: usize, warnings: usize) -> io::Result<()> {
    let plural = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    let line = format!(
        "{} {} {}",
        plural(errors, "error"),
        look.pick("·", "-"),
        plural(warnings, "warning")
    );
    writeln!(w, "{}", look.paint(DIM, &line))
}

/// `812 B`, `18 KB` (whole KB under 1 MiB), `2.4 MB`.
pub fn size(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1 << 20 => format!("{} KB", (b as f64 / 1024.0).round()),
        b => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
    }
}

/// `0.21` under 10 s, `12.3` from 10 s.
pub fn seconds(took: Duration) -> String {
    match took.as_secs_f64() {
        s if s < 10.0 => format!("{s:.2}"),
        s => format!("{s:.1}"),
    }
}

/// One line per format: its name, engine, extensions, and `ok` or why its engine doesn't run.
pub fn formats(w: &mut impl Write, look: Look, rows: &[(&Format, &Probe)]) -> io::Result<()> {
    let cells: Vec<_> = (rows.iter())
        .map(|(f, _)| {
            let (engine, version) = f.engine();
            (
                f.id(),
                format!("{engine} {version}"),
                f.extensions().join(" "),
            )
        })
        .collect();
    let a = cells.iter().map(|c| c.0.len()).max().unwrap_or(0) + 2;
    let b = cells.iter().map(|c| c.1.len()).max().unwrap_or(0) + 2;
    let c = cells.iter().map(|c| c.2.len()).max().unwrap_or(0) + 2;
    for ((id, engine, extensions), (_, probe)) in cells.iter().zip(rows) {
        let status = match &probe.problem {
            None => look.paint(GREEN, "ok"),
            Some(e) => look.paint(RED, &escape(&e.message)),
        };
        writeln!(w, "{id:a$}{engine:b$}{extensions:c$}{status}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLOUR: Look = Look {
        colour: true,
        ascii: false,
    };
    const ASCII: Look = Look {
        colour: false,
        ascii: true,
    };

    fn printed(look: Look, input: Option<&str>, e: &Error) -> String {
        let mut w = vec![];
        error(&mut w, look, input, e).unwrap();
        String::from_utf8(w).unwrap()
    }

    #[test]
    fn errors_read_as_two_lines() {
        let e = Error::no_input();
        assert_eq!(
            printed(Look::default(), None, &e),
            "✗ No diagram given.\n  Try: diagram-render flow.mmd, or diagram-render --help.\n"
        );
        assert_eq!(
            printed(COLOUR, Some("a.mmd"), &e),
            "\x1b[31m✗\x1b[0m a.mmd: \x1b[1mNo diagram given.\x1b[0m\n  \x1b[2mTry: diagram-render flow.mmd, or diagram-render --help.\x1b[0m\n"
        );
        assert!(printed(ASCII, None, &e).starts_with("x No diagram given.\n"));
    }

    #[test]
    fn placed_errors_show_the_line_and_a_caret() {
        let lines = crate::input::Lines::new("graph TD\n  A --> B\n\tA --> B --\n");
        let e = lines.place(Error::not_utf8(3), 3, Some(10));
        assert_eq!(
            printed(Look::default(), Some("flow.mmd"), &e),
            "✗ flow.mmd:3:10: Line 3 isn't valid UTF-8 text.\n    3 | \tA --> B --\n      | \t        ^\n  Save the file as UTF-8.\n"
        );
        let e = lines.place(Error::not_utf8(3), 3, None);
        assert_eq!(
            printed(Look::default(), Some("flow.mmd"), &e),
            "✗ flow.mmd:3: Line 3 isn't valid UTF-8 text.\n    3 | \tA --> B --\n  Save the file as UTF-8.\n"
        );
        let long = format!("{}\u{1b}x", "a".repeat(300));
        let e = crate::input::Lines::new(&long).place(Error::not_utf8(1), 1, Some(302));
        let ascii = printed(ASCII, Some("a"), &e);
        let rows: Vec<_> = ascii.lines().collect();
        assert!(rows[1].starts_with("    1 | ...aaa"), "{}", rows[1]);
        assert!(rows[1].ends_with("a<U+001B>x"), "{}", rows[1]);
        assert_eq!(rows[2].find('^'), rows[1].rfind('x'));
    }

    #[test]
    fn warnings_name_the_input_and_their_line() {
        let mut warning = Warning::format_guessed();
        let mut w = vec![];
        super::warning(&mut w, COLOUR, "-", &warning).unwrap();
        warning.line = Some(4);
        super::warning(&mut w, Look::default(), "a.d2", &warning).unwrap();
        let message = "Read as D2 (no Mermaid or DOT header found); pass --format to choose.";
        let want = format!("\x1b[33m!\x1b[0m -: {message}\n! a.d2:4: {message}\n");
        assert_eq!(String::from_utf8(w).unwrap(), want);
    }

    #[test]
    fn control_and_bidi_characters_are_shown_as_code_points() {
        assert_eq!(
            escape("a\tb\x1b[2J\u{202E}c\u{9B}é漢"),
            "a\tb<U+001B>[2J<U+202E>c<U+009B>é漢"
        );
    }
}
