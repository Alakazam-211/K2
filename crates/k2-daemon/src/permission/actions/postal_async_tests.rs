//! A Postal card's Allow and Deny answered on a tokio worker, the way a
//! route inside the runtime would run them. The plane is a stub on
//! 127.0.0.1 reached with the daemon's own client (`postal_net::plane::Https`,
//! reqwest's blocking one), which must not run inside the runtime: a debug
//! build panics there with "Cannot drop a runtime in a context where
//! blocking is not allowed". `postal_net::off_async` makes the client and
//! sends each call on a plain thread. Covered: the claim card's Allow
//! registers the key, the invite card's Allow sends the request, and a
//! request's Deny and the code form's accept each make the plane client on
//! first use.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use k2_core::connect_users::Role;
use k2_core::postal::store;
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, card_of, login, seed, Ws};
use crate::compute_routes::Who;
use crate::postal_net::plane::{Https, Plane};

const LABEL: &str = "initech";

/// The plane over HTTP on 127.0.0.1 (`/session`, `/me`, `/pairs`); each
/// request is kept as "METHOD /path".
struct StubPlane {
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl StubPlane {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub plane");
        let base = format!("http://{}", listener.local_addr().expect("the stub's address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let mut conn = conn.expect("a connection to the stub plane");
                let mut reader = BufReader::new(conn.try_clone().expect("clone the connection"));
                let mut line = String::new();
                reader.read_line(&mut line).expect("the request line");
                let mut words = line.split_whitespace();
                let method = words.next().expect("a method").to_string();
                let path = words.next().expect("a path").to_string();
                let mut len = 0usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).expect("a header line");
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = header.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().expect("a Content-Length");
                        }
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).expect("the request body");
                let path = path.strip_prefix("/api/postal/v1").expect("a v1 path").to_string();
                log.lock().unwrap().push(format!("{method} {path}"));
                let (status, answer) = match (method.as_str(), path.as_str()) {
                    ("POST", "/session") => ("200 OK", json!({ "token": "pst_0123456789abcdef0123456789abcdef", "label": LABEL, "expires_at": "x" })),
                    ("PUT", "/me") => ("200 OK", json!({ "ok": true })),
                    ("POST", "/pairs") => ("200 OK", json!({ "id": "pr_async", "status": "pending_out" })),
                    _ => ("404 Not Found", json!({ "error": { "code": "not_found", "message": "" } })),
                };
                let answer = answer.to_string();
                write!(
                    conn,
                    "HTTP/1.1 {status}\r\nPostal-Api: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                )
                .expect("the stub's answer");
            }
        });
        Self { base, seen }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// The network marked connected for the test, and not after.
struct Connected;

impl Connected {
    fn on() -> Self {
        k2_core::postal::gate::set_network_available(true);
        Connected
    }
}

impl Drop for Connected {
    fn drop(&mut self) {
        k2_core::postal::gate::set_network_available(false);
    }
}

/// Run `f` on a tokio worker with the test's DB; a panic there fails the test.
fn on_a_worker<T: Send + 'static>(rt: &tokio::runtime::Runtime, f: impl FnOnce() -> T + Send + 'static) -> T {
    let db = k2_core::db::scoped_handle_for_test().expect("the test's own DB");
    rt.block_on(async move {
        tokio::spawn(async move {
            assert!(tokio::runtime::Handle::try_current().is_ok(), "inside the runtime");
            k2_core::db::with_scoped_handle(db, f)
        })
        .await
    })
    .unwrap_or_else(|e| panic!("the answer on a tokio worker: {e}"))
}

fn answer_on_a_worker(rt: &tokio::runtime::Runtime, author: &str, ws: &Arc<Ws>, card: &str, answer: &str) -> (String, Value) {
    let (author, ws, card, body) = (author.to_string(), ws.clone(), card.to_string(), json!({ "answer": answer }));
    on_a_worker(rt, move || answer_as(&author, &ws, &card, body))
}

fn agent_post(path: &str, ws: &Ws, body: Value) -> Value {
    let who = Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) };
    let r = crate::postal_routes::handle(path, who, true, body.to_string().as_bytes(), &std::collections::HashMap::new());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    serde_json::from_str(&r.body).expect("json")
}

fn friend(ws: &Ws, peer: &str) -> store::Friend {
    k2_core::db::read(|c| {
        let h = store::handle_for_workspace(c, &ws.id).expect("the workspace's address");
        store::friend_by_peer(c, &h.id, peer)
    })
    .unwrap_or_else(|| panic!("no friend row for {peer}"))
}

fn incoming(ws: &Ws, peer: &str, pair_id: &str, code: Option<&str>) -> (String, String) {
    let peer = k2_core::postal::parse_address(peer).expect("peer");
    let got = crate::postal_routes::receive_request(&ws.id, &peer, None, None, Some(pair_id), None, code).expect("recorded");
    let card = got["card"]["card"].as_str().unwrap_or_else(|| panic!("K2 put a card up: {got}")).to_string();
    let id = got["friend"]["id"].as_str().unwrap_or_else(|| panic!("the request's id: {got}")).to_string();
    (card, id)
}

#[test]
fn a_cards_allow_and_deny_reach_the_plane_from_a_tokio_worker() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    k2_core::postal::keys::install(Arc::new(k2_core::postal::keys::MemorySecrets::default()));
    let _on = crate::postal_routes::switch_for_test(true);
    let _connected = Connected::on();
    let plane = StubPlane::start();
    let https = Https::new(plane.base.clone()).expect("the plane client");
    crate::postal_net::install(Arc::new(Plane::new(Box::new(https), Box::new(|| Ok(("k2c_connect_test".to_string(), LABEL.to_string()))))));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a tokio runtime");
    let ws = Arc::new(seed("pasy"));
    let admin = login("pasa", Role::Admin);

    // The claim card's Allow registers the address's key on the plane.
    let addr = format!("{}::{LABEL}.postal.bot", ws.handle);
    let card = card_of(&agent_post(crate::postal_routes::CLAIM, &ws, json!({ "address": addr })));
    let (s, a) = answer_on_a_worker(&rt, &admin, &ws, &card, "Allow");
    assert_eq!((s.as_str(), a["action"]["result"]["outcome"].as_str()), ("200 OK", Some("approved")), "{a}");
    let h = k2_core::db::read(|c| store::handle_for_workspace(c, &ws.id)).expect("claimed");
    assert_eq!((h.address.as_str(), h.status.as_str()), (addr.as_str(), "claimed"), "the key is on the plane: {h:?}");
    assert_eq!(plane.seen(), ["POST /session", "PUT /me"]);

    // The invite card's Allow sends the request.
    let peer = "ops::globex.postal.bot";
    let card = card_of(&agent_post(crate::postal_routes::INVITE, &ws, json!({ "to": peer, "why": "to review each other's PRs" })));
    let (s, a) = answer_on_a_worker(&rt, &admin, &ws, &card, "Allow");
    assert_eq!((s.as_str(), a["action"]["result"]["outcome"].as_str()), ("200 OK", Some("approved")), "{a}");
    assert_eq!(friend(&ws, peer).pair_id.as_deref(), Some("pr_async"), "sent as a pairing");
    assert_eq!(plane.seen().last().map(String::as_str), Some("POST /pairs"), "{:?}", plane.seen());

    // No plane client yet: a request's Deny makes the daemon's on first use.
    let _url = k2_core::test_env::EnvVar::set("K2_POSTAL_PLANE_URL", &plane.base);
    let denied = "lab::gamma.postal.bot";
    let (card, _) = incoming(&ws, denied, "pr_deny", None);
    crate::postal_net::forget_for_test();
    let (s, a) = answer_on_a_worker(&rt, "owner", &ws, &card, "Deny");
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(friend(&ws, denied).status, "rejected");
    assert!(crate::postal_net::made_for_test(), "the Deny made the plane client");

    // The same for the code form's accept: the client is made, then this
    // server (no K2 Connect sign-in here) can't get its approval token.
    let coded = "lab2::gamma.postal.bot";
    let (_, id) = incoming(&ws, coded, "pr_code", Some("482913"));
    crate::postal_net::forget_for_test();
    let body = json!({ "workspace": ws.id, "id": id, "decision": "accept", "code": "123456" }).to_string();
    let r = on_a_worker(&rt, move || crate::postal_routes::handle(crate::postal_routes::DECIDE, Who::Owner, true, body.as_bytes(), &std::collections::HashMap::new()));
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!((r.status, v["error"]["code"].as_str()), ("409 Conflict", Some("postal_no_connect")), "{v}");
    assert!(crate::postal_net::made_for_test(), "the accept made the plane client");
    assert_eq!(friend(&ws, coded).status, "pending_in", "nothing pretended");
}
