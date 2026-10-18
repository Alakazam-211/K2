//! Same-server secret handoff routes (prd-agent-secret-handoff-v1 §14,
//! daemon tests 8–10 and the same-server smoke list). Temp HOME, the test
//! vault (fake key store) and the test DB; never the real home, keychain
//! or daemon.

use std::collections::HashMap;
use std::sync::Arc;

use k2_vault::{Actor, AgentActor, Kind, LocalVault, Namespace, PutRequest, Requester, Role, Scope, SecretBytes, SecretStore, Selector, Use};
use serde_json::{json, Value};

use super::*;
use crate::test_support::with_temp_home;
use crate::vault::actor::{Channel, VaultCaller};

fn sentinel() -> String {
    format!("k2test_HANDOFF_SENTINEL_{}", uuid::Uuid::new_v4().simple())
}

struct Ws {
    id: String,
    name: String,
    _path: String,
}

fn workspace(tag: &str) -> Ws {
    let name = format!("{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let path = std::env::temp_dir().join(format!("k2-handoff-{name}")).to_string_lossy().into_owned();
    std::fs::create_dir_all(&path).expect("mkdir");
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, name, path]).expect("project");
    // A pinned Chat, so the workspace has a Thread for the handoff card.
    let conv = uuid::Uuid::new_v4().to_string();
    k2_core::db::schema::WorkspaceSession::upsert(&conn, &uuid::Uuid::new_v4().to_string(), &id, None, Some(&conv), "claude", "owner", "idle")
        .expect("pin chat");
    Ws { id, name, _path: path }
}

fn link(a: &Ws, b: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceRelation::create(&conn, &uuid::Uuid::new_v4().to_string(), &a.id, &b.id, "collaborator").expect("link");
}

fn agent(ws: &Ws) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.id.clone(), session_id: Some("sess-1".into()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("sess-1".into()),
    }
}

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
}

fn owner_tty() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Tty, session_id: None }
}

fn put(v: &LocalVault, ws: &Ws, name: &str, value: &str, by: &Actor) {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(&ws.id), name, Kind::ApiKey);
    r.hosts = vec!["api.stripe.com".into()];
    v.put(r, SecretBytes::from_slice(value.as_bytes()), by).expect("put");
}

fn call(v: &Arc<LocalVault>, path: &str, c: &VaultCaller, body: Value) -> (u16, Value, String) {
    call_p(v, path, c, body, HashMap::new())
}

fn call_p(v: &Arc<LocalVault>, path: &str, c: &VaultCaller, body: Value, params: HashMap<String, String>) -> (u16, Value, String) {
    let b = if body.is_null() { vec![] } else { body.to_string().into_bytes() };
    let r = handle(v, path, c, &b, &params, "test");
    let code: u16 = r.status[..3].parse().expect("status");
    let j: Value = serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json from {path}: {}", r.body));
    (code, j, r.body)
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn value_of(v: &LocalVault, ws: &Ws, name: &str) -> Vec<u8> {
    v.resolve(&Selector::user(Scope::workspace(&ws.id), name), Use::System, &Requester::System("t".into())).expect("resolve").value.expose().to_vec()
}

fn ws_param(ws: &Ws) -> HashMap<String, String> {
    HashMap::from([("workspace".to_string(), ws.id.clone())])
}

/// Everything K2 wrote about handoffs where a person or agent could read
/// it: the auth audit (Settings → Logs), the vault audit chain, the
/// handoff rows, Thread cards, chatter and receipts.
fn surfaces(v: &LocalVault, ws: &[&Ws]) -> String {
    let mut out = String::new();
    for e in k2_core::auth_audit::tail(1000).expect("audit tail") {
        out.push_str(&e.to_string());
    }
    for r in v.audit(&k2_vault::AuditQuery { limit: Some(5000), ..Default::default() }, &Actor::Owner).expect("vault audit") {
        out.push_str(&format!("{r:?}"));
    }
    for w in ws {
        for r in v.handoffs(Some(&w.id), None).expect("rows") {
            out.push_str(&serde_json::to_string(&r).expect("row json"));
            if let Some(card) = r.card.as_deref() {
                let id = card.rsplit_once('/').map(|x| x.1).unwrap_or("");
                if let Ok(Some(doc)) = k2_core::overlay::get_doc(id) {
                    out.push_str(&serde_json::to_string(&doc).expect("doc json"));
                }
            }
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(mut st) = conn.prepare("SELECT * FROM msg_deliveries") {
        let n = st.column_count();
        let rows = st
            .query_map([], |r| {
                let mut s = String::new();
                for i in 0..n {
                    if let Ok(t) = r.get::<_, String>(i) {
                        s.push_str(&t);
                    }
                }
                Ok(s)
            })
            .expect("deliveries");
        for r in rows.flatten() {
            out.push_str(&r);
        }
    }
    out
}

#[test]
fn same_server_send_pends_then_lands_with_accept_from_and_rotates_on_resend() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (sales, ops) = (workspace("sales"), workspace("ops"));
        link(&sales, &ops);
        let secret = sentinel();
        put(&v, &sales, "STRIPE_KEY", &secret, &Actor::Agent(AgentActor { workspace_id: sales.id.clone(), session_id: None, shell: false, vault_manage: false }));

        // Not on ops's accept-from list: pending (exit 3 in the CLI).
        let (st, j, raw) = call(&v, SEND, &agent(&sales), json!({ "name": "STRIPE_KEY", "to": ops.name, "note": "for the refund job" }));
        assert_eq!(st, 200, "{raw}");
        assert_eq!(j["status"], "pending");
        assert_eq!(j["reason"], "not_allowed");
        assert!(j["who"].as_str().expect("who").contains("a person on this server"), "{raw}");
        assert!(!raw.contains(&secret));
        let id = j["deliveryId"].as_str().expect("id").to_string();
        assert!(v.meta(&Selector::user(Scope::workspace(&ops.id), "STRIPE_KEY"), &Actor::Owner).is_err(), "nothing landed");

        // ops sees it (names only); sales sees what it sent.
        let (_, inc, raw) = call_p(&v, INCOMING, &agent(&ops), Value::Null, HashMap::new());
        assert!(!raw.contains(&secret));
        let items = inc["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "{raw}");
        assert_eq!(items[0]["status"], "pending");
        assert_eq!(items[0]["peer"], sales.name.to_lowercase());
        let (_, sent, _) = call_p(&v, SENT, &agent(&sales), Value::Null, HashMap::new());
        assert_eq!(sent["items"][0]["deliveryId"], id.as_str());
        // An agent can't read another workspace's lists.
        let (st, j, _) = call_p(&v, INCOMING, &agent(&sales), Value::Null, ws_param(&ops));
        assert_eq!((st, code(&j)), (403, "vault_not_yours"));

        // Agents never decide or edit the allow-list (KV8 parity, H19).
        let (st, j, _) = call(&v, DECIDE, &agent(&ops), json!({ "id": id, "action": "accept" }));
        assert_eq!((st, code(&j)), (403, "handoff_person_only"));
        let (st, j, _) = call(&v, ACCEPT_FROM, &agent(&ops), json!({ "workspace": ops.id, "sender": sales.name, "action": "add" }));
        assert_eq!((st, code(&j)), (403, "handoff_person_only"));

        // The receiving agent's Thread has a person-only handoff card (H18).
        let card_ref = v.handoff_get(&id, "in").expect("row").card.expect("a pending copy posts a Thread card");
        let card_id = card_ref.rsplit_once('/').expect("conv/card").1.to_string();
        let doc = k2_core::overlay::get_doc(&card_id).expect("doc").expect("card doc");
        assert_eq!(doc.kind, "handoff");
        let h = doc.handoff.as_ref().expect("handoff body");
        assert_eq!((h.status.as_str(), h.name.as_str(), h.delivery_id.as_str()), ("pending", "STRIPE_KEY", id.as_str()));
        assert!(doc.body.as_deref().expect("plain body").contains("Settings → Vault → Incoming"), "H18 body for old renderers");

        // A person accepts, ticking "always accept from".
        let (st, j, raw) = call(&v, DECIDE, &owner(), json!({ "id": id, "action": "accept", "always": true }));
        assert_eq!(st, 200, "{raw}");
        let doc = k2_core::overlay::get_doc(&card_id).expect("doc").expect("card doc");
        let h = doc.handoff.as_ref().expect("handoff body");
        assert_eq!(h.status, "landed");
        assert_eq!(h.answered_by.as_ref().expect("who").kind, "owner");
        assert!(h.reference.as_deref().expect("ref").starts_with("k2v://"));
        assert_eq!(j["delivery"]["status"], "landed");
        assert_eq!(j["alwaysAdded"], format!("ws:{}", sales.id));
        assert!(j["ref"].as_str().expect("ref").starts_with("k2v://"));
        assert_eq!(value_of(&v, &ops, "STRIPE_KEY"), secret.as_bytes());
        let (_, sent, _) = call_p(&v, SENT, &agent(&sales), Value::Null, HashMap::new());
        assert_eq!(sent["items"][0]["status"], "landed", "the sender's row follows");

        // Now allow-listed: a new secret lands straight in …
        let other = sentinel();
        put(&v, &sales, "OPENAI_KEY", &other, &Actor::Agent(AgentActor { workspace_id: sales.id.clone(), session_id: None, shell: false, vault_manage: false }));
        let (st, j, raw) = call(&v, SEND, &agent(&sales), json!({ "name": "OPENAI_KEY", "to": ops.name }));
        assert_eq!(st, 200, "{raw}");
        assert_eq!(j["status"], "landed");
        assert_eq!(value_of(&v, &ops, "OPENAI_KEY"), other.as_bytes());
        // … and a re-send of the same secret is a new version, never a clash.
        let newer = sentinel();
        v.rotate(&Selector::user(Scope::workspace(&sales.id), "OPENAI_KEY"), SecretBytes::from_slice(newer.as_bytes()), &Actor::Owner).expect("rotate");
        let (_, j, raw) = call(&v, SEND, &agent(&sales), json!({ "name": "OPENAI_KEY", "to": ops.name }));
        assert_eq!(j["status"], "landed", "{raw}");
        assert_eq!(j["reason"], "new_version");
        let m = v.meta(&Selector::user(Scope::workspace(&ops.id), "OPENAI_KEY"), &Actor::Owner).expect("meta");
        assert_eq!(m.latest_version, 2);
        assert_eq!(value_of(&v, &ops, "OPENAI_KEY"), newer.as_bytes());
        // The landed copy is never api (agents can't `get` it).
        assert_ne!(m.delivery_ceiling, k2_vault::Delivery::Api);
        // Settings → Vault provenance.
        let mj = crate::vault_routes::meta_json(&m);
        assert_eq!(mj["handoff"]["from"], sales.name.to_lowercase());

        // H14 / test 10: the value never reached any K2 surface.
        let all = surfaces(&v, &[&sales, &ops]);
        for s in [&secret, &other, &newer] {
            assert!(!all.contains(s.as_str()), "a value leaked into a K2 surface");
        }
    });
}

#[test]
fn local_gate_and_never_sendable_refusals() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (sales, ops, stranger) = (workspace("sales"), workspace("ops"), workspace("stranger"));
        link(&sales, &ops);
        let me = Actor::Agent(AgentActor { workspace_id: sales.id.clone(), session_id: None, shell: false, vault_manage: false });
        put(&v, &sales, "KEY", &sentinel(), &me);

        // Not connected (the k2 msg gate): exit 3.
        let (st, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "KEY", "to": stranger.name }));
        assert_eq!((st, code(&j)), (403, "not_connected"));
        // Unknown agent / unknown secret: exit 4.
        let (st, _, _) = call(&v, SEND, &agent(&sales), json!({ "name": "KEY", "to": "nobody-here-xyz" }));
        assert_eq!(st, 404);
        let (st, _, _) = call(&v, SEND, &agent(&sales), json!({ "name": "NOPE", "to": ops.name }));
        assert_eq!(st, 404);
        // K2 credential shapes (H20): exit 2.
        put(&v, &sales, "API_KEY_K2", "k2sk_0123456789abcdef", &me);
        let (st, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "API_KEY_K2", "to": ops.name }));
        assert_eq!((st, code(&j)), (422, "never_sendable"));
        let owner_token = format!("owner-token-{}", uuid::Uuid::new_v4().simple());
        let home = dirs::home_dir().expect("home").join(".k2");
        std::fs::create_dir_all(&home).expect("mkdir");
        std::fs::write(home.join("daemon.token"), &owner_token).expect("token");
        put(&v, &sales, "LOOKS_FINE", &owner_token, &me);
        let (st, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "LOOKS_FINE", "to": ops.name }));
        assert_eq!((st, code(&j)), (422, "never_sendable"), "the owner token never travels");
        // The note can't carry the value (H14).
        let s2 = sentinel();
        put(&v, &sales, "NOTED", &s2, &me);
        let (st, j, raw) = call(&v, SEND, &agent(&sales), json!({ "name": "NOTED", "to": ops.name, "note": format!("it is {s2}") }));
        assert_eq!((st, code(&j)), (422, "note_has_value"));
        assert!(!raw.contains(&s2));
        // Server scope isn't sendable in v1: the grant hint (exit 3).
        let mut r = PutRequest::new(Namespace::User, Scope::server(), "SERVER_WIDE", Kind::ApiKey);
        r.hosts = vec!["api.example.com".into()];
        v.put(r, SecretBytes::from_slice(sentinel().as_bytes()), &Actor::Owner).expect("server put");
        let (st, j, _) = call(&v, SEND, &owner(), json!({ "name": "SERVER_WIDE", "scope": "server", "to": ops.name }));
        assert_eq!((st, code(&j)), (403, "send_not_allowed"));
        assert!(j["error"]["hint"].as_str().expect("hint").contains("k2 vault grant"));
        // An agent can only name its own workspace's secrets.
        let (st, _, _) = call(&v, SEND, &agent(&sales), json!({ "name": "SERVER_WIDE", "to": ops.name }));
        assert_eq!(st, 404);
        // --stdin is a person's, cross server only (H15).
        let (st, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "X", "to": "ops::luzz.k2.dev", "valueB64": "c2VjcmV0dmFsdWU=" }));
        assert_eq!((st, code(&j)), (422, "send_by_name_only"));
        let (st, j, _) = call(&v, SEND, &owner(), json!({ "name": "X", "to": "ops::luzz.k2.dev", "valueB64": "c2VjcmV0dmFsdWU=" }));
        assert_eq!((st, code(&j)), (422, "send_by_name_only"), "the owner token over TCP isn't the person channel");
        let (st, j, _) = call(&v, SEND, &owner_tty(), json!({ "name": "X", "to": ops.name, "valueB64": "c2VjcmV0dmFsdWU=" }));
        assert_eq!((st, code(&j)), (422, "use_vault_set"));
        // Every refusal is in the Settings logs data source, with its reason.
        let lines: Vec<String> = k2_core::auth_audit::tail(1000)
            .expect("tail")
            .into_iter()
            .filter(|e| e["event"] == AUDIT_EVENT)
            .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
            .collect();
        for want in ["not_connected", "never_sendable", "note_has_value", "send_not_allowed", "send_by_name_only", "use_vault_set"] {
            assert!(lines.iter().any(|l| l.starts_with("deny send") && l.ends_with(want)), "no deny line for {want}: {lines:?}");
        }
    });
}

#[test]
fn person_given_secrets_pend_even_from_an_allow_listed_sender() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (sales, ops) = (workspace("sales"), workspace("ops"));
        link(&sales, &ops);
        // The owner gave sales this key.
        put(&v, &sales, "PERSON_KEY", &sentinel(), &Actor::Owner);
        let (st, _, _) = call(&v, ACCEPT_FROM, &owner(), json!({ "workspace": ops.id, "sender": sales.name, "action": "add" }));
        assert_eq!(st, 200);
        let (st, j, raw) = call(&v, SEND, &agent(&sales), json!({ "name": "PERSON_KEY", "to": ops.name }));
        assert_eq!(st, 200, "{raw}");
        assert_eq!(j["status"], "pending", "R3: a person-given secret needs a person");
        assert_eq!(j["reason"], "person_given");
    });
}

#[test]
fn decline_clash_and_accept_from_lists() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (sales, ops) = (workspace("sales"), workspace("ops"));
        link(&sales, &ops);
        let me = Actor::Agent(AgentActor { workspace_id: sales.id.clone(), session_id: None, shell: false, vault_manage: false });
        put(&v, &sales, "DUP", &sentinel(), &me);
        put(&v, &ops, "DUP", "ops-own-value-123", &Actor::Agent(AgentActor { workspace_id: ops.id.clone(), session_id: None, shell: false, vault_manage: false }));
        let (_, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "DUP", "to": ops.name }));
        assert_eq!(j["reason"], "clash");
        let id = j["deliveryId"].as_str().expect("id").to_string();
        let (st, j, _) = call(&v, DECIDE, &owner(), json!({ "id": id, "action": "accept" }));
        assert_eq!((st, code(&j)), (409, "name_taken"));
        let admin = VaultCaller { actor: Actor::Login { username: "lee".into(), role: Role::Admin }, channel: Channel::Login, session_id: None };
        let (st, j, _) = call(&v, DECIDE, &admin, json!({ "id": id, "action": "decline" }));
        assert_eq!(st, 200);
        assert_eq!(j["delivery"]["status"], "declined");
        assert_eq!(j["delivery"]["decidedBy"], "connect:lee");
        assert_eq!(value_of(&v, &ops, "DUP"), b"ops-own-value-123", "never overwritten");
        // Lists: agents read their own allow-list; people add and remove.
        let (_, j, _) = call(&v, ACCEPT_FROM, &owner(), json!({ "workspace": ops.name, "sender": "sales::rosson.k2.dev", "action": "add" }));
        assert_eq!(j["entries"][0]["sender"], "sales::rosson.k2.dev");
        let (st, j, _) = call_p(&v, ACCEPT_FROM, &agent(&ops), Value::Null, HashMap::new());
        assert_eq!(st, 200);
        assert_eq!(j["entries"].as_array().expect("entries").len(), 1);
        let (_, j, _) = call(&v, ACCEPT_FROM, &owner(), json!({ "workspace": ops.name, "sender": "sales::rosson.k2.dev", "action": "remove" }));
        assert!(j["entries"].as_array().expect("entries").is_empty());
        // The decisions are in the Settings logs data source.
        let lines: Vec<String> = k2_core::auth_audit::tail(1000)
            .expect("tail")
            .into_iter()
            .filter(|e| e["event"] == AUDIT_EVENT && e["client"] == "vault")
            .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(lines.iter().any(|l| l.starts_with("ok decline DUP") && l.contains(&id)), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("deny accept DUP") && l.ends_with("name_taken")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("ok accept-from add sales::rosson.k2.dev")), "{lines:?}");
    });
}

#[test]
fn expiry_closes_pending_copies_and_logs_it() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (sales, ops) = (workspace("sales"), workspace("ops"));
        link(&sales, &ops);
        put(&v, &sales, "OLD_KEY", &sentinel(), &Actor::Agent(AgentActor { workspace_id: sales.id.clone(), session_id: None, shell: false, vault_manage: false }));
        let (_, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "OLD_KEY", "to": ops.name }));
        let id = j["deliveryId"].as_str().expect("id").to_string();
        v.handoff_backdate_for_tests(&id, "in").expect("backdate");
        tick(&v);
        assert_eq!(v.handoff_get(&id, "in").expect("in").status, "expired");
        assert_eq!(v.handoff_get(&id, "out").expect("out").status, "expired");
        assert!(!v.handoff_has_value(&id));
        let lines: Vec<String> = k2_core::auth_audit::tail(1000)
            .expect("tail")
            .into_iter()
            .filter(|e| e["event"] == AUDIT_EVENT)
            .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(lines.iter().any(|l| l.starts_with("ok expire OLD_KEY") && l.contains(&id)), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("ok send OLD_KEY") && l.contains(&id)), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("ok receive OLD_KEY") && l.contains(&id)), "{lines:?}");
    });
}

#[test]
fn the_stamp_message_is_names_only() {
    let item = k2_vault::handoff::HandoffItem {
        name: "STRIPE_KEY".into(),
        as_name: None,
        kind: "api_key".into(),
        hosts: vec![],
        inject: None,
        description: String::new(),
        delivery: "fetch".into(),
        ceiling: "env".into(),
        human_given: false,
        ttl_secs: None,
    };
    let mut rec = HandoffRecord {
        delivery_id: k2_vault::handoff::new_delivery_id(),
        direction: "in".into(),
        status: "landed".into(),
        local_workspace_id: "w".into(),
        peer_addr: "sales".into(),
        peer_fp: None,
        sender_actor: "agent:x".into(),
        source_vault_id: None,
        name: "STRIPE_KEY".into(),
        landed_name: Some("STRIPE_KEY".into()),
        kind: "api_key".into(),
        note: "for the refund job".into(),
        reply_to: None,
        item,
        secret_id: None,
        version: None,
        reason: None,
        card: None,
        created_at: String::new(),
        decided_at: None,
        decided_by: None,
        expires_at: None,
    };
    let t = stamp_text(&rec, "sales", Some("k2v://ws/ops/STRIPE_KEY"));
    assert_eq!(
        t,
        "[k2 vault] sales sent STRIPE_KEY → your vault (k2v://ws/ops/STRIPE_KEY). Use: k2 vault fetch --secret STRIPE_KEY <url>. Note: \"for the refund job\""
    );
    rec.status = "pending".into();
    assert!(stamp_text(&rec, "sales", None).contains("It waits for your owner to accept (k2 vault incoming)"));
}

#[test]
fn route_walk_policy_post_guard_and_passport_rows() {
    // Test 8: every new route has a policy row, POST routes 405 on GET
    // (the vault arm's require_post), and a passport classification.
    for p in ROUTES {
        assert!(crate::vault_routes::is_route(p), "{p} is served by the vault arm");
        assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has a route_policy row");
    }
    assert!(crate::vault_routes::is_post_route(SEND));
    assert!(crate::vault_routes::is_post_route(DECIDE));
    assert!(!crate::vault_routes::is_post_route(ACCEPT_FROM), "accept-from serves GET and POST");
    for p in [SEND, INCOMING, SENT, ACCEPT_FROM] {
        assert!(crate::session_token::is_agent_verb(p), "{p}: agents reach it with a passport");
    }
    assert!(!crate::session_token::is_agent_verb(DECIDE), "a passport never reaches decide");
}
