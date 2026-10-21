//! The node queue at run time (prd-compute-shared-queue-v1 §7, §12): the
//! start loop, usage accounting, positions and ETAs pushed to each server,
//! quotes and queue views. The rules themselves are pure
//! ([`k2_node_proto::queue`]); the queued jobs are rows in each tenant's
//! ledger (state `queued`), so they survive restarts (V47).
//!
//! Inputs other lanes feed (all idempotent, call on every change):
//! - [`FairQ::set_share`]: each server's share and status, from
//!   `servers.toml` and time (S1). Two or more `active` shares = shared mode.
//!   The owner without a line gets its default (policy-sized) share.
//! - [`FairQ::set_protocol`]: the protocol each server's session said in
//!   `Challenge.protocol` (S1). Below 3 = an old server that never queues.
//! - [`FairQ::set_blocked`]: a tenant-wide wait (`tenant_disk_full`,
//!   `tenant_wall_missing`; S3).
//! - [`crate::node::Node::queue_attach`]: right after a server's session
//!   attached, with its `Welcome.resume` (S1, session.rs).
//!
//! Outputs: [`FairQ::quotes`] (S1: `ShareView.quotes`), [`FairQ::view`]
//! (S1: answer `queue_get`), `StateUpdate.queue` pushed here on change.
//!
//! Solo nodes have no queued rows: the loop only accounts usage.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::frames::{Assign, Frame, JobState, StateUpdate};
use k2_node_proto::queue::{self, Caps, Config, Decision, Queued, Rotation, Running, Viewer};
use k2_node_proto::share::{codes, Network, QueuePos, QueueView, Quote, ServerStatus, ShareLimits, Slot, OWNER_SLOT};
use tokio::sync::Notify;

use crate::ledger::Row;
use crate::node::{Node, Tenant};
use crate::paths::Layout;
use crate::queue_store::{day_of, QueueStore};

/// How often the loop runs without a kick.
pub const TICK: Duration = Duration::from_secs(5);
/// How often decayed usage is written down.
pub const PERSIST_SECS: i64 = 60;

/// One server's share as the queue needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInfo {
    pub label: String,
    pub limits: ShareLimits,
    pub status: ServerStatus,
}

/// A running attempt's accounting.
#[derive(Debug, Clone)]
struct RunMeta {
    class: k2_node_proto::share::JobClass,
    ws: String,
    session: Option<String>,
    cmd_hash: String,
    cpu: u64,
    mem: u64,
    exclusive: bool,
    started_at: i64,
    accounted_at: i64,
    /// (p50, p90) run time when it started.
    est: Option<(u64, u64)>,
}

type Key = (Slot, String, u32);

#[derive(Default)]
struct State {
    shares: BTreeMap<Slot, ShareInfo>,
    protocol: BTreeMap<Slot, u32>,
    blocked: BTreeMap<Slot, String>,
    /// Queued attempts that may start in the session with this hub epoch.
    gates: HashMap<Key, u64>,
    runs: HashMap<Key, RunMeta>,
    /// Decayed usage (millicore-seconds) and when it was last decayed.
    usage: BTreeMap<Slot, (u64, i64)>,
    /// The last `QueuePos` sent per queued attempt, with its session epoch.
    sent: HashMap<Key, (u64, QueuePos)>,
    rotation: Rotation,
    last_persist: i64,
    /// Today's CPU per server not yet written down (millicore-seconds),
    /// by `(slot, day)`: flushed with the usage, so a tick does no disk
    /// write while jobs just run.
    day_cpu: BTreeMap<(Slot, String), u64>,
}

pub struct FairQ {
    store: Mutex<QueueStore>,
    pub cfg: Config,
    st: Mutex<State>,
    /// Run the loop now (an assign, a job end, an attach).
    pub kick: Notify,
}

/// The command's key for run-time history (first 16 hex of its argv hash).
pub fn cmd_hash(argv: &[String]) -> String {
    k2_node_proto::frames::argv_sha256(argv)[..16].to_string()
}

/// One snapshot the rules run on.
struct Snapshot {
    now: i64,
    caps: Caps,
    tenants: Vec<queue::Tenant>,
    running: Vec<Running>,
    queued: Vec<Queued>,
    /// Per `queued` index: its tenant and ledger row.
    rows: Vec<(Arc<Tenant>, Row)>,
    rotation: Rotation,
    labels: BTreeMap<Slot, String>,
}

impl Snapshot {
    fn input<'a>(&'a self, cfg: &'a Config) -> queue::Input<'a> {
        queue::Input {
            now: self.now,
            cfg,
            caps: &self.caps,
            tenants: &self.tenants,
            running: &self.running,
            queued: &self.queued,
            rotation: &self.rotation,
        }
    }
}

impl FairQ {
    pub fn open(layout: &Layout) -> Result<Self, String> {
        let store = QueueStore::open(&layout.state().join("queue.sqlite"))?;
        Self::with_store(store)
    }

    fn with_store(store: QueueStore) -> Result<Self, String> {
        let st = State { usage: store.usage()?, rotation: store.rotation()?, ..Default::default() };
        Ok(Self { store: Mutex::new(store), cfg: Config::default(), st: Mutex::new(st), kick: Notify::new() })
    }

    // ── inputs ──────────────────────────────────────────────────────

    pub fn set_share(&self, slot: Slot, share: Option<ShareInfo>) {
        let mut st = self.st.lock().unwrap();
        match share {
            Some(s) => {
                st.shares.insert(slot, s);
            }
            None => {
                st.shares.remove(&slot);
            }
        }
        drop(st);
        self.kick.notify_one();
    }

    pub fn set_protocol(&self, slot: Slot, protocol: Option<u32>) {
        let mut st = self.st.lock().unwrap();
        match protocol {
            Some(p) => {
                st.protocol.insert(slot, p);
            }
            None => {
                st.protocol.remove(&slot);
            }
        }
    }

    /// The protocol `slot`'s server said (0 = not known yet: treated as old).
    pub fn protocol(&self, slot: Slot) -> u32 {
        self.st.lock().unwrap().protocol.get(&slot).copied().unwrap_or(0)
    }

    pub fn set_blocked(&self, slot: Slot, why: Option<String>) {
        let mut st = self.st.lock().unwrap();
        match why {
            Some(w) => {
                st.blocked.insert(slot, w);
            }
            None => {
                st.blocked.remove(&slot);
            }
        }
        drop(st);
        self.kick.notify_one();
    }

    /// Shared mode: two or more servers active on this node.
    pub fn shared(&self, _now: i64) -> bool {
        self.st.lock().unwrap().shares.values().filter(|s| s.status == ServerStatus::Active).count() >= 2
    }

    /// `slot`'s share; the owner without a `servers.toml` line gets one
    /// sized by the node's policy (it adds no cap the owner didn't have).
    pub fn share(&self, slot: Slot, node: &Node) -> Option<ShareInfo> {
        if let Some(s) = self.st.lock().unwrap().shares.get(&slot) {
            return Some(s.clone());
        }
        (slot == OWNER_SLOT).then(|| owner_default(node))
    }

    // ── bookkeeping from the node ───────────────────────────────────

    /// A job entered the queue in the session with hub `epoch`.
    pub(crate) fn queued(&self, slot: Slot, a: &Assign, epoch: u64, now: i64, caps: &Caps) {
        let key = (slot, a.fence.job_id.clone(), a.fence.generation);
        self.st.lock().unwrap().gates.insert(key, epoch);
        let cpu = a.plan.limits.cpu_millis.unwrap_or_else(|| caps.default_cpu());
        let mem = a.plan.limits.mem_bytes.unwrap_or_else(|| caps.default_mem());
        if let Err(e) = self.store.lock().unwrap().add_ask(slot, cpu, mem, now) {
            crate::nlog!("queue store: {e}");
        }
    }

    /// May start in the session with hub `epoch` (it listed the job).
    pub(crate) fn gate(&self, slot: Slot, job_id: &str, generation: u32, epoch: u64) {
        self.st.lock().unwrap().gates.insert((slot, job_id.to_string(), generation), epoch);
    }

    pub(crate) fn forget(&self, slot: Slot, job_id: &str, generation: u32) {
        let key = (slot, job_id.to_string(), generation);
        let mut st = self.st.lock().unwrap();
        st.gates.remove(&key);
        st.sent.remove(&key);
    }

    /// Send `slot`'s positions again (a new session).
    pub(crate) fn resend(&self, slot: Slot) {
        self.st.lock().unwrap().sent.retain(|k, _| k.0 != slot);
    }

    /// An attempt started (any path): account it from now.
    pub(crate) fn started(&self, slot: Slot, a: &Assign, now: i64, caps: &Caps) {
        let hash = cmd_hash(&a.plan.argv);
        let hist = self.store.lock().unwrap().duration(slot, &hash).ok().flatten();
        let est = hist.or_else(|| a.plan.est_secs.map(|p| (p, a.plan.est_p90_secs.unwrap_or(p))));
        let meta = RunMeta {
            class: a.plan.class.unwrap_or_default(),
            ws: a.plan.workspace_id.clone(),
            session: a.plan.session_id.clone(),
            cmd_hash: hash,
            cpu: a.plan.limits.cpu_millis.unwrap_or_else(|| caps.default_cpu()),
            mem: a.plan.limits.mem_bytes.unwrap_or_else(|| caps.default_mem()),
            exclusive: a.plan.exclusive,
            started_at: now,
            accounted_at: now,
            est,
        };
        let key = (slot, a.fence.job_id.clone(), a.fence.generation);
        let mut st = self.st.lock().unwrap();
        st.gates.remove(&key);
        st.sent.remove(&key);
        st.runs.insert(key, meta);
    }

    /// Decayed usage of every server now (millicore-seconds).
    pub fn usage(&self, now: i64) -> BTreeMap<Slot, u64> {
        let st = self.st.lock().unwrap();
        st.usage.iter().map(|(s, (u, at))| (*s, queue::decay(*u, now - at, self.cfg.half_life_secs))).collect()
    }

    // ── the loop ────────────────────────────────────────────────────

    /// Account running jobs up to `now`: decayed usage, daily totals, and
    /// the run time of jobs that ended (ETAs).
    fn account(&self, node: &Node, now: i64) {
        let live: HashSet<Key> = node.running_snaps().into_iter().map(|r| (r.slot, r.job_id, r.generation)).collect();
        let day = day_of(now);
        let store = self.store.lock().unwrap();
        let mut st = self.st.lock().unwrap();
        let half = self.cfg.half_life_secs;
        let mut add = Vec::new();
        let mut ended = Vec::new();
        for (k, m) in st.runs.iter_mut() {
            let dt = (now - m.accounted_at).max(0) as u64;
            if dt > 0 {
                add.push((k.0, m.cpu.saturating_mul(dt)));
                m.accounted_at = now;
            }
            if !live.contains(k) {
                ended.push(k.clone());
            }
        }
        for (slot, used) in add {
            let (u, at) = st.usage.get(&slot).copied().unwrap_or((0, now));
            st.usage.insert(slot, (queue::decay(u, now - at, half).saturating_add(used), now));
            *st.day_cpu.entry((slot, day.clone())).or_default() += used;
        }
        for k in &ended {
            let m = st.runs.remove(k).expect("listed");
            let secs = (now - m.started_at).max(0) as u64;
            if secs > 0 {
                let _ = store.add_duration(k.0, &m.cmd_hash, secs);
            }
            let _ = store.add_day(k.0, &day, 0, secs, 1);
        }
        if !ended.is_empty() || now - st.last_persist >= PERSIST_SECS {
            for (slot, (u, at)) in st.usage.clone() {
                let _ = store.set_usage(slot, u, at);
            }
            // Whole CPU-seconds go down; the remainder waits for the next flush.
            for ((slot, d), ms) in st.day_cpu.iter_mut() {
                if *ms >= 1000 && store.add_day(*slot, d, *ms / 1000, 0, 0).is_ok() {
                    *ms %= 1000;
                }
            }
            st.day_cpu.retain(|(_, d), ms| *ms > 0 && *d == day);
            st.last_persist = now;
        }
    }

    /// The rules' view of the node right now.
    fn snapshot(&self, node: &Node, now: i64) -> Snapshot {
        let caps = node.queue_caps();
        let tenants_all = node.tenants();
        let day = day_of(now);
        let shares: Vec<(Arc<Tenant>, ShareInfo)> = tenants_all.iter().filter_map(|t| self.share(t.slot, node).map(|s| (t.clone(), s))).collect();
        let store = self.store.lock().unwrap();
        let st = self.st.lock().unwrap();
        let tenants: Vec<queue::Tenant> = shares
            .iter()
            .filter(|(_, s)| matches!(s.status, ServerStatus::Active | ServerStatus::Paused))
            .map(|(t, s)| {
                let (u, at) = st.usage.get(&t.slot).copied().unwrap_or((0, now));
                let blocked = if s.status == ServerStatus::Paused { Some(codes::SHARE_PAUSED.to_string()) } else { st.blocked.get(&t.slot).cloned() };
                queue::Tenant {
                    slot: t.slot,
                    limits: s.limits.clone(),
                    usage: queue::decay(u, now - at, self.cfg.half_life_secs),
                    cpu_secs_today: store.day_cpu_secs(t.slot, &day).unwrap_or(0) + st.day_cpu.get(&(t.slot, day.clone())).copied().unwrap_or(0) / 1000,
                    blocked,
                }
            })
            .collect();
        let running: Vec<Running> = st
            .runs
            .iter()
            .map(|(k, m)| {
                let elapsed = (now - m.started_at).max(0) as u64;
                let left = |d: u64| d.saturating_sub(elapsed).max(30);
                Running {
                    slot: k.0,
                    job_id: k.1.clone(),
                    generation: k.2,
                    workspace_id: m.ws.clone(),
                    session_id: m.session.clone(),
                    class: m.class,
                    cpu_millis: m.cpu,
                    mem_bytes: m.mem,
                    exclusive: m.exclusive,
                    left_p50: m.est.map(|e| left(e.0)),
                    left_p90: m.est.map(|e| left(e.1)),
                }
            })
            .collect();
        let mut queued = Vec::new();
        let mut rows = Vec::new();
        for t in &tenants_all {
            let epoch = *t.hub.connected.borrow();
            for r in t.ledger.lock().unwrap().queued().unwrap_or_default() {
                let hash = cmd_hash(&r.plan.argv);
                let hist = store.duration(t.slot, &hash).ok().flatten();
                let gate = st.gates.get(&(t.slot, r.job_id.clone(), r.generation)).copied();
                queued.push(Queued {
                    slot: t.slot,
                    job_id: r.job_id.clone(),
                    generation: r.generation,
                    workspace_id: r.plan.workspace_id.clone(),
                    session_id: r.plan.session_id.clone(),
                    class: r.plan.class.unwrap_or_default(),
                    cpu_millis: r.plan.limits.cpu_millis,
                    mem_bytes: r.plan.limits.mem_bytes,
                    exclusive: r.plan.exclusive,
                    enqueued_at: r.enqueued_at.unwrap_or(r.created_at),
                    est_p50: hist.map(|h| h.0).or(r.plan.est_secs),
                    est_p90: hist.map(|h| h.1).or(r.plan.est_p90_secs).or(r.plan.est_secs),
                    ws_max_running: r.plan.ws_max_running,
                    eligible: epoch != 0 && gate == Some(epoch),
                });
                rows.push((t.clone(), r));
            }
        }
        let labels = shares.iter().map(|(t, s)| (t.slot, s.label.clone())).collect();
        Snapshot { now, caps, tenants, running, queued, rows, rotation: st.rotation.clone(), labels }
    }

    /// One pass: account, end what time ended, start what the rules say,
    /// push positions that moved.
    pub fn tick(&self, node: &Arc<Node>) {
        let now = node.now();
        self.account(node, now);
        // Shares that ended by time or were revoked: their queued jobs end
        // (running ones finish).
        for t in node.tenants() {
            match self.share(t.slot, node).map(|s| s.status) {
                Some(ServerStatus::Expired) => {
                    node.queue_cancel_tenant(&t, codes::SHARE_EXPIRED);
                }
                Some(ServerStatus::Revoked) => {
                    node.queue_cancel_tenant(&t, codes::SHARE_REVOKED);
                }
                _ => {}
            }
        }
        let snap = self.snapshot(node, now);
        if snap.queued.is_empty() {
            return;
        }
        let d: Decision = queue::decide(&snap.input(&self.cfg));
        let mut started = HashSet::new();
        for i in &d.start {
            let (t, r) = &snap.rows[*i];
            if node.start_queued(t, r) {
                started.insert(*i);
            }
        }
        {
            let mut st = self.st.lock().unwrap();
            st.rotation = d.rotation.clone();
            if let Err(e) = self.store.lock().unwrap().set_rotation(&d.rotation) {
                crate::nlog!("queue store: {e}");
            }
        }
        // Positions and ETAs for the jobs still waiting, sent on change.
        for (i, (t, r)) in snap.rows.iter().enumerate() {
            if started.contains(&i) || !snap.queued[i].eligible {
                continue;
            }
            let info = &d.info[i];
            let pos = QueuePos { pos: info.pos, eta_secs: info.eta_secs, eta_p90_secs: info.eta_p90_secs, blocked_by: info.blocked_by.clone() };
            let epoch = *t.hub.connected.borrow();
            let key = (t.slot, r.job_id.clone(), r.generation);
            let mut st = self.st.lock().unwrap();
            if st.sent.get(&key) == Some(&(epoch, pos.clone())) {
                continue;
            }
            let f = Frame::State(StateUpdate {
                queue: Some(pos.clone()),
                job_id: r.job_id.clone(),
                generation: r.generation,
                state: JobState::Queued,
                reason: Some(codes::NODE_QUEUED.into()),
                detail: None,
                at: now,
                exit: None,
                src: None,
                log_seq: 0,
                outputs: None,
            });
            if t.hub.send(f) {
                st.sent.insert(key, (epoch, pos));
            }
        }
    }

    // ── outputs ─────────────────────────────────────────────────────

    /// `t`'s quotes: start-now or a rounded ETA for its last four distinct
    /// job sizes on this node (V53). Rides the 30 s offer.
    pub fn quotes(&self, node: &Node, t: &Tenant) -> Vec<Quote> {
        let now = node.now();
        let asks = self.store.lock().unwrap().asks(t.slot).unwrap_or_default();
        if asks.is_empty() {
            return Vec::new();
        }
        let snap = self.snapshot(node, now);
        let input = snap.input(&self.cfg);
        asks.into_iter().map(|(cpu, mem)| queue::quote(&input, t.slot, cpu, mem)).collect()
    }

    /// The queue as `t`'s server may see it (`queue_get`).
    pub fn view(&self, node: &Node, t: &Tenant, request_id: &str) -> QueueView {
        let now = node.now();
        let snap = self.snapshot(node, now);
        let input = snap.input(&self.cfg);
        let d = queue::decide(&input);
        queue::view(&input, &d, &Viewer { slot: t.slot, owner: t.is_owner(), labels: &snap.labels }, request_id)
    }
}

/// The owner's share when `servers.toml` has no line for it: sized by the
/// node's own policy, weight 2, gate allowed (§11.1).
fn owner_default(node: &Node) -> ShareInfo {
    let caps = node.queue_caps();
    let gb = (caps.mem_bytes >> 30).max(1) as u32;
    let cpus = caps.cpu_millis.div_ceil(1000).max(1) as u32;
    let network = Network::of_policy(&node.local.lock().unwrap().policy.network);
    ShareInfo {
        label: node.owner().pin.lock().unwrap().as_ref().map(|p| p.controller_label()).unwrap_or_default(),
        limits: ShareLimits::owner(caps.parallel_max.clamp(1, 64), cpus.min(1024), gb.min(4096), 100, network),
        status: ServerStatus::Active,
    }
}

/// The loop: every [`TICK`], and at once on a kick.
pub fn spawn(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = node.fairq.kick.notified() => {}
                _ = tokio::time::sleep(TICK) => {}
            }
            // SQLite and ledger reads: off the async workers.
            let n = node.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || n.fairq.tick(&n)).await {
                crate::nlog!("queue tick: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_hash_is_the_argv_digest_prefix() {
        let a = vec!["cargo".to_string(), "test".to_string()];
        assert_eq!(cmd_hash(&a).len(), 16);
        assert_eq!(cmd_hash(&a), cmd_hash(&a.clone()));
        assert_ne!(cmd_hash(&a), cmd_hash(&["cargo".to_string()]));
    }

    #[test]
    fn shared_mode_needs_two_active_shares() {
        let q = FairQ::with_store(QueueStore::open_in_memory().unwrap()).unwrap();
        let s = |status| ShareInfo { label: "x".into(), limits: ShareLimits::new_server(k2_node_proto::share::Trust::Own, false), status };
        assert!(!q.shared(0));
        q.set_share(0, Some(s(ServerStatus::Active)));
        assert!(!q.shared(0), "one server is solo");
        q.set_share(1, Some(s(ServerStatus::Paused)));
        assert!(!q.shared(0), "a paused second server doesn't share yet");
        q.set_share(1, Some(s(ServerStatus::Active)));
        assert!(q.shared(0));
        q.set_share(1, None);
        assert!(!q.shared(0));
        assert_eq!(q.protocol(1), 0, "unknown protocol is old");
        q.set_protocol(1, Some(3));
        assert_eq!(q.protocol(1), 3);
    }
}
