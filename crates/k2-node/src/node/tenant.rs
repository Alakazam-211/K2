//! One server connected to this node: a *tenant*
//! (prd-compute-shared-queue-v1 §2, §5).
//!
//! Every per-server singleton the node used to hold lives here: the pin,
//! the ledger, the live session (hub and epoch), its narrowing, learned
//! routes, offer state and paths ([`TenantLayout`]). A node with one
//! server (solo mode, today's behaviour) has exactly one tenant: the
//! owner at slot 0, with every 0.46.2 path and name.
//!
//! Node-wide things stay on [`Node`]: the key, the machine owner's policy
//! and control, the running-jobs map (keyed by `(slot, job_id)`, V48), warm
//! slots, toolchains, the wall and the status file.
//!
//! Lanes after S0 (§15.2): S1 opens tenants per `servers.toml`, runs a
//! session per server and makes revoke/confirm/narrow and owner-only
//! frames per tenant; S2 adds the node queue; S3 the tenant wall.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::frames::{Frame, JobState, NodeLimits, Offer, Transfer, TransferFailed, TransferKind};
use k2_node_proto::share::{Role, Slot, OWNER_SLOT};
use tokio::sync::{mpsc, watch, Notify};

use super::{Node, NodeOptions, StopReason};
use crate::identity::{self, Pin};
use crate::ledger::Ledger;
use crate::paths::{Layout, TenantLayout};
use crate::wall::Wall;

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

/// One tenant's live session outbound queue, if any.
pub struct Hub {
    conn: Mutex<Option<(u64, mpsc::Sender<Frame>)>>,
    next_epoch: AtomicU64,
    /// Current epoch (0 = no session).
    pub connected: watch::Sender<u64>,
    pub(super) transfers: Mutex<HashMap<TransferKey, mpsc::UnboundedSender<TransferEvent>>>,
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

/// One server on this node.
pub struct Tenant {
    pub slot: Slot,
    pub paths: TenantLayout,
    /// The server pin (`controller.json` for the owner).
    pub pin: Mutex<Option<Pin>>,
    /// This server's jobs (the owner's is `state/ledger.sqlite`).
    pub ledger: Mutex<Ledger>,
    /// This server's live session.
    pub hub: Hub,
    /// Wakes this session's log shipper.
    pub ship: Notify,
    /// Wakes this session's offer sender.
    pub offer_changed: Notify,
    /// Learned routes (this server's `Welcome.routes`), tried before the
    /// pinned ones, and only ever for this server.
    pub learned_routes: Mutex<Vec<String>>,
    /// This session's in-memory narrowing (`Welcome.narrow`, `PolicyNarrow`).
    pub(super) narrow: Mutex<NodeLimits>,
    /// Offer revision and dedupe, per session (V53).
    pub(super) offer_rev: AtomicU64,
    pub(super) last_offer: Mutex<Option<Offer>>,
}

impl Tenant {
    /// Open slot `slot`'s pin and ledger (and its node-only dirs). The
    /// caller reconciles the ledger ([`reconcile`]).
    pub fn open(layout: &Layout, slot: Slot) -> Result<Self, String> {
        if !k2_node_proto::share::valid_slot(slot) {
            return Err(format!("tenant slot {slot} is out of range"));
        }
        let paths = layout.tenant(slot);
        paths.ensure_state()?;
        let ledger = Ledger::open(&paths.ledger())?;
        let pin = identity::read_pin(&paths.pin())?;
        Ok(Self {
            slot,
            paths,
            pin: Mutex::new(pin),
            ledger: Mutex::new(ledger),
            hub: Hub::new(),
            ship: Notify::new(),
            offer_changed: Notify::new(),
            learned_routes: Mutex::new(Vec::new()),
            narrow: Mutex::new(NodeLimits::default()),
            offer_rev: AtomicU64::new(0),
            last_offer: Mutex::new(None),
        })
    }

    pub fn role(&self) -> Role {
        Role::of(self.slot)
    }

    pub fn is_owner(&self) -> bool {
        self.slot == OWNER_SLOT
    }

    /// The opaque key of one attempt on this node ([`job_key`]).
    pub fn job_key(&self, job_id: &str, generation: u32) -> String {
        job_key(self.slot, job_id, generation)
    }

    pub fn ledger_id(&self) -> String {
        self.ledger.lock().unwrap().ledger_id.clone()
    }

    /// The node id this server gave us (empty before enrol).
    pub fn node_id(&self) -> String {
        self.pin.lock().unwrap().as_ref().map(|p| p.node_id.clone()).unwrap_or_default()
    }

    /// Dial order: learned routes first, then the pinned ones.
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
        match identity::read_pin(&self.paths.pin()) {
            Ok(p) => *self.pin.lock().unwrap() = p,
            Err(e) => crate::nlog!("{e}"),
        }
    }

    /// The session (in-memory) narrowing, for `policy.show`.
    pub fn session_narrow(&self) -> NodeLimits {
        self.narrow.lock().unwrap().clone()
    }
}

/// One attempt's key on this node: unit names (`k2job-<key>.service`) and
/// cgroup leaves. The owner keeps today's `<job_id>-g<gen>`. A tenant's is
/// `t<slot>-<16 hex of sha256(slot, job_id, gen)>`: opaque, so a unit
/// listing reveals nothing useful, and never equal to another slot's (the
/// owner's always ends in `-g<digits>`, which isn't hex) (V48).
pub fn job_key(slot: Slot, job_id: &str, generation: u32) -> String {
    if slot == OWNER_SLOT {
        format!("{job_id}-g{generation}")
    } else {
        let h = k2_node_proto::crypto::sha256_hex(format!("{slot}\n{job_id}\n{generation}").as_bytes());
        format!("t{slot}-{}", &h[..16])
    }
}

/// After the tenant's ledger opened: mark attempts from another boot
/// `interrupted/node_reboot`, and this boot's open ones (the previous
/// `k2-node` process's, which can't be re-adopted)
/// `interrupted/node_restart`, stopping what's left of them.
pub(super) async fn reconcile(t: &Tenant, opts: &NodeOptions, wall: &Wall) -> Result<(), String> {
    let (changed, interrupted) = {
        let ledger = t.ledger.lock().unwrap();
        let changed = ledger.reconcile_boot(opts.clock.as_ref())?;
        // Same boot, new process: the old process's jobs can't be re-adopted.
        let interrupted = ledger.interrupt_open("node_restart", opts.clock.as_ref())?;
        (changed, interrupted)
    };
    for (j, g) in &changed {
        crate::nlog!("job {j} gen {g}: interrupted by a node reboot");
    }
    for (j, g, pgid) in interrupted {
        crate::nlog!("job {j} gen {g}: interrupted by a k2-node restart");
        // Under the Linux wall the job lives in its own unit: stop it.
        if matches!(wall, Wall::JobUser { .. }) {
            if let Some(unit) = crate::admin::jobs::unit_name(&t.job_key(&j, g)) {
                let sock = opts.admin_socket.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::admin::server::job_request(&sock, &crate::admin::jobs::JobOp::Release { unit }, &[], Duration::from_secs(60))
                })
                .await;
            }
        }
        // The node user can only signal its own processes, so a reused
        // pgid can only be another k2node process; never our own group,
        // and never in --dev (there the user is a human).
        let own = unsafe { (libc::getpid(), libc::getpgrp()) };
        if let Some(p) = pgid.filter(|p| *p != own.0 && *p != own.1 && !opts.dev) {
            crate::runner::signal_group(p, libc::SIGKILL);
        }
    }
    Ok(())
}

impl Node {
    /// The owner server (slot 0). Always present.
    pub fn owner(&self) -> Arc<Tenant> {
        self.tenants.read().unwrap().get(&OWNER_SLOT).cloned().expect("the owner tenant (slot 0) is opened with the node")
    }

    pub fn tenant(&self, slot: Slot) -> Option<Arc<Tenant>> {
        self.tenants.read().unwrap().get(&slot).cloned()
    }

    /// Every open tenant, owner first.
    pub fn tenants(&self) -> Vec<Arc<Tenant>> {
        self.tenants.read().unwrap().values().cloned().collect()
    }

    /// Open tenant `slot` (or return it if open), reconciling its ledger
    /// like the owner's at start. Solo mode never calls it; S1 does for
    /// each server `servers.toml` approves.
    pub async fn add_tenant(&self, slot: Slot) -> Result<Arc<Tenant>, String> {
        if let Some(t) = self.tenant(slot) {
            return Ok(t);
        }
        let t = Arc::new(Tenant::open(self.layout(), slot)?);
        reconcile(&t, &self.opts, &self.wall).await?;
        let mut m = self.tenants.write().unwrap();
        Ok(m.entry(slot).or_insert(t).clone())
    }

    /// Wake every session's offer sender (node-wide state changed).
    pub fn notify_offers(&self) {
        for t in self.tenants() {
            t.offer_changed.notify_one();
        }
    }

    /// Wake every session's log shipper.
    pub fn notify_ship(&self) {
        for t in self.tenants() {
            t.ship.notify_one();
        }
    }

    /// The owner session's narrowing, for `policy.show`.
    pub fn session_narrow(&self) -> NodeLimits {
        self.owner().session_narrow()
    }

    /// Server narrowing: only tightens. Narrows `t`'s view of the node.
    pub fn narrow(&self, t: &Tenant, n: NodeLimits) {
        *t.narrow.lock().unwrap() = n;
        t.offer_changed.notify_one();
    }

    /// Re-read the owner's pin (enrol requests).
    pub fn reload_pin(&self) {
        self.owner().reload_pin();
    }

    pub fn mark_confirmed(&self, t: &Tenant) {
        let changed = {
            let mut g = t.pin.lock().unwrap();
            match g.as_mut() {
                Some(p) if !p.confirmed => {
                    p.confirmed = true;
                    let _ = identity::write_pin(&t.paths.pin(), p);
                    true
                }
                _ => false,
            }
        };
        // `k2-node status` must stop showing the enroll code the moment the
        // controller accepts the node (found in the mini-1 smoke: the status
        // file still said "waiting for confirmation" while online).
        if changed {
            self.write_status();
        }
    }

    /// `revoked` from `t`'s server (or `Refused{revoked}`): end that
    /// server's jobs and mark its pin revoked.
    pub fn revoke(&self, t: &Tenant, reason: &str) {
        crate::nlog!("controller removed this node: {reason}");
        self.stop_tenant(t.slot, JobState::Cancelled, "node_removed");
        let mut g = t.pin.lock().unwrap();
        if let Some(p) = g.as_mut() {
            p.revoked = true;
            p.revoked_reason = Some(reason.to_string());
            if let Err(e) = identity::write_pin(&t.paths.pin(), p) {
                crate::nlog!("can't record revoke: {e}");
            }
        }
        drop(g);
        if t.is_owner() {
            self.set_status_state("revoked", None, Some(format!("Removed by the controller: {reason}")));
        }
    }

    /// Stop every running job of one tenant.
    pub fn stop_tenant(&self, slot: Slot, state: JobState, reason: &str) {
        for ((s, _), j) in self.jobs.lock().unwrap().iter() {
            if *s == slot && j.stop.borrow().is_none() {
                j.stop.send_replace(Some(StopReason { state, reason: reason.to_string() }));
            }
        }
    }

    /// Running job ids by tenant (sorted).
    pub fn running_by_tenant(&self) -> BTreeMap<Slot, Vec<String>> {
        let mut m: BTreeMap<Slot, Vec<String>> = BTreeMap::new();
        for (s, id) in self.jobs.lock().unwrap().keys() {
            m.entry(*s).or_default().push(id.clone());
        }
        for v in m.values_mut() {
            v.sort();
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_keys_are_per_slot_and_opaque() {
        assert_eq!(job_key(0, "7f3a", 2), "7f3a-g2", "the owner keeps today's unit names");
        let a = job_key(1, "7f3a", 2);
        assert!(a.starts_with("t1-") && a.len() == 3 + 16 && !a.contains("7f3a"), "{a}");
        assert_ne!(a, job_key(2, "7f3a", 2), "same job id, another slot");
        assert_ne!(a, job_key(1, "7f3a", 3), "same job, another generation");
        assert_eq!(a, job_key(1, "7f3a", 2), "stable");
        // A tenant's key can never equal an owner key: the owner's always
        // ends `-g<digits>`, a tenant's in 16 hex.
        let owner_like = job_key(0, &a[..a.len() - 3], 1);
        assert_ne!(owner_like, a);
        for k in [a, job_key(7, "x", 1), job_key(0, "j", 1)] {
            assert!(crate::admin::jobs::unit_name(&k).is_some(), "{k} is a valid unit key");
        }
    }

    /// `servers.toml` as the PRD writes it (§11.1) parses with the proto
    /// types, validates, and round-trips through TOML.
    #[test]
    fn servers_toml_parses_and_round_trips() {
        use k2_node_proto::share::{JobClass, Network, ServerStatus, ServersFile, ShareState, Trust};
        let owner = "a".repeat(64);
        let text = format!(
            r#"version = 1
owner = "{owner}"
max_servers = 8

[[server]]
fp_prefix = "{owner}"
label = "rosson.k2.dev"
state = "active"
trust = "own"
slot = 0
weight = 2
top_class = "gate"
gate = true
exclusive = true
max_running = 2
max_queued = 1000
cpu_hours_day = 24576
max_job_secs = 604800
max_cpus = 8
max_mem_gb = 16
disk_gb = 100
network = "on"
added_at = 1761000000
added_by = "local:rosson"

[[server]]
fp_prefix = "0123456789abcdef"
label = "baden.k2.dev"
state = "active"
trust = "team"
slot = 1
uid = 61001
weight = 1
top_class = "interactive"
gate = false
exclusive = false
max_running = 1
max_queued = 10
cpu_hours_day = 8
max_job_secs = 7200
max_cpus = 4
max_mem_gb = 8
disk_gb = 40
network = "no-lan"
expires_at = 1761600000
pending_until = 0
added_at = 1761000000
added_by = "owner-approval:req-1"
"#
        );
        let f: ServersFile = toml::from_str(&text).unwrap();
        f.validate().unwrap();
        let b = f.entry(1).unwrap();
        assert_eq!((b.trust, b.state, b.network, b.top_class, b.uid), (Trust::Team, ShareState::Active, Network::NoLan, JobClass::Interactive, Some(61001)));
        assert_eq!(b.status(1761599999), ServerStatus::Active);
        assert_eq!(b.status(1761600000), ServerStatus::Expired);
        assert!(f.shared(1761000000));
        let back: ServersFile = toml::from_str(&toml::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
        assert!(toml::from_str::<ServersFile>(&text.replace("slot = 1", "slot = 1\nsurprise = 1")).is_err(), "strict: an unknown key fails closed");
    }

    #[test]
    fn a_tenant_opens_its_own_pin_and_ledger() {
        let d = crate::util::temp_dir("tenant-open");
        let l = Layout::new(d.join("home"), d.join("cfg"));
        l.ensure().unwrap();
        let owner = Tenant::open(&l, 0).unwrap();
        let t = Tenant::open(&l, 2).unwrap();
        assert_eq!((owner.role(), t.role()), (Role::Owner, Role::Tenant));
        assert!(owner.is_owner() && !t.is_owner());
        assert_ne!(owner.ledger_id(), t.ledger_id(), "separate ledgers");
        assert!(l.ledger().exists() && l.tenant(2).ledger().exists());
        assert!(owner.pin.lock().unwrap().is_none() && t.node_id().is_empty());
        assert!(Tenant::open(&l, 8).is_err(), "slots are 0–7");
        assert_eq!(t.job_key("j", 1), job_key(2, "j", 1));
    }
}
