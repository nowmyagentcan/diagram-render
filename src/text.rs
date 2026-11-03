//! Text output for people: colour only on a terminal without `NO_COLOR`, ASCII marks on a
//! Windows console whose output code page isn't UTF-8, and control characters shown as
//! `<U+XXXX>` so a file name or a message can't rewrite the terminal.

use std::env;
use std::io::{self, IsTerminal, Write};

use crate::error::Error;

const DIM: &str = "2";
const BOLD: &str = "1";
const RED: &str = "31";

/// How one stream shows text.
#[derive(Debug, Clone, Copy, Default)]
pub struct Look {
    pub colour: bool,
    pub ascii: bool,
}

impl Look {
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
    fn paint(self, sgr: &str, s: &str) -> String {
        if self.colour {
            format!("\x1b[{sgr}m{s}\x1b[0m")
        } else {
            s.into()
        }
    }

    /// `unicode`, or `ascii` where the console can't show it.
    fn pick(self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.ascii { ascii } else { unicode }
    }
}

#[cfg(windows)]
mod console {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleOutputCP,
        GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, SetConsoleMode,
    };

    pub type Stream = STD_HANDLE;
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

/// `✗ [input: ]message`, then the hint indented on the next line.
pub fn error(w: &mut impl Write, look: Look, input: Option<&str>, e: &Error) -> io::Result<()> {
    let mark = look.paint(RED, look.pick("✗", "x"));
    let place = input
        .map(|i| format!("{}: ", escape(i)))
        .unwrap_or_default();
    let message = look.paint(BOLD, &escape(&e.message));
    writeln!(w, "{mark} {place}{message}")?;
    writeln!(w, "  {}", look.paint(DIM, &escape(&e.hint)))
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
    fn control_and_bidi_characters_are_shown_as_code_points() {
        assert_eq!(
            escape("a\tb\x1b[2J\u{202E}c\u{9B}é漢"),
            "a\tb<U+001B>[2J<U+202E>c<U+009B>é漢"
        );
    }
}
