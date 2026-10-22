//! Housekeeping verbs the node runs itself, from the fixed list
//! (prd-k2-node-admin-v1 §7.6, S9): `ps`, `jobs.kill`, `gc`, and the
//! stuck-job watch (no output and no CPU for a while → an alert in
//! health, never a kill). All of it is the node user's own files and
//! processes; nothing here needs root.

use std::collections::HashMap;
use std::path::Path;

use k2_node_proto::admin::{code, AdminRefusal};
use k2_node_proto::health::StuckJob;
use serde_json::{json, Value};

use crate::node::{Node, Tenant};

/// A job counts as stuck after this long with no output and no CPU.
pub const STUCK_SECS: i64 = 30 * 60;
/// `gc` default and floor for "older than N days".
pub const GC_DEFAULT_DAYS: i64 = 3;

fn bad(m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(code::BAD_ARGS, m)
}

/// `ps -A -o pgid=,time=` → CPU seconds per process group.
pub fn parse_ps_cpu(text: &str) -> HashMap<i32, u64> {
    let mut out: HashMap<i32, u64> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(g), Some(t)) = (it.next(), it.next()) else { continue };
        let (Ok(g), Some(secs)) = (g.parse::<i32>(), parse_cpu_time(t)) else { continue };
        *out.entry(g).or_default() += secs;
    }
    out
}

/// `[[dd-]hh:]mm:ss[.cc]` → seconds.
pub fn parse_cpu_time(t: &str) -> Option<u64> {
    let (days, rest) = match t.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, t),
    };
    let rest = rest.split('.').next()?;
    let parts: Vec<u64> = rest.split(':').map(|p| p.parse::<u64>().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(days * 86400 + secs)
}

fn cpu_by_group() -> HashMap<i32, u64> {
    let mut c = std::process::Command::new("/bin/ps");
    c.args(["-A", "-o", "pgid=,time="]).env_clear().env("LANG", "C");
    crate::util::output_limited(c, std::time::Duration::from_secs(10)).map(|t| parse_ps_cpu(&t)).unwrap_or_default()
}

fn mtime(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// One row per running job, every tenant's (with its tenant).
fn running_rows(node: &Node) -> Vec<(std::sync::Arc<Tenant>, crate::ledger::Row, Option<i64>)> {
    let running = node.running_by_tenant();
    let mut out = Vec::new();
    for t in node.tenants() {
        let Some(ids) = running.get(&t.slot) else { continue };
        let l = t.ledger.lock().unwrap();
        for id in ids {
            if let Ok(Some(r)) = l.latest(id) {
                let pid = l.pid(&r.job_id, r.generation).ok().flatten();
                out.push((t.clone(), r, pid));
            }
        }
    }
    out
}

/// Tracks CPU per job between calls (for the stuck watch), by
/// `(slot, job_id, generation)`.
#[derive(Default)]
pub struct StuckTrack(std::sync::Mutex<HashMap<StuckKey, (u64, i64)>>);

/// `(slot, job_id, generation)`.
type StuckKey = (u8, String, u32);

/// Jobs quiet (no journal write) and idle (no CPU change) ≥ `STUCK_SECS`.
pub fn stuck_jobs(node: &Node) -> Vec<StuckJob> {
    let now = node.now();
    let rows = running_rows(node);
    if rows.is_empty() {
        node.stuck_track.0.lock().unwrap().clear();
        return vec![];
    }
    let cpu = cpu_by_group();
    let mut track = node.stuck_track.0.lock().unwrap();
    track.retain(|k, _| rows.iter().any(|(t, r, _)| t.slot == k.0 && r.job_id == k.1 && r.generation == k.2));
    let mut out = Vec::new();
    for (t, r, pid) in rows {
        let key = (t.slot, r.job_id.clone(), r.generation);
        let c = pid.and_then(|p| cpu.get(&(p as i32)).copied()).unwrap_or(0);
        let entry = track.entry(key).or_insert((c, now));
        if entry.0 != c {
            *entry = (c, now);
        }
        let last_out = mtime(&t.paths.journal(&r.job_id, r.generation)).or(r.started_at).unwrap_or(now);
        let quiet = now - last_out.max(entry.1);
        if now - last_out >= STUCK_SECS && now - entry.1 >= STUCK_SECS {
            out.push(StuckJob { job_id: r.job_id.clone(), generation: r.generation, quiet_secs: quiet.max(0) as u64 });
        }
    }
    out
}

/// `ps`: the node's running jobs.
pub fn ps(node: &Node) -> Value {
    let now = node.now();
    let cpu = cpu_by_group();
    let rows: Vec<Value> = running_rows(node)
        .into_iter()
        .map(|(t, r, pid)| {
            let last_out = mtime(&t.paths.journal(&r.job_id, r.generation));
            json!({
                "job_id": r.job_id, "generation": r.generation, "state": r.state.as_str(),
                "workspace": r.plan.workspace_label, "requested_by": r.plan.requested_by,
                "argv0": r.plan.argv.first(), "started_at": r.started_at, "pid": pid,
                "cpu_secs": pid.and_then(|p| cpu.get(&(p as i32)).copied()),
                "quiet_secs": last_out.map(|t| (now - t).max(0)), "log_seq": r.log_seq,
            })
        })
        .collect();
    json!({"jobs": rows, "stuck": stuck_jobs(node)})
}

/// `jobs.kill {job_id}`: stop a K2 job's process group (as a cancel).
pub fn kill(node: &Node, args: &Value) -> Result<Value, AdminRefusal> {
    super::exec::only_keys(args, &["job_id"])?;
    let job = super::exec::str_arg(args, "job_id")?;
    let row = running_rows(node).into_iter().find(|(_, r, _)| r.job_id == job).map(|(t, r, _)| (t, r));
    let (t, r) = row.ok_or_else(|| bad("that job is not running on this node"))?;
    node.cancel(&t, &r.job_id, r.generation, "admin_kill");
    Ok(json!({"job_id": r.job_id, "generation": r.generation, "stopping": true}))
}

fn dir_bytes(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| dir_bytes(&e.path())).sum()).unwrap_or(0)
}

fn remove(p: &Path, freed: &mut u64, n: &mut u32) {
    let b = dir_bytes(p);
    if std::fs::remove_dir_all(p).is_ok() {
        *freed += b;
        *n += 1;
    }
}

/// `gc {older_days?, toolchains?}`: job trees and idle warm slots older
/// than N days (default 3, at least 1); download caches with
/// `toolchains: true`. Never touches a running job or a busy slot.
pub async fn gc(node: &Node, args: &Value) -> Result<Value, AdminRefusal> {
    super::exec::only_keys(args, &["older_days", "toolchains"])?;
    let days = match args.get("older_days") {
        None => GC_DEFAULT_DAYS,
        Some(v) => v.as_i64().filter(|d| *d >= 1).ok_or_else(|| bad("older_days must be an integer ≥ 1"))?,
    };
    let toolchains = match args.get("toolchains") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad("toolchains must be true or false"))?,
    };
    gc_run(node, days, toolchains).await
}

/// The `gc` work. `days` 0 (only the owner's own `k2-node gc
/// --older-days 0`) takes every ended job's tree and every idle warm slot.
pub async fn gc_run(node: &Node, days: i64, toolchains: bool) -> Result<Value, AdminRefusal> {
    let cutoff = node.now() - days.max(0) * 86400;
    let (mut freed, mut job_dirs, mut slots, mut caches) = (0u64, 0u32, 0u32, 0u32);
    // Job trees: ended (terminal) before the cutoff, every tenant's.
    let running_by = node.running_by_tenant();
    let running = node.running_ids();
    for t in node.tenants() {
        let rows = t.ledger.lock().unwrap().all().unwrap_or_default();
        let mine = running_by.get(&t.slot).cloned().unwrap_or_default();
        for r in &rows {
            if !r.state.is_terminal() || r.cleaned || mine.contains(&r.job_id) || r.ended_at.is_none_or(|e| e > cutoff) {
                continue;
            }
            let dir = t.paths.job_dir(&r.job_id, r.generation);
            if dir.exists() {
                node.reclaim_blocking(&dir);
                remove(&dir, &mut freed, &mut job_dirs);
            }
            let _ = t.ledger.lock().unwrap().mark_cleaned(&r.job_id, r.generation);
        }
    }
    // Idle warm slots.
    for (dir, bytes) in node.gc_slots(cutoff) {
        freed += bytes;
        slots += 1;
        let _ = dir;
    }
    if toolchains {
        if !running.is_empty() {
            return Err(AdminRefusal::new(code::BUSY, "jobs are running; clear toolchain caches when the node is idle"));
        }
        let t = node.layout().toolchains();
        for d in [t.join("mise-cache"), t.join("cargo/registry/cache")] {
            if d.exists() {
                remove(&d, &mut freed, &mut caches);
            }
        }
    }
    Ok(json!({"older_days": days, "freed_bytes": freed, "job_dirs": job_dirs, "slots": slots, "caches": caches}))
}

/// `k2-node gc` on the machine: the owner (a `k2nodectl` member, or
/// root) writes this into the config dir; the running node picks it up
/// within seconds, runs `gc` and answers in `run/gc-result.json`. Each
/// request id runs once.
pub const GC_REQUEST_FILE: &str = "gc-request.json";
pub const GC_RESULT_FILE: &str = "gc-result.json";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GcRequest {
    pub id: String,
    /// Ended jobs and idle warm slots older than this many days; 0 = all.
    pub older_days: i64,
    #[serde(default)]
    pub toolchains: bool,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub requested_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GcOutcome {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub message: Option<String>,
    pub at: i64,
}

pub fn gc_request_path(layout: &crate::paths::Layout) -> std::path::PathBuf {
    layout.config.join(GC_REQUEST_FILE)
}

pub fn gc_result_path(layout: &crate::paths::Layout) -> std::path::PathBuf {
    layout.run().join(GC_RESULT_FILE)
}

pub fn read_gc_result(layout: &crate::paths::Layout) -> Option<GcOutcome> {
    serde_json::from_str(&std::fs::read_to_string(gc_result_path(layout)).ok()?).ok()
}

/// `k2-node gc …`: write the request (the config dir is the owner's).
pub fn write_gc_request(layout: &crate::paths::Layout, r: &GcRequest) -> Result<(), String> {
    if r.older_days < 0 {
        return Err("--older-days must be 0 or more".into());
    }
    let body = serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&gc_request_path(layout), &body, 0o660)
        .map_err(|e| format!("{e}\n{} is the machine owner's: run this as root (sudo) or as a member of the k2nodectl group", layout.config.display()))
}

/// Run the waiting `k2-node gc` request once (the config poll calls
/// this). True when one ran (or a bad one was answered).
pub async fn process_gc_request(node: &Node) -> bool {
    let layout = node.layout().clone();
    let Ok(text) = std::fs::read_to_string(gc_request_path(&layout)) else { return false };
    let at = node.now();
    let last = read_gc_result(&layout).map(|o| o.id);
    let (id, outcome) = match serde_json::from_str::<GcRequest>(&text) {
        Ok(r) if last.as_deref() == Some(r.id.as_str()) => return false,
        Ok(r) if r.older_days < 0 => (r.id.clone(), Err("older_days must be 0 or more".to_string())),
        Ok(r) => {
            crate::nlog!("gc request {} from {}: older than {} day(s){}", r.id, r.requested_by.clone().unwrap_or_default(), r.older_days, if r.toolchains { ", toolchains" } else { "" });
            (r.id.clone(), gc_run(node, r.older_days, r.toolchains).await.map_err(|e| e.message))
        }
        Err(e) => {
            let id = format!("bad-{}", &k2_node_proto::crypto::sha256_hex(text.as_bytes())[..16]);
            if last.as_deref() == Some(id.as_str()) {
                return false;
            }
            (id, Err(format!("{GC_REQUEST_FILE}: {e}")))
        }
    };
    let o = match outcome {
        Ok(v) => GcOutcome { id, ok: true, result: Some(v), message: None, at },
        Err(m) => GcOutcome { id, ok: false, result: None, message: Some(m), at },
    };
    match serde_json::to_vec_pretty(&o) {
        Ok(b) => {
            if let Err(e) = crate::util::atomic_write(&gc_result_path(&layout), &b, 0o644) {
                crate::nlog!("gc result: {e}");
            }
        }
        Err(e) => crate::nlog!("gc result: {e}"),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (e) The owner's `k2-node gc --older-days 0`: the running node takes
    /// every ended job's tree and idle warm slot, answers once per id, and
    /// leaves a running job alone.
    #[tokio::test]
    async fn the_owners_gc_request_runs_once_and_spares_running_work() {
        let d = crate::util::temp_dir("gc-req");
        let layout = crate::paths::Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        std::fs::write(layout.policy(), "availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\nmax_parallel = 4\n").unwrap();
        crate::identity::write_pin(&layout.pin(), &crate::identity::Pin::for_test("n")).unwrap();
        let mut o = crate::node::NodeOptions::new(layout.clone());
        o.dev = true;
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.cancel_grace = std::time::Duration::from_millis(200);
        o.wall = Some(crate::wall::Wall::None { why: "t".into() });
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") });
        let n = Node::open(o).await.unwrap();
        // An ended job's tree, an idle warm slot, and a running job.
        let old = layout.job_dir("old", 1);
        std::fs::create_dir_all(old.join("home")).unwrap();
        std::fs::write(old.join("log"), "x").unwrap();
        {
            let owner = n.owner();
            let l = owner.ledger.lock().unwrap();
            l.insert(&crate::ledger::tests::assign("old", 1), &*n.opts.clock).unwrap();
            l.finish("old", 1, k2_node_proto::frames::JobState::Done, None, None, None, 0, n.now()).unwrap();
        }
        let slot = layout.project_cache("pk1").join("target-1");
        std::fs::create_dir_all(slot.join("debug")).unwrap();
        std::fs::write(slot.join("debug/app"), vec![0u8; 4096]).unwrap();
        let mut a = crate::ledger::tests::assign("live", 1);
        a.plan.argv = vec!["/bin/sleep".into(), "30".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&n.owner(), a), Some(("live".into(), 1)));
        let t0 = std::time::Instant::now();
        while !layout.job_dir("live", 1).join("home").exists() {
            assert!(t0.elapsed() < std::time::Duration::from_secs(20), "the live job never started");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!process_gc_request(&n).await, "no request: nothing runs");
        let req = GcRequest { id: "cli-0001".into(), older_days: 0, toolchains: false, requested_by: Some("owner".into()), requested_at: Some(1) };
        write_gc_request(&layout, &req).unwrap();
        assert!(process_gc_request(&n).await);
        let out = read_gc_result(&layout).unwrap();
        assert!(out.ok, "{out:?}");
        assert_eq!(out.id, "cli-0001");
        let r = out.result.unwrap();
        assert_eq!((r["job_dirs"].as_u64(), r["slots"].as_u64()), (Some(1), Some(1)), "{r}");
        assert!(!old.exists() && !slot.exists());
        assert!(layout.job_dir("live", 1).exists(), "a running job's tree stays");
        assert!(!process_gc_request(&n).await, "the same id runs once");
        // A bad request is answered (once), not run.
        std::fs::write(gc_request_path(&layout), "{\"id\":\"x\",\"older_days\":-1}").unwrap();
        assert!(process_gc_request(&n).await);
        let out = read_gc_result(&layout).unwrap();
        assert!(!out.ok && out.message.unwrap().contains("0 or more"));
        assert!(!process_gc_request(&n).await, "answered once");
        assert!(write_gc_request(&layout, &GcRequest { older_days: -1, ..req }).is_err());
        n.stop_all(k2_node_proto::frames::JobState::Cancelled, "test_over");
    }

    #[test]
    fn cpu_times_parse() {
        assert_eq!(parse_cpu_time("0:01.50"), Some(1));
        assert_eq!(parse_cpu_time("12:34"), Some(754));
        assert_eq!(parse_cpu_time("01:02:03"), Some(3723));
        assert_eq!(parse_cpu_time("2-01:00:00"), Some(2 * 86400 + 3600));
        assert_eq!(parse_cpu_time("x"), None);
        let m = parse_ps_cpu("  100  0:01.00\n  100  1:00.00\n  7 0:00.00\nbad line\n");
        assert_eq!(m.get(&100), Some(&61));
        assert_eq!(m.get(&7), Some(&0));
    }
}
