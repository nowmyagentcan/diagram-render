//! The local page on 127.0.0.1: `GET /` serves it and `POST /api/render` renders one diagram and
//! answers with the image. Nothing is written to disk.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::{Value, json};

use crate::error::Error;
use crate::input;
use crate::output::{self, Target, To};
use crate::run::{self, Options, Picture, Source};
use crate::text::Made;

const PAGE: &str = include_str!("ui.html");
/// The most a request may send in its line and headers.
const HEAD_LIMIT: u64 = 16 * 1024;
/// The page reaches nothing but its own server; results are object URLs.
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data: blob:; connect-src 'self'; base-uri 'none'; form-action 'none'";
const JSON: &str = "application/json";
/// What `decodeURIComponent` reads back, as `encodeURIComponent` leaves it.
const ENCODED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Requests are answered side by side, but diagrams are drawn one at a time: an engine or a large
/// PNG can take hundreds of megabytes.
static RENDER: Mutex<()> = Mutex::new(());

struct Response {
    status: u16,
    kind: &'static str,
    body: Vec<u8>,
    /// `X-Result`: the render's report, percent-encoded.
    result: Option<String>,
}

fn json(status: u16, body: Value) -> Response {
    Response {
        status,
        kind: JSON,
        body: body.to_string().into_bytes(),
        result: None,
    }
}

fn refused(status: u16, code: &str, message: &str) -> Response {
    let body = json!({"error": {"code": code, "message": message, "hint": "Reload the page and try again."}});
    json(status, body)
}

/// Serves on 127.0.0.1:`port` (0 picks a free port) until the process is stopped, and opens the
/// page in the default browser when `open` is set. `stop` says how to stop it.
pub fn serve(port: u16, open: bool, stop: &str, debug: bool) -> Result<(), Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|e| {
        if port == 0 {
            Error::internal(&format!("can't start the page: {e}"))
        } else {
            Error::port_in_use(port)
        }
    })?;
    let port = listener.local_addr().map_or(port, |a| a.port());
    let url = format!("http://127.0.0.1:{port}");
    // With stdout closed nobody learns the address, so end quietly as other commands do.
    match writeln!(io::stdout(), "Open {url}{stop}") {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
        Err(e) => return Err(Error::internal(&format!("can't print the address: {e}"))),
        Ok(()) => {}
    }
    if open {
        browse(url);
    }
    for stream in listener.incoming().flatten() {
        // A client that hangs up or stalls loses only its own connection.
        thread::spawn(move || handle(&stream, port, debug));
    }
    Ok(())
}

/// Asks the OS to open `url`; when it can't, the printed address is enough.
fn browse(url: String) {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Some openers wait for the browser to close, so they run beside the server.
    thread::spawn(move || command.status());
}

/// One request, one response, then the connection closes.
fn handle(stream: &TcpStream, port: u16, debug: bool) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let r = respond(&mut BufReader::new(stream.take(HEAD_LIMIT)), port, debug)?;
    let reason = match r.status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Content Too Large",
        _ => "Unprocessable Content",
    };
    let result = r
        .result
        .map(|x| format!("X-Result: {x}\r\n"))
        .unwrap_or_default();
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{result}\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: {CSP}\r\nConnection: close\r\n\r\n",
        r.status,
        r.kind,
        r.body.len()
    )?;
    out.write_all(&r.body)?;
    stream.shutdown(Shutdown::Write)?;
    // Read what the client still sends, so closing doesn't reset the connection before it
    // has read the answer.
    io::copy(&mut stream.take(1 << 20), &mut io::sink()).map(drop)
}

fn respond(
    reader: &mut BufReader<io::Take<&TcpStream>>,
    port: u16,
    debug: bool,
) -> io::Result<Response> {
    // Lines are bytes: a header value may carry opaque ones (RFC 9110 §5.5).
    let (mut request, mut line) = (Vec::new(), Vec::new());
    reader.read_until(b'\n', &mut request)?;
    let (mut host, mut origin, mut length, mut name, mut options) = (None, None, None, None, None);
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(match reader.get_ref().limit() {
                0 => refused(
                    413,
                    "too_large",
                    "The request headers are larger than 16 KB.",
                ),
                _ => refused(400, "bad_request", "The request ended early."),
            });
        }
        if line.iter().all(|b| b"\r\n".contains(b)) {
            break;
        }
        let token = |n: &[u8]| {
            !n.is_empty()
                && n.iter()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
        };
        // RFC 9110 §5.5: a field value is visible bytes, spaces and tabs; any other control byte
        // makes it invalid, and only the line's own CRLF ends it.
        let value = |v: &[u8]| {
            let v = v
                .strip_suffix(b"\r\n")
                .or(v.strip_suffix(b"\n"))
                .unwrap_or(v);
            !v.iter().any(|&b| (b < b' ' && b != b'\t') || b == 0x7f)
        };
        let Some(colon) = line
            .iter()
            .position(|&b| b == b':')
            .filter(|&i| token(&line[..i]) && value(&line[i + 1..]))
        else {
            return Ok(refused(400, "bad_request", "Can't read a request header."));
        };
        let field = match line[..colon].to_ascii_lowercase().as_slice() {
            b"host" => &mut host,
            b"origin" => &mut origin,
            b"content-length" => &mut length,
            b"x-file-name" => &mut name,
            b"x-options" => &mut options,
            // Bodies come only with a Content-Length; chunked ones aren't read.
            b"transfer-encoding" => {
                let message = "Send the body with a Content-Length.";
                return Ok(refused(400, "bad_request", message));
            }
            _ => continue,
        };
        // The headers read here are text; other headers may carry any bytes.
        let Ok(value) = std::str::from_utf8(&line[colon + 1..]) else {
            return Ok(refused(400, "bad_request", "Can't read a request header."));
        };
        // A second value could disagree with the first one checked (RFC 9112 §3.2, §6.3).
        if field.replace(value.trim().to_owned()).is_some() {
            return Ok(refused(400, "bad_request", "A request header is repeated."));
        }
    }
    // RFC 9112 §3: the request line is ASCII.
    let request = std::str::from_utf8(&request).ok().filter(|r| r.is_ascii());
    let mut parts = request
        .unwrap_or_default()
        .trim_end_matches(['\r', '\n'])
        .split(' ');
    let (Some(method), Some(target), Some("HTTP/1.1" | "HTTP/1.0"), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok(refused(400, "bad_request", "Can't read the request line."));
    };
    let local = |scheme: &str, value: &str| {
        ["127.0.0.1", "localhost"]
            .iter()
            .any(|h| value.eq_ignore_ascii_case(&format!("{scheme}{h}:{port}")))
    };
    if !host.is_some_and(|h| local("", &h)) || origin.is_some_and(|o| !local("http://", &o)) {
        let message = "Only this computer's page can use diagram-render ui.";
        return Ok(refused(403, "forbidden", message));
    }
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    match (method, path) {
        ("GET", "/") => Ok(Response {
            status: 200,
            kind: "text/html; charset=utf-8",
            body: PAGE.into(),
            result: None,
        }),
        ("POST", "/api/render") => {
            // Digits only: Rust would also read a leading `+`.
            let digits = |l: &String| l.bytes().all(|b| b.is_ascii_digit());
            let Some(length) = length.map_or(Some(0), |l| l.parse().ok().filter(|_| digits(&l)))
            else {
                return Ok(refused(
                    400,
                    "bad_request",
                    "Can't read the request length.",
                ));
            };
            if length > input::LIMIT {
                return Ok(refused(
                    413,
                    "too_large",
                    "The diagram is larger than 64 MiB.",
                ));
            }
            let buffered = reader.buffer().len() as u64;
            reader.get_mut().set_limit(length.saturating_sub(buffered));
            let mut body = Vec::new();
            reader.take(length).read_to_end(&mut body)?;
            if (body.len() as u64) < length {
                return Ok(refused(400, "bad_request", "The request ended early."));
            }
            Ok(render(name.as_deref(), options.as_deref(), body, debug))
        }
        _ => Ok(refused(404, "not_found", "There's nothing here.")),
    }
}

/// Renders one upload: 200 with the image and its report in `X-Result`, 422 with the report of a
/// failure or the options' usage error, 400 when the name or options can't be read.
fn render(name: Option<&str>, options: Option<&str>, data: Vec<u8>, debug: bool) -> Response {
    let start = Instant::now();
    let Some(name) = unescape(name.unwrap_or_default()) else {
        return refused(400, "bad_request", "Can't read the file name.");
    };
    let options = options.map(serde_json::from_str::<Options>).transpose();
    let options = match options {
        Ok(options) => options.unwrap_or_default(),
        Err(e) => return refused(400, "bad_request", &format!("Can't read the options: {e}.")),
    };
    let (choices, to) = match options.choices() {
        Ok(choices) => choices,
        Err(e) => return json(422, json!({"error": e.json()})),
    };
    let to = to.unwrap_or(To::Svg);
    // Only the name of a file is used: its extension and the download's name.
    let file = Path::new(name.as_ref()).file_name().unwrap_or_default();
    let file = file.to_string_lossy();
    let source = Source {
        name: if file.is_empty() { "-" } else { &file },
        shown: if file.is_empty() {
            "The diagram"
        } else {
            &file
        },
        path: (!file.is_empty()).then(|| Path::new(file.as_ref())),
    };
    let input = source.name;
    let _one_at_a_time = RENDER.lock().unwrap_or_else(|e| e.into_inner());
    let mut opened = match run::open(source, Ok(data), &choices) {
        Ok(opened) => opened,
        Err(stopped) => return json(422, stopped.json(input)),
    };
    let drawn = opened.draw(to);
    let picture = drawn.and_then(|drawn| {
        let page = choices.page.as_ref().and(drawn.page.as_ref());
        let output = download_name(&file, to, page.map(|p| p.name.as_str()))?;
        let picture = opened.picture(drawn.svg, to)?;
        Ok((picture, output, drawn.page, drawn.pages))
    });
    if debug {
        crate::debug(opened.log());
    }
    let known = std::mem::take(&mut opened.known);
    let (picture, output, page, pages) = match picture {
        Ok(done) => done,
        Err(e) => return json(422, run::Stopped::new(e, known).json(input)),
    };
    let Picture {
        bytes,
        width,
        height,
        scale,
    } = picture;
    let made = Made {
        input: input.into(),
        output,
        to,
        width,
        height,
        bytes: bytes.len(),
        scale,
        took: start.elapsed(),
        page,
        pages,
    };
    let result = run::render_json(&known, &made).to_string();
    Response {
        status: 200,
        kind: match to {
            To::Svg => "image/svg+xml",
            To::Png => "image/png",
        },
        body: bytes,
        result: Some(utf8_percent_encode(&result, ENCODED).to_string()),
    }
}

/// Percent-decodes `s` as UTF-8, refusing what `encodeURIComponent` never writes.
fn unescape(s: &str) -> Option<Cow<'_, str>> {
    let hex = |p: &str| p.bytes().take(2).filter(u8::is_ascii_hexdigit).count() == 2;
    s.split('%').skip(1).all(hex).then_some(())?;
    percent_decode_str(s).decode_utf8().ok()
}

/// The name a download gets: the output of `file`, or of `diagram` when pasted text has none.
fn download_name(file: &str, to: To, page: Option<&str>) -> Result<String, Error> {
    let stem = if file.is_empty() || file == "-" {
        "diagram"
    } else {
        file
    };
    let (target, _) = output::target(OsStr::new(stem), None, Some(to), page)?;
    let Target::File(path) = target else {
        unreachable!("a named input goes to a file")
    };
    Ok(path.to_string_lossy().into_owned())
}
