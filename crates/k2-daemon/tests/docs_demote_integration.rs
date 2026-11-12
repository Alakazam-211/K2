//! A doc stored as live text under a name that now routes to tier 0 opens
//! through K2's normal open, headless: the real dispatcher
//! (`test_harness`), the real docs runtime (the vendored kit and the text
//! pack, a temp `~/.k2`), driven over HTTP.
//!
//! A `.kicad_sch` opened as live text before text v0.2.22 moved the
//! extension to not-text is still a text doc in the store. Its tier-0 open
//! used to be the kit's `PackMismatch`, which K2 answered with a 409. Now
//! K2 demotes it (kit v0.17.0), once:
//!   1. `status` answers 200 at tier 0 (no 409);
//!   2. the same doc id, now a tier-0 doc;
//!   3. every earlier version is a tier-0 version with the same bytes;
//!   4. the file on disk is the live state (a change the file never got
//!      included), and it is the current version;
//!   5. a second open doesn't demote again: nothing moves.
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use k2_daemon::docs::caller::DocCaller;
use k2_daemon::test_harness;
use k2_mpc_base::session::{Stay, WriteOutcome};
use k2_mpc_base::store::{DocKey, BLOB_DOC_PACK};
use rusqlite::params;
use serde_json::Value as J;

const OWNER: &str = "owner-token-docs-demote";

fn http(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
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

fn get(port: u16, route: &str, path: &str) -> J {
    let (st, body) = http(port, "GET", &format!("/cli/docs/{route}?token={OWNER}&path={}", enc(path)));
    assert_eq!(st, 200, "{route} {path}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{route} JSON ({e}): {body}"))
}

const SCH: &str = "(kicad_sch (version 20231120) (generator eeschema)\n\
  (uuid \"5b0f6c1e-0d6a-4c39-9d43-1f0b3c6f5a10\")\n\
  (paper \"A4\")\n\
)\n";

fn sch_with(lines: &[&str]) -> String {
    let mut out = SCH[..SCH.len() - 2].to_string();
    for l in lines {
        out.push_str("  ");
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(")\n");
    out
}

const R1: &str = "(symbol (lib_id \"Device:R\") (at 50 50 0) (uuid \"a1c2e3f4-0000-4000-8000-000000000001\"))";
const C1: &str = "(symbol (lib_id \"Device:C\") (at 80 50 0) (uuid \"a1c2e3f4-0000-4000-8000-000000000002\"))";
const D1: &str = "(symbol (lib_id \"Device:D\") (at 80 80 0) (uuid \"a1c2e3f4-0000-4000-8000-000000000003\"))";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kicad_schematic_live_as_text_opens_at_tier_0_headless() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-demote-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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

    let ws_dir = tmp.join("ws-board");
    std::fs::create_dir_all(ws_dir.join(".k2")).expect("ws");
    std::fs::create_dir_all(ws_dir.join("board")).expect("board folder");
    let ws_dir = ws_dir.canonicalize().expect("canonical ws");
    let project_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'boardws', ?2, 'boardws')",
            params![project_id, ws_dir.to_string_lossy()],
        )
        .expect("seed project");
    }
    let rel = "board/main.kicad_sch";
    let file = ws_dir.join(rel);
    std::fs::write(&file, SCH).expect("the schematic");
    let path = file.to_string_lossy().to_string();

    let on = tokio::task::spawn_blocking(|| {
        let after: std::collections::BTreeMap<String, bool> =
            [("any-file", true), ("text", true)].iter().map(|(c, on)| (c.to_string(), *on)).collect();
        k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token")
    })
    .await
    .expect("flip");
    assert!(k2_daemon::docs::cores::is_on("any-file"), "{on:?}");

    // ── Stored as live text, the way it was opened before text v0.2.22 ──
    let key = DocKey { space: project_id.clone(), path: rel.to_string() };
    let live = sch_with(&[R1, C1, D1]);
    let (before, id) = {
        let (key, file, ws_dir, live) = (key.clone(), file.clone(), ws_dir.clone(), live.clone());
        tokio::task::spawn_blocking(move || {
            let rt = k2_daemon::docs::runtime().expect("the runtime");
            let s = rt.open(&key, &file, &ws_dir, Arc::new(k2_mpc_text::PACK)).expect("a text open");
            let person = DocCaller::Person { key: "owner".into(), via: "cli" }.participant(None);
            let c = s.join(person, Stay::Held).expect("the owner joins").conn;
            let write = |to: &str| {
                let v = s.read(c).expect("read").version;
                let out = s.write_base(c, k2_mpc_text::ROOT, v, to).expect("write");
                assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
            };
            write(&sch_with(&[R1]));
            s.flush(c).expect("flush");
            s.name_version(c, "R1 placed", None).expect("named");
            write(&sch_with(&[R1, C1]));
            s.flush(c).expect("flush");
            s.name_version(c, "C1 placed", None).expect("named");
            // A change the file never got (the kit's write-back waits for a
            // quiet second; the open below doesn't depend on whether it ran).
            write(&live);
            let before: Vec<Vec<u8>> =
                s.versions(c).expect("versions").into_iter().map(|v| s.file_at(c, v.id).expect("file_at").bytes).collect();
            let store = rt.store().expect("the store");
            let id = store.find(&key).expect("find").expect("stored");
            assert_eq!(store.meta(id).expect("meta").pack, "text", "a live text doc");
            (before, id)
        })
        .await
        .expect("the text doc")
    };
    assert!(before.len() >= 3, "the import and two named versions: {}", before.len());

    // ── 1. K2's normal open: 200 at tier 0, no 409 ──
    let status = get(port, "status", &path);
    assert_eq!(status["tier"], 0, "{status}");
    assert_eq!(status["pack"], "blob", "{status}");
    assert!(status.get("fallback").is_none(), "a demoted doc is a plain tier-0 doc, no fallback: {status}");

    // ── 2-4. Same doc id, every version, the live state on disk and current ──
    let versions = get(port, "versions", &path)["versions"].as_array().expect("versions").len();
    assert!(versions >= before.len(), "{versions} tier-0 versions for {} text versions", before.len());
    assert_eq!(std::fs::read(&file).expect("file"), live.as_bytes(), "the file is the live state");
    let check = {
        let (key, live, before) = (key.clone(), live.clone(), before.clone());
        move || {
            let rt = k2_daemon::docs::runtime().expect("the runtime");
            let store = rt.store().expect("the store");
            assert_eq!(store.find(&key).expect("find"), Some(id), "the same doc id");
            assert_eq!(store.meta(id).expect("meta").pack, BLOB_DOC_PACK, "a tier-0 doc now");
            let blob = rt.get_blob(&key).expect("the tier-0 doc is loaded");
            let person = DocCaller::Person { key: "owner".into(), via: "cli" }.participant(None);
            let c = blob.join(person, Stay::Held).expect("join").conn;
            let read = |r: k2_mpc_base::session::BlobRead| {
                let mut out = Vec::new();
                let mut reader = r.reader;
                reader.read_to_end(&mut out).expect("bytes");
                out
            };
            let held: Vec<Vec<u8>> = blob
                .versions(c)
                .expect("versions")
                .into_iter()
                .map(|(v, _)| read(blob.read_version(c, v).expect("read_version")))
                .collect();
            for (i, b) in before.iter().enumerate() {
                assert!(held.contains(b), "text version #{i} isn't a tier-0 version:\n{}", String::from_utf8_lossy(b));
            }
            assert_eq!(read(blob.read(c).expect("read")), live.as_bytes(), "the live state is current");
            let _ = blob.leave(c);
            held.len()
        }
    };
    let held = tokio::task::spawn_blocking(check.clone()).await.expect("after the open");
    assert_eq!(held, versions, "the route lists every version");

    // ── 5. A second open never demotes again ──
    let again = get(port, "status", &path);
    assert_eq!(again["tier"], 0, "{again}");
    assert_eq!(again["version"], status["version"], "nothing moved: {again}");
    let held_again = tokio::task::spawn_blocking(check).await.expect("after the second open");
    assert_eq!(held_again, held, "no version was added");
    assert_eq!(std::fs::read(&file).expect("file"), live.as_bytes());
}
