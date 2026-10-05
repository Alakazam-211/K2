//! Zen S2 — `GET /cli/thread/latest?addrs=` (prd-zen-mode-v1 Z41, vs-live
//! Z66, test T2.1), through the real dispatcher (`test_harness`).
//!
//! Three addrs with one unknown give three items, one carrying `error`;
//! previews are cut at 140 chars on the server; a choice card previews as
//! "Asked: …" and a secret card as "Asked for a secret"; an empty thread is
//! an item with null fields; > 50 addrs and no addrs are 400; POST is 405;
//! an app pass gets 403 here and `zen_local_only` on `/cli/zen/*`; a
//! Connect Member gets 200 (Member GET row).
//!
//! ISOLATION: one test, temp HOME + agent shim (Thread posts never spawn
//! the real `claude`).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use k2_core::connect_users;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::routes::route_policy::{self, Floor};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value as J;

const OWNER_TOKEN: &str = "owner-token-thread-latest-zen";

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
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_else(|| panic!("no body: {text:?}"));
    Resp { status, body }
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev_home: Option<std::ffi::OsString>,
        prev_shim: Option<std::ffi::OsString>,
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
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let tmp = std::env::temp_dir().join(format!("k2-thread-latest-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    f();
}

/// A workspace with a pinned Chat, so its bare handle is a Thread address.
fn seed_thread_addr(handle: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/thread-latest-{handle}-{id}");
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

fn post(port: u16, route: &str, body: J) -> J {
    let r = http(port, "POST", &format!("{route}?token={OWNER_TOKEN}"), Some(&body.to_string()));
    assert_eq!(r.status, 200, "{route}: {}", r.body);
    r.json()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t2_1_thread_latest_previews_errors_limits_and_refusals() {
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let tag = &uuid::Uuid::new_v4().to_string()[..8];
        let (h1, h2, h3) = (format!("zlt1{tag}"), format!("zlt2{tag}"), format!("zlt3{tag}"));
        let conv1 = seed_thread_addr(&h1);
        let conv2 = seed_thread_addr(&h2);
        let conv3 = seed_thread_addr(&h3);

        // Policy: a Member GET row, never POST.
        let row = route_policy::lookup("/cli/thread/latest").expect("thread/latest is classified");
        assert_eq!(row.get, Some(Floor::Member));
        assert_eq!(row.post, None);

        let long = format!("Pushed   the fix.\n\nDetails: {}", "word ".repeat(80));
        post(port, "/cli/thread/post", serde_json::json!({ "addr": h1, "text": "older message" }));
        post(port, "/cli/thread/post", serde_json::json!({ "addr": h1, "text": long }));
        post(port, "/cli/thread/ask", serde_json::json!({ "addr": h2, "prompt": "Want me to ship it?", "options": "Ship,Wait" }));

        let r = http(port, "GET", &format!("/cli/thread/latest?token={OWNER_TOKEN}&addrs={h1},{h2},nosuch{tag},{h3}"), None);
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        assert_eq!(v["ok"], true);
        let items = v["items"].as_array().expect("items");
        assert_eq!(items.len(), 4, "one item per addr, in order: {v}");

        let a = &items[0];
        assert_eq!(a["addr"], h1.as_str());
        assert_eq!(a["conversationId"], conv1.as_str());
        assert_eq!(a["kind"], "text");
        assert!(a["seq"].as_i64().is_some_and(|s| s >= 2), "newest seq: {a}");
        assert!(a["at"].as_i64().is_some_and(|t| t > 0), "{a}");
        assert!(a["from"].is_string(), "{a}");
        assert!(a.get("via").is_some(), "{a}");
        let p = a["preview"].as_str().expect("preview");
        assert!(p.chars().count() <= 140, "preview cut at 140 chars, got {}: {p:?}", p.chars().count());
        assert!(p.ends_with('…'), "a cut preview ends with an ellipsis: {p:?}");
        assert!(p.starts_with("Pushed the fix. Details: word"), "whitespace collapsed: {p:?}");

        let b = &items[1];
        assert_eq!(b["conversationId"], conv2.as_str());
        assert_eq!(b["kind"], "choice");
        assert_eq!(b["preview"], "Asked: Want me to ship it?", "{b}");

        let c = &items[2];
        assert_eq!(c["addr"], format!("nosuch{tag}"));
        assert!(c["error"]["code"].is_string(), "an unknown addr is an item with error: {c}");
        assert!(c.get("preview").is_none(), "{c}");

        let d = &items[3];
        assert_eq!(d["conversationId"], conv3.as_str());
        for k in ["seq", "at", "from", "via", "kind", "preview"] {
            assert!(d[k].is_null(), "an empty thread has null {k}: {d}");
        }

        // A secret card never previews its name or value.
        post(port, "/cli/thread/secret", serde_json::json!({ "addr": h1, "name": "STRIPE_KEY", "prompt": "Paste the live key" }));
        let r = http(port, "GET", &format!("/cli/thread/latest?token={OWNER_TOKEN}&addrs={h1}"), None);
        let v = r.json();
        assert_eq!(v["items"][0]["kind"], "secret", "{v}");
        assert_eq!(v["items"][0]["preview"], "Asked for a secret", "{v}");
        assert!(!r.body.contains("STRIPE_KEY"), "secret name must not leak into previews: {}", r.body);

        // Limits.
        let many: Vec<String> = (0..51).map(|i| format!("a{i}")).collect();
        let r = http(port, "GET", &format!("/cli/thread/latest?token={OWNER_TOKEN}&addrs={}", many.join(",")), None);
        assert_eq!(r.status, 400, "51 addrs must be refused: {}", r.body);
        let r = http(port, "GET", &format!("/cli/thread/latest?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 400, "no addrs: {}", r.body);
        let r = http(port, "POST", &format!("/cli/thread/latest?token={OWNER_TOKEN}"), Some("{}"));
        assert_eq!(r.status, 405, "GET only: {}", r.body);
        let r = http(port, "GET", &format!("/cli/thread/latest?addrs={h1}"), None);
        assert_eq!(r.status, 403, "no token: {}", r.body);

        // A Connect Member reads previews (Member row).
        connect_users::add_user("zlmember", "password123").expect("add member");
        let member = connect_users::create_session("zlmember");
        let r = http(port, "GET", &format!("/cli/thread/latest?token={member}&addrs={h2}"), None);
        assert_eq!(r.status, 200, "member: {}", r.body);
        assert_eq!(r.json()["items"][0]["kind"], "choice");
        // ...but never this computer's Zen.
        let r = http(port, "GET", &format!("/cli/zen/status?token={member}"), None);
        assert_eq!(r.status, 403, "{}", r.body);
        assert_eq!(r.json()["error"], "zen_local_only");

        // An app pass: refused on thread/latest and on /cli/zen/*.
        let r = http(port, "POST", &format!("/cli/skin/users?token={OWNER_TOKEN}"), Some(r#"{"username":"zlguest"}"#));
        assert_eq!(r.status, 200, "skin user: {}", r.body);
        let r = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(&serde_json::json!({ "name": "zl-pass", "caps": ["thread:read"], "rooms": [h1] }).to_string()),
        );
        assert_eq!(r.status, 200, "mint: {}", r.body);
        let pass = r.json()["token"].as_str().expect("pass").to_string();
        assert!(pass.starts_with("k2skn_"), "{pass}");
        let r = http(port, "GET", &format!("/cli/thread?token={pass}&addr={h1}"), None);
        assert_eq!(r.status, 200, "the pass itself works on /cli/thread: {}", r.body);
        let r = http(port, "GET", &format!("/cli/thread/latest?token={pass}&addrs={h1}"), None);
        assert_eq!(r.status, 403, "app pass on thread/latest: {}", r.body);
        for p in ["/cli/zen/get", "/cli/zen/status", "/cli/zen/validate"] {
            let r = http(port, "GET", &format!("{p}?token={pass}"), None);
            assert_eq!(r.status, 403, "app pass on {p}: {}", r.body);
            assert_eq!(r.json()["error"], "zen_local_only", "{p}: {}", r.body);
        }
        let r = http(port, "POST", &format!("/cli/zen/setup?token={pass}"), Some("{}"));
        assert_eq!(r.status, 403, "{}", r.body);
        assert!(!k2_core::zen::is_set_up(), "a refused setup must not set Zen up");
    });
}
