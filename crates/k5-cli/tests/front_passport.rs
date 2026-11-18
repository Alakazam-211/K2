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

const LIST: &[&str] = &["heartbeat", "list", "--json"];
const PATH: &str = "/cli/heartbeat/list";

#[test]
fn no_passport_inside_a_session_is_counted() {
    let rig = Rig::new("counted");
    for marker in ["CLAUDECODE", "K2_PANE_ID", "K5_CELL", "CODEX_THREAD_ID"] {
        assert_eq!(
            rig.refuse(LIST, &[(marker, "1")], PATH),
            [["1"]],
            "{marker}"
        );
    }
}

#[test]
fn a_passport_is_never_counted() {
    let rig = Rig::new("passport");
    for name in ["K2_HOOK_TOKEN", "K5_HOOK_TOKEN", "K2SO_HOOK_TOKEN"] {
        assert_eq!(
            rig.refuse(LIST, &[("CLAUDECODE", "1"), (name, "test-token")], PATH),
            [Vec::<String>::new()],
            "{name}"
        );
    }
}

#[test]
fn outside_a_session_nothing_is_counted_whatever_the_caller_set() {
    let rig = Rig::new("outside");
    assert_eq!(rig.refuse(LIST, &[], PATH), [Vec::<String>::new()]);
    assert_eq!(
        rig.refuse(LIST, &[("K5_CLI_WOULD_REFUSE", "1")], PATH),
        [Vec::<String>::new()]
    );
    // An empty marker counts as unset.
    assert_eq!(
        rig.refuse(LIST, &[("CLAUDECODE", "")], PATH),
        [Vec::<String>::new()]
    );
}

#[test]
fn no_owner_fallback_is_left_to_the_shell() {
    let rig = Rig::new("nofb");
    let out = rig.run(LIST, &[("CLAUDECODE", "1"), ("K2_NO_OWNER_FALLBACK", "1")]);
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
    let out = rig.run(LIST, &[("CLAUDECODE", "1"), ("SHELLOPTS", "xtrace")]);
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
        .all(|s| s.path == PATH && s.refuse == ["1"]));
}
