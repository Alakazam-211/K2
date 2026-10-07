//! prd-daemon-activity-and-thread-working-v1 S1 — `POST /hook/event`
//! end to end through the REAL dispatcher (TCP) and the REAL per-cell
//! socket server (UDS), plus `POST /cli/hooks/install`.
//!
//! - T-S1f: GET → 405, a 2 MiB body → 413, a scoped token for pane B
//!   posting as pane A → 403, over both transports (A4).
//! - The owner check against real processes: the PTY child's own pid is
//!   accepted (and shows in `k2 hooks status`); pid 1 and an unrelated
//!   process are foreign; a dead pane is `unknown_pane`.
//! - T-S1d: a detached process (reparented to init) carrying a live
//!   pane's id is foreign and the pane gets no owner.
//! - `/cli/hooks/install` runs the installer into the temp HOME only.
//!
//! ISOLATION: `$HOME` (short, under /tmp, for the socket path limit), the
//! shared DB, the v2 map and the scoped-token registry are process-wide.
//! One test in this binary. No agent CLI is ever started: the PTY child
//! is `cat`, and the installer's `claude --version` probe is refused
//! under a temp HOME.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::{cell_server, cell_uds, session_token, test_harness, v2_session_map};

const OWNER_TOKEN: &str = "owner-token-hook-event-s1";

struct Resp {
    status: u16,
    body: String,
}

fn parse_resp(raw: &[u8]) -> Resp {
    let text = String::from_utf8_lossy(raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

/// One request; `Connection: close` so the read ends at EOF.
fn request(stream: &mut dyn ReadWrite, head: &str, body: &[u8]) -> Resp {
    stream.write_all(head.as_bytes()).expect("write head");
    // A refused oversized body may close the socket mid-write; that is
    // the expected 413 path, so a broken pipe here is not a failure.
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    parse_resp(&raw)
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

fn tcp(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    s.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    s
}

fn uds(sock: &Path) -> UnixStream {
    let s = UnixStream::connect(sock).expect("connect cell socket");
    s.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    s
}

fn hook_head(method: &str, path: &str, auth: &str, pane: &str, pid: Option<i32>, len: usize) -> String {
    let pid = pid.map(|p| format!("X-K2-Agent-Pid: {p}\r\n")).unwrap_or_default();
    format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}X-K2-Pane: {pane}\r\n{pid}\
         X-K2-Hook-Source: claude\r\nX-K2-Hook-Version: 2\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n\r\n"
    )
}

fn bearer(t: &str) -> String {
    format!("Authorization: Bearer {t}\r\n")
}

fn post_event(port: u16, pane: &str, pid: Option<i32>, body: &str) -> Resp {
    let head = hook_head("POST", "/hook/event", &bearer(OWNER_TOKEN), pane, pid, body.len());
    request(&mut tcp(port), &head, body.as_bytes())
}

fn hooks_status(port: u16) -> serde_json::Value {
    let head = format!(
        "GET /cli/hooks/status?token={OWNER_TOKEN} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    );
    let r = request(&mut tcp(port), &head, b"");
    assert_eq!(r.status, 200, "hooks status: {}", r.body);
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("status JSON ({e}): {}", r.body))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// A short `$HOME` under /tmp (cell socket paths are capped near 104
/// bytes), an in-memory DB and an empty v2 map; restored on drop.
fn with_short_home<F: FnOnce(&Path)>(f: F) {
    struct Restore {
        prev: Option<std::ffi::OsString>,
        tmp: PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            v2_session_map::clear_for_tests();
            match self.prev.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let tmp = PathBuf::from(format!("/tmp/k2he{}{}", std::process::id(), nanos % 1_000_000));
    std::fs::create_dir_all(tmp.join(".k2")).expect("create temp HOME");
    let _restore = Restore { prev: std::env::var_os("HOME"), tmp: tmp.clone() };
    std::env::set_var("HOME", &tmp);
    k2_core::test_isolation::assert_isolated_from_prod();
    let _ = k2_core::db::init_for_tests();
    v2_session_map::clear_for_tests();
    f(&tmp);
}

fn spawn_cat(cwd: &Path, agent: &str) -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(cwd.to_path_buf()),
        program: Some("cat".to_string()),
        ..DaemonPtyConfig::default()
    };
    let session = DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
    v2_session_map::register(agent.to_string(), Arc::clone(&session));
    session
}

fn principal() -> session_token::HookPrincipal {
    session_token::HookPrincipal { workspace_uuid: String::new(), agent_address: "agent-s1".to_string() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hook_event_transport_auth_and_owner_check() {
    with_short_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let ws = home.join("ws");
        std::fs::create_dir_all(&ws).expect("ws dir");

        // Two live sessions: A (the hooked agent) and B.
        let a = spawn_cat(&ws, "tab-hook-a");
        let b = spawn_cat(&ws, "tab-hook-b");
        let pane_a = a.session_id.to_string();
        let pane_b = b.session_id.to_string();
        let a_pid = a.child_pid().expect("A has a child pid");

        // ── T-S1f over TCP ──────────────────────────────────────────
        let r = request(&mut tcp(port), &hook_head("GET", "/hook/event", &bearer(OWNER_TOKEN), &pane_a, None, 0), b"");
        assert_eq!(r.status, 405, "GET /hook/event: {}", r.body);

        // The daemon refuses on the declared length, before the body: send
        // the head only so the refused body can't reset the socket.
        let big = vec![b'x'; 2 * 1024 * 1024];
        let r = request(
            &mut tcp(port),
            &hook_head("POST", "/hook/event", &bearer(OWNER_TOKEN), &pane_a, Some(a_pid), big.len()),
            b"",
        );
        assert_eq!(r.status, 413, "2 MiB body: {}", r.body);

        let token_b = session_token::mint_session_token(
            &b.session_id,
            &pane_b,
            principal(),
            session_token::CredMode::ApiKey,
            session_token::Provider::Anthropic,
        );
        let body = r#"{"hook_event_name":"Stop"}"#;
        let r = request(
            &mut tcp(port),
            &hook_head("POST", "/hook/event", &bearer(&token_b), &pane_a, Some(a_pid), body.len()),
            body.as_bytes(),
        );
        assert_eq!(r.status, 403, "scoped B posting as A over TCP: {}", r.body);
        // The same token for its own pane is accepted at the transport.
        let r = request(
            &mut tcp(port),
            &hook_head("POST", "/hook/event", &bearer(&token_b), &pane_b, Some(a_pid), body.len()),
            body.as_bytes(),
        );
        assert_eq!(r.status, 204, "scoped B for B: {}", r.body);
        let r = request(&mut tcp(port), &hook_head("POST", "/hook/event", "", &pane_a, None, body.len()), body.as_bytes());
        assert_eq!(r.status, 403, "no token: {}", r.body);

        // ── T-S1f over the cell socket (B's socket, B's token) ──────
        let listener = cell_uds::bind_cell_socket(&b.session_id).expect("bind B's cell socket");
        let sock = cell_uds::cell_socket_path(&b.session_id);
        cell_server::serve_cell(b.session_id, listener, None);
        let r = request(&mut uds(&sock), &hook_head("GET", "/hook/event", &bearer(&token_b), &pane_b, None, 0), b"");
        assert_eq!(r.status, 405, "UDS GET: {}", r.body);
        let r = request(
            &mut uds(&sock),
            &hook_head("POST", "/hook/event", &bearer(&token_b), &pane_a, Some(a_pid), body.len()),
            body.as_bytes(),
        );
        assert_eq!(r.status, 403, "scoped B posting as A over UDS: {}", r.body);
        let r = request(
            &mut uds(&sock),
            &hook_head("POST", "/hook/event", &bearer(&token_b), &pane_b, Some(a_pid), big.len()),
            &big,
        );
        assert_eq!(r.status, 413, "UDS 2 MiB: {}", r.body);
        let r = request(
            &mut uds(&sock),
            &hook_head("POST", "/hook/event", &bearer(&token_b), &pane_b, Some(a_pid), body.len()),
            body.as_bytes(),
        );
        assert_eq!(r.status, 204, "UDS own pane: {}", r.body);

        // ── The owner check against real processes ──────────────────
        let prompt = format!(
            r#"{{"hook_event_name":"UserPromptSubmit","session_id":"conv-a","prompt":"[thread:{}] run it"}}"#,
            "ws"
        );
        assert_eq!(post_event(port, &pane_a, Some(a_pid), &prompt).status, 204);
        // pid 1 is never an agent (§10 step 3); this test process is not
        // under A's PTY child.
        assert_eq!(post_event(port, &pane_a, Some(1), body).status, 204);
        let me = std::process::id() as i32;
        assert_eq!(post_event(port, &pane_a, Some(me), body).status, 204);
        // T-S1d: a detached process (reparented to init) with A's pane id.
        let detached: i32 = {
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", "/bin/sleep 20 >/dev/null 2>&1 & echo $!"])
                .output()
                .expect("spawn detached sleep");
            String::from_utf8_lossy(&out.stdout).trim().parse().expect("detached pid")
        };
        assert_eq!(post_event(port, &pane_a, Some(detached), body).status, 204);
        // A pane that isn't live.
        let dead = SessionId::new().to_string();
        assert_eq!(post_event(port, &dead, Some(a_pid), body).status, 204);
        // A body that isn't JSON: still 204, counted.
        assert_eq!(post_event(port, &pane_a, Some(a_pid), "not json").status, 204);

        let status = hooks_status(port);
        let sa = &status["sessions"][&pane_a];
        assert_eq!(sa["counters"]["accepted"], 1, "{status}");
        assert_eq!(sa["counters"]["foreign"], 3, "pid 1, this process, detached: {status}");
        assert_eq!(sa["counters"]["parseError"], 1, "{status}");
        assert_eq!(sa["owner"]["proc"]["pid"], a_pid, "{status}");
        assert_eq!(sa["owner"]["via"], "pty_child");
        assert_eq!(sa["conversationId"], "conv-a");
        assert_eq!(sa["lastEvent"], "UserPromptSubmit");
        assert!(status["totals"]["unknownPane"].as_u64().expect("count") >= 1, "{status}");
        assert_eq!(status["ownerCheck"], "ancestry");
        // DA5: the ring buffer has the event name and verdict, never the
        // prompt text.
        let ring = serde_json::to_string(&status["recent_events"]).expect("ring");
        assert!(ring.contains("UserPromptSubmit") && ring.contains("accepted"), "{ring}");
        assert!(!ring.contains("run it"), "prompt text leaked into the ring: {ring}");
        // T-S1d: the pane B session never got an owner from A's pid either
        // (A's pid is not under B's PTY child).
        assert_eq!(status["sessions"][&pane_b]["owner"], serde_json::Value::Null, "{status}");
        let _ = std::process::Command::new("/bin/kill").arg(detached.to_string()).status();

        // ── /cli/hooks/install (temp HOME only) ────────────────────
        let head = format!("GET /cli/hooks/install?token={OWNER_TOKEN} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        assert_eq!(request(&mut tcp(port), &head, b"").status, 405);
        std::fs::create_dir_all(home.join(".claude")).expect(".claude");
        let install = |body: &str| -> serde_json::Value {
            let head = format!(
                "POST /cli/hooks/install?token={OWNER_TOKEN} HTTP/1.1\r\nHost: localhost\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let r = request(&mut tcp(port), &head, body.as_bytes());
            assert_eq!(r.status, 200, "install: {}", r.body);
            serde_json::from_str(&r.body).expect("install JSON")
        };
        let run = install("{}");
        assert_eq!(run["skipped"], serde_json::Value::Null, "{run}");
        assert_eq!(run["report"]["action"], "install");
        assert_eq!(run["report"]["claudeVersion"], serde_json::Value::Null, "no real claude probed under a temp HOME");
        assert_eq!(run["report"]["claudeEventSet"], "base");
        assert!(home.join(".k2/hooks/notify.sh").is_file());
        let settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(home.join(".claude/settings.json")).expect("settings")).expect("json");
        assert!(settings["hooks"]["Stop"].is_array(), "{settings}");
        let run = install(r#"{"remove":true}"#);
        assert_eq!(run["report"]["action"], "remove");
        let settings = std::fs::read_to_string(home.join(".claude/settings.json")).expect("settings");
        assert!(!settings.contains("notify.sh"), "{settings}");

        drop(v2_session_map::unregister("tab-hook-a"));
        drop(v2_session_map::unregister("tab-hook-b"));
        drop(a);
        drop(b);
    });
}
