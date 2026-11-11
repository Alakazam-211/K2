//! `k2 app db connect|status|check|disconnect` in process
//! (prd-app-database-integrations-v1 S2), through the real App routes
//! (`routes::handle`: Member floor, held cards, the approval replay):
//!
//! - K2's records in the test's database (a running SQL server, a 17 and
//!   a 16 cluster), Postgres itself the sql fake (`FakeSystemOps`: every
//!   role and database statement is recorded);
//! - the App's real database door (`egress_proxy::ensure`) in front of a
//!   fake Postgres upstream that asks for the password and checks it
//!   against the `CREATE ROLE … PASSWORD` the fake recorded, so a PASS
//!   means: the App's own login exists with the password the vault holds,
//!   the door admits it on that database, and `select 1` answers;
//! - "inside the App" is the URL the App's service resolves from the vault,
//!   turned into its in-wall form (`127.0.0.1:5432`, as a container's start
//!   does) with pasta's forward to the door's host port.
//!
//! The real-Postgres run (SCRAM, a real cluster, the JS check through
//! node) is `real_pg_connect` (`db_connect_real_pg.rs`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_core::deploy::store;
use serde_json::{json, Value as J};

use super::super::db_door::test_seams::Seam;
use super::super::db_probe;
use super::*;
use crate::sql::sysops::FakeSystemOps;

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

// ── a fake Postgres that checks the App's password ───────────────────

fn read_n(s: &mut TcpStream, n: usize) -> Option<Vec<u8>> {
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).ok()?;
    Some(b)
}

fn pgmsg(t: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![t];
    v.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    v.extend_from_slice(body);
    v
}

fn pg_error(code: &str, m: &str) -> Vec<u8> {
    pgmsg(b'E', format!("SFATAL\0C{code}\0M{m}\0\0").as_bytes())
}

/// The password the fake recorded for `role` (`… "role" LOGIN PASSWORD '…'`).
fn recorded_password(fake: &FakeSystemOps, role: &str) -> Option<String> {
    let pg = fake.pg.lock().unwrap();
    let needle = format!("\"{role}\" LOGIN PASSWORD '");
    pg.role_sql.iter().rev().find_map(|s| {
        let i = s.rfind(&needle)? + needle.len();
        Some(s[i..].split('\'').next()?.to_string())
    })
}

fn fake_pg(fake: &'static FakeSystemOps) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { return };
            std::thread::spawn(move || {
                c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let Some(w) = read_n(&mut c, 4) else { return };
                let len = u32::from_be_bytes([w[0], w[1], w[2], w[3]]) as usize;
                let Some(body) = read_n(&mut c, len - 4) else { return };
                let mut parts = body[4..].split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).to_string());
                let mut params = HashMap::new();
                while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                    if k.is_empty() {
                        break;
                    }
                    params.insert(k, v);
                }
                let user = params.get("user").cloned().unwrap_or_default();
                let database = params.get("database").cloned().unwrap_or_default();
                if !fake.pg.lock().unwrap().dbs.contains(&database) {
                    let _ = c.write_all(&pg_error("3D000", &format!("database \"{database}\" does not exist")));
                    return;
                }
                let _ = c.write_all(&pgmsg(b'R', &3u32.to_be_bytes()));
                let Some(h) = read_n(&mut c, 5) else { return };
                let n = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
                let Some(pw) = read_n(&mut c, n - 4) else { return };
                let given = String::from_utf8_lossy(&pw).trim_end_matches('\0').to_string();
                if recorded_password(fake, &user).is_none_or(|want| want != given) {
                    let _ = c.write_all(&pg_error("28P01", &format!("password authentication failed for user \"{user}\"")));
                    return;
                }
                let mut out = pgmsg(b'R', &0u32.to_be_bytes());
                out.extend(pgmsg(b'S', b"server_version\x0017.6\0"));
                out.extend(pgmsg(b'Z', b"I"));
                if c.write_all(&out).is_err() {
                    return;
                }
                loop {
                    let Some(h) = read_n(&mut c, 5) else { return };
                    let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
                    let Some(_b) = read_n(&mut c, len - 4) else { return };
                    match h[0] {
                        b'Q' => {
                            let v = b"PostgreSQL 17.6 (k2 fake)";
                            let mut d = 2u16.to_be_bytes().to_vec();
                            d.extend_from_slice(&1u32.to_be_bytes());
                            d.push(b'1');
                            d.extend_from_slice(&(v.len() as u32).to_be_bytes());
                            d.extend_from_slice(v);
                            let mut out = pgmsg(b'T', &[0, 0]);
                            out.extend(pgmsg(b'D', &d));
                            out.extend(pgmsg(b'C', b"SELECT 1\0"));
                            out.extend(pgmsg(b'Z', b"I"));
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
    port
}

// ── the fixture ──────────────────────────────────────────────────────

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
    app: String,
    door: u16,
    fake: &'static FakeSystemOps,
}

impl Drop for Fx {
    fn drop(&mut self) {
        super::super::egress_proxy::stop(&self.app);
        super::super::db_door::test_seams::set(&self.app, None);
        test_seams::set_in_wall(None);
    }
}

fn ws_path(ws: &str) -> String {
    format!("/tmp/k2-dbc-test/{ws}")
}

fn fx(app: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    crate::vault::ensure_test_vault();
    let (ws_a, ws_b) = (format!("ws-a-{app}"), format!("ws-b-{app}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for w in [&ws_a, &ws_b] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![w, ws_path(w)]).unwrap();
        }
        let svc = k2_core::published_services::insert(
            &conn,
            &ws_a,
            app,
            k2_core::published_services::CMD_APP_SENTINEL,
            &format!("/tmp/k2-dbc-test/apps/{app}"),
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
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
        // Running: a live Seed and deployment.
        conn.execute(
            "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
             VALUES (?1, ?2, 'cmd', '{}', '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
            rusqlite::params![format!("seed-{app}"), app],
        )
        .unwrap();
        conn.execute(
            "UPDATE apps SET live_seed = ?2, live_deployment = 'd-0001', state = 'running' WHERE app_id = ?1",
            rusqlite::params![app, format!("seed-{app}")],
        )
        .unwrap();
        for t in ["sql_server", "sql_clusters", "sql_databases", "sql_grants"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', 17, 'localhost', 1)", []).unwrap();
        let mut c = crate::sql::cluster::Cluster::layout(17, 5432, "baked");
        c.role = "k2".into();
        c.state = "online".into();
        crate::sql::cluster::record_in(&conn, &c, true).unwrap();
    }
    MSGS.lock().unwrap().clear();
    super::super::alerts::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    let fake: &'static FakeSystemOps = Box::leak(Box::new(FakeSystemOps::baked()));
    let pg = fake_pg(fake);
    let handle = db.handle();
    let id = app.to_string();
    super::super::db_door::test_seams::set(
        app,
        Some(Seam {
            admit: Arc::new(move || {
                let c = handle.lock();
                super::super::db_door::admitted_in(&c, &id)
            }),
            upstream: [(5432, pg), (5433, pg)].into(),
        }),
    );
    super::super::db_door::test_seams::set_engine_ready(true);
    super::super::egress_proxy::ensure(app, 0).expect("the App's listener");
    let door = super::super::egress_proxy::listening(app).expect("its port");
    // "Inside the App": the URL its service resolves, in its in-wall form,
    // through pasta's forward to the door.
    test_seams::set_in_wall(Some(Box::new(move |app: &str, as_name: &str| {
        let row = super::super::reach::app_row(app).expect("the App");
        let Some(url) = url_for_app(&row, as_name) else {
            return db_probe::Probe::fail("no_url", format!("{as_name} is not in the App environment"));
        };
        let inwall = super::super::pg_choice::in_wall_dsn(&url).expect("a box Postgres URL");
        assert!(inwall.contains("@127.0.0.1:5432/") && inwall.ends_with("sslmode=disable"), "the App dials its in-wall door");
        db_probe::probe_url(&inwall.replace("127.0.0.1:5432", &format!("127.0.0.1:{door}")), Duration::from_secs(5))
    })));
    Fx { _db: db, _home: home, ws_a, ws_b, app: app.to_string(), door, fake }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

/// (HTTP status, JSON body) through the whole App route table.
fn call(f: &Fx, path: &str, who: &Who, body: J) -> (u16, J) {
    let r = crate::sql::routes::with_fake_ops(f.fake, || super::super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new()));
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn get(f: &Fx, path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::sql::routes::with_fake_ops(f.fake, || super::super::routes::handle(path, who.clone(), b"", &p));
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn set_switches(ws: &str, build: bool, core_db: bool, create: bool, versions: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "UPDATE projects SET agents_can_manage_skin = ?2, app_cores = ?3, db_agent_access = ?4, agents_can_upgrade_postgres = ?5 WHERE id = ?1",
        rusqlite::params![ws, build as i64, if core_db { "[\"db\"]" } else { "[]" }, if create { "write" } else { "off" }, versions as i64],
    )
    .unwrap();
}

fn pending_cards(app: &str) -> usize {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row("SELECT COUNT(*) FROM app_widenings WHERE app_label = ?1 AND status = 'pending'", [app], |r| r.get::<_, i64>(0)).unwrap() as usize
}

fn audit_events(app: &str) -> Vec<(String, String)> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut st = conn.prepare("SELECT event, payload_json FROM deploy_audit WHERE subject_id = ?1 ORDER BY seq").unwrap();
    st.query_map([app], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().flatten().collect()
}

/// Every password the fake recorded (none may appear in an answer).
fn passwords(f: &Fx) -> Vec<String> {
    let pg = f.fake.pg.lock().unwrap();
    pg.role_sql
        .iter()
        .flat_map(|s| s.match_indices("PASSWORD '").map(move |(i, _)| s[i + 10..].split('\'').next().unwrap_or("").to_string()))
        .filter(|p| p.len() >= 16)
        .collect()
}

fn assert_no_secret(f: &Fx, what: &str, v: &J) {
    let s = v.to_string();
    for p in passwords(f) {
        assert!(!s.contains(&p), "{what} printed a password: {s}");
    }
    assert!(!s.contains("postgres://") && !s.contains("postgresql://"), "{what} printed a URL: {s}");
}

/// Log in through the door as `user` with the password the vault gave the
/// App, and keep the session open.
fn door_session(f: &Fx, user: &str, db: &str) -> Result<TcpStream, String> {
    let pw = recorded_password(f.fake, user).ok_or("no password recorded")?;
    let mut s = TcpStream::connect(("127.0.0.1", f.door)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut body = 196_608u32.to_be_bytes().to_vec();
    for (k, v) in [("user", user), ("database", db)] {
        body.extend_from_slice(k.as_bytes());
        body.push(0);
        body.extend_from_slice(v.as_bytes());
        body.push(0);
    }
    body.push(0);
    let mut start = ((body.len() + 4) as u32).to_be_bytes().to_vec();
    start.extend(body);
    s.write_all(&start).map_err(|e| e.to_string())?;
    loop {
        let h = read_n(&mut s, 5).ok_or("closed")?;
        let n = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
        let b = read_n(&mut s, n - 4).ok_or("closed")?;
        match h[0] {
            b'E' => return Err(String::from_utf8_lossy(&b).to_string()),
            b'R' if b[..4] == 3u32.to_be_bytes() => {
                let mut p = pw.clone().into_bytes();
                p.push(0);
                s.write_all(&pgmsg(b'p', &p)).map_err(|e| e.to_string())?;
            }
            b'Z' => return Ok(s),
            _ => {}
        }
    }
}

// ── 1. a fresh database, end to end, PASS from inside ────────────────

#[test]
fn connect_new_gives_the_app_its_own_login_and_passes_from_inside() {
    let f = fx("dbc-new");
    let (st, v) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true }));
    assert_eq!(st, 200, "{v}");
    let db = v["db"].as_str().unwrap().to_string();
    assert!(db.starts_with("ws_ws_a_dbc_new"), "a database of the App's workspace: {db}");
    assert_eq!((v["major"].as_u64(), v["cluster"].as_str(), v["kind"].as_str()), (Some(17), Some("17/main"), Some("container")));
    let group = super::super::db_door::app_group_role(&db, &f.app);
    let login = format!("{group}__g1");
    assert_eq!(v["roles"], json!([login, format!("{db}_migrator")]), "its own login, never {db}_agent");
    assert_eq!(v["secrets"], json!(["app-db.dbc-new.DATABASE_URL", "app-db.dbc-new.DATABASE_MIGRATE_URL"]));
    assert_eq!(v["doorOpen"], true);
    assert_eq!(v["check"]["ok"], true, "{v}");
    assert_eq!(v["check"]["version"], "PostgreSQL 17.6 (k2 fake)");
    assert_eq!(v["check"]["from"], "app", "inside the App's wall");
    assert_eq!(v["rehost"]["result"], "done");
    assert_eq!(test_seams::rehosted(), vec![f.app.clone()], "re-hosted once: secrets arrive at a start");
    assert_no_secret(&f, "connect", &v);
    // The roles, as Postgres got them.
    let sql = f.fake.pg.lock().unwrap().role_sql.join("\n");
    for want in [
        format!("CREATE ROLE \"{group}\" NOLOGIN INHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS"),
        format!("CREATE ROLE \"{login}\" LOGIN PASSWORD '"),
        "CONNECTION LIMIT 20".to_string(),
        format!("GRANT \"{group}\" TO \"{login}\""),
        format!("GRANT \"{db}_agent\" TO \"{group}\""),
        format!("GRANT CONNECT ON DATABASE \"{db}\" TO \"{group}\""),
    ] {
        assert!(sql.contains(&want), "missing {want} in:\n{sql}");
    }
    // What the App's next start gets: both URLs, as env, by name.
    let row = super::super::reach::app_row(&f.app).unwrap();
    let g = crate::vault::publish_env::granted(&row.service_id, &f.ws_a, std::path::Path::new("/nonexistent"));
    let names: Vec<&str> = g.env.iter().map(|(k, _)| k.as_str()).collect();
    assert!(names.contains(&"DATABASE_URL") && names.contains(&"DATABASE_MIGRATE_URL"), "{names:?}");
    let url = &g.env.iter().find(|(k, _)| k == "DATABASE_URL").unwrap().1;
    assert!(url.starts_with(&format!("postgres://{login}:")) && url.ends_with(&format!("@127.0.0.1:5432/{db}")), "the App's own login");
    // Recorded, audited, shown by status.
    assert!(audit_events(&f.app).iter().any(|(e, p)| e == "db.connect" && p.contains(&login) && !p.contains("postgres://")));
    let (st, s) = get(&f, STATUS, &Who::Owner, &[("app", &f.app)]);
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["connections"][0]["db"], db.as_str());
    assert_eq!(s["connections"][0]["check"]["ok"], true);
    assert_eq!(s["majors"], json!(k2_root_proto::pg_major::SUPPORTED), "the majors come from the daemon");
    assert_no_secret(&f, "status", &s);
    // A check on demand: PASS again, audited.
    let (st, c) = call(&f, CHECK, &Who::Owner, json!({ "app": f.app }));
    assert_eq!((st, &c["check"]["ok"]), (200, &json!(true)), "{c}");
    assert!(audit_events(&f.app).iter().any(|(e, _)| e == "db.check"));
    // The same connect again: the same database and login, a new password.
    let (st, again) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true }));
    assert_eq!((st, again["db"].as_str(), again["roles"][0].as_str()), (200, Some(db.as_str()), Some(login.as_str())), "{again}");
    assert_eq!(again["check"]["ok"], true, "{again}");
}

// ── 2. an agent: one card for every switch ───────────────────────────

#[test]
fn an_agent_without_its_switches_gets_one_card_naming_each_and_approval_connects() {
    let f = fx("dbc-card");
    set_switches(&f.ws_a, false, false, false, false);
    let a = agent(&f.ws_a);
    let (st, v) = call(&f, CONNECT, &a, json!({ "app": f.app, "new": true, "reason": "feedback store" }));
    assert_eq!(st, 202, "held: {v}");
    assert_eq!(v["status"], "held");
    let id = v["request"]["id"].as_str().unwrap().to_string();
    let off: Vec<&str> = v["switches"].as_array().unwrap().iter().filter(|s| s["on"] == false).map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(off, vec!["Build and run Apps", "Database (what its Apps may use)", "Let this agent create databases"], "each by its Settings name");
    assert!(v["switches"][0]["where"].as_str().unwrap().starts_with("Settings → Workspaces → "), "{v}");
    let lines = v["diff"]["lines"].to_string();
    for s in &off {
        assert!(lines.contains(s), "the card lists {s}: {lines}");
    }
    assert!(v["hint"].as_str().unwrap().contains(&format!("k2 app approve {id}")));
    // Nothing happened yet.
    assert!(f.fake.pg.lock().unwrap().dbs.is_empty(), "no database before the owner says yes");
    assert!(connections_of(&f.app).unwrap().is_empty());
    // Asking again is the same card, never a second one.
    let (st, again) = call(&f, CONNECT, &a, json!({ "app": f.app, "new": true, "reason": "feedback store" }));
    assert_eq!((st, again["request"]["id"].as_str()), (202, Some(id.as_str())));
    assert_eq!(pending_cards(&f.app), 1, "one card, not several");
    // Status shows it pending.
    let (_, s) = get(&f, STATUS, &a, &[("app", &f.app), ("request", &id)]);
    assert_eq!(s["pending"][0]["id"], id.as_str());
    assert_eq!(s["request"]["status"], "pending");
    // The owner approves once, in person: every switch goes on, the App is
    // connected and checked.
    let tok = super::super::egress_routes::test_seams::vault_handle().confirm.mint(super::super::approvals::CONFIRM_ACTION, Some(&id), "owner", "local-auth");
    // The approval routes are served with the held-request routes.
    let r = crate::sql::routes::with_fake_ops(f.fake, || {
        super::super::egress_routes::handle("/cli/app/approve", &Who::Owner, json!({ "request": id, "confirmToken": tok }).to_string().as_bytes(), &HashMap::new())
    })
    .unwrap_or_else(|e| panic!("approve: {} {}", e.code, e.hint));
    assert_eq!(r["status"], "approved", "{r}");
    assert_eq!(r["result"]["check"]["ok"], true, "connected and checked: {r}");
    assert_eq!(r["result"]["switchesTurnedOn"], json!(["apps_build", "app_core_db", "db_create"]));
    assert_eq!(super::super::access::level(&f.ws_a), super::super::access::Level::Build);
    assert!(super::super::cores::ticked(&f.ws_a).iter().any(|c| c == "db"));
    assert_eq!(k2_core::workspace::settings::db_agent_access_for_path(&ws_path(&f.ws_a)), "write");
    assert_no_secret(&f, "approval", &r);
    assert_eq!(pending_cards(&f.app), 0);
    let (_, s) = get(&f, STATUS, &a, &[("app", &f.app), ("request", &id)]);
    assert_eq!(s["request"]["status"], "approved");
    assert_eq!(s["connections"][0]["check"]["ok"], true);
    // Now every switch is on and the database is its own: no card.
    let (st, c) = call(&f, CONNECT, &a, json!({ "app": f.app, "db": s["connections"][0]["db"], "as": "REPORTS_URL" }));
    assert_eq!(st, 200, "no card once every switch is on: {c}");
    assert_eq!(c["secrets"], json!(["app-db.dbc-card.REPORTS_URL", "app-db.dbc-card.REPORTS_MIGRATE_URL"]));
}

#[test]
fn another_workspaces_database_always_takes_a_card_and_never_its_migrator() {
    let f = fx("dbc-grant");
    set_switches(&f.ws_a, true, true, true, true);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major, agent_secret_ref, migrator_secret_ref) \
             VALUES ('db-b', ?1, 'b_shared', 'active', 1, 17, 'x', 'y')",
            [&f.ws_b],
        )
        .unwrap();
        conn.execute("INSERT INTO sql_grants (database_id, project_id, level, can_manage, created_at, updated_at) VALUES ('db-b', ?1, 'read', 0, 1, 1)", [&f.ws_a])
            .unwrap();
    }
    f.fake.pg.lock().unwrap().dbs.push("b_shared".into());
    let (st, v) = call(&f, CONNECT, &agent(&f.ws_a), json!({ "app": f.app, "db": "b_shared" }));
    assert_eq!(st, 202, "{v}");
    assert!(v["diff"]["lines"].to_string().contains("belongs to workspace"), "{v}");
    // The owner, directly: the App gets the URL, never the migrator's.
    let (st, o) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "db": "b_shared" }));
    assert_eq!(st, 200, "{o}");
    assert_eq!(o["secrets"], json!(["app-db.dbc-grant.DATABASE_URL"]));
    let grantee = crate::sql::ops::default_agent_role(&f.ws_a);
    assert!(f.fake.pg.lock().unwrap().role_sql.join("\n").contains(&format!("GRANT \"{grantee}\" TO")), "the App holds its workspace's grant, no more");
}

// ── 3. who may ───────────────────────────────────────────────────────

#[test]
fn a_member_is_refused_and_another_workspaces_agent_never_sees_the_app() {
    let f = fx("dbc-who");
    let member = Who::Login { username: "mo".into(), role: Role::Member };
    for path in [CONNECT, CHECK, DISCONNECT] {
        let (st, v) = call(&f, path, &member, json!({ "app": f.app, "new": true }));
        assert_eq!((st, v["error"]["code"].as_str()), (403, Some(super::super::access::MEMBER_READ_ONLY)), "{path}: {v}");
    }
    let (st, v) = call(&f, CONNECT, &agent(&f.ws_b), json!({ "app": f.app, "new": true }));
    assert_eq!((st, v["error"]["code"].as_str()), (404, Some("app_not_found")), "{v}");
    assert!(f.fake.pg.lock().unwrap().dbs.is_empty());
    // Usage errors are `usage` (exit 1 in the CLI), never held.
    for (body, why) in [
        (json!({ "app": f.app }), "neither"),
        (json!({ "app": f.app, "new": true, "db": "x" }), "both"),
        (json!({ "app": f.app, "db": "x", "major": 17 }), "--major with --db"),
        (json!({ "app": f.app, "new": true, "as": "DATABASE" }), "--as without _URL"),
        (json!({ "app": f.app, "new": true, "as": "K2_TOKEN_URL" }), "--as K2_*"),
    ] {
        let (st, v) = call(&f, CONNECT, &Who::Owner, body);
        assert_eq!((st, v["error"]["code"].as_str()), (400, Some(codes::USAGE)), "{why}: {v}");
    }
}

// ── 4. the major is a parameter ──────────────────────────────────────

#[test]
fn the_major_is_honoured_and_a_missing_cluster_or_major_is_refused() {
    let f = fx("dbc-major");
    let (st, v) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true, "major": 16 }));
    assert_eq!((st, v["error"]["code"].as_str()), (409, Some(codes::CLUSTER_MISSING)), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("k2 db create --major 16"), "{v}");
    let (st, v) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true, "major": 18 }));
    assert_eq!((st, v["error"]["code"].as_str()), (409, Some(codes::MAJOR_UNSUPPORTED)), "{v}");
    let (st, v) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true, "cluster": "nope" }));
    assert_eq!((st, v["error"]["code"].as_str()), (409, Some(codes::CLUSTER_MISSING)), "{v}");
    assert!(f.fake.pg.lock().unwrap().dbs.is_empty(), "nothing made");
    // A 16 cluster on 5433: --major 16 lands there.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut c = crate::sql::cluster::Cluster::layout(16, 5433, "created");
        c.role = "k2".into();
        c.state = "online".into();
        crate::sql::cluster::record_in(&conn, &c, true).unwrap();
    }
    let (st, v) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true, "major": 16 }));
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["major"].as_u64(), v["cluster"].as_str()), (Some(16), Some("16/main")), "{v}");
    let major: i64 = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.query_row("SELECT cluster_major FROM sql_databases WHERE name = ?1", [v["db"].as_str().unwrap()], |r| r.get(0)).unwrap()
    };
    assert_eq!(major, 16, "K2's record says 16");
    assert_eq!(v["check"]["ok"], true, "{v}");
    // --new again on another major: the App's own database is on 16.
    let (st, e) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true, "major": 17 }));
    assert_eq!((st, e["error"]["code"].as_str()), (409, Some(codes::MAJOR_MISMATCH)), "{e}");
    assert!(e["error"]["hint"].as_str().unwrap().contains("--db ws_"), "{e}");
    // The name is connected now: another database under it is refused.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO sql_databases (id, project_id, name, status, created_at, cluster_major) VALUES ('db-other', ?1, 'ws_other', 'active', 2, 17)",
            [&f.ws_a],
        )
        .unwrap();
    }
    let (st, e) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "db": "ws_other" }));
    assert_eq!((st, e["error"]["code"].as_str()), (409, Some(codes::ALREADY_CONNECTED)), "{e}");
}

// ── 5. disconnect ────────────────────────────────────────────────────

#[test]
fn disconnect_revokes_cuts_open_sessions_and_shuts_the_door() {
    let f = fx("dbc-off");
    let (st, v) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true }));
    assert_eq!(st, 200, "{v}");
    let (db, login) = (v["db"].as_str().unwrap().to_string(), v["roles"][0].as_str().unwrap().to_string());
    // An open session through the door, as the App's login.
    let mut s = door_session(&f, &login, &db).expect("the App's login through its door");
    assert_eq!(super::super::db_door::open_sessions(&f.app), 1);
    let (st, d) = call(&f, DISCONNECT, &Who::Owner, json!({ "app": f.app }));
    assert_eq!(st, 200, "{d}");
    assert_eq!((d["ok"].as_bool(), d["doorOpen"].as_bool(), d["sessionsCut"].as_u64()), (Some(true), Some(false), Some(1)), "{d}");
    assert_no_secret(&f, "disconnect", &d);
    // The session is gone, the door refuses the login, Postgres was told.
    let mut b = [0u8; 1];
    assert!(matches!(s.read(&mut b), Ok(0) | Err(_)), "the open session was cut");
    let r = door_session(&f, &login, &db).unwrap_err();
    assert!(r.contains("[k2:db_door_role_refused]"), "{r}");
    let sql = f.fake.pg.lock().unwrap().helper_sql.join("\n");
    assert!(sql.contains("NOLOGIN") && sql.contains("pg_terminate_backend") && sql.contains("REVOKE CONNECT ON DATABASE"), "{sql}");
    // Its secrets went; the database stays.
    let row = super::super::reach::app_row(&f.app).unwrap();
    assert!(url_for_app(&row, "DATABASE_URL").is_none() && url_for_app(&row, "DATABASE_MIGRATE_URL").is_none());
    let g = crate::vault::publish_env::granted(&row.service_id, &f.ws_a, std::path::Path::new("/nonexistent"));
    assert!(!g.env.iter().any(|(k, _)| k.starts_with("DATABASE_")), "{:?}", g.env.iter().map(|(k, _)| k).collect::<Vec<_>>());
    assert!(f.fake.pg.lock().unwrap().dbs.contains(&db), "never dropped");
    let (st, c) = call(&f, CHECK, &Who::Owner, json!({ "app": f.app }));
    assert_eq!((st, c["error"]["code"].as_str()), (404, Some(codes::NOT_CONNECTED)), "{c}");
    assert!(audit_events(&f.app).iter().any(|(e, _)| e == "db.disconnect"));
    // An agent may always disconnect (narrowing, no switch).
    set_switches(&f.ws_a, true, true, true, false);
    let (st, _) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true }));
    assert_eq!(st, 200);
    set_switches(&f.ws_a, false, false, false, false);
    let (st, d) = call(&f, DISCONNECT, &agent(&f.ws_a), json!({ "app": f.app }));
    assert_eq!(st, 200, "narrowing never waits: {d}");
}

// ── 6. FAIL says why, with the App's output ──────────────────────────

#[test]
fn a_failed_check_names_the_step_and_shows_the_apps_output() {
    let f = fx("dbc-fail");
    test_seams::set_rehost(&f.app, Rehost::Failed("health gate: no answer on its port".into()));
    test_seams::set_output(&f.app, vec!["Error: connect ECONNREFUSED 127.0.0.1:5432".into(), "\u{1b}[31mexit 1".into()]);
    let (st, v) = call(&f, CONNECT, &Who::Owner, json!({ "app": f.app, "new": true }));
    assert_eq!(st, 200, "connected, the check failed: {v}");
    assert_eq!((v["ok"].as_bool(), v["check"]["ok"].as_bool(), v["check"]["step"].as_str()), (Some(false), Some(false), Some("rehost_failed")), "{v}");
    assert_eq!(v["check"]["appOutput"][0], "Error: connect ECONNREFUSED 127.0.0.1:5432");
    assert!(v["check"]["hint"].as_str().unwrap().contains("k2 app rehost"), "{v}");
    // The door refusing the App: its record gone behind K2's back.
    test_seams::set_rehost(&f.app, Rehost::Done("d-0002".into()));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE app_db_connections SET login_role = group_role || '__g7' WHERE app_id = ?1", [&f.app]).unwrap();
    }
    let (_, c) = call(&f, CHECK, &Who::Owner, json!({ "app": f.app }));
    assert_eq!(c["check"]["step"], "door_refused", "{c}");
    assert!(c["check"]["error"].as_str().unwrap().contains("db_door_role_refused"), "{c}");
}

// ── 7. a Seed's requires.postgres: the same state ────────────────────

#[test]
fn the_install_path_leaves_the_same_state_as_connect() {
    let f = fx("dbc-seed");
    // What install does after its gates: create the workspace's database
    // (create_for_app_install), then connect the App to it.
    crate::sql::routes::with_fake_ops(f.fake, || {
        let made = crate::sql::routes::create_for_app_install(&f.ws_a, &ws_path(&f.ws_a), 17, false).expect("install's database");
        let name = made["name"].as_str().unwrap().to_string();
        let c = connect_for_install("owner", &f.app, &name).expect("connect");
        let login = super::super::db_door::app_login_role(&name, &f.app, 1);
        assert_eq!(c["roles"], json!([login, format!("{name}_migrator")]), "the App's own login, as connect");
        assert_eq!(c["secrets"], json!(["app-db.dbc-seed.DATABASE_URL", "app-db.dbc-seed.DATABASE_MIGRATE_URL"]));
        assert_eq!(c["check"]["ok"], true, "{c}");
    });
    let conns = connections_of(&f.app).unwrap();
    assert_eq!(conns.len(), 1);
    assert_eq!(conns[0].login_role, super::super::db_door::app_login_role(&conns[0].database, &f.app, 1));
    let row = super::super::reach::app_row(&f.app).unwrap();
    let url = url_for_app(&row, "DATABASE_URL").unwrap();
    assert!(!url.contains("_agent:"), "never the agent's DSN");
}

// ── naming ───────────────────────────────────────────────────────────

#[test]
fn app_role_names_fit_postgres_and_stay_one_to_one() {
    let long_db = "ws_0b7a3c2e_1f4d_4a8e_9c6b_2d5e8f1a3b7c_app_feedback";
    let g = super::super::db_door::app_group_role(long_db, "feedback-reader");
    assert!(g.len() <= super::super::db_door::MAX_GROUP, "{g}");
    let l = super::super::db_door::app_login_role(long_db, "feedback-reader", 9999);
    assert!(l.len() <= 63, "{l}");
    assert!(super::super::db_door::is_app_role(&l, long_db, "feedback-reader"));
    assert!(!super::super::db_door::is_app_role(&l, long_db, "feedback-writer"));
    assert_ne!(g, super::super::db_door::app_group_role(long_db, "feedback-writer"));
    assert_eq!(super::super::db_door::app_group_role("ws_a", "x"), "ws_a_app_x", "short names stay readable");
    assert_eq!(migrate_name("DATABASE_URL"), "DATABASE_MIGRATE_URL");
    assert_eq!(migrate_name("ANALYTICS_URL"), "ANALYTICS_MIGRATE_URL");
    assert!(as_name_ok("ANALYTICS_URL") && !as_name_ok("analytics_url") && !as_name_ok("_URL") && !as_name_ok("K2_DB_URL"));
}
