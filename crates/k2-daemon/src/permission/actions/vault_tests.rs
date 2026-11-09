//! `vault.delivery` end to end (2026-10-28): the agent's own `k2 vault
//! delivery NAME env` is refused with `vault_owner_confirm_required` (a
//! person gave the secret), the refusal teaches the ask, the agent asks, a
//! Member can't answer, the owner taps Allow and the card runs
//! `/cli/vault/policy` as the owner. Test vault (fake key store), test DB,
//! temp HOME; no real agent is woken (turns go to `inject`'s sink).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{ConfirmAction, Direction, Floor, Grant, ParamKind, Proof, Undo};
use k2_vault::{AgentActor, AuditQuery, Delivery, Kind, PutRequest, SecretBytes, SecretStore};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, apply_as, ask, ask_ok, ask_refused, below_floor, card_of, id_of, last_turn, login, refused_code, row, seed, view, Ws};
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
        workspace_id: "",
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

// ── vault.share-service ─────────────────────────────────────────────
//
// The refusal's Ask-with line with cards on needs the process-wide switch,
// so it lives in `tests/permission_vault_share_service.rs`.

/// The value these tests share; it must never surface.
const SHARED_VALUE: &[u8] = b"k2test-share-pin-not-real-8841";

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", &uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase())
}

/// A secret a person gave this workspace through a card: delivered as a file.
fn given_file(v: &k2_vault::LocalVault, ws: &Ws, name: &str) {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), name, Kind::Generic);
    r.default_delivery = Some(Delivery::File);
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(SHARED_VALUE), &Actor::Owner).expect("put");
}

fn cmd_service(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ps::insert(&conn, &ws.id, name, "node s.mjs", "/tmp", 47_301, ps::EXPOSE_LOCAL, ps::DESIRED_STOPPED, ps::KIND_CMD, "").expect("service").id
}

/// An App row the way `k2 app install` makes one: a sentinel service and a
/// container (guarded) App.
fn walled_app(ws: &Ws, name: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let svc = ps::insert(&conn, &ws.id, name, ps::CMD_APP_SENTINEL, "/tmp", 0, ps::EXPOSE_LOCAL, ps::DESIRED_STOPPED, ps::KIND_CMD, "").expect("service");
    app_store::insert_app(
        &conn,
        &app_store::NewApp {
            app_id: name,
            service_id: &svc.id,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "container",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .expect("app");
    svc.id
}

fn confirm_for(v: &k2_vault::LocalVault, id: &str, stamp: &str) -> String {
    v.confirm.mint(SHARE_CONFIRM, Some(id), stamp, "local-auth")
}

fn grant_to(v: &k2_vault::LocalVault, ws: &Ws, name: &str, service_id: &str) -> Option<k2_vault::Grant> {
    let sel = Selector::user(Scope::workspace(ws.id.clone()), name);
    v.grants(&sel, &Actor::Owner).expect("grants").into_iter().find(|g| g.consumer.kind == ConsumerKind::PublishedService && g.consumer.id == service_id)
}

fn share_params(name: &str, service: &str, how: &str) -> Value {
    json!({ "secret": name, "service": service, "as": how })
}

#[test]
fn the_share_card_is_registered_with_the_grant_routes_people_and_a_confirm() {
    let r = super::super::find(SHARE_ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.grant), (Floor::Admin, Grant::OneTime));
    assert_eq!(r.spec.proof, Proof::SettingsConfirm { action: ConfirmAction::VaultShareService });
    assert_eq!(ConfirmAction::VaultShareService.as_str(), SHARE_CONFIRM);
    assert!(matches!(r.spec.undo, Undo::None));
    assert_eq!(r.spec.params[2].choices, &["file", "env"], "api, fetch and agent never reach a service from a card");
    assert_eq!(r.spec.walls[0].route_prefix, "/cli/vault/grant");
    assert!(crate::routes::route_policy::lookup(SHARE_ROUTE).and_then(|x| x.post).is_some());
}

/// Who may ask: its own secret, its own service. Everything else reads as a
/// name that doesn't exist; nothing about the other workspace leaks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_its_own_secret_and_its_own_service_can_be_asked_for() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (ws, other) = (seed("vsa"), seed("vsb"));
    let (mine, theirs) = (uniq("MINE"), uniq("THEIRS"));
    given_file(&v, &ws, &mine);
    given_file(&v, &other, &theirs);
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let own = format!("own{tag}");
    cmd_service(&ws, &own);
    let foreign = format!("foreign{tag}");
    let foreign_id = cmd_service(&other, &foreign);

    for service in [foreign.as_str(), foreign_id.as_str()] {
        let (s, e) = ask(&ws, SHARE_ACTION, share_params(&mine, service, "file"));
        assert!(s.starts_with("400"), "another workspace's service {service}: {s}: {e}");
        assert!(e.to_string().contains("has no service or App named"), "{e}");
    }
    let (s, e) = ask(&ws, SHARE_ACTION, share_params(&theirs, &own, "file"));
    assert!(s.starts_with("400"), "another workspace's secret: {s}: {e}");
    assert!(e.to_string().contains("has no secret named"), "{e}");
    let (s, e) = ask(&ws, SHARE_ACTION, share_params("NO_SUCH_SECRET", &own, "file"));
    assert!(s.starts_with("400"), "{s}: {e}");
    // A delivery a card doesn't give.
    for how in ["api", "fetch", "agent"] {
        let (s, e) = ask(&ws, SHARE_ACTION, share_params(&mine, &own, how));
        assert!(s.starts_with("400"), "{how}: {s}: {e}");
    }
    // Names go with their delivery.
    let (s, e) = ask(&ws, SHARE_ACTION, json!({ "secret": mine, "service": own, "as": "file", "env_name": "THE_PIN" }));
    assert!(s.starts_with("400"), "{s}: {e}");
    assert!(grant_to(&v, &ws, &mine, &foreign_id).is_none());
}

/// A name shared by two workspaces: the card reaches the asking workspace's
/// service, whichever the box's table would have found first, and so does
/// the agent's own call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_name_two_workspaces_share_reaches_only_the_askers() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (ws, other) = (seed("vsc"), seed("vsd"));
    let name = uniq("SAME");
    given_file(&v, &ws, &name);
    let label = format!("same{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let theirs = cmd_service(&other, &label);
    let mine = cmd_service(&ws, &label);

    let body = json!({ "name": name, "to": format!("service:{label}"), "as": "file" }).to_string();
    let r = crate::vault_routes::handle_on(&v, SHARE_ROUTE, Ok(agent_caller(&ws)), body.as_bytes(), &HashMap::new(), "test", None);
    assert_eq!(serde_json::from_str::<Value>(&r.body).unwrap()["error"]["code"], "vault_owner_confirm_required", "its own service was found: {}", r.body);

    let id = id_of(&ask_ok(&ws, SHARE_ACTION, share_params(&name, &label, "file")));
    let card = row(&id).card_id.expect("a card");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert!(grant_to(&v, &ws, &name, &mine).is_some(), "its own");
    assert!(grant_to(&v, &ws, &name, &theirs).is_none(), "never the other workspace's");
}

/// The whole card: words, who answers, the confirm, once only, no value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_allows_it_once_with_a_confirm_and_no_value_shows() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vse");
    let name = uniq("PIN");
    given_file(&v, &ws, &name);
    let label = format!("svc{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let svc = cmd_service(&ws, &label);

    // The agent's own share is refused, and the refusal's text with cards
    // off is today's, byte for byte (the off golden pins the other routes).
    let body = json!({ "name": name, "to": format!("service:{label}"), "as": "file" }).to_string();
    let r = crate::vault_routes::handle_on(&v, SHARE_ROUTE, Ok(agent_caller(&ws)), body.as_bytes(), &HashMap::new(), "test", None);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(
        r.body,
        json!({ "error": { "code": "vault_owner_confirm_required", "hint": format!(
            "a person gave this secret, so an agent can't widen where it goes or who can read it. Settings → Vault can't share with a service, so ask the owner or an Admin to run this in their own terminal (Terminal or iTerm, not a K2 terminal; it asks for Touch ID or the vault password; --scope is there because their default scope is server): k2 publish secret add {label} {name} --scope ws:{} --as file",
            crate::vault::workspace_label(&ws.id)
        ) } })
        .to_string()
    );

    // It asks; the card says what happens and to whom.
    let asked = ask_ok(&ws, SHARE_ACTION, share_params(&name, &label, "file"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["setting"], format!("{name} shared with {label}"), "{vw}");
    assert_eq!(vw["whatChanges"], format!("Let {} give its service {label} the secret {name} as a file.", ws.handle), "{vw}");
    assert_eq!(
        vw["consequences"],
        format!(
            "{label} can read the value whenever it runs ($K2_SECRETS_DIR/{name}), and so can anything it runs. {h} decides what {label} runs, so this lets {h} use the value too. It takes effect the next time {label} starts; {h} restarts it. To take it back: k2 publish secret remove {label} {name} --scope ws:{h}, then restart it.",
            h = ws.handle
        ),
        "{vw}"
    );
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": SHARE_CONFIRM, "secretId": id }), "{vw}");
    assert_eq!(vw["grant"], "one-time");

    // The confirm sheet's words come from the stored values.
    assert_eq!(confirm_reason(SHARE_CONFIRM, Some(&id)), Some(Ok(format!("share {name} with {label} as a file"))));
    assert!(matches!(confirm_reason(SHARE_CONFIRM, Some("no-such-request")), Some(Err(_))));
    assert_eq!(confirm_reason("reveal", Some(&id)), None);

    // An agent never answers, and never gets a confirm.
    let params: HashMap<String, String> = [
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
        ("project_id".to_string(), ws.id.clone()),
        ("from".to_string(), ws.handle.clone()),
    ]
    .into();
    let b = json!({ "addr": ws.handle, "id": card, "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") });
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &params, b.to_string().as_bytes(), "owner");
    let e: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!((r.status, &e["error"]["code"]), ("403 Forbidden", &json!("action_card_person_only")), "{e}");
    for path in [crate::vault_routes::CONFIRM_START, crate::vault_routes::CONFIRM_FINISH] {
        let b = json!({ "action": SHARE_CONFIRM, "secretId": id, "method": "vault-password", "password": "x" }).to_string();
        let r = crate::vault_routes::handle_on(&v, path, Ok(agent_caller(&ws)), b.as_bytes(), &HashMap::new(), "test", None);
        assert!(r.status.starts_with("403"), "{path}: {} {}", r.status, r.body);
    }
    // K2 knows the confirm action (a person's start isn't `bad_action`).
    let owner = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
    let b = json!({ "action": SHARE_CONFIRM, "secretId": id }).to_string();
    let r = crate::vault_routes::handle_on(&v, crate::vault_routes::CONFIRM_START, Ok(owner), b.as_bytes(), &HashMap::new(), "test", None);
    assert!(!r.body.contains("bad_action"), "{}", r.body);

    // The wrong people: a Member is below the floor, confirm or not.
    let member = login("vsm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, &format!("connect:{member}")) })), "admin");
    // The right person without the right proof: no token, another request's,
    // someone else's. Each leaves the card open and nothing shared.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, "pr_another_card", "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let admin = login("vsa", Role::Admin);
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, &format!("connect:{admin}")) }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": "k2vc_made_up" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "a login needs a real confirm too: {a}");
    // A grant-read token isn't this card's proof.
    let m = v.meta(&Selector::user(Scope::workspace(ws.id.clone()), &name), &Actor::Owner).expect("meta");
    let gr = v.confirm.mint(crate::vault_routes::GRANT_READ_CONFIRM_ACTION, Some(&m.id), "owner", "local-auth");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": gr }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert!(row(&id).is_answerable(), "a refused confirm leaves the card open");
    assert!(grant_to(&v, &ws, &name, &svc).is_none(), "nothing shared yet");

    // The owner, with the confirm bound to this request.
    let token = confirm_for(&v, &id, "owner");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": token, "client": "phone" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert!(v.confirm.take(&token, SHARE_CONFIRM, Some(&id), "owner").is_err(), "the Allow spent the token");
    let g = grant_to(&v, &ws, &name, &svc).expect("shared");
    assert_eq!((g.delivery, g.granted_by.as_str()), (Delivery::File, "owner"), "the route ran as the owner");
    let turn = last_turn(&ws);
    assert!(turn.contains("approved by owner"), "{turn}");
    assert!(turn.contains(&format!("{label} gets {name} as a file ($K2_SECRETS_DIR/{name}) from its next start: k2 publish stop {label}, then k2 publish start {label}.")), "{turn}");

    // Once: a second answer applies nothing, the token is spent, and the
    // grant isn't remade.
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") }));
    assert_eq!(s, "409 Conflict");
    assert_eq!(grant_to(&v, &ws, &name, &svc).expect("still shared").id, g.id, "not granted twice");
    // Asking again: already done.
    let (s, again) = ask(&ws, SHARE_ACTION, share_params(&name, &label, "file"));
    assert_eq!((s.as_str(), &again["status"]), ("200 OK", &json!("already_done")), "{again}");

    // The value is nowhere a person, the agent or the log can read: the
    // card, the request, the turn, the vault's audit.
    let value = std::str::from_utf8(SHARED_VALUE).unwrap();
    let audit = v.audit(&AuditQuery { secret_id: Some(m.id.clone()), ..Default::default() }, &Actor::Owner).expect("audit");
    assert!(audit.iter().any(|r| r.action == "grant"), "the share is audited: {audit:?}");
    for (what, text) in [
        ("view", view(&ws, &card).to_string()),
        ("request", format!("{:?}", row(&id))),
        ("turn", last_turn(&ws)),
        ("answer", a.to_string()),
        ("audit", format!("{audit:?}")),
        ("grant", format!("{g:?}")),
    ] {
        assert!(!text.contains(value), "{what} carries the value: {text}");
        assert!(what != "request" || text.contains(&name), "the request names the secret: {text}");
    }
}

/// An Admin login answers it too, as itself; a replayed token can't drive
/// the route a second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_login_allows_it_as_itself_and_a_token_works_once() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vsf");
    let name = uniq("PIN");
    given_file(&v, &ws, &name);
    let label = format!("svc{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let svc = cmd_service(&ws, &label);
    let admin = login("vsg", Role::Admin);
    let stamp = format!("connect:{admin}");

    let asked = ask_ok(&ws, SHARE_ACTION, share_params(&name, &label, "file"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let token = confirm_for(&v, &id, &stamp);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": token }));
    assert_eq!(s, "200 OK", "{a}");
    let g = grant_to(&v, &ws, &name, &svc).expect("shared");
    assert_eq!(g.granted_by, stamp, "the route ran as the login");

    // Apply never runs without a token, and outside an answer it has no
    // request to bind one to: nothing is granted again.
    let who = Answerer::Login { user: admin.clone(), role: "admin".into() };
    assert_eq!(refused_code(apply_as(&id, &who, None)), "vault_confirm_required");
    assert_eq!(refused_code(apply_as(&id, &who, Some(&token))), "failed");
    assert_eq!(grant_to(&v, &ws, &name, &svc).expect("shared").id, g.id);
    // The route itself refuses a Member, whatever the card says.
    let member = login("vsn", Role::Member);
    let caller = VaultCaller { actor: Actor::Login { username: member, role: k2_vault::Role::Member }, channel: Channel::Login, session_id: None };
    let b = json!({ "id": g.secret_id, "to": format!("service:{svc}"), "as": "file" }).to_string();
    let r = crate::vault_routes::handle_on(&v, SHARE_ROUTE, Ok(caller), b.as_bytes(), &HashMap::new(), "test", None);
    assert!(r.status.starts_with("403") && r.body.contains("vault_admin_required"), "{} {}", r.status, r.body);
}

/// The customer's case: a guarded (container) App. The card's grant comes
/// out of the same two calls a guarded start makes, as a file the wall
/// carries into `$K2_SECRETS_DIR`, and the restart it names is `k2 app`'s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guarded_app_gets_the_shared_file_through_its_wall() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vsh");
    let name = uniq("MEETING_PREVIEW_PIN");
    given_file(&v, &ws, &name);
    let app = format!("app{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let svc = walled_app(&ws, &app);

    // Before the card: nothing reaches it.
    let g = crate::vault::publish_env::granted(&svc, &ws.id, std::path::Path::new("/nonexistent-share-cwd"));
    assert!(g.files.iter().all(|(n, _)| *n != name), "not shared yet");

    let asked = ask_ok(&ws, SHARE_ACTION, share_params(&name, &app, "file"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert!(last_turn(&ws).contains(&format!("from its next start: k2 app restart {app}.")), "{}", last_turn(&ws));

    // What every guarded start does (`guarded::start_with`).
    let g = crate::vault::publish_env::granted(&svc, &ws.id, std::path::Path::new("/nonexistent-share-cwd"));
    assert!(g.env.iter().all(|(k, _)| *k != name), "a file, not an env variable");
    let (items, dropped) = crate::apps::guarded::secret_items(g.env, g.files);
    assert!(dropped.is_empty(), "the wall drops nothing: {dropped:?}");
    let item = items.iter().find(|i| i.file.as_deref() == Some(name.as_str())).expect("the wall carries the file");
    assert!(item.env.is_none());
    let hex: String = SHARED_VALUE.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(item.value_hex, hex, "the secret's own bytes");
    assert!(k2_root_proto::engine::secret_file_ok(&name));

    // A name the wall would drop (the vault itself allows both) is refused
    // when asked, not lost at start.
    let other = uniq("OTHER");
    given_file(&v, &ws, &other);
    for (extra, shown) in [(json!({ "as": "file", "file_name": ".hidden" }), "$K2_SECRETS_DIR/.hidden"), (json!({ "as": "env", "env_name": "PORT" }), "$PORT")] {
        let mut p = json!({ "secret": other, "service": app });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (s, e) = ask(&ws, SHARE_ACTION, p);
        assert!(s.starts_with("400"), "{s}: {e}");
        assert!(e.to_string().contains(&format!("its wall doesn't carry the name {shown}")), "{e}");
    }
    // A name the vault refuses, chosen or by default, for any service.
    let reserved = uniq("K2_RESERVED");
    given_file(&v, &ws, &reserved);
    let (s, e) = ask(&ws, SHARE_ACTION, share_params(&reserved, &app, "env"));
    assert!(s.starts_with("400"), "{s}: {e}");
}

/// What a card can't do is refused when asked, with what to do instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_the_route_would_refuse_is_refused_when_asked() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vsi");
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let label = format!("svc{tag}");
    let svc = cmd_service(&ws, &label);

    // Above the secret's ceiling: the delivery card comes first.
    let low = uniq("LOW");
    given(&v, &ws, &low);
    let (s, e) = ask(&ws, SHARE_ACTION, share_params(&low, &label, "file"));
    assert!(s.starts_with("400"), "{s}: {e}");
    assert!(e.to_string().contains(&format!("vault.delivery secret={low} delivery=file")), "{e}");

    // A static site runs nothing that could read it.
    let site = format!("site{tag}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::insert(&conn, &ws.id, &site, "", "/tmp", 47_302, ps::EXPOSE_LOCAL, ps::DESIRED_STOPPED, ps::KIND_SKIN, "").expect("site");
    }
    let name = uniq("PIN");
    given_file(&v, &ws, &name);
    let (s, e) = ask(&ws, SHARE_ACTION, share_params(&name, &site, "file"));
    assert!(s.starts_with("400"), "{s}: {e}");
    assert!(e.to_string().contains("static site"), "{e}");

    // An env share already there wins over a file at a start: say so.
    let asked = ask_ok(&ws, SHARE_ACTION, share_params(&name, &label, "env"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(grant_to(&v, &ws, &name, &svc).expect("shared").delivery, Delivery::Env);
    let (s, e) = ask(&ws, SHARE_ACTION, share_params(&name, &label, "file"));
    assert!(s.starts_with("400"), "{s}: {e}");
    assert!(e.to_string().contains("wins over a file"), "{e}");

    // A chosen name travels with the card and reaches the service.
    let named = uniq("NAMED");
    given_file(&v, &ws, &named);
    let asked = ask_ok(&ws, SHARE_ACTION, json!({ "secret": named, "service": label, "as": "file", "file_name": "pin.txt" }));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(grant_to(&v, &ws, &named, &svc).expect("shared").file_name.as_deref(), Some("pin.txt"));
    let g = crate::vault::publish_env::granted(&svc, &ws.id, std::path::Path::new("/nonexistent-share-cwd"));
    assert!(g.files.iter().any(|(n, b)| n == "pin.txt" && b.as_slice() == SHARED_VALUE), "named as asked");
}

// ── vault.use ───────────────────────────────────────────────────────

const USE_VALUE: &[u8] = b"k2test-use-card-value-not-real-7731";

fn server_secret(v: &k2_vault::LocalVault, name: &str, default: Delivery, ceiling: Delivery) {
    let mut r = PutRequest::new(Namespace::User, Scope::server(), name, Kind::Generic);
    r.hosts = vec!["api.example.com".to_string()];
    r.default_delivery = Some(default);
    r.delivery_ceiling = Some(ceiling);
    v.put(r, SecretBytes::from_slice(USE_VALUE), &Actor::Owner).expect("a person adds the server secret");
}

fn use_params(name: &str, from: &str, how: &str, span: &str) -> Value {
    json!({ "secret": name, "from": from, "as": how, "for": span })
}

/// The session the test kit's agent asks from (`held::tests::agent`).
fn asking_session(ws: &Ws) -> String {
    format!("sess-{}", &ws.id[..8])
}

fn session_of(ws: &Ws, session: &str) -> k2_vault::Requester {
    k2_vault::Requester::Session { workspace_id: ws.id.clone(), session_id: Some(session.to_string()) }
}

fn requester(ws: &Ws) -> k2_vault::Requester {
    session_of(ws, &asking_session(ws))
}

fn fetch_with(v: &k2_vault::LocalVault, name: &str, who: &k2_vault::Requester) -> Result<k2_vault::Resolved, k2_vault::VaultError> {
    let (_, origin) = k2_vault::hosts::Origin::parse_url("https://api.example.com/v1").expect("origin");
    v.resolve(&Selector::user(Scope::server(), name), k2_vault::Use::Fetch(&origin), who)
}

fn server_grants(v: &k2_vault::LocalVault, name: &str) -> Vec<k2_vault::Grant> {
    v.grants(&Selector::user(Scope::server(), name), &Actor::Owner).expect("grants")
}

fn passport_params(ws: &Ws) -> HashMap<String, String> {
    [
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
        ("project_id".to_string(), ws.id.clone()),
        ("from".to_string(), ws.handle.clone()),
    ]
    .into()
}

/// The stored card as `viewer` sees it, through the projection every door uses.
fn projected(ws: &Ws, card: &str, viewer: &k2_core::permission::card::Viewer, cards: u8) -> k2_core::overlay::doc::OverlayDoc {
    let doc = k2_core::overlay::get_thread_doc(&ws.conv, card).expect("doc read").expect("card doc");
    crate::permission::view::project(&doc, viewer, cards)
}

fn option_labels(d: &k2_core::overlay::doc::OverlayDoc) -> Vec<String> {
    d.choice.as_ref().map(|c| c.options.iter().map(|o| o.label.clone()).collect()).unwrap_or_default()
}

fn utc(ts: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(ts).expect("rfc3339").with_timezone(&chrono::Utc)
}

/// Seconds between an expiry and `now + secs`.
fn off_by(expires_at: &str, secs: i64) -> i64 {
    (utc(expires_at) - (chrono::Utc::now() + chrono::Duration::seconds(secs))).num_seconds().abs()
}

#[test]
fn the_use_card_is_registered_with_a_person_floor_and_a_proof_that_follows_the_delivery() {
    let r = super::super::find(USE_ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.grant, r.spec.direction), (Floor::Admin, Grant::OneTime, Direction::Loosen));
    assert!(matches!(r.spec.undo, Undo::None));
    assert_eq!(r.spec.params[0].kind, ParamKind::OtherSecret, "a Secret kind would mean the asking workspace's own");
    assert_eq!(r.spec.params[2].choices, &["fetch", "agent", "file", "env"], "api never comes from a card");
    assert_eq!(r.spec.params[3].choices, &["1h", "2h", "8h", "24h", "session"]);
    // The strict proof is the catalog's: only the two deliveries the grantee
    // can't read come down to a click, and nothing unexpected does.
    assert_eq!(r.spec.proof, Proof::SettingsConfirm { action: ConfirmAction::VaultShareService });
    assert!(!r.spec.proof.batchable(), "Allow all never covers it");
    let ctx = |how: &str| Ctx { workspace_id: "w".into(), workspace_handle: "h".into(), params: use_params("N", "server", how, "1h").as_object().unwrap().clone() };
    for how in ["fetch", "agent"] {
        assert_eq!(use_proof(USE_ACTION, &ctx(how)), Some(Proof::Click), "{how}");
    }
    for how in ["file", "env", "api", "FETCH", ""] {
        assert_eq!(use_proof(USE_ACTION, &ctx(how)), None, "{how} keeps the confirm");
    }
    assert_eq!(use_proof(ACTION, &ctx("fetch")), None, "no other action's proof moves");
    assert!(crate::routes::route_policy::lookup(USE_ROUTE).and_then(|x| x.post).is_some());
}

/// V20: asking says nothing about what exists. The same answer, the same
/// card and the same refusals for a real name and a made-up one; only a
/// person who can answer is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_name_that_does_not_exist_gets_the_same_answers_as_one_that_does() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (ws, other) = (seed("vua"), seed("vub"));
    let (real, fake, theirs) = (uniq("REAL"), uniq("FAKE"), uniq("THEIRS"));
    server_secret(&v, &real, Delivery::Fetch, Delivery::Env);
    given_file(&v, &other, &theirs);
    let from_other = format!("ws:{}", other.handle);

    // The ask: both accepted, both cards, the same words but for the name.
    let a = ask_ok(&ws, USE_ACTION, use_params(&real, "server", "fetch", "1h"));
    let b = ask_ok(&ws, USE_ACTION, use_params(&fake, "server", "fetch", "1h"));
    let c = ask_ok(&ws, USE_ACTION, use_params(&theirs, &from_other, "fetch", "1h"));
    let d = ask_ok(&ws, USE_ACTION, use_params(&fake, "ws:no-such-workspace", "fetch", "1h"));
    let shape = |x: &Value| x.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(shape(&a), shape(&b));
    assert_eq!(shape(&a), shape(&c));
    assert_eq!(shape(&a), shape(&d));
    let (real_card, fake_card) = (card_of(&a), card_of(&b));
    // Each is read on its own: a batch has no room for the approver's line.
    assert_ne!(real_card, fake_card, "never batched");
    assert_ne!(card_of(&c), card_of(&d), "never batched");

    // What the agent reads back: nothing about the name.
    let agent_view = k2_core::permission::card::Viewer::Passport { workspace_id: ws.id.clone() };
    let body_of = |card: &str| serde_json::to_value(projected(&ws, card, &agent_view, 2)).expect("json")["body"].clone();
    assert_eq!(body_of(&real_card).as_str().map(|s| s.replace(&real, "NAME")), body_of(&fake_card).as_str().map(|s| s.replace(&fake, "NAME")), "the same waiting line");

    // The owner sees it, on its own card only, with no Allow to press.
    let owner = k2_core::permission::card::Viewer::OwnerToken;
    let seen = serde_json::to_value(projected(&ws, &fake_card, &owner, 2)).expect("json");
    let line = format!("K2 has no server secret {fake}; nothing to allow.");
    assert_eq!(seen["choice"]["action"]["view"]["consequences"], line, "{seen}");
    assert_eq!(option_labels(&projected(&ws, &fake_card, &owner, 2)), ["Deny"], "nothing to allow");
    let fallback = projected(&ws, &fake_card, &owner, 0);
    assert_eq!(option_labels(&fallback), ["Deny"], "an old client too");
    assert!(fallback.choice.as_ref().unwrap().prompt.contains(&line), "{fallback:?}");
    assert_eq!(option_labels(&projected(&ws, &real_card, &owner, 2)), ["Allow", "Deny"], "the real one is answerable");
    let not_there = serde_json::to_value(projected(&ws, &card_of(&d), &owner, 2)).expect("json");
    assert_eq!(not_there["choice"]["action"]["view"]["consequences"], format!("K2 has no workspace named no-such-workspace, so no secret {fake} there; nothing to allow."), "{not_there}");
    // A Member can't answer, so a Member isn't told either.
    let member = k2_core::permission::card::Viewer::Login { user: login("vun", Role::Member), role: "member".into() };
    let m = serde_json::to_value(projected(&ws, &fake_card, &member, 2)).expect("json");
    assert!(!m.to_string().contains("nothing to allow"), "{m}");
    // The stored doc holds no answer either way.
    let stored = serde_json::to_value(k2_core::overlay::get_thread_doc(&ws.conv, &fake_card).expect("read").expect("doc")).expect("json");
    assert!(!stored.to_string().contains("nothing to allow"), "{stored}");
}

/// The refusal for using a name it can't see: byte for byte the same before
/// the secret exists and after, on all three use routes, and it teaches the
/// card.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_use_route_refuses_a_name_it_cant_see_the_same_whether_or_not_it_exists() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (ws, other) = (seed("vuw"), seed("vux"));
    let theirs = uniq("THEIRS");
    given_file(&v, &other, &theirs);
    let from_other = format!("ws:{}", other.handle);
    let name = uniq("LATER");
    let caller = agent_caller(&ws);
    let call = |path: &str, body: &Value| {
        let r = crate::vault_use_routes::handle(&v, path, &caller, body.to_string().as_bytes(), &HashMap::new(), "test", None);
        (r.status.to_string(), r.body)
    };
    let fetch = json!({ "url": "https://api.example.com/v1", "secrets": [{ "name": name, "scope": "server" }] });
    let file = json!({ "secret": name, "scope": "server" });
    let env = json!({ "secrets": [name], "scope": "server" });
    let before = [call(crate::vault_use_routes::FETCH, &fetch), call(crate::vault_use_routes::FILE, &file), call(crate::vault_use_routes::ENV_FILE, &env)];
    server_secret(&v, &name, Delivery::Env, Delivery::Env);
    let after = [call(crate::vault_use_routes::FETCH, &fetch), call(crate::vault_use_routes::FILE, &file), call(crate::vault_use_routes::ENV_FILE, &env)];
    assert_eq!(before, after, "the refusal must not say whether the name exists");
    for ((status, body), want) in before.iter().zip(["fetch", "file", "env"]) {
        assert_eq!(status, "403 Forbidden", "{body}");
        let e: Value = serde_json::from_str(body).expect("json");
        assert_eq!(e["error"]["code"], crate::vault_use_routes::OUT_OF_REACH, "{e}");
        assert_eq!((e["error"]["secret"].as_str(), e["error"]["from"].as_str(), e["error"]["as"].as_str()), (Some(name.as_str()), Some("server"), Some(want)), "{e}");
    }
    // The same from another workspace, by name.
    let b = json!({ "secret": uniq("NOT_THERE"), "scope": "workspace", "workspace": other.handle });
    let (s, body) = call(crate::vault_use_routes::FILE, &b);
    let e: Value = serde_json::from_str(&body).expect("json");
    assert_eq!((s.as_str(), &e["error"]["code"], e["error"]["from"].as_str()), ("403 Forbidden", &json!(crate::vault_use_routes::OUT_OF_REACH), Some(from_other.as_str())), "{e}");
    let b = json!({ "secret": theirs, "scope": "workspace", "workspace": other.handle });
    let (s2, body2) = call(crate::vault_use_routes::FILE, &b);
    let e2: Value = serde_json::from_str(&body2).expect("json");
    assert_eq!((s2.as_str(), &e2["error"]["code"]), ("403 Forbidden", &json!(crate::vault_use_routes::OUT_OF_REACH)), "{e2}");
    // Its own workspace's missing name is a plain not found, unchanged.
    let (s, body) = call(crate::vault_use_routes::FILE, &json!({ "secret": uniq("OWN_MISSING") }));
    let e: Value = serde_json::from_str(&body).expect("json");
    assert_eq!((s.as_str(), &e["error"]["code"]), ("404 Not Found", &json!("not_found")), "{e}");

    // The wall teaches the card.
    let refusal: Value = serde_json::from_str(&before[1].1).expect("json");
    let params = HashMap::new();
    let req = crate::permission::walls::Refused { status: 403, method: "POST", path: crate::vault_use_routes::FILE, params: &params, body: None, response: &refusal, workspace_id: "" };
    let p = crate::permission::walls::permission_for(&req).expect("teaches");
    assert_eq!(p["action"], USE_ACTION, "{p}");
    assert_eq!(p["command"], format!("k2 permission request vault.use as=file for=1h from=server secret={name} --why \"<what you need it for>\""), "{p}");
}

/// A card can't widen what the agent gets by asking: `from` is checked for
/// shape only, and never names its own workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_card_can_ask_for_is_fixed() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let _v = crate::vault::ensure_test_vault();
    let ws = seed("vuc");
    for (key, bad) in [("for", "7d"), ("for", "forever"), ("as", "api"), ("as", "proxy"), ("from", "session"), ("from", "ws:"), ("from", "engine:sew")] {
        let mut p = use_params("ANY", "server", "fetch", "1h");
        p[key] = json!(bad);
        let (s, e) = ask(&ws, USE_ACTION, p);
        assert_eq!((s.as_str(), &e["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{key}={bad}: {e}");
    }
    let (s, e) = ask(&ws, USE_ACTION, json!({ "secret": "ANY", "from": "server", "as": "fetch", "for": "1h", "to": "ws:*" }));
    assert_eq!((s.as_str(), &e["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "the grantee is never a value: {e}");
    let hint = ask_refused(&ws, USE_ACTION, use_params("ANY", &format!("ws:{}", ws.handle), "fetch", "1h"));
    assert!(hint.contains("vault.delivery"), "{hint}");
}

/// An agent can't answer its own card, another agent's, or grant itself
/// the secret some other way; a Member can't either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_cant_approve_its_own_card() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vud");
    let name = uniq("STRIPE");
    server_secret(&v, &name, Delivery::Fetch, Delivery::Env);
    assert!(fetch_with(&v, &name, &requester(&ws)).is_err(), "nothing yet");
    let asked = ask_ok(&ws, USE_ACTION, use_params(&name, "server", "fetch", "2h"));
    let (id, card) = (id_of(&asked), card_of(&asked));

    // The asking agent: its own token, whatever author the call claims.
    for answer in ["Allow", "Deny"] {
        let b = json!({ "addr": ws.handle, "id": card, "answer": answer });
        let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &passport_params(&ws), b.to_string().as_bytes(), "owner");
        let e: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!((r.status, &e["error"]["code"]), ("403 Forbidden", &json!("action_card_person_only")), "{answer}: {e}");
    }
    // A different agent, with the card's id.
    let other = seed("vue");
    let b = json!({ "addr": ws.handle, "id": card, "answer": "Allow" });
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &passport_params(&other), b.to_string().as_bytes(), "owner");
    assert!(r.status.starts_with("403") || r.status.starts_with("404"), "{} {}", r.status, r.body);
    // Nor can it run the card's apply with an agent's own actor: an Answerer
    // is a person by type, so the only agent route is the grant route itself.
    let b = json!({ "name": name, "scope": "server", "to": format!("ws:{}", ws.handle), "as": "fetch" }).to_string();
    let r = crate::vault_routes::handle_on(&v, crate::vault_routes::GRANT, Ok(agent_caller(&ws)), b.as_bytes(), &HashMap::new(), "test", None);
    assert!(!r.status.starts_with("200"), "an agent can't grant itself a server secret: {}", r.body);
    // A Member is below the floor.
    let member = login("vum", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    assert!(row(&id).is_answerable(), "still open");
    assert!(server_grants(&v, &name).is_empty(), "nothing granted by any of them");
    assert!(fetch_with(&v, &name, &requester(&ws)).is_err());
}

/// The whole path for a `fetch` grant: a person's click, the time box, and
/// the end of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_allow_grants_for_the_time_asked_and_it_ends() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (ws, other) = (seed("vun"), seed("vuo"));
    let name = uniq("STRIPE");
    server_secret(&v, &name, Delivery::Fetch, Delivery::Env);
    let asked = ask_ok(&ws, USE_ACTION, use_params(&name, "server", "fetch", "2h"));
    let card = card_of(&asked);
    let vw = view(&ws, &card);
    assert_eq!(vw["whatChanges"], format!("Let {} use {name} (server) as fetch for 2h.", ws.handle), "{vw}");
    assert_eq!(vw["duration"], "for 2h", "{vw}");
    assert_eq!(
        vw["consequences"],
        format!("It ends by itself 2h after you allow it. {h} can't read the value: K2 adds it to {h}'s calls. To end it sooner: Settings → Vault.", h = ws.handle),
        "{vw}"
    );
    assert_eq!(vw["proof"], json!({ "kind": "click" }), "fetch is a click: {vw}");

    // The owner taps Allow: a click.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "client": "phone" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let grants = server_grants(&v, &name);
    assert_eq!(grants.len(), 1, "{grants:?}");
    let g = &grants[0];
    assert_eq!((&g.consumer, g.delivery, g.granted_by.as_str()), (&k2_vault::Consumer::workspace(ws.id.clone()), Delivery::Fetch, "owner"));
    let until = g.expires_at.as_deref().expect("time-boxed");
    assert!(off_by(until, 2 * 3600) <= 60, "within a minute of now+2h: {until}");
    fetch_with(&v, &name, &requester(&ws)).expect("fetch works now");
    assert!(v.resolve(&Selector::user(Scope::server(), &name), k2_vault::Use::File, &requester(&ws)).is_err(), "fetch only: it can't read it");
    assert!(fetch_with(&v, &name, &session_of(&other, "other")).is_err(), "only the asking workspace");
    let turn = last_turn(&ws);
    assert!(turn.contains("approved by owner") && turn.contains(&format!("k2 vault fetch --secret {name} --scope server")), "{turn}");
    assert!(!turn.contains(std::str::from_utf8(USE_VALUE).unwrap()), "never the value");

    // Once.
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "409 Conflict");
    assert_eq!(server_grants(&v, &name)[0].id, g.id, "not granted twice");

    // Time passes: the same grant, its end now behind us.
    let past = k2_vault::GrantRequest {
        selector: Selector::user(Scope::server(), &name),
        consumer: k2_vault::Consumer::workspace(ws.id.clone()),
        delivery: Delivery::Fetch,
        env_name: None,
        file_name: None,
        hosts_narrowed: None,
        expires_at: Some((chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        version: None,
    };
    v.grant(past, &Actor::Owner).expect("the clock moves");
    assert!(fetch_with(&v, &name, &requester(&ws)).is_err(), "an expired grant gives nothing");

    // Other workspaces' secrets, by name.
    let (a2, a3) = (seed("vuf"), seed("vug"));
    let theirs = uniq("THEIRS");
    given_file(&v, &a3, &theirs);
    let asked = ask_ok(&a2, USE_ACTION, use_params(&theirs, &format!("ws:{}", a3.handle), "fetch", "1h"));
    let (s, a) = answer_as("owner", &a2, &card_of(&asked), json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    let g = v.grants(&Selector::user(Scope::workspace(a3.id.clone()), &theirs), &Actor::Owner).expect("grants");
    assert_eq!((g.len(), &g[0].consumer), (1, &k2_vault::Consumer::workspace(a2.id.clone())), "{g:?}");
}

/// A denied card gives nothing, and can't be answered into giving
/// something later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_card_grants_nothing() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vuh");

    // Denied: the person's own answer, by a person who could have allowed it.
    let denied = uniq("DENIED");
    server_secret(&v, &denied, Delivery::Fetch, Delivery::Env);
    let asked = ask_ok(&ws, USE_ACTION, use_params(&denied, "server", "fetch", "1h"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Deny", "reason": "not this one" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("denied")), "{a}");
    assert_eq!(row(&id).state.as_str(), "denied");
    assert!(server_grants(&v, &denied).is_empty(), "a Deny grants nothing");
    assert!(fetch_with(&v, &denied, &requester(&ws)).is_err());
    assert!(last_turn(&ws).contains("not this one"), "the agent hears why");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "409 Conflict", "a Deny is final: {a}");
    assert!(server_grants(&v, &denied).is_empty());
}

/// Nobody answered in time: nothing is granted, a late Allow does nothing,
/// and asking again is a new card a person still has to answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_card_grants_nothing() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vuq");
    let expired = uniq("EXPIRED");
    server_secret(&v, &expired, Delivery::Fetch, Delivery::Env);
    let asked = ask_ok(&ws, USE_ACTION, use_params(&expired, "server", "fetch", "1h"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let closed = crate::permission::answer::close_open(&[row(&id)], k2_core::permission::request::RequestState::Expired, "Expired unanswered.");
    assert_eq!(closed.len(), 1);
    assert_eq!(row(&id).state.as_str(), "expired");
    assert!(server_grants(&v, &expired).is_empty(), "expiring grants nothing");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert!(!s.starts_with("200"), "an expired card can't be answered: {s}: {a}");
    assert_eq!(row(&id).state.as_str(), "expired", "still expired");
    assert!(server_grants(&v, &expired).is_empty(), "nothing after a late Allow");
    assert!(fetch_with(&v, &expired, &requester(&ws)).is_err());
    // The agent can't re-open the same ask to reuse the old answer: asking
    // again is a fresh card, which a person still has to answer.
    let again = ask_ok(&ws, USE_ACTION, use_params(&expired, "server", "fetch", "1h"));
    assert_ne!(card_of(&again), card, "a new card");
    assert!(server_grants(&v, &expired).is_empty());
}

/// `file` and `env`: the grantee could read the value, so a person confirms,
/// bound to this card.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_readable_delivery_needs_a_confirm_and_stops_at_the_ceiling() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vui");
    let name = uniq("READABLE");
    server_secret(&v, &name, Delivery::File, Delivery::Env);
    let asked = ask_ok(&ws, USE_ACTION, use_params(&name, "server", "file", "1h"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let vw = view(&ws, &card);
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": SHARE_CONFIRM, "secretId": id }), "{vw}");
    assert!(vw["consequences"].as_str().unwrap().contains(&format!("{} can read the value until then, and so can anything it runs.", ws.handle)), "{vw}");
    assert_eq!(confirm_reason(SHARE_CONFIRM, Some(&id)), Some(Ok(format!("let {} read {name} (server) as file for 1h", ws.handle))));

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, "pr_some_other_card", "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "another card's confirm: {a}");
    assert!(row(&id).is_answerable(), "a refused confirm leaves the card open");
    assert!(server_grants(&v, &name).is_empty());
    // An agent can't get a confirm for it.
    for path in [crate::vault_routes::CONFIRM_START, crate::vault_routes::CONFIRM_FINISH] {
        let b = json!({ "action": SHARE_CONFIRM, "secretId": id, "method": "vault-password", "password": "x" }).to_string();
        let r = crate::vault_routes::handle_on(&v, path, Ok(agent_caller(&ws)), b.as_bytes(), &HashMap::new(), "test", None);
        assert!(r.status.starts_with("403"), "{path}: {} {}", r.status, r.body);
    }

    let token = confirm_for(&v, &id, "owner");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": token }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert!(v.confirm.take(&token, SHARE_CONFIRM, Some(&id), "owner").is_err(), "the Allow spent the token");
    let g = server_grants(&v, &name);
    assert_eq!((g.len(), g[0].delivery), (1, Delivery::File), "{g:?}");
    assert!(off_by(g[0].expires_at.as_deref().expect("time-boxed"), 3600) <= 60, "{g:?}");
    v.resolve(&Selector::user(Scope::server(), &name), k2_vault::Use::File, &requester(&ws)).expect("readable as a file now");
    assert!(v.resolve(&Selector::user(Scope::server(), &name), k2_vault::Use::Env, &requester(&ws)).is_err(), "not env");

    // Above the secret's ceiling: refused at the click, by the vault.
    let capped = uniq("CAPPED");
    server_secret(&v, &capped, Delivery::Fetch, Delivery::File);
    let asked = ask_ok(&ws, USE_ACTION, use_params(&capped, "server", "env", "1h"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": confirm_for(&v, &id, "owner") }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("failed")), "{a}");
    assert_eq!(row(&id).state.as_str(), "failed");
    assert!(server_grants(&v, &capped).is_empty(), "nothing above the ceiling");

    // A name that isn't there, allowed anyway (an old client): nothing.
    let gone = uniq("GONE");
    let asked = ask_ok(&ws, USE_ACTION, use_params(&gone, "server", "fetch", "1h"));
    let (id, card) = (id_of(&asked), card_of(&asked));
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("failed")), "{a}");
    assert_eq!(row(&id).result_message.as_deref(), Some(format!("K2 has no server secret {gone}; nothing to allow.").as_str()));
}

/// `for=session`: the grant is the asking session's, with an end even so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_card_reaches_only_the_session_that_asked() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vuj");
    let name = uniq("SESSION");
    server_secret(&v, &name, Delivery::Fetch, Delivery::Env);
    crate::session_liveness::note_spawning(&asking_session(&ws));

    let asked = ask_ok(&ws, USE_ACTION, use_params(&name, "server", "fetch", "session"));
    let card = card_of(&asked);
    let vw = view(&ws, &card);
    assert_eq!(vw["duration"], "until this session ends", "{vw}");
    assert!(vw["whatChanges"].as_str().unwrap().ends_with("as fetch until this session ends."), "{vw}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    let g = server_grants(&v, &name);
    assert_eq!((g.len(), &g[0].consumer), (1, &k2_vault::Consumer::session(asking_session(&ws))), "{g:?}");
    assert!(off_by(g[0].expires_at.as_deref().expect("a session card still ends"), 7 * 24 * 3600) <= 60, "{g:?}");
    fetch_with(&v, &name, &requester(&ws)).expect("the session that asked");
    assert!(fetch_with(&v, &name, &session_of(&ws, "another-session")).is_err(), "not another session of the workspace");

    // A session that isn't running gets nothing.
    let later = seed("vuk");
    let other_name = uniq("NOT_RUNNING");
    server_secret(&v, &other_name, Delivery::Fetch, Delivery::Env);
    let asked = ask_ok(&later, USE_ACTION, use_params(&other_name, "server", "fetch", "session"));
    let (s, a) = answer_as("owner", &later, &card_of(&asked), json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("failed")), "{a}");
    assert!(server_grants(&v, &other_name).is_empty());
}

/// A card never takes away access a person already gave.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_time_boxed_card_never_shortens_a_longer_grant() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let ws = seed("vul");
    let name = uniq("STANDING");
    server_secret(&v, &name, Delivery::Fetch, Delivery::Env);
    let standing = k2_vault::GrantRequest {
        selector: Selector::user(Scope::server(), &name),
        consumer: k2_vault::Consumer::workspace(ws.id.clone()),
        delivery: Delivery::Fetch,
        env_name: None,
        file_name: None,
        hosts_narrowed: None,
        expires_at: None,
        version: None,
    };
    let g = v.grant(standing, &Actor::Owner).expect("a standing grant");
    let asked = ask_ok(&ws, USE_ACTION, use_params(&name, "server", "fetch", "1h"));
    let (s, a) = answer_as("owner", &ws, &card_of(&asked), json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    let now = server_grants(&v, &name);
    assert_eq!((now.len(), now[0].id.as_str(), now[0].expires_at.as_deref()), (1, g.id.as_str(), None), "untouched: {now:?}");
    assert!(last_turn(&ws).contains("nothing changed"));
}
