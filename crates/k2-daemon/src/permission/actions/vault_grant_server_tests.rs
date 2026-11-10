//! `vault.grant-server` end to end (0.46.11): an agent's `k2 vault grant
//! NAME --scope server --to ws:<itself>` of a server secret no one shared
//! with it is refused the same whether or not the secret exists, the refusal
//! teaches the ask, a Member can't answer, the owner allows it (with the
//! vault confirm for a readable delivery) and the card runs the grant route
//! as the owner; Deny grants nothing. Test vault (fake key store), test DB,
//! temp HOME; no real agent is woken (turns go to `inject`'s sink).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{ConfirmAction, Floor, Proof};
use k2_vault::{AgentActor, Kind, PutRequest, SecretBytes};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask_ok, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use super::*;
use crate::vault::actor::{Channel, VaultCaller};

const VALUE: &[u8] = b"k2test-server-secret-not-real-3307";

fn agent_caller(ws: &Ws) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.id.clone(), session_id: Some("sess-gs".into()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("sess-gs".into()),
    }
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", &uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase())
}

/// A server secret an Owner saved, delivered as env.
fn server_secret(v: &k2_vault::LocalVault, name: &str) {
    let mut r = PutRequest::new(Namespace::User, Scope::server(), name, Kind::Generic);
    r.default_delivery = Some(Delivery::Env);
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(VALUE), &Actor::Owner).expect("put");
}

fn agent_grant(v: &k2_vault::LocalVault, ws: &Ws, name: &str, how: Option<&str>) -> (String, Value) {
    let mut b = json!({ "name": name, "scope": "server", "to": format!("ws:{}", ws.handle) });
    if let Some(h) = how {
        b["as"] = json!(h);
    }
    let r = crate::vault_routes::handle_on(v, ROUTE, Ok(agent_caller(ws)), b.to_string().as_bytes(), &HashMap::new(), "test", None);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or(Value::Null))
}

fn ws_grant(v: &k2_vault::LocalVault, ws: &Ws, name: &str) -> Option<k2_vault::Grant> {
    v.grants(&selector(name), &Actor::Owner)
        .expect("grants")
        .into_iter()
        .find(|g| g.consumer.kind == ConsumerKind::Workspace && g.consumer.id == ws.id)
}

fn confirm_for(v: &k2_vault::LocalVault, request: &str, stamp: &str) -> String {
    v.confirm.mint(SHARE_CONFIRM, Some(request), stamp, "local-auth")
}

#[test]
fn the_card_is_registered_with_the_share_floor_a_confirm_and_its_wall() {
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof), (Floor::Admin, Proof::SettingsConfirm { action: ConfirmAction::VaultShareService }));
    assert_eq!(r.spec.params[0].kind, ParamKind::OtherSecret, "never the asking workspace's own");
    assert_eq!(r.spec.walls[0].route_prefix, "/cli/vault/grant");
    assert!(crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post).is_some(), "{ROUTE} has a POST row");
}

/// The ask: one refusal whether or not the secret exists, naming what to
/// ask for; the walls map teaches that exact line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agents_grant_of_a_server_secret_it_cant_see_teaches_the_ask() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("gsa");
    let name = uniq("SHARED_DB");
    server_secret(&v, &name);
    let ghost = uniq("NO_SUCH");

    let (s, e) = agent_grant(&v, &ws, &name, Some("env"));
    assert_eq!(s, "403 Forbidden", "{e}");
    assert_eq!(e["error"]["code"], crate::vault_routes::GRANT_OUT_OF_REACH, "{e}");
    assert_eq!((e["error"]["secret"].as_str(), e["error"]["as"].as_str()), (Some(name.as_str()), Some("env")), "{e}");
    let hint = e["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("Ask your person (an Owner or Admin) to share it with you: in Settings → Vault, Share on"), "cards off: {hint}");
    assert!(hint.contains(&format!("k2 vault grant {name} --scope server --to ws:{} --as env", ws.handle)), "{hint}");
    assert!(!e.to_string().contains("k2test-server-secret"), "never the value");
    // A name that doesn't exist gets the same answer (V20).
    let (s2, e2) = agent_grant(&v, &ws, &ghost, Some("env"));
    assert_eq!(s2, s);
    assert_eq!(e2.to_string().replace(&ghost, "X"), e.to_string().replace(&name, "X"), "the same words whether or not it exists");
    // Without --as: still the card, and no `as` on it.
    let (_, e3) = agent_grant(&v, &ws, &name, None);
    assert_eq!(e3["error"]["code"], crate::vault_routes::GRANT_OUT_OF_REACH, "{e3}");
    assert!(e3["error"].get("as").is_none(), "{e3}");
    assert!(ws_grant(&v, &ws, &name).is_none(), "nothing granted");

    let params = HashMap::new();
    let req = crate::permission::walls::Refused { status: 403, method: "POST", path: ROUTE, params: &params, body: None, response: &e, workspace_id: "" };
    let p = crate::permission::walls::permission_for(&req).expect("teaches");
    assert_eq!(p["command"], format!("k2 permission request vault.grant-server as=env secret={name} --why \"<what you need it for>\""), "{p}");
    let req = crate::permission::walls::Refused { response: &e3, ..req };
    let p = crate::permission::walls::permission_for(&req).expect("teaches");
    assert_eq!(p["command"], format!("k2 permission request vault.grant-server secret={name} --why \"<what you need it for>\""), "{p}");

    // The ask is accepted for either name, and its card says the same (two
    // workspaces, so the second ask isn't batched onto the first card).
    let ws2 = seed("gsz");
    let a = ask_ok(&ws, ACTION, json!({ "secret": name, "as": "env" }));
    let b = ask_ok(&ws2, ACTION, json!({ "secret": ghost, "as": "env" }));
    let (va, vb) = (view(&ws, &card_of(&a)), view(&ws2, &card_of(&b)));
    assert_eq!(va["whatChanges"], format!("Let {} use the server secret {name} as env.", ws.handle), "{va}");
    assert_eq!(
        va["consequences"].as_str().unwrap().replace(&name, "X").replace(&ws.handle, "W"),
        vb["consequences"].as_str().unwrap().replace(&ghost, "X").replace(&ws2.handle, "W")
    );
    // Only someone who can answer is told a name isn't there.
    assert_eq!(missing(ACTION, &id_of(&b)), Some(format!("K2 has no server secret {ghost}; nothing to allow.")));
    assert_eq!(missing(ACTION, &id_of(&a)), None);
}

/// Approve, then the grant: a Member can't; no confirm grants nothing; the
/// owner's confirmed Allow runs the grant route as the owner, and the turn
/// says env reaches new sessions only (k2 agent refresh).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_allows_it_with_the_confirm_and_the_grant_route_runs_as_the_owner() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("gsb");
    let name = uniq("SHARED_DB");
    server_secret(&v, &name);

    let asked = ask_ok(&ws, ACTION, json!({ "secret": name }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("Server secret {name} for {}", ws.handle), "{vw}");
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": SHARE_CONFIRM, "secretId": id }), "its own delivery may be readable: {vw}");
    assert_eq!(
        super::super::vault::confirm_reason(SHARE_CONFIRM, Some(&id)),
        Some(Ok(format!("let {} use the server secret {name} the way it is set up (its own delivery)", ws.handle)))
    );

    let member = login("gsm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, "pr_another", "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert!(row(&id).is_answerable());
    assert!(ws_grant(&v, &ws, &name).is_none(), "nothing granted without the confirm");

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let g = ws_grant(&v, &ws, &name).expect("granted");
    assert_eq!((g.delivery, g.granted_by.as_str(), g.expires_at.as_deref()), (Delivery::Env, "owner", None), "the route ran as the owner, with no end");
    let turn = last_turn(&ws);
    assert!(turn.contains("approved by owner") && turn.contains(&format!("You can use {name} (server) as env until someone removes it")), "{turn}");
    assert!(turn.contains("reaches new sessions only") && turn.contains("k2 agent refresh"), "{turn}");
    assert!(!turn.contains("k2test-server-secret"), "never the value");
    // The agent sees it now.
    assert!(v.meta(&selector(&name), &agent_caller(&ws).actor).is_ok(), "shared with the workspace");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denying_grants_nothing_and_tells_the_agent() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("gsc");
    let name = uniq("SHARED_DB");
    server_secret(&v, &name);
    let asked = ask_ok(&ws, ACTION, json!({ "secret": name, "as": "fetch" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    assert_eq!(view(&ws, &card)["proof"], json!({ "kind": "click" }), "a fetch grant the agent can't read is a click");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Deny", "reason": "use the read replica" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state.as_str(), "denied");
    assert!(ws_grant(&v, &ws, &name).is_none(), "nothing granted");
    let turn = last_turn(&ws);
    assert!(turn.contains("denied") && turn.contains("use the read replica"), "{turn}");
}
