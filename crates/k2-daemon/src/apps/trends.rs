//! CPU and memory trends for box usage (prd-app-resources-disk-guard-v1
//! §18 Slice B, research B1; 0.46.4; migration 0217).
//!
//! ```text
//! GET /cli/app/resources/trends [app, window=1h|6h|24h|7d|30d]   the view's readers
//! ```
//!
//! - **The sampler** (one daemon thread, every platform, headless): every
//!   60 s it reads the box (CPU from `/proc/stat`, RAM, load), each running
//!   App (the engine's `stats`: memory and, from 0.46.4 engines, `cpu.stat`)
//!   and each running `--cmd` service (`/proc` by session: resident memory
//!   and CPU time), and writes one minute row each. Each finished hour
//!   rolls up into one hour row; minutes are kept 24 h, hours 30 days
//!   (`k2_core::resource_samples`). Unknown is `null`, never 0 (macOS has no
//!   box CPU split; the process engine has no cgroup).
//! - **RAM budgets** (App runtime PRD Q9): each App's row carries its
//!   budget at sample time (`apps.memory_limit_bytes`, the admission
//!   budget) so a trend shows use against it. This module only reads:
//!   nothing here enforces a budget.
//! - CPU is **cores busy** on average (`0.5` = half of one core); the box
//!   row counts every core (`cores` says how many there are).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use k2_core::deploy::store;
use k2_core::disk_guard as dg;
use k2_core::resource_samples::{self as rs, Kind, Point, Sample};
use serde_json::{json, Value as J};

use super::routes::{db_err, no, R, Who};

pub const ROUTE: &str = "/cli/app/resources/trends";
pub const TICK: Duration = Duration::from_secs(60);
/// The windows a reader may ask for, in seconds.
pub const WINDOWS: &[(&str, i64)] = &[("1h", 3600), ("6h", 6 * 3600), ("24h", 86_400), ("7d", 7 * 86_400), ("30d", 30 * 86_400)];
pub const DEFAULT_WINDOW: &str = "24h";

// ── the sampler ──────────────────────────────────────────────────────

/// `/proc/<pid>/stat` → `(session, rss pages, utime + stime ticks)`.
pub fn parse_proc_stat(stat: &str) -> Option<(i32, u64, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After the comm: f[0] = state (field 3), so field N is f[N - 3]:
    // session 6, utime 14, stime 15, rss 24.
    let sid = f.get(3)?.parse().ok()?;
    let ut: u64 = f.get(11)?.parse().ok()?;
    let st: u64 = f.get(12)?.parse().ok()?;
    let rss = f.get(21)?.parse().ok()?;
    Some((sid, rss, ut.saturating_add(st)))
}

/// Resident bytes and CPU microseconds per session id, from one `/proc`
/// pass (Linux; empty elsewhere).
#[allow(unused_mut)]
pub fn by_session() -> HashMap<i32, (u64, u64)> {
    let mut out: HashMap<i32, (u64, u64)> = HashMap::new();
    #[cfg(target_os = "linux")]
    {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        if let Ok(rd) = std::fs::read_dir("/proc") {
            for e in rd.flatten() {
                let name = e.file_name();
                let Some(n) = name.to_str() else { continue };
                if !n.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
                if let Some((sid, pages, ticks)) = parse_proc_stat(&stat) {
                    let v = out.entry(sid).or_insert((0, 0));
                    v.0 = v.0.saturating_add(pages.saturating_mul(page));
                    v.1 = v.1.saturating_add(ticks.saturating_mul(1_000_000) / tck);
                }
            }
        }
    }
    out
}

/// The box's `/proc/stat` busy/total jiffies (Linux).
fn box_stat() -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        rs::parse_proc_stat_cpu(&std::fs::read_to_string("/proc/stat").ok()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The last cumulative readings, to turn counters into cores.
#[derive(Default)]
pub struct Counters {
    box_stat: Option<(u64, u64)>,
    cpu: HashMap<(Kind, String), (u64, Instant)>,
}

impl Counters {
    /// Cores busy since the last reading of this subject (`None` the first
    /// time, or when the counter went back).
    pub fn cores(&mut self, k: Kind, id: &str, usec: Option<u64>, at: Instant) -> Option<f64> {
        let usec = usec?;
        let prev = self.cpu.insert((k, id.to_string()), (usec, at));
        let (p, t) = prev?;
        rs::cores_between(p, usec, at.duration_since(t).as_micros() as u64)
    }

    fn box_cores(&mut self, now: Option<(u64, u64)>) -> Option<f64> {
        let prev = std::mem::replace(&mut self.box_stat, now);
        rs::box_cores_between(prev?, now?, dg::cores().unwrap_or(1))
    }

    /// Forget subjects not seen this round (a stopped App's counter).
    fn retain(&mut self, seen: &[(Kind, String)]) {
        self.cpu.retain(|k, _| seen.contains(k));
    }
}

/// One round of samples (outside the DB lock but for the App list).
pub fn sample(c: &mut Counters) -> Vec<(Kind, Sample)> {
    let at = Instant::now();
    let mut rows = Vec::new();
    let (total, avail) = dg::box_mem();
    rows.push((
        Kind::Box,
        Sample {
            id: rs::BOX_ID.into(),
            cpu_cores: c.box_cores(box_stat()),
            mem_bytes: total.zip(avail).map(|(t, a)| t.saturating_sub(a)),
            mem_available_bytes: avail,
            load5: dg::load5(),
            ..Sample::default()
        },
    ));
    let apps = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_apps(&conn, None).unwrap_or_default()
    };
    for a in &apps {
        let Some(inst) = super::supervisor::live_instance(&a.app_id) else { continue };
        let Ok(s) = super::supervisor::engine_of(&inst.handle).stats(&inst.handle) else { continue };
        rows.push((
            Kind::App,
            Sample {
                id: a.app_id.clone(),
                cpu_cores: c.cores(Kind::App, &a.app_id, s.cpu_usec, at),
                mem_bytes: s.mem_current,
                mem_budget_bytes: a.memory_limit_bytes.and_then(|b| u64::try_from(b).ok()),
                ..Sample::default()
            },
        ));
    }
    let procs = by_session();
    for (ws, name, sid) in crate::publish_runtime::live_children() {
        let id = format!("{ws}/{name}");
        let (mem, usec) = match procs.get(&sid) {
            Some((m, u)) => (Some(*m), Some(*u)),
            None => (None, None),
        };
        let cpu = c.cores(Kind::Service, &id, usec, at);
        rows.push((Kind::Service, Sample { id, cpu_cores: cpu, mem_bytes: mem, ..Sample::default() }));
    }
    let seen: Vec<(Kind, String)> = rows.iter().map(|(k, s)| (*k, s.id.clone())).collect();
    c.retain(&seen);
    rows
}

/// Write one round and keep the table bounded.
pub fn record(conn: &rusqlite::Connection, rows: &[(Kind, Sample)], now: i64) -> rusqlite::Result<()> {
    rs::insert_minute(conn, now, rows)?;
    rs::roll_up(conn, now)?;
    rs::prune(conn, now)?;
    Ok(())
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// The sampler thread (spawned with the disk watch; every platform,
/// headless). The first round only primes the CPU counters' baseline.
pub fn spawn() {
    if cfg!(test) || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("resource-trends".into()).spawn(|| {
        super::disk_scan::wait_for_db();
        let mut c = Counters::default();
        loop {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rows = sample(&mut c);
                let now = k2_core::deploy::now();
                let db = k2_core::db::shared();
                let conn = db.lock();
                if let Err(e) = record(&conn, &rows, now) {
                    k2_core::log_debug!("[trends] record: {e}");
                }
            }));
            if res.is_err() {
                k2_core::log_debug!("[trends] a round panicked; the next runs in 60 s");
            }
            std::thread::sleep(TICK);
        }
    });
}

// ── the route ────────────────────────────────────────────────────────

/// Averages and peaks of one series.
fn summary(points: &[Point]) -> J {
    let cpu: Vec<f64> = points.iter().filter_map(|p| p.cpu_cores).collect();
    let mem: Vec<u64> = points.iter().filter_map(|p| p.mem_bytes).collect();
    let peak_mem = points.iter().filter_map(|p| p.mem_peak_bytes.or(p.mem_bytes)).max();
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    json!({
        "cpuAvgCores": (!cpu.is_empty()).then(|| round(cpu.iter().sum::<f64>() / cpu.len() as f64)),
        "cpuPeakCores": cpu.iter().cloned().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))),
        "memAvgBytes": (!mem.is_empty()).then(|| mem.iter().sum::<u64>() / mem.len() as u64),
        "memPeakBytes": peak_mem,
        "last": points.last(),
    })
}

/// GET `/cli/app/resources/trends [app, window]`.
pub(crate) fn route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    super::resources::need_reader(who)?;
    let w = params.get("window").map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_WINDOW.into());
    let Some(&(window, secs)) = WINDOWS.iter().find(|(n, _)| *n == w) else {
        return Err(no(
            k2_core::deploy::codes::BAD_REQUEST,
            400,
            format!("window is one of {}", WINDOWS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")),
        ));
    };
    let app = params.get("app").map(|a| a.trim().to_ascii_lowercase()).filter(|a| !a.is_empty());
    let now = k2_core::deploy::now();
    let res = rs::res_for_window(secs);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let apps = store::list_apps(&conn, None).map_err(db_err)?;
    if let Some(a) = &app {
        if !apps.iter().any(|r| &r.app_id == a) {
            return Err(no(k2_core::deploy::codes::APP_NOT_FOUND, 404, format!("no App '{a}' on this box (k2 app list)")));
        }
    }
    let all = rs::series(&conn, res, now - secs, None).map_err(db_err)?;
    let owner = |ws: &str| {
        let m = super::coordination::maintainer_with(&conn, ws);
        json!({ "kind": m.kind, "handle": m.handle, "address": m.address })
    };
    let box_points = all.get(&(Kind::Box, rs::BOX_ID.to_string())).cloned().unwrap_or_default();
    let mut app_rows = Vec::new();
    for a in apps.iter().filter(|a| app.as_ref().is_none_or(|x| x == &a.app_id)) {
        let points = all.get(&(Kind::App, a.app_id.clone())).cloned().unwrap_or_default();
        if points.is_empty() && app.is_none() {
            continue;
        }
        app_rows.push(json!({
            "app": a.app_id,
            "state": a.state,
            "owner": owner(&a.owner_workspace),
            "memBudgetBytes": a.memory_limit_bytes.and_then(|b| u64::try_from(b).ok()),
            "summary": summary(&points),
            "points": points,
        }));
    }
    let mut services = Vec::new();
    if app.is_none() {
        for ((k, id), points) in &all {
            if *k != Kind::Service {
                continue;
            }
            let (ws, name) = id.split_once('/').unwrap_or(("", id.as_str()));
            services.push(json!({ "service": name, "owner": owner(ws), "summary": summary(points), "points": points }));
        }
    }
    let (total, _) = dg::box_mem();
    Ok(json!({
        "ok": true,
        "window": window,
        "resSecs": res,
        "from": now - secs,
        "to": now,
        "box": { "cores": dg::cores(), "memTotalBytes": total, "summary": summary(&box_points), "points": box_points },
        "apps": app_rows,
        "services": services,
        "retention": { "minutesSecs": rs::KEEP_MINUTES_SECS, "hoursSecs": rs::KEEP_HOURS_SECS },
        "budgets": {
            "enforced": false,
            "note": "Each App's RAM budget is shown beside its memory; nothing here stops an App for using more.",
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_gives_session_rss_and_cpu() {
        // pid (comm with ) and spaces) state ppid pgrp session tty tpgid flags
        // minflt cminflt majflt cmajflt utime stime cutime cstime prio nice
        // threads itreal starttime vsize rss …
        let stat = "4242 (node (x) y) S 1 4242 4200 0 -1 4194560 10 0 0 0 150 50 0 0 20 0 7 0 12345 999999 321 18446744073709551615";
        assert_eq!(parse_proc_stat(stat), Some((4200, 321, 200)));
        assert_eq!(parse_proc_stat("garbage"), None);
    }

    fn get(who: &Who, q: &[(&str, &str)]) -> (u16, J) {
        let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = super::super::routes::handle(ROUTE, who.clone(), b"", &p);
        let st: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
        (st, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
    }

    /// The sampler's round lands in the table, and the route reads it back
    /// per window: the box, each App with its RAM budget beside its use,
    /// a summary, and refusals for a bad window, an unknown App, a Member.
    #[test]
    fn samples_land_and_the_route_reads_them_per_window() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        k2_core::deploy::set_enabled(true);
        let now = k2_core::deploy::now();
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('ws-t', 'Agent', '/tmp/ws-t', 'hand-t')", []).unwrap();
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: "jobs",
                    service_id: "svc-jobs-t",
                    seed_id: None,
                    owner_workspace: "ws-t",
                    isolation: "process",
                    memory_limit_bytes: Some(512 << 20),
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        // A real round: the box row (nothing runs, so no App row).
        let mut c = Counters::default();
        let rows = sample(&mut c);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!((rows[0].0, rows[0].1.id.as_str()), (Kind::Box, rs::BOX_ID));
        assert_eq!(rows[0].1.cpu_cores, None, "the first round is the CPU baseline");
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            record(&conn, &rows, now - 120).unwrap();
            let app = |cpu: f64, mem: u64| {
                (Kind::App, Sample { id: "jobs".into(), cpu_cores: Some(cpu), mem_bytes: Some(mem), mem_budget_bytes: Some(512 << 20), ..Sample::default() })
            };
            record(&conn, &[app(0.25, 100 << 20)], now - 120).unwrap();
            record(&conn, &[app(0.75, 300 << 20), (Kind::Service, Sample { id: "ws-t/site".into(), mem_bytes: Some(1 << 20), ..Sample::default() })], now - 60)
                .unwrap();
        }
        let (st, v) = get(&Who::Owner, &[("window", "1h")]);
        assert_eq!(st, 200, "{v}");
        assert_eq!((v["window"].as_str(), v["resSecs"].as_i64()), (Some("1h"), Some(60)));
        assert_eq!(v["box"]["points"].as_array().unwrap().len(), 1, "{v}");
        let jobs = &v["apps"][0];
        assert_eq!(jobs["app"], "jobs");
        assert_eq!(jobs["memBudgetBytes"], 512u64 << 20, "the budget beside the use (read only)");
        assert_eq!(jobs["owner"]["handle"], "hand-t");
        assert_eq!(jobs["points"].as_array().unwrap().len(), 2);
        assert_eq!(jobs["summary"]["cpuAvgCores"], 0.5);
        assert_eq!(jobs["summary"]["cpuPeakCores"], 0.75);
        assert_eq!(jobs["summary"]["memPeakBytes"], 300u64 << 20);
        assert_eq!(v["services"][0]["service"], "site", "{v}");
        assert!(v["services"][0]["summary"]["cpuAvgCores"].is_null(), "unknown CPU is null, never 0");
        assert!(!v.to_string().contains("ws-t/"), "a service shows its name, never its workspace id");
        assert_eq!(v["budgets"]["enforced"], false);
        let (st, v) = get(&Who::Owner, &[("window", "30d"), ("app", "jobs")]);
        assert_eq!((st, v["resSecs"].as_i64()), (200, Some(3600)), "{v}");
        assert!(v["services"].as_array().unwrap().is_empty(), "one App asked for: no services");
        let (st, v) = get(&Who::Owner, &[("window", "2h")]);
        assert_eq!((st, v["error"]["code"].as_str()), (400, Some("bad_request")), "{v}");
        let (st, v) = get(&Who::Owner, &[("app", "nope")]);
        assert_eq!((st, v["error"]["code"].as_str()), (404, Some("app_not_found")), "{v}");
        let member = Who::Login { username: "m".into(), role: k2_core::connect_users::Role::Member };
        let (st, v) = get(&member, &[]);
        assert_eq!((st, v["error"]["code"].as_str()), (403, Some("role_required")), "{v}");
        let (st, _) = get(&Who::Agent { project_id: "ws-t".into(), session_id: "s".into() }, &[]);
        assert_eq!(st, 200, "agents read the trends like the view");
    }

    #[test]
    fn counters_turn_cpu_time_into_cores_and_forget_stopped_apps() {
        let mut c = Counters::default();
        let t0 = Instant::now();
        assert_eq!(c.cores(Kind::App, "jobs", Some(1_000_000), t0), None, "the first reading is the baseline");
        let t1 = t0 + Duration::from_secs(60);
        assert_eq!(c.cores(Kind::App, "jobs", Some(31_000_000), t1), Some(0.5), "30 s of CPU in 60 s");
        assert_eq!(c.cores(Kind::App, "jobs", None, t1), None, "no counter, no number");
        c.retain(&[]);
        assert_eq!(c.cores(Kind::App, "jobs", Some(40_000_000), t1 + Duration::from_secs(60)), None, "a forgotten App starts again");
    }
}
