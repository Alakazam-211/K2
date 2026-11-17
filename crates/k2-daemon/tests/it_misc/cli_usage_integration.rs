//! R1 slice 10: the k5 CLI's usage counters on a real daemon, at both doors.
//!
//! - `X-K5-Cli-Via: fallthrough` + `X-K5-Cli-Noun`, `X-K5-Would-Refuse: 1`
//!   and `X-K5-Owner-Source: disk` are counted over TCP and an agent's cell
//!   socket, exact values only, and only once the request's credential
//!   checked out (no token, a junk token, a bad bearer: nothing counted).
//! - `GET /cli/cli-usage` is the owner's: the rows, after a flush. A
//!   passport (either door), a Member login and no token are refused.
// The cell door is the per-cell Unix socket (cell_server::serve_cell).
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const OWNER: &str = "owner-token-cli-usage-counts";
/// An agent verb on the cell socket that reads only (folder trust status).
const VERB: &str = "/cli/folder-trust/status";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", self.status, self.body))
    }
}

fn parse(text: &str) -> Resp {
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no HTTP answer: {text:?}"));
    let line = head.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    Resp { status, body: body.to_string() }
}

fn lines(extra: &[&str]) -> String {
    extra.iter().map(|h| format!("{h}\r\n")).collect()
}

fn tcp_blocking(port: u16, pq: &str, extra: &str) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = format!("GET {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra}\r\n");
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    match s.read_to_end(&mut out) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("GET {pq}: {e}"),
    }
    parse(&String::from_utf8_lossy(&out))
}

/// A GET over TCP, `token` as `?token=` (the CLI's form) when given.
async fn tcp(port: u16, path: &str, token: Option<&str>, extra: &[&str]) -> Resp {
    let pq = match token {
        Some(t) => format!("{path}{}token={t}", if path.contains('?') { '&' } else { '?' }),
        None => path.to_string(),
    };
    let extra = lines(extra);
    tokio::task::spawn_blocking(move || tcp_blocking(port, &pq, &extra)).await.unwrap()
}

/// A GET on the agent's cell socket, the passport as a Bearer.
async fn cell(sock: &Path, path: &str, bearer: &str, extra: &[&str]) -> Resp {
    let raw = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\n{}\r\n", lines(extra));
    let mut stream = UnixStream::connect(sock).await.expect("connect cell socket");
    stream.write_all(raw.as_bytes()).await.expect("write");
    stream.flush().await.ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    parse(&String::from_utf8_lossy(&buf))
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// `(kind, key, count)` of every stored row, in table order.
fn stored() -> Vec<(String, String, i64)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::cli_usage::list(&conn).expect("list rows").into_iter().map(|r| (r.kind, r.key, r.count)).collect()
}

fn rows_of(v: &Value) -> Vec<(String, String, i64)> {
    v["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows array: {v}"))
        .iter()
        .map(|r| {
            let first = r["first_at"].as_i64().unwrap_or_else(|| panic!("first_at: {r}"));
            let last = r["last_at"].as_i64().unwrap_or_else(|| panic!("last_at: {r}"));
            assert!(first > 0 && first <= last, "times: {r}");
            (
                r["kind"].as_str().unwrap_or_else(|| panic!("kind: {r}")).to_string(),
                r["key"].as_str().unwrap_or_else(|| panic!("key: {r}")).to_string(),
                r["count"].as_i64().unwrap_or_else(|| panic!("count: {r}")),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn counts_at_both_doors_after_auth_and_the_owner_reads_them() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2cu{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let ws_path = home.join("ws-cu-billing");
    std::fs::create_dir_all(&ws_path).unwrap();
    let ws = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'cu-billing', ?2, 'cu-billing')",
            rusqlite::params![ws, ws_path.to_string_lossy()],
        )
        .unwrap();
    }
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-cu",
        HookPrincipal { workspace_uuid: ws.clone(), agent_address: "cu-billing".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let listener = k2_daemon::cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = k2_daemon::cell_uds::cell_socket_path(&sid);
    k2_daemon::cell_server::serve_cell(sid, listener, None);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(k2_daemon::session_token::is_agent_verb(VERB), "{VERB} is an agent verb on the cell socket");

    let all = ["X-K5-Cli-Via: fallthrough", "X-K5-Cli-Noun: msg", "X-K5-Would-Refuse: 1", "X-K5-Owner-Source: disk"];
    let here = format!("{VERB}?project=cu-billing");

    // ── nothing is counted before a credential checks out ────────────
    let r = tcp(port, &here, None, &all).await;
    assert_eq!(r.status, 403, "no token: {}", r.body);
    let r = tcp(port, &here, Some("not-a-real-token"), &all).await;
    assert_eq!(r.status, 403, "junk token: {}", r.body);
    let r = cell(&sock, VERB, "not-a-passport.0000", &all).await;
    assert_eq!(r.status, 403, "bad bearer: {}", r.body);
    let r = cell(&sock, "/cli/cli-usage", &passport, &all).await;
    assert_eq!(r.status, 403, "not an agent verb: {}", r.body);

    // ── TCP, the owner token ─────────────────────────────────────────
    tcp(port, &here, Some(OWNER), &all).await;
    // A workspace that doesn't resolve counts as `other`.
    tcp(port, &format!("{VERB}?project=no-such-workspace"), Some(OWNER), &["X-K5-Would-Refuse: 1"]).await;
    // Only the exact values count.
    tcp(
        port,
        &here,
        Some(OWNER),
        &["X-K5-Cli-Via: native", "X-K5-Cli-Noun: msg", "X-K5-Would-Refuse: true", "X-K5-Owner-Source: Disk"],
    )
    .await;
    tcp(port, &here, Some(OWNER), &["X-K5-Cli-Via: Fallthrough", "X-K5-Would-Refuse: 0", "X-K5-Owner-Source: env"]).await;
    // A bad noun counts as `other`.
    tcp(port, &here, Some(OWNER), &["X-K5-Cli-Via: fallthrough", "X-K5-Cli-Noun: Bad_Noun"]).await;

    // ── TCP, a passport: keyed by its own workspace ───────────────────
    tcp(port, VERB, Some(&passport), &["X-K5-Cli-Via: fallthrough", "X-K5-Cli-Noun: inbox", "X-K5-Would-Refuse: 1"]).await;

    // ── the cell socket ───────────────────────────────────────────────
    let r = cell(&sock, VERB, &passport, &["X-K5-Cli-Via: fallthrough", "X-K5-Cli-Noun: heartbeat", "X-K5-Owner-Source: disk"]).await;
    assert_ne!(r.status, 403, "the passport is authorized for {VERB}: {}", r.body);

    // In table order (kind, then key: a workspace id may sort either side of `other`).
    let mut want = vec![
        ("fallthrough".to_string(), "heartbeat".to_string(), 1),
        ("fallthrough".to_string(), "inbox".to_string(), 1),
        ("fallthrough".to_string(), "msg".to_string(), 1),
        ("fallthrough".to_string(), "other".to_string(), 1),
        ("owner_from_disk".to_string(), ws.clone(), 2),
        ("would_refuse".to_string(), "other".to_string(), 1),
        ("would_refuse".to_string(), ws.clone(), 2),
    ];
    want.sort();
    assert!(stored().is_empty(), "nothing is written on a request");

    // ── GET /cli/cli-usage: the owner's, after a flush ────────────────
    let r = tcp(port, "/cli/cli-usage", Some(&passport), &[]).await;
    assert_eq!(r.status, 403, "a passport over TCP: {}", r.body);
    let r = tcp(port, "/cli/cli-usage", None, &[]).await;
    assert_eq!(r.status, 403, "no token: {}", r.body);
    k2_core::connect_users::add_user("cu-member", "password123").expect("add_user");
    k2_core::connect_users::set_role("cu-member", k2_core::connect_users::Role::Member).expect("set_role");
    let member = k2_core::connect_users::create_session("cu-member");
    let r = tcp(port, "/cli/cli-usage", Some(&member), &[]).await;
    assert_eq!(r.status, 403, "a Member login: {}", r.body);
    assert!(stored().is_empty(), "a refused read writes nothing");

    let r = tcp(port, "/cli/cli-usage", Some(OWNER), &[]).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(rows_of(&r.json()), want);
    assert_eq!(stored(), want, "the read flushed the counts to the table");
    // A second read writes nothing more.
    let r = tcp(port, "/cli/cli-usage", Some(OWNER), &[]).await;
    assert_eq!(rows_of(&r.json()), want);
}
