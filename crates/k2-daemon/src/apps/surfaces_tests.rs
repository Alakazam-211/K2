//! Room-scope R9/R16 (0.46.1), in process: `room.members`, `app.self`,
//! `app.usage` on the App transport (off the link), the Garden transport
//! and the owner/agent routes; the hide-names rule; the events; and every
//! answer against its catalog row (closed schemas: no field the row
//! doesn't name, every required field, the right types).
//!
//! The App's own link (`app.self` / `app.usage` with the link pass, and
//! another App's id answering 404 with the same bytes) is driven end to
//! end in `tests/room_surfaces_integration.rs`.

use std::collections::{BTreeMap, HashMap};

use k2_core::connect_users::Role;
use k2_core::deploy::store;
use k2_core::skin::{RoomPolicy, SkinPass, CAP_ROOM_MEMBERS, CAP_THREAD_READ};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::routes::Who;
use super::surfaces;

const SEED: &str = "abababababababababababababababababababababababababababababababab";

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    room: String,
    other: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (room, other) = (format!("sales-{tag}"), format!("ops-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&room, &other] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, format!("/tmp/k2-surf/{ws}")])
                .unwrap();
        }
        for (app, ws) in [("jobs", &room), ("other", &other)] {
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &format!("svc-{app}"),
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: "container",
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
    }
    // ada (staff, room:members) and bo (rooms only) on the box login; dee
    // on the jobs App; cy holds another room only.
    let mut staff = RoomPolicy::new();
    staff.insert(room.clone(), vec![CAP_THREAD_READ.into(), CAP_ROOM_MEMBERS.into()]);
    let mut elsewhere = RoomPolicy::new();
    elsewhere.insert(other.clone(), vec![CAP_THREAD_READ.into()]);
    for u in ["ada", "bo", "cy"] {
        k2_core::skin::add_principal(u).unwrap();
    }
    k2_core::skin::create_role("staff", &staff).unwrap();
    k2_core::skin::create_role("elsewhere", &elsewhere).unwrap();
    k2_core::skin::assign_role("ada", "staff").unwrap();
    k2_core::skin::assign_role("cy", "elsewhere").unwrap();
    k2_core::skin::set_principal_default_rooms("bo", &[room.clone()], false).unwrap();
    k2_core::skin::set_principal_full_name("ada", Some("Ada Lovelace")).unwrap();
    k2_core::skin::add_principal_for_app("dee", None, "svc-jobs", Some("staff"), "owner").unwrap();
    Fx { _db: db, _home: home, room, other }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn login(role: Role) -> Who {
    Who::Login { username: "lee".into(), role }
}

fn status(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split_whitespace().next().unwrap().parse().unwrap()
}

fn body(r: &crate::cli_response::CliResponse) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
}

fn owner_get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = surfaces::handle(path, who.clone(), b"", &p);
    (status(&r), body(&r))
}

fn owner_post(path: &str, who: &Who, b: J) -> (u16, J) {
    let r = surfaces::handle(path, who.clone(), b.to_string().as_bytes(), &HashMap::new());
    (status(&r), body(&r))
}

fn pass_get(path: &str, pass: &SkinPass, q: &[(&str, &str)]) -> crate::cli_response::CliResponse {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    surfaces::handle_pass(path, pass, &p, None)
}

/// A guest's login session holding `caps` in each room.
fn guest(app: &str, rooms: &[(&str, &[&str])]) -> SkinPass {
    let room_policy: RoomPolicy = rooms.iter().map(|(r, c)| (r.to_string(), c.iter().map(|x| x.to_string()).collect())).collect();
    SkinPass {
        id: "t-guest".into(),
        principal_id: Some("p-guest".into()),
        username: "guest".into(),
        caps: vec![],
        rooms: rooms.iter().map(|(r, _)| r.to_string()).collect(),
        session: true,
        room_policy,
        app: app.into(),
        room_paths: Default::default(),
    }
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn usernames(v: &J) -> Vec<J> {
    v["members"].as_array().unwrap_or_else(|| panic!("members in {v}")).iter().map(|m| m["username"].clone()).collect()
}

// ── the catalog's closed schemas ─────────────────────────────────────

/// Problems with `v` against a catalog response schema (the subset the
/// lint allows): closed objects, required keys, types and enums.
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
    let ok = types.iter().any(|t| match *t {
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
    if !ok {
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

/// `v` is exactly what the catalog row `verb` says it answers.
fn assert_row_shape(verb: &str, v: &J) {
    let row = k2_core::contract::catalog().verb(verb).unwrap_or_else(|| panic!("no catalog row {verb}"));
    let mut out = Vec::new();
    conforms(&row.response, v, verb, &mut out);
    assert!(out.is_empty(), "{verb} drifted from its catalog row:\n{}\n{v}", out.join("\n"));
}

/// An owner-route answer minus its `ok` (the route's envelope).
fn unwrapped(mut v: J) -> J {
    assert_eq!(v["ok"], true, "{v}");
    v.as_object_mut().unwrap().remove("ok");
    v
}

// ── room.members ─────────────────────────────────────────────────────

#[test]
fn members_owner_view_from_grants_with_names_and_every_app() {
    let f = fx("own");
    let (st, v) = owner_get("/cli/app/members", &Who::Owner, &[("workspace", &f.room)]);
    assert_eq!(st, 200, "{v}");
    let v = unwrapped(v);
    assert_row_shape("room.members", &v);
    assert_eq!(usernames(&v), [json!("ada"), json!("bo"), json!("dee")], "cy holds another room only");
    assert_eq!(v["admins"], "admins of this box");
    assert_eq!((v["count"].as_i64(), v["namesHidden"].as_bool()), (Some(3), Some(false)));
    let ada = &v["members"][0];
    assert_eq!((ada["displayName"].as_str(), ada["role"].as_str(), ada["app"].as_str()), (Some("Ada Lovelace"), Some("staff"), Some("host")));
    assert_eq!(v["members"][2]["app"], "jobs", "a grant on a published service shows its App id");
    // One App's grants: host = the box login.
    let (_, v) = owner_get("/cli/app/members", &Who::Owner, &[("workspace", &f.room), ("app", "host")]);
    assert_eq!(usernames(&v), [json!("ada"), json!("bo")]);
    let (_, v) = owner_get("/cli/app/members", &Who::Owner, &[("workspace", &f.room), ("app", "jobs")]);
    assert_eq!(usernames(&v), [json!("dee")]);
    let (st, v) = owner_get("/cli/app/members", &Who::Owner, &[("workspace", &f.room), ("app", "ghost")]);
    assert_eq!((st, code(&v)), (404, "app_not_found"));
    // The other room.
    let (_, v) = owner_get("/cli/app/members", &Who::Owner, &[("workspace", &f.other)]);
    assert_eq!(usernames(&v), [json!("cy")]);
}

#[test]
fn members_owner_routes_who_may_read() {
    let f = fx("who");
    assert_eq!(owner_get("/cli/app/members", &login(Role::Admin), &[("workspace", &f.room)]).0, 200, "an admin");
    let (st, v) = owner_get("/cli/app/members", &login(Role::Member), &[("workspace", &f.room)]);
    assert_eq!((st, code(&v)), (403, "forbidden"), "a member login: {v}");
    // An agent: its own room (named or not), never another.
    let (st, v) = owner_get("/cli/app/members", &agent(&f.room), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["room"], f.room.as_str());
    let (st, v) = owner_get("/cli/app/members", &agent(&f.room), &[("workspace", &f.other)]);
    assert_eq!((st, code(&v)), (403, "forbidden"), "{v}");
    assert_eq!(owner_get("/cli/app/members", &Who::Wrong, &[("workspace", &f.room)]).0, 403);
}

#[test]
fn hiding_names_applies_to_app_callers_only_and_is_the_owners() {
    let f = fx("hide");
    let (st, v) = owner_post("/cli/app/room-names", &agent(&f.room), json!({ "hide": true }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "an agent can't: {v}");
    let (st, v) = owner_post("/cli/app/room-names", &login(Role::Admin), json!({ "workspace": f.room, "hide": true }));
    assert_eq!((st, code(&v)), (403, "owner_only"), "an admin can't: {v}");
    let (st, v) = owner_post("/cli/app/room-names", &Who::Owner, json!({ "workspace": f.room, "hide": "yes" }));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    let (st, v) = owner_post("/cli/app/room-names", &Who::Owner, json!({ "workspace": f.room, "hide": true }));
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["namesHidden"].as_bool(), v["changed"].as_bool()), (Some(true), Some(true)));

    // A guest with room:members: the list, with names nulled.
    let pass = guest("host", &[(&f.room, &[CAP_ROOM_MEMBERS])]);
    let r = pass_get(surfaces::MEMBERS_ROUTE, &pass, &[("workspace", &f.room)]);
    assert_eq!(status(&r), 200, "{}", r.body);
    let v = body(&r);
    assert_row_shape("room.members", &v);
    assert_eq!(v["namesHidden"], true);
    assert_eq!(usernames(&v), [J::Null, J::Null], "the box login's two, no names");
    assert!(v["members"].as_array().unwrap().iter().all(|m| m["displayName"].is_null() && m["kind"] == "guest"), "{v}");
    assert!(!r.body.contains("ada") && !r.body.contains("Lovelace"), "no name anywhere: {}", r.body);

    // The owner still sees every name (CLI and Garden), with the flag.
    let (_, v) = owner_get("/cli/app/members", &Who::Owner, &[("workspace", &f.room)]);
    assert_eq!(usernames(&v), [json!("ada"), json!("bo"), json!("dee")]);
    assert_eq!(v["namesHidden"], true);
    let g = surfaces::garden_call("room.members", &[json!({ "workspace": f.room })]).unwrap();
    assert_row_shape("room.members", &g);
    assert_eq!(usernames(&g), [json!("ada"), json!("bo"), json!("dee")], "a Garden is the owner");
    // Per room: the other room's names are shown.
    let pass = guest("host", &[(&f.other, &[CAP_ROOM_MEMBERS])]);
    let v = body(&pass_get(surfaces::MEMBERS_ROUTE, &pass, &[("workspace", &f.other)]));
    assert_eq!(usernames(&v), [json!("cy")]);
    // Shown again.
    let (_, v) = owner_post("/cli/app/room-names", &Who::Owner, json!({ "workspace": f.room, "hide": false }));
    assert_eq!(v["changed"], true);
    let pass = guest("host", &[(&f.room, &[CAP_ROOM_MEMBERS])]);
    let v = body(&pass_get(surfaces::MEMBERS_ROUTE, &pass, &[("workspace", &f.room)]));
    assert_eq!(usernames(&v), [json!("ada"), json!("bo")]);
}

#[test]
fn members_on_the_app_transport_enforce_room_cap_and_own_app() {
    let f = fx("pass");
    // The cap: holding the room without room:members is cap_not_granted.
    let pass = guest("host", &[(&f.room, &[CAP_THREAD_READ])]);
    let r = pass_get(surfaces::MEMBERS_ROUTE, &pass, &[("workspace", &f.room)]);
    assert_eq!(status(&r), 403, "{}", r.body);
    let v = body(&r);
    assert_eq!((code(&v), v["error"]["cap"].as_str()), ("cap_not_granted", Some(CAP_ROOM_MEMBERS)));
    // A room it doesn't hold is the uniform skin_room, the same as one
    // that doesn't exist (no oracle).
    let pass = guest("host", &[(&f.room, &[CAP_ROOM_MEMBERS])]);
    let held_not = pass_get(surfaces::MEMBERS_ROUTE, &pass, &[("workspace", &f.other)]);
    let missing = pass_get(surfaces::MEMBERS_ROUTE, &pass, &[("workspace", "no-such-room")]);
    assert_eq!((status(&held_not), status(&missing)), (403, 403));
    assert_eq!(held_not.body, missing.body, "a room it doesn't hold answers like one that doesn't exist");
    assert_eq!(code(&body(&missing)), "skin_room");
    let r = pass_get(surfaces::MEMBERS_ROUTE, &pass, &[]);
    assert_eq!((status(&r), code(&body(&r))), (400, "usage"));
    // Its own App's grants only: a box-login guest sees the box login's.
    let v = body(&pass_get(surfaces::MEMBERS_ROUTE, &pass, &[("workspace", &f.room)]));
    assert_eq!(usernames(&v), [json!("ada"), json!("bo")]);
    assert!(v["members"].as_array().unwrap().iter().all(|m| m["app"] == "host"));
    // A jobs guest sees the jobs App's grants.
    let jobs = guest("svc-jobs", &[(&f.room, &[CAP_ROOM_MEMBERS])]);
    let v = body(&pass_get(surfaces::MEMBERS_ROUTE, &jobs, &[("workspace", &f.room)]));
    assert_eq!(usernames(&v), [json!("dee")]);
    assert_eq!(v["members"][0]["app"], "jobs");
    // Naming itself is fine; naming another App is exactly "no such App".
    assert_eq!(status(&pass_get(surfaces::MEMBERS_ROUTE, &jobs, &[("workspace", &f.room), ("app", "jobs")])), 200);
    let other = pass_get(surfaces::MEMBERS_ROUTE, &jobs, &[("workspace", &f.room), ("app", "other")]);
    let ghost = pass_get(surfaces::MEMBERS_ROUTE, &jobs, &[("workspace", &f.room), ("app", "ghost")]);
    assert_eq!((status(&other), status(&ghost)), (404, 404));
    assert_eq!(other.body, ghost.body, "another App's answer is byte-identical to no such App");
    assert_eq!(code(&body(&other)), "not_found");
    assert_eq!(other.body, surfaces::not_found().body);
}

#[test]
fn app_self_and_usage_never_answer_a_guest_or_off_the_link() {
    let f = fx("offlink");
    for path in [surfaces::SELF_ROUTE, surfaces::USAGE_ROUTE] {
        let pass = guest("svc-jobs", &[(&f.room, &[CAP_ROOM_MEMBERS])]);
        let r = pass_get(path, &pass, &[]);
        assert_eq!((status(&r), code(&body(&r))), (403, "not_exposed"), "{path}: a guest of the App");
        let platform = SkinPass { session: false, app: String::new(), ..guest("", &[(&f.room, &[])]) };
        let r = pass_get(path, &platform, &[("app", "jobs")]);
        assert_eq!((status(&r), code(&body(&r))), (403, "not_exposed"), "{path}: a pass off any link");
    }
}

// ── app.self / app.usage (owner, agents, Garden) ─────────────────────

#[test]
fn app_self_answers_the_row_and_agents_see_their_own_apps_only() {
    let f = fx("self");
    let (st, v) = owner_get("/cli/app/self", &Who::Owner, &[("app", "jobs")]);
    assert_eq!(st, 200, "{v}");
    let v = unwrapped(v);
    assert_row_shape("app.self", &v);
    assert_eq!((v["app"].as_str(), v["workspace"].as_str()), (Some("jobs"), Some(f.room.as_str())));
    assert_eq!((v["health"].as_str(), v["liveSeed"].clone(), v["deployedAt"].clone()), (Some("stopped"), J::Null, J::Null));
    // An agent: its own workspace's App, never another's (no oracle: the
    // same app_not_found as an App that doesn't exist).
    assert_eq!(owner_get("/cli/app/self", &agent(&f.room), &[("app", "jobs")]).0, 200);
    let (st_o, v_o) = owner_get("/cli/app/self", &agent(&f.room), &[("app", "other")]);
    let (st_g, v_g) = owner_get("/cli/app/self", &agent(&f.room), &[("app", "ghost")]);
    assert_eq!((st_o, code(&v_o)), (404, "app_not_found"));
    assert_eq!((st_g, code(&v_g)), (404, "app_not_found"));
    // Live: a deployment that went live.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
             VALUES (?1, 'jobs', 'cmd', '{}', '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
            params![SEED],
        )
        .unwrap();
        conn.execute("UPDATE apps SET live_seed = ?1, live_deployment = 'd-1', state = 'running' WHERE app_id = 'jobs'", params![SEED]).unwrap();
        conn.execute(
            "INSERT INTO deployments (id, app_id, kind, actor, result, started_at, live_at) VALUES ('d-1', 'jobs', 'deploy', 'owner', 'live', 100, 1760000000)",
            [],
        )
        .unwrap();
    }
    let g = surfaces::garden_call("app.self", &[json!({ "app": "jobs" })]).unwrap();
    assert_row_shape("app.self", &g);
    assert_eq!((g["liveSeed"].as_str(), g["deployment"].as_str(), g["deployedAt"].as_i64()), (Some(SEED), Some("d-1"), Some(1_760_000_000)));
    assert_eq!(g["health"], "stopped", "no live instance on this box: stopped, whatever the row says");
    let e = surfaces::garden_call("app.self", &[json!({ "app": "ghost" })]).unwrap_err();
    assert_eq!(e.code(), "not_found");
    assert_eq!(surfaces::garden_call("app.self", &[json!({})]).unwrap_err().code(), "bad_request");
    assert_eq!(surfaces::garden_call("app.self", &[json!({ "app": "jobs", "x": 1 })]).unwrap_err().code(), "bad_request");
}

#[test]
fn app_usage_counts_its_own_audit_rows_with_catalog_verbs() {
    let f = fx("usage");
    let now = chrono::Utc::now().timestamp_millis();
    let row = |app: &str, verb: &str, result: u16, at_ms: i64| k2_core::app_audit::Call {
        at_ms,
        app_id: app.into(),
        actor: "app".into(),
        verb: verb.into(),
        result,
        ..Default::default()
    };
    {
        let conn = k2_core::app_audit::open(&k2_core::app_audit::db_path()).unwrap();
        k2_core::app_audit::append(
            &conn,
            &[
                row("jobs", "GET /cli/thread", 200, now - 1000),
                row("jobs", "GET /cli/thread", 403, now - 900),
                row("jobs", "GET /cli/overlay/events", 101, now - 800),
                row("jobs", "GET /cli/nope", 404, now - 700),
                row("jobs", "GET /cli/skin/me", 200, now - 30 * 3_600_000),
                row("other", "GET /cli/thread", 200, now - 500),
            ],
        )
        .unwrap();
    }
    let (st, v) = owner_get("/cli/app/usage", &Who::Owner, &[("app", "jobs")]);
    assert_eq!(st, 200, "{v}");
    let v = unwrapped(v);
    assert_row_shape("app.usage", &v);
    assert_eq!(v["hours"], 24);
    assert_eq!((v["totals"]["calls"].as_i64(), v["totals"]["denied"].as_i64()), (Some(4), Some(2)), "{v}");
    let verbs: BTreeMap<String, (J, i64, i64)> = v["verbs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["route"].as_str().unwrap().to_string(), (x["verb"].clone(), x["calls"].as_i64().unwrap(), x["denied"].as_i64().unwrap())))
        .collect();
    assert_eq!(verbs["GET /cli/thread"], (json!("thread.page"), 2, 1));
    assert_eq!(verbs["GET /cli/overlay/events"], (json!("thread.events"), 1, 0), "a socket names its subscribe verb");
    assert_eq!(verbs["GET /cli/nope"], (J::Null, 1, 1), "a refused route isn't a verb");
    assert!(!verbs.contains_key("GET /cli/skin/me"), "older than the window");
    assert_eq!(v["quotas"]["rps"], 50, "the defaults: {v}");
    // A longer window; bad windows are refused.
    let (_, v) = owner_get("/cli/app/usage", &Who::Owner, &[("app", "jobs"), ("hours", "48")]);
    assert_eq!(v["totals"]["calls"], 5);
    for bad in ["0", "721", "soon"] {
        let (st, v) = owner_get("/cli/app/usage", &Who::Owner, &[("app", "jobs"), ("hours", bad)]);
        assert_eq!((st, code(&v)), (400, "bad_request"), "hours={bad}");
    }
    // The Garden: the same answer for a local App.
    let g = surfaces::garden_call("app.usage", &[json!({ "app": "jobs", "hours": 48 })]).unwrap();
    assert_row_shape("app.usage", &g);
    assert_eq!(g["totals"]["calls"], 5);
    assert_eq!(surfaces::garden_call("app.usage", &[json!({ "app": "jobs", "hours": 0 })]).unwrap_err().code(), "bad_request");
    // An agent of another workspace can't read it.
    let (st, v) = owner_get("/cli/app/usage", &agent(&f.other), &[("app", "jobs")]);
    assert_eq!((st, code(&v)), (404, "app_not_found"));
}

#[test]
fn garden_room_members_binds_local_rooms_only() {
    let f = fx("garden");
    let e = surfaces::garden_call("room.members", &[json!({ "workspace": "nowhere" })]).unwrap_err();
    assert_eq!(e.code(), "not_bound");
    let e = surfaces::garden_call("room.members", &[json!({ "workspace": f.room, "app": "ghost" })]).unwrap_err();
    assert_eq!(e.code(), "not_found");
    let v = surfaces::garden_call("room.members", &[json!({ "workspace": f.room, "app": "jobs" })]).unwrap();
    assert_eq!(usernames(&v), [json!("dee")]);
    assert_eq!(surfaces::garden_call("room.members", &[]).unwrap_err().code(), "bad_request");
    assert_eq!(surfaces::garden_call("room.members", &[json!(1)]).unwrap_err().code(), "bad_request");
}

// ── events ───────────────────────────────────────────────────────────

/// R16 (0.46.4): `app_deployed` says what went live, so a client (and the
/// App's agent) can tell K2's automatic re-host from a deploy.
#[test]
fn deployed_event_says_what_went_live() {
    let f = fx("deploykind");
    let mut rx = crate::session_events::subscribe();
    surfaces::emit_deployed("jobs", store::KIND_REHOST);
    surfaces::emit_deployed("jobs", store::KIND_DEPLOY);
    surfaces::emit_deployed("jobs", store::KIND_AUTO_ROLLBACK);
    let mut kinds = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let crate::session_events::SessionEvent::AppDeployed { app_id, project_id, deploy_kind, .. } = e {
            if app_id == "jobs" && project_id == f.room {
                kinds.push(deploy_kind);
            }
        }
    }
    assert_eq!(kinds, ["rehost", "deploy", "auto_rollback"]);
}

#[test]
fn members_changes_emit_on_the_bus_and_map_per_room() {
    let f = fx("events");
    surfaces::install_listener();
    let mut rx = crate::session_events::subscribe();
    k2_core::skin::members::set_names_hidden(&f.room, true).unwrap();
    k2_core::skin::add_principal("eve").unwrap();
    let mut rooms = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let crate::session_events::SessionEvent::RoomMembersChanged { room } = e {
            rooms.push(room);
        }
    }
    assert!(rooms.contains(&Some(f.room.clone())), "the hide switch names its room: {rooms:?}");
    assert!(rooms.contains(&None), "a guest change is any room: {rooms:?}");

    use crate::activity_events_ws::{app_deployed_frame, members_changed_frame};
    use crate::session_events::SessionEvent;
    let any = SessionEvent::RoomMembersChanged { room: None };
    let mine = SessionEvent::RoomMembersChanged { room: Some(f.room.clone()) };
    let theirs = SessionEvent::RoomMembersChanged { room: Some(f.other.clone()) };
    assert_eq!(members_changed_frame(&f.room, "sales", &any).as_deref(), Some(r#"{"kind":"room_members_changed","workspace":"sales"}"#));
    assert!(members_changed_frame(&f.room, "sales", &mine).is_some());
    assert!(members_changed_frame(&f.room, "sales", &theirs).is_none(), "never another room's");

    let dep = SessionEvent::AppDeployed {
        app_id: "jobs".into(),
        service_id: "svc-jobs".into(),
        project_id: f.room.clone(),
        seed: Some("abc".into()),
        deployment: Some("d-1".into()),
        deploy_kind: "rehost".into(),
    };
    // R16 (0.46.4): owner clients read what went live as `deployKind`; the
    // event tag stays `kind`. The App's activity frame below is unchanged.
    let wire: J = serde_json::to_value(&dep).unwrap();
    assert_eq!(wire["kind"], "app_deployed", "the event tag: {wire}");
    assert_eq!(wire["deployKind"], "rehost", "what went live: {wire}");
    assert_eq!(wire["deployment"], "d-1", "{wire}");
    let link_pass = SkinPass { session: false, app: String::new(), ..guest("", &[(&f.room, &[])]) };
    let want = r#"{"app":"jobs","kind":"app_deployed","workspace":"sales"}"#;
    let frame = app_deployed_frame(&f.room, "sales", Some(&link_pass), Some("jobs"), &dep).expect("its own link pass");
    let got: J = serde_json::from_str(&frame).unwrap();
    assert_eq!(got, serde_json::from_str::<J>(want).unwrap());
    assert!(app_deployed_frame(&f.room, "sales", None, None, &dep).is_some(), "the owner");
    assert!(app_deployed_frame(&f.room, "sales", Some(&guest("svc-jobs", &[])), None, &dep).is_some(), "the App's guest");
    assert!(app_deployed_frame(&f.room, "sales", Some(&guest("svc-other", &[])), None, &dep).is_none(), "another App's guest");
    assert!(app_deployed_frame(&f.room, "sales", Some(&link_pass), Some("other"), &dep).is_none(), "another App's link");
    assert!(app_deployed_frame(&f.other, "ops", None, None, &dep).is_none(), "another room");
    let frame: J = serde_json::from_str(&frame).unwrap();
    let mut out = Vec::new();
    conforms(&k2_core::contract::catalog().verb("activity.events").unwrap().response, &frame, "activity.events", &mut out);
    assert!(out.is_empty(), "{out:?}");
}
