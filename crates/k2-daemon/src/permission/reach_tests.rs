//! A2 reach tests (PRD rev 3.1 §6.5, §6.7, §13.3, §17.5, §17.7, §17.8):
//! the targeted push and its budget, the Tickets item, notes, "Turn off
//! again" and "Waiting on you", on A1's test-kit actions.
//!
//! The gate is passed (A1's `routes::handle_gated`, and the note/undo
//! handlers below `overlay_routes`' switch check): lane B's rule, no
//! `set_server_switch` in lib tests. The switch and the person refusals at
//! the route are in `tests/permission_reach_integration.rs`. No test wakes
//! a real agent (turns go to `inject`'s sink); pushes go to
//! `push_routes::test_push_capture`.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay;
use k2_core::permission::request::{PermissionRequest, RequestState};
use k2_core::push::{PushEvent, ThreadBudget, ThreadMsgKind, ThreadPush};
use serde_json::{json, Value};

use crate::compute_routes::Who;
use crate::permission::actions::Answerer;
use crate::permission::notes::{self, CardCtx};
use crate::permission::routes::{self, handle_gated};
use crate::permission::{inject, store, testkit, undo, waiting};
use crate::push_routes::test_push_capture;

struct Ws {
    id: String,
    handle: String,
    conv: String,
}

fn seed(tag: &str) -> Ws {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        rusqlite::params![id, handle, format!("/tmp/pc-a2-{handle}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    Ws { id, handle, conv }
}

fn agent(ws: &Ws) -> Who {
    Who::Agent { project_id: ws.id.clone(), session_id: format!("sess-{}", &ws.id[..8]) }
}

fn ask(ws: &Ws, action: &str) -> Value {
    ask_with(ws, action, json!({}))
}

fn ask_with(ws: &Ws, action: &str, values: Value) -> Value {
    let body = json!({ "action": action, "why": "Run the suite on the minis", "params": values });
    let r = handle_gated(routes::REQUEST, &agent(ws), true, body.to_string().as_bytes(), &HashMap::new());
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body));
    assert_eq!(r.status, "200 OK", "{v}");
    assert_eq!(v["status"], "pending", "{v}");
    v
}

fn card_of(v: &Value) -> String {
    v["card"].as_str().unwrap_or_else(|| panic!("no card in {v}")).to_string()
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap_or_else(|| panic!("no id in {v}")).to_string()
}

fn answer_as(author: &str, ws: &Ws, card: &str, body: Value) -> (String, Value) {
    let mut b = json!({ "addr": ws.handle, "id": card });
    for (k, v) in body.as_object().expect("object") {
        b[k] = v.clone();
    }
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), b.to_string().as_bytes(), author);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn row(id: &str) -> PermissionRequest {
    store::get_by_display_id(id).expect("db").unwrap_or_else(|| panic!("no request {id}"))
}

fn doc(ws: &Ws, card: &str) -> Value {
    serde_json::to_value(overlay::get_thread_doc(&ws.conv, card).expect("doc read").expect("card doc")).expect("doc json")
}

fn login(name: &str, role: Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password123").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

fn person(user: &str, role: &str) -> Answerer {
    Answerer::Login { user: user.to_string(), role: role.to_string() }
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn ctx<'a>(ws: &'a Ws, card: &'a str) -> CardCtx<'a> {
    CardCtx { conversation_id: &ws.conv, card_id: card, batch: false }
}

fn note(ws: &Ws, card: &str, who: &Answerer, text: &str) -> (String, Value) {
    let r = notes::handle(&params(&[("text", text)]), &ctx(ws, card), who);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn undo_as(ws: &Ws, card: &str, who: &Answerer) -> (String, Value) {
    let r = undo::handle(&HashMap::new(), &ctx(ws, card), who);
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn audit_lines(home: &crate::test_support::TempHome, needle: &str) -> Vec<String> {
    let audit = std::fs::read_to_string(home.path().join(".k2/auth-audit.jsonl")).unwrap_or_default();
    audit.lines().filter(|l| l.contains(needle)).map(str::to_string).collect()
}

/// The permission pushes for this conversation since `mark`.
fn card_pushes(mark: usize, conv: &str) -> Vec<ThreadPush> {
    test_push_capture::since(mark)
        .into_iter()
        .filter_map(|e| match e {
            PushEvent::ThreadPost(t) if t.conversation_id == conv => Some(t),
            _ => None,
        })
        .collect()
}

fn tickets(ws: &Ws) -> Vec<k2_core::feedback::FeedbackItem> {
    k2_core::feedback::list_for_project(&ws.id, &k2_core::feedback::ListFilter::All).expect("tickets")
}

// ── the push (§6.7, §17.7) ──────────────────────────────────────────

#[test]
fn a_card_is_pushed_to_the_people_who_may_answer_it_only() {
    let _home = crate::test_support::TempHome::new();
    let admin = login("pcadm", Role::Admin);
    let member = login("pcmem", Role::Member);
    let owner_login = login("pcown", Role::Owner);
    for u in [&admin, &member, &owner_login] {
        k2_core::push_devices::upsert_device(&format!("dev-{u}"), "apns", &format!("tok-{u}"), u).expect("device");
    }
    let ws = seed("pcpush");
    let mark = test_push_capture::mark();
    let v = ask(&ws, "test.switch");
    let id = id_of(&v);
    let pushes = card_pushes(mark, &ws.conv);
    assert!(!pushes.is_empty(), "the card was pushed");
    let mut to: Vec<&str> = pushes.iter().map(|t| t.principal.as_str()).filter(|p| *p != "owner").collect();
    to.sort_unstable();
    let mut want = vec![admin.as_str(), owner_login.as_str()];
    want.sort_unstable();
    assert_eq!(to, want, "admin-floor card: the Owner and Admin logins (plus the owner person), never the Member");
    for t in &pushes {
        assert!(!t.device_usernames.is_empty(), "never an empty list: {t:?}");
        assert!(!t.device_usernames.contains(&member), "a Member's phone never: {t:?}");
        assert_eq!(t.budget, ThreadBudget::Permission { request_id: id.clone(), setting: "Allow test things".into() });
        assert_eq!(t.msg_kind, ThreadMsgKind::Choice);
        assert_eq!((t.count, t.preview.as_str(), t.msg.is_none()), (1, "", true), "no --why, note or text rides it");
        let e = PushEvent::ThreadPost(t.clone());
        assert_eq!(e.title(), format!("{} asks for a permission", ws.handle));
        assert_eq!(e.body(), "Allow test things");
        assert_eq!(e.data().get("requestId"), Some(id.as_str()));
        assert_eq!(e.data().get("permission"), Some("1"));
        assert_eq!(e.data().get("kind"), Some("thread"));
        assert_eq!(e.budget_class(), k2_core::push::budget::BudgetClass::Permission);
    }
    assert!(
        pushes.iter().all(|t| matches!(t.budget, ThreadBudget::Permission { .. })),
        "a permission card never takes the agent-post push (push_agent_post)"
    );

    // An Owner-floor card: no Admin.
    let ws2 = seed("pcpusho");
    let mark = test_push_capture::mark();
    ask(&ws2, "test.owner-switch");
    let to: Vec<String> = card_pushes(mark, &ws2.conv).into_iter().map(|t| t.principal).collect();
    assert!(to.contains(&owner_login), "{to:?}");
    assert!(!to.contains(&admin) && !to.contains(&member), "owner floor: {to:?}");
}

#[test]
fn the_card_budget_is_30_an_hour_per_person_and_apart_from_thread_posts() {
    let who = format!("pcbudget-{}", uuid::Uuid::new_v4().simple());
    let conv = format!("conv-{who}");
    let push = |budget: ThreadBudget| ThreadPush {
        agent_name: "billing".into(),
        conversation_id: conv.clone(),
        addr: "billing".into(),
        seq: 1,
        project_id: "p".into(),
        msg_kind: ThreadMsgKind::Choice,
        count: 1,
        principal: who.clone(),
        device_usernames: vec![who.clone()],
        unread_total: "0".into(),
        waiting_total: "0".into(),
        avatar_rev: String::new(),
        agent_color: String::new(),
        preview: String::new(),
        msg: None,
        budget,
    };
    let card = ThreadBudget::Permission { request_id: "pr_abcdef".into(), setting: "Allow compute nodes".into() };
    let mark = test_push_capture::mark();
    for _ in 0..31 {
        crate::push_routes::deliver_thread(PushEvent::ThreadPost(push(card.clone())));
    }
    assert_eq!(card_pushes(mark, &conv).len(), 30, "the 31st card push this hour is dropped");
    crate::push_routes::deliver_thread(PushEvent::ThreadPost(push(ThreadBudget::Thread)));
    let all = card_pushes(mark, &conv);
    assert_eq!(all.len(), 31, "a Thread post still goes: the coalescer's bucket is its own");
    assert_eq!(all.last().map(|t| t.budget.clone()), Some(ThreadBudget::Thread));
    let other = format!("{who}-2");
    let mark = test_push_capture::mark();
    crate::push_routes::deliver_thread(PushEvent::ThreadPost(ThreadPush { principal: other.clone(), ..push(card) }));
    assert_eq!(card_pushes(mark, &conv).len(), 1, "another person's budget is untouched");
}

// ── the Tickets item (§6.7, C26, C28) ───────────────────────────────

#[test]
fn the_tickets_item_goes_to_eligible_logins_when_nobody_was_looking() {
    let _home = crate::test_support::TempHome::new();
    let admin = login("pctadm", Role::Admin);
    let member = login("pctmem", Role::Member);
    let owner_login = login("pctown", Role::Owner);
    let ws = seed("pctick");
    let v = ask(&ws, "test.switch");
    let t = tickets(&ws);
    assert_eq!(t.len(), 1, "one Tickets item: {t:?}");
    assert_eq!(t[0].kind, "fyi");
    assert_eq!(t[0].title, format!("{} asks for a permission: Allow test things", ws.handle));
    let body = t[0].body.clone().unwrap_or_default();
    assert!(body.contains(&id_of(&v)) && !body.contains("Run the suite"), "K2's words and the id, never --why: {body}");
    let mut assignees = t[0].assignees.clone();
    assignees.sort();
    let mut want = vec![admin.clone(), owner_login.clone()];
    want.sort();
    assert_eq!(assignees, want, "eligible logins only, never {member}");

    // Owner floor: Owner logins only.
    let ws2 = seed("pcticko");
    ask(&ws2, "test.owner-switch");
    assert_eq!(tickets(&ws2)[0].assignees, vec![owner_login.clone()]);

    // Someone who may answer looked at the Thread a minute ago: no item.
    let ws3 = seed("pctickv");
    let sock = crate::thread_viewers::next_socket_id();
    crate::thread_viewers::global().set(sock, &admin.to_ascii_lowercase(), &ws3.conv, true, std::time::Instant::now());
    crate::thread_viewers::global().set(sock, &admin.to_ascii_lowercase(), &ws3.conv, false, std::time::Instant::now());
    ask(&ws3, "test.switch");
    assert!(tickets(&ws3).is_empty(), "an approver saw the Thread within 10 minutes");

    // A Member looking doesn't count.
    let ws4 = seed("pctickm");
    let sock = crate::thread_viewers::next_socket_id();
    crate::thread_viewers::global().set(sock, &member.to_ascii_lowercase(), &ws4.conv, true, std::time::Instant::now());
    ask(&ws4, "test.switch");
    assert_eq!(tickets(&ws4).len(), 1, "a Member can't answer: the item is filed");
    crate::thread_viewers::global().close_socket(sock);
}

#[test]
fn with_no_eligible_login_the_item_is_unassigned_and_pushes_nothing() {
    let _home = crate::test_support::TempHome::new();
    let _member = login("pcnone", Role::Member);
    let ws = seed("pctnone");
    let mark = test_push_capture::mark();
    ask(&ws, "test.switch");
    let t = tickets(&ws);
    assert_eq!(t.len(), 1);
    assert!(t[0].assignees.is_empty(), "nobody eligible: unassigned");
    let ticket_pushes = test_push_capture::since(mark)
        .into_iter()
        .filter(|e| matches!(e, PushEvent::FeedbackCreated { feedback_id, .. } if *feedback_id == t[0].id))
        .count();
    assert_eq!(ticket_pushes, 0, "an unassigned item would push every device: none is sent");
}

// ── notes (§4.3, §17.5) ─────────────────────────────────────────────

#[test]
fn a_person_below_the_floor_leaves_one_note_that_approvers_see() {
    let home = crate::test_support::TempHome::new();
    let member = login("pcnm", Role::Member);
    let admin = login("pcna", Role::Admin);
    let ws = seed("pcnote");
    let v = ask(&ws, "test.switch");
    let (id, card) = (id_of(&v), card_of(&v));
    let dana = person(&member, "member");

    let (s, r) = note(&ws, &card, &dana, "  I need this for the payroll run  ");
    assert_eq!(s, "200 OK", "{r}");
    assert_eq!(r["note"]["text"], "I need this for the payroll run");
    assert_eq!((r["note"]["by"].as_str(), r["note"]["role"].as_str()), (Some(member.as_str()), Some("member")));
    assert_eq!(r["requestId"], id);
    let notes_on = |d: &Value| d["choice"]["action"]["view"]["notes"].clone();
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["status"], "pending", "the card still waits");
    assert_eq!(notes_on(&d)[0]["text"], "I need this for the payroll run", "the stored view carries it: {d}");

    // Replace, then delete: one per person.
    let (s, _) = note(&ws, &card, &dana, "Payroll runs Friday");
    assert_eq!(s, "200 OK");
    let all = store::notes(row(&id).id.as_str()).expect("notes");
    assert_eq!(all.len(), 1, "replaced, not added: {all:?}");
    assert_eq!(all[0].text, "Payroll runs Friday");
    let (s, r) = note(&ws, &card, &dana, "");
    assert_eq!((s.as_str(), &r["note"]), ("200 OK", &Value::Null));
    assert!(store::notes(row(&id).id.as_str()).expect("notes").is_empty());
    assert_eq!(notes_on(&doc(&ws, &card)), json!([]));

    // Approvers answer instead.
    for who in [person(&admin, "admin"), Answerer::OwnerToken] {
        let (s, r) = note(&ws, &card, &who, "hi");
        assert_eq!((s.as_str(), r["error"]["code"].as_str()), ("403 Forbidden", Some("note_not_allowed")), "{r}");
    }

    // The shape rules (bad_note + reason).
    let long = "x".repeat(501);
    for (text, reason) in [
        (long.as_str(), "too_long"),
        ("line one\nline two", "newline"),
        ("see https://example.com", "link"),
        ("see www.example.com", "link"),
        ("my key is sk-abcdefghijklmnopqrstuvwxyz0123", "secret"),
    ] {
        let (s, r) = note(&ws, &card, &dana, text);
        assert_eq!(s, "400 Bad Request", "{text}: {r}");
        assert_eq!((r["error"]["code"].as_str(), r["error"]["reason"].as_str()), (Some("bad_note"), Some(reason)), "{r}");
    }
    let at_limit = format!("{}ok", "ok ".repeat(166));
    assert_eq!(at_limit.chars().count(), 500);
    let (s, r) = note(&ws, &card, &dana, &at_limit);
    assert_eq!(s, "200 OK", "500 characters is the limit: {r}");

    // The note never reaches the agent's turn.
    let (s, _) = note(&ws, &card, &dana, "SECRETWORD-payroll please");
    assert_eq!(s, "200 OK");
    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    let turns = inject::sent_to(&ws.handle);
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert!(!turns[0].contains("SECRETWORD"), "notes never reach the agent: {}", turns[0]);

    // Only while waiting.
    let (s, r) = note(&ws, &card, &dana, "too late");
    assert_eq!((s.as_str(), r["error"]["code"].as_str()), ("409 Conflict", Some("request_not_pending")), "{r}");

    // Audit: one line per write, never the text.
    let lines = audit_lines(&home, "permission-note");
    assert!(lines.len() >= 4, "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains("payroll") && !l.contains("SECRETWORD")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("note=deleted")) && lines.iter().any(|l| l.contains("note=replaced")));
    assert!(lines.iter().all(|l| l.contains(&format!("login:{member}")) && l.contains("via=login")), "{lines:?}");
}

#[test]
fn an_admin_may_note_an_owner_card_and_a_batch_note_names_its_item() {
    let _home = crate::test_support::TempHome::new();
    let admin = login("pcnb", Role::Admin);
    let ws = seed("pcnoteb");
    let v = ask(&ws, "test.owner-switch");
    let (s, r) = note(&ws, &card_of(&v), &person(&admin, "admin"), "Ok by me");
    assert_eq!(s, "200 OK", "an Admin waits on an Owner card: {r}");

    // Two asks in one session within 5 minutes: one batch card.
    let ws = seed("pcnoteb2");
    let a = ask(&ws, "test.switch");
    let b = ask(&ws, "test.slow");
    assert_eq!(card_of(&a), card_of(&b), "batched");
    let card = card_of(&a);
    let member = login("pcnbm", Role::Member);
    let dana = person(&member, "member");
    let batch = CardCtx { conversation_id: &ws.conv, card_id: &card, batch: true };
    let r = notes::handle(&params(&[("text", "for both")]), &batch, &dana);
    assert_eq!(r.status, "400 Bad Request", "a batch note names its item: {}", r.body);
    let b_id = id_of(&b);
    let r = notes::handle(&params(&[("text", "for the slow one"), ("item", b_id.as_str())]), &batch, &dana);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let d = doc(&ws, &card);
    let items = d["choice"]["action"]["view"]["items"].as_array().expect("batch items").clone();
    let slow = items.iter().find(|i| i["requestId"] == b_id.as_str()).expect("item");
    assert_eq!(slow["notes"][0]["text"], "for the slow one");
}

// ── Turn off again (§4.4, §17.3) ────────────────────────────────────

#[test]
fn turn_off_again_runs_the_twin_once_as_the_person_within_7_days() {
    let home = crate::test_support::TempHome::new();
    let admin = login("pcua", Role::Admin);
    let member = login("pcum", Role::Member);
    let ws = seed("pcundo");
    let v = ask(&ws, "test.switch");
    let (id, card) = (id_of(&v), card_of(&v));

    // Not yet approved.
    let (s, r) = undo_as(&ws, &card, &person(&admin, "admin"));
    assert_eq!((s.as_str(), r["error"]["code"].as_str()), ("409 Conflict", Some("undo_expired")), "{r}");

    let (s, a) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(doc(&ws, &card)["choice"]["action"]["view"]["undo"]["label"], "Turn off again");
    let applied = testkit::applied(&ws.id, "test.switch");

    // Below the floor: refused, with `required`.
    let (s, r) = undo_as(&ws, &card, &person(&member, "member"));
    assert_eq!((s.as_str(), r["error"]["code"].as_str(), r["error"]["required"].as_str()), ("403 Forbidden", Some("action_card_person_only"), Some("admin")), "{r}");

    let (s, r) = undo_as(&ws, &card, &person(&admin, "admin"));
    assert_eq!(s, "200 OK", "{r}");
    assert_eq!((r["undone"].as_bool(), r["state"].as_str()), (Some(true), Some("approved")));
    let msg = format!("Turned off again by {admin}.");
    assert_eq!(r["message"], msg);
    assert_eq!(testkit::applied(&ws.id, "test.switch"), applied + 1, "the twin ran once");
    assert_eq!(testkit::last_actor(&ws.id, "test.switch"), Some(format!("connect:{admin}")), "as the person");
    let rr = row(&id);
    assert_eq!((rr.state, rr.result_message.as_deref()), (RequestState::Approved, Some(msg.as_str())));
    let d = doc(&ws, &card);
    assert_eq!(d["choice"]["status"], "answered", "same card");
    assert_eq!(d["choice"]["action"]["result"]["message"], msg);
    assert_eq!(d["choice"]["action"]["view"]["undo"], Value::Null, "no second Turn off again: {d}");

    // Once.
    let (s, r) = undo_as(&ws, &card, &person(&admin, "admin"));
    assert_eq!((s.as_str(), r["error"]["code"].as_str()), ("409 Conflict", Some("undo_expired")), "{r}");
    assert_eq!(testkit::applied(&ws.id, "test.switch"), applied + 1);
    let lines = audit_lines(&home, "permission-undo");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("undo=applied") && lines[0].contains(&id) && lines[0].contains("via=login"), "{}", lines[0]);

    // Past 7 days.
    let ws2 = seed("pcundo7");
    let v2 = ask(&ws2, "test.switch");
    let (s, _) = answer_as("owner", &ws2, &card_of(&v2), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE permission_requests SET answered_at = answered_at - 8 * 86400 WHERE id = ?1",
            rusqlite::params![row(&id_of(&v2)).id.as_str()],
        )
        .expect("age the approval");
    }
    let (s, r) = undo_as(&ws2, &card_of(&v2), &Answerer::OwnerToken);
    assert_eq!((s.as_str(), r["error"]["code"].as_str()), ("409 Conflict", Some("undo_expired")), "{r}");

    // A one-time act has no Turn off again.
    let ws3 = seed("pcundo1");
    let held = format!("ar_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let v3 = ask_with(&ws3, "test.confirm", json!({ "request": held }));
    let (s, a) = answer_as("owner", &ws3, &card_of(&v3), json!({ "answer": "Allow", "confirmToken": "good-token" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(row(&held).state, RequestState::Approved);
    let (s, r) = undo_as(&ws3, &card_of(&v3), &Answerer::OwnerToken);
    assert_eq!((s.as_str(), r["error"]["code"].as_str()), ("409 Conflict", Some("undo_expired")), "{r}");
}

#[test]
fn two_turn_off_agains_at_once_run_the_twin_once() {
    let _home = crate::test_support::TempHome::new();
    let ws = seed("pcundor");
    let v = ask(&ws, "test.switch");
    let card = card_of(&v);
    let (s, _) = answer_as("owner", &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK");
    let applied = testkit::applied(&ws.id, "test.switch");
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let (conv, card) = (ws.conv.clone(), card.clone());
            std::thread::spawn(move || {
                undo::handle(&HashMap::new(), &CardCtx { conversation_id: &conv, card_id: &card, batch: false }, &Answerer::OwnerToken)
                    .status
                    .to_string()
            })
        })
        .collect();
    let mut st: Vec<String> = handles.into_iter().map(|h| h.join().expect("thread")).collect();
    st.sort();
    assert_eq!(st, ["200 OK", "409 Conflict"]);
    assert_eq!(testkit::applied(&ws.id, "test.switch"), applied + 1);
}

// ── Waiting on you (§17.8) ──────────────────────────────────────────

#[test]
fn waiting_on_you_lists_what_this_person_can_answer() {
    let _home = crate::test_support::TempHome::new();
    let admin = login("pcwa", Role::Admin);
    let member = login("pcwm", Role::Member);
    let ws = seed("pcwait");
    let a = ask(&ws, "test.switch");
    let ws_o = seed("pcwaito");
    let o = ask(&ws_o, "test.owner-switch");
    let (a_id, o_id) = (id_of(&a), id_of(&o));
    let ids = |who: &Who| -> Vec<String> {
        let r = waiting::pending(who);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        v["requests"]
            .as_array()
            .expect("requests")
            .iter()
            .filter_map(|x| x["requestId"].as_str().map(str::to_string))
            .filter(|i| *i == a_id || *i == o_id)
            .collect()
    };
    assert_eq!(ids(&Who::Owner), vec![a_id.clone(), o_id.clone()], "the owner answers both, oldest first");
    assert_eq!(ids(&Who::Login { username: admin.clone(), role: Role::Admin }), vec![a_id.clone()], "an Admin: the admin-floor one");
    assert!(ids(&Who::Login { username: member.clone(), role: Role::Member }).is_empty());
    let r = waiting::pending(&agent(&ws));
    assert_eq!((r.status, r.body.contains("forbidden")), ("403 Forbidden", true), "people only");

    // The row (§17.8), through the family's own door.
    let r = handle_gated(routes::PENDING, &Who::Owner, false, b"", &HashMap::new());
    assert_eq!(r.status, "200 OK");
    let v: Value = serde_json::from_str(&r.body).expect("json");
    let rowv = v["requests"].as_array().expect("rows").iter().find(|x| x["requestId"] == a_id.as_str()).expect("row").clone();
    let mut keys: Vec<&str> = rowv.as_object().expect("obj").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["action", "addr", "agent", "askedFor", "cardId", "createdAt", "expiresAt", "floor", "proof", "requestId", "seq", "setting", "whatChanges"]
    );
    assert_eq!(rowv["addr"], ws.handle);
    assert_eq!(rowv["cardId"], card_of(&a));
    let seq = overlay::find_thread_item(&ws.conv, &card_of(&a)).expect("item read").expect("item").seq;
    assert_eq!(rowv["seq"], json!(seq));
    assert_eq!((rowv["setting"].as_str(), rowv["floor"].as_str()), (Some("Allow test things"), Some("admin")));
    assert_eq!(rowv["proof"], json!({ "kind": "click" }));
    assert!(!r.body.contains("Run the suite"), "never the agent's --why");
    let r = handle_gated(routes::PENDING, &Who::Owner, true, b"", &HashMap::new());
    assert_eq!(r.status, "405 Method Not Allowed", "GET only");

    // Answered: gone.
    let (s, _) = answer_as("owner", &ws, &card_of(&a), json!({ "answer": "Deny" }));
    assert_eq!(s, "200 OK");
    assert_eq!(ids(&Who::Owner), vec![o_id]);
}

// ── off: today's answer (§2.4) ──────────────────────────────────────

#[test]
fn while_the_switch_is_off_the_card_routes_answer_as_unknown_thread_routes() {
    assert!(!k2_core::permission::gate::server_on(), "lib tests never turn the switch on");
    let body = json!({ "addr": "nobody", "id": "x", "text": "hi" }).to_string();
    let today = crate::overlay_routes::dispatch_post_as("/cli/thread/not-a-card-route", &HashMap::new(), body.as_bytes(), "owner");
    for route in [k2_core::permission::NOTE_ROUTE, k2_core::permission::UNDO_ROUTE] {
        for author in ["owner", "someone"] {
            let r = crate::overlay_routes::dispatch_post_as(route, &HashMap::new(), body.as_bytes(), author);
            assert_eq!((r.status, r.content_type, r.body.as_str()), (today.status, today.content_type, today.body.as_str()), "{route}");
        }
    }
}
