//! One-time reminders on a real headless daemon, driven over `/cli/*`
//! (prd-routines-reminders-and-agent-mail-v1 R1). The real `k2-daemon` binary
//! under a temp HOME with `cat` shims for the agent: no OS jobs, no real
//! agent, no client attached. What this pins:
//!
//! - a reminder fires within seconds of its time (W10), not the 60 s tick;
//! - it fires exactly once, across a daemon restart and across a window the
//!   daemon was down for (late, with the catch-up stamp);
//! - more than 24 h late it retires as missed and never fires;
//! - POST-only routes answer 405 to a GET; a bad time is a usage refusal.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

struct KillOnDrop(std::process::Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "k2-rem-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

/// A headless daemon on a temp HOME. Restartable: `stop` then `start`.
struct HeadlessBox {
    home: PathBuf,
    k2_dir: PathBuf,
    shim_dir: PathBuf,
    _rt: k2_core::test_env::DaemonRuntime,
    daemon: Option<KillOnDrop>,
    db: PathBuf,
}

impl HeadlessBox {
    fn new() -> Self {
        let home = scratch("home");
        let k2_dir = home.join(".k2");
        std::fs::create_dir_all(&k2_dir).expect(".k2");
        let shim_dir = home.join("agent-shim");
        std::fs::create_dir_all(&shim_dir).expect("shim dir");
        k2_core::test_env::write_executable(&shim_dir.join("cat"), "#!/bin/sh\nexec /bin/cat\n");
        let rt = k2_core::test_env::DaemonRuntime::new(&home);
        let mut b = HeadlessBox { home, k2_dir, shim_dir, _rt: rt, daemon: None, db: PathBuf::new() };
        b.start();
        b
    }

    fn start(&mut self) {
        // A restart must not see the previous run's port file as ready.
        let _ = std::fs::remove_file(self.k2_dir.join("heartbeat.port"));
        let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &self.home)
            .envs(self._rt.envs())
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2_TEST_AGENT_SHIM_DIR", &self.shim_dir)
            .env("K2SO_WAKE_HEADLESS_TEST_COMMAND", "cat")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn daemon");
        self.daemon = Some(KillOnDrop(child));
        self.db = self.wait_ready();
    }

    fn stop(&mut self) {
        self.daemon = None;
    }

    fn wait_ready(&self) -> PathBuf {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let path = k2_core::db::resolve_home_db_path(&self.k2_dir);
            if path.exists() && self.k2_dir.join("heartbeat.port").exists() {
                if let Ok(conn) = rusqlite::Connection::open(&path) {
                    let migrated: rusqlite::Result<i64> = conn.query_row(
                        "SELECT COUNT(*) FROM _migrations WHERE name = '0278_heartbeat_reminders'",
                        [],
                        |r| r.get(0),
                    );
                    // Past the boot gate: a real route answers, even if only to
                    // say this workspace doesn't exist.
                    let open = self
                        .http("GET", "/cli/heartbeat/reminder/list?project=/nope", "")
                        .is_some_and(|(status, _)| !status.contains("503"));
                    if migrated == Ok(1) && open {
                        return path;
                    }
                }
            }
            assert!(Instant::now() < deadline, "daemon did not come up in time");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn token(&self) -> Option<String> {
        ["heartbeat.token", "daemon.token"]
            .iter()
            .find_map(|f| std::fs::read_to_string(self.k2_dir.join(f)).ok())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    /// `(status line, body)`. `None` when the daemon isn't listening yet.
    fn http(&self, method: &str, path_query: &str, body: &str) -> Option<(String, String)> {
        let port: u16 = std::fs::read_to_string(self.k2_dir.join("heartbeat.port")).ok()?.trim().parse().ok()?;
        let token = self.token()?;
        let sep = if path_query.contains('?') { '&' } else { '?' };
        let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
        let req = format!(
            "{method} {path_query}{sep}token={token} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).ok()?;
        let mut out = String::new();
        s.read_to_string(&mut out).ok()?;
        let (head, rest) = out.split_once("\r\n\r\n")?;
        Some((head.lines().next()?.to_string(), rest.to_string()))
    }

    fn open(&self) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(&self.db).expect("open daemon DB");
        conn.busy_timeout(Duration::from_secs(10)).expect("busy timeout");
        conn
    }

    /// A registered workspace with a manager agent. Returns `(path, id)`.
    fn workspace(&self, id: &str) -> (String, String) {
        let project = scratch("ws");
        let agent_dir = project.join(".k2so/agents/manager");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        std::fs::write(agent_dir.join("AGENT.md"), "---\nname: manager\ntype: manager\n---\n# manager\n").expect("AGENT.md");
        let path = project.to_string_lossy().into_owned();
        self.open()
            .execute(
                "INSERT INTO projects (id, path, name, agent_mode, agent_enabled) VALUES (?1, ?2, 'rem', 'manager', 1)",
                rusqlite::params![id, path],
            )
            .expect("insert project");
        (path, id.to_string())
    }

    fn add(&self, path: &str, name: &str, when_key: &str, when_value: &str) -> (String, String) {
        let q = format!(
            "/cli/heartbeat/reminder/add?project={}&name={name}&text=hello&{when_key}={}",
            urlenc(path),
            urlenc(when_value)
        );
        self.http("POST", &q, "").expect("daemon answered")
    }

    fn fires(&self, id: &str, name: &str, decision: &str) -> i64 {
        self.open()
            .query_row(
                "SELECT COUNT(*) FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 AND decision = ?3",
                rusqlite::params![id, name, decision],
                |r| r.get(0),
            )
            .expect("count fires")
    }

    fn all_fires(&self, id: &str) -> Vec<(String, Option<String>)> {
        self.open()
            .prepare("SELECT decision, reason FROM heartbeat_fires WHERE project_id = ?1 ORDER BY id")
            .expect("prepare")
            .query_map(rusqlite::params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows")
    }

    fn retired(&self, id: &str, name: &str) -> (Option<String>, i64) {
        self.open()
            .query_row(
                "SELECT retired_reason, enabled FROM workspace_heartbeats WHERE project_id = ?1 AND name = ?2",
                rusqlite::params![id, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row")
    }

    fn wait_for_fire(&self, id: &str, name: &str, decision: &str, within: Duration) -> Duration {
        let start = Instant::now();
        loop {
            if self.fires(id, name, decision) >= 1 {
                return start.elapsed();
            }
            assert!(start.elapsed() < within, "no {decision} within {within:?}; fires: {:?}", self.all_fires(id));
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Long enough for the boot scan (3 s) and the first tick after it.
    fn settle_past_boot_scan(&self) {
        std::thread::sleep(Duration::from_secs(12));
    }
}

impl Drop for HeadlessBox {
    fn drop(&mut self) {
        self.daemon = None;
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn in_secs(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn ago_secs(secs: i64) -> String {
    in_secs(-secs)
}

#[test]
fn a_reminder_fires_on_time_and_only_once_across_a_restart() {
    let mut b = HeadlessBox::new();
    let (path, id) = b.workspace("rem-once");
    let t0 = Instant::now();
    let (status, body) = b.add(&path, "soon", "at", &in_secs(10));
    assert!(status.contains("200"), "add: {status}: {body}");

    // W10: near its time, not on the 60 s tick (which, this soon after boot,
    // is still about a minute away). The bound leaves room for the delivery
    // itself, which waits for a new session to be ready.
    b.wait_for_fire(&id, "soon", "fired", Duration::from_secs(35));
    // `fire_at` is stored in whole seconds, so it can be up to 1 s early.
    assert!(t0.elapsed() >= Duration::from_secs(9), "must not fire before its time: {:?}", t0.elapsed());
    assert_eq!(b.retired(&id, "soon"), (Some("fired".to_string()), 0), "retired by the delivery stamp");

    b.stop();
    b.start();
    b.settle_past_boot_scan();
    assert_eq!(b.fires(&id, "soon", "fired"), 1, "a restart must not fire it again: {:?}", b.all_fires(&id));
    assert_eq!(b.fires(&id, "soon", "fired_catchup"), 0, "nor catch it up: {:?}", b.all_fires(&id));
    assert_eq!(b.fires(&id, "soon", "error"), 0, "{:?}", b.all_fires(&id));
}

#[test]
fn down_across_the_window_it_fires_once_late_and_only_once() {
    let mut b = HeadlessBox::new();
    let (path, id) = b.workspace("rem-late");
    let (status, body) = b.add(&path, "while-down", "in", "10m");
    assert!(status.contains("200"), "add: {status}: {body}");

    // The daemon is off across the fire time, by two hours.
    b.stop();
    b.open()
        .execute(
            "UPDATE workspace_heartbeats SET fire_at = ?1 WHERE project_id = ?2 AND name = 'while-down'",
            rusqlite::params![ago_secs(2 * 3600), id],
        )
        .expect("backdate");
    b.start();
    b.wait_for_fire(&id, "while-down", "fired_catchup", Duration::from_secs(30));
    assert_eq!(b.retired(&id, "while-down"), (Some("fired".to_string()), 0));

    // And a second restart finds nothing left to do.
    b.stop();
    b.start();
    b.settle_past_boot_scan();
    assert_eq!(b.fires(&id, "while-down", "fired_catchup"), 1, "exactly once: {:?}", b.all_fires(&id));
    assert_eq!(b.fires(&id, "while-down", "fired"), 0, "{:?}", b.all_fires(&id));
}

#[test]
fn more_than_a_day_late_it_retires_as_missed_and_does_not_fire() {
    let mut b = HeadlessBox::new();
    let (path, id) = b.workspace("rem-missed");
    let (status, body) = b.add(&path, "too-late", "in", "10m");
    assert!(status.contains("200"), "add: {status}: {body}");
    b.stop();
    b.open()
        .execute(
            "UPDATE workspace_heartbeats SET fire_at = ?1 WHERE project_id = ?2 AND name = 'too-late'",
            rusqlite::params![ago_secs(25 * 3600), id],
        )
        .expect("backdate");
    b.start();
    b.wait_for_fire(&id, "too-late", "skipped_missed", Duration::from_secs(30));
    assert_eq!(b.retired(&id, "too-late"), (Some("missed".to_string()), 0));
    b.settle_past_boot_scan();
    assert_eq!(b.fires(&id, "too-late", "fired"), 0, "{:?}", b.all_fires(&id));
    assert_eq!(b.fires(&id, "too-late", "fired_catchup"), 0, "{:?}", b.all_fires(&id));
    assert_eq!(b.fires(&id, "too-late", "skipped_missed"), 1, "one audit row, not one per tick: {:?}", b.all_fires(&id));
}

#[test]
fn routes_refuse_a_get_write_and_bad_input_and_list_what_is_pending() {
    let b = HeadlessBox::new();
    let (path, id) = b.workspace("rem-routes");

    for verb in ["add", "snooze", "cancel"] {
        let (status, body) = b
            .http("GET", &format!("/cli/heartbeat/reminder/{verb}?project={}&name=x", urlenc(&path)), "")
            .expect("answered");
        assert!(status.contains("405"), "GET {verb}: {status}: {body}");
    }

    let (status, body) = b.add(&path, "past", "at", &ago_secs(3600));
    assert!(status.contains("400"), "a past time: {status}: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["error"]["code"], "usage", "{body}");
    assert!(v["error"]["hint"].as_str().expect("hint").contains("past"), "{body}");

    let (status, body) = b.add(&path, "tooshort", "in", "30s");
    assert!(status.contains("400") && body.contains("too soon"), "{status}: {body}");

    let (status, body) = b.add(&path, "keeper", "in", "1h");
    assert!(status.contains("200"), "add: {status}: {body}");

    // A reminder is not a heartbeat: the heartbeat list leaves it out, so a
    // client that knows only heartbeats never draws a row with no WAKEUP.md.
    let (_, heartbeats) = b.http("GET", &format!("/cli/heartbeat/list?project={}", urlenc(&path)), "").expect("list");
    assert!(!heartbeats.contains("keeper"), "{heartbeats}");
    let (_, reminders) = b.http("GET", &format!("/cli/heartbeat/reminder/list?project={}", urlenc(&path)), "").expect("list");
    let rows: serde_json::Value = serde_json::from_str(&reminders).expect("json");
    assert_eq!(rows[0]["name"], "keeper", "{reminders}");
    assert_eq!(rows[0]["state"], "pending", "{reminders}");

    // The heartbeat verbs that assume a WAKEUP.md refuse a reminder by name.
    let (status, body) = b
        .http("POST", &format!("/cli/heartbeat/edit?project={}&name=keeper", urlenc(&path)), "")
        .expect("answered");
    assert!(status.contains("400") && body.contains("is a reminder"), "{status}: {body}");

    // `next_fire_at` equals `fire_at` from the daemon's own wait pass.
    let (fire_at, next): (String, String) = b
        .open()
        .query_row(
            "SELECT fire_at, next_fire_at FROM workspace_heartbeats WHERE project_id = ?1 AND name = 'keeper'",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(fire_at, next);
}

/// Delivery fails 5 times: the reminder finishes as `failed` rather than
/// sitting disabled forever. In process, because the failure path needs a
/// workspace that can't be delivered to.
#[test]
fn five_failed_deliveries_retire_the_reminder_as_failed() {
    use k2_core::db::schema::AgentHeartbeat;
    let _home = k2_core::test_env::StubHome::new("reminders-");
    k2_core::db::init_for_tests();
    let project = scratch("fail");
    let path = project.to_string_lossy().into_owned();
    let agent_dir = Path::new(&path).join(".k2so/agents/manager");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::write(agent_dir.join("AGENT.md"), "---\nname: manager\ntype: manager\n---\n# manager\n").expect("AGENT.md");
    let id = "rem-fail".to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // agent_enabled=0: deliver_live answers no_agent_mode, a real
        // delivery failure, and nothing spawns.
        conn.execute(
            "INSERT INTO projects (id, path, name, agent_mode, agent_enabled) VALUES (?1, ?2, 'f', 'manager', 0)",
            rusqlite::params![id, path],
        )
        .expect("project");
    }
    k2_core::heartbeats::reminders::add(
        &path,
        k2_core::heartbeats::reminders::NewReminder {
            text: "ping",
            when: k2_core::heartbeats::when::WhenInput { in_: Some("1m"), ..Default::default() },
            name: Some("doomed"),
            file: None,
            session: k2_core::heartbeats::reminders::SessionChoice::Pinned,
            created_by: "owner-token",
        },
        chrono::Utc::now(),
    )
    .expect("add");

    for attempt in 1..=5 {
        let v = k2_daemon::heartbeat_launch::smart_launch_scheduled(&path, "doomed", None, None);
        assert_eq!(v["success"], false, "attempt {attempt}: {v}");
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = AgentHeartbeat::get_by_name(&conn, &id, "doomed").expect("q").expect("row");
    assert_eq!(row.retired_reason.as_deref(), Some("failed"));
    assert!(!row.enabled && row.archived_at.is_some(), "retired, off, archived");
    assert_eq!(row.consecutive_failures, 5, "counted every failure");
}
