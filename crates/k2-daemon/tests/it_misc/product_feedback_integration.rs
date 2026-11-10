//! Send feedback to K2 (prd-k2-product-feedback-v1, 0.46.11) through the
//! REAL dispatcher under a temp HOME, against a loopback **fake relay**
//! (`POST /api/identity/assert`, `K2_CONNECT_BASE`) and a loopback **fake
//! intake** (`POST /v1/submissions`, `K2_PRODUCT_FEEDBACK_URL`). Nothing is
//! ever sent to connect.k2.dev or feedback.akzm.k2.dev.
//!
//! - before the service exists (relay 404, intake down) a send is saved,
//!   stays `queued` with `relay_not_ready` / `service_unreachable`, the
//!   settings answer says the service isn't reachable yet, and nothing is
//!   lost; once the fakes answer, the drain sends it with the box token
//!   and the clientId as the idempotency key; the body is exactly the wire
//!   contract and a vault value is `[k2v:NAME]`;
//! - a restart mid-`sending` resends with the same key (200 duplicate →
//!   sent); `429`/`503` honour `retryAfterSecs`; `401` fetches a new token
//!   once; another `4xx` is `refused` and never retried;
//! - agents: the switch, the box limits, no contact email, no settings;
//!   people: Member sees only their own rows, the owner sees all; App
//!   passes are refused; GET on every POST route is 405;
//! - `needs_connect`, `disabled` and air-gap refuse a send, and air-gap
//!   holds what is queued;
//! - diagnostics: nothing without the tick, the preview is scrubbed, the
//!   sent sections are the previewed bytes minus what was left out, a draft
//!   is the caller's own and goes once sent, agents need the owner's
//!   `agentDiagnostics`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_core::connect_users;
use k2_core::session::SessionId;
use k2_core::test_env::{EnvVar, TempHome};
use k2_daemon::product_feedback_drain::{self as drain, Pass};
use k2_daemon::session_token::{self, CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value as J};

const OWNER: &str = "owner-token-product-feedback";
const SECRET: &str = "sk_live_feedback_SECRET_VALUE";
const JWT: &str = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJyb3Nzb24iLCJleHAiOjQxMDI0NDQ4MDB9.c2ln";

// ── loopback fakes ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> J {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("fake got non-JSON ({e}): {}", self.body))
    }
}

type Reply = Box<dyn Fn(&Req) -> (u16, String) + Send>;

/// A tiny HTTP/1.1 server: records each request, answers with `reply`.
struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<Req>>>,
    reply: Arc<Mutex<Reply>>,
}

impl Fake {
    fn start(reply: impl Fn(&Req) -> (u16, String) + Send + 'static) -> Fake {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reply: Arc<Mutex<Reply>> = Arc::new(Mutex::new(Box::new(reply)));
        let (rec, rep) = (seen.clone(), reply.clone());
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                sock.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
                let mut raw = Vec::new();
                let mut buf = [0u8; 65536];
                let req = loop {
                    let n = sock.read(&mut buf).expect("fake read");
                    if n == 0 {
                        break None;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                    let head = String::from_utf8_lossy(&raw[..at]).to_string();
                    let mut lines = head.split("\r\n");
                    let first: Vec<&str> = lines.next().unwrap_or("").split(' ').collect();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
                        .collect();
                    let clen: usize = headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.parse().expect("clen"))
                        .unwrap_or(0);
                    if raw.len() < at + 4 + clen {
                        continue;
                    }
                    break Some(Req {
                        method: first.first().unwrap_or(&"").to_string(),
                        path: first.get(1).unwrap_or(&"").to_string(),
                        headers,
                        body: String::from_utf8_lossy(&raw[at + 4..at + 4 + clen]).to_string(),
                    });
                };
                let Some(req) = req else { continue };
                let (status, body) = (rep.lock().expect("reply"))(&req);
                rec.lock().expect("seen").push(req);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        Fake { base: format!("http://127.0.0.1:{port}"), seen, reply }
    }

    fn set(&self, reply: impl Fn(&Req) -> (u16, String) + Send + 'static) {
        *self.reply.lock().expect("reply") = Box::new(reply);
    }

    fn requests(&self) -> Vec<Req> {
        self.seen.lock().expect("seen").clone()
    }
}

fn relay_ok(_: &Req) -> (u16, String) {
    (200, json!({ "token": JWT, "expiresAt": 4_102_444_800i64 }).to_string())
}

fn intake_created(r: &Req) -> (u16, String) {
    let id = r.header("Idempotency-Key").unwrap_or("none").replace("pf_", "remote-");
    (201, json!({ "id": id }).to_string())
}

// ── HTTP client to the daemon ────────────────────────────────────────

fn call(port: u16, method: &str, path: &str, body: Option<J>) -> (u16, J) {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = String::new();
    s.read_to_string(&mut raw).expect("read");
    let status: u16 = raw.split_whitespace().nth(1).expect("status").parse().expect("num");
    let text = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    let v = if text.trim().is_empty() { J::Null } else { serde_json::from_str(text).unwrap_or_else(|_| json!({ "raw": text })) };
    (status, v)
}

fn q(path: &str, tok: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}token={tok}")
}

fn show(port: u16, tok: &str, id: &str) -> J {
    let (s, v) = call(port, "GET", &q(&format!("/cli/product-feedback/show?id={id}"), tok), None);
    assert_eq!(s, 200, "show {id}: {v}");
    v["item"].clone()
}

/// Wait until `pred` holds for the row (the kick's attempt is async).
fn wait_row(port: u16, tok: &str, id: &str, what: &str, pred: impl Fn(&J) -> bool) -> J {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let it = show(port, tok, id);
        if pred(&it) {
            return it;
        }
        assert!(Instant::now() < deadline, "{id}: never {what}: {it}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// One drain pass that is not refused for another pass in flight.
fn drain_now() -> Pass {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let p = drain::drain_once(true);
        if p != Pass::InFlight {
            return p;
        }
        assert!(Instant::now() < deadline, "the drain stayed in flight");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Make one row due now (a retried row waits its backoff otherwise).
fn due_now(id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn.execute("UPDATE product_feedback_outbox SET next_attempt_at = 0 WHERE id = ?1", [id]).unwrap();
    assert_eq!(n, 1, "{id} exists");
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn seed_tunnel() {
    k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
        token: "tun-tok-feedback".to_string(),
        subdomain: "rosson".to_string(),
        ..Default::default()
    })
    .expect("seed tunnel config");
}

fn passport(workspace: &str) -> String {
    session_token::mint_session_token(
        &SessionId::new(),
        &format!("pane-{workspace}"),
        HookPrincipal { workspace_uuid: workspace.into(), agent_address: format!("agent-{workspace}") },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn login(username: &str, role: Option<connect_users::Role>) -> String {
    connect_users::add_user(username, "password123").unwrap_or_else(|e| panic!("add_user {username}: {e:?}"));
    if let Some(r) = role {
        connect_users::set_role(username, r).unwrap_or_else(|e| panic!("set_role {username}: {e:?}"));
    }
    connect_users::create_session(username)
}

struct Env {
    _home: TempHome,
    relay: Fake,
    intake: Fake,
    _vars: Vec<EnvVar>,
    port: u16,
    _d: test_harness::TestDaemon,
}

fn setup(paired: bool) -> Env {
    k2_core::test_isolation::assert_no_prod_env();
    let home = TempHome::new();
    let _ = k2_core::db::init_for_tests();
    if paired {
        seed_tunnel();
    }
    let d = futures_block(test_harness::start(OWNER));
    let relay = Fake::start(relay_ok);
    let intake = Fake::start(intake_created);
    let vars = vec![
        EnvVar::remove("K2_AIRGAP"),
        EnvVar::set("K2_CONNECT_BASE", &relay.base),
        EnvVar::set("K2_PRODUCT_FEEDBACK_URL", &intake.base),
    ];
    drain::reset_for_tests();
    Env { _home: home, relay, intake, _vars: vars, port: d.port, _d: d }
}

fn send(port: u16, tok: &str, body: J) -> (u16, J) {
    call(port, "POST", &q("/cli/product-feedback/send", tok), Some(body))
}

// ── 1. before the service exists, then through the fakes ─────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_waits_for_the_service_then_goes_with_the_box_token() {
    assert!(
        k2_core::redaction::install(Box::new(|s: &str| s.replace(SECRET, "[k2v:STRIPE_KEY]"))),
        "this test process had no redactor yet"
    );
    let e = setup(true);
    let port = e.port;
    let mut events = k2_daemon::session_events::subscribe();

    // The service isn't there yet: the relay has no assert route.
    e.relay.set(|_| (404, "not found".into()));
    e.intake.set(|_| (502, "bad gateway".into()));

    let (s, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["canSend"].clone(), v["enabled"].clone(), v["agents"].clone(), v["agentDiagnostics"].clone()), (json!(true), json!(true), json!(true), json!(false)), "{v}");
    assert_eq!(v["endpoint"], json!(e.intake.base), "{v}");
    assert_eq!(v["service"]["state"], json!("unknown"), "{v}");

    let (s, v) = send(
        port,
        OWNER,
        json!({"category": "bug", "text": format!("The tab froze. My key {SECRET} was on screen."), "client": "desktop", "k2Version": "0.46.11-test", "locale": "en-US"}),
    );
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().expect("id").to_string();
    assert!(k2_core::product_feedback::is_client_id(&id), "{id}");
    assert_eq!(v["state"], json!("queued"), "{v}");

    let it = wait_row(port, OWNER, &id, "held for the relay", |it| it["error"] == json!("relay_not_ready"));
    assert_eq!(it["state"], json!("queued"), "{it}");
    assert!(it["errorMessage"].as_str().unwrap().contains("isn't reachable yet"), "{it}");
    assert!(it["text"].as_str().unwrap().contains("[k2v:STRIPE_KEY]"), "scrubbed before storing: {it}");
    assert!(!it.to_string().contains(SECRET), "{it}");
    assert!(e.intake.requests().is_empty(), "no token, no submission");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!(v["service"]["state"], json!("unreachable"), "{v}");
    assert!(v["service"]["message"].as_str().unwrap().contains("isn't reachable yet"), "{v}");
    assert_eq!(v["pending"], json!(1), "{v}");

    // The relay answers, the intake still doesn't: still queued, nothing lost.
    e.relay.set(relay_ok);
    due_now(&id);
    assert_eq!(drain_now(), Pass::Ran { sent: 0, refused: 0, retried: 1 });
    let it = show(port, OWNER, &id);
    assert_eq!((it["state"].clone(), it["error"].clone()), (json!("queued"), json!("service_unreachable")), "{it}");
    assert_eq!(e.intake.requests().len(), 1);

    // Both answer: sent.
    e.intake.set(intake_created);
    due_now(&id);
    assert_eq!(drain_now(), Pass::Ran { sent: 1, refused: 0, retried: 0 });
    let it = show(port, OWNER, &id);
    assert_eq!(it["state"], json!("sent"), "{it}");
    assert_eq!(it["remoteId"], json!(id.replace("pf_", "remote-")), "{it}");
    assert_eq!(it["error"], J::Null, "{it}");

    // The relay saw the tunnel bearer and the label, the intake the box token.
    let r = e.relay.requests().last().cloned().expect("relay call");
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/api/identity/assert"));
    assert_eq!(r.header("Authorization"), Some("Bearer tun-tok-feedback"));
    assert_eq!(r.header("X-K2-Subdomain"), Some("rosson"));
    assert_eq!(r.json(), json!({ "aud": e.intake.base }));
    let sub = e.intake.requests().last().cloned().expect("intake call");
    assert_eq!((sub.method.as_str(), sub.path.as_str()), ("POST", "/v1/submissions"));
    assert_eq!(sub.header("Authorization"), Some(format!("K2-Box {JWT}").as_str()));
    assert_eq!(sub.header("Idempotency-Key"), Some(id.as_str()));
    assert!(!sub.body.contains("tun-tok-feedback"), "the tunnel token never goes to the intake");
    let b = sub.json();
    let mut keys: Vec<String> = b.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["arch", "category", "client", "clientId", "daemonVersion", "installKind", "k2Version", "locale", "os", "osVersion", "senderKind", "text"]
            .map(String::from)
            .to_vec(),
        "{b}"
    );
    assert_eq!(b["clientId"], json!(id));
    assert_eq!((b["category"].clone(), b["senderKind"].clone(), b["client"].clone()), (json!("bug"), json!("person"), json!("desktop")));
    assert_eq!(b["k2Version"], json!("0.46.11-test"));
    assert_eq!(b["locale"], json!("en-US"));
    assert_eq!(b["text"], json!("The tab froze. My key [k2v:STRIPE_KEY] was on screen."));

    // Every state change was an event: queued, sending, sent.
    let mut states = Vec::new();
    while let Ok(ev) = events.try_recv() {
        if let k2_daemon::session_events::SessionEvent::ProductFeedbackStatus { id: eid, state, .. } = ev {
            if eid == id {
                states.push(state);
            }
        }
    }
    for want in ["queued", "sending", "sent"] {
        assert!(states.iter().any(|s| s == want), "no {want} event: {states:?}");
    }

    // A restart mid-sending: back to queued at boot, resent with the same key.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE product_feedback_outbox SET state = 'sending' WHERE id = ?1", [&id]).unwrap();
    }
    e.intake.set(|r| (200, json!({ "id": r.header("Idempotency-Key").unwrap().replace("pf_", "remote-"), "duplicate": true }).to_string()));
    drain::recover_at_boot();
    assert_eq!(show(port, OWNER, &id)["state"], json!("queued"));
    assert_eq!(drain_now(), Pass::Ran { sent: 1, refused: 0, retried: 0 });
    assert_eq!(show(port, OWNER, &id)["state"], json!("sent"));
    let last = e.intake.requests().last().cloned().unwrap();
    assert_eq!(last.header("Idempotency-Key"), Some(id.as_str()), "the same key");
}

// ── 2. the intake's answers ──────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_after_reauth_and_refusals() {
    let e = setup(true);
    let port = e.port;

    // 429 with retryAfterSecs: queued, and no pass before then.
    e.intake.set(|_| (429, json!({"retryAfterSecs": 120, "scope": "box"}).to_string()));
    let (s, v) = send(port, OWNER, json!({"text": "one"}));
    assert_eq!(s, 200, "{v}");
    let a = v["id"].as_str().unwrap().to_string();
    let it = wait_row(port, OWNER, &a, "rate limited", |it| it["error"] == json!("rate_limited"));
    let now = k2_core::product_feedback::now_ms();
    let next = it["nextAttemptAt"].as_i64().unwrap();
    assert!(next > now + 100_000 && next < now + 125_000, "next try ~120 s away: {it}");
    due_now(&a);
    assert_eq!(drain_now(), Pass::BackingOff, "retryAfterSecs holds even an urgent pass");

    // 401 once: a new token, then 201.
    drain::reset_for_tests();
    let n_relay = e.relay.requests().len();
    let tries = Arc::new(Mutex::new(0));
    let t2 = tries.clone();
    e.intake.set(move |r| {
        let mut n = t2.lock().unwrap();
        *n += 1;
        if *n == 1 {
            (401, json!({"code": "replayed"}).to_string())
        } else {
            intake_created(r)
        }
    });
    assert_eq!(drain_now(), Pass::Ran { sent: 1, refused: 0, retried: 0 });
    assert_eq!(e.relay.requests().len(), n_relay + 2, "one token, then a new one after the 401");
    assert_eq!(*tries.lock().unwrap(), 2);

    // 503 intake_paused: queued, the service shown as paused.
    e.intake.set(|_| (503, json!({"code": "intake_paused", "retryAfterSecs": 5}).to_string()));
    let (_, v) = send(port, OWNER, json!({"text": "two"}));
    let b = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &b, "paused", |it| it["error"] == json!("intake_paused"));
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!(v["service"]["state"], json!("paused"), "{v}");

    // 400: refused, never retried; retry is for failed ones only.
    drain::reset_for_tests();
    e.intake.set(|_| (400, json!({"code": "bad_body"}).to_string()));
    due_now(&b);
    assert_eq!(drain_now(), Pass::Ran { sent: 0, refused: 1, retried: 0 });
    let it = show(port, OWNER, &b);
    assert_eq!((it["state"].clone(), it["error"].clone()), (json!("refused"), json!("bad_body")), "{it}");
    let n = e.intake.requests().len();
    assert_eq!(drain_now(), Pass::Empty, "nothing due");
    assert_eq!(e.intake.requests().len(), n, "a refused one is not sent again");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/retry", OWNER), Some(json!({"id": b})));
    assert_eq!((s, v["error"].clone()), (409, json!("wrong_state")), "{v}");

    // Seven days of trying: failed; retry puts it back.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE product_feedback_outbox SET state = 'queued', queued_at = 0, attempts = 3, next_attempt_at = ?2 WHERE id = ?1", rusqlite::params![&b, i64::MAX]).unwrap();
    }
    assert_eq!(drain_now(), Pass::Empty);
    assert_eq!(show(port, OWNER, &b)["state"], json!("failed"));
    e.intake.set(intake_created);
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/retry", OWNER), Some(json!({"id": b})));
    assert_eq!((s, v["state"].clone()), (200, json!("queued")), "{v}");
    wait_row(port, OWNER, &b, "sent after retry", |it| it["state"] == json!("sent"));
}

// ── 3. who may do what ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_people_and_wrong_credentials() {
    let e = setup(true);
    let port = e.port;
    // Keep everything queued: the intake is down for this test.
    e.intake.set(|_| (503, "down".into()));

    // GET on every POST-only route: 405.
    for p in ["send", "diagnostics", "retry", "cancel"] {
        let (s, v) = call(port, "GET", &q(&format!("/cli/product-feedback/{p}"), OWNER), None);
        assert_eq!(s, 405, "GET {p}: {v}");
    }

    // App pass and API key: refused.
    for tok in ["k2skn_0123456789abcdef0123456789abcdef", "k2sk_0123456789abcdef0123456789abcdef"] {
        let (s, v) = send(port, tok, json!({"text": "x"}));
        assert_eq!((s, v["error"].clone()), (403, json!("product_feedback_wrong_credential")), "{v}");
    }

    // An agent: cli whatever it says, person-only fields refused, limits.
    let agent = passport("ws-feedback-agent");
    let (s, v) = send(port, &agent, json!({"text": "x", "contactEmail": "a@b.co"}));
    assert_eq!((s, v["error"].clone()), (400, json!("contact_people_only")), "{v}");
    let (s, v) = send(port, &agent, json!({"text": "from an agent", "client": "desktop"}));
    assert_eq!(s, 200, "{v}");
    let aid = v["id"].as_str().unwrap().to_string();
    let it = show(port, OWNER, &aid);
    assert_eq!((it["senderKind"].clone(), it["client"].clone()), (json!("agent"), json!("cli")), "{it}");
    let (s, v) = send(port, &agent, json!({"text": "again"}));
    assert_eq!((s, v["error"].clone()), (429, json!("agent_limit")), "{v}");
    assert_eq!(v["retryAfterSecs"], json!(60), "{v}");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/settings", &agent), Some(json!({"agents": true})));
    assert_eq!((s, v["error"].clone()), (403, json!("settings_people_only")), "{v}");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/settings", &agent), Some(json!({"agentDiagnostics": true})));
    assert_eq!((s, v["error"].clone()), (403, json!("settings_people_only")), "an agent can't allow itself: {v}");

    // The owner turns agents off.
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"agents": false})));
    assert_eq!((s, v["agents"].clone()), (200, json!(false)), "{v}");
    let other = passport("ws-feedback-other");
    let (s, v) = send(port, &other, json!({"text": "x"}));
    assert_eq!((s, v["error"].clone()), (403, json!("agents_off")), "{v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", &other), None);
    assert_eq!((v["canSend"].clone(), v["why"].clone()), (json!(false), json!("agents_off")), "{v}");

    // People: a Member's own rows only; settings are Admin; remembering is per person.
    let alice = login("pf-alice", None);
    let bob = login("pf-bob", None);
    let (s, v) = send(port, &alice, json!({"text": "alice's", "contactEmail": "alice@example.com", "client": "web"}));
    assert_eq!(s, 200, "{v}");
    let alice_id = v["id"].as_str().unwrap().to_string();
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/list", &bob), None);
    assert_eq!(v["items"], json!([]), "bob sees none of alice's: {v}");
    let (s, v) = call(port, "GET", &q(&format!("/cli/product-feedback/show?id={alice_id}"), &bob), None);
    assert_eq!((s, v["error"].clone()), (404, json!("not_found")), "{v}");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/cancel", &bob), Some(json!({"id": alice_id})));
    assert_eq!(s, 404, "{v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/list", &alice), None);
    let ids: Vec<&str> = v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![alice_id.as_str()], "{v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/list", OWNER), None);
    assert_eq!(v["all"], json!(true));
    let all: Vec<&str> = v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(all.contains(&alice_id.as_str()) && all.contains(&aid.as_str()), "the owner sees everyone's, agents' too: {v}");

    let (s, v) = call(port, "POST", &q("/cli/product-feedback/settings", &alice), Some(json!({"enabled": false})));
    assert_eq!((s, v["error"].clone()), (403, json!("admin_only")), "{v}");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/settings", &alice), Some(json!({"rememberDiagnostics": true})));
    assert_eq!((s, v["rememberDiagnostics"].clone()), (200, json!(true)), "{v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", &bob), None);
    assert_eq!(v["rememberDiagnostics"], json!(false), "bob doesn't inherit alice's: {v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", &agent), None);
    assert_eq!(v["rememberDiagnostics"], J::Null, "never an agent's: {v}");

    // Cancel: queued → cancelled, then not again.
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/cancel", &alice), Some(json!({"id": alice_id})));
    assert_eq!((s, v["state"].clone()), (200, json!("cancelled")), "{v}");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/cancel", &alice), Some(json!({"id": alice_id})));
    assert_eq!((s, v["error"].clone()), (409, json!("wrong_state")), "{v}");

    // Bad input.
    let (s, v) = send(port, OWNER, json!({"text": "  "}));
    assert_eq!((s, v["error"].clone()), (400, json!("text_required")), "{v}");
    let (s, v) = send(port, OWNER, json!({"text": "x", "category": "rant"}));
    assert_eq!((s, v["error"].clone()), (400, json!("bad_category")), "{v}");
    let (s, v) = send(port, OWNER, json!({"text": "y".repeat(20_001)}));
    assert_eq!((s, v["error"].clone()), (400, json!("text_too_long")), "{v}");
    let (s, v) = send(port, OWNER, json!({"text": "x", "contactEmail": "not an email"}));
    assert_eq!((s, v["error"].clone()), (400, json!("bad_email")), "{v}");
}

// ── 4. when nothing may leave ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn needs_connect_disabled_and_airgap() {
    let e = setup(false);
    let port = e.port;

    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!((v["canSend"].clone(), v["why"].clone()), (json!(false), json!("needs_connect")), "{v}");
    assert!(v["message"].as_str().unwrap().contains("hello@alakazamlabs.com"), "{v}");
    let (s, v) = send(port, OWNER, json!({"text": "unpaired"}));
    assert_eq!((s, v["error"].clone()), (409, json!("needs_connect")), "{v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/list", OWNER), None);
    assert_eq!(v["items"], json!([]), "nothing stored: {v}");

    seed_tunnel();
    let (s, _) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"enabled": false})));
    assert_eq!(s, 200);
    let (s, v) = send(port, OWNER, json!({"text": "off"}));
    assert_eq!((s, v["error"].clone()), (403, json!("disabled")), "{v}");
    let (s, _) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"enabled": true})));
    assert_eq!(s, 200);

    // A row queued before air-gap came on is held, never sent.
    e.intake.set(|_| (503, "down".into()));
    let (s, v) = send(port, OWNER, json!({"text": "before air-gap"}));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &id, "tried once", |it| it["error"] == json!("service_unreachable"));
    let n = e.intake.requests().len();
    let _airgap = EnvVar::set("K2_AIRGAP", "1");
    let (s, v) = send(port, OWNER, json!({"text": "during air-gap"}));
    assert_eq!((s, v["error"].clone()), (403, json!("airgap")), "{v}");
    due_now(&id);
    e.intake.set(intake_created);
    assert_eq!(drain_now(), Pass::Held("airgap"));
    assert_eq!(e.intake.requests().len(), n, "air-gap: nothing leaves");
    let it = show(port, OWNER, &id);
    assert_eq!((it["state"].clone(), it["error"].clone()), (json!("queued"), json!("airgap")), "{it}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!((v["enabled"].clone(), v["canSend"].clone(), v["why"].clone()), (json!(false), json!(false), json!("airgap")), "{v}");
}

// ── 5. diagnostics: consent, preview, exactly what was shown ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_are_previewed_scrubbed_and_sent_as_shown() {
    let e = setup(true);
    let port = e.port;
    let home = dirs::home_dir().expect("home");
    let ws = home.join("work/acme-payroll");
    std::fs::create_dir_all(&ws).unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES ('p-acme', 'acme-payroll', ?1)",
            [ws.to_string_lossy().to_string()],
        )
        .unwrap();
    }
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let log = format!(
        "{now} [agent-hooks] path=Some(\"{}/src\") failed\n\
         {now} [tunnel] dial 10.0.0.7:7000 refused Authorization: Bearer abc.def.ghi\n\
         {now} [mail] send to dana@example.com failed with k2sk_0123456789abcdef\n\
         {now} [ok] started\n",
        ws.display()
    );
    std::fs::create_dir_all(k2_core::paths::k2_home()).unwrap();
    std::fs::write(k2_core::paths::k2_home().join("daemon.stderr.log"), log).unwrap();

    // No tick: nothing collected or sent.
    let (_, v) = send(port, OWNER, json!({"text": "plain"}));
    let plain = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &plain, "sent", |it| it["state"] == json!("sent"));
    assert!(!e.intake.requests().last().unwrap().json().as_object().unwrap().contains_key("diagnostics"));
    assert_eq!(show(port, OWNER, &plain)["diagnostics"], J::Null);

    // The preview: scrubbed, every section.
    let (s, d) = call(
        port,
        "POST",
        &q("/cli/product-feedback/diagnostics", OWNER),
        Some(json!({"client": "desktop", "k2Version": "0.46.11-test", "page": "Agents"})),
    );
    assert_eq!(s, 200, "{d}");
    let draft = d["draftId"].as_str().expect("draftId").to_string();
    let ids: Vec<&str> = d["sections"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["versions", "computer", "k2-state", "agent-tools", "recent-errors", "log-tail", "context"], "{d}");
    let all = d.to_string();
    for leak in [ws.to_string_lossy().as_ref(), home.to_string_lossy().as_ref(), "10.0.0.7", "dana@example.com", "k2sk_0123456789abcdef", "abc.def.ghi", "acme-payroll"] {
        assert!(!all.contains(leak), "{leak} left in the preview: {all}");
    }
    let errors = d["sections"][4]["text"].as_str().unwrap().to_string();
    assert!(errors.contains("path=Some(\\\"<workspace-1>/src\\\")") || errors.contains("path=Some(\"<workspace-1>/src\")"), "{errors}");
    assert!(errors.contains("Bearer [bearer]") && errors.contains("<ip>") && errors.contains("<email>") && errors.contains("[k2-credential]"), "{errors}");
    assert!(!errors.contains("[ok] started"), "only failure-looking lines: {errors}");
    assert!(d["sections"][0]["text"].as_str().unwrap().contains("Desktop app: 0.46.11-test"), "{d}");
    assert!(d["sections"][6]["text"].as_str().unwrap().contains("Page: Agents"), "{d}");

    // Another person can't use this preview.
    let bob = login("pf-diag-bob", None);
    let (s, v) = send(port, &bob, json!({"text": "x", "diagnostics": {"draftId": draft}}));
    assert_eq!((s, v["error"].clone()), (410, json!("draft_expired")), "{v}");
    // An edit may only delete lines.
    let mut bad_edit = BTreeMap::new();
    bad_edit.insert("recent-errors", format!("{errors}\nan added line"));
    let (s, v) = send(port, OWNER, json!({"text": "x", "diagnostics": {"draftId": draft, "edited": bad_edit}}));
    assert_eq!((s, v["error"].clone()), (400, json!("diagnostics_edit")), "{v}");

    // Leave out a section and delete a line: sent is exactly that.
    let kept_errors: String = errors.lines().skip(1).collect::<Vec<_>>().join("\n");
    let (s, v) = send(
        port,
        OWNER,
        json!({"text": "with details", "category": "bug", "diagnostics": {"draftId": draft, "drop": ["agent-tools"], "edited": {"recent-errors": kept_errors}}}),
    );
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &id, "sent", |it| it["state"] == json!("sent"));
    let body = e.intake.requests().last().unwrap().json();
    let want: Vec<J> = d["sections"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["id"] != json!("agent-tools"))
        .map(|s| {
            let text = if s["id"] == json!("recent-errors") { json!(kept_errors) } else { s["text"].clone() };
            json!({"id": s["id"], "title": s["title"], "text": text})
        })
        .collect();
    assert_eq!(body["diagnostics"], json!({ "sections": want }), "sent == previewed minus what was left out");
    assert_eq!(show(port, OWNER, &id)["diagnostics"], json!(want), "the Sent tab shows what was sent");
    // The draft went with the send.
    let (s, v) = send(port, OWNER, json!({"text": "x", "diagnostics": {"draftId": draft}}));
    assert_eq!((s, v["error"].clone()), (410, json!("draft_expired")), "{v}");

    // Agents: refused until the owner allows agent diagnostics.
    let agent = passport("ws-feedback-diag");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent), Some(json!({})));
    assert_eq!((s, v["error"].clone()), (403, json!("diagnostics_need_owner")), "{v}");
    let (s, v) = send(port, &agent, json!({"text": "x", "diagnostics": {"draftId": draft}}));
    assert_eq!((s, v["error"].clone()), (403, json!("diagnostics_need_owner")), "{v}");
    let (s, _) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"agentDiagnostics": true})));
    assert_eq!(s, 200);
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent), Some(json!({"client": "desktop", "includeClientFaults": true})));
    assert_eq!(s, 200, "{v}");
    assert!(v["sections"][6]["text"].as_str().unwrap().contains("Client: cli"), "a passport is always cli: {v}");
}
