//! The resolver: every caller finds its database's cluster in K2's records
//! (a box with 16 on 5432 and 17 on 5433), and the create path picks one.

use super::*;
use crate::sql::ops::{self, OpsError};
use crate::sql::secrets::MemSecretStore;
use crate::sql::sysops::{helper_psql_argv_ok, FakeSystemOps};

fn reset(installed_major: Option<i64>) {
    k2_core::db::init_for_tests();
    let db = k2_core::db::shared();
    let conn = db.lock();
    for t in ["sql_server", "sql_clusters", "sql_databases", "sql_grants", "sql_moves"] {
        conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
    }
    conn.execute(
        "INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', ?1, 'localhost', 1)",
        rusqlite::params![installed_major],
    )
    .unwrap();
}

fn record(major: u32, port: u16, role: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut c = Cluster::layout(major, port, if port == 5432 { "baked" } else { "created" });
    c.role = role.into();
    c.state = "online".into();
    record_in(&conn, &c, true).unwrap();
}

fn db_row(id: &str, name: &str, major: Option<i64>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major) VALUES (?1, 'p-x', ?2, 'active', 1, ?3)",
        rusqlite::params![id, name, major],
    )
    .unwrap();
}

fn insert_project(name: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = std::env::temp_dir().join(format!("k2-pgmaj-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, name, path],
    )
    .unwrap();
    (id, path)
}

fn lines_with(ops: &FakeSystemOps, needle: &str) -> Vec<String> {
    ops.recorded().into_iter().filter(|l| l.contains(needle)).collect()
}

fn major_of(name: &str) -> Option<i64> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT cluster_major FROM sql_databases WHERE name = ?1 AND status != 'dropped'",
        [name],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn lookup_by_major_and_database() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    record(17, 5433, "k2");
    db_row("d-a", "ws_a", Some(16));
    db_row("d-b", "ws_b", Some(17));
    db_row("d-c", "ws_c", None);
    db_row("d-z", "ws_z", Some(18));

    let a = for_db("ws_a").unwrap();
    assert_eq!((a.major, a.port, a.unit.as_str()), (16, 5432, "postgresql@16-main.service"));
    let b = for_db("ws_b").unwrap();
    assert_eq!((b.major, b.port, b.unit.as_str()), (17, 5433, "postgresql@17-main.service"));
    assert_eq!(b.data_dir, "/var/lib/postgresql/17/main");
    assert_eq!(for_db("ws_c").unwrap().port, 5432, "NULL = the 5432 cluster");
    assert_eq!(for_db("postgres").unwrap().major, 16, "the maintenance db is the 5432 cluster");
    assert_eq!(for_db("ws_unknown").unwrap().port, 5432);
    let err = for_db("ws_z").unwrap_err();
    assert!(err.contains("Postgres 18") && err.contains("no record"), "never a silent 5432: {err}");
    assert_eq!(for_major(17).unwrap().port, 5433);
    assert!(for_major(18).is_none());
    assert_eq!(primary().major, 16);
    // A dropped row never decides where a name lives.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE sql_databases SET status = 'dropped' WHERE id = 'd-b'", []).unwrap();
    }
    assert_eq!(for_db("ws_b").unwrap().port, 5432);
}

#[test]
fn legacy_box_resolves_the_5432_cluster_from_installed_major() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    let p = primary();
    assert!(p.synthesized && p.is_primary());
    assert_eq!((p.major, p.port, p.unit.as_str()), (16, 5432, "postgresql@16-main.service"));
    assert_eq!(for_major(16).unwrap().port, 5432);
    assert!(for_major(17).is_none());
    db_row("d-a", "ws_a", Some(16));
    assert_eq!(for_db("ws_a").unwrap().port, 5432);
    // Never read: the wrapper unit, still 5432.
    reset(None);
    let p = primary();
    assert_eq!((p.major, p.port, p.unit.as_str()), (0, 5432, "postgresql"));
}

#[test]
fn helper_argv_adds_the_cluster_only_off_5432() {
    let p = Cluster::layout(16, 5432, "baked");
    assert_eq!(p.helper_psql_argv("ws_a", &["-v", "ON_ERROR_STOP=1"]), ["psql", "-d", "ws_a", "-v", "ON_ERROR_STOP=1"]);
    let c = Cluster::layout(17, 5433, "created");
    let argv = c.helper_psql_argv("ws_b", &["-tA"]);
    assert_eq!(argv, ["psql", "--cluster", "17", "-d", "ws_b", "-tA"]);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    assert!(helper_psql_argv_ok(&refs[1..]));
    for bad in [
        &["--cluster", "15x", "-d", "a"][..],
        &["--cluster", "19", "-d", "a"][..],
        &["--cluster", "-d", "a"][..],
        &["-d", "a", "--cluster", "17"][..],
    ] {
        assert!(!helper_psql_argv_ok(bad), "{bad:?}");
    }
    assert!(crate::sql::sysops::needs_pgmajor(&refs));
    assert!(!crate::sql::sysops::needs_pgmajor(&["psql", "-d", "ws_a"]));
    assert!(crate::sql::sysops::needs_pgmajor(&["ensure-cluster", "17"]));
    // A cluster off 5432 with no major is never sent.
    let mut nomajor = c.clone();
    nomajor.major = 0;
    assert!(nomajor.helper_psql(&FakeSystemOps::baked(), "x", &[], None).is_err());
}

#[test]
fn client_tools_use_that_majors_binaries_when_installed() {
    let c = Cluster::layout(17, 5433, "created");
    let bare = FakeSystemOps::baked();
    assert_eq!(c.client_bin(&bare, "pg_dump"), "/usr/bin/pg_dump");
    let mut with17 = FakeSystemOps::baked();
    with17.existing_paths.push("/usr/lib/postgresql/17/bin/pg_dump".into());
    assert_eq!(c.client_bin(&with17, "pg_dump"), "/usr/lib/postgresql/17/bin/pg_dump");
    assert_eq!(c.client_bin(&with17, "psql"), "/usr/bin/psql", "per tool");
    assert_eq!(Cluster::legacy_primary(0).client_bin(&with17, "pg_dump"), "/usr/bin/pg_dump");
}

#[test]
fn helper_view_parses_and_folds_roles() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    let raw = r#"{"clusters":[
        {"major":16,"name":"main","port":5432,"dataDir":"/var/lib/postgresql/16/main","running":true,"startConf":"auto"},
        {"major":17,"name":"main","port":5433,"dataDir":"/var/lib/postgresql/17/main","running":false,"startConf":"manual"},
        {"major":16,"name":"extra","port":5440,"dataDir":"/srv/pg","running":true,"startConf":"auto"}
    ]}"#;
    let seen = parse_helper_clusters(raw).unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[1].state, "down");
    assert_eq!(seen[1].start_conf, "manual");
    assert_eq!(seen[2].role, "foreign", "off K2's layout");
    let after = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        fold_helper_view_in(&conn, &seen).unwrap()
    };
    let by = |m: u32, n: &str| after.iter().find(|c| c.major == m && c.name == n).unwrap().clone();
    assert_eq!((by(16, "main").role.as_str(), by(16, "main").origin.as_str()), ("k2", "baked"), "first look: the 5432 cluster is K2's");
    assert_eq!((by(17, "main").role.as_str(), by(17, "main").origin.as_str()), ("adopted", "adopted"), "a cluster K2 didn't make");
    assert_eq!(by(16, "extra").role, "foreign");
    assert!(parse_helper_clusters(r#"[{"major":9,"port":5432}]"#).is_err());
    assert!(parse_helper_clusters(r#"[{"major":17}]"#).is_err(), "no port");
    assert!(parse_helper_clusters(r#"[{"major":17,"port":5433,"startConf":"sometimes"}]"#).is_err());
    assert!(parse_helper_clusters("not json").is_err());
}

#[test]
fn ensure_cluster_records_what_the_helper_made() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    // Policy first: 18 is not available in 0.46.3, nothing is asked.
    let none = FakeSystemOps::baked();
    assert_eq!(ensure_cluster(&none, 18, false).unwrap_err().code(), "unsupported_major");
    assert_eq!(ensure_cluster(&none, 15, false).unwrap_err().code(), "unknown_major");
    assert!(lines_with(&none, "ensure-cluster").is_empty());
    // Recorded: no helper call.
    assert_eq!(ensure_cluster(&none, 16, false).unwrap().port, 5432);
    // An older helper: helper_needed.
    let e = ensure_cluster(&none, 17, false).unwrap_err();
    assert_eq!(e.code(), "helper_needed", "{e:?}");
    // Outside the band, or a port K2 recorded for another major: refused.
    for (port, why) in [(5460, "outside"), (5432, "recorded for the 16")] {
        let mut ops = FakeSystemOps::baked();
        ops.query_answers.insert(
            "helper ensure-cluster 17".into(),
            format!(r#"{{"major":17,"name":"main","port":{port},"dataDir":"/var/lib/postgresql/17/main","running":true,"startConf":"auto"}}"#),
        );
        let e = ensure_cluster(&ops, 17, false).unwrap_err();
        assert!(e.hint().contains(why), "{e:?}");
    }
    let mut ops = FakeSystemOps::baked();
    ops.query_answers.insert(
        "helper ensure-cluster 17".into(),
        r#"{"major":17,"name":"main","port":5433,"dataDir":"/var/lib/postgresql/17/main","running":true,"startConf":"auto"}"#.into(),
    );
    let c = ensure_cluster(&ops, 17, false).unwrap();
    assert_eq!((c.major, c.port, c.origin.as_str(), c.role.as_str()), (17, 5433, "created", "k2"));
    assert_eq!(for_major(17).unwrap().port, 5433, "recorded");
    assert_eq!(lines_with(&ops, "ensure-cluster").len(), 1);
    ensure_cluster(&ops, 17, false).unwrap();
    assert_eq!(lines_with(&ops, "ensure-cluster").len(), 1, "second ask reads the record");
}

#[test]
fn default_major_is_17_until_the_owner_pins_one() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    assert_eq!(default_major(), (17, false));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        set_default_major_in(&conn, 16).unwrap();
        assert_eq!(set_default_major_in(&conn, 18).unwrap_err().code(), "unsupported_major");
        assert_eq!(set_default_major_in(&conn, 14).unwrap_err().code(), "unknown_major");
    }
    assert_eq!(default_major(), (16, true));
}

#[test]
fn record_primary_never_rewrites_a_different_major() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    let db = k2_core::db::shared();
    let conn = db.lock();
    let c = record_primary_major(&conn, 16).unwrap();
    assert_eq!((c.major, c.port, c.state.as_str()), (16, 5432, "online"));
    assert!(record_primary_major(&conn, 16).is_ok(), "idempotent");
    let e = record_primary_major(&conn, 17).unwrap_err();
    assert!(e.contains("answers Postgres 17") && e.contains("recorded 16"), "{e}");
    assert_eq!(primary_in(&conn).major, 16);
}

/// The whole create → use path on a box with both clusters: a database on
/// 17 gets its roles, CREATE DATABASE, grants and seal on the 17 cluster
/// (`--cluster 17`), a 5433 DSN, `-p 5433` for every client call and the
/// 17 binaries; one on 16 keeps today's argv. The boot seal reaches each
/// database on its own cluster (V50).
#[test]
fn every_caller_goes_through_the_resolver() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    record(17, 5433, "k2");
    let mut ops = FakeSystemOps::baked();
    ops.existing_paths.push("/usr/lib/postgresql/17/bin/pg_dump".into());
    ops.existing_paths.push("/usr/lib/postgresql/17/bin/psql".into());
    let secrets = MemSecretStore::default();
    let (p17, path17) = insert_project("pg17");
    let (p16, _path16) = insert_project("pg16");

    // Default (17, recorded): no --major needed.
    let v = ops::create_database(&ops, &secrets, &p17, 1, None, None).unwrap();
    let name17 = v["name"].as_str().unwrap().to_string();
    assert_eq!(v["major"], 17);
    assert!(v["dsn"].as_str().unwrap().contains("@127.0.0.1:5433/"), "{v}");
    assert!(v.get("majorNote").is_none());
    assert_eq!(major_of(&name17), Some(17));
    let created = ops.recorded();
    let psql: Vec<&String> = created.iter().filter(|l| l.starts_with("helper psql")).collect();
    assert!(!psql.is_empty());
    for l in &psql {
        assert!(l.starts_with("helper psql --cluster 17 -d "), "every create step on 17: {l}");
    }

    // Explicit 16: today's argv, a 5432 DSN.
    let v16 = ops::create_database_on(&ops, &secrets, &p16, 1, None, None, Some(16)).unwrap();
    let name16 = v16["name"].as_str().unwrap().to_string();
    assert_eq!(v16["major"], 16);
    assert!(v16["dsn"].as_str().unwrap().contains("@127.0.0.1:5432/"), "{v16}");
    assert_eq!(major_of(&name16), Some(16));
    let n = created.len();
    for l in ops.recorded()[n..].iter().filter(|l| l.starts_with("helper psql")) {
        assert!(!l.contains("--cluster"), "16 is the 5432 cluster: {l}");
    }

    // The agent's own calls on the 17 database: port and binary.
    let before = ops.recorded().len();
    ops::store_put(&ops, &secrets, &p17, "notes", "n1", &serde_json::json!({"t": 1})).unwrap();
    let calls = ops.recorded()[before..].to_vec();
    let client: Vec<&String> = calls.iter().filter(|l| l.starts_with("cmd ")).collect();
    assert!(!client.is_empty(), "{calls:?}");
    for l in &client {
        assert!(l.starts_with("cmd /usr/lib/postgresql/17/bin/psql -h 127.0.0.1 -p 5433 "), "{l}");
    }
    // DSN on read.
    let dsn = ops::dsn_for_project(&ops, &secrets, &p17, 1).unwrap();
    assert!(dsn["dsn"].as_str().unwrap().contains(":5433/"), "{dsn}");
    let dsn16 = ops::dsn_for_project(&ops, &secrets, &p16, 1).unwrap();
    assert!(dsn16["dsn"].as_str().unwrap().contains(":5432/"), "{dsn16}");

    // Dump: 17's pg_dump on 5433.
    let before = ops.recorded().len();
    ops::dump(&ops, &secrets, &p17, &path17, None).unwrap();
    let dump: Vec<String> = ops.recorded()[before..].iter().filter(|l| l.contains("pg_dump")).cloned().collect();
    assert_eq!(dump.len(), 1, "{dump:?}");
    assert!(dump[0].starts_with("cmd /usr/lib/postgresql/17/bin/pg_dump -Fc -h 127.0.0.1 -p 5433 "), "{}", dump[0]);

    // Boot seal (V50): each database on the cluster its record names.
    let before = ops.recorded().len();
    let lines = ops::seal_existing_databases(&ops);
    assert!(lines.iter().all(|l| l.ends_with(": sealed")), "{lines:?}");
    let seal = ops.recorded()[before..].to_vec();
    assert!(seal.iter().any(|l| l.starts_with(&format!("helper psql --cluster 17 -d {name17} "))), "{seal:?}");
    assert!(seal.iter().any(|l| l.starts_with(&format!("helper psql -d {name16} "))), "{seal:?}");
    assert!(!seal.iter().any(|l| l.contains(&format!("--cluster 17 -d {name16} "))), "{seal:?}");

    // Grant the 17 database to the 16 workspace: its login (minted on 16)
    // is brought into 17 with its vaulted password, never ALTERed, and
    // CONNECT is granted on 17.
    let before = ops.recorded().len();
    ops::grant_access(&ops, &secrets, None, &name17, &p16, "read", false, &[]).unwrap();
    let grant = ops.recorded()[before..].to_vec();
    assert!(
        grant.iter().filter(|l| l.starts_with("helper psql")).all(|l| l.starts_with("helper psql --cluster 17 ")),
        "the grant happens on the 17 cluster: {grant:?}"
    );
    let helper_sql = ops.pg.lock().unwrap().helper_sql.clone();
    let login = ops::default_agent_role(&p16);
    assert!(
        helper_sql.iter().any(|s| s.contains(&format!("GRANT CONNECT ON DATABASE \"{name17}\" TO \"{login}\""))),
        "CONNECT on 17"
    );

    // Drop the 17 database: on 17.
    let before = ops.recorded().len();
    ops::drop_database(&ops, &p17).unwrap();
    let drop = ops.recorded()[before..].to_vec();
    assert!(drop.iter().any(|l| l.starts_with("helper psql --cluster 17 -d postgres ")), "{drop:?}");
}

#[test]
fn create_major_choice_and_refusals() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    let ops = FakeSystemOps::baked();
    let secrets = MemSecretStore::default();
    let (pa, _) = insert_project("pick-a");
    // K2's default (17) isn't on this box and the helper is too old: the
    // 5432 cluster, with a note.
    let v = ops::create_database(&ops, &secrets, &pa, 1, None, None).unwrap();
    assert_eq!(v["major"], 16);
    let note = v["majorNote"].as_str().unwrap();
    assert!(note.contains("Postgres 17") && note.contains("k2 db upgrade"), "{note}");
    // The test database follows the live one.
    let t = ops::create_test_database(&ops, &secrets, &pa, 1, None).unwrap();
    assert_eq!(t["major"], 16);
    assert!(t.get("majorNote").is_none());
    // Explicit majors are exact.
    let (pb, _) = insert_project("pick-b");
    let refused = |m: u32| match ops::create_database_on(&ops, &secrets, &pb, 1, None, None, Some(m)) {
        Err(OpsError::Refused(code, hint)) => (code, hint),
        other => panic!("--major {m} must be refused, got {other:?}"),
    };
    assert_eq!(refused(17).0, "helper_needed");
    assert_eq!(refused(18).0, "unsupported_major");
    assert_eq!(refused(15).0, "unknown_major");
    assert_eq!(OpsError::Refused("x", String::new()).status(), "409 Conflict");
    // A pinned default is exact too.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        set_default_major_in(&conn, 17).unwrap();
    }
    match ops::create_database(&ops, &secrets, &pb, 1, None, None) {
        Err(OpsError::Refused(code, _)) => assert_eq!(code, "helper_needed"),
        other => panic!("pinned 17 without a 17 cluster must refuse, got {other:?}"),
    }
    // An adopted 17 cluster is never created into.
    record(17, 5433, "adopted");
    match ops::create_database_on(&ops, &secrets, &pb, 1, None, None, Some(17)) {
        Err(OpsError::Refused(code, hint)) => {
            assert_eq!(code, "cluster_refused");
            assert!(hint.contains("adopted"), "{hint}");
        }
        other => panic!("adopted cluster must refuse, got {other:?}"),
    }
    assert!(lines_with(&ops, "--cluster").is_empty(), "nothing was sent to a cluster K2 doesn't own");
}

#[test]
fn status_lists_majors_and_upgrades() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    record(17, 5433, "k2");
    db_row("d-a", "ws_a", Some(16));
    db_row("d-b", "ws_b", Some(17));
    let v = crate::sql::supervisor::majors_json();
    assert_eq!(v["clusters"].as_array().unwrap().len(), 2);
    assert_eq!(v["defaultMajor"], 17);
    assert_eq!(v["defaultMajorPinned"], false);
    let up = v["upgradeAvailable"].as_array().unwrap();
    assert_eq!(up.len(), 1, "{v}");
    assert_eq!(up[0]["name"], "ws_a");
    assert_eq!(up[0]["from"], 16);
    assert_eq!(up[0]["to"], serde_json::json!([17]));
    assert_eq!(up[0]["check"], "k2 db upgrade ws_a --check");
    let dbs = v["databases"].as_array().unwrap();
    assert!(dbs.iter().any(|d| d["name"] == "ws_b" && d["major"] == 17));
}
