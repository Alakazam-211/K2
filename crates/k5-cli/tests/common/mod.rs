//! What the parity tests share: the shell CLI's source, read from the
//! repo, a way to run a piece of it under bash, a logging stub daemon
//! ([`stub`]), the rig a native family's parity test runs on
//! ([`parity`]), and the permission cards probe's rows ([`cards`]).
//!
//! The source is `cli/src`, found from this crate's folder. It must be
//! there: a test that can't read it fails, it never skips.

#![allow(dead_code)]

pub mod cards;
pub mod parity;
pub mod stub;
pub mod vault;

use std::path::PathBuf;

/// `cli/src` in this checkout.
pub fn cli_src() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/src");
    assert!(
        dir.join("prelude.sh").is_file(),
        "the shell CLI's source is not at {}",
        dir.display()
    );
    dir
}

/// Every `cli/src/*.sh`, by name, with its text.
pub fn shell_sources() -> Vec<(String, String)> {
    let dir = cli_src();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .path();
        if path.extension().is_some_and(|x| x == "sh") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let name = path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            out.push((name, text));
        }
    }
    out.sort();
    assert!(
        out.len() > 50,
        "only {} files in {}: not the shell CLI's source",
        out.len(),
        dir.display()
    );
    out
}

/// `cli/k2`, the built shell CLI (what the binary carries).
pub fn cli_script() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `cli/src/router.sh`, the shell router.
pub fn router() -> String {
    let path = cli_src().join("router.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `cli/src/schema-head.sh`, which lists the router words left out of
/// `--schema` on purpose.
pub fn schema_head() -> String {
    let path = cli_src().join("schema-head.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `cli/src/prelude.sh`.
pub fn prelude() -> String {
    let path = cli_src().join("prelude.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The lines of `text` from the one line that is exactly `first` to the
/// first line after it that is exactly `last`, both kept, joined with
/// newlines. Fails unless `first` is there exactly once.
pub fn block(text: &str, first: &str, last: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == first)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "the line {first:?} is there {} times, wanted once",
        starts.len()
    );
    let start = starts[0];
    let end = lines[start..]
        .iter()
        .position(|l| *l == last)
        .unwrap_or_else(|| panic!("no line {last:?} after {first:?}"));
    lines[start..=start + end].join("\n")
}

/// Runs `script` under `bash -c` with only `env` (and `PATH`) set and `$0`
/// set to `argv0`; returns its stdout. Fails on any error or stderr.
#[cfg(unix)]
pub fn bash(script: &str, argv0: &str, env: &[(&str, &str)]) -> String {
    run_bash(script, argv0, &[], env)
}

/// Runs `script` under `bash -c` with `$1...` set to `args`, nothing in
/// its environment but `PATH`; returns its stdout.
#[cfg(unix)]
pub fn bash_args(script: &str, args: &[&str]) -> String {
    run_bash(script, "bash", args, &[])
}

#[cfg(unix)]
fn run_bash(script: &str, argv0: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = std::process::Command::new("bash");
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.envs(env.iter().copied())
        .arg("-c")
        .arg(script)
        .arg(argv0)
        .args(args);
    let out = cmd.output().unwrap_or_else(|e| panic!("run bash: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stderr.is_empty(),
        "bash failed ({}): {stderr}\nscript:\n{script}",
        out.status
    );
    String::from_utf8(out.stdout).expect("bash printed UTF-8")
}

/// This process's `PATH` with every `python3` taken out: each folder that
/// has one is replaced by a folder under `scratch` with a link to each of
/// its other entries, so every other program is found as before. The
/// binary's rule ([`k5_cli::python::in_env`]) must then find none.
#[cfg(unix)]
pub fn path_without_python3(scratch: &std::path::Path) -> String {
    let path = std::env::var_os("PATH").expect("a PATH to run the tests with");
    let mut dirs = Vec::new();
    for (i, dir) in std::env::split_paths(&path).enumerate() {
        if std::fs::symlink_metadata(dir.join("python3")).is_err() {
            dirs.push(dir);
            continue;
        }
        let shadow = scratch.join(format!("no-python3-{i}"));
        if !shadow.exists() {
            std::fs::create_dir_all(&shadow).unwrap();
            let entries =
                std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
            for entry in entries {
                let name = entry
                    .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
                    .file_name();
                if name != "python3" {
                    std::os::unix::fs::symlink(dir.join(&name), shadow.join(&name)).unwrap();
                }
            }
        }
        dirs.push(shadow);
    }
    let joined = std::env::join_paths(dirs)
        .expect("the folders join")
        .into_string()
        .expect("a UTF-8 PATH");
    assert_eq!(
        k5_cli::python::in_env(&k5_cli::offline::Env::of(&[("PATH", &joined)])),
        k5_cli::python::Python3::Missing,
        "python3 is still on {joined}"
    );
    joined
}
