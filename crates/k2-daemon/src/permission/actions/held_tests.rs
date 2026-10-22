//! E2 tests: the held-request actions end to end on the real routes
//! (PRD rev 3.1 §13.2, §13.3). An agent asks through
//! `/cli/permission/request` (gate passed: `routes::handle_gated`), a
//! person answers through `POST /cli/thread/answer`, and the held item is
//! decided by the Settings route's own entry. Parity: each row's floor is
//! checked through both doors (the card and the route), and the stored
//! state is the route's own. No test wakes a real agent (turns go to
//! `inject`'s sink; App alerts to a no-op sink).

use std::collections::HashMap;

use k2_core::app_grace::requests as app_reqs;
use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::permission::catalog::{ConfirmAction, Floor, Proof};
use k2_core::permission::request::{PermissionRequest, RequestState};
use serde_json::{json, Value};

use super::super::{Answerer, Applied};
use crate::compute_routes::Who;
use crate::permission::{inject, present, routes, store};

// ── the kit (compute_tests.rs uses it too) ──────────────────────────

pub(crate) struct Ws {
    pub id: String,
    pub handle: String,
    pub conv: String,
}

pub(crate) fn seed(tag: &str) -> Ws {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, format!("/tmp/pc-e2-{handle}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

pub(crate) fn agent(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) }
}

pub(crate) fn ask(ws: &Ws, action: &str, params: Value) -> (String, Value) {
    let body = json!({ "action": action, "params": params, "why": "The owner asked me to finish this today" });
    let r = routes::handle_gated(routes::REQUEST, &agent(ws), true, body.to_string().as_bytes(), &HashMap::new());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

pub(crate) fn ask_ok(ws: &Ws, action: &str, params: Value) -> Value {
    let (s, v) = ask(ws, action, params);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["status"], "pending", "{v}");
    v
}

/// An ask the action's current-state check refuses (400 `bad_params`);
/// returns the hint.
pub(crate) fn ask_refused(ws: &Ws, action: &str, params: Value) -> String {
    let (s, v) = ask(ws, action, params);
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("bad_params")), "{v}");
    v["error"]["hint"].as_str().expect("hint").to_string()
}

pub(crate) fn answer_as(author: &str, ws: &Ws, card: &str, body: Value) -> (String, Value) {
    let mut b = json!({ "addr": ws.handle, "id": card });
    if let (Some(o), Some(e)) = (b.as_object_mut(), body.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), b.to_string().as_bytes(), author);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

pub(crate) fn below_floor(r: (String, Value), required: &str) {
    let (s, a) = r;
    assert_eq!(
        (s.as_str(), &a["error"]["code"], &a["error"]["required"]),
        ("403 Forbidden", &json!("action_card_person_only"), &json!(required)),
        "{a}"
    );
}

pub(crate) fn row(id: &str) -> PermissionRequest {
    store::get_by_display_id(id).expect("db").unwrap_or_else(|| panic!("no request {id}"))
}

pub(crate) fn view(ws: &Ws, card: &str) -> Value {
    let d = serde_json::to_value(k2_core::overlay::get_thread_doc(&ws.conv, card).expect("doc read").expect("card doc")).expect("doc json");
    d["choice"]["action"]["view"].clone()
}

pub(crate) fn card_of(v: &Value) -> String {
    v["card"].as_str().unwrap_or_else(|| panic!("no card in {v}")).to_string()
}

pub(crate) fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap_or_else(|| panic!("no id in {v}")).to_string()
}

pub(crate) fn login(name: &str, role: Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password123").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

pub(crate) fn last_turn(ws: &Ws) -> String {
    inject::sent_to(&ws.handle).last().cloned().expect("the agent got a turn")
}

/// `apply` run directly, as `who` (the route's own floor at the click).
pub(crate) fn apply_as(id: &str, who: &Answerer, token: Option<&str>) -> Applied {
    let r = row(id);
    let reg = present::find_action(&r.action).expect("registered");
    (reg.apply)(&present::ctx_of(&r), who, token)
}

pub(crate) fn refused_code(a: Applied) -> String {
    match a {
        Applied::Refused { code, .. } => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn now() -> i64 {
    k2_core::overlay::now_secs()
}

fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..10].to_string()
}

// ── the registry and the floors ─────────────────────────────────────

#[test]
fn the_held_actions_are_registered_with_the_code_floors_and_proofs() {
    let rows = [
        ("apps.approve", Floor::Owner, Proof::SettingsConfirm { action: ConfirmAction::AppApprove }, "/cli/app/approve"),
        ("mail.approve", Floor::Admin, Proof::Click, "/cli/mail/approvals/approve"),
        ("sources.approve", Floor::Admin, Proof::Click, "/cli/sources/approve"),
        ("garden.proposal.approve", Floor::GardenOwner, Proof::Click, "/cli/zen/proposals/approve"),
    ];
    for (id, floor, proof, route) in rows {
        let r = super::super::find(id).unwrap_or_else(|| panic!("{id} is registered"));
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.proof), (floor, proof), "{id}");
        // Every one of them is a one-time act on an existing held item.
        assert_eq!(r.spec.grant, k2_core::permission::catalog::Grant::OneTime, "{id}");
        assert!(matches!(r.spec.undo, k2_core::permission::catalog::Undo::None), "{id}");
        // The route has a POST row in the policy table.
        assert!(crate::routes::route_policy::lookup(route).and_then(|x| x.post).is_some(), "{route} has a POST row");
    }
    // The two Admin rows' floors live only in the route table: the card's
    // floor is that table's floor (parity, §6.4).
    use crate::routes::route_policy::Floor as RouteFloor;
    for route in ["/cli/mail/approvals/approve", "/cli/sources/approve", "/cli/sources/batch-approve"] {
        assert_eq!(crate::routes::route_policy::lookup(route).and_then(|x| x.post), Some(RouteFloor::Admin), "{route}");
    }
}

#[test]
fn rows_outside_v1_say_they_cant_be_requested_or_name_their_cover() {
    let ws = seed("e2nc");
    not_requestable(&ws, &["vault.incoming.accept"]);
}

/// Each id answers 403 `not_cardable`, ending "It can't be requested."
pub(crate) fn not_requestable(ws: &Ws, ids: &[&str]) {
    for id in ids {
        let (s, v) = ask(ws, id, json!({}));
        assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("not_cardable")), "{id}: {v}");
        assert!(v["error"]["hint"].as_str().expect("hint").ends_with("It can't be requested."), "{id}: {v}");
        assert_eq!(v["permission"]["requestable"], false, "{id}: {v}");
    }
}

#[test]
fn route_floor_reads_the_policy_table() {
    let route = "/cli/mail/approvals/approve";
    assert!(super::super::held::route_floor(route, &Answerer::OwnerToken).is_ok());
    assert!(super::super::held::route_floor(route, &Answerer::Login { user: "a".into(), role: "admin".into() }).is_ok());
    assert!(super::super::held::route_floor(route, &Answerer::Login { user: "o".into(), role: "owner".into() }).is_ok());
    for role in ["member", "", "nonsense"] {
        let r = super::super::held::route_floor(route, &Answerer::Login { user: "m".into(), role: role.into() });
        assert_eq!(refused_code(r.expect_err("below the floor")), "role_required", "{role}");
    }
    let r = super::super::held::route_floor("/cli/no/such/route", &Answerer::Login { user: "a".into(), role: "owner".into() });
    assert_eq!(refused_code(r.expect_err("no row: fail closed")), "route_unclassified");
}

// ── apps.approve ────────────────────────────────────────────────────

fn quiet_apps() {
    crate::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    // The same applier door_tests registers for this kind.
    fn ok(r: &app_reqs::AppRequest, by: &str) -> crate::apps::routes::R<Value> {
        Ok(json!({ "applied": r.kind, "by": by }))
    }
    crate::apps::approvals::register_applier(app_reqs::KIND_DB_RAW, ok);
}

/// A held App request filed by `ws`'s agent. `db_raw` has a test applier;
/// `overcommit` has none, so approving it fails to apply.
fn app_request(ws: &Ws, kind: &'static str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let by = format!("agent:{}", ws.handle);
    app_reqs::create(
        &conn,
        &app_reqs::NewRequest {
            kind,
            app_id: None,
            service_id: None,
            app_label: "shop",
            workspace_id: &ws.id,
            body: json!({ "n": uniq() }),
            diff: json!({ "lines": ["raw database access for shop"] }),
            summary: "raw database access for shop".into(),
            reasons: vec!["test".into()],
            reason: None,
            deployment_id: None,
            requested_by: &by,
            requester_workspace: Some(&ws.id),
            requester_session: None,
        },
        now(),
    )
    .expect("file the App request")
    .id
}

fn app_status(id: &str) -> app_reqs::AppRequest {
    let db = k2_core::db::shared();
    let conn = db.lock();
    app_reqs::get(&conn, id).expect("db").expect("the App request")
}

fn app_token(id: &str, stamp: &str) -> String {
    crate::apps::egress_routes::test_seams::vault_handle().confirm.mint(crate::apps::approvals::CONFIRM_ACTION, Some(id), stamp, "local-auth")
}

#[test]
fn apps_approve_runs_the_held_request_through_app_approvals_as_the_owner() {
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("e2ap");
    let id = app_request(&ws, app_reqs::KIND_DB_RAW);

    let v = ask_ok(&ws, "apps.approve", json!({ "request": id }));
    assert_eq!(v["id"], id.as_str(), "a held request keeps its own id: {v}");
    assert_eq!(v["approvers"], "owner");
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["requestId"], id.as_str());
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": "app-approve", "secretId": id }));
    assert_eq!(vw["whatChanges"], format!("Approve {}'s held App request {id}.", ws.handle));
    assert_eq!(vw["scope"], format!("request {id}"));
    assert_eq!(vw["grant"], "one-time");
    assert_eq!(vw["settingsPage"], "app-approvals");

    // An Admin is below the floor: at the card, and at the route itself.
    let admin = login("e2apa", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow", "confirmToken": app_token(&id, &format!("connect:{admin}")) })), "owner");
    let e = crate::apps::approvals::handle(
        "/cli/app/approve",
        &Who::Login { username: admin.clone(), role: Role::Admin },
        json!({ "request": id, "confirmToken": app_token(&id, &format!("connect:{admin}")) }).to_string().as_bytes(),
        &HashMap::new(),
    )
    .expect_err("an Admin login never approves");
    assert_eq!(e.code, "owner_only");
    let direct = apply_as(&id, &Answerer::Login { user: admin.clone(), role: "admin".into() }, Some(&app_token(&id, &format!("connect:{admin}"))));
    assert_eq!(refused_code(direct), "owner_only", "the route's own floor runs at the click");

    // No confirm, or one for another request: nothing is decided.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": app_token("w_someone_else", "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    assert!(row(&id).is_answerable(), "a refused confirm leaves the card open");
    assert_eq!(app_status(&id).status, app_reqs::STATUS_PENDING);

    // The owner, with the confirm bound to this request.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": app_token(&id, "owner"), "client": "phone" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let held = app_status(&id);
    assert_eq!(held.status, app_reqs::STATUS_APPROVED, "the App request is decided by the route");
    assert_eq!(held.decided_by.as_deref(), Some("owner"));
    assert_eq!(held.confirm_via.as_deref(), Some("local-auth"));
    assert_eq!(held.result, Some(json!({ "applied": "db_raw", "by": "owner" })));
    let r = row(&id);
    assert_eq!((r.state, r.answered_by.as_deref(), r.answered_via.as_deref()), (RequestState::Approved, Some("owner"), Some("phone")));
    assert_eq!(last_turn(&ws), format!("[permission:{id}] approved by owner: once, for request {id}. It applies now."));

    // Single use: a second answer applies nothing.
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": app_token(&id, "owner") }));
    assert_eq!(s, "409 Conflict");
}

#[test]
fn apps_approve_follows_the_app_request_as_settings_decides_it() {
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("e2as");
    let other = seed("e2ao");

    // Only the asking workspace's own requests, by their full id.
    let theirs = app_request(&other, app_reqs::KIND_DB_RAW);
    assert!(ask_refused(&ws, "apps.approve", json!({ "request": theirs })).contains("no held App request"));
    let mine = app_request(&ws, app_reqs::KIND_DB_RAW);
    assert!(ask_refused(&ws, "apps.approve", json!({ "request": &mine[..6] })).contains("no held App request"));

    // Approved in Settings while the card waits → "Already on"; Close
    // applies nothing.
    let v = ask_ok(&ws, "apps.approve", json!({ "request": mine }));
    crate::apps::approvals::handle(
        "/cli/app/approve",
        &Who::Owner,
        json!({ "request": mine, "confirmToken": app_token(&mine, "owner") }).to_string().as_bytes(),
        &HashMap::new(),
    )
    .expect("approved in Settings");
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Close" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&mine).state, RequestState::AlreadyDone);
    assert!(last_turn(&ws).starts_with(&format!("[permission:{mine}] already on:")), "{}", last_turn(&ws));
    // Asking about a request that is already approved: nothing is stored.
    let (s, v) = ask(&ws, "apps.approve", json!({ "request": mine }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");

    // Denied in Settings while the card waits → stale at the click.
    let denied = app_request(&ws, app_reqs::KIND_DB_RAW);
    let v = ask_ok(&ws, "apps.approve", json!({ "request": denied }));
    crate::apps::approvals::handle("/cli/app/deny", &Who::Owner, json!({ "request": denied, "reason": "not now" }).to_string().as_bytes(), &HashMap::new())
        .expect("denied in Settings");
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow", "confirmToken": app_token(&denied, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&denied).state, RequestState::Stale);
    assert_eq!(last_turn(&ws), format!("[permission:{denied}] stale: App request {denied} was denied meanwhile. Nothing was applied."));

    // Approved, but the App side fails to apply it → failed, said plainly.
    let broken = app_request(&ws, app_reqs::KIND_OVERCOMMIT);
    let v = ask_ok(&ws, "apps.approve", json!({ "request": broken }));
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow", "confirmToken": app_token(&broken, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "failed", "{a}");
    assert_eq!(row(&broken).state, RequestState::Failed);
    assert_eq!(app_status(&broken).status, app_reqs::STATUS_FAILED);
    let turn = last_turn(&ws);
    assert!(turn.starts_with(&format!("[permission:{broken}] failed: App request {broken} was approved, but applying it failed: ")), "{turn}");
}

// ── mail.approve ────────────────────────────────────────────────────

/// A send of `ws`'s held for approval (`send_after`: scheduled later).
fn held_mail(ws: &Ws, created: i64, send_after: Option<i64>) -> String {
    let id = format!("out_{}", uniq());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, to_json, subject, status, created_at, updated_at, send_after) \
         VALUES (?1, ?2, ?3, ?4, '[\"human@example.com\"]', 'Digest', 'pending', ?5, ?5, ?6)",
        rusqlite::params![id, ws.id, ws.handle, format!("bot@{}.example", ws.handle), created, send_after],
    )
    .expect("queue the send");
    id
}

fn mail_row(id: &str) -> (String, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT status, decided_by FROM mail_outbound WHERE id = ?1", rusqlite::params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("the send")
}

#[test]
fn mail_approve_releases_the_held_send_through_the_approvals_route() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("e2ml");
    let msg = held_mail(&ws, now(), Some(now() + 3600));
    let v = ask_ok(&ws, "mail.approve", json!({ "message": msg }));
    let id = id_of(&v);
    assert!(id.starts_with("pr_"), "{v}");
    assert_eq!(v["approvers"], "owner or admin");
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["proof"], json!({ "kind": "click" }));
    assert_eq!(vw["whatChanges"], format!("Send {}'s held email {msg}.", ws.handle));

    // A Member is below the floor at the card and at the route's table row.
    let member = login("e2mm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(refused_code(apply_as(&id, &Answerer::Login { user: member.clone(), role: "member".into() }, None)), "role_required");
    assert_eq!(mail_row(&msg).0, "pending", "nothing moved");

    let admin = login("e2ma", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(mail_row(&msg), ("scheduled".to_string(), Some(admin.clone())), "decided by the route, as the admin");
    assert_eq!(row(&id).state, RequestState::Approved);
    let turn = last_turn(&ws);
    assert!(turn.starts_with(&format!("[permission:{id}] approved by {admin}: once, for message {msg}. It leaves after ")), "{turn}");
}

#[test]
fn mail_approve_follows_the_queue() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("e2mq");
    let other = seed("e2mo");
    let theirs = held_mail(&other, now(), Some(now() + 3600));
    assert!(ask_refused(&ws, "mail.approve", json!({ "message": theirs })).contains("no held email"));
    let old = held_mail(&ws, now() - 8 * 86_400, None);
    assert!(ask_refused(&ws, "mail.approve", json!({ "message": old })).contains("more than 7 days"));

    // Rejected in Settings while the card waits → stale.
    let msg = held_mail(&ws, now(), Some(now() + 3600));
    let v = ask_ok(&ws, "mail.approve", json!({ "message": msg }));
    let id = id_of(&v);
    let r = crate::mail::routes_send::handle_approvals_deny(json!({ "id": msg, "note": "use the newsletter" }).to_string().as_bytes());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale);
    assert_eq!(last_turn(&ws), format!("[permission:{id}] stale: email {msg} was rejected meanwhile. Nothing was applied."));

    // Already released: nothing to ask.
    let done = held_mail(&ws, now(), Some(now() + 3600));
    let r = crate::mail::routes_send::handle_approvals_approve(json!({ "id": done }).to_string().as_bytes());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let (s, v) = ask(&ws, "mail.approve", json!({ "message": done }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
}

#[test]
fn mail_approve_with_no_mail_server_fails_loudly() {
    let _home = crate::test_support::TempHome::new();
    let _g = crate::mail::mail_server_test_lock();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_server WHERE id = 1", []).expect("clear the mail server");
    }
    let ws = seed("e2mf");
    let msg = held_mail(&ws, now(), None);
    let v = ask_ok(&ws, "mail.approve", json!({ "message": msg }));
    let id = id_of(&v);
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "failed", "{a}");
    assert_eq!(row(&id).state, RequestState::Failed);
    assert_eq!(mail_row(&msg).0, "failed", "the queue says so too");
    let turn = last_turn(&ws);
    assert!(turn.starts_with(&format!("[permission:{id}] failed: approved, but submission failed:")), "{turn}");
}

// ── sources.approve ─────────────────────────────────────────────────

fn spec_hash() -> String {
    format!("{:064}", 7)
}

/// The prefix fallback for `spec=`: the first 32 characters of the hash.
fn spec_arg() -> String {
    spec_hash()[..super::super::held::SPEC_PREFIX_MIN].to_string()
}

/// A source `ws`'s agent proposed.
fn proposed_source(ws: &Ws, secret: Option<&str>, lan: bool) -> String {
    let name = format!("e2src{}", uniq());
    let db = k2_core::db::shared();
    let c = db.lock();
    c.execute(
        "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, lan, secret_name, every_secs, \
         viewers, agent_access, keep, status, made_by, made_by_project, rev, created_at, updated_at) \
         VALUES (?1, ?2, ?2, '{\"fetch\":{\"targets\":[{\"key\":\"a\",\"url\":\"https://api.example.com/x\"}]}}', \
         ?3, '[\"https://api.example.com:443\"]', ?4, ?5, 60, 'member', 'values', 'values', 'proposed', ?6, ?7, 1, 0, 0)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), name, spec_hash(), i64::from(lan), secret, format!("agent:{}", ws.id), ws.id],
    )
    .expect("propose the source");
    name
}

fn source_row(name: &str) -> k2_core::sources::SourceRow {
    let db = k2_core::db::shared();
    let c = db.lock();
    k2_core::sources::store::get_by_name(&c, name).expect("db").expect("the source")
}

#[test]
fn sources_approve_activates_plain_sources_as_one_batch() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("e2sr");
    let (a, b) = (proposed_source(&ws, None, false), proposed_source(&ws, None, false));
    // The full specHash (a 64-hex content hash passes the value check).
    let v = ask_ok(&ws, "sources.approve", json!({ "source": [a, b], "spec": [spec_hash(), spec_hash()] }));
    let id = id_of(&v);
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["whatChanges"], format!("Approve {}'s data source {a}, {b}.", ws.handle));
    assert_eq!(vw["proof"], json!({ "kind": "click" }));

    let member = login("e2sm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(refused_code(apply_as(&id, &Answerer::Login { user: member, role: "member".into() }, None)), "role_required");

    let admin = login("e2sa", Role::Admin);
    let (s, r) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{r}");
    for name in [&a, &b] {
        let src = source_row(name);
        assert_eq!(src.status, k2_core::sources::spec::SourceStatus::Active, "{name}");
        assert_eq!(src.approved_by.as_deref(), Some(format!("login:{admin}").as_str()), "approved by the route, as the admin");
        assert_eq!(src.approved_spec_hash.as_deref(), Some(spec_hash().as_str()));
    }
    assert_eq!(row(&id).state, RequestState::Approved);
    assert_eq!(last_turn(&ws), format!("[permission:{id}] approved by {admin}: once, for data source {a}, {b}. K2 starts fetching all 2 now."));

    // Asking again about active sources: nothing to ask.
    let (s, v) = ask(&ws, "sources.approve", json!({ "source": a, "spec": spec_arg() }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
}

#[test]
fn sources_approve_takes_only_plain_unchanged_sources_of_ones_own() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("e2sp");
    let other = seed("e2so");
    let secret = proposed_source(&ws, Some("VERCEL_RO"), false);
    let lan = proposed_source(&ws, None, true);
    for name in [&secret, &lan] {
        let hint = ask_refused(&ws, "sources.approve", json!({ "source": name, "spec": spec_arg() }));
        assert!(hint.contains("Settings → Gardens → External Data") && hint.ends_with("It can't be requested."), "{hint}");
    }
    let theirs = proposed_source(&other, None, false);
    assert!(ask_refused(&ws, "sources.approve", json!({ "source": theirs, "spec": spec_arg() })).contains("no source"));
    let plain = proposed_source(&ws, None, false);
    assert!(ask_refused(&ws, "sources.approve", json!({ "source": plain })).contains("needs spec="), "spec= is required");
    assert!(ask_refused(&ws, "sources.approve", json!({ "source": plain, "spec": &spec_arg()[..16] })).contains("changed since it was asked"), "a short prefix is not enough");
    assert!(ask_refused(&ws, "sources.approve", json!({ "source": plain, "spec": [spec_arg(), spec_arg()] })).contains("one spec= per source="));
    assert!(ask_refused(&ws, "sources.approve", json!({ "source": plain, "spec": "1".repeat(32) })).contains("changed since it was asked"));
    assert!(ask_refused(&ws, "sources.approve", json!({ "source": plain, "spec": "1".repeat(64) })).contains("changed since it was asked"), "a full hash of another spec");

    // The prefix fallback still asks (and binds the same way): edited
    // after the ask → stale at the click; nothing is approved.
    let v = ask_ok(&ws, "sources.approve", json!({ "source": plain, "spec": spec_arg() }));
    let id = id_of(&v);
    {
        let db = k2_core::db::shared();
        let c = db.lock();
        c.execute("UPDATE sources SET spec_hash = ?2 WHERE name = ?1", rusqlite::params![plain, "1".repeat(64)]).expect("edit");
    }
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale);
    assert_eq!(source_row(&plain).status, k2_core::sources::spec::SourceStatus::Proposed);
}

// ── garden.proposal.approve ─────────────────────────────────────────

struct ZenEnv {
    _home: crate::test_support::TempHome,
    _db: k2_core::db::TestDbGuard,
}

fn zen_env() -> ZenEnv {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::zen::ZenFiles::local().setup().expect("zen setup");
    ZenEnv { _home: home, _db: db }
}

fn zen(path: &str, owner: bool, caller: &crate::sidecar_routes::Caller, body: Value) -> (String, Value) {
    let r = crate::zen_routes::handle(path, owner, caller, "test", &HashMap::new(), body.to_string().as_bytes());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

fn widget_folder(id: &str, body: &str) -> Value {
    json!([
        {"name": "manifest.json", "text": format!("{{\"schema\": 1, \"id\": \"{id}\", \"name\": \"Board\", \"caps\": [\"agents:read\"]}}")},
        {"name": "index.html", "text": format!("<!doctype html><p>{body}</p><script src=\"app.js\"></script>")},
        {"name": "app.js", "text": "console.log(1)\n"},
    ])
}

/// `ws`'s agent (no "Manage Gardens") proposes a new version of `widget`
/// (which the owner made first).
fn garden_proposal(ws: &Ws, widget: &str, body: &str) -> String {
    let agent = crate::sidecar_routes::Caller::Agent(crate::sidecar_routes::AgentCaller {
        project_id: ws.id.clone(),
        map_key: None,
        args: vec![],
        sandboxed: false,
    });
    let (s, v) = zen("/cli/zen/widget/folder/put", false, &agent, json!({ "widget": widget, "files": widget_folder(widget, body) }));
    assert_eq!(s, "200 OK", "{v}");
    v["proposal"]["id"].as_str().unwrap_or_else(|| panic!("a proposal: {v}")).to_string()
}

fn proposal_status(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::zen::agent_edit::get(&conn, id).expect("db").expect("the proposal").status
}

#[test]
fn garden_proposal_approve_is_the_gardens_owner_through_the_same_door_as_settings() {
    let _e = zen_env();
    let owner_c = crate::sidecar_routes::Caller::Owner;
    let (s, v) = zen("/cli/zen/widget/folder/put", true, &owner_c, json!({ "widget": "board", "files": widget_folder("board", "v1") }));
    assert_eq!(s, "200 OK", "{v}");
    let ws = seed("e2gp");
    let id = garden_proposal(&ws, "board", "v2");

    let v = ask_ok(&ws, "garden.proposal.approve", json!({ "proposal": id }));
    let pr = id_of(&v);
    assert_eq!(v["approvers"], "owner");
    let card = card_of(&v);
    let vw = view(&ws, &card);
    assert_eq!(vw["whatChanges"], format!("Apply {}'s Garden proposal {id}.", ws.handle));
    assert_eq!(vw["proof"], json!({ "kind": "click" }));
    assert_eq!(vw["approvers"]["floor"], "owner");

    // An Admin login is not the Garden's owner: at the card and in Settings.
    let admin = login("e2gpa", Role::Admin);
    below_floor(answer_as(&admin, &ws, &card, json!({ "answer": "Allow" })), "owner");
    let (s, v) = zen("/cli/zen/proposals/approve", false, &crate::sidecar_routes::Caller::Login { username: admin.clone() }, json!({ "id": id }));
    assert_eq!((s.as_str(), v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    assert_eq!(refused_code(apply_as(&pr, &Answerer::Login { user: admin, role: "admin".into() }, None)), "zen_local_only");
    assert_eq!(proposal_status(&id), "pending");

    // An Owner login (the phone, the web) approves it as the person.
    let owner = login("e2gpo", Role::Owner);
    let (s, a) = answer_as(&owner, &ws, &card, json!({ "answer": "Allow", "client": "phone" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_eq!(proposal_status(&id), "approved", "decided by the route");
    assert_eq!(row(&pr).state, RequestState::Approved);
    assert_eq!(last_turn(&ws), format!("[permission:{pr}] approved by {owner}: once, for proposal {id}. It is written now."));
}

#[test]
fn garden_proposal_approve_follows_the_queue() {
    let _e = zen_env();
    let owner_c = crate::sidecar_routes::Caller::Owner;
    let (s, v) = zen("/cli/zen/widget/folder/put", true, &owner_c, json!({ "widget": "deck", "files": widget_folder("deck", "v1") }));
    assert_eq!(s, "200 OK", "{v}");
    let ws = seed("e2gq");
    let other = seed("e2gr");
    let theirs = garden_proposal(&other, "deck", "theirs");
    assert!(ask_refused(&ws, "garden.proposal.approve", json!({ "proposal": theirs })).contains("no Garden proposal"));

    // Rejected in Settings while the card waits → stale.
    let id = garden_proposal(&ws, "deck", "v2");
    let v = ask_ok(&ws, "garden.proposal.approve", json!({ "proposal": id }));
    let pr = id_of(&v);
    let (s, r) = zen("/cli/zen/proposals/reject", true, &owner_c, json!({ "id": id }));
    assert_eq!(s, "200 OK", "{r}");
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&pr).state, RequestState::Stale);
    let status = proposal_status(&id);
    assert_ne!(status, "pending");
    assert_eq!(last_turn(&ws), format!("[permission:{pr}] stale: Garden proposal {id} was {status} meanwhile. Nothing was applied."));

    // Approved in Settings first: nothing to ask.
    let done = garden_proposal(&ws, "deck", "v3");
    let (s, r) = zen("/cli/zen/proposals/approve", true, &owner_c, json!({ "id": done }));
    assert_eq!(s, "200 OK", "{r}");
    let (s, v) = ask(&ws, "garden.proposal.approve", json!({ "proposal": done }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
}
