//! The offer one session's server sees (§10.1 of the nodes PRD; per
//! session since node sharing, prd-compute-shared-queue-v1 V53).
//!
//! Solo mode with a 0.46.2 server: exactly today's offer (the solo goldens
//! hold it). Per session:
//! - `slots`: only this server's project keys (its own cache root);
//! - `features`: the build's and the wall's; `servers` to a server that
//!   speaks protocol 3; never `admin`, `self-update` or `health` to a
//!   server that isn't the node's owner (V52);
//! - `labels`: this server's own pin labels (never the owner's) plus the
//!   machine facts;
//! - `control_by` (a person's name) only to the owner;
//! - `share`: this server's [`ShareView`] when it speaks protocol 3 and the
//!   node is set up for sharing (`servers.toml`), with `priority` (does its
//!   Priority count here) when it speaks protocol 5;
//! - `tiers` (protocol 5) in `features` to a server that speaks 5;
//! - revision and dedupe per session; `share.expires_in_secs` and the
//!   quotes don't count as a change (they ride the 30 s offer).

use std::sync::atomic::Ordering;

use k2_node_proto::frames::{Availability, Caps, Free, Offer, PROTOCOL};
use k2_node_proto::refusal::{self, NodeView};
use k2_node_proto::share::{ShareView, FEATURE_QUEUE, FEATURE_SERVERS, FEATURE_SERVERS_OWNER, FEATURE_TENANT_WALL};
use k2_node_proto::tiers::FEATURE_TIERS;

use super::{Node, Tenant, NODE_VERSION};

/// Features only the owner server may see (V52).
const OWNER_ONLY_FEATURES: &[&str] = &[
    k2_node_proto::admin::FEATURE_ADMIN,
    k2_node_proto::admin::FEATURE_SELF_UPDATE,
    k2_node_proto::admin::FEATURE_HEALTH,
    k2_node_proto::admin::FEATURE_WALL_RESTART,
];

impl Node {
    /// `t`'s offer right now (its revision bumps only when content changed).
    pub fn offer(&self, t: &Tenant) -> Offer {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(t, &l);
        let (used_cpu, used_mem, running, excl) = self.used(cap_cpu, cap_mem, par);
        let disk_free = self.disk_free_bytes();
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
            network_none: l.policy.network == "none",
            network_none_enforced: self.wall.network_none(),
        };
        let node_refusal = refusal::node_refusal(&view);
        let pin = t.pin.lock().unwrap().clone();
        let mut labels = pin.as_ref().map(|p| p.labels.clone()).unwrap_or_default();
        labels.insert("os".into(), crate::sysinfo::os_label().into());
        labels.insert("arch".into(), crate::sysinfo::arch_label().into());
        if let Some(p) = &pin {
            labels.insert("name".into(), p.name.clone());
        }
        if crate::sysinfo::is_vm() {
            labels.insert("vm".into(), "true".into());
        }
        let ledger_id = t.ledger_id();
        let mut o = Offer {
            share: self.share_view(t),
            revision: 0,
            control,
            control_by: if t.is_owner() { l.control.by.clone() } else { None },
            control_at: l.control.at,
            availability: Availability { ok: l.unavailable.is_empty(), reasons: l.unavailable.clone() },
            caps: Caps { cpu_millis: cap_cpu, mem_bytes: cap_mem, disk_budget_bytes: l.policy.disk_budget_bytes, hard: self.caps_hard() },
            free: Free { cpu_millis: cap_cpu.saturating_sub(used_cpu), mem_bytes: free_mem, disk_free_bytes: disk_free },
            slots: self.slots.lock().unwrap().list(&t.paths.cache()),
            running,
            parallel_max: par,
            refusal: node_refusal,
            labels,
            tools,
            protocol: PROTOCOL,
            boot_id: self.opts.clock.boot_id(),
            ledger_id,
            node_version: NODE_VERSION.to_string(),
            features: self.session_features(t),
        };
        let mut last = t.last_offer.lock().unwrap();
        let same = last.as_ref().is_some_and(|p| {
            let mut q = p.clone();
            q.revision = 0;
            // Free disk moves constantly; only a 1 GiB change is news.
            q.free.disk_free_bytes = o.free.disk_free_bytes;
            // The end-date countdown and quotes ride the periodic offer.
            if let (Some(qs), Some(os)) = (q.share.as_mut(), o.share.as_ref()) {
                qs.expires_in_secs = os.expires_in_secs;
                qs.skew_secs = os.skew_secs;
                qs.quotes = os.quotes.clone();
            }
            q == o && p.free.disk_free_bytes.abs_diff(o.free.disk_free_bytes) < (1 << 30)
        });
        if same {
            o.revision = last.as_ref().map(|p| p.revision).unwrap_or(0);
            o.free.disk_free_bytes = last.as_ref().map(|p| p.free.disk_free_bytes).unwrap_or(0);
        } else {
            o.revision = t.offer_rev.fetch_add(1, Ordering::SeqCst) + 1;
            *last = Some(o.clone());
        }
        o
    }

    /// What `t`'s session is told this node understands (§10.1, V52).
    pub fn session_features(&self, t: &Tenant) -> Vec<String> {
        let mut f = self.features();
        if !t.is_owner() {
            f.retain(|x| !OWNER_ONLY_FEATURES.contains(&x.as_str()));
        }
        // Protocol 3: this node takes more than one server and keeps the
        // node queue (`queue_get`, the withdraw/if_queued cancels, `Plan`'s
        // queue fields). Without `queue` a server never sends into the
        // node's queue and falls back to today's placement.
        for feat in [FEATURE_SERVERS, FEATURE_QUEUE] {
            if t.speaks_shared() && !f.iter().any(|x| x == feat) {
                f.push(feat.to_string());
            }
        }
        // `servers-owner`: the owner may move the node to another of its
        // own servers here (prd-macos-tenant-compute-v1 §6.4). Owner and
        // protocol 3 only.
        if t.is_owner() && t.speaks_shared() && !f.iter().any(|x| x == FEATURE_SERVERS_OWNER) {
            f.push(FEATURE_SERVERS_OWNER.to_string());
        }
        // `tenant-wall` only when the per-tenant wall is really on (lane
        // S3's probe), and only to protocol 3.
        if t.speaks_shared() && t.tenant_wall.load(Ordering::SeqCst) && !f.iter().any(|x| x == FEATURE_TENANT_WALL) {
            f.push(FEATURE_TENANT_WALL.to_string());
        }
        // Secret-env (SE16): only when every condition holds for this
        // session (protocol 4, Linux wall, trust own, tenant wall for a
        // tenant, an executor that relays stdin). Not owner-only.
        if self.secret_env_refusal(t).is_none() {
            f.push(k2_node_proto::secret::FEATURE_SECRET_ENV.to_string());
        }
        // Job tiers (protocol 5): `Plan.tier`, the tier ledger column, the
        // tier-first queue, `servers.limits priority=`. Only to a server
        // that speaks protocol 5; an older one keeps sending `class`.
        if t.speaks_tiers() && !f.iter().any(|x| x == FEATURE_TIERS) {
            f.push(FEATURE_TIERS.to_string());
        }
        f
    }

    /// `Offer.share` for `t` (V53): only to a protocol-3 server, only on a
    /// node set up for sharing, and only `t`'s own share and counts.
    pub fn share_view(&self, t: &Tenant) -> Option<ShareView> {
        if !t.speaks_shared() {
            return None;
        }
        let info = t.share()?;
        let now = self.now();
        let state = t.status_at(now)?;
        // The owner gets its view whenever `servers.toml` exists (`share()`
        // is set only then), shared or not: a new owner on a node left with
        // no other server must still learn it's the owner (A2, MT34). A
        // solo node without the file sends none, as in 0.46.2.
        let running = self.running_by_tenant().get(&t.slot).map(|v| v.len() as u32).unwrap_or(0);
        // Its own statement: the ledger guard must be gone before `quotes`
        // (the queue's snapshot locks every ledger).
        let queued = t.ledger.lock().unwrap().queued().map(|q| q.len() as u32).unwrap_or(0);
        let quotes = self.fairq.quotes(self, t);
        Some(ShareView {
            role: t.role(),
            slot: t.slot,
            shared: info.shared,
            servers: info.servers,
            owner_label: (!t.is_owner()).then(|| info.owner_label.clone()),
            state,
            limits: info.entry.limits(),
            running,
            queued,
            // Today's CPU isn't exposed by the queue store yet (S2 follow-up).
            cpu_secs_today: 0,
            expires_in_secs: info.entry.expires_at.map(|e| (e - now).max(0)),
            skew_secs: t.link.lock().unwrap().skew_secs,
            // Start-now / ETA for this server's last four sizes (V53).
            quotes,
            // Does this server's Priority count here (`queue.toml`, T7)?
            // Protocol 5 only.
            priority: t.speaks_tiers().then(|| self.fairq.priority(t.slot)),
        })
    }
}

/// SE16/SE17: who is offered `secret-env`, and who is refused at assign.
/// The node runs with the Linux job-user wall set (no executor needed for
/// the offer) and an executor that relays stdin.
#[cfg(all(test, target_os = "linux"))]
mod secret_env_tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use k2_node_proto::frames::{Frame, JobState};
    use k2_node_proto::secret::{codes, FEATURE_SECRET_ENV};
    use k2_node_proto::share::{Network, ServersFile, ShareLimits, Trust};

    use crate::node::tenant::ShareInfo;
    use crate::node::{Node, NodeOptions};
    use crate::wall::Wall;

    fn has(n: &Node, t: &crate::node::Tenant) -> bool {
        n.session_features(t).iter().any(|f| f == FEATURE_SECRET_ENV)
    }

    #[tokio::test]
    async fn only_trusted_walled_protocol_4_sessions_get_secret_env() {
        let d = crate::util::temp_dir("secret-offer");
        let layout = crate::paths::Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        std::fs::create_dir_all(&layout.config).unwrap();
        let mut f = ServersFile::new(&"a".repeat(64), "o", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1, "local:t");
        for (slot, trust) in [(1u8, Trust::Own), (2, Trust::Team)] {
            let mut e = f.servers[0].clone();
            e.slot = slot;
            e.label = format!("s{slot}");
            e.fp_prefix = format!("{:016x}", slot);
            e.trust = trust;
            e.set_limits(&ShareLimits::new_server(trust, false));
            f.servers.push(e);
        }
        let entries = f.servers.clone();
        std::fs::write(layout.config.join(k2_node_proto::share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
        let mut o = NodeOptions::new(layout.clone());
        o.health = false;
        o.keep_awake = false;
        o.use_cgroups = false;
        o.wall = Some(Wall::JobUser { user: "k2job".into() });
        o.exec_stdin = Some(true);
        o.cancel_grace = Duration::from_millis(100);
        o.facts = Some(crate::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: false });
        let n = Node::open(o).await.unwrap();
        let owner = n.owner();
        // The owner: only with a protocol-4 session.
        owner.protocol.store(3, Ordering::SeqCst);
        assert!(!has(&n, &owner), "a protocol-3 server never sees it");
        owner.protocol.store(4, Ordering::SeqCst);
        assert!(has(&n, &owner));
        // An executor that can't relay stdin: no.
        n.exec_stdin.store(false, Ordering::SeqCst);
        assert!(!has(&n, &owner));
        assert_eq!(n.secret_env_refusal(&owner).map(|r| r.0), Some(codes::NEEDS_UPDATE));
        n.exec_stdin.store(true, Ordering::SeqCst);
        // Tenants: `own` with the tenant wall only; `team` never.
        for slot in [1u8, 2] {
            layout.tenant(slot).ensure_state().unwrap();
        }
        let t_own = n.add_tenant(1).await.unwrap();
        let t_team = n.add_tenant(2).await.unwrap();
        for (t, e) in [(&t_own, &entries[1]), (&t_team, &entries[2])] {
            *t.share.lock().unwrap() = Some(ShareInfo { entry: e.clone(), servers: 3, shared: true, owner_label: "o".into() });
            t.protocol.store(4, Ordering::SeqCst);
        }
        assert!(!has(&n, &t_own), "no tenant wall yet");
        assert_eq!(n.secret_env_refusal(&t_own).map(|r| r.0), Some(codes::NEEDS_WALL));
        t_own.tenant_wall.store(true, Ordering::SeqCst);
        t_team.tenant_wall.store(true, Ordering::SeqCst);
        assert!(has(&n, &t_own), "a trust-own server with the tenant wall");
        assert!(!has(&n, &t_team), "a team server never sees it");
        assert_eq!(n.secret_env_refusal(&t_team).map(|r| r.0), Some(codes::UNTRUSTED_SERVER));
        // At assign: a team server's secret plan fails before any row.
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        t_team.hub.attach(tx);
        let mut a = crate::ledger::tests::assign("sj", 1);
        a.plan.node_id = t_team.node_id();
        a.plan.secret_env = vec!["K2T_SECRET".into()];
        a.fence.plan_digest = a.plan.digest().unwrap();
        assert_eq!(n.on_assign(&t_team, a), None);
        let Some(Frame::State(s)) = rx.recv().await else { panic!("no answer") };
        assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some(codes::UNTRUSTED_SERVER)));
        assert!(t_team.ledger.lock().unwrap().get("sj", 1).unwrap().is_none(), "no ledger row");
        // The offer itself (owner) lists the feature beside the others.
        let off = n.offer(&owner);
        assert!(off.features.iter().any(|f| f == FEATURE_SECRET_ENV), "{:?}", off.features);
    }
}
