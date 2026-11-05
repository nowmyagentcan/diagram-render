//! Network paths in diagram sources. `dot` and `d2` open image paths with the system's file calls,
//! and on Windows `//host/share/x.png` or `\\host\share\x.png` makes the system contact that host,
//! whatever proxy or option is set. Neither engine has a switch for it, so a source that names
//! one is refused before the engine runs, on every system, so a file means the same everywhere.
//!
//! The scans read a source as its language does (joined DOT strings, D2 variables and imports)
//! and err on the side of refusing.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::input;

/// A network path found in a source: its offset, when it is in the source itself and not an
/// imported file, and the path as written.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub at: Option<usize>,
    pub path: String,
}

fn separator(c: char) -> bool {
    matches!(c, '/' | '\\')
}

/// Whether Windows takes `path` to the network: two separators first (`\\host`, `/\host`, `//?/`)
/// or the `\??\` prefix.
fn network(path: &str) -> bool {
    let mut chars = path.chars();
    chars.next().is_some_and(separator)
        && (chars.next().is_some_and(separator) || path[1..].starts_with("??"))
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
        Token::Word(w) if w.html => html_src(&w.text).map(|path| Found {
            at: Some(w.at),
            path,
        }),
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
            at: Some(value.at),
            path: path.clone(),
        })
    });
    html.or(attribute)
}

/// `key: value` in D2 text, as a scan sees it.
struct Value<'a> {
    key: String,
    /// What follows the colon, past spaces, quotes and escapes.
    value: &'a str,
    at: usize,
    /// Nothing stands between the colon and the value, as in `https://`.
    tight: bool,
}

fn values(text: &str) -> impl Iterator<Item = Value<'_>> {
    text.match_indices(':').map(|(i, _)| {
        let before = text[..i].trim_end_matches([' ', '\t']);
        let word = |c: &char| c.is_alphanumeric() || matches!(c, '_' | '-' | '\\');
        let key: Vec<char> = before.chars().rev().take_while(word).collect();
        let key = key.into_iter().rev().filter(|c| *c != '\\').collect();
        let after = &text[i + 1..];
        let value = after.trim_start_matches([' ', '\t', '\r', '\n', '"', '\'', '\\']);
        Value {
            key,
            value,
            at: text.len() - value.len(),
            tight: after.starts_with('/'),
        }
    })
}

/// Whether a D2 value starts with two slashes once its escapes are read. Go keeps a backslash as
/// `%5C` in an icon's URL, so only slashes reach the system.
fn slashes(value: &str) -> bool {
    value
        .chars()
        .filter(|c| *c != '\\')
        .take(2)
        .eq("//".chars())
}

/// The text of an icon's value inside a block string (`|md //host/x.png |`), else `value`.
fn unblocked(value: &str) -> &str {
    let bare = value.trim_start_matches(['|', '`']);
    match bare.len() == value.len() {
        true => value,
        false => bare.trim_start_matches(char::is_alphanumeric).trim_start(),
    }
}

/// The variable an icon's value starts with, after any slashes, and whether slashes came first.
fn substitution(value: &str) -> Option<(&str, bool)> {
    let bare = value.trim_start_matches(separator);
    let name = bare.strip_prefix("${")?.split('}').next()?;
    let name = name.rsplit('.').next()?;
    Some((name, bare.len() != value.len()))
}

/// Whether every value `name` is given in `texts` is an http(s) URL, which no substitution can
/// turn into a network path.
fn web_address(name: &str, texts: &[String]) -> bool {
    let mut found = texts
        .iter()
        .flat_map(|t| values(t))
        .filter(|v| v.key == name)
        .peekable();
    found.peek().is_some()
        && found.all(|v| {
            let head = v.value.get(..8).unwrap_or(v.value).to_ascii_lowercase();
            head.starts_with("http://") || head.starts_with("https://")
        })
}

/// The D2 files `text` in `dir` imports, by every name an `@` could mean: `@x`, `@x.d2`, `@x.y`.
fn imported(text: &str, dir: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    for (i, _) in text.match_indices('@') {
        let rest = text[i + 1..].trim_start_matches(['"', '\'']);
        let end = |c: char| c.is_whitespace() || "\"';{},".contains(c);
        let name = rest.split(end).next().unwrap_or_default();
        // d2 refuses these, and opening one here would be what this module stops.
        if name.is_empty() || name.starts_with(separator) || name.contains(':') {
            continue;
        }
        let stops = name.match_indices('.').map(|(i, _)| i);
        for stem in stops.chain([name.len()]).map(|i| &name[..i]) {
            let stem = stem.strip_suffix(".d2").unwrap_or(stem);
            files.push(dir.join(format!("{stem}.d2")));
        }
    }
    files
}

/// The first network path a D2 source can make of an icon, in the text or in what it imports
/// from `dir`. The value of a substitution isn't known here, so an icon that starts with one
/// passes only when the variable is a web address.
pub fn d2(text: &str, dir: &Path) -> Option<Found> {
    let mut texts = vec![text.to_string()];
    let mut dirs = vec![dir.to_path_buf()];
    let mut seen = BTreeSet::new();
    let mut next = 0;
    while next < texts.len() {
        for file in imported(&texts[next], &dirs[next]) {
            let plain = fs::metadata(&file).is_ok_and(|m| m.is_file() && m.len() <= input::LIMIT);
            if plain
                && seen.insert(file.clone())
                && let Ok(bytes) = fs::read(&file)
            {
                texts.push(String::from_utf8_lossy(&bytes).into_owned());
                dirs.push(file.parent().unwrap_or(dir).to_path_buf());
            }
        }
        next += 1;
    }
    texts.iter().enumerate().find_map(|(index, text)| {
        values(text).find_map(|v| {
            let icon = v.key == "icon";
            let value = if icon { unblocked(v.value) } else { v.value };
            let scheme = v.tight && !v.key.is_empty() && !icon;
            let bad = slashes(value) && !scheme
                || icon
                    && substitution(value)
                        .is_some_and(|(name, after)| after || !web_address(name, &texts));
            bad.then(|| Found {
                at: (index == 0).then_some(v.at),
                path: value.lines().next().unwrap_or_default().trim().into(),
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(found.at, Some(20));
        assert_eq!(found.path, "//h/x");
    }

    #[test]
    fn d2_values_are_read_past_quotes_variables_and_blocks() {
        let here = Path::new(".");
        for source in [
            r#"a: {shape: image; icon: "//h/s/x.png"}"#,
            "a.icon: //h/s/x.png",
            "a.icon://h/s/x.png",
            "a.ic\\on: //h/s/x.png",
            r#"a.icon: '//h/s/x.png'"#,
            r#"a.icon: "\/\/h/s/x.png""#,
            "a.icon: \\\n  //h/s/x.png",
            "a.icon: |md //h/s/x.png |",
            "classes: {c: {icon: //h/s/x.png}}\na.class: c",
            "*.icon: //h/s/x.png",
            r#"vars: {p: "//h/s/x.png"}"#,
            "vars: {p: /}\na.icon: ${p}/h/s/x.png",
            "vars: {p: /h/s/x.png}\na.icon: /${p}",
            "vars: {p: https://x}\na.icon: /${p}",
            "vars: {e: \"\"}\na.icon: ${e}//h/s/x.png",
            "a.icon: ${nowhere}/x.png",
            "vars: {p: https://x; q: {p: /}}\na.icon: ${p}/h",
        ] {
            assert!(d2(source, here).is_some(), "{source}");
        }
    }

    #[test]
    fn what_d2_never_opens_as_a_network_path_is_left_alone() {
        let here = Path::new(".");
        for source in [
            "a.icon: https://icons.terrastruct.com/x.svg",
            "a.icon: HTTP://icons.terrastruct.com/x.svg",
            "vars: {base: https://icons.terrastruct.com}\na.icon: ${base}/x.svg",
            "vars: {a: {base: 'https://x'}}\nb.icon: ${a.base}/x.svg",
            "a.icon: ./images/x.png",
            "a.icon: images/x.png",
            "a.icon: /usr/share/x.png",
            "a.link: https://example.com/x",
            "a: see s3://bucket/x",
            "a: |go\n  x := 1 // a comment\n|",
            "a -> b: \"x // y\"",
        ] {
            assert_eq!(d2(source, here), None, "{source}");
        }
    }

    #[test]
    fn a_d2_import_is_read_too() {
        let dir = std::env::temp_dir().join(format!("netpath-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("one.d2"), "...@sub/two\n").unwrap();
        fs::write(dir.join("sub/two.d2"), "...@three\n").unwrap();
        fs::write(dir.join("sub/three.d2"), "x.icon: //h/s/x.png\n").unwrap();
        fs::write(dir.join("vars.d2"), "base: https://x\n").unwrap();
        for source in [
            "a: @one",
            "...@one",
            "...@\"one\"",
            "...@one.d2",
            "a: @one.x",
        ] {
            let found = d2(source, &dir).expect(source);
            assert_eq!(found.at, None, "{source}");
        }
        assert_eq!(d2("...@vars\na.icon: ${base}/x.svg", &dir), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
