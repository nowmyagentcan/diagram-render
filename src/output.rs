//! Where a render goes: `-o FILE`, `-o -` for stdout, stdout for stdin, or beside the input
//! with the diagram's extension replaced.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::detect;
use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum To {
    Svg,
    Png,
}

impl To {
    pub fn parse(v: &str) -> Option<Self> {
        match v.to_ascii_lowercase().as_str() {
            "svg" => Some(Self::Svg),
            "png" => Some(Self::Png),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Stdout,
    File(PathBuf),
}

/// The output for `input` (`-` for stdin) and its format, from `-o` and `--to`.
pub fn target(
    input: &OsStr,
    output: Option<&OsStr>,
    to: Option<To>,
) -> Result<(Target, To), Error> {
    let file = match output {
        Some(o) if o == "-" => return Ok((Target::Stdout, to.unwrap_or(To::Svg))),
        Some(o) => PathBuf::from(o),
        None if input == "-" => return Ok((Target::Stdout, to.unwrap_or(To::Svg))),
        None => {
            let to = to.unwrap_or(To::Svg);
            let input = Path::new(input);
            let mut name = detect::stem(input);
            name.push(format!(".{}", to.ext()));
            return Ok((Target::File(input.with_file_name(name)), to));
        }
    };
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    let by_ext = ext
        .as_deref()
        .and_then(To::parse)
        .ok_or_else(Error::bad_extension)?;
    match to {
        Some(to) if to != by_ext => Err(Error::format_conflict(by_ext.ext(), to.ext())),
        _ => Ok((Target::File(file), by_ext)),
    }
}

/// Writes `bytes` to `target`. A file is written beside itself under a temporary name, then
/// renamed over the old one, so a failed or stopped run leaves the old file whole. A reader
/// of stdout that hung up isn't an error.
pub fn write(target: &Target, bytes: &[u8]) -> Result<(), Error> {
    let path = match target {
        Target::Stdout => {
            let mut out = io::stdout().lock();
            return match out.write_all(bytes).and_then(|()| out.flush()) {
                Err(e) if e.kind() != io::ErrorKind::BrokenPipe => {
                    Err(Error::write_failed("stdout", &e))
                }
                _ => Ok(()),
            };
        }
        Target::File(path) => path,
    };
    let mut temp = OsString::from(".");
    temp.push(path.file_name().unwrap_or_default());
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = path.with_file_name(temp);
    let written = fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, path));
    written.map_err(|e| {
        let _ = fs::remove_file(&temp);
        Error::write_failed(&path.display().to_string(), &e)
    })
}

/// Whether `output` is the file `input` names.
pub fn is_input(output: &Path, input: &Path) -> bool {
    match (output.canonicalize(), input.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(input: &str, output: Option<&str>, to: Option<To>) -> Result<(Target, To), Error> {
        target(OsStr::new(input), output.map(OsStr::new), to)
    }

    fn file(path: &str) -> Target {
        Target::File(PathBuf::from(path))
    }

    #[test]
    fn outputs_go_beside_the_input_with_the_diagram_extension_replaced() {
        let cases = [
            ("flow.mmd", None, ("flow.svg", To::Svg)),
            ("dir/board.drawio.png", None, ("dir/board.svg", To::Svg)),
            (
                "scene.excalidraw.png",
                Some(To::Png),
                ("scene.png", To::Png),
            ),
            ("graph.gv", Some(To::Png), ("graph.png", To::Png)),
            ("notes.txt", None, ("notes.svg", To::Svg)),
            ("my board.drawio", None, ("my board.svg", To::Svg)),
        ];
        for (input, to, (path, want)) in cases {
            assert_eq!(named(input, None, to), Ok((file(path), want)), "{input}");
        }
    }

    #[test]
    fn stdin_and_dash_go_to_stdout() {
        assert_eq!(named("-", None, None), Ok((Target::Stdout, To::Svg)));
        assert_eq!(
            named("-", None, Some(To::Png)),
            Ok((Target::Stdout, To::Png))
        );
        assert_eq!(
            named("a.mmd", Some("-"), Some(To::Png)),
            Ok((Target::Stdout, To::Png))
        );
        assert_eq!(
            named("-", Some("out.PNG"), None),
            Ok((file("out.PNG"), To::Png))
        );
    }

    #[test]
    fn the_output_extension_sets_the_format_and_must_agree_with_to() {
        assert_eq!(
            named("a.mmd", Some("a.png"), Some(To::Png)),
            Ok((file("a.png"), To::Png))
        );
        let conflict = named("a.mmd", Some("a.png"), Some(To::Svg)).unwrap_err();
        assert_eq!(
            (conflict.code, conflict.message.as_str()),
            ("format_conflict", "-o says PNG but --to says SVG.")
        );
        for bad in ["a.jpg", "a", "a.svg.txt", ".png"] {
            assert_eq!(
                named("a.mmd", Some(bad), None).unwrap_err().code,
                "bad_extension",
                "{bad}"
            );
        }
    }
}
