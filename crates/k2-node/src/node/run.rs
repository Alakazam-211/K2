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
use k2_node_proto::frames::{Assign, ExitInfo, Frame, JobState, Receipt, ReceiptLog, SignedReceipt, SrcRan};
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
    /// `cg`: the unit's cgroup dir under `/sys/fs/cgroup`, once the
    /// executor said it (CE-14, the CE-10 stall rule).
    Unit { unit: String, last: Option<crate::admin::jobs::UnitStatus>, cg: Option<PathBuf> },
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

    /// Where the job's memory and CPU are read.
    fn usage_source(&self) -> crate::jobstat::Source {
        use crate::jobstat::Source;
        match self {
            Proc::Local { leaf: Some(l), .. } => Source::Cgroup(l.dir.clone()),
            Proc::Local { leaf: None, pgid, .. } => Source::Group(*pgid),
            Proc::Unit { cg: Some(c), .. } => Source::Cgroup(c.clone()),
            Proc::Unit { cg: None, .. } => Source::None,
        }
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
    let gb = cap as f64 / (1u64 << 30) as f64;
    format!("stopped: the job used more than its memory limit of {gb:.1} GB ({} MiB, its --mem or the node's share); ask for more with --mem", cap >> 20)
}

/// [`mem_cap_detail`] with the peak the node measured (CE-14): "… used
/// 25.3G of 24.0G at its peak".
pub fn mem_cap_detail_used(cap: u64, peak: Option<u64>) -> String {
    match peak {
        Some(p) => format!("{} (used {} of {} at its peak)", mem_cap_detail(cap), crate::jobstat::gib(p), crate::jobstat::gib(cap)),
        None => mem_cap_detail(cap),
    }
}

/// CE-15 + loopback ports (0.46.7): the job's CPUs as thread-count hints
/// (each only when the plan's env doesn't set it: a caller's `--env` or a
/// controller's hint wins), `K2_JOB_CPUS`, and its own port block.
pub fn with_job_hints(
    mut env: Vec<(String, String)>,
    plan_env: &std::collections::BTreeMap<String, String>,
    cpus: u64,
    port_block: Option<u16>,
) -> Vec<(String, String)> {
    use k2_node_proto::env as e;
    let cpus = cpus.max(1).to_string();
    env.retain(|(k, _)| k != e::ENV_JOB_CPUS && k != e::ENV_JOB_PORT_BASE && k != e::ENV_JOB_PORT_COUNT);
    env.push((e::ENV_JOB_CPUS.into(), cpus.clone()));
    for v in e::CPU_HINT_VARS {
        if !plan_env.contains_key(*v) {
            env.retain(|(k, _)| k != v);
            env.push((v.to_string(), cpus.clone()));
        }
    }
    if let Some(b) = port_block {
        let base = e::JOB_PORT_FIRST + b * e::JOB_PORT_BLOCK;
        env.push((e::ENV_JOB_PORT_BASE.into(), base.to_string()));
        env.push((e::ENV_JOB_PORT_COUNT.into(), e::JOB_PORT_BLOCK.to_string()));
    }
    env
}

/// A partial line goes to the journal after this long without more bytes
/// (a prompt or a progress bar still shows).
pub const LINE_IDLE: Duration = Duration::from_millis(200);
/// A line this long goes to the journal without its newline.
pub const LINE_MAX: usize = 64 * 1024;

/// One stream's bytes held to a line end (0.46.7: line-atomic logs, so a
/// machine-readable line is never split by the other stream's bytes).
#[derive(Default)]
pub struct LineBuf {
    held: Vec<u8>,
}

impl LineBuf {
    /// Add bytes; returns what may go to the journal now: everything up
    /// to the last newline, or all of it once [`LINE_MAX`] is held.
    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        self.held.extend_from_slice(data);
        if self.held.len() >= LINE_MAX {
            return self.take_all();
        }
        match self.held.iter().rposition(|b| *b == b'\n') {
            Some(i) => {
                let rest = self.held.split_off(i + 1);
                std::mem::replace(&mut self.held, rest)
            }
            None => Vec::new(),
        }
    }

    pub fn pending(&self) -> bool {
        !self.held.is_empty()
    }

    pub fn take_all(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }

    /// Zero what's held (a secret job's buffer, SE23).
    pub fn wipe(&mut self) {
        k2_redact::wipe(&mut self.held);
    }
}

/// `35m`, `2h05m`, `40s` for log lines.
fn human_secs(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
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

    pub(super) fn tenant_over_disk_with(&self, t: &Tenant, budget: impl Fn(u32) -> u64) -> bool {
        if t.is_owner() {
            return false;
        }
        match crate::admin::jobs::tenant_entry(&self.layout().config, t.slot) {
            Ok(e) => {
                // A walk that hit its caps counts as over (review B F1).
                let (bytes, capped) = disk_usage_walk(&t.paths.root());
                if capped {
                    crate::nlog!("warning: server slot {}'s folder hit the disk walk's cap ({WALK_MAX_DEPTH} levels or {WALK_MAX_ENTRIES} entries, {} GB counted so far); it counts as over its share", t.slot, bytes >> 30);
                }
                capped || bytes > budget(e.disk_gb)
            }
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
        // A secret job (secret-env, review B-H1) keeps nothing where another
        // workspace's job (same uid on the owner's slot) could read it: its
        // journal is node-only (0600), it gets no warm slot and no shared
        // project cache or sccache server, and its whole tree goes when it
        // ends, before another workspace's job may start.
        let secret_job = !a.plan.secret_env.is_empty();
        let journal_path = t.paths.journal(&job, gen);
        let mut journal = match dirs.create_for(unit_mode).and_then(|_| JournalWriter::create(&journal_path, a.plan.limits.log_cap_bytes)) {
            Ok(j) => j,
            Err(e) => {
                self.finish_job(&t, &a, None, JobState::Failed, Some("job_dir_failed"), Some(&e), None, None, None, None, None).await;
                return;
            }
        };
        if secret_job {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&journal_path, std::fs::Permissions::from_mode(0o600)) {
                let e = format!("journal mode: {e}");
                self.finish_job(&t, &a, Some(&dirs), JobState::Failed, Some("job_dir_failed"), Some(&e), None, None, Some(journal), None, None).await;
                return;
            }
        }
        let sys = |j: &mut JournalWriter, msg: &str| {
            let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {msg}\n").as_bytes());
        };

        // Sync.
        let mut src_ran: Option<SrcRan> = None;
        let mut lease: Option<Lease> = None;
        let mut cwd = dirs.root.clone();
        // The main checkout (`src/`, or `src/<src_path>` with `--with`).
        let mut main_checkout = dirs.src.clone();
        if let Some(src) = &a.plan.src {
            match self.sync_src(&t, &a, &dirs, &mut journal, &mut stop).await {
                Ok((ran, main_dir)) if secret_job => {
                    // No warm slot: the build's outputs may hold the value.
                    main_checkout = main_dir.clone();
                    cwd = main_dir;
                    sys(&mut journal, "target dir: job-private (a secret job never uses a warm slot)");
                    src_ran = Some(SrcRan { slot: None, warmth: Some("private".into()), ..ran });
                }
                Ok((ran, main_dir)) => {
                    main_checkout = main_dir.clone();
                    cwd = main_dir;
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
            // A secret job's cargo home and sccache dir are its own.
            Some(_) if secret_job => dirs.private_cache(),
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
        // A secret job's sccache server is its own (in its own tmp, killed
        // with its unit), never the slot's shared one.
        let sccache_socket = if secret_job { dirs.tmp.join("sccache.sock") } else { t.paths.sccache_socket() };
        // `--cache NAME` (protocol 6): the workspace's folder that outlives
        // the job. Never for a secret job (it keeps nothing between runs);
        // the server refuses that too. The hold goes back in `finish_job`.
        let mut job_cache_dir: Option<PathBuf> = None;
        if let Some(name) = a.plan.cache.clone() {
            if secret_job {
                let detail = "a job with secret env keeps nothing between runs: no --cache".to_string();
                sys(&mut journal, &detail);
                self.finish_job(&t, &a, Some(&dirs), JobState::Failed, Some(k2_node_proto::cache::codes::CACHE_WITH_SECRET), Some(&detail), src_ran, None, Some(journal), lease, None).await;
                return;
            }
            let (n, tt, ws) = (self.clone(), t.clone(), a.plan.workspace_id.clone());
            let opened = tokio::task::spawn_blocking(move || n.job_cache_open(&tt, &ws, &name, unit_mode)).await.map_err(|e| e.to_string()).and_then(|r| r);
            match opened {
                Ok((held, notes)) => {
                    for l in notes {
                        sys(&mut journal, &l);
                    }
                    sys(&mut journal, &format!("{}={}", k2_node_proto::cache::ENV_JOB_CACHE, held.dir.display()));
                    job_cache_dir = Some(held.dir.clone());
                    self.job_cache_holds.lock().unwrap().insert((t.slot, job.clone(), gen), held);
                }
                Err(e) => {
                    let detail = format!("cache: {e}");
                    sys(&mut journal, &detail);
                    self.finish_job(&t, &a, Some(&dirs), JobState::Failed, Some(k2_node_proto::cache::codes::CACHE_FAILED), Some(&detail), src_ran, None, Some(journal), lease, None).await;
                    return;
                }
            }
        }
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
            sccache_socket: (secret_job || slot_tmp.is_dir()).then_some(sccache_socket.as_path()),
            src_dir: a.plan.src.is_some().then_some(main_checkout.as_path()),
            job_cache: job_cache_dir.as_deref(),
        }) {
            Ok(mut e) => {
                // `--with` repos are the node's checkouts too: git may read them.
                for w in &a.plan.with {
                    runner::git_safe_directory(&mut e, &dirs.src.join(&w.path));
                }
                e
            }
            Err((code, name)) => {
                self.finish_job(&t, &a, Some(&dirs), JobState::Failed, Some("env_refused"), Some(&format!("{code}: {name}")), src_ran, None, Some(journal), lease, None).await;
                return;
            }
        };
        // The easy-mode job work (protocol 6) only for a server that speaks it:
        // an older one gets its own release's job, frames and log.
        let easy = t.speaks_easy();
        // CE-15 + its own loopback ports.
        let job_cpus = self.job_cpus(&t, &a);
        let port_block = self.jobs.lock().unwrap().get(&(t.slot, job.clone())).and_then(|j| j.port_block);
        let env = if easy { with_job_hints(env, &a.plan.env, job_cpus, port_block) } else { env };
        if easy {
            let ports = port_block.map(|b| {
                let base = k2_node_proto::env::JOB_PORT_FIRST + b * k2_node_proto::env::JOB_PORT_BLOCK;
                format!("; ports {base}-{} (K2_JOB_PORT_BASE, K2_JOB_PORT_COUNT)", base + k2_node_proto::env::JOB_PORT_BLOCK - 1)
            });
            sys(&mut journal, &format!("cpus: {job_cpus} (K2_JOB_CPUS; CARGO_BUILD_JOBS and the other thread counts unless its env sets them){}", ports.unwrap_or_default()));
        }
        // CE-4: what the job's PATH really resolves to, before it starts.
        let mut tools_used = std::collections::BTreeMap::new();
        if easy {
            let explicit_needs = k2_node_proto::needs::parse_all(&a.plan.needs).unwrap_or_default();
            tools_used = crate::toolchain::tools_used(&env, &cwd, &explicit_needs).await;
            sys(&mut journal, &crate::toolchain::tools_line(&tools_used));
        }
        if stop.borrow().is_some() {
            let s = stop.borrow().clone().unwrap_or(StopReason { state: JobState::Cancelled, reason: "cancelled".into() });
            self.finish_job(&t, &a, Some(&dirs), s.state, Some(&s.reason), None, src_ran, None, Some(journal), lease, None).await;
            return;
        }

        // Secret-env (SE18–SE20): right before spawn, the job's secret env,
        // sealed to a one-time key; held in memory only, until the spawn.
        // A secret job always takes the pipe path below (SE25: no spool
        // file, no adoption across a k2-node restart; a restart interrupts
        // it and the retry asks again).
        let mut secrets: Option<k2_node_proto::secret::Plain> = None;
        let mut redactor: Option<Arc<k2_redact::Redactor>> = None;
        if !a.plan.secret_env.is_empty() {
            match self.pull_secrets(&t, &a, unit_mode, &env, &mut stop).await {
                Ok(p) => {
                    let (r, short) = job_redactor(&p);
                    journal.set_redactor(r.clone());
                    for n in short {
                        sys(&mut journal, &format!("secret env {n}: its value is under {} bytes, so it isn't redacted from the logs", k2_redact::SURFACE_MIN_LEN));
                    }
                    sys(&mut journal, &format!("secret env: {} (from the server's vault, sealed to this attempt)", p.names().join(", ")));
                    redactor = Some(r);
                    secrets = Some(p);
                }
                Err((state, code, detail)) => {
                    sys(&mut journal, &format!("secret env: {code}: {detail}"));
                    self.finish_job(&t, &a, Some(&dirs), state, Some(&code), Some(&detail), src_ran, None, Some(journal), lease, None).await;
                    return;
                }
            }
        }

        // Smoke lock + spawn, behind the wall.
        let mem_limit = a.plan.limits.mem_bytes.unwrap_or_else(|| self.local.lock().unwrap().policy.mem_cap_bytes);
        self.take_smoke_lock(&job);
        let started = if unit_mode {
            let mut rw = vec![target.clone(), sccache.clone(), cargo_home.clone()];
            // The slot's shared tmp (its sccache socket) only for other jobs.
            if slot_tmp.is_dir() && !secret_job {
                rw.push(slot_tmp.clone());
            }
            // The job cache (under the slot's `cache/`, which the executor
            // allows as a read-write path).
            if let Some(c) = &job_cache_dir {
                rw.push(c.clone());
            }
            self.start_unit(&t, &a, &dirs, &cwd, &env, rw, network, mem_limit, secrets.as_ref()).await
        } else {
            self.start_local(&t, &a, &cwd, &env, network)
        };
        // SE19: the plaintext goes as soon as the unit has it (the
        // redactor's copy stays until the job ends).
        drop(secrets.take());
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
        if let Proc::Unit { unit, cg, .. } = &mut proc {
            *cg = self.unit_status(unit).await.and_then(|s| s.control_group).map(|c| PathBuf::from("/sys/fs/cgroup").join(c.trim_start_matches('/')));
        }
        let mut usage = crate::jobstat::ResWatch::new(proc.usage_source());
        let watch_usage = usage.source != crate::jobstat::Source::None;

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
                let red = redactor.clone();
                pumps.push(tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut buf = vec![0u8; k2_node_proto::frames::MAX_LOG_CHUNK];
                    // SE23: a secret job's output is stream-redacted before
                    // the journal (a value split across reads is caught).
                    let mut sr = red.as_ref().map(|r| r.stream());
                    // Line-atomic journal (0.46.7): whole lines only, so
                    // stdout and stderr never interleave mid-line; a partial
                    // line goes after LINE_IDLE quiet or at LINE_MAX bytes.
                    let mut lines = LineBuf::default();
                    let flush = |b: &[u8]| {
                        if !b.is_empty() {
                            let _ = j.lock().unwrap().append(stream, b);
                            tenant.ship.notify_one();
                        }
                    };
                    loop {
                        let read = if lines.pending() { tokio::time::timeout(LINE_IDLE, pipe.read(&mut buf)).await } else { Ok(pipe.read(&mut buf).await) };
                        match read {
                            Err(_) => flush(&lines.take_all()),
                            Ok(Ok(0)) | Ok(Err(_)) => break,
                            Ok(Ok(n)) => {
                                let out: std::borrow::Cow<[u8]> = match sr.as_mut() {
                                    Some(s) => std::borrow::Cow::Owned(s.push(&buf[..n])),
                                    None => std::borrow::Cow::Borrowed(&buf[..n]),
                                };
                                flush(&lines.push(&out));
                                last.store(t0.elapsed().as_millis() as u64, Ordering::Relaxed);
                            }
                        }
                    }
                    if let Some(s) = sr.as_mut() {
                        let rest = s.finish();
                        flush(&lines.push(&rest));
                    }
                    flush(&lines.take_all());
                    drop(sr);
                    lines.wipe();
                    k2_redact::wipe(&mut buf);
                }));
            }
        }

        // CE-19: the host at the start, off the job's path (the steal
        // window); its line lands in the log when it's read.
        let host_start = easy.then(|| {
            let (j, at, cpus, others) = (journal.clone(), self.now(), self.facts.cores as u32, self.jobs.lock().unwrap().len().saturating_sub(1) as u32);
            tokio::spawn(async move {
                let h = tokio::task::spawn_blocking(move || crate::jobstat::host_sample(at, cpus, others, Duration::from_secs(1))).await.ok()?;
                let _ = j.lock().unwrap().append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {}\n", crate::jobstat::host_line("start", &h)).as_bytes());
                Some(h)
            })
        });

        // Wait: exit, stop, timeout, memory.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(a.plan.limits.max_secs.max(1));
        // Only a plain process without a cgroup leaf needs the RSS cap
        // (macOS); a leaf or a job unit has a hard `memory.max`.
        let mem_cap = proc.needs_rss_sampler().then_some(mem_limit);
        // Stall watch: no output for `stall_secs` and (D4, `stall-cpu`)
        // under 1% of one CPU as long ends the job (a hung test shouldn't
        // hold the slot until the wall clock; a silent fuzzer that works
        // keeps running).
        let stall_ms = a.plan.stall_secs.filter(|s| *s > 0).map(|s| s.saturating_mul(1000));
        let mut stall_tick = tokio::time::interval(Duration::from_millis(stall_ms.map(|m| (m / 4).clamp(50, 1000)).unwrap_or(1000)));
        stall_tick.tick().await;
        // The usage tick (CE-14, the stall rule's CPU, the macOS cap):
        // every `rss_every`, or a quarter of the stall window when shorter.
        let usage_every = stall_ms.map_or(self.opts.rss_every, |m| self.opts.rss_every.min(Duration::from_millis((m / 4).max(50))));
        let mut usage_tick = tokio::time::interval(usage_every);
        if easy && stall_ms.is_some() && !watch_usage {
            let _ = journal.lock().unwrap().append(
                k2_node_proto::frames::LogStream::Sys,
                b"[k2-node] stall watch for this job: silence alone ends it (this node can't read this job's CPU; its root executor may be older than k2-node)\n",
            );
        }
        let mut next_busy_note_ms = 0u64;
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
                    let now_ms = t0.elapsed().as_millis() as u64;
                    let quiet = now_ms.saturating_sub(last_out.load(Ordering::Relaxed));
                    let m = stall_ms.unwrap_or(u64::MAX);
                    // An older server's job keeps the output-only rule.
                    if if easy { usage.stalled(quiet, now_ms, m) } else { quiet >= m } {
                        ended_by = Some(StopReason { state: JobState::Timeout, reason: "stalled".into() });
                        break;
                    }
                    if !easy || quiet < m {
                        next_busy_note_ms = 0;
                    } else if now_ms >= next_busy_note_ms {
                        // Silent but working (D4): it keeps running; say so
                        // now and every 30 min.
                        let line = format!(
                            "[k2-node] quiet but busy: no output for {}, but the job is using CPU ({}.{}% of one CPU just now); a silent job is stopped only when it is also idle\n",
                            human_secs(quiet / 1000),
                            usage.last_rate_permille / 10,
                            usage.last_rate_permille % 10
                        );
                        let _ = journal.lock().unwrap().append(k2_node_proto::frames::LogStream::Sys, line.as_bytes());
                        t.ship.notify_one();
                        next_busy_note_ms = now_ms.saturating_add(k2_node_proto::frames::QUIET_BUSY_NOTE_SECS * 1000);
                    }
                }
                _ = usage_tick.tick(), if watch_usage || mem_cap.is_some() => {
                    let (src, sampler) = (usage.source.clone(), self.opts.rss.clone());
                    let r = tokio::task::spawn_blocking(move || crate::jobstat::read_usage(&src, sampler.as_ref())).await.ok();
                    let now = r.and_then(|r| usage.fold(t0.elapsed().as_millis() as u64, &r));
                    if let (Some(m), Some(cap)) = (now, mem_cap) {
                        if m > cap {
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
        // CE-14: the last reading, before the leaf or the unit goes (its
        // `memory.peak` and `cpu.stat` go with it).
        if watch_usage {
            let (src, sampler) = (usage.source.clone(), self.opts.rss.clone());
            if let Ok(r) = tokio::task::spawn_blocking(move || crate::jobstat::read_usage(&src, sampler.as_ref())).await {
                usage.fold(t0.elapsed().as_millis() as u64, &r);
            }
        }
        let used = usage.resources(Some(mem_limit));
        // CE-19: the host at the end (the steal window runs beside the
        // reap and the artifacts).
        let host_end = easy.then(|| {
            let (at, cpus, others) = (self.now(), self.facts.cores as u32, self.jobs.lock().unwrap().len().saturating_sub(1) as u32);
            tokio::task::spawn_blocking(move || crate::jobstat::host_sample(at, cpus, others, Duration::from_secs(1)))
        });
        let peak_shown = if easy { used.peak_mem_bytes } else { None };
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
        // The start stamp's task holds a journal handle: done before the
        // journal is ours alone again.
        let host_start = match host_start {
            Some(h) => h.await.ok().flatten(),
            None => None,
        };
        let mut journal = Arc::try_unwrap(journal).ok().map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()));
        let (state, reason) = match ended_by {
            Some(s) => (s.state, Some(s.reason)),
            None if oom => (JobState::Failed, Some("mem_cap".to_string())),
            None => (JobState::Done, None),
        };
        if reason.as_deref() == Some("mem_cap") {
            if let Some(j) = journal.as_mut() {
                let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {}\n", mem_cap_detail_used(mem_limit, peak_shown)).as_bytes());
            }
        }
        // Artifacts and test results: after the command (and its group)
        // are gone, before the warm slot goes back to the pool.
        let collected = if a.plan.artifacts.is_empty() {
            None
        } else {
            // CE-23: the note names the job's cwd as a job-relative path.
            let label = match cwd.strip_prefix(&dirs.root) {
                Ok(r) if r.as_os_str().is_empty() => "<job>".to_string(),
                Ok(r) => format!("<job>/{}", r.display()),
                Err(_) => "the job's working directory".to_string(),
            };
            let (globs, cwd, target, keep) = (a.plan.artifacts.clone(), cwd.clone(), target.clone(), crate::artifacts::dir(&t.paths.artifacts(), &job, gen));
            let o = tokio::task::spawn_blocking(move || {
                crate::artifacts::collect(&globs, &crate::artifacts::Roots { cwd: &cwd, target: Some(&target), cwd_label: easy.then_some(label.as_str()) }, &keep)
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
        // CE-4, CE-14, CE-19: every end that ran carries what ran, what it
        // used and the host around it (not only `--artifact` jobs).
        let host_end = match host_end {
            Some(h) => h.await.ok(),
            None => None,
        };
        if let Some(j) = journal.as_mut().filter(|_| easy) {
            let mut line = |m: String| {
                let _ = j.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {m}\n").as_bytes());
            };
            if let Some(h) = &host_end {
                line(crate::jobstat::host_line("end", h));
            }
            let mem = used.peak_mem_bytes.map(|p| format!("peak memory {} ({})", crate::jobstat::gib(p), used.mem_source)).unwrap_or_else(|| "peak memory unknown".into());
            let cpu = used.cpu_ms.map(|c| format!("CPU {}.{} s ({})", c / 1000, (c % 1000) / 100, used.cpu_source)).unwrap_or_else(|| "CPU unknown".into());
            line(format!("used: {mem}, {cpu}"));
        }
        let outputs = if easy {
            let mut o = collected.unwrap_or_default();
            o.tools_used = tools_used;
            o.resources = Some(used.clone());
            o.host = Some(k2_node_proto::results::HostStamps { start: host_start, end: host_end });
            Some(o.capped())
        } else {
            collected
        };
        let detail = match reason.as_deref() {
            Some("stalled") => Some(format!("no output for {}s", a.plan.stall_secs.unwrap_or(0))),
            Some("mem_cap") => Some(mem_cap_detail_used(mem_limit, peak_shown)),
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
    /// unit; we keep the read ends of its stdout/stderr pipes. With
    /// `secrets` (SE21) a third pipe carries them to the unit's stdin: the
    /// node writes `K2SE1\n` + JSON from a blocking thread (a small pipe
    /// can't deadlock it), closes its end, and passes the read end; the
    /// `job-exec` shim reads it to EOF. No value is in argv, a unit
    /// property, D-Bus, `exec.json` or any file.
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
        secrets: Option<&k2_node_proto::secret::Plain>,
    ) -> Result<(Proc, Option<OutPipe>, Option<OutPipe>), String> {
        use crate::admin::jobs::{JobAnswer, JobOp, JobStart};
        let key = t.job_key(&a.fence.job_id, a.fence.generation);
        crate::wall::write_exec_spec(&dirs.root, &crate::wall::ExecSpec { argv: a.plan.argv.clone(), env: env.to_vec(), cwd: cwd.to_path_buf(), stdin_env: secrets.is_some() })?;
        let (out_r, out_w) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
        let (err_r, err_w) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
        let mut fds: Vec<std::os::fd::OwnedFd> = vec![out_w.into(), err_w.into()];
        if let Some(p) = secrets {
            let mut body = secret_pipe_body(p)?;
            let (in_r, mut in_w) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
            fds.push(in_r.into());
            std::thread::spawn(move || {
                use std::io::Write;
                // EPIPE (the unit never started) just ends the write.
                let _ = in_w.write_all(&body);
                drop(in_w);
                k2_redact::wipe(&mut body);
            });
        }
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
            stdin: secrets.is_some(),
        };
        // The write ends (and the stdin read end) go to root (and the
        // unit); ours close with the call.
        let ans = self.job_op(JobOp::Start(start), fds, Duration::from_secs(60)).await;
        let unit = match ans {
            Ok(JobAnswer::Started { unit }) => unit,
            Ok(JobAnswer::Refused(r)) => return Err(format!("the job wall refused to start it: {}: {}", r.code, r.message)),
            Ok(other) => return Err(format!("unexpected executor answer: {other:?}")),
            Err(e) => return Err(format!("couldn't start the job unit: {e}")),
        };
        let rx = |r: std::io::PipeReader| -> Option<OutPipe> {
            tokio::net::unix::pipe::Receiver::from_owned_fd(std::os::fd::OwnedFd::from(r)).ok().map(|p| Box::new(p) as OutPipe)
        };
        Ok((Proc::Unit { unit, last: None, cg: None }, rx(out_r), rx(err_r)))
    }

    /// The command ended: its exit (code or signal).
    async fn proc_wait(&self, p: &mut Proc) -> Option<ExitInfo> {
        match p {
            Proc::Local { child, .. } => child.wait().await.ok().map(|s| {
                use std::os::unix::process::ExitStatusExt;
                ExitInfo { code: s.code(), signal: s.signal() }
            }),
            Proc::Unit { unit, last, .. } => {
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
                    // CE-18: back under the tool-cache cap, then re-probe.
                    let node = self.clone();
                    tokio::spawn(async move {
                        node.toolchain_gc().await;
                        node.probe_tools().await
                    });
                }
                Ok(res)
            }
            Err((code, detail)) => Err((JobState::Failed, code.to_string(), detail)),
        }
    }

    /// SE18: send `secret_need` with a fresh one-time key and wait (≤ 60 s)
    /// for this attempt's answer. A cancel ends the wait; no session, a
    /// dropped link or the timeout is `secret_unavailable` (the retry asks
    /// again). The answer must open with this key and binding and name
    /// exactly the plan's env names (SE20). `Err((state, code, detail))`.
    async fn pull_secrets(
        &self,
        t: &Tenant,
        a: &Assign,
        unit_mode: bool,
        env: &[(String, String)],
        stop: &mut watch::Receiver<Option<StopReason>>,
    ) -> Result<k2_node_proto::secret::Plain, (JobState, String, String)> {
        use k2_node_proto::secret::{codes, NodeKey};
        let failed = |code: &str, detail: String| (JobState::Failed, code.to_string(), detail);
        if !cfg!(target_os = "linux") {
            return Err(failed(codes::LINUX_ONLY, "secret env runs only on Linux nodes in this release".into()));
        }
        if !unit_mode {
            return Err(failed(codes::NEEDS_WALL, "secret env needs the job wall (sudo k2-node setup)".into()));
        }
        if !self.exec_stdin.load(Ordering::SeqCst) {
            return Err(failed(codes::NEEDS_UPDATE, "the node's root executor can't pass secret env yet (update k2-node)".into()));
        }
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        let controller_fp = t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()).ok_or_else(|| failed(codes::UNAVAILABLE, "no server pin".into()))?;
        let key = NodeKey::generate().map_err(|e| failed(codes::UNAVAILABLE, e))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        t.hub.secrets.lock().unwrap().insert((job.clone(), gen), tx);
        let epoch = *t.hub.connected.borrow();
        let need = k2_node_proto::frames::SecretNeed { job_id: job.clone(), generation: gen, attempt: a.fence.attempt, plan_digest: a.fence.plan_digest.clone(), epk: key.public_b64() };
        let sent = epoch != 0 && t.speaks_secrets() && t.hub.send(Frame::SecretNeed(need));
        let answer = if !sent {
            None
        } else {
            let mut connected = t.hub.connected.subscribe();
            let deadline = tokio::time::sleep(self.opts.secret_wait);
            tokio::pin!(deadline);
            tokio::pin!(rx);
            let mut stop_open = true;
            loop {
                tokio::select! {
                    r = &mut rx => break r.ok().map(Ok),
                    _ = &mut deadline => break None,
                    r = connected.changed() => {
                        if r.is_err() || *connected.borrow() != epoch { break None; }
                    }
                    c = stop.changed(), if stop_open => {
                        if c.is_err() {
                            stop_open = false;
                        } else if let Some(s) = stop.borrow().clone() {
                            break Some(Err(s));
                        }
                    }
                }
            }
        };
        t.hub.secrets.lock().unwrap().remove(&(job.clone(), gen));
        let answer = match answer {
            None => return Err(failed(codes::UNAVAILABLE, format!("no answer from the server within {}s (or the link dropped)", self.opts.secret_wait.as_secs()))),
            Some(Err(s)) => return Err((s.state, s.reason, "stopped while waiting for the secret env".into())),
            Some(Ok(a)) => a,
        };
        let env_msg = match answer {
            crate::node::tenant::SecretAnswer::Refused(r) => {
                // Only codes K2 defines pass through (a server can't make up reasons).
                let known = [
                    codes::NOT_GRANTED,
                    codes::NOT_ACTIVE,
                    codes::ATTEMPT_STALE,
                    codes::ALREADY_RUNNING,
                    codes::TOO_LARGE,
                    codes::VAULT_LOCKED,
                    codes::UNAVAILABLE,
                ];
                let code = known.iter().find(|c| **c == r.code).copied().unwrap_or(codes::UNAVAILABLE);
                let detail: String = r.message.chars().filter(|c| !c.is_control()).take(300).collect();
                return Err(failed(code, format!("the server refused: {detail}")));
            }
            crate::node::tenant::SecretAnswer::Env(e) => e,
        };
        let binding = k2_node_proto::secret::Binding {
            controller_fp,
            node_fp: self.key.fingerprint(),
            job_id: job.clone(),
            generation: gen,
            attempt: a.fence.attempt,
            plan_digest: a.fence.plan_digest.clone(),
        };
        let plain = key.open(&env_msg.epk, &env_msg.ct, &binding).map_err(|e| failed(codes::PAYLOAD_MISMATCH, e))?;
        check_secret_names(&plain, &a.plan.secret_env, env).map_err(|e| failed(codes::PAYLOAD_MISMATCH, e))?;
        Ok(plain)
    }

    /// The CPUs a job may use (CE-15): its `--cpus` rounded up, else the
    /// node's default share for its server (`caps.cpu ÷ parallel_max`, the
    /// reservation `used` counts), at least 1.
    fn job_cpus(&self, t: &Tenant, a: &Assign) -> u64 {
        if let Some(m) = a.plan.limits.cpu_millis.filter(|m| *m > 0) {
            return m.div_ceil(1000).max(1);
        }
        let l = self.local.lock().unwrap();
        let (cpu, _, par) = self.caps(t, &l);
        (cpu / u64::from(par.max(1)) / 1000).max(1)
    }

    /// Every need of a running or queued job on this node (every
    /// server's): the tool-cache cap never removes one of them.
    fn needs_in_use(&self) -> Vec<k2_node_proto::needs::Need> {
        let running = self.running_by_tenant();
        let mut out = Vec::new();
        for t in self.tenants() {
            let l = t.ledger.lock().unwrap();
            let mut rows = l.queued().unwrap_or_default();
            for id in running.get(&t.slot).into_iter().flatten() {
                rows.extend(l.latest(id).ok().flatten());
            }
            for r in rows {
                // A need list that doesn't parse protects nothing more than
                // its job (which fails anyway); the parse is the job's own.
                out.extend(k2_node_proto::needs::parse_all(&r.plan.needs).unwrap_or_default());
            }
        }
        out
    }

    /// CE-18: over the policy's `toolchain_budget_gb`, remove the least
    /// recently used tool versions nothing running or queued needs, each
    /// logged. One at a time with installs (the tool lock).
    pub async fn toolchain_gc(&self) {
        let _guard = self.tool_lock.lock().await;
        let budget = self.local.lock().unwrap().policy.toolchain_budget_bytes;
        let (tc, needs, now) = (self.toolchains(), self.needs_in_use(), self.now());
        let gone = tokio::task::spawn_blocking(move || tc.gc_to_budget(budget, &needs, now)).await.unwrap_or_default();
        for v in &gone {
            crate::nlog!("toolchain cap ({} GB): removed {} ({} MB, last used at {})", budget >> 30, v.key, v.bytes >> 20, v.last_used);
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
    pub(super) async fn finish_job(
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
        // Review B-H1: a secret attempt's tree (home, checkout, target,
        // caches, tmp) goes now, while the job still counts as running, so
        // no other workspace's job (same uid) starts before it's gone. Only
        // the node-only journal stays (it may not be shipped yet).
        // Round 2, item 2: fail closed. If the delete fails, the slot stays
        // held (a stand-in for a secret job of an unknown workspace takes
        // this job's place before it leaves the running map) and the delete
        // is retried until it works.
        let mut secret_tree_left: Option<PathBuf> = None;
        if !a.plan.secret_env.is_empty() {
            if let Some(d) = dirs {
                let (sock, walled, slot, root) = (self.opts.admin_socket.clone(), matches!(self.wall, Wall::JobUser { .. }), t.slot, d.root.clone());
                let r = tokio::task::spawn_blocking(move || super::disk::delete_secret_tree(&sock, walled, slot, &root)).await;
                match r {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        crate::nlog!("job {job} gen {gen}: its secret attempt's folder isn't deleted yet ({e}); slot {} stays held until it is", t.slot);
                        secret_tree_left = Some(d.root.clone());
                    }
                    Err(e) => {
                        crate::nlog!("job {job} gen {gen}: deleting its secret attempt's folder: {e}; slot {} stays held", t.slot);
                        secret_tree_left = Some(d.root.clone());
                    }
                }
            }
        }
        // SE23: a secret job's detail and test summary go through its
        // redactor too (the journal holds it); kept artifacts are the
        // job's own bytes and are never rewritten (SV10).
        let redactor = journal.as_ref().and_then(|j| j.redactor().cloned());
        let detail_red = match (&redactor, detail) {
            (Some(r), Some(d)) => Some(r.redact_str(d)),
            _ => detail.map(str::to_string),
        };
        let detail = detail_red.as_deref();
        let outputs = match (&redactor, outputs) {
            (Some(r), Some(o)) => Some(redact_outputs(r, o)),
            (_, o) => o,
        };
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
        if self.job_cache_holds.lock().unwrap().contains_key(&(t.slot, job.clone(), gen)) {
            env_names.push(k2_node_proto::cache::ENV_JOB_CACHE.to_string());
        }
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
            // SE29: names only.
            secret_env: a.plan.secret_env.clone(),
            tools_used: outputs.as_ref().map(|o| o.tools_used.clone()).unwrap_or_default(),
        };
        let signed = SignedReceipt::sign(&self.key, &receipt);
        if let Err(e) = &signed {
            crate::nlog!("receipt for {job}: {e}");
        }
        let record = || self.record_finish(t, job, gen, state, reason, detail, exit.as_ref(), log_seq, now, src.as_ref(), outputs.as_ref(), signed.as_ref().ok());
        let mut recorded = record();
        let gone = || {
            if let Some(root) = secret_tree_left.clone() {
                // The stand-in goes in first: no moment with the slot free.
                self.hold_lingering(t.slot, vec![super::Linger { key: format!("{job}-g{gen}"), unit: None, tree: Some(root) }]);
            }
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
        if matches!(recorded, Ok(false)) {
            // Another end stands (review B F2: a sweep that raced the
            // runner); nothing was written and nothing more is sent.
            crate::nlog!("job {job} gen {gen} had already ended; this {} end isn't recorded", state.as_str());
        }
        // The big parts of the dir go now (module `disk`): before any
        // retry, since a full disk is the usual reason a write failed.
        if let Some(d) = dirs {
            self.strip_job_dir(t, &d.root, a.plan.src.as_ref().map(|s| s.project_key.as_str()), super::disk::JOB_END_KEEP).await;
        }
        // A job never stays `running` because its finish didn't land (k2n4,
        // 2026-10-21: a full disk). Its server hears the end from memory at
        // once; the row is retried, backing off, until it lands (then the
        // shipper sends it again with its receipt). The job stays counted
        // (nothing new takes its place) until then.
        if let Err(e) = &recorded {
            crate::nlog!("ledger finish {job}: {e}; telling the server from memory and retrying");
            t.hub.send(Frame::State(k2_node_proto::frames::StateUpdate {
                queue: None,
                job_id: job.clone(),
                generation: gen,
                state,
                reason: reason.map(str::to_string),
                detail: detail.map(str::to_string),
                at: now,
                exit: exit.clone(),
                src: src.clone(),
                log_seq,
                outputs: outputs.clone(),
            }));
        }
        let mut tries = 0u32;
        while let Err(e) = &recorded {
            tries += 1;
            if tries % 30 == 0 {
                crate::nlog!("ledger finish {job}: still failing after {tries} tries: {e}");
            }
            tokio::time::sleep(finish_backoff(self.opts.disk_poll, tries)).await;
            recorded = record();
        }
        if tries > 0 {
            crate::nlog!("ledger finish {job}: landed after {tries} retries");
            t.ship.notify_one();
        }
        if !landed {
            gone();
        }
        if let Some(l) = lease.as_ref().filter(|l| l.slot.is_some()) {
            self.after_slot_use(&l.dir).await;
        }
        // The job cache goes back last: its unit is gone, so nothing writes
        // it while it's measured, and it's never evicted under the job.
        let held = self.job_cache_holds.lock().unwrap().remove(&(t.slot, job.clone(), gen));
        if let Some(h) = held {
            self.job_cache_close(h).await;
        }
    }

    /// The terminal row and its receipt, in one transaction (the shipper
    /// never sees a terminal row without its receipt). `Ok(false)`: the
    /// attempt had already ended; nothing was written.
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
    ) -> Result<bool, String> {
        #[cfg(test)]
        if job == tests::FINISH_FAILS_JOB && tests::FINISH_FAILS.load(Ordering::SeqCst) > 0 {
            tests::FINISH_FAILS.fetch_sub(1, Ordering::SeqCst);
            return Err("database or disk is full (test)".into());
        }
        t.ledger.lock().unwrap().end_attempt(job, gen, state, reason, detail, exit, log_seq, now, src, outputs, signed)
    }
}

/// A secret job's redactor (SE23) and the names whose values are too short
/// to redact (under [`k2_redact::SURFACE_MIN_LEN`]).
pub fn job_redactor(p: &k2_node_proto::secret::Plain) -> (Arc<k2_redact::Redactor>, Vec<String>) {
    let mut short = Vec::new();
    let mut entries: Vec<(&str, &[u8])> = Vec::new();
    for (k, v) in &p.0 {
        if v.len() < k2_redact::SURFACE_MIN_LEN {
            short.push(k.clone());
        } else {
            entries.push((k.as_str(), v.as_bytes()));
        }
    }
    (Arc::new(k2_redact::Redactor::new(entries)), short)
}

/// SE20: the opened names are exactly the plan's (sorted, unique), each
/// passes the node's rule, and none repeats a name the job's env sets.
pub fn check_secret_names(p: &k2_node_proto::secret::Plain, plan: &[String], env: &[(String, String)]) -> Result<(), String> {
    let mut got = p.names();
    got.sort();
    let mut want = plan.to_vec();
    want.sort();
    want.dedup();
    if got != want {
        return Err(format!("the secret env names ({}) aren't the plan's ({})", got.join(", "), want.join(", ")));
    }
    for n in &got {
        if let Some(c) = k2_node_proto::secret::name_refusal(n) {
            return Err(format!("secret env name {n}: {c}"));
        }
        if env.iter().any(|(k, _)| k == n) {
            return Err(format!("secret env name {n} is also set by the job's env"));
        }
    }
    Ok(())
}

/// The bytes on the stdin pipe (SE21): `K2SE1\n` + the canonical payload.
fn secret_pipe_body(p: &k2_node_proto::secret::Plain) -> Result<Vec<u8>, String> {
    let mut values: Vec<(String, Vec<u8>)> = p.0.iter().map(|(k, v)| (k.clone(), v.as_bytes().to_vec())).collect();
    let json = k2_node_proto::secret::payload(&values);
    for (_, v) in &mut values {
        k2_redact::wipe(v);
    }
    let mut json = json.map_err(|c| format!("secret env: {c}"))?;
    let mut body = Vec::with_capacity(k2_node_proto::secret::PIPE_MAGIC.len() + json.len());
    body.extend_from_slice(k2_node_proto::secret::PIPE_MAGIC);
    body.extend_from_slice(&json);
    k2_redact::wipe(&mut json);
    Ok(body)
}

/// The test summary K2 derives, redacted (SE23): failing names, classes,
/// files and messages, and the node's notes. Artifacts are untouched.
pub fn redact_outputs(r: &k2_redact::Redactor, mut o: Outputs) -> Outputs {
    if let Some(t) = o.tests.as_mut() {
        for f in &mut t.failing {
            f.name = r.redact_str(&f.name);
            f.classname = f.classname.as_deref().map(|c| r.redact_str(c));
            f.file = f.file.as_deref().map(|c| r.redact_str(c));
            f.message = r.redact_str(&f.message);
        }
    }
    for n in &mut o.notes {
        *n = r.redact_str(n);
    }
    for v in o.tools_used.values_mut() {
        *v = r.redact_str(v);
    }
    o
}

/// The wait before ledger finish retry `n` (from 1): `base` doubling,
/// at most 5 minutes.
pub(super) fn finish_backoff(base: Duration, n: u32) -> Duration {
    base.saturating_mul(1u32 << (n.saturating_sub(1)).min(10)).min(Duration::from_secs(300))
}

/// Allocated bytes under `root` (V74), no link followed, at most
/// 2 million entries and 256 levels.
pub fn disk_usage(root: &Path) -> u64 {
    disk_usage_walk(root).0
}

/// [`disk_usage`], and whether the walk hit its caps (2 million entries
/// or 256 levels): then the bytes are a floor, not the size, and a tenant
/// whose root does that counts as over its share (security review B, F1:
/// data hidden below the caps went uncounted).
pub fn disk_usage_walk(root: &Path) -> (u64, bool) {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0u64;
    let mut seen = 0u64;
    let mut capped = false;
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    while let Some((p, depth)) = stack.pop() {
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        total = total.saturating_add(m.blocks() * 512);
        seen += 1;
        if seen > WALK_MAX_ENTRIES {
            capped = true;
            break;
        }
        if m.is_dir() {
            if depth >= WALK_MAX_DEPTH {
                capped = true;
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| (e.path(), depth + 1)));
            }
        }
    }
    (total, capped)
}

const WALK_MAX_ENTRIES: u64 = 2_000_000;
pub(super) const WALK_MAX_DEPTH: u32 = 256;

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
            cg: None,
        };
        assert!(unit.oom() && unit.pgid() == 0 && !unit.needs_rss_sampler());
        // CE-14: a unit's usage is read from its cgroup once the executor
        // said it; a leaf's from the leaf; a plain group from ps.
        assert_eq!(unit.usage_source(), crate::jobstat::Source::None);
        let with_cg = Proc::Unit { unit: "u".into(), last: None, cg: Some("/sys/fs/cgroup/system.slice/u".into()) };
        assert_eq!(with_cg.usage_source(), crate::jobstat::Source::Cgroup("/sys/fs/cgroup/system.slice/u".into()));
        assert_eq!(p.usage_source(), crate::jobstat::Source::Cgroup(l.dir.clone()));
        let s = crate::admin::jobs::parse_show("LoadState=loaded\nControlGroup=/system.slice/k2job-j-g1.service\n");
        assert_eq!(s.control_group.as_deref(), Some("/system.slice/k2job-j-g1.service"));
        assert_eq!(crate::admin::jobs::parse_show("ControlGroup=/a/../b\n").control_group, None, "never a path out");
        assert_eq!(crate::admin::jobs::parse_show("ControlGroup=\n").control_group, None);
        assert!(mem_cap_detail(64 << 20).contains("64 MiB") && mem_cap_detail(64 << 20).contains("--mem"));
        // CE-14: a mem_cap end names the peak beside the limit.
        let d = mem_cap_detail_used(24 << 30, Some((25 << 30) + (3 << 30) / 10));
        assert!(d.ends_with("(used 25.3G of 24.0G at its peak)"), "{d}");
        assert_eq!(mem_cap_detail_used(1 << 30, None), mem_cap_detail(1 << 30));
    }

    /// Line-atomic logs: whole lines pass, a partial line waits for its
    /// newline (or the size cap), nothing is lost or reordered.
    #[test]
    fn line_buffer_passes_whole_lines_only() {
        let mut b = LineBuf::default();
        assert_eq!(b.push(b"{\"a\":"), b"");
        assert!(b.pending());
        assert_eq!(b.push(b"1}\n{\"b\""), b"{\"a\":1}\n");
        assert_eq!(b.push(b":2}\nx\ny"), b"{\"b\":2}\nx\n");
        assert_eq!(b.take_all(), b"y");
        assert!(!b.pending());
        let big = vec![b'z'; LINE_MAX + 1];
        assert_eq!(b.push(&big).len(), LINE_MAX + 1, "an endless line still goes, at the cap");
        assert_eq!(b.push(b"\n"), b"\n");
    }

    /// CE-15 + ports: the hints fill only what the plan's env doesn't set;
    /// `K2_JOB_CPUS` and the port pair are the node's alone.
    #[test]
    fn job_hints_fill_only_unset_names() {
        let mut plan_env = std::collections::BTreeMap::new();
        plan_env.insert("CARGO_BUILD_JOBS".to_string(), "2".to_string());
        let base = vec![("CARGO_BUILD_JOBS".to_string(), "2".to_string()), ("PATH".to_string(), "/bin".to_string())];
        let env = with_job_hints(base, &plan_env, 4, Some(3));
        let get = |k: &str| env.iter().filter(|(ek, _)| ek == k).map(|(_, v)| v.as_str()).collect::<Vec<_>>();
        assert_eq!(get("CARGO_BUILD_JOBS"), vec!["2"], "the caller's --env wins");
        for k in ["RUST_TEST_THREADS", "NEXTEST_TEST_THREADS", "OMP_NUM_THREADS", "K2_JOB_CPUS"] {
            assert_eq!(get(k), vec!["4"], "{k}");
        }
        assert_eq!((get("K2_JOB_PORT_BASE"), get("K2_JOB_PORT_COUNT")), (vec!["20300"], vec!["100"]));
        let none = with_job_hints(vec![], &Default::default(), 0, None);
        assert!(none.iter().any(|(k, v)| k == "K2_JOB_CPUS" && v == "1") && !none.iter().any(|(k, _)| k == "K2_JOB_PORT_BASE"));
        // The pool stays below Linux's ephemeral ports.
        let last = u32::from(k2_node_proto::env::JOB_PORT_FIRST) + u32::from(k2_node_proto::env::JOB_PORT_BLOCKS) * u32::from(k2_node_proto::env::JOB_PORT_BLOCK);
        assert!(last <= 32768, "{last}");
        assert_eq!(human_secs(40), "40s");
        assert_eq!(human_secs(35 * 60), "35m");
        assert_eq!(human_secs(2 * 3600 + 5 * 60), "2h05m");
    }
}
