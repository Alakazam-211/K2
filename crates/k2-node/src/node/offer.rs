//! The offer one session's server sees (§10.1 of the nodes PRD; per
//! session since node sharing, prd-compute-shared-queue-v1 V53).
//!
//! Solo mode: the owner's offer, exactly today's. Lane S1 adds the share
//! view, per-session features and the owner-only labels here.

use std::sync::atomic::Ordering;

use k2_node_proto::frames::{Availability, Caps, Free, Offer, PROTOCOL};
use k2_node_proto::refusal::{self, NodeView};

use super::{Node, Tenant, NODE_VERSION};

impl Node {
    /// `t`'s offer right now (its revision bumps only when content changed).
    pub fn offer(&self, t: &Tenant) -> Offer {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(t, &l);
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
            share: None,
            revision: 0,
            control,
            control_by: l.control.by.clone(),
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
            features: self.features(),
        };
        let mut last = t.last_offer.lock().unwrap();
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
            o.revision = t.offer_rev.fetch_add(1, Ordering::SeqCst) + 1;
            *last = Some(o.clone());
        }
        o
    }
}
