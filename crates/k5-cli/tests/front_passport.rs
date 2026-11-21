//! The built `k5-cli` binary and the passport rule, end to end: a verb
//! that falls through reaches a stub daemon, and its request carries
//! `X-K5-Would-Refuse: 1` only when there is no passport inside an agent
//! session (here `CLAUDECODE=1`). Offline verbs and `--version` never
//! count, and a value the caller set never stands.
//!
//! Each run is detached (a subshell whose parent exits, so it is adopted
//! away from this test), so the walk up from the binary never meets the
//! process that runs the tests, whatever started it. Every run gets an
//! empty environment but `PATH`, a temporary `HOME` with an owner token
//! file, and the stub's port.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");

/// One request the stub saw: its path, and its `X-K5-Would-Refuse` and
/// `X-K5-Cli-Via` values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    path: String,
    refuse: Vec<String>,
    via: Vec<String>,
}

/// A daemon that answers every request with a small JSON body and writes
/// down what it saw.
struct Stub {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    fn start() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let path = first
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let (mut refuse, mut via, mut length) = (Vec::new(), Vec::new(), 0usize);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let Some((k, v)) = line.split_once(':') else {
                        continue;
                    };
                    let v = v.trim().to_string();
                    match k.to_ascii_lowercase().as_str() {
                        "x-k5-would-refuse" => refuse.push(v),
                        "x-k5-cli-via" => via.push(v),
                        "content-length" => length = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                log.lock().unwrap().push(Seen { path, refuse, via });
                let data = br#"{"ok":true,"success":true,"heartbeats":[]}"#;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(data);
            }
        });
        Stub { port, seen }
    }

    fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

struct Rig {
    dir: PathBuf,
    stub: Stub,
}

#[derive(Debug)]
struct Out {
    code: String,
    stdout: String,
    stderr: String,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("k5-passport-{tag}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        for sub in ["bin", "home/.k2", "proj", "run"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::os::unix::fs::symlink(BIN, dir.join("bin/k2")).unwrap();
        std::fs::write(dir.join("home/.k2/heartbeat.token"), "owner-test-token\n").unwrap();
        Rig {
            dir,
            stub: Stub::start(),
        }
    }

    /// The binary as `k2 <args>`, detached, with `extra` in its
    /// environment; waits for it to end.
    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> Out {
        let run = self.dir.join("run");
        for f in ["out", "err", "rc"] {
            let _ = std::fs::remove_file(run.join(f));
        }
        let mut cmd = Command::new("sh");
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", self.dir.join("home"))
            .env("K2_PORT", self.stub.port.to_string())
            .env("K2_PROJECT_PATH", self.dir.join("proj"))
            .envs(extra.iter().copied())
            .current_dir(self.dir.join("proj"))
            .arg("-c")
            .arg(r#"o="$1"; shift; ( "$@" >"$o/out" 2>"$o/err" </dev/null; echo $? >"$o/rc.tmp"; mv "$o/rc.tmp" "$o/rc" ) & exit 0"#)
            .arg("sh")
            .arg(&run)
            .arg(self.dir.join("bin/k2"))
            .args(args);
        let status = cmd.status().expect("run sh");
        assert!(status.success(), "{status}");
        let deadline = Instant::now() + Duration::from_secs(60);
        while !run.join("rc").exists() {
            assert!(Instant::now() < deadline, "k2 {args:?} never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        let read = |p: &Path| {
            std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        Out {
            code: read(&run.join("rc")).trim().to_string(),
            stdout: read(&run.join("out")),
            stderr: read(&run.join("err")),
        }
    }

    /// The would-refuse values of every request to `path` in this run.
    fn refuse(&self, args: &[&str], extra: &[(&str, &str)], path: &str) -> Vec<Vec<String>> {
        let out = self.run(args, extra);
        let seen = self.stub.take();
        let hits: Vec<&Seen> = seen.iter().filter(|s| s.path == path).collect();
        assert!(!hits.is_empty(), "no request to {path}: {seen:?}\n{out:?}");
        for s in &hits {
            assert_eq!(s.via, ["fallthrough"], "{s:?}");
        }
        hits.iter().map(|s| s.refuse.clone()).collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Lines that each make one request to the daemon when the shell answers
/// them, with that request's path. The counted tests use the first one
/// the front still hands to the shell ([`shell_line`]), so a lane that
/// makes one native moves the pick on by itself.
const CANDIDATES: &[(&[&str], &str)] = &[
    (&["heartbeat", "list", "--json"], "/cli/heartbeat/list"),
    (&["publish", "ps", "--json"], "/cli/publish/list"),
    (&["release", "--agent", "ada"], "/cli/release"),
    (&["reserve", "--agent", "ada", "src"], "/cli/reserve"),
    // `setup agent-reports` is native now; the agent hire reads are still
    // the shell's: one GET each through cli_request.
    (&["agent", "team", "--json"], "/cli/agent-teams/links"),
    (&["agent", "hire-root", "--json"], "/cli/agents/hire-root"),
];

/// The hand-off's line when there is no `python3`
/// ([`k5_cli::fallthrough::FrontError::NoPython`]).
const NO_PYTHON: &str = "is not built into k5 yet and needs Python 3";

/// Whether the built binary hands `line` to the shell: run with no
/// `python3` on `PATH` (an empty folder), every hand-off stops on its one
/// line and exit 69 before bash starts, as the native bar probes it. The
/// route table alone can't say: a native family may still hand a verb
/// back at run time.
fn falls_through(line: &[&str]) -> bool {
    let dir = std::env::temp_dir().join(format!("k5-front-passport-probe-{}", std::process::id()));
    let (bin, home) = (dir.join("bin"), dir.join("home"));
    std::fs::create_dir_all(&bin).expect("the probe's empty PATH");
    std::fs::create_dir_all(&home).expect("the probe's HOME");
    let mut child = Command::new(BIN)
        .args(line)
        .env_clear()
        .env("PATH", &bin)
        .env("HOME", &home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run the binary");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(st) = child.try_wait().expect("wait for the binary") {
            break Some(st);
        }
        if Instant::now() > deadline {
            // Still running: it is in its own code, so native.
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut err = String::new();
    child
        .stderr
        .take()
        .expect("the binary's stderr")
        .read_to_string(&mut err)
        .expect("read the binary's stderr");
    status.is_some_and(|st| st.code() == Some(69)) && err.contains(NO_PYTHON)
}

/// The first candidate the binary still hands to the shell; it fails
/// loudly when every one is native.
fn shell_line() -> (&'static [&'static str], &'static str) {
    for &(line, path) in CANDIDATES {
        if falls_through(line) {
            return (line, path);
        }
    }
    panic!("every line in CANDIDATES is native now: add one the shell still answers");
}

fn list() -> &'static [&'static str] {
    shell_line().0
}

fn path() -> &'static str {
    shell_line().1
}

#[test]
fn no_passport_inside_a_session_is_counted() {
    let rig = Rig::new("counted");
    for marker in ["CLAUDECODE", "K2_PANE_ID", "K5_CELL", "CODEX_THREAD_ID"] {
        assert_eq!(
            rig.refuse(list(), &[(marker, "1")], path()),
            [["1"]],
            "{marker}"
        );
    }
}

#[test]
fn a_passport_is_never_counted() {
    let rig = Rig::new("passport");
    let (line, path) = shell_line();
    for name in ["K2_HOOK_TOKEN", "K5_HOOK_TOKEN", "K2SO_HOOK_TOKEN"] {
        assert_eq!(
            rig.refuse(line, &[("CLAUDECODE", "1"), (name, "test-token")], path),
            [Vec::<String>::new()],
            "{name}"
        );
    }
}

#[test]
fn outside_a_session_nothing_is_counted_whatever_the_caller_set() {
    let rig = Rig::new("outside");
    assert_eq!(rig.refuse(list(), &[], path()), [Vec::<String>::new()]);
    assert_eq!(
        rig.refuse(list(), &[("K5_CLI_WOULD_REFUSE", "1")], path()),
        [Vec::<String>::new()]
    );
    // An empty marker counts as unset.
    assert_eq!(
        rig.refuse(list(), &[("CLAUDECODE", "")], path()),
        [Vec::<String>::new()]
    );
}

#[test]
fn no_owner_fallback_is_left_to_the_shell() {
    let rig = Rig::new("nofb");
    let line = list();
    let out = rig.run(line, &[("CLAUDECODE", "1"), ("K2_NO_OWNER_FALLBACK", "1")]);
    assert_eq!(out.code, "3", "{out:?}");
    assert!(
        out.stderr.contains("won't act as this server's owner"),
        "{out:?}"
    );
    assert!(rig.stub.take().is_empty());
}

#[test]
fn offline_verbs_and_the_version_never_count() {
    let rig = Rig::new("offline");
    let out = rig.run(&["--version"], &[("CLAUDECODE", "1")]);
    assert_eq!(out.code, "0", "{out:?}");
    assert!(out.stdout.starts_with("k2 "), "{out:?}");
    // help runs in the shell: its trace shows the variable never came.
    for args in [&["help"][..], &["msg", "--help"], &[]] {
        let out = rig.run(args, &[("CLAUDECODE", "1"), ("SHELLOPTS", "xtrace")]);
        let traced = |line: &str| {
            out.stderr
                .lines()
                .any(|l| l.trim_start_matches('+') == format!(" {line}"))
        };
        assert!(
            traced("_K5_CLI_VIA=rust"),
            "{args:?}: not through the front:\n{}",
            out.stderr
        );
        assert!(
            traced("_K5_WOULD_REFUSE="),
            "{args:?}: no read of the variable:\n{}",
            out.stderr
        );
        assert!(
            !traced("_K5_WOULD_REFUSE=1"),
            "{args:?}: counted:\n{}",
            out.stderr
        );
    }
    // And the same trace does show it for a verb that is counted.
    let out = rig.run(list(), &[("CLAUDECODE", "1"), ("SHELLOPTS", "xtrace")]);
    assert!(
        out.stderr
            .lines()
            .any(|l| l.trim_start_matches('+') == " _K5_WOULD_REFUSE=1"),
        "{}",
        out.stderr
    );
    assert!(rig
        .stub
        .take()
        .iter()
        .all(|s| s.path == path() && s.refuse == ["1"]));
}
