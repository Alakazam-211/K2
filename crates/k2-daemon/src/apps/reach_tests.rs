//! An App's reach after a widening (0.46.11 field report): a database
//! change reaches the running App's nft rule live, `k2 app doctor <app>`
//! names what is approved but not applied, a container App is told it
//! has no database door, and a secret granted to a running App says it
//! takes a re-host.

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::deploy::store;
use serde_json::{json, Value as J};

use super::*;
use crate::apps::routes::Who;
use crate::root_helper::{test_seam as helper_seam, Kind, RootHelperState};

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

const SEED: &str = "seed-reach-0000000000000000000000000000000000000000000000000000000";

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
                rusqlite::params![ws, format!("/tmp/k2-reach-test/{ws}")],
            )
            .unwrap();
        }
        let manifest = json!({
            "schema": 1, "kind": "app", "id": "boxed", "name": "Boxed", "version": "1.0.0",
            "host": {"mode": "cmd", "start": ["./serve"]},
            "requires": {"postgres": ">=16"},
        });
        conn.execute(
            "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
             VALUES (?1, 'boxed', 'cmd', ?2, '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
            rusqlite::params![SEED, manifest.to_string()],
        )
        .unwrap();
        // ledger: the process exception (path open, isolation process),
        // the shape that reaches the host's 127.0.0.1:5432 through its rule.
        // boxed: a guarded container. stopped: the exception, not running.
        // Each App has its service row (a held config change files
        // against it; a secret is granted to it).
        for (app, iso) in [("ledger", "process"), ("boxed", "container"), ("stopped", "process")] {
            let svc = k2_core::published_services::insert(
                &conn,
                &ws_a,
                app,
                k2_core::published_services::CMD_APP_SENTINEL,
                &format!("/tmp/k2-reach-test/apps/{app}"),
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
                    owner_workspace: &ws_a,
                    isolation: iso,
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        conn.execute("UPDATE apps SET path = 'open' WHERE app_id IN ('ledger', 'stopped')", []).unwrap();
        conn.execute("UPDATE apps SET state = 'running', live_seed = ?1 WHERE app_id IN ('ledger', 'boxed')", [SEED]).unwrap();
        // Two K2 clusters: 16 on 5432, 17 on 5433. Workspace A's database is
        // on 16; workspace B's on 17.
        for t in ["sql_clusters", "sql_databases", "sql_grants"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        for (major, port) in [(16u32, 5432u16), (17, 5433)] {
            let mut c = crate::sql::cluster::Cluster::layout(major, port, if port == 5432 { "baked" } else { "created" });
            c.role = "k2".into();
            c.state = "online".into();
            crate::sql::cluster::record_in(&conn, &c, true).unwrap();
        }
        for (id, name, ws, major) in [("db-a", "dr_main", &ws_a, 16i64), ("db-b", "shared17", &ws_b, 17)] {
            conn.execute(
                "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major) VALUES (?1, ?2, ?3, 'active', 1, ?4)",
                rusqlite::params![id, ws, name, major],
            )
            .unwrap();
        }
    }
    // The root helper speaks protocol 15 with named clusters: the per-App
    // table, every rule carries `--db`.
    let mut st = RootHelperState::missing();
    st.kind = Kind::Root;
    st.protocol = 15;
    st.version_json = Some(json!({"protocol": 15, "namespaces": {"pgnamed": true}}));
    helper_seam::set(Some(st));
    MSGS.lock().unwrap().clear();
    super::super::alerts::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn grant_b_to_a(f: &Fx) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "INSERT INTO sql_grants (database_id, project_id, level, can_manage, created_at, updated_at) VALUES ('db-b', ?1, 'write', 0, 1, 1)",
        [&f.ws_a],
    )
    .unwrap();
}

fn last_rule() -> String {
    test_seams::sent().last().map(|a| a.join(" ")).unwrap_or_default()
}

fn msgs() -> Vec<String> {
    MSGS.lock().unwrap().iter().map(|(_, t)| t.clone()).collect()
}

fn app(id: &str) -> store::AppRow {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app(&conn, id).unwrap().unwrap()
}

fn svc(id: &str) -> String {
    app(id).service_id
}

fn findings(v: &J) -> Vec<String> {
    v["findings"].as_array().unwrap().iter().map(|f| f["detail"].as_str().unwrap().to_string()).collect()
}

/// The field report: a database grant lands while the App runs. Its rule
/// is re-sent at once with the new cluster's port (no re-host), the grant's
/// answer says so, and a revoke shuts it again.
#[test]
fn a_database_grant_reaches_the_running_app_live() {
    let f = fx("live");
    // The App starts: its rule names its workspace's cluster only.
    super::super::guarded::install_gateway_rule("ledger", 38_601).unwrap();
    assert_eq!(last_rule(), "apps nft app ledger 38601 --db 5432");
    assert_eq!(applied_of("ledger").unwrap().db_ports, Some(vec![5432]));
    assert_eq!(doctor_json(&app("ledger"))["ok"], true);

    grant_b_to_a(&f);
    // Before anything re-sends it, the doctor names the gap.
    let d = doctor_json(&app("ledger"));
    assert_eq!(d["ok"], false, "{d}");
    assert_eq!(
        findings(&d),
        vec!["database approved but not yet applied to the running App: Postgres port(s) 5433. Takes effect after a re-host: k2 app rehost ledger"]
    );

    // The grant's answer (k2 db grant): re-sent live, and said.
    let mut v = json!({"ok": true});
    after_db_change(&mut v);
    assert_eq!(last_rule(), "apps nft app ledger 38601 --db 5432 5433");
    assert_eq!(v["apps"][0]["note"], "App ledger: its door to Postgres port(s) 5433 opened now (no re-host needed)", "{v}");
    assert_eq!(v["apps"][0]["applied"], true);
    assert_eq!(doctor_json(&app("ledger"))["ok"], true, "nothing pending after the live re-send");
    // Nothing moved: nothing re-sent, nothing said.
    let n = test_seams::sent().len();
    let mut v = json!({"ok": true});
    after_db_change(&mut v);
    assert_eq!(test_seams::sent().len(), n);
    assert!(v.get("apps").is_none(), "{v}");

    // Revoke: shut at once.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("DELETE FROM sql_grants", []).unwrap();
    }
    let mut v = json!({"ok": true});
    after_db_change(&mut v);
    assert_eq!(last_rule(), "apps nft app ledger 38601 --db 5432");
    assert_eq!(v["apps"][0]["note"], "App ledger: Postgres port(s) 5433 shut now", "{v}");
    // A stopped App: its next start sends a fresh rule, nothing now.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE apps SET state = 'stopped' WHERE app_id = 'ledger'", []).unwrap();
    }
    grant_b_to_a(&f);
    let n = test_seams::sent().len();
    let mut v = json!({"ok": true});
    after_db_change(&mut v);
    assert_eq!(test_seams::sent().len(), n, "a stopped App's rule isn't re-sent");
    assert!(v.get("apps").is_none(), "{v}");
    assert_eq!(doctor_json(&app("ledger"))["running"], false);
    // Uninstalled: the rule record goes with it.
    super::super::guarded::remove_gateway_rule("ledger");
    assert!(applied_of("ledger").is_none());
    let _ = &f.ws_b;
}

/// The rule doesn't load: the answer and the App's agent both say to
/// re-host, and the doctor keeps naming the gap.
#[test]
fn a_rule_that_does_not_reload_says_rehost() {
    let f = fx("fail");
    super::super::guarded::install_gateway_rule("ledger", 38_601).unwrap();
    grant_b_to_a(&f);
    test_seams::fail_with(Some("root helper refused `apps nft app`: busy"));
    let mut v = json!({"ok": true});
    after_db_change(&mut v);
    test_seams::fail_with(None);
    let note = v["apps"][0]["note"].as_str().unwrap();
    assert_eq!(
        note,
        "App ledger: its door to this box's Postgres didn't reload (root helper refused `apps nft app`: busy). Takes effect after a re-host: k2 app rehost ledger"
    );
    assert_eq!(v["apps"][0]["applied"], false);
    assert!(msgs().iter().any(|m| m == &format!("[k2 apps] {note}")), "{:?}", msgs());
    assert_eq!(doctor_json(&app("ledger"))["ok"], false);
}

/// `k2 app doctor <app>`: a box service approved while the App ran, whose
/// live reload didn't happen, and a container's missing forward.
#[test]
fn doctor_reports_a_box_service_approved_but_not_applied() {
    let _f = fx("doctor");
    super::super::guarded::install_gateway_rule("ledger", 38_601).unwrap();
    super::super::guarded::install_gateway_rule("boxed", 38_602).unwrap();
    super::super::box_service_routes::record_live("boxed", &json!({"boxForwards": [], "gwPort": 38_602}));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let w = k2_core::deploy::box_service::Want { name: "sew-api".into(), host_port: 8470, app_port: 8470, proto: "tcp".into(), preset: None };
        for a in ["ledger", "boxed"] {
            k2_core::deploy::box_service::allow_approved(&conn, a, std::slice::from_ref(&w), "owner", 1).unwrap();
        }
    }
    let d = doctor_json(&app("ledger"));
    assert_eq!(
        findings(&d),
        vec!["box service approved but not yet applied to the running App: sew-api (host port 8470). Takes effect after a re-host: k2 app rehost ledger"],
        "{d}"
    );
    let d = doctor_json(&app("boxed"));
    let got = findings(&d);
    assert!(
        got.contains(&"box service approved but not yet applied to the running App: sew-api has no forward in its running container. Takes effect after a re-host: k2 app rehost boxed".to_string()),
        "{d}"
    );
    // The same answer `k2 app box-service <app>` carries (`reach`).
    let p: HashMap<String, String> = [("app".to_string(), "ledger".to_string())].into();
    let shown = super::super::box_service_routes::handle("/cli/app/box-service", &Who::Owner, b"", &p).unwrap_or_else(|e| panic!("{}", e.hint));
    assert_eq!(shown["reach"]["ok"], false, "{shown}");
    assert_eq!(shown["reach"]["rule"]["dbPorts"], json!([5432]), "{shown}");
    super::super::box_service_routes::forget_live("boxed");
}

/// S21 (0.46.12): a container App reaches box Postgres through its
/// database door. `dbRaw` is still refused up front (a private-database
/// connection K2 doesn't make), and says where the door is; a Postgres box
/// service points at the door; a grant needs no rule change (the door
/// checks per connection), and the container's rule names no cluster port.
#[test]
fn a_container_app_reaches_box_postgres_through_its_door() {
    let f = fx("door");
    super::super::db_door::test_seams::set_engine_ready(true);
    let door = "boxed reaches its workspace's databases (and ones granted to it) through its database door: 127.0.0.1:5432 inside \
                its wall, checked per connection (k2 app doctor boxed)";
    assert_eq!(door_line("boxed"), door);

    // --db-raw on: refused for the owner too, before anything is held,
    // and pointed at the door.
    let r = super::super::routes::handle("/cli/app/config", Who::Owner, json!({"app": "boxed", "dbRaw": "on"}).to_string().as_bytes(), &HashMap::new());
    assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
    let v: J = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["error"]["code"], codes::DB_DOOR_MISSING, "{v}");
    assert_eq!(
        v["error"]["hint"],
        format!(
            "--db-raw is the raw connection to an App's private database, which K2 doesn't make yet, so approving it would change \
             nothing. {door}; no widening needed"
        ),
        "{v}"
    );

    // A Postgres box service for it: never, and where the way in is.
    let agent = Who::Agent { project_id: f.ws_a.clone(), session_id: "sess-a".into() };
    let e = super::super::box_service_routes::handle(
        "/cli/app/box-service/request",
        &agent,
        json!({"app": "boxed", "name": "pg", "hostPort": 5433}).to_string().as_bytes(),
        &HashMap::new(),
    )
    .unwrap_err();
    assert_eq!((e.status, e.code), (403, super::super::box_service_routes::codes::BOX_SERVICE_REFUSED));
    assert_eq!(e.hint, format!("port 5433 can never be a box service: it is Postgres (5432-5449). {door}"));

    // Its rule names no cluster port (the door dials, not pasta), so a
    // grant while it runs moves nothing and says nothing: the door admits
    // the granted database at the next connection.
    super::super::guarded::install_gateway_rule("boxed", 38_602).unwrap();
    assert_eq!(last_rule(), "apps nft app boxed 38602 --db");
    grant_b_to_a(&f);
    let n = test_seams::sent().len();
    let mut v = json!({"ok": true});
    after_db_change(&mut v);
    assert_eq!(test_seams::sent().len(), n, "no rule to re-send for a container");
    assert!(v["apps"].as_array().is_none_or(|a| a.iter().all(|x| x["app"] != "boxed")), "{v}");
    assert!(!msgs().iter().any(|m| m.contains("App boxed")), "{:?}", msgs());

    // The doctor: running with a door, its database admitted: nothing.
    super::super::db_door::record_live("boxed", &json!({"dbPort": 38_902}));
    let d = doctor_json(&app("boxed"));
    assert_eq!(d["ok"], true, "{d}");
    assert_eq!(d["dbDoor"]["hostPort"], 38_902, "{d}");
    assert_eq!(d["dbDoor"]["inWallPort"], 5432, "{d}");
    assert_eq!(d["dbDoor"]["databases"], json!(["dr_main", "shared17"]), "{d}");
    // Running without one (started on an older engine): re-host.
    super::super::db_door::forget_live("boxed");
    let d = doctor_json(&app("boxed"));
    assert_eq!(
        findings(&d),
        vec!["boxed's running deployment has no database door (it started before this box's App engine had one). Takes effect after a re-host: k2 app rehost boxed"],
        "{d}"
    );
    assert_eq!(d["findings"][0]["id"], "database_door_missing");
    super::super::db_door::test_seams::set_engine_ready(false);
}

/// The cases the door still can't serve keep `app_db_door_missing`'s
/// up-front words: an App engine that predates the door.
#[test]
fn a_container_app_on_an_engine_without_the_door_is_told_why() {
    let f = fx("olddoor");
    super::super::db_door::test_seams::set_engine_ready(false);
    let words = "boxed runs in a container, and this box's App engine predates the database door, so nothing answers on \
                 127.0.0.1:5432 in its wall: the owner updates it (k2 app engine install), then k2 app rehost boxed";
    assert_eq!(engine_no_door("boxed"), words);
    let agent = Who::Agent { project_id: f.ws_a.clone(), session_id: "sess-a".into() };
    let e = super::super::box_service_routes::handle(
        "/cli/app/box-service/request",
        &agent,
        json!({"app": "boxed", "name": "pg", "hostPort": 5432}).to_string().as_bytes(),
        &HashMap::new(),
    )
    .unwrap_err();
    assert_eq!(e.hint, format!("port 5432 can never be a box service: it is Postgres (5432-5449). {words}"));
    let v = after_service_grant(&svc("boxed"), Some("DATABASE_URL")).unwrap();
    assert_eq!(v["rehost"], "no_door", "{v}");
    assert_eq!(v["note"], format!("App boxed: {words}"));
    assert!(msgs().contains(&format!("[k2 apps] App boxed: {words}")), "{:?}", msgs());
    // The doctor says it even while the App is stopped.
    let d = doctor_json(&app("boxed"));
    assert!(findings(&d).contains(&words.to_string()), "{d}");
}

/// A secret granted to an App reaches it at its next start: the answer
/// says to re-host, its agent hears it, and an idle App whose
/// DATABASE_URL it is gets re-hosted by K2.
#[test]
fn a_secret_granted_to_a_running_app_says_rehost() {
    let _f = fx("grant");
    super::super::guarded::install_gateway_rule("ledger", 38_601).unwrap();

    let v = after_service_grant(&svc("ledger"), Some("DATABASE_URL")).unwrap();
    assert_eq!(v["rehost"], "started", "{v}");
    assert_eq!(
        v["note"],
        "Takes effect after a re-host: k2 app rehost ledger. ledger was idle, so K2 is re-hosting it now (k2 app rehost status ledger)"
    );
    assert_eq!(test_seams::kicks(), vec!["ledger".to_string()]);

    let v = after_service_grant(&svc("ledger"), Some("STRIPE_KEY")).unwrap();
    assert_eq!(v["rehost"], "needed", "{v}");
    assert_eq!(v["note"], "Takes effect after a re-host: k2 app rehost ledger (ledger is running and keeps what it started with)");
    assert!(
        msgs().contains(&"[k2 apps] App ledger: STRIPE_KEY was granted to it; the running App keeps what it started with. Takes effect after a re-host: k2 app rehost ledger".to_string()),
        "{:?}",
        msgs()
    );
    assert_eq!(test_seams::kicks().len(), 1, "only a DATABASE_URL re-hosts by itself");

    // A container with the door (S21): like any App, the URL arrives at
    // its next start, and K2 re-hosts an idle one itself.
    super::super::db_door::test_seams::set_engine_ready(true);
    let v = after_service_grant(&svc("boxed"), Some("DATABASE_URL")).unwrap();
    super::super::db_door::test_seams::set_engine_ready(false);
    assert_eq!(v["rehost"], "started", "{v}");
    assert_eq!(test_seams::kicks(), vec!["ledger".to_string(), "boxed".to_string()]);
    // Not running: it gets it at its start. Not an App: nothing to say.
    let v = after_service_grant(&svc("stopped"), Some("DATABASE_URL")).unwrap();
    assert_eq!((v["rehost"].as_str(), v["note"].as_str()), (Some("not_running"), Some("App stopped gets DATABASE_URL when it next starts")));
    assert!(after_service_grant("svc-not-an-app", Some("DATABASE_URL")).is_none());
}
