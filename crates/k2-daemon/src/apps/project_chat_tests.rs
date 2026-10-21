//! Project chat as an App surface (0.46.3), in process: the all-members
//! visibility rule (one unreachable agent hides a project), R3 sameness
//! (an unreadable project answers exactly what an unknown one answers),
//! paging (byte-identical to the owner's read), posting with and without
//! `project:write`, per-person read state (`app:<principalId>`), live
//! frames, and every answer against its closed catalog row.
//!
//! The doors (helper and link), the socket and the dispatcher wiring are
//! driven end to end in `tests/app_project_chat_integration.rs`.

use std::collections::HashMap;

use k2_core::project_groups;
use k2_core::skin::{RoomPolicy, SkinPass, CAP_FILES_READ, CAP_PROJECT_WRITE, CAP_THREAD_READ};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::project_chat::{self as pc, LIST_ROUTE, MESSAGES_ROUTE, POST_ROUTE, READ_ROUTE};
use crate::cli_response::CliResponse;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    /// Rooms (agent workspaces): sales, ops, hr.
    sales: String,
    ops: String,
    hr: String,
    /// sales + ops.
    both: String,
    /// sales + hr (hr is the room the guests can't reach).
    with_hr: String,
    /// sales only.
    solo: String,
    /// No members.
    empty: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (sales, ops, hr) = (format!("sales-{tag}"), format!("ops-{tag}"), format!("hr-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&sales, &ops, &hr] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
                params![ws, format!("/tmp/k2-pc-{}/{ws}", std::process::id())],
            )
            .unwrap();
        }
    }
    let group = |name: &str, members: &[&String]| {
        let g = project_groups::create_group(&format!("{name}-{tag}")).expect("group");
        for m in members {
            project_groups::add_member(&g.id, m).expect("member");
        }
        g.id
    };
    let both = group("launch", &[&sales, &ops]);
    let with_hr = group("payroll", &[&sales, &hr]);
    let solo = group("solo", &[&sales]);
    let empty = group("empty", &[]);
    Fx { _db: db, _home: home, sales, ops, hr, both, with_hr, solo, empty }
}

/// A guest's login session holding `caps` in each room.
fn guest(pid: &str, username: &str, rooms: &[(&str, &[&str])]) -> SkinPass {
    let room_policy: RoomPolicy = rooms.iter().map(|(r, c)| (r.to_string(), c.iter().map(|x| x.to_string()).collect())).collect();
    SkinPass {
        id: format!("t-{username}"),
        principal_id: Some(pid.into()),
        username: username.into(),
        caps: vec![],
        rooms: rooms.iter().map(|(r, _)| r.to_string()).collect(),
        session: true,
        room_policy,
        app: "host".into(),
        room_paths: Default::default(),
    }
}

/// An App's own (link) pass: flat caps in its rooms, no person.
fn app_pass(caps: &[&str], rooms: &[&str]) -> SkinPass {
    SkinPass {
        id: "t-app".into(),
        principal_id: None,
        username: "jobs".into(),
        caps: caps.iter().map(|c| c.to_string()).collect(),
        rooms: rooms.iter().map(|r| r.to_string()).collect(),
        session: false,
        room_policy: Default::default(),
        app: String::new(),
        room_paths: Default::default(),
    }
}

fn status(r: &CliResponse) -> u16 {
    r.status.split_whitespace().next().unwrap().parse().unwrap()
}

fn body(r: &CliResponse) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
}

fn get(path: &str, pass: &SkinPass, q: &[(&str, &str)]) -> CliResponse {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    pc::handle_pass(path, pass, &p, b"")
}

fn post(path: &str, pass: &SkinPass, b: J) -> CliResponse {
    pc::handle_pass(path, pass, &HashMap::new(), b.to_string().as_bytes())
}

fn ok(r: CliResponse) -> J {
    assert_eq!(status(&r), 200, "{}", r.body);
    body(&r)
}

fn ids(list: &J) -> Vec<String> {
    list["projects"].as_array().unwrap_or_else(|| panic!("projects in {list}")).iter().map(|p| p["groupId"].as_str().unwrap().to_string()).collect()
}

fn seqs(page: &J) -> Vec<i64> {
    page["messages"].as_array().unwrap_or_else(|| panic!("messages in {page}")).iter().map(|m| m["seq"].as_i64().unwrap()).collect()
}

// ── the catalog's closed schemas ─────────────────────────────────────

fn conforms(schema: &J, v: &J, at: &str, out: &mut Vec<String>) {
    if let Some(e) = schema.get("enum").and_then(J::as_array) {
        if !e.contains(v) {
            out.push(format!("{at}: {v} isn't one of {e:?}"));
        }
        return;
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(J::String(t)) => vec![t.as_str()],
        Some(J::Array(a)) => a.iter().filter_map(J::as_str).collect(),
        _ => vec![],
    };
    let fits = types.iter().any(|t| match *t {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "json" => true,
        _ => false,
    });
    if !fits {
        out.push(format!("{at}: {v} isn't {types:?}"));
        return;
    }
    if let (Some(items), Some(a)) = (schema.get("items"), v.as_array()) {
        for (i, x) in a.iter().enumerate() {
            conforms(items, x, &format!("{at}[{i}]"), out);
        }
    }
    if let (Some(props), Some(o)) = (schema.get("properties").and_then(J::as_object), v.as_object()) {
        for k in o.keys() {
            if !props.contains_key(k) {
                out.push(format!("{at}: field '{k}' isn't in the catalog row"));
            }
        }
        for r in schema.get("required").and_then(J::as_array).into_iter().flatten().filter_map(J::as_str) {
            if !o.contains_key(r) {
                out.push(format!("{at}: required field '{r}' is missing"));
            }
        }
        for (k, p) in props {
            if let Some(x) = o.get(k) {
                conforms(p, x, &format!("{at}.{k}"), out);
            }
        }
    }
}

fn assert_row_shape(verb: &str, v: &J) {
    let row = k2_core::contract::catalog().verb(verb).unwrap_or_else(|| panic!("no catalog row {verb}"));
    let mut out = Vec::new();
    conforms(&row.response, v, verb, &mut out);
    assert!(out.is_empty(), "{verb} drifted from its catalog row:\n{}\n{v}", out.join("\n"));
}

// ── visibility ───────────────────────────────────────────────────────

#[test]
fn a_project_shows_only_when_every_member_room_is_held_with_thread_read() {
    let f = fx("vis");
    let ada = guest("pid-ada", "ada", &[(&f.sales, &[CAP_THREAD_READ]), (&f.ops, &[CAP_THREAD_READ])]);
    let v = ok(get(LIST_ROUTE, &ada, &[]));
    assert_row_shape("project.list", &v);
    let mut got = ids(&v);
    got.sort();
    let mut want = vec![f.both.clone(), f.solo.clone()];
    want.sort();
    assert_eq!(got, want, "payroll has hr (unreachable) and empty has no members: {v}");
    // R3: nothing of the hidden projects in the answer: no id, no name, no count.
    for hidden in [&f.with_hr, &f.empty] {
        assert!(!v.to_string().contains(hidden.as_str()), "{hidden} leaked: {v}");
    }
    assert!(!v.to_string().contains("payroll") && !v.to_string().contains(&f.hr), "{v}");
    let launch = v["projects"].as_array().unwrap().iter().find(|p| p["groupId"] == f.both.as_str()).unwrap();
    assert_eq!(launch["members"], json!([{ "room": f.sales, "handle": f.sales }, { "room": f.ops, "handle": f.ops }]));
    assert_eq!(launch["poc"], f.sales.as_str(), "the first member is the point of contact");
    assert_eq!(launch["canPost"], false, "no project:write");
    assert_eq!((launch["unread"].as_i64(), launch["lastSeq"].as_i64(), launch["lastReadSeq"].as_i64()), (Some(0), Some(0), Some(0)));

    // A room held without thread:read doesn't count: ops with files:read only.
    let bo = guest("pid-bo", "bo", &[(&f.sales, &[CAP_THREAD_READ]), (&f.ops, &[CAP_FILES_READ])]);
    assert_eq!(ids(&ok(get(LIST_ROUTE, &bo, &[]))), vec![f.solo.clone()]);
    // A pass with no rooms sees no projects at all.
    assert_eq!(ids(&ok(get(LIST_ROUTE, &guest("pid-cy", "cy", &[]), &[]))), Vec::<String>::new());

    // Adding an unreachable agent hides a project at once; removing it shows it again.
    project_groups::add_member(&f.both, &f.hr).unwrap();
    assert!(!ids(&ok(get(LIST_ROUTE, &ada, &[]))).contains(&f.both), "hr joined launch");
    assert!(!pc::can_read(&ada, &f.both));
    project_groups::remove_member(&f.both, &f.hr).unwrap();
    assert!(ids(&ok(get(LIST_ROUTE, &ada, &[]))).contains(&f.both));
    assert!(pc::can_read(&ada, &f.both));
    assert!(!pc::can_read(&ada, "no-such-group"), "unknown fails closed");

    // An App's own pass: flat caps in its rooms, no person (read state null).
    let app = app_pass(&[CAP_THREAD_READ], &[&f.sales]);
    let v = ok(get(LIST_ROUTE, &app, &[]));
    assert_row_shape("project.list", &v);
    assert_eq!(ids(&v), vec![f.solo.clone()]);
    assert_eq!((v["projects"][0]["unread"].clone(), v["projects"][0]["muted"].clone()), (J::Null, J::Null));
}

#[test]
fn an_unreadable_project_answers_exactly_what_an_unknown_one_does() {
    let f = fx("r3");
    project_groups::post_message(&f.with_hr, "owner", "payroll secret").unwrap();
    let ada = guest("pid-ada", "ada", &[(&f.sales, &[CAP_THREAD_READ, CAP_PROJECT_WRITE]), (&f.ops, &[CAP_THREAD_READ])]);
    let name = project_groups::get_group_by_id(&f.with_hr).unwrap().name;
    let unknown = ["no-such-project", "00000000-0000-4000-8000-000000000000"];
    let refusals = |sel: &str| {
        vec![
            get(MESSAGES_ROUTE, &ada, &[("group", sel)]),
            get(MESSAGES_ROUTE, &ada, &[("group", sel), ("before_seq", "0")]),
            get(MESSAGES_ROUTE, &ada, &[("group", sel), ("after_seq", "1"), ("before_seq", "5")]),
            post(POST_ROUTE, &ada, json!({ "group": sel, "body": "hi" })),
            post(POST_ROUTE, &ada, json!({ "group": sel, "body": "" })),
            post(READ_ROUTE, &ada, json!({ "group": sel, "seq": 1 })),
        ]
    };
    let want = refusals(unknown[0]);
    for r in &want {
        assert_eq!((status(r), r.body.as_str()), (403, crate::skin_routes::SKIN_ROOM_JSON), "unknown");
    }
    for sel in [f.with_hr.as_str(), name.as_str(), &name.to_uppercase(), &f.empty, &f.with_hr[..8], unknown[1]] {
        let got = refusals(sel);
        for (g, w) in got.iter().zip(want.iter()) {
            assert_eq!((g.status, g.body.as_str()), (w.status, w.body.as_str()), "{sel} must look unknown");
        }
    }
    // Nothing was stored in the hidden chat.
    let page = project_groups::list_messages(&f.with_hr, None, None).unwrap();
    assert_eq!(page.messages.len(), 1);
    // A missing group is 400 for everyone, before any lookup.
    for r in [get(MESSAGES_ROUTE, &ada, &[]), post(POST_ROUTE, &ada, json!({ "body": "x" })), post(READ_ROUTE, &ada, json!({}))] {
        assert_eq!((status(&r), body(&r)["error"]["code"].as_str()), (400, Some("usage")), "{}", r.body);
    }
    // The names they can read resolve; prefixes never do.
    assert_eq!(status(&get(MESSAGES_ROUTE, &ada, &[("group", &f.both)])), 200);
    let launch = project_groups::get_group_by_id(&f.both).unwrap().name;
    assert_eq!(status(&get(MESSAGES_ROUTE, &ada, &[("group", &launch.to_uppercase())])), 200, "exact name, any case");
    assert_eq!(status(&get(MESSAGES_ROUTE, &ada, &[("group", &f.both[..8])])), 403, "no prefix match");
}

// ── paging ───────────────────────────────────────────────────────────

#[test]
fn paging_matches_the_owner_read_byte_for_byte() {
    let f = fx("page");
    for i in 1..=120 {
        project_groups::post_message(&f.both, if i % 2 == 0 { "owner" } else { f.ops.as_str() }, &format!("pc{i}")).unwrap();
    }
    let ada = guest("pid-ada", "ada", &[(&f.sales, &[CAP_THREAD_READ]), (&f.ops, &[CAP_THREAD_READ])]);
    let both = |q: &[(&str, &str)]| {
        let g = get(MESSAGES_ROUTE, &ada, q);
        let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let o = crate::project_group_routes::dispatch("/cli/project-group/messages", &p).expect("owner route");
        assert_eq!((g.status, g.body.as_str()), (o.status, o.body.as_str()), "guest and owner differ for {q:?}");
        g
    };
    let g = f.both.as_str();
    let v = ok(both(&[("group", g), ("before_seq", "121"), ("limit", "50")]));
    assert_row_shape("project.read", &v);
    assert_eq!((seqs(&v).first().copied(), seqs(&v).last().copied(), v["has_more"].as_bool()), (Some(71), Some(120), Some(true)));
    let v = ok(both(&[("group", g), ("before_seq", "71"), ("limit", "50")]));
    assert_eq!((seqs(&v).first().copied(), seqs(&v).last().copied(), v["has_more"].as_bool()), (Some(21), Some(70), Some(true)));
    let v = ok(both(&[("group", g), ("before_seq", "21"), ("limit", "50")]));
    assert_eq!((seqs(&v), v["has_more"].as_bool()), ((1..=20).collect::<Vec<_>>(), Some(false)));
    let v = ok(both(&[("group", g), ("before_seq", "1"), ("limit", "50")]));
    assert_eq!((seqs(&v).len(), v["has_more"].as_bool()), (0, Some(false)));
    // after_seq catches up, oldest first.
    let v = ok(both(&[("group", g), ("after_seq", "100")]));
    assert_row_shape("project.read", &v);
    assert_eq!(seqs(&v), (101..=120).collect::<Vec<_>>());
    assert_eq!((v["has_more"].as_bool(), v["changed"].clone(), v["removed"].clone()), (Some(false), json!([]), json!([])));
    // The plain page: the latest 20, truncated.
    let v = ok(both(&[("group", g)]));
    assert_row_shape("project.read", &v);
    assert_eq!((seqs(&v), v["truncated"].as_bool()), ((101..=120).collect::<Vec<_>>(), Some(true)));
    // Bad params: the same 400 as the owner's read (for a project it can read).
    for q in [vec![("group", g), ("before_seq", "0")], vec![("group", g), ("after_seq", "1"), ("before_seq", "5")], vec![("group", g), ("limit", "0")]] {
        let r = both(&q);
        assert_eq!(status(&r), 400, "{q:?}: {}", r.body);
    }
    // Every line carries its author kind; agents' and the owner's as stored.
    let v = ok(get(MESSAGES_ROUTE, &ada, &[("group", g), ("limit", "2")]));
    assert_eq!(v["messages"][0]["authorKind"], "agent");
    assert_eq!(v["messages"][1]["authorKind"], "person");
}

// ── posting ──────────────────────────────────────────────────────────

#[test]
fn posting_needs_project_write_in_every_member_room() {
    let f = fx("post");
    let reader = guest("pid-r", "rae", &[(&f.sales, &[CAP_THREAD_READ]), (&f.ops, &[CAP_THREAD_READ])]);
    let r = post(POST_ROUTE, &reader, json!({ "group": f.both, "body": "hello" }));
    let v = body(&r);
    assert_eq!((status(&r), v["error"]["code"].as_str(), v["error"]["cap"].as_str()), (403, Some("cap_not_granted"), Some(CAP_PROJECT_WRITE)), "{v}");
    // The cap in one member room only is not enough.
    let half = guest("pid-h", "hal", &[(&f.sales, &[CAP_THREAD_READ, CAP_PROJECT_WRITE]), (&f.ops, &[CAP_THREAD_READ])]);
    assert_eq!(status(&post(POST_ROUTE, &half, json!({ "group": f.both, "body": "hello" }))), 403);
    // project:write without thread:read somewhere: the project is invisible, so skin_room.
    let blind = guest("pid-b", "bea", &[(&f.sales, &[CAP_THREAD_READ, CAP_PROJECT_WRITE]), (&f.ops, &[CAP_PROJECT_WRITE])]);
    let r = post(POST_ROUTE, &blind, json!({ "group": f.both, "body": "hello" }));
    assert_eq!((status(&r), r.body.as_str()), (403, crate::skin_routes::SKIN_ROOM_JSON));
    assert!(project_groups::list_messages(&f.both, None, None).unwrap().messages.is_empty(), "nothing stored by a refusal");

    let writer = guest("pid-w", "wes", &[(&f.sales, &[CAP_THREAD_READ, CAP_PROJECT_WRITE]), (&f.ops, &[CAP_THREAD_READ, CAP_PROJECT_WRITE])]);
    assert_eq!(ok(get(LIST_ROUTE, &writer, &[]))["projects"].as_array().unwrap().iter().find(|p| p["groupId"] == f.both.as_str()).unwrap()["canPost"], true);
    for (b, why) in [(json!({ "group": f.both, "body": "   " }), "empty"), (json!({ "group": f.both, "body": "x".repeat(pc::MAX_BODY_BYTES + 1) }), "too long"), (json!({ "group": f.both, "body": "x", "author": "owner" }), "no author override")] {
        let r = post(POST_ROUTE, &writer, b);
        assert_eq!((status(&r), body(&r)["error"]["code"].as_str()), (400, Some("usage")), "{why}: {}", r.body);
    }
    let v = ok(post(POST_ROUTE, &writer, json!({ "group": f.both, "body": "Can we ship Friday?" })));
    assert_row_shape("project.post", &v);
    assert_eq!((v["author"].as_str(), v["authorKind"].as_str(), v["seq"].as_i64()), (Some("guest:wes"), Some("guest"), Some(1)));
    assert!(v.get("deliveredSessionId").is_none(), "a guest never sees a session id: {v}");
    assert_eq!(v["delivered"], false, "the dead test workspace can't take it: {v}");
    let stored = project_groups::list_messages(&f.both, None, None).unwrap().messages;
    assert_eq!((stored.len(), stored[0].author.as_str(), stored[0].author_kind.as_str()), (1, "guest:wes", "guest"));
    // The owner sees the guest's line in the owner read, as a guest's.
    let p: HashMap<String, String> = [("group".to_string(), f.both.clone())].into();
    let owner = body(&crate::project_group_routes::dispatch("/cli/project-group/messages", &p).unwrap());
    assert_eq!(owner["messages"][0]["author"], "guest:wes");
    // By name, any case.
    let name = project_groups::get_group_by_id(&f.both).unwrap().name;
    assert_eq!(ok(post(POST_ROUTE, &writer, json!({ "group": name.to_uppercase(), "body": "by name" })))["seq"], 2);
}

// ── read state ───────────────────────────────────────────────────────

#[test]
fn read_state_is_per_person_and_never_an_apps_own_pass() {
    let f = fx("read");
    let caps: &[&str] = &[CAP_THREAD_READ, CAP_PROJECT_WRITE];
    let ada = guest("pid-ada", "ada", &[(&f.sales, caps)]);
    let bo = guest("pid-bo", "bo", &[(&f.sales, caps)]);
    // First sight: everything starts read (the person's baseline).
    let row = |p: &SkinPass| ok(get(LIST_ROUTE, p, &[]))["projects"][0].clone();
    assert_eq!(row(&ada)["unread"], 0);
    assert_eq!(row(&bo)["unread"], 0);
    project_groups::post_message(&f.solo, &f.sales, "agent line").unwrap();
    ok(post(POST_ROUTE, &ada, json!({ "group": f.solo, "body": "ada line" })));
    project_groups::post_message(&f.solo, &f.sales, "agent again").unwrap();
    let a = row(&ada);
    assert_eq!(
        (a["unread"].as_i64(), a["lastReadSeq"].as_i64(), a["lastSeq"].as_i64()),
        (Some(1), Some(2), Some(3)),
        "her own post read through it; the agent's later line is unread: {a}"
    );
    let b = row(&bo);
    assert_eq!((b["unread"].as_i64(), b["lastReadSeq"].as_i64()), (Some(3), Some(0)), "ada's guest line counts for bo: {b}");
    let v = ok(post(READ_ROUTE, &bo, json!({ "group": f.solo, "seq": 1 })));
    assert_row_shape("project.markRead", &v);
    assert_eq!((v["changed"].as_bool(), v["unread"].as_i64(), v["lastReadSeq"].as_i64(), v["lastSeq"].as_i64()), (Some(true), Some(2), Some(1), Some(3)));
    let v = ok(post(READ_ROUTE, &bo, json!({ "group": f.solo, "seq": 1 })));
    assert_eq!(v["changed"], false, "forward only");
    let v = ok(post(READ_ROUTE, &bo, json!({ "group": f.solo })));
    assert_eq!((v["unread"].as_i64(), v["lastReadSeq"].as_i64()), (Some(0), Some(3)), "no seq = the newest");
    assert_eq!(row(&ada)["unread"], 1, "bo's reads are bo's");
    // The key is the person guest:<principalId> (read_routes::PassReader),
    // never the Connect username space and never the App namespace app:.
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM read_cursors WHERE person = 'guest:pid-bo' AND conv_id = ?1 AND last_read_seq = 3", params![f.solo], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM read_cursors WHERE person = 'bo'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0, "a guest never writes the Connect user 'bo''s state");
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM read_cursors WHERE person LIKE 'app:%'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0, "a guest never writes under the App namespace");
    drop(conn);
    let r = post(READ_ROUTE, &bo, json!({ "group": f.solo, "seq": -1 }));
    assert_eq!(status(&r), 400);
    // An App's own pass has no person: a platform pass and a link pass alike.
    let app = app_pass(&[CAP_THREAD_READ], &[&f.sales]);
    let r = post(READ_ROUTE, &app, json!({ "group": f.solo }));
    assert_eq!((status(&r), body(&r)["error"]["code"].as_str()), (403, Some("not_exposed")), "{}", r.body);
    let mut link = app_pass(&[CAP_THREAD_READ], &[&f.sales]);
    link.app = "svc-jobs".into();
    assert_eq!(crate::read_routes::pass_reader(&link).key, "app:svc-jobs");
    let r = post(READ_ROUTE, &link, json!({ "group": f.solo }));
    assert_eq!((status(&r), body(&r)["error"]["code"].as_str()), (403, Some("not_exposed")), "{}", r.body);
    assert_eq!(ok(get(LIST_ROUTE, &link, &[]))["projects"][0]["unread"], J::Null);
}

/// The moves of `guest:pid-dee` on the (process-wide) guest bus so far.
fn dee_moves(rx: &mut tokio::sync::broadcast::Receiver<crate::read_live::GuestMoved>) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(m) if m.key == "guest:pid-dee" => out.push(m.conversation_id),
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(_) => return out,
        }
    }
}

/// Rosson Q11: one guest is one person on every surface. The same
/// principal signed in to the box login (surface A) and to a published
/// App on another session (surface B): a mark-read on A is B's state, the
/// key is the one its Thread read state uses, and B's sockets hear it.
#[test]
fn a_mark_read_on_one_surface_is_the_same_persons_state_on_another() {
    let f = fx("q11");
    let caps: &[&str] = &[CAP_THREAD_READ, CAP_PROJECT_WRITE];
    let a = guest("pid-dee", "dee", &[(&f.sales, caps)]);
    let mut b = guest("pid-dee", "dee", &[(&f.sales, caps)]);
    b.id = "t-dee-phone".into();
    b.app = "svc-jobs".into();
    assert_eq!(crate::read_routes::pass_reader(&a).key, "guest:pid-dee");
    assert_eq!(crate::read_routes::pass_reader(&b).key, crate::read_routes::pass_reader(&a).key, "one person, one key");
    assert_eq!(pc::person_of(&a).map(|p| p.key), Some(crate::read_routes::guest_key_of_principal("pid-dee")));
    let row = |p: &SkinPass| ok(get(LIST_ROUTE, p, &[]))["projects"][0].clone();
    assert_eq!(row(&b)["unread"], 0, "baseline");
    for i in 0..3 {
        project_groups::post_message(&f.solo, &f.sales, &format!("agent {i}")).unwrap();
    }
    assert_eq!((row(&a)["unread"].as_i64(), row(&b)["unread"].as_i64()), (Some(3), Some(3)));
    let mut moves = crate::read_live::subscribe_guest();
    let v = ok(post(READ_ROUTE, &a, json!({ "group": f.solo, "seq": 2 })));
    assert_eq!((v["changed"].as_bool(), v["lastReadSeq"].as_i64()), (Some(true), Some(2)));
    let seen = row(&b);
    assert_eq!((seen["unread"].as_i64(), seen["lastReadSeq"].as_i64()), (Some(1), Some(2)), "surface B sees A's read: {seen}");
    // The move went out under the person's key, for this project.
    assert_eq!(dee_moves(&mut moves), vec![f.solo.clone()], "one move, this project");
    // B's read frame: the overlay read-frame shape, own project only.
    let frame: J = serde_json::from_str(&pc::read_frame_wire(&{
        let db = k2_core::db::shared();
        let conn = db.lock();
        let person = pc::person_of(&b).expect("a person");
        k2_core::read_state::item(&conn, &person, k2_core::read_state::KIND_PROJECT, &f.solo, k2_core::read_state::now_secs()).unwrap()
    }))
    .unwrap();
    assert_row_shape("project.events", &frame);
    assert_eq!(
        (frame["collection"].as_str(), frame["id"].as_str(), frame["unread"].as_i64(), frame["lastReadSeq"].as_i64()),
        (Some("read"), Some(f.solo.as_str()), Some(1), Some(2))
    );
    // Marking on B moves A too; a no-op sends nothing.
    ok(post(READ_ROUTE, &b, json!({ "group": f.solo })));
    assert_eq!(row(&a)["unread"], 0);
    assert_eq!(dee_moves(&mut moves), vec![f.solo.clone()], "the second move");
    let v = ok(post(READ_ROUTE, &a, json!({ "group": f.solo })));
    assert_eq!(v["changed"], false);
    assert!(dee_moves(&mut moves).is_empty(), "no move for an unchanged cursor");
    let _ = (&f.ops, &f.hr, &f.both, &f.with_hr, &f.empty);
}

// ── frames ───────────────────────────────────────────────────────────

#[test]
fn every_stored_line_goes_out_as_a_project_frame() {
    let f = fx("frames");
    let mut rx = pc::subscribe();
    let writer = guest("pid-w", "wes", &[(&f.sales, &[CAP_THREAD_READ, CAP_PROJECT_WRITE]), (&f.ops, &[CAP_THREAD_READ, CAP_PROJECT_WRITE])]);
    ok(post(POST_ROUTE, &writer, json!({ "group": f.both, "body": "from the app" })));
    // An owner line through the owner route goes out too.
    let r = crate::project_group_routes::dispatch_post("/cli/project-group/msg", json!({ "group": f.both, "body": "from the owner" }).to_string().as_bytes(), "owner");
    assert_eq!(status(&r), 200, "{}", r.body);
    let mut got = Vec::new();
    while let Ok(m) = rx.try_recv() {
        if m.group_id == f.both {
            got.push(m);
        }
    }
    assert_eq!(got.iter().map(|m| (m.seq, m.author.as_str())).collect::<Vec<_>>(), vec![(1, "guest:wes"), (2, "owner")]);
    let frame: J = serde_json::from_str(&pc::frame_wire(&got[0])).unwrap();
    assert_row_shape("project.events", &frame);
    assert_eq!((frame["collection"].as_str(), frame["seq"].as_i64(), frame["groupId"].as_str()), (Some("project"), Some(1), Some(f.both.as_str())));
    assert_eq!(frame["id"], frame["doc"]["id"]);
    // The frame's doc is exactly the message project.read returns.
    let page = ok(get(MESSAGES_ROUTE, &writer, &[("group", &f.both)]));
    assert_eq!(frame["doc"], page["messages"][0]);
    // can_read is the filter a stream applies per pass.
    let outsider = guest("pid-o", "olly", &[(&f.sales, &[CAP_THREAD_READ])]);
    assert!(pc::can_read(&writer, &got[0].group_id) && !pc::can_read(&outsider, &got[0].group_id));
    let _ = (&f.hr, &f.with_hr, &f.empty);
}

// ── the catalog and the doors ────────────────────────────────────────

#[test]
fn the_rows_are_app_only_on_both_doors_with_project_write_a_known_cap() {
    let c = k2_core::contract::catalog();
    for (verb, method, route) in [
        ("project.list", "GET", LIST_ROUTE),
        ("project.read", "GET", MESSAGES_ROUTE),
        ("project.post", "POST", POST_ROUTE),
        ("project.markRead", "POST", READ_ROUTE),
    ] {
        let v = c.verb(verb).unwrap_or_else(|| panic!("{verb}"));
        assert!(v.exposed_to(k2_core::contract::Exposure::App), "{verb}");
        assert!(!v.exposed_to(k2_core::contract::Exposure::GardenLocal), "{verb}: no Projects on the Garden bridge");
        assert_eq!(v.feature, pc::FEATURE);
        assert!(k2_core::app_surface::link_allows_http(method, route), "{verb} on the link");
        assert!(k2_core::app_surface::helper_allows_http(method, route), "{verb} on the helper");
        assert_eq!(k2_core::app_surface::verb_for(method, route), Some(verb));
    }
    assert_eq!(k2_core::app_surface::ws_param(pc::EVENTS_WS_PATH), Some("group"));
    assert_eq!(k2_core::app_surface::verb_for("WS", pc::EVENTS_WS_PATH), Some("project.events"));
    assert_eq!(c.verb("project.post").and_then(|v| v.cap.as_deref()), Some(CAP_PROJECT_WRITE));
    assert!(k2_core::skin::parse_caps(Some(&[CAP_PROJECT_WRITE.to_string()])).is_ok(), "an owner can grant it");
    assert_eq!(k2_core::workspace::settings::app_core_of_cap(CAP_PROJECT_WRITE), None, "person-held, no core");
    assert!(crate::boot_status::features().contains(&pc::FEATURE));
}
