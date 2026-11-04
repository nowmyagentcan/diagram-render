//! Running the built binary in tests, the way people and agents do.
#![allow(dead_code)] // each test file uses some of these

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// A fresh, empty folder, never shared with a test running beside this one.
pub fn folder(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let id = std::process::id();
    let dir = std::env::temp_dir().join(format!("diagram-render-{name}-{id}-{n}"));
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
    (cmd.current_dir(dir).stdin(Stdio::piped()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A fresh copy of the binary is busy on Linux while a sibling test's fork still holds its
    // write handle, until that child's exec closes it.
    let mut child = loop {
        match cmd.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            spawned => break spawned.unwrap(),
        }
    };
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

/// Grey levels (0–255) of an image drawn on white.
pub fn grey(png: &[u8]) -> (usize, usize, Vec<f64>) {
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(png).unwrap();
    let levels = (pixmap.pixels().iter())
        .map(|p| {
            // Premultiplied, so white shows through what is left of the alpha.
            let white = f64::from(255 - p.alpha());
            let (r, g, b) = (p.red(), p.green(), p.blue());
            0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b) + white
        })
        .collect();
    (pixmap.width() as usize, pixmap.height() as usize, levels)
}

/// `image` (`w` wide) blurred with a Gaussian of σ = `sigma` px, edges held at their last pixel;
/// it comes out turned a quarter, which leaves differences between images as they were.
pub fn blurred(w: usize, h: usize, image: &[f64], sigma: i32) -> Vec<f64> {
    let rows = blur_rows(w, image, sigma);
    let turned: Vec<f64> = (0..w * h).map(|i| rows[(i % h) * w + i / h]).collect();
    blur_rows(h, &turned, sigma)
}

fn blur_rows(w: usize, image: &[f64], sigma: i32) -> Vec<f64> {
    let reach = 3 * sigma;
    let kernel: Vec<f64> = (-reach..=reach)
        .map(|i| (-f64::from(i * i) / f64::from(2 * sigma * sigma)).exp())
        .collect();
    let total: f64 = kernel.iter().sum();
    let at =
        |row: &[f64], i: usize, k: usize| row[(i + k).saturating_sub(reach as usize).min(w - 1)];
    (image.chunks(w))
        .flat_map(|row| {
            (0..w).map(|i| {
                let sum: f64 = kernel
                    .iter()
                    .enumerate()
                    .map(|(k, v)| v * at(row, i, k))
                    .sum();
                sum / total
            })
        })
        .collect()
}

/// The mean difference of two same-size PNGs drawn on white, grey and blurred, as a share of
/// full scale.
pub fn blurred_difference(ours: &[u8], reference: &[u8], name: &str, sigma: i32) -> f64 {
    let (w, h, ours) = grey(ours);
    let (rw, rh, reference) = grey(reference);
    assert_eq!((w, h), (rw, rh), "{name}: framed as the reference");
    let (a, b) = (
        blurred(w, h, &ours, sigma),
        blurred(w, h, &reference, sigma),
    );
    a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64 / 255.0
}

/// Copies `source` as `file` into `dir` and runs `args` on it there with `--json`.
pub fn in_dir(dir: &Path, file: &str, source: &[u8], args: &[&str]) -> (i32, Value) {
    std::fs::write(dir.join(file), source).unwrap();
    let (code, out, err) = run_in(dir, &[&[file, "--json"], args].concat(), b"");
    assert_eq!(err, "", "{file} {args:?}");
    (code, serde_json::from_str(&out).unwrap())
}

/// The codes of a result's warnings.
pub fn codes(v: &Value) -> Vec<&str> {
    let all = v["warnings"].as_array().unwrap();
    all.iter().map(|w| w["code"].as_str().unwrap()).collect()
}

/// `large.mmd`: a flowchart of 300 nodes and 450 edges in 10 subgraphs.
pub fn large_mermaid() -> String {
    let mut text = String::from("flowchart TD\n");
    for group in 0..10 {
        text += &format!("  subgraph g{group}\n");
        for i in group * 30..(group + 1) * 30 {
            text += &format!("    n{i}[Node {i}]\n");
        }
        text += &format!("  end\n");
    }
    for i in 0..299 {
        text += &format!("  n{i} --> n{}\n", i + 1);
    }
    for i in 0..151 {
        text += &format!("  n{} --> n{}\n", i * 2 % 300, (i * 7 + 13) % 300);
    }
    text
}
