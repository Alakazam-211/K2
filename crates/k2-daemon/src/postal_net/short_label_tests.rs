//! A server label of two characters (`z3`, a live K2 Connect label) goes
//! all the way through: K2's label rule takes it, and so does the Postal
//! SDK's, so the handle registers its key on the plane and a friend
//! request to another short label goes out as a pairing. Against a stub
//! plane; nothing leaves the test.

use std::sync::{Arc, Mutex};

use k2_core::postal::sdk::plane::{Method, Request};
use k2_core::postal::store;
use serde_json::{json, Value};

use crate::postal_net::pairing;
use crate::postal_net::plane::{Answer, Plane, Transport};

const ME: &str = "scout::z3.postal.bot";
const PEER: &str = "ops::q7.postal.bot";

/// The plane's session, `/me` and `/pairs`, each call recorded.
#[derive(Default)]
struct Plane2 {
    calls: Mutex<Vec<(String, String, String)>>,
}

impl Transport for Arc<Plane2> {
    fn call(&self, _bearer: &str, req: &Request) -> Result<Answer, String> {
        let method = match req.method {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
        };
        let path = req.path.strip_prefix("/api/postal/v1").expect("a v1 path").to_string();
        self.calls.lock().unwrap().push((method.into(), path.clone(), req.body.clone().unwrap_or_default()));
        let out = match (method, path.as_str()) {
            ("POST", "/session") => json!({ "token": "pst_0123456789abcdef0123456789abcdef", "label": "z3", "expires_at": "x" }),
            ("PUT", "/me") => json!({ "ok": true }),
            ("POST", "/pairs") => json!({ "id": "pr_short", "status": "pending_out" }),
            _ => return Ok((404, br#"{"error":{"code":"not_found","message":""}}"#.to_vec())),
        };
        Ok((200, out.to_string().into_bytes()))
    }
    fn post_open(&self, _path: &str, _body: &Value) -> Result<Answer, String> {
        Err("not in this test".into())
    }
}

#[test]
fn a_two_character_label_registers_and_pairs_through_the_sdk() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    k2_core::postal::keys::install(Arc::new(k2_core::postal::keys::MemorySecrets::default()));
    // Both rules take it: K2's and the SDK's.
    let me = k2_core::postal::parse_address(ME).expect("K2's label rule takes z3");
    assert_eq!(me.label, "z3");
    k2_core::postal::sdk::address::Address::parse(ME).expect("the SDK's label rule takes z3");
    k2_core::postal::sdk::address::Address::parse(PEER).expect("the SDK's label rule takes q7");

    let ws_id = uuid::Uuid::new_v4().to_string();
    let handle_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'scout', '/tmp/k2-postal-z3', 'scout')", rusqlite::params![ws_id])
            .expect("workspace");
        store::claim(&conn, &ws_id, &me, "owner").expect("claim").id
    };
    let stub = Arc::new(Plane2::default());
    let net = Plane::new(Box::new(stub.clone()), Box::new(|| Ok(("k2c_connect_test".to_string(), "z3".to_string()))));

    // Register: the key goes to the plane and the handle is on the network.
    pairing::register_handles(&net).expect("z3 registers");
    let h = k2_core::db::read(|c| store::handle(c, &handle_id)).expect("handle");
    assert_eq!(h.status, "claimed", "{h:?}");
    let calls = stub.calls.lock().unwrap().clone();
    let me_put = calls.iter().find(|c| c.0 == "PUT" && c.1 == "/me").expect("PUT /me");
    assert!(me_put.2.contains(ME), "the address the plane registers: {}", me_put.2);

    // Pair: a friend request to another short label goes out (this server
    // holds its approval token, so it can accept the answer).
    k2_core::postal::keys::set_decide_token("z3", "k2c_pd_0123456789abcdef0123456789abcdef").expect("the approval token");
    let friend_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::record_outgoing(&conn, &h, &k2_core::postal::parse_address(PEER).expect("peer"), Some("hello"), "owner").expect("request").id
    };
    pairing::send_unsent(&net).expect("the request goes out");
    let calls = stub.calls.lock().unwrap().clone();
    let pair = calls.iter().find(|c| c.0 == "POST" && c.1 == "/pairs").expect("POST /pairs: the SDK built the pairing request");
    assert!(pair.2.contains(ME) && pair.2.contains(PEER), "from and to: {}", pair.2);
    let f = k2_core::db::read(|c| store::friend(c, &friend_id)).expect("friend");
    assert!(f.pair_id.is_some(), "the pairing is recorded: {f:?}");
}
