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
//!   node is set up for sharing (`servers.toml`);
//! - revision and dedupe per session; `share.expires_in_secs` and the
//!   quotes don't count as a change (they ride the 30 s offer).

use std::sync::atomic::Ordering;

use k2_node_proto::frames::{Availability, Caps, Free, Offer, PROTOCOL};
use k2_node_proto::refusal::{self, NodeView};
use k2_node_proto::share::{ShareView, FEATURE_QUEUE, FEATURE_SERVERS, FEATURE_TENANT_WALL};

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
        // `tenant-wall` only when the per-tenant wall is really on (lane
        // S3's probe), and only to protocol 3.
        if t.speaks_shared() && t.tenant_wall.load(Ordering::SeqCst) && !f.iter().any(|x| x == FEATURE_TENANT_WALL) {
            f.push(FEATURE_TENANT_WALL.to_string());
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
        // The owner sees a share view only once someone shares its node.
        if t.is_owner() && !info.shared {
            return None;
        }
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
        })
    }
}
