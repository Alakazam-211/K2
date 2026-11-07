//! Jobs keep running through k2-node updates
//! (prd-compute-update-continuity-v1 §5.3–§5.7, U3).
//!
//! - **Run facts** ([`RunFacts`], the ledger's `run_json`): how a later
//!   k2-node process finds a running job again. Written as `provisional`
//!   before the spawn, then `unit` (Linux wall) or `holder` (macOS, test
//!   nodes) once it runs; the spool offsets move as the journal fills.
//! - **Detach** ([`Node::detach_for_update`]): on a K2-made restart whose
//!   mark says `keep` and names this very process (pid + start time, §5.4),
//!   every job with `unit`/`holder` facts is left running; anything else
//!   (a secret job, a job still preparing, an unwalled one) stops with
//!   `node_update` as before.
//! - **Adopt, two phases (C1).** Phase 1 is in `tenant::reconcile`: this
//!   boot's open rows with known facts are kept open (adoptable), a
//!   `provisional` row's process is stopped, the rest end as before.
//!   Phase 2 ([`Node::adopt_rows`]) runs once the `Node` exists and before
//!   anything else does (the queue, the disk guard, the session): each job
//!   goes back into the running map with its reservation, warm slot, job
//!   cache and smoke lock, its journal reopens, and `supervise` takes over.
//!   A job that ended while no node watched ends now with what its unit or
//!   holder says; a newer format than this build knows is stopped, never
//!   adopted (C10).
//! - **Tenant slots (C2).** At start, a slot whose server will open again
//!   adopts in `add_tenant`; a slot that won't (no line, a changed line, no
//!   pin) has its jobs stopped and released before anything of it is
//!   deleted. A slot that should open but doesn't within
//!   [`TENANT_ADOPT_WAIT`] is treated the same way.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::frames::{Assign, Fence, JobState, LogStream};
use k2_node_proto::share::{Slot, OWNER_SLOT};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::run::{kill_job_group, Live, OutSrc, Proc};
use super::{Node, RunningJob, StopReason, Tenant};
use crate::hold::HoldFacts;
use crate::journal::JournalWriter;
use crate::ledger::Row;
use crate::runner::JobDirs;
use crate::slots::Lease;
use crate::spool::SpoolFile;
use crate::wall::Wall;

/// The run facts' format. A row with a higher `v` (a rollback from a newer
/// k2-node) is stopped, never adopted (C10).
pub const RUN_V: u32 = 1;

/// The stop reason that means "leave it running for the next process"
/// (never a job state anyone sees).
pub const DETACH: &str = "__k2_node_detach";

/// How long a slot that should open again may take before its adopted
/// jobs are stopped instead (its server's line never came back).
pub const TENANT_ADOPT_WAIT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// Written before the spawn: whatever it names is stopped at the next
    /// start (C18).
    Provisional,
    Unit,
    Holder,
}

/// A warm slot lease, as the run facts keep it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseFacts {
    pub project_key: String,
    pub slot: Option<u32>,
    pub dir: PathBuf,
    pub warm: bool,
}

impl LeaseFacts {
    pub fn of(l: &Lease) -> Self {
        Self { project_key: l.project_key.clone(), slot: l.slot, dir: l.dir.clone(), warm: l.warmth == "warm" }
    }

    pub fn lease(&self) -> Lease {
        Lease { project_key: self.project_key.clone(), slot: self.slot, dir: self.dir.clone(), warmth: if self.warm { "warm" } else { "cold" } }
    }
}

/// A job cache hold, as the run facts keep it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldFacts {
    pub dir: PathBuf,
    pub ws_dir: PathBuf,
    pub name: String,
    pub uid: u32,
}

impl HeldFacts {
    pub fn of(h: &super::job_cache::Held) -> Self {
        Self { dir: h.dir.clone(), ws_dir: h.ws_dir.clone(), name: h.name.clone(), uid: h.uid }
    }
}

/// How a running job can be found again (ledger `run_json`, §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFacts {
    pub v: u32,
    pub kind: RunKind,
    /// The k2-node that started it.
    pub node_version: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub hold_dir: Option<PathBuf>,
    /// Copied once from the holder's `hold.json` (never re-read, C6).
    #[serde(default)]
    pub holder: Option<HoldFacts>,
    #[serde(default)]
    pub out: Option<SpoolFile>,
    #[serde(default)]
    pub err: Option<SpoolFile>,
    pub cwd: PathBuf,
    pub target: PathBuf,
    #[serde(default)]
    pub lease: Option<LeaseFacts>,
    pub mem_limit: u64,
    /// Unix seconds.
    pub deadline: i64,
    #[serde(default)]
    pub smoke: bool,
    #[serde(default)]
    pub port_block: Option<u16>,
    #[serde(default)]
    pub job_cache: Option<HeldFacts>,
    /// The server spoke the easy-mode protocol when it started (its log
    /// lines and outputs).
    #[serde(default)]
    pub easy: bool,
    #[serde(default)]
    pub tools_used: BTreeMap<String, String>,
    /// Output the journal dropped over the log cap so far.
    #[serde(default)]
    pub log_dropped: u64,
    /// The Rust channels it resolved (never moved under it).
    #[serde(default)]
    pub rust: Vec<String>,
    /// Protocol 9: receipt v2's start-time facts (a plan with
    /// `receipt_v: 2`), so the process that adopts the job signs what was
    /// true when it started. Absent otherwise (older rows read as none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_run: Option<k2_node_proto::ci::RunFacts>,
}

/// Phase 1's sort of this boot's open rows with run facts.
#[derive(Default)]
pub struct Sorted {
    pub adoptable: Vec<(Row, RunFacts)>,
    /// `provisional` rows, and rows whose facts this build can't read or
    /// use: ended as before; their process is stopped.
    pub stop: Vec<(Row, Option<RunFacts>)>,
}

/// Phase 1 (C1, C10): which open rows of this boot the new process adopts.
/// `unit` facts need the job wall; `holder` facts need nothing but files
/// and signals.
pub fn sort(rows: Vec<Row>, wall: &Wall) -> Sorted {
    let mut out = Sorted::default();
    for r in rows {
        let facts = r.run_json.as_deref().and_then(|j| serde_json::from_str::<RunFacts>(j).ok());
        let usable = match &facts {
            Some(f) if f.v > RUN_V => false,
            Some(f) if f.kind == RunKind::Unit => matches!(wall, Wall::JobUser { .. }) && f.unit.is_some(),
            Some(f) if f.kind == RunKind::Holder => f.holder.as_ref().is_some_and(|h| h.v <= crate::hold::HOLD_V) && f.hold_dir.is_some(),
            _ => false,
        };
        match facts {
            Some(f) if usable => out.adoptable.push((r, f)),
            f => out.stop.push((r, f)),
        }
    }
    out
}

/// Stop what a row's facts name, without adopting it: its holder (and the
/// job's group) or nothing more (a unit is released by the caller like
/// every interrupted row's).
pub fn stop_named(f: &RunFacts) {
    if let Some(h) = &f.holder {
        if crate::hold::alive(h.pid, h.start) {
            crate::hold::terminate(h.pid, h.start);
        }
        crate::hold::kill_job_group(h);
    } else if let Some(dir) = &f.hold_dir {
        // Provisional: the holder may have started and written its facts.
        if let Some(h) = crate::hold::read_small(dir, crate::hold::FACTS_FILE).and_then(|b| serde_json::from_slice::<HoldFacts>(&b).ok()) {
            if crate::hold::alive(h.pid, h.start) {
                crate::hold::terminate(h.pid, h.start);
            }
            crate::hold::kill_job_group(&h);
        }
    }
}

impl Node {
    /// Phase 2 (C1): put `rows` (sorted adoptable by `reconcile`) back in
    /// the running map and supervise them. Runs before the node's
    /// background loops and session, and for a tenant before it joins the
    /// tenant map, so no sweep ever sees an adoptable row as an orphan.
    pub(super) async fn adopt_rows(self: &Arc<Self>, t: &Arc<Tenant>, rows: Vec<(Row, RunFacts)>) {
        let mut keep: Vec<String> = Vec::new();
        let n = rows.len();
        for (row, f) in rows {
            keep.push(t.job_key(&row.job_id, row.generation));
            self.adopt_one(t, row, f).await;
        }
        if n > 0 {
            crate::nlog!("slot {}: adopted {n} running job(s) from the last k2-node", t.slot);
        }
        self.sweep_run_leftovers(t, &keep);
        self.write_status();
    }

    async fn adopt_one(self: &Arc<Self>, t: &Arc<Tenant>, row: Row, f: RunFacts) {
        let job = row.job_id.clone();
        let gen = row.generation;
        let key = t.job_key(&job, gen);
        let a = Assign { fence: Fence { job_id: job.clone(), attempt: row.attempt, generation: gen, plan_digest: row.plan_digest.clone() }, plan: row.plan.clone() };
        let dirs = JobDirs::new(t.paths.job_dir(&job, gen));
        let mut preset: Option<(StopReason, String)> = None;
        let unknown_exit = || {
            (
                StopReason { state: JobState::Failed, reason: k2_node_proto::share::codes::NODE_UPDATE.into() },
                "it ended while no node watched; its exit is unknown".to_string(),
            )
        };
        let proc = match f.kind {
            RunKind::Unit => {
                // Only this attempt's own unit (its name is its key's).
                let unit = match (f.unit.clone(), crate::admin::jobs::unit_name(&key)) {
                    (Some(u), Some(want)) if u == want => u,
                    _ => {
                        crate::nlog!("job {job} gen {gen}: its run facts name another unit; not adopted");
                        preset = Some(unknown_exit());
                        crate::admin::jobs::unit_name(&key).unwrap_or_default()
                    }
                };
                // C23: a unit systemd already forgot ended with no exit.
                if preset.is_none() && self.unit_status(&unit).await.is_some_and(|s| !s.loaded) {
                    preset = Some(unknown_exit());
                }
                Proc::Unit { unit, last: None, cg: None }
            }
            RunKind::Holder => {
                let h = f.holder.clone().expect("sorted: a holder row has holder facts");
                let dir = self.hold_dir(t, &key);
                if f.hold_dir.as_ref() != Some(&dir) {
                    crate::nlog!("job {job} gen {gen}: its run facts name another holder folder; using {}", dir.display());
                }
                if crate::hold::read_exit(&dir).is_none() && !crate::hold::alive(h.pid, h.start) {
                    // C7: the holder is gone and never said how the job
                    // ended: whatever is left of its group goes.
                    crate::hold::kill_job_group(&h);
                    preset = Some((
                        StopReason { state: JobState::Interrupted, reason: k2_node_proto::share::codes::NODE_UPDATE.into() },
                        "the job's holder is gone and never said how the job ended; its group was stopped".into(),
                    ));
                }
                Proc::Holder { facts: h, dir, gone: false, timeout: false }
            }
            RunKind::Provisional => unreachable!("sorted: provisional rows are never adoptable"),
        };
        let journal_path = t.paths.journal(&job, gen);
        let mut journal = match JournalWriter::reopen(&journal_path, a.plan.limits.log_cap_bytes) {
            Ok(j) => j,
            Err(e) => {
                crate::nlog!("job {job} gen {gen}: reopen its journal: {e}; a new one starts");
                match JournalWriter::create(&journal_path, a.plan.limits.log_cap_bytes) {
                    Ok(j) => j,
                    Err(e) => {
                        crate::nlog!("job {job} gen {gen}: no journal ({e}); stopping it");
                        let mut p = proc;
                        self.stop_unadopted(&mut p).await;
                        let _ = t.ledger.lock().unwrap().finish(&job, gen, JobState::Interrupted, Some(k2_node_proto::share::codes::NODE_UPDATE), Some(&e), None, row.log_seq, self.now());
                        return;
                    }
                }
            }
        };
        journal.dropped = f.log_dropped;
        let sys = |j: &mut JournalWriter, m: String| {
            let _ = j.append(LogStream::Sys, format!("[k2-node] {m}\n").as_bytes());
        };
        let mut outs: Vec<(OutSrc, LogStream)> = Vec::new();
        for (sf, stream) in [(f.out.as_ref(), LogStream::Out), (f.err.as_ref(), LogStream::Err)] {
            let Some(sf) = sf else { continue };
            match crate::spool::reopen(sf) {
                Ok(file) => outs.push((OutSrc::Spool(crate::spool::Tail::new(file, sf.off)), stream)),
                Err(e) => sys(&mut journal, format!("this job's output after the update can't be read: {e}")),
            }
        }
        // Back in the running map with its reservation (C9), its port block,
        // and its place in the node queue's accounting.
        let (stop_tx, stop_rx) = watch::channel(None);
        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(
                (t.slot, job.clone()),
                RunningJob {
                    generation: gen,
                    cpu_millis: a.plan.limits.cpu_millis,
                    mem_bytes: a.plan.limits.mem_bytes,
                    exclusive: a.plan.exclusive,
                    stop: stop_tx,
                    seq: self.job_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                    gate: a.plan.tier_or_class() == k2_node_proto::tiers::Tier::Ci,
                    workspace: a.plan.workspace_id.clone(),
                    secret: false,
                    port_block: f.port_block,
                    adoptable: true,
                    rust: f.rust.clone(),
                },
            );
        }
        let caps = self.queue_caps();
        self.fairq.started(t.slot, &a, self.now(), &caps);
        self.set_ci_run(t, &job, gen, f.ci_run.clone());
        let lease = f.lease.as_ref().map(|l| {
            if let Some(n) = l.slot {
                self.slots.lock().unwrap().mark_busy(&l.project_key, n);
            }
            l.lease()
        });
        if let Some(h) = &f.job_cache {
            self.job_cache_readopt(t.slot, &job, gen, h);
        }
        if f.smoke {
            // The lock names the last process's pid: it's ours now.
            let path = self.local.lock().unwrap().policy.write_smoke_lock.clone();
            if let Some(p) = path {
                crate::locks::clear_stale_smoke(&p);
                let now = self.now();
                self.smoke.lock().unwrap().acquire(&p, &job, now);
            }
        }
        sys(&mut journal, format!("adopted by k2-node {} (started by k2-node {}); the job kept running through the update", super::NODE_VERSION, f.node_version));
        self.status.lock().unwrap().adopted.push(crate::status::AdoptedJob { job: key.clone(), from: f.node_version.clone() });
        crate::nlog!("job {job} gen {gen}: adopted ({:?}, started by k2-node {})", f.kind, f.node_version);
        let remaining = (f.deadline - self.now()).max(0) as u64;
        let (preset, preset_detail) = match preset {
            Some((r, d)) => (Some(r), Some(d)),
            None => (None, None),
        };
        let live = Live {
            dirs,
            cwd: f.cwd.clone(),
            target: f.target.clone(),
            src_ran: row.src.clone(),
            lease,
            proc,
            outs,
            journal,
            redactor: None,
            mem_limit: f.mem_limit,
            easy: f.easy,
            tools_used: f.tools_used.clone(),
            deadline: std::time::Instant::now() + Duration::from_secs(remaining),
            facts: Some(Arc::new(Mutex::new(f))),
            preset,
            preset_detail,
            adopted: true,
        };
        self.jobs_changed();
        let (node, tenant) = (self.clone(), t.clone());
        tokio::spawn(async move { node.supervise(tenant, a, live, stop_rx).await });
    }

    /// A job this process can't take over: stop its process.
    async fn stop_unadopted(&self, p: &mut Proc) {
        match p {
            Proc::Unit { unit, .. } => {
                let _ = self.job_op(crate::admin::jobs::JobOp::Release { unit: unit.clone() }, vec![], Duration::from_secs(60)).await;
            }
            Proc::Holder { facts, .. } => {
                crate::hold::terminate(facts.pid, facts.start);
                crate::hold::kill_job_group(facts);
            }
            Proc::Local { pgid, .. } => kill_job_group(*pgid),
        }
    }

    /// Spool files and holder folders no adopted job of `t` owns: left by
    /// jobs that ended or were stopped while no node ran (§5.1).
    fn sweep_run_leftovers(&self, t: &Tenant, keep: &[String]) {
        let spool = t.paths.state().join("spool");
        if let Ok(rd) = std::fs::read_dir(&spool) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let key = name.rsplit_once('.').map(|(k, _)| k.to_string()).unwrap_or_default();
                if !keep.contains(&key) && e.file_type().is_ok_and(|ft| ft.is_file() || ft.is_symlink()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let hold = t.paths.state().join("hold");
        if let Ok(rd) = std::fs::read_dir(&hold) {
            for e in rd.flatten() {
                let key = e.file_name().to_string_lossy().into_owned();
                if keep.contains(&key) {
                    continue;
                }
                if e.file_type().is_ok_and(|ft| ft.is_dir()) {
                    let _ = std::fs::remove_dir_all(e.path());
                } else {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    /// §5.4: this process stops for an update the executor said keeps jobs.
    /// Every job with `unit`/`holder` facts is left running (its tailers
    /// record how far the journal got); every other job stops with
    /// `node_update`. Waits (≤ 15 s) until each is off this process.
    /// Returns (kept, stopped).
    pub async fn detach_for_update(&self) -> (usize, usize) {
        let (mut kept, mut stopped) = (0, 0);
        {
            let jobs = self.jobs.lock().unwrap();
            for ((_, id), j) in jobs.iter() {
                if id.starts_with(super::LINGER_PREFIX) || j.stop.borrow().is_some() {
                    continue;
                }
                if j.adoptable {
                    j.stop.send_replace(Some(StopReason { state: JobState::Interrupted, reason: DETACH.into() }));
                    kept += 1;
                } else {
                    j.stop.send_replace(Some(StopReason { state: JobState::Interrupted, reason: k2_node_proto::share::codes::NODE_UPDATE.into() }));
                    stopped += 1;
                }
            }
        }
        for _ in 0..150 {
            let left = self.jobs.lock().unwrap().keys().filter(|(_, id)| !id.starts_with(super::LINGER_PREFIX)).count();
            if left == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        (kept, stopped)
    }

    /// C2, at start: every tenant slot with a ledger on disk. One whose
    /// server will open again (its `servers.toml` line, its pin and the
    /// line mark all still say so) keeps its rows for `add_tenant` to
    /// adopt, with a [`TENANT_ADOPT_WAIT`] watch; one that won't has its
    /// jobs stopped and their units released now, before anything of the
    /// slot is deleted.
    pub(super) async fn settle_tenant_slots(self: &Arc<Self>) {
        let file = crate::admin::servers::read_file(&self.layout().config).ok().flatten();
        for slot in 1..k2_node_proto::share::MAX_SLOTS {
            let tl = self.layout().tenant(slot);
            if !tl.ledger().is_file() || self.tenant(slot).is_some() {
                continue;
            }
            let returns = file.as_ref().and_then(|f| f.entry(slot)).is_some_and(|e| {
                let marked = std::fs::read_to_string(tl.state().join(crate::session::LINE_MARK_FILE)).is_ok_and(|m| m == crate::session::line_mark(e));
                let pin = crate::identity::read_tenant_pin(&tl.pin()).ok().flatten();
                marked && pin.is_some_and(|(p, _)| super::tenant::pin_serves(e, &p))
            });
            if returns {
                let node = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(node.opts.tenant_adopt_wait).await;
                    if node.tenant(slot).is_none() {
                        crate::nlog!("server slot {slot} didn't open again within {} s; stopping the jobs it left running", node.opts.tenant_adopt_wait.as_secs());
                        node.stop_slot_jobs(slot).await;
                    }
                });
            } else {
                self.stop_slot_jobs(slot).await;
            }
        }
    }

    /// Stop every job slot `slot` left running (it won't be adopted): the
    /// ledger rows end `node_update`, units are released, holders and
    /// groups stopped; its stand-ins hold the slot until they're gone.
    pub(super) async fn stop_slot_jobs(self: &Arc<Self>, slot: Slot) {
        if slot == OWNER_SLOT || self.tenant(slot).is_some() {
            return;
        }
        let t = match Tenant::open(self.layout(), slot) {
            Ok(t) => t,
            Err(e) => {
                crate::nlog!("server slot {slot}: can't open its ledger to stop its jobs: {e}");
                return;
            }
        };
        match super::tenant::reconcile(&t, &self.opts, &self.wall, false).await {
            Ok((lingering, _)) => {
                if !lingering.is_empty() {
                    crate::nlog!("server slot {slot}: its server won't come back; {} job unit(s) stopped", lingering.len());
                }
                self.hold_lingering(slot, lingering);
            }
            Err(e) => crate::nlog!("server slot {slot}: stopping its jobs: {e}"),
        }
        self.sweep_run_leftovers(&t, &[]);
    }
}
