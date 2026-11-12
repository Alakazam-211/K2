//! A `.pptx` deck opens live as a keyed doc, headless: the real dispatcher
//! (`test_harness`) and the real docs runtime (the vendored kit and the pptx
//! pack, a temp `~/.k2/docs.redb`), driven over HTTP with no client.
//!
//! What it pins (the Slides core):
//!   1. With the Slides core off, a deck is an any-file doc (tier 0), as
//!      before that core.
//!   2. With it on, a fresh deck opens live: `status` says tier 1, pack
//!      `pptx`, family `deck`, nothing declined; the pack's own verbs run
//!      through `/cli/docs/tool` (`slide list` names the slides by their
//!      titles), and the file on disk is untouched.
//!   3. A deck that was opened at tier 0 before the core was on stays an
//!      any-file doc (`stored_as_any_file`).
//!   4. `cores?path=` names the Slides core for a deck while it is on, with
//!      no live view for people (it opens the usual way).
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use k2_daemon::test_harness;
use k2_mpc_pptx::testgen::{deck, DeckSpec, Slide};
use rusqlite::params;
use serde_json::{json, Value as J};

const OWNER: &str = "owner-token-docs-pptx";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
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
    Resp { status, body }
}

fn json_of(r: &Resp) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", r.body))
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
    let r = http(port, "GET", &format!("/cli/docs/status?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(r.status, 200, "status {path}: {}", r.body);
    json_of(&r)
}

async fn flip(cores: &[(&str, bool)]) -> k2_daemon::docs::cores::Report {
    let after: std::collections::BTreeMap<String, bool> = cores.iter().map(|(c, on)| (c.to_string(), *on)).collect();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token"))
        .await
        .expect("flip")
}

/// Two titled slides, as a person's app would save them.
fn two_slide_deck() -> Vec<u8> {
    deck(&DeckSpec::new(vec![Slide::titled(256, "Quarterly review", ""), Slide::titled(257, "Goals", "Grow the pilot")]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pptx_opens_live_as_a_keyed_doc_headless() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-pptx-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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

    let ws = tmp.join("ws-deck");
    std::fs::create_dir_all(ws.join(".k2")).expect("ws");
    let ws = ws.canonicalize().expect("canonical ws");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'deckws', ?2, 'deckws')",
            params![uuid::Uuid::new_v4().to_string(), ws.to_string_lossy()],
        )
        .expect("seed project");
    }
    let bytes = two_slide_deck();
    let early = ws.join("early.pptx");
    std::fs::write(&early, &bytes).expect("early deck");
    let talk = ws.join("talk.pptx");
    std::fs::write(&talk, &bytes).expect("talk deck");
    let (early, talk) = (early.to_string_lossy().to_string(), talk.to_string_lossy().to_string());

    // ── 1. Slides off: a deck is an any-file doc ──
    let on = flip(&[("any-file", true)]).await;
    assert_eq!(on.changed, vec![("any-file".to_string(), true)], "{on:?}");
    let s = status(port, &early);
    assert_eq!(s["tier"], 0, "Slides off: tier 0: {s}");
    let r = http(port, "GET", &format!("/cli/docs/cores?token={OWNER}&path={}", enc(&talk)), None);
    assert_eq!(json_of(&r)["file"], json!({"core": "any-file", "on": true, "liveView": false}), "{}", r.body);

    // ── 2. Slides on: a fresh deck opens live, as a keyed doc ──
    let on = flip(&[("any-file", true), ("slides", true)]).await;
    assert_eq!(on.changed, vec![("slides".to_string(), true)], "{on:?}");
    let s = status(port, &talk);
    assert_eq!(s["tier"], 1, "a live doc: {s}");
    assert_eq!(s["pack"], "pptx", "{s}");
    assert_eq!(s["family"], "deck", "{s}");
    assert_eq!(s["declined"], J::Null, "the pack holds the deck: {s}");
    let r = http(
        port,
        "POST",
        &format!("/cli/docs/tool?token={OWNER}&path={}", enc(&talk)),
        Some(&json!({"argv": ["slide", "list"]}).to_string()),
    );
    assert_eq!(r.status, 200, "slide list: {}", r.body);
    let out = json_of(&r);
    assert_eq!(out["exit"], 0, "{out}");
    let listed = out["stdout"].as_str().expect("stdout");
    for title in ["Quarterly review", "Goals"] {
        assert!(listed.contains(title), "slide list names {title}: {listed}");
    }
    assert_eq!(std::fs::read(&talk).expect("the deck"), bytes, "a read never writes the file");

    // ── 3. A deck opened at tier 0 before the core stays an any-file doc ──
    let s = status(port, &early);
    assert_eq!(s["tier"], 0, "stays tier 0: {s}");
    assert!(s.to_string().contains("stored_as_any_file"), "says why: {s}");

    // ── 4. The read route: the Slides core, no live view for people ──
    let r = http(port, "GET", &format!("/cli/docs/cores?token={OWNER}&path={}", enc(&talk)), None);
    assert_eq!(json_of(&r)["file"], json!({"core": "slides", "on": true, "liveView": false}), "{}", r.body);
}
