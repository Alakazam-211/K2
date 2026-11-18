//! homes-sync-v1 (0.46.1) through the real dispatcher: `/boot-status`
//! `serverId` + feature key, `/cli/me/homes*` per person, the agent /
//! app-pass / API-key refusals, device removal (410 + frame) and the
//! person-scoped `homes_changed` frame.

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

const OWNER_TOKEN: &str = "owner-token-homes-sync-0461";

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
        _g: k2_core::test_env::SerialGuard,
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
    let g = k2_core::test_env::serial(&TEST_LOCK);
    k2_core::test_isolation::assert_no_prod_env();
    let tmp = std::env::temp_dir().join(format!("k2-homes-sync-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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

/// The test DB is one per process: forget Homes an earlier test left.
fn isolate_homes() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch("DELETE FROM homes_items; DELETE FROM homes_chat_items; DELETE FROM homes_people; DELETE FROM homes_devices;")
        .expect("isolate homes");
}

fn login(port: u16, username: &str) -> String {
    ok(port, "POST", &format!("/cli/users/add?token={OWNER_TOKEN}"), Some(json!({ "username": username, "password": "password123", "mustChangePassword": false })));
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
        HookPrincipal { workspace_uuid: workspace_uuid.to_string(), agent_address: "hs-agent".to_string() },
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
        &format!("/cli/me/homes/push?token={token}"),
        Some(&json!({ "deviceId": device, "changes": changes }).to_string()),
    )
}

fn pull(port: u16, token: &str, device: &str, since: Option<i64>) -> Resp {
    let since = since.map(|s| format!("&since={s}")).unwrap_or_default();
    http(port, "GET", &format!("/cli/me/homes?token={token}&deviceId={device}{since}"), None)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn home_item(id: &str, name: &str, order: &str, at: i64) -> J {
    json!({ "kind": "home", "id": id, "name": name, "order": order, "updatedAt": at })
}

fn row_item(home: &str, address: &str, label: &str, order: &str, at: i64) -> J {
    json!({ "kind": "row", "homeId": home, "address": address, "label": label, "workspaceId": null, "order": order, "updatedAt": at })
}

fn home_names(v: &J) -> Vec<String> {
    v["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter(|c| c["kind"] == "home" && c.get("deleted").is_none())
        .map(|c| c["name"].as_str().expect("name").to_string())
        .collect()
}

// ── 1. Routes ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn homes_routes_sync_one_person_across_devices_and_refuse_everyone_else() {
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_homes();
        // homes-sync-v1 on its own: the People group (seeded on a pull) has
        // its own test below.
        let _people_off = k2_core::test_env::EnvVar::set(k2_core::homes_sync::PEOPLE_SWITCH_ENV, "off");

        // /boot-status: the feature key and a stable 16-hex serverId from the identity key.
        let boot = ok(port, "GET", "/boot-status", None);
        assert!(boot["features"].as_array().expect("features").iter().any(|f| f == "homes-sync-v1"), "{boot}");
        let sid = boot["serverId"].as_str().unwrap_or_else(|| panic!("serverId: {boot}")).to_string();
        assert_eq!(sid.len(), 16, "{sid}");
        assert!(sid.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{sid}");
        assert_eq!(sid, k2_core::federation::identity::server_id().expect("identity"), "from the identity key");
        assert!(boot["publicHost"].is_null(), "no tunnel in the test: {boot}");
        let again = ok(port, "GET", "/boot-status", None);
        assert_eq!(again["serverId"], sid.as_str(), "stable across reads");

        // The phone: a Connect login the owner linked = the owner person.
        let phone = login(port, "hsphone");
        ok(port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(json!({ "accounts": ["hsphone"] })));
        // Gina: an unlinked login = her own person.
        let gina = login(port, "hsgina");

        // deviceId is required; an unknown device reads but is not syncing.
        let r = http(port, "GET", &format!("/cli/me/homes?token={OWNER_TOKEN}"), None);
        assert_eq!((r.status, r.code().as_str()), (400, "usage"), "{}", r.body);
        let r = pull(port, OWNER_TOKEN, "desk-1", None);
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        assert_eq!((v["cursor"].as_i64(), v["full"].as_bool()), (Some(0), Some(true)), "{v}");
        assert_eq!(v["device"], json!({ "deviceId": "desk-1", "syncing": false }));
        assert_eq!(v["person"], "owner");
        assert_eq!(v["serverId"], sid.as_str());
        // Pushing before registering is refused.
        let r = push(port, OWNER_TOKEN, "desk-1", json!([home_item("h1", "Home", "V", now_ms())]));
        assert_eq!((r.status, r.code().as_str()), (409, "device_not_registered"), "{}", r.body);

        // The desktop registers and pushes its Homes (rows named by serverId).
        let reg = register(port, OWNER_TOKEN, "desk-1", "Rosson's MacBook", "macos");
        assert_eq!(reg["device"]["deviceId"], "desk-1");
        assert_eq!(reg["device"]["current"], true);
        let t = now_ms();
        let addr = format!("atlas::{sid}");
        let r = push(
            port,
            OWNER_TOKEN,
            "desk-1",
            json!([home_item("h1", "Home", "V", t), row_item("h1", &addr, "Atlas", "V", t), home_item("h2", "Work", "l", t)]),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let pr = r.json();
        assert_eq!((pr["cursor"].as_i64(), pr["applied"].as_i64(), pr["rejected"].as_array().map(Vec::len)), (Some(1), Some(3), Some(0)), "{pr}");

        // The linked phone (same person) registers and sees the same Homes.
        register(port, &phone, "phone-1", "Rosson's iPhone", "ios");
        let v = pull(port, &phone, "phone-1", None).json();
        assert_eq!(v["person"], "owner", "a linked login is the owner person");
        assert_eq!((v["cursor"].as_i64(), v["device"]["syncing"].as_bool()), (Some(1), Some(true)), "{v}");
        let mut names = home_names(&v);
        names.sort();
        assert_eq!(names, vec!["Home", "Work"]);
        let row = v["changes"].as_array().unwrap().iter().find(|c| c["kind"] == "row").expect("row").clone();
        assert_eq!(row["address"], addr.as_str());
        assert_eq!(row["homeId"], "h1");
        assert_eq!(row["deviceId"], "desk-1", "who wrote it (LWW tie-break)");
        // 0306: the server lists the kinds it keeps.
        assert_eq!(v["kinds"], json!(["home", "row", "person", "channel"]), "{v}");

        // Gina is her own person: nothing of the owner's.
        register(port, &gina, "gina-phone", "Gina's phone", "android");
        let v = pull(port, &gina, "gina-phone", None).json();
        assert_eq!((v["person"].as_str(), v["cursor"].as_i64()), (Some("hsgina"), Some(0)), "{v}");
        assert!(v["changes"].as_array().unwrap().is_empty(), "per-person isolation: {v}");
        // Her device list doesn't show the owner's devices, and the owner's not hers.
        let gl = ok(port, "GET", &format!("/cli/me/homes/devices?token={gina}&deviceId=gina-phone"), None);
        assert_eq!(gl["devices"].as_array().unwrap().len(), 1, "{gl}");
        let ol = ok(port, "GET", &format!("/cli/me/homes/devices?token={OWNER_TOKEN}&deviceId=desk-1"), None);
        let ids: Vec<&str> = ol["devices"].as_array().unwrap().iter().map(|d| d["deviceId"].as_str().unwrap()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["desk-1", "phone-1"], "{ol}");
        let desk = ol["devices"].as_array().unwrap().iter().find(|d| d["deviceId"] == "desk-1").unwrap();
        assert_eq!((desk["current"].as_bool(), desk["platform"].as_str(), desk["name"].as_str()), (Some(true), Some("macos"), Some("Rosson's MacBook")));
        assert!(desk["lastSyncAt"].as_i64().is_some(), "{desk}");

        // Concurrent edits: the phone renames later than the desk; the desk's older rename loses.
        let r = push(port, &phone, "phone-1", json!([home_item("h1", "Home (phone)", "V", t + 20)])).json();
        assert_eq!(r["applied"], 1);
        let r = push(port, OWNER_TOKEN, "desk-1", json!([home_item("h1", "Home (desk)", "V", t + 10)])).json();
        assert_eq!((r["applied"].as_i64(), r["superseded"].as_i64()), (Some(0), Some(1)), "{r}");
        // The desk catches up incrementally from cursor 1.
        let v = pull(port, OWNER_TOKEN, "desk-1", Some(1)).json();
        assert_eq!((v["full"].as_bool(), v["cursor"].as_i64()), (Some(false), Some(2)), "{v}");
        assert_eq!(home_names(&v), vec!["Home (phone)"]);
        // Delete vs edit: the desk deletes the row; the phone's older edit can't revive it.
        let mut tomb = row_item("h1", &addr, "Atlas", "V", t + 30);
        tomb["deleted"] = json!(true);
        assert_eq!(push(port, OWNER_TOKEN, "desk-1", json!([tomb])).json()["applied"], 1);
        assert_eq!(push(port, &phone, "phone-1", json!([row_item("h1", &addr, "Atlas", "k", t + 25)])).json()["applied"], 0);
        let v = pull(port, &phone, "phone-1", Some(2)).json();
        let ch = v["changes"].as_array().unwrap();
        assert!(ch.iter().any(|c| c["kind"] == "row" && c["deleted"] == true), "the tombstone rides the delta: {v}");
        // The last Home is never deleted: drop Work, then try Home.
        let mut w = home_item("h2", "Work", "l", t + 40);
        w["deleted"] = json!(true);
        assert_eq!(push(port, OWNER_TOKEN, "desk-1", json!([w])).json()["applied"], 1);
        let mut h = home_item("h1", "Home", "V", t + 41);
        h["deleted"] = json!(true);
        let r = push(port, OWNER_TOKEN, "desk-1", json!([h])).json();
        assert_eq!(r["rejected"][0]["reason"], "last_home", "{r}");
        // A merged Home's id stays as a redirect.
        let mut m = home_item("h-old", "Home", "V", t + 50);
        m["deleted"] = json!(true);
        m["mergedInto"] = json!("h1");
        assert_eq!(push(port, &phone, "phone-1", json!([m])).json()["applied"], 1);
        assert_eq!(pull(port, OWNER_TOKEN, "desk-1", None).json()["redirects"], json!({ "h-old": "h1" }));
        // Invalid items are rejected one by one with a reason; the rest lands.
        let r = push(port, OWNER_TOKEN, "desk-1", json!([home_item("h9", "  ", "V", t), row_item("h1", "no-separator", "x", "V", t), home_item("h3", "Ok", "x", t + 60)])).json();
        let reasons: Vec<&str> = r["rejected"].as_array().unwrap().iter().map(|x| x["reason"].as_str().unwrap()).collect();
        assert_eq!(reasons, vec!["invalid_name", "invalid_address"], "{r}");
        assert_eq!(r["applied"], 1);
        assert_eq!(r["rejected"][0]["item"]["id"], "h9", "the refused item is echoed back");
        // 0306: a person row and a channel row sync across the person's
        // devices like an agent row; an agent's external id is no person.
        let mut person = row_item("h1", &format!("k2:owner::{sid}"), "Owner", "w", t + 70);
        person["kind"] = json!("person");
        let mut channel = row_item("h1", &format!("4242::{sid}"), "design", "x", t + 70);
        channel["kind"] = json!("channel");
        let mut agent_as_person = row_item("h1", &format!("k2:workspace:5d1c::{sid}"), "agent", "y", t + 70);
        agent_as_person["kind"] = json!("person");
        let r = push(port, OWNER_TOKEN, "desk-1", json!([person, channel, agent_as_person])).json();
        assert_eq!(r["applied"], 2, "{r}");
        assert_eq!(r["rejected"][0]["reason"], "invalid_address", "{r}");
        let v = pull(port, &phone, "phone-1", None).json();
        let kinds: Vec<&str> = v["changes"].as_array().unwrap().iter().filter_map(|c| c["kind"].as_str()).collect();
        assert!(kinds.contains(&"person") && kinds.contains(&"channel"), "{v}");
        // Bad bodies.
        let r = http(port, "POST", &format!("/cli/me/homes/push?token={OWNER_TOKEN}"), Some(r#"{"deviceId":"desk-1"}"#));
        assert_eq!((r.status, r.code().as_str()), (400, "usage"), "{}", r.body);
        let r = http(port, "POST", &format!("/cli/me/homes/push?token={OWNER_TOKEN}"), Some("not json"));
        assert_eq!(r.status, 400, "{}", r.body);
        let r = pull(port, OWNER_TOKEN, "desk-1", None);
        assert_eq!(r.status, 200);
        let r = http(port, "GET", &format!("/cli/me/homes?token={OWNER_TOKEN}&deviceId=desk-1&since=abc"), None);
        assert_eq!(r.status, 400, "{}", r.body);

        // ── Refusals ──
        // GET on the POST routes is 405 (with or without a token); POST on the reads too.
        for p in ["/cli/me/homes/push", "/cli/me/homes/devices/register", "/cli/me/homes/devices/remove"] {
            for tok in [OWNER_TOKEN, ""] {
                let r = http(port, "GET", &format!("{p}?token={tok}"), None);
                assert_eq!(r.status, 405, "GET {p}: {}", r.body);
            }
        }
        for p in ["/cli/me/homes", "/cli/me/homes/devices"] {
            let r = http(port, "POST", &format!("{p}?token={OWNER_TOKEN}"), Some("{}"));
            assert_eq!(r.status, 405, "POST {p}: {}", r.body);
        }
        // Agents (a real passport), app passes (a real k2skn_ pass), API keys, junk, no token: 403.
        let ws = seed_workspace(home, "hsroom");
        let agent = passport(&ws);
        let pass = app_pass(port, "hsguest", "hsroom");
        for (who, tok) in [("agent passport", agent.as_str()), ("app pass", pass.as_str()), ("api key", "k2sk_notarealkey"), ("junk", "nope"), ("none", "")] {
            for (method, p, body) in [
                ("GET", "/cli/me/homes", None),
                ("GET", "/cli/me/homes/devices", None),
                ("POST", "/cli/me/homes/push", Some(r#"{"deviceId":"x","changes":[]}"#)),
                ("POST", "/cli/me/homes/devices/register", Some(r#"{"deviceId":"x","name":"x","platform":"ios"}"#)),
                ("POST", "/cli/me/homes/devices/remove", Some(r#"{"deviceId":"desk-1"}"#)),
            ] {
                let r = http(port, method, &format!("{p}?token={tok}&deviceId=x"), body);
                assert_eq!(r.status, 403, "{who} {method} {p}: {}", r.body);
                assert_eq!(r.code(), "homes_person_only", "{who} {method} {p}");
            }
        }
        // Nothing the refused callers sent landed.
        let ol = ok(port, "GET", &format!("/cli/me/homes/devices?token={OWNER_TOKEN}"), None);
        assert_eq!(ol["devices"].as_array().unwrap().len(), 2, "desk-1 was not removed by an agent: {ol}");
    });
}

// ── 2. Device removal and live frames ─────────────────────────────────

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
async fn device_removal_answers_410_and_frames_reach_only_that_person() {
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_homes();
        // The People group's seed has its own test below.
        let _people_off = k2_core::test_env::EnvVar::set(k2_core::homes_sync::PEOPLE_SWITCH_ENV, "off");
        let phone = login(port, "hsfphone");
        ok(port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(json!({ "accounts": ["hsfphone"] })));
        let gina = login(port, "hsfgina");
        register(port, OWNER_TOKEN, "desk-1", "Desk", "macos");
        register(port, &phone, "phone-1", "Phone", "ios");
        register(port, &gina, "gina-1", "Gina", "android");

        futures_block(async {
            let mut desk = events_socket(port, OWNER_TOKEN).await;
            let mut phone_ws = events_socket(port, &phone).await;
            let mut gina_ws = events_socket(port, &gina).await;

            // A push from the desk: every owner socket (the phone too) hears homes_changed.
            let t = now_ms();
            let r = tokio::task::block_in_place(|| push(port, OWNER_TOKEN, "desk-1", json!([home_item("h1", "Home", "V", t)])));
            assert_eq!(r.status, 200, "{}", r.body);
            let cursor = r.json()["cursor"].as_i64().expect("cursor");
            for (name, ws) in [("desk", &mut desk), ("linked phone", &mut phone_ws)] {
                let f = next_kind(ws, "homes_changed", Duration::from_secs(5)).await.unwrap_or_else(|| panic!("{name}: no homes_changed"));
                assert_eq!(f, json!({ "kind": "homes_changed", "cursor": cursor }), "{name}: no person key, no names");
            }
            assert!(next_kind(&mut gina_ws, "homes_changed", Duration::from_millis(1200)).await.is_none(), "another person's socket never hears it");
            // A push that changes nothing emits nothing.
            tokio::task::block_in_place(|| push(port, OWNER_TOKEN, "desk-1", json!([home_item("h1", "Home", "V", t)])));
            assert!(next_kind(&mut desk, "homes_changed", Duration::from_millis(1200)).await.is_none(), "no-op push: no frame");
            // Gina's own push reaches only her.
            tokio::task::block_in_place(|| push(port, &gina, "gina-1", json!([home_item("g1", "Mine", "V", t)])));
            assert!(next_kind(&mut gina_ws, "homes_changed", Duration::from_secs(5)).await.is_some(), "gina hears her own");
            assert!(next_kind(&mut phone_ws, "homes_changed", Duration::from_millis(1200)).await.is_none(), "the owner never hears gina's");

            // The desk removes the phone (Server settings): the owner's sockets hear it, gina's don't.
            let r = tokio::task::block_in_place(|| {
                ok(port, "POST", &format!("/cli/me/homes/devices/remove?token={OWNER_TOKEN}"), Some(json!({ "deviceId": "phone-1" })))
            });
            assert_eq!((r["removed"].as_bool(), r["deviceId"].as_str()), (Some(true), Some("phone-1")), "{r}");
            for (name, ws) in [("desk", &mut desk), ("phone", &mut phone_ws)] {
                let f = next_kind(ws, "homes_device_removed", Duration::from_secs(5)).await.unwrap_or_else(|| panic!("{name}: no frame"));
                assert_eq!(f, json!({ "kind": "homes_device_removed", "deviceId": "phone-1" }));
            }
            assert!(next_kind(&mut gina_ws, "homes_device_removed", Duration::from_millis(1200)).await.is_none());
        });

        // The removed phone: pull and push are 410 device_removed.
        let r = pull(port, &phone, "phone-1", None);
        assert_eq!((r.status, r.code().as_str()), (410, "device_removed"), "{}", r.body);
        let r = push(port, &phone, "phone-1", json!([home_item("h1", "Phone edit", "V", now_ms() + 100)]));
        assert_eq!((r.status, r.code().as_str()), (410, "device_removed"), "{}", r.body);
        assert_eq!(home_names(&pull(port, OWNER_TOKEN, "desk-1", None).json()), vec!["Home"], "the removed device's edit never landed");
        // It is gone from the list; removing again is fine; an unknown device is 404.
        let l = ok(port, "GET", &format!("/cli/me/homes/devices?token={OWNER_TOKEN}&deviceId=desk-1"), None);
        assert_eq!(l["devices"].as_array().unwrap().iter().map(|d| d["deviceId"].as_str().unwrap()).collect::<Vec<_>>(), vec!["desk-1"]);
        assert_eq!(ok(port, "POST", &format!("/cli/me/homes/devices/remove?token={OWNER_TOKEN}"), Some(json!({ "deviceId": "phone-1" })))["removed"], true);
        let r = http(port, "POST", &format!("/cli/me/homes/devices/remove?token={OWNER_TOKEN}"), Some(r#"{"deviceId":"ghost"}"#));
        assert_eq!((r.status, r.code().as_str()), (404, "unknown_device"), "{}", r.body);
        // Gina can't remove the owner's devices (they aren't hers).
        let r = http(port, "POST", &format!("/cli/me/homes/devices/remove?token={gina}"), Some(r#"{"deviceId":"desk-1"}"#));
        assert_eq!(r.status, 404, "{}", r.body);
        // Registering again re-syncs it.
        register(port, &phone, "phone-1", "Phone", "ios");
        let v = pull(port, &phone, "phone-1", None).json();
        assert_eq!((v["device"]["syncing"].as_bool(), home_names(&v)), (Some(true), vec!["Home".to_string()]), "{v}");
        let r = push(port, &phone, "phone-1", json!([home_item("h1", "Back again", "V", now_ms() + 200)]));
        assert_eq!(r.status, 200, "{}", r.body);
    });
}

// ── 3. The People group (homes-people-group-v1, 0.46.16) ──────────────

fn people_groups(v: &J) -> Vec<J> {
    v["changes"].as_array().expect("changes").iter().filter(|c| c["kind"] == "home" && c["id"] == "people" && c.get("deleted").is_none()).cloned().collect()
}

/// The seed through the real routes: listed on /boot-status, made once on
/// a pull (never before the person has a group), the same for every device,
/// merged with a client's own copy, never deleted (the item comes back in
/// `rejected` with `builtin_group` and the hint; the rest of the push
/// lands), renamed and moved like any group, marker kept; an agent row stays
/// in its group. Switch off: no key, no seed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_people_group_is_seeded_once_and_never_deleted() {
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_homes();
        let _people_on = k2_core::test_env::EnvVar::remove(k2_core::homes_sync::PEOPLE_SWITCH_ENV);
        let boot = ok(port, "GET", "/boot-status", None);
        let features = boot["features"].as_array().expect("features").clone();
        let at = |k: &str| features.iter().position(|f| f == k).unwrap_or_else(|| panic!("{k} listed: {boot}"));
        assert_eq!(at("homes-people-group-v1"), at("homes-sync-v1") + 1, "listed with homes-sync-v1");
        let sid = boot["serverId"].as_str().expect("serverId").to_string();
        let phone = login(port, "hspphone");
        ok(port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(json!({ "accounts": ["hspphone"] })));
        let gina = login(port, "hspgina");
        register(port, OWNER_TOKEN, "desk-1", "Desk", "macos");
        register(port, &phone, "phone-1", "Phone", "ios");
        register(port, &gina, "gina-1", "Gina", "android");

        // No group yet: nothing is made (the device's own groups come first).
        let v = pull(port, OWNER_TOKEN, "desk-1", None).json();
        assert_eq!((v["cursor"].as_i64(), people_groups(&v).len()), (Some(0), 0), "{v}");
        let t = now_ms();
        let agent = format!("atlas::{sid}");
        let r = push(port, OWNER_TOKEN, "desk-1", json!([home_item("h1", "Home", "V", t), home_item("h2", "Work", "l", t), row_item("h2", &agent, "Atlas", "V", t)]));
        assert_eq!(r.status, 200, "{}", r.body);
        let pushed = r.json()["cursor"].as_i64().expect("cursor");
        // The next pull seeds it: last, the chat icon, low stamp, the marker.
        let v = pull(port, OWNER_TOKEN, "desk-1", Some(pushed)).json();
        assert_eq!(v["cursor"].as_i64(), Some(pushed + 1), "{v}");
        let p = people_groups(&v);
        assert_eq!(p.len(), 1, "{v}");
        assert_eq!((p[0]["name"].clone(), p[0]["builtin"].clone(), p[0]["updatedAt"].clone()), (json!("People"), json!("people"), json!(1)));
        assert_eq!(p[0]["icon"], json!("message-circle"), "{v}");
        let order = p[0]["order"].as_str().expect("order").to_string();
        assert!(order.as_str() > "l", "after every other group: {order}");
        // Every pull after, on every device of the person: exactly one.
        for (tok, dev) in [(OWNER_TOKEN, "desk-1"), (phone.as_str(), "phone-1"), (OWNER_TOKEN, "desk-1")] {
            let v = pull(port, tok, dev, None).json();
            assert_eq!((people_groups(&v).len(), v["cursor"].as_i64()), (1, Some(pushed + 1)), "{dev}: {v}");
            let rows: Vec<&J> = v["changes"].as_array().unwrap().iter().filter(|c| c["kind"] == "row").collect();
            assert_eq!((rows.len(), rows[0]["homeId"].clone()), (1, json!("h2")), "the agent row stays in its group: {v}");
        }

        // Delete: refused, item by item, with the hint; the rest of the push lands.
        let mut del = home_item("people", "People", &order, t + 10);
        del["deleted"] = json!(true);
        let r = push(port, &phone, "phone-1", json!([del, home_item("h3", "Kids", "x", t + 10)]));
        assert_eq!(r.status, 200, "{}", r.body);
        let a = r.json();
        assert_eq!(a["applied"], 1, "{a}");
        assert_eq!(a["rejected"].as_array().map(Vec::len), Some(1), "{a}");
        assert_eq!(
            (a["rejected"][0]["reason"].clone(), a["rejected"][0]["hint"].clone(), a["rejected"][0]["item"]["id"].clone()),
            (json!("builtin_group"), json!("The People group can't be deleted."), json!("people"))
        );
        let v = pull(port, OWNER_TOKEN, "desk-1", None).json();
        assert_eq!(people_groups(&v).len(), 1, "still there: {v}");
        // Rename + move (an old client sends no marker): the marker stays.
        let r = push(port, &phone, "phone-1", json!([home_item("people", "Humans", "y", t + 20)])).json();
        assert_eq!(r["applied"], 1, "{r}");
        let p = people_groups(&pull(port, OWNER_TOKEN, "desk-1", None).json());
        assert_eq!((p[0]["name"].clone(), p[0]["order"].clone(), p[0]["builtin"].clone()), (json!("Humans"), json!("y"), json!("people")));

        // A client that made People itself before its first sync: one group.
        let mut local = home_item("people", "People", "W", t);
        local["builtin"] = json!("people");
        let r = push(port, &gina, "gina-1", json!([home_item("g1", "Home", "V", t), local])).json();
        assert_eq!((r["applied"].as_i64(), r["rejected"].as_array().map(Vec::len)), (Some(2), Some(0)), "{r}");
        let v = pull(port, &gina, "gina-1", None).json();
        assert_eq!(people_groups(&v).len(), 1, "{v}");
        assert_eq!(people_groups(&v)[0]["deviceId"], "gina-1", "her copy, not a second one");

        // Switch off: no key, and a person with no People group gets none.
        let _off = k2_core::test_env::EnvVar::set(k2_core::homes_sync::PEOPLE_SWITCH_ENV, "off");
        let boot = ok(port, "GET", "/boot-status", None);
        assert!(!boot["features"].as_array().unwrap().iter().any(|f| f == "homes-people-group-v1"), "{boot}");
        let pat = login(port, "hsppat");
        register(port, &pat, "pat-1", "Pat", "web");
        push(port, &pat, "pat-1", json!([home_item("p1", "Home", "V", t)]));
        let v = pull(port, &pat, "pat-1", None).json();
        assert!(people_groups(&v).is_empty(), "{v}");
        // A People group made before stays undeletable.
        let mut del = home_item("people", "Humans", "y", t + 30);
        del["deleted"] = json!(true);
        let r = push(port, &phone, "phone-1", json!([del])).json();
        assert_eq!(r["rejected"][0]["reason"], "builtin_group", "{r}");
    });
}

/// Home ids as a device lays them out: by `order`, then id.
fn home_order(v: &J) -> Vec<String> {
    let mut h: Vec<(String, String)> = v["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter(|c| c["kind"] == "home" && c.get("deleted").is_none())
        .map(|c| (c["order"].as_str().expect("order").to_string(), c["id"].as_str().expect("id").to_string()))
        .collect();
    h.sort();
    h.into_iter().map(|(_, id)| id).collect()
}

/// 0.46.15 defaults through the real routes: a People group seeded before
/// (no icon, right after the first group, never touched) takes the chat
/// icon and goes last on the first pull after the upgrade, once; both of
/// the person's devices see the same order and the next pulls change nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_people_seed_takes_the_chat_icon_and_the_bottom_once() {
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_homes();
        let _people_on = k2_core::test_env::EnvVar::remove(k2_core::homes_sync::PEOPLE_SWITCH_ENV);
        register(port, OWNER_TOKEN, "desk-1", "Desk", "macos");
        register(port, OWNER_TOKEN, "lap-1", "Laptop", "macos");
        let t = now_ms();
        let r = push(port, OWNER_TOKEN, "desk-1", json!([home_item("h1", "Home", "V", t), home_item("h2", "Work", "l", t)]));
        assert_eq!(r.status, 200, "{}", r.body);
        let pushed = r.json()["cursor"].as_i64().expect("cursor");
        // The old seed, as 0.46.14 wrote it; the pass not run yet on this DB.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO homes_items (person, kind, home_id, address, name, ord, updated_at, device_id, deleted, seq) \
                 VALUES ('owner', 'home', 'people', '', 'People', 'a', 1, 'k2-server', 0, ?1)",
                params![pushed + 1],
            )
            .expect("old seed");
            conn.execute("UPDATE homes_people SET seq = ?1 WHERE person = 'owner'", params![pushed + 1]).expect("old seq");
            conn.execute("DELETE FROM code_migrations WHERE id = ?1", params![k2_core::homes_sync::PEOPLE_DEFAULTS_PASS_ID])
                .expect("forget the pass");
        }
        // The first pull after the upgrade: icon + bottom, one cursor step.
        let v = pull(port, OWNER_TOKEN, "desk-1", Some(pushed + 1)).json();
        assert_eq!(v["cursor"].as_i64(), Some(pushed + 2), "{v}");
        let p = people_groups(&v);
        assert_eq!(p.len(), 1, "{v}");
        assert_eq!((p[0]["icon"].clone(), p[0]["updatedAt"].clone(), p[0]["deviceId"].clone()), (json!("message-circle"), json!(2), json!("k2-server")));
        let desk = home_order(&pull(port, OWNER_TOKEN, "desk-1", None).json());
        assert_eq!(desk, vec!["h1", "h2", "people"]);
        assert_eq!(home_order(&pull(port, OWNER_TOKEN, "lap-1", None).json()), desk, "the same order on the second device");
        // Pull after pull: nothing more.
        for dev in ["desk-1", "lap-1"] {
            let v = pull(port, OWNER_TOKEN, dev, Some(pushed + 2)).json();
            assert_eq!((v["cursor"].as_i64(), v["changes"].as_array().map(Vec::len)), (Some(pushed + 2), Some(0)), "{dev}: {v}");
        }
    });
}
