//! The node core: local state, offers, assigns, and running jobs.
//!
//! One [`Node`] lives for the whole process. Sessions come and go
//! ([`crate::session`]); jobs keep running and journaling while no
//! session exists (no lease kill: these are the owner's own machines and
//! the controller is often a laptop, §11.5).
//!
//! Each connected server is a [`Tenant`] (prd-compute-shared-queue-v1):
//! its pin, ledger, session and paths. Solo mode, today's behaviour, is
//! one tenant: the owner at slot 0. The files of this module are owned by
//! one build lane each (§15.2):
//!
//! - `mod.rs`: the node-wide core (options, wall, local state, caps,
//!   status, keep-awake, housekeeping, the tenant registry);
//! - [`tenant`]: the per-server handle, its hub and paths;
//! - `offer.rs`: the per-session offer;
//! - `assign.rs`: `assign`, `cancel` and their state frames;
//! - `run.rs`: one job's life (toolchains, spawn behind the wall, wait,
//!   artifacts, finish);
//! - `sync_src.rs`: the job's code (mirror, fetch, bundle, dirty) and blob
//!   transfers.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use k2_node_proto::crypto::SigningKey;
use k2_node_proto::frames::{Control, JobState, NodeLimits, PROTOCOL};
use k2_node_proto::refusal;
use k2_node_proto::results;
use k2_node_proto::share::{Slot, OWNER_SLOT};
use tokio::sync::watch;

use crate::cgroup::Cgroups;
use crate::config::{self, AvailabilityMode, ControlState, MachineFacts, OnOwnerReturn, Policy};
use crate::ledger::Clock;
use crate::locks::{self, LockScan, SmokeLock};
use crate::paths::Layout;
use crate::runner::{self, RssSampler};
use crate::slots::Slots;
use crate::status::Status;
use crate::sync::Git;
use crate::wall::Wall;

mod assign;
mod offer;
mod run;
mod sync_src;
pub mod tenant;

pub use run::mem_cap_detail;
pub use tenant::{job_key, kind_name, Hub, Tenant, TransferEvent, TransferKey};

/// The product version (build.rs reads the daemon's), not the crate's.
pub const NODE_VERSION: &str = crate::PRODUCT_VERSION;

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
    /// The root executor's socket (node admin).
    pub admin_socket: PathBuf,
    /// Send health facts (node admin §10). Off in tests.
    pub health: bool,
    /// The job wall to use; `None` = detect at start ([`detect_wall`]).
    pub wall: Option<Wall>,
    /// How often a job unit's state is read (Linux wall).
    pub unit_poll: Duration,
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
            admin_socket: crate::admin::config::default_socket(),
            health: true,
            wall: None,
            unit_poll: Duration::from_millis(300),
        }
    }
}

/// How this node walls its jobs (module `wall`). `--dev` (the node runs as
/// a person, its home under theirs) never walls. macOS: Seatbelt when
/// `sandbox-exec` is there. Linux: the executor's job user when it says
/// it's set up; an older executor or no `k2job` means no wall.
pub async fn detect_wall(dev: bool, admin_socket: &Path) -> Wall {
    if dev {
        return Wall::None { why: "--dev: jobs run as the person running k2-node".into() };
    }
    if cfg!(target_os = "macos") {
        return if crate::wall::seatbelt_available() {
            Wall::Seatbelt
        } else {
            Wall::None { why: format!("{} is missing", crate::wall::SANDBOX_EXEC) }
        };
    }
    let sock = admin_socket.to_path_buf();
    let r = tokio::task::spawn_blocking(move || {
        crate::admin::server::job_request(&sock, &crate::admin::jobs::JobOp::Ready, &[], Duration::from_secs(10))
    })
    .await;
    match r {
        Ok(Ok(crate::admin::jobs::JobAnswer::Ready { job_user })) => Wall::JobUser { user: job_user },
        Ok(Ok(other)) => Wall::None { why: format!("the executor answered {other:?}") },
        Ok(Err(e)) => Wall::None { why: format!("{}: {}", e.code, e.message) },
        Err(e) => Wall::None { why: format!("wall check: {e}") },
    }
}

/// May this `--dev` node act as walled for sharing (V59) and run
/// `k2-node servers` without sudo? Only in a DEBUG build: tests and the
/// S5 harness run debug nodes in temp dirs; a release `k2-node` never
/// takes the exemption, `--dev` or not (and the installers and service
/// units never pass `--dev`; `tests/cli.rs` checks it).
pub fn dev_test_exemption(dev: bool) -> bool {
    dev && cfg!(debug_assertions)
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

pub struct Node {
    pub opts: NodeOptions,
    pub key: SigningKey,
    pub local: Mutex<Local>,
    pub facts: MachineFacts,
    /// Every connected server, by slot. Solo mode: just the owner (slot 0).
    tenants: RwLock<BTreeMap<Slot, Arc<Tenant>>>,
    /// The controller's persisted narrowing (`narrow.toml`, root-owned;
    /// node admin `policy.narrow`). Applied on top of each session's.
    persisted_narrow: Mutex<NodeLimits>,
    /// Running attempts by `(slot, job_id)`: job ids come from servers, so
    /// they're only unique within one tenant (V48).
    jobs: Mutex<HashMap<(Slot, String), RunningJob>>,
    slots: Mutex<Slots>,
    smoke: Mutex<SmokeLock>,
    pub tools: Mutex<BTreeMap<String, String>>,
    cgroups: Option<Cgroups>,
    /// The job wall (fixed for the process; the installer restarts us).
    pub wall: Wall,
    pub status: Mutex<Status>,
    keep_awake: Mutex<Option<tokio::process::Child>>,
    /// One toolchain install at a time (mise and rustup share caches).
    tool_lock: tokio::sync::Mutex<()>,
    /// Nonces of node-answered admin verbs.
    pub admin_nonces: crate::admin::courier::NodeNonces,
    /// One pending-updates refresh at a time (it hits the network).
    pub updates_refreshing: std::sync::atomic::AtomicBool,
    /// CPU samples for the stuck-job watch.
    pub stuck_track: crate::admin::housekeeping::StuckTrack,
    /// Offered self-update builds being received (in memory only).
    pub admin_updates: crate::admin::courier::IncomingUpdates,
    /// The node queue (shared nodes; prd-compute-shared-queue-v1 §7).
    pub fairq: crate::fairq::FairQ,
    /// `k2-node` is shutting down: nothing new starts (queued jobs stay
    /// queued for the next start; new assigns are refused).
    stopping: std::sync::atomic::AtomicBool,
}

/// What this node understands beyond protocol 1 (the offer says it).
pub fn node_features() -> Vec<String> {
    let mut f = vec![
        k2_node_proto::needs::FEATURE_NEEDS.to_string(),
        k2_node_proto::admin::FEATURE_ADMIN.to_string(),
        k2_node_proto::admin::FEATURE_SELF_UPDATE.to_string(),
        k2_node_proto::admin::FEATURE_HEALTH.to_string(),
        results::FEATURE_ARTIFACTS.to_string(),
        k2_node_proto::frames::FEATURE_STALL.to_string(),
    ];
    if crate::packages::installer_present() {
        f.push(k2_node_proto::needs::FEATURE_SYSTEM_PACKAGES.to_string());
    }
    f
}

impl Node {
    /// Open the key, the owner's ledger and pin; mark jobs from another boot.
    pub async fn open(opts: NodeOptions) -> Result<Arc<Self>, String> {
        let layout = opts.layout.clone();
        layout.ensure()?;
        let key = crate::identity::load_or_create_key(&layout.key())?;
        let owner = Tenant::open(&layout, OWNER_SLOT)?;
        let wall = match &opts.wall {
            Some(w) => w.clone(),
            None => detect_wall(opts.dev, &opts.admin_socket).await,
        };
        crate::nlog!("job wall: {}{}", wall.label(), match &wall {
            Wall::None { why } => format!(" ({why})"),
            Wall::JobUser { user } => format!(" (jobs run as {user})"),
            Wall::Seatbelt => String::new(),
        });
        tenant::reconcile(&owner, &opts, &wall).await?;
        let facts = opts.facts.clone().unwrap_or_else(|| machine_facts(&layout));
        let policy = Policy::resolve(&config::PolicyFile::default(), &facts)?;
        let cgroups = if opts.use_cgroups { Cgroups::detect() } else { None };
        let node = Arc::new(Self {
            key,
            local: Mutex::new(Local {
                policy,
                policy_error: None,
                control: ControlState { control: Control::Active, by: None, at: None },
                on_battery: false,
                unavailable: vec![],
                locks: LockScan::default(),
            }),
            facts,
            tenants: RwLock::new(BTreeMap::from([(OWNER_SLOT, Arc::new(owner))])),
            persisted_narrow: Mutex::new(NodeLimits::default()),
            jobs: Mutex::new(HashMap::new()),
            slots: Mutex::new(Slots::default()),
            smoke: Mutex::new(SmokeLock::default()),
            tools: Mutex::new(BTreeMap::new()),
            cgroups,
            wall,
            status: Mutex::new(Status::default()),
            keep_awake: Mutex::new(None),
            tool_lock: tokio::sync::Mutex::new(()),
            admin_nonces: Default::default(),
            admin_updates: Default::default(),
            stuck_track: Default::default(),
            updates_refreshing: Default::default(),
            fairq: crate::fairq::FairQ::open(&layout)?,
            stopping: std::sync::atomic::AtomicBool::new(false),
            opts,
        });
        node.refresh_local(true);
        node.probe_tools().await;
        {
            let mut s = node.status.lock().unwrap();
            s.version = NODE_VERSION.to_string();
            s.protocol = PROTOCOL;
            s.node_fp = node.key.fingerprint();
            s.caps_hard = node.caps_hard();
            s.wall = node.wall.label().to_string();
            s.network_none = node.wall.network_none();
            s.since = crate::util::now();
            s.notes = vec!["idle-based availability is not built yet; idle modes act like always / plugged_in".into()];
        }
        node.write_status();
        Ok(node)
    }

    pub fn layout(&self) -> &Layout {
        &self.opts.layout
    }

    /// Hard caps: a delegated cgroup leaf, or the job unit's `MemoryMax`.
    pub fn caps_hard(&self) -> bool {
        self.cgroups.is_some() || matches!(self.wall, Wall::JobUser { .. })
    }

    /// The offer's features: the build's plus what the wall gives.
    pub fn features(&self) -> Vec<String> {
        let mut f = node_features();
        if self.wall.is_walled() {
            f.push(k2_node_proto::frames::FEATURE_JOB_WALL.to_string());
        }
        if self.wall.network_none() {
            f.push(k2_node_proto::frames::FEATURE_NETWORK_NONE.to_string());
        }
        f
    }

    /// What a Seatbelt profile closes (macOS): the node's own state, the
    /// executor's, the owner's config, people's homes and the keychains.
    pub fn seatbelt_deny(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let l = self.layout();
        let dirs = vec![
            l.state(),
            l.run(),
            l.config.clone(),
            crate::admin::config::default_state_dir(),
            crate::wall::homes_root().to_path_buf(),
            PathBuf::from("/Library/Keychains"),
            PathBuf::from("/private/var/root"),
        ];
        (dirs, vec![self.opts.admin_socket.clone()])
    }

    /// One job-wall op on the executor (Linux), off the async runtime.
    pub async fn job_op(&self, op: crate::admin::jobs::JobOp, fds: Vec<std::os::fd::OwnedFd>, timeout: Duration) -> Result<crate::admin::jobs::JobAnswer, String> {
        let sock = self.opts.admin_socket.clone();
        tokio::task::spawn_blocking(move || {
            use std::os::fd::AsRawFd;
            let raw: Vec<i32> = fds.iter().map(|f| f.as_raw_fd()).collect();
            let r = crate::admin::server::job_request(&sock, &op, &raw, timeout);
            drop(fds);
            r
        })
        .await
        .map_err(|e| format!("executor call: {e}"))?
        .map_err(|e| format!("{}: {}", e.code, e.message))
    }

    /// Linux wall: have the executor give a job-written tree back to the
    /// node user (blocking; admin `gc` and the slot sweep). No-op otherwise.
    pub fn reclaim_blocking(&self, p: &Path) {
        if matches!(self.wall, Wall::JobUser { .. }) && p.exists() {
            let op = crate::admin::jobs::JobOp::Reclaim { path: p.to_path_buf() };
            if let Err(e) = crate::admin::server::job_request(&self.opts.admin_socket, &op, &[], Duration::from_secs(600)) {
                crate::nlog!("reclaim {}: {}: {}", p.display(), e.code, e.message);
            }
        }
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

    /// The toolchain layer for this node's home.
    pub fn toolchains(&self) -> crate::toolchain::Toolchains {
        crate::toolchain::Toolchains::new(&self.layout().home, self.job_env_base())
    }

    pub async fn probe_tools(&self) {
        let mut tools = crate::sysinfo::probe_tools(&self.job_env_base()).await;
        tools.extend(self.toolchains().installed_tools());
        *self.tools.lock().unwrap() = tools;
        self.notify_offers();
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
        let persisted = crate::admin::policy::read_narrow(&layout.config);
        let now = self.now();
        let mut l = self.local.lock().unwrap();
        let before = (l.control.control, l.unavailable.clone(), l.policy_error.clone(), l.locks.clone());
        if let Some(p) = policy {
            l.policy = p;
        }
        l.policy_error = policy_error.clone();
        match persisted {
            Ok(n) => {
                let changed = *self.persisted_narrow.lock().unwrap() != n;
                *self.persisted_narrow.lock().unwrap() = n;
                if changed {
                    self.notify_offers();
                }
            }
            // An unreadable narrow file is fail-closed like the policy.
            Err(e) => l.policy_error = Some(e),
        }
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
            self.notify_offers();
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

    /// The caps `t`'s server sees: the policy, narrowed by its session
    /// and by the persisted narrowing.
    fn caps(&self, t: &Tenant, l: &Local) -> (u64, u64, u32) {
        let session = t.narrow.lock().unwrap().clone();
        let persisted = self.persisted_narrow.lock().unwrap().clone();
        let mut cpu = l.policy.cpu_cap_millis(self.facts.cores);
        let mut mem = l.policy.mem_cap_bytes;
        let mut par = l.policy.max_parallel;
        for n in [session, persisted] {
            if let Some(c) = n.cpu_millis {
                cpu = cpu.min(c);
            }
            if let Some(m) = n.mem_bytes {
                mem = mem.min(m);
            }
            if let Some(p) = n.max_parallel {
                par = par.min(p.max(1));
            }
        }
        (cpu, mem, par)
    }

    /// What every running job (all tenants) holds.
    fn used(&self, cap_cpu: u64, cap_mem: u64, par: u32) -> (u64, u64, u32, bool) {
        let jobs = self.jobs.lock().unwrap();
        let share = |cap: u64| cap / par.max(1) as u64;
        let cpu: u64 = jobs.values().map(|j| j.cpu_millis.unwrap_or_else(|| share(cap_cpu))).sum();
        let mem: u64 = jobs.values().map(|j| j.mem_bytes.unwrap_or_else(|| share(cap_mem))).sum();
        (cpu, mem, jobs.len() as u32, jobs.values().any(|j| j.exclusive))
    }

    /// Jobs that look stuck (no output, no CPU for 30 min): an alert in
    /// health, never a kill (node admin S9).
    pub fn stuck_jobs(&self) -> Vec<k2_node_proto::health::StuckJob> {
        crate::admin::housekeeping::stuck_jobs(self)
    }

    /// Remove idle warm slots (not busy, untouched since `cutoff`), under
    /// the slot lock so no lease races the delete. (dir, bytes freed).
    pub fn gc_slots(&self, cutoff: i64) -> Vec<(PathBuf, u64)> {
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
                    if slots.is_busy(&key, n) {
                        continue;
                    }
                    let m = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                    if m.is_none_or(|t| t.as_secs() as i64 > cutoff) {
                        continue;
                    }
                    let bytes = du(&e.path());
                    self.reclaim_blocking(&e.path());
                    if std::fs::remove_dir_all(e.path()).is_ok() {
                        slots.forget(&project.path(), n);
                        out.push((e.path(), bytes));
                    }
                }
            }
        }
        out
    }

    /// The process is about to stop: from now on nothing new starts. Call
    /// before stopping the running jobs, or the capacity they free starts
    /// queued work during the shutdown.
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Stop every running job (the machine owner's stop, pause-now,
    /// shutdown).
    pub fn stop_all(&self, state: JobState, reason: &str) {
        for j in self.jobs.lock().unwrap().values() {
            if j.stop.borrow().is_none() {
                j.stop.send_replace(Some(StopReason { state, reason: reason.to_string() }));
            }
        }
    }

    /// Every running job id (all tenants), sorted.
    pub fn running_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.jobs.lock().unwrap().keys().map(|(_, id)| id.clone()).collect();
        v.sort();
        v
    }

    fn jobs_changed(&self) {
        self.fairq.kick.notify_one();
        self.notify_offers();
        self.notify_ship();
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

    /// The status file: the node, its owner server and every running job.
    pub fn write_status(&self) {
        let pin = self.owner().pin.lock().unwrap().clone();
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

    // ── housekeeping ────────────────────────────────────────────────

    /// Delete job trees 24 h after they ended, ledger rows after 14 days.
    pub async fn janitor_pass(&self) {
        let now = self.now();
        let git = Git::find(&self.layout().git_home(), &crate::util::base_path(&self.layout().home));
        for t in self.tenants() {
            let rows = t.ledger.lock().unwrap().all().unwrap_or_default();
            for r in rows {
                let ended = match r.ended_at {
                    Some(e) if r.state.is_terminal() && !r.cleaned && now - e > 24 * 3600 => e,
                    _ => continue,
                };
                let _ = ended;
                let dir = t.paths.job_dir(&r.job_id, r.generation);
                if matches!(self.wall, Wall::JobUser { .. }) && dir.exists() {
                    // The job user's files come back to us before git and rm.
                    let _ = self.job_op(crate::admin::jobs::JobOp::Reclaim { path: dir.clone() }, vec![], Duration::from_secs(600)).await;
                }
                if let (Some(g), Some(src)) = (&git, &r.plan.src) {
                    g.worktree_remove(&t.paths.mirror(&src.project_key), &dir.join("src")).await;
                }
                let _ = std::fs::remove_dir_all(&dir);
                let _ = t.ledger.lock().unwrap().mark_cleaned(&r.job_id, r.generation);
            }
            let _ = t.ledger.lock().unwrap().prune(now - 14 * 24 * 3600);
            crate::artifacts::sweep(&t.paths.artifacts(), now);
        }
    }

    /// Background loops: config poll, power, tools, janitor, the queue.
    pub fn spawn_background(self: &Arc<Self>) {
        crate::fairq::spawn(self);
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
}

fn du(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| du(&e.path())).sum()).unwrap_or(0)
}

#[cfg(test)]
mod wall_tests {
    use super::*;

    /// The offer says what the wall gives; an unwalled node says nothing.
    #[tokio::test]
    async fn features_follow_the_wall() {
        for (wall, job_wall, net) in [
            (Wall::None { why: "t".into() }, false, false),
            (Wall::Seatbelt, true, true),
            (Wall::JobUser { user: "k2job".into() }, true, true),
        ] {
            let d = crate::util::temp_dir("feat");
            let mut o = NodeOptions::new(Layout::new(d.join("home"), d.join("cfg")));
            o.dev = true;
            o.health = false;
            o.keep_awake = false;
            o.use_cgroups = false;
            o.wall = Some(wall.clone());
            let n = Node::open(o).await.unwrap();
            let f = n.features();
            assert_eq!(f.iter().any(|x| x == k2_node_proto::frames::FEATURE_JOB_WALL), job_wall, "{wall:?}");
            assert_eq!(f.iter().any(|x| x == k2_node_proto::frames::FEATURE_NETWORK_NONE), net, "{wall:?}");
            assert_eq!(n.caps_hard(), matches!(wall, Wall::JobUser { .. }), "only the unit (or a leaf) is a hard cap");
            let s = crate::status::read(&n.layout().status()).unwrap();
            assert_eq!((s.wall.as_str(), s.network_none), (wall.label(), net));
        }
    }

    #[tokio::test]
    async fn dev_mode_never_walls() {
        let w = detect_wall(true, Path::new("/nonexistent.sock")).await;
        assert!(matches!(w, Wall::None { .. }), "{w:?}");
    }

    /// Solo mode is one tenant, the owner, on today's paths; another
    /// tenant opens beside it with its own ledger and none of its jobs.
    #[tokio::test]
    async fn solo_is_the_owner_alone_and_tenants_open_beside_it() {
        let d = crate::util::temp_dir("tenants");
        let mut o = NodeOptions::new(Layout::new(d.join("home"), d.join("cfg")));
        o.dev = true;
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.wall = Some(Wall::None { why: "t".into() });
        let n = Node::open(o).await.unwrap();
        assert_eq!(n.tenants().iter().map(|t| t.slot).collect::<Vec<_>>(), vec![0]);
        assert!(n.tenant(1).is_none());
        let owner = n.owner();
        assert_eq!(owner.paths.ledger(), n.layout().ledger());
        let t = n.add_tenant(1).await.unwrap();
        assert!(Arc::ptr_eq(&t, &n.add_tenant(1).await.unwrap()), "opening twice returns the same tenant");
        assert_eq!(n.tenants().iter().map(|t| t.slot).collect::<Vec<_>>(), vec![0, 1]);
        assert_ne!(owner.ledger_id(), t.ledger_id());
        assert!(n.running_by_tenant().is_empty());
    }

    fn assign_for(job: &str, node_id: &str, argv: &[&str]) -> k2_node_proto::frames::Assign {
        let plan = k2_node_proto::frames::Plan {
            class: None,
            session_id: None,
            est_secs: None,
            est_p90_secs: None,
            ws_max_running: None,
            job_id: job.into(),
            node_id: node_id.into(),
            workspace_id: "ws-1".into(),
            workspace_label: "k2".into(),
            requested_by: "agent:k2".into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            env: Default::default(),
            cwd: None,
            limits: k2_node_proto::frames::JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 0, log_cap_bytes: 1 << 20 },
            src: None,
            exclusive: false,
            created_at: 1,
            needs: vec![],
            artifacts: vec![],
            stall_secs: None,
        };
        let fence = k2_node_proto::frames::Fence { job_id: job.into(), attempt: 1, generation: 1, plan_digest: plan.digest().unwrap() };
        k2_node_proto::frames::Assign { fence, plan }
    }

    async fn ended(t: &Tenant, job: &str) -> crate::ledger::Row {
        for _ in 0..200 {
            if let Some(r) = t.ledger.lock().unwrap().get(job, 1).unwrap() {
                if r.state.is_terminal() && r.receipt.is_some() {
                    return r;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{job} on slot {} never ended", t.slot);
    }

    /// V48 at the node: job ids are only unique per server. The owner and
    /// a tenant run the same job id and generation at once, each in its
    /// own dirs, journal and ledger; a tenant's cancel and plans reach
    /// only its own jobs (W7's node half).
    #[tokio::test]
    async fn the_same_job_id_from_two_servers_runs_apart() {
        let d = crate::util::temp_dir("tenant-jobs");
        let layout = Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        std::fs::write(layout.policy(), "availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\n").unwrap();
        let pin = |node_id: &str| {
            let k = SigningKey::generate().unwrap();
            crate::identity::Pin {
                controller_fp: k.fingerprint(),
                controller_public_key_pem: k.spki_pem(),
                routes: vec![],
                node_id: node_id.into(),
                name: "mini-1".into(),
                enrolled_at: 1,
                sas: "000 000".into(),
                revoked: false,
                revoked_reason: None,
                labels: Default::default(),
                confirmed: true,
            }
        };
        layout.tenant(1).ensure_state().unwrap();
        crate::identity::write_pin(&layout.tenant(0).pin(), &pin("node-a")).unwrap();
        crate::identity::write_pin(&layout.tenant(1).pin(), &pin("node-b")).unwrap();
        let mut o = NodeOptions::new(layout.clone());
        o.dev = true;
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.cancel_grace = Duration::from_millis(300);
        o.wall = Some(Wall::None { why: "t".into() });
        o.facts = Some(MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") });
        let n = Node::open(o).await.unwrap();
        let owner = n.owner();
        let t1 = n.add_tenant(1).await.unwrap();

        // A plan naming another server's node id is refused (never runs).
        assert_eq!(n.on_assign(&t1, assign_for("x", "node-a", &["/bin/echo", "x"])), None);
        assert!(t1.ledger.lock().unwrap().get("x", 1).unwrap().is_none());

        // Both servers are approved, speak protocol 3 and have a session
        // (a stand-in channel), as S1 sets up; the queue starts both.
        let share = |label: &str| {
            let limits = k2_node_proto::share::ShareLimits { max_running: 2, ..k2_node_proto::share::ShareLimits::new_server(k2_node_proto::share::Trust::Own, false) };
            Some(crate::fairq::ShareInfo { label: label.into(), limits, status: k2_node_proto::share::ServerStatus::Active })
        };
        n.fairq.set_share(0, share("a"));
        n.fairq.set_share(1, share("b"));
        n.fairq.set_protocol(0, Some(3));
        n.fairq.set_protocol(1, Some(3));
        let (tx0, _rx0) = tokio::sync::mpsc::channel(256);
        let (tx1, _rx1) = tokio::sync::mpsc::channel(256);
        owner.hub.attach(tx0);
        t1.hub.attach(tx1);
        let long = ["/bin/sh", "-c", "echo owner; sleep 30"];
        assert_eq!(n.on_assign(&owner, assign_for("j", "node-a", &long)), Some(("j".into(), 1)));
        assert_eq!(n.on_assign(&t1, assign_for("j", "node-b", &["/bin/sh", "-c", "echo tenant; sleep 30"])), Some(("j".into(), 1)));
        n.fairq.tick(&n);
        let running = n.running_by_tenant();
        assert_eq!((running.get(&0), running.get(&1)), (Some(&vec!["j".to_string()]), Some(&vec!["j".to_string()])), "{running:?}");

        // Both have printed (each into its own journal) before the cancel.
        let printed = |t: &Tenant, word: &str| std::fs::read(t.paths.journal("j", 1)).is_ok_and(|b| String::from_utf8_lossy(&b).contains(word));
        for _ in 0..200 {
            if printed(&owner, "owner") && printed(&t1, "tenant") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(printed(&owner, "owner") && printed(&t1, "tenant"), "both jobs started");

        // The tenant cancels "j": its own ends, the owner's keeps running.
        n.cancel(&t1, "j", 1, "cancelled");
        let r = ended(&t1, "j").await;
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Cancelled, Some("cancelled")));
        let o_row = owner.ledger.lock().unwrap().get("j", 1).unwrap().unwrap();
        assert!(!o_row.state.is_terminal(), "the owner's j survived the tenant's cancel: {:?}", o_row.state);
        assert_eq!(n.running_by_tenant().get(&0), Some(&vec!["j".to_string()]));

        // Each wrote its own journal, in its own place.
        let tj = std::fs::read(t1.paths.journal("j", 1)).unwrap();
        assert!(String::from_utf8_lossy(&tj).contains("tenant") && !String::from_utf8_lossy(&tj).contains("owner"));
        assert!(t1.paths.journal("j", 1).starts_with(layout.state().join("tenants/1")));
        assert!(t1.paths.job_dir("j", 1).starts_with(layout.tenants_root().join("1")) && t1.paths.job_dir("j", 1).exists());
        assert!(owner.paths.journal("j", 1) == layout.job_dir("j", 1).join("log") && owner.paths.journal("j", 1).exists());

        n.cancel(&owner, "j", 1, "cancelled");
        assert_eq!(ended(&owner, "j").await.state, JobState::Cancelled);
        assert!(n.running_ids().is_empty());
    }
}
