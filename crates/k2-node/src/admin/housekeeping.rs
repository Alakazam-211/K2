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

use crate::node::{journal_path, Node};

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

/// One row per running job.
fn running_rows(node: &Node) -> Vec<(crate::ledger::Row, Option<i64>)> {
    let ids = node.running_ids();
    let l = node.ledger.lock().unwrap();
    let mut out = Vec::new();
    for id in ids {
        if let Ok(Some(r)) = l.latest(&id) {
            let pid = l.pid(&r.job_id, r.generation).ok().flatten();
            out.push((r, pid));
        }
    }
    out
}

/// Tracks CPU per job between calls (for the stuck watch).
#[derive(Default)]
pub struct StuckTrack(std::sync::Mutex<HashMap<(String, u32), (u64, i64)>>);

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
    track.retain(|k, _| rows.iter().any(|(r, _)| r.job_id == k.0 && r.generation == k.1));
    let mut out = Vec::new();
    for (r, pid) in rows {
        let key = (r.job_id.clone(), r.generation);
        let c = pid.and_then(|p| cpu.get(&(p as i32)).copied()).unwrap_or(0);
        let entry = track.entry(key).or_insert((c, now));
        if entry.0 != c {
            *entry = (c, now);
        }
        let last_out = mtime(&journal_path(node.layout(), &r.job_id, r.generation)).or(r.started_at).unwrap_or(now);
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
        .map(|(r, pid)| {
            let last_out = mtime(&journal_path(node.layout(), &r.job_id, r.generation));
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
    let row = running_rows(node).into_iter().find(|(r, _)| r.job_id == job).map(|(r, _)| r);
    let r = row.ok_or_else(|| bad("that job is not running on this node"))?;
    node.cancel(&r.job_id, r.generation, "admin_kill");
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
    let cutoff = node.now() - days * 86400;
    let (mut freed, mut job_dirs, mut slots, mut caches) = (0u64, 0u32, 0u32, 0u32);
    // Job trees: ended (terminal) before the cutoff.
    let rows = node.ledger.lock().unwrap().all().unwrap_or_default();
    let running = node.running_ids();
    for r in &rows {
        if !r.state.is_terminal() || r.cleaned || running.contains(&r.job_id) || r.ended_at.is_none_or(|t| t > cutoff) {
            continue;
        }
        let dir = node.layout().job_dir(&r.job_id, r.generation);
        if dir.exists() {
            remove(&dir, &mut freed, &mut job_dirs);
        }
        let _ = node.ledger.lock().unwrap().mark_cleaned(&r.job_id, r.generation);
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

#[cfg(test)]
mod tests {
    use super::*;

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
