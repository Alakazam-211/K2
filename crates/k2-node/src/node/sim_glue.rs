//! iPhone sessions inside the node (prd-ios-on-mac-node-0467-v1 §3.2;
//! review IR-6 to IR-8, IR-15): the attach socket's sim frames, admission
//! against this node's own state, the effects of the lease table, the
//! offer's `sim-ios` and `Offer.sim`, and the background watch (lease
//! ticks, simhost's memory, its facts, the owner's switch).

use std::sync::Arc;
use std::time::Duration;

use k2_node_proto::frames::{Control, Frame};
use k2_node_proto::sim::lease::{Admission, Effect};
use k2_node_proto::sim::{SessionState, SimFacts, PROTOCOL_SIM};
use std::sync::atomic::Ordering;

use crate::sim::local::{self, Hello};

use super::{Node, Tenant};

impl Tenant {
    /// The server speaks sim sessions (protocol 11).
    pub fn speaks_sim(&self) -> bool {
        self.protocol.load(Ordering::SeqCst) >= PROTOCOL_SIM
    }
}

impl Node {
    /// `sim-ios` goes to this session: a Mac, the slot turned on, protocol 11.
    pub fn sim_feature(&self, t: &Tenant) -> bool {
        t.speaks_sim() && self.sim.on(t.slot)
    }

    /// `Offer.sim` for this session (absent unless [`Self::sim_feature`]).
    pub fn sim_offer(&self, t: &Tenant, free_mem: u64) -> Option<SimFacts> {
        if !self.sim_feature(t) {
            return None;
        }
        let remote = crate::admin::config::read(&self.layout().config.join(crate::admin::config::ADMIN_FILE))
            .ok()
            .flatten()
            .is_some_and(|c| c.level != k2_node_proto::admin::Level::Off);
        Some(self.sim.facts_for(t.slot, free_mem, remote))
    }

    /// What a `sim_open` from `t` is judged against: this node's own state.
    pub fn sim_admission(&self, t: &Tenant) -> Admission {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(t, &l);
        let (_, used_mem, _, _) = self.used(cap_cpu, cap_mem, par);
        let mut free_mem = cap_mem.saturating_sub(used_mem);
        if let Some(avail) = crate::sysinfo::mem_available() {
            free_mem = free_mem.min(avail);
        }
        let control = self.effective_control(&l);
        Admission {
            enabled: self.sim.on(t.slot) && self.sim.ready(),
            paused: matches!(control, Control::Paused | Control::Stopped),
            draining: control == Control::Draining,
            foreign_lock: l.locks.holding.clone(),
            free_mem,
            disk_ok: self.disk_free_bytes() > l.policy.disk_floor_bytes,
        }
    }

    /// A sim frame on `t`'s attach socket.
    pub fn on_sim_frame(self: &Arc<Self>, t: &Arc<Tenant>, f: Frame) {
        if !t.speaks_sim() {
            return;
        }
        let now = self.now().max(0) as u64;
        match f {
            Frame::SimOpen(o) => {
                let adm = self.sim_admission(t);
                let e = self.sim.leases.lock().unwrap().open(t.slot, &o, &adm, now);
                let admitted = e.iter().any(|x| x.frame.session == o.session && x.frame.state == SessionState::Opening);
                self.sim_apply(e);
                if admitted {
                    let (n, tt, s, k) = (self.clone(), t.clone(), o.session.clone(), o.workspace_key.clone());
                    let h = tokio::spawn(crate::sim::dial::run(n, tt, s, k));
                    self.sim.streams.lock().unwrap().insert(o.session, h.abort_handle());
                }
            }
            Frame::SimClose(c) => {
                let e = self.sim.leases.lock().unwrap().close(t.slot, &c.session);
                self.sim_apply(e);
            }
            Frame::SimSync(s) => {
                let e = self.sim.leases.lock().unwrap().sync(t.slot, &s.sessions);
                self.sim_apply(e);
            }
            Frame::SimErase(x) => {
                if !k2_node_proto::sim::valid_workspace_key(&x.workspace_key) {
                    return;
                }
                // A running session of that workspace ends first.
                let live: Vec<String> = self.sim.leases.lock().unwrap().all().iter().filter(|l| l.slot == t.slot && l.workspace_key == x.workspace_key).map(|l| l.session.clone()).collect();
                for s in live {
                    let e = self.sim.leases.lock().unwrap().close(t.slot, &s);
                    self.sim_apply(e);
                }
                let (sock, slot) = (self.sim.opts.socket.clone(), t.slot);
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = local::call(&sock, &Hello::Erase { slot, workspace_key: x.workspace_key }, Duration::from_secs(300)) {
                        crate::nlog!("sim erase: {e}");
                    }
                });
            }
            _ => {}
        }
    }

    /// Do what the lease table said: tell the session's server, end the
    /// stream of a finished session, shut its device down.
    pub fn sim_apply(&self, effects: Vec<Effect>) {
        if effects.is_empty() {
            return;
        }
        for e in effects {
            if let Some(t) = self.tenant(e.slot).filter(|t| t.speaks_sim()) {
                t.hub.send(Frame::SimState(e.frame.clone()));
            }
            if e.frame.state.is_final() {
                if let Some(h) = self.sim.streams.lock().unwrap().remove(&e.frame.session) {
                    h.abort();
                }
            }
            if e.shutdown {
                let (sock, slot, key) = (self.sim.opts.socket.clone(), e.slot, e.workspace_key.clone());
                let job = move || {
                    if let Err(err) = local::call(&sock, &Hello::Shutdown { slot, workspace_key: key }, Duration::from_secs(180)) {
                        crate::nlog!("sim shutdown: {err}");
                    }
                };
                match tokio::runtime::Handle::try_current() {
                    Ok(h) => {
                        h.spawn_blocking(job);
                    }
                    Err(_) => job(),
                }
            }
        }
        for t in self.tenants() {
            t.offer_changed.notify_one();
        }
        self.write_status();
    }

    /// The watch: at start a `reset` (IR-7), then every tick the owner's
    /// switch, the lease clock and simhost's memory (IR-8), and simhost's
    /// facts every `probe_every` (sooner while it hasn't answered).
    pub fn spawn_sim(self: &Arc<Self>) {
        if !self.sim.opts.mac {
            return;
        }
        let node = self.clone();
        tokio::spawn(async move {
            let sock = node.sim.opts.socket.clone();
            let config = node.layout().config.clone();
            let call = |h: Hello, t: Duration| {
                let s = sock.clone();
                tokio::task::spawn_blocking(move || local::call(&s, &h, t))
            };
            let switch = crate::sim::switch::read(&config).unwrap_or_default();
            node.sim.set_switch(switch);
            if node.sim.any_on() || node.sim.opts.socket.exists() {
                match call(Hello::Reset, Duration::from_secs(300)).await {
                    Ok(Ok(a)) => crate::nlog!("sim: shut down {} device(s) left from before this start", a.count.unwrap_or(0)),
                    Ok(Err(e)) => crate::nlog!("sim: reset: {e}"),
                    Err(e) => crate::nlog!("sim: reset: {e}"),
                }
            }
            let mut last_probe: Option<std::time::Instant> = None;
            loop {
                let switch = match crate::sim::switch::read(&config) {
                    Ok(s) => s,
                    Err(e) => {
                        crate::nlog!("sim: {e}; iPhones are off until it's fixed");
                        Default::default()
                    }
                };
                let mut changed = node.sim.set_switch(switch);
                if node.sim.any_on() {
                    let due = match last_probe {
                        None => true,
                        Some(at) => at.elapsed() >= node.sim.opts.probe_every || (!node.sim.ready() && at.elapsed() >= Duration::from_secs(60)),
                    };
                    if due {
                        last_probe = Some(std::time::Instant::now());
                        let f = match call(Hello::Facts, Duration::from_secs(600)).await {
                            Ok(Ok(a)) => a.facts.ok_or_else(|| "simhost sent no facts".to_string()),
                            Ok(Err(e)) => Err(e),
                            Err(e) => Err(e.to_string()),
                        };
                        changed |= node.sim.set_facts(f);
                    }
                }
                let now = node.now().max(0) as u64;
                let e = node.sim.leases.lock().unwrap().tick(now);
                node.sim_apply(e);
                if node.sim.busy() {
                    if let Ok(Ok(a)) = call(Hello::Rss, Duration::from_secs(20)).await {
                        let e = node.sim.leases.lock().unwrap().police_memory(a.rss.unwrap_or(0));
                        node.sim_apply(e);
                    }
                }
                if changed {
                    for t in node.tenants() {
                        t.offer_changed.notify_one();
                    }
                }
                tokio::time::sleep(node.sim.opts.tick).await;
            }
        });
    }
}

#[cfg(test)]
#[path = "sim_glue_tests.rs"]
mod tests;
