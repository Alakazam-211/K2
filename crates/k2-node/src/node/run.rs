//! One job's life: toolchains, the spawn behind the wall, the wait (exit,
//! stop, timeout, stall, memory), artifacts, and the finish (receipt,
//! ledger). Paths, the journal and the ledger are the job's tenant's; unit
//! and cgroup names use its per-slot key (V48).
//!
//! The tenant wall (S3a/S3m): a non-owner server's job runs only behind a
//! wall that keeps it from the other servers' (V59): on Linux as its own
//! user (`k2job-<slot>`, the executor's `[tenants]`), on macOS (own
//! servers only, 0.46.3) in its own Seatbelt profile. Its network is the
//! narrower of the node policy's and its share's (V65), its caches and
//! sccache socket are its slot's (V63), and nothing it wrote is read by
//! the node until its unit is gone.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::crypto;
use k2_node_proto::frames::{Assign, ExitInfo, JobState, Receipt, ReceiptLog, SignedReceipt, SrcRan};
use k2_node_proto::refusal;
use k2_node_proto::results::Outputs;
use k2_node_proto::share::{codes, Network, Trust};
use tokio::sync::watch;

use super::{Node, StopReason, Tenant};
use crate::cgroup::Leaf;
use crate::journal::JournalWriter;
use crate::paths::safe_relative;
use crate::runner::{self, JobDirs};
use crate::slots::Lease;
use crate::wall::Wall;

/// A job's stdout or stderr, read by the journal pumps.
type OutPipe = Box<dyn tokio::io::AsyncRead + Unpin + Send>;

/// A running job: our child (no wall / Seatbelt) or a unit (Linux wall).
enum Proc {
    Local { child: tokio::process::Child, pgid: i32, leaf: Option<Leaf> },
    Unit { unit: String, last: Option<crate::admin::jobs::UnitStatus> },
}

impl Proc {
    /// The process group (0 for a unit: the executor signals it).
    fn pgid(&self) -> i32 {
        match self {
            Proc::Local { pgid, .. } => *pgid,
            Proc::Unit { .. } => 0,
        }
    }

    fn needs_rss_sampler(&self) -> bool {
        matches!(self, Proc::Local { leaf: None, .. })
    }

    /// Was anything in the job OOM-killed? (V24.) Read before the leaf or
    /// the unit goes away.
    fn oom(&self) -> bool {
        match self {
            Proc::Local { leaf: Some(l), .. } => std::fs::read_to_string(l.dir.join("memory.events"))
                .map(|t| crate::admin::jobs::parse_oom_kills(&t) > 0)
                .unwrap_or(false),
            Proc::Local { leaf: None, .. } => false,
            Proc::Unit { last, .. } => last.as_ref().is_some_and(|s| s.oom()),
        }
    }

    fn cleanup(&self) {
        if let Proc::Local { leaf: Some(l), .. } = self {
            l.remove();
        }
    }
}

/// The `mem_cap` detail: the cap and how to ask for more.
pub fn mem_cap_detail(cap: u64) -> String {
    format!("the job used more than its memory cap ({} MiB) and was killed; ask for more with --mem", cap >> 20)
}

/// Linux wall, before a job: the dirs it writes get the node's group
/// (`k2job` runs with it). A warm target the node filled before the wall
/// is shared once (marker); the code checkout every time (it's fresh).
fn prepare_for_job_user(target: &Path, caches: &[&Path], src: Option<&Path>) -> Result<(), String> {
    crate::wall::group_dir(target)?;
    let mark = target.join(crate::wall::SHARED_MARK);
    if !mark.exists() {
        crate::wall::share_with_jobs(target)?;
        std::fs::write(&mark, "").map_err(|e| format!("write {}: {e}", mark.display()))?;
    }
    for c in caches {
        crate::wall::group_dir(c)?;
    }
    if let Some(s) = src {
        crate::wall::share_with_jobs(s)?;
    }
    Ok(())
}

impl Node {
    /// `k2-node run --dev` (dev/smoke only: the node runs as a person and
    /// every job can read that person's files; it claims no wall) may run
    /// another server's job without one, so the queue and per-tenant
    /// bookkeeping are testable without root. It can't get a second server
    /// in life: its offer has no `job-wall`, so `servers.add` refuses
    /// (V59). Every node without `--dev` and without a wall (no
    /// `jobs.toml`, an old setup) refuses tenant jobs.
    fn dev_unwalled(&self) -> bool {
        crate::node::dev_test_exemption(self.opts.dev, &self.layout().config) && matches!(self.wall, Wall::None { .. })
    }

    /// A job's network: the node policy's for the owner; for a tenant the
    /// narrower of the policy's and its share's (V65). macOS can't close
    /// only the LAN, so a tenant's `no-lan` is `none` there.
    pub fn job_network(&self, t: &Tenant) -> Result<Network, (String, String)> {
        let policy = Network::of_policy(&self.local.lock().unwrap().policy.network);
        if t.is_owner() || self.dev_unwalled() {
            return Ok(policy);
        }
        let entry = crate::admin::jobs::tenant_entry(&self.layout().config, t.slot).map_err(|r| (r.code, r.message))?;
        let n = policy.min(entry.network);
        Ok(if cfg!(target_os = "macos") && n == Network::NoLan { Network::None } else { n })
    }

    /// May a job of `t` start behind this node's wall? Always for the
    /// owner. A tenant needs (V59): a wall at all; on Linux the executor's
    /// tenant wall and a user for its slot; on macOS an `own` server and
    /// its prepared root. `Err((code, detail))`: the job never runs as
    /// another server's user.
    pub async fn tenant_wall_check(&self, t: &Tenant) -> Result<(), (String, String)> {
        if t.is_owner() || self.dev_unwalled() {
            return Ok(());
        }
        let missing = |m: String| (codes::TENANT_WALL_MISSING.to_string(), m);
        let entry = crate::admin::jobs::tenant_entry(&self.layout().config, t.slot).map_err(|r| (r.code, r.message))?;
        match &self.wall {
            Wall::None { why } => Err(missing(format!("this node has no job wall ({why}); it serves only its owner"))),
            Wall::Seatbelt => {
                if entry.trust == Trust::Team {
                    return Err((codes::TEAM_NEEDS_TENANT_WALL.into(), "Macs take only your own servers until per-tenant users (0.46.4)".into()));
                }
                if !t.paths.root().is_dir() {
                    return Err(missing(format!("{} isn't prepared (tenant.prepare)", t.paths.root().display())));
                }
                Ok(())
            }
            Wall::JobUser { .. } => match self.job_op(crate::admin::jobs::JobOp::Tenants, vec![], Duration::from_secs(30)).await {
                Ok(crate::admin::jobs::JobAnswer::Tenants(w)) if !w.ready => {
                    let why = w.why.unwrap_or_default();
                    let code = if why.starts_with(codes::TEAM_NEEDS_TENANT_WALL) { codes::TEAM_NEEDS_TENANT_WALL } else { codes::TENANT_WALL_MISSING };
                    Err((code.into(), why))
                }
                Ok(crate::admin::jobs::JobAnswer::Tenants(w)) => match w.users.get(&t.slot.to_string()) {
                    Some(u) if *u == crate::admin::jobs::tenant_user_name(t.slot) && crate::admin::server::user_ids(u).is_some() => Ok(()),
                    Some(u) => Err(missing(format!("{u} isn't a user on this machine (tenant.prepare)"))),
                    None => Err(missing(format!("slot {} has no job user (tenant.prepare)", t.slot))),
                },
                Ok(other) => Err(missing(format!("the executor can't wall tenants (an older k2-node admin?): {other:?}"))),
                Err(e) => Err(missing(format!("the executor can't wall tenants (an older k2-node admin?): {e}"))),
            },
        }
    }

    /// Bytes a tenant's root holds on disk (V74): allocated blocks, never
    /// following a link, bounded (a hostile tree can't make the scan
    /// endless). The owner's home isn't a tenant root (0).
    pub fn tenant_disk_bytes(&self, t: &Tenant) -> u64 {
        if t.is_owner() {
            return 0;
        }
        disk_usage(&t.paths.root())
    }

    /// Is `t` over its share's disk budget (`disk_gb`)? New jobs of an
    /// over-budget tenant wait (`tenant_disk_full`; the queue asks this).
    pub fn tenant_over_disk(&self, t: &Tenant) -> bool {
        self.tenant_over_disk_with(t, |gb| u64::from(gb) << 30)
    }

    fn tenant_over_disk_with(&self, t: &Tenant, budget: impl Fn(u32) -> u64) -> bool {
        if t.is_owner() {
            return false;
        }
        match crate::admin::jobs::tenant_entry(&self.layout().config, t.slot) {
            Ok(e) => self.tenant_disk_bytes(t) > budget(e.disk_gb),
            Err(_) => false,
        }
    }

    /// Is the per-tenant wall really on here (offer `tenant-wall` only
    /// then)? Conservative: Linux only (macOS 0.46.3 is one uid, own
    /// servers only: false); the node runs behind the job-user wall; the
    /// executor says the tenant wall is ready, which includes its runtime
    /// self-test (a unit with the tenant sandbox where root's PID 1 is
    /// invisible, `/dev/shm` private and the `no-lan` filter accepted;
    /// systemd ≥ 247; the user/ACL tools); and every approved server
    /// already on this node has its own user. One local socket round
    /// trip (the self-test runs once per executor process).
    pub async fn tenant_wall_on(&self) -> bool {
        if !cfg!(target_os = "linux") || !matches!(self.wall, Wall::JobUser { .. }) {
            return false;
        }
        let w = match self.job_op(crate::admin::jobs::JobOp::Tenants, vec![], Duration::from_secs(60)).await {
            Ok(crate::admin::jobs::JobAnswer::Tenants(w)) if w.ready => w,
            _ => return false,
        };
        for t in self.tenants() {
            if t.is_owner() {
                continue;
            }
            let approved = crate::admin::jobs::tenant_entry(&self.layout().config, t.slot).is_ok_and(|e| e.may_attach(self.now()));
            if !approved {
                continue;
            }
            let want = crate::admin::jobs::tenant_user_name(t.slot);
            if w.users.get(&t.slot.to_string()) != Some(&want) || crate::admin::server::user_ids(&want).is_none() {
                return false;
            }
        }
        true
    }

    /// One pass of the tenant wall watch: for every other server, hold
    /// its queued jobs (`fairq.set_blocked`) while its wall isn't there
    /// (`tenant_wall_missing`, `team_needs_tenant_wall`: no job wall, no
    /// user, an older executor, an unprepared root) or its root is over
    /// its disk budget (`tenant_disk_full`); clear the hold once it is.
    /// Then the `team` disk scan stops running jobs over budget (V74).
    /// Returns the holds set, by slot.
    pub async fn tenant_wall_pass(&self) -> std::collections::BTreeMap<k2_node_proto::share::Slot, String> {
        let mut held = std::collections::BTreeMap::new();
        for t in self.tenants() {
            if t.is_owner() {
                continue;
            }
            let why = match self.tenant_wall_check(&t).await {
                Err((code, _)) => Some(code),
                Ok(()) if self.tenant_over_disk(&t) => Some(codes::TENANT_DISK_FULL.to_string()),
                Ok(()) => None,
            };
            if let Some(w) = &why {
                held.insert(t.slot, w.clone());
            }
            self.fairq.set_blocked(t.slot, why);
        }
        for (slot, job) in self.team_disk_scan() {
            crate::nlog!("slot {slot} job {job}: stopped, its server is over its disk budget");
        }
        held
    }

    /// Run [`Self::tenant_wall_pass`] now and every `every` (60 s in life;
    /// the node's background start calls this once).
    pub fn spawn_tenant_wall_watch(self: &Arc<Self>, every: Duration) {
        let node = self.clone();
        tokio::spawn(async move {
            loop {
                node.tenant_wall_pass().await;
                tokio::time::sleep(every).await;
            }
        });
    }

    /// The 60 s `team` scan (V74): every `team` tenant over its disk
    /// budget has its running jobs stopped as a **result** (`failed /
    /// tenant_disk_full`, not infra, so it isn't retried into the same
    /// full disk). Returns the stopped `(slot, job_id)`s. The node ticker
    /// calls it.
    pub fn team_disk_scan(&self) -> Vec<(k2_node_proto::share::Slot, String)> {
        self.team_disk_scan_with(|gb| u64::from(gb) << 30)
    }

    fn team_disk_scan_with(&self, budget: impl Fn(u32) -> u64) -> Vec<(k2_node_proto::share::Slot, String)> {
        let mut stopped = Vec::new();
        for t in self.tenants() {
            let Ok(e) = crate::admin::jobs::tenant_entry(&self.layout().config, t.slot) else { continue };
            if t.is_owner() || e.trust != Trust::Team || !self.tenant_over_disk_with(&t, &budget) {
                continue;
            }
            for ((slot, job), j) in self.jobs.lock().unwrap().iter() {
                if *slot == t.slot && j.stop.borrow().is_none() {
                    j.stop.send_replace(Some(StopReason { state: JobState::Failed, reason: codes::TENANT_DISK_FULL.into() }));
                    stopped.push((*slot, job.clone()));
                }
            }
        }
        stopped
    }

    async fn unit_status(&self, unit: &str) -> Option<crate::admin::jobs::UnitStatus> {
        match self.job_op(crate::admin::jobs::JobOp::Status { unit: unit.to_string() }, vec![], Duration::from_secs(30)).await {
            Ok(crate::admin::jobs::JobAnswer::Status(s)) => Some(s),
            _ => None,
        }
    }

    /// Poll a job unit until its command is over.
    async fn unit_wait_end(&self, unit: &str) -> crate::admin::jobs::UnitStatus {
        loop {
            if let Some(s) = self.unit_status(unit).await {
                if s.ended() {
                    return s;
                }
            }
            tokio::time::sleep(self.opts.unit_poll).await;
        }
    }

    pub(super) async fn run_job(self: Arc<Self>, t: Arc<Tenant>, a: Assign, mut stop: watch::Receiver<Option<StopReason>>) {
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        let dirs = JobDirs::new(t.paths.job_dir(&job, gen));
        let unit_mode = matches!(self.wall, Wall::JobUser { .. });
        // A tenant's job runs only behind its own wall (V59): nothing of it
        // is made before that's sure.
        let network = match self.tenant_wall_check(&t).await.and_then(|_| self.job_network(&t)) {
            Ok(n) => n,
            Err((code, detail)) => {
                self.finish_job(&t, &a, None, JobState::Failed, Some(&code), Some(&detail), None, None, None, None, None).await;
                return;
            }
        };
        let mut journal = match dirs.create_for(unit_mode).and_then(|_| JournalWriter::create(&t.paths.journal(&job, gen), a.plan.limits.log_cap_bytes)) {
            Ok(j) => j,
            Err(e) => {
                self.finish_job(&t, &a, None, JobState::Failed, Some("job_dir_failed"), Some(&e), None, None, None, None, None).await;
                return;
            }
        };
        let sys = |j: &mut JournalWriter, msg: &str| {
            let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {msg}\n").as_bytes());
        };

        // Sync.
        let mut src_ran: Option<SrcRan> = None;
        let mut lease: Option<Lease> = None;
        let mut cwd = dirs.root.clone();
        if let Some(src) = &a.plan.src {
            match self.sync_src(&t, &a, &dirs, &mut journal, &mut stop).await {
                Ok(ran) => {
                    cwd = dirs.src.clone();
                    let pc = t.paths.project_cache(&src.project_key);
                    let _ = std::fs::create_dir_all(&pc);
                    let l = self.slots.lock().unwrap().lease(&pc, &src.project_key, src.slots, &src.commit, dirs.private_target.clone());
                    sys(&mut journal, &format!("target dir: {} ({})", l.slot.map(|n| format!("slot {n}")).unwrap_or_else(|| "job-private".into()), l.warmth));
                    src_ran = Some(SrcRan { slot: l.slot, warmth: Some(l.warmth.to_string()), ..ran });
                    lease = Some(l);
                }
                Err((code, detail)) => {
                    let stopped = stop.borrow().clone();
                    let (state, reason) = match stopped {
                        Some(s) => (s.state, s.reason),
                        None => (JobState::Failed, code),
                    };
                    sys(&mut journal, &format!("sync failed: {detail}"));
                    self.finish_job(&t, &a, Some(&dirs), state, Some(&reason), Some(&detail), None, None, Some(journal), None, None).await;
                    return;
                }
            }
        }
        if let Some(rel) = &a.plan.cwd {
            match safe_relative(rel) {
                Some(p) => cwd = cwd.join(p),
                None => {
                    self.finish_job(&t, &a, Some(&dirs), JobState::Failed, Some("cwd_invalid"), Some(rel), src_ran, None, Some(journal), lease, None).await;
                    return;
                }
            }
        }
        // Toolchains: explicit needs (required) + what the repo declares
        // (best effort), installed into the node home, cached.
        let resolved = match self.resolve_toolchains(&a, &dirs, &mut journal, &mut stop).await {
            Ok(r) => r,
            Err((state, code, detail)) => {
                sys(&mut journal, &format!("toolchains: {detail}"));
                self.finish_job(&t, &a, Some(&dirs), state, Some(&code), Some(&detail), src_ran, None, Some(journal), lease, None).await;
                return;
            }
        };
        let target = lease.as_ref().map(|l| l.dir.clone()).unwrap_or_else(|| dirs.private_target.clone());
        let _ = std::fs::create_dir_all(&target);
        // Warm caches by the project key (the controller's hash of its own
        // workspace id and remote; never a name the submitter picks), or
        // the job's own dir when there's no code.
        let cache_dir = match &a.plan.src {
            Some(s) => t.paths.project_cache(&s.project_key),
            None => dirs.private_cache(),
        };
        let (sccache, cargo_home) = runner::cache_paths(&cache_dir);
        for d in [&sccache, &cargo_home] {
            let _ = std::fs::create_dir_all(d);
        }
        // The slot's sccache socket dir (V63). A tenant's is made by
        // tenant.prepare (its group's); the owner's here, for its job user.
        let slot_tmp = t.paths.tmp();
        if t.is_owner() {
            if let Err(e) = crate::util::mkdir_mode(&slot_tmp, if unit_mode { 0o1770 } else { 0o700 }) {
                sys(&mut journal, &format!("preparing {}: {e}", slot_tmp.display()));
            }
        }
        let sccache_socket = t.paths.sccache_socket();
        if unit_mode {
            // The job (k2job, the node's group) writes these.
            let prep = (target.clone(), sccache.clone(), cargo_home.clone(), a.plan.src.is_some().then(|| dirs.src.clone()));
            let r = tokio::task::spawn_blocking(move || prepare_for_job_user(&prep.0, &[&prep.1, &prep.2], prep.3.as_deref())).await;
            if let Ok(Err(e)) | Err(e) = r.map_err(|e| e.to_string()) {
                sys(&mut journal, &format!("preparing the job's folders: {e}"));
            }
        }
        let env = match runner::job_env(&runner::EnvInputs {
            node_home: &self.layout().home,
            job_home: &dirs.home,
            job_tmp: &dirs.tmp,
            target_dir: &target,
            cache_dir: &cache_dir,
            job_id: &job,
            user: &match &self.wall {
                Wall::JobUser { .. } if !t.is_owner() => crate::admin::jobs::tenant_user_name(t.slot),
                Wall::JobUser { user } => user.clone(),
                _ => self.opts.user.clone(),
            },
            extra: &a.plan.env,
            tool_paths: &resolved.path_prepend,
            tool_env: &resolved.env,
            sccache_socket: slot_tmp.is_dir().then_some(sccache_socket.as_path()),
        }) {
            Ok(e) => e,
            Err((code, name)) => {
                self.finish_job(&t, &a, Some(&dirs), JobState::Failed, Some("env_refused"), Some(&format!("{code}: {name}")), src_ran, None, Some(journal), lease, None).await;
                return;
            }
        };
        if stop.borrow().is_some() {
            let s = stop.borrow().clone().unwrap_or(StopReason { state: JobState::Cancelled, reason: "cancelled".into() });
            self.finish_job(&t, &a, Some(&dirs), s.state, Some(&s.reason), None, src_ran, None, Some(journal), lease, None).await;
            return;
        }

        // Smoke lock + spawn, behind the wall.
        let mem_limit = a.plan.limits.mem_bytes.unwrap_or_else(|| self.local.lock().unwrap().policy.mem_cap_bytes);
        self.take_smoke_lock(&job);
        let started = if unit_mode {
            let mut rw = vec![target.clone(), sccache.clone(), cargo_home.clone()];
            if slot_tmp.is_dir() {
                rw.push(slot_tmp.clone());
            }
            self.start_unit(&t, &a, &dirs, &cwd, &env, rw, network, mem_limit).await
        } else {
            self.start_local(&t, &a, &cwd, &env, network)
        };
        let (mut proc, out_pipe, err_pipe) = match started {
            Ok(s) => s,
            Err(e) => {
                sys(&mut journal, &e);
                self.finish_job(&t, &a, Some(&dirs), JobState::Failed, Some("spawn_failed"), Some(&e), src_ran, None, Some(journal), lease, None).await;
                return;
            }
        };
        let pgid = proc.pgid();
        {
            let l = t.ledger.lock().unwrap();
            let _ = l.set_running(&job, gen, pgid.max(0) as u32, src_ran.as_ref(), self.now());
        }
        t.ship.notify_one();

        // Pump output into the journal. `last_out` = ms after `t0` of the
        // last stdout/stderr bytes (the stall watch reads it).
        let journal = Arc::new(Mutex::new(journal));
        let t0 = std::time::Instant::now();
        let last_out = Arc::new(AtomicU64::new(0));
        let mut pumps = Vec::new();
        for (pipe, stream) in [(out_pipe, k2_node_proto::frames::LogStream::Out), (err_pipe, k2_node_proto::frames::LogStream::Err)] {
            if let Some(mut pipe) = pipe {
                let j = journal.clone();
                let tenant = t.clone();
                let last = last_out.clone();
                pumps.push(tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut buf = vec![0u8; k2_node_proto::frames::MAX_LOG_CHUNK];
                    loop {
                        match pipe.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let _ = j.lock().unwrap().append(stream, &buf[..n]);
                                last.store(t0.elapsed().as_millis() as u64, Ordering::Relaxed);
                                tenant.ship.notify_one();
                            }
                        }
                    }
                }));
            }
        }

        // Wait: exit, stop, timeout, memory.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(a.plan.limits.max_secs.max(1));
        // Only a plain process without a cgroup leaf needs the RSS sampler
        // (macOS); a leaf or a job unit has a hard `memory.max`.
        let mem_cap = proc.needs_rss_sampler().then_some(mem_limit);
        let mut rss_tick = tokio::time::interval(self.opts.rss_every);
        rss_tick.tick().await;
        // Stall watch: no output for `stall_secs` ends the job (a hung test
        // shouldn't hold the slot until the wall clock).
        let stall_ms = a.plan.stall_secs.filter(|s| *s > 0).map(|s| s.saturating_mul(1000));
        let mut stall_tick = tokio::time::interval(Duration::from_millis(stall_ms.map(|m| (m / 4).clamp(50, 1000)).unwrap_or(1000)));
        stall_tick.tick().await;
        let mut ended_by: Option<StopReason> = None;
        let mut status = None;
        loop {
            tokio::select! {
                s = self.proc_wait(&mut proc) => { status = s; break; }
                r = stop.changed() => {
                    if r.is_ok() {
                        if let Some(s) = stop.borrow().clone() { ended_by = Some(s); break; }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    ended_by = Some(StopReason { state: JobState::Timeout, reason: "timeout".into() });
                    break;
                }
                _ = stall_tick.tick(), if stall_ms.is_some() => {
                    let quiet = (t0.elapsed().as_millis() as u64).saturating_sub(last_out.load(Ordering::Relaxed));
                    if stall_ms.is_some_and(|m| quiet >= m) {
                        ended_by = Some(StopReason { state: JobState::Timeout, reason: "stalled".into() });
                        break;
                    }
                }
                _ = rss_tick.tick(), if mem_cap.is_some() => {
                    let sampler = self.opts.rss.clone();
                    let rss = tokio::task::spawn_blocking(move || sampler.group_rss(pgid)).await.ok().flatten();
                    if let (Some(r), Some(cap)) = (rss, mem_cap) {
                        if r > cap {
                            ended_by = Some(StopReason { state: JobState::Failed, reason: "mem_cap".into() });
                            break;
                        }
                    }
                }
            }
        }
        if let Some(s) = &ended_by {
            {
                let mut j = journal.lock().unwrap();
                let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] stopping job: {}\n", s.reason).as_bytes());
            }
            self.proc_stop(&mut proc).await;
            status = self.proc_after_stop(&mut proc).await;
        }
        let _ = t.ledger.lock().unwrap().set_state(&job, gen, JobState::Finishing, None, None);
        t.ship.notify_one();
        // V24: an OOM kill is `mem_cap` on every platform (the leaf's or
        // the unit's `memory.events`, or the unit's `oom-kill` result).
        let oom = proc.oom();
        self.proc_reap(&mut proc).await;
        for mut p in pumps {
            if tokio::time::timeout(Duration::from_secs(2), &mut p).await.is_err() {
                // Something outside the group still holds the pipe.
                p.abort();
                let _ = p.await;
            }
        }
        proc.cleanup();
        let exit = status;
        let mut journal = Arc::try_unwrap(journal).ok().map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()));
        let (state, reason) = match ended_by {
            Some(s) => (s.state, Some(s.reason)),
            None if oom => (JobState::Failed, Some("mem_cap".to_string())),
            None => (JobState::Done, None),
        };
        if reason.as_deref() == Some("mem_cap") {
            if let Some(j) = journal.as_mut() {
                let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {}\n", mem_cap_detail(mem_limit)).as_bytes());
            }
        }
        // Artifacts and test results: after the command (and its group)
        // are gone, before the warm slot goes back to the pool.
        let outputs = if a.plan.artifacts.is_empty() {
            None
        } else {
            let (globs, cwd, target, keep) = (a.plan.artifacts.clone(), cwd.clone(), target.clone(), crate::artifacts::dir(&t.paths.artifacts(), &job, gen));
            let o = tokio::task::spawn_blocking(move || {
                crate::artifacts::collect(&globs, &crate::artifacts::Roots { cwd: &cwd, target: Some(&target) }, &keep)
            })
            .await
            .ok();
            if let (Some(j), Some(o)) = (journal.as_mut(), o.as_ref()) {
                let mut line = |m: String| {
                    let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {m}\n").as_bytes());
                };
                let bytes: u64 = o.artifacts.iter().map(|x| x.bytes).sum();
                line(format!("artifacts: kept {} file(s), {} KB", o.artifacts.len(), bytes.div_ceil(1024)));
                if let Some(t) = &o.tests {
                    line(format!("tests: {} passed, {} failed, {} errors, {} skipped", t.passed, t.failed, t.errors, t.skipped));
                }
                for n in &o.notes {
                    line(format!("artifacts: {n}"));
                }
            }
            o
        };
        let detail = match reason.as_deref() {
            Some("stalled") => Some(format!("no output for {}s", a.plan.stall_secs.unwrap_or(0))),
            Some("mem_cap") => Some(mem_cap_detail(mem_limit)),
            Some(refusal::DISK_LOW) => {
                let d = super::disk::disk_low_detail(self.disk_free_bytes(), self.local.lock().unwrap().policy.disk_floor_bytes);
                if let Some(j) = journal.as_mut() {
                    let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {d}\n").as_bytes());
                }
                Some(d)
            }
            _ => None,
        };
        self.finish_job(&t, &a, Some(&dirs), state, reason.as_deref(), detail.as_deref(), src_ran, exit, journal, lease, outputs).await;
    }

    /// A plain child process (no wall, or the Seatbelt profile around it).
    fn start_local(&self, t: &Tenant, a: &Assign, cwd: &Path, env: &[(String, String)], network: Network) -> Result<(Proc, Option<OutPipe>, Option<OutPipe>), String> {
        let network_none = network != Network::On;
        if network_none && !self.wall.network_none() {
            return Err(refusal::NETWORK_NONE_MESSAGE.to_string());
        }
        if !t.is_owner() && !matches!(self.wall, Wall::Seatbelt) && !self.dev_unwalled() {
            return Err(format!("{}: a tenant's job never runs without its wall", codes::TENANT_WALL_MISSING));
        }
        let key = t.job_key(&a.fence.job_id, a.fence.generation);
        let leaf: Option<Leaf> = self.cgroups.as_ref().and_then(|cg| {
            cg.create_leaf(&key, a.plan.limits.cpu_millis, a.plan.limits.mem_bytes).map_err(|e| crate::nlog!("cgroup leaf for {key}: {e}")).ok()
        });
        let argv = match &self.wall {
            Wall::Seatbelt => {
                let (dirs, sockets) = self.seatbelt_deny();
                let who = crate::wall::SeatbeltTenant { slot: t.slot, home: self.layout().home.clone(), tenants_root: self.layout().tenants_root() };
                crate::wall::seatbelt_argv(&crate::wall::seatbelt_tenant_profile(&dirs, &sockets, network_none, &who), &a.plan.argv)
            }
            _ => a.plan.argv.clone(),
        };
        match runner::spawn(&argv, env, cwd, leaf.as_ref()) {
            Ok(runner::Spawned { mut child, pgid }) => {
                let out = child.stdout.take().map(|p| Box::new(p) as OutPipe);
                let err = child.stderr.take().map(|p| Box::new(p) as OutPipe);
                Ok((Proc::Local { child, pgid, leaf }, out, err))
            }
            Err(e) => {
                if let Some(l) = &leaf {
                    l.remove();
                }
                Err(e)
            }
        }
    }

    /// Linux wall: the executor starts the job as the job user in its own
    /// unit; we keep the read ends of its stdout/stderr pipes.
    #[allow(clippy::too_many_arguments)]
    async fn start_unit(
        &self,
        t: &Tenant,
        a: &Assign,
        dirs: &JobDirs,
        cwd: &Path,
        env: &[(String, String)],
        rw: Vec<PathBuf>,
        network: Network,
        mem_limit: u64,
    ) -> Result<(Proc, Option<OutPipe>, Option<OutPipe>), String> {
        use crate::admin::jobs::{JobAnswer, JobOp, JobStart};
        let key = t.job_key(&a.fence.job_id, a.fence.generation);
        crate::wall::write_exec_spec(&dirs.root, &crate::wall::ExecSpec { argv: a.plan.argv.clone(), env: env.to_vec(), cwd: cwd.to_path_buf() })?;
        let (out_r, out_w) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
        let (err_r, err_w) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
        let start = JobStart {
            key,
            root: dirs.root.clone(),
            rw,
            network_none: network == Network::None,
            mem_bytes: Some(mem_limit),
            cpu_millis: a.plan.limits.cpu_millis,
            stop_grace_secs: self.opts.cancel_grace.as_secs().max(1),
            tenant: t.slot,
            no_lan: network == Network::NoLan,
        };
        // The write ends go to root (and the unit); ours close with the call.
        let ans = self.job_op(JobOp::Start(start), vec![out_w.into(), err_w.into()], Duration::from_secs(60)).await;
        let unit = match ans {
            Ok(JobAnswer::Started { unit }) => unit,
            Ok(JobAnswer::Refused(r)) => return Err(format!("the job wall refused to start it: {}: {}", r.code, r.message)),
            Ok(other) => return Err(format!("unexpected executor answer: {other:?}")),
            Err(e) => return Err(format!("couldn't start the job unit: {e}")),
        };
        let rx = |r: std::io::PipeReader| -> Option<OutPipe> {
            tokio::net::unix::pipe::Receiver::from_owned_fd(std::os::fd::OwnedFd::from(r)).ok().map(|p| Box::new(p) as OutPipe)
        };
        Ok((Proc::Unit { unit, last: None }, rx(out_r), rx(err_r)))
    }

    /// The command ended: its exit (code or signal).
    async fn proc_wait(&self, p: &mut Proc) -> Option<ExitInfo> {
        match p {
            Proc::Local { child, .. } => child.wait().await.ok().map(|s| {
                use std::os::unix::process::ExitStatusExt;
                ExitInfo { code: s.code(), signal: s.signal() }
            }),
            Proc::Unit { unit, last } => {
                let s = self.unit_wait_end(unit).await;
                let e = s.exit().map(|(code, signal)| ExitInfo { code, signal });
                *last = Some(s);
                e
            }
        }
    }

    /// SIGTERM everything, SIGKILL after the grace period.
    async fn proc_stop(&self, p: &mut Proc) {
        match p {
            Proc::Local { child, pgid, leaf } => runner::stop_group(child, *pgid, leaf.as_ref(), self.opts.cancel_grace).await,
            Proc::Unit { unit, .. } => {
                let _ = self.job_op(crate::admin::jobs::JobOp::Stop { unit: unit.clone() }, vec![], Duration::from_secs(30)).await;
            }
        }
    }

    /// The exit after a stop (bounded: a unit we can't read ends anyway).
    async fn proc_after_stop(&self, p: &mut Proc) -> Option<ExitInfo> {
        let cap = self.opts.cancel_grace + Duration::from_secs(30);
        tokio::time::timeout(cap, self.proc_wait(p)).await.ok().flatten()
    }

    /// After the command: nothing it started may live on.
    async fn proc_reap(&self, p: &mut Proc) {
        match p {
            Proc::Local { pgid, leaf, .. } => runner::reap_group(*pgid, leaf.as_ref()),
            Proc::Unit { unit, .. } => {
                // `release` doesn't wait (V58): poll until the unit is
                // stopped, so nothing of the job still runs while the node
                // reads its tree (artifacts, slots), then forget it.
                if let Err(e) = self.job_op(crate::admin::jobs::JobOp::Release { unit: unit.clone() }, vec![], Duration::from_secs(60)).await {
                    crate::nlog!("release {unit}: {e}");
                }
                let deadline = tokio::time::Instant::now() + self.opts.cancel_grace + Duration::from_secs(60);
                loop {
                    match self.unit_status(unit).await {
                        Some(s) if s.loaded && s.active_state != "inactive" && s.active_state != "failed" => {}
                        Some(s) => {
                            if s.loaded {
                                let _ = self.job_op(crate::admin::jobs::JobOp::Release { unit: unit.clone() }, vec![], Duration::from_secs(60)).await;
                            }
                            break;
                        }
                        None => {}
                    }
                    if tokio::time::Instant::now() >= deadline {
                        crate::nlog!("{unit} didn't stop within its stop timeout");
                        break;
                    }
                    tokio::time::sleep(self.opts.unit_poll).await;
                }
            }
        }
    }

    /// The job's toolchains. `Err((state, reason, detail))` ends the job.
    async fn resolve_toolchains(
        self: &Arc<Self>,
        a: &Assign,
        dirs: &JobDirs,
        journal: &mut JournalWriter,
        stop: &mut watch::Receiver<Option<StopReason>>,
    ) -> Result<crate::toolchain::Resolved, (JobState, String, String)> {
        use crate::toolchain;
        // Same parser as the controller: an unsupported or malformed need
        // fails the job before anything runs (never dropped).
        let explicit = k2_node_proto::needs::parse_all(&a.plan.needs).map_err(|e| (JobState::Failed, e.code.to_string(), e.message))?;
        let repo_files = a.plan.src.is_some() && self.local.lock().unwrap().policy.repo_toolchains;
        let (repo, skipped, rust_file) = if repo_files {
            let (r, s) = toolchain::declared_in_repo(&dirs.src);
            (r, s, toolchain::rust_toolchain_file(&dirs.src))
        } else {
            (Vec::new(), Vec::new(), None)
        };
        let mut log = |m: &str| {
            let _ = journal.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {m}
").as_bytes());
        };
        for s in &skipped {
            log(&format!("toolchains: skipped {s}"));
        }
        if explicit.is_empty() && repo.is_empty() && rust_file.is_none() {
            return Ok(toolchain::Resolved::default());
        }
        let wanted: Vec<String> = explicit
            .iter()
            .map(|n| n.token())
            .chain(repo.iter().map(|(n, src)| format!("{} ({src})", n.token())))
            .chain(rust_file.iter().map(|r| format!("rust@{} (rust-toolchain)", r.channel)))
            .collect();
        log(&format!("toolchains: {}", wanted.join(", ")));
        let _guard = self.tool_lock.lock().await;
        let tc = self.toolchains();
        let r = {
            let fut = tc.resolve(&explicit, &repo, rust_file.as_ref(), &mut log);
            tokio::pin!(fut);
            let mut stop_open = true;
            loop {
                tokio::select! {
                    r = &mut fut => break Some(r),
                    c = stop.changed(), if stop_open => {
                        if c.is_err() {
                            stop_open = false;
                        } else if stop.borrow().is_some() {
                            break None;
                        }
                    }
                }
            }
        };
        let Some(r) = r else {
            let s = stop.borrow().clone().unwrap_or(StopReason { state: JobState::Cancelled, reason: "cancelled".into() });
            return Err((s.state, s.reason, "stopped while installing toolchains".into()));
        };
        match r {
            Ok(res) => {
                for n in &res.notes {
                    log(&format!("toolchains: {n}"));
                }
                if res.installed_any {
                    let node = self.clone();
                    tokio::spawn(async move { node.probe_tools().await });
                }
                Ok(res)
            }
            Err((code, detail)) => Err((JobState::Failed, code.to_string(), detail)),
        }
    }

    fn take_smoke_lock(&self, job: &str) {
        let path = self.local.lock().unwrap().policy.write_smoke_lock.clone();
        if let Some(p) = path {
            let now = self.now();
            self.smoke.lock().unwrap().acquire(&p, job, now);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_job(
        self: &Arc<Self>,
        t: &Tenant,
        a: &Assign,
        dirs: Option<&JobDirs>,
        state: JobState,
        reason: Option<&str>,
        detail: Option<&str>,
        src: Option<SrcRan>,
        exit: Option<ExitInfo>,
        journal: Option<JournalWriter>,
        lease: Option<Lease>,
        outputs: Option<Outputs>,
    ) {
        let job = &a.fence.job_id;
        let gen = a.fence.generation;
        let (log_sha, log_bytes, truncated, log_seq) = match journal {
            Some(mut j) => match j.finish() {
                Ok((s, b, t)) => (s, b, t, j.seq),
                Err(_) => (String::new(), j.bytes, false, j.seq),
            },
            None => (crypto::sha256_hex(b""), 0, false, 0),
        };
        if let (Some(l), Some(s)) = (&lease, &a.plan.src) {
            let pc = t.paths.project_cache(&s.project_key);
            self.slots.lock().unwrap().release(&pc, l, Some(&s.commit));
        }
        let now = self.now();
        let started_at = t.ledger.lock().unwrap().get(job, gen).ok().flatten().and_then(|r| r.started_at);
        let pin = t.pin.lock().unwrap().clone();
        let mut env_names: Vec<String> = k2_node_proto::env::NODE_SET_VARS
            .iter()
            .filter(|n| !matches!(**n, "K2_SECRETS_DIR"))
            .map(|s| s.to_string())
            .chain(a.plan.env.keys().cloned())
            .collect();
        env_names.sort();
        let receipt = Receipt {
            v: 1,
            job_id: job.clone(),
            attempt: a.fence.attempt,
            generation: gen,
            plan_digest: a.fence.plan_digest.clone(),
            node_fp: self.key.fingerprint(),
            controller_fp: pin.map(|p| p.controller_fp).unwrap_or_default(),
            workspace_id: a.plan.workspace_id.clone(),
            argv_sha256: k2_node_proto::frames::argv_sha256(&a.plan.argv),
            env_names,
            src: src.clone(),
            tools: self.tools.lock().unwrap().clone(),
            boot_id: self.opts.clock.boot_id(),
            started_at,
            ended_at: now,
            state,
            exit: exit.clone(),
            log: ReceiptLog { sha256: log_sha, bytes: log_bytes, truncated },
            artifacts: outputs.as_ref().map(|o| o.artifacts.clone()).unwrap_or_default(),
        };
        let signed = SignedReceipt::sign(&self.key, &receipt);
        if let Err(e) = &signed {
            crate::nlog!("receipt for {job}: {e}");
        }
        let record = || self.record_finish(t, job, gen, state, reason, detail, exit.as_ref(), log_seq, now, src.as_ref(), outputs.as_ref(), signed.as_ref().ok());
        let mut recorded = record();
        let gone = || {
            self.jobs.lock().unwrap().remove(&(t.slot, job.clone()));
            if self.jobs.lock().unwrap().is_empty() {
                self.smoke.lock().unwrap().release();
            }
            self.jobs_changed();
        };
        let landed = recorded.is_ok();
        if landed {
            gone();
        }
        // The big parts of the dir go now (module `disk`): before any
        // retry, since a full disk is the usual reason a write failed.
        if let Some(d) = dirs {
            self.strip_job_dir(t, &d.root, a.plan.src.as_ref().map(|s| s.project_key.as_str()), super::disk::JOB_END_KEEP).await;
        }
        // A job never stays `running` because its finish didn't land: the
        // job stays counted (nothing new takes its place) until it does.
        let mut tries = 0u32;
        while let Err(e) = &recorded {
            tries += 1;
            if tries == 1 || tries % 30 == 0 {
                crate::nlog!("ledger finish {job}: {e} (retrying)");
            }
            if tries > FINISH_RETRIES {
                crate::nlog!("ledger finish {job}: gave up after {tries} tries: {e}");
                break;
            }
            tokio::time::sleep(self.opts.disk_poll).await;
            recorded = record();
        }
        if !landed {
            gone();
        }
        if let Some(l) = lease.as_ref().filter(|l| l.slot.is_some()) {
            self.after_slot_use(&l.dir).await;
        }
    }

    /// The terminal row and its receipt, under one ledger lock (the
    /// shipper never sees a terminal row without its receipt).
    #[allow(clippy::too_many_arguments)]
    fn record_finish(
        &self,
        t: &Tenant,
        job: &str,
        gen: u32,
        state: JobState,
        reason: Option<&str>,
        detail: Option<&str>,
        exit: Option<&ExitInfo>,
        log_seq: u64,
        now: i64,
        src: Option<&SrcRan>,
        outputs: Option<&Outputs>,
        signed: Option<&SignedReceipt>,
    ) -> Result<(), String> {
        #[cfg(test)]
        if job == tests::FINISH_FAILS_JOB && tests::FINISH_FAILS.load(Ordering::SeqCst) > 0 {
            tests::FINISH_FAILS.fetch_sub(1, Ordering::SeqCst);
            return Err("database or disk is full (test)".into());
        }
        let l = t.ledger.lock().unwrap();
        if let Some(s) = src {
            l.set_src(job, gen, s)?;
        }
        if let Some(o) = outputs {
            l.set_outputs(job, gen, o)?;
        }
        l.finish(job, gen, state, reason, detail, exit, log_seq, now)?;
        if let Some(r) = signed {
            l.set_receipt(job, gen, r)?;
        }
        Ok(())
    }
}

/// Ledger finish retries (`disk_poll` apart: about an hour by default).
const FINISH_RETRIES: u32 = 360;

/// Allocated bytes under `root` (V74), no link followed, at most
/// 2 million entries and 256 levels.
pub fn disk_usage(root: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0u64;
    let mut seen = 0u64;
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    while let Some((p, depth)) = stack.pop() {
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        total += m.blocks() * 512;
        seen += 1;
        if seen > 2_000_000 {
            break;
        }
        if m.is_dir() && depth < 256 {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| (e.path(), depth + 1)));
            }
        }
    }
    total
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::cgroup::Cgroups;

    /// The job whose ledger finish fails [`FINISH_FAILS`] more times.
    pub const FINISH_FAILS_JOB: &str = "finish-fails";
    pub static FINISH_FAILS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// V74: a `team` tenant over its disk budget has its running job
    /// stopped as a result (`failed / tenant_disk_full`); an `own` tenant
    /// over budget and a team tenant under it are left alone. The budget
    /// is scaled down here (1 "GB" = 4 KiB) so a small file crosses it.
    #[tokio::test]
    async fn the_team_disk_scan_stops_an_over_budget_tenants_job() {
        use k2_node_proto::share::{Network, ServersFile, ShareLimits};
        let d = crate::util::temp_dir("disk-scan");
        let layout = crate::paths::Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        let mut f = ServersFile::new(&"a".repeat(64), "o", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1, "local:t");
        for (slot, trust) in [(1u8, Trust::Team), (2, Trust::Own), (3, Trust::Team)] {
            let mut e = f.servers[0].clone();
            e.slot = slot;
            e.label = format!("s{slot}");
            e.fp_prefix = format!("{:016x}", slot);
            e.trust = trust;
            e.set_limits(&ShareLimits::new_server(trust, false));
            e.disk_gb = 1;
            f.servers.push(e);
        }
        std::fs::write(layout.config.join(k2_node_proto::share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
        std::fs::write(layout.policy(), "availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\nmax_parallel = 4\n").unwrap();
        let pin = |id: &str| crate::identity::Pin {
            controller_fp: "c".repeat(64),
            controller_public_key_pem: String::new(),
            routes: vec![],
            node_id: id.into(),
            name: "n".into(),
            enrolled_at: 1,
            sas: String::new(),
            revoked: false,
            revoked_reason: None,
            labels: Default::default(),
            confirmed: true,
        };
        for slot in [1u8, 2, 3] {
            layout.tenant(slot).ensure_state().unwrap();
            crate::identity::write_pin(&layout.tenant(slot).pin(), &pin("node-b")).unwrap();
        }
        let mut o = super::super::NodeOptions::new(layout.clone());
        o.dev = true;
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.cancel_grace = Duration::from_millis(200);
        o.wall = Some(Wall::None { why: "t".into() });
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") });
        let n = Node::open(o).await.unwrap();
        let mut tens = Vec::new();
        for s in [1u8, 2, 3] {
            tens.push(n.add_tenant(s).await.unwrap());
        }
        // Approved, protocol 3, a session stand-in each (as S1 sets up).
        let mut sessions = Vec::new();
        for s in [0u8, 1, 2, 3] {
            let limits = ShareLimits { max_running: 2, ..ShareLimits::new_server(Trust::Own, false) };
            n.fairq.set_share(s, Some(crate::fairq::ShareInfo { label: format!("s{s}"), limits, status: k2_node_proto::share::ServerStatus::Active }));
            n.fairq.set_protocol(s, Some(3));
        }
        for t in &tens {
            let (tx, rx) = tokio::sync::mpsc::channel(256);
            t.hub.attach(tx);
            sessions.push(rx);
        }
        // Slots 1 (team) and 2 (own) hold 4 MiB; slot 3 (team) only its
        // folders. The test budget is 1 MiB a "GB": far above a job's
        // folders (Linux counts 4 KiB per directory, APFS nothing; at 4 KiB
        // a GB slot 3's started job alone was over on Linux), far below
        // the 4 MiB.
        for s in [1u8, 2] {
            let p = layout.tenant(s).cache().join("big");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![7u8; 4 << 20]).unwrap();
        }
        let budget = |gb: u32| u64::from(gb) << 20;
        assert!(n.tenant_disk_bytes(&tens[0]) >= 4 << 20);
        assert!(n.tenant_over_disk_with(&tens[0], budget) && n.tenant_over_disk_with(&tens[1], budget));
        assert!(!n.tenant_over_disk_with(&tens[2], budget), "slot 3 is under");
        assert!(!n.tenant_over_disk(&tens[0]), "40 real GB is far away");
        assert!(!n.tenant_wall_on().await, "an unwalled node never offers tenant-wall");
        let job = |t: &Arc<Tenant>| {
            let mut a = crate::ledger::tests::assign("j", 1);
            a.plan.node_id = "node-b".into();
            a.plan.argv = vec!["/bin/sleep".into(), "30".into()];
            a.fence.plan_digest = a.plan.digest().unwrap();
            assert_eq!(n.on_assign(t, a), Some(("j".into(), 1)));
        };
        for t in &tens {
            job(t);
        }
        n.fairq.tick(&n);
        assert_eq!(n.running_by_tenant().len(), 3, "all three started: {:?}", n.running_by_tenant());
        tokio::time::sleep(Duration::from_millis(300)).await;
        let stopped = n.team_disk_scan_with(budget);
        assert_eq!(stopped, vec![(1u8, "j".to_string())], "only the over-budget team tenant's job");
        let t0 = std::time::Instant::now();
        let row = loop {
            let r = tens[0].ledger.lock().unwrap().get("j", 1).unwrap().unwrap();
            if r.state.is_terminal() {
                break r;
            }
            assert!(t0.elapsed() < Duration::from_secs(20));
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!((row.state, row.reason.as_deref()), (JobState::Failed, Some(codes::TENANT_DISK_FULL)));
        for t in &tens[1..] {
            assert!(!t.ledger.lock().unwrap().get("j", 1).unwrap().unwrap().state.is_terminal(), "slot {} untouched", t.slot);
        }
        n.stop_all(JobState::Cancelled, "test_over");
    }

    /// V24 on an unwalled Linux node: the job's delegated cgroup leaf
    /// counts an OOM kill in `memory.events`, and that's `mem_cap`.
    #[tokio::test]
    async fn a_leaf_oom_kill_reads_as_oom() {
        let root = crate::util::temp_dir("leaf-oom");
        let unit = root.join("system.slice/k2-node.service");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("cgroup.subtree_control"), "").unwrap();
        let cg = Cgroups::setup(&root, "/system.slice/k2-node.service", 1).unwrap();
        let leaf = cg.create_leaf("j-g1", None, Some(1 << 20)).unwrap();
        let child = tokio::process::Command::new("/usr/bin/true").spawn().unwrap();
        std::fs::write(leaf.dir.join("memory.events"), "low 0\nhigh 0\nmax 3\noom 1\noom_kill 0\n").unwrap();
        let p = Proc::Local { child, pgid: 0, leaf: Some(leaf) };
        assert!(!p.oom(), "oom without a kill isn't a kill");
        let Proc::Local { leaf: Some(l), .. } = &p else { unreachable!() };
        std::fs::write(l.dir.join("memory.events"), "low 0\nhigh 0\nmax 9\noom 2\noom_kill 1\n").unwrap();
        assert!(p.oom());
        assert!(!p.needs_rss_sampler(), "a leaf has a hard memory.max");
        let unit = Proc::Unit {
            unit: "k2job-j-g1.service".into(),
            last: Some(crate::admin::jobs::parse_show("LoadState=loaded\nActiveState=failed\nResult=oom-kill\n")),
        };
        assert!(unit.oom() && unit.pgid() == 0 && !unit.needs_rss_sampler());
        assert!(mem_cap_detail(64 << 20).contains("64 MiB") && mem_cap_detail(64 << 20).contains("--mem"));
    }
}
