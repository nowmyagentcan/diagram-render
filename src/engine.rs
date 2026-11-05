//! The engines in `engines/` beside the executable (merman for Mermaid, Graphviz's `dot`, `d2`),
//! each run as its own process with a cleared environment, a time limit and a memory limit, tied
//! to this process so none outlives it. This binary's own renderers run the same way (`worker`).

use std::env;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::detect::Format;
use crate::error::{Code, Error, Warning, kept_option};
use crate::input::Lines;
use crate::leash::{self, Leash};
use crate::output::To;
use crate::raster::Png;
use crate::{d2, dot, mermaid, worker};

/// How long a version check may take. A first start after install can take seconds while the
/// system scans a 50 MB executable.
const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// The most an engine may print: an SVG on stdout, messages on stderr (cut there).
const STDOUT_CAP: u64 = 512 << 20;
const STDERR_CAP: u64 = 1 << 20;

/// The most memory a run may hold, so that a small hostile source can't take the computer's. The
/// largest real diagrams measured stay under 600 MB.
pub const MEMORY_LIMIT: u64 = 4 << 30;

/// The engine's file name (without `.exe`) and its name in sentences.
fn names(format: Format) -> Option<(&'static str, &'static str)> {
    match format {
        Format::Mermaid => Some(("merman-cli", "Mermaid")),
        Format::Dot => Some(("dot", "Graphviz")),
        Format::D2 => Some(("d2", "D2")),
        _ => None,
    }
}

/// The install folder: the executable's, or that of the one a symlink (as install.sh makes)
/// points to. Windows paths stay as given, without the verbatim prefix `canonicalize` adds there.
pub fn home() -> PathBuf {
    let exe = env::current_exe().map(|p| match cfg!(windows) {
        true => p,
        false => p.canonicalize().unwrap_or(p),
    });
    let dir = exe.ok().and_then(|p| p.parent().map(Path::to_path_buf));
    dir.unwrap_or_default()
}

/// The folder the engines live in.
fn folder() -> PathBuf {
    home().join("engines")
}

/// An engine executable and the `--debug` lines of its runs.
pub struct Engine {
    pub exe: PathBuf,
    /// Its name in sentences: Mermaid, Graphviz or D2.
    pub label: &'static str,
    pub log: Vec<String>,
    /// This binary as its own worker, which keeps the environment: fonts are found through it.
    pub own: bool,
}

/// What an engine is given besides its arguments.
#[derive(Clone)]
pub struct Job<'a> {
    pub stdin: &'a [u8],
    /// Its working folder, so relative imports and images resolve as they would for its own CLI.
    pub dir: &'a Path,
    /// Variables set on top of the cleared environment.
    pub env: Vec<(&'static str, OsString)>,
    pub limit: Duration,
    /// The bytes of memory it may hold.
    pub memory: u64,
}

/// What an engine run gave back.
pub struct Ran {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// The engine for `format`; `None` for the formats this binary renders itself.
pub fn locate(format: Format) -> Option<Result<Engine, Error>> {
    let (file, label) = names(format)?;
    let exe = folder().join(format!("{file}{}", env::consts::EXE_SUFFIX));
    Some(match exe.is_file() {
        true => Ok(Engine {
            exe,
            label,
            log: vec![],
            own: false,
        }),
        false => Err(Error::engine_missing(label, &exe.display().to_string())),
    })
}

impl Engine {
    /// This binary, for what it draws itself.
    pub fn ours() -> Self {
        Self {
            exe: env::current_exe().unwrap_or_default(),
            label: "diagram-render",
            log: vec![],
            own: true,
        }
    }

    /// Runs the engine with `args` and `job`, with only what the system needs in its
    /// environment, so variables such as `D2_LAYOUT` and programs on the user's PATH can't
    /// change what it does. `None` when it was stopped at the time limit; past the memory limit
    /// it is `too_large`. It is reaped either way.
    pub fn run(&mut self, args: &[&str], job: &Job) -> Result<Option<Ran>, Error> {
        let mut cmd = Command::new(&self.exe);
        if !self.own {
            cmd.env_clear()
                .env("PATH", self.exe.parent().unwrap_or(Path::new(".")));
        }
        leash::prepare(&mut cmd);
        cmd.args(args)
            .envs(job.env.iter().map(|(k, v)| (k, v)))
            .current_dir(job.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            // merman finds the computer's fonts through WINDIR.
            for key in ["SystemRoot", "TEMP", "TMP", "WINDIR"] {
                if let Some(value) = env::var_os(key) {
                    cmd.env(key, value);
                }
            }
            // A hidden console costs a worker more time than its drawing does.
            cmd.creation_flags(match self.own {
                true => DETACHED_PROCESS,
                false => CREATE_NO_WINDOW,
            });
        }
        let shown = format!("{} {}", self.exe.display(), args.join(" "));
        self.log.push(shown.trim_end().into());
        let start = Instant::now();
        let mut child = cmd.spawn().map_err(|e| {
            self.log.push(format!("couldn't start: {e}"));
            Error::engine_unstartable(self.label, &e)
        })?;
        let pid = child.id();
        let leash = Leash::new(&child);
        if let Err(e) = &leash {
            self.log.push(format!("not tied to this process: {e}"));
        }
        let mut memory_out = false;
        let (stdin, out, err) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let ran = thread::scope(|s| {
            s.spawn(move || {
                // An engine may stop reading early, for example when it fails.
                let _ = stdin.map(|mut pipe| pipe.write_all(job.stdin));
            });
            let stdout = s.spawn(|| capped(out, STDOUT_CAP));
            let stderr = s.spawn(|| capped(err, STDERR_CAP));
            let status = loop {
                if let Some(status) = child.try_wait()? {
                    break Some(status);
                }
                memory_out = leash.as_ref().is_ok_and(|l| l.memory() > job.memory);
                if memory_out || start.elapsed() >= job.limit {
                    child.kill()?;
                    child.wait()?;
                    break None;
                }
                thread::sleep(Duration::from_millis(5));
            };
            let (stdout, stderr) = (stdout.join().unwrap()?, stderr.join().unwrap()?);
            io::Result::Ok(status.map(|status| (status, stdout, stderr)))
        });
        let ms = start.elapsed().as_millis();
        let (status, (stdout, over), (stderr, _)) = match ran {
            Ok(Some(ran)) => ran,
            Ok(None) if memory_out => {
                let mib = job.memory >> 20;
                self.log.push(format!("stopped over {mib} MiB (pid {pid})"));
                return Err(Error::out_of_memory(mib));
            }
            Ok(None) => {
                let s = job.limit.as_secs_f64();
                self.log.push(format!("stopped after {s} s (pid {pid})"));
                return Ok(None);
            }
            Err(e) => {
                self.log.push(format!("failed: {e} (pid {pid})"));
                return Err(Error::engine_unstartable(self.label, &e));
            }
        };
        let ran = Ran {
            code: status.code(),
            stdout,
            stderr,
        };
        self.log
            .push(format!("{} after {ms} ms (pid {pid})", exit(&ran)));
        let text = String::from_utf8_lossy(&ran.stderr);
        self.log.extend(text.lines().map(Into::into));
        if over {
            let said = format!("over {} MiB of output", STDOUT_CAP >> 20);
            return Err(Error::engine_failed(self.label, &said, None));
        }
        Ok(Some(ran))
    }

    /// `engine_failed` for a run that ended in a way no input explains.
    pub fn failed(&self, ran: &Ran) -> Error {
        let stderr = String::from_utf8_lossy(&ran.stderr);
        Error::engine_failed(self.label, &exit(ran), stderr.lines().next())
    }

    /// Runs the engine with `args` within the probe limit; any failure is `engine_failed`.
    fn checked(&mut self, args: &[&str]) -> Result<Ran, Error> {
        let job = Job {
            stdin: b"",
            dir: &folder(),
            env: vec![],
            limit: PROBE_LIMIT,
            memory: MEMORY_LIMIT,
        };
        match self.run(args, &job)? {
            Some(ran) if ran.code == Some(0) => Ok(ran),
            Some(ran) => Err(self.failed(&ran)),
            None => Err(Error::engine_slow(self.label, PROBE_LIMIT.as_secs())),
        }
    }
}

/// One diagram for an engine: its name in messages, its lines, and how to run the engine.
pub struct Task<'a> {
    pub name: &'a str,
    pub lines: Lines<'a>,
    pub job: Job<'a>,
    /// `white`, `transparent` or `#rrggbb`; `None` keeps the diagram's own.
    pub background: Option<&'a str>,
    /// The D2 board to render; `None` for the root.
    pub page: Option<&'a str>,
    /// Whether D2 may fetch remote images.
    pub allow_network: bool,
    /// What the run makes in the end.
    pub to: To,
}

/// A page of a diagram file: its number (from 1) when it has one, and its name.
#[derive(Serialize, Deserialize)]
pub struct Page {
    pub index: Option<usize>,
    pub name: String,
}

impl Page {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"index": self.index, "name": self.name})
    }
}

/// An SVG and what was warned about while making it; for files with pages, the one drawn and
/// every page; and the kind of diagram, when only drawing it tells.
#[derive(Default, Serialize, Deserialize)]
pub struct Rendered {
    #[serde(skip)]
    pub svg: Vec<u8>,
    pub warnings: Vec<Warning>,
    pub page: Option<Page>,
    pub pages: Vec<Page>,
    #[serde(deserialize_with = "kept_option")]
    pub diagram_type: Option<Code>,
    /// Unsafe items a renderer of ours left out before writing the SVG.
    pub removed: usize,
}

/// What a check found.
#[derive(Default, Serialize, Deserialize)]
pub struct Checked {
    pub errors: Vec<Error>,
    pub warnings: Vec<Warning>,
    #[serde(deserialize_with = "kept_option")]
    pub diagram_type: Option<Code>,
}

impl Engine {
    /// Runs the engine on `task`; stopped at the limit is `timeout`.
    pub fn go(&mut self, args: &[&str], task: &Task) -> Result<Ran, Error> {
        let ran = self.run(args, &task.job)?;
        ran.ok_or_else(|| Error::timeout(task.job.limit.as_secs()))
    }
}

/// Renders `task` to SVG with the engine for `format`, or with this binary's own renderer.
pub fn render(format: Format, engine: Option<&mut Engine>, task: &Task) -> Result<Rendered, Error> {
    match (format, engine) {
        (Format::Mermaid, Some(engine)) => mermaid::render(engine, task),
        (Format::Dot, Some(engine)) => dot::render(engine, task),
        (Format::D2, Some(engine)) => d2::render(engine, task),
        (_, Some(engine)) => worker::render(engine, format, task),
        _ => Err(Error::internal("no engine was found")),
    }
}

/// The PNG at `scale` of `svg`, which `render` made: drawn by merman or d2 for their own
/// diagrams, else by resvg.
pub fn png(
    format: Format,
    engine: Option<&mut Engine>,
    task: &Task,
    svg: &str,
    scale: f64,
    warnings: &mut Vec<Warning>,
) -> Result<Png, Error> {
    match (format, engine) {
        (Format::Mermaid, Some(engine)) => mermaid::png(engine, task, scale, warnings),
        (Format::D2, Some(engine)) => d2::png(engine, task, svg, scale),
        // Graphviz measures a family it has no widths for as Times.
        (_, Some(engine)) => {
            let serif = format == Format::Dot;
            worker::png(engine, task, svg, scale, serif, warnings)
        }
        _ => Err(Error::internal("no engine was found")),
    }
}

/// Checks `task` with the engine for `format`, or with this binary's own renderer, without
/// writing anything.
pub fn check(format: Format, engine: Option<&mut Engine>, task: &Task) -> Result<Checked, Error> {
    match (format, engine) {
        (Format::Mermaid, Some(engine)) => mermaid::check(engine, task),
        (Format::Dot, Some(engine)) => dot::check(engine, task),
        (Format::D2, Some(engine)) => d2::check(engine, task),
        (_, Some(engine)) => worker::check(engine, format, task),
        _ => Err(Error::internal("no engine was found")),
    }
}

/// `exit code 1`, or `killed by a signal`.
fn exit(ran: &Ran) -> String {
    ran.code
        .map_or("killed by a signal".into(), |c| format!("exit code {c}"))
}

/// What a child's pipe gives until it closes, up to `cap` bytes, and whether it gave more.
/// The rest is read and dropped, so the child never blocks on a full pipe.
fn capped(pipe: Option<impl Read>, cap: u64) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = vec![];
    let Some(mut pipe) = pipe else {
        return Ok((bytes, false));
    };
    pipe.by_ref().take(cap).read_to_end(&mut bytes)?;
    Ok((bytes, io::copy(&mut pipe, &mut io::sink())? > 0))
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
    let Some(found) = locate(format) else {
        return probe;
    };
    let version = match format {
        Format::Dot => "-V",
        _ => "--version",
    };
    let result = found.and_then(|mut engine| {
        let ran = engine.checked(&[version]).and_then(|ran| match format {
            Format::Mermaid => engine.checked(&["capabilities", "--json"]),
            _ => Ok(ran),
        });
        probe.log = std::mem::take(&mut engine.log);
        let caps: serde_json::Value = serde_json::from_slice(&ran?.stdout).unwrap_or_default();
        let families = caps["diagram_families"].as_array().into_iter().flatten();
        probe.families = (families.filter_map(|f| f.as_str().map(Into::into))).collect();
        Ok(())
    });
    probe.problem = result.err();
    probe
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An engine fetched into the target folder, beside the test binary's `deps/`.
    fn engine(file: &str, label: &'static str) -> Engine {
        let exe = env::current_exe().unwrap();
        let dir = exe.parent().unwrap().parent().unwrap().join("engines");
        Engine {
            exe: dir.join(format!("{file}{}", env::consts::EXE_SUFFIX)),
            label,
            log: vec![],
            own: false,
        }
    }

    fn job(stdin: &[u8], limit: Duration) -> Job<'_> {
        Job {
            stdin,
            dir: Path::new("."),
            env: vec![],
            limit,
            memory: MEMORY_LIMIT,
        }
    }

    #[test]
    fn an_engine_reads_stdin_and_answers_on_stdout() {
        let mut dot = engine("dot", "Graphviz");
        let ran = dot.run(&["-Tsvg"], &job(b"digraph { a -> b }", PROBE_LIMIT));
        let ran = ran.unwrap().unwrap();
        assert_eq!(ran.code, Some(0));
        assert!(String::from_utf8(ran.stdout).unwrap().contains("<svg"));
        assert!(
            dot.log[1].starts_with("exit code 0 after "),
            "{:?}",
            dot.log
        );
    }

    #[test]
    fn an_engine_past_its_limit_is_stopped() {
        // A dense random graph that dot needs minutes to lay out.
        let mut source = String::from("digraph {\n");
        let mut x: u32 = 1;
        for _ in 0..6000 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            source += &format!("n{} -> n{}\n", (x >> 4) % 3000, (x >> 16) % 3000);
        }
        source += "}\n";
        let mut dot = engine("dot", "Graphviz");
        let start = Instant::now();
        let ran = dot.run(
            &["-Tsvg"],
            &job(source.as_bytes(), Duration::from_millis(300)),
        );
        assert!(ran.unwrap().is_none());
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(
            dot.log[1].starts_with("stopped after 0.3 s (pid "),
            "{:?}",
            dot.log
        );
    }

    #[test]
    fn an_engine_past_the_memory_limit_is_stopped() {
        // Under 1 KB: each variable doubles the last, gigabytes by the end.
        let mut d2 = String::from("vars: {\n  a0: xxxxxxxxxxxxxxxx\n");
        for i in 1..32 {
            d2 += &format!("  a{i}: ${{a{}}}${{a{}}}\n", i - 1, i - 1);
        }
        d2 += "}\nb: ${a31}\n";
        let cases = [
            ("d2", "D2", vec!["-", "-"], d2),
            (
                "dot",
                "Graphviz",
                vec!["-Tsvg"],
                "digraph { a [peripheries=100000000] }".into(),
            ),
        ];
        for (file, label, args, source) in cases {
            let mut run = engine(file, label);
            let job = Job {
                memory: 200 << 20,
                ..job(source.as_bytes(), Duration::from_secs(30))
            };
            let e = run.run(&args, &job).err().expect(file);
            assert_eq!(e.code, "too_large", "{file}: {}", e.message);
        }
    }

    #[test]
    fn output_past_the_cap_is_counted_and_dropped() {
        let abcd = b"abcd".to_vec();
        assert_eq!(
            capped(Some(&b"abcdef"[..]), 4).unwrap(),
            (abcd.clone(), true)
        );
        assert_eq!(capped(Some(&b"abcd"[..]), 4).unwrap(), (abcd, false));
        assert_eq!(capped(None::<&[u8]>, 4).unwrap(), (vec![], false));
    }

    #[test]
    fn a_missing_executable_cant_start() {
        let mut gone = engine("no-such-engine", "D2");
        let e = gone.run(&[], &job(b"", PROBE_LIMIT)).err().unwrap();
        assert_eq!(e.code, "engine_failed");
        assert!(e.message.starts_with("The D2 engine couldn't start: "));
    }
}
