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

/// One idle warm slot: `(project cache, project key, n, dir)`.
struct Idle {
    project: PathBuf,
    key: String,
    n: u32,
    dir: PathBuf,
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
    /// the node first. Returns the entries removed.
    pub(crate) async fn strip_job_dir(&self, t: &Tenant, root: &Path, project_key: Option<&str>, keep: &[&str]) -> usize {
        let Ok(rd) = std::fs::read_dir(root) else { return 0 };
        let doomed: Vec<PathBuf> = rd.flatten().filter(|e| !keep.iter().any(|k| e.file_name() == *k)).map(|e| e.path()).collect();
        if doomed.is_empty() {
            return 0;
        }
        if matches!(self.wall, Wall::JobUser { .. }) {
            for p in &doomed {
                let op = crate::admin::jobs::JobOp::Reclaim { path: p.clone() };
                if let Err(e) = self.job_op(op, vec![], Duration::from_secs(600)).await {
                    crate::nlog!("reclaim {}: {e}", p.display());
                }
            }
        }
        let src = root.join("src");
        if let (Some(pk), true) = (project_key, doomed.contains(&src)) {
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
                let key = project.file_name().to_string_lossy().into_owned();
                let Ok(inner) = std::fs::read_dir(project.path()) else { continue };
                for e in inner.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Some(n) = name.strip_prefix("target-").and_then(|n| n.parse::<u32>().ok()) else { continue };
                    if slots.is_busy(&key, n) || !e.path().is_dir() {
                        continue;
                    }
                    out.push(Idle { project: project.path(), key: key.clone(), n, dir: e.path() });
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
        self.delete_evicted(gone).await;
        true
    }

    async fn delete_evicted(&self, p: PathBuf) {
        if matches!(self.wall, Wall::JobUser { .. }) {
            let op = crate::admin::jobs::JobOp::Reclaim { path: p.clone() };
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
            for project in rd.flatten() {
                let Ok(inner) = std::fs::read_dir(project.path()) else { continue };
                for e in inner.flatten() {
                    if e.file_name().to_string_lossy().starts_with(EVICTED_PREFIX) {
                        found.push(e.path());
                    }
                }
            }
        }
        for p in found {
            self.delete_evicted(p).await;
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
                if !r.state.is_terminal() || r.cleaned || mine.contains(&r.job_id) {
                    continue;
                }
                let dir = t.paths.job_dir(&r.job_id, r.generation);
                if dir.is_dir() && self.strip_job_dir(&t, &dir, r.plan.src.as_ref().map(|s| s.project_key.as_str()), PRESSURE_KEEP).await > 0 {
                    n += 1;
                }
            }
        }
        n
    }

    /// One guard pass (see the module doc). Returns the jobs it stopped,
    /// `(slot, job_id)`.
    pub async fn disk_guard_pass(self: &Arc<Self>) -> Vec<(k2_node_proto::share::Slot, String)> {
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
        // Still under the floor: the newest running job stops (one a
        // pass; its dir goes when it ends, then the next pass looks again).
        let jobs = self.jobs.lock().unwrap();
        let newest = jobs.iter().filter(|(_, j)| j.stop.borrow().is_none()).max_by_key(|(_, j)| j.seq);
        let Some(((slot, job), j)) = newest else { return vec![] };
        crate::nlog!("disk low ({} GB free, floor {} GB): stopping job {job} (slot {slot})", free >> 30, floor >> 30);
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
                n.disk_guard_pass().await;
            }
        });
    }
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
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&n.owner(), a), Some(("big".into(), 1)));
        let r = ended(&n, "big").await;
        assert_eq!(r.state, JobState::Done, "{r:?}");
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
        assert_eq!(n.on_assign(&n.owner(), a), Some((id.into(), 1)));
        let r = ended(&n, id).await;
        assert_eq!(r.state, JobState::Done, "{:?} {:?}", r.reason, r.detail);
        assert!(r.receipt.is_some(), "the receipt landed with it");
        assert_eq!(super::super::run::tests::FINISH_FAILS.load(Ordering::SeqCst), 0, "every injected failure was retried");
    }
}
