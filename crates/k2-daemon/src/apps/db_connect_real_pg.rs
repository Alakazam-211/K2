//! `k2 app db connect` against a real Postgres (prd-app-database-integrations-v1
//! S2's headless e2e). `#[ignore]`: it runs only where REALPG_CONNECT=1 is
//! set, with a Postgres server's binaries (`REALPG_BIN`: `initdb`, `pg_ctl`,
//! `postgres`, `psql`; default `/usr/lib/postgresql/17/bin`; a compute job
//! may unpack a relocatable build there), as an ordinary user. `node` on
//! PATH (or `REALPG_NODE`) also runs the in-wall JavaScript check
//! (`db_probe::PROBE_JS`) through the door; without it that part is skipped
//! and says so.
//!
//! It makes its own throwaway server under `/tmp` on port 5445 (K2's band,
//! never the box's own), records it as the box's cluster for its major in
//! the test's database, and drives the real App routes
//! (`routes::handle`): the route handlers and K2's SQL run unchanged; only
//! `SystemOps` points at the throwaway server (the root helper's superuser
//! psql over its socket, the daemon's psql over TCP), and "inside the App"
//! is the App's vaulted URL in its in-wall form, sent to the App's real
//! door listener. Every check prints PASS/FAIL; the test fails if any did.
//!
//! Run: `REALPG_CONNECT=1 cargo test -p k2-daemon --lib real_pg_connect -- --ignored --nocapture --test-threads=1`
//! with every K2_* variable cleared.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use k2_core::deploy::store;
use serde_json::{json, Value as J};

use super::super::db_door::test_seams::Seam;
use super::super::db_probe;
use super::*;
use crate::sql::sysops::{RealSystemOps, SystemOps};

const PORT: u16 = 5445;
const APP: &str = "conn-real";

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into())
}

fn run(prog: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<String, String> {
    let mut e: Vec<(&str, &str)> = vec![("PSQLRC", "/dev/null"), ("LC_ALL", "C")];
    e.extend_from_slice(env);
    let out = RealSystemOps.run_cmd(prog, args, &e, stdin)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// The throwaway server, and the `SystemOps` that points K2 at it.
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
    /// Superuser SQL over the socket, never through the door.
    fn su(&self, db: &str, sql: &str) -> String {
        let p = PORT.to_string();
        run(&format!("{}/psql", bin()), &["-X", "-tA", "-h", &self.sock(), "-p", &p, "-U", "postgres", "-d", db, "-v", "ON_ERROR_STOP=1", "-c", sql], &[], None)
            .unwrap_or_else(|e| panic!("superuser SQL failed: {e}\n{sql}"))
    }
    /// One statement as `user` over TCP on `port` (the door's, or the
    /// server's own).
    fn as_role(&self, port: u16, user: &str, password: &str, db: &str, sql: &str) -> Result<String, String> {
        let p = port.to_string();
        run(
            &format!("{}/psql", bin()),
            &["-X", "-q", "-tA", "-h", "127.0.0.1", "-p", &p, "-U", user, "-d", db, "-v", "ON_ERROR_STOP=1", "-c", sql],
            &[("PGPASSWORD", password), ("PGSSLMODE", "disable"), ("PGCONNECT_TIMEOUT", "5")],
            None,
        )
    }
}

/// Stops the (leaked, `'static` for the route seam) server however the
/// test ends.
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
        Err(format!("systemctl {} is not part of the e2e", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        "active".into()
    }
    /// The root helper's `psql [--cluster M] -d <db> …`: superuser, SQL
    /// on stdin, over the throwaway server's socket.
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        if args.first().copied() != Some("psql") {
            return Err(format!("helper {} is not part of the e2e", args.join(" ")));
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
    /// The daemon's own client runs (`psql`, `pg_dump`) with this
    /// server's binaries.
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

fn node() -> Option<String> {
    if let Ok(n) = std::env::var("REALPG_NODE") {
        return Some(n);
    }
    k2_core::bg_proc::command("node").arg("--version").output().ok().filter(|o| o.status.success()).map(|_| "node".to_string())
}

/// [`db_probe::PROBE_JS`] under node, the URL in its environment as `name`
/// (as in the App's wall), pointed at the door's host port.
fn js_probe(node: &str, name: &str, url: &str) -> Result<db_probe::Probe, String> {
    let out = k2_core::bg_proc::command(node)
        .args(["-e", db_probe::PROBE_JS, name])
        .env_clear()
        // The lookup of `node` reads the child's PATH once env is changed.
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env(name, url)
        .output()
        .map_err(|e| format!("node: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    db_probe::parse_marked(&text).ok_or(format!("no K2PROBE line: {text}"))
}

fn call(ops: &'static Server, path: &str, who: &Who, body: J) -> (u16, J) {
    let r = crate::sql::routes::with_test_ops(ops, || super::super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new()));
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or(J::Null))
}

#[test]
#[ignore = "runs only with REALPG_CONNECT=1 and Postgres binaries (a Linux compute job)"]
fn real_pg_connect_end_to_end() {
    assert_eq!(std::env::var("REALPG_CONNECT").as_deref(), Ok("1"), "REALPG_CONNECT=1 marks the throwaway job");
    assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres (REALPG_BIN)", bin());
    let version = run(&format!("{}/postgres", bin()), &["--version"], &[], None).expect("postgres --version");
    let major: u32 = version.split_whitespace().last().and_then(|v| v.split('.').next()).and_then(|m| m.parse().ok()).expect("a major");
    eprintln!("server: {version} (major {major})");
    let root = PathBuf::from(format!("/tmp/k2rpc-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(root.join("sock")).unwrap();
    let srv: &'static Server = Box::leak(Box::new(Server { root }));
    let _stop = Stop(srv);
    run(&format!("{}/initdb", bin()), &["-D", &srv.data(), "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"], &[], None)
        .expect("initdb");
    let opts = format!("-p {PORT} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256", srv.sock());
    run(&format!("{}/pg_ctl", bin()), &["-D", &srv.data(), "-o", &opts, "-l", &format!("{}.log", srv.data()), "-w", "start"], &[], None).expect("pg_ctl start");

    // K2's records: a running SQL server whose `major` cluster is this
    // one; workspace A (a real folder for migrations) and its container App.
    let _home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    crate::vault::ensure_test_vault();
    super::super::alerts::set_sink(Some(Box::new(|_ws: &str, _text: &str| {})));
    let ws = "ws-a-conn-real".to_string();
    let ws_dir = std::env::temp_dir().join(format!("k2-rpc-ws-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(ws_dir.join(".k2/db/migrations")).unwrap();
    std::fs::write(ws_dir.join(".k2/db/migrations/0001_notes.sql"), "CREATE TABLE notes (id serial PRIMARY KEY, body text NOT NULL);\n").unwrap();
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
            "/tmp/k2-rpc-app",
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
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
             VALUES ('seed-conn-real', ?1, 'cmd', '{}', '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
            [APP],
        )
        .unwrap();
        conn.execute("UPDATE apps SET live_seed = 'seed-conn-real', live_deployment = 'd-0001', state = 'running' WHERE app_id = ?1", [APP]).unwrap();
        for t in ["sql_server", "sql_clusters", "sql_databases", "sql_grants"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute(
            "INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', ?1, 'localhost', 1)",
            [major as i64],
        )
        .unwrap();
        let mut c = crate::sql::cluster::Cluster::layout(major, PORT, "created");
        c.role = "k2".into();
        c.state = "online".into();
        crate::sql::cluster::record_in(&conn, &c, true).unwrap();
    }
    // The App's real door, admitting from the test's records; "inside the
    // App" is its vaulted URL in its in-wall form, through the door.
    let handle = db.handle();
    super::super::db_door::test_seams::set(
        APP,
        Some(Seam {
            admit: Arc::new(move || {
                let c = handle.lock();
                super::super::db_door::admitted_in(&c, APP)
            }),
            upstream: HashMap::new(),
        }),
    );
    super::super::db_door::test_seams::set_engine_ready(true);
    super::super::egress_proxy::ensure(APP, 0).expect("the App's listener");
    let door = super::super::egress_proxy::listening(APP).expect("its port");
    let in_wall = move |as_name: &str| -> Option<String> {
        let row = super::super::reach::app_row(APP)?;
        let url = url_for_app(&row, as_name)?;
        Some(super::super::pg_choice::in_wall_dsn(&url)?.replace("127.0.0.1:5432", &format!("127.0.0.1:{door}")))
    };
    test_seams::set_in_wall(Some(Box::new(move |_app: &str, as_name: &str| match in_wall(as_name) {
        Some(u) => db_probe::probe_url(&u, Duration::from_secs(10)),
        None => db_probe::Probe::fail("no_url", format!("{as_name} is not in the App environment")),
    })));
    let mut fails = Vec::new();

    // 1. Connect a new database on this major: PASS from inside, SCRAM end to end.
    let (st, v) = call(srv, CONNECT, &Who::Owner, json!({ "app": APP, "new": true, "major": major }));
    check("connect --new answers 200", st == 200, &v, &mut fails);
    check("the check PASSes from inside the App", v["check"]["ok"] == true, &v["check"], &mut fails);
    check(
        "it reports the server's version",
        v["check"]["version"].as_str().is_some_and(|s| s.starts_with(&format!("PostgreSQL {major}"))),
        &v["check"],
        &mut fails,
    );
    let dbname = v["db"].as_str().unwrap_or_default().to_string();
    let login = v["roles"][0].as_str().unwrap_or_default().to_string();
    let printed = v.to_string();
    check("no URL or password printed", !printed.contains("postgres://") && !printed.contains("PASSWORD"), &printed, &mut fails);
    // 2. The App's own login, as Postgres has it.
    let attrs = srv.su("postgres", &format!(
        "select rolcanlogin, rolsuper, rolcreatedb, rolcreaterole, rolreplication, rolbypassrls, rolconnlimit from pg_roles where rolname = '{login}'"
    ));
    check("its own login: LOGIN, no superuser/createdb/createrole/replication/bypassrls, 20 connections", attrs == "t|f|f|f|f|f|20", &attrs, &mut fails);
    let member = srv.su("postgres", &format!("select pg_has_role('{login}', '{dbname}_agent', 'USAGE')"));
    check("it holds the workspace's role through its group", member == "t", &member, &mut fails);
    // 3. A migration (as the migrator), then the App's login reads and writes it through the door.
    let migrated = crate::sql::routes::with_test_ops(srv, || {
        crate::sql::ops::migrate_db(srv, &crate::sql::secrets::FileSecretStore::default(), &ws, Some(&dbname), &ws_path, None)
    });
    check("k2 db migrate applies 0001_notes.sql", migrated.as_ref().is_ok_and(|m| m["applied"].to_string().contains("0001")), format!("{migrated:?}"), &mut fails);
    let row = super::super::reach::app_row(APP).expect("app");
    let url = url_for_app(&row, "DATABASE_URL").expect("the App's URL in the vault");
    let pw = db_probe::parse_url(&url).expect("a URL").password;
    let wrote = srv.as_role(door, &login, &pw, &dbname, "insert into notes (body) values ('from the app') returning id");
    check("the App's login inserts through its door", wrote.as_ref().is_ok_and(|s| s.trim() == "1"), format!("{wrote:?}"), &mut fails);
    let read = srv.as_role(door, &login, &pw, &dbname, "select body from notes");
    check("and reads it back", read.as_deref() == Ok("from the app"), format!("{read:?}"), &mut fails);
    let ddl = srv.as_role(door, &login, &pw, &dbname, "create table not_mine (x int)");
    check("but can't create tables (DML only)", ddl.is_err(), format!("{ddl:?}"), &mut fails);
    // 4. The migrator's URL works through the door too.
    let (_, c) = call(srv, CHECK, &Who::Owner, json!({ "app": APP }));
    check("k2 app db check PASSes", c["check"]["ok"] == true, &c, &mut fails);
    match in_wall("DATABASE_MIGRATE_URL") {
        Some(m) => {
            let p = db_probe::probe_url(&m, Duration::from_secs(10));
            check("DATABASE_MIGRATE_URL logs in through the door", p.ok, format!("{p:?}"), &mut fails);
        }
        None => check("DATABASE_MIGRATE_URL is granted", false, "missing", &mut fails),
    }
    // 5. The in-wall JavaScript check (what runs inside a real container).
    match node() {
        Some(n) => {
            let u = in_wall("DATABASE_URL").unwrap_or_default();
            let p = js_probe(&n, "DATABASE_URL", &u);
            check("PROBE_JS (node) PASSes through the door with SCRAM", p.as_ref().is_ok_and(|p| p.ok && p.version.is_some()), format!("{p:?}"), &mut fails);
            let bad = js_probe(&n, "DATABASE_URL", &u.replace(&pw, "wrong-password"));
            check("PROBE_JS reports auth_failed for a wrong password", bad.as_ref().is_ok_and(|p| p.step.as_deref() == Some("auth_failed")), format!("{bad:?}"), &mut fails);
            // The path only (the login's name starts with the database's).
            let gone = js_probe(&n, "DATABASE_URL", &u.replace(&format!("/{dbname}?"), "/no_such_db?"));
            check("PROBE_JS reports door_refused for another database", gone.as_ref().is_ok_and(|p| p.step.as_deref() == Some("door_refused")), format!("{gone:?}"), &mut fails);
        }
        None => eprintln!("SKIP PROBE_JS: no node on this machine (REALPG_NODE)"),
    }
    // 6. Disconnect: the login stops, the door refuses it, the database stays.
    let (st, d) = call(srv, DISCONNECT, &Who::Owner, json!({ "app": APP }));
    check("disconnect answers 200", st == 200 && d["ok"] == true, &d, &mut fails);
    let can = srv.su("postgres", &format!("select rolcanlogin from pg_roles where rolname = '{login}'"));
    check("its login is NOLOGIN", can == "f", &can, &mut fails);
    let refused = srv.as_role(door, &login, &pw, &dbname, "select 1");
    check("the door refuses it", refused.as_ref().is_err_and(|e| e.contains("db_door_role_refused")), format!("{refused:?}"), &mut fails);
    let direct = srv.as_role(PORT, &login, &pw, &dbname, "select 1");
    check("and so does Postgres", direct.as_ref().is_err_and(|e| e.contains("not permitted to log in")), format!("{direct:?}"), &mut fails);
    let kept = srv.su("postgres", &format!("select count(*) from pg_database where datname = '{dbname}'"));
    check("the database is kept", kept == "1", &kept, &mut fails);
    // 7. Connect again: back on, a fresh password, PASS.
    let (st, again) = call(srv, CONNECT, &Who::Owner, json!({ "app": APP, "db": dbname }));
    check("connect --db again PASSes", st == 200 && again["check"]["ok"] == true, &again, &mut fails);

    test_seams::set_in_wall(None);
    super::super::egress_proxy::stop(APP);
    super::super::db_door::test_seams::set(APP, None);
    let _ = std::fs::remove_dir_all(&ws_dir);
    assert!(fails.is_empty(), "{} check(s) failed:\n{}", fails.len(), fails.join("\n"));
}
