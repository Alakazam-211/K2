//! `k2-daemon --vault-run`, the helper behind `k2 vault run`
//! (prd-vault-for-agents-v1 A1 §2.6), against a fake daemon on a temp Unix
//! socket. Never a real daemon, vault or secret.

#![cfg(unix)]

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};

const VALUE: &str = "k2test_HELPER_SENTINEL_0123456789";
const MASKED: &str = "[k2v:KEY]";

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

/// A short temp folder: a Unix socket path is limited to about 104 bytes,
/// and macOS's temp dir uses most of that.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let p = PathBuf::from(format!("/tmp/k2vr-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&p).expect("scratch dir");
        Scratch(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fake {
    _dir: Scratch,
    sock: PathBuf,
    root: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
}

fn fake_daemon(status: &'static str, answer: Value) -> Fake {
    fake_daemon_in(Scratch::new(), status, answer)
}

/// The same answer to every connection.
fn fake_daemon_in(dir: Scratch, status: &'static str, answer: Value) -> Fake {
    let sock = dir.path().join("cell.sock");
    let root = dir.path().to_path_buf();
    let listener = UnixListener::bind(&sock).expect("bind");
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let mut c = conn.expect("accept");
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let (head_end, want) = loop {
                let n = c.read(&mut buf).expect("read");
                assert!(n > 0, "the helper hung up mid-request");
                raw.extend_from_slice(&buf[..n]);
                if let Some(he) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..he]).to_ascii_lowercase();
                    let cl: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).expect("content-length").trim().parse().expect("number");
                    break (he + 4, cl);
                }
            };
            while raw.len() < head_end + want {
                let n = c.read(&mut buf).expect("read");
                assert!(n > 0, "the helper hung up mid-body");
                raw.extend_from_slice(&buf[..n]);
            }
            seen.lock().expect("lock").push(String::from_utf8_lossy(&raw).into_owned());
            let body = answer.to_string();
            let resp = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            c.write_all(resp.as_bytes()).expect("reply");
        }
    });
    Fake { _dir: dir, sock, root, requests }
}

fn ok_env() -> Value {
    json!({ "runId": "r1", "items": [{ "name": "KEY", "env": "KEY", "how": "env", "valueB64": b64(VALUE) }] })
}

fn helper(f: &Fake, flags: &[&str], child: &[&OsStr]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_k2-daemon"));
    c.arg("--vault-run").args(flags).arg("--").args(child);
    c.env("K2_HOOK_SOCK", &f.sock).env("K2_HOOK_TOKEN", "passport-t");
    c.stdin(Stdio::null());
    c
}

fn sh(script: &str) -> Vec<&OsStr> {
    vec![OsStr::new("/bin/sh"), OsStr::new("-c"), OsStr::new(script)]
}

fn run(f: &Fake, flags: &[&str], child: &[&OsStr]) -> Output {
    helper(f, flags, child).output().expect("run helper")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn the_childs_output_is_masked() {
    let f = fake_daemon("200 OK", ok_env());
    let out = run(&f, &["--secret", "KEY"], &sh(r#"printf %s "$KEY"; printf %s "$KEY" >&2"#));
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), MASKED);
    assert_eq!(text(&out.stderr), MASKED);
}

#[test]
fn the_env_var_really_is_the_value() {
    let f = fake_daemon("200 OK", ok_env());
    let script = format!(r#"[ "$KEY" = "{VALUE}" ] && echo same"#);
    let out = run(&f, &["--secret", "KEY"], &sh(&script));
    assert_eq!(text(&out.stdout).trim(), "same", "{}", text(&out.stderr));
}

#[test]
fn a_value_split_across_two_writes_is_still_masked() {
    let f = fake_daemon("200 OK", ok_env());
    let (a, b) = VALUE.split_at(10);
    let script = format!("printf %s '{a}'; sleep 0.3; printf %s '{b}'");
    let out = run(&f, &["--secret", "KEY"], &sh(&script));
    assert_eq!(text(&out.stdout), MASKED, "{}", text(&out.stderr));
}

#[test]
fn other_output_flows_through_untouched() {
    let f = fake_daemon("200 OK", ok_env());
    let out = run(&f, &["--secret", "KEY"], &sh("printf 'hello\\nworld'"));
    assert_eq!(text(&out.stdout), "hello\nworld");
}

#[test]
fn the_childs_exit_code_passes_through() {
    let f = fake_daemon("200 OK", ok_env());
    assert_eq!(run(&f, &["--secret", "KEY"], &sh("exit 7")).status.code(), Some(7));
}

#[test]
fn a_child_killed_by_a_signal_is_128_plus_it() {
    let f = fake_daemon("200 OK", ok_env());
    assert_eq!(run(&f, &["--secret", "KEY"], &sh("kill -9 $$")).status.code(), Some(137));
}

#[test]
fn sigterm_reaches_the_child() {
    let f = fake_daemon("200 OK", ok_env());
    // `sleep` starts before "ready", and only a signal that reaches the whole
    // group, unblocked, ends this quickly. (The helper holds these signals
    // until the child exists; a child that inherited that mask sat out the
    // full 30 s and then printed got-term.)
    let script = r#"trap 'echo got-term; exit 9' TERM; sleep 30 & echo ready; wait"#;
    let started = Instant::now();
    let mut child: Child = helper(&f, &["--secret", "KEY"], &sh(script)).stdout(Stdio::piped()).spawn().expect("spawn");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    assert_eq!(lines.next().expect("a line").expect("read"), "ready");
    // SAFETY: a plain kill(2) on our own child.
    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) }, 0);
    let rest: Vec<String> = lines.map(|l| l.expect("read")).collect();
    let status = child.wait().expect("wait");
    assert_eq!(rest, vec!["got-term".to_string()]);
    assert_eq!(status.code(), Some(9), "{:?}", status.signal());
    assert!(started.elapsed() < Duration::from_secs(15), "the child's process group didn't get it: {:?}", started.elapsed());
}

#[test]
fn no_mask_prints_the_raw_value() {
    let f = fake_daemon("200 OK", ok_env());
    let out = run(&f, &["--secret", "KEY", "--no-mask"], &sh(r#"printf %s "$KEY""#));
    assert_eq!(text(&out.stdout), VALUE);
}

#[cfg(target_os = "linux")]
#[test]
fn the_helpers_own_argv_holds_no_value() {
    // --no-mask: masked output would hide a value that was really there.
    let f = fake_daemon("200 OK", ok_env());
    let out = run(&f, &["--secret", "KEY", "--no-mask"], &sh("cat /proc/$PPID/cmdline"));
    let cmdline = text(&out.stdout);
    assert!(cmdline.contains("--vault-run"), "read the wrong process: {cmdline}");
    assert!(!cmdline.contains(VALUE), "{cmdline}");
}

#[test]
fn only_names_are_sent_and_the_command_is_a_basename() {
    let f = fake_daemon("200 OK", ok_env());
    run(&f, &["--secret", "KEY=OTHER", "--no-mask", "--scope", "server"], &sh("true"));
    let req = f.requests.lock().expect("lock")[0].clone();
    assert!(req.starts_with("POST /cli/vault/run HTTP/1.1\r\n"), "{req}");
    assert!(req.contains("Authorization: Bearer passport-t\r\n"), "{req}");
    let body: Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).expect("body")).expect("json");
    assert_eq!(body["secrets"], json!([{ "name": "KEY", "env": "OTHER" }]));
    assert_eq!(body["cmd"], "sh", "basename, not /bin/sh and no arguments: {body}");
    assert_eq!(body["noMask"], true);
    assert_eq!(body["scope"], "server");
}

#[test]
fn a_per_run_file_is_gone_after_exit_and_masked_too() {
    let f = fake_daemon_with_file();
    let out = run(&f, &["--secret", "KEY"], &sh(r#"[ -f "$KEY_FILE" ] && echo there; cat "$KEY_FILE""#));
    assert_eq!(text(&out.stdout), format!("there\n{MASKED}"), "{}", text(&out.stderr));
    assert!(!f.root.join("run").join("r2").exists(), "the run folder is removed");
}

fn fake_daemon_with_file() -> Fake {
    let dir = Scratch::new();
    let run_dir = dir.path().join("run").join("r2");
    std::fs::create_dir_all(&run_dir).expect("mkdir");
    let path = run_dir.join("0-KEY");
    std::fs::write(&path, VALUE).expect("write");
    let answer = json!({ "runId": "r2", "runDir": run_dir.display().to_string(), "items": [
        { "name": "KEY", "env": "KEY_FILE", "how": "file", "path": path.display().to_string(), "maskB64": b64(VALUE) }
    ]});
    fake_daemon_in(dir, "200 OK", answer)
}

#[test]
fn a_run_folder_the_answer_names_elsewhere_is_left_alone() {
    let dir = Scratch::new();
    let elsewhere = dir.path().join("precious");
    std::fs::create_dir_all(&elsewhere).expect("mkdir");
    let f = fake_daemon("200 OK", json!({ "runId": "r3", "runDir": elsewhere.display().to_string(), "items": [{ "name": "KEY", "env": "KEY", "how": "env", "valueB64": b64(VALUE) }] }));
    run(&f, &["--secret", "KEY"], &sh("true"));
    assert!(elsewhere.exists());
}

#[test]
fn a_childs_own_flags_are_its_own() {
    // These would divert a daemon argv check, or panic std::env::args.
    let f = fake_daemon("200 OK", ok_env());
    let out = run(&f, &["--secret", "KEY"], &[OsStr::new("/bin/echo"), OsStr::new("--skin-gateway"), OsStr::new("--version")]);
    assert_eq!(text(&out.stdout).trim(), "--skin-gateway --version", "{}", text(&out.stderr));
    let bad = OsStr::from_bytes(b"\xff\xfe");
    let out = run(&f, &["--secret", "KEY"], &[OsStr::new("/bin/sh"), OsStr::new("-c"), OsStr::new("exit 3"), OsStr::new("_"), bad]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
}

#[test]
fn stdin_reaches_the_child() {
    let f = fake_daemon("200 OK", ok_env());
    let mut child = helper(&f, &["--secret", "KEY"], &sh("cat")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn");
    child.stdin.take().expect("stdin").write_all(b"from-stdin").expect("write");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(text(&out.stdout), "from-stdin");
}

#[test]
fn a_background_process_holding_the_pipe_does_not_hang_the_run() {
    let f = fake_daemon("200 OK", ok_env());
    let started = Instant::now();
    let out = run(&f, &["--secret", "KEY"], &sh("sleep 30 & exit 5"));
    assert_eq!(out.status.code(), Some(5));
    assert!(started.elapsed() < Duration::from_secs(15), "took {:?}", started.elapsed());
}

#[test]
fn a_missing_command_is_127_and_the_files_still_go() {
    let f = fake_daemon_with_file();
    let out = run(&f, &["--secret", "KEY"], &[OsStr::new("/no/such/k2test-binary")]);
    assert_eq!(out.status.code(), Some(127), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("command not found"), "{}", text(&out.stderr));
    assert!(!f.root.join("run").join("r2").exists(), "the run folder is removed");
}

#[test]
fn a_refusal_uses_the_vault_exit_codes_and_the_closing_line() {
    let f = fake_daemon("403 Forbidden", json!({
        "error": { "code": "delivery_not_granted", "hint": "S is fetch only" },
        "permission": { "command": "k2 permission request vault.delivery secret=S" }
    }));
    let out = run(&f, &["--secret", "S"], &sh("echo must-not-run"));
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(text(&out.stdout), "", "the child never ran");
    let err = text(&out.stderr);
    assert!(err.contains("delivery_not_granted: S is fetch only"), "{err}");
    assert!(err.contains("Ask with: k2 permission request vault.delivery secret=S"), "{err}");

    let f = fake_daemon("404 Not Found", json!({ "error": { "code": "not_found", "hint": "no such secret" } }));
    assert_eq!(run(&f, &["--secret", "S"], &sh("true")).status.code(), Some(4));
    let f = fake_daemon("400 Bad Request", json!({ "error": { "code": "bad_env_name", "hint": "x" } }));
    assert_eq!(run(&f, &["--secret", "S"], &sh("true")).status.code(), Some(2));
}

#[test]
fn outside_a_k2_session_it_says_so() {
    let f = fake_daemon("200 OK", ok_env());
    let out = helper(&f, &["--secret", "KEY"], &sh("echo must-not-run")).env_remove("K2_HOOK_SOCK").output().expect("run");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(text(&out.stdout), "");
    assert!(text(&out.stderr).contains("use_run_as_agent"), "{}", text(&out.stderr));
}

#[test]
fn a_dead_socket_fails_loudly() {
    let dir = Scratch::new();
    let mut c = Command::new(env!("CARGO_BIN_EXE_k2-daemon"));
    c.args(["--vault-run", "--secret", "KEY", "--", "/bin/echo", "x"]);
    c.env("K2_HOOK_SOCK", dir.path().join("gone.sock")).env("K2_HOOK_TOKEN", "t").stdin(Stdio::null());
    let out = c.output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stdout), "", "the child never ran");
    assert!(text(&out.stderr).contains("run_unavailable"), "{}", text(&out.stderr));
}

#[test]
fn a_delivery_this_helper_cant_honour_never_runs_the_child() {
    let f = fake_daemon("200 OK", json!({ "runId": "r", "items": [{ "name": "KEY", "env": "KEY", "how": "tomorrow" }] }));
    let out = run(&f, &["--secret", "KEY"], &sh("echo must-not-run"));
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn usage_errors_are_exit_2() {
    let mut c = Command::new(env!("CARGO_BIN_EXE_k2-daemon"));
    c.args(["--vault-run", "--secret", "KEY"]).stdin(Stdio::null());
    let out = c.output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("usage:"), "{}", text(&out.stderr));
}
