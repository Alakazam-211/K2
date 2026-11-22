//! Point-in-time restore, the daemon's side: times, the restore record,
//! the drill's checks, the daily base backup and the route gate, over the
//! fake helper (protocol 19 answers set per test). The real-Postgres drill
//! is the root helper's `pitr_real_pg.rs`.

use super::*;
use crate::sql::cluster::{for_name, list, record_in};
use crate::sql::sysops::FakeSystemOps;

const NOW: i64 = 1_763_806_500; // 2025-11-22T10:15:00Z

fn reset() {
    k2_core::db::init_for_tests();
    let db = k2_core::db::shared();
    let conn = db.lock();
    for t in ["sql_server", "sql_clusters", "sql_databases", "pg_cluster_pitr"] {
        conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
    }
    conn.execute(
        "INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', 16, 'localhost', 1)",
        [],
    )
    .unwrap();
}

fn insert_project(name: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = std::env::temp_dir().join(format!("k2-pitr-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let db = k2_core::db::shared();
    db.lock().execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, name, path]).unwrap();
    (id, path)
}

/// `17/feedback` recorded as K2's, archiving, running, in `pid`.
fn feedback(pid: &str) -> Cluster {
    let mut c = Cluster::layout(17, 5434, "created");
    c.name = "feedback".into();
    c.unit = "postgresql@17-feedback.service".into();
    c.data_dir = "/var/lib/postgresql/17/feedback".into();
    c.state = "online".into();
    c.wal_archive = true;
    c.owner_project = Some(pid.into());
    let db = k2_core::db::shared();
    record_in(&db.lock(), &c, true).unwrap();
    c
}

fn named_answer(name: &str, port: u16, extra: Value) -> String {
    let mut v = json!({
        "major": 17, "name": name, "port": port, "dataDir": format!("/var/lib/postgresql/17/{name}"),
        "running": true, "startConf": "auto", "k2Named": true, "walArchive": false, "systemId": "7354340000000000000",
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v.to_string()
}

fn ops_19() -> FakeSystemOps {
    let mut ops = FakeSystemOps::baked();
    ops.query_answers.insert("helper pgnamed".into(), String::new());
    ops.query_answers.insert("helper pgrestore".into(), String::new());
    ops
}

fn sent(ops: &FakeSystemOps, needle: &str) -> Vec<String> {
    ops.recorded().into_iter().filter(|l| l.contains(needle)).collect()
}

#[test]
fn times_people_write() {
    let t = |s: &str| parse_time(s, NOW).map(UtcTime::token);
    // Review L7: non-ASCII never panics.
    for bad in ["-5é", "é", "2025-11-22T10:15:00Ζ", "-１m"] {
        assert!(t(bad).is_err(), "{bad:?}");
    }
    // Review L8: the drill's name carries the source name's hash.
    assert_eq!(drill_name("feedback"), "feedback_da602");
    assert_ne!(drill_name("feedback"), drill_name("feedbacl"));
    assert!(ClusterName::parse(&drill_name("a_very_long_cluster_name")).is_some());
    assert_eq!(t("2025-11-22T10:15:00Z").unwrap(), "2025-11-22T10:15:00Z");
    assert_eq!(t("2025-11-22T10:15Z").unwrap(), "2025-11-22T10:15:00Z");
    assert_eq!(t("2025-11-22 10:15").unwrap(), "2025-11-22T10:15:00Z");
    assert_eq!(t("2025-11-22 10:15:30").unwrap(), "2025-11-22T10:15:30Z");
    assert_eq!(t("2025-11-22T12:15:00+02:00").unwrap(), "2025-11-22T10:15:00Z");
    assert_eq!(t("2025-11-22T05:15:00-05:00").unwrap(), "2025-11-22T10:15:00Z");
    assert_eq!(t("-15m").unwrap(), "2025-11-22T10:00:00Z");
    assert_eq!(t("-2h").unwrap(), "2025-11-22T08:15:00Z");
    assert_eq!(t("-1d").unwrap(), "2025-11-21T10:15:00Z");
    for bad in ["", "now", "yesterday", "-0m", "-5x", "-m", "2025-11-22", "2025-11-22T25:00Z", "2025-11-22T10:15:00+15:00", "11/22/2025 10:15"] {
        assert!(t(bad).is_err(), "{bad:?}");
    }
}

/// A restore makes a new cluster K2 records as its own, in the source's
/// workspace; the old one is unchanged. The name must be new.
#[test]
fn restore_records_the_new_cluster_beside_the_old_one() {
    let _g = crate::sql::sql_server_test_lock();
    reset();
    let (pid, _) = insert_project("pitr-restore");
    let c = feedback(&pid);
    let mut ops = ops_19();
    let t = UtcTime::parse("2025-11-22T10:15:00Z").unwrap();
    ops.query_answers.insert(
        "helper restore-to 17-feedback fb_old 20251122T101500Z".into(),
        named_answer("fb_old", 5435, json!({"restoredFrom": "17/feedback", "baseBackup": "20251122T000000Z", "created": true})),
    );
    let v = restore(&ops, &c, "fb_old", t).unwrap();
    assert_eq!((v["cluster"].clone(), v["port"].clone(), v["restoredFrom"].clone()), (json!("17/fb_old"), json!(5435), json!("17/feedback")), "{v}");
    let r = for_name("fb_old").expect("recorded");
    assert_eq!((r.role.as_str(), r.origin.as_str(), r.owner_project.as_deref(), r.wal_archive), ("k2", "created", Some(pid.as_str()), false));
    assert!(for_name("feedback").is_some());
    // Never the source's own name, never a name K2 has, never a bad one:
    // refused before anything is sent.
    let before = sent(&ops, "helper restore-to").len();
    for (name, code) in [("feedback", "usage"), ("fb_old", "cluster_refused"), ("Fb-Old", "usage"), ("main", "usage")] {
        let e = restore(&ops, &c, name, t).unwrap_err();
        assert_eq!(e.code(), code, "{name}: {e:?}");
    }
    assert_eq!(sent(&ops, "helper restore-to").len(), before);
    // A helper refusal is the person's (409), with the helper's words.
    let mut ops2 = ops_19();
    assert!(matches!(helper_err("wal_gap: WAL segment 000000010000000000000004 is missing".into()), EnsureError::Refused(_)));
    assert!(matches!(helper_err("target_after_wal: …".into()), EnsureError::Refused(_)));
    assert!(matches!(helper_err("recovery_failed: …".into()), EnsureError::Helper(_)));
    // An 18 helper: nothing is sent.
    ops2.query_answers.remove("helper pgrestore");
    let e = restore(&ops2, &c, "fb_new", t).unwrap_err();
    assert_eq!(e.code(), "helper_needed", "{e:?}");
    assert!(sent(&ops2, "restore-to").is_empty());
}

/// The scripted source and drill clusters of a drill to now.
fn drill_ops(markers_on_drill: &str, drill_rows: &str) -> FakeSystemOps {
    let mut ops = ops_19();
    let qa = &mut ops.query_answers;
    qa.insert("helper drop-drill 17-feedback_da602".into(), String::new());
    qa.insert(
        "helper backups 17-feedback".into(),
        json!({"baseBackups": [{"stamp": "20251122T000000Z", "complete": true}]}).to_string(),
    );
    // The source (`psql --cluster 17-feedback`).
    qa.insert("psql 17-feedback postgres|FROM pg_catalog.pg_database".into(), "app_db\n".into());
    qa.insert("psql 17-feedback app_db|query_to_xml".into(), "public.orders|3\npublic.users|2\n".into());
    qa.insert("psql 17-feedback postgres|CREATE SCHEMA IF NOT EXISTS k2_restore_drill AUTHORIZATION postgres".into(), String::new());
    qa.insert("psql 17-feedback postgres|pg_trigger".into(), "r|postgres|postgres|0|0\n".into());
    qa.insert("psql 17-feedback postgres|INSERT INTO k2_restore_drill.markers (id) VALUES ('k2drill".into(), "1763806501\n".into());
    qa.insert("psql 17-feedback postgres|> 1763806501 + 1".into(), "true\n".into());
    qa.insert("psql 17-feedback postgres|DROP SCHEMA IF EXISTS k2_restore_drill".into(), String::new());
    qa.insert(
        "helper restore-to 17-feedback feedback_da602 20251122T101501Z --drill".into(),
        named_answer("feedback_da602", 5436, json!({"baseBackup": "20251122T000000Z", "drill": true})),
    );
    // The drill (`psql --cluster 17-feedback_da602`).
    qa.insert("psql 17-feedback_da602 postgres|FROM pg_catalog.pg_database".into(), "app_db\n".into());
    qa.insert("psql 17-feedback_da602 app_db|query_to_xml".into(), drill_rows.into());
    qa.insert("psql 17-feedback_da602 postgres|split_part(id".into(), markers_on_drill.into());
    ops
}

/// A drill to now: the counts and the first marker before the moment, the
/// second after it, the restore to that moment, both checks on the drill,
/// then the drill and the markers removed and the result recorded.
#[test]
fn a_drill_proves_the_moment_and_cleans_up() {
    let _g = crate::sql::sql_server_test_lock();
    reset();
    let (pid, _) = insert_project("pitr-drill");
    let c = feedback(&pid);
    // The insert of the "after" row answers nothing.
    let mut ops = drill_ops("before\n", "public.orders|3\npublic.users|2\n");
    ops.query_answers.insert("helper drop-drill 17-feedback_da602".into(), json!({"dropped": true}).to_string());
    let v = drill(&ops, &c, None).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["target"], "2025-11-22T10:15:01Z");
    assert_eq!(v["markers"], json!({"before": true, "after": false}));
    assert_eq!(v["databases"][0]["rowsMatch"], true, "{v}");
    assert_eq!(v["dropped"], true);
    let log = ops.recorded();
    let pos = |needle: &str| log.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}: {log:?}"));
    assert!(pos("psql 17-feedback app_db") < pos("VALUES ('k2drill"), "counts before the first marker");
    assert!(pos("VALUES ('k2drill") < pos("helper restore-to"), "markers before the restore");
    assert!(pos("helper restore-to") < pos("psql 17-feedback_da602 app_db"), "checked on the drill");
    assert!(log.iter().rposition(|l| l.contains("helper drop-drill")).unwrap() > pos("psql 17-feedback_da602 postgres SELECT split_part"));
    assert!(pos("psql 17-feedback postgres DROP SCHEMA IF EXISTS k2_restore_drill") > pos("helper restore-to"), "the markers' schema goes after");
    assert!(for_name("feedback_da602").is_none(), "a drill is never recorded as a cluster");
    let r = recorded(&c);
    assert_eq!((r["lastDrillOk"].clone(), r["drillWarning"].clone()), (json!(true), Value::Null), "{r}");
}

/// The "after" row came back, or a table's count differs: the drill fails
/// loudly and the failure is recorded (with the 30-day warning standing).
#[test]
fn a_drill_that_doesnt_match_the_moment_fails() {
    let _g = crate::sql::sql_server_test_lock();
    reset();
    let (pid, _) = insert_project("pitr-drill-bad");
    let c = feedback(&pid);
    let mut ops = drill_ops("after\nbefore\n", "public.orders|4\npublic.users|2\n");
    ops.query_answers.insert("helper drop-drill 17-feedback_da602".into(), json!({"dropped": true}).to_string());
    let v = drill(&ops, &c, None).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["markers"], json!({"before": true, "after": true}));
    assert_eq!(v["databases"][0]["diffs"], json!([{"table": "public.orders", "atTheMoment": 3, "restored": 4}]));
    let r = recorded(&c);
    assert_eq!(r["lastDrillOk"], false);
    assert!(r["drillWarning"].as_str().unwrap().contains("no good restore drill yet"), "{r}");
    // Review M4: another drill within ten minutes is refused, nothing sent.
    let again = drill_ops("before\n", "");
    let e = drill(&again, &c, None).unwrap_err();
    assert!(e.hint().starts_with("busy"), "{e:?}");
    assert!(sent(&again, "helper").iter().all(|l| !l.contains("restore-to") && !l.contains("drop-drill")), "{:?}", again.recorded());
    {
        let db = k2_core::db::shared();
        db.lock().execute("UPDATE pg_cluster_pitr SET last_drill_at = last_drill_at - 3600", []).unwrap();
    }
    // A helper refusal mid-drill: recorded, the drill still dropped.
    let mut ops = drill_ops("before\n", "");
    ops.query_answers.remove("helper restore-to 17-feedback feedback_da602 20251122T101501Z --drill");
    ops.query_answers.insert("helper drop-drill 17-feedback_da602".into(), json!({"dropped": true}).to_string());
    assert!(drill(&ops, &c, None).is_err());
    assert_eq!(sent(&ops, "helper drop-drill").len(), 2, "before and after: {:?}", ops.recorded());
    assert_eq!(recorded(&c)["lastDrillOk"], false);
}

/// The daily base backup: only K2's archiving, running named clusters,
/// once a day; a failure is recorded and the loop goes on.
#[test]
fn the_daily_base_backup_takes_only_what_is_due() {
    let _g = crate::sql::sql_server_test_lock();
    reset();
    let (pid, _) = insert_project("pitr-tick");
    let c = feedback(&pid);
    let mut quiet = c.clone();
    quiet.name = "quiet".into();
    quiet.port = 5437;
    quiet.wal_archive = false;
    {
        let db = k2_core::db::shared();
        record_in(&db.lock(), &quiet, true).unwrap();
    }
    let mut ops = ops_19();
    ops.query_answers.insert("helper basebackup 17-feedback".into(), json!({"stamp": "20251122T000000Z", "complete": true}).to_string());
    assert_eq!(tick(&ops), vec!["17/feedback".to_string()]);
    assert!(sent(&ops, "helper basebackup 17-quiet").is_empty());
    assert_eq!(recorded(&c)["lastBasebackupOk"], true);
    // Done today: not again.
    assert!(tick(&ops).is_empty());
    assert_eq!(sent(&ops, "helper basebackup").len(), 1);
    // An 18 helper: the loop sends nothing.
    {
        let db = k2_core::db::shared();
        db.lock().execute("DELETE FROM pg_cluster_pitr", []).unwrap();
    }
    ops.query_answers.remove("helper pgrestore");
    assert!(tick(&ops).is_empty());
    assert_eq!(sent(&ops, "helper basebackup").len(), 1);
    assert!(list().iter().any(|k| k.name == "quiet"));
}

/// `POST /cli/db/clusters {op: …}`: the owner; an agent only with the
/// switch, for its own cluster; usage refused before anything is sent.
#[test]
fn pitr_ops_route_gate() {
    use crate::sql::versions_access::Who;
    let _g = crate::sql::sql_server_test_lock();
    reset();
    let (pid, path) = insert_project("pitr-route");
    let (other, _) = insert_project("pitr-route-other");
    feedback(&pid);
    let mut ops = ops_19();
    ops.query_answers.insert("helper backups 17-feedback".into(), json!({"baseBackups": []}).to_string());
    let project = serde_json::to_string(&path).unwrap();
    let body = |extra: &str| format!(r#"{{"project":{project}{extra}}}"#).into_bytes();
    let p = std::collections::HashMap::new();
    let h = |who: Who, b: &[u8]| crate::sql::named_routes::handle(&ops, who, true, b, &p);

    let r = h(Who::Owner, &body(r#","op":"backups","name":"feedback""#));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(r.body.contains("drillWarning"), "{}", r.body);
    for (b, want) in [
        (body(r#","op":"restore","name":"feedback","as":"fb_old""#), "--to"),
        (body(r#","op":"restore","name":"feedback","to":"-15m""#), "--as"),
        (body(r#","op":"restore","name":"feedback","to":"yesterday","as":"fb_old""#), "not a time"),
        (body(r#","op":"backups""#), "basebackup|backups"),
    ] {
        let r = h(Who::Owner, &b);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains(want), "{want}: {}", r.body);
    }
    // An agent: the switch off; a Member; another workspace's agent.
    let agent = Who::Agent { project_id: pid.clone(), session_id: "s".into(), local: true, sandboxed: false };
    let r = h(agent.clone(), &body(r#","op":"basebackup","name":"feedback""#));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert_eq!(h(Who::Member { username: "m".into() }, &body(r#","op":"backups","name":"feedback""#)).status, "403 Forbidden");
    {
        let db = k2_core::db::shared();
        db.lock().execute("UPDATE projects SET agents_can_upgrade_postgres = 1 WHERE id IN (?1, ?2)", [&pid, &other]).unwrap();
    }
    let r = h(agent.clone(), &body(r#","op":"backups","name":"feedback""#));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    // A drill K2 has no record of: the owner's alone.
    let r = h(agent, &body(r#","op":"drop-drill","name":"feedback_da602""#));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(sent(&ops, "helper basebackup").is_empty() && sent(&ops, "helper drop-drill").is_empty(), "{:?}", ops.recorded());
}

/// Review L1: a marker table someone else made (a trigger, another owner)
/// refuses the drill before any marker is written.
#[test]
fn an_unsafe_marker_table_refuses_the_drill() {
    let _g = crate::sql::sql_server_test_lock();
    reset();
    let (pid, _) = insert_project("pitr-drill-l1");
    let c = feedback(&pid);
    let mut ops = drill_ops("before\n", "public.orders|3\n");
    ops.query_answers.insert("psql 17-feedback postgres|pg_trigger".into(), "r|postgres|postgres|1|0\n".into());
    ops.query_answers.insert("helper drop-drill 17-feedback_da602".into(), json!({"dropped": false}).to_string());
    let e = drill(&ops, &c, None).unwrap_err();
    assert!(e.hint().contains("marker table"), "{e:?}");
    assert!(sent(&ops, "INSERT INTO k2_restore_drill.markers").is_empty(), "{:?}", ops.recorded());
    assert!(sent(&ops, "helper restore-to").is_empty());
}

/// Review L2: a failed daily base backup is tried again after an hour.
#[test]
fn a_failed_daily_base_backup_is_retried_after_an_hour() {
    let _g = crate::sql::sql_server_test_lock();
    reset();
    let (pid, _) = insert_project("pitr-tick-retry");
    let c = feedback(&pid);
    let mut ops = ops_19();
    ops.query_answers.insert("helper basebackup 17-feedback".into(), "not json".into());
    assert!(tick(&ops).is_empty(), "the backup fails");
    assert_eq!(recorded(&c)["lastBasebackupOk"], false);
    assert_eq!(sent(&ops, "helper basebackup").len(), 1);
    assert!(tick(&ops).is_empty());
    assert_eq!(sent(&ops, "helper basebackup").len(), 1, "not again within the hour");
    {
        let db = k2_core::db::shared();
        db.lock().execute("UPDATE pg_cluster_pitr SET last_basebackup_at = last_basebackup_at - 3700", []).unwrap();
    }
    tick(&ops);
    assert_eq!(sent(&ops, "helper basebackup").len(), 2, "tried again after an hour");
}
