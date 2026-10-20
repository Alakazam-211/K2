//! Slice B of prd-agent-app-permissions-v1 (0.46.3), in process: the App
//! cores. A cap in a ticked core applies (switch 1, any caller); an
//! unticked core is held even with switch 2, naming the core to tick; the
//! agent-reaching caps are always the person's; `dbPrivate` needs the
//! Database core; the owner ticks, an Admin unticks, an agent never; every
//! change is audited and noted (never a wake); the box view and
//! `k2 app access` show the cores; 0197's backfill is corrected once.

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::connect_users::Role;
use k2_core::deploy::store;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::access::{self, Level};
use super::routes::Who;
use crate::sidecar_routes::Caller;

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

const S_TF: &str = "2222222222222222222222222222222222222222222222222222222222222222"; // thread:read, files:write
const S_T: &str = "1111111111111111111111111111111111111111111111111111111111111111"; // thread:read
const S_TFD: &str = "3333333333333333333333333333333333333333333333333333333333333333"; // + store:read
const S_HB: &str = "4444444444444444444444444444444444444444444444444444444444444444"; // thread:read, heartbeats:write

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

fn ws_path(ws: &str) -> String {
    format!("/tmp/k2-cores-test/{ws}")
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, ws_path(ws)]).unwrap();
        }
        for (app, ws, iso) in [("jobs", &ws_a, "container"), ("other", &ws_b, "container"), ("plain", &ws_a, "process")] {
            let svc = k2_core::published_services::insert(
                &conn,
                ws,
                app,
                k2_core::published_services::CMD_APP_SENTINEL,
                &format!("/tmp/k2-cores-test/apps/{app}"),
                0,
                k2_core::published_services::EXPOSE_LOCAL,
                k2_core::published_services::DESIRED_STOPPED,
                k2_core::published_services::KIND_CMD,
                "",
            )
            .unwrap();
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &svc.id,
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: iso,
                    memory_limit_bytes: Some(256 << 20),
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        for (hash, caps) in [
            (S_TF, json!(["thread:read", "files:write"])),
            (S_T, json!(["thread:read"])),
            (S_TFD, json!(["thread:read", "files:write", "store:read"])),
            (S_HB, json!(["thread:read", "heartbeats:write"])),
        ] {
            conn.execute(
                "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
                 VALUES (?1, 'jobs', 'cmd', ?2, '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
                params![hash, json!({ "caps": caps }).to_string()],
            )
            .unwrap();
        }
    }
    MSGS.lock().unwrap().clear();
    super::alerts::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn set_level(ws: &str, l: Level) {
    let (b, f) = match l {
        Level::Off => (0, 0),
        Level::Build => (1, 0),
        Level::Full => (1, 1),
    };
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_apps_full = ?3 WHERE id = ?1", params![ws, b, f]).unwrap();
}

fn set_cores(ws: &str, cores: &[&str]) {
    let v: Vec<String> = cores.iter().map(|c| c.to_string()).collect();
    k2_core::workspace::settings::set_agents_app_cores(ws, &v).unwrap();
}

fn call(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::routes::handle(path, who.clone(), b"", &p);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn access_set(caller: Caller, body: J) -> (u16, J) {
    let r = crate::sidecar_routes::handle("/cli/agent-access/set", caller, body.to_string().as_bytes(), &HashMap::new(), "test");
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("access set: not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn recorded(app: &str) -> Option<Vec<String>> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    super::grants::recorded(&conn, app).unwrap()
}

fn audit_events(app: &str) -> Vec<(String, J)> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, app, 100)
        .unwrap()
        .into_iter()
        .map(|r| (r.event, r.payload))
        .collect()
}

fn deploy(who: &Who, seed: &str) -> (u16, J) {
    call("/cli/app/deploy", who, json!({"app": "jobs", "seed": seed}))
}

fn is_held(st: u16, v: &J) -> bool {
    st == 202 && v["status"] == "held"
}

/// §3 Use layer: the ticked cores pre-approve new caps (first install and
/// later deploys) under switch 1; an unticked core is held even with
/// switch 2, and the hold names the core to tick.
#[test]
fn ticked_cores_pre_approve_new_caps_and_an_unticked_core_is_held() {
    let f = fx("use");
    set_level(&f.ws_a, Level::Build);
    let a = agent(&f.ws_a);

    // No cores: a first install asking for Thread and Files is held, and
    // the hold says which cores to tick.
    let (st, v) = deploy(&a, S_TF);
    assert!(is_held(st, &v), "{st} {v}");
    assert_eq!(v["kind"], "install", "{v}");
    assert_eq!(v["diff"]["cores"]["missing"], json!(["files", "thread"]), "{v}");
    let hint = v["hint"].as_str().unwrap();
    assert!(hint.starts_with("Tick Files and Thread for "), "{hint}");
    assert!(hint.contains("--core files=on,thread=on"), "{hint}");
    let reasons = v["reasons"].to_string();
    assert!(reasons.contains("tick Files and Thread for"), "{reasons}");
    assert_eq!(recorded("jobs"), None, "nothing recorded while held");

    // Thread alone isn't enough. (The same pending request is found again,
    // so its stored diff stays as filed; the hint is today's.)
    set_cores(&f.ws_a, &["thread"]);
    let (st, v) = deploy(&a, S_TF);
    assert!(is_held(st, &v), "{st} {v}");
    let hint = v["hint"].as_str().unwrap();
    assert!(hint.starts_with("Tick Files for ") && hint.contains("--core files=on, the owner"), "{hint}");

    // Both ticked: the first install records its set and goes on.
    set_cores(&f.ws_a, &["thread", "files"]);
    let (st, v) = deploy(&a, S_TF);
    assert!(!is_held(st, &v), "pre-approved, never held: {st} {v}");
    assert_eq!(recorded("jobs"), Some(vec!["files:write".to_string(), "thread:read".to_string()]));
    let ev = audit_events("jobs");
    let pre = ev.iter().find(|(e, _)| e == "caps_preapproved").expect("a caps_preapproved audit row");
    assert_eq!(pre.1["first"], json!(true), "{:?}", pre.1);
    assert_eq!(pre.1["cores"], json!(["files", "thread"]), "{:?}", pre.1);
    // The install held before the ticks (same Seed) is withdrawn: approving
    // it later can't redeploy an old Seed.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let rows = k2_core::app_grace::requests::list(
            &conn,
            &k2_core::app_grace::requests::Filter { app_id: Some("jobs"), ..Default::default() },
            50,
        )
        .unwrap();
        let installs: Vec<(&str, Option<&str>)> =
            rows.iter().filter(|r| r.kind == "install").map(|r| (r.status.as_str(), r.confirm_via.as_deref())).collect();
        assert_eq!(installs, vec![("withdrawn", Some("cores"))], "{rows:?}");
    }

    // A later deploy adding a Database cap: held without the Database
    // core, switch 2 included (switch 2 never adds a core).
    set_level(&f.ws_a, Level::Full);
    let (st, v) = deploy(&a, S_TFD);
    assert!(is_held(st, &v), "{st} {v}");
    assert_eq!((v["kind"].as_str(), v["diff"]["cores"]["missing"].clone()), (Some("deploy"), json!(["db"])), "{v}");
    assert!(v["hint"].as_str().unwrap().starts_with("Tick Database for "), "{v}");

    // Database ticked, back to switch 1: applies at once.
    set_level(&f.ws_a, Level::Build);
    set_cores(&f.ws_a, &["thread", "files", "db"]);
    let (st, v) = deploy(&a, S_TFD);
    assert!(!is_held(st, &v), "{st} {v}");
    assert_eq!(
        recorded("jobs"),
        Some(vec!["files:write".to_string(), "store:read".to_string(), "thread:read".to_string()])
    );

    // Ticking Mail and Sew covers no App cap today.
    let n = super::cores::need(&["mail".to_string(), "sew".to_string()], &["thread:read".to_string()]);
    assert_eq!(n.cores, vec!["thread"]);
}

/// The cores pre-approve only while switch 1 is on, for whoever deploys
/// (the owner token included); `heartbeats:write` reaches the agent itself
/// and is always the person's, every core ticked and switch 2 on.
#[test]
fn cores_need_switch_1_and_never_cover_the_agent_reaching_caps() {
    let f = fx("floor");
    set_cores(&f.ws_a, &["db", "files", "thread", "mail", "sew"]);
    // Switch 1 off: the owner's own deploy is held as before (S20).
    let (st, v) = deploy(&Who::Owner, S_T);
    assert!(is_held(st, &v), "{st} {v}");
    assert!(v["reasons"].to_string().contains("\\\"Build and run Apps\\\" is off for"), "{v}");
    assert!(v["diff"]["cores"]["missing"].as_array().unwrap().is_empty(), "{v}");
    // Switch 1 on: the same deploy by the owner token goes on.
    set_level(&f.ws_a, Level::Build);
    let (st, v) = deploy(&Who::Owner, S_T);
    assert!(!is_held(st, &v), "{st} {v}");
    assert_eq!(recorded("jobs"), Some(vec!["thread:read".to_string()]));
    // heartbeats:write: held for the person, switch 2 and every core on.
    set_level(&f.ws_a, Level::Full);
    let (st, v) = deploy(&agent(&f.ws_a), S_HB);
    assert!(is_held(st, &v), "{st} {v}");
    assert_eq!(v["diff"]["cores"]["person"], json!(["heartbeats:write"]), "{v}");
    assert!(v["diff"]["cores"]["missing"].as_array().unwrap().is_empty(), "{v}");
    assert!(v["reasons"].to_string().contains("heartbeats:write reaches the agent itself"), "{v}");
    assert!(!v["hint"].as_str().unwrap().starts_with("Tick"), "no tick helps: {v}");
    assert_eq!(recorded("jobs"), Some(vec!["thread:read".to_string()]), "unchanged");
    // Another workspace's ticks never cover this App.
    set_cores(&f.ws_a, &[]);
    set_cores(&f.ws_b, &["db", "files", "thread"]);
    set_level(&f.ws_b, Level::Full);
    let (st, v) = deploy(&agent(&f.ws_a), S_TF);
    assert!(is_held(st, &v), "{st} {v}");
}

/// §3.1 Database: the App's own private database applies for an agent at
/// switch 1 with Database ticked; without it, it waits (switch 2
/// included); `dbRaw` stays the person's.
#[test]
fn db_private_needs_the_database_core() {
    let f = fx("dbpriv");
    set_level(&f.ws_a, Level::Full);
    let a = agent(&f.ws_a);
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "dbPrivate": "on"}));
    assert!(is_held(st, &v), "switch 2, no Database core: {st} {v}");
    assert!(v.to_string().contains("the Database core isn't ticked for"), "{v}");
    set_level(&f.ws_a, Level::Build);
    set_cores(&f.ws_a, &["db"]);
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "dbPrivate": "on"}));
    assert_eq!(st, 200, "switch 1 + Database: {v}");
    assert!(v["applied"].to_string().contains("private database on"), "{v}");
    let (st, v) = call("/cli/app/config", &a, json!({"app": "jobs", "dbRaw": "on"}));
    assert!(is_held(st, &v), "dbRaw loosens the wall: {st} {v}");
    // Turning it off narrows, ticked or not.
    set_cores(&f.ws_a, &[]);
    let (st, v) = call("/cli/app/config", &a, json!({"app": "plain", "dbPrivate": "off"}));
    assert_eq!(st, 200, "{v}");
    let _ = &f.ws_b;
}

/// Q2 for cores: the owner ticks, an Admin only unticks, an agent (or a
/// shell tab) never; unknown cores are refused; every change writes an
/// `access_changed` row and leaves the agent a note naming the cores
/// (never a wake); no change, no note.
#[test]
fn the_owner_ticks_an_admin_unticks_and_agents_never() {
    let f = fx("writer");
    k2_core::connect_users::add_user("ada", "Correct-Horse-9-Battery").unwrap();
    k2_core::connect_users::set_role("ada", Role::Admin).unwrap();
    k2_core::connect_users::add_user("rosson", "Correct-Horse-9-Battery").unwrap();
    k2_core::connect_users::set_role("rosson", Role::Owner).unwrap();
    let admin = || Caller::Login { username: "ada".into() };
    let ws = f.ws_a.clone();
    let one = |core: &str, value: bool| json!({"workspace": ws, "toggle": "app_core", "core": core, "value": value});

    let ag = crate::sidecar_routes::AgentCaller { project_id: ws.clone(), map_key: None, args: vec![], sandboxed: false };
    let (st, _) = access_set(Caller::Agent(ag), one("db", true));
    assert_eq!(st, 403, "an agent never ticks its own");
    let (st, _) = access_set(Caller::Shell { project_id: ws.clone() }, one("db", true));
    assert_eq!(st, 403, "a shell tab doesn't change agent access");
    let (st, v) = access_set(admin(), one("db", true));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let (st, v) = access_set(Caller::Owner, one("postgres", true));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    let (st, v) = access_set(Caller::Owner, json!({"workspace": ws, "toggle": "app_core", "core": "db"}));
    assert_eq!((st, code(&v)), (400, "bad_request"), "a value is required: {v}");
    assert!(access::level(&ws) == Level::Off && super::cores::ticked(&ws).is_empty());
    assert!(MSGS.lock().unwrap().is_empty(), "nothing changed, no note");

    // The owner ticks several at once (one note).
    let (st, v) = access_set(
        Caller::Login { username: "rosson".into() },
        json!({"workspace": ws, "toggle": "app_core", "cores": {"db": true, "files": "on", "mail": true}}),
    );
    assert_eq!((st, v["cores"].clone(), v["changed"].as_bool()), (200, json!(["db", "files", "mail"]), Some(true)), "{v}");
    assert_eq!(v["coreList"][3]["later"], "takes effect when App mail lands", "{v}");
    {
        let msgs = MSGS.lock().unwrap();
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        let (w, t) = &msgs[0];
        assert_eq!(w, &ws);
        assert!(t.starts_with("[note] Your App access changed"), "{t}");
        assert!(t.contains("cores: Database, Files, Mail (by owner:rosson)"), "{t}");
        assert!(t.contains("Mail and Sew take effect when"), "{t}");
    }
    // An Admin unticks; ticking again in the same call is refused whole.
    let (st, v) = access_set(admin(), json!({"workspace": ws, "toggle": "app_core", "cores": {"mail": false, "thread": true}}));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    assert_eq!(super::cores::ticked(&ws), vec!["db", "files", "mail"], "a refused call changes nothing");
    let (st, v) = access_set(admin(), one("mail", false));
    assert_eq!((st, v["cores"].clone()), (200, json!(["db", "files"])), "{v}");
    assert!(MSGS.lock().unwrap().last().unwrap().1.contains("(by admin:ada)"));
    // No change, no note.
    let before = MSGS.lock().unwrap().len();
    let (st, v) = access_set(admin(), one("mail", false));
    assert_eq!((st, v["changed"].as_bool()), (200, Some(false)), "{v}");
    assert_eq!(MSGS.lock().unwrap().len(), before);
    // Every change: an access_changed row on the workspace's Apps.
    let rows: Vec<J> = audit_events("jobs").into_iter().filter(|(e, _)| e == "access_changed").map(|(_, p)| p).collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["cores"], json!({"from": ["db", "files", "mail"], "to": ["db", "files"]}), "{rows:?}");
    // The other workspace was never touched.
    assert!(super::cores::ticked(&f.ws_b).is_empty());
}

/// `k2 app access <ws>` reads the switches and cores from
/// `/cli/app/list?workspace=`; the agent sees its own; the box view shows
/// each App's cores (never what's inside) and, to people only, the
/// workspace id for the Settings row.
#[test]
fn access_reads_and_the_box_view_show_the_cores() {
    let f = fx("reads");
    set_level(&f.ws_a, Level::Build);
    set_cores(&f.ws_a, &["thread", "sew"]);
    let (st, v) = get("/cli/app/list", &Who::Owner, &[("workspace", &f.ws_a)]);
    assert_eq!(st, 200, "{v}");
    let acc = &v["access"];
    assert_eq!((acc["level"].as_str(), acc["buildAndRun"].as_bool(), acc["manageEndToEnd"].as_bool()), (Some("build"), Some(true), Some(false)), "{v}");
    assert_eq!(acc["cores"], json!(["thread", "sew"]), "{v}");
    let list = acc["coreList"].as_array().unwrap();
    assert_eq!(list.iter().map(|c| c["core"].as_str().unwrap()).collect::<Vec<_>>(), vec!["db", "files", "thread", "mail", "sew"]);
    assert_eq!(list.iter().filter(|c| c["enforced"] == json!(true)).count(), 3, "{v}");
    let (_, mine) = get("/cli/app/list", &agent(&f.ws_a), &[("workspace", &f.ws_b)]);
    assert_eq!(mine["access"]["workspaceId"], json!(f.ws_a), "an agent reads its own: {mine}");

    let (_, bx) = get("/cli/app/list", &Who::Owner, &[("box", "1")]);
    let jobs = bx["apps"].as_array().unwrap().iter().find(|r| r["appId"] == "jobs").unwrap().clone();
    assert_eq!(jobs["switches"]["cores"], json!(["thread", "sew"]), "{jobs}");
    assert_eq!(jobs["maintainer"]["workspaceId"], json!(f.ws_a), "{jobs}");
    let (_, bx) = get("/cli/app/list", &agent(&f.ws_b), &[("box", "1")]);
    let jobs = bx["apps"].as_array().unwrap().iter().find(|r| r["appId"] == "jobs").unwrap().clone();
    assert!(jobs["maintainer"].get("workspaceId").is_none(), "agents get the address, not the id: {jobs}");
    assert_eq!(jobs["switches"]["cores"], json!(["thread", "sew"]), "{jobs}");
}

/// Q4 review list: the owner or an Admin takes an agent off it (the
/// Settings → Apps banner's dismiss); an agent can't.
#[test]
fn the_review_list_dismisses_per_agent() {
    let f = fx("review");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&f.ws_a, &f.ws_b] {
            conn.execute("INSERT INTO app_access_review (project_id, created_at) VALUES (?1, 1)", params![ws]).unwrap();
        }
    }
    let ag = crate::sidecar_routes::AgentCaller { project_id: f.ws_a.clone(), map_key: None, args: vec![], sandboxed: false };
    let (st, _) = access_set(Caller::Agent(ag), json!({"workspace": f.ws_a, "toggle": "app_review", "value": false}));
    assert_eq!(st, 403);
    let (st, v) = access_set(Caller::Owner, json!({"workspace": f.ws_a, "toggle": "app_review", "value": true}));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    let (st, v) = access_set(Caller::Owner, json!({"workspace": f.ws_a, "toggle": "app_review", "value": false}));
    assert_eq!((st, v["changed"].as_bool()), (200, Some(true)), "{v}");
    let (_, list) = get("/cli/app/list", &Who::Owner, &[]);
    let left: Vec<&str> = list["accessReview"].as_array().unwrap().iter().map(|r| r["workspace"].as_str().unwrap()).collect();
    assert_eq!(left, vec![f.ws_b.as_str()], "{list}");
    let (st, v) = access_set(Caller::Owner, json!({"workspace": f.ws_a, "toggle": "app_review", "value": false}));
    assert_eq!((st, v["changed"].as_bool()), (200, Some(false)), "{v}");
}

/// 0197 mapped `heartbeats:write` to Thread and `files:agentconfig` to
/// Files. The boot fix re-derives rows still exactly as 0197 wrote them,
/// leaves rows the owner changed, and runs once.
#[test]
fn the_0197_backfill_is_corrected_once() {
    let f = fx("backfill");
    let ws_c = "ws-c-backfill".to_string();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws_c, ws_path(&ws_c)]).unwrap();
        // ws_a: only heartbeats:write + wiki:read → 0197 wrote files+thread.
        conn.execute("UPDATE apps SET approved_caps_json = '[\"wiki:read\",\"heartbeats:write\"]' WHERE app_id = 'jobs'", []).unwrap();
        conn.execute("UPDATE projects SET app_cores = '[\"files\",\"thread\"]' WHERE id = ?1", params![f.ws_a]).unwrap();
        // ws_b: the same mistake, but the owner has changed it since.
        conn.execute("UPDATE apps SET approved_caps_json = '[\"heartbeats:write\"]' WHERE app_id = 'other'", []).unwrap();
        conn.execute("UPDATE projects SET app_cores = '[\"db\",\"thread\"]' WHERE id = ?1", params![f.ws_b]).unwrap();
        // ws_c: right already (no Apps, no cores).
        for ws in [&f.ws_a, &f.ws_b, &ws_c] {
            conn.execute("INSERT INTO app_access_review (project_id, created_at) VALUES (?1, 1)", params![ws]).unwrap();
        }
    }
    let fixed = super::cores::fix_backfill_once();
    assert_eq!(fixed, vec![f.ws_a.clone()]);
    assert_eq!(super::cores::ticked(&f.ws_a), vec!["files"], "heartbeats:write ticks no core");
    assert_eq!(super::cores::ticked(&f.ws_b), vec!["db", "thread"], "the owner's change is kept");
    assert!(super::cores::ticked(&ws_c).is_empty());
    // Once.
    k2_core::workspace::settings::set_agents_app_cores(&f.ws_a, &["files".to_string(), "thread".to_string()]).unwrap();
    assert!(super::cores::fix_backfill_once().is_empty());
    assert_eq!(super::cores::ticked(&f.ws_a), vec!["files", "thread"]);
}
