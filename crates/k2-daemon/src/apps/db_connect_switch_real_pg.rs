//! `k2 db cluster switch` against two REAL Postgres clusters (lane E
//! addendum): an App's database on a named cluster, a restored copy of it
//! beside it (a `pg_basebackup` taken at a moment, its logins locked as
//! `pg restore-to` locks them), more rows written after the moment; then
//! the switch, the App's `DATABASE_URL` reaching the restored rows, the
//! rollback, and the old rows back. The route handlers and K2's SQL run
//! unchanged; only `SystemOps` points the root helper's psql at the two
//! servers (`--cluster 17-<name>` picks the port).
//!
//! `#[ignore]`: it runs only where REALPG_SWITCH=1 is set, with a Postgres
//! server's binaries (`REALPG_BIN`, default `/usr/lib/postgresql/17/bin`),
//! as an ordinary user:
//! `REALPG_SWITCH=1 cargo test -p k2-daemon --lib real_pg_switch -- --ignored --nocapture --test-threads=1`
//! with every K2_* variable cleared. Every check prints PASS/FAIL; the test
//! fails if any did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::deploy::store;
use serde_json::{json, Value as J};

use super::super::db_probe;
use super::*;
use crate::sql::sysops::{RealSystemOps, SystemOps};

const SRC_PORT: u16 = 5446;
const OLD_PORT: u16 = 5447;
const APP: &str = "switch-real";

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into())
}

fn run(prog: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<String, String> {
    let mut e: Vec<(&str, &str)> = vec![("PSQLRC", "/dev/null"), ("LC_ALL", "C")];
    e.extend_from_slice(env);
    let out = RealSystemOps.run_cmd(prog, args, &e, stdin)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// The two servers, and the `SystemOps` that points K2 at them.
struct Servers {
    root: PathBuf,
    major: u32,
}

impl Servers {
    fn sock(&self) -> String {
        self.root.join("sock").to_string_lossy().into_owned()
    }
    fn data(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().into_owned()
    }
    fn port_of(&self, cluster_token: &str) -> Option<u16> {
        match cluster_token.split_once('-').map(|(_, n)| n) {
            Some("fbsrc") => Some(SRC_PORT),
            Some("fbold") => Some(OLD_PORT),
            _ => None,
        }
    }
    fn start(&self, name: &str, port: u16) {
        let opts = format!("-p {port} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256", self.sock());
        run(&format!("{}/pg_ctl", bin()), &["-D", &self.data(name), "-o", &opts, "-l", &format!("{}.log", self.data(name)), "-w", "start"], &[], None)
            .unwrap_or_else(|e| panic!("start {name}: {e}"));
    }
    /// Superuser SQL over the socket.
    fn su(&self, port: u16, db: &str, sql: &str) -> String {
        let p = port.to_string();
        run(&format!("{}/psql", bin()), &["-X", "-tA", "-h", &self.sock(), "-p", &p, "-U", "postgres", "-d", db, "-v", "ON_ERROR_STOP=1", "-c", sql], &[], None)
            .unwrap_or_else(|e| panic!("superuser SQL on {port} failed: {e}\n{sql}"))
    }
    /// Superuser SQL on stdin (psql meta-commands such as `\gexec` work).
    fn su_stdin(&self, port: u16, sql: &str) -> String {
        let p = port.to_string();
        run(&format!("{}/psql", bin()), &["-X", "-tA", "-h", &self.sock(), "-p", &p, "-U", "postgres", "-d", "postgres", "-v", "ON_ERROR_STOP=1"], &[], Some(sql.as_bytes()))
            .unwrap_or_else(|e| panic!("superuser SQL on {port} failed: {e}\n{sql}"))
    }
    /// SQL as a login over TCP, the way the App connects with its URL.
    fn with_url(&self, url: &str, sql: &str) -> Result<String, String> {
        let u = db_probe::parse_url(url).ok_or("not a URL")?;
        run(
            &format!("{}/psql", bin()),
            &["-X", "-q", "-tA", "-h", "127.0.0.1", "-p", &u.port.to_string(), "-U", &u.user, "-d", &u.database, "-v", "ON_ERROR_STOP=1", "-c", sql],
            &[("PGPASSWORD", &u.password), ("PGSSLMODE", "disable"), ("PGCONNECT_TIMEOUT", "5")],
            None,
        )
    }
}

struct Stop(&'static Servers);
impl Drop for Stop {
    fn drop(&mut self) {
        for n in ["fbsrc", "fbold"] {
            let _ = run(&format!("{}/pg_ctl", bin()), &["-D", &self.0.data(n), "-m", "immediate", "-w", "stop"], &[], None);
        }
        let _ = std::fs::remove_dir_all(&self.0.root);
    }
}

impl SystemOps for Servers {
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
        Err(format!("systemctl {} is not part of the e2e", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        "active".into()
    }
    fn helper_pgnamed(&self) -> Result<(), String> {
        Ok(())
    }
    fn helper_pgarchive(&self) -> Result<(), String> {
        Ok(())
    }
    fn helper_pgrestore(&self) -> Result<(), String> {
        Ok(())
    }
    /// The root helper: `psql --cluster <M>-<name>` as the superuser on
    /// that server's socket; `wal-archive` answers as the helper does (the
    /// archive itself is the helper drill's); anything else isn't part of
    /// this run.
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        match args.first().copied() {
            Some("psql") => {
                let i = args.iter().position(|a| *a == "--cluster").ok_or("psql without --cluster")?;
                let port = self.port_of(args[i + 1]).ok_or_else(|| format!("no server for {}", args[i + 1]))?.to_string();
                let rest: Vec<&str> = args[1..].iter().enumerate().filter(|(j, _)| *j + 1 != i && *j + 1 != i + 1).map(|(_, a)| *a).collect();
                let sock = self.sock();
                let mut full: Vec<&str> = vec!["-X", "-h", &sock, "-p", &port, "-U", "postgres"];
                full.extend(rest);
                run(&format!("{}/psql", bin()), &full, &[], stdin)
            }
            Some("wal-archive") => Ok(json!({"major": self.major, "name": args[1].split_once('-').map(|(_, n)| n).unwrap_or(""), "port": OLD_PORT,
                "k2Named": true, "walArchive": args.get(2) == Some(&"on"), "changed": true, "restarted": false})
            .to_string()),
            _ => Err(format!("helper {} is not part of the e2e", args.join(" "))),
        }
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

fn named_record(major: u32, name: &str, port: u16, ws: &str, wal: bool) {
    let mut c = crate::sql::cluster::Cluster::layout(major, port, "created");
    c.name = name.into();
    c.unit = format!("postgresql@{major}-{name}.service");
    c.data_dir = format!("/var/lib/postgresql/{major}/{name}");
    c.state = "online".into();
    c.owner_project = Some(ws.into());
    c.wal_archive = wal;
    let h = k2_core::db::shared();
    crate::sql::cluster::record_in(&h.lock(), &c, true).unwrap();
}

#[test]
#[ignore = "runs only with REALPG_SWITCH=1 and Postgres binaries (a Linux compute job)"]
fn real_pg_switch_and_rollback() {
    assert_eq!(std::env::var("REALPG_SWITCH").as_deref(), Ok("1"), "REALPG_SWITCH=1 marks the throwaway job");
    assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres (REALPG_BIN)", bin());
    let version = run(&format!("{}/postgres", bin()), &["--version"], &[], None).expect("postgres --version");
    let major: u32 = version.split_whitespace().last().and_then(|v| v.split('.').next()).and_then(|m| m.parse().ok()).expect("a major");
    let root = PathBuf::from(format!("/tmp/k2sw-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(root.join("sock")).unwrap();
    let srv: &'static Servers = Box::leak(Box::new(Servers { root, major }));
    let _stop = Stop(srv);
    run(&format!("{}/initdb", bin()), &["-D", &srv.data("fbsrc"), "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"], &[], None)
        .expect("initdb");
    srv.start("fbsrc", SRC_PORT);

    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    crate::vault::ensure_test_vault();
    super::super::alerts::set_sink(Some(Box::new(|_ws: &str, _text: &str| {})));
    let ws = "ws-switch-real".to_string();
    let ws_dir = std::env::temp_dir().join(format!("k2-sw-ws-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(k2_core::workspace_dot_dir(&ws_dir).join("db/migrations")).unwrap();
    std::fs::write(k2_core::workspace_dot_dir(&ws_dir).join("db/migrations/0001_notes.sql"), "CREATE TABLE notes (id int PRIMARY KEY, body text NOT NULL);\n").unwrap();
    let ws_path = ws_dir.to_string_lossy().into_owned();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![ws, ws_path]).unwrap();
        let svc = k2_core::published_services::insert(
            &conn,
            &ws,
            APP,
            k2_core::published_services::CMD_APP_SENTINEL,
            "/tmp/k2-sw-app",
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
                app_id: APP,
                service_id: &svc.id,
                seed_id: None,
                owner_workspace: &ws,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
        for t in ["sql_server", "sql_clusters", "sql_databases", "sql_grants", "pg_cluster_pitr", "pg_db_switches"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', ?1, 'localhost', 1)", [major as i64])
            .unwrap();
    }
    named_record(major, "fbsrc", SRC_PORT, &ws, true);
    let mut fails = Vec::new();
    let call = |path: &str, body: J| -> (u16, J) {
        let r = crate::sql::routes::with_test_ops(srv, || super::super::routes::handle(path, Who::Owner, body.to_string().as_bytes(), &HashMap::new()));
        (r.status.split_whitespace().next().unwrap().parse().unwrap(), serde_json::from_str(&r.body).unwrap_or(J::Null))
    };
    let app_url = || -> String {
        let row = super::super::reach::app_row(APP).expect("app");
        url_for_app(&row, "DATABASE_URL").expect("the App's URL in the vault")
    };
    let port_of_url = |u: &str| db_probe::parse_url(u).map(|p| p.port).unwrap_or(0);

    // 1. The App's database on the named cluster, its rows 1-3.
    let (st, v) = call(CONNECT, json!({"app": APP, "new": true, "cluster": "fbsrc"}));
    check("connect --new --cluster fbsrc answers 200", st == 200, &v, &mut fails);
    let dbname = v["db"].as_str().unwrap_or_default().to_string();
    let migrated = crate::sql::routes::with_test_ops(srv, || {
        crate::sql::ops::migrate_db(srv, &crate::sql::secrets::FileSecretStore::default(), &ws, Some(&dbname), &ws_path, None)
    });
    check("the migration applies", migrated.is_ok(), format!("{migrated:?}"), &mut fails);
    let url0 = app_url();
    check("the App's URL is on fbsrc", port_of_url(&url0) == SRC_PORT, &url0.split('@').nth(1).unwrap_or(""), &mut fails);
    let w = srv.with_url(&url0, "insert into notes values (1, 'a'), (2, 'b'), (3, 'c'); select count(*) from notes");
    check("the App writes rows 1-3", w.as_deref() == Ok("3"), format!("{w:?}"), &mut fails);
    // Another workspace's database on the same cluster, and a workspace's
    // login that may read it at the moment (re-review N2).
    srv.su(SRC_PORT, "postgres", "CREATE DATABASE other_db");
    srv.su(SRC_PORT, "other_db", "CREATE TABLE secrets_t (v text); INSERT INTO secrets_t VALUES ('theirs')");
    srv.su(SRC_PORT, "postgres", "CREATE ROLE ws_gone_agent LOGIN PASSWORD 'gone-pass'");
    srv.su(SRC_PORT, "other_db", "GRANT SELECT ON secrets_t TO ws_gone_agent");
    let gone_url = format!("postgres://ws_gone_agent:gone-pass@127.0.0.1:{SRC_PORT}/other_db");
    check("the workspace reads other_db at the moment", srv.with_url(&gone_url, "select v from secrets_t").as_deref() == Ok("theirs"), "", &mut fails);

    // 2. The restored copy: a base backup at the moment, beside it, its
    // logins locked as `pg restore-to` locks them.
    run(
        &format!("{}/pg_basebackup", bin()),
        &["-h", &srv.sock(), "-p", &SRC_PORT.to_string(), "-U", "postgres", "-D", &srv.data("fbold"), "-Fp", "-X", "stream"],
        &[],
        None,
    )
    .expect("pg_basebackup");
    let moment = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    srv.start("fbold", OLD_PORT);
    // What `pg restore-to` runs on a serving restore, word for word.
    let locked = srv.su_stdin(OLD_PORT, k2_root_proto::pg_restore::LOCK_LOGINS_SQL);
    check("the restored copy's logins are locked", srv.su(OLD_PORT, "postgres", "select count(*) from pg_roles where rolcanlogin and not rolsuper") == "0", &locked, &mut fails);
    named_record(major, "fbold", OLD_PORT, &ws, false);
    k2_core::db::shared()
        .lock()
        .execute("INSERT INTO pg_cluster_pitr (major, name, restored_from, restored_to) VALUES (?1, 'fbold', 'fbsrc', ?2)", rusqlite::params![major as i64, moment])
        .unwrap();
    // Rows after the moment, on the source only.
    let w = srv.with_url(&url0, "insert into notes values (4, 'd'), (5, 'e'); select count(*) from notes");
    check("rows 4-5 land on fbsrc after the moment", w.as_deref() == Ok("5"), format!("{w:?}"), &mut fails);
    // After the moment the workspace's access goes (K2 holds no grant for it).
    srv.su(SRC_PORT, "postgres", "ALTER ROLE ws_gone_agent NOLOGIN");
    check("other_db is closed on the restored copy", srv.su(OLD_PORT, "postgres", "select datallowconn::text from pg_database where datname = 'other_db'") == "false", "", &mut fails);

    let secrets = crate::sql::secrets::FileSecretStore::default();
    let switch = |to: &str, yes: bool| crate::sql::routes::with_test_ops(srv, || crate::sql::switch::switch(srv, &secrets, &dbname, to, yes, "owner"));

    // 3. The preview: the moment, and nothing moves.
    let p = switch("fbold", false);
    check("the preview asks for a confirm and names the moment", p.as_ref().is_ok_and(|v| v["needsConfirm"] == true && v["kind"] == "restore"), format!("{p:?}"), &mut fails);
    check("nothing moved before the confirm", port_of_url(&app_url()) == SRC_PORT, "", &mut fails);

    // 4. The switch.
    let s = switch("fbold", true);
    check("the switch answers ok", s.as_ref().is_ok_and(|v| v["ok"] == true && v["switched"] == true), format!("{s:?}"), &mut fails);
    check("one active row, now on fbold", crate::sql::switch::active_rows_named(&dbname) == 1 && crate::sql::cluster::for_db(&dbname).map(|c| c.name).as_deref() == Ok("fbold"), "", &mut fails);
    let url1 = app_url();
    check("the App's DATABASE_URL now names fbold's port", port_of_url(&url1) == OLD_PORT, &url1.split('@').nth(1).unwrap_or(""), &mut fails);
    let r = srv.with_url(&url1, "select string_agg(id::text, ',' order by id) from notes");
    check("the App's DATABASE_URL reaches the restored rows (1-3)", r.as_deref() == Ok("1,2,3"), format!("{r:?}"), &mut fails);
    let mig = srv.su(OLD_PORT, "postgres", &format!("select rolcanlogin from pg_roles where rolname = '{dbname}_migrator'"));
    check("the migrator login is set again on fbold", mig == "t", &mig, &mut fails);
    let closed = srv.su(SRC_PORT, "postgres", &format!("select datallowconn from pg_database where datname = '{dbname}'"));
    check("the old copy is closed", closed == "f", &closed, &mut fails);
    let old = srv.with_url(&url0, "select 1");
    check("the old URL can't reach the old copy", old.is_err(), format!("{old:?}"), &mut fails);
    // Re-review N2: the revoked workspace reads nothing on the restored
    // cluster: its login stays locked, and other_db stays closed.
    let gone_restored = gone_url.replace(&format!(":{SRC_PORT}/"), &format!(":{OLD_PORT}/"));
    let r = srv.with_url(&gone_restored, "select v from secrets_t");
    check("a workspace revoked after the moment can't read another database on the restored cluster", r.is_err(), format!("{r:?}"), &mut fails);
    let r = srv.with_url(&gone_restored.replace("/other_db", &format!("/{dbname}")), "select 1");
    check("nor the switched database", r.is_err(), format!("{r:?}"), &mut fails);
    check("other_db is still closed after the switch", srv.su(OLD_PORT, "postgres", "select datallowconn::text from pg_database where datname = 'other_db'") == "false", "", &mut fails);

    // 5. The rollback: the old rows are back.
    let b = switch("fbsrc", true);
    check("the rollback answers ok", b.as_ref().is_ok_and(|v| v["ok"] == true && v["kind"] == "rollback"), format!("{b:?}"), &mut fails);
    check("one active row, back on fbsrc", crate::sql::switch::active_rows_named(&dbname) == 1 && crate::sql::cluster::for_db(&dbname).map(|c| c.name).as_deref() == Ok("fbsrc"), "", &mut fails);
    let url2 = app_url();
    check("the App's DATABASE_URL names fbsrc's port again", port_of_url(&url2) == SRC_PORT, "", &mut fails);
    let r = srv.with_url(&url2, "select string_agg(id::text, ',' order by id) from notes");
    check("the old rows are back (1-5)", r.as_deref() == Ok("1,2,3,4,5"), format!("{r:?}"), &mut fails);
    let closed = srv.su(OLD_PORT, "postgres", &format!("select datallowconn from pg_database where datname = '{dbname}'"));
    check("the restored copy is closed and kept", closed == "f", &closed, &mut fails);

    let _ = std::fs::remove_dir_all(&ws_dir);
    assert!(fails.is_empty(), "{} check(s) failed:\n{}", fails.len(), fails.join("\n"));
}
