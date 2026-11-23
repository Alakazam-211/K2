//! Settings → Getting started (plan-getting-started-settings-v1): `restart`
//! for the owner token, an Admin and a Member with the setup list off and
//! on, a Member's ask as a ticket and the yes to it, and Kessel's welcome
//! again from its lock only. A temp home and a per-thread database. Fail
//! loudly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use k2_core::connect_users::Role;
use k2_core::hire::starter_permissions::{standing_switch, KESSEL_STARTER_PERMISSIONS};
use serde_json::{json, Value};

use super::*;

struct Env {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
}

/// A temp home and database: the setup list and Kessel switches as given,
/// codex signed in.
fn env(list: bool, kessel: bool) -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    k2_core::app_settings::update(json!({ "setupChecklist": list, "kesselFirstRun": kessel })).expect("switches");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO llm_accounts (id, tool, label, kind, created_at, state) VALUES ('acct-codex', 'codex', 'work', 'subscription', 1, 'signed_in')",
            [],
        )
        .expect("sign in");
    }
    Env { _db: db, _home: home }
}

fn login(name: &str, role: Role) -> Actor {
    k2_core::connect_users::add_user(name, "password123").expect("add");
    if role != Role::Member {
        k2_core::connect_users::set_role(name, role).expect("role");
    }
    Actor::login(name)
}

fn json_of(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{}: {e}: {}", r.status, r.body))
}

fn code(r: &CliResponse) -> String {
    json_of(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("a code: {} {}", r.status, r.body)).to_string()
}

fn post(path: &str, body: Value, actor: &Actor) -> CliResponse {
    handle(true, path, body.to_string().as_bytes(), actor)
}

fn status_for(actor: &Actor) -> Value {
    let r = handle(false, STATUS_PATH, b"", actor);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json_of(&r)
}

fn switches() -> (bool, bool) {
    let s = k2_core::app_settings::load();
    (s.setup_checklist, s.kessel_first_run)
}

/// Every Access Audit line, as one string per line.
fn audit_lines() -> Vec<String> {
    k2_core::auth_audit::tail(1000).expect("audit").iter().map(|e| e.to_string()).collect()
}

fn audited(event: &str, who: &str, outcome: &str) -> bool {
    audit_lines().iter().any(|l| l.contains(event) && l.contains(who) && l.contains(outcome))
}

fn asks() -> Vec<k2_core::feedback::FeedbackItem> {
    k2_core::feedback::list_for_project(k2_core::db::AUDIT_SENTINEL_IDS[0], &k2_core::feedback::ListFilter::All)
        .expect("list")
        .into_iter()
        .filter(is_setup_ask)
        .collect()
}

fn ok() -> CliResponse {
    CliResponse::ok_json("{}".to_string())
}

/// A ticket write the person arm ran for `actor`, then the hook it calls.
fn ticket_write(path: &str, body: Value, actor: &Actor) -> CliResponse {
    let author = if actor.owner_token { "owner".to_string() } else { actor.login.clone() };
    let door = if actor.owner_token { crate::feedback_routes::CreateDoor::Owner } else { crate::feedback_routes::CreateDoor::Connect };
    let bytes = body.to_string().into_bytes();
    let r = crate::feedback_routes::dispatch_post_as_gated(path, &bytes, &author, None, door);
    after_ticket_write(path, &bytes, &r, actor);
    r
}

#[test]
fn restart_and_welcome_are_post_only_and_a_person_s() {
    for p in [RESTART_PATH, KESSEL_WELCOME_PATH] {
        assert!(is_route(p) && is_post_only(p), "{p}");
        assert!(!is_kessel_route(p), "{p} isn't behind kesselFirstRun");
        assert_eq!(refuse_outside_person_arm(p, false).expect("door").status, "405 Method Not Allowed", "{p}");
        let r = refuse_outside_person_arm(p, true).expect("door");
        assert_eq!(r.status, "403 Forbidden", "{p}: an agent's cell socket");
        assert_eq!(handle(false, p, b"", &Actor::owner_token()).status, "405 Method Not Allowed", "{p}");
    }
}

#[test]
fn the_owner_token_turns_the_list_on_and_kessel_with_kessel_true_audited_with_who() {
    let _e = env(false, false);
    let mut rx = crate::session_events::sender().subscribe();
    let r = post(RESTART_PATH, json!({}), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (true, false), "the list only, without kessel");
    let v = json_of(&r);
    assert_eq!((v["enabled"].clone(), v["kesselFirstRun"].clone()), (json!(true), json!(false)), "{v}");
    assert_eq!(v["kesselSeat"], json!({ "exists": false }), "{v}");
    assert!(audited("first-run-switch", "owner-token", "setupChecklist on"), "{:?}", audit_lines());
    let mut everyone = false;
    while let Ok(ev) = rx.try_recv() {
        if let crate::session_events::SessionEvent::FirstRunChanged { person: None } = ev {
            everyone = true;
        }
    }
    assert!(everyone, "the settings write sends first_run_changed to everyone");

    let r = post(RESTART_PATH, json!({ "kessel": true }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (true, true));
    assert!(audited("first-run-switch", "owner-token", "kesselFirstRun on"), "{:?}", audit_lines());
    let v = json_of(&r);
    assert_eq!(v["kesselFirstRun"], json!(true));
    assert_eq!(v["offer"], json!("meet_kessel"), "the Kessel part is sent now: {v}");

    for bad in [json!({ "kessel": "yes" }), json!({ "kessel": 1 }), json!([1])] {
        let r = post(RESTART_PATH, bad.clone(), &Actor::owner_token());
        assert_eq!((r.status, code(&r).as_str()), ("400 Bad Request", "usage"), "{bad}");
    }
}

#[test]
fn an_admin_turns_both_on_and_the_audit_names_them() {
    let _e = env(false, false);
    let julie = login("julie", Role::Admin);
    let r = post(RESTART_PATH, json!({ "kessel": true }), &julie);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (true, true));
    assert!(audited("first-run-switch", "julie", "setupChecklist on"), "{:?}", audit_lines());
    assert!(audited("first-run-switch", "julie", "kesselFirstRun on"), "{:?}", audit_lines());
    // Already on: nothing more is written or audited.
    let before = audit_lines().len();
    let r = post(RESTART_PATH, json!({ "kessel": true }), &julie);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(audit_lines().len(), before, "no switch flipped, no line");
}

#[test]
fn restart_clears_skips_hidden_finished_and_the_meet_kessel_dismissal() {
    let _e = env(true, true);
    let owner = Actor::owner_token();
    assert_eq!(post(SKIP_PATH, json!({ "row": "tunnel" }), &owner).status, "200 OK");
    assert_eq!(post(OFFER_DISMISS_PATH, json!({}), &owner).status, "200 OK");
    assert_eq!(post(HIDE_PATH, json!({}), &owner).status, "200 OK");
    let v = status_for(&owner);
    assert_eq!((v["hidden"].clone(), v["offer"].clone()), (json!(true), Value::Null), "{v}");
    // show keeps the "Not now"; restart clears it too.
    let v = json_of(&post(SHOW_PATH, json!({}), &owner));
    assert_eq!(v["offer"], Value::Null, "{v}");
    assert_eq!(post(SKIP_PATH, json!({ "row": "tunnel" }), &owner).status, "200 OK");
    let r = post(RESTART_PATH, json!({}), &owner);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_of(&r);
    assert_eq!((v["hidden"].clone(), v["finished"].clone()), (json!(false), json!(false)), "{v}");
    assert_eq!(v["offer"], json!("meet_kessel"), "{v}");
    let rows = v["rows"].as_array().expect("rows");
    assert!(rows.iter().all(|r| r["state"] != json!("skipped")), "{rows:?}");
}

#[test]
fn a_member_with_the_list_on_restarts_their_own_list_only() {
    let _e = env(true, false);
    let anna = login("anna", Role::Member);
    assert_eq!(post(HIDE_PATH, json!({}), &anna).status, "200 OK");
    assert_eq!(post(HIDE_PATH, json!({}), &Actor::owner_token()).status, "200 OK");
    let r = post(RESTART_PATH, json!({}), &anna);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json_of(&r)["hidden"], json!(false));
    assert_eq!(status_for(&Actor::owner_token())["hidden"], json!(true), "the owner's list is the owner's");
    assert!(asks().is_empty(), "nothing to ask");
    // kessel: true is the owner's or an admin's to turn on.
    let r = post(RESTART_PATH, json!({ "kessel": true }), &anna);
    assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "role_required"));
    assert_eq!(switches(), (true, false));
}

#[test]
fn a_member_with_the_list_off_asks_the_owner_on_one_ticket() {
    let _e = env(false, false);
    let anna = login("anna", Role::Member);
    let r = post(RESTART_PATH, json!({}), &anna);
    assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "role_required"), "{}", r.body);
    let v = json_of(&r);
    assert_eq!(v["asked"]["already"], json!(false), "{v}");
    let id = v["asked"]["ticketId"].as_str().expect("ticket").to_string();
    assert!(v["error"]["hint"].as_str().expect("hint").contains("asked"), "{v}");
    assert_eq!(switches(), (false, false), "a Member never turns it on");

    let t = k2_core::feedback::get_item(&id).expect("the ticket");
    assert_eq!(t.title, format!("anna{ASK_TITLE_TAIL}"));
    assert_eq!(t.assignees, vec!["owner".to_string()], "assigned to the owner");
    assert_eq!(t.options, Some(vec![START_AGAIN.to_string()]));
    assert_eq!((t.kind.as_str(), t.status.as_str(), t.agent_name.as_str()), ("question", "waiting", "anna"));
    assert!(t.body.as_deref().is_some_and(|b| b.contains(START_AGAIN)), "{:?}", t.body);
    assert!(audited("first-run-ask", "anna", &id), "{:?}", audit_lines());

    // A second press: already asked, the same ticket, none more.
    let r = post(RESTART_PATH, json!({}), &anna);
    assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "role_required"));
    let v = json_of(&r);
    assert_eq!((v["asked"]["already"].clone(), v["asked"]["ticketId"].clone()), (json!(true), json!(id)), "{v}");
    assert_eq!(asks().len(), 1);
    // Another Member's ask is their own.
    let gina = login("gina", Role::Member);
    let r = post(RESTART_PATH, json!({}), &gina);
    assert_eq!(json_of(&r)["asked"]["already"], json!(false));
    assert_eq!(asks().len(), 2);
}

#[test]
fn the_owner_resolving_the_ask_turns_the_list_on_and_starts_the_members_setup_again() {
    let _e = env(false, false);
    let anna = login("anna", Role::Member);
    // anna's list counts as finished on a server already in use.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT OR REPLACE INTO first_run_meta (id, existing_cutoff) VALUES (1, ?1)", [chrono::Utc::now().timestamp() + 60])
            .expect("cutoff");
    }
    assert_eq!(status_for(&anna)["finished"], json!(true));
    let id = json_of(&post(RESTART_PATH, json!({}), &anna))["asked"]["ticketId"].as_str().expect("ticket").to_string();

    // anna can't say yes to her own ask.
    let r = ticket_write("/cli/feedback/resolve", json!({ "id": id, "status": "resolved" }), &anna);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (false, false), "a Member's resolve turns nothing on");
    // Dismissed: no.
    ticket_write("/cli/feedback/resolve", json!({ "id": id, "status": "dismissed" }), &Actor::owner_token());
    assert_eq!(switches(), (false, false), "dismissed leaves it off");

    // The owner token marks it Resolved: yes.
    ticket_write("/cli/feedback/resolve", json!({ "id": id, "status": "waiting" }), &Actor::owner_token());
    let r = ticket_write("/cli/feedback/resolve", json!({ "id": id, "status": "resolved" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (true, false), "the list is on, Kessel untouched");
    let v = status_for(&anna);
    assert_eq!((v["enabled"].clone(), v["finished"].clone()), (json!(true), json!(false)), "anna's setup starts again: {v}");
    let (item, comments) = k2_core::feedback::get_with_comments(&id).expect("ticket");
    assert_eq!(item.status, "resolved");
    assert!(comments.iter().any(|c| c.body.contains("setup starts again")), "the closing note");
    assert!(audited("first-run-switch", "owner-token", "setupChecklist on"), "{:?}", audit_lines());
    assert!(audited("first-run-ask-yes", "owner-token", &id), "{:?}", audit_lines());

    // With the list on, anna's next press is her own restart.
    assert_eq!(post(RESTART_PATH, json!({}), &anna).status, "200 OK");
}

#[test]
fn an_admin_picking_start_setup_again_says_yes_and_another_answer_does_not() {
    let _e = env(false, false);
    let julie = login("julie", Role::Admin);
    let gina = login("gina", Role::Member);
    let id = json_of(&post(RESTART_PATH, json!({}), &gina))["asked"]["ticketId"].as_str().expect("ticket").to_string();
    // Another answer: nothing.
    k2_core::feedback::record_answer(&id, "Not yet").expect("answer");
    after_ticket_write("/cli/feedback/comment", json!({ "id": id }).to_string().as_bytes(), &ok(), &julie);
    assert_eq!(switches(), (false, false));
    // A refused write: nothing, even with the option recorded.
    k2_core::feedback::record_answer(&id, START_AGAIN).expect("answer");
    let refused = CliResponse { status: "400 Bad Request", content_type: "application/json", body: "{}".into() };
    after_ticket_write("/cli/feedback/comment", json!({ "id": id }).to_string().as_bytes(), &refused, &julie);
    assert_eq!(switches(), (false, false));
    // Another route (assign): nothing.
    after_ticket_write("/cli/feedback/assign", json!({ "id": id }).to_string().as_bytes(), &ok(), &julie);
    assert_eq!(switches(), (false, false));
    // The option, picked by an Admin: yes.
    after_ticket_write("/cli/feedback/comment", json!({ "id": id }).to_string().as_bytes(), &ok(), &julie);
    assert_eq!(switches(), (true, false));
    assert_eq!(k2_core::feedback::get_item(&id).expect("ticket").status, "resolved");
    assert!(audited("first-run-ask-yes", "julie", &id), "{:?}", audit_lines());
}

#[test]
fn a_ticket_that_only_looks_like_an_ask_is_not_one() {
    let _e = env(false, false);
    let julie = login("julie", Role::Admin);
    let t = crate::feedback_routes::file_person_ask(
        k2_core::db::AUDIT_SENTINEL_IDS[0],
        "anna",
        &format!("anna{ASK_TITLE_TAIL}"),
        "b",
        &["Yes".to_string()],
        &["owner".to_string()],
    )
    .expect("file");
    assert!(!is_setup_ask(&t), "another option");
    k2_core::feedback::set_status(&t.id, "resolved").expect("resolve");
    after_ticket_write("/cli/feedback/resolve", json!({ "id": t.id }).to_string().as_bytes(), &ok(), &julie);
    assert_eq!(switches(), (false, false));
}

// ── Kessel's welcome again ───────────────────────────────────────────────

/// Kessel made by the owner token: `(workspace id, path, handle)`.
fn make_kessel() -> (String, String, String) {
    let r = post(KESSEL_PATH, json!({ "tool": "codex" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_of(&r);
    (
        v["workspaceId"].as_str().expect("id").to_string(),
        v["path"].as_str().expect("path").to_string(),
        v["handle"].as_str().expect("handle").to_string(),
    )
}

/// Every file under `dir` but the template lock, with its bytes.
fn files_but_the_lock(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let lock = dir.join(k2_core::hire::template::LOCK_REL);
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("read_dir") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p != lock {
                out.insert(p.clone(), std::fs::read(&p).expect("read"));
            }
        }
    }
    out
}

/// K2's own bookkeeping markers in a workspace's `.k2/`, written by its
/// background regen whenever a workspace is touched (the AGENTS.md generate
/// flag, the skill regen stamp), on a timer of their own after Kessel is
/// made. They are not Kessel's notes, chats or permissions, and the welcome
/// never writes them, so the check leaves them out by name.
const K2_MARKERS: &[&str] = &[".agents-md-generate", ".last-skill-regen"];

fn is_k2_marker(rel: &str) -> bool {
    Path::new(rel).file_name().and_then(|n| n.to_str()).is_some_and(|n| K2_MARKERS.contains(&n))
}

/// Paths (relative to the folder's parent) that were added, removed or
/// changed between two snapshots, with which.
fn changed_files(before: &BTreeMap<PathBuf, Vec<u8>>, after: &BTreeMap<PathBuf, Vec<u8>>) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for (p, b) in before {
        match after.get(p) {
            None => out.push((p.to_string_lossy().into_owned(), "removed")),
            Some(a) if a != b => out.push((p.to_string_lossy().into_owned(), "changed")),
            Some(_) => {}
        }
    }
    for p in after.keys().filter(|p| !before.contains_key(*p)) {
        out.push((p.to_string_lossy().into_owned(), "added"));
    }
    out
}

fn hellos_for(handle: &str) -> Vec<String> {
    crate::template_hello::TEST_HELLOS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|(h, _, _)| h == handle)
        .map(|(_, hello, _)| hello.clone())
        .collect()
}

fn workspaces() -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).expect("count")
}

#[test]
fn no_kessel_is_404_no_kessel_and_a_member_is_refused() {
    let _e = env(true, true);
    let r = post(KESSEL_WELCOME_PATH, json!({}), &Actor::owner_token());
    assert_eq!((r.status, code(&r).as_str()), ("404 Not Found", "no_kessel"), "{}", r.body);
    let anna = login("anna", Role::Member);
    let r = post(KESSEL_WELCOME_PATH, json!({}), &anna);
    assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "role_required"));
}

#[test]
fn the_welcome_is_sent_once_per_call_from_the_lock_and_nothing_else_changes() {
    let _e = env(true, true);
    let (id, path, handle) = make_kessel();
    std::fs::create_dir_all(Path::new(&path).join(".k2/wiki")).expect("wiki");
    std::fs::write(Path::new(&path).join(".k2/wiki/Kessel's notes.md"), "# Notes\n\nThe bakery opens at six.\n").expect("note");
    let files = files_but_the_lock(Path::new(&path));
    let seat = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::orchestrator::load(&conn).expect("seat")
    };
    let count = workspaces();
    let before = k2_core::hire::template::lock::read(&path).expect("lock").expect("a lock");
    let sent_before = hellos_for(&handle).len();

    // Even with the switch off since: it replays a Kessel that exists.
    k2_core::app_settings::update(json!({ "kesselFirstRun": false })).expect("off");
    let r = post(KESSEL_WELCOME_PATH, json!({}), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json_of(&r), json!({ "ok": true, "workspaceId": id, "sent": true }));
    let sent = hellos_for(&handle);
    assert_eq!(sent.len(), sent_before + 1, "one welcome per call");
    let hello = sent.last().expect("sent").clone();
    assert!(!hello.trim().is_empty() && !hello.contains("{{"), "{hello}");

    let julie = login("julie", Role::Admin);
    let r = handle_as(true, KESSEL_WELCOME_PATH, b"", &julie, true);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(r.body.contains(&format!("thread {handle}")), "{}", r.body);
    assert_eq!(hellos_for(&handle).len(), sent_before + 2, "and again");
    assert_eq!(hellos_for(&handle).last(), Some(&hello), "the same words from the lock");

    let after = k2_core::hire::template::lock::read(&path).expect("lock").expect("a lock");
    assert!(after.hello_sent_at.is_some() && after.hello_pending.is_none(), "{after:?}");
    let strip = |l: &k2_core::hire::template::TemplateLock| k2_core::hire::template::TemplateLock {
        hello_pending: None,
        hello_sent_at: None,
        ..l.clone()
    };
    assert_eq!(strip(&after), strip(&before), "only the hello's fields moved");
    let changed = changed_files(&files, &files_but_the_lock(Path::new(&path)));
    eprintln!("welcome: files that differ from before, K2's own markers included: {changed:?}");
    let person_visible: Vec<&String> = changed.iter().filter(|(p, _)| !is_k2_marker(p)).map(|(p, _)| p).collect();
    assert!(person_visible.is_empty(), "notes and every other file untouched: {changed:?}");
    assert_eq!(workspaces(), count, "nothing made");
    let seat_after = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::orchestrator::load(&conn).expect("seat")
    };
    assert_eq!(seat_after, seat, "the seat (and its Chief of Staff) as it was");
    for a in KESSEL_STARTER_PERMISSIONS {
        assert!(crate::first_run_grants::switch_on(&id, standing_switch(a).expect("standing")), "{a} still on");
    }
    assert!(audited("first-run-kessel-welcome", "julie", &handle), "{:?}", audit_lines());
}

// ── An Admin turns the two switches on, never off, as themselves ─────────

fn settings_write(body: Value, actor: crate::settings_routes::SettingsActor, by: &str) -> CliResponse {
    crate::settings_routes::handle_settings_update_as(body.to_string().as_bytes(), actor, by)
}

#[test]
fn an_admin_may_only_turn_the_two_switches_on_through_the_settings_route() {
    use crate::settings_routes::SettingsActor;
    let _e = env(false, false);
    // Off, anything but true, or another Owner-only key beside them: 403,
    // nothing written.
    for bad in [
        json!({ "setupChecklist": false }),
        json!({ "kesselFirstRun": false }),
        json!({ "setupChecklist": "yes" }),
        json!({ "kesselFirstRun": 1 }),
        json!({ "setupChecklist": true, "airgap": true }),
        json!({ "kesselFirstRun": true, "listenLan": true }),
    ] {
        let r = settings_write(bad.clone(), SettingsActor::Admin, "user:julie");
        assert_eq!(r.status, "403 Forbidden", "{bad}: {}", r.body);
        assert_eq!(switches(), (false, false), "{bad}: nothing written");
    }
    let s = k2_core::app_settings::load();
    assert!(!s.airgap && !s.listen_lan, "no other Owner-only key moved");
    let r = settings_write(json!({ "setupChecklist": false }), SettingsActor::Member, "user:anna");
    assert_eq!(r.status, "403 Forbidden", "a Member never");
    let r = settings_write(json!({ "setupChecklist": true }), SettingsActor::Member, "user:anna");
    assert_eq!(r.status, "403 Forbidden", "a Member never, on either");

    // On: allowed, and the change names the Admin.
    let r = settings_write(json!({ "setupChecklist": true, "kesselFirstRun": true }), SettingsActor::Admin, "user:julie");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (true, true));
    assert!(audited("first-run-switch", "julie", "setupChecklist on"), "{:?}", audit_lines());
    assert!(audited("first-run-switch", "julie", "kesselFirstRun on"), "{:?}", audit_lines());
    // And never back off.
    for off in [json!({ "setupChecklist": false }), json!({ "kesselFirstRun": false })] {
        let r = settings_write(off.clone(), SettingsActor::Admin, "user:julie");
        assert_eq!(r.status, "403 Forbidden", "{off}");
        assert!(r.body.contains("only turn"), "{}", r.body);
    }
    assert_eq!(switches(), (true, true));
    // The Owner still may, and is named.
    let r = settings_write(json!({ "setupChecklist": false }), SettingsActor::Owner, "owner-token");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(audited("first-run-switch", "owner-token", "setupChecklist off"), "{:?}", audit_lines());
}

/// The restart route only ever writes these two keys, only `true`; every
/// other field of its body is ignored (only `kessel` is read).
#[test]
fn the_restart_route_writes_only_the_two_switches_on_and_ignores_the_rest_of_the_body() {
    let _e = env(false, false);
    let julie = login("julie", Role::Admin);
    let r = post(
        RESTART_PATH,
        json!({ "kessel": true, "airgap": true, "listenLan": true, "setupChecklist": false, "kesselFirstRun": false, "apiEnabled": true }),
        &julie,
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (true, true), "on, whatever else the body said");
    let s = k2_core::app_settings::load();
    assert!(!s.airgap && !s.listen_lan && !s.api_enabled, "nothing else moved");
    // Never off through the route: kessel false leaves Kessel on.
    let r = post(RESTART_PATH, json!({ "kessel": false }), &julie);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(switches(), (true, true));
    // The change record names the Admin, not the Owner.
    let lines: Vec<String> = audit_lines().into_iter().filter(|l| l.contains("first-run-switch")).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines.iter().all(|l| l.contains("julie") && !l.contains("owner")), "{lines:?}");
}

#[test]
fn a_passport_never_reaches_restart_or_welcome() {
    for p in [RESTART_PATH, KESSEL_WELCOME_PATH] {
        assert!(!crate::session_token::is_agent_verb(p), "{p}: an agent's passport is on the first-run denylist");
        let r = refuse_outside_person_arm(p, true).expect("door");
        assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "first_run_person_only"), "{p}");
    }
}

// ── The one click on the ticket ──────────────────────────────────────────

#[test]
fn the_ask_says_who_may_pick_its_option_and_other_tickets_say_nothing() {
    let _e = env(false, false);
    let anna = login("anna", Role::Member);
    let id = json_of(&post(RESTART_PATH, json!({}), &anna))["asked"]["ticketId"].as_str().expect("ticket").to_string();
    let show = |id: &str| {
        let params: std::collections::HashMap<String, String> = [("id".to_string(), id.to_string())].into_iter().collect();
        let r = crate::feedback_routes::dispatch("/cli/feedback/show", &params).expect("show");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        json_of(&r)
    };
    assert_eq!(show(&id)["optionRoles"], json!({ START_AGAIN: ["owner", "admin"] }));
    let other = crate::feedback_routes::file_person_ask(
        k2_core::db::AUDIT_SENTINEL_IDS[0],
        "disk-watch",
        "Disk / 90% full",
        "b",
        &["Clean up".to_string()],
        &["owner".to_string()],
    )
    .expect("file");
    assert!(show(&other.id).get("optionRoles").is_none(), "an owner notice without roles keeps its options hidden");
}

/// The app's click is `feedback/comment` with `optionPick`: the comment
/// core records the option as the answer, then the person arm's hook runs
/// with the credential's actor. An Admin's pick names the Admin.
#[test]
fn an_admins_pick_in_the_app_turns_the_list_on_as_the_admin() {
    let _e = env(false, false);
    let julie = login("julie", Role::Admin);
    let anna = login("anna", Role::Member);
    let id = json_of(&post(RESTART_PATH, json!({}), &anna))["asked"]["ticketId"].as_str().expect("ticket").to_string();
    k2_core::feedback::record_answer(&id, START_AGAIN).expect("pick");
    let body = json!({ "id": id, "body": START_AGAIN, "optionPick": true }).to_string();
    after_ticket_write("/cli/feedback/comment", body.as_bytes(), &ok(), &julie);
    assert_eq!(switches(), (true, false));
    assert!(audited("first-run-switch", "julie", "setupChecklist on"), "{:?}", audit_lines());
    assert!(!audit_lines().iter().any(|l| l.contains("first-run-switch") && l.contains("owner")), "the Admin, never the Owner");
    assert_eq!(status_for(&anna)["finished"], json!(false));
}
