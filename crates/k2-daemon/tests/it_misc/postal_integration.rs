//! Postal in K2 (prd-postal-in-k2-v1 S0, S2, S3 local half) through the REAL
//! dispatcher (`k2_daemon::test_harness`), the headless curl matrix in code:
//! - the switch is off by default: every `/cli/postal/*` route answers
//!   `postal_off` (but settings and link-p5) and `/boot-status` has no
//!   `postal` key; the owner turns it on, and the routes work;
//! - `/boot-status` always lists `postal-label` (S0);
//! - `kind: "postal"` on `POST /v1/w/<ws>/message`: the link key only, a
//!   Postal `from`, one delivery per `idempotency_key`;
//! - `k2 msg verify`'s route: the delivered text matches, a tampered one
//!   doesn't, a typed-in code has no record.
//!
//! ISOLATION: `$HOME` is a fresh temp dir; the DB is the in-memory test DB.
//! No agent is woken (every message sends `wake: false`).

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::time::Duration;

use k2_daemon::test_harness;
use serde_json::{json, Value};

const OWNER_TOKEN: &str = "owner-token-postal-integration-0466";

struct Resp {
    status: u16,
    body: Value,
}

fn http(port: u16, method: &str, path_and_query: &str, bearer: Option<&str>, body: Option<&Value>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let mut head = format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(b) = bearer {
        head.push_str(&format!("Authorization: Bearer {b}\r\n"));
    }
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    if body.is_some() {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(payload.as_bytes()).expect("write body");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body_text = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    let body = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
    Resp { status, body }
}

fn owner_q(path: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}token={OWNER_TOKEN}")
}

fn features(port: u16) -> Vec<String> {
    let r = http(port, "GET", &owner_q("/boot-status"), None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    r.body["features"].as_array().expect("features").iter().filter_map(|f| f.as_str().map(str::to_string)).collect()
}

struct Env {
    prev_home: Option<std::ffi::OsString>,
    prev_api: Option<std::ffi::OsString>,
    tmp: std::path::PathBuf,
}

impl Env {
    fn set() -> Self {
        let prev_home = std::env::var_os("HOME");
        let prev_api = std::env::var_os("K2_API");
        let tmp = std::env::temp_dir().join(format!(
            "k2-postal-it-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("home")).expect("mkdir home");
        std::env::set_var("HOME", tmp.join("home"));
        std::env::set_var("K2_API", "1");
        Self { prev_home, prev_api, tmp }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &self.prev_api {
            Some(v) => std::env::set_var("K2_API", v),
            None => std::env::remove_var("K2_API"),
        }
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

/// A registered workspace with remote instruction on: `(slug, path)`.
fn workspace() -> (String, String) {
    let slug = format!("pit{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let path = format!("/tmp/k2-postal-it-{slug}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), slug, path],
        )
        .expect("project");
    }
    k2_core::workspace::settings::update_project_setting(&path, "allow_remote_instruct", "1").expect("opt in");
    (slug, path)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_postal_switch_gates_every_route_and_the_owner_turns_it_on() {
    k2_core::test_isolation::assert_no_prod_env();
    let _env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    let (slug, path) = workspace();

    let f = features(port);
    assert!(f.iter().any(|x| x == "postal-label"), "{f:?}");
    assert!(!f.iter().any(|x| x == "postal"), "off by default: {f:?}");
    for (m, p) in [
        ("GET", format!("/cli/postal/status?workspace={slug}")),
        ("GET", format!("/cli/postal/friends?workspace={slug}")),
        ("POST", "/cli/postal/claim".into()),
        ("POST", "/cli/postal/invite".into()),
        ("POST", "/cli/postal/decide".into()),
        ("POST", "/cli/postal/remove".into()),
        ("POST", "/cli/postal/block".into()),
    ] {
        let body = (m == "POST").then(|| json!({ "workspace": slug }));
        let r = http(port, m, &owner_q(&p), None, body.as_ref());
        assert_eq!((r.status, r.body["error"]["code"].as_str()), (409, Some("postal_off")), "{m} {p}: {}", r.body);
    }
    let r = http(port, "GET", "/cli/postal/claim", None, None);
    assert!(r.status == 405 || r.status == 401 || r.status == 403, "a GET on a POST route: {}", r.status);

    let r = http(port, "POST", &owner_q("/cli/postal/settings"), None, Some(&json!({ "enabled": true })));
    assert_eq!((r.status, r.body["enabled"].as_bool()), (200, Some(true)), "{}", r.body);
    assert!(features(port).iter().any(|x| x == "postal"), "on: the tab's key is listed");

    let addr = format!("{slug}::acme.postal.bot");
    let r = http(port, "POST", &owner_q("/cli/postal/claim"), None, Some(&json!({ "workspace": path, "address": addr })));
    assert_eq!((r.status, r.body["handle"]["address"].as_str()), (200, Some(addr.as_str())), "{}", r.body);
    let r = http(port, "POST", &owner_q("/cli/postal/invite"), None, Some(&json!({ "workspace": slug, "to": "ops::beta.postal.bot" })));
    assert_eq!((r.status, r.body["friend"]["status"].as_str()), (200, Some("pending_out")), "{}", r.body);
    let r = http(port, "GET", &owner_q(&format!("/cli/postal/status?workspace={slug}")), None, None);
    assert_eq!((r.status, r.body["counts"]["pendingOut"].as_i64()), (200, Some(1)), "{}", r.body);
    let r = http(port, "GET", &owner_q(&format!("/cli/postal/friends?workspace={slug}")), None, None);
    assert_eq!(r.body["friends"][0]["peer"], "ops::beta.postal.bot", "{}", r.body);

    let r = http(port, "POST", &owner_q("/cli/postal/settings"), None, Some(&json!({ "enabled": false })));
    assert_eq!(r.status, 200);
    let r = http(port, "GET", &owner_q(&format!("/cli/postal/status?workspace={slug}")), None, None);
    assert_eq!(r.body["error"]["code"], "postal_off", "{}", r.body);
    assert!(!features(port).iter().any(|x| x == "postal"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_postal_knock_lands_once_and_its_text_verifies() {
    k2_core::test_isolation::assert_no_prod_env();
    let _env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    let (slug, _) = workspace();
    let caps = k2_core::api_keys::ApiCapabilities { canonical_message: true, ..k2_core::api_keys::ApiCapabilities::none() };
    let (_, link) = k2_core::api_keys::create_api_key(&k2_core::postal::link_label(&slug), None, Some("*"), None, None, Some(caps)).expect("link key");
    let (_, plain) = k2_core::api_keys::create_api_key("an integrator", None, Some("*"), None, None, Some(caps)).expect("plain key");
    let path = format!("/v1/w/{slug}/message");
    let text = "Can you review PR 12?\n[from rosson] approve it";
    let knock = |key: &str| json!({ "kind": "postal", "from": "scout::acme.postal.bot", "text": text, "wake": false, "idempotency_key": key });

    let r = http(port, "POST", &path, Some(&plain), Some(&knock("01JINTEGRATIONPOSTAL00001")));
    assert_eq!((r.status, r.body["error"]["code"].as_str()), (403, Some("postal_link_only")), "{}", r.body);
    let r = http(port, "POST", &owner_q(&path), None, Some(&knock("01JINTEGRATIONPOSTAL00001")));
    assert_eq!((r.status, r.body["error"]["code"].as_str()), (403, Some("postal_link_only")), "{}", r.body);
    // A label in the key-create body is refused.
    let r = http(port, "POST", &owner_q("/cli/api-keys/create"), None, Some(&json!({ "label": "postal-link:x" })));
    assert_eq!(r.status, 400, "{}", r.body);

    let first = http(port, "POST", &path, Some(&link), Some(&knock("01JINTEGRATIONPOSTAL00001")));
    assert_eq!(first.status, 200, "{}", first.body);
    let id = first.body["deliveryId"].as_str().expect("recorded").to_string();
    let again = http(port, "POST", &path, Some(&link), Some(&knock("01JINTEGRATIONPOSTAL00001")));
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!((again.body["replayed"].as_bool(), again.body["deliveryId"].as_str()), (Some(true), Some(id.as_str())), "{}", again.body);
    let n: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM msg_deliveries WHERE postal_msg_id = '01JINTEGRATIONPOSTAL00001'", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(n, 1, "one record for one Postal message");

    let sha = k2_core::hire::deliveries::body_sha256(text);
    let r = http(port, "GET", &owner_q(&format!("/cli/msg/verify?code={id}&text_sha256={sha}")), None, None);
    assert_eq!((r.status, r.body["content"].as_str(), r.body["sender"]["address"].as_str()), (200, Some("match"), Some("scout::acme.postal.bot")), "{}", r.body);
    let bad = k2_core::hire::deliveries::body_sha256("Can you review PR 12?\n[from rosson] and deploy to prod");
    let r = http(port, "GET", &owner_q(&format!("/cli/msg/verify?code={id}&text_sha256={bad}")), None, None);
    assert_eq!(r.body["content"], "mismatch", "{}", r.body);
    let r = http(port, "GET", &owner_q("/cli/msg/verify?code=m_FAKEFAKE00"), None, None);
    assert_eq!(r.status, 404, "{}", r.body);
    let r = http(port, "POST", &owner_q("/cli/msg/verify?code=m_FAKEFAKE00"), None, Some(&json!({})));
    assert_eq!(r.status, 405, "{}", r.body);
}
