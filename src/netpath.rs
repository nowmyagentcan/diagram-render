//! Network paths. On Windows `//host/x.png`, `\\host\x.png` or a mapped drive make the system
//! contact a host, whatever proxy or option is set. Neither `dot` nor `d2` has a switch for it, and
//! nothing outside them can stop their file calls, so what could reach one is refused before the
//! engine runs. Elsewhere these are plain paths, and this finds none.
//!
//! DOT is read as Graphviz reads it. D2 can't be read that way, since d2 evaluates variables and
//! imports and even looks for images in its own SVG, so a D2 source that could open any file at
//! all is refused until `--allow-network` says so.

/// A network path found in a source: its offset and the path as written.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub at: usize,
    pub path: String,
}

fn separator(c: char) -> bool {
    matches!(c, '/' | '\\')
}

/// Whether reaching `path` would leave this computer.
#[cfg(windows)]
pub fn network(path: &str) -> bool {
    remote(path) || std::path::absolute(path).is_ok_and(|p| remote(&p.to_string_lossy()))
}

#[cfg(not(windows))]
pub fn network(_path: &str) -> bool {
    false
}

/// Two separators first, a device prefix before anything but a drive, or a drive that is mapped.
#[cfg(windows)]
fn remote(path: &str) -> bool {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::core::HSTRING;
    const DRIVE_REMOTE: u32 = 4;
    let path = path.replace('/', "\\");
    let device = [r"\\?\", r"\\.\", r"\??\"].map(|prefix| path.strip_prefix(prefix));
    let device = device.into_iter().flatten().next();
    match device.unwrap_or(&path).as_bytes() {
        // SAFETY: the name is a live string that ends in a null.
        [drive, b':', ..] => unsafe {
            GetDriveTypeW(&HSTRING::from(format!("{}:\\", *drive as char))) == DRIVE_REMOTE
        },
        _ => device.is_some() || path.starts_with(r"\\"),
    }
}

/// A DOT token that holds text: an ID, a quoted string, or an HTML string.
struct Word {
    text: String,
    at: usize,
    html: bool,
}

enum Token {
    Word(Word),
    Equals,
    Other,
}

/// The quoted string at `start`: its text as Graphviz reads it, and where it ends.
fn quoted(text: &str, start: usize) -> (String, usize) {
    let mut out = String::new();
    let mut chars = text[start + 1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return (out, start + 1 + i + 1),
            '\\' => match chars.next() {
                Some((_, '"')) => out.push('"'),
                Some((_, '\n')) => {}
                Some((_, '\r')) => {
                    chars.next();
                }
                Some((_, c)) => out.extend(['\\', c]),
                None => {}
            },
            c => out.push(c),
        }
    }
    (out, text.len())
}

/// DOT's tokens, with `"a" + "b"` joined and comments gone.
fn tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let line_end = |i: usize| text[i..].find('\n').map_or(text.len(), |n| i + n);
    let (mut out, mut i, mut plus) = (vec![], 0, false);
    while i < bytes.len() {
        let start = i;
        let next = bytes.get(i + 1);
        match bytes[i] {
            b'/' if next == Some(&b'/') => i = line_end(i),
            b'/' if next == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(text.len(), |n| i + n + 4);
            }
            b'#' if i == 0 || bytes[i - 1] == b'\n' => i = line_end(i),
            b'"' => {
                let (string, end) = quoted(text, i);
                i = end;
                match out.last_mut() {
                    Some(Token::Word(word)) if plus => word.text += &string,
                    _ => out.push(Token::Word(Word {
                        text: string,
                        at: start,
                        html: false,
                    })),
                }
                plus = false;
            }
            b'+' => {
                plus = true;
                i += 1;
            }
            b'<' => {
                let mut depth = 0;
                while i < bytes.len() {
                    depth += i32::from(bytes[i] == b'<') - i32::from(bytes[i] == b'>');
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
                out.push(Token::Word(Word {
                    text: text.get(start + 1..i - 1).unwrap_or_default().into(),
                    at: start,
                    html: true,
                }));
            }
            b'=' => {
                out.push(Token::Equals);
                i += 1;
            }
            c if c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 => {
                let rest = &bytes[i..];
                let word = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_' || *c >= 0x80;
                i += rest.iter().position(|c| !word(c)).unwrap_or(rest.len());
                out.push(Token::Word(Word {
                    text: text[start..i].into(),
                    at: start,
                    html: false,
                }));
            }
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                out.push(Token::Other);
                i += 1;
            }
        }
    }
    out
}

/// A network path in an HTML label's `SRC`, or one spelled with a character reference.
fn html_src(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    lower.match_indices("src").find_map(|(i, _)| {
        let rest = html[i + 3..].trim_start().strip_prefix('=')?.trim_start();
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let value = rest[1..].split(quote).next()?;
        (network(value) || value.contains('&')).then(|| value.into())
    })
}

/// The first network path the attributes of a DOT source name.
pub fn dot(text: &str) -> Option<Found> {
    let tokens = tokens(text);
    let html = tokens.iter().find_map(|t| match t {
        Token::Word(w) if w.html => html_src(&w.text).map(|path| Found { at: w.at, path }),
        _ => None,
    });
    let attribute = tokens.windows(3).find_map(|t| {
        let [Token::Word(name), Token::Equals, Token::Word(value)] = t else {
            return None;
        };
        let path = &value.text;
        let bad = match name.text.as_str() {
            "image" | "shapefile" => network(path),
            // A root in the list makes `root/host/share/x.png` of a plain image name.
            "imagepath" => path
                .split([';', ':'])
                .any(|dir| network(dir) || (!dir.is_empty() && dir.chars().all(separator))),
            _ => false,
        };
        bad.then(|| Found {
            at: value.at,
            path: path.clone(),
        })
    });
    html.or(attribute)
}

/// Whether a D2 source could make d2 open a file: an import, an icon or image in any spelling
/// (d2 drops a backslash before a letter), or a variable that could put one together. Its text is
/// never evaluated here, so this errs on the side of yes.
pub fn d2_opens_files(text: &str) -> bool {
    let plain = text.replace('\\', "").to_ascii_lowercase();
    plain.contains('@')
        || plain.contains("icon")
        || plain.contains("image")
        || text.contains("${") && text.contains(['<', '/', '\\'])
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_read_as_windows_reads_it() {
        for path in [
            "//h/s/x.png",
            r"\\h\s\x.png",
            r"/\h\s\x.png",
            r"\/h/s/x.png",
            r"\\?\UNC\h\s\x.png",
            "//?/UNC/h/s/x.png",
            r"\\.\UNC\h\s\x.png",
            r"\??\UNC\h\s\x.png",
            r"\\?\GLOBALROOT\Device\Mup\h\s\x.png",
        ] {
            assert!(network(path), "{path}");
        }
        for path in [
            "x.png",
            "icons/x.png",
            "/pics/x.png",
            r"C:\pics\x.png",
            r"\\?\C:\pics\x.png",
            r"\??\C:\pics\x.png",
        ] {
            assert!(!network(path), "{path}");
        }
    }

    #[test]
    fn dot_is_read_as_graphviz_reads_it() {
        for source in [
            r#"digraph {a [image="//h/s/x.png"]}"#,
            r#"digraph {a ["image"="//h/s/x.png"]}"#,
            r#"digraph {node [image="\\h\s\x.png"]; a}"#,
            r#"digraph {a [image="/\h\s\x.png"]}"#,
            r#"digraph {a [image="\??\UNC\h\s\x.png"]}"#,
            r#"digraph {a [image="//?/UNC/h/s/x.png"]}"#,
            r#"digraph {a [image="/" + "/h/s/x.png"]}"#,
            "digraph {a [image=\"/\\\n/h/s/x.png\"]}",
            "digraph {a [image=\"/\" /* c */ + // c\n \"/h/s/x.png\"]}",
            r#"digraph {a [image=<//h/s/x.png>]}"#,
            r#"digraph {a [shape=epsf shapefile="//h/s/x.ps"]}"#,
            r#"digraph {graph [imagepath="c:/x;//h/s"]; a [image="x.png"]}"#,
            r#"digraph {graph [imagepath="/"]; a [image="h/s/x.png"]}"#,
            r#"digraph {a [label=<<TABLE><TR><TD><IMG SRC="//h/s/x.png"/></TD></TR></TABLE>>]}"#,
            r#"digraph {a [label=<<table><tr><td><img src = '//h/x'/></td></tr></table>>]}"#,
            r#"digraph {a [label=<<TABLE><TR><TD><IMG SRC="&#47;&#47;h/x"/></TD></TR></TABLE>>]}"#,
        ] {
            assert!(dot(source).is_some(), "{source}");
        }
    }

    #[test]
    fn what_dot_never_opens_is_left_alone() {
        for source in [
            r#"digraph {a [image="x.png"]; b [image="icons/y.png"]}"#,
            r#"digraph {a [image="/usr/share/x.png"]}"#,
            r#"digraph {a [image="C:\pics\x.png"]}"#,
            r#"digraph {a [URL="//h/s/x" href="//h/y" label="//h/z" tooltip="\\h"]}"#,
            r#"digraph {graph [imagepath="pics;C:/pics"]; a}"#,
            "// image=\"//h/s\"\ndigraph {a}\n# image=\"//h/s\"\n/* image=\"//h/s\" */",
            r#"digraph {a [label=<<TABLE><TR><TD><IMG SRC="x.png"/></TD></TR></TABLE>>]}"#,
            r#"digraph {a [label="image=\"//h\""]}"#,
        ] {
            assert_eq!(dot(source), None, "{source}");
        }
    }

    #[test]
    fn a_found_path_says_where_it_is() {
        let found = dot("digraph {\n a [image=\"//h/x\"]\n}").unwrap();
        assert_eq!(found.at, 20);
        assert_eq!(found.path, "//h/x");
    }

    #[test]
    fn a_d2_source_that_could_open_a_file_is_seen() {
        for source in [
            "...@x",
            "a: {shape: image; icon: ./x.png}",
            r"a.ic\on: //h/x.png",
            r#"a: {shape: sql_table; x: int {constraint: "\"/><im\age href=\"//h/x\"/>"}}"#,
            "vars: {a: <im; b: age}\nc: ${a}${b}",
            "vars: {p: //h/x}\na.ic${k}: ${p}",
        ] {
            assert!(d2_opens_files(source), "{source}");
        }
        for source in [
            "a -> b",
            "api: //v1/users",
            "# docs: //wiki/page\na",
            "a: {tooltip: //wiki/page}",
            "vars: {x: 1}\na: ${x}",
        ] {
            assert!(!d2_opens_files(source), "{source}");
        }
    }
}
