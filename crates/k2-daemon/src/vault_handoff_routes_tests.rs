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
        let home = k2_core::paths::home_dir().expect("home").join(".k2");
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
    let t = stamp_text(&rec, "sales", Some("k2v://ws/ops/STRIPE_KEY"), Some(k2_vault::Delivery::Fetch));
    assert_eq!(
        t,
        "[k2 vault] sales sent STRIPE_KEY → your vault (k2v://ws/ops/STRIPE_KEY, delivered to you as fetch). Use: k2 vault fetch --secret STRIPE_KEY <url> (K2 adds it to the request). Note: \"for the refund job\""
    );
    // 2026-10-28: the "Use:" line follows the copy's real delivery (a file
    // copy was told to use k2 vault fetch), and names its --ttl end.
    rec.expires_at = Some("2026-10-28T13:00:00Z".into());
    let t = stamp_text(&rec, "sales", None, None);
    assert!(t.contains("delivered to you as file, until 2026-10-28T13:00:00Z). Use: k2 vault file --secret STRIPE_KEY"), "{t}");
    assert!(!t.contains("k2 vault fetch"), "{t}");
    rec.status = "pending".into();
    assert!(stamp_text(&rec, "sales", None, None).contains("It waits for your owner to accept (k2 vault incoming)"));
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
    for p in ["/cli/federation/vault-recipient", "/cli/federation/vault-handoff"] {
        assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has a route_policy row");
    }
}

// ── H5: ask, reply, unsend ──────────────────────────────────────────

#[test]
fn a_reply_to_your_own_ask_lands_straight_in_and_unsend_takes_it_back() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (sales, ops, other) = (workspace("sales"), workspace("ops"), workspace("other"));
        link(&sales, &ops);
        let me = Actor::Agent(AgentActor { workspace_id: sales.id.clone(), session_id: None, shell: false, vault_manage: false });
        let secret = sentinel();
        put(&v, &sales, "STRIPE_KEY", &secret, &me);
        put(&v, &sales, "OTHER_KEY", &sentinel(), &me);

        // ops asks sales (no accept-from list at all).
        let (st, j, raw) = call(&v, ASK, &agent(&ops), json!({ "to": sales.name, "name": "STRIPE_KEY", "note": "for refunds" }));
        assert_eq!(st, 200, "{raw}");
        let ask_id = j["askId"].as_str().expect("ask id").to_string();
        assert!(k2_vault::handoff::valid_ask_id(&ask_id));
        let (_, asks_out, _) = call_p(&v, ASKS, &agent(&ops), Value::Null, HashMap::new());
        assert_eq!(asks_out["items"][0]["status"], "open");
        let (_, asks_in, _) = call_p(&v, ASKS, &agent(&sales), Value::Null, HashMap::new());
        assert_eq!(asks_in["items"][0]["direction"], "in", "{asks_in}");
        let text = ask_text(&ask_id, &sales.name, "STRIPE_KEY", "for refunds", &ops.name);
        assert!(text.contains(&format!("k2 vault send STRIPE_KEY --to {} --reply {ask_id}", ops.name)), "{text}");

        // A reply for another name doesn't ride the ask: pending.
        let (_, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "OTHER_KEY", "to": ops.name, "replyTo": ask_id }));
        assert_eq!(j["status"], "pending");
        // The reply for the asked name lands straight in (R1) and closes the ask.
        let (st, j, raw) = call(&v, SEND, &agent(&sales), json!({ "name": "STRIPE_KEY", "to": ops.name, "replyTo": ask_id }));
        assert_eq!(st, 200, "{raw}");
        assert_eq!(j["status"], "landed", "{raw}");
        assert_eq!(value_of(&v, &ops, "STRIPE_KEY"), secret.as_bytes());
        assert_eq!(v.handoff_get(&ask_id, "ask_out").expect("ask").status, "answered");
        let id = j["deliveryId"].as_str().expect("id").to_string();
        // A second reply to the same ask doesn't land.
        let (_, j, _) = call(&v, SEND, &agent(&sales), json!({ "name": "STRIPE_KEY", "to": ops.name, "replyTo": ask_id }));
        assert_ne!(j["status"], "landed", "an ask is answered once");

        // Only the sender takes a delivery back.
        let (st, j, _) = call(&v, UNSEND, &agent(&other), json!({ "id": id }));
        assert_eq!((st, code(&j)), (403, "not_yours"));
        let (st, j, raw) = call(&v, UNSEND, &agent(&sales), json!({ "id": id }));
        assert_eq!(st, 200, "{raw}");
        assert_eq!(j["disabled"], true);
        assert!(j["honest"].as_str().expect("honest").contains("not a revoke"));
        let m = v.meta(&Selector::user(Scope::workspace(&ops.id), "STRIPE_KEY"), &Actor::Owner).expect("meta");
        assert_eq!(m.status, k2_vault::SecretStatus::Disabled);
        assert_eq!(v.handoff_get(&id, "out").expect("out").status, "unsent");
        let lines: Vec<String> = k2_core::auth_audit::tail(1000)
            .expect("tail")
            .into_iter()
            .filter(|e| e["event"] == AUDIT_EVENT)
            .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(lines.iter().any(|l| l.starts_with("ok ask STRIPE_KEY") && l.ends_with(&ask_id)), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("ok unsend STRIPE_KEY") && l.contains(&id)), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("deny unsend STRIPE_KEY") && l.ends_with("not_yours")), "{lines:?}");
        assert!(!surfaces(&v, &[&sales, &ops]).contains(&secret));
    });
}

/// The 2026-10-28 incident, daemon side: another server's agent sends its
/// own api_key (fetch delivery, no sites) with --ttl 1h. The receiving
/// list says it lands as a file and keeps 1 hour; the accept answer says
/// file and how to use it; env-file points at `k2 vault file`; `k2 vault
/// file` gives the agent a 0600 file holding exactly the value; a
/// fetch-only secret is refused with fetch and who can widen it. The
/// value is never in an answer.
#[test]
fn a_cross_server_key_without_sites_lands_as_a_file_the_agent_reads() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let run = k2_vault::fsutil::TestDir::new("ho-file-run");
        let _rt = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, run.path().join("run"));
        let v = crate::vault::ensure_test_vault();
        let ops = workspace("ops");
        let name = format!("K2N12_ENROLL_{}", &uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase());
        let enroll = format!("ABCDE-FGHJK.{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let item = k2_vault::handoff::HandoffItem {
            name: name.clone(),
            as_name: None,
            kind: "api_key".into(),
            hosts: vec![],
            inject: None,
            description: String::new(),
            delivery: "fetch".into(),
            ceiling: "env".into(),
            human_given: false,
            ttl_secs: Some(3600),
        };
        let id = k2_vault::handoff::new_delivery_id();
        let inc = Incoming {
            delivery_id: id.clone(),
            workspace_id: ops.id.clone(),
            peer_addr: "k2-dev-web::z3thon.k2.dev".into(),
            peer_fp: Some("fp-peer".into()),
            sender_actor: "agent:x@z3thon.k2.dev".into(),
            source_vault_id: Some("vault-peer".into()),
            item,
            note: "run k2-node servers add with it".into(),
            reply_to: None,
            auto_land: false,
            remote_off: false,
            sender_label: "k2-dev-web on z3thon.k2.dev".into(),
        };
        v.handoff_receive(inc, SecretBytes::from_slice(enroll.as_bytes())).expect("pending");

        // The person's list: how it would land, and its --ttl (bug 3).
        let (st, list, raw) = call_p(&v, INCOMING, &owner(), Value::Null, ws_param(&ops));
        assert_eq!(st, 200, "{raw}");
        let it = &list["items"][0];
        assert_eq!((it["landsAs"]["delivery"].as_str(), it["landsAs"]["ceiling"].as_str()), (Some("file"), Some("env")), "{it}");
        assert_eq!(it["ttlSecs"], 3600, "{it}");
        let t = |k: &str| chrono::DateTime::parse_from_rfc3339(it[k].as_str().unwrap_or_else(|| panic!("{k}: {it}"))).expect("rfc3339").timestamp();
        assert_eq!(t("expiresAt") - t("createdAt"), 3600, "the card ends with the --ttl, not in 7 days: {it}");
        assert!(!raw.contains(&enroll));

        // The owner accepts: the answer says file, until when, and how (bug 2's "show it").
        let (st, j, raw) = call(&v, DECIDE, &owner(), json!({ "id": id, "action": "accept" }));
        assert_eq!(st, 200, "{raw}");
        assert_eq!((j["secret"]["delivery"].as_str(), j["secret"]["ceiling"].as_str()), (Some("file"), Some("env")), "{j}");
        assert!(j["secret"]["expiresAt"].is_string(), "{j}");
        assert!(j["secret"]["use"].as_str().unwrap_or("").contains(&format!("k2 vault file --secret {name}")), "{j}");
        assert!(!raw.contains(&enroll));

        let me = agent(&ops);
        let use_call = |path: &str, body: Value| -> (u16, Value, String) {
            let r = crate::vault_routes::handle_on(&v, path, Ok(me.clone()), body.to_string().as_bytes(), &HashMap::new(), "test", None);
            let st: u16 = r.status[..3].parse().expect("status");
            (st, serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json from {path}: {}", r.body)), r.body)
        };
        // env-file: refused, and it says what works (bug 1c).
        let (st, e, raw) = use_call(crate::vault_use_routes::ENV_FILE, json!({ "secrets": [name] }));
        assert_eq!((st, code(&e)), (403, "delivery_not_granted"), "{raw}");
        assert!(e["error"]["hint"].as_str().unwrap_or("").contains(&format!("run k2 vault file --secret {name}")), "{raw}");
        // k2 vault file: a 0600 file holding exactly the value (bug 1b).
        let (st, f, raw) = use_call(crate::vault_use_routes::FILE, json!({ "secret": name, "ttlSecs": 30 }));
        assert_eq!(st, 200, "{raw}");
        assert!(!raw.contains(&enroll), "the answer names the path, never the value");
        let path = std::path::PathBuf::from(f["path"].as_str().expect("path"));
        assert_eq!(std::fs::read(&path).expect("read the file"), enroll.as_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777, 0o600);
        }
        let (st, d, raw) = use_call(crate::vault_use_routes::FILE, json!({ "done": path.display().to_string() }));
        assert_eq!(st, 200, "{raw}");
        assert_eq!(d["removed"].as_str(), Some(path.display().to_string().as_str()));
        assert!(!path.exists(), "--done removes it");
        let (st, e, _) = use_call(crate::vault_use_routes::FILE, json!({ "done": "/etc/passwd" }));
        assert_eq!((st, code(&e)), (400, "not_a_secret_file"));
        let (st, e, _) = use_call(crate::vault_use_routes::FILE, json!({}));
        assert_eq!((st, code(&e)), (400, "secret_required"));

        // A fetch-only secret (sites, no readable delivery): refused with
        // fetch and who can widen it.
        put(&v, &ops, "STRIPE_KEY", "sk_test_k2test_fetch_only", &Actor::Owner);
        let (st, e, raw) = use_call(crate::vault_use_routes::FILE, json!({ "secret": "STRIPE_KEY" }));
        assert_eq!((st, code(&e)), (403, "delivery_not_granted"), "{raw}");
        let hint = e["error"]["hint"].as_str().unwrap_or("");
        assert!(hint.contains("k2 vault fetch --secret STRIPE_KEY") && hint.contains("k2 vault delivery STRIPE_KEY file"), "{hint}");
        // People use reveal, never this.
        let r = crate::vault_routes::handle_on(&v, crate::vault_use_routes::FILE, Ok(owner()), json!({ "secret": name }).to_string().as_bytes(), &HashMap::new(), "test", None);
        assert!(r.status.starts_with("403"), "{}", r.body);
        assert!(!surfaces(&v, &[&ops]).contains(&enroll));
    });
}
