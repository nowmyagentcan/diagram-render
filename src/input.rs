//! Reading a diagram: a file or stdin within the size limit, decoded to UTF-8 text, with lines
//! counted the same way for LF, CRLF and CR files.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::error::{Error, Excerpt};
use crate::netpath::Found;

/// The most any input may hold.
pub const LIMIT: u64 = 64 << 20;

/// The bytes of `arg`, a path or `-` for stdin.
pub fn read(arg: &OsStr) -> Result<Vec<u8>, Error> {
    if arg == "-" {
        return limited(io::stdin().lock())
            .map_err(|e| Error::read_failed("stdin", &e))?
            .ok_or_else(|| Error::too_large("stdin", "over 64 MiB"));
    }
    read_file(arg)
}

/// The bytes of the file at `arg`, whatever its name.
pub fn read_file(arg: &OsStr) -> Result<Vec<u8>, Error> {
    let name = arg.to_string_lossy();
    let path = Path::new(arg);
    let failed = |e: io::Error| match e.kind() {
        io::ErrorKind::NotFound => Error::file_not_found(&name),
        _ => Error::read_failed(&name, &e),
    };
    let meta = path.metadata().map_err(failed)?;
    if meta.is_dir() {
        return Err(Error::not_a_file(&name));
    }
    if meta.len() > LIMIT {
        return Err(Error::too_large(&name, &mib(meta.len())));
    }
    File::open(path)
        .and_then(limited)
        .map_err(failed)?
        .ok_or_else(|| Error::too_large(&name, "over 64 MiB"))
}

/// Everything `r` holds, or `None` past the limit.
fn limited(r: impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = vec![];
    r.take(LIMIT + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= LIMIT).then_some(bytes))
}

/// The most an embedded or compressed diagram may unpack to.
pub const UNPACKED_LIMIT: u64 = 256 << 20;

/// `data` unpacked from zlib, or from raw deflate; `None` past the limit.
pub fn inflate(data: &[u8], zlib: bool) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = vec![];
    match zlib {
        true => flate2::read::ZlibDecoder::new(data)
            .take(UNPACKED_LIMIT + 1)
            .read_to_end(&mut bytes),
        false => flate2::read::DeflateDecoder::new(data)
            .take(UNPACKED_LIMIT + 1)
            .read_to_end(&mut bytes),
    }?;
    Ok((bytes.len() as u64 <= UNPACKED_LIMIT).then_some(bytes))
}

/// `5.0 MiB`.
pub fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1 << 20) as f64)
}

/// The text of `bytes`: UTF-8 with its BOM dropped, or UTF-16 with a BOM converted.
pub fn text(bytes: &[u8]) -> Result<String, Error> {
    let (text, bad) = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, u16::from_be_bytes)
    } else {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_owned(), None),
            Err(e) => (
                String::from_utf8_lossy(bytes).into_owned(),
                Some(e.valid_up_to()),
            ),
        }
    };
    let Some(at) = bad else {
        return match text.trim().is_empty() {
            true => Err(Error::empty_input()),
            false => Ok(text),
        };
    };
    // The text before `at` is the input's own, so the bad byte is where `at` points.
    let lines = Lines::new(&text);
    let (line, column) = lines.locate(at);
    Err(lines.place(Error::not_utf8(line), line, Some(column)))
}

/// UTF-16 units from byte pairs, decoded; also where the first unit that isn't text starts.
fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> (String, Option<usize>) {
    // A lone last byte becomes an unpaired surrogate, so it reads as bad text too.
    let units = bytes.chunks(2).map(|c| match *c {
        [a, b] => unit([a, b]),
        _ => 0xDC00,
    });
    let (mut text, mut bad) = (String::new(), None);
    for c in char::decode_utf16(units) {
        if c.is_err() {
            bad.get_or_insert(text.len());
        }
        text.push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    (text, bad)
}

/// Where each line of a text starts; a line ends at LF, CRLF or CR.
pub struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    /// `network_path` for what a scan of this text found, at its place.
    pub fn refuse(&self, found: &Found) -> Error {
        let e = Error::network_path(&found.path);
        let (line, column) = self.locate(found.at);
        self.place(e, line, Some(column))
    }

    pub fn new(text: &'a str) -> Self {
        let b = text.as_bytes();
        let mut starts = vec![0];
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\n' => starts.push(i + 1),
                b'\r' if b.get(i + 1) == Some(&b'\n') => {}
                b'\r' => starts.push(i + 1),
                _ => {}
            }
            i += 1;
        }
        Self { text, starts }
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    /// Line `n` (from 1) without its line end; empty past the last line.
    pub fn line(&self, n: usize) -> &'a str {
        let Some(&start) = self.starts.get(n.wrapping_sub(1)) else {
            return "";
        };
        let end = self.starts.get(n).copied().unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\r', '\n'])
    }

    /// The line and character column (both from 1) of byte `offset`, an engine's: one past the
    /// end or inside a character counts as the place before it.
    pub fn locate(&self, offset: usize) -> (usize, usize) {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s <= offset);
        let start = self.starts[line - 1];
        (line, self.text[start..offset].chars().count() + 1)
    }

    /// `e` placed at `line` and `column`, with that line as its excerpt.
    pub fn place(&self, mut e: Error, line: usize, column: Option<usize>) -> Error {
        e.line = Some(line);
        e.column = column;
        e.excerpt = Some(Box::new(excerpt(self.line(line), column)));
        e
    }
}

/// `line` cut to 200 characters around `column`, with `…` where it was cut.
fn excerpt(line: &str, column: Option<usize>) -> Excerpt {
    const MAX: usize = 200;
    let n = line.chars().count();
    if n <= MAX {
        return Excerpt {
            text: line.into(),
            caret: column,
            end: None,
        };
    }
    let at = column.unwrap_or(1).min(n);
    let start = at.saturating_sub(MAX / 2 + 1).min(n - MAX);
    let mut chars: Vec<char> = line.chars().skip(start).take(MAX).collect();
    if start > 0 {
        chars[0] = '…';
    }
    if start + MAX < n {
        chars[MAX - 1] = '…';
    }
    Excerpt {
        text: chars.into_iter().collect(),
        caret: column.map(|c| c - start),
        end: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_end_at_lf_crlf_or_cr() {
        for text in ["a\nbé\n\nc", "a\r\nbé\r\n\r\nc", "a\rbé\r\rc"] {
            let lines = Lines::new(text);
            let got: Vec<_> = (1..=5).map(|n| lines.line(n)).collect();
            assert_eq!(got, ["a", "bé", "", "c", ""], "{text:?}");
            let c = text.rfind('c').unwrap();
            assert_eq!(lines.locate(c), (4, 1));
            assert_eq!(lines.locate(text.find('é').unwrap() + 2), (2, 3));
        }
    }

    #[test]
    fn long_lines_are_cut_around_the_column() {
        let line: String = (0..300)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let x = excerpt(&line, Some(150));
        assert_eq!(x.text.chars().count(), 200);
        assert!(x.text.starts_with('…') && x.text.ends_with('…'));
        let caret = x.caret.unwrap();
        assert_eq!(x.text.chars().nth(caret - 1), line.chars().nth(149));
        let x = excerpt(&line, Some(3));
        assert_eq!((x.text.chars().next(), x.caret), (Some('a'), Some(3)));
        assert!(x.text.ends_with('…'));
        let x = excerpt(&line, Some(300));
        assert!(x.text.starts_with('…') && x.text.ends_with('n'));
        assert_eq!(
            x.text.chars().nth(x.caret.unwrap() - 1),
            line.chars().last()
        );
        assert_eq!(excerpt("short", Some(9)).caret, Some(9));
    }

    #[test]
    fn utf16_with_a_bom_becomes_utf8() {
        let utf16: Vec<u16> = "graph TD\r\n  A-->漢".encode_utf16().collect();
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(utf16.iter().flat_map(|u| u.to_le_bytes()))
            .collect();
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain(utf16.iter().flat_map(|u| u.to_be_bytes()))
            .collect();
        assert_eq!(text(&le).unwrap(), "graph TD\r\n  A-->漢");
        assert_eq!(text(&be).unwrap(), "graph TD\r\n  A-->漢");
        assert_eq!(text(b"\xEF\xBB\xBFa -> b").unwrap(), "a -> b");
    }

    #[test]
    fn bad_text_names_the_line_and_column_of_the_first_bad_byte() {
        let lf = text(b"a\nb\nc\xC3d\xFF\n").unwrap_err();
        assert_eq!(
            (lf.code, lf.line, lf.column),
            ("not_utf8", Some(3), Some(2))
        );
        assert_eq!(lf.message, "Line 3 isn't valid UTF-8 text.");
        assert_eq!(lf.excerpt.unwrap().text, "c\u{FFFD}d\u{FFFD}");
        let crlf = text(b"\xEF\xBB\xBFa\r\nb\r\nc\xC3d\r\n").unwrap_err();
        assert_eq!((crlf.line, crlf.column), (Some(3), Some(2)));
        let mut le = vec![0xFF, 0xFE];
        for u in "a\rb\rc".encode_utf16().chain([0xD800, 0x41]) {
            le.extend(u.to_le_bytes());
        }
        let utf16 = text(&le).unwrap_err();
        assert_eq!((utf16.line, utf16.column), (Some(3), Some(2)));
        assert_eq!(text(&[0xFF, 0xFE, 0x41]).unwrap_err().column, Some(1));
    }

    #[test]
    fn blank_text_is_empty() {
        for blank in ["", " \r\n\t", "\u{FEFF}"] {
            assert_eq!(
                text(blank.as_bytes()).unwrap_err().code,
                "empty_input",
                "{blank:?}"
            );
        }
    }
}
