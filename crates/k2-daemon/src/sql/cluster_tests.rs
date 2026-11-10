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

/// `k2 db restore`'s `pg_restore`: the newest installed major at or above
/// the cluster's (a newer pg_restore reads a newer pg_dump's archive into
/// an older server; an older one can't), never an older major's, else
/// today's `client_bin` choice.
#[test]
fn restore_bin_takes_the_newest_installed_major_at_or_above_the_cluster() {
    const R16: &str = "/usr/lib/postgresql/16/bin/pg_restore";
    const R17: &str = "/usr/lib/postgresql/17/bin/pg_restore";
    const R18: &str = "/usr/lib/postgresql/18/bin/pg_restore";
    let with = |paths: &[&str]| {
        let mut o = FakeSystemOps::baked();
        o.existing_paths.extend(paths.iter().map(|p| p.to_string()));
        o
    };
    let c16 = Cluster::layout(16, 5432, "baked");
    let c17 = Cluster::layout(17, 5433, "created");

    assert_eq!(c16.restore_bin(&with(&[R16, R17])), (R17.to_string(), Some(17)), "16 cluster, 16 + 17 installed");
    assert_eq!(c16.restore_bin(&with(&[R16])), (R16.to_string(), Some(16)), "16 cluster, only 16");
    assert_eq!(c16.restore_bin(&with(&[R16, R17, R18])), (R18.to_string(), Some(18)), "the newest one");
    assert_eq!(c16.restore_bin(&with(&[])), ("/usr/bin/pg_restore".to_string(), None), "none: today's wrapper");

    assert_eq!(c17.restore_bin(&with(&[R16, R17])), (R17.to_string(), Some(17)), "17 cluster, 16 + 17");
    assert_eq!(c17.restore_bin(&with(&[R16])), ("/usr/bin/pg_restore".to_string(), None), "17 never takes 16's");

    // An unknown major keeps today's choice even with clients installed.
    assert_eq!(Cluster::legacy_primary(0).restore_bin(&with(&[R16, R17])), ("/usr/bin/pg_restore".to_string(), None));
    // Only pg_restore: the other tools keep their own major's binary.
    let both = with(&[R16, R17, "/usr/lib/postgresql/16/bin/pg_dump", "/usr/lib/postgresql/17/bin/pg_dump"]);
    assert_eq!(c16.client_bin(&both, "pg_dump"), "/usr/lib/postgresql/16/bin/pg_dump");
    assert_eq!(c16.client_bin(&both, "pg_restore"), R16, "client_bin itself is unchanged");
}

/// End to end through `k2 db restore`: a database on the 16 cluster is
/// restored with 17's pg_restore when 17's client is installed, with 16's
/// when only 16's is, and a database on 17 never gets 16's.
#[test]
fn restore_runs_the_newest_pg_restore_for_the_databases_cluster() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    record(17, 5433, "k2");
    let mut ops = FakeSystemOps::baked();
    let secrets = MemSecretStore::default();
    let (p16, path16) = insert_project("restore16");
    let (p17, path17) = insert_project("restore17");
    ops::create_database_on(&ops, &secrets, &p16, 1, None, None, Some(16)).unwrap();
    ops::create_database_on(&ops, &secrets, &p17, 1, None, None, Some(17)).unwrap();
    let rel = ".k2/db/dumps/r.dump";
    for path in [&path16, &path17] {
        let dumps = std::path::Path::new(path).join(".k2/db/dumps");
        std::fs::create_dir_all(&dumps).unwrap();
        std::fs::write(dumps.join("r.dump"), b"FAKE-PG-DUMP").unwrap();
        std::fs::write(dumps.join("r.dump.grants.json"), br#"{"databaseId":"","databaseName":"","grants":[]}"#).unwrap();
    }
    let restore_line = |ops: &FakeSystemOps, pid: &str, path: &str| -> String {
        let before = ops.recorded().len();
        let v = ops::restore(ops, &secrets, pid, path, rel).unwrap();
        assert_eq!(v["ok"], true, "{v}");
        let lines: Vec<String> =
            ops.recorded()[before..].iter().filter(|l| l.contains("pg_restore")).cloned().collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        lines[0].clone()
    };

    ops.existing_paths.push("/usr/lib/postgresql/16/bin/pg_restore".into());
    let l = restore_line(&ops, &p16, &path16);
    assert!(l.starts_with("cmd /usr/lib/postgresql/16/bin/pg_restore -h 127.0.0.1 -p 5432 "), "only 16: {l}");
    let l = restore_line(&ops, &p17, &path17);
    assert!(l.starts_with("cmd /usr/bin/pg_restore -h 127.0.0.1 -p 5433 "), "17 never takes 16's: {l}");

    ops.existing_paths.push("/usr/lib/postgresql/17/bin/pg_restore".into());
    let l = restore_line(&ops, &p16, &path16);
    assert!(l.starts_with("cmd /usr/lib/postgresql/17/bin/pg_restore -h 127.0.0.1 -p 5432 "), "16 + 17 on a 16 cluster: {l}");
    let l = restore_line(&ops, &p17, &path17);
    assert!(l.starts_with("cmd /usr/lib/postgresql/17/bin/pg_restore -h 127.0.0.1 -p 5433 "), "{l}");
    for path in [path16, path17] {
        let _ = std::fs::remove_dir_all(path);
    }
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
fn default_major_follows_the_box_until_the_owner_moves_it() {
    let _g = crate::sql::sql_server_test_lock();
    // An existing 16 box (unset default): the 5432 cluster's major, never
    // a silent 17 (PRD Q8; keep today's path until the new one is proven).
    reset(Some(16));
    let source = |_: ()| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        default_major_source_in(&conn)
    };
    assert_eq!(default_major(), (16, true));
    assert_eq!(source(()), (16, "primary"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        set_default_major_in(&conn, 17).unwrap();
        assert_eq!(set_default_major_in(&conn, 18).unwrap_err().code(), "unsupported_major");
        assert_eq!(set_default_major_in(&conn, 14).unwrap_err().code(), "unknown_major");
    }
    assert_eq!(default_major(), (17, true), "after k2 db default-major 17");
    assert_eq!(source(()), (17, "set"));
    // A fresh box (no Postgres major read yet): K2's new-install major.
    reset(None);
    assert_eq!(default_major(), (17, false));
    assert_eq!(source(()), (17, "new-install"));
}

/// The 0.46.2 → 0.46.3 upgrade of a 16 box: 0201 recorded 16/main on
/// 5432 and pinned the default to 16. `k2 db create` with no `--major`
/// lands on 16 (5432), asks the helper for nothing new, creates no
/// cluster and carries no note. The same holds with the default unset.
#[test]
fn an_upgraded_16_box_keeps_creating_on_16() {
    let _g = crate::sql::sql_server_test_lock();
    for pinned in [true, false] {
        reset(Some(16));
        record(16, 5432, "k2");
        if pinned {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE sql_server SET default_major = 16 WHERE id = 1", []).unwrap();
        }
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let (pid, _) = insert_project(if pinned { "up16-pinned" } else { "up16-unset" });
        let v = ops::create_database(&ops, &secrets, &pid, 1, None, None).unwrap();
        let name = v["name"].as_str().unwrap().to_string();
        assert_eq!(v["major"], 16, "pinned={pinned}: {v}");
        assert!(v["dsn"].as_str().unwrap().contains("@127.0.0.1:5432/"), "{v}");
        assert!(v.get("majorNote").is_none(), "{v}");
        assert_eq!(major_of(&name), Some(16));
        assert!(lines_with(&ops, "ensure-cluster").is_empty(), "no new cluster asked for");
        assert!(lines_with(&ops, "--cluster").is_empty(), "today's argv on 5432");
        assert_eq!(list().len(), 1, "no cluster recorded beyond 16/main");
    }
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

    // The owner moved the default (`k2 db default-major 17`): no --major
    // needed for 17 from here on.
    assert_eq!(default_major(), (16, true), "before: the 5432 cluster's major");
    let r = crate::sql::routes::handle_server_default_major(br#"{"major":17}"#);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(default_major(), (17, true));
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
    // A box whose Postgres major was never read: K2's new-install default
    // (17) isn't there and the helper is too old, so the 5432 cluster,
    // with a note.
    reset(None);
    let ops = FakeSystemOps::baked();
    let secrets = MemSecretStore::default();
    let (p0, _) = insert_project("pick-0");
    let v = ops::create_database(&ops, &secrets, &p0, 1, None, None).unwrap();
    assert!(v["major"].is_null(), "{v}");
    let note = v["majorNote"].as_str().unwrap();
    assert!(note.contains("Postgres 17") && note.contains("k2 db upgrade"), "{note}");
    // An existing 16 box: 16, no note.
    reset(Some(16));
    let (pa, _) = insert_project("pick-a");
    let v = ops::create_database(&ops, &secrets, &pa, 1, None, None).unwrap();
    assert_eq!(v["major"], 16);
    assert!(v.get("majorNote").is_none(), "{v}");
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
    // A hand-made cluster off K2's layout (foreign) is refused too, and the
    // hint names it, the one-cluster-per-major rule and what the owner can do.
    record(17, 5433, "foreign");
    match ops::create_database_on(&ops, &secrets, &pb, 1, None, None, Some(17)) {
        Err(OpsError::Refused(code, hint)) => {
            assert_eq!(code, "cluster_refused");
            for want in ["17/main", "port 5433", "recorded foreign", "k2 db clusters", "k2 db cluster create 17 --name", "no command to take this cluster over"] {
                assert!(hint.contains(want), "missing {want:?}: {hint}");
            }
        }
        other => panic!("foreign cluster must refuse, got {other:?}"),
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
    assert_eq!(v["defaultMajor"], 16, "an existing 16 box");
    assert_eq!(v["defaultMajorPinned"], false);
    assert_eq!(v["defaultMajorSource"], "primary");
    let up = v["upgradeAvailable"].as_array().unwrap();
    assert_eq!(up.len(), 1, "{v}");
    assert_eq!(up[0]["name"], "ws_a");
    assert_eq!(up[0]["from"], 16);
    assert_eq!(up[0]["to"], serde_json::json!([17]));
    assert_eq!(up[0]["check"], "k2 db upgrade ws_a --check");
    let dbs = v["databases"].as_array().unwrap();
    assert!(dbs.iter().any(|d| d["name"] == "ws_b" && d["major"] == 17));
}

/// Smoke r6: with the sidecar stopped, `k2 db status` never suggests
/// `k2 db upgrade <db> --check` (it can't run); it says how to start
/// Postgres instead, the same words the upgrade route answers with.
#[test]
fn status_with_postgres_stopped_says_start_it_not_check() {
    let _g = crate::sql::sql_server_test_lock();
    crate::sql::pgmove::worker::reset_for_tests();
    reset(Some(16));
    record(16, 5432, "k2");
    record(17, 5433, "k2");
    db_row("d-a", "ws_a", Some(16));
    // Running: the check is the next step.
    let v = crate::sql::supervisor::status_json(false);
    assert!(v["upgradeBlocked"].is_null(), "{v}");
    assert_eq!(v["upgradeAvailable"][0]["check"], "k2 db upgrade ws_a --check");
    // Stopped (what the health loop writes).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE sql_server SET status = 'stopped', last_error = 'systemd reports the postgresql unit is ''inactive''' WHERE id = 1",
            [],
        )
        .unwrap();
    }
    let v = crate::sql::supervisor::status_json(false);
    assert_eq!(v["upgradeBlocked"]["code"], "postgres_stopped", "{v}");
    let hint = v["upgradeBlocked"]["hint"].as_str().unwrap();
    assert!(hint.contains("start it with `k2 db enable`") && hint.contains("inactive"), "{hint}");
    let up = &v["upgradeAvailable"][0];
    assert_eq!(up["name"], "ws_a");
    assert!(up["check"].is_null(), "no --check while it can't run: {up}");
    assert_eq!(up["blockedBy"], "postgres_stopped");
    assert_eq!(up["next"], hint);
    assert!(!v.to_string().contains("--check"), "nothing in status suggests --check: {v}");
    // The route's own words for the same state.
    let (status, code, route_hint) =
        crate::sql::pgmove::k2::sidecar_refusal(Some("stopped"), Some("systemd reports the postgresql unit is 'inactive'"), &[])
            .expect("refused");
    assert_eq!((status, code), (409, "postgres_stopped"));
    assert_eq!(route_hint, hint);
}

/// `create --major` other than the default: an agent passport needs the
/// "manage Postgres versions" switch; the owner token never does; the
/// default major never needs it.
#[test]
fn create_on_a_non_default_major_needs_the_switch_for_agents() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    let (pid, _) = insert_project("gate");
    let agent = Some(crate::session_token::HookPrincipal { workspace_uuid: pid.clone(), agent_address: "a".into() });
    let as_agent =
        |m: u32| crate::caller_workspace::with_request_principal(agent.clone(), || crate::sql::routes::may_choose_major(&pid, m));
    let refused = as_agent(17).unwrap_err();
    assert_eq!(refused.status, "403 Forbidden");
    assert!(refused.body.contains("db_upgrade_off"), "{}", refused.body);
    assert!(as_agent(16).is_ok(), "the default major (this 16 box's) needs no switch");
    assert!(crate::sql::routes::may_choose_major(&pid, 17).is_ok(), "the owner token");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET agents_can_upgrade_postgres = 1 WHERE id = ?1", [&pid]).unwrap();
    }
    assert!(as_agent(17).is_ok(), "with the switch on");
}

/// `k2 db default-major` is the owner's: an Owner-floor POST, GET is 405,
/// and agent passports are denied by the `/cli/db/server/` prefix (an
/// agent with the Postgres versions switch may only suggest it).
#[test]
fn default_major_is_owner_only() {
    use crate::routes::route_policy::{lookup, Floor};
    let row = lookup("/cli/db/server/default-major").expect("policy row");
    assert_eq!(row.post, Some(Floor::Owner));
    assert_eq!(row.get, None);
    assert!(!crate::session_token::is_agent_verb("/cli/db/server/default-major"));
    let get = crate::sql_routes::dispatch("/cli/db/server/default-major", &Default::default()).expect("claimed");
    assert_eq!(get.status, "405 Method Not Allowed");
    assert!(
        crate::sql::versions_access::decide(
            &crate::sql::versions_access::Who::Agent {
                project_id: "p".into(),
                session_id: "s".into(),
                local: true,
                sandboxed: false
            },
            crate::sql::versions_access::VersionsAct::DefaultMajor,
            Some("p"),
            false,
            true,
        )
        .is_err(),
        "the switch does not grant default-major"
    );
}

// ── named clusters (0.46.11, prd-named-pg-clusters-v1) ──────────────────

const FEEDBACK: &str = r#"{"major":17,"name":"feedback","port":5434,"dataDir":"/var/lib/postgresql/17/feedback","running":true,"startConf":"auto","k2Named":true,"walArchive":true,"systemId":"7354340000000000000","created":true}"#;

/// akzm: 16/main on 5432 (K2's), the hand-made `17/k2db` on 5433, and a
/// helper on protocol 15 that made `17/feedback` when asked.
fn akzm_ops() -> FakeSystemOps {
    let mut ops = FakeSystemOps::baked();
    ops.query_answers.insert("helper pgnamed".into(), String::new());
    ops.query_answers.insert("helper ensure-cluster 17-feedback --wal-archive".into(), FEEDBACK.into());
    ops
}

fn helper_view(feedback_vouched: Option<bool>) -> Vec<Cluster> {
    let mut rows = vec![
        r#"{"major":16,"name":"main","port":5432,"dataDir":"/var/lib/postgresql/16/main","running":true,"startConf":"auto"}"#.to_string(),
        r#"{"major":17,"name":"k2db","port":5433,"dataDir":"/var/lib/postgresql/17/k2db","running":true,"startConf":"auto","k2Named":false}"#.to_string(),
    ];
    if let Some(v) = feedback_vouched {
        rows.push(format!(
            r#"{{"major":17,"name":"feedback","port":5434,"dataDir":"/var/lib/postgresql/17/feedback","running":true,"startConf":"auto","k2Named":{v}}}"#
        ));
    }
    parse_helper_clusters(&format!("[{}]", rows.join(","))).unwrap()
}

fn fold(view: &[Cluster]) -> Vec<Cluster> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    fold_helper_view_in(&conn, view).unwrap()
}

fn cluster_name_of(db: &str) -> Option<String> {
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    db_cluster_name_in(&conn, db)
}

#[test]
fn named_cluster_never_adopts_a_foreign_one() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    let (pid, _) = insert_project("fb-owner");
    let (other, _) = insert_project("fb-other");
    let secrets = MemSecretStore::default();
    // The helper's view: 17/k2db is never K2's, whatever it is called.
    let after = fold(&helper_view(None));
    let k2db = after.iter().find(|c| c.name == "k2db").unwrap();
    assert_eq!((k2db.role.as_str(), k2db.port), ("foreign", 5433));
    // Even a helper that (wrongly) vouches for a cluster K2 never recorded
    // doesn't make it K2's: only `k2 db cluster create` records one.
    let mut bogus = helper_view(None);
    bogus[1].k2_named = Some(true);
    assert_eq!(fold(&bogus).iter().find(|c| c.name == "k2db").unwrap().role, "foreign");

    let ops = akzm_ops();
    // It can't be (re)made, created into, restarted or reached by name.
    let e = ensure_named_cluster(&ops, 17, "k2db", None, false, &pid).unwrap_err();
    assert_eq!(e.code(), "cluster_refused", "{e:?}");
    assert!(e.hint().contains("17/k2db") && e.hint().contains("didn't make"), "{}", e.hint());
    match ops::create_database_in_cluster(&ops, &secrets, &pid, 5, None, Some("staging_x"), None, "k2db") {
        Err(OpsError::Refused(code, _)) => assert_eq!(code, "cluster_refused"),
        other => panic!("17/k2db must refuse, got {other:?}"),
    }
    assert_eq!(named_target("k2db", None).unwrap_err().code(), "cluster_refused");
    assert!(lines_with(&ops, "k2db").is_empty(), "nothing was ever sent naming 17/k2db: {:?}", ops.recorded());

    // 17/feedback: made through the helper, recorded K2's, this workspace's.
    let c = ensure_named_cluster(&ops, 17, "feedback", None, true, &pid).unwrap();
    assert_eq!((c.label().as_str(), c.port, c.role.as_str(), c.origin.as_str()), ("17/feedback", 5434, "k2", "created"));
    assert_eq!(c.owner_project.as_deref(), Some(pid.as_str()));
    assert!(c.wal_archive);
    assert_eq!(lines_with(&ops, "helper ensure-cluster 17-feedback --wal-archive").len(), 1);
    // The name is the box's: no second `feedback` on another major.
    assert_eq!(ensure_named_cluster(&ops, 16, "feedback", None, false, &pid).unwrap_err().code(), "cluster_refused");
    // Another workspace's cluster: not yours.
    assert_eq!(ensure_named_cluster(&ops, 17, "feedback", None, true, &other).unwrap_err().code(), "cluster_not_yours");

    // A database in it: its psql goes `--cluster 17-feedback`, its record
    // names the cluster, its DSN the cluster's port.
    let v = ops::create_database_in_cluster(&ops, &secrets, &pid, 5, None, Some("feedback"), Some(17), "feedback").unwrap();
    assert_eq!(v["cluster"], "17/feedback", "{v}");
    assert_eq!(v["major"], 17);
    assert!(v["dsn"].as_str().unwrap().contains(":5434/feedback"), "{v}");
    assert_eq!(cluster_name_of("feedback").as_deref(), Some("feedback"));
    assert_eq!(for_db("feedback").unwrap().label(), "17/feedback");
    assert!(!lines_with(&ops, "helper psql --cluster 17-feedback -d postgres").is_empty(), "{:?}", ops.recorded());
    for l in ops.recorded().iter().filter(|l| l.starts_with("helper psql")) {
        assert!(helper_psql_argv_ok(&l["helper psql ".len()..].split(" stdin=").next().unwrap().split(' ').collect::<Vec<_>>()), "{l}");
    }
    // --major other than the cluster's: refused. Another workspace: refused.
    match ops::create_database_in_cluster(&ops, &secrets, &pid, 5, None, Some("fb2"), Some(16), "feedback") {
        Err(OpsError::Refused(code, _)) => assert_eq!(code, "major_mismatch"),
        other => panic!("{other:?}"),
    }
    match ops::create_database_in_cluster(&ops, &secrets, &other, 5, None, Some("fb3"), None, "feedback") {
        Err(OpsError::Refused(code, _)) => assert_eq!(code, "cluster_not_yours"),
        other => panic!("{other:?}"),
    }
    // A plain `k2 db create --major 17` never lands in either named
    // cluster: it asks for 17/main (this fake helper has no answer).
    match ops::create_database_on(&ops, &secrets, &other, 5, None, Some("plain17"), Some(17)) {
        Err(OpsError::Refused(code, _)) => assert_eq!(code, "helper_needed"),
        other => panic!("{other:?}"),
    }
    assert!(lines_with(&ops, "plain17").iter().all(|l| !l.contains("--cluster 17-")), "{:?}", ops.recorded());
    // The upgrade list leaves it out (a named cluster keeps its major).
    let m = crate::sql::supervisor::majors_json();
    assert!(m["upgradeAvailable"].as_array().unwrap().iter().all(|u| u["name"] != "feedback"), "{m}");
    assert!(m["databases"].as_array().unwrap().iter().any(|d| d["name"] == "feedback" && d["cluster"] == "17/feedback"), "{m}");

    // §9a: the App's door ports are its workspace's K2 clusters only, never
    // 17/k2db's 5433 (its workspace has a database on 5434 alone).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: "feedback",
                service_id: "svc-feedback-named",
                seed_id: None,
                owner_workspace: &pid,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
    }
    assert_eq!(crate::apps::pg_choice::door_ports("feedback"), vec![5434]);
    assert!(crate::apps::pg_choice::door_ports("no-such-app").is_empty());

    // The helper stops vouching for it (the list or the cluster changed):
    // K2 keeps its record but no longer counts it as online.
    let after = fold(&helper_view(Some(false)));
    assert_eq!(after.iter().find(|c| c.name == "feedback").unwrap().state, "unknown");
    let after = fold(&helper_view(Some(true)));
    let fb = after.iter().find(|c| c.name == "feedback").unwrap();
    assert_eq!((fb.state.as_str(), fb.role.as_str(), fb.owner_project.as_deref()), ("online", "k2", Some(pid.as_str())));
    assert!(fb.wal_archive, "a refresh keeps what the create recorded");
}

/// Old helper, new daemon: nothing named is ever sent to a helper below
/// protocol 15 (it would refuse the argv anyway).
#[test]
fn named_clusters_need_helper_15_and_send_nothing_before() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    let (pid, _) = insert_project("fb-old-helper");
    let old = FakeSystemOps::baked();
    let e = ensure_named_cluster(&old, 17, "feedback", None, true, &pid).unwrap_err();
    assert_eq!(e.code(), "helper_needed", "{e:?}");
    assert!(e.hint().contains("protocol 15"), "{}", e.hint());
    assert!(old.recorded().iter().all(|l| !l.contains("ensure-cluster")), "{:?}", old.recorded());
    // A cluster recorded while the helper was 15, then the helper rolled
    // back: databases in it refuse before any psql.
    let new = akzm_ops();
    ensure_named_cluster(&new, 17, "feedback", None, true, &pid).unwrap();
    let secrets = MemSecretStore::default();
    match ops::create_database_in_cluster(&old, &secrets, &pid, 5, None, Some("feedback"), None, "feedback") {
        Err(OpsError::Refused(code, _)) => assert_eq!(code, "helper_needed"),
        other => panic!("{other:?}"),
    }
    assert!(old.recorded().iter().all(|l| !l.contains("--cluster 17-feedback")), "{:?}", old.recorded());
    assert!(crate::sql::sysops::needs_pgnamed(&["psql", "--cluster", "17-feedback", "-d", "x"]));
    assert!(crate::sql::sysops::needs_pgnamed(&["ensure-cluster", "17-feedback"]));
    assert!(crate::sql::sysops::needs_pgnamed(&["systemctl", "restart", "17-feedback"]));
    assert!(!crate::sql::sysops::needs_pgnamed(&["psql", "--cluster", "17", "-d", "x"]));
    assert!(!crate::sql::sysops::needs_pgnamed(&["ensure-cluster", "17"]));
}

/// New helper, old daemon: the helper's extra `k2Named` field changes
/// nothing for a daemon that never sends the named verbs; a non-`main`
/// row it hasn't seen is foreign (0.46.3's rule, kept).
#[test]
fn a_new_helper_view_reads_the_same_for_main_clusters() {
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    let rows = parse_helper_clusters(
        r#"{"clusters":[{"major":16,"name":"main","port":5432,"dataDir":"/var/lib/postgresql/16/main","running":true,"startConf":"auto","k2Named":true},
            {"major":17,"name":"feedback","port":5434,"dataDir":"/var/lib/postgresql/17/feedback","running":true,"startConf":"auto","k2Named":true}]}"#,
    )
    .unwrap();
    assert_eq!(rows[0].k2_named, None, "k2Named means nothing on a main row");
    assert_eq!(rows[0].role, "k2");
    assert_eq!(rows[1].role, "foreign");
    let after = fold(&rows);
    assert_eq!(after.iter().find(|c| c.name == "feedback").unwrap().role, "foreign", "only a create records a named cluster");
}

/// `POST /cli/db/clusters` (named_routes): the owner, or a local agent
/// with the Postgres versions switch for its own workspace; never a Member.
#[test]
fn named_cluster_route_gate() {
    use crate::sql::versions_access::Who;
    let _g = crate::sql::sql_server_test_lock();
    reset(Some(16));
    record(16, 5432, "k2");
    let (pid, path) = insert_project("fb-route");
    let ops = akzm_ops();
    let body = |op: &str, extra: &str| format!(r#"{{"op":"{op}","project":"{path}"{extra}}}"#).into_bytes();
    let agent = Who::Agent { project_id: pid.clone(), session_id: "s".into(), local: true, sandboxed: false };
    let create = body("create", r#","major":17,"name":"feedback","walArchive":true"#);
    let p = Default::default();
    let r = crate::sql::named_routes::handle(&ops, Who::Member { username: "m".into() }, true, &create, &p);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let r = crate::sql::named_routes::handle(&ops, agent.clone(), true, &create, &p);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("db_upgrade_off"), "{}", r.body);
    let sandboxed = Who::Agent { project_id: pid.clone(), session_id: "s".into(), local: true, sandboxed: true };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET agents_can_upgrade_postgres = 1 WHERE id = ?1", [&pid]).unwrap();
    }
    assert_eq!(crate::sql::named_routes::handle(&ops, sandboxed, true, &create, &p).status, "403 Forbidden");
    let r = crate::sql::named_routes::handle(&ops, agent.clone(), true, &create, &p);
    assert!(r.status.starts_with("200"), "{}", r.body);
    assert!(r.body.contains("17/feedback"), "{}", r.body);
    // Restart: only the helper's systemctl for that cluster.
    let r = crate::sql::named_routes::handle(&ops, agent.clone(), true, &body("restart", r#","name":"feedback""#), &p);
    assert!(r.status.starts_with("200"), "{}", r.body);
    assert_eq!(lines_with(&ops, "helper systemctl restart 17-feedback").len(), 1, "{:?}", ops.recorded());
    // GET lists both, 17/k2db foreign; the owner sees health for K2's.
    let mut ops2 = akzm_ops();
    ops2.query_answers.insert(
        "helper clusters".into(),
        r#"{"clusters":[{"major":16,"name":"main","port":5432,"dataDir":"/var/lib/postgresql/16/main","running":true,"startConf":"auto"},
            {"major":17,"name":"k2db","port":5433,"dataDir":"/var/lib/postgresql/17/k2db","running":true,"startConf":"auto","k2Named":false},
            {"major":17,"name":"feedback","port":5434,"dataDir":"/var/lib/postgresql/17/feedback","running":true,"startConf":"auto","k2Named":true}]}"#
            .into(),
    );
    let mut q = std::collections::HashMap::new();
    q.insert("health".to_string(), "1".to_string());
    let r = crate::sql::named_routes::handle(&ops2, Who::Owner, false, b"", &q);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    let rows = v["clusters"].as_array().unwrap();
    let k2db = rows.iter().find(|c| c["name"] == "k2db").unwrap();
    assert_eq!(k2db["role"], "foreign");
    assert!(k2db.get("health").is_none(), "never SQL on a foreign cluster: {k2db}");
    assert!(rows.iter().find(|c| c["name"] == "feedback").unwrap().get("health").is_some(), "{v}");
    assert!(lines_with(&ops2, "17-k2db").is_empty() && lines_with(&ops2, "5433").is_empty(), "{:?}", ops2.recorded());
    // An unknown op, a GET with no credential.
    assert_eq!(crate::sql::named_routes::handle(&ops, agent, true, &body("drop", ""), &p).status, "400 Bad Request");
    assert_eq!(crate::sql::named_routes::handle(&ops, Who::Other, false, b"", &p).status, "403 Forbidden");
}

