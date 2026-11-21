//! 0.46.17: an agent's held App request is a tappable permission card
//! (the owner's standing rule: a person who must change something gets a
//! one-tap card, never a Settings hunt or a terminal step).
//!
//! - every agent's hold raises K2's `apps.approve` card, permission cards
//!   (Beta) on or off, and says who it went to;
//! - `--assign <login>` (`?assign=`) sends it to those people only; a name
//!   that can't approve is refused before anything runs;
//! - Deny on the card denies the held request; a request decided in
//!   Settings, on the CLI or by expiry closes its card;
//! - an agent lists its own requests in every state, never another
//!   workspace's, and never approves or answers its own card;
//! - the card approves exactly the request it was raised for.

use std::collections::HashMap;

use k2_core::app_grace::requests as app_reqs;
use k2_core::connect_users::Role;
use k2_core::permission::request::RequestState;
use serde_json::{json, Value};

use crate::apps::{approvals, held_cards};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::permission::actions::held::tests::{
    agent, answer_as, app_request, app_status, app_token, below_floor, card_status, login, quiet_apps, row, seed, Ws,
};
use crate::permission::approvers;

/// `enrich_held` on a plain held answer for `id`, as `who` (the one
/// chokepoint every held `/cli/app/*` answer goes through).
fn enrich(who: &Who, id: &str) -> Value {
    let resp = CliResponse { status: "202 Accepted", content_type: "application/json", body: approvals::held_answer(&app_status(id)).to_string() };
    serde_json::from_str(&approvals::enrich_held(who, resp).body).expect("json")
}

fn card_id(out: &Value) -> String {
    out["card"]["card"].as_str().unwrap_or_else(|| panic!("a card went up: {out}")).to_string()
}

fn list(who: &Who, q: &[(&str, &str)]) -> Value {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    approvals::handle("/cli/app/approvals", who, b"", &params).unwrap_or_else(|e| panic!("list: {} {}", e.code, e.hint))
}

fn ids(v: &Value) -> Vec<String> {
    v["requests"].as_array().expect("requests").iter().map(|r| r["id"].as_str().expect("id").to_string()).collect()
}

fn assign(raw: &str) -> Result<Option<Vec<String>>, (String, u16)> {
    let p: HashMap<String, String> = [(held_cards::ASSIGN_PARAM.to_string(), raw.to_string())].into();
    held_cards::assign_param(&p).map_err(|e| (e.code.to_string(), e.status))
}

fn open_cards(ws: &Ws, id: &str) -> usize {
    crate::permission::store::open(Some(&ws.id))
        .expect("open")
        .iter()
        .filter(|q| q.action == held_cards::ACTION && q.params["request"] == id)
        .count()
}

#[test]
fn every_held_request_of_an_agent_gets_a_card_with_cards_off_and_says_who_it_went_to() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc1");
    let a = agent(&ws);
    assert!(!k2_core::permission::gate::enabled_for(&ws.id), "the Beta switch is off for this agent");
    let ada = login("hc1a", Role::Admin);
    let id = app_request(&ws, app_reqs::KIND_DB_RAW);

    let out = enrich(&a, &id);
    let c = &out["card"];
    assert_eq!((c["raised"].as_bool(), c["new"].as_bool(), c["assigned"].as_bool()), (Some(true), Some(true), Some(false)), "{out}");
    assert_eq!(c["id"], id.as_str(), "the card keeps the held request's own id: {out}");
    assert_eq!(c["thread"], ws.handle.as_str(), "on the agent's Thread: {out}");
    let to: Vec<String> = serde_json::from_value(c["routedTo"].clone()).expect("routedTo");
    assert!(to.contains(&ada), "every owner and admin: {out}");
    let where_ = c["where"].as_str().expect("where");
    assert!(where_.contains(&format!("permission card {id} is on {}'s Thread", ws.handle)), "{where_}");
    assert!(where_.contains("sent to every owner and admin") && where_.contains("pass --assign <login> to send it to one"), "{where_}");
    // The standard card: Allow / Deny, the App Approvals confirm bound to
    // this request.
    let card = card_id(&out);
    let vw = crate::permission::actions::held::tests::view(&ws, &card);
    assert_eq!(vw["requestId"], id.as_str());
    assert_eq!(vw["proof"], json!({ "kind": "confirm", "action": "app-approve", "secretId": id }));
    assert_eq!(vw["whatChanges"], format!("Approve {}'s held App request {id}.", ws.handle));

    // Asked again: the same card, nothing new.
    let again = enrich(&a, &id);
    assert_eq!((again["card"]["new"].as_bool(), card_id(&again)), (Some(false), card.clone()), "{again}");
    assert_eq!(open_cards(&ws, &id), 1);

    // A person's own hold raises no card (they're the approvers).
    let mine = app_request(&ws, app_reqs::KIND_DB_RAW);
    let p = enrich(&Who::Login { username: ada.clone(), role: Role::Admin }, &mine);
    assert_eq!(p["card"]["raised"], false, "{p}");
    assert_eq!(open_cards(&ws, &mine), 0);
}

#[test]
fn assign_sends_the_card_to_the_named_person_only() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc2");
    let a = agent(&ws);
    let ada = login("hc2a", Role::Admin);
    let bo = login("hc2b", Role::Admin);
    let id = app_request(&ws, app_reqs::KIND_DB_RAW);

    let names = assign(&ada).expect("an admin may be assigned");
    let out = held_cards::with_assign(names, || enrich(&a, &id));
    let c = &out["card"];
    assert_eq!((c["raised"].as_bool(), c["assigned"].as_bool()), (Some(true), Some(true)), "{out}");
    assert_eq!(c["routedTo"], json!([ada]), "only the named person: {out}");
    assert!(c["where"].as_str().expect("where").contains(&format!("sent to {ada}, as you asked (--assign)")), "{out}");
    let q = row(&id);
    assert_eq!(approvers::of(q.id.as_str()), Some(vec![ada.clone()]), "stored once, with the request");
    assert!(approvers::asked(q.id.as_str(), &crate::permission::actions::Answerer::Login { user: ada.clone(), role: "admin".into() }));
    assert!(!approvers::asked(q.id.as_str(), &crate::permission::actions::Answerer::Login { user: bo.clone(), role: "admin".into() }), "bo is not asked");
    // The --assign is the call's only: cleared after.
    assert_eq!(held_cards::assigned(), None);
    // The approvals list says where it went.
    let l = list(&a, &[("state", "all")]);
    let r = l["requests"].as_array().expect("rows").iter().find(|r| r["id"] == id.as_str()).expect("its own request").clone();
    assert_eq!((r["card"]["state"].as_str(), r["card"]["narrowed"].as_bool()), (Some("pending"), Some(true)), "{r}");
    assert_eq!(r["card"]["routedTo"][0]["login"], ada.as_str(), "{r}");
    // `owner` is the owner token's person.
    assert_eq!(assign("owner").expect("owner"), Some(vec!["owner".to_string()]));
    assert_eq!(assign(&format!(" {ada} , owner ")).expect("a list"), Some(vec![ada.clone(), "owner".to_string()]));
    assert_eq!(assign("  ").expect("blank"), None, "blank: the default");
}

#[test]
fn assign_refuses_a_name_that_cant_approve_before_anything_runs() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc3");
    let mel = login("hc3m", Role::Member);
    assert_eq!(assign("nobody-here").unwrap_err(), (approvers::code::UNKNOWN.to_string(), 400));
    assert_eq!(assign(&mel).unwrap_err(), (approvers::code::NOT_ELIGIBLE.to_string(), 400), "a Member can't approve");
    let many: Vec<String> = (0..=approvers::MAX_NAMES).map(|i| format!("u{i}")).collect();
    assert_eq!(assign(&many.join(",")).unwrap_err().0, approvers::code::BAD);
    // At the route: refused, nothing run, nothing filed.
    let before = approvals::pending_count();
    let p: HashMap<String, String> = [("assign".to_string(), "nobody-here".to_string())].into();
    for path in ["/cli/app/approvals", "/cli/app/key/mint"] {
        let r = crate::apps::grace::handle(path, agent(&ws), json!({ "app": "shop" }).to_string().as_bytes(), &p);
        assert_eq!(r.status, "400 Bad Request", "{path}: {}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], approvers::code::UNKNOWN, "{v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains("--assign") && !hint.contains("--to"), "the agent's own flag: {hint}");
    }
    let r = crate::apps::routes::handle("/cli/app/install", agent(&ws), json!({ "app": "shop" }).to_string().as_bytes(), &p);
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert_eq!(approvals::pending_count(), before, "nothing filed");
}

#[test]
fn deny_on_the_card_denies_the_held_request_and_below_the_floor_moves_nothing() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc4");
    let id = app_request(&ws, app_reqs::KIND_DB_RAW);
    let card = card_id(&enrich(&agent(&ws), &id));

    let mel = login("hc4m", Role::Member);
    below_floor(answer_as(&mel, &ws, &card, json!({ "answer": "Deny" })), "admin");
    assert_eq!(app_status(&id).status, app_reqs::STATUS_PENDING, "a Member denies nothing");

    let ada = login("hc4a", Role::Admin);
    let (s, a) = answer_as(&ada, &ws, &card, json!({ "answer": "Deny", "reason": "not this week" }));
    assert_eq!(s, "200 OK", "{a}");
    let held = app_status(&id);
    assert_eq!(held.status, app_reqs::STATUS_DENIED, "Deny on the card denies the request itself");
    assert_eq!((held.decided_by.as_deref(), held.decision_reason.as_deref()), (Some(format!("user:{ada}").as_str()), Some("not this week")));
    assert_eq!(row(&id).state, RequestState::Denied);
    assert_eq!(open_cards(&ws, &id), 0);
    // Approving it after: refused, it was decided.
    let e = approvals::handle(
        "/cli/app/approve",
        &Who::Owner,
        json!({ "request": id, "confirmToken": app_token(&id, "owner") }).to_string().as_bytes(),
        &HashMap::new(),
    )
    .expect_err("decided once");
    assert_eq!(e.status, 409, "{}", e.hint);
}

#[test]
fn a_request_decided_elsewhere_closes_its_card() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc5");
    let a = agent(&ws);

    // Approved on the CLI / in Settings.
    let ok = app_request(&ws, app_reqs::KIND_DB_RAW);
    let card = card_id(&enrich(&a, &ok));
    approvals::handle("/cli/app/approve", &Who::Owner, json!({ "request": ok, "confirmToken": app_token(&ok, "owner") }).to_string().as_bytes(), &HashMap::new())
        .expect("approved in Settings");
    let r = row(&ok);
    assert_eq!((r.state, r.answered_by.as_deref(), r.answered_via.as_deref()), (RequestState::Approved, Some("owner"), Some("app-approvals")));
    assert_ne!(card_status(&ws, &card), "pending", "no card stays answerable");
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": app_token(&ok, "owner") }));
    assert_eq!(s, "409 Conflict");

    // Withdrawn by its agent.
    let w = app_request(&ws, app_reqs::KIND_DB_RAW);
    let card = card_id(&enrich(&a, &w));
    approvals::handle("/cli/app/deny", &a, json!({ "request": w }).to_string().as_bytes(), &HashMap::new()).expect("withdrawn");
    assert_eq!(app_status(&w).status, app_reqs::STATUS_WITHDRAWN);
    assert_eq!(row(&w).state, RequestState::Cancelled);
    assert_ne!(card_status(&ws, &card), "pending");

    // Expired (seven days): closed as expired when the list sweeps it.
    let x = app_request(&ws, app_reqs::KIND_DB_RAW);
    let card = card_id(&enrich(&a, &x));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE app_widenings SET expires_at = 1 WHERE id = ?1", [&x]).expect("age it");
    }
    let _ = list(&Who::Owner, &[]);
    assert_eq!(app_status(&x).status, app_reqs::STATUS_EXPIRED);
    assert_eq!(row(&x).state, RequestState::Expired);
    assert_ne!(card_status(&ws, &card), "pending");
}

#[test]
fn an_agent_sees_only_its_own_requests_in_every_state_and_never_approves() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc6");
    let other = seed("hc6o");
    let a = agent(&ws);
    let b = agent(&other);
    let mine = app_request(&ws, app_reqs::KIND_DB_RAW);
    let theirs = app_request(&other, app_reqs::KIND_DB_RAW);
    // `other`'s agent asking on an App of `ws`'s workspace: `other`'s own
    // request, never `ws`'s to see (before 0.46.17 the App's workspace saw it).
    let cross = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let by = format!("agent:{}", other.handle);
        app_reqs::create(
            &conn,
            &app_reqs::NewRequest {
                kind: app_reqs::KIND_DB_RAW,
                app_id: None,
                service_id: None,
                app_label: "shop",
                workspace_id: &ws.id,
                body: json!({ "cross": true }),
                diff: json!({ "lines": ["raw database access for shop"] }),
                summary: "raw database access for shop".into(),
                reasons: vec!["test".into()],
                reason: None,
                deployment_id: None,
                requested_by: &by,
                requester_workspace: Some(&other.id),
                requester_session: None,
            },
            k2_core::overlay::now_secs(),
        )
        .expect("file")
        .id
    };
    approvals::handle("/cli/app/deny", &Who::Owner, json!({ "request": mine, "reason": "no" }).to_string().as_bytes(), &HashMap::new()).expect("denied");

    // Pending by default; every state with state=all.
    assert!(ids(&list(&a, &[])).is_empty(), "nothing of its own pending");
    let all = list(&a, &[("state", "all")]);
    assert_eq!(ids(&all), vec![mine.clone()], "its own, denied: {all}");
    assert_eq!(all["requests"][0]["status"], "denied");
    assert_eq!(all["canApprove"], false);
    assert_eq!(ids(&list(&a, &[("state", "denied")])), vec![mine.clone()]);
    let mut theirs_all = ids(&list(&b, &[("state", "all")]));
    theirs_all.sort();
    let mut want = vec![theirs.clone(), cross.clone()];
    want.sort();
    assert_eq!(theirs_all, want, "the other agent: what it asked, wherever");
    // One by id: its own only; another's reads as no such request.
    assert_eq!(ids(&list(&a, &[("request", &mine)])), vec![mine.clone()]);
    for id in [&theirs, &cross] {
        let p: HashMap<String, String> = [("request".to_string(), id.to_string())].into();
        let e = approvals::handle("/cli/app/approvals", &a, b"", &p).expect_err("not its own");
        assert_eq!((e.code, e.status), ("request_not_found", 404), "{}", e.hint);
    }
    // A person sees all.
    let owner = ids(&list(&Who::Owner, &[("state", "all")]));
    for id in [&mine, &theirs, &cross] {
        assert!(owner.contains(id), "the owner sees {id}");
    }

    // Never approves: its own or another's, with or without a confirm.
    for id in [&theirs, &cross] {
        let e = approvals::handle("/cli/app/approve", &b, json!({ "request": id, "confirmToken": app_token(id, "owner") }).to_string().as_bytes(), &HashMap::new())
            .expect_err("an agent never approves");
        assert_eq!(e.code, "owner_only");
    }
    assert_eq!(app_status(&theirs).status, app_reqs::STATUS_PENDING);
    // Never denies another's.
    let e = approvals::handle("/cli/app/deny", &a, json!({ "request": theirs }).to_string().as_bytes(), &HashMap::new()).expect_err("not its own");
    assert_eq!(e.code, "request_not_found");
}

#[test]
fn an_agent_never_answers_its_own_card() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc7");
    let id = app_request(&ws, app_reqs::KIND_DB_RAW);
    let card = card_id(&enrich(&agent(&ws), &id));
    let passport: HashMap<String, String> = [
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
        ("project_id".to_string(), ws.id.clone()),
        ("from".to_string(), ws.id.clone()),
    ]
    .into();
    for answer in ["Allow", "Deny"] {
        let body = json!({ "addr": ws.handle, "id": card, "answer": answer, "confirmToken": app_token(&id, "owner") }).to_string();
        let r = crate::overlay_routes::dispatch_post("/cli/thread/answer", &passport, body.as_bytes());
        assert_eq!(r.status, "403 Forbidden", "{answer}: {}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "action_card_person_only", "{v}");
    }
    assert_eq!(app_status(&id).status, app_reqs::STATUS_PENDING, "the agent moved nothing");
    assert!(row(&id).is_answerable());
}

#[test]
fn the_card_approves_exactly_the_request_it_was_raised_for() {
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    quiet_apps();
    let ws = seed("hc8");
    let id = app_request(&ws, app_reqs::KIND_DB_RAW);
    let other = app_request(&ws, app_reqs::KIND_DB_RAW);
    let card = card_id(&enrich(&agent(&ws), &id));

    // A confirm for another request never approves this one.
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": app_token(&other, "owner") }));
    assert_eq!((s.as_str(), &a["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{a}");
    // The request swapped behind the card (its stored value rewritten):
    // stale at the click, neither request moves.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n = conn
            .execute(
                "UPDATE permission_requests SET params = ?2 WHERE held_ref = ?1",
                rusqlite::params![id, json!({ "request": other }).to_string()],
            )
            .expect("swap");
        assert_eq!(n, 1);
    }
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow", "confirmToken": app_token(&other, "owner") }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&id).state, RequestState::Stale, "the words it was asked on changed");
    assert_eq!((app_status(&id).status.as_str(), app_status(&other).status.as_str()), (app_reqs::STATUS_PENDING, app_reqs::STATUS_PENDING));
}
