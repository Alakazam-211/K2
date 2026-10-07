//! Thread sync — every Thread write, whoever sends it, reaches every live
//! view of that Thread over `WS /cli/overlay/events`, through the real
//! dispatcher (`test_harness`).
//!
//! Rosson 2026-10-04: a message added from a Zen Garden did not show in
//! the Thread on the Agents page (and the other way round). The daemon is
//! the one source of truth: each write route publishes one overlay frame,
//! and a client that missed frames catches up with `GET /cli/thread
//! ?since_seq=`. This test pins that contract headless:
//!
//!   1. Two subscribers on one conversation (two windows, or two people)
//!      both get a frame for every write route: `thread/post` as the
//!      human compose bar (Agents page, Zen, Home), as an agent
//!      (`k2 thread`), as another Connect user, and as an app guest;
//!      `thread/ask`, `thread/answer`, `thread/secret`, `thread/void`;
//!      and the card a human's compose prose resolves.
//!   2. The frame carries the same id and seq as the POST answer, so a
//!      client's own send and the echo merge by id.
//!   3. An app guest's socket sees the Thread frames of its room only
//!      (fail-closed: never another room's, never chatter).
//!   4. The socket is kept alive with server Pings (an idle Thread is not
//!      cut by the tunnel edge).
//!   5. `since_seq` returns exactly the items after a seq, including a
//!      card whose status changed, for the client's catch-up.
//!
//! ISOLATION: one test, temp HOME + agent shim (a compose post may wake
//! the pinned Chat; it must never spawn the real `claude`).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value as J;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const OWNER_TOKEN: &str = "owner-token-thread-sync";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> J {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(15))).expect("timeout");
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
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("no body: {text:?}"));
    Resp { status, body }
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev_home: Option<std::ffi::OsString>,
        prev_shim: Option<std::ffi::OsString>,
        prev_ping: Option<std::ffi::OsString>,
        tmp: std::path::PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.prev_home.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_shim.take() {
                Some(p) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", p),
                None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
            }
            match self.prev_ping.take() {
                Some(p) => std::env::set_var("K2_OVERLAY_WS_PING_MS", p),
                None => std::env::remove_var("K2_OVERLAY_WS_PING_MS"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let tmp = std::env::temp_dir().join(format!("k2-thread-sync-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let _restore = Restore {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        prev_ping: std::env::var_os("K2_OVERLAY_WS_PING_MS"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    // A short keepalive so contract 4 is seen in well under a second.
    std::env::set_var("K2_OVERLAY_WS_PING_MS", "200");
    f();
}

/// A workspace with a pinned Chat; its bare handle is a Thread address.
fn seed_thread_addr(handle: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/thread-sync-{handle}-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    conv
}

fn post_as(port: u16, token: &str, route: &str, body: J) -> J {
    let r = http(port, "POST", &format!("{route}?token={token}"), Some(&body.to_string()));
    assert_eq!(r.status, 200, "{route}: {}", r.body);
    let v = r.json();
    assert_eq!(v["ok"], true, "{route}: {}", r.body);
    v
}

fn login_member(port: u16, username: &str) -> String {
    let password = "correct-horse-battery-staple-9";
    let r = http(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"username":"{username}","password":"{password}"}}"#)),
    );
    assert_eq!(r.status, 200, "users/add: {}", r.body);
    let r = http(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(r#"{{"username":"{username}","password":"{password}"}}"#)),
    );
    assert_eq!(r.status, 200, "login: {}", r.body);
    r.json()["token"].as_str().expect("login token").to_string()
}

fn mint_guest(port: u16, name: &str, room: &str) -> String {
    let r = http(
        port,
        "POST",
        &format!("/cli/skin/users?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"username":"{name}"}}"#)),
    );
    assert_eq!(r.status, 200, "skin user add: {}", r.body);
    let r = http(
        port,
        "POST",
        &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"name":"{name}","caps":["thread:read","thread:post"],"rooms":["{room}"]}}"#)),
    );
    assert_eq!(r.status, 200, "mint: {}", r.body);
    let tok = r.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

async fn open_ws(port: u16, conv: &str, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={token}");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .unwrap_or_else(|e| panic!("overlay WS must open for {conv}: {e}"));
    ws
}

/// The next Text frame (Pings/Pongs skipped). Fails loudly after 5 s in
/// total (one deadline: the 200 ms keepalive must not keep it waiting).
async fn next_frame(ws: &mut Ws, what: &str) -> J {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = timeout(left, ws.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for overlay frame: {what}"))
            .unwrap_or_else(|| panic!("overlay WS closed while waiting for: {what}"))
            .unwrap_or_else(|e| panic!("overlay WS error while waiting for {what}: {e}"));
        match msg {
            Message::Text(t) => {
                let f: J = serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}"));
                // The Thread working strip's ephemeral `activity` frames
                // (prd-daemon-activity-and-thread-working-v1 S6) ride the
                // same socket; this test is about stored Thread writes.
                if f["collection"] == "activity" {
                    continue;
                }
                return f;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected overlay message while waiting for {what}: {other:?}"),
        }
    }
}

/// Every frame on `ws` until `id`'s frame arrives; returns that frame.
/// Frames for other ids may come first (a compose post's card update).
async fn frame_for(ws: &mut Ws, id: &str, what: &str) -> J {
    for _ in 0..8 {
        let f = next_frame(ws, what).await;
        if f["id"] == id {
            return f;
        }
    }
    panic!("no frame for id {id} ({what}) within 8 frames");
}

/// No Text frame within `ms` (Pings allowed).
async fn assert_no_frame(ws: &mut Ws, ms: u64, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return;
        }
        match timeout(left, ws.next()).await {
            Err(_) => return,
            Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => continue,
            Ok(Some(Ok(Message::Text(t)))) if t.contains(r#""collection":"activity""#) => continue,
            Ok(Some(Ok(Message::Text(t)))) => panic!("{what}: must not receive {t}"),
            Ok(other) => panic!("{what}: unexpected {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_thread_write_reaches_every_live_view() {
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let tag = &uuid::Uuid::new_v4().to_string()[..8];
        let sales = format!("tsync{tag}");
        let other = format!("tsyncother{tag}");
        let _seeded = seed_thread_addr(&sales);
        let _seeded_other = seed_thread_addr(&other);
        // A compose post wakes the agent; the first wake spawns its Chat
        // (the shim) and pins that new session, which moves the Thread to
        // a new conversation — the pin swap the client follows. Warm both
        // agents up first so every write below lands on one conversation.
        let pinned_after_warmup = |handle: &str| -> String {
            post_as(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": handle, "text": "warm-up", "via": "compose"
            }));
            let r = http(port, "GET", &format!("/cli/thread?token={OWNER_TOKEN}&addr={handle}&limit=1"), None);
            assert_eq!(r.status, 200, "{}", r.body);
            r.json()["conversation_id"].as_str().expect("conversation_id").to_string()
        };
        let conv = pinned_after_warmup(&sales);
        let other_conv = pinned_after_warmup(&other);
        let member_tok = login_member(port, &format!("pat{tag}"));
        let guest_tok = mint_guest(port, &format!("guest{tag}"), &sales);

        let (guest_seq, card_seqs) = futures_block(async {
            // Window A (the Agents page), window B (a Zen Garden), and the
            // app guest's room view. A fourth socket on another room.
            let mut a = open_ws(port, &conv, OWNER_TOKEN).await;
            let mut b = open_ws(port, &conv, &member_tok).await;
            let mut guest = open_ws(port, &conv, &guest_tok).await;
            let mut elsewhere = open_ws(port, &other_conv, OWNER_TOKEN).await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Each write: the POST answer's id/seq, and what both views see.
            let check = |resp: &J, f: &J, what: &str| {
                assert_eq!(f["collection"], "thread", "{what}: {f}");
                assert_eq!(f["id"], resp["id"], "{what}: frame id must be the POST id; {f} vs {resp}");
                assert_eq!(f["seq"], resp["seq"], "{what}: frame seq must be the POST seq; {f} vs {resp}");
                assert_eq!(f["doc"]["id"], resp["id"], "{what}: {f}");
                assert_eq!(resp["conversation_id"], conv.as_str(), "{what}: one conversation; {resp}");
            };

            // 1. The human compose bar (Agents page, Zen, Home all send this).
            let compose = post_as(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": sales, "text": "from the garden", "via": "compose"
            }));
            let id = compose["id"].as_str().expect("id").to_string();
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B"), (&mut guest, "app guest")] {
                let f = frame_for(ws, &id, &format!("compose post on {who}")).await;
                check(&compose, &f, &format!("compose post on {who}"));
                assert_eq!(f["doc"]["body"], "from the garden");
                assert_eq!(f["doc"]["via"], "compose");
            }

            // 2. The agent (`k2 thread sales "..."`).
            let agent = post_as(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": sales, "text": "on it", "from": sales
            }));
            let id = agent["id"].as_str().expect("id").to_string();
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B"), (&mut guest, "app guest")] {
                let f = frame_for(ws, &id, &format!("agent post on {who}")).await;
                check(&agent, &f, &format!("agent post on {who}"));
                assert_eq!(f["doc"]["via"], "thread");
            }

            // 3. Another person (a Connect Member) on the compose bar.
            let member = post_as(port, &member_tok, "/cli/thread/post", serde_json::json!({
                "addr": sales, "text": "me too", "via": "compose"
            }));
            let id = member["id"].as_str().expect("id").to_string();
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B")] {
                let f = frame_for(ws, &id, &format!("member post on {who}")).await;
                check(&member, &f, &format!("member post on {who}"));
                assert_eq!(f["doc"]["from"], format!("pat{tag}"), "member stamp: {f}");
            }
            frame_for(&mut guest, &id, "member post on app guest").await;

            // 4. An app guest posting into its room.
            let from_guest = post_as(port, &guest_tok, "/cli/thread/post", serde_json::json!({
                "addr": sales, "text": "hello from the app"
            }));
            let id = from_guest["id"].as_str().expect("id").to_string();
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B"), (&mut guest, "app guest")] {
                let f = frame_for(ws, &id, &format!("guest post on {who}")).await;
                check(&from_guest, &f, &format!("guest post on {who}"));
            }

            // 5. Cards: ask → answer, secret → void.
            let ask = post_as(port, OWNER_TOKEN, "/cli/thread/ask", serde_json::json!({
                "addr": sales, "prompt": "Ship?", "options": "Ship,Wait", "from": sales
            }));
            let ask_id = ask["id"].as_str().expect("id").to_string();
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B")] {
                let f = frame_for(ws, &ask_id, &format!("ask on {who}")).await;
                check(&ask, &f, &format!("ask on {who}"));
                assert_eq!(f["doc"]["choice"]["status"], "pending", "{f}");
            }
            let answered = post_as(port, &member_tok, "/cli/thread/answer", serde_json::json!({
                "addr": sales, "id": ask_id, "answer": "Ship"
            }));
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B")] {
                let f = frame_for(ws, &ask_id, &format!("answer on {who}")).await;
                check(&answered, &f, &format!("answer on {who}"));
                assert_eq!(f["doc"]["choice"]["status"], "answered", "{f}");
                assert_eq!(f["doc"]["choice"]["answer"], "Ship", "{f}");
            }

            let secret = post_as(port, OWNER_TOKEN, "/cli/thread/secret", serde_json::json!({
                "addr": sales, "name": "API_TOKEN", "from": sales
            }));
            let secret_id = secret["id"].as_str().expect("id").to_string();
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B")] {
                let f = frame_for(ws, &secret_id, &format!("secret on {who}")).await;
                check(&secret, &f, &format!("secret on {who}"));
            }
            let voided = post_as(port, OWNER_TOKEN, "/cli/thread/void", serde_json::json!({
                "addr": sales, "id": secret_id
            }));
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B")] {
                let f = frame_for(ws, &secret_id, &format!("void on {who}")).await;
                check(&voided, &f, &format!("void on {who}"));
                assert_eq!(f["doc"]["secret"]["status"], "voided", "{f}");
            }

            // 6. A human's compose prose resolves a pending card: the card's
            //    new status is a frame too, not only the prose itself.
            let ask2 = post_as(port, OWNER_TOKEN, "/cli/thread/ask", serde_json::json!({
                "addr": sales, "prompt": "Color?", "options": "Red,Blue", "from": sales
            }));
            let ask2_id = ask2["id"].as_str().expect("id").to_string();
            frame_for(&mut a, &ask2_id, "ask2 on window A").await;
            frame_for(&mut b, &ask2_id, "ask2 on window B").await;
            let prose = post_as(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": sales, "text": "Blue", "via": "compose"
            }));
            let prose_id = prose["id"].as_str().expect("id").to_string();
            for (ws, who) in [(&mut a, "window A"), (&mut b, "window B")] {
                frame_for(ws, &prose_id, &format!("prose on {who}")).await;
                let f = frame_for(ws, &ask2_id, &format!("prose-answered card on {who}")).await;
                assert_eq!(f["doc"]["choice"]["status"], "answered", "{f}");
                assert_eq!(f["doc"]["choice"]["answer"], "Blue", "{f}");
                assert_eq!(f["seq"], ask2["seq"], "a card keeps its seq: {f}");
            }

            // 3 (fail-closed). Another room's socket saw none of it.
            assert_no_frame(&mut elsewhere, 300, "a socket on another conversation").await;
            // ...and a post there never reaches this room's guest.
            let there = post_as(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": other, "text": "other room", "via": "compose"
            }));
            let f = next_frame(&mut elsewhere, "post on the other room").await;
            assert_eq!(f["id"], there["id"], "{f}");
            // Drain this room's own remaining frames for 500 ms: Thread
            // frames of this room only.
            let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
            loop {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                match timeout(left, guest.next()).await {
                    Err(_) => break,
                    Ok(Some(Ok(Message::Text(t)))) => {
                        let v: J = serde_json::from_str(&t).expect("frame JSON");
                        assert_ne!(v["id"], there["id"], "app guest must never see another room's Thread: {t}");
                        assert_eq!(v["collection"], "thread", "app guest gets Thread frames only: {t}");
                    }
                    Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => continue,
                    Ok(other) => panic!("app guest socket: unexpected {other:?}"),
                }
            }

            // 4. Keepalive: an idle socket gets a server Ping.
            let mut idle = open_ws(port, &conv, OWNER_TOKEN).await;
            let got_ping = timeout(Duration::from_secs(3), async {
                loop {
                    match idle.next().await {
                        Some(Ok(Message::Ping(_))) => return true,
                        Some(Ok(_)) => continue,
                        other => panic!("idle overlay WS ended before a Ping: {other:?}"),
                    }
                }
            })
            .await
            .expect("an idle overlay WS must get a server Ping (keepalive)");
            assert!(got_ping);

            let seq = |v: &J| v["seq"].as_i64().unwrap_or_else(|| panic!("no seq: {v}"));
            (seq(&from_guest), vec![seq(&ask), seq(&secret), seq(&ask2), seq(&prose)])
        });

        // 5. Catch-up: since_seq returns what came after, cards with their
        //    current status included.
        let r = http(
            port,
            "GET",
            &format!("/cli/thread?token={OWNER_TOKEN}&addr={sales}&since_seq={guest_seq}&limit=0"),
            None,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        assert_eq!(v["conversation_id"], conv, "{}", r.body);
        let items = v["items"].as_array().expect("items");
        let seqs: Vec<i64> = items.iter().map(|it| it["seq"].as_i64().expect("seq")).collect();
        assert_eq!(seqs, card_seqs, "since the guest post → ask, secret, ask2, prose; {}", r.body);
        assert_eq!(items[0]["doc"]["choice"]["status"], "answered", "{}", r.body);
        assert_eq!(items[1]["doc"]["secret"]["status"], "voided", "{}", r.body);
        assert_eq!(items[2]["doc"]["choice"]["status"], "answered", "{}", r.body);
    });
}
