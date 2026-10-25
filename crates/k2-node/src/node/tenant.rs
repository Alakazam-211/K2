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
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::frames::{Frame, JobState, NodeLimits, Offer, Transfer, TransferFailed, TransferKind, PROTOCOL_SHARED};
use k2_node_proto::share::{codes, Network, Role, ServerEntry, ServerStatus, ShareState, Slot, OWNER_SLOT};
use serde::Serialize;
use tokio::sync::{mpsc, watch, Notify};

use super::{Node, NodeOptions, StopReason};
use crate::identity::{self, Pin, TenantFlags};
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
    /// Jobs waiting for their secret env (secret-env SE18), by (job,
    /// generation): the answer goes to exactly one waiter, once.
    pub(super) secrets: Mutex<HashMap<(String, u32), tokio::sync::oneshot::Sender<SecretAnswer>>>,
}

/// The controller's answer to `secret_need`.
pub enum SecretAnswer {
    Env(k2_node_proto::frames::SecretEnv),
    Refused(k2_node_proto::frames::SecretRefused),
}

impl Hub {
    fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            next_epoch: AtomicU64::new(1),
            connected: watch::channel(0).0,
            transfers: Mutex::new(HashMap::new()),
            secrets: Mutex::new(HashMap::new()),
        }
    }

    /// Route a `secret_env` / `secret_refused` to the job waiting for it
    /// (dropped when nobody waits: a stale or unasked answer).
    pub fn route_secret(&self, job_id: &str, generation: u32, a: SecretAnswer) {
        if let Some(tx) = self.secrets.lock().unwrap().remove(&(job_id.to_string(), generation)) {
            let _ = tx.send(a);
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

/// What `servers.toml` says about one tenant right now, set by the servers
/// loop ([`crate::session::run_forever`]). `None` in solo mode (no file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInfo {
    /// This server's line (the node owner's decision).
    pub entry: ServerEntry,
    /// Servers counting toward shared mode (active or paused).
    pub servers: u32,
    /// Two or more of them.
    pub shared: bool,
    /// The owner server's label (for "shared by …").
    pub owner_label: String,
}

/// One server's connection, for `k2-node status` and `servers list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Link {
    /// unenrolled | enrolling | pending | connecting | online | offline |
    /// refused | revoked | stopped
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    pub since: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// This node's clock minus the server's at the last attach
    /// (`Welcome.server_time`), seconds; positive = the node is ahead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skew_secs: Option<i64>,
}

/// One server on this node.
pub struct Tenant {
    pub slot: Slot,
    pub paths: TenantLayout,
    /// The server pin (`controller.json` for the owner).
    pub pin: Mutex<Option<Pin>>,
    /// The node's runtime flags for a non-owner server (beside its pin,
    /// V45). Always default for the owner.
    pub flags: Mutex<TenantFlags>,
    /// What the live session's server said in `Challenge.protocol` (0 = no
    /// session). Sharing fields and features go only to ≥ 3 (§10.1).
    pub protocol: AtomicU32,
    /// This server's line in `servers.toml` (`None` = solo mode).
    pub share: Mutex<Option<ShareInfo>>,
    /// Keep a session with this server (the owner always; another server
    /// while `servers.toml` lets it attach). Turning it off ends the live
    /// session after a last offer.
    pub serve: watch::Sender<bool>,
    /// This server's connection state (non-owners; the owner's is the
    /// status file's own `state`).
    pub link: Mutex<Link>,
    /// The node's per-tenant wall is really on (lane S3's probe, mirrored
    /// here by the servers loop): offers say `tenant-wall` then.
    pub tenant_wall: std::sync::atomic::AtomicBool,
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
        let (pin, flags) = if slot == OWNER_SLOT {
            (identity::read_pin(&paths.pin())?, TenantFlags::default())
        } else {
            match identity::read_tenant_pin(&paths.pin())? {
                Some((p, f)) => (Some(p), f),
                None => (None, TenantFlags::default()),
            }
        };
        Ok(Self {
            slot,
            paths,
            pin: Mutex::new(pin),
            flags: Mutex::new(flags),
            protocol: AtomicU32::new(0),
            share: Mutex::new(None),
            serve: watch::channel(slot == OWNER_SLOT).0,
            link: Mutex::new(Link::default()),
            tenant_wall: std::sync::atomic::AtomicBool::new(false),
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
        if self.is_owner() {
            match identity::read_pin(&self.paths.pin()) {
                Ok(p) => *self.pin.lock().unwrap() = p,
                Err(e) => crate::nlog!("{e}"),
            }
            return;
        }
        match identity::read_tenant_pin(&self.paths.pin()) {
            Ok(Some((p, f))) => {
                *self.pin.lock().unwrap() = Some(p);
                *self.flags.lock().unwrap() = f;
            }
            Ok(None) => {
                *self.pin.lock().unwrap() = None;
                *self.flags.lock().unwrap() = TenantFlags::default();
            }
            Err(e) => crate::nlog!("{e}"),
        }
    }

    /// Write this server's pin (and, for a non-owner, its runtime flags).
    pub fn write_pin(&self, p: &Pin) -> Result<(), String> {
        if self.is_owner() {
            identity::write_pin(&self.paths.pin(), p)
        } else {
            identity::write_tenant_pin(&self.paths.pin(), p, &self.flags.lock().unwrap())
        }
    }

    /// Change a non-owner's runtime flags and write them beside its pin.
    pub fn update_flags(&self, f: impl FnOnce(&mut TenantFlags)) {
        if self.is_owner() {
            return;
        }
        let mut flags = self.flags.lock().unwrap();
        let before = flags.clone();
        f(&mut flags);
        if *flags == before {
            return;
        }
        let pin = self.pin.lock().unwrap().clone();
        if let Some(p) = pin {
            if let Err(e) = identity::write_tenant_pin(&self.paths.pin(), &p, &flags) {
                crate::nlog!("server slot {}: {e}", self.slot);
            }
        }
    }

    /// The session (in-memory) narrowing, for `policy.show`.
    pub fn session_narrow(&self) -> NodeLimits {
        self.narrow.lock().unwrap().clone()
    }

    /// The live session's protocol (0 = none).
    pub fn protocol(&self) -> u32 {
        self.protocol.load(Ordering::SeqCst)
    }

    /// May this session see protocol-3 sharing fields and features?
    pub fn speaks_shared(&self) -> bool {
        self.protocol() >= PROTOCOL_SHARED
    }

    /// May this session get secret-env frames (protocol 4)?
    pub fn speaks_secrets(&self) -> bool {
        self.protocol() >= k2_node_proto::frames::PROTOCOL_SECRETS
    }

    pub fn share(&self) -> Option<ShareInfo> {
        self.share.lock().unwrap().clone()
    }

    /// Does this server's pin belong to its `servers.toml` line? (A reused
    /// slot's old pin never serves a new line; the owner's line carries the
    /// full fingerprint.)
    pub fn pin_matches(&self, entry: &ServerEntry) -> bool {
        let pin = self.pin.lock().unwrap();
        let Some(p) = pin.as_ref() else { return false };
        if self.is_owner() {
            k2_node_proto::crypto::ct_eq(p.controller_fp.as_bytes(), entry.fp_prefix.as_bytes())
        } else {
            pin_serves(entry, p)
        }
    }

    /// This server's status at `now`: the owner's decision, judged by time,
    /// plus what the node knows (a pending line whose SAS was confirmed is
    /// active; the executor's `servers.prune` writes it so; a server this
    /// node refused or that removed the node). `None` in solo mode.
    pub fn status_at(&self, now: i64) -> Option<ServerStatus> {
        let info = self.share()?;
        let e = &info.entry;
        let pin = self.pin.lock().unwrap().clone();
        let matches = self.pin_matches(e);
        let confirmed = matches && pin.as_ref().is_some_and(|p| p.confirmed && !p.revoked);
        let expired = e.expires_at.is_some_and(|x| now >= x);
        let mut s = e.status(now);
        if e.state == ShareState::Pending && confirmed && !expired {
            s = ServerStatus::Active;
        }
        if !self.is_owner() && matches!(s, ServerStatus::Active | ServerStatus::Paused | ServerStatus::Pending) {
            if pin.as_ref().is_some_and(|p| p.revoked) && matches {
                s = ServerStatus::Revoked;
            } else if self.flags.lock().unwrap().refused.is_some() {
                s = ServerStatus::Refused;
            }
        }
        Some(s)
    }

    /// Why this server may not start a job now, if it may not: the share's
    /// state by time (pause, end date, revoke), and for a non-owner a node
    /// that lost its job wall (V59). `None` = go ahead. Solo mode always
    /// goes ahead (today).
    pub fn job_gate(&self, now: i64, walled: bool) -> Option<&'static str> {
        let Some(s) = self.status_at(now) else {
            return (!self.is_owner()).then_some(codes::SERVER_NOT_APPROVED);
        };
        if !self.is_owner() && !walled {
            return Some(codes::TENANT_WALL_MISSING);
        }
        match s {
            ServerStatus::Active => None,
            ServerStatus::Paused => Some(codes::SHARE_PAUSED),
            ServerStatus::Expired => Some(codes::SHARE_EXPIRED),
            ServerStatus::Revoked => Some(codes::SHARE_REVOKED),
            ServerStatus::Pending | ServerStatus::Refused => Some(codes::SERVER_NOT_APPROVED),
        }
    }

    /// This server's jobs' network: `min(policy, share)` (V65). Solo mode:
    /// the policy's.
    pub fn network(&self, policy: Network) -> Network {
        match self.share() {
            Some(i) => policy.min(i.entry.network),
            None => policy,
        }
    }

    /// The node policy's `workspaces` allowlist names the owner server's
    /// workspaces only (V65). For the owner it gates as today (a non-empty
    /// list refuses every other id). Another server's jobs are governed by
    /// its share instead, and a plan of its naming one of the owner's listed
    /// ids is refused (P8): those ids are the owner's, never a tenant's.
    pub fn workspace_refused(&self, allow: &[String], workspace_id: &str) -> bool {
        if self.is_owner() {
            !allow.is_empty() && !allow.iter().any(|w| w == workspace_id)
        } else {
            allow.iter().any(|w| w == workspace_id)
        }
    }

    pub fn set_link(&self, state: &str, route: Option<String>, err: Option<String>, now: i64) {
        let mut l = self.link.lock().unwrap();
        if l.state != state {
            l.since = now;
        }
        l.state = state.to_string();
        if route.is_some() || state != "online" {
            l.route = route;
        }
        if err.is_some() || state == "online" {
            l.last_error = err;
        }
    }
}

/// Does a non-owner pin belong to this `servers.toml` line? Its server's
/// key starts with the line's prefix, and for a line still waiting for its
/// SAS the pin came from this add's enrol (not an earlier, expired one).
pub fn pin_serves(entry: &ServerEntry, p: &Pin) -> bool {
    entry.matches_fp(&p.controller_fp) && (entry.state != ShareState::Pending || p.enrolled_at >= entry.added_at)
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
/// Returns the job units it asked the executor to release (Linux wall):
/// until each is confirmed gone, the node holds the slot as if a secret job
/// of an unknown workspace still ran there (review B-M2, SE26).
pub(super) async fn reconcile(t: &Tenant, opts: &NodeOptions, wall: &Wall) -> Result<Vec<String>, String> {
    // A restart the executor made for an update, rollback or k2-node
    // restart: `node_update` (infra, no node-health strike, V55).
    let why = if crate::admin::update::recent_update_mark(&opts.layout.config, opts.clock.now()) {
        k2_node_proto::share::codes::NODE_UPDATE
    } else {
        "node_restart"
    };
    let (changed, interrupted) = {
        let ledger = t.ledger.lock().unwrap();
        let changed = ledger.reconcile_boot(opts.clock.as_ref())?;
        // Same boot, new process: the old process's jobs can't be re-adopted.
        let interrupted = ledger.interrupt_open(why, opts.clock.as_ref())?;
        (changed, interrupted)
    };
    for (j, g) in &changed {
        crate::nlog!("job {j} gen {g}: interrupted by a node reboot");
    }
    let mut lingering = Vec::new();
    for (j, g, pgid) in interrupted {
        crate::nlog!("job {j} gen {g}: interrupted by a k2-node restart");
        // Under the Linux wall the job lives in its own unit: stop it. It
        // may outlive this call (a job that ignores SIGTERM): the node
        // holds its slot until `Status` says it's gone.
        if matches!(wall, Wall::JobUser { .. }) {
            if let Some(unit) = crate::admin::jobs::unit_name(&t.job_key(&j, g)) {
                let sock = opts.admin_socket.clone();
                let u = unit.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::admin::server::job_request(&sock, &crate::admin::jobs::JobOp::Release { unit: u }, &[], Duration::from_secs(60))
                })
                .await;
                lingering.push(unit);
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
    Ok(lingering)
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
    pub async fn add_tenant(self: &Arc<Self>, slot: Slot) -> Result<Arc<Tenant>, String> {
        if let Some(t) = self.tenant(slot) {
            return Ok(t);
        }
        let t = Arc::new(Tenant::open(self.layout(), slot)?);
        let lingering = reconcile(&t, &self.opts, &self.wall).await?;
        let t = {
            let mut m = self.tenants.write().unwrap();
            m.entry(slot).or_insert(t).clone()
        };
        self.hold_lingering(slot, lingering);
        Ok(t)
    }

    /// Forget a non-owner tenant whose `servers.toml` line went away (wiped)
    /// or changed hands, so its slot opens fresh for the next server. Never
    /// the owner, never while one of its jobs runs. True when it's gone.
    pub fn drop_tenant(&self, slot: Slot) -> bool {
        if slot == OWNER_SLOT || self.jobs.lock().unwrap().keys().any(|(s, _)| *s == slot) {
            return false;
        }
        self.tenants.write().unwrap().remove(&slot).is_some()
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

    /// `t`'s server accepted an attach: its SAS was confirmed. Only `t`'s
    /// pin changes (V51).
    pub fn mark_confirmed(&self, t: &Tenant) {
        let confirmed = {
            let mut g = t.pin.lock().unwrap();
            match g.as_mut() {
                Some(p) if !p.confirmed => {
                    p.confirmed = true;
                    Some(p.clone())
                }
                _ => None,
            }
        };
        let changed = confirmed.is_some();
        if let Some(p) = confirmed {
            let _ = t.write_pin(&p);
        }
        if !t.is_owner() {
            let now = self.now();
            t.update_flags(|f| {
                f.last_seen = Some(now);
                f.refused = None;
            });
        }
        // `k2-node status` must stop showing the enroll code the moment the
        // controller accepts the node (found in the mini-1 smoke: the status
        // file still said "waiting for confirmation" while online).
        if changed {
            self.write_status();
        }
    }

    /// `revoked` from `t`'s server (or `Refused{revoked}`): end that
    /// server's jobs and mark its pin revoked.
    ///
    /// Only `t` ends (V51): a tenant leaving stops its own jobs and marks its
    /// own pin; the owner's `controller.json`, the status file and every
    /// other server's jobs are untouched.
    pub fn revoke(&self, t: &Tenant, reason: &str) {
        if t.is_owner() {
            crate::nlog!("controller removed this node: {reason}");
        } else {
            crate::nlog!("server slot {} removed this node: {reason}", t.slot);
        }
        self.stop_tenant(t.slot, JobState::Cancelled, "node_removed");
        self.queue_cancel_tenant(t, "node_removed");
        let pin = {
            let mut g = t.pin.lock().unwrap();
            g.as_mut().map(|p| {
                p.revoked = true;
                p.revoked_reason = Some(reason.to_string());
                p.clone()
            })
        };
        if !t.is_owner() {
            t.flags.lock().unwrap().revoked_by_server = true;
        }
        if let Some(p) = pin {
            if let Err(e) = t.write_pin(&p) {
                crate::nlog!("can't record revoke: {e}");
            }
        }
        if t.is_owner() {
            self.set_status_state("revoked", None, Some(format!("Removed by the controller: {reason}")));
        } else {
            t.set_link("revoked", None, Some(format!("removed by the server: {reason}")), self.now());
            self.write_status();
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
weight = 1
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
