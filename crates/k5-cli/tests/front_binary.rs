//! The built `k5-cli` binary against the shell CLI it replaces: the
//! version marker in its bytes, `--version` printed as `bash cli/k2
//! --version` prints it, and a verb that falls through answering exactly
//! as `bash cli/k2` does, through the cache under a temporary K2 home.
//!
//! Every run gets an empty environment but `PATH`, a temporary `HOME`
//! and a dead port with a dummy token (as tests/cli/_hermetic_cli.sh does),
//! so nothing reads or writes a real K2 home or reaches a daemon.

#![cfg(unix)]

mod common;

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");

/// `cli/k2` in this checkout.
fn cli_k2() -> PathBuf {
    let p = common::cli_src().join("../k2");
    assert!(p.is_file(), "no shell CLI at {}", p.display());
    p
}

/// The version on `cli/k2`'s `K2_CLI_VERSION="x.y.z"` line, read here
/// and not from the crate, so the two are checked against each other.
fn script_version() -> String {
    let text = std::fs::read_to_string(cli_k2()).expect("cli/k2 is UTF-8");
    let line = text
        .lines()
        .take(40)
        .find_map(|l| l.strip_prefix("K2_CLI_VERSION=\""))
        .expect("cli/k2 has a K2_CLI_VERSION line near the top");
    line.strip_suffix('"')
        .expect("the version is quoted")
        .to_string()
}

fn blake3_hex(bytes: &[u8]) -> String {
    k5_cli::fallthrough::blake3_hex(bytes)
}

static N: AtomicUsize = AtomicUsize::new(0);

/// A temporary folder with `bin/k2` and `bin/k5` linked to the binary (so
/// its `argv[0]` is a real name) and an empty `home`.
struct Rig {
    dir: PathBuf,
    home: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!(
            "k5-front-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        for name in ["k2", "k5"] {
            std::os::unix::fs::symlink(BIN, dir.join("bin").join(name)).unwrap();
        }
        Rig { dir, home }
    }

    fn env(&self, cmd: &mut Command, extra: &[(&str, &str)]) {
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", &self.home)
            .env("K2_PORT", "1")
            .env("K2_HOOK_TOKEN", "test-token");
        cmd.envs(extra.iter().copied());
    }

    /// The binary, called as `name`.
    fn front(&self, name: &str, args: &[OsString], extra: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new(self.dir.join("bin").join(name));
        self.env(&mut cmd, extra);
        Out::of(cmd.args(args))
    }

    /// `bash cli/k2`.
    fn bash(&self, args: &[OsString], extra: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new("bash");
        self.env(&mut cmd, extra);
        Out::of(cmd.arg(cli_k2()).args(args))
    }

    /// The cache folder under the default K2 home.
    fn cache(&self) -> PathBuf {
        k5_cli::fallthrough::cache_dir(&k2_client::paths::k2_home_in(&self.home))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn of(cmd: &mut Command) -> Out {
        let out = cmd.output().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
        Out {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn the_binary_carries_the_version_marker() {
    let bytes = std::fs::read(BIN).unwrap_or_else(|e| panic!("read {BIN}: {e}"));
    let marker = format!("K5_CLI_VERSION={}\0", script_version());
    assert!(
        bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
        "{marker:?} is not in the bytes of {BIN}"
    );
    // The install code's check, on the built binary: read, never run.
    assert_eq!(
        k2_client::cli_install::binary_versions(&bytes),
        vec![script_version()]
    );
    k2_client::cli_install::check_binary(std::path::Path::new(BIN), Some(&script_version()))
        .unwrap_or_else(|e| panic!("the built binary fails the install check: {e}"));
    assert!(k2_client::cli_install::is_k5_binary(std::path::Path::new(
        BIN
    )));
    let other = k2_client::cli_install::check_binary(std::path::Path::new(BIN), Some("0.0.1"))
        .expect_err("another version must fail");
    assert!(other.contains("0.0.1"), "{other}");
}

#[test]
fn version_prints_what_the_shell_prints() {
    let rig = Rig::new("version");
    for one in ["--version", "-v", "version"] {
        let want = rig.bash(&args(&[one]), &[]);
        assert_eq!(want.code, Some(0), "bash cli/k2 {one}: {want:?}");
        assert_eq!(
            want.stdout,
            format!("k2 {}\n", script_version()),
            "bash cli/k2 {one}"
        );
        for name in ["k2", "k5"] {
            assert_eq!(rig.front(name, &args(&[one]), &[]), want, "{name} {one}");
        }
    }
    // Answered here: the shell CLI was never written out.
    assert!(
        !rig.home.join(".k2").exists(),
        "--version wrote under the K2 home"
    );
}

/// A known family's line the front hands to the shell: the first form of
/// the native bar's ceiling (`tests/data/native_bar_ceiling.tsv`, the
/// probes k5 still hands to bash) that the front routes to the shell, so a
/// lane that makes one native moves the pick on by itself. A run-time row
/// (class R) is a form under a variant, not a line of its own: never
/// picked. With an empty
/// ceiling (every probe native), a help word on a line that is no help
/// form, `mail create x --help`, which the front still hands over. It
/// fails loudly when the front answers that too.
fn shell_family_line() -> Vec<OsString> {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/native_bar_ceiling.tsv"
    );
    let text = std::fs::read_to_string(file).expect("read the native bar's ceiling");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("class\t") && !l.starts_with("R\t"))
        .filter_map(|l| l.split('\t').nth(2))
        .map(|form| form.split(' ').map(OsString::from).collect::<Vec<_>>())
        .chain(std::iter::once(args(&["mail", "create", "x", "--help"])))
        .find(|line| k5_cli::front::route(line) == k5_cli::front::Route::Fallthrough)
        .expect("no ceiling form nor mail create x --help falls through: pick another line")
}

/// A line the front hands to the shell, which needs no daemon: an
/// unknown verb with a word after it that is not UTF-8 (the binary
/// answers only UTF-8 lines). bash prints `Unknown command` and its help,
/// exit 1. The tests that use it test the hand-over, so it must stay one.
fn unknown() -> Vec<OsString> {
    let line = vec![
        OsString::from("nosuchverb"),
        OsString::from_vec(b"x\xff".to_vec()),
    ];
    assert_eq!(
        k5_cli::front::route(&line),
        k5_cli::front::Route::Fallthrough,
        "the binary answers {line:?} itself now: pick another fall-through line"
    );
    line
}

#[test]
fn a_verb_falls_through_and_answers_as_the_shell_does() {
    let rig = Rig::new("fall");
    let mut odd = b"no\xffsuch verb".to_vec();
    odd.push(b'\n');
    let cases: Vec<Vec<OsString>> = vec![
        unknown(),
        args(&["study", "app-api"]),
        args(&["version", "extra"]),
        // A word after help's that is not cmd_help's is the binary's;
        // two are the shell's.
        args(&["help", "nope", "x"]),
        vec![OsString::from_vec(odd)],
    ];
    for case in &cases {
        let want = rig.bash(case, &[]);
        assert_eq!(rig.front("k2", case, &[]), want, "k2 {case:?}");
        assert_eq!(rig.front("k5", case, &[]), want, "k5 {case:?}");
    }
    // The unknown verb's exit code came through.
    assert_eq!(rig.front("k2", &unknown(), &[]).code, Some(1));

    // The cache: one file named by the hash, 0755, in a 0700 folder, and
    // it is cli/k2.
    let script = std::fs::read(cli_k2()).unwrap();
    let digest = blake3_hex(&script);
    let file = rig.cache().join(format!("k2-{}.sh", &digest[..12]));
    assert_eq!(
        std::fs::read(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display())),
        script
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::metadata(rig.cache()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let names: Vec<_> = std::fs::read_dir(rig.cache())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn a_changed_cache_file_is_never_run() {
    let rig = Rig::new("tamper");
    let want = rig.bash(&unknown(), &[]);
    let first = rig.front("k2", &unknown(), &[]);
    assert_eq!(first, want);
    let file = std::fs::read_dir(rig.cache())
        .unwrap()
        .next()
        .expect("a cache file")
        .unwrap()
        .path();
    std::fs::write(&file, "#!/bin/bash\necho PWNED\nexit 42\n").unwrap();
    let ino = std::fs::metadata(&file).unwrap().ino();
    assert_eq!(rig.front("k2", &unknown(), &[]), want);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        std::fs::read(cli_k2()).unwrap()
    );
    assert_ne!(
        std::fs::metadata(&file).unwrap().ino(),
        ino,
        "replaced by a new file, not edited in place"
    );
}

#[test]
fn the_cache_follows_the_home_override() {
    let rig = Rig::new("override");
    let over = rig.dir.join("k2home");
    let over_s = over.to_str().unwrap().to_string();
    let want = rig.bash(&unknown(), &[]);
    assert_eq!(
        rig.front("k2", &unknown(), &[("K2_HOME_OVERRIDE", &over_s)]),
        want
    );
    assert!(k5_cli::fallthrough::cache_dir(&over).is_dir());
    assert!(
        !rig.home.join(".k2").exists(),
        "the cache went to the user home, not the override"
    );
}

#[test]
fn a_cache_that_cannot_be_written_is_one_line_and_exit_1() {
    let rig = Rig::new("readonly");
    // The K2 home is a file: nothing can be made under it.
    let blocked = rig.dir.join("blocked");
    std::fs::write(&blocked, "").unwrap();
    let blocked_s = blocked.to_str().unwrap().to_string();
    let out = rig.front("k5", &unknown(), &[("K5_HOME_OVERRIDE", &blocked_s)]);
    assert_eq!(out.code, Some(1), "{out:?}");
    assert_eq!(out.stdout, "");
    // The command's own name, whichever name ran it.
    let want = format!(
        "{}: can't write the shell CLI to {}/run/cli: ",
        k2_client::brand::CLI_NAME,
        blocked.display()
    );
    assert!(out.stderr.starts_with(&want), "{out:?}");
    assert_eq!(out.stderr.lines().count(), 1, "{out:?}");
}

/// The front adds its three variables and bash reads them; bash's `$0` is
/// `/dev/fd/N`, the open cache file, and the shell closes N. Shown by the
/// shell's own trace (bash takes `SHELLOPTS=xtrace` from the environment,
/// which the front passes on).
#[test]
fn bash_runs_the_open_cache_file_and_the_front_s_variables_arrive() {
    let rig = Rig::new("dollar0");
    // A value left by some other call is replaced by the front's own.
    let out = rig.front(
        "k5",
        &unknown(),
        &[("SHELLOPTS", "xtrace"), ("K5_CLI_SCRIPT_DIR", "/stale")],
    );
    assert_eq!(out.code, Some(1), "{}", out.stdout);
    let lines: Vec<&str> = out
        .stderr
        .lines()
        .map(|l| l.trim_start_matches('+'))
        .collect();
    let traced = |line: &str| lines.iter().any(|l| *l == format!(" {line}"));
    // `$0` is /dev/fd/N, and the shell closes N, a number of 3 or more.
    let n = lines
        .iter()
        .find_map(|l| l.strip_prefix(" eval 'exec ")?.strip_suffix("<&-'"))
        .unwrap_or_else(|| panic!("no exec N<&- in the trace:\n{}", out.stderr));
    assert!(
        n.parse::<u32>().is_ok_and(|n| n >= 3),
        "$0 is not /dev/fd/N: closed {n}"
    );
    // The folder of the binary as run, links resolved: the rig's links
    // point at the build.
    let dir = std::fs::canonicalize(BIN)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    for line in [
        "_K5_NOTICE_DONE=1".to_string(),
        "_K5_CLI_VIA=rust".to_string(),
        format!("_K5_SCRIPT_DIR={}", dir.display()),
        "unset K5_CLI_SCRIPT_DIR".to_string(),
        format!("eval 'exec {n}<&-'"),
    ] {
        assert!(traced(&line), "no `{line}` in the trace:\n{}", out.stderr);
    }
    // Run straight, the shell sees none of them and closes nothing.
    let plain = rig.bash(&unknown(), &[("SHELLOPTS", "xtrace")]);
    let traced = |line: &str| {
        plain
            .stderr
            .lines()
            .any(|l| l.trim_start_matches('+') == format!(" {line}"))
    };
    assert!(
        traced("_K5_SCRIPT_DIR=") && !traced("_K5_CLI_VIA=rust"),
        "{}",
        plain.stderr
    );
    // The shell reads no name it was called by, through the front or not.
    for trace in [&out.stderr, &plain.stderr] {
        assert!(!trace.contains("_K2_CALLED_AS"), "{trace}");
    }
    assert!(!plain.stderr.contains("exec 3<&-"), "{}", plain.stderr);
}

#[test]
fn a_verb_that_falls_through_without_python3_says_so_and_runs_nothing() {
    let rig = Rig::new("no-python3");
    let path = common::path_without_python3(&rig.dir);
    for (name, case) in [
        ("k5", shell_family_line()),
        ("k2", shell_family_line()),
        ("k5", unknown()),
    ] {
        let noun = k5_cli::fallthrough::noun(&case);
        let line = k5_cli::fallthrough::FrontError::NoPython { noun }.to_string();
        assert_eq!(
            rig.front(name, &case, &[("PATH", &path)]),
            Out {
                code: Some(69),
                stdout: String::new(),
                stderr: format!("{line}\n"),
            },
            "{name} {case:?}"
        );
        assert!(line.contains("needs Python 3"), "{line}");
        // bash never ran: the shell CLI was never written out.
        assert!(!rig.cache().exists(), "{name} {case:?} wrote the shell CLI");
    }
    // With python3 the same verb goes to the shell.
    let with = rig.front("k5", &unknown(), &[]);
    assert_eq!(with, rig.bash(&unknown(), &[]));
    assert!(rig.cache().exists(), "the shell CLI was not written out");
}
