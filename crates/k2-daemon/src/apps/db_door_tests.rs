//! The container database door (S21), end to end on loopback: the App's
//! real proxy listener (`egress_proxy::ensure`), K2's records in the
//! test's database (owned databases, a grant, two clusters), and a fake
//! Postgres upstream per cluster that speaks just enough protocol 3 for a
//! startup, `select 1`, a cancel and a terminate. A small client sends
//! what libpq sends (SSLRequest first, then the startup).
//!
//! The real-Postgres run (libpq's psql through the door, SCRAM end to end)
//! is `real_pg_door` (`db_door_real_pg.rs`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_core::deploy::store;
use serde_json::{json, Value as J};

use super::test_seams::Seam;
use super::*;

// ── a fake Postgres ──────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Seen {
    startups: Arc<Mutex<Vec<Vec<(String, String)>>>>,
    cancels: Arc<Mutex<Vec<(u32, Vec<u8>)>>>,
}

fn read_n(s: &mut TcpStream, n: usize) -> Option<Vec<u8>> {
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).ok()?;
    Some(b)
}

fn msg(t: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![t];
    v.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    v.extend_from_slice(body);
    v
}

/// A fake cluster: AuthenticationOk, a ParameterStatus, BackendKeyData
/// (pid 1000 + n, key n), ReadyForQuery; then `Q` → one row "1".
fn fake_pg(version: &'static str) -> (u16, Seen) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Seen::default();
    let s2 = seen.clone();
    std::thread::spawn(move || {
        let mut n: u32 = 0;
        for c in l.incoming() {
            let Ok(mut c) = c else { return };
            n += 1;
            let seen = s2.clone();
            std::thread::spawn(move || {
                c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let Some(w) = read_n(&mut c, 4) else { return };
                let len = u32::from_be_bytes([w[0], w[1], w[2], w[3]]) as usize;
                let Some(body) = read_n(&mut c, len - 4) else { return };
                let code = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                if code == 80_877_102 {
                    let pid = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                    seen.cancels.lock().unwrap().push((pid, body[8..].to_vec()));
                    return;
                }
                let mut params = Vec::new();
                let mut parts = body[4..].split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).to_string());
                while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                    if k.is_empty() {
                        break;
                    }
                    params.push((k, v));
                }
                seen.startups.lock().unwrap().push(params);
                let mut out = msg(b'R', &0u32.to_be_bytes());
                out.extend(msg(b'S', format!("server_version\0{version}\0").as_bytes()));
                let mut k = (1000 + n).to_be_bytes().to_vec();
                k.extend_from_slice(&n.to_be_bytes());
                out.extend(msg(b'K', &k));
                out.extend(msg(b'Z', b"I"));
                if c.write_all(&out).is_err() {
                    return;
                }
                loop {
                    let Some(h) = read_n(&mut c, 5) else { return };
                    let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
                    let Some(_b) = read_n(&mut c, len - 4) else { return };
                    match h[0] {
                        b'Q' => {
                            let mut row = 1u16.to_be_bytes().to_vec();
                            row.extend_from_slice(b"?column?\0");
                            row.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 23, 0, 4, 0xff, 0xff, 0xff, 0xff, 0, 0]);
                            let mut out = msg(b'T', &row);
                            let mut d = 1u16.to_be_bytes().to_vec();
                            d.extend_from_slice(&1u32.to_be_bytes());
                            d.push(b'1');
                            out.extend(msg(b'D', &d));
                            out.extend(msg(b'C', b"SELECT 1\0"));
                            out.extend(msg(b'Z', b"I"));
                            if c.write_all(&out).is_err() {
                                return;
                            }
                        }
                        b'X' => return,
                        _ => {}
                    }
                }
            });
        }
    });
    (port, seen)
}

// ── a small client ───────────────────────────────────────────────────

fn startup(params: &[(&str, &str)]) -> Vec<u8> {
    let mut b = 196_608u32.to_be_bytes().to_vec();
    for (k, v) in params {
        b.extend_from_slice(k.as_bytes());
        b.push(0);
        b.extend_from_slice(v.as_bytes());
        b.push(0);
    }
    b.push(0);
    let mut out = ((b.len() + 4) as u32).to_be_bytes().to_vec();
    out.extend(b);
    out
}

fn dial(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s
}

/// One server message; `None` at EOF or a reset.
fn read_msg(s: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let h = read_n(s, 5)?;
    let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
    Some((h[0], read_n(s, len - 4)?))
}

fn fields(body: &[u8]) -> HashMap<char, String> {
    body.split(|b| *b == 0).filter(|f| !f.is_empty()).map(|f| (f[0] as char, String::from_utf8_lossy(&f[1..]).to_string())).collect()
}

/// What the door said instead of a session.
#[derive(Debug)]
struct Refused {
    code: String,
    sqlstate: String,
    message: String,
}

fn refused_from(f: HashMap<char, String>) -> Refused {
    let m = f.get(&'M').cloned().unwrap_or_default();
    let code = m.rsplit_once("[k2:").map(|(_, c)| c.trim_end_matches(']').to_string()).unwrap_or_default();
    Refused { code, sqlstate: f.get(&'C').cloned().unwrap_or_default(), message: m }
}

/// Read up to ReadyForQuery: `Ok(backend key)` or the ErrorResponse.
fn until_ready(s: &mut TcpStream) -> Result<(u32, Vec<u8>), Refused> {
    let mut key = None;
    loop {
        match read_msg(s) {
            Some((b'E', b)) => return Err(refused_from(fields(&b))),
            Some((b'K', b)) => key = Some((u32::from_be_bytes([b[0], b[1], b[2], b[3]]), b[4..].to_vec())),
            Some((b'Z', _)) => return Ok(key.expect("the server sent BackendKeyData")),
            Some(_) => {}
            None => panic!("closed before ReadyForQuery or an error"),
        }
    }
}

/// libpq's order: SSLRequest (the door says N), then the startup.
fn open(port: u16, params: &[(&str, &str)]) -> Result<(TcpStream, (u32, Vec<u8>)), Refused> {
    let mut s = dial(port);
    s.write_all(&[0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f]).unwrap();
    let mut n = [0u8; 1];
    s.read_exact(&mut n).unwrap();
    assert_eq!(n[0], b'N', "no TLS inside the wall");
    s.write_all(&startup(params)).unwrap();
    let k = until_ready(&mut s)?;
    Ok((s, k))
}

fn login(port: u16, user: &str, db: &str) -> Result<(TcpStream, (u32, Vec<u8>)), Refused> {
    open(port, &[("user", user), ("database", db), ("application_name", "door-test")])
}

/// `select 1` on an open session: the one value, or why it failed.
fn select1(s: &mut TcpStream) -> Result<String, String> {
    s.write_all(&msg(b'Q', b"select 1\0")).map_err(|e| e.to_string())?;
    let mut val = None;
    loop {
        match read_msg(s) {
            Some((b'D', b)) => val = Some(String::from_utf8_lossy(&b[6..]).to_string()),
            Some((b'Z', _)) => return val.ok_or_else(|| "no row".into()),
            Some((b'E', b)) => return Err(format!("{:?}", fields(&b))),
            Some(_) => {}
            None => return Err("closed".into()),
        }
    }
}

/// Raw bytes in, the door's ErrorResponse out.
fn raw_refusal(port: u16, bytes: &[u8]) -> Refused {
    let mut s = dial(port);
    s.write_all(bytes).unwrap();
    match read_msg(&mut s) {
        Some((b'E', b)) => refused_from(fields(&b)),
        other => panic!("not an ErrorResponse: {other:?}"),
    }
}

/// EOF without any answer.
fn closes_silently(s: &mut TcpStream) {
    let mut b = [0u8; 64];
    match s.read(&mut b) {
        Ok(0) => {}
        Ok(n) => panic!("expected a silent close, got {:?}", &b[..n]),
        Err(e) => assert!(matches!(e.kind(), std::io::ErrorKind::ConnectionReset), "{e}"),
    }
}

// ── the fixture ──────────────────────────────────────────────────────

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    app: String,
    door: u16,
    ws_a: String,
    pg16: Seen,
    pg17: Seen,
}

impl Drop for Fx {
    fn drop(&mut self) {
        // Every log line written before the temp HOME goes.
        super::super::egress_proxy::stop(&self.app);
        let t = Instant::now();
        while (in_flight(&self.app) > 0 || open_sessions(&self.app) > 0) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        test_seams::set(&self.app, None);
    }
}

/// Workspaces A (owns `dr_main` on 16 and a test database, is granted
/// `shared17` on 17), B (owns `shared17` and `b_main`), C (owns
/// nothing). `app` is a guarded container App of workspace `ws`.
fn fx(app: &str, ws: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let (ws_a, ws_b, ws_c) = (format!("ws-a-{app}"), format!("ws-b-{app}"), format!("ws-c-{app}"));
    let owner = match ws {
        "a" => ws_a.clone(),
        "b" => ws_b.clone(),
        _ => ws_c.clone(),
    };
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for w in [&ws_a, &ws_b, &ws_c] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![w, format!("/tmp/k2-door-test/{w}")])
                .unwrap();
        }
        for (id, w) in [(app.to_string(), owner.clone()), (format!("{app}-other"), ws_a.clone())] {
            let svc = k2_core::published_services::insert(
                &conn,
                &w,
                &id,
                k2_core::published_services::CMD_APP_SENTINEL,
                &format!("/tmp/k2-door-test/apps/{id}"),
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
                    app_id: &id,
                    service_id: &svc.id,
                    seed_id: None,
                    owner_workspace: &w,
                    isolation: "container",
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        for t in ["sql_clusters", "sql_databases", "sql_grants"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        for (major, port) in [(16u32, 5432u16), (17, 5433)] {
            let mut c = crate::sql::cluster::Cluster::layout(major, port, if port == 5432 { "baked" } else { "created" });
            c.role = "k2".into();
            c.state = "online".into();
            crate::sql::cluster::record_in(&conn, &c, true).unwrap();
        }
        for (id, name, w, major, status) in [
            ("db-a", "dr_main", &ws_a, 16i64, "active"),
            ("db-at", "dr_test", &ws_a, 16, "test"),
            ("db-s", "shared17", &ws_b, 17, "active"),
            ("db-b", "b_main", &ws_b, 16, "active"),
            ("db-gone", "dropped_one", &ws_a, 16, "dropped"),
        ] {
            conn.execute(
                "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major) VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                rusqlite::params![id, w, name, status, major],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sql_grants (database_id, project_id, level, can_manage, created_at, updated_at) VALUES ('db-s', ?1, 'write', 0, 1, 1)",
            [&ws_a],
        )
        .unwrap();
    }
    // Notes to App agents stay in the test.
    super::super::alerts::set_sink(Some(Box::new(|_ws: &str, _text: &str| {})));
    let (p16, pg16) = fake_pg("16.4");
    let (p17, pg17) = fake_pg("17.6");
    let handle = db.handle();
    let id = app.to_string();
    test_seams::set(
        app,
        Some(Seam {
            admit: Arc::new(move || {
                let c = handle.lock();
                admitted_in(&c, &id)
            }),
            upstream: [(5432, p16), (5433, p17)].into(),
        }),
    );
    super::super::egress_proxy::ensure(app, 0).expect("the App's listener");
    let door = super::super::egress_proxy::listening(app).expect("its port");
    Fx { _db: db, _home: home, app: app.to_string(), door, ws_a, pg16, pg17 }
}

fn last_startup(s: &Seen) -> Vec<(String, String)> {
    s.startups.lock().unwrap().last().cloned().unwrap_or_default()
}

fn log_has(app: &str, pred: impl Fn(&J) -> bool) -> Vec<J> {
    let t = Instant::now();
    loop {
        let l = tail(app, 200);
        if l.iter().any(&pred) {
            return l;
        }
        assert!(t.elapsed() < Duration::from_secs(5), "{l:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ── what an App may open (records) ───────────────────────────────────

#[test]
fn an_app_may_open_its_workspaces_databases_and_its_grants_only() {
    let f = fx("door-rec", "a");
    let h = k2_core::db::shared();
    let conn = h.lock();
    let grantee = crate::sql::ops::default_agent_role(&f.ws_a);
    assert_eq!(
        admitted_in(&conn, "door-rec"),
        vec![
            Admit { database: "dr_main".into(), roles: vec!["dr_main_agent".into(), "dr_main_migrator".into()], port: 5432, owned: true },
            Admit { database: "dr_test".into(), roles: vec!["dr_test_agent".into(), "dr_test_migrator".into()], port: 5432, owned: true },
            Admit { database: "shared17".into(), roles: vec![grantee], port: 5433, owned: false },
        ],
        "never b_main (another workspace's), never a dropped database"
    );
    assert!(admitted_in(&conn, "no-such-app").is_empty());
    // The App's own roles (S2 makes them), never another App's.
    let a = &admitted_in(&conn, "door-rec")[0];
    for ok in ["dr_main_app_door_rec", "dr_main_app_door_rec__g1", "dr_main_app_door_rec__g12"] {
        assert!(a.allows(ok, "door-rec"), "{ok}");
    }
    for no in [
        "dr_main_app_door_rec_other",
        "dr_main_app_door_rec__g",
        "dr_main_app_door_rec__g01",
        "dr_main_app_door_rec__g1x",
        "dr_main_app_door_rec_g1",
        "b_main_agent",
        "postgres",
    ] {
        assert!(!a.allows(no, "door-rec"), "{no}");
    }
    assert!(!a.allows("dr_main_app_door_rec", "door-rec-other"), "another App's group role");
    // App `door-rec-g1`'s group role is never App `door-rec`'s login.
    assert_eq!(app_group_role("dr_main", "door-rec-g1"), "dr_main_app_door_rec_g1");
    assert!(!is_app_role("dr_main_app_door_rec_g1", "dr_main", "door-rec"));
    assert!(!is_app_role("dr_main_app_door_rec__g1", "dr_main", "door-rec-g1"));
}

// ── 1. a granted App connects and runs select 1 ──────────────────────

#[test]
fn a_container_app_with_a_grant_connects_and_runs_select_1() {
    let f = fx("door-ok", "a");
    // Its own workspace's database, on the 16 cluster.
    let (mut s, _) = login(f.door, "dr_main_agent", "dr_main").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s).unwrap(), "1");
    let got = last_startup(&f.pg16);
    assert!(got.contains(&("user".into(), "dr_main_agent".into())) && got.contains(&("database".into(), "dr_main".into())), "{got:?}");
    assert!(got.contains(&("application_name".into(), "door-test".into())), "the startup goes upstream as it came: {got:?}");
    // A database granted to its workspace, on the 17 cluster: the door
    // picks the cluster by name, the App dials the same 5432.
    let grantee = crate::sql::ops::default_agent_role(&f.ws_a);
    let (mut s17, _) = login(f.door, &grantee, "shared17").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s17).unwrap(), "1");
    assert_eq!(f.pg17.startups.lock().unwrap().len(), 1, "17's cluster got that one");
    assert_eq!(f.pg16.startups.lock().unwrap().len(), 1);
    // Its migrator too (its workspace owns the database).
    let (mut m, _) = login(f.door, "dr_main_migrator", "dr_main").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut m).unwrap(), "1");
    assert_eq!(open_sessions("door-ok"), 3);
    s.write_all(&msg(b'X', b"")).unwrap();
    drop(s);
    let log = log_has("door-ok", |l| l["result"] == "ok" && l["database"] == "dr_main" && l["role"] == "dr_main_agent");
    let line = log.iter().find(|l| l["result"] == "ok").unwrap();
    assert_eq!(line["upstream"], 5432, "{line}");
    assert!(line["up"].as_u64().unwrap() > 0 && line["down"].as_u64().unwrap() > 0, "{line}");
    assert!(!line.to_string().contains("select"), "never SQL in the log: {line}");
    // The same listener still serves the HTTP proxy: an origin-form
    // request gets the proxy's own refusal, not the door's.
    let mut h = dial(f.door);
    h.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut b = Vec::new();
    let _ = h.read_to_end(&mut b);
    let text = String::from_utf8_lossy(&b);
    assert!(text.starts_with("HTTP/1.1 400"), "{text}");
    assert!(text.contains("bad_proxy_request"), "{text}");
}

// ── 2. no grant, no door ─────────────────────────────────────────────

#[test]
fn without_a_grant_it_is_refused() {
    let f = fx("door-none", "c");
    let r = login(f.door, "dr_main_agent", "dr_main").unwrap_err();
    assert_eq!((r.code.as_str(), r.sqlstate.as_str()), (codes::NO_DATABASE, "28000"), "{r:?}");
    assert!(r.message.contains("App door-none has no database on this box"), "{r:?}");
    assert!(f.pg16.startups.lock().unwrap().is_empty(), "nothing reached a cluster");
    log_has("door-none", |l| l["result"] == "refused" && l["code"] == codes::NO_DATABASE && l["role"] == "dr_main_agent");
}

#[test]
fn a_database_not_granted_yet_is_refused_until_it_is() {
    let f = fx("door-later", "a");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("DELETE FROM sql_grants", []).unwrap();
    }
    let grantee = crate::sql::ops::default_agent_role(&f.ws_a);
    let r = login(f.door, &grantee, "shared17").unwrap_err();
    assert_eq!(r.code, codes::DATABASE_REFUSED, "{r:?}");
    assert!(r.message.contains("may not open database shared17"), "{r:?}");
    // The grant counts at the next connection: no re-host, no rule.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO sql_grants (database_id, project_id, level, can_manage, created_at, updated_at) VALUES ('db-s', ?1, 'read', 0, 1, 1)",
            [&f.ws_a],
        )
        .unwrap();
    }
    let (mut s, _) = login(f.door, &grantee, "shared17").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s).unwrap(), "1");
    assert!(f.pg16.startups.lock().unwrap().is_empty());
}

// ── 3. another App's role or database ────────────────────────────────

#[test]
fn another_apps_role_or_database_is_refused() {
    let f = fx("door-a", "a");
    // Another workspace's database (its App's), with its own role.
    let r = login(f.door, "b_main_agent", "b_main").unwrap_err();
    assert_eq!(r.code, codes::DATABASE_REFUSED, "{r:?}");
    // Its own database, another workspace's role.
    let r = login(f.door, "b_main_agent", "dr_main").unwrap_err();
    assert_eq!(r.code, codes::ROLE_REFUSED, "{r:?}");
    assert!(r.message.contains("the door admits dr_main_agent, dr_main_migrator there"), "{r:?}");
    // Another App's own role on a database both may open.
    let r = login(f.door, "dr_main_app_door_a_other", "dr_main").unwrap_err();
    assert_eq!(r.code, codes::ROLE_REFUSED, "{r:?}");
    // The granted database's owner's roles: never through a grant.
    let r = login(f.door, "shared17_agent", "shared17").unwrap_err();
    assert_eq!(r.code, codes::ROLE_REFUSED, "{r:?}");
    let r = login(f.door, "shared17_migrator", "shared17").unwrap_err();
    assert_eq!(r.code, codes::ROLE_REFUSED, "{r:?}");
    // No database named: Postgres takes the user's name. Same check.
    let r = open(f.door, &[("user", "b_main")]).unwrap_err();
    assert_eq!(r.code, codes::DATABASE_REFUSED, "{r:?}");
    // A name twice: the server keeps the last, the door would check the
    // first. Never.
    let r = open(f.door, &[("user", "dr_main_agent"), ("database", "dr_main"), ("user", "b_main_agent")]).unwrap_err();
    assert_eq!(r.code, codes::BAD_STARTUP, "{r:?}");
    let r = open(f.door, &[("user", "dr_main_agent"), ("database", "dr_main"), ("database", "b_main")]).unwrap_err();
    assert_eq!(r.code, codes::BAD_STARTUP, "{r:?}");
    // A name past 63 bytes would be cut by the server to something else.
    let long = format!("dr_main_agent{}", "x".repeat(60));
    let r = login(f.door, &long, "dr_main").unwrap_err();
    assert_eq!(r.code, codes::BAD_STARTUP, "{r:?}");
    assert!(f.pg16.startups.lock().unwrap().is_empty() && f.pg17.startups.lock().unwrap().is_empty(), "nothing reached a cluster");
    // The App's own role (S2) on its own database: through.
    let (mut s, _) = login(f.door, "dr_main_app_door_a__g1", "dr_main").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s).unwrap(), "1");
}

// ── 4. the superuser ─────────────────────────────────────────────────

#[test]
fn the_superuser_role_is_refused() {
    let f = fx("door-su", "a");
    for (user, db) in [("postgres", "dr_main"), ("postgres", "postgres"), ("pg_monitor", "dr_main"), ("pg_read_all_data", "dr_main")] {
        let r = login(f.door, user, db).unwrap_err();
        assert_eq!((r.code.as_str(), r.sqlstate.as_str()), (codes::SUPERUSER_REFUSED, "28000"), "{user}: {r:?}");
    }
    // No database named: `postgres` both ways.
    let r = open(f.door, &[("user", "postgres")]).unwrap_err();
    assert_eq!(r.code, codes::SUPERUSER_REFUSED, "{r:?}");
    // Replication, as any role.
    for rep in ["database", "true", "on", "1"] {
        let r = open(f.door, &[("user", "dr_main_agent"), ("database", "dr_main"), ("replication", rep)]).unwrap_err();
        assert_eq!(r.code, codes::REPLICATION_REFUSED, "{rep}: {r:?}");
    }
    let (mut s, _) = open(f.door, &[("user", "dr_main_agent"), ("database", "dr_main"), ("replication", "false")]).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s).unwrap(), "1");
    assert_eq!(f.pg16.startups.lock().unwrap().len(), 1, "only the plain one reached the cluster");
}

// ── 5. a revoke ──────────────────────────────────────────────────────

/// A revoke cuts new connections and open ones (Postgres keeps a revoked
/// role's open sessions; the door doesn't). Sessions it still allows stay.
#[test]
fn a_revoke_cuts_new_and_open_connections() {
    let f = fx("door-rv", "a");
    let grantee = crate::sql::ops::default_agent_role(&f.ws_a);
    let (mut granted, _) = login(f.door, &grantee, "shared17").unwrap_or_else(|r| panic!("{r:?}"));
    let (mut own, _) = login(f.door, "dr_main_agent", "dr_main").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut granted).unwrap(), "1");
    assert_eq!(open_sessions("door-rv"), 2);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("DELETE FROM sql_grants", []).unwrap();
    }
    // What `k2 db revoke` runs after the change.
    let mut v = json!({"ok": true});
    crate::apps::reach::after_db_change(&mut v);
    let note = v["apps"].as_array().unwrap().iter().find(|a| a["app"] == "door-rv").cloned().unwrap_or(J::Null);
    assert_eq!(note["doorCut"], 1, "{v}");
    assert_eq!(
        note["note"],
        format!("App door-rv: 1 open database session(s) cut now (shared17 as {grantee}): it may no longer use them, and new connections are refused"),
        "{v}"
    );
    // The open one is gone at once.
    assert!(select1(&mut granted).is_err(), "the revoked session was cut");
    let log = log_has("door-rv", |l| l["database"] == "shared17" && l["end"] == "revoked");
    assert!(log.iter().any(|l| l["result"] == "ok" && l["end"] == "revoked"), "{log:?}");
    // A new one is refused.
    let r = login(f.door, &grantee, "shared17").unwrap_err();
    assert_eq!(r.code, codes::DATABASE_REFUSED, "{r:?}");
    // The App's own database's session is untouched.
    assert_eq!(select1(&mut own).unwrap(), "1");
    assert_eq!(open_sessions("door-rv"), 1);
    // Nothing more to cut.
    assert!(recheck_open().is_empty());
}

/// Dropping a database cuts its sessions too (same path).
#[test]
fn a_dropped_database_cuts_its_sessions() {
    let f = fx("door-drop", "a");
    let (mut s, _) = login(f.door, "dr_test_agent", "dr_test").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s).unwrap(), "1");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE sql_databases SET status = 'dropped' WHERE name = 'dr_test'", []).unwrap();
    }
    assert_eq!(recheck_open(), vec![Cut { app: "door-drop".into(), database: "dr_test".into(), role: "dr_test_agent".into() }]);
    assert!(select1(&mut s).is_err());
}

// ── 6. a re-host ─────────────────────────────────────────────────────

/// The door survives a re-host: each deployment's start sends the App's
/// own proxy port as `dbPort` (its slot doesn't move), the start re-opens
/// the same listener (`ensure`, same port: kept), so the old deployment's
/// sessions run until it stops and the new one connects at once.
#[test]
fn the_door_survives_an_app_rehost() {
    let f = fx("door-rh", "a");
    let with = json!({"protocol": 3, "features": ["box-forward", "relaxed", "build", "db-forward", "db-door"]});
    let without = json!({"protocol": 3, "features": ["box-forward", "relaxed", "build", "db-forward"]});
    for gw in [38_601u16, 38_601] {
        let px = k2_root_proto::apps::proxy_port_for_gateway(gw);
        assert_eq!(door_port_for(px, Some(&with)), Some(38_901), "every deployment of the App gets its door");
    }
    assert_eq!(door_port_for(Some(38_901), Some(&without)), None, "an engine without the door: no dbPort, K2 says so");
    assert_eq!(door_port_for(Some(38_901), None), None);
    assert_eq!(door_port_for(Some(5432), Some(&with)), None, "never a cluster port");
    assert_eq!(door_port_for(None, Some(&with)), None);
    // The in-wall URL a start delivers (the vault's 127.0.0.1:5433 URL).
    let (env, moved) = super::super::guarded::in_wall_db_env(vec![
        ("DATABASE_URL".into(), "postgres://shared_ro:pw@127.0.0.1:5433/shared17".into()),
        ("STRIPE_KEY".into(), "sk_test_x".into()),
        ("OUTSIDE_URL".into(), "postgres://u:p@db.example.com:5432/x".into()),
    ]);
    assert_eq!(moved, vec!["DATABASE_URL".to_string()]);
    assert_eq!(env[0].1, "postgres://shared_ro:pw@127.0.0.1:5432/shared17?sslmode=disable");
    assert_eq!((env[1].1.as_str(), env[2].1.as_str()), ("sk_test_x", "postgres://u:p@db.example.com:5432/x"));

    let (mut old, _) = login(f.door, "dr_main_agent", "dr_main").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut old).unwrap(), "1");
    // The re-host's start: the same listener, kept.
    super::super::egress_proxy::ensure("door-rh", f.door).unwrap();
    assert_eq!(super::super::egress_proxy::listening("door-rh"), Some(f.door));
    assert_eq!(select1(&mut old).unwrap(), "1", "the old deployment's session runs on");
    let (mut new, _) = login(f.door, "dr_main_agent", "dr_main").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut new).unwrap(), "1");
    assert_eq!(open_sessions("door-rh"), 2);
    // Uninstall closes the door and every session at once.
    super::super::egress_proxy::stop("door-rh");
    assert!(select1(&mut new).is_err() && select1(&mut old).is_err());
    log_has("door-rh", |l| l["end"] == "app_stopped");
}

// ── 7. malformed and oversized ───────────────────────────────────────

#[test]
fn a_malformed_or_oversized_startup_is_refused_without_a_panic() {
    let f = fx("door-bad", "a");
    let p = f.door;
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("length 4", vec![0, 0, 0, 4], codes::BAD_STARTUP),
        ("length 7", vec![0, 0, 0, 7, 0, 0, 0], codes::BAD_STARTUP),
        ("over 10000", vec![0, 0, 0x27, 0x11], codes::STARTUP_TOO_LARGE),
        ("65535", vec![0, 0, 0xff, 0xff], codes::STARTUP_TOO_LARGE),
        ("16 MiB - 1", vec![0, 0xff, 0xff, 0xff], codes::STARTUP_TOO_LARGE),
        ("protocol 2", [&[0u8, 0, 0, 9][..], &[0, 2, 0, 0, 0]].concat(), codes::PROTOCOL_UNSUPPORTED),
        ("protocol 4", [&[0u8, 0, 0, 9][..], &[0, 4, 0, 0, 0]].concat(), codes::PROTOCOL_UNSUPPORTED),
        ("no terminator", [&[0u8, 0, 0, 13][..], &[0, 3, 0, 0], b"user\0"].concat(), codes::BAD_STARTUP),
        ("value cut", [&[0u8, 0, 0, 16][..], &[0, 3, 0, 0], b"user\0abc\0"].concat(), codes::BAD_STARTUP),
        ("bytes after the end", [&[0u8, 0, 0, 20][..], &[0, 3, 0, 0], b"user\0abc\0\0xyz"].concat(), codes::BAD_STARTUP),
        ("not UTF-8", [&[0u8, 0, 0, 16][..], &[0, 3, 0, 0], b"user\0\xff\xfe\0\0\0"].concat(), codes::BAD_STARTUP),
        ("empty name", [&[0u8, 0, 0, 13][..], &[0, 3, 0, 0], b"\0abc\0\0"].concat(), codes::BAD_STARTUP),
        ("no user", [&[0u8, 0, 0, 20][..], &[0, 3, 0, 0], b"database\0x\0\0\0"].concat(), codes::BAD_STARTUP),
        ("ssl with a body", vec![0, 0, 0, 12, 0x04, 0xd2, 0x16, 0x2f, 1, 2, 3, 4], codes::BAD_STARTUP),
        ("short cancel", vec![0, 0, 0, 12, 0x04, 0xd2, 0x16, 0x2e, 0, 0, 0, 1], codes::BAD_STARTUP),
        ("TLS first", vec![0x16, 0x03, 0x01, 0x00, 0x2f, 0x01, 0x00, 0x00, 0x2b], codes::TLS_REFUSED),
    ];
    for (what, bytes, want) in &cases {
        let r = raw_refusal(p, bytes);
        assert_eq!((r.code.as_str(), r.sqlstate.as_str()), (*want, if *want == codes::PROTOCOL_UNSUPPORTED { "0A000" } else { "08P01" }), "{what}: {r:?}");
    }
    // A second SSLRequest after the N.
    let mut s = dial(p);
    s.write_all(&[0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f]).unwrap();
    let mut n = [0u8; 1];
    s.read_exact(&mut n).unwrap();
    s.write_all(&[0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f]).unwrap();
    match read_msg(&mut s) {
        Some((b'E', b)) => assert_eq!(refused_from(fields(&b)).code, codes::BAD_STARTUP),
        other => panic!("{other:?}"),
    }
    // A slow startup: the length word, then nothing.
    let mut s = dial(p);
    s.write_all(&[0, 0, 0, 40]).unwrap();
    let t = Instant::now();
    match read_msg(&mut s) {
        Some((b'E', b)) => assert!(refused_from(fields(&b)).message.contains("no startup packet within"), "{:?}", fields(&b)),
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() >= STARTUP_TIMEOUT && t.elapsed() < STARTUP_TIMEOUT * 3, "{:?}", t.elapsed());
    // Cut off halfway: nothing to answer, nothing breaks.
    let mut s = dial(p);
    s.write_all(&[0, 0, 0, 100, 0, 3, 0, 0, b'u']).unwrap();
    drop(s);
    // A cancel with a key no session of this App holds: dropped silently.
    let mut s = dial(p);
    s.write_all(&[0, 0, 0, 16, 0x04, 0xd2, 0x16, 0x2e, 0, 0, 0x03, 0xe9, 0, 0, 0, 1]).unwrap();
    closes_silently(&mut s);
    // A little fuzz: random bytes after a 0x00 never panic the door.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..3000 {
        let mut b = Vec::new();
        let len = 4 + (x % 40) as usize;
        for _ in 0..len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            b.push((x & 0xff) as u8);
        }
        let _ = parse_packet(&b);
        let word = [0, b[0], b[1], b[2]];
        let _ = packet_len(word);
        if let Ok(Packet::Startup { params, .. }) = parse_packet(&b) {
            let _ = decide("door-bad", &params, &[]);
        }
    }
    // After all that the door still serves.
    let (mut s, _) = login(p, "dr_main_agent", "dr_main").unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s).unwrap(), "1");
    assert!(f.pg16.startups.lock().unwrap().len() == 1, "only the good one reached a cluster");
    let l = log_has("door-bad", |l| l["code"] == codes::STARTUP_TOO_LARGE);
    assert!(l.iter().any(|l| l["code"] == codes::TLS_REFUSED), "{l:?}");
}

/// Postgres 18's protocol 3.2 passes through untouched.
#[test]
fn protocol_3_2_passes_untouched() {
    let f = fx("door-32", "a");
    let mut b = 196_610u32.to_be_bytes().to_vec();
    b.extend_from_slice(b"user\0dr_main_agent\0database\0dr_main\0_pq_.test\0x\0\0");
    let mut pkt = ((b.len() + 4) as u32).to_be_bytes().to_vec();
    pkt.extend(b);
    let mut s = dial(f.door);
    s.write_all(&pkt).unwrap();
    until_ready(&mut s).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(select1(&mut s).unwrap(), "1");
    assert!(last_startup(&f.pg16).contains(&("_pq_.test".into(), "x".into())));
}

/// A cancel reaches the upstream that issued that backend's key, only for
/// this App's own sessions.
#[test]
fn a_cancel_reaches_the_session_that_issued_its_key() {
    let f = fx("door-cx", "a");
    let grantee = crate::sql::ops::default_agent_role(&f.ws_a);
    let (_s, (pid, key)) = login(f.door, &grantee, "shared17").unwrap_or_else(|r| panic!("{r:?}"));
    let cancel = |pid: u32, key: &[u8]| {
        let mut b = vec![0, 0, 0, (12 + key.len()) as u8, 0x04, 0xd2, 0x16, 0x2e];
        b.extend_from_slice(&pid.to_be_bytes());
        b.extend_from_slice(key);
        let mut c = dial(f.door);
        c.write_all(&b).unwrap();
        closes_silently(&mut c);
    };
    cancel(pid, &[9, 9, 9, 9]);
    cancel(pid + 77, &key);
    cancel(pid, &key);
    let t = Instant::now();
    while f.pg17.cancels.lock().unwrap().is_empty() && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(*f.pg17.cancels.lock().unwrap(), vec![(pid, key.clone())], "to the 17 cluster, only the right key");
    assert!(f.pg16.cancels.lock().unwrap().is_empty());
    let l = log_has("door-cx", |l| l["kind"] == "cancel" && l["result"] == "sent");
    assert_eq!(l.iter().filter(|l| l["kind"] == "cancel" && l["result"] == "unknown_key").count(), 2, "{l:?}");
}

/// Pure: the ErrorResponse the App's driver shows.
#[test]
fn the_error_response_names_the_reason() {
    let r = Refusal { code: codes::ROLE_REFUSED, sqlstate: "28000", message: "App x may not log in to d as r".into() };
    let b = error_response("x", &r);
    assert_eq!(b[0], b'E');
    assert_eq!(u32::from_be_bytes([b[1], b[2], b[3], b[4]]) as usize, b.len() - 1);
    let f = fields(&b[5..]);
    assert_eq!(f[&'S'], "FATAL");
    assert_eq!(f[&'C'], "28000");
    assert_eq!(f[&'M'], "K2 database door: App x may not log in to d as r [k2:db_door_role_refused]");
    assert_eq!(f[&'H'], "k2 app doctor x shows what the App may reach");
    assert_eq!(*b.last().unwrap(), 0);
}
