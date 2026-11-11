//! Real Postgres through the container database door (S21): libpq (through
//! psycopg, one of the drivers an App uses) dials the App's door the way an
//! App's driver does from inside its wall, and SCRAM runs end to end with a
//! real server. `#[ignore]`: it runs only where REALPG_DOOR=1 is set, with
//! a Postgres server's binaries (`REALPG_BIN`: `initdb`, `pg_ctl`,
//! `postgres`; default `/usr/lib/postgresql/17/bin`) and a Python with
//! psycopg (`REALPG_PY`, default `python3`; its `PYTHONPATH` is inherited),
//! as an ordinary user. It makes its own throwaway server under `/tmp`
//! (port 5498, never the box's own), and K2's records live in the test's
//! database: the door dials that server where the records say "the 5432
//! cluster".
//!
//! Run: `REALPG_DOOR=1 cargo test -p k2-daemon --lib real_pg_door -- --ignored --nocapture`
//! with every K2_* variable cleared.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_core::bg_proc::command;
use k2_core::deploy::store;

use super::test_seams::Seam;
use super::*;

const PORT: u16 = 5498;
const APP: &str = "door-real";

/// The client: connect with a libpq conninfo (password from PGPASSWORD),
/// print READY, then one line per SQL line read: `ROW <first value>` or
/// `ERR <message>` (and exit 1). A failed connect prints `CONNECT <why>`
/// on stderr and exits 2.
const CLIENT_PY: &str = r#"
import sys
import psycopg
try:
    conn = psycopg.connect(sys.argv[1], autocommit=True)
except Exception as e:
    print("CONNECT " + str(e).replace("\n", " "), file=sys.stderr, flush=True)
    sys.exit(2)
print("READY", flush=True)
for line in sys.stdin:
    sql = line.strip()
    if not sql:
        continue
    try:
        cur = conn.execute(sql)
        row = cur.fetchone() if cur.description else None
        print("ROW " + ("" if row is None else str(row[0])), flush=True)
    except Exception as e:
        print("ERR " + str(e).replace("\n", " "), flush=True)
        sys.exit(1)
"#;

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into())
}

fn py() -> String {
    std::env::var("REALPG_PY").unwrap_or_else(|_| "python3".into())
}

fn run(prog: &str, args: &[&str]) -> Result<String, String> {
    let out = command(prog).args(args).env("LC_ALL", "C").output().map_err(|e| format!("{prog}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(text.trim().to_string())
    }
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
    fn script(&self) -> String {
        self.root.join("client.py").to_string_lossy().into_owned()
    }
    fn spawn(&self, conninfo: &str, pass: &str) -> std::process::Child {
        command(py())
            .args([&self.script(), conninfo])
            .env("PGPASSWORD", pass)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the Python client")
    }
    /// Run SQL lines on one connection: the first value of each, or why
    /// it failed (the connect's or a statement's message).
    fn client(&self, conninfo: &str, pass: &str, sqls: &[&str]) -> Result<Vec<String>, String> {
        let mut ch = self.spawn(conninfo, pass);
        let mut sin = ch.stdin.take().unwrap();
        for s in sqls {
            let _ = writeln!(sin, "{s}");
        }
        drop(sin);
        let out = ch.wait_with_output().expect("client exit");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let rows: Vec<String> = stdout.lines().filter_map(|l| l.strip_prefix("ROW ").map(str::to_string)).collect();
        if out.status.success() {
            Ok(rows)
        } else {
            Err(format!("{}{}", stdout, String::from_utf8_lossy(&out.stderr)).trim().to_string())
        }
    }
    /// Superuser SQL over the socket (trust), never through the door.
    fn su(&self, sqls: &[&str]) -> Vec<String> {
        let conn = format!("host={} port={PORT} user=postgres dbname=postgres", self.sock());
        self.client(&conn, "", sqls).unwrap_or_else(|e| panic!("superuser SQL failed: {e}\n{sqls:?}"))
    }
    /// One statement through the door, as the App's driver would.
    fn via_door(&self, door: u16, user: &str, pass: &str, db: &str, extra: &str, sql: &str) -> Result<String, String> {
        let conn = format!("host=127.0.0.1 port={door} user={user} dbname={db} connect_timeout=5 {extra}");
        self.client(&conn, pass, &[sql]).map(|r| r.into_iter().next().unwrap_or_default())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = run(&format!("{}/pg_ctl", bin()), &["-D", &self.data(), "-m", "immediate", "-w", "stop"]);
        let _ = std::fs::remove_dir_all(&self.root);
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

#[test]
#[ignore = "runs only with REALPG_DOOR=1, Postgres binaries and psycopg (a Linux compute job)"]
fn real_pg_door_end_to_end() {
    assert_eq!(std::env::var("REALPG_DOOR").as_deref(), Ok("1"), "REALPG_DOOR=1 marks the throwaway job");
    assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres (REALPG_BIN)", bin());
    let version = run(&format!("{}/postgres", bin()), &["--version"]).expect("postgres --version");
    eprintln!("server: {version}");
    // Short: a Unix socket path must stay under 108 bytes.
    let root = PathBuf::from(format!("/tmp/k2rpd-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(root.join("sock")).unwrap();
    let srv = Server { root };
    std::fs::write(srv.script(), CLIENT_PY).unwrap();
    let libpq = run(&py(), &["-c", "import psycopg; print(psycopg.__version__, psycopg.pq.version())"]).expect("psycopg (REALPG_PY, PYTHONPATH)");
    eprintln!("client: psycopg / libpq {libpq}");
    run(
        &format!("{}/initdb", bin()),
        &["-D", &srv.data(), "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"],
    )
    .expect("initdb");
    let opts = format!("-p {PORT} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256", srv.sock());
    let log = format!("{}.log", srv.data());
    run(&format!("{}/pg_ctl", bin()), &["-D", &srv.data(), "-o", &opts, "-l", &log, "-w", "start"]).expect("pg_ctl start");
    srv.su(&[
        "CREATE ROLE door_main_agent LOGIN PASSWORD 'door-pass-a'",
        "CREATE ROLE door_main_migrator LOGIN PASSWORD 'door-pass-m'",
        "CREATE ROLE other_main_agent LOGIN PASSWORD 'door-pass-o'",
        "ALTER ROLE postgres PASSWORD 'su-pass'",
        "CREATE DATABASE door_main OWNER door_main_migrator",
        "CREATE DATABASE other_main",
        "REVOKE CONNECT ON DATABASE door_main FROM PUBLIC",
        "GRANT CONNECT ON DATABASE door_main TO door_main_agent",
        "REVOKE CONNECT ON DATABASE other_main FROM PUBLIC",
        "GRANT CONNECT ON DATABASE other_main TO other_main_agent",
    ]);
    // The superuser's password works on the server itself over TCP: the
    // door, not pg_hba, is what keeps an App out.
    let direct = srv.client(&format!("host=127.0.0.1 port={PORT} user=postgres dbname=postgres"), "su-pass", &["select 1"]);
    assert_eq!(direct, Ok(vec!["1".to_string()]), "the superuser logs in directly");

    // K2's records: workspace A owns door_main, B owns other_main; the
    // App is a container App of A.
    let _home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    super::super::alerts::set_sink(Some(Box::new(|_ws: &str, _text: &str| {})));
    let (ws_a, ws_b) = ("ws-a-door-real".to_string(), "ws-b-door-real".to_string());
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for w in [&ws_a, &ws_b] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![w, format!("/tmp/k2-door-real/{w}")]).unwrap();
        }
        let svc = k2_core::published_services::insert(
            &conn,
            &ws_a,
            APP,
            k2_core::published_services::CMD_APP_SENTINEL,
            "/tmp/k2-door-real/app",
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
                owner_workspace: &ws_a,
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
        for t in ["sql_clusters", "sql_databases", "sql_grants"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        let mut c = crate::sql::cluster::Cluster::layout(17, 5432, "baked");
        c.role = "k2".into();
        c.state = "online".into();
        crate::sql::cluster::record_in(&conn, &c, true).unwrap();
        for (id, name, w) in [("db-dm", "door_main", &ws_a), ("db-om", "other_main", &ws_b)] {
            conn.execute(
                "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major) VALUES (?1, ?2, ?3, 'active', 1, 17)",
                rusqlite::params![id, w, name],
            )
            .unwrap();
        }
    }
    let handle = db.handle();
    test_seams::set(
        APP,
        Some(Seam {
            admit: Arc::new(move || {
                let c = handle.lock();
                admitted_in(&c, APP)
            }),
            upstream: [(5432, PORT)].into(),
        }),
    );
    super::super::egress_proxy::ensure(APP, 0).expect("listener");
    let door = super::super::egress_proxy::listening(APP).expect("port");
    let mut fails = Vec::new();

    // 1. A granted App: select 1, SCRAM end to end, TLS asked and declined.
    let r = srv.via_door(door, "door_main_agent", "door-pass-a", "door_main", "sslmode=prefer", "select 1");
    check("select 1 through the door (sslmode=prefer: SSLRequest, N)", r.as_deref() == Ok("1"), format!("{r:?}"), &mut fails);
    let r = srv.via_door(door, "door_main_agent", "door-pass-a", "door_main", "sslmode=disable", "select current_user || '@' || current_database()");
    check("the session is the App's role on its database", r.as_deref() == Ok("door_main_agent@door_main"), format!("{r:?}"), &mut fails);
    let r = srv.via_door(door, "door_main_migrator", "door-pass-m", "door_main", "", "select 1");
    check("its migrator (its workspace owns the database)", r.as_deref() == Ok("1"), format!("{r:?}"), &mut fails);
    let r = srv.via_door(door, "door_main_agent", "door-pass-a", "door_main", "", "select version()");
    check("version() through the door", r.as_ref().is_ok_and(|v| v.starts_with("PostgreSQL")), format!("{r:?}"), &mut fails);
    let r = srv.via_door(door, "door_main_agent", "wrong", "door_main", "", "select 1");
    check(
        "a wrong password fails in Postgres's SCRAM (the door never sees it)",
        r.as_ref().is_err_and(|e| e.contains("password authentication failed for user \"door_main_agent\"")),
        format!("{r:?}"),
        &mut fails,
    );
    let r = srv.via_door(door, "door_main_agent", "door-pass-a", "door_main", "sslmode=require", "select 1");
    check("sslmode=require: no TLS inside the wall", r.as_ref().is_err_and(|e| e.contains("server does not support SSL")), format!("{r:?}"), &mut fails);
    // 2-4. Refusals, with the superuser's working password.
    let r = srv.via_door(door, "postgres", "su-pass", "door_main", "", "select 1");
    check("the superuser is refused", r.as_ref().is_err_and(|e| e.contains("[k2:db_door_superuser_refused]")), format!("{r:?}"), &mut fails);
    let r = srv.via_door(door, "postgres", "su-pass", "postgres", "", "select 1");
    check("the superuser on postgres is refused", r.as_ref().is_err_and(|e| e.contains("[k2:db_door_superuser_refused]")), format!("{r:?}"), &mut fails);
    let r = srv.via_door(door, "other_main_agent", "door-pass-o", "other_main", "", "select 1");
    check("another workspace's database is refused", r.as_ref().is_err_and(|e| e.contains("[k2:db_door_database_refused]")), format!("{r:?}"), &mut fails);
    let r = srv.via_door(door, "other_main_agent", "door-pass-o", "door_main", "", "select 1");
    check("another workspace's role is refused", r.as_ref().is_err_and(|e| e.contains("[k2:db_door_role_refused]")), format!("{r:?}"), &mut fails);

    // 5. A revoke: an idle session (the client reading a pipe kept open).
    let conn = format!("host=127.0.0.1 port={door} user=door_main_agent dbname=door_main connect_timeout=5");
    let mut idle = srv.spawn(&conn, "door-pass-a");
    let mut sin = idle.stdin.take().unwrap();
    sin.write_all(b"select 1\n").unwrap();
    sin.flush().unwrap();
    let t = Instant::now();
    while open_sessions(APP) == 0 && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    check("the idle session is open", open_sessions(APP) == 1, open_sessions(APP), &mut fails);
    let backends = || srv.su(&["select count(*) from pg_stat_activity where usename = 'door_main_agent'"]).join("");
    check("its backend is on the server", backends() == "1", backends(), &mut fails);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE sql_databases SET project_id = ?1 WHERE name = 'door_main'", [&ws_b]).unwrap();
    }
    let cut = recheck_open();
    check("the revoke cuts it", cut.len() == 1 && cut[0].database == "door_main", format!("{cut:?}"), &mut fails);
    let t = Instant::now();
    while backends() != "0" && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
    check("its backend is gone from the server", backends() == "0", backends(), &mut fails);
    let _ = sin.write_all(b"select 2\n");
    drop(sin);
    let out = idle.wait_with_output().expect("client exit");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    check(
        "the client saw the connection go",
        !out.status.success() && said.contains("ROW 1") && said.contains("ERR ") && !said.contains("ROW 2"),
        &said,
        &mut fails,
    );
    let r = srv.via_door(door, "door_main_agent", "door-pass-a", "door_main", "", "select 1");
    check("a new connection after the revoke is refused (the App has no database left)", r.as_ref().is_err_and(|e| e.contains("[k2:db_door_no_database]")), format!("{r:?}"), &mut fails);

    super::super::egress_proxy::stop(APP);
    let t = Instant::now();
    while in_flight(APP) > 0 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let lines = tail(APP, 200);
    let leaked = lines.iter().any(|l| {
        let s = l.to_string();
        s.contains("door-pass") || s.contains("su-pass") || s.contains("select")
    });
    check("db.log has no password and no SQL", !leaked && !lines.is_empty(), format!("{lines:?}"), &mut fails);
    eprintln!("db.log: {} line(s), e.g. {}", lines.len(), lines.first().map(|l| l.to_string()).unwrap_or_default());
    test_seams::set(APP, None);
    assert!(fails.is_empty(), "{} check(s) failed:\n{}", fails.len(), fails.join("\n"));
}
