//! The Codex per-tab passport check on a real in-process daemon
//! (`session_token::check_conversation`): its two doors, the tab socket
//! and loopback TCP.
//!
//! The "codex" here is a shell stub: `--help` lists `--no-daemon`,
//! `--version` prints the version the test gave it, and any other launch
//! records its argv, socket and passport, then runs each line typed into
//! its terminal. A line the test types is a tool call: `curl` on the tab's
//! socket, run as a child of the tab's terminal process, as a Codex tool
//! call is. A call from the test process itself stands in for a process
//! outside the tab (a shared Codex app-server). Never the real Codex.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::v2_spawn::handle_v2_spawn;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
const CHECKED: &str = "0.163.0";
const OWNER: &str = "codex-bind-owner-token";

/// `$HOME` beside the test binary (the spawn guard refuses a temp HOME),
/// and the K2 home in a short `/tmp` folder (a cell socket path must fit
/// the OS's socket path limit). The `EnvVar` guards hold the one shared
/// env lock for the whole test and restore both on drop.
struct StubHome {
    home: PathBuf,
    k2_home: PathBuf,
    _home_var: k2_core::test_env::EnvVar,
    _k2_home_var: k2_core::test_env::EnvVar,
    _k5_home_var: k2_core::test_env::EnvVar,
}

impl StubHome {
    fn new() -> Self {
        let home = crate::test_support::stub_home_dir("codex-bind");
        std::fs::create_dir_all(k2_core::paths::k2_home_in(&home)).expect("create stub HOME");
        let home_var = k2_core::test_env::EnvVar::set("HOME", &home);
        assert!(
            !k2_core::terminal::agent_spawn_guard::GuardEnv::from_process().home_is_temp(),
            "stub HOME {} is under the OS temp dir; build with a target dir outside $TMPDIR",
            home.display()
        );
        let k2_home = k2_core::test_env::short_tmp(format!("k2cb{}", &uuid::Uuid::new_v4().simple().to_string()[..10]));
        std::fs::create_dir_all(&k2_home).expect("create short K2 home");
        let k5_home_var = k2_core::test_env::EnvVar::remove("K5_HOME_OVERRIDE");
        let k2_home_var = k2_core::test_env::EnvVar::set("K2_HOME_OVERRIDE", &k2_home);
        Self { home, k2_home, _home_var: home_var, _k2_home_var: k2_home_var, _k5_home_var: k5_home_var }
    }
}

impl Drop for StubHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_dir_all(&self.k2_home);
    }
}

/// One stub Codex tab.
struct Tab {
    agent: String,
    dir: PathBuf,
    sock: PathBuf,
    token: String,
    calls: AtomicUsize,
}

impl Drop for Tab {
    fn drop(&mut self) {
        if let Some(session) = crate::v2_session_map::unregister(&self.agent) {
            session.kill();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `curl` from the test's PATH (the tab's tool calls run it).
fn curl() -> String {
    let path = std::env::var("PATH").expect("PATH");
    let found = std::env::split_paths(&path)
        .map(|d| d.join("curl"))
        .find(|p| p.is_file())
        .expect("curl on PATH: the tab's tool calls use it");
    found.to_string_lossy().into_owned()
}

/// A stub Codex in its own dir (no spaces: the provider is read from the
/// basename of the first word).
fn write_codex_stub(version: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "k2-codex-bind-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("stub dir");
    let script = r#"#!/bin/sh
if [ "$1" = "--help" ]; then printf '      --no-daemon\n          Run without the shared background server\n'; exit 0; fi
if [ "$1" = "--version" ]; then echo 'codex-cli @VERSION@'; exit 0; fi
d="$(dirname "$0")"
{ for a in "$@"; do printf 'ARG=%s\n' "$a"; done; printf 'SOCK=%s\nTOKEN=%s\n---\n' "$K2_HOOK_SOCK" "$K2_HOOK_TOKEN"; } > "$d/launch.tmp"
mv "$d/launch.tmp" "$d/launch.log"
stty -echo 2>/dev/null
while IFS= read -r line; do eval "$line"; done
"#
    .replace("@VERSION@", version);
    let path = dir.join("codex");
    k2_core::test_env::write_executable(&path, &script);
    path
}

fn seed_project(pid: &str, cwd: &str) {
    std::fs::create_dir_all(cwd).expect("workspace dir");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
        rusqlite::params![pid, cwd, "codex-bind"],
    )
    .expect("seed project");
}

/// A registered workspace folder under the stub HOME.
fn workspace(label: &str) -> String {
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let pid = format!("codex-bind-{label}-{n}");
    let home = std::env::var_os("HOME").expect("StubHome sets HOME");
    let cwd = PathBuf::from(home).join(format!("ws-{pid}")).to_string_lossy().into_owned();
    seed_project(&pid, &cwd);
    cwd
}

/// Start a stub Codex tab with `args` in `cwd` and wait for its launch.
fn spawn_tab(cwd: &str, version: &str, args: &[&str]) -> Tab {
    let stub = write_codex_stub(version);
    let dir = stub.parent().expect("stub dir").to_path_buf();
    let agent = format!("tab-codex-bind-{}", uuid::Uuid::new_v4());
    let r = handle_v2_spawn(
        serde_json::json!({
            "agent_name": agent,
            "cwd": cwd,
            "command": stub.to_string_lossy(),
            "args": args,
        })
        .to_string()
        .as_bytes(),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let log = dir.join("launch.log");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !log.is_file() {
        assert!(Instant::now() < deadline, "stub codex {} never launched", stub.display());
        std::thread::sleep(Duration::from_millis(25));
    }
    let raw = std::fs::read_to_string(&log).expect("launch.log");
    let field = |k: &str| {
        raw.lines()
            .find_map(|l| l.strip_prefix(k))
            .unwrap_or_else(|| panic!("launch.log has no {k}: {raw}"))
            .to_string()
    };
    let token = field("TOKEN=");
    assert!(token.contains('.'), "the tab got a passport: {raw}");
    Tab { agent, dir, sock: PathBuf::from(field("SOCK=")), token, calls: AtomicUsize::new(0) }
}

fn header_args(conv: Option<&str>, thread: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(c) = conv {
        out.push_str(&format!(" -H 'X-K2-Conversation: {c}'"));
    }
    if let Some(t) = thread {
        out.push_str(&format!(" -H 'X-K2-Thread: {t}'"));
    }
    out
}

/// A tool call FROM INSIDE the tab: typed into its terminal, so `curl`
/// runs as a child of the tab's process, with the tab's own passport.
fn tool_call(tab: &Tab, conv: Option<&str>, thread: Option<&str>) -> (u16, serde_json::Value) {
    let n = tab.calls.fetch_add(1, Ordering::SeqCst);
    let dir = tab.dir.to_string_lossy();
    let line = format!(
        "{curl} -s -o '{dir}/out-{n}' -w '%{{http_code}}' --unix-socket \"$K2_HOOK_SOCK\" -H \"Authorization: Bearer $K2_HOOK_TOKEN\"{h} http://localhost/cli/whoami > '{dir}/st-{n}.tmp'; mv '{dir}/st-{n}.tmp' '{dir}/st-{n}'\n",
        curl = curl(),
        h = header_args(conv, thread),
    );
    let session = crate::v2_session_map::lookup_by_agent_name(&tab.agent).expect("tab is live");
    session.write(line.into_bytes());
    let st = tab.dir.join(format!("st-{n}"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !st.is_file() {
        assert!(Instant::now() < deadline, "tool call {n} in {} never finished", tab.agent);
        std::thread::sleep(Duration::from_millis(25));
    }
    let status: u16 = std::fs::read_to_string(&st).expect("status").trim().parse().expect("http code");
    let body = std::fs::read_to_string(tab.dir.join(format!("out-{n}"))).expect("body");
    (status, parse_body(&body))
}

fn parse_body(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::Value::String(body.to_string()))
}

fn request(token: &str, conv: Option<&str>, thread: Option<&str>) -> String {
    let mut req = format!("GET /cli/whoami HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n");
    if let Some(c) = conv {
        req.push_str(&format!("X-K2-Conversation: {c}\r\n"));
    }
    if let Some(t) = thread {
        req.push_str(&format!("X-K2-Thread: {t}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    req
}

fn split_response(raw: &[u8]) -> (u16, serde_json::Value) {
    let text = String::from_utf8_lossy(raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status line: {text}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (status, parse_body(body))
}

/// A call over loopback TCP (no peer process is seen there).
async fn tcp_call(port: u16, token: &str, conv: Option<&str>, thread: Option<&str>) -> (u16, serde_json::Value) {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
    s.write_all(request(token, conv, thread).as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.expect("read");
    split_response(&raw)
}

/// A call on a tab's socket from the TEST process: a process outside the
/// tab's terminal, as a shared Codex app-server is.
async fn outside_sock_call(tab: &Tab, token: &str, conv: Option<&str>, thread: Option<&str>) -> (u16, serde_json::Value) {
    let mut s = tokio::net::UnixStream::connect(&tab.sock).await.expect("connect cell socket");
    s.write_all(request(token, conv, thread).as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.expect("read");
    split_response(&raw)
}

fn assert_ok(got: (u16, serde_json::Value), what: &str) {
    assert_eq!(got.0, 200, "{what}: {}", got.1);
}

fn assert_refused(got: (u16, serde_json::Value), code: &str, what: &str) {
    assert_eq!(got.0, 403, "{what}: {}", got.1);
    assert_eq!(got.1["error"], code, "{what}: {}", got.1);
    let msg = got.1["message"].as_str().unwrap_or_else(|| panic!("{what}: the refusal says what to do: {}", got.1));
    assert!(!msg.is_empty(), "{what}");
}

/// A Codex rollout log for `id` in `cwd`, begun at `at_ms`.
fn write_rollout(home: &Path, id: &str, cwd: &str, at_ms: i64) {
    let day = chrono::Local::now().format("%Y/%m/%d").to_string();
    let dir = home.join(".codex").join("sessions").join(day);
    std::fs::create_dir_all(&dir).expect("codex day dir");
    let ts = chrono::DateTime::from_timestamp_millis(at_ms).expect("ts").to_rfc3339();
    let header = serde_json::json!({
        "timestamp": ts,
        "type": "session_meta",
        "payload": {"id": id, "timestamp": ts, "cwd": cwd, "originator": "codex_cli_rs"}
    });
    std::fs::write(dir.join(format!("rollout-test-{id}.jsonl")), format!("{header}\n")).expect("rollout");
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The daemon (TCP door) plus two Codex tabs resumed from their argv, in
/// two folders: A on conv-A, B on conv-B.
struct Bed {
    port: u16,
    a: Tab,
    b: Tab,
    _daemon: crate::test_harness::TestDaemon,
    home: StubHome,
}

async fn bed() -> Bed {
    k2_core::db::init_for_tests();
    let home = StubHome::new();
    let daemon = crate::test_harness::start(OWNER).await;
    let a = spawn_tab(&workspace("a"), CHECKED, &["--yolo", "resume", "conv-A"]);
    let b = spawn_tab(&workspace("b"), CHECKED, &["--yolo", "resume", "conv-B"]);
    Bed { port: daemon.port, a, b, _daemon: daemon, home }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_tab_with_its_own_conversation_works_on_both_doors() {
    let t = bed().await;
    assert_ok(tool_call(&t.a, Some("conv-A"), Some("conv-A")), "A on its socket");
    assert_ok(tool_call(&t.b, Some("conv-B"), Some("conv-B")), "B on its socket");
    assert_ok(tcp_call(t.port, &t.a.token, Some("conv-A"), Some("conv-A")).await, "A over TCP");
    assert_ok(tcp_call(t.port, &t.b.token, Some("conv-B"), Some("conv-B")).await, "B over TCP");
    // A subagent: the same root, a thread of its own.
    assert_ok(tool_call(&t.a, Some("conv-A"), Some("child-thread-1")), "A's subagent on the socket");
    assert_ok(tcp_call(t.port, &t.a.token, Some("conv-A"), Some("child-thread-1")).await, "A's subagent over TCP");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_or_an_unknown_conversation_or_none_is_refused() {
    let t = bed().await;
    assert_refused(tool_call(&t.a, Some("conv-B"), Some("conv-B")), "passport_not_this_conversation", "A's tab, B's conversation, socket");
    assert_refused(tcp_call(t.port, &t.a.token, Some("conv-B"), Some("conv-B")).await, "passport_not_this_conversation", "A's passport, B's conversation, TCP");
    assert_refused(tool_call(&t.a, Some("conv-nobody"), None), "unknown_conversation", "unknown, socket");
    assert_refused(tcp_call(t.port, &t.a.token, Some("conv-nobody"), None).await, "unknown_conversation", "unknown, TCP");
    // A Codex call with no conversation header.
    assert_refused(tool_call(&t.a, None, Some("conv-A")), "conversation_missing", "thread only, socket");
    assert_refused(tool_call(&t.a, None, None), "conversation_missing", "no headers, Codex tab, socket");
    assert_refused(tcp_call(t.port, &t.a.token, None, None).await, "conversation_missing", "no headers, Codex tab, TCP");
    // A call with no passport is never checked: the owner token reaches
    // whoami itself, which can't name a workspace for the owner (400).
    let (status, body) = tcp_call(t.port, OWNER, Some("conv-nobody"), Some("x")).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("cannot determine identity")), "the owner token is not checked: {body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_socket_caller_outside_the_tab_is_refused() {
    let t = bed().await;
    // The test process holds A's passport and A's own conversation, but it
    // doesn't descend from A's terminal process.
    assert_refused(
        outside_sock_call(&t.a, &t.a.token, Some("conv-A"), Some("conv-A")).await,
        "not_this_tabs_process",
        "outside process on A's socket",
    );
    assert_ok(tool_call(&t.a, Some("conv-A"), None), "the same call from inside the tab");
}

/// The Steward's case: a tab started before the fix ran a shared Codex
/// app-server, so another conversation's tool call carries the starter's
/// passport and socket, with its own thread id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_server_call_with_the_starters_passport_is_refused() {
    let t = bed().await;
    // The starter: a pre-fix tab, launched without --no-daemon.
    let starter = {
        let _off = k2_core::test_env::EnvVar::set(k2_core::terminal::codex_no_daemon::OVERRIDE_ENV, "0");
        spawn_tab(&workspace("prefix"), CHECKED, &["--yolo", "resume", "conv-S"])
    };
    let raw = std::fs::read_to_string(starter.dir.join("launch.log")).expect("launch.log");
    assert!(!raw.contains("ARG=--no-daemon"), "the starter is a pre-fix tab: {raw}");
    for (conv, code) in [("conv-B", "passport_not_this_conversation"), ("conv-outside-k2", "unknown_conversation")] {
        // Over TCP: the starter's passport, the other conversation.
        assert_refused(tcp_call(t.port, &starter.token, Some(conv), Some(conv)).await, code, "shared server, TCP");
        // On the starter's socket, from a process outside its terminal.
        assert_refused(
            outside_sock_call(&starter, &starter.token, Some(conv), Some(conv)).await,
            "not_this_tabs_process",
            "shared server, socket",
        );
    }
    // Its own conversation still works from inside it.
    assert_ok(tool_call(&starter, Some("conv-S"), None), "the starter's own call");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_new_a_call_works_once_its_log_exists() {
    let t = bed().await;
    let cwd = workspace("new");
    write_rollout(&t.home.home, "conv-N", &cwd, now_ms() - 60_000);
    let tab = spawn_tab(&cwd, CHECKED, &["--yolo", "resume", "conv-N"]);
    assert_ok(tcp_call(t.port, &tab.token, Some("conv-N"), None).await, "before /new");
    // `/new`: Codex starts conv-N2. Before its log exists: refused.
    assert_refused(tcp_call(t.port, &tab.token, Some("conv-N2"), None).await, "unknown_conversation", "no log yet");
    assert_refused(tool_call(&tab, Some("conv-N2"), None), "unknown_conversation", "no log yet, socket");
    write_rollout(&t.home.home, "conv-N2", &cwd, now_ms());
    assert_ok(tcp_call(t.port, &tab.token, Some("conv-N2"), None).await, "the new log exists");
    assert_ok(tool_call(&tab, Some("conv-N2"), None), "the new log exists, socket");
    assert_eq!(
        crate::activity_transcript::conversation_of(&live_sid(&tab)).map(|c| c.0).as_deref(),
        Some("conv-N2"),
        "the re-check moved the tab"
    );
    // B's log in another folder never moves A.
    write_rollout(&t.home.home, "conv-elsewhere", &workspace("other"), now_ms());
    assert_refused(tcp_call(t.port, &t.a.token, Some("conv-elsewhere"), None).await, "unknown_conversation", "another folder's log");
}

fn live_sid(tab: &Tab) -> String {
    crate::v2_session_map::lookup_by_agent_name(&tab.agent).expect("live").session_id.to_string()
}

/// Wait for the follower to adopt a log for each fresh tab.
fn adopted(tabs: &[&Tab]) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for t in tabs {
            crate::activity_transcript::note_claim(&live_sid(t));
        }
        crate::activity_transcript::poll_all_once(now_ms());
        let got: Vec<Option<String>> =
            tabs.iter().map(|t| crate::activity_transcript::conversation_of(&live_sid(t)).map(|c| c.0)).collect();
        if got.iter().all(Option::is_some) {
            return got.into_iter().map(|c| c.expect("adopted")).collect();
        }
        assert!(Instant::now() < deadline, "the fresh tabs never adopted their logs: {got:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_unsettled_tabs_in_one_folder_are_refused_over_tcp_until_settled() {
    let t = bed().await;
    let cwd = workspace("pair");
    let c = spawn_tab(&cwd, CHECKED, &["--yolo"]);
    let d = spawn_tab(&cwd, CHECKED, &["--yolo"]);
    let at = now_ms();
    write_rollout(&t.home.home, "conv-fresh-1", &cwd, at);
    write_rollout(&t.home.home, "conv-fresh-2", &cwd, at + 10);
    let convs = adopted(&[&c, &d]);
    assert_ne!(convs[0], convs[1], "distinct logs");
    assert_refused(tcp_call(t.port, &c.token, Some(convs[0].as_str()), None).await, "conversation_unsettled", "C over TCP");
    assert_refused(tcp_call(t.port, &d.token, Some(convs[1].as_str()), None).await, "conversation_unsettled", "D over TCP");
    // A call proven on C's socket settles C.
    assert_ok(tool_call(&c, Some(convs[0].as_str()), None), "C on its socket");
    assert_ok(tcp_call(t.port, &c.token, Some(convs[0].as_str()), None).await, "C over TCP, settled");
    assert_ok(tcp_call(t.port, &d.token, Some(convs[1].as_str()), None).await, "D over TCP, the only unsettled tab left");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unchecked_codex_version_is_refused() {
    let t = bed().await;
    let tab = spawn_tab(&workspace("ver"), "0.999.0", &["--yolo", "resume", "conv-V"]);
    let got = tcp_call(t.port, &tab.token, Some("conv-V"), None).await;
    assert_refused(got.clone(), "codex_version_unverified", "unknown version, TCP");
    let msg = got.1["message"].as_str().expect("message");
    assert!(msg.contains("(0.999.0)") && msg.contains("K2_CODEX_PASSPORT_BIND=0"), "{msg}");
    assert_refused(tool_call(&tab, Some("conv-V"), None), "codex_version_unverified", "unknown version, socket");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_switch_off_restores_todays_behaviour() {
    let t = bed().await;
    let _off = k2_core::test_env::EnvVar::set("K2_CODEX_PASSPORT_BIND", "0");
    assert_ok(tcp_call(t.port, &t.a.token, Some("conv-B"), Some("conv-B")).await, "switch off, TCP");
    assert_ok(tcp_call(t.port, &t.a.token, None, None).await, "switch off, no header");
    assert_ok(outside_sock_call(&t.a, &t.a.token, Some("conv-B"), None).await, "switch off, outside the tab");
}
