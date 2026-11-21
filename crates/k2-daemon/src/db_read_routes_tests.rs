//! `k2 db browse`, `k2 db request-read` and the `db.read` card end to end on
//! the fake Postgres (`FakeSystemOps`): the reader finds another
//! workspace's database by name (never a DSN), asks, a Member and the agent
//! itself can't answer, Deny changes nothing, an Admin's Allow grants read
//! (and only read) on exactly that database through `/cli/db/grant`'s own
//! entry, and `k2 db dsn --db` then answers the reader's read-only login.
//! That the login really can't write is `db_read_real_pg` (a real Postgres).
//! Test DB, temp HOME; no agent is woken (turns go to `inject`'s sink).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, ParamKind, Proof};
use serde_json::{json, Value};

use super::*;
use crate::permission::actions::held::tests::{agent, answer_as, below_floor, card_of, id_of, last_turn, login, row, seed, view, Ws};
use crate::sql::sysops::FakeSystemOps;

fn fake() -> &'static FakeSystemOps {
    Box::leak(Box::new(FakeSystemOps::baked()))
}

/// A running sidecar on the scoped DB (as `sql::tests::seed_running_sidecar`).
fn running() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for t in ["sql_grants", "sql_server", "sql_clusters", "sql_databases"] {
        conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
    }
    conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', 16, 'localhost', 1)", [])
        .unwrap();
}

fn create(ops: &'static FakeSystemOps, ws: &Ws, client_id: Option<&str>, name: &str) -> String {
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let v = crate::sql::ops::create_database(ops, &secrets, &ws.id, 10, client_id, Some(name)).expect("create");
    v["name"].as_str().expect("name").to_string()
}

/// An App of `ws` (its row only: the database list reads nothing else).
fn app_of(ws: &Ws, app: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let svc = k2_core::published_services::insert(
        &conn,
        &ws.id,
        app,
        k2_core::published_services::CMD_APP_SENTINEL,
        "/tmp/k2-dbr-app",
        0,
        k2_core::published_services::EXPOSE_LOCAL,
        k2_core::published_services::DESIRED_STOPPED,
        k2_core::published_services::KIND_CMD,
        "",
    )
    .unwrap();
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id: app,
            service_id: &svc.id,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "container",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .unwrap();
}

fn browse_as(ws: Option<&Ws>, extra: &[(&str, &str)]) -> (String, Value) {
    let mut p = HashMap::new();
    if let Some(ws) = ws {
        p.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        p.insert("project_id".to_string(), ws.id.clone());
    }
    for (k, v) in extra {
        p.insert((*k).to_string(), (*v).to_string());
    }
    let r = crate::sql_routes::dispatch(BROWSE, &p).expect("claimed");
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn ask(who: &Who, body: Value) -> (String, Value) {
    let r = handle_request_read(who, true, body.to_string().as_bytes());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn grant_of(db: &str, ws: &Ws) -> Option<(String, i64)> {
    let h = k2_core::db::shared();
    let c = h.lock();
    c.query_row(
        "SELECT g.level, g.can_manage FROM sql_grants g JOIN sql_databases d ON d.id = g.database_id WHERE d.name = ?1 AND g.project_id = ?2",
        rusqlite::params![db, ws.id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// No DSN, password or credential shape anywhere in `text`.
fn no_secret(what: &str, text: &str) {
    for bad in ["postgres://", "postgresql://", "password", "PGPASSWORD", "_migrator:"] {
        assert!(!text.contains(bad), "{what} shows {bad:?}: {text}");
    }
}

fn cards_on(ws: &Ws) {
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, true).expect("the agent's cards switch");
}

#[test]
fn the_card_is_registered_at_the_admin_floor_with_its_database_value() {
    let _db = k2_core::db::scoped_for_test();
    let r = crate::permission::actions::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime));
    let params: Vec<(&str, ParamKind, bool)> = r.spec.params.iter().map(|p| (p.key, p.kind, p.required)).collect();
    assert_eq!(params, [("db", ParamKind::Database, true)], "the reader is never a value: it comes from the passport");
    for (path, post) in [("/cli/db/grant", true), (REQUEST_READ, true), (BROWSE, false)] {
        let row = crate::routes::route_policy::lookup(path).unwrap_or_else(|| panic!("{path} has a policy row"));
        assert!(if post { row.post.is_some() } else { row.get.is_some() }, "{path}");
    }
    // A GET of the ask is 405 on the TCP door's chain; the handler says so too.
    assert_eq!(crate::sql_routes::dispatch(REQUEST_READ, &HashMap::new()).expect("claimed").status, "405 Method Not Allowed");
    assert_eq!(handle_request_read(&Who::Owner, false, b"{}").status, "405 Method Not Allowed");
}

#[test]
fn browse_names_every_database_its_owner_and_apps_and_never_a_dsn() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    running();
    let ops = fake();
    let owner = seed("dbrown");
    let reader = seed("dbrread");
    let app = format!("dbr-app-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    app_of(&owner, &app);
    let (jobs, notes) = crate::sql::routes::with_fake_ops(ops, || {
        (create(ops, &owner, Some(&format!("app-{app}")), "dbr_jobs"), create(ops, &owner, None, "dbr_notes"))
    });

    let (s, v) = browse_as(Some(&reader), &[]);
    assert_eq!(s, "200 OK", "{v}");
    no_secret("browse", &v.to_string());
    let rows = v["databases"].as_array().expect("databases");
    assert_eq!(rows.len(), 2, "{v}");
    let jobs_row = rows.iter().find(|r| r["name"] == jobs).expect("jobs listed");
    assert_eq!((&jobs_row["owner"], &jobs_row["apps"], &jobs_row["yours"]), (&json!(owner.handle), &json!([app]), &Value::Null), "{v}");
    let keys: Vec<&str> = jobs_row.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys.len(), 5, "names, owner, Apps and what you hold only: {keys:?}");
    assert!(v["hint"].as_str().unwrap().contains("db request-read --db <name>"), "{v}");

    // The owner sees its own; an App filter keeps that App's database.
    let (_, mine) = browse_as(Some(&owner), &[]);
    assert!(mine["databases"].as_array().unwrap().iter().all(|r| r["yours"] == "owner"), "{mine}");
    let (_, only) = browse_as(Some(&reader), &[("app", &app)]);
    let names: Vec<&str> = only["databases"].as_array().unwrap().iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(names, [jobs.as_str()], "{only}");
    let (s, none) = browse_as(Some(&reader), &[("app", "no-such-app-xyz")]);
    assert_eq!((s.as_str(), &none["error"]["code"]), ("404 Not Found", &json!(codes::APP_NOT_FOUND)), "{none}");
    // A person sees them all, holding nothing as a workspace.
    let (_, person) = browse_as(None, &[]);
    assert!(person["databases"].as_array().unwrap().iter().any(|r| r["name"] == notes), "{person}");
}

#[test]
fn the_reader_asks_deny_changes_nothing_and_an_admins_allow_grants_read_on_exactly_that_database() {
    let _home = crate::test_support::TempHome::new();
    let _cards = k2_core::test_env::EnvVar::set("K2_PERMISSION_CARDS", "1");
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    running();
    let ops = fake();
    let owner = seed("dbaown");
    let reader = seed("dbaread");
    let app = format!("dba-app-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    app_of(&owner, &app);
    let (jobs, other) = crate::sql::routes::with_fake_ops(ops, || {
        (create(ops, &owner, Some(&format!("app-{app}")), "dba_jobs"), create(ops, &owner, None, "dba_other"))
    });
    let me = agent(&reader);

    // A person can't ask (they grant), the owner's own database needs no card.
    let (s, v) = ask(&Who::Owner, json!({ "db": jobs }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!(codes::AGENT_VERB)), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("db grant <workspace> --db <name> --level read"), "{v}");
    let (s, v) = ask(&agent(&owner), json!({ "db": jobs }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!(codes::OWN)), "{v}");

    // Cards off for this agent: a clear refusal naming the switch and the grant.
    let (s, v) = ask(&me, json!({ "db": jobs }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!(codes::CARDS_OFF)), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("Let this agent ask for permission") && hint.contains(&format!("db grant {} --db {jobs} --level read", reader.handle)), "{hint}");
    cards_on(&reader);

    // Names that don't hold are refused before any card.
    let (s, v) = ask(&me, json!({ "db": "no_such_db_xyz" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("404 Not Found", &json!(codes::NOT_FOUND)), "{v}");
    let (s, v) = ask(&me, json!({}));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!(codes::USAGE)), "{v}");

    // The ask by App: its one database. The card says K2's words, no secret.
    let (s, v) = ask(&me, json!({ "app": app, "why": "verify the jobs import" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("pending")), "{v}");
    assert_eq!((&v["db"], &v["action"]), (&json!(jobs), &json!(ACTION)), "{v}");
    let card = card_of(&v);
    let id = id_of(&v);
    let vw = view(&reader, &card);
    assert_eq!(vw["whatChanges"], format!("Let {} read the {jobs} database (owned by {}, App {app}).", reader.handle, owner.handle), "{vw}");
    assert!(vw["consequences"].as_str().unwrap().contains("read-only login"), "{vw}");
    let db_id = v["dbId"].as_str().unwrap().to_string();
    assert!(vw["scope"].as_str().unwrap().contains(&db_id), "the card's words pin the database's id: {vw}");
    no_secret("the card", &vw.to_string());
    no_secret("the ask's answer", &v.to_string());
    assert_eq!(row(&id).params, json!({ "db": jobs }), "the card names the database by its unique name");
    // The same ask again is the open card.
    let (_, again) = ask(&me, json!({ "db": jobs }));
    assert_eq!((&again["status"], &again["id"]), (&json!("already_pending"), &json!(id)), "{again}");

    // A Member is below the floor; the agent itself never answers; nothing changes.
    let member = login("dbam", Role::Member);
    crate::sql::routes::with_fake_ops(ops, || below_floor(answer_as(&member, &reader, &card, json!({ "answer": "Allow" })), "admin"));
    let mut stamped = HashMap::new();
    stamped.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
    stamped.insert("project_id".to_string(), reader.id.clone());
    let own = json!({ "addr": reader.handle, "id": card, "answer": "Allow" }).to_string();
    let r = crate::sql::routes::with_fake_ops(ops, || {
        crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &stamped, own.as_bytes(), &reader.handle)
    });
    assert_eq!(r.status, "403 Forbidden", "the asking agent can't allow its own card: {}", r.body);
    assert_eq!(row(&id).state.as_str(), "pending", "still waiting for a person");
    assert_eq!(grant_of(&jobs, &reader), None);

    // Deny on the other database's card: nothing granted, there or here.
    let (_, v2) = ask(&me, json!({ "db": other }));
    let (s, d) = crate::sql::routes::with_fake_ops(ops, || answer_as("owner", &reader, &card_of(&v2), json!({ "answer": "Deny" })));
    assert_eq!(s, "200 OK", "{d}");
    assert_eq!(row(&id_of(&v2)).state.as_str(), "denied", "{d}");
    assert_eq!(grant_of(&other, &reader), None, "Deny changes nothing");

    // An Admin taps Allow: read, no manage, on jobs only.
    let admin = login("dbaa", Role::Admin);
    let (s, a) = crate::sql::routes::with_fake_ops(ops, || answer_as(&admin, &reader, &card, json!({ "answer": "Allow" })));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert_eq!(grant_of(&jobs, &reader), Some(("read".to_string(), 0)), "read, never manage");
    assert_eq!(grant_of(&other, &reader), None, "exactly the database the card names");
    let turn = last_turn(&reader);
    assert!(turn.contains(&format!("db dsn --db {jobs} gives your read-only DSN")), "{turn}");
    no_secret("the agent's turn", &turn);

    // Asking again: already done. Browse says so; dsn answers the reader's login.
    let (s, v) = ask(&me, json!({ "db": jobs }));
    assert_eq!((s.as_str(), &v["status"], &v["level"]), ("200 OK", &json!("already_done"), &json!("read")), "{v}");
    let (_, b) = browse_as(Some(&reader), &[]);
    assert!(b["databases"].as_array().unwrap().iter().any(|r| r["name"] == jobs && r["yours"] == "read"), "{b}");
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let dsn = crate::sql::routes::with_fake_ops(ops, || crate::sql::ops::dsn_for_db(ops, &secrets, &reader.id, 10, &jobs)).expect("a dsn");
    let user = crate::sql::ops::default_agent_role(&reader.id);
    assert!(dsn["dsn"].as_str().unwrap().contains(&format!("://{user}:")), "the reader's own login, never the owner's: {dsn}");
}

#[test]
fn read_only_connect_hands_the_read_dsn_without_its_password_and_refuses_a_writer() {
    let _home = crate::test_support::TempHome::new();
    let _cards = k2_core::test_env::EnvVar::set("K2_PERMISSION_CARDS", "1");
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    running();
    let ops = fake();
    let owner = seed("dbcown");
    let reader = seed("dbcread");
    let writer = seed("dbcwrite");
    let app = format!("dbc-app-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    app_of(&owner, &app);
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let jobs = crate::sql::routes::with_fake_ops(ops, || {
        let n = create(ops, &owner, Some(&format!("app-{app}")), "dbc_jobs");
        crate::sql::ops::grant_access(ops, &secrets, None, &n, &reader.id, "read", false, &[]).expect("read grant");
        crate::sql::ops::grant_access(ops, &secrets, None, &n, &writer.id, "write", false, &[]).expect("write grant");
        n
    });

    let (s, v) = crate::sql::routes::with_fake_ops(ops, || ask(&agent(&reader), json!({ "app": app, "connect": true })));
    assert_eq!((s.as_str(), &v["status"], &v["readOnly"]), ("200 OK", &json!("read_only"), &json!(true)), "{v}");
    let dsn = v["dsn"].as_str().expect("dsn");
    let creds = dsn.split("://").nth(1).and_then(|r| r.rsplit_once('@')).map(|(c, _)| c).expect("user@host");
    assert!(!creds.contains(':'), "no password in the answer: {dsn}");
    assert_eq!(v["dsnHasPassword"], false, "{v}");

    let (s, v) = crate::sql::routes::with_fake_ops(ops, || ask(&agent(&writer), json!({ "app": app, "connect": true })));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!(codes::READ_ONLY_UNAVAILABLE)), "{v}");
    // The App's own workspace: its own database has no read-only login.
    let (s, v) = ask(&agent(&owner), json!({ "app": app, "connect": true }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!(codes::OWN)), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains(&format!("db dsn --db {jobs}")), "{v}");
}

#[test]
fn an_app_with_two_databases_needs_one_named_and_assign_asks_one_person() {
    let _home = crate::test_support::TempHome::new();
    let _cards = k2_core::test_env::EnvVar::set("K2_PERMISSION_CARDS", "1");
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    running();
    let ops = fake();
    let owner = seed("dbtown");
    let reader = seed("dbtread");
    cards_on(&reader);
    let app = format!("dbt-app-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    app_of(&owner, &app);
    let (a, b) = crate::sql::routes::with_fake_ops(ops, || (create(ops, &owner, Some(&format!("app-{app}")), "dbt_a"), create(ops, &owner, None, "dbt_b")));
    // The App connected to b as well (a second name).
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        let b_id: String = c.query_row("SELECT id FROM sql_databases WHERE name = ?1", [&b], |r| r.get(0)).unwrap();
        c.execute(
            "INSERT INTO app_db_connections (app_id, as_name, env, kind, database_id, database_name, database_project, group_role, login_role, inherit_from, created_by, created_at) \
             VALUES (?1, 'ANALYTICS_URL', 'production', 'box', ?2, ?3, ?4, 'g', 'l', 'i', 'owner', 1)",
            rusqlite::params![app, b_id, b, owner.id],
        )
        .unwrap();
    }
    let me = agent(&reader);
    let (s, v) = ask(&me, json!({ "app": app }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("409 Conflict", &json!(codes::APP_DB_AMBIGUOUS)), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains(&a) && v["error"]["hint"].as_str().unwrap().contains(&b), "{v}");
    let (s, v) = ask(&me, json!({ "app": app, "db": "dbt_nope" }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("404 Not Found", &json!(codes::APP_DB_NOT_ITS)), "{v}");

    // --assign: only that person is asked; a login that can't answer is refused.
    let ada = login("dbtada", Role::Admin);
    let (s, v) = ask(&me, json!({ "app": app, "db": b, "assign": ada }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("pending")), "{v}");
    assert_eq!(v["routedTo"], json!([{ "role": "admin", "login": ada }]), "{v}");
    let member = login("dbtmem", Role::Member);
    let (s, v) = ask(&me, json!({ "db": a, "assign": member }));
    assert_eq!((s.as_str(), &v["error"]["code"]), ("400 Bad Request", &json!("approver_not_eligible")), "{v}");
    // A dry run files nothing.
    let (s, v) = ask(&me, json!({ "db": a, "dryRun": true }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("dry_run")), "{v}");
    assert!(v["text"].as_str().unwrap().contains(&format!("read the {a} database")), "{v}");
}
