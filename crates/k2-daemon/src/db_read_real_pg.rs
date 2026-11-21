//! `db.read` against a real Postgres: read means read. `#[ignore]`: it runs
//! only where REALPG_READ=1 is set, with a Postgres server's binaries
//! (`REALPG_BIN`: `initdb`, `pg_ctl`, `postgres`, `psql`; default
//! `/usr/lib/postgresql/17/bin`), as an ordinary user.
//!
//! It makes its own throwaway server under `/tmp` on port 5446 (K2's band,
//! never the box's own), records it as the box's cluster for its major in
//! the test's database, and drives the real routes: the reader agent's
//! `POST /cli/db/request-read`, the owner's Allow on the card
//! (`/cli/thread/answer`, which runs `/cli/db/grant`'s own entry), then
//! `k2 db dsn --db` for the reader's login. That login then SELECTs, and is
//! refused every INSERT, UPDATE, DELETE, TRUNCATE, sequence bump and DDL;
//! the owner's other database stays closed to it. Every check prints
//! PASS/FAIL; the test fails if any did.
//!
//! Run: `REALPG_READ=1 cargo test -p k2-daemon --lib real_pg_read -- --ignored --nocapture --test-threads=1`
//! with every K2_* variable cleared.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::permission::actions::held::tests::{agent, answer_as, card_of, seed};
use crate::sql::sysops::{RealSystemOps, SystemOps};

const PORT: u16 = 5446;

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into())
}

fn run(prog: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<String, String> {
    let mut e: Vec<(&str, &str)> = vec![("PSQLRC", "/dev/null"), ("LC_ALL", "C")];
    e.extend_from_slice(env);
    let out = RealSystemOps.run_cmd(prog, args, &e, stdin)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

struct Server {
    root: PathBuf,
}

impl Server {
    fn sock(&self) -> String {
        self.root.join("sock").to_string_lossy().into_owned()
    }
    fn data(&self) -> String {
        self.root.join("data").to_string_lossy().into_owned()
    }
    fn su(&self, db: &str, sql: &str) -> String {
        let p = PORT.to_string();
        run(&format!("{}/psql", bin()), &["-X", "-tA", "-h", &self.sock(), "-p", &p, "-U", "postgres", "-d", db, "-v", "ON_ERROR_STOP=1", "-c", sql], &[], None)
            .unwrap_or_else(|e| panic!("superuser SQL failed: {e}\n{sql}"))
    }
    /// One statement as `user` over TCP.
    fn as_role(&self, user: &str, password: &str, db: &str, sql: &str) -> Result<String, String> {
        let p = PORT.to_string();
        run(
            &format!("{}/psql", bin()),
            &["-X", "-q", "-tA", "-h", "127.0.0.1", "-p", &p, "-U", user, "-d", db, "-v", "ON_ERROR_STOP=1", "-c", sql],
            &[("PGPASSWORD", password), ("PGSSLMODE", "disable"), ("PGCONNECT_TIMEOUT", "5")],
            None,
        )
    }
}

struct Stop(&'static Server);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = run(&format!("{}/pg_ctl", bin()), &["-D", &self.0.data(), "-m", "immediate", "-w", "stop"], &[], None);
        let _ = std::fs::remove_dir_all(&self.0.root);
    }
}

impl SystemOps for Server {
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
        RealSystemOps.write_file(path, contents, mode)
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        RealSystemOps.read_file(path)
    }
    fn create_dir_all(&self, path: &str) -> Result<(), String> {
        RealSystemOps.create_dir_all(path)
    }
    fn remove_path(&self, path: &str) -> Result<(), String> {
        RealSystemOps.remove_path(path)
    }
    fn path_exists(&self, path: &str) -> bool {
        RealSystemOps.path_exists(path)
    }
    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        Err(format!("systemctl {} is not part of the test", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        "active".into()
    }
    /// The root helper's `psql [--cluster M] -d <db> …`: superuser, SQL on
    /// stdin, over the throwaway server's socket.
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        if args.first().copied() != Some("psql") {
            return Err(format!("helper {} is not part of the test", args.join(" ")));
        }
        let mut rest: Vec<&str> = Vec::new();
        let mut it = args[1..].iter();
        while let Some(a) = it.next() {
            if *a == "--cluster" {
                it.next();
                continue;
            }
            rest.push(a);
        }
        let p = PORT.to_string();
        let sock = self.sock();
        let mut full: Vec<&str> = vec!["-X", "-h", &sock, "-p", &p, "-U", "postgres"];
        full.extend(rest);
        run(&format!("{}/psql", bin()), &full, &[], stdin)
    }
    fn run_cmd(&self, cmd: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let tool = Path::new(cmd).file_name().and_then(|n| n.to_str()).unwrap_or(cmd);
        let mut env: Vec<(&str, &str)> = env.to_vec();
        env.push(("PSQLRC", "/dev/null"));
        RealSystemOps.run_cmd(&format!("{}/{tool}", bin()), args, &env, stdin)
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

fn check(name: &str, ok: bool, detail: impl std::fmt::Display, fails: &mut Vec<String>) {
    if ok {
        eprintln!("PASS {name}");
    } else {
        eprintln!("FAIL {name}: {detail}");
        fails.push(format!("{name}: {detail}"));
    }
}

/// Postgres refused it for want of a privilege (not a typo in the test).
fn denied(r: &Result<String, String>) -> bool {
    r.as_ref().is_err_and(|e| e.contains("permission denied") || e.contains("must be owner"))
}

#[test]
#[ignore = "runs only with REALPG_READ=1 and Postgres binaries (a Linux compute job)"]
fn real_pg_read_grant_reads_and_never_writes() {
    assert_eq!(std::env::var("REALPG_READ").as_deref(), Ok("1"), "REALPG_READ=1 marks the throwaway job");
    assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres (REALPG_BIN)", bin());
    let version = run(&format!("{}/postgres", bin()), &["--version"], &[], None).expect("postgres --version");
    let major: u32 = version.split_whitespace().last().and_then(|v| v.split('.').next()).and_then(|m| m.parse().ok()).expect("a major");
    eprintln!("server: {version} (major {major})");
    let root = PathBuf::from(format!("/tmp/k2rdr-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(root.join("sock")).unwrap();
    let srv: &'static Server = Box::leak(Box::new(Server { root }));
    let _stop = Stop(srv);
    run(&format!("{}/initdb", bin()), &["-D", &srv.data(), "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"], &[], None)
        .expect("initdb");
    let opts = format!("-p {PORT} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256", srv.sock());
    run(&format!("{}/pg_ctl", bin()), &["-D", &srv.data(), "-o", &opts, "-l", &format!("{}.log", srv.data()), "-w", "start"], &[], None).expect("pg_ctl start");

    let _home = crate::test_support::TempHome::new();
    let _cards = k2_core::test_env::EnvVar::set("K2_PERMISSION_CARDS", "1");
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    let owner_id = format!("ws-rdr-own-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let ws_dir = std::env::temp_dir().join(format!("k2-rdr-ws-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(ws_dir.join(".k2/db/migrations")).unwrap();
    std::fs::write(
        ws_dir.join(".k2/db/migrations/0001_notes.sql"),
        "CREATE TABLE notes (id serial PRIMARY KEY, body text NOT NULL);\nINSERT INTO notes (body) VALUES ('hello');\n",
    )
    .unwrap();
    let ws_path = ws_dir.to_string_lossy().into_owned();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![owner_id, ws_path]).unwrap();
        for t in ["sql_server", "sql_clusters", "sql_databases", "sql_grants"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', ?1, 'localhost', 1)", [major as i64])
            .unwrap();
        let mut c = crate::sql::cluster::Cluster::layout(major, PORT, "created");
        c.state = "online".into();
        crate::sql::cluster::record_in(&conn, &c, true).unwrap();
    }
    let reader = seed("rdrread");
    k2_core::workspace::settings::set_agents_can_request_permissions(&reader.id, true).expect("cards switch");
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let mut fails = Vec::new();

    // The owner's two databases, each with notes(id serial, body) and one row.
    for name in ["rdr_main", "rdr_other"] {
        let made = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::create_database(srv, &secrets, &owner_id, 10, None, Some(name)));
        check(&format!("create {name}"), made.is_ok(), format!("{made:?}"), &mut fails);
        let migrated = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::migrate_db(srv, &secrets, &owner_id, Some(name), &ws_path, None));
        check(&format!("migrate {name}"), migrated.is_ok(), format!("{migrated:?}"), &mut fails);
    }
    assert!(fails.is_empty(), "set-up failed:\n{}", fails.join("\n"));

    // The reader asks; the owner allows it on the card.
    let r = crate::sql::routes::with_test_ops(srv, || handle_request_read(&agent(&reader), true, json!({ "db": "rdr_main", "why": "verify" }).to_string().as_bytes()));
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    check("the ask files a card", r.status == "200 OK" && v["status"] == "pending", &r.body, &mut fails);
    assert!(fails.is_empty(), "no card:\n{}", fails.join("\n"));
    let (s, a) = crate::sql::routes::with_test_ops(srv, || answer_as("owner", &reader, &card_of(&v), json!({ "answer": "Allow" })));
    check("the owner's Allow is approved", s == "200 OK" && a["action"]["result"]["outcome"] == "approved", &a, &mut fails);

    // The reader's DSN, as `k2 db dsn --db rdr_main` would mint it.
    let dsn = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::dsn_for_db(srv, &secrets, &reader.id, 10, "rdr_main"));
    let target = dsn.as_ref().ok().and_then(|d| d["dsn"].as_str()).and_then(crate::apps::db_probe::parse_url);
    check("db dsn --db gives the reader a login", target.is_some(), format!("{dsn:?}"), &mut fails);
    let t = target.expect("a dsn");
    check("it is the reader's own login", t.user == crate::sql::ops::default_agent_role(&reader.id), &t.user, &mut fails);
    let attrs = srv.su("postgres", &format!("select rolsuper, rolcreatedb, rolcreaterole, rolreplication, rolbypassrls from pg_roles where rolname = '{}'", t.user));
    check("no superuser, createdb, createrole, replication or bypassrls", attrs == "f|f|f|f|f", &attrs, &mut fails);

    let read = srv.as_role(&t.user, &t.password, "rdr_main", "select body from notes");
    check("it reads the table", read.as_deref() == Ok("hello"), format!("{read:?}"), &mut fails);
    for (what, sql) in [
        ("INSERT", "insert into notes (body) values ('x')"),
        ("UPDATE", "update notes set body = 'y'"),
        ("DELETE", "delete from notes"),
        ("TRUNCATE", "truncate notes"),
        ("a sequence bump", "select nextval('notes_id_seq')"),
        ("CREATE TABLE", "create table public.mine (x int)"),
        ("CREATE SCHEMA", "create schema mine"),
        ("ALTER TABLE", "alter table notes add column z int"),
        ("CREATE INDEX", "create index on notes (body)"),
        ("DROP TABLE", "drop table notes"),
    ] {
        let r = srv.as_role(&t.user, &t.password, "rdr_main", sql);
        check(&format!("{what} is refused"), denied(&r), format!("{r:?}"), &mut fails);
    }
    let rows = srv.su("rdr_main", "select count(*) || ':' || string_agg(body, ',') from notes");
    check("nothing it tried changed a row", rows == "1:hello", &rows, &mut fails);
    let cols = srv.su("rdr_main", "select count(*) from information_schema.columns where table_name = 'notes'");
    check("nor the table", cols == "2", &cols, &mut fails);

    // K2's own tables are never the grantee's: not the documents, not the history.
    for tbl in ["_k2_store", "_k2_migrations"] {
        let r = srv.as_role(&t.user, &t.password, "rdr_main", &format!("select count(*) from public.{tbl}"));
        check(&format!("the reader can't SELECT {tbl}"), denied(&r), format!("{r:?}"), &mut fails);
    }
    // The owner's own agent role keeps what K2 uses (its documents; the
    // history to read) and can't write the migration history.
    let own = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::dsn_for_db(srv, &secrets, &owner_id, 10, "rdr_main"));
    let a = own.as_ref().ok().and_then(|d| d["dsn"].as_str()).and_then(crate::apps::db_probe::parse_url).expect("the owner's agent dsn");
    check("the owner's login is the database's agent role", a.user == "rdr_main_agent", &a.user, &mut fails);
    for (what, sql) in [
        ("INSERT", "insert into public._k2_migrations (version) values ('9999_forged')"),
        ("UPDATE", "update public._k2_migrations set checksum = 'forged'"),
        ("DELETE", "delete from public._k2_migrations"),
        ("TRUNCATE", "truncate public._k2_migrations"),
    ] {
        let r = srv.as_role(&a.user, &a.password, "rdr_main", sql);
        check(&format!("the agent role can't {what} _k2_migrations"), denied(&r), format!("{r:?}"), &mut fails);
    }
    let hist = srv.as_role(&a.user, &a.password, "rdr_main", "select count(*) from public._k2_migrations");
    check("it still reads the history (SELECT)", hist.as_deref() == Ok("1"), format!("{hist:?}"), &mut fails);
    let doc = srv.as_role(&a.user, &a.password, "rdr_main", "insert into public._k2_store (collection, id, doc) values ('c', 'i', '{}') returning id");
    check("and writes its documents in _k2_store, as before", doc.as_deref() == Ok("i"), format!("{doc:?}"), &mut fails);
    let hist_rows = srv.su("rdr_main", "select count(*) from public._k2_migrations");
    check("the history is unchanged", hist_rows == "1", &hist_rows, &mut fails);

    // Exactly the database the card named.
    let other = srv.as_role(&t.user, &t.password, "rdr_other", "select body from notes");
    check("the owner's other database stays closed", denied(&other), format!("{other:?}"), &mut fails);
    let grants: i64 = {
        let h = k2_core::db::shared();
        let c = h.lock();
        c.query_row("SELECT COUNT(*) FROM sql_grants WHERE project_id = ?1 AND level = 'read' AND can_manage = 0", [&reader.id], |r| r.get(0)).unwrap()
    };
    check("one read grant, no manage", grants == 1, grants, &mut fails);

    // k2 db revoke takes it away again: no privilege left on the public
    // tables (PUBLIC keeps CONNECT on every database, so the grants are what
    // stops it).
    let revoked = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::revoke_access(srv, None, "rdr_main", &reader.id));
    check("db revoke answers", revoked.is_ok(), format!("{revoked:?}"), &mut fails);
    let left = srv.su("rdr_main", &format!("select has_table_privilege('{}', 'public.notes', 'SELECT')", t.user));
    check("after revoke it holds no SELECT on the table", left == "f", &left, &mut fails);
    let after = srv.as_role(&t.user, &t.password, "rdr_main", "select body from notes");
    check("and can't read it", after.is_err(), format!("{after:?}"), &mut fails);

    let _ = std::fs::remove_dir_all(&ws_dir);
    assert!(fails.is_empty(), "{} check(s) failed:\n{}", fails.len(), fails.join("\n"));
}
