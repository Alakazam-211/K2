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
//!   sent); `429`/`503` honour `retryAfterSecs`; `401 replayed` mints a
//!   fresh token and sends again once, another `401` waits; another `4xx`
//!   is `refused` and never retried;
//! - 0.46.18: every POST has its own box token, so a batch against an
//!   intake that spends each `jti` lands whole; a lost answer (stored, then
//!   `401 replayed`) is resent and its `200 duplicate` is delivered with the
//!   stored id; rows an older drain left failed on `replayed` go back once;
//! - agents: the switch, the box limits, no contact email, no settings;
//!   people: Member sees only their own rows, the owner sees all; App
//!   passes are refused; GET on every POST route is 405;
//! - the server keys: a Member login gets `admin_only`, an agent's or a
//!   terminal's passport `settings_people_only`, an Admin and the owner
//!   change them; each key that changes is one auth audit row (who, the
//!   key, old to new), none for a refusal, a no-op or a Member's own
//!   remembered choice;
//! - `needs_connect`, `disabled` and air-gap refuse a send, and air-gap
//!   holds what is queued;
//! - diagnostics: nothing without the tick, the preview is scrubbed, the
//!   sent sections are the previewed bytes minus what was left out, a draft
//!   is the caller's own and goes once sent, agents need the owner's
//!   `agentDiagnostics`.
//! - a screenshot (prd-feedback-screenshot-v1): too big, not a PNG, a
//!   body over 3 MiB refused unread; chunks cut; the draft is the uploader's
//!   own; passports and App passes refused; `send` names the draft by hash
//!   and a different hash is refused; the intake gets the stored bytes;
//!   sent and refused and cancelled delete the picture from the box, a
//!   failed attempt keeps it and Retry resends the same bytes.
//! - rows that failed because the service couldn't be reached go again by
//!   themselves once it answers: the intake's `GET /health` says `open`
//!   (no answer or `paused` change nothing; at most once an hour; never
//!   under air-gap or with sending off), or it answers a send; oldest
//!   first; a person's switch or a row older than 90 days stays failed.

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
        // The drain is blocking (reqwest::blocking): off the async worker.
        let p = tokio::task::block_in_place(|| drain::drain_once(true));
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

/// The tests in this binary share one database and run one at a time (the
/// env lock): a test that counts what is pending starts from an empty
/// outbox, not what an earlier test left.
fn empty_outbox() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch("DELETE FROM product_feedback_screenshots; DELETE FROM product_feedback_outbox;").unwrap();
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
    empty_outbox();
    let mut events = k2_daemon::session_events::subscribe();

    // The service isn't there yet: the relay has no assert route.
    e.relay.set(|_| (404, "not found".into()));
    e.intake.set(|_| (502, "bad gateway".into()));

    let (s, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["canSend"].clone(), v["enabled"].clone(), v["agents"].clone(), v["agentDiagnostics"].clone()), (json!(true), json!(true), json!(false), json!(false)), "{v}");
    assert_eq!(v["agentsAsked"], json!(false), "a new home: agents off until a person answers: {v}");
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
    // (A person lets agents report anything first: off until asked.)
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"agents": true, "agentPlatform": true})));
    assert_eq!((s, v["agents"].clone(), v["agentPlatform"].clone()), (200, json!(true), json!(true)), "{v}");
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

// ── 3b. who changes the server's settings, and the audit ─────────────

/// The `product-feedback-settings` rows in the auth audit, as (user, outcome).
fn settings_audit() -> Vec<(String, String)> {
    k2_core::auth_audit::tail(1000)
        .expect("audit tail")
        .into_iter()
        .filter(|e| e["event"] == "product-feedback-settings")
        .map(|e| {
            let field = |k: &str| e[k].as_str().unwrap_or_else(|| panic!("audit row without {k}: {e}")).to_string();
            (field("user"), field("outcome"))
        })
        .collect()
}

fn post_settings(port: u16, tok: &str, body: J) -> (u16, J) {
    call(port, "POST", &q("/cli/product-feedback/settings", tok), Some(body))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_keys_need_an_admin_and_each_change_is_audited() {
    use k2_core::product_feedback::settings::{self, Settings};
    let e = setup(true);
    let port = e.port;
    let defaults = Settings::default();
    assert_eq!(settings::load(), defaults);
    assert_eq!(settings_audit(), Vec::<(String, String)>::new(), "a fresh home has no rows");

    // A person typing in a K2 terminal tab: the passport of a plain shell
    // tab a person's client opened (0.46.18: by how K2 started it).
    let shell_key = "tab-pf-settings";
    let shell = {
        let home = k2_core::paths::k2_home();
        std::fs::create_dir_all(&home).expect("k2 home");
        let body = json!({ "agent_name": shell_key, "cwd": home.to_string_lossy(), "cols": 80, "rows": 24 });
        let (s, v) = call(port, "POST", &q("/cli/sessions/v2/spawn", OWNER), Some(body));
        assert_eq!(s, 200, "a person's shell tab: {v}");
        let sid = v["sessionId"].as_str().and_then(SessionId::parse).unwrap_or_else(|| panic!("no sessionId: {v}"));
        session_token::mint_session_token(
            &sid,
            &sid.to_string(),
            HookPrincipal { workspace_uuid: "ws-pf-settings-shell".into(), agent_address: shell_key.into() },
            CredMode::ApiKey,
            Provider::Anthropic,
        )
    };
    let agent = passport("ws-pf-settings-agent");
    let member = login("pfs-member", None);
    let voice = login("pfs-voice", Some(connect_users::Role::VoiceChat));
    let admin = login("pfs-admin", Some(connect_users::Role::Admin));

    // Refused: a Member (even with a per-person key in the same write), a
    // Voice & Chat login (below the route's Member floor), an agent and a
    // terminal. Nothing changes and nothing is audited.
    let all_on = json!({"agents": true, "agentPlatform": true, "agentAttach": "all"});
    for (who, tok, body, code) in [
        ("Member", &member, json!({"agents": true}), Some("admin_only")),
        ("Member, all three", &member, all_on.clone(), Some("admin_only")),
        ("Member with remember", &member, json!({"agents": true, "rememberDiagnostics": true}), Some("admin_only")),
        ("Voice & Chat", &voice, all_on.clone(), None),
        ("agent passport", &agent, all_on.clone(), Some("settings_people_only")),
        ("agent passport, attach", &agent, json!({"agentAttach": ["log-tail"]}), Some("settings_people_only")),
        ("terminal passport", &shell, all_on.clone(), Some("settings_people_only")),
    ] {
        let (s, v) = post_settings(port, tok, body);
        assert_eq!(s, 403, "{who}: {v}");
        if let Some(code) = code {
            assert_eq!(v["error"], json!(code), "{who}: {v}");
        }
        assert_eq!(settings::load(), defaults, "{who} changed nothing");
    }
    let (s, v) = call(port, "GET", &q("/cli/product-feedback/settings", &voice), None);
    assert_eq!(s, 403, "Voice & Chat doesn't read them either: {v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", &member), None);
    assert_eq!(v["rememberDiagnostics"], json!(false), "the refused write kept nothing: {v}");
    assert_eq!(v["canEdit"], json!(false), "{v}");
    assert_eq!(settings_audit(), Vec::<(String, String)>::new(), "a refused attempt writes no row");

    // Bad answers: refused, nothing written.
    for (body, code) in [
        (json!({"agents": false, "agentPlatform": true}), "platform_needs_agents"),
        (json!({"agentPlatform": true}), "platform_needs_agents"),
        (json!({"agentAttach": ["log-tail", "context"]}), "bad_attach_part"),
        (json!({"agentAttach": ["versions"]}), "bad_attach_part"),
        (json!({"agentAttach": ["none", "log-tail"]}), "bad_attach_part"),
        (json!({"agentAttach": 3}), "bad_attach_part"),
        (json!({"agentAttach": "none", "agentDiagnostics": true}), "attach_twice"),
        (json!({"agentCli": true}), "bad_json"),
    ] {
        let (s, v) = post_settings(port, &admin, body.clone());
        assert_eq!((s, v["error"].clone()), (400, json!(code)), "{body}: {v}");
        assert_eq!(settings::load(), defaults, "{body} changed nothing");
    }
    // The parts alone aren't an answer while nobody has been asked.
    for body in [json!({"agentAttach": ["log-tail"]}), json!({"agentDiagnostics": true})] {
        let (s, v) = post_settings(port, &admin, body.clone());
        assert_eq!((s, v["error"].clone()), (409, json!("answer_agents_first")), "{body}: {v}");
        assert_eq!(settings::load(), defaults, "{body} changed nothing");
    }

    // An Admin answers "CLI problems, nothing attached" in one write: the
    // daemon stamps when and who; rows for what moved (and the answer).
    let before = k2_core::product_feedback::now_ms();
    let (s, v) = post_settings(port, &admin, json!({"agents": true, "agentPlatform": false, "agentAttach": "none"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["agents"].clone(), v["agentPlatform"].clone(), v["agentAttach"].clone(), v["agentDiagnostics"].clone()), (json!(true), json!(false), json!([]), json!(false)), "{v}");
    assert_eq!((v["agentsAsked"].clone(), v["agentsAskedBy"].clone(), v["agentScopes"].clone()), (json!(true), json!("pfs-admin"), json!(["cli"])), "{v}");
    assert!(v["agentsAskedAt"].as_i64().unwrap() >= before, "{v}");
    let parts: Vec<&str> = v["attachParts"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(parts, vec!["computer", "k2-state", "agent-tools", "recent-errors", "log-tail"], "{v}");
    assert!(v["attachParts"].as_array().unwrap().iter().all(|p| !p["label"].as_str().unwrap().is_empty()), "{v}");
    let mut want = vec![
        ("login:pfs-admin".to_string(), "ok agentsAsked no->yes".to_string()),
        ("login:pfs-admin".to_string(), "ok agents off->on".to_string()),
    ];
    assert_eq!(settings_audit(), want);

    // The terminal's passport was a person's, the agent's an agent's.
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", &shell), None);
    assert_eq!(v["senderKind"], json!("person"), "{v}");
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", &agent), None);
    assert_eq!((v["senderKind"].clone(), v["canSend"].clone(), v["agentScopes"].clone()), (json!("agent"), json!(true), json!(["cli"])), "{v}");

    // A write of the values already set: 200 and no row.
    let (s, v) = post_settings(port, &admin, json!({"agents": true, "enabled": true, "agentAttach": []}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(settings_audit(), want, "a no-op writes no row");

    // The owner: everything, then two parts; one row per key that changed.
    let (s, v) = post_settings(port, OWNER, all_on.clone());
    assert_eq!((s, v["agentPlatform"].clone(), v["agentDiagnostics"].clone(), v["agentsAskedBy"].clone()), (200, json!(true), json!(true), json!("owner")), "{v}");
    let (s, v) = post_settings(port, OWNER, json!({"agentAttach": "log-tail,computer"}));
    assert_eq!((s, v["agentAttach"].clone(), v["agentPlatform"].clone()), (200, json!(["computer", "log-tail"]), json!(true)), "the others stay: {v}");
    want.extend([
        ("owner".to_string(), "ok agentPlatform off->on".to_string()),
        ("owner".to_string(), "ok agentAttach none->computer,k2-state,agent-tools,recent-errors,log-tail".to_string()),
        ("owner".to_string(), "ok agentAttach computer,k2-state,agent-tools,recent-errors,log-tail->computer,log-tail".to_string()),
    ]);
    assert_eq!(settings_audit(), want);
    let after_owner = settings::load();
    assert!(after_owner.agents && after_owner.agent_platform && after_owner.agent_diagnostics, "{after_owner:?}");

    // Agents off takes platform with it.
    let (s, v) = post_settings(port, OWNER, json!({"agents": false}));
    assert_eq!((s, v["agents"].clone(), v["agentPlatform"].clone(), v["agentAttach"].clone()), (200, json!(false), json!(false), json!([])), "{v}");
    want.extend([
        ("owner".to_string(), "ok agents on->off".to_string()),
        ("owner".to_string(), "ok agentPlatform on->off".to_string()),
        ("owner".to_string(), "ok agentAttach computer,log-tail->none".to_string()),
    ]);
    assert_eq!(settings_audit(), want);
    let after_owner = settings::load();

    // A Member remembers their own choice: 200, no server key moves, no row.
    let (s, v) = post_settings(port, &member, json!({"rememberDiagnostics": true}));
    assert_eq!((s, v["rememberDiagnostics"].clone()), (200, json!(true)), "{v}");
    assert_eq!(settings::load(), after_owner);
    assert_eq!(settings_audit(), want, "a per-person choice is not a server change");

    drop(k2_daemon::v2_session_map::unregister(shell_key));
}

// ── 3c. agents report on their own only after a person says yes (0.46.17) ──

fn outbox_count() -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM product_feedback_outbox", [], |r| r.get(0)).unwrap()
}

/// A registered workspace for `id`, so its AGENTS.md is recomposed when the
/// switches change; returns the folder.
fn workspace(id: &str) -> std::path::PathBuf {
    let dir = dirs::home_dir().expect("home").join("work").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, agents_can_request_permissions) VALUES (?1, ?1, ?2, 1)",
        rusqlite::params![id, dir.to_string_lossy().to_string()],
    )
    .unwrap();
    dir
}

fn agents_md(dir: &std::path::Path) -> String {
    let p = k2_core::workspace_dot_dir(dir).join("AGENTS.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The recompose runs on its own thread after the write.
fn wait_agents_md(dir: &std::path::Path, what: &str, pred: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        k2_core::workspace::skill_regen::ensure_agents_md_if_missing(&dir.to_string_lossy());
        let body = agents_md(dir);
        if pred(&body) {
            return body;
        }
        assert!(Instant::now() < deadline, "AGENTS.md never {what}: {body}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_report_only_what_a_person_allowed() {
    use k2_core::product_feedback::{settings, teach};
    let e = setup(true);
    let port = e.port;
    empty_outbox();
    let _cards = EnvVar::set("K2_PERMISSION_CARDS", "1");
    let ws_a = "ws-optin-a";
    let dir_a = workspace(ws_a);
    let agent = passport(ws_a);
    let agent_b = passport("ws-optin-b");
    workspace("ws-optin-b");

    // The migration: a 0.46.16 file (agents on by default, diagnostics on)
    // reads as everything off for agents.
    std::fs::write(settings::path(), r#"{"enabled": true, "agents": true, "agentDiagnostics": true}"#).unwrap();
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!(
        (v["agents"].clone(), v["agentPlatform"].clone(), v["agentAttach"].clone(), v["agentDiagnostics"].clone(), v["agentsAsked"].clone(), v["agentsAskedAt"].clone()),
        (json!(false), json!(false), json!([]), json!(false), json!(false), J::Null),
        "an old file: all off until asked: {v}"
    );
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", &agent), None);
    assert_eq!((v["canSend"].clone(), v["why"].clone(), v["agentScopes"].clone()), (json!(false), json!("agents_off"), json!([])), "{v}");
    let n = outbox_count();
    for body in [json!({"text": "x"}), json!({"text": "x", "scope": "cli", "command": "k2 msg x"}), json!({"text": "x", "scope": "platform"})] {
        let (s, v) = send(port, &agent, body.clone());
        assert_eq!((s, v["error"].clone()), (403, json!("agents_off")), "{body}: {v}");
        // Permission cards on: the refusal says it can't be requested.
        assert_eq!(v["permission"]["requestable"], json!(false), "{v}");
    }
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent), Some(json!({})));
    assert_eq!((s, v["error"].clone()), (403, json!("agents_off")), "{v}");
    assert_eq!(outbox_count(), n, "nothing stored while off");
    // No note while off: AGENTS.md says nothing about reporting.
    k2_core::workspace::skill_regen::recompose_agents_md(&dir_a.to_string_lossy());
    let off_md = agents_md(&dir_a);
    assert!(!off_md.contains(teach::HEADING) && !off_md.contains("send-feedback"), "{off_md}");

    // A person says yes to CLI problems only, nothing attached.
    let (s, v) = post_settings(port, OWNER, json!({"agents": true, "agentPlatform": false, "agentAttach": "none"}));
    assert_eq!(s, 200, "{v}");
    let md = wait_agents_md(&dir_a, "gained the section", |b| b.contains(teach::HEADING));
    assert!(md.contains("Not on this server: report only CLI problems") && md.contains("Nothing: send text only"), "{md}");

    // cli needs a command; a bad scope or command is refused.
    for (body, code) in [
        (json!({"text": "x", "scope": "cli"}), "command_required"),
        (json!({"text": "x", "scope": "cli", "command": "  "}), "command_required"),
        (json!({"text": "x", "scope": "cli", "command": "k2 a\nk2 b"}), "bad_command"),
        (json!({"text": "x", "scope": "cli", "command": "k".repeat(1001)}), "bad_command"),
        (json!({"text": "x", "scope": "everything"}), "bad_scope"),
    ] {
        let (s, v) = send(port, &agent, body.clone());
        assert_eq!((s, v["error"].clone()), (400, json!(code)), "{body}: {v}");
    }
    // platform (or no scope: an older CLI) while only cli is on: scope_off.
    for body in [json!({"text": "the Garden is slow", "scope": "platform"}), json!({"text": "no scope"})] {
        let (s, v) = send(port, &agent, body.clone());
        assert_eq!((s, v["error"].clone()), (403, json!("scope_off")), "{body}: {v}");
        assert!(v["message"].as_str().unwrap().contains("--scope cli"), "{v}");
        assert_eq!(v["permission"]["requestable"], json!(false), "the Ask-with closing line: {v}");
    }
    // Details refused: nothing attachable.
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent), Some(json!({})));
    assert_eq!((s, v["error"].clone()), (403, json!("diagnostics_need_owner")), "{v}");
    assert_eq!(outbox_count(), n, "nothing stored for a refusal");

    // A cli report: tagged by scope, with the command, as the first details
    // section the intake gets; the Sent tab shows it as sent.
    let (s, v) = send(port, &agent, json!({"text": "help says --json, the command refuses it", "scope": "cli", "command": "k2 msg list --json", "category": "bug"}));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let it = wait_row(port, OWNER, &id, "sent", |it| it["state"] == json!("sent"));
    assert_eq!((it["scope"].clone(), it["senderKind"].clone()), (json!("cli"), json!("agent")), "{it}");
    let tag = json!([{"id": "agent-report", "title": "Agent report", "text": "Scope: cli (a K2 CLI problem)\nCommand: k2 msg list --json"}]);
    assert_eq!(it["diagnostics"], tag, "{it}");
    let body = e.intake.requests().last().unwrap().json();
    assert_eq!(body["diagnostics"], json!({ "sections": tag }), "the service's tag: {body}");
    let mut keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["arch", "category", "client", "clientId", "daemonVersion", "diagnostics", "installKind", "k2Version", "locale", "os", "osVersion", "senderKind", "text"],
        "no field the intake doesn't take: {body}"
    );

    // A person's report has no scope (one sent is ignored, never refused).
    let (s, v) = send(port, OWNER, json!({"text": "a person's", "scope": "platform", "command": "k2 x"}));
    assert_eq!(s, 200, "{v}");
    let pid = v["id"].as_str().unwrap().to_string();
    let it = wait_row(port, OWNER, &pid, "sent", |it| it["state"] == json!("sent"));
    assert_eq!((it["scope"].clone(), it["diagnostics"].clone()), (J::Null, J::Null), "{it}");
    assert!(!e.intake.requests().last().unwrap().json().as_object().unwrap().contains_key("diagnostics"));

    // Platform too, with two parts: a platform report goes, and an agent's
    // details are the versions and exactly those parts.
    let (s, v) = post_settings(port, OWNER, json!({"agents": true, "agentPlatform": true, "agentAttach": ["log-tail", "computer"]}));
    assert_eq!((s, v["agentScopes"].clone()), (200, json!(["cli", "platform"])), "{v}");
    wait_agents_md(&dir_a, "said platform is allowed", |b| b.contains("Also allowed: `--scope platform`"));
    let (s, d) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent_b), Some(json!({})));
    assert_eq!(s, 200, "{d}");
    let ids: Vec<&str> = d["sections"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["versions", "computer", "log-tail"], "{d}");
    let draft = d["draftId"].as_str().unwrap().to_string();
    // The person narrows it before the agent sends: only what's allowed now.
    let (s, _) = post_settings(port, OWNER, json!({"agentAttach": ["computer"]}));
    assert_eq!(s, 200);
    let (s, v) = send(port, &agent_b, json!({"text": "Settings froze", "scope": "platform", "diagnostics": {"draftId": draft}}));
    assert_eq!(s, 200, "{v}");
    let bid = v["id"].as_str().unwrap().to_string();
    let it = wait_row(port, OWNER, &bid, "sent", |it| it["state"] == json!("sent"));
    let sent: Vec<&str> = it["diagnostics"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(sent, vec!["agent-report", "versions", "computer"], "{it}");
    assert_eq!(it["diagnostics"][0]["text"], json!("Scope: platform (something about K2)"), "{it}");
    assert_eq!(it["scope"], json!("platform"));
    let body = e.intake.requests().last().unwrap().json();
    let wire_ids: Vec<&str> = body["diagnostics"]["sections"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(wire_ids, sent, "{body}");

    // A no: everything off again, and the section goes.
    let (s, v) = post_settings(port, OWNER, json!({"agents": false, "agentPlatform": false, "agentAttach": "none"}));
    assert_eq!((s, v["agents"].clone(), v["agentsAsked"].clone()), (200, json!(false), json!(true)), "{v}");
    let gone = wait_agents_md(&dir_a, "lost the section", |b| !b.contains(teach::HEADING));
    assert!(!gone.contains("send-feedback"), "{gone}");
    let (s, v) = send(port, &agent, json!({"text": "x", "scope": "cli", "command": "k2 y"}));
    assert_eq!((s, v["error"].clone()), (403, json!("agents_off")), "{v}");
}

// ── 4. when nothing may leave ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn needs_connect_disabled_and_airgap() {
    let e = setup(false);
    let port = e.port;

    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!((v["canSend"].clone(), v["why"].clone()), (json!(false), json!("needs_connect")), "{v}");
    assert!(v["message"].as_str().unwrap().contains("hello@alakazamlabs.com"), "{v}");
    // The tests in this binary share one database: count, don't expect empty.
    let outbox_rows = || -> i64 {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM product_feedback_outbox", [], |r| r.get(0)).unwrap()
    };
    let before = outbox_rows();
    let (s, v) = send(port, OWNER, json!({"text": "unpaired"}));
    assert_eq!((s, v["error"].clone()), (409, json!("needs_connect")), "{v}");
    assert_eq!(outbox_rows(), before, "nothing stored");

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
    // The workspace's number depends on the other workspaces on the box.
    let path_line = errors.lines().find(|l| l.contains("[agent-hooks]")).expect("the agent-hooks line");
    let ph = path_line.split("path=Some(\"").nth(1).and_then(|r| r.strip_suffix("/src\") failed")).unwrap_or_else(|| panic!("{path_line}"));
    assert!(ph.starts_with("<workspace-") && ph.ends_with('>') && ph[11..ph.len() - 1].bytes().all(|b| b.is_ascii_digit()), "{path_line}");
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

    // Agents: off until a person answers; then refused until a person
    // allows some part of the details.
    let agent = passport("ws-feedback-diag");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent), Some(json!({})));
    assert_eq!((s, v["error"].clone()), (403, json!("agents_off")), "{v}");
    let (s, _) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"agents": true, "agentPlatform": true})));
    assert_eq!(s, 200);
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent), Some(json!({})));
    assert_eq!((s, v["error"].clone()), (403, json!("diagnostics_need_owner")), "{v}");
    let (s, v) = send(port, &agent, json!({"text": "x", "diagnostics": {"draftId": draft}}));
    assert_eq!((s, v["error"].clone()), (403, json!("diagnostics_need_owner")), "{v}");
    // The 0.46.11 switch still works: every part.
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"agentDiagnostics": true})));
    assert_eq!((s, v["agentAttach"].clone()), (200, json!(["computer", "k2-state", "agent-tools", "recent-errors", "log-tail"])), "{v}");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/diagnostics", &agent), Some(json!({"client": "desktop", "includeClientFaults": true, "page": "Agents"})));
    assert_eq!(s, 200, "{v}");
    let ids: Vec<&str> = v["sections"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["versions", "computer", "k2-state", "agent-tools", "recent-errors", "log-tail"], "never a person's page: {v}");
    assert!(!v.to_string().contains("Page: Agents"), "{v}");
}

// ── 6. a screenshot: seen, sent, deleted from the box ────────────────

fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn unb64(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).expect("base64")
}

fn upload(port: u16, tok: &str, png: &[u8], source: &str, capture: Option<&str>) -> (u16, J) {
    let mut body = json!({ "image": b64(png), "source": source });
    if let Some(c) = capture {
        body["capture"] = json!(c);
    }
    call(port, "POST", &q("/cli/product-feedback/screenshot", tok), Some(body))
}

fn draft_bytes(port: u16, tok: &str, draft: &str) -> (u16, J) {
    call(port, "GET", &q(&format!("/cli/product-feedback/screenshot?draftId={draft}"), tok), None)
}

fn row_shot(port: u16, tok: &str, id: &str) -> (u16, J) {
    call(port, "GET", &q(&format!("/cli/product-feedback/screenshot?id={id}"), tok), None)
}

fn data_url_bytes(v: &J) -> Vec<u8> {
    let url = v["dataUrl"].as_str().unwrap_or_else(|| panic!("no dataUrl: {v}"));
    unb64(url.strip_prefix("data:image/png;base64,").unwrap_or_else(|| panic!("not a PNG data URL: {url}")))
}

fn sha(b: &[u8]) -> String {
    k2_core::product_feedback::screenshot::sha256_hex(b)
}

fn n_rows(port: u16) -> usize {
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/list", OWNER), None);
    v["items"].as_array().expect("items").len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_screenshot_is_seen_then_sent_then_deleted_from_the_box() {
    use k2_core::product_feedback::screenshot::fixture;
    let e = setup(true);
    let port = e.port;
    // The intake is down at first: everything stays queued.
    e.intake.set(|_| (503, "down".into()));

    // The feature key.
    let (_, b) = call(port, "GET", "/boot-status", None);
    assert!(b["features"].as_array().unwrap().iter().any(|f| f == "product-feedback-screenshot-v1"), "{b}");

    // GET on the POST-only forget route: 405. GET with neither id: 400.
    let (s, v) = call(port, "GET", &q("/cli/product-feedback/screenshot/forget", OWNER), None);
    assert_eq!(s, 405, "{v}");
    let (s, v) = call(port, "GET", &q("/cli/product-feedback/screenshot", OWNER), None);
    assert_eq!((s, v["error"].clone()), (400, json!("draft_id_required")), "{v}");

    // Not a PNG; too big; a body over 3 MiB refused from its head.
    let (s, v) = upload(port, OWNER, b"GIF89a this is not a png", "file", None);
    assert_eq!((s, v["error"].clone()), (400, json!("bad_screenshot")), "{v}");
    let (s, v) = upload(port, OWNER, &vec![0u8; 2_200_000], "file", None);
    assert_eq!((s, v["error"].clone()), (413, json!("screenshot_too_large")), "{v}");
    {
        let big = "A".repeat(3 * 1024 * 1024 + 1024);
        let body = format!(r#"{{"source":"file","image":"{big}"}}"#);
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(20))).expect("timeout");
        let head = format!(
            "POST /cli/product-feedback/screenshot?token={OWNER} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).expect("head");
        let _ = stream.write_all(body.as_bytes());
        let mut raw = Vec::new();
        let _ = stream.read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("HTTP/1.1 413"), "{text}");
        assert!(text.contains("screenshot_too_large"), "{text}");
    }
    // The other routes keep the 1 MiB cap.
    let (s, v) = send(port, OWNER, json!({"text": "x".repeat(1024 * 1024 + 10)}));
    assert_eq!((s, v["error"].clone()), (413, json!("body_too_large")), "{v}");

    // A window picture with a text chunk: the stored copy has none, and
    // its hash is of the stored bytes.
    let clean = fixture::png(320, 200);
    let dirty = fixture::with_chunks_after_ihdr(&clean, &[fixture::chunk(b"tEXt", b"Title\0Payroll - acme")]);
    let (s, up) = upload(port, OWNER, &dirty, "window", Some("page"));
    assert_eq!(s, 200, "{up}");
    let draft = up["draftId"].as_str().expect("draftId").to_string();
    let hash = up["sha256"].as_str().expect("sha256").to_string();
    assert_eq!((up["width"].clone(), up["height"].clone(), up["source"].clone(), up["capture"].clone()), (json!(320), json!(200), json!("window"), json!("page")), "{up}");
    assert_eq!(up["bytes"], json!(clean.len()), "{up}");
    let (s, g) = draft_bytes(port, OWNER, &draft);
    assert_eq!(s, 200, "{g}");
    let seen = data_url_bytes(&g);
    assert_eq!(seen, clean, "the text chunk was cut, nothing else");
    assert_eq!(sha(&seen), hash, "the hash names the bytes the person sees");
    assert_ne!(hash, sha(&dirty));

    // Passports and App passes: refused, on every screenshot route and on send.
    let agent = passport("ws-shot-agent");
    let (s, v) = upload(port, &agent, &clean, "file", None);
    assert_eq!((s, v["error"].clone()), (403, json!("screenshot_people_only")), "{v}");
    let (s, v) = draft_bytes(port, &agent, &draft);
    assert_eq!((s, v["error"].clone()), (403, json!("screenshot_people_only")), "{v}");
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/screenshot/forget", &agent), Some(json!({"draftId": draft})));
    assert_eq!((s, v["error"].clone()), (403, json!("screenshot_people_only")), "{v}");
    let (s, v) = send(port, &agent, json!({"text": "x", "screenshot": {"draftId": draft, "sha256": hash}}));
    assert_eq!((s, v["error"].clone()), (403, json!("screenshot_people_only")), "{v}");
    let (s, v) = upload(port, "k2skn_0123456789abcdef0123456789abcdef", &clean, "file", None);
    assert_eq!((s, v["error"].clone()), (403, json!("product_feedback_wrong_credential")), "{v}");

    // Another person can't see or use the draft.
    let bob = login("pf-shot-bob", None);
    let (s, v) = draft_bytes(port, &bob, &draft);
    assert_eq!((s, v["error"].clone()), (410, json!("screenshot_expired")), "{v}");
    let (s, v) = send(port, &bob, json!({"text": "x", "screenshot": {"draftId": draft, "sha256": hash}}));
    assert_eq!((s, v["error"].clone()), (410, json!("screenshot_expired")), "{v}");

    // The "seen" gate: a hash that isn't the draft's is refused, nothing saved.
    let before = n_rows(port);
    let (s, v) = send(port, OWNER, json!({"text": "x", "screenshot": {"draftId": draft, "sha256": sha(&dirty)}}));
    assert_eq!((s, v["error"].clone()), (409, json!("screenshot_changed")), "{v}");
    let (s, v) = send(port, OWNER, json!({"text": "x", "screenshot": {"draftId": draft}}));
    assert_eq!((s, v["error"].clone()), (409, json!("screenshot_changed")), "no hash is no consent: {v}");
    assert_eq!(n_rows(port), before, "nothing stored");

    // The right hash: saved with the row; the intake is down, so it stays.
    let (s, v) = send(port, OWNER, json!({"text": "the tab froze, see the picture", "category": "bug", "client": "desktop", "screenshot": {"draftId": draft, "sha256": hash.to_uppercase()}}));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &id, "tried once", |it| it["error"] == json!("service_unreachable"));
    let it = show(port, OWNER, &id);
    assert_eq!(it["hasScreenshot"], json!(true), "{it}");
    assert_eq!(it["screenshot"], json!({"bytes": clean.len(), "width": 320, "height": 200, "source": "window", "capture": "page", "onBox": true}), "{it}");
    let first = e.intake.requests().last().cloned().expect("the failed attempt");
    assert_eq!(unb64(first.json()["screenshot"]["data"].as_str().unwrap()), clean, "every attempt carries the picture");
    let (s, v) = row_shot(port, OWNER, &id);
    assert_eq!(s, 200, "a failed attempt keeps it: {v}");
    assert_eq!(data_url_bytes(&v), clean);
    let (s, v) = row_shot(port, &bob, &id);
    assert_eq!(s, 404, "another person's row: {v}");
    // The draft went with the send.
    let (s, v) = send(port, OWNER, json!({"text": "again", "screenshot": {"draftId": draft, "sha256": hash}}));
    assert_eq!((s, v["error"].clone()), (410, json!("screenshot_expired")), "{v}");

    // The intake answers: sent with the picture, and the box forgets it.
    e.intake.set(intake_created);
    due_now(&id);
    assert_eq!(drain_now(), Pass::Ran { sent: 1, refused: 0, retried: 0 });
    let sub = e.intake.requests().last().cloned().expect("submission");
    assert_eq!(sub.header("Idempotency-Key"), Some(id.as_str()), "the same key as the failed attempt");
    let b = sub.json();
    let w = &b["screenshot"];
    let mut keys: Vec<String> = w.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["capture", "data", "height", "mime", "sha256", "source", "width"].map(String::from).to_vec(), "{w}");
    assert_eq!((w["mime"].clone(), w["source"].clone(), w["capture"].clone(), w["width"].clone(), w["height"].clone()), (json!("image/png"), json!("window"), json!("page"), json!(320), json!(200)));
    let got = unb64(w["data"].as_str().unwrap());
    assert_eq!(got, seen, "the intake got exactly the bytes the person saw");
    assert_eq!(w["sha256"], json!(sha(&got)));
    assert!(sub.body.len() < 3 * 1024 * 1024, "under the intake's cap");
    let it = show(port, OWNER, &id);
    assert_eq!((it["state"].clone(), it["hasScreenshot"].clone(), it["screenshot"]["onBox"].clone()), (json!("sent"), json!(true), json!(false)), "{it}");
    let (s, v) = row_shot(port, OWNER, &id);
    assert_eq!((s, v["error"].clone()), (404, json!("screenshot_not_on_box")), "{v}");

    // A row without one says so.
    let (s, v) = send(port, OWNER, json!({"text": "no picture"}));
    assert_eq!(s, 200, "{v}");
    let plain = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &plain, "sent", |it| it["state"] == json!("sent"));
    assert!(e.intake.requests().last().unwrap().json().get("screenshot").is_none());
    let (s, v) = row_shot(port, OWNER, &plain);
    assert_eq!((s, v["error"].clone()), (404, json!("no_screenshot")), "{v}");

    // Cancelled: the picture goes. A person's own Remove forgets a draft.
    e.intake.set(|_| (503, "down".into()));
    let (_, up) = upload(port, OWNER, &fixture::png(50, 40), "file", None);
    assert_eq!(up["capture"], J::Null, "{up}");
    let (s, v) = send(port, OWNER, json!({"text": "to cancel", "screenshot": {"draftId": up["draftId"], "sha256": up["sha256"]}}));
    assert_eq!(s, 200, "{v}");
    let c_id = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &c_id, "tried once", |it| it["error"] == json!("service_unreachable"));
    let (s, _) = row_shot(port, OWNER, &c_id);
    assert_eq!(s, 200);
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/cancel", OWNER), Some(json!({"id": c_id})));
    assert_eq!((s, v["state"].clone()), (200, json!("cancelled")), "{v}");
    let (s, v) = row_shot(port, OWNER, &c_id);
    assert_eq!((s, v["error"].clone()), (404, json!("screenshot_not_on_box")), "cancel deletes it: {v}");
    let (_, up) = upload(port, OWNER, &clean, "file", None);
    let d2 = up["draftId"].as_str().unwrap().to_string();
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/screenshot/forget", OWNER), Some(json!({"draftId": d2})));
    assert_eq!((s, v["forgotten"].clone()), (200, json!(true)), "{v}");
    let (s, _) = draft_bytes(port, OWNER, &d2);
    assert_eq!(s, 410, "Remove forgot it");

    // Failed keeps it; Retry resends the same bytes. A login uploads too.
    let alice = login("pf-shot-alice", None);
    let pic = fixture::png(64, 64);
    let (s, up) = upload(port, &alice, &pic, "window", Some("webview"));
    assert_eq!(s, 200, "{up}");
    let (s, v) = send(port, &alice, json!({"text": "alice's", "client": "web", "screenshot": {"draftId": up["draftId"], "sha256": up["sha256"]}}));
    assert_eq!(s, 200, "{v}");
    let f_id = v["id"].as_str().unwrap().to_string();
    wait_row(port, &alice, &f_id, "tried once", |it| it["error"] == json!("service_unreachable"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE product_feedback_outbox SET state = 'failed' WHERE id = ?1", [&f_id]).unwrap();
    }
    let (s, _) = row_shot(port, &alice, &f_id);
    assert_eq!(s, 200, "failed keeps it");
    e.intake.set(intake_created);
    drain::reset_for_tests();
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/retry", &alice), Some(json!({"id": f_id})));
    assert_eq!((s, v["state"].clone()), (200, json!("queued")), "{v}");
    wait_row(port, &alice, &f_id, "sent after retry", |it| it["state"] == json!("sent"));
    let last = e.intake.requests().last().cloned().unwrap();
    assert_eq!(last.header("Idempotency-Key"), Some(f_id.as_str()));
    assert_eq!(unb64(last.json()["screenshot"]["data"].as_str().unwrap()), pic, "Retry sent the same bytes");
    assert_eq!(last.json()["screenshot"]["capture"], json!("webview"));
    let (s, _) = row_shot(port, &alice, &f_id);
    assert_eq!(s, 404, "sent: gone");

    // Refused: the picture goes.
    e.intake.set(|_| (400, json!({"code": "bad_screenshot"}).to_string()));
    let (_, up) = upload(port, OWNER, &pic, "file", None);
    let (s, v) = send(port, OWNER, json!({"text": "refused", "screenshot": {"draftId": up["draftId"], "sha256": up["sha256"]}}));
    assert_eq!(s, 200, "{v}");
    let r_id = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &r_id, "refused", |it| it["state"] == json!("refused"));
    let (s, v) = row_shot(port, OWNER, &r_id);
    assert_eq!((s, v["error"].clone()), (404, json!("screenshot_not_on_box")), "refused deletes it: {v}");
    let left: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM product_feedback_screenshots", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(left, 0, "no picture left on the box");
}

// ── 7. failed rows go again once the service answers ─────────────────

/// Make a row `failed` after 7 days of trying, created `days_ago`, with
/// `error` as its last reason.
fn fail_row(id: &str, days_ago: i64, error: &str) {
    let now = k2_core::product_feedback::now_ms();
    let created = now - days_ago * 86_400_000;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "UPDATE product_feedback_outbox SET state = 'failed', created_at = ?2, queued_at = ?2, attempts = 40, \
             last_error = ?3, updated_at = ?4 WHERE id = ?1",
            rusqlite::params![id, created, error, now - 86_400_000],
        )
        .unwrap();
    assert_eq!(n, 1, "{id} exists");
}

const HEALTH_OPEN: &str = r#"{"db":true,"intake":"open","ok":true}"#;
const HEALTH_PAUSED: &str = r#"{"db":true,"intake":"paused","ok":true}"#;

/// An intake whose `/health` answers `health` and whose submissions answer
/// `submit`.
fn intake_answers(
    health: (u16, &'static str),
    submit: impl Fn(&Req) -> (u16, String) + Send + 'static,
) -> impl Fn(&Req) -> (u16, String) + Send + 'static {
    move |r: &Req| if r.path == "/health" { (health.0, health.1.to_string()) } else { submit(r) }
}

fn health_checks(intake: &Fake) -> usize {
    intake.requests().iter().filter(|r| r.method == "GET" && r.path == "/health").count()
}

fn submissions(intake: &Fake) -> Vec<String> {
    intake
        .requests()
        .iter()
        .filter(|r| r.path == "/v1/submissions")
        .map(|r| r.header("Idempotency-Key").expect("key").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_rows_go_again_by_themselves_once_the_service_answers() {
    let e = setup(true);
    let port = e.port;
    // The health check is only asked while nothing else is queued.
    empty_outbox();

    // Five rows saved while the service wasn't there.
    e.intake.set(|_| (502, "bad gateway".into()));
    let mut ids = Vec::new();
    for t in ["oldest", "second", "third", "turned off", "too old"] {
        let (s, v) = send(port, OWNER, json!({"text": t}));
        assert_eq!(s, 200, "{v}");
        let id = v["id"].as_str().unwrap().to_string();
        wait_row(port, OWNER, &id, "tried once", |it| it["error"] == json!("service_unreachable"));
        ids.push(id);
    }
    let [oldest, second, third, off, old] = <[String; 5]>::try_from(ids).unwrap();
    // After 7 days of trying they are failed, for these reasons.
    fail_row(&oldest, 30, "service_unreachable: connection refused");
    fail_row(&second, 20, "relay_not_ready: the K2 Connect relay isn't ready");
    fail_row(&third, 10, "intake_paused: maintenance");
    fail_row(&off, 15, "disabled: Sending feedback to K2 is turned off on this server");
    fail_row(&old, 91, "service_unreachable: connection refused");
    let sent_before = submissions(&e.intake).len();

    // No answer from /health: nothing changes, and no row is used to ask.
    drain::reset_for_tests();
    assert_eq!(drain_now(), Pass::Probed { health: "down", requeued: 0 });
    assert_eq!(health_checks(&e.intake), 1, "one GET /health");
    assert_eq!(submissions(&e.intake).len(), sent_before, "no row sent to ask");
    for id in [&oldest, &second, &third, &off, &old] {
        let it = show(port, OWNER, id);
        assert_eq!((it["state"].clone(), it["attempts"].clone()), (json!("failed"), json!(40)), "{id}: {it}");
    }
    // At most once an hour.
    assert_eq!(drain_now(), Pass::BackingOff, "no second health check at once");
    assert_eq!(health_checks(&e.intake), 1);

    // Up but paused: hold off, nothing queued again.
    e.intake.set(intake_answers((200, HEALTH_PAUSED), |_| (502, "bad gateway".into())));
    drain::reset_for_tests();
    assert_eq!(drain_now(), Pass::Probed { health: "paused", requeued: 0 });
    assert_eq!(health_checks(&e.intake), 2);
    for id in [&oldest, &second, &third] {
        assert_eq!(show(port, OWNER, id)["state"], json!("failed"), "{id}");
    }
    let (_, v) = call(port, "GET", &q("/cli/product-feedback/settings", OWNER), None);
    assert_eq!(v["service"]["state"], json!("paused"), "{v}");
    assert_eq!(drain_now(), Pass::BackingOff, "paused keeps the hourly cadence");

    // Open: the rows that failed because the service couldn't be reached are
    // queued again with a fresh window; the health check itself sends nothing.
    e.intake.set(intake_answers((200, HEALTH_OPEN), intake_created));
    drain::reset_for_tests();
    assert_eq!(drain_now(), Pass::Probed { health: "open", requeued: 3 });
    assert_eq!(submissions(&e.intake).len(), sent_before, "the health check sends no row");
    for id in [&oldest, &second, &third] {
        let it = show(port, OWNER, id);
        assert_eq!((it["state"].clone(), it["attempts"].clone()), (json!("queued"), json!(0)), "{id}: {it}");
    }
    assert_eq!(show(port, OWNER, &off)["state"], json!("failed"), "a person's switch is not overridden");
    assert_eq!(show(port, OWNER, &old)["state"], json!("failed"), "older than 90 days");

    // They go through the normal drain, oldest first.
    assert_eq!(drain_now(), Pass::Ran { sent: 3, refused: 0, retried: 0 });
    assert_eq!(submissions(&e.intake)[sent_before..].to_vec(), vec![oldest.clone(), second.clone(), third.clone()]);
    for id in [&oldest, &second, &third] {
        assert_eq!(show(port, OWNER, id)["state"], json!("sent"), "{id}");
    }
    let checks = health_checks(&e.intake);
    drain::reset_for_tests();
    assert_eq!(drain_now(), Pass::Empty, "nothing left that goes by itself: no health check");
    assert_eq!(health_checks(&e.intake), checks);
    assert_eq!(show(port, OWNER, &off)["attempts"], json!(40));

    // Retry by hand still works for the rest.
    let (s, v) = call(port, "POST", &q("/cli/product-feedback/retry", OWNER), Some(json!({"id": off})));
    assert_eq!((s, v["state"].clone()), (200, json!("queued")), "{v}");
    wait_row(port, OWNER, &off, "sent after retry", |it| it["state"] == json!("sent"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_answer_to_a_send_also_requeues_and_air_gap_or_off_asks_nothing() {
    let e = setup(true);
    let port = e.port;
    empty_outbox();

    e.intake.set(|_| (502, "bad gateway".into()));
    let (_, v) = send(port, OWNER, json!({"text": "stranded"}));
    let stranded = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &stranded, "tried once", |it| it["error"] == json!("service_unreachable"));
    fail_row(&stranded, 12, "service_unreachable: connection refused");

    // Air-gap: no health check at all.
    e.intake.set(intake_answers((200, HEALTH_OPEN), intake_created));
    drain::reset_for_tests();
    {
        let _airgap = EnvVar::set("K2_AIRGAP", "1");
        assert_eq!(drain_now(), Pass::Held("airgap"));
    }
    // Sending turned off: none either.
    let (s, _) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"enabled": false})));
    assert_eq!(s, 200);
    assert_eq!(drain_now(), Pass::Held("disabled"));
    assert_eq!(health_checks(&e.intake), 0, "nothing asked while held");
    assert_eq!(show(port, OWNER, &stranded)["state"], json!("failed"));
    // Turning sending back on kicks the drain; with nothing queued it asks
    // /health, which doesn't answer here, so the next check is an hour off.
    e.intake.set(intake_answers((502, "bad gateway"), intake_created));
    let (s, _) = call(port, "POST", &q("/cli/product-feedback/settings", OWNER), Some(json!({"enabled": true})));
    assert_eq!(s, 200);
    let deadline = Instant::now() + Duration::from_secs(20);
    while health_checks(&e.intake) == 0 {
        assert!(Instant::now() < deadline, "the kick never asked /health");
        std::thread::sleep(Duration::from_millis(50));
    }
    // drain_now waits out the kick still in flight, then finds the hour not up.
    assert_eq!(drain_now(), Pass::BackingOff, "the kick's health check went first");
    let checks = health_checks(&e.intake);
    assert_eq!(checks, 1, "the kick asked once");
    assert_eq!(show(port, OWNER, &stranded)["state"], json!("failed"), "no answer: still failed");

    // A new send the intake takes: the stranded row is queued again too,
    // without another health check.
    let (s, v) = send(port, OWNER, json!({"text": "fresh"}));
    assert_eq!(s, 200, "{v}");
    let fresh = v["id"].as_str().unwrap().to_string();
    wait_row(port, OWNER, &fresh, "sent", |it| it["state"] == json!("sent"));
    let it = wait_row(port, OWNER, &stranded, "queued again", |it| it["state"] != json!("failed"));
    assert_eq!((it["state"].clone(), it["attempts"].clone()), (json!("queued"), json!(0)), "a fresh window: {it}");
    assert_eq!(health_checks(&e.intake), checks);
    assert_eq!(drain_now(), Pass::Ran { sent: 1, refused: 0, retried: 0 });
    assert_eq!(show(port, OWNER, &stranded)["state"], json!("sent"));
}

// ── 0.46.18: a fresh box token for every POST ────────────────────────

/// A relay that mints a different token each time (as the real one does:
/// each has its own one-use `jti`).
fn relay_minting() -> impl Fn(&Req) -> (u16, String) + Send + 'static {
    let n = std::sync::atomic::AtomicUsize::new(0);
    move |_| {
        let i = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        (200, json!({ "token": format!("{JWT}{i}"), "expiresAt": 4_102_444_800i64 }).to_string())
    }
}

/// What the strict intake keeps.
#[derive(Default)]
struct Strict {
    /// Every box token it has seen (its `jti`, spent on first sight).
    spent: std::collections::HashSet<String>,
    /// One row per clientId, as `UNIQUE (box_label, client_id)`.
    rows: BTreeMap<String, String>,
    /// Resends of a stored row it answered `200 {duplicate: true}`.
    duplicates: usize,
    /// Store the next submission but answer it `401 replayed` (its answer
    /// was lost: the token was spent and the row stored).
    lose_next_answer: bool,
}

/// The intake as K2 Web runs it: a token seen before is `401 replayed`
/// whatever the body; a clientId it already stored is `200 {id, duplicate:
/// true}`; a new one is `201 {id}`.
fn intake_one_use(st: Arc<Mutex<Strict>>) -> impl Fn(&Req) -> (u16, String) + Send + 'static {
    move |r| {
        let mut s = st.lock().unwrap();
        let tok = r.header("Authorization").unwrap_or("").to_string();
        if !s.spent.insert(tok) {
            return (401, json!({ "code": "replayed" }).to_string());
        }
        let key = r.header("Idempotency-Key").unwrap_or("none").to_string();
        if let Some(id) = s.rows.get(&key).cloned() {
            s.duplicates += 1;
            return (200, json!({ "id": id, "duplicate": true }).to_string());
        }
        let id = format!("ticket-{}", s.rows.len() + 1);
        s.rows.insert(key, id.clone());
        if std::mem::take(&mut s.lose_next_answer) {
            return (401, json!({ "code": "replayed" }).to_string());
        }
        (201, json!({ "id": id }).to_string())
    }
}

/// The submissions the intake saw for `id`, in order.
fn posts_for(intake: &Fake, id: &str) -> Vec<Req> {
    intake
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path == "/v1/submissions" && r.header("Idempotency-Key") == Some(id))
        .collect()
}

/// Send `text` while the relay has no route, and wait until its attempt is
/// over (queued with `relay_not_ready`), so no kick is left to race.
fn send_and_hold(port: u16, text: &str) -> String {
    let (s, v) = send(port, OWNER, json!({ "text": text }));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let it = show(port, OWNER, &id);
        if it["error"] == json!("relay_not_ready") {
            return id;
        }
        assert!(Instant::now() < deadline, "{id}: never tried: {it}");
        due_now(&id);
        let _ = drain_now();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_against_one_use_box_tokens_all_lands() {
    let e = setup(true);
    let port = e.port;
    empty_outbox();
    e.relay.set(|_| (404, "not found".into()));
    let ids: Vec<String> = ["one", "two", "three"].iter().map(|t| send_and_hold(port, t)).collect();

    let st = Arc::new(Mutex::new(Strict::default()));
    e.relay.set(relay_minting());
    e.intake.set(intake_one_use(st.clone()));
    let n_relay = e.relay.requests().len();
    for id in &ids {
        due_now(id);
    }
    drain::reset_for_tests();
    assert_eq!(drain_now(), Pass::Ran { sent: 3, refused: 0, retried: 0 }, "the whole batch in one pass");
    let s = st.lock().unwrap();
    for id in &ids {
        let it = show(port, OWNER, id);
        assert_eq!((it["state"].clone(), it["remoteId"].clone()), (json!("sent"), json!(s.rows[id])), "{it}");
        assert_eq!(posts_for(&e.intake, id).len(), 1, "{id}: one POST, no 401 to recover from");
    }
    let auths: std::collections::HashSet<String> =
        ids.iter().map(|id| posts_for(&e.intake, id)[0].header("Authorization").unwrap().to_string()).collect();
    assert_eq!(auths.len(), 3, "a fresh token for every POST");
    assert_eq!(e.relay.requests().len(), n_relay + 3, "one mint per POST");
    assert_eq!(s.duplicates, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_answer_is_sent_again_once_with_a_fresh_token_and_the_duplicate_is_delivered() {
    let e = setup(true);
    let port = e.port;
    empty_outbox();
    let st = Arc::new(Mutex::new(Strict { lose_next_answer: true, ..Strict::default() }));
    e.relay.set(relay_minting());
    e.intake.set(intake_one_use(st.clone()));
    let n_relay = e.relay.requests().len();

    // The intake stores it but answers 401 replayed; the resend with a fresh
    // token is 200 duplicate: delivered, with the stored row's id.
    let (s, v) = send(port, OWNER, json!({ "text": "its answer was lost" }));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let it = wait_row(port, OWNER, &id, "sent", |it| it["state"] == json!("sent"));
    let stored = st.lock().unwrap().rows[&id].clone();
    assert_eq!((it["remoteId"].clone(), it["error"].clone()), (json!(stored), J::Null), "{it}");
    let posts = posts_for(&e.intake, &id);
    assert_eq!(posts.len(), 2, "the first and one resend");
    assert_ne!(posts[0].header("Authorization"), posts[1].header("Authorization"), "the resend has a fresh token");
    assert_eq!(posts[0].body, posts[1].body, "the same submission");
    assert_eq!(e.relay.requests().len(), n_relay + 2);
    {
        let s = st.lock().unwrap();
        assert_eq!((s.duplicates, s.rows.len()), (1, 1), "one row at the intake, the resend its duplicate");
    }

    // Any other 401 is not sent again at once: queued with the reason.
    e.intake.set(|_| (401, json!({ "code": "bad_assertion" }).to_string()));
    let n_relay = e.relay.requests().len();
    let (_, v) = send(port, OWNER, json!({ "text": "a bad token" }));
    let bad = v["id"].as_str().unwrap().to_string();
    let it = wait_row(port, OWNER, &bad, "refused the token", |it| it["error"] == json!("token_rejected"));
    assert_eq!(it["state"], json!("queued"), "{it}");
    assert_eq!(posts_for(&e.intake, &bad).len(), 1, "no resend for bad_assertion");
    assert_eq!(e.relay.requests().len(), n_relay + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_failed_on_a_replayed_token_go_back_once_at_boot() {
    let e = setup(true);
    let port = e.port;
    empty_outbox();
    e.relay.set(|_| (404, "not found".into()));
    let replayed = send_and_hold(port, "a reused token");
    let other_401 = send_and_hold(port, "a bad token");
    let off = send_and_hold(port, "sending was off");
    fail_row(&replayed, 3, k2_core::product_feedback::store::REPLAYED_ERROR);
    fail_row(&other_401, 3, "token_rejected: bad_assertion");
    fail_row(&off, 3, "disabled: Sending feedback to K2 is turned off on this server");
    forget_requeue_marker();

    drain::recover_at_boot();
    let it = show(port, OWNER, &replayed);
    assert_eq!((it["state"].clone(), it["attempts"].clone()), (json!("queued"), json!(0)), "queued again, a fresh window: {it}");
    for id in [&other_401, &off] {
        assert_eq!(show(port, OWNER, id)["state"], json!("failed"), "{id} stays failed");
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::db::has_code_migration_applied(&conn, drain::REPLAYED_REQUEUE_ID));
    }
    // Once: a row that fails that way again stays failed.
    fail_row(&replayed, 3, k2_core::product_feedback::store::REPLAYED_ERROR);
    drain::recover_at_boot();
    assert!(drain::requeue_replayed_once().is_empty());
    assert_eq!(show(port, OWNER, &replayed)["state"], json!("failed"), "only once per server");

    // And a requeued one sends with a fresh token.
    forget_requeue_marker();
    assert_eq!(drain::requeue_replayed_once(), vec![replayed.clone()]);
    e.relay.set(relay_minting());
    e.intake.set(intake_one_use(Arc::new(Mutex::new(Strict::default()))));
    drain::reset_for_tests();
    assert_eq!(drain_now(), Pass::Ran { sent: 1, refused: 0, retried: 0 });
    assert_eq!(show(port, OWNER, &replayed)["state"], json!("sent"));
}

fn forget_requeue_marker() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM code_migrations WHERE id = ?1", [drain::REPLAYED_REQUEUE_ID]).unwrap();
}
