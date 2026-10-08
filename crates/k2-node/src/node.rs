//! The node core: local state, offers, assigns, and running jobs.
//!
//! One [`Node`] lives for the whole process. Sessions come and go
//! ([`crate::session`]); jobs keep running and journaling while no
//! session exists (no lease kill: these are the owner's own machines and
//! the controller is often a laptop, §11.5).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{
    Assign, Availability, Caps, Control, ExitInfo, Frame, Free, JobState, Need, NodeLimits, Offer, Receipt,
    ReceiptLog, SignedReceipt, SrcRan, StateUpdate, Transfer, TransferFailed, TransferKind, PROTOCOL,
};
use k2_node_proto::refusal::{self, JobAsk, NodeView};
use tokio::sync::{mpsc, watch, Notify};

use crate::cgroup::{Cgroups, Leaf};
use crate::config::{self, AvailabilityMode, ControlState, MachineFacts, OnOwnerReturn, Policy};
use crate::identity::{self, Pin};
use crate::journal::JournalWriter;
use crate::ledger::{Clock, Ledger};
use crate::locks::{self, LockScan, SmokeLock};
use crate::paths::{safe_relative, Layout};
use crate::runner::{self, JobDirs, RssSampler};
use crate::slots::{Lease, Slots};
use crate::status::Status;
use crate::sync::{valid_sha, Git};

pub const NODE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Knobs (tests shorten the waits).
#[derive(Clone)]
pub struct NodeOptions {
    pub layout: Layout,
    pub dev: bool,
    pub user: String,
    pub cancel_grace: Duration,
    pub config_poll: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    pub offer_every: Duration,
    pub ping_every: Duration,
    pub transfer_timeout: Duration,
    pub keep_awake: bool,
    pub use_cgroups: bool,
    pub rss: Arc<dyn RssSampler>,
    pub rss_every: Duration,
    pub clock: Arc<dyn Clock>,
    /// Override machine facts (tests).
    pub facts: Option<MachineFacts>,
}

impl NodeOptions {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            dev: false,
            user: current_user(),
            cancel_grace: Duration::from_secs(10),
            config_poll: Duration::from_secs(2),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            offer_every: Duration::from_secs(k2_node_proto::frames::OFFER_SECS),
            ping_every: Duration::from_secs(k2_node_proto::frames::PING_SECS),
            transfer_timeout: Duration::from_secs(1800),
            keep_awake: true,
            use_cgroups: true,
            rss: runner::default_sampler(),
            rss_every: Duration::from_secs(1),
            clock: Arc::new(crate::ledger::SystemClock::new()),
            facts: None,
        }
    }
}

pub fn current_user() -> String {
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        }
    }
    std::env::var("USER").unwrap_or_else(|_| "k2node".into())
}

pub fn machine_facts(layout: &Layout) -> MachineFacts {
    let (_, total) = crate::sysinfo::disk(&layout.home);
    MachineFacts {
        cores: crate::sysinfo::cores(),
        mem_bytes: crate::sysinfo::mem_total(),
        disk_total_bytes: total,
        has_battery: crate::sysinfo::power().0,
        is_macos: cfg!(target_os = "macos"),
    }
}

/// Why a running job is being stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopReason {
    pub state: JobState,
    pub reason: String,
}

struct RunningJob {
    generation: u32,
    cpu_millis: Option<u64>,
    mem_bytes: Option<u64>,
    exclusive: bool,
    stop: watch::Sender<Option<StopReason>>,
}

/// The machine owner's side, refreshed every poll.
#[derive(Debug, Clone)]
pub struct Local {
    pub policy: Policy,
    pub policy_error: Option<String>,
    pub control: ControlState,
    pub on_battery: bool,
    pub unavailable: Vec<String>,
    pub locks: LockScan,
}

pub enum TransferEvent {
    Chunk(Transfer),
    Failed(TransferFailed),
}

/// TransferKind isn't `Hash`; key on its wire name.
pub type TransferKey = (String, u32, &'static str);

pub fn kind_name(k: TransferKind) -> &'static str {
    match k {
        TransferKind::Bundle => "bundle",
        TransferKind::Dirty => "dirty",
    }
}

/// The live session's outbound queue, if any.
pub struct Hub {
    conn: Mutex<Option<(u64, mpsc::Sender<Frame>)>>,
    next_epoch: AtomicU64,
    /// Current epoch (0 = no session).
    pub connected: watch::Sender<u64>,
    transfers: Mutex<HashMap<TransferKey, mpsc::UnboundedSender<TransferEvent>>>,
}

impl Hub {
    fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            next_epoch: AtomicU64::new(1),
            connected: watch::channel(0).0,
            transfers: Mutex::new(HashMap::new()),
        }
    }

    /// Register a session's sender. Returns its epoch.
    pub fn attach(&self, tx: mpsc::Sender<Frame>) -> u64 {
        let e = self.next_epoch.fetch_add(1, Ordering::SeqCst);
        *self.conn.lock().unwrap() = Some((e, tx));
        self.connected.send_replace(e);
        e
    }

    pub fn detach(&self, epoch: u64) {
        let mut g = self.conn.lock().unwrap();
        if g.as_ref().is_some_and(|(e, _)| *e == epoch) {
            *g = None;
            self.connected.send_replace(0);
        }
    }

    /// Queue a frame on the live session; false when there is none (or
    /// it's full: everything sent this way is also recoverable).
    pub fn send(&self, f: Frame) -> bool {
        let g = self.conn.lock().unwrap();
        match g.as_ref() {
            Some((_, tx)) => tx.try_send(f).is_ok(),
            None => false,
        }
    }

    pub fn route_transfer(&self, key: TransferKey, ev: TransferEvent) {
        if let Some(tx) = self.transfers.lock().unwrap().get(&key) {
            let _ = tx.send(ev);
        }
    }
}

pub struct Node {
    pub opts: NodeOptions,
    pub key: SigningKey,
    pub ledger: Mutex<Ledger>,
    pub pin: Mutex<Option<Pin>>,
    pub local: Mutex<Local>,
    pub facts: MachineFacts,
    narrow: Mutex<NodeLimits>,
    jobs: Mutex<HashMap<String, RunningJob>>,
    slots: Mutex<Slots>,
    smoke: Mutex<SmokeLock>,
    pub tools: Mutex<BTreeMap<String, String>>,
    cgroups: Option<Cgroups>,
    pub hub: Hub,
    /// Wakes the session's log shipper.
    pub ship: Notify,
    /// Wakes the session's offer sender.
    pub offer_changed: Notify,
    offer_rev: AtomicU64,
    last_offer: Mutex<Option<Offer>>,
    pub status: Mutex<Status>,
    keep_awake: Mutex<Option<tokio::process::Child>>,
    /// Learned routes (Welcome) tried before the pinned ones.
    pub learned_routes: Mutex<Vec<String>>,
}

impl Node {
    /// Open the key, ledger and pin; mark jobs from another boot.
    pub async fn open(opts: NodeOptions) -> Result<Arc<Self>, String> {
        let layout = opts.layout.clone();
        layout.ensure()?;
        let key = identity::load_or_create_key(&layout.key())?;
        let ledger = Ledger::open(&layout.ledger())?;
        let changed = ledger.reconcile_boot(opts.clock.as_ref())?;
        for (j, g) in &changed {
            crate::nlog!("job {j} gen {g}: interrupted by a node reboot");
        }
        // Same boot, new process: the old process's jobs can't be re-adopted.
        for (j, g, pgid) in ledger.interrupt_open("node_restart", opts.clock.as_ref())? {
            crate::nlog!("job {j} gen {g}: interrupted by a k2-node restart");
            // The node user can only signal its own processes, so a reused
            // pgid can only be another k2node process; never our own group,
            // and never in --dev (there the user is a human).
            let own = unsafe { (libc::getpid(), libc::getpgrp()) };
            if let Some(p) = pgid.filter(|p| *p != own.0 && *p != own.1 && !opts.dev) {
                runner::signal_group(p, libc::SIGKILL);
            }
        }
        let pin = identity::read_pin(&layout.pin())?;
        let facts = opts.facts.clone().unwrap_or_else(|| machine_facts(&layout));
        let policy = Policy::resolve(&config::PolicyFile::default(), &facts)?;
        let cgroups = if opts.use_cgroups { Cgroups::detect() } else { None };
        let node = Arc::new(Self {
            key,
            ledger: Mutex::new(ledger),
            pin: Mutex::new(pin),
            local: Mutex::new(Local {
                policy,
                policy_error: None,
                control: ControlState { control: Control::Active, by: None, at: None },
                on_battery: false,
                unavailable: vec![],
                locks: LockScan::default(),
            }),
            facts,
            narrow: Mutex::new(NodeLimits::default()),
            jobs: Mutex::new(HashMap::new()),
            slots: Mutex::new(Slots::default()),
            smoke: Mutex::new(SmokeLock::default()),
            tools: Mutex::new(BTreeMap::new()),
            cgroups,
            hub: Hub::new(),
            ship: Notify::new(),
            offer_changed: Notify::new(),
            offer_rev: AtomicU64::new(0),
            last_offer: Mutex::new(None),
            status: Mutex::new(Status::default()),
            keep_awake: Mutex::new(None),
            learned_routes: Mutex::new(Vec::new()),
            opts,
        });
        node.refresh_local(true);
        node.probe_tools().await;
        {
            let mut s = node.status.lock().unwrap();
            s.version = NODE_VERSION.to_string();
            s.protocol = PROTOCOL;
            s.node_fp = node.key.fingerprint();
            s.caps_hard = node.cgroups.is_some();
            s.since = crate::util::now();
            s.notes = vec!["idle-based availability is not built yet; idle modes act like always / plugged_in".into()];
        }
        node.write_status();
        Ok(node)
    }

    pub fn layout(&self) -> &Layout {
        &self.opts.layout
    }

    pub fn now(&self) -> i64 {
        self.opts.clock.now()
    }

    pub fn job_env_base(&self) -> Vec<(String, String)> {
        vec![
            ("PATH".into(), crate::util::base_path(&self.layout().home)),
            ("HOME".into(), self.layout().git_home().display().to_string()),
            ("CARGO_HOME".into(), self.layout().toolchains().join("cargo").display().to_string()),
            ("RUSTUP_HOME".into(), self.layout().toolchains().join("rustup").display().to_string()),
            ("LANG".into(), "C".into()),
        ]
    }

    pub async fn probe_tools(&self) {
        let tools = crate::sysinfo::probe_tools(&self.job_env_base()).await;
        *self.tools.lock().unwrap() = tools;
        self.offer_changed.notify_one();
    }

    // ── local state ─────────────────────────────────────────────────

    /// Re-read policy, control, power and locks. Applies `stopped` and
    /// `on_owner_return = pause_now` to running jobs.
    pub fn refresh_local(&self, check_power: bool) {
        let layout = self.layout().clone();
        let (policy, policy_error) = match config::read_policy_file(&layout.policy()).and_then(|f| Policy::resolve(&f, &self.facts)) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        let control = config::read_control(&layout.control());
        let now = self.now();
        let mut l = self.local.lock().unwrap();
        let before = (l.control.control, l.unavailable.clone(), l.policy_error.clone(), l.locks.clone());
        if let Some(p) = policy {
            l.policy = p;
        }
        l.policy_error = policy_error.clone();
        match control {
            Ok(c) => l.control = c,
            Err(e) => {
                // Unreadable control = stopped for new work; say why.
                l.policy_error = Some(e);
            }
        }
        if check_power {
            l.on_battery = crate::sysinfo::power().1;
        }
        let mut un = Vec::new();
        match l.policy.availability {
            AvailabilityMode::PluggedIn | AvailabilityMode::IdleAndPluggedIn if l.on_battery => un.push(refusal::ON_BATTERY.to_string()),
            AvailabilityMode::Window => {
                let (wd, min) = config::local_weekday_minute(now);
                if !config::in_window(&l.policy.window, wd, min) {
                    un.push(refusal::OUTSIDE_WINDOW.to_string());
                }
            }
            _ => {}
        }
        l.unavailable = un;
        let owned = self.smoke.lock().unwrap().owned();
        l.locks = locks::scan(&l.policy.foreign_locks, l.policy.write_smoke_lock.as_deref(), owned, now);
        let after = (l.control.control, l.unavailable.clone(), l.policy_error.clone(), l.locks.clone());
        let stop_all = l.control.control == Control::Stopped;
        let pause_now = l.policy.on_owner_return == OnOwnerReturn::PauseNow && !l.unavailable.is_empty();
        let pause_reason = l.unavailable.first().cloned();
        drop(l);
        if stop_all {
            self.stop_all(JobState::Interrupted, refusal::LOCAL_STOPPED);
        } else if pause_now {
            self.stop_all(JobState::Interrupted, pause_reason.as_deref().unwrap_or(refusal::OUTSIDE_WINDOW));
        }
        if before != after {
            self.offer_changed.notify_one();
            self.write_status();
        }
    }

    fn effective_control(&self, l: &Local) -> Control {
        if l.policy_error.is_some() {
            Control::Paused
        } else {
            l.control.control
        }
    }

    fn caps(&self, l: &Local) -> (u64, u64, u32) {
        let n = self.narrow.lock().unwrap().clone();
        let mut cpu = l.policy.cpu_cap_millis(self.facts.cores);
        let mut mem = l.policy.mem_cap_bytes;
        let mut par = l.policy.max_parallel;
        if let Some(c) = n.cpu_millis {
            cpu = cpu.min(c);
        }
        if let Some(m) = n.mem_bytes {
            mem = mem.min(m);
        }
        if let Some(p) = n.max_parallel {
            par = par.min(p.max(1));
        }
        (cpu, mem, par)
    }

    fn used(&self, cap_cpu: u64, cap_mem: u64, par: u32) -> (u64, u64, u32, bool) {
        let jobs = self.jobs.lock().unwrap();
        let share = |cap: u64| cap / par.max(1) as u64;
        let cpu: u64 = jobs.values().map(|j| j.cpu_millis.unwrap_or_else(|| share(cap_cpu))).sum();
        let mem: u64 = jobs.values().map(|j| j.mem_bytes.unwrap_or_else(|| share(cap_mem))).sum();
        (cpu, mem, jobs.len() as u32, jobs.values().any(|j| j.exclusive))
    }

    /// Controller narrowing: only tightens.
    pub fn narrow(&self, n: NodeLimits) {
        *self.narrow.lock().unwrap() = n;
        self.offer_changed.notify_one();
    }

    /// The offer right now (revision bumps only when content changed).
    pub fn offer(&self) -> Offer {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(&l);
        let (used_cpu, used_mem, running, excl) = self.used(cap_cpu, cap_mem, par);
        let (disk_free, _) = crate::sysinfo::disk(&self.layout().home);
        let mut free_mem = cap_mem.saturating_sub(used_mem);
        if let Some(avail) = crate::sysinfo::mem_available() {
            free_mem = free_mem.min(avail);
        }
        let tools = self.tools.lock().unwrap().clone();
        let control = self.effective_control(&l);
        let view = NodeView {
            control,
            unavailable: &l.unavailable,
            foreign_lock: l.locks.holding.as_deref(),
            disk_free_bytes: disk_free,
            disk_floor_bytes: l.policy.disk_floor_bytes,
            workspaces_allow: &l.policy.workspaces,
            tools: &tools,
            running,
            parallel_max: par,
            exclusive_running: excl,
            free_cpu_millis: cap_cpu.saturating_sub(used_cpu),
            free_mem_bytes: free_mem,
        };
        let node_refusal = refusal::node_refusal(&view);
        let pin = self.pin.lock().unwrap().clone();
        let mut labels = pin.as_ref().map(|p| p.labels.clone()).unwrap_or_default();
        labels.insert("os".into(), crate::sysinfo::os_label().into());
        labels.insert("arch".into(), crate::sysinfo::arch_label().into());
        if let Some(p) = &pin {
            labels.insert("name".into(), p.name.clone());
        }
        if crate::sysinfo::is_vm() {
            labels.insert("vm".into(), "true".into());
        }
        let ledger_id = self.ledger.lock().unwrap().ledger_id.clone();
        let mut o = Offer {
            revision: 0,
            control,
            control_by: l.control.by.clone(),
            control_at: l.control.at,
            availability: Availability { ok: l.unavailable.is_empty(), reasons: l.unavailable.clone() },
            caps: Caps { cpu_millis: cap_cpu, mem_bytes: cap_mem, disk_budget_bytes: l.policy.disk_budget_bytes, hard: self.cgroups.is_some() },
            free: Free { cpu_millis: cap_cpu.saturating_sub(used_cpu), mem_bytes: free_mem, disk_free_bytes: disk_free },
            slots: self.slots.lock().unwrap().list(&self.layout().cache()),
            running,
            parallel_max: par,
            refusal: node_refusal,
            labels,
            tools,
            protocol: PROTOCOL,
            boot_id: self.opts.clock.boot_id(),
            ledger_id,
            node_version: NODE_VERSION.to_string(),
        };
        let mut last = self.last_offer.lock().unwrap();
        let same = last.as_ref().is_some_and(|p| {
            let mut q = p.clone();
            q.revision = 0;
            // Free disk moves constantly; only a 1 GiB change is news.
            q.free.disk_free_bytes = o.free.disk_free_bytes;
            q == o && p.free.disk_free_bytes.abs_diff(o.free.disk_free_bytes) < (1 << 30)
        });
        if same {
            o.revision = last.as_ref().map(|p| p.revision).unwrap_or(0);
            o.free.disk_free_bytes = last.as_ref().map(|p| p.free.disk_free_bytes).unwrap_or(0);
        } else {
            o.revision = self.offer_rev.fetch_add(1, Ordering::SeqCst) + 1;
            *last = Some(o.clone());
        }
        o
    }

    // ── assigns ─────────────────────────────────────────────────────

    fn state_frame(&self, job_id: &str, generation: u32, state: JobState, reason: Option<&str>, detail: Option<&str>) -> Frame {
        Frame::State(StateUpdate {
            job_id: job_id.to_string(),
            generation,
            state,
            reason: reason.map(str::to_string),
            detail: detail.map(str::to_string),
            at: self.now(),
            exit: None,
            src: None,
            log_seq: 0,
        })
    }

    pub fn row_state_frame(&self, r: &crate::ledger::Row) -> Frame {
        Frame::State(StateUpdate {
            job_id: r.job_id.clone(),
            generation: r.generation,
            state: r.state,
            reason: r.reason.clone(),
            detail: r.detail.clone(),
            at: r.ended_at.or(r.started_at).unwrap_or(r.created_at),
            exit: r.exit.clone(),
            src: r.src.clone(),
            log_seq: r.log_seq,
        })
    }

    /// Handle an `assign`. Returns `Some((job_id, generation))` when a new
    /// attempt started (the session tracks its logs).
    pub fn on_assign(self: &Arc<Self>, a: Assign) -> Option<(String, u32)> {
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        if !a.consistent() {
            self.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("plan_mismatch"), Some("plan digest or job id doesn't match the fence")));
            return None;
        }
        let my_id = self.pin.lock().unwrap().as_ref().map(|p| p.node_id.clone()).unwrap_or_default();
        if a.plan.node_id != my_id {
            self.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("wrong_node"), Some("this plan names another node")));
            return None;
        }
        // No blind replay: a known fence (or a newer generation) only
        // reports its state.
        let known = {
            let l = self.ledger.lock().unwrap();
            match l.get(&job, gen) {
                Ok(Some(r)) => Some(r),
                _ => l.latest(&job).ok().flatten().filter(|r| r.generation > gen),
            }
        };
        if let Some(r) = known {
            self.hub.send(self.row_state_frame(&r));
            return None;
        }
        if let Some(r) = self.assign_refusal(&a) {
            self.hub.send(self.state_frame(&job, gen, JobState::Queued, Some(&r.code), Some(&r.message)));
            return None;
        }
        if let Err(e) = self.ledger.lock().unwrap().insert(&a, self.opts.clock.as_ref()) {
            self.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("ledger_error"), Some(&e)));
            return None;
        }
        let (stop_tx, stop_rx) = watch::channel(None);
        self.jobs.lock().unwrap().insert(
            job.clone(),
            RunningJob {
                generation: gen,
                cpu_millis: a.plan.limits.cpu_millis,
                mem_bytes: a.plan.limits.mem_bytes,
                exclusive: a.plan.exclusive,
                stop: stop_tx,
            },
        );
        self.jobs_changed();
        let node = self.clone();
        tokio::spawn(async move { node.run_job(a, stop_rx).await });
        Some((job, gen))
    }

    fn assign_refusal(&self, a: &Assign) -> Option<k2_node_proto::frames::Refusal> {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(&l);
        let (used_cpu, used_mem, running, excl) = self.used(cap_cpu, cap_mem, par);
        let (disk_free, _) = crate::sysinfo::disk(&self.layout().home);
        let tools = self.tools.lock().unwrap().clone();
        let view = NodeView {
            control: self.effective_control(&l),
            unavailable: &l.unavailable,
            foreign_lock: l.locks.holding.as_deref(),
            disk_free_bytes: disk_free,
            disk_floor_bytes: l.policy.disk_floor_bytes,
            workspaces_allow: &l.policy.workspaces,
            tools: &tools,
            running,
            parallel_max: par,
            exclusive_running: excl,
            free_cpu_millis: cap_cpu.saturating_sub(used_cpu),
            free_mem_bytes: cap_mem.saturating_sub(used_mem),
        };
        let ask = JobAsk {
            workspace_id: &a.plan.workspace_id,
            cpu_millis: a.plan.limits.cpu_millis,
            mem_bytes: a.plan.limits.mem_bytes,
            // `limits.disk_bytes` is the grant's ceiling for this job, not
            // space it needs free right now; a start only needs the node's
            // floor (checked in `node_refusal`). Requiring the ceiling free
            // refused every job on a node with less than 60 GB free.
            disk_bytes: 0,
            exclusive: a.plan.exclusive,
            needs_git: a.plan.src.is_some(),
        };
        refusal::refusal(&view, &ask)
    }

    /// Controller `cancel`.
    pub fn cancel(&self, job_id: &str, generation: u32, reason: &str) {
        let jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.get(job_id).filter(|j| j.generation == generation) {
            j.stop.send_replace(Some(StopReason { state: JobState::Cancelled, reason: reason.to_string() }));
        } else {
            drop(jobs);
            // Not running: report what the ledger has (or that it's unknown).
            let row = self.ledger.lock().unwrap().get(job_id, generation).ok().flatten();
            match row {
                Some(r) => {
                    self.hub.send(self.row_state_frame(&r));
                }
                None => {
                    self.hub.send(self.state_frame(job_id, generation, JobState::Unknown, Some("not_on_node"), None));
                }
            }
        }
    }

    pub fn stop_all(&self, state: JobState, reason: &str) {
        for j in self.jobs.lock().unwrap().values() {
            if j.stop.borrow().is_none() {
                j.stop.send_replace(Some(StopReason { state, reason: reason.to_string() }));
            }
        }
    }

    /// `revoked` from the controller (or `Refused{revoked}`).
    pub fn revoke(&self, reason: &str) {
        crate::nlog!("controller removed this node: {reason}");
        self.stop_all(JobState::Cancelled, "node_removed");
        let mut g = self.pin.lock().unwrap();
        if let Some(p) = g.as_mut() {
            p.revoked = true;
            p.revoked_reason = Some(reason.to_string());
            if let Err(e) = identity::write_pin(&self.layout().pin(), p) {
                crate::nlog!("can't record revoke: {e}");
            }
        }
        drop(g);
        self.set_status_state("revoked", None, Some(format!("Removed by the controller: {reason}")));
    }

    pub fn running_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }

    fn jobs_changed(&self) {
        self.offer_changed.notify_one();
        self.ship.notify_one();
        let running = !self.jobs.lock().unwrap().is_empty();
        self.update_keep_awake(running);
        self.write_status();
    }

    fn update_keep_awake(&self, running: bool) {
        if !self.opts.keep_awake {
            return;
        }
        let mut g = self.keep_awake.lock().unwrap();
        if running && g.is_none() {
            let cmd = if cfg!(target_os = "macos") {
                Some(("/usr/bin/caffeinate", vec!["-i", "-s"]))
            } else {
                crate::sysinfo::which("systemd-inhibit", &[("PATH".into(), "/usr/bin:/bin".into())]).map(|_| {
                    ("systemd-inhibit", vec!["--what=sleep", "--why=k2 compute job", "--mode=block", "sleep", "infinity"])
                })
            };
            if let Some((prog, args)) = cmd {
                let mut c = tokio::process::Command::new(prog);
                c.args(args).env_clear().env("PATH", "/usr/bin:/bin").kill_on_drop(true);
                c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                *g = c.spawn().ok();
            }
        } else if !running {
            *g = None; // kill_on_drop
        }
    }

    // ── status ──────────────────────────────────────────────────────

    pub fn set_status_state(&self, state: &str, route: Option<String>, err: Option<String>) {
        {
            let mut s = self.status.lock().unwrap();
            if s.state != state {
                s.since = crate::util::now();
            }
            s.state = state.to_string();
            if route.is_some() || state != "online" {
                s.route = route;
            }
            if err.is_some() || state == "online" {
                s.last_error = err;
            }
        }
        self.write_status();
    }

    pub fn write_status(&self) {
        let pin = self.pin.lock().unwrap().clone();
        let l = self.local.lock().unwrap().clone();
        let running = self.running_ids();
        let mut s = self.status.lock().unwrap();
        s.name = pin.as_ref().map(|p| p.name.clone());
        s.node_id = pin.as_ref().map(|p| p.node_id.clone());
        s.controller = pin.as_ref().map(|p| p.controller_label());
        s.controller_fp = pin.as_ref().map(|p| p.controller_fp.clone());
        s.sas = pin.as_ref().filter(|p| !p.confirmed && !p.revoked).map(|p| p.sas.clone());
        s.running = running;
        s.control = self.effective_control(&l).as_str().to_string();
        s.control_by = l.control.by.clone();
        s.policy_error = l.policy_error.clone();
        s.unavailable = l.unavailable.clone();
        s.holding_lock = l.locks.holding.clone();
        s.stale_locks = l.locks.stale.clone();
        s.updated_at = crate::util::now();
        if s.state.is_empty() {
            s.state = if pin.is_none() { "unenrolled".into() } else { "connecting".into() };
        }
        let snapshot = s.clone();
        drop(s);
        if let Err(e) = crate::status::write(&self.layout().status(), &snapshot) {
            crate::nlog!("status: {e}");
        }
    }

    // ── transfers ───────────────────────────────────────────────────

    /// Ask the controller for a blob and write it to `dest`. Retries across
    /// reconnects; stops on the job's stop signal or the overall timeout.
    async fn fetch_blob(
        &self,
        need: Need,
        dest: &Path,
        stop: &mut watch::Receiver<Option<StopReason>>,
    ) -> Result<(), String> {
        let key: TransferKey = (need.job_id.clone(), need.generation, kind_name(need.kind));
        let deadline = tokio::time::Instant::now() + self.opts.transfer_timeout;
        let mut connected = self.hub.connected.subscribe();
        loop {
            // Wait for a session.
            while *connected.borrow() == 0 {
                tokio::select! {
                    r = connected.changed() => { if r.is_err() { return Err("node shutting down".into()); } }
                    _ = stop.changed() => return Err("stopped".into()),
                    _ = tokio::time::sleep_until(deadline) => return Err("transfer timed out".into()),
                }
            }
            let epoch = *connected.borrow();
            let (tx, mut rx) = mpsc::unbounded_channel();
            self.hub.transfers.lock().unwrap().insert(key.clone(), tx);
            let _ = std::fs::remove_file(dest);
            let mut file = std::fs::File::create(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
            if !self.hub.send(Frame::Need(need.clone())) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            let mut next = 0u32;
            let mut hash = crypto::Sha256Stream::default();
            let mut got = 0u64;
            let outcome: Option<Result<(), String>> = loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        None => break None,
                        Some(TransferEvent::Failed(f)) => break Some(Err(f.code)),
                        Some(TransferEvent::Chunk(t)) => {
                            if t.seq != next {
                                break Some(Err(format!("transfer chunk {} out of order (want {next})", t.seq)));
                            }
                            next += 1;
                            let data = match crypto::unb64(&t.data) { Ok(d) => d, Err(e) => break Some(Err(e)) };
                            if data.len() > k2_node_proto::frames::TRANSFER_CHUNK {
                                break Some(Err("transfer chunk too large".into()));
                            }
                            got += data.len() as u64;
                            if got > t.total_bytes {
                                break Some(Err("transfer longer than announced".into()));
                            }
                            hash.update(&data);
                            if let Err(e) = std::io::Write::write_all(&mut file, &data) { break Some(Err(format!("write blob: {e}"))); }
                            if t.last {
                                let sha = std::mem::take(&mut hash).finish_hex();
                                if got != t.total_bytes || !crypto::ct_eq(sha.as_bytes(), t.sha256.as_bytes()) {
                                    break Some(Err("transfer digest mismatch".into()));
                                }
                                break Some(Ok(()));
                            }
                        }
                    },
                    r = connected.changed() => {
                        if r.is_err() || *connected.borrow() != epoch { break None; }
                    }
                    _ = stop.changed() => break Some(Err("stopped".into())),
                    _ = tokio::time::sleep_until(deadline) => break Some(Err("transfer timed out".into())),
                }
            };
            self.hub.transfers.lock().unwrap().remove(&key);
            match outcome {
                Some(r) => return r,
                None => continue, // session changed: ask again
            }
        }
    }

    // ── one job ─────────────────────────────────────────────────────

    async fn run_job(self: Arc<Self>, a: Assign, mut stop: watch::Receiver<Option<StopReason>>) {
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        let dirs = JobDirs::new(self.layout().job_dir(&job, gen));
        let mut journal = match dirs.create().and_then(|_| JournalWriter::create(&dirs.log, a.plan.limits.log_cap_bytes)) {
            Ok(j) => j,
            Err(e) => {
                self.finish_job(&a, None, JobState::Failed, Some("job_dir_failed"), Some(&e), None, None, None, None).await;
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
            match self.sync_src(&a, &dirs, &mut journal, &mut stop).await {
                Ok(ran) => {
                    cwd = dirs.src.clone();
                    let pc = self.layout().project_cache(&src.project_key);
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
                    self.finish_job(&a, Some(&dirs), state, Some(&reason), Some(&detail), None, None, Some(journal), None).await;
                    return;
                }
            }
        }
        if let Some(rel) = &a.plan.cwd {
            match safe_relative(rel) {
                Some(p) => cwd = cwd.join(p),
                None => {
                    self.finish_job(&a, Some(&dirs), JobState::Failed, Some("cwd_invalid"), Some(rel), src_ran, None, Some(journal), lease).await;
                    return;
                }
            }
        }
        let target = lease.as_ref().map(|l| l.dir.clone()).unwrap_or_else(|| dirs.private_target.clone());
        let _ = std::fs::create_dir_all(&target);
        let env = match runner::job_env(&runner::EnvInputs {
            node_home: &self.layout().home,
            job_home: &dirs.home,
            job_tmp: &dirs.tmp,
            target_dir: &target,
            job_id: &job,
            user: &self.opts.user,
            extra: &a.plan.env,
        }) {
            Ok(e) => e,
            Err((code, name)) => {
                self.finish_job(&a, Some(&dirs), JobState::Failed, Some("env_refused"), Some(&format!("{code}: {name}")), src_ran, None, Some(journal), lease).await;
                return;
            }
        };
        if stop.borrow().is_some() {
            let s = stop.borrow().clone().unwrap_or(StopReason { state: JobState::Cancelled, reason: "cancelled".into() });
            self.finish_job(&a, Some(&dirs), s.state, Some(&s.reason), None, src_ran, None, Some(journal), lease).await;
            return;
        }

        // Leaf + smoke lock + spawn.
        let leaf: Option<Leaf> = self.cgroups.as_ref().and_then(|cg| {
            cg.create_leaf(&format!("{job}-g{gen}"), a.plan.limits.cpu_millis, a.plan.limits.mem_bytes)
                .map_err(|e| crate::nlog!("cgroup leaf for {job}: {e}"))
                .ok()
        });
        self.take_smoke_lock(&job);
        let spawned = match runner::spawn(&a.plan.argv, &env, &cwd, leaf.as_ref()) {
            Ok(s) => s,
            Err(e) => {
                if let Some(l) = &leaf {
                    l.remove();
                }
                sys(&mut journal, &e);
                self.finish_job(&a, Some(&dirs), JobState::Failed, Some("spawn_failed"), Some(&e), src_ran, None, Some(journal), lease).await;
                return;
            }
        };
        let runner::Spawned { mut child, pgid } = spawned;
        {
            let l = self.ledger.lock().unwrap();
            let _ = l.set_running(&job, gen, pgid as u32, src_ran.as_ref(), self.now());
        }
        self.ship.notify_one();

        // Pump output into the journal.
        let journal = Arc::new(Mutex::new(journal));
        let mut pumps = Vec::new();
        for (pipe, stream) in [
            (child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), k2_node_proto::frames::LogStream::Out),
            (child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), k2_node_proto::frames::LogStream::Err),
        ] {
            if let Some(mut pipe) = pipe {
                let j = journal.clone();
                let node = self.clone();
                pumps.push(tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut buf = vec![0u8; k2_node_proto::frames::MAX_LOG_CHUNK];
                    loop {
                        match pipe.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let _ = j.lock().unwrap().append(stream, &buf[..n]);
                                node.ship.notify_one();
                            }
                        }
                    }
                }));
            }
        }

        // Wait: exit, stop, timeout, memory.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(a.plan.limits.max_secs.max(1));
        let mem_cap = if leaf.is_none() {
            Some(a.plan.limits.mem_bytes.unwrap_or_else(|| self.local.lock().unwrap().policy.mem_cap_bytes))
        } else {
            None
        };
        let mut rss_tick = tokio::time::interval(self.opts.rss_every);
        rss_tick.tick().await;
        let mut ended_by: Option<StopReason> = None;
        let mut status = None;
        loop {
            tokio::select! {
                s = child.wait() => { status = s.ok(); break; }
                r = stop.changed() => {
                    if r.is_ok() {
                        if let Some(s) = stop.borrow().clone() { ended_by = Some(s); break; }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    ended_by = Some(StopReason { state: JobState::Timeout, reason: "timeout".into() });
                    break;
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
            runner::stop_group(&mut child, pgid, leaf.as_ref(), self.opts.cancel_grace).await;
            status = child.wait().await.ok();
        }
        let _ = self.ledger.lock().unwrap().set_state(&job, gen, JobState::Finishing, None, None);
        self.ship.notify_one();
        runner::reap_group(pgid, leaf.as_ref());
        for mut p in pumps {
            if tokio::time::timeout(Duration::from_secs(2), &mut p).await.is_err() {
                // Something outside the group still holds the pipe.
                p.abort();
                let _ = p.await;
            }
        }
        if let Some(l) = &leaf {
            l.remove();
        }
        let exit = status.map(|s| {
            use std::os::unix::process::ExitStatusExt;
            ExitInfo { code: s.code(), signal: s.signal() }
        });
        let journal = Arc::try_unwrap(journal).ok().map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()));
        let (state, reason) = match ended_by {
            Some(s) => (s.state, Some(s.reason)),
            None => (JobState::Done, None),
        };
        self.finish_job(&a, Some(&dirs), state, reason.as_deref(), None, src_ran, exit, journal, lease).await;
    }

    /// Mirror → (fetch | bundle) → worktree → dirty. `Err((code, detail))`.
    async fn sync_src(
        &self,
        a: &Assign,
        dirs: &JobDirs,
        journal: &mut JournalWriter,
        stop: &mut watch::Receiver<Option<StopReason>>,
    ) -> Result<SrcRan, (String, String)> {
        let src = a.plan.src.as_ref().ok_or(("sync_failed".to_string(), "no src".to_string()))?;
        let fail = |c: &str, d: String| (c.to_string(), d);
        if !valid_sha(&src.commit) {
            return Err(fail("sync_failed", format!("commit {:?} is not a full sha", src.commit)));
        }
        if src.project_key.is_empty() || !src.project_key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(fail("sync_failed", "project key is not a plain name".into()));
        }
        let git = Git::find(&self.layout().git_home(), &crate::util::base_path(&self.layout().home))
            .ok_or_else(|| fail(refusal::TOOL_MISSING, "git is not installed on the node".into()))?;
        let mirror = self.layout().mirror(&src.project_key);
        let mut sys = |msg: String| {
            let _ = journal.append(k2_node_proto::frames::LogStream::Sys, format!("[k2-node] {msg}\n").as_bytes());
            self.ship.notify_one();
        };
        git.ensure_mirror(&mirror).await.map_err(|e| fail("sync_failed", e))?;
        if !git.has_commit(&mirror, &src.commit).await {
            if let Some(url) = &src.remote_url {
                match git.fetch_remote(&mirror, url).await {
                    Ok(()) => sys(format!("fetched history from {url}")),
                    Err(e) => sys(format!("couldn't fetch from the remote ({e}); asking the controller")),
                }
            }
        }
        if !git.has_commit(&mirror, &src.commit).await {
            let have = git.tips(&mirror).await.unwrap_or_default();
            let blob = dirs.root.join("bundle");
            let need = Need { job_id: a.fence.job_id.clone(), generation: a.fence.generation, kind: TransferKind::Bundle, want: src.commit.clone(), have };
            self.fetch_blob(need, &blob, stop).await.map_err(|e| fail(if e == "stopped" { "stopped" } else { "sync_failed" }, e))?;
            git.fetch_bundle(&mirror, &blob).await.map_err(|e| fail("sync_failed", e))?;
            let _ = std::fs::remove_file(&blob);
            sys("applied the controller's bundle".into());
            if !git.has_commit(&mirror, &src.commit).await {
                return Err(fail("sync_failed", format!("commit {} not found after the bundle", src.commit)));
            }
        }
        let _ = std::fs::remove_dir_all(&dirs.src);
        git.worktree_add(&mirror, &dirs.src, &src.commit).await.map_err(|e| fail("sync_failed", e))?;
        let tree = git.tree_of(&mirror, &src.commit).await.map_err(|e| fail("sync_failed", e))?;
        let mut dirty_sha = None;
        if let Some(d) = &src.dirty {
            let blob = dirs.root.join("dirty.tar");
            let need = Need { job_id: a.fence.job_id.clone(), generation: a.fence.generation, kind: TransferKind::Dirty, want: src.commit.clone(), have: vec![] };
            self.fetch_blob(need, &blob, stop).await.map_err(|e| fail(if e == "stopped" { "stopped" } else { "sync_failed" }, e))?;
            let got = crypto::sha256_hex(&std::fs::read(&blob).map_err(|e| fail("sync_failed", format!("read dirty blob: {e}")))?);
            if !crypto::ct_eq(got.as_bytes(), d.sha256.as_bytes()) {
                return Err(fail("sync_failed", "dirty blob doesn't match the plan's digest".into()));
            }
            git.apply_dirty(&dirs.src, &blob, &dirs.dirty).await.map_err(|e| fail("sync_failed", e))?;
            sys("applied the dirty patch and untracked files".into());
            dirty_sha = Some(d.sha256.clone());
        }
        Ok(SrcRan { commit: src.commit.clone(), tree, dirty_sha256: dirty_sha, slot: None, warmth: None })
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
        &self,
        a: &Assign,
        _dirs: Option<&JobDirs>,
        state: JobState,
        reason: Option<&str>,
        detail: Option<&str>,
        src: Option<SrcRan>,
        exit: Option<ExitInfo>,
        journal: Option<JournalWriter>,
        lease: Option<Lease>,
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
            let pc = self.layout().project_cache(&s.project_key);
            self.slots.lock().unwrap().release(&pc, l, Some(&s.commit));
        }
        let now = self.now();
        let started_at = self.ledger.lock().unwrap().get(job, gen).ok().flatten().and_then(|r| r.started_at);
        let pin = self.pin.lock().unwrap().clone();
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
        };
        let signed = SignedReceipt::sign(&self.key, &receipt);
        {
            // One lock: the shipper never sees a terminal row without its receipt.
            let l = self.ledger.lock().unwrap();
            if let Some(s) = &src {
                let _ = l.set_src(job, gen, s);
            }
            if let Err(e) = l.finish(job, gen, state, reason, detail, exit.as_ref(), log_seq, now) {
                crate::nlog!("ledger finish {job}: {e}");
            }
            match &signed {
                Ok(r) => {
                    let _ = l.set_receipt(job, gen, r);
                }
                Err(e) => crate::nlog!("receipt for {job}: {e}"),
            }
        }
        self.jobs.lock().unwrap().remove(job);
        if self.jobs.lock().unwrap().is_empty() {
            self.smoke.lock().unwrap().release();
        }
        self.jobs_changed();
    }

    // ── housekeeping ────────────────────────────────────────────────

    /// Delete job trees 24 h after they ended, ledger rows after 14 days.
    pub async fn janitor_pass(&self) {
        let now = self.now();
        let rows = self.ledger.lock().unwrap().all().unwrap_or_default();
        let git = Git::find(&self.layout().git_home(), &crate::util::base_path(&self.layout().home));
        for r in rows {
            let ended = match r.ended_at {
                Some(t) if r.state.is_terminal() && !r.cleaned && now - t > 24 * 3600 => t,
                _ => continue,
            };
            let _ = ended;
            let dir = self.layout().job_dir(&r.job_id, r.generation);
            if let (Some(g), Some(src)) = (&git, &r.plan.src) {
                g.worktree_remove(&self.layout().mirror(&src.project_key), &dir.join("src")).await;
            }
            let _ = std::fs::remove_dir_all(&dir);
            let _ = self.ledger.lock().unwrap().mark_cleaned(&r.job_id, r.generation);
        }
        let _ = self.ledger.lock().unwrap().prune(now - 14 * 24 * 3600);
    }

    /// Background loops: config poll, power, tools, janitor.
    pub fn spawn_background(self: &Arc<Self>) {
        let node = self.clone();
        tokio::spawn(async move {
            let mut n = 0u64;
            loop {
                tokio::time::sleep(node.opts.config_poll).await;
                n += 1;
                let power = n % 5 == 0;
                node.refresh_local(power);
                if n % 300 == 0 {
                    node.probe_tools().await;
                }
                if n % 1800 == 0 {
                    node.janitor_pass().await;
                }
                if n % 5 == 0 {
                    node.write_status();
                }
            }
        });
    }

    pub fn routes(&self) -> Vec<String> {
        let mut v = self.learned_routes.lock().unwrap().clone();
        if let Some(p) = self.pin.lock().unwrap().as_ref() {
            for r in &p.routes {
                if !v.contains(r) {
                    v.push(r.clone());
                }
            }
        }
        v
    }

    pub fn reload_pin(&self) {
        match identity::read_pin(&self.layout().pin()) {
            Ok(p) => *self.pin.lock().unwrap() = p,
            Err(e) => crate::nlog!("{e}"),
        }
    }

    pub fn mark_confirmed(&self) {
        let mut g = self.pin.lock().unwrap();
        if let Some(p) = g.as_mut() {
            if !p.confirmed {
                p.confirmed = true;
                let _ = identity::write_pin(&self.layout().pin(), p);
            }
        }
    }
}

/// Paths of a finished attempt's journal (shipper).
pub fn journal_path(layout: &Layout, job_id: &str, generation: u32) -> PathBuf {
    layout.job_dir(job_id, generation).join("log")
}
