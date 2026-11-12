//! Cache generations in a job's life (prd-compute-blacksmith-class-v1
//! §4.1, slice B1): the fork before the job, the commit or discard after
//! it, the node-only fork records, and the sweep of forks no job holds.
//! The filesystem side is [`crate::gens`].
//!
//! A job forks only when every one of these holds: the machine owner's
//! switch is on, the node has its job wall (and, on Linux, an executor that
//! hides `gens/`), the probe found the gens root closed, the server speaks
//! protocol 10 and its plan carries `cache_commit` (sent only to a node
//! offering `cache-gens`). Otherwise the job runs exactly as before: warm
//! slots, private builds, today's results and receipts.

use std::path::PathBuf;
use std::sync::Arc;

use k2_node_proto::frames::{Assign, ExitInfo, JobState, SignedReceipt};
use k2_node_proto::gens::{CacheCommit, CacheResult};
use k2_node_proto::share::Slot;

use super::{Node, Tenant};
use crate::gens::{Ended, Fork, ForkAsk, Gens};

/// Node-only fork records: `<tenant state>/gens-forks/<job key>.json`.
const FORK_RECORDS: &str = "gens-forks";

/// One fork record: which attempt holds the fork.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkRecord {
    slot: Slot,
    job: String,
    gen: u32,
    fork: Fork,
}

/// The node's generations, opened at start: the switch, the wall and the
/// probe of the owner's gens root. `None` (with the reason logged) when
/// any is missing. `test_exempt`: a debug test node with no wall
/// ([`super::dev_test_exemption`]; never a release binary or an installed
/// node), so the end-to-end tests run without root.
pub fn open_gens(switch: &crate::gens::SwitchFile, wall: &crate::wall::Wall, exec_gens_wall: bool, home: &std::path::Path, test_exempt: bool) -> Option<Arc<Gens>> {
    if !switch.on {
        return None;
    }
    if let Some(why) = crate::gens::wall_refusal(wall, false, exec_gens_wall).filter(|_| !test_exempt) {
        crate::nlog!("cache generations: off ({why})");
        return None;
    }
    let root = crate::gens::gens_root(home);
    match crate::gens::ensure_closed(&root).and_then(|_| crate::gens::probe(&root)) {
        Ok(fs) => {
            crate::nlog!("cache generations: on ({}, clone by {:?}{})", fs.fs, fs.clone, if fs.volume { ", cache volume" } else { "" });
            Some(Arc::new(Gens::new(fs)))
        }
        Err(e) => {
            crate::nlog!("cache generations: off ({e})");
            None
        }
    }
}

impl Node {
    /// Why this node keeps no cache generations now, or `None`.
    pub fn gens_refusal(&self) -> Option<String> {
        if self.gens.is_none() {
            return Some("cache generations are off on this node".into());
        }
        if super::dev_test_exemption(self.opts.dev, &self.layout().config) && matches!(self.wall, crate::wall::Wall::None { .. }) {
            return None;
        }
        crate::gens::wall_refusal(&self.wall, self.opts.dev, self.exec_gens_wall.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// The generations `t`'s jobs may use: on, walled, and a server that
    /// speaks protocol 10 (the only one that sends `cache_commit`).
    pub fn gens_for(&self, t: &Tenant) -> Option<Arc<Gens>> {
        if t.protocol() < k2_node_proto::frames::PROTOCOL_GENS || self.gens_refusal().is_some() {
            return None;
        }
        self.gens.clone()
    }

    fn fork_record(t: &Tenant, key: &str) -> PathBuf {
        t.paths.state().join(FORK_RECORDS).join(format!("{key}.json"))
    }

    /// Fork for `a`'s job, if its plan asks and this node keeps
    /// generations. `own_uid_secret`: a secret job running under its own
    /// uid (Linux pool), the only secret job that may fork (BK-14); its
    /// fork goes in its own job root, handed to that uid by `Start`. Every
    /// other secret job, and every release build, never forks here.
    pub(super) async fn gens_fork(&self, t: &Tenant, a: &Assign, job_root: &std::path::Path, own_uid_secret: bool) -> Option<Result<Fork, String>> {
        let commit = a.plan.cache_commit?;
        let src = a.plan.src.as_ref()?;
        let g = self.gens_for(t)?;
        let secret = !a.plan.secret_env.is_empty();
        if secret && !(own_uid_secret && commit == CacheCommit::None) {
            return None;
        }
        if a.plan.receipt_v.is_some() {
            // Release builds (receipt v2) keep no lineage in this slice (B3).
            return None;
        }
        let key = t.job_key(&a.fence.job_id, a.fence.generation);
        let set = match crate::gens::SetId::target(&t.paths.root(), &src.project_key) {
            Ok(s) => s,
            Err(e) => return Some(Err(e)),
        };
        let dst = if secret { job_root.join(crate::gens::SECRET_FORK_DIR) } else { crate::gens::forks_root(&t.paths.root()).join(&key) };
        let (cache_ref, now, rec) = (a.plan.cache_ref.clone(), self.now(), Self::fork_record(t, &key));
        let (slot, job, gen) = (t.slot, a.fence.job_id.clone(), a.fence.generation);
        let unit_mode = matches!(self.wall, crate::wall::Wall::JobUser { .. });
        let r = tokio::task::spawn_blocking(move || {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
                if unit_mode && !secret {
                    crate::wall::group_dir(parent)?;
                }
            }
            let _ = crate::gens::remove_tree(&dst);
            let f = g.fork(&ForkAsk { set: &set, commit, cache_ref: cache_ref.as_deref(), dst: &dst, now })?;
            if unit_mode && !secret {
                // The clone is the node's; the job (the slot's group) writes
                // its target, CARGO_HOME and sccache dir: group rw on every
                // entry the node owns (O(files), BK-3).
                crate::wall::group_dir(&f.dir)?;
                crate::wall::share_with_jobs(&f.dir)?;
            }
            let body = serde_json::to_vec(&ForkRecord { slot, job, gen, fork: f.clone() }).map_err(|e| e.to_string())?;
            if let Some(p) = rec.parent() {
                crate::util::mkdir_mode(p, 0o700)?;
            }
            crate::util::atomic_write(&rec, &body, 0o600)?;
            Ok(f)
        })
        .await
        .map_err(|e| format!("fork: {e}"))
        .and_then(|r| r);
        if let Ok(f) = &r {
            self.gens_forks.lock().unwrap().insert((t.slot, a.fence.job_id.clone(), a.fence.generation), f.clone());
        }
        Some(r)
    }

    /// The fork `t`'s job attempt holds (memory, else its node-only record:
    /// an adopted job after a restart).
    fn take_fork(&self, t: &Tenant, job: &str, gen: u32) -> Option<Fork> {
        if let Some(f) = self.gens_forks.lock().unwrap().remove(&(t.slot, job.to_string(), gen)) {
            return Some(f);
        }
        let rec = Self::fork_record(t, &t.job_key(job, gen));
        let r: ForkRecord = serde_json::from_slice(&std::fs::read(&rec).ok()?).ok()?;
        if (r.slot, r.job.as_str(), r.gen) != (t.slot, job, gen) {
            return None;
        }
        let f = r.fork;
        if let Some(g) = &self.gens {
            g.adopt(&f);
        }
        Some(f)
    }

    /// The fork of `t`'s job attempt, for its Seatbelt profile (the one
    /// fork it may write).
    pub(super) fn fork_dir_of(&self, t: &Tenant, job: &str, gen: u32) -> Option<PathBuf> {
        self.gens_forks.lock().unwrap().get(&(t.slot, job.to_string(), gen)).map(|f| f.dir.clone())
    }

    /// The job ended: commit its fork or discard it (§4.1.4), after its
    /// receipt is signed (the producer's digest names it). The fork's
    /// folder goes in the background when discarded.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn gens_finish(
        &self,
        t: &Tenant,
        a: &Assign,
        state: JobState,
        reason: Option<&str>,
        exit: Option<&ExitInfo>,
        tests_failed: u64,
        signed: Option<&SignedReceipt>,
    ) -> Option<CacheResult> {
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        let fork = self.take_fork(t, &job, gen)?;
        // Linux wall: what the job wrote is its uid's; the tree comes back
        // to the node before it is saved or deleted (a generation is never
        // a reclaim target: gens/ is outside jobs/ and cache/).
        if matches!(self.wall, crate::wall::Wall::JobUser { .. }) && fork.dir.starts_with(crate::gens::forks_root(&t.paths.root())) {
            let op = crate::admin::jobs::JobOp::Reclaim { path: fork.dir.clone(), slot: t.slot };
            if let Err(e) = self.job_op(op, vec![], std::time::Duration::from_secs(600)).await {
                crate::nlog!("cache generations: reclaim {}: {e}", fork.dir.display());
            }
        }
        let rec = Self::fork_record(t, &t.job_key(&job, gen));
        let g = self.gens.clone()?;
        let src = a.plan.src.clone();
        let code = exit.and_then(|e| e.code);
        let ok = state == JobState::Done && code == Some(0) && tests_failed == 0;
        let end = match (state, reason, code) {
            (JobState::Done, _, Some(0)) if tests_failed > 0 => format!("{tests_failed} test(s) failed"),
            (_, Some(r), _) => r.to_string(),
            (_, None, Some(c)) => format!("exit {c}"),
            (s, None, None) => s.as_str().to_string(),
        };
        let receipt_digest = signed.map(|s| format!("sha256:{}", k2_node_proto::crypto::sha256_hex(s.body.as_bytes()))).unwrap_or_default();
        let commit = src.as_ref().map(|s| s.commit.clone()).unwrap_or_default();
        let mirror = src.as_ref().map(|s| t.paths.mirror(&s.project_key));
        let git = crate::sync::Git::find(&self.layout().git_home(), &crate::util::base_path(&self.layout().home));
        // Rule 5: the node fetched this repo's remote itself (its refs are
        // the remote's, not the controller's): the commit must be on
        // origin/<branch> there.
        let own_fetch = self.own_fetches.lock().unwrap().remove(&(t.slot, job.clone(), gen));
        let on_branch = match (a.plan.cache_commit, a.plan.cache_ref.as_deref(), own_fetch, &git, &mirror) {
            (Some(CacheCommit::Trusted), Some(b), true, Some(g), Some(m)) => Some(git_on_branch(g, m, &commit, b)),
            _ => None,
        };
        let ended = (
            ok,
            state == JobState::Cancelled,
            end,
            commit.clone(),
            src.as_ref().is_some_and(|s| s.dirty.is_some()),
            !a.plan.with.is_empty(),
            !a.plan.secret_env.is_empty(),
            k2_node_proto::ci::CacheProducer { job_id: job.clone(), attempt: a.fence.attempt, receipt_digest, commit },
            self.now(),
        );
        let (res, doomed) = tokio::task::spawn_blocking(move || {
            let anc = move |x: &str, y: &str| -> Result<bool, String> {
                let (Some(g), Some(m)) = (&git, &mirror) else { return Err("no git or mirror on this node".into()) };
                git_is_ancestor(g, m, x, y)
            };
            let e = Ended {
                ok: ended.0,
                cancelled: ended.1,
                end: ended.2,
                commit: ended.3,
                dirty: ended.4,
                with_repos: ended.5,
                secret: ended.6,
                producer: ended.7,
                ref_checked: Some(k2_node_proto::ci::RefChecked::Controller),
                on_branch,
                is_ancestor: &anc,
                now: ended.8,
            };
            g.finish(fork, &e)
        })
        .await
        .ok()?;
        let _ = std::fs::remove_file(&rec);
        remove_later(doomed);
        if res.committed {
            self.gens_evict(&t.paths.root());
            self.send_lineage_status(t);
        }
        Some(res)
    }

    /// Remember that `a`'s main repo came from the node's own fetch of its
    /// remote (rule 5).
    pub(super) fn note_own_fetch(&self, t: &Tenant, a: &Assign, project_key: &str) {
        if a.plan.src.as_ref().is_some_and(|s| s.project_key == project_key) && a.plan.cache_commit == Some(CacheCommit::Trusted) {
            self.own_fetches.lock().unwrap().insert((t.slot, a.fence.job_id.clone(), a.fence.generation));
        }
    }

    /// The `lineage_status` frame for `t` (protocol 10, `cache-gens`), or
    /// `None` when `t`'s jobs don't use generations.
    pub fn lineage_status_for(&self, t: &Tenant) -> Option<k2_node_proto::gens::LineageStatus> {
        let g = self.gens_for(t)?;
        Some(g.status(&t.paths.root(), self.now()))
    }

    /// Send `t` its lineages (a head moved, or its session started).
    pub fn send_lineage_status(&self, t: &Tenant) {
        if let Some(s) = self.lineage_status_for(t) {
            t.hub.send(k2_node_proto::frames::Frame::LineageStatus(s));
        }
    }

    /// The eviction limits now (§4.1.3): generations get the warm budget
    /// (half the disk budget, as warm slots), one lineage half of that
    /// unless the owner's switch file says otherwise; low disk drops
    /// scratch first.
    fn gens_policy(&self) -> crate::gens::EvictPolicy {
        let (cap, floor) = {
            let l = self.local.lock().unwrap();
            (super::disk::warm_cap(&l.policy), l.policy.disk_floor_bytes)
        };
        let max = self.opts.cache_gens.as_ref().and_then(|s| s.lineage_max_gb).map(|g| g.saturating_mul(1 << 30)).unwrap_or(cap / 2);
        crate::gens::EvictPolicy { now: self.now(), budget_bytes: cap, lineage_max_bytes: max, pressure: self.disk_free_bytes() < floor.saturating_add(super::disk::RECLAIM_MARGIN) }
    }

    /// Evict in one server's root (in the background; logs each step).
    pub(super) fn gens_evict(&self, root: &std::path::Path) {
        let Some(g) = self.gens.clone() else { return };
        let (p, root) = (self.gens_policy(), root.to_path_buf());
        tokio::task::spawn_blocking(move || {
            let (lines, doomed) = g.evict(&root, &p);
            for l in lines {
                crate::nlog!("cache generations: {l}");
            }
            for d in doomed {
                if let Err(e) = crate::gens::remove_tree(&d) {
                    crate::nlog!("cache generations: {e}");
                }
            }
        });
    }

    /// Forks no running job holds (a crash between fork and end, or a job
    /// that ended while k2-node was down) and their records go.
    pub(super) async fn gens_sweep(&self) {
        let Some(_g) = self.gens.clone() else { return };
        // Running attempts by (slot, job) and by job key (a fork being made
        // has its folder before its record).
        let (running, running_keys): (std::collections::HashSet<(Slot, String)>, std::collections::HashSet<String>) = {
            let j = self.jobs.lock().unwrap();
            (j.keys().cloned().collect(), j.iter().map(|((s, job), r)| super::tenant::job_key(*s, job, r.generation)).collect())
        };
        let tenants: Vec<Arc<Tenant>> = self.tenants.read().unwrap().values().cloned().collect();
        let held: std::collections::HashSet<PathBuf> = self.gens_forks.lock().unwrap().values().map(|f| f.dir.clone()).collect();
        for t in tenants {
            let recs = t.paths.state().join(FORK_RECORDS);
            let mut kept = std::collections::HashSet::new();
            if let Ok(rd) = std::fs::read_dir(&recs) {
                for e in rd.flatten() {
                    let Ok(r) = serde_json::from_slice::<ForkRecord>(&std::fs::read(e.path()).unwrap_or_default()) else {
                        let _ = std::fs::remove_file(e.path());
                        continue;
                    };
                    let f = r.fork;
                    let live = r.slot == t.slot && running.contains(&(t.slot, r.job.clone()));
                    if live || held.contains(&f.dir) {
                        kept.insert(f.dir.clone());
                    } else {
                        let _ = crate::gens::remove_tree(&f.dir);
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let forks = crate::gens::forks_root(&t.paths.root());
            if let Ok(rd) = std::fs::read_dir(&forks) {
                for e in rd.flatten() {
                    let p = e.path();
                    let made_now = e.file_name().to_str().is_some_and(|k| running_keys.contains(k));
                    if !kept.contains(&p) && !held.contains(&p) && !made_now {
                        crate::nlog!("cache generations: removing {} (no job holds it)", p.display());
                        let _ = crate::gens::remove_tree(&p);
                    }
                }
            }
            self.gens_evict(&t.paths.root());
        }
    }
}

/// `git merge-base --is-ancestor <a> <b>` in a bare mirror: exit 0 yes,
/// 1 no, anything else an error (never a yes).
pub fn git_is_ancestor(g: &crate::sync::Git, mirror: &std::path::Path, a: &str, b: &str) -> Result<bool, String> {
    for c in [a, b] {
        if !crate::gens::is_commit(c) {
            return Err(format!("{c:?} is not a commit"));
        }
    }
    let out = std::process::Command::new(&g.program)
        .env_clear()
        .envs(g.env.iter().cloned())
        .arg("--git-dir")
        .arg(mirror)
        .args(["merge-base", "--is-ancestor", a, b])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("git merge-base: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

/// Delete folders in the background, logging what fails.
fn remove_later(doomed: Vec<PathBuf>) {
    if doomed.is_empty() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        for d in doomed {
            if let Err(e) = crate::gens::remove_tree(&d) {
                crate::nlog!("cache generations: {e}");
            }
        }
    });
}

/// Rule 5: is `commit` on `refs/remotes/origin/<branch>` in the mirror the
/// node fetched itself? A missing ref is an error (never a yes).
pub fn git_on_branch(g: &crate::sync::Git, mirror: &std::path::Path, commit: &str, branch: &str) -> Result<bool, String> {
    if !crate::gens::is_commit(commit) || !k2_node_proto::gens::valid_ref(branch) || branch.starts_with(k2_node_proto::gens::WORKTREE_REF_PREFIX) {
        return Err("not a commit and branch".into());
    }
    let r = format!("refs/remotes/origin/{branch}");
    let out = std::process::Command::new(&g.program)
        .env_clear()
        .envs(g.env.iter().cloned())
        .arg("--git-dir")
        .arg(mirror)
        .args(["merge-base", "--is-ancestor", commit, &r])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("git merge-base {r}: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}
