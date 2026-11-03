//! The engines in `engines/` beside the executable (merman for Mermaid, Graphviz's `dot`, `d2`),
//! each run as its own process with a cleared environment and a time limit.

use std::env;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::detect::Format;
use crate::error::Error;

/// How long a version check may take. A first start after install can take seconds while the
/// system scans a 50 MB executable.
const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// The engine's file name (without `.exe`) and its name in sentences.
fn names(format: Format) -> Option<(&'static str, &'static str)> {
    match format {
        Format::Mermaid => Some(("merman-cli", "Mermaid")),
        Format::Dot => Some(("dot", "Graphviz")),
        Format::D2 => Some(("d2", "D2")),
        _ => None,
    }
}

/// The folder the engines live in, beside the executable a symlink (as install.sh makes) points
/// to. Windows paths stay as given, without the verbatim prefix `canonicalize` adds there.
fn folder() -> PathBuf {
    let exe = env::current_exe().map(|p| match cfg!(windows) {
        true => p,
        false => p.canonicalize().unwrap_or(p),
    });
    let dir = exe.ok().and_then(|p| p.parent().map(Path::to_path_buf));
    dir.unwrap_or_default().join("engines")
}

/// The engine for `format`; `None` for the formats this binary renders itself.
pub fn locate(format: Format) -> Option<Result<PathBuf, Error>> {
    let (file, label) = names(format)?;
    let exe = folder().join(format!("{file}{}", env::consts::EXE_SUFFIX));
    Some(match exe.is_file() {
        true => Ok(exe),
        false => Err(Error::engine_missing(label, &exe.display().to_string())),
    })
}

/// What an engine run gave back.
pub struct Ran {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub took: Duration,
}

/// Runs `exe` with `args` and nothing on stdin; `None` when it was stopped at `limit`.
pub fn run(exe: &Path, args: &[&str], limit: Duration) -> io::Result<Option<Ran>> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_clear()
        .env("PATH", dir)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        for key in ["SystemRoot", "TEMP", "TMP"] {
            if let Some(value) = env::var_os(key) {
                cmd.env(key, value);
            }
        }
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let start = Instant::now();
    let mut child = cmd.spawn()?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    thread::scope(|s| {
        let stdout = s.spawn(|| all(out));
        let stderr = s.spawn(|| all(err));
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if start.elapsed() >= limit {
                child.kill()?;
                child.wait()?;
                break None;
            }
            thread::sleep(Duration::from_millis(5));
        };
        let (stdout, stderr) = (stdout.join().unwrap()?, stderr.join().unwrap()?);
        Ok(status.map(|status| Ran {
            code: status.code(),
            stdout,
            stderr,
            took: start.elapsed(),
        }))
    })
}

/// Everything a child's pipe gives until it closes.
fn all(pipe: Option<impl Read>) -> io::Result<Vec<u8>> {
    let mut bytes = vec![];
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

/// Whether an engine runs here, with Mermaid's diagram families and the `--debug` lines.
pub struct Probe {
    pub problem: Option<Error>,
    pub families: Vec<String>,
    pub log: Vec<String>,
}

/// Runs `format`'s engine version command, and for Mermaid lists its diagram families.
pub fn probe(format: Format) -> Probe {
    let mut probe = Probe {
        problem: None,
        families: vec![],
        log: vec![],
    };
    let Some((_, label)) = names(format) else {
        return probe;
    };
    let version = if format == Format::Dot {
        "-V"
    } else {
        "--version"
    };
    let result = locate(format).unwrap().and_then(|exe| {
        checked(&exe, &[version], label, &mut probe.log)?;
        if format == Format::Mermaid {
            let ran = checked(&exe, &["capabilities", "--json"], label, &mut probe.log)?;
            let caps: serde_json::Value = serde_json::from_slice(&ran.stdout).unwrap_or_default();
            let families = caps["diagram_families"].as_array().into_iter().flatten();
            probe.families = families
                .filter_map(|f| f.as_str().map(Into::into))
                .collect();
        }
        Ok(())
    });
    probe.problem = result.err();
    probe
}

/// Runs `exe` with `args` within the probe limit, logging it; a failure is `engine_failed`.
fn checked(exe: &Path, args: &[&str], label: &str, log: &mut Vec<String>) -> Result<Ran, Error> {
    log.push(format!("{} {}", exe.display(), args.join(" ")));
    let ran = match run(exe, args, PROBE_LIMIT) {
        Ok(Some(ran)) => ran,
        Ok(None) => {
            log.push(format!("stopped after {} s", PROBE_LIMIT.as_secs()));
            return Err(Error::engine_slow(label, PROBE_LIMIT.as_secs()));
        }
        Err(e) => {
            log.push(format!("couldn't start: {e}"));
            return Err(Error::engine_unstartable(label, &e));
        }
    };
    let code = ran
        .code
        .map_or("killed by a signal".into(), |c| format!("exit code {c}"));
    log.push(format!("{code} after {} ms", ran.took.as_millis()));
    let stderr = String::from_utf8_lossy(&ran.stderr);
    log.extend(stderr.lines().map(Into::into));
    match ran.code {
        Some(0) => Ok(ran),
        _ => Err(Error::engine_failed(label, &code, stderr.lines().next())),
    }
}
