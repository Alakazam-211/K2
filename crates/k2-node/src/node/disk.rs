//! The disk guard: a node running Rust builds never fills its disk.
//!
//! K2 Web's numbers (0.46.2 minis, 2026-10-21): an ended job's dir kept
//! its job-private `target` (155–165 GB each, `jobs/` 213–273 GB a node),
//! the one warm `target-1` grew to ~160 GB, the floor was checked only
//! when a job started, and a job that died on a full disk stayed
//! `running` (its ledger finish failed for want of space). So:
//!
//! - **Job end** ([`Node::strip_job_dir`]): the big parts of an ended
//!   job's dir go at once (`target`, `src`, `cache`, `tmp`, `dirty`);
//!   `home` and the owner's `log` stay for the janitor (24 h).
//! - **Warm slots** are capped ([`warm_cap`]: half the policy's
//!   `disk_budget_bytes`, all slots together): a slot over the cap is
//!   evicted when its job ends, then idle slots least recently used first.
//! - **The guard pass** ([`Node::disk_guard_pass`], every `disk_poll`):
//!   under the floor plus [`RECLAIM_MARGIN`] it reclaims (leftover
//!   evictions, ended job dirs, idle warm slots oldest first); still under
//!   the floor, it stops the newest running job (`failed / disk_low`).
//!   Admission already refuses under the floor (`disk_low`).
//! - **Finish retries** (run.rs): a ledger finish that fails is retried
//!   after the job's dir is stripped, so a job never stays `running`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use k2_node_proto::frames::JobState;
use k2_node_proto::refusal;

use super::{Node, StopReason, Tenant};
use crate::sync::Git;
use crate::wall::Wall;

/// What of an ended job's dir stays until the janitor: the job's own
/// `home` (small; what it wrote for itself) and the owner's journal.
pub const JOB_END_KEEP: &[&str] = &["home", "log"];
/// Under disk pressure only the journal stays (it may not be shipped yet).
pub const PRESSURE_KEEP: &[&str] = &["log"];
/// Reclaim starts this far above the floor, so jobs aren't stopped first.
pub const RECLAIM_MARGIN: u64 = 10 << 30;
/// An evicted slot is renamed to `.evicted-…` under the slot lock and
/// deleted outside it; a leftover one (a crash mid-delete) goes next pass.
const EVICTED_PREFIX: &str = ".evicted-";

/// A warm slot's last measured size and use (node clock).
#[derive(Debug, Clone, Copy, Default)]
pub struct SlotUse {
    pub bytes: u64,
    pub used_at: i64,
}

/// All warm target slots together may hold at most half the disk budget.
pub fn warm_cap(policy: &crate::config::Policy) -> u64 {
    policy.disk_budget_bytes / 2
}

/// One idle warm slot: its server's slot, project cache, project key,
/// number and dir.
struct Idle {
    slot: k2_node_proto::share::Slot,
    project: PathBuf,
    key: String,
    n: u32,
    dir: PathBuf,
}

/// A real directory, not a link to one (review A-disk 1: listings never
/// follow a link a job planted).
fn real_dir(e: &std::fs::DirEntry) -> bool {
    e.file_type().is_ok_and(|f| f.is_dir())
}

/// Delete a secret attempt's job folder, all but its node-only journal
/// (`log`, 0600) (review B-H1). Under the Linux wall each subfolder is first
/// given back to the node by the executor (`Reclaim`, never through a
/// link), as the end-of-job strip does. Blocking. `Err` unless only `log`
/// is left (or the folder is gone): the caller keeps the slot held and
/// tries again; it never frees a slot over a failed delete.
pub(crate) fn delete_secret_tree(admin_socket: &Path, walled: bool, slot: k2_node_proto::share::Slot, root: &Path) -> Result<(), String> {
    let meta = match std::fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", root.display())),
    };
    if !meta.is_dir() {
        return Err(format!("{} isn't a plain folder", root.display()));
    }
    let entries: Vec<std::fs::DirEntry> = std::fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?.flatten().filter(|e| e.file_name() != "log").collect();
    for e in &entries {
        let p = e.path();
        if real_dir(e) {
            if walled {
                let op = crate::admin::jobs::JobOp::Reclaim { path: p.clone(), slot };
                crate::admin::server::job_request(admin_socket, &op, &[], Duration::from_secs(600)).map_err(|x| format!("reclaim {}: {}: {}", p.display(), x.code, x.message))?;
            }
            std::fs::remove_dir_all(&p).map_err(|x| format!("delete {}: {x}", p.display()))?;
        } else {
            std::fs::remove_file(&p).map_err(|x| format!("delete {}: {x}", p.display()))?;
        }
    }
    let left: Vec<String> = std::fs::read_dir(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "log")
        .collect();
    if !left.is_empty() {
        return Err(format!("still in {}: {}", root.display(), left.join(", ")));
    }
    Ok(())
}

/// The job units that are live on this machine right now (loaded and
/// active, activating or deactivating), from `systemctl list-units` as the
/// node user (review round 2, item 4: holds also come from what runs, not
/// only from the ledger). `None` when systemctl can't say.
pub(crate) fn live_job_units() -> Option<Vec<String>> {
    let text = crate::admin::jobs::systemctl(&["list-units", "--all", "--plain", "--no-legend", "--no-pager", "k2job-*"]).ok()?;
    Some(parse_live_units(&text))
}

pub(crate) fn parse_live_units(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cols = line.split_whitespace().filter(|c| *c != "●" && *c != "*");
        let (Some(unit), Some(_load), Some(active)) = (cols.next(), cols.next(), cols.next()) else { continue };
        if !crate::admin::jobs::valid_unit(unit) || unit.starts_with("k2job-selftest-") {
            continue;
        }
        if matches!(active, "active" | "activating" | "deactivating" | "reloading") {
            out.push(unit.to_string());
        }
    }
    out
}

/// Bytes on disk under `p` (allocated blocks, no link followed).
fn bytes_of(p: &Path) -> u64 {
    super::run::disk_usage(p)
}

impl Node {
    /// Free bytes on the node's disk (`disk_free` fakes it in tests).
    pub fn disk_free_bytes(&self) -> u64 {
        match &self.opts.disk_free {
            Some(f) => f(),
            None => crate::sysinfo::disk(&self.layout().home).0,
        }
    }

    /// Remove everything in an ended job's dir but `keep` (the job's code
    /// worktree through git). Linux wall: the job's trees come back to
    /// the node first (only real dirs: a link is removed as a link, never
    /// handed to root). Returns the entries removed.
    pub(crate) async fn strip_job_dir(&self, t: &Tenant, root: &Path, project_key: Option<&str>, keep: &[&str]) -> usize {
        if !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
            return 0;
        }
        let Ok(rd) = std::fs::read_dir(root) else { return 0 };
        let entries: Vec<(PathBuf, bool)> = rd.flatten().filter(|e| !keep.iter().any(|k| e.file_name() == *k)).map(|e| (e.path(), real_dir(&e))).collect();
        if entries.is_empty() {
            return 0;
        }
        let doomed: Vec<PathBuf> = entries.iter().map(|(p, _)| p.clone()).collect();
        if matches!(self.wall, Wall::JobUser { .. }) {
            for (p, _) in entries.iter().filter(|(_, dir)| *dir) {
                let op = crate::admin::jobs::JobOp::Reclaim { path: p.clone(), slot: t.slot };
                if let Err(e) = self.job_op(op, vec![], Duration::from_secs(600)).await {
                    crate::nlog!("reclaim {}: {e}", p.display());
                }
            }
        }
        let src = root.join("src");
        // A row's key from before arrival checks may not be a plain name
        // (review B F5): then no git on another folder, only the delete.
        let project_key = project_key.filter(|k| k2_node_proto::frames::valid_project_key(k));
        if let (Some(pk), true) = (project_key, entries.iter().any(|(p, dir)| *dir && *p == src)) {
            if let Some(g) = Git::find(&self.layout().git_home(), &crate::util::base_path(&self.layout().home)) {
                g.worktree_remove(&t.paths.mirror(pk), &src).await;
            }
        }
        let mut n = 0;
        let list = doomed.clone();
        let removed = tokio::task::spawn_blocking(move || {
            let mut n = 0;
            for p in list {
                let r = match std::fs::symlink_metadata(&p) {
                    Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
                    Ok(_) => std::fs::remove_file(&p),
                    Err(_) => continue,
                };
                match r {
                    Ok(()) => n += 1,
                    Err(e) => crate::nlog!("remove {}: {e}", p.display()),
                }
            }
            n
        })
        .await;
        if let Ok(r) = removed {
            n = r;
        }
        n
    }

    /// A warm slot's job ended: measure it, then keep the warm slots
    /// under [`warm_cap`] (this one first when it alone is over).
    pub(crate) async fn after_slot_use(self: &Arc<Self>, dir: &Path) {
        let d = dir.to_path_buf();
        let bytes = tokio::task::spawn_blocking(move || bytes_of(&d)).await.unwrap_or(0);
        self.slot_use.lock().unwrap().insert(dir.to_path_buf(), SlotUse { bytes, used_at: self.now() });
        self.trim_warm_slots().await;
    }

    /// Evict idle warm slots until all slots together fit [`warm_cap`]:
    /// any one over the cap alone, then the least recently used.
    /// Returns the evicted dirs and their bytes.
    pub(crate) async fn trim_warm_slots(self: &Arc<Self>) -> Vec<(PathBuf, u64)> {
        let cap = warm_cap(&self.local.lock().unwrap().policy);
        let idle = self.idle_slots();
        // Measure idle slots we have no size for (made before this run).
        for s in &idle {
            if self.slot_use.lock().unwrap().contains_key(&s.dir) {
                continue;
            }
            let d = s.dir.clone();
            let bytes = tokio::task::spawn_blocking(move || bytes_of(&d)).await.unwrap_or(0);
            let used_at = std::fs::metadata(&s.dir).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|t| t.as_secs() as i64).unwrap_or(0);
            self.slot_use.lock().unwrap().insert(s.dir.clone(), SlotUse { bytes, used_at });
        }
        let uses = self.slot_use.lock().unwrap().clone();
        let mut total: u64 = uses.iter().filter(|(d, _)| d.exists()).map(|(_, u)| u.bytes).sum();
        let mut order: Vec<&Idle> = idle.iter().collect();
        // Over the cap alone first, then oldest use first.
        order.sort_by_key(|s| {
            let u = uses.get(&s.dir).copied().unwrap_or_default();
            (u.bytes <= cap, u.used_at)
        });
        let mut out = Vec::new();
        for s in order {
            let u = uses.get(&s.dir).copied().unwrap_or_default();
            if total <= cap && u.bytes <= cap {
                continue;
            }
            if self.evict_slot(s).await {
                crate::nlog!("warm slot {} evicted: {} GB (warm cap {} GB)", s.dir.display(), u.bytes >> 30, cap >> 30);
                total = total.saturating_sub(u.bytes);
                out.push((s.dir.clone(), u.bytes));
            }
        }
        out
    }

    /// Every warm slot no job holds right now, in every tenant's cache.
    fn idle_slots(&self) -> Vec<Idle> {
        let slots = self.slots.lock().unwrap();
        let mut out = Vec::new();
        for t in self.tenants() {
            let Ok(rd) = std::fs::read_dir(t.paths.cache()) else { continue };
            for project in rd.flatten() {
                if !real_dir(&project) {
                    continue;
                }
                let key = project.file_name().to_string_lossy().into_owned();
                let Ok(inner) = std::fs::read_dir(project.path()) else { continue };
                for e in inner.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Some(n) = name.strip_prefix("target-").and_then(|n| n.parse::<u32>().ok()) else { continue };
                    if slots.is_busy(&key, n) || !real_dir(&e) {
                        continue;
                    }
                    out.push(Idle { slot: t.slot, project: project.path(), key: key.clone(), n, dir: e.path() });
                }
            }
        }
        out
    }

    /// Take one idle slot out of the pool (renamed under the slot lock, so
    /// no lease races it) and delete it. False when a job holds it now.
    async fn evict_slot(&self, s: &Idle) -> bool {
        let gone = {
            let slots = self.slots.lock().unwrap();
            if slots.is_busy(&s.key, s.n) {
                return false;
            }
            let to = s.project.join(format!("{EVICTED_PREFIX}target-{}-{}", s.n, k2_node_proto::crypto::random_id().unwrap_or_else(|_| self.now().to_string())));
            if let Err(e) = std::fs::rename(&s.dir, &to) {
                crate::nlog!("evict {}: {e}", s.dir.display());
                return false;
            }
            slots.forget(&s.project, s.n);
            to
        };
        self.slot_use.lock().unwrap().remove(&s.dir);
        self.delete_evicted(s.slot, gone).await;
        true
    }

    async fn delete_evicted(&self, slot: k2_node_proto::share::Slot, p: PathBuf) {
        if matches!(self.wall, Wall::JobUser { .. }) && std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            let op = crate::admin::jobs::JobOp::Reclaim { path: p.clone(), slot };
            if let Err(e) = self.job_op(op, vec![], Duration::from_secs(600)).await {
                crate::nlog!("reclaim {}: {e}", p.display());
            }
        }
        let shown = p.display().to_string();
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&p)).await {
            crate::nlog!("remove {shown}: {e}");
        }
    }

    /// Leftover `.evicted-…` dirs (a delete cut short) in every cache.
    async fn sweep_evicted(&self) {
        let mut found = Vec::new();
        for t in self.tenants() {
            let Ok(rd) = std::fs::read_dir(t.paths.cache()) else { continue };
            for project in rd.flatten().filter(real_dir) {
                let Ok(inner) = std::fs::read_dir(project.path()) else { continue };
                for e in inner.flatten() {
                    if e.file_name().to_string_lossy().starts_with(EVICTED_PREFIX) {
                        found.push((t.slot, e.path()));
                    }
                }
            }
        }
        for (slot, p) in found {
            self.delete_evicted(slot, p).await;
        }
    }

    /// Ended jobs' dirs, but for their journal (not yet cleaned by the
    /// janitor, not running). Returns the dirs touched.
    async fn strip_ended_job_dirs(&self) -> usize {
        let running = self.running_by_tenant();
        let mut n = 0;
        for t in self.tenants() {
            let rows = t.ledger.lock().unwrap().all().unwrap_or_default();
            let mine = running.get(&t.slot).cloned().unwrap_or_default();
            for r in rows {
                if !r.state.is_terminal() || r.cleaned || mine.contains(&r.job_id) || !k2_node_proto::frames::valid_job_id(&r.job_id) {
                    continue;
                }
                let dir = t.paths.job_dir(&r.job_id, r.generation);
                if self.strip_job_dir(&t, &dir, r.plan.src.as_ref().map(|s| s.project_key.as_str()), PRESSURE_KEEP).await > 0 {
                    n += 1;
                }
            }
        }
        n
    }

    /// One guard pass (see the module doc). Returns the jobs it stopped,
    /// `(slot, job_id)`.
    pub async fn disk_guard_pass(self: &Arc<Self>) -> Vec<(k2_node_proto::share::Slot, String)> {
        self.disk_guard_pass_with(|gb| u64::from(gb) << 30).await
    }

    /// The other server with running jobs furthest over its disk share
    /// (`disk_gb`, measured as the V74 scan does), with its overage.
    /// `budget` turns a share's GB into bytes (tests scale it down).
    async fn furthest_over_share(&self, budget: impl Fn(u32) -> u64) -> Option<(k2_node_proto::share::Slot, u64)> {
        let running = self.running_by_tenant();
        let mut worst: Option<(k2_node_proto::share::Slot, u64)> = None;
        for t in self.tenants() {
            // The owner is never ranked, by design: the box is theirs, and
            // their own jobs filling it may cost a tenant its job. Real
            // per-uid or project quotas (and ranking the owner) are 0.46.4
            // (security review B, F1).
            if t.is_owner() || !running.get(&t.slot).is_some_and(|v| !v.is_empty()) {
                continue;
            }
            let Ok(e) = crate::admin::jobs::tenant_entry(&self.layout().config, t.slot) else { continue };
            let share = budget(e.disk_gb);
            let root = t.paths.root();
            let (used, capped) = tokio::task::spawn_blocking(move || super::run::disk_usage_walk(&root)).await.unwrap_or((0, false));
            // A walk that hit its caps (data hidden past 256 levels or 2
            // million entries) counts as furthest over (review B F1).
            if capped {
                crate::nlog!("warning: server slot {}'s folder hit the disk walk's cap ({} GB counted so far); ranked furthest over its share", t.slot, used >> 30);
            }
            let over = if capped { u64::MAX } else { used.saturating_sub(share) };
            if over > 0 && worst.is_none_or(|(_, w)| over > w) {
                worst = Some((t.slot, over));
            }
        }
        worst
    }

    async fn disk_guard_pass_with(self: &Arc<Self>, budget: impl Fn(u32) -> u64) -> Vec<(k2_node_proto::share::Slot, String)> {
        let floor = self.local.lock().unwrap().policy.disk_floor_bytes;
        if floor == 0 {
            return vec![];
        }
        let mut free = self.disk_free_bytes();
        if free >= floor.saturating_add(RECLAIM_MARGIN) {
            return vec![];
        }
        self.sweep_evicted().await;
        free = self.disk_free_bytes();
        if free < floor.saturating_add(RECLAIM_MARGIN) {
            let n = self.strip_ended_job_dirs().await;
            if n > 0 {
                crate::nlog!("disk low ({} GB free, floor {} GB): stripped {n} ended job dir(s)", free >> 30, floor >> 30);
            }
            free = self.disk_free_bytes();
        }
        if free < floor.saturating_add(RECLAIM_MARGIN) {
            let mut idle = self.idle_slots();
            let uses = self.slot_use.lock().unwrap().clone();
            idle.sort_by_key(|s| uses.get(&s.dir).map(|u| u.used_at).unwrap_or(0));
            for s in &idle {
                if self.disk_free_bytes() >= floor.saturating_add(RECLAIM_MARGIN) {
                    break;
                }
                if self.evict_slot(s).await {
                    crate::nlog!("disk low: evicted warm slot {}", s.dir.display());
                }
            }
            free = self.disk_free_bytes();
        }
        if free >= floor {
            return vec![];
        }
        // Still under the floor: one job stops a pass (its dir goes when it
        // ends, then the next pass looks again). See [`disk_victim`].
        let over = self.furthest_over_share(budget).await;
        let jobs = self.jobs.lock().unwrap();
        let cands: Vec<Candidate> = jobs
            .iter()
            // A lingering unit's stand-in (review B-M2) isn't a job to stop.
            .filter(|((_, id), j)| j.stop.borrow().is_none() && !id.starts_with(super::LINGER_PREFIX))
            .map(|((slot, _), j)| Candidate { slot: *slot, seq: j.seq, gate: j.gate })
            .collect();
        let Some(pick) = disk_victim(&cands, over.map(|(s, _)| s)) else { return vec![] };
        let picked = cands[pick];
        let Some(((slot, job), j)) = jobs.iter().find(|((s, id), j)| *s == picked.slot && j.seq == picked.seq && j.stop.borrow().is_none() && !id.starts_with(super::LINGER_PREFIX)) else {
            return vec![];
        };
        let why = over.map(|(s, b)| format!(", server slot {s} is {} MB over its disk share", b >> 20)).unwrap_or_default();
        crate::nlog!("disk low ({} GB free, floor {} GB{why}): stopping job {job} (slot {slot})", free >> 30, floor >> 30);
        j.stop.send_replace(Some(StopReason { state: JobState::Failed, reason: refusal::DISK_LOW.into() }));
        vec![(*slot, job.clone())]
    }

    pub(super) fn spawn_disk_guard(self: &Arc<Self>) {
        let node = Arc::downgrade(self);
        let every = self.opts.disk_poll;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(n) = node.upgrade() else { return };
                n.reap_orphans();
                n.disk_guard_pass().await;
            }
        });
    }
}

/// A running job as the disk guard weighs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub slot: k2_node_proto::share::Slot,
    pub seq: u64,
    pub gate: bool,
}

/// Which running job the disk guard stops (an index into `cands`).
///
/// 1. A CI job (`gate`: a release build, plan-ci-release-on-k2-compute
///    G5) only when no other job is left: before 0.46.4 the newest job
///    paid, and a release build submitted last was exactly that job.
/// 2. Among the rest, the server furthest over its disk share pays first
///    (`over`), so no server fills the disk to get another's job stopped
///    (review A-disk 3)…
/// 3. …its newest job; with none over (or none of its jobs left), the
///    newest job on the node.
///
/// Priority and Normal are one band here (prd-compute-job-tiers-v1 T6):
/// Priority needs no permission, so sparing it would let any agent shield
/// its job by flagging it. The disk guard is a safety stop, not a tier
/// action.
pub fn disk_victim(cands: &[Candidate], over: Option<k2_node_proto::share::Slot>) -> Option<usize> {
    let any_plain = cands.iter().any(|c| !c.gate);
    let pool: Vec<usize> = (0..cands.len()).filter(|i| !any_plain || !cands[*i].gate).collect();
    let mine: Vec<usize> = pool.iter().copied().filter(|i| over.is_some_and(|s| cands[*i].slot == s)).collect();
    let from = if mine.is_empty() { &pool } else { &mine };
    from.iter().copied().max_by_key(|i| cands[*i].seq)
}

/// The `disk_low` stop's detail for the job's row and log.
pub fn disk_low_detail(free: u64, floor: u64) -> String {
    format!("free disk fell under the node's floor ({} GB free, floor {} GB); the job was stopped so the disk doesn't fill", free >> 30, floor >> 30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Layout;
    use std::sync::atomic::{AtomicU64, Ordering};

    const GIB: u64 = 1 << 30;

    /// G5 (plan-ci-release-on-k2-compute-v1): the release build is
    /// likely the newest job, and the guard used to stop the newest. A
    /// gate job is stopped only when nothing else is left.
    #[test]
    fn the_disk_guard_stops_a_gate_job_only_when_nothing_else_runs() {
        let c = |slot: k2_node_proto::share::Slot, seq: u64, gate: bool| Candidate { slot, seq, gate };
        // The gate is newest: the newest plain job pays instead.
        let cands = [c(0, 1, false), c(1, 2, false), c(1, 3, true)];
        assert_eq!(disk_victim(&cands, None), Some(1));
        // The server over its share pays first, its plain job…
        let cands = [c(0, 5, false), c(1, 2, false), c(1, 9, true)];
        assert_eq!(disk_victim(&cands, Some(1)), Some(1));
        // …and when its only job is a gate, another server's plain job
        // goes before any gate (the plan's rule: never a gate while a
        // batch job exists).
        let cands = [c(0, 5, false), c(1, 9, true)];
        assert_eq!(disk_victim(&cands, Some(1)), Some(0));
        // Only gates left: the over server's newest gate, else the newest.
        let cands = [c(0, 5, true), c(1, 4, true)];
        assert_eq!(disk_victim(&cands, Some(1)), Some(1));
        assert_eq!(disk_victim(&cands, None), Some(0));
        // Unchanged without gates: newest, or the over server's newest.
        let cands = [c(0, 7, false), c(2, 3, false), c(2, 4, false)];
        assert_eq!(disk_victim(&cands, None), Some(0));
        assert_eq!(disk_victim(&cands, Some(2)), Some(2));
        assert_eq!(disk_victim(&[], None), None);
    }

    async fn node(tag: &str, policy: &str, free: Arc<AtomicU64>) -> (Arc<Node>, Layout) {
        let d = crate::util::temp_dir(tag);
        let layout = Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        std::fs::write(layout.policy(), format!("availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\nmax_parallel = 4\n{policy}")).unwrap();
        crate::identity::write_pin(&layout.pin(), &crate::identity::Pin::for_test("n")).unwrap();
        let mut o = super::super::NodeOptions::new(layout.clone());
        o.dev = true;
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.cancel_grace = Duration::from_millis(200);
        o.wall = Some(Wall::None { why: "t".into() });
        // The guard loop isn't spawned here (passes are called by hand);
        // this paces the finish retries.
        o.disk_poll = Duration::from_millis(50);
        o.disk_free = Some(Arc::new(move || free.load(Ordering::SeqCst)));
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") });
        (Node::open(o).await.unwrap(), layout)
    }

    fn put(p: &Path, bytes: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![1u8; bytes]).unwrap();
    }

    fn sleeper(n: &Arc<Node>, id: &str) {
        let mut a = crate::ledger::tests::assign(id, 1);
        a.plan.argv = vec!["/bin/sleep".into(), "30".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&n.owner(), a), Some((id.to_string(), 1)));
    }

    async fn ended(n: &Arc<Node>, id: &str) -> crate::ledger::Row {
        let t0 = std::time::Instant::now();
        loop {
            let r = n.owner().ledger.lock().unwrap().get(id, 1).unwrap().unwrap();
            if r.state.is_terminal() && !n.running_ids().contains(&id.to_string()) && !n.layout().job_dir(id, 1).join("tmp").exists() {
                return r;
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "{id} never ended: {:?}", r.state);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// (a) An ended job's dir keeps only its `home` and journal: the
    /// job-private target, tmp and caches go when it ends.
    #[tokio::test]
    async fn a_job_dir_loses_its_target_when_the_job_ends() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-end", "disk_floor_gb = 0\n", free).await;
        let mut a = crate::ledger::tests::assign("big", 1);
        a.plan.argv = vec!["/bin/sh".into(), "-c".into(), "mkdir -p target/debug cache && head -c 65536 /dev/zero > target/debug/app && echo hi > cache/x && echo mine > \"$HOME/note\" && echo done".into()];
        // A declared output in the folder the strip takes: it's collected
        // (kept outside the job dir) before anything is removed.
        a.plan.artifacts = vec!["cache/x".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&n.owner(), a), Some(("big".into(), 1)));
        let r = ended(&n, "big").await;
        assert_eq!(r.state, JobState::Done, "{r:?}");
        let kept = r.outputs.as_ref().map(|o| o.artifacts.iter().map(|x| x.name.clone()).collect::<Vec<_>>()).unwrap_or_default();
        assert_eq!(kept, vec!["cache/x".to_string()], "{:?}", r.outputs);
        let copy = crate::artifacts::dir(&n.owner().paths.artifacts(), "big", 1);
        assert_eq!(std::fs::read_to_string(copy.join("cache/x")).unwrap(), "hi\n", "the kept copy survives the strip");
        let dir = layout.job_dir("big", 1);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, vec!["home".to_string(), "log".to_string()], "only home and the journal stay");
        assert_eq!(std::fs::read_to_string(dir.join("home/note")).unwrap(), "mine\n");
        let log = std::fs::read(dir.join("log")).unwrap();
        assert!(log.windows(4).any(|w| w == b"done"), "the journal is kept");
    }

    /// (b) A warm slot over the cap (half the disk budget) is evicted when
    /// its job ends; one under it stays warm.
    #[tokio::test]
    async fn a_warm_slot_over_the_cap_is_evicted_and_one_under_stays() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-warm", "disk_floor_gb = 0\n", free).await;
        let pc = layout.project_cache("pk1");
        put(&pc.join("target-1/debug/app"), 64 << 10);
        put(&pc.join("target-2/debug/app"), 64 << 10);
        // Sizes as a job end would have measured them (real bytes would be
        // 100 GB of files).
        n.slot_use.lock().unwrap().insert(pc.join("target-1"), SlotUse { bytes: 100 * GIB, used_at: 10 });
        n.slot_use.lock().unwrap().insert(pc.join("target-2"), SlotUse { bytes: 10 * GIB, used_at: 5 });
        // Default budget 150 GB → cap 75 GB: target-1 (100 GB) alone is over.
        let gone = n.trim_warm_slots().await;
        assert_eq!(gone.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>(), vec![pc.join("target-1")]);
        assert!(!pc.join("target-1").exists() && pc.join("target-2").exists(), "the slot under the cap stays warm");
        assert!(std::fs::read_dir(&pc).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(EVICTED_PREFIX)), "no leftovers");
        // Together over the cap: the least recently used goes first.
        put(&pc.join("target-3/debug/app"), 64 << 10);
        n.slot_use.lock().unwrap().insert(pc.join("target-3"), SlotUse { bytes: 70 * GIB, used_at: 20 });
        let gone = n.trim_warm_slots().await;
        assert_eq!(gone.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>(), vec![pc.join("target-2")], "80 GB > 75: the older slot goes");
        assert!(pc.join("target-3").exists());
        // A busy slot is never evicted.
        n.slot_use.lock().unwrap().insert(pc.join("target-3"), SlotUse { bytes: 200 * GIB, used_at: 20 });
        let leases: Vec<_> = (1..=3).map(|_| n.slots.lock().unwrap().lease(&pc, "pk1", 3, "c", layout.job_dir("x", 1).join("target"))).collect();
        assert!(leases.iter().any(|l| l.slot == Some(3)), "{leases:?}");
        assert!(n.trim_warm_slots().await.is_empty(), "held by a job");
        assert!(pc.join("target-3").exists());
    }

    /// Review A-disk 1: links a job planted are never followed. A slot or
    /// project that is a link isn't a slot (nothing it points at is
    /// evicted), and stripping a job dir removes a link as a link.
    #[tokio::test]
    async fn cleanup_never_follows_a_planted_link() {
        let free = Arc::new(AtomicU64::new(40 * GIB));
        let (n, layout) = node("disk-links", "disk_floor_gb = 50\n", free).await;
        let elsewhere = layout.home.parent().unwrap().join("elsewhere");
        put(&elsewhere.join("pk/target-1/keep"), 10);
        put(&elsewhere.join("job/keep"), 10);
        let pc = layout.project_cache("pk1");
        std::fs::create_dir_all(&pc).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("pk/target-1"), pc.join("target-2")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("pk"), layout.home.join("cache/pk2")).unwrap();
        assert!(n.idle_slots().is_empty(), "links aren't slots");
        assert!(n.disk_guard_pass().await.is_empty());
        assert!(n.gc_slots(n.now() + 1).is_empty());
        assert!(elsewhere.join("pk/target-1/keep").exists(), "nothing behind a link is evicted");
        // A job dir whose `target` is a link: the link goes, not its target.
        let dir = layout.job_dir("j", 1);
        put(&dir.join("log"), 1);
        std::os::unix::fs::symlink(elsewhere.join("job"), dir.join("target")).unwrap();
        assert_eq!(n.strip_job_dir(&n.owner(), &dir, None, JOB_END_KEEP).await, 1);
        // Live units from systemctl's list (review round 2, item 4).
        let listed = "k2job-j1-g1.service loaded active running K2 compute job k2job-j1-g1\n\
                      ● k2job-j2-g1.service loaded failed failed K2 compute job\n\
                      k2job-t1-0123456789abcdef.service loaded deactivating stop-sigterm K2\n\
                      k2job-selftest-1-0.service loaded active running x\n\
                      k2job-j3-g1.service loaded inactive dead K2\n";
        assert_eq!(parse_live_units(listed), vec!["k2job-j1-g1.service".to_string(), "k2job-t1-0123456789abcdef.service".to_string()]);
        assert!(std::fs::symlink_metadata(dir.join("target")).is_err() && elsewhere.join("job/keep").exists());
        // A job dir that is itself a link is left alone.
        std::os::unix::fs::symlink(elsewhere.join("job"), layout.job_dir("k", 1)).unwrap();
        assert_eq!(n.strip_job_dir(&n.owner(), &layout.job_dir("k", 1), None, &[]).await, 0);
        assert!(elsewhere.join("job/keep").exists());
    }

    /// Review A-disk 2: a job id that isn't a safe folder name is refused
    /// when it arrives: no ledger row, no folder, nothing for cleanup to
    /// walk out of the slot with.
    #[tokio::test]
    async fn a_job_id_that_isnt_folder_safe_is_refused_at_arrival() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-ids", "disk_floor_gb = 0\n", free).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        n.owner().hub.attach(tx);
        let long = "a".repeat(65);
        for id in ["../x", "a/b", long.as_str()] {
            let mut a = crate::ledger::tests::assign(id, 1);
            a.plan.argv = vec!["/bin/sh".into(), "-c".into(), "exit 0".into()];
            a.fence.plan_digest = a.plan.digest().unwrap();
            assert!(a.consistent(), "only the id is wrong");
            assert_eq!(n.on_assign(&n.owner(), a), None, "{id}");
            assert!(n.owner().ledger.lock().unwrap().get(id, 1).unwrap().is_none(), "{id}: no row");
            let f = rx.recv().await.unwrap();
            assert!(matches!(&f, k2_node_proto::frames::Frame::State(s) if s.reason.as_deref() == Some("bad_job_id") && s.state == JobState::Failed), "{f:?}");
        }
        assert!(!layout.home.join("x-g1").exists() && !layout.job_dir("a", 1).exists());
        assert_eq!(std::fs::read_dir(layout.home.join("jobs")).map(|d| d.count()).unwrap_or(0), 0, "no job folder at all");
        // The longest safe id still runs.
        let ok = "b".repeat(64);
        let mut a = crate::ledger::tests::assign(&ok, 1);
        a.plan.argv = vec!["/bin/sh".into(), "-c".into(), "exit 0".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&n.owner(), a), Some((ok.clone(), 1)));
    }

    /// Review A-disk 3: under the floor, the server furthest over its disk
    /// share loses its newest job first, not whoever started last; with
    /// nobody over, the newest job on the node. (Budget scaled: 1 "GB" =
    /// 1 MiB, so a 4 MiB file is over a 1 GB share.)
    #[tokio::test]
    async fn under_the_floor_the_server_over_its_share_pays_first() {
        use k2_node_proto::share::{Network, ServersFile, ShareLimits, Trust};
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-share", "disk_floor_gb = 50\n", free.clone()).await;
        let mut f = ServersFile::new(&"a".repeat(64), "o", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1, "local:t");
        for slot in [1u8, 2] {
            let mut e = f.servers[0].clone();
            e.slot = slot;
            e.label = format!("s{slot}");
            e.fp_prefix = format!("{:016x}", slot);
            e.trust = Trust::Own;
            e.set_limits(&ShareLimits::new_server(Trust::Own, false));
            e.disk_gb = 1;
            f.servers.push(e);
        }
        std::fs::write(layout.config.join(k2_node_proto::share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
        let mut tens = Vec::new();
        for slot in [1u8, 2] {
            layout.tenant(slot).ensure_state().unwrap();
            crate::identity::write_pin(&layout.tenant(slot).pin(), &crate::identity::Pin::for_test("n")).unwrap();
            tens.push(n.add_tenant(slot).await.unwrap());
        }
        let mut sessions = Vec::new();
        for s in [0u8, 1, 2] {
            let limits = ShareLimits { max_running: 2, ..ShareLimits::new_server(Trust::Own, false) };
            n.fairq.set_share(s, Some(crate::fairq::ShareInfo { label: format!("s{s}"), limits, status: k2_node_proto::share::ServerStatus::Active }));
            n.fairq.set_protocol(s, Some(3));
        }
        for t in &tens {
            let (tx, rx) = tokio::sync::mpsc::channel(256);
            t.hub.attach(tx);
            sessions.push(rx);
        }
        let start = |t: &Arc<Tenant>, id: &str| {
            let mut a = crate::ledger::tests::assign(id, 1);
            a.plan.argv = vec!["/bin/sleep".into(), "30".into()];
            a.fence.plan_digest = a.plan.digest().unwrap();
            assert_eq!(n.on_assign(t, a), Some((id.to_string(), 1)));
            n.fairq.tick(&n);
        };
        // Slot 1 starts first and is over its share; slot 2 starts last.
        start(&tens[0], "one");
        tokio::time::sleep(Duration::from_millis(100)).await;
        start(&tens[1], "two");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(n.running_by_tenant().values().map(|v| v.len()).sum::<usize>(), 2, "{:?}", n.running_by_tenant());
        put(&layout.tenant(1).cache().join("big"), 4 << 20);
        let budget = |gb: u32| u64::from(gb) << 20;
        free.store(40 * GIB, Ordering::SeqCst);
        assert_eq!(n.disk_guard_pass_with(budget).await, vec![(1u8, "one".to_string())], "the server over its share, not the newest job");
        let t0 = std::time::Instant::now();
        while n.running_by_tenant().get(&1).is_some_and(|v| !v.is_empty()) {
            assert!(t0.elapsed() < Duration::from_secs(20));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let r = tens[0].ledger.lock().unwrap().get("one", 1).unwrap().unwrap();
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Failed, Some(refusal::DISK_LOW)));
        // Nobody over (slot 1 has nothing running).
        assert_eq!(n.furthest_over_share(budget).await, None);
        // Review B F1: slot 2 hides data below the walk's depth cap: its
        // few bytes count as furthest over.
        let mut deep = layout.tenant(2).cache().join("hide");
        for _ in 0..(super::super::run::WALK_MAX_DEPTH + 4) {
            deep = deep.join("d");
        }
        put(&deep.join("blob"), 10);
        assert_eq!(n.furthest_over_share(budget).await, Some((2u8, u64::MAX)), "a capped walk is over");
        assert!(n.tenant_over_disk_with(&tens[1], budget), "the V74 scan agrees");
        assert_eq!(n.disk_guard_pass_with(budget).await, vec![(2u8, "two".to_string())]);
        n.stop_all(JobState::Cancelled, "test_over");
    }

    /// The walk's caps are reported, not hidden (review B F1).
    #[test]
    fn a_walk_says_when_it_hit_its_cap() {
        let d = crate::util::temp_dir("walk-cap");
        put(&d.join("a/b/c"), 4096);
        let (bytes, capped) = super::super::run::disk_usage_walk(&d);
        assert!(bytes >= 4096 && !capped);
        let mut deep = d.join("x");
        for _ in 0..(super::super::run::WALK_MAX_DEPTH + 1) {
            deep = deep.join("d");
        }
        std::fs::create_dir_all(&deep).unwrap();
        assert!(super::super::run::disk_usage_walk(&d).1, "past 256 levels");
    }

    /// Review B F5: a project key that isn't a safe folder name is refused
    /// when it arrives, like a bad job id (it names the mirror and caches
    /// the janitor and cleanup run git and deletes on).
    #[tokio::test]
    async fn a_project_key_that_isnt_folder_safe_is_refused_at_arrival() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-keys", "disk_floor_gb = 0\n", free).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        n.owner().hub.attach(tx);
        for (i, key) in ["../other", "a/b", "", &"k".repeat(129)].into_iter().enumerate() {
            let id = format!("pk{i}");
            let mut a = crate::ledger::tests::assign(&id, 1);
            a.plan.src = Some(k2_node_proto::frames::Src { project_key: key.to_string(), remote_url: None, commit: "a".repeat(40), dirty: None, slots: 1 });
            a.fence.plan_digest = a.plan.digest().unwrap();
            assert!(a.consistent() && a.job_id_ok(), "only the key is wrong");
            assert_eq!(n.on_assign(&n.owner(), a), None, "{key:?}");
            assert!(n.owner().ledger.lock().unwrap().get(&id, 1).unwrap().is_none(), "{key:?}: no row");
            let f = rx.recv().await.unwrap();
            assert!(matches!(&f, k2_node_proto::frames::Frame::State(s) if s.reason.as_deref() == Some("bad_project_key")), "{f:?}");
        }
        assert!(!layout.home.join("repos").join("..").join("other.git").exists());
        assert_eq!(std::fs::read_dir(layout.home.join("jobs")).map(|d| d.count()).unwrap_or(0), 0, "no job folder at all");
    }

    /// Review C L2: a row queued before the arrival checks (an older hold
    /// build) is checked again when the queue starts it: it ends
    /// cancelled with the reason, never reaching a folder.
    #[tokio::test]
    async fn a_queued_row_with_a_bad_id_or_key_is_refused_when_it_starts() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-queued", "disk_floor_gb = 0\n", free).await;
        let owner = n.owner();
        let mut bad_key = crate::ledger::tests::assign("okid", 1);
        bad_key.plan.src = Some(k2_node_proto::frames::Src { project_key: "../other".into(), remote_url: None, commit: "a".repeat(40), dirty: None, slots: 1 });
        bad_key.fence.plan_digest = bad_key.plan.digest().unwrap();
        for (a, why) in [(crate::ledger::tests::assign("../x", 1), "bad_job_id"), (bad_key, "bad_project_key")] {
            let id = a.fence.job_id.clone();
            owner.ledger.lock().unwrap().insert_queued(&a, &*n.opts.clock).unwrap();
            let row = owner.ledger.lock().unwrap().get(&id, 1).unwrap().unwrap();
            assert!(!n.start_queued(&owner, &row), "{id}");
            let r = owner.ledger.lock().unwrap().get(&id, 1).unwrap().unwrap();
            assert_eq!((r.state, r.reason.as_deref()), (JobState::Cancelled, Some(why)), "{id}");
            assert!(r.receipt.is_some(), "an unrun receipt, as for any queued end");
        }
        assert!(n.running_ids().is_empty());
        assert_eq!(std::fs::read_dir(layout.home.join("jobs")).map(|d| d.count()).unwrap_or(0), 0, "no job folder at all");
    }

    /// (c) Under the floor the guard reclaims first (ended job dirs, idle
    /// warm slots) and stops a running job (`failed / disk_low`) only when
    /// that wasn't enough: the newest first, one a pass. Above the floor
    /// plus the margin it does nothing.
    #[tokio::test]
    async fn the_guard_reclaims_then_stops_the_newest_job_under_the_floor() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-guard", "disk_floor_gb = 50\n", free.clone()).await;
        // An ended job's leftovers (before this build) and an idle slot.
        let old = layout.job_dir("old", 1);
        put(&old.join("target/debug/app"), 4096);
        put(&old.join("home/x"), 10);
        put(&old.join("log"), 10);
        {
            let owner = n.owner();
            let l = owner.ledger.lock().unwrap();
            l.insert(&crate::ledger::tests::assign("old", 1), &*n.opts.clock).unwrap();
            l.finish("old", 1, JobState::Done, None, None, None, 0, n.now()).unwrap();
        }
        let pc = layout.project_cache("pk1");
        put(&pc.join("target-1/debug/app"), 4096);
        sleeper(&n, "a");
        sleeper(&n, "b");
        n.fairq.tick(&n);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(n.disk_guard_pass().await.is_empty(), "plenty free");
        assert!(old.join("target").exists() && pc.join("target-1").exists(), "nothing touched above floor + margin");
        // Inside the margin: reclaim, no stop.
        free.store(55 * GIB, Ordering::SeqCst);
        assert!(n.disk_guard_pass().await.is_empty(), "above the floor: no job stops");
        assert!(!old.join("target").exists() && !old.join("home").exists(), "the ended job's dir is stripped");
        assert!(old.join("log").exists(), "its journal stays");
        assert!(!pc.join("target-1").exists(), "the idle warm slot is evicted");
        assert_eq!(n.running_ids().len(), 2);
        // Under the floor with nothing left to reclaim: the newest stops.
        free.store(40 * GIB, Ordering::SeqCst);
        assert_eq!(n.disk_guard_pass().await, vec![(0u8, "b".to_string())]);
        let r = ended(&n, "b").await;
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Failed, Some(refusal::DISK_LOW)));
        assert!(r.detail.as_deref().is_some_and(|d| d.contains("floor")), "{:?}", r.detail);
        assert_eq!(n.disk_guard_pass().await, vec![(0u8, "a".to_string())], "still low: the next one");
        let r = ended(&n, "a").await;
        assert_eq!(r.reason.as_deref(), Some(refusal::DISK_LOW));
        // New work is refused under the floor (admission, unchanged).
        let mut c = crate::ledger::tests::assign("c", 1);
        c.plan.argv = vec!["/bin/true".into()];
        assert_eq!(n.assign_refusal(&n.owner(), &c).map(|r| r.code), Some(refusal::DISK_LOW.to_string()));
    }

    /// G5 end to end: the newest running job is a gate (a release build);
    /// under the floor the older plain jobs stop first, the gate last.
    #[tokio::test]
    async fn the_guard_stops_plain_jobs_before_the_newest_gate_job() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, _layout) = node("disk-gate", "disk_floor_gb = 50\n", free.clone()).await;
        sleeper(&n, "a");
        sleeper(&n, "b");
        let mut g = crate::ledger::tests::assign("g", 1);
        g.plan.argv = vec!["/bin/sleep".into(), "30".into()];
        g.plan.class = Some(k2_node_proto::share::JobClass::Gate);
        g.fence.plan_digest = g.plan.digest().unwrap();
        assert_eq!(n.on_assign(&n.owner(), g), Some(("g".to_string(), 1)));
        n.fairq.tick(&n);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(n.running_ids().len(), 3);
        free.store(40 * GIB, Ordering::SeqCst);
        assert_eq!(n.disk_guard_pass().await, vec![(0u8, "b".to_string())], "the newest plain job, not the gate");
        ended(&n, "b").await;
        assert_eq!(n.disk_guard_pass().await, vec![(0u8, "a".to_string())]);
        ended(&n, "a").await;
        assert_eq!(n.disk_guard_pass().await, vec![(0u8, "g".to_string())], "only the gate left");
        let r = ended(&n, "g").await;
        assert_eq!(r.reason.as_deref(), Some(refusal::DISK_LOW));
    }

    /// (d) A ledger finish that fails (a full disk) is retried until it
    /// lands: the job never stays `running` once its process is gone.
    #[tokio::test]
    async fn a_failed_ledger_finish_is_retried_until_the_job_is_ended() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, _layout) = node("disk-finish", "disk_floor_gb = 0\n", free).await;
        let id = super::super::run::tests::FINISH_FAILS_JOB;
        super::super::run::tests::FINISH_FAILS.store(3, Ordering::SeqCst);
        let mut a = crate::ledger::tests::assign(id, 1);
        a.plan.argv = vec!["/bin/sh".into(), "-c".into(), "exit 0".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        n.owner().hub.attach(tx);
        assert_eq!(n.on_assign(&n.owner(), a), Some((id.into(), 1)));
        // The server hears the end from memory while the row can't be
        // written (k2n4: a full disk kept two jobs `running` for hours).
        let t0 = std::time::Instant::now();
        loop {
            let f = tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("a frame").expect("the hub");
            if let k2_node_proto::frames::Frame::State(s) = f {
                if s.job_id == id && s.state == JobState::Done {
                    let row = n.owner().ledger.lock().unwrap().get(id, 1).unwrap().unwrap();
                    assert!(!row.state.is_terminal(), "told before the row landed: {row:?}");
                    break;
                }
            }
            assert!(t0.elapsed() < Duration::from_secs(20));
        }
        assert!(n.running_ids().contains(&id.to_string()), "it keeps its place until the row lands");
        let r = ended(&n, id).await;
        assert_eq!(r.state, JobState::Done, "{:?} {:?}", r.reason, r.detail);
        assert!(r.receipt.is_some(), "the receipt landed with it");
        assert_eq!(super::super::run::tests::FINISH_FAILS.load(Ordering::SeqCst), 0, "every injected failure was retried");
    }

    #[test]
    fn finish_retries_back_off_to_five_minutes() {
        let b = Duration::from_secs(10);
        let waits: Vec<u64> = (1..=8).map(|n| super::super::run::finish_backoff(b, n).as_secs()).collect();
        assert_eq!(waits, vec![10, 20, 40, 80, 160, 300, 300, 300]);
        assert_eq!(super::super::run::finish_backoff(b, u32::MAX), Duration::from_secs(300));
    }

    /// A row a lost runner left `running` (a 0.46.2 full-disk finish, or a
    /// task that died): no process, but it held a slot and showed
    /// `running` for hours (k2n4, 2026-10-21). A cancel records its end
    /// as cancelled, with a receipt, and frees its place.
    #[tokio::test]
    async fn a_cancel_ends_a_job_whose_runner_is_gone() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, layout) = node("disk-orphan", "disk_floor_gb = 0\n", free).await;
        let owner = n.owner();
        let mut j = crate::journal::JournalWriter::create(&owner.paths.journal("lost", 1), 1 << 20).unwrap_or_else(|_| {
            std::fs::create_dir_all(layout.job_dir("lost", 1)).unwrap();
            crate::journal::JournalWriter::create(&owner.paths.journal("lost", 1), 1 << 20).unwrap()
        });
        j.append(k2_node_proto::frames::LogStream::Out, b"half a build\n").unwrap();
        drop(j);
        {
            let l = owner.ledger.lock().unwrap();
            l.insert(&crate::ledger::tests::assign("lost", 1), &*n.opts.clock).unwrap();
            l.set_running("lost", 1, 4242, None, n.now()).unwrap();
        }
        // Never a queued row or one only just taken off the queue.
        {
            let l = owner.ledger.lock().unwrap();
            l.insert_queued(&crate::ledger::tests::assign("waiting", 1), &*n.opts.clock).unwrap();
            l.insert(&crate::ledger::tests::assign("dequeued", 1), &*n.opts.clock).unwrap();
        }
        assert!(!n.finish_if_orphan(&owner, "waiting", 1, JobState::Cancelled, "x", "x"));
        assert!(!n.finish_if_orphan(&owner, "dequeued", 1, JobState::Cancelled, "x", "x"));
        n.cancel(&owner, "lost", 1, "cancelled");
        let r = ended(&n, "lost").await;
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Cancelled, Some("cancelled")));
        assert!(r.receipt.is_some(), "a receipt, as for any end");
        assert!(r.log_seq >= 2, "the journal carried on: {}", r.log_seq);
        assert!(n.running_ids().is_empty(), "its place is free");
        let l = owner.ledger.lock().unwrap();
        assert_eq!(l.get("waiting", 1).unwrap().unwrap().state, JobState::Queued);
        assert!(!l.get("dequeued", 1).unwrap().unwrap().state.is_terminal());
    }

    /// Review B F2: a sweep that loses the race with the runner (it read
    /// the row before the runner's end landed) never rewrites that end:
    /// the orphan check backs out, and a second end that slips through
    /// writes nothing (the runner's state and its one receipt stand) and
    /// tells the server nothing.
    #[tokio::test]
    async fn a_sweep_that_races_a_real_finish_leaves_the_runners_end() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, _layout) = node("disk-race", "disk_floor_gb = 0\n", free).await;
        let owner = n.owner();
        let mut a = crate::ledger::tests::assign("r", 1);
        a.plan.argv = vec!["/bin/sh".into(), "-c".into(), "exit 0".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&owner, a.clone()), Some(("r".into(), 1)));
        let first = ended(&n, "r").await;
        assert_eq!(first.state, JobState::Done);
        let receipt = serde_json::to_string(first.receipt.as_ref().expect("the runner's receipt")).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        owner.hub.attach(tx);
        // The orphan check, after the runner's end landed: backs out.
        assert!(!n.finish_if_orphan(&owner, "r", 1, JobState::Failed, super::super::assign::NODE_LOST_JOB, "x"));
        assert!(n.running_ids().is_empty(), "no placeholder left behind");
        // A second end that got past the checks anyway.
        n.finish_job(&owner, &a, None, JobState::Failed, Some(super::super::assign::NODE_LOST_JOB), Some("x"), None, None, None, None, None).await;
        let r = owner.ledger.lock().unwrap().get("r", 1).unwrap().unwrap();
        assert_eq!((r.state, r.reason.as_deref(), r.ended_at), (JobState::Done, None, first.ended_at), "the runner's end stands");
        assert_eq!(serde_json::to_string(r.receipt.as_ref().unwrap()).unwrap(), receipt, "one receipt: the runner's");
        while let Ok(f) = rx.try_recv() {
            let about_r = matches!(&f, k2_node_proto::frames::Frame::State(s) if s.job_id == "r") || matches!(&f, k2_node_proto::frames::Frame::Receipt(x) if x.job_id == "r");
            assert!(!about_r, "nothing sent for the losing end: {f:?}");
        }
        assert!(n.running_ids().is_empty());
    }

    /// The same without a cancel: the guard loop's sweep ends a lost
    /// job `failed / node_lost_job`, once; a live job is never touched.
    #[tokio::test]
    async fn the_sweep_ends_a_lost_job_and_leaves_live_ones() {
        let free = Arc::new(AtomicU64::new(500 * GIB));
        let (n, _layout) = node("disk-sweep", "disk_floor_gb = 0\n", free).await;
        let owner = n.owner();
        {
            let l = owner.ledger.lock().unwrap();
            l.insert(&crate::ledger::tests::assign("lost", 1), &*n.opts.clock).unwrap();
            l.set_running("lost", 1, 4242, None, n.now()).unwrap();
        }
        sleeper(&n, "live");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(n.reap_orphans(), 1);
        assert_eq!(n.reap_orphans(), 0, "once");
        let r = ended(&n, "lost").await;
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Failed, Some(super::super::assign::NODE_LOST_JOB)));
        assert!(!owner.ledger.lock().unwrap().get("live", 1).unwrap().unwrap().state.is_terminal(), "the live job runs on");
        n.stop_all(JobState::Cancelled, "test_over");
    }
}
