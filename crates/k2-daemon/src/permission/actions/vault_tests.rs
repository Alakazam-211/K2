//! `vault.delivery` end to end (2026-10-28): the agent's own `k2 vault
//! delivery NAME env` is refused with `vault_owner_confirm_required` (a
//! person gave the secret), the refusal teaches the ask, the agent asks, a
//! Member can't answer, the owner taps Allow and the card runs
//! `/cli/vault/policy` as the owner. Test vault (fake key store), test DB,
//! temp HOME; no real agent is woken (turns go to `inject`'s sink).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof, Undo};
use k2_vault::{AgentActor, Delivery, Kind, PutRequest, SecretBytes, SecretStore};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, below_floor, card_of, last_turn, login, seed, view, Ws};
use super::*;

fn agent_caller(ws: &Ws) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.id.clone(), session_id: Some("sess-vd".into()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("sess-vd".into()),
    }
}

/// A secret the owner gave this workspace (person-given: the agent can't
/// widen it), delivered by fetch, no sites.
fn given(v: &k2_vault::LocalVault, ws: &Ws, name: &str) {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), name, Kind::Generic);
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Fetch);
    v.put(r, SecretBytes::from_slice(b"k2test-enroll-code-not-real"), &Actor::Owner).expect("put");
}

fn delivery(v: &k2_vault::LocalVault, ws: &Ws, name: &str) -> (Delivery, Delivery) {
    let m = v.meta(&Selector::user(Scope::workspace(ws.id.clone()), name), &Actor::Owner).expect("meta");
    (m.default_delivery, m.delivery_ceiling)
}

#[test]
fn the_card_is_registered_with_the_widen_floor_a_click_and_its_wall() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime));
    assert!(matches!(r.spec.undo, Undo::None));
    assert_eq!(r.spec.params[1].choices, &["file", "env"], "api is never widened from a card");
    assert_eq!(r.spec.walls.len(), 1);
    assert_eq!(r.spec.walls[0].route_prefix, "/cli/vault/policy");
    assert!(crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post).is_some(), "{ROUTE} has a POST row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_refused_a_wider_delivery_asks_and_the_owner_allow_widens_it() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vdl");
    let name = format!("ENROLL_{}", &uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase());
    given(&v, &ws, &name);

    // The agent's own `k2 vault delivery NAME env`: refused, and the
    // refusal names the secret and delivery (the card's wall).
    let body = json!({ "name": name, "delivery": "env" }).to_string();
    let r = crate::vault_routes::handle_on(&v, ROUTE, Ok(agent_caller(&ws)), body.as_bytes(), &HashMap::new(), "test", None);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let e: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(e["error"]["code"], "vault_owner_confirm_required", "{e}");
    assert_eq!((e["error"]["secret"].as_str(), e["error"]["delivery"].as_str()), (Some(name.as_str()), Some("env")), "{e}");
    assert!(e["error"]["hint"].as_str().unwrap_or("").contains("Settings → Vault"), "the hint is unchanged: {e}");
    assert_eq!(delivery(&v, &ws, &name), (Delivery::Fetch, Delivery::Fetch), "nothing changed");
    // A hosts-only widening carries no delivery: no extra fields, so the
    // card's wall doesn't claim it.
    let body = json!({ "name": name, "addHosts": ["api.example.com"] }).to_string();
    let r = crate::vault_routes::handle_on(&v, ROUTE, Ok(agent_caller(&ws)), body.as_bytes(), &HashMap::new(), "test", None);
    let e: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(e["error"]["code"], "vault_owner_confirm_required", "{e}");
    assert!(e["error"].get("delivery").is_none(), "{e}");

    // The walls map teaches exactly this ask.
    let refusal = json!({ "error": { "code": "vault_owner_confirm_required", "hint": "x", "secret": name, "delivery": "env" } });
    let params = HashMap::new();
    let req = crate::permission::walls::Refused {
        status: 403,
        method: "POST",
        path: ROUTE,
        params: &params,
        body: None,
        response: &refusal,
    };
    let p = crate::permission::walls::permission_for(&req).expect("teaches");
    assert_eq!(p["action"], ACTION, "{p}");
    assert!(p["command"].as_str().unwrap().starts_with(&format!("k2 permission request vault.delivery delivery=env secret={name} --why")), "{p}");
    assert_eq!((p["params"]["secret"].as_str(), p["params"]["delivery"].as_str()), (Some(name.as_str()), Some("env")), "{p}");

    // It asks; the card says what changes.
    let v1 = ask_ok(&ws, ACTION, json!({ "secret": name, "delivery": "env" }));
    let card = card_of(&v1);
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("{name} delivered as env"), "{vw}");
    assert!(vw["whatChanges"].as_str().unwrap_or("").contains(&format!("read {name} as env")), "{vw}");

    // A Member is below the floor; nothing changes.
    let member = login("vdm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(delivery(&v, &ws, &name), (Delivery::Fetch, Delivery::Fetch));

    // The owner taps Allow: the route runs as the owner; the ceiling rises
    // with the delivery.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(delivery(&v, &ws, &name), (Delivery::Env, Delivery::Env));
    let turn = last_turn(&ws);
    assert!(turn.contains("approved by owner") && turn.contains(&format!("k2 vault env-file --secret {name}")), "{turn}");
    assert!(!turn.contains("k2test-enroll-code-not-real"), "never the value");

    // Asking again: already done. api is not a choice; an unknown secret moved.
    let (s, v2): (String, Value) = ask(&ws, ACTION, json!({ "secret": name, "delivery": "file" }));
    assert_eq!((s.as_str(), &v2["status"]), ("200 OK", &json!("already_done")), "{v2}");
    let (s, v3) = ask(&ws, ACTION, json!({ "secret": name, "delivery": "api" }));
    assert!(s.starts_with("400"), "{s}: {v3}");
    let (s, v4) = ask(&ws, ACTION, json!({ "secret": "NO_SUCH_SECRET", "delivery": "env" }));
    assert!(s.starts_with("400"), "{s}: {v4}");
}
