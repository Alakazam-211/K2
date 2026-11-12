//! A tier-0 doc's new version reaches every client, headless: the real
//! dispatcher (`test_harness`), the real docs runtime (the vendored kit, a
//! temp `~/.k2`), driven over HTTP, with the session bus as the clients get
//! it.
//!
//! An any-file doc (a `.obj` 3D model) open in a tab: another writer runs
//! `k2 doc write --base V` three times. The file and `status` moved to the
//! last version, but the open tab kept drawing the first, because nothing
//! told it. What it pins:
//!   1. each write that records a version broadcasts `doc_version_changed`
//!      with the workspace, the path and the new version, once (the kit's
//!      own write to the file is an echo, not a second version);
//!   2. a write of the same bytes records nothing and says nothing;
//!   3. a restore is a new version and is told the same way;
//!   4. another program's save, seen by the watcher, is told the same way.
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use k2_daemon::session_events::SessionEvent;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};
use tokio::sync::broadcast;

const OWNER: &str = "owner-token-docs-tier0-version";

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    match s.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_else(|| panic!("no body: {text:?}"));
    (status, body)
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn status(port: u16, path: &str) -> J {
    let (st, body) = http(port, "GET", &format!("/cli/docs/status?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(st, 200, "status {path}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("status JSON ({e}): {body}"))
}

fn post(port: u16, route: &str, body: J) -> J {
    let (st, text) = http(port, "POST", &format!("/cli/docs/{route}?token={OWNER}"), Some(&body.to_string()));
    assert_eq!(st, 200, "{route}: {text}");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{route} JSON ({e}): {text}"))
}

/// The `doc_version_changed` frames for `rel` that arrive within `ms`
/// (other kinds skipped), as their wire JSON.
async fn version_events(rx: &mut broadcast::Receiver<SessionEvent>, rel: &str, ms: u64) -> Vec<J> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Err(_) => return out,
            Ok(Ok(ev)) => {
                let v = serde_json::to_value(&ev).expect("event JSON");
                if v["kind"] == "doc_version_changed" && v["rel"] == rel {
                    out.push(v);
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => panic!("the test's receiver lagged by {n}"),
            Ok(Err(broadcast::error::RecvError::Closed)) => panic!("the session bus closed"),
        }
    }
}

/// One cube face per call: a different model each version.
fn model(n: u32) -> String {
    format!("# version {n}\nv 0 0 0\nv {n} 0 0\nv 0 {n} 0\nf 1 2 3\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_new_version_of_an_any_file_doc_is_told_to_clients() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-t0-version-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("home");
    let tmp = tmp.canonicalize().expect("canonical home");
    std::env::set_var("HOME", &tmp);
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }

    let daemon = test_harness::start(OWNER).await;
    let port = daemon.port;
    tokio::task::spawn_blocking(k2_daemon::docs::start).await.expect("docs start");
    assert!(k2_daemon::docs::live_ready(), "the docs runtime is up: {}", k2_daemon::docs::pool_json());

    let ws_dir = tmp.join("ws-models");
    std::fs::create_dir_all(ws_dir.join(".k2")).expect("ws");
    std::fs::create_dir_all(ws_dir.join("play")).expect("play folder");
    let ws_dir = ws_dir.canonicalize().expect("canonical ws");
    let workspace = ws_dir.to_string_lossy().to_string();
    let project_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'models', ?2, 'models')",
            params![project_id, workspace],
        )
        .expect("seed project");
    }
    let rel = "play/house.obj";
    let file = ws_dir.join(rel);
    std::fs::write(&file, model(1)).expect("the model");
    let path = file.to_string_lossy().to_string();

    let on = tokio::task::spawn_blocking(|| {
        let after: std::collections::BTreeMap<String, bool> =
            [("any-file", true)].iter().map(|(c, on)| (c.to_string(), *on)).collect();
        k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token")
    })
    .await
    .expect("flip");
    assert!(k2_daemon::docs::cores::is_on("any-file"), "{on:?}");

    // The open tab: the doc is loaded, at its first version.
    let first = status(port, &path);
    assert_eq!(first["tier"], 0, "a .obj is an any-file doc: {first}");
    let v1 = first["version"].as_u64().unwrap_or_else(|| panic!("a version number: {first}"));

    // Subscribed after the open: only what the writes cause.
    let mut rx = k2_daemon::session_events::subscribe();

    // ── 1. Three writes, three versions, each told once ──
    let mut base = v1;
    for n in 2..=4u32 {
        let w = post(port, "write", json!({ "path": path, "base": base.to_string(), "text": model(n) }));
        let next = w["result"]["version"].as_u64().unwrap_or_else(|| panic!("the new version: {w}"));
        assert!(next > base, "a new version: {w}");
        assert_eq!(w["result"]["changed"], true, "{w}");
        let got = version_events(&mut rx, rel, 1_500).await;
        assert_eq!(
            got,
            vec![json!({"kind": "doc_version_changed", "projectId": project_id, "workspace": workspace, "rel": rel, "version": next})],
            "write {n}: one event at the new version"
        );
        base = next;
    }
    assert_eq!(status(port, &path)["version"], json!(base), "status is at the last write");

    // ── 2. The same bytes again: no version, nothing told ──
    let same = post(port, "write", json!({ "path": path, "base": base.to_string(), "text": model(4) }));
    assert_eq!(same["result"]["changed"], false, "{same}");
    assert_eq!(version_events(&mut rx, rel, 1_000).await, Vec::<J>::new(), "equal bytes are not a version");

    // ── 3. A restore is a version too ──
    let r = post(port, "restore", json!({ "path": path, "version": v1 }));
    let restored = r["result"]["version"].as_u64().unwrap_or_else(|| panic!("the restore's version: {r}"));
    assert!(restored > base, "a restore is a forward version: {r}");
    let got = version_events(&mut rx, rel, 1_500).await;
    assert_eq!(got.len(), 1, "one event for the restore: {got:?}");
    assert_eq!(got[0]["version"], json!(restored));
    base = restored;

    // ── 4. Another program saves the file: the watcher's version is told ──
    std::fs::write(&file, model(9)).expect("an outside save");
    k2_daemon::session_events::emit_fs_changed(&workspace, [path.clone()]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let outside = loop {
        let got = version_events(&mut rx, rel, 500).await;
        if !got.is_empty() {
            break got;
        }
        assert!(tokio::time::Instant::now() < deadline, "no event for the outside save");
    };
    assert_eq!(outside.len(), 1, "{outside:?}");
    let v_out = outside[0]["version"].as_u64().unwrap_or_else(|| panic!("a version: {}", outside[0]));
    assert!(v_out > base, "the outside save is the newest version: {}", outside[0]);
    assert_eq!(status(port, &path)["version"], json!(v_out), "status agrees");
    assert_eq!(std::fs::read_to_string(&file).expect("file"), model(9));
}
