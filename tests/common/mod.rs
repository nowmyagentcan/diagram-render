//! Running the built binary in tests, the way people and agents do.
#![allow(dead_code)] // each test file uses some of these

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

pub fn cmd(args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_diagram-render"));
    cmd.args(args).env_remove("NO_COLOR").stdin(Stdio::null());
    cmd
}

/// Exit code, stdout and stderr.
pub fn run(args: &[&str]) -> (i32, String, String) {
    let out = cmd(args).output().unwrap();
    let text = |b| String::from_utf8(b).unwrap();
    (
        out.status.code().unwrap(),
        text(out.stdout),
        text(out.stderr),
    )
}

/// The `--json` answer: one line on stdout, nothing on stderr.
pub fn json(args: &[&str]) -> (i32, Value) {
    let (code, out, err) = run(args);
    assert_eq!(err, "", "{args:?}");
    assert_eq!(out.matches('\n').count(), 1, "one line: {out}");
    (code, serde_json::from_str(&out).unwrap())
}

/// A pipe whose reading end is already closed, so every write to it fails.
pub fn closed() -> Stdio {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    writer.into()
}

/// A fresh, empty folder for one test.
pub fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("diagram-render-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs in `dir` with `stdin` as input: exit code, stdout and stderr.
pub fn run_in(dir: &Path, args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    feed(cmd(args), dir, stdin)
}

/// Runs `cmd` in `dir` with `stdin` as input: exit code, stdout and stderr.
pub fn feed(mut cmd: Command, dir: &Path, stdin: &[u8]) -> (i32, String, String) {
    use std::io::Write;
    let mut child = (cmd.current_dir(dir).stdin(Stdio::piped()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    let stdin = stdin.to_vec();
    let feeder = std::thread::spawn(move || {
        let _ = pipe.write_all(&stdin);
    });
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    let text = |b| String::from_utf8(b).unwrap();
    (
        out.status.code().unwrap(),
        text(out.stdout),
        text(out.stderr),
    )
}

/// A copy of the binary in a folder of its own, whose `engines/` holds `engines` (each a copy of
/// the binary under that name, so it answers `--version` but fails anything else).
pub fn installed(name: &str, engines: &[&str]) -> PathBuf {
    let dir = folder(name);
    let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let bin = dir.join(exe("diagram-render"));
    std::fs::copy(env!("CARGO_BIN_EXE_diagram-render"), &bin).unwrap();
    std::fs::create_dir(dir.join("engines")).unwrap();
    for engine in engines {
        std::fs::copy(&bin, dir.join("engines").join(exe(engine))).unwrap();
    }
    // The binary resolves its own path (macOS temp folders sit behind a /var symlink).
    match cfg!(windows) {
        true => bin,
        false => bin.canonicalize().unwrap(),
    }
}

/// Runs the copy `bin` in its own folder.
pub fn run_at(bin: &Path, args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    let mut cmd = Command::new(bin);
    cmd.args(args).env_remove("NO_COLOR");
    feed(cmd, bin.parent().unwrap(), stdin)
}
