//! stash-sync-v1 (0.46.7) through the real dispatcher: the feature key,
//! `/cli/me/stash*` per person on the Homes device registry, the agent /
//! app-pass / API-key refusals, the 50 cap, device removal (410) and the
//! person-scoped `stash_changed` frame. Contract:
//! `.k2/prds/contract-stash-sync-v1.md`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};

const OWNER_TOKEN: &str = "owner-token-stash-sync-0467";

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> J {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code: {}", self.body)).to_string()
    }
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).expect("timeout");
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
        Err(e) => panic!("{method} {path}: read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn ok(port: u16, method: &str, path: &str, body: Option<J>) -> J {
    let b = body.map(|b| b.to_string());
    let r = http(port, method, path, b.as_deref());
    assert_eq!(r.status, 200, "{method} {path}: {}", r.body);
    r.json()
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce(&Path)>(f: F) {
    struct Restore {
        _g: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<std::ffi::OsString>,
        prev_shim: Option<std::ffi::OsString>,
        tmp: PathBuf,
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
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let tmp = std::env::temp_dir().join(format!("k2-stash-sync-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    std::fs::create_dir_all(tmp.join(".k2")).expect(".k2");
    let shim = shim_dir.join("claude");
    k2_core::test_env::write_executable(&shim, "#!/bin/sh\nexec cat\n");
    let tmp = dunce::canonicalize(&tmp).expect("canonical tmp");
    let _restore = Restore {
        _g: g,
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    k2_core::test_isolation::assert_isolated_from_prod();
    f(&tmp);
}

/// The test DB is one per process: forget what an earlier test left.
fn isolate() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch("DELETE FROM stash_items; DELETE FROM stash_people; DELETE FROM homes_devices;")
        .expect("isolate stash");
}

fn login(port: u16, username: &str) -> String {
    ok(port, "POST", &format!("/cli/users/add?token={OWNER_TOKEN}"), Some(json!({ "username": username, "password": "password123" })));
    let r = ok(port, "POST", "/cli/auth/login", Some(json!({ "username": username, "password": "password123" })));
    r["token"].as_str().expect("token").to_string()
}

fn seed_workspace(home: &Path, handle: &str) -> String {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).expect("workspace dir");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .expect("seed project");
    id
}

fn passport(workspace_uuid: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: workspace_uuid.to_string(), agent_address: "ss-agent".to_string() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn app_pass(port: u16, name: &str, room: &str) -> String {
    let r = http(port, "POST", &format!("/cli/skin/users?token={OWNER_TOKEN}"), Some(&format!(r#"{{"username":"{name}"}}"#)));
    assert_eq!(r.status, 200, "skin user add: {}", r.body);
    let r = http(
        port,
        "POST",
        &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"name":"{name}","caps":["thread:read","thread:post"],"rooms":["{room}"]}}"#)),
    );
    assert_eq!(r.status, 200, "mint app pass: {}", r.body);
    let tok = r.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

fn register(port: u16, token: &str, device: &str, name: &str, platform: &str) -> J {
    ok(
        port,
        "POST",
        &format!("/cli/me/homes/devices/register?token={token}"),
        Some(json!({ "deviceId": device, "name": name, "platform": platform })),
    )
}

fn push(port: u16, token: &str, device: &str, changes: J) -> Resp {
    http(
        port,
        "POST",
        &format!("/cli/me/stash/push?token={token}"),
        Some(&json!({ "deviceId": device, "changes": changes }).to_string()),
    )
}

fn pull(port: u16, token: &str, device: &str, since: Option<i64>) -> Resp {
    let since = since.map(|s| format!("&since={s}")).unwrap_or_default();
    http(port, "GET", &format!("/cli/me/stash?token={token}&deviceId={device}{since}"), None)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn uid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

fn rec(n: u32, text: &str, at: i64) -> J {
    json!({
        "v": 1, "id": uid(n), "text": text, "attachments": [], "quotes": [],
        "source": { "server_id": "rosson.k2.dev", "server_label": null, "user": "rosson", "agent": null, "agent_name": null },
        "created_at": at, "updated_at": at, "deleted_at": null
    })
}

fn texts(v: &J) -> Vec<String> {
    let mut out: Vec<String> = v["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter(|c| c["deleted_at"].is_null())
        .map(|c| c["text"].as_str().expect("text").to_string())
        .collect();
    out.sort();
    out
}

// ── 1. Routes ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stash_routes_sync_one_person_across_devices_and_refuse_everyone_else() {
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate();

        let boot = ok(port, "GET", "/boot-status", None);
        assert!(boot["features"].as_array().expect("features").iter().any(|f| f == "stash-sync-v1"), "{boot}");

        // The phone: a login the owner linked = the owner person. Gina: her own.
        let phone = login(port, "ssphone");
        ok(port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(json!({ "accounts": ["ssphone"] })));
        let gina = login(port, "ssgina");

        // deviceId is required; an unregistered device may read.
        let r = http(port, "GET", &format!("/cli/me/stash?token={OWNER_TOKEN}"), None);
        assert_eq!((r.status, r.code().as_str()), (400, "usage"), "{}", r.body);
        let v = pull(port, OWNER_TOKEN, "desk-1", None).json();
        assert_eq!(v, json!({ "ok": true, "cursor": 0, "full": true, "changes": [] }));
        // Pushing before registering in the Homes registry is refused.
        let r = push(port, OWNER_TOKEN, "desk-1", json!([rec(1, "draft", now_ms())]));
        assert_eq!((r.status, r.code().as_str()), (409, "device_not_registered"), "{}", r.body);

        // The desktop registers (Homes registry) and pushes a stash with a file it holds.
        register(port, OWNER_TOKEN, "desk-1", "Rosson's MacBook", "macos");
        let t = now_ms();
        let mut r1 = rec(1, "review the deploy log", t);
        r1["attachments"] = json!([{ "id": "a1", "name": "shot.png", "type": "image/png", "size": 48213, "device_id": "desk-1", "path": "/Users/r/Desktop/shot.png" }]);
        let pr = push(port, OWNER_TOKEN, "desk-1", json!([r1.clone(), rec(2, "second", t)])).json();
        assert_eq!((pr["cursor"].as_i64(), pr["applied"].as_i64(), pr["superseded"].as_i64()), (Some(1), Some(2), Some(0)), "{pr}");
        assert_eq!(pr["rejected"], json!([]));

        // The linked phone sees the same stash: paths and devices, never bytes or names.
        register(port, &phone, "phone-1", "Rosson's iPhone", "ios");
        let v = pull(port, &phone, "phone-1", None).json();
        assert_eq!((v["cursor"].as_i64(), v["full"].as_bool()), (Some(1), Some(true)), "{v}");
        assert_eq!(texts(&v), vec!["review the deploy log", "second"]);
        let got = v["changes"].as_array().unwrap().iter().find(|c| c["id"] == uid(1).as_str()).expect("r1").clone();
        assert_eq!(got["attachments"][0], json!({ "id": "a1", "name": "shot.png", "type": "image/png", "size": 48213, "device_id": "desk-1", "path": "/Users/r/Desktop/shot.png" }));
        assert_eq!(got["updated_by"], "desk-1");
        // The device name comes from the registry join, not the stash.
        let devs = ok(port, "GET", &format!("/cli/me/homes/devices?token={}&deviceId=phone-1", phone), None);
        let desk = devs["devices"].as_array().unwrap().iter().find(|d| d["deviceId"] == "desk-1").expect("desk listed").clone();
        assert_eq!(desk["name"], "Rosson's MacBook");

        // Gina is her own person: nothing of the owner's, and her stash is hers.
        register(port, &gina, "gina-1", "Gina's phone", "android");
        let v = pull(port, &gina, "gina-1", None).json();
        assert_eq!((v["cursor"].as_i64(), texts(&v).len()), (Some(0), 0), "per-person isolation: {v}");
        let r = push(port, &gina, "gina-1", json!([rec(9, "gina's", t)])).json();
        assert_eq!(r["applied"], 1);
        assert_eq!(texts(&pull(port, OWNER_TOKEN, "desk-1", None).json()), vec!["review the deploy log", "second"]);
        // Gina deleting the owner's record id writes only her own tombstone.
        let mut gt = rec(1, "", t + 5);
        gt["deleted_at"] = json!(t + 5);
        assert_eq!(push(port, &gina, "gina-1", json!([gt])).json()["applied"], 1);
        assert_eq!(texts(&pull(port, &phone, "phone-1", None).json()).len(), 2, "the owner's record is untouched");

        // Concurrent edits: the phone's later edit wins; the desk's older one is superseded.
        let mut pe = r1.clone();
        pe["text"] = json!("phone edit");
        pe["updated_at"] = json!(t + 20);
        assert_eq!(push(port, &phone, "phone-1", json!([pe])).json()["applied"], 1);
        let mut de = r1.clone();
        de["text"] = json!("desk edit");
        de["updated_at"] = json!(t + 10);
        let r = push(port, OWNER_TOKEN, "desk-1", json!([de])).json();
        assert_eq!((r["applied"].as_i64(), r["superseded"].as_i64()), (Some(0), Some(1)), "{r}");
        // The desk catches up incrementally from cursor 1; the delete reaches it as a tombstone.
        let mut del = rec(2, "", t);
        del["deleted_at"] = json!(t + 30);
        del["updated_at"] = json!(t + 30);
        assert_eq!(push(port, &phone, "phone-1", json!([del])).json()["applied"], 1);
        let v = pull(port, OWNER_TOKEN, "desk-1", Some(1)).json();
        assert_eq!(v["full"], false);
        let ch = v["changes"].as_array().unwrap();
        assert!(ch.iter().any(|c| c["id"] == uid(2).as_str() && c["deleted_at"] == t + 30 && c["text"] == ""), "tombstone rides the delta: {v}");
        assert_eq!(texts(&v), vec!["phone edit"]);

        // Invalid records are rejected one by one; the rest lands.
        let mut bad = rec(3, "x", t);
        bad["id"] = json!("NOT-A-UUID");
        let r = push(port, OWNER_TOKEN, "desk-1", json!([bad, rec(4, "fine", t)])).json();
        assert_eq!((r["applied"].as_i64(), r["rejected"][0]["reason"].as_str()), (Some(1), Some("invalid_id")), "{r}");
        // Bad bodies.
        let r = http(port, "POST", &format!("/cli/me/stash/push?token={OWNER_TOKEN}"), Some(r#"{"deviceId":"desk-1"}"#));
        assert_eq!((r.status, r.code().as_str()), (400, "usage"), "{}", r.body);
        let r = http(port, "POST", &format!("/cli/me/stash/push?token={OWNER_TOKEN}"), Some("not json"));
        assert_eq!(r.status, 400, "{}", r.body);
        let r = http(port, "GET", &format!("/cli/me/stash?token={OWNER_TOKEN}&deviceId=desk-1&since=abc"), None);
        assert_eq!(r.status, 400, "{}", r.body);
        let many: Vec<J> = (0..201).map(|i| rec(100 + i, "x", t)).collect();
        let r = push(port, OWNER_TOKEN, "desk-1", J::Array(many));
        assert_eq!((r.status, r.code().as_str()), (413, "too_many_changes"), "{}", r.body);

        // ── Refusals ──
        for tok in [OWNER_TOKEN, ""] {
            let r = http(port, "GET", &format!("/cli/me/stash/push?token={tok}"), None);
            assert_eq!(r.status, 405, "GET push: {}", r.body);
        }
        let r = http(port, "POST", &format!("/cli/me/stash?token={OWNER_TOKEN}"), Some("{}"));
        assert_eq!(r.status, 405, "POST pull: {}", r.body);
        let ws = seed_workspace(home, "ssroom");
        let agent = passport(&ws);
        let pass = app_pass(port, "ssguest", "ssroom");
        for (who, tok) in [("agent passport", agent.as_str()), ("app pass", pass.as_str()), ("api key", "k2sk_notarealkey"), ("junk", "nope"), ("none", "")] {
            for (method, p, body) in [
                ("GET", "/cli/me/stash", None),
                ("POST", "/cli/me/stash/push", Some(r#"{"deviceId":"desk-1","changes":[]}"#)),
            ] {
                let r = http(port, method, &format!("{p}?token={tok}&deviceId=desk-1"), body);
                assert_eq!(r.status, 403, "{who} {method} {p}: {}", r.body);
                assert_eq!(r.code(), "stash_person_only", "{who} {method} {p}");
            }
        }
    });
}

// ── 2. The cap ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_50_live_cap_holds_on_push() {
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate();
        register(port, OWNER_TOKEN, "desk-1", "Desk", "macos");
        let t = now_ms();
        let fifty: Vec<J> = (1..=50).map(|n| rec(n, &format!("t{n}"), t + n as i64)).collect();
        assert_eq!(push(port, OWNER_TOKEN, "desk-1", J::Array(fifty)).json()["applied"], 50);
        let r = push(port, OWNER_TOKEN, "desk-1", json!([rec(51, "51st", t + 100)])).json();
        assert_eq!((r["applied"].as_i64(), r["rejected"][0]["reason"].as_str()), (Some(0), Some("cap_reached")), "{r}");
        assert_eq!(r["rejected"][0]["item"]["id"], uid(51).as_str());
        // Remove the oldest and save, in one push: fits.
        let mut oldest = rec(1, "", t + 1);
        oldest["deleted_at"] = json!(t + 101);
        oldest["updated_at"] = json!(t + 101);
        let r = push(port, OWNER_TOKEN, "desk-1", json!([rec(51, "51st", t + 101), oldest])).json();
        assert_eq!((r["applied"].as_i64(), r["rejected"].as_array().map(Vec::len)), (Some(2), Some(0)), "{r}");
        assert_eq!(texts(&pull(port, OWNER_TOKEN, "desk-1", None).json()).len(), 50);
    });
}

// ── 3. Device removal and the live frame ──────────────────────────────

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn events_socket(port: u16, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={token}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events WS opens");
    assert!(next_kind(&mut ws, "hello", Duration::from_secs(5)).await.is_some(), "no hello");
    ws
}

async fn next_kind(ws: &mut Ws, kind: &str, wait: Duration) -> Option<J> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, ws.next()).await {
            Err(_) => return None,
            Ok(None) => panic!("events socket closed"),
            Ok(Some(Err(e))) => panic!("events socket error: {e}"),
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t)))) => {
                let v: J = serde_json::from_str(&t).expect("frame JSON");
                if v["kind"] == kind {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stash_changed_reaches_only_that_person_and_a_removed_device_gets_410() {
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate();
        let phone = login(port, "ssfphone");
        ok(port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(json!({ "accounts": ["ssfphone"] })));
        let gina = login(port, "ssfgina");
        register(port, OWNER_TOKEN, "desk-1", "Desk", "macos");
        register(port, &phone, "phone-1", "Phone", "ios");
        register(port, &gina, "gina-1", "Gina", "android");

        futures_block(async {
            let mut desk = events_socket(port, OWNER_TOKEN).await;
            let mut phone_ws = events_socket(port, &phone).await;
            let mut gina_ws = events_socket(port, &gina).await;
            let t = now_ms();
            let r = tokio::task::block_in_place(|| push(port, OWNER_TOKEN, "desk-1", json!([rec(1, "hello", t)])));
            assert_eq!(r.status, 200, "{}", r.body);
            let cursor = r.json()["cursor"].as_i64().expect("cursor");
            for (name, ws) in [("desk", &mut desk), ("linked phone", &mut phone_ws)] {
                let f = next_kind(ws, "stash_changed", Duration::from_secs(5)).await.unwrap_or_else(|| panic!("{name}: no stash_changed"));
                assert_eq!(f, json!({ "kind": "stash_changed", "cursor": cursor }), "{name}: no person key, no content");
            }
            assert!(next_kind(&mut gina_ws, "stash_changed", Duration::from_millis(1200)).await.is_none(), "another person never hears it");
            // A no-op push emits nothing.
            tokio::task::block_in_place(|| push(port, OWNER_TOKEN, "desk-1", json!([rec(1, "hello", t)])));
            assert!(next_kind(&mut desk, "stash_changed", Duration::from_millis(1200)).await.is_none(), "no-op push: no frame");
        });

        // Removed from the shared registry: stash pull and push answer 410.
        ok(port, "POST", &format!("/cli/me/homes/devices/remove?token={OWNER_TOKEN}"), Some(json!({ "deviceId": "phone-1" })));
        let r = pull(port, &phone, "phone-1", None);
        assert_eq!((r.status, r.code().as_str()), (410, "device_removed"), "{}", r.body);
        let r = push(port, &phone, "phone-1", json!([rec(2, "from a removed phone", now_ms())]));
        assert_eq!((r.status, r.code().as_str()), (410, "device_removed"), "{}", r.body);
        assert_eq!(texts(&pull(port, OWNER_TOKEN, "desk-1", None).json()), vec!["hello"], "nothing landed");
        // Registering again resumes.
        register(port, &phone, "phone-1", "Phone", "ios");
        assert_eq!(push(port, &phone, "phone-1", json!([rec(2, "back", now_ms())])).json()["applied"], 1);
    });
}
