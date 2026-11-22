//! `k2 db cluster switch` over the fake helper: the preview and its
//! confirm, the repoint in one transaction (exactly one active row per
//! name, including when it fails half way), the old copy closed, K2's
//! logins set again, archiving on the restored target, and the rollback.
//! The real two-cluster run is `tests/real_pg_switch.rs`.

use super::*;
use crate::sql::cluster::{for_db, record_in, Cluster};
use crate::sql::secrets::MemSecretStore;
use crate::sql::sysops::FakeSystemOps;

const MOMENT: i64 = 1_763_806_500; // 2025-11-22T10:15:00Z

struct Fx {
    pid: String,
    secrets: MemSecretStore,
}

fn named(pid: &str, name: &str, port: u16, wal_archive: bool) -> Cluster {
    let mut c = Cluster::layout(17, port, "created");
    c.name = name.into();
    c.unit = format!("postgresql@17-{name}.service");
    c.data_dir = format!("/var/lib/postgresql/17/{name}");
    c.state = "online".into();
    c.wal_archive = wal_archive;
    c.owner_project = Some(pid.into());
    let db = k2_core::db::shared();
    record_in(&db.lock(), &c, true).unwrap();
    c
}

fn fx() -> Fx {
    k2_core::db::init_for_tests();
    let pid = uuid::Uuid::new_v4().to_string();
    let secrets = MemSecretStore::default();
    let (mref, aref, gref) = (
        secrets.store("migrator", "mig-pass-now").unwrap(),
        secrets.store("agent", "agent-pass-now").unwrap(),
        secrets.store("grantee", "grantee-pass-now").unwrap(),
    );
    let db = k2_core::db::shared();
    {
        let conn = db.lock();
        for t in ["sql_server", "sql_clusters", "sql_databases", "sql_grants", "pg_cluster_pitr", "pg_db_switches"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', 17, 'localhost', 1)", []).unwrap();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'sw', '/tmp/k2-switch-ws')", [&pid]).unwrap();
        conn.execute(
            "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major, cluster_name, agent_secret_ref, migrator_secret_ref) \
             VALUES ('d-app', ?1, 'app_db', 'active', 1, 17, 'feedback', ?2, ?3)",
            rusqlite::params![pid, aref, mref],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sql_grants (database_id, project_id, level, can_manage, created_at, updated_at, agent_secret_ref) VALUES ('d-app', 'p-other', 'read', 0, 1, 1, ?1)",
            [&gref],
        )
        .unwrap();
    }
    named(&pid, "feedback", 5434, true);
    named(&pid, "fb_old", 5435, false);
    db.lock()
        .execute("INSERT INTO pg_cluster_pitr (major, name, restored_from, restored_to) VALUES (17, 'fb_old', 'feedback', ?1)", [MOMENT])
        .unwrap();
    Fx { pid, secrets }
}

fn ops() -> FakeSystemOps {
    let mut o = FakeSystemOps::baked();
    let qa = &mut o.query_answers;
    for k in ["helper pgnamed", "helper pgrestore", "helper pgarchive"] {
        qa.insert(k.into(), String::new());
    }
    for c in ["feedback", "fb_old"] {
        qa.insert(format!("psql 17-{c} postgres|pg_get_userbyid(datdba)"), "app_db_migrator\n".into());
        qa.insert(format!("psql 17-{c} postgres|ALLOW_CONNECTIONS false"), "0\n".into());
        qa.insert(format!("psql 17-{c} postgres|ALLOW_CONNECTIONS true"), "0\n".into());
        qa.insert(format!("psql 17-{c} postgres|ALTER ROLE"), String::new());
    }
    qa.insert(
        "helper wal-archive 17-fb_old on".into(),
        json!({"major": 17, "name": "fb_old", "port": 5435, "k2Named": true, "walArchive": true, "changed": true, "restarted": true}).to_string(),
    );
    qa.insert("helper basebackup 17-fb_old".into(), json!({"stamp": "20251122T110000Z", "complete": true}).to_string());
    o
}

fn sent(o: &FakeSystemOps, needle: &str) -> Vec<String> {
    o.recorded().into_iter().filter(|l| l.contains(needle)).collect()
}

fn helper_sql(o: &FakeSystemOps) -> Vec<String> {
    o.pg.lock().unwrap_or_else(|p| p.into_inner()).helper_sql.clone()
}

#[test]
fn the_switch_shows_the_moment_and_changes_nothing_until_confirmed() {
    let _g = crate::sql::sql_server_test_lock();
    let f = fx();
    let o = ops();
    let v = switch(&o, &f.secrets, "app_db", "fb_old", false, "owner").unwrap();
    assert_eq!((v["ok"].clone(), v["needsConfirm"].clone(), v["kind"].clone()), (json!(false), json!(true), json!("restore")), "{v}");
    assert_eq!(v["moment"], "2025-11-22T10:15:00Z");
    assert!(v["lost"].as_str().unwrap().contains("every write to it on 17/feedback after that moment is left behind"), "{v}");
    assert_eq!(for_db("app_db").unwrap().name, "feedback", "nothing moved");
    assert!(sent(&o, "ALLOW_CONNECTIONS").is_empty() && sent(&o, "ALTER ROLE").is_empty(), "{:?}", o.recorded());
    assert_eq!(active_rows_named("app_db"), 1);
}

#[test]
fn a_switch_closes_the_old_copy_sets_the_logins_and_moves_the_one_row() {
    let _g = crate::sql::sql_server_test_lock();
    let f = fx();
    let o = ops();
    let v = switch(&o, &f.secrets, "app_db", "fb_old", true, "owner").unwrap();
    assert_eq!((v["ok"].clone(), v["switched"].clone()), (json!(true), json!(true)), "{v}");
    assert_eq!(for_db("app_db").unwrap().name, "fb_old");
    assert_eq!(active_rows_named("app_db"), 1, "exactly one active row");
    // The old copy closed first, the target opened, K2's logins set there.
    let log = o.recorded();
    let pos = |needle: &str| log.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}: {log:?}"));
    assert!(pos("psql 17-feedback postgres ALTER DATABASE \"app_db\" ALLOW_CONNECTIONS false") < pos("psql 17-fb_old postgres ALTER DATABASE \"app_db\" ALLOW_CONNECTIONS true"));
    let sql = helper_sql(&o).join("\n");
    for role in ["\"app_db_migrator\" WITH LOGIN PASSWORD 'mig-pass-now'", "\"app_db_agent\" WITH LOGIN PASSWORD 'agent-pass-now'", "PASSWORD 'grantee-pass-now'"] {
        assert!(sql.contains(role), "{role}: K2's logins set again on the target");
    }
    assert!(sent(&o, "psql 17-feedback postgres ALTER ROLE").is_empty(), "never on the old cluster");
    assert_eq!(v["loginsSet"].as_array().unwrap().len(), 3, "{v}");
    assert!(!v.to_string().contains("pass-now"), "no password in the answer");
    // Archiving and a base backup on the restored target.
    assert_eq!(v["archive"], json!({"walArchive": true, "baseBackup": "20251122T110000Z"}));
    let rec: (String, String, String, i64) = k2_core::db::shared()
        .lock()
        .query_row("SELECT from_cluster, to_cluster, kind, moment FROM pg_db_switches WHERE db_name = 'app_db'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap();
    assert_eq!(rec, ("feedback".into(), "fb_old".into(), "restore".into(), MOMENT));

    // The rollback: back to the old one, while it is kept.
    let p = switch(&o, &f.secrets, "app_db", "feedback", false, "owner").unwrap();
    assert_eq!(p["kind"], "rollback", "{p}");
    assert!(p["lost"].as_str().unwrap().contains("every write to it on 17/fb_old since then is left behind"), "{p}");
    let v = switch(&o, &f.secrets, "app_db", "feedback", true, "owner").unwrap();
    assert_eq!(v["switched"], true, "{v}");
    assert_eq!(for_db("app_db").unwrap().name, "feedback");
    assert_eq!(active_rows_named("app_db"), 1);
    assert!(sent(&o, "psql 17-fb_old postgres ALTER DATABASE \"app_db\" ALLOW_CONNECTIONS false").len() == 1, "the restored copy closed");
    assert_eq!(v["archive"], Value::Null, "the old cluster archives already");
    // And forward again (the restore is still a restore of its source).
    assert_eq!(switch(&o, &f.secrets, "app_db", "fb_old", true, "owner").unwrap()["switched"], true);
    let _ = f.pid;
}

#[test]
fn a_repoint_that_fails_half_way_leaves_one_row_where_it_was() {
    let _g = crate::sql::sql_server_test_lock();
    let f = fx();
    let o = ops();
    test_seam::FAIL_REPOINT.with(|x| x.set(true));
    let e = switch(&o, &f.secrets, "app_db", "fb_old", true, "owner").unwrap_err();
    test_seam::FAIL_REPOINT.with(|x| x.set(false));
    assert!(e.hint().contains("injected failure"), "{e:?}");
    assert_eq!(for_db("app_db").unwrap().name, "feedback", "the UPDATE rolled back");
    assert_eq!(active_rows_named("app_db"), 1, "exactly one active row");
    let n: i64 = k2_core::db::shared().lock().query_row("SELECT count(*) FROM pg_db_switches", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0, "no switch recorded");
    // The old copy is open again, the target closed again.
    let log = o.recorded();
    let last_old = log.iter().rposition(|l| l.contains("psql 17-feedback postgres ALTER DATABASE")).unwrap();
    assert!(log[last_old].contains("ALLOW_CONNECTIONS true"), "{log:?}");
    let last_new = log.iter().rposition(|l| l.contains("psql 17-fb_old postgres ALTER DATABASE")).unwrap();
    assert!(log[last_new].contains("ALLOW_CONNECTIONS false"), "{log:?}");
}

#[test]
fn a_switch_goes_only_to_its_restore_or_back() {
    let _g = crate::sql::sql_server_test_lock();
    let f = fx();
    let mut o = ops();
    named(&f.pid, "unrelated", 5436, false);
    let e = switch(&o, &f.secrets, "app_db", "unrelated", true, "owner").unwrap_err();
    assert!(matches!(e, EnsureError::Refused(_)) && e.hint().contains("neither a restore"), "{e:?}");
    assert_eq!(switch(&o, &f.secrets, "app_db", "feedback", true, "owner").unwrap_err().code(), "usage", "already there");
    assert_eq!(switch(&o, &f.secrets, "nope_db", "fb_old", true, "owner").unwrap_err().code(), "not_found");
    assert_eq!(switch(&o, &f.secrets, "App-DB", "fb_old", true, "owner").unwrap_err().code(), "usage");
    // The database isn't on the target (or another owner has it).
    o.query_answers.insert("psql 17-fb_old postgres|pg_get_userbyid(datdba)".into(), "someone_else\n".into());
    let e = switch(&o, &f.secrets, "app_db", "fb_old", true, "owner").unwrap_err();
    assert!(e.hint().contains("no database app_db owned by app_db_migrator"), "{e:?}");
    assert!(sent(&o, "ALLOW_CONNECTIONS").is_empty(), "nothing closed");
    assert_eq!(for_db("app_db").unwrap().name, "feedback");
}

/// The route: POST only for the switch (a GET with its body lists and
/// changes nothing); an agent needs the switch and its own database.
#[test]
fn the_switch_route_is_gated() {
    use crate::sql::versions_access::Who;
    let _g = crate::sql::sql_server_test_lock();
    let f = fx();
    let o = ops();
    k2_core::db::shared().lock().execute("INSERT OR IGNORE INTO projects (id, name, path) VALUES ('p-other', 'other', '/tmp/k2-switch-other')", []).unwrap();
    let body = json!({"op": "switch", "project": "/tmp/k2-switch-ws", "db": "app_db", "to": "fb_old", "yes": true}).to_string();
    let p = std::collections::HashMap::new();
    let r = crate::sql::named_routes::handle(&o, Who::Owner, false, body.as_bytes(), &p);
    assert!(r.body.contains("\"clusters\""), "a GET lists: {}", r.body);
    assert_eq!(for_db("app_db").unwrap().name, "feedback", "a GET never switches");
    let r = crate::sql::named_routes::handle(&o, Who::Member { username: "m".into() }, true, body.as_bytes(), &p);
    assert_eq!(r.status, "403 Forbidden");
    let stranger = Who::Agent { project_id: "p-other".into(), session_id: "s".into(), local: true, sandboxed: false };
    let r = crate::sql::named_routes::handle(&o, stranger, true, body.as_bytes(), &p);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(for_db("app_db").unwrap().name, "feedback");
    let _ = f;
}
