//! The node's sim state (k2-node's own process): which slots may run
//! iPhones, what simhost last reported, the lease table, and the streams
//! being dialed. The rules live in [`k2_node_proto::sim::lease`]; the
//! glue into the node's offers, memory and session is `node/sim_glue.rs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use k2_node_proto::share::Slot;
use k2_node_proto::sim::lease::Leases;
use k2_node_proto::sim::{SimFacts, SimNeed, HELPER_PROTOCOL, RESERVE_MEM_BYTES};

use super::local::HostFacts;
use super::switch::SimSwitch;
use super::SimRoot;

/// Where and how often (tests shorten and redirect).
#[derive(Debug, Clone)]
pub struct SimOptions {
    /// Sim sessions are offered only on macOS (tests force it).
    pub mac: bool,
    pub root: SimRoot,
    pub socket: PathBuf,
    /// Lease ticks, the memory watch and the switch re-read (IR-8: 5 s).
    pub tick: Duration,
    /// How often simhost's facts are asked again (§3.3: 10 minutes).
    pub probe_every: Duration,
}

impl SimOptions {
    pub fn system() -> Self {
        let root = SimRoot::system();
        Self { mac: cfg!(target_os = "macos"), socket: root.socket(), root, tick: Duration::from_secs(5), probe_every: Duration::from_secs(600) }
    }
}

/// The owner step when simhost doesn't answer.
pub fn unreachable_need(why: &str) -> SimNeed {
    SimNeed {
        what: format!("set up the Simulator service on this Mac ({why})"),
        command: "sudo k2-node setup --sim".into(),
    }
}

pub struct NodeSim {
    pub opts: SimOptions,
    pub leases: Mutex<Leases>,
    switch: Mutex<SimSwitch>,
    /// simhost's last facts, or why it didn't answer.
    facts: Mutex<Option<Result<HostFacts, String>>>,
    /// Each session's stream task, by session id.
    pub streams: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

impl NodeSim {
    pub fn new(opts: SimOptions) -> Self {
        Self { opts, leases: Mutex::new(Leases::new()), switch: Mutex::new(SimSwitch::default()), facts: Mutex::new(None), streams: Mutex::new(HashMap::new()) }
    }

    /// Returns true when it changed.
    pub fn set_switch(&self, s: SimSwitch) -> bool {
        let mut cur = self.switch.lock().unwrap();
        let changed = *cur != s;
        *cur = s;
        changed
    }

    pub fn any_on(&self) -> bool {
        self.opts.mac && !self.switch.lock().unwrap().slots.is_empty()
    }

    /// This slot may run iPhones here (a Mac, and the owner turned it on).
    pub fn on(&self, slot: Slot) -> bool {
        self.opts.mac && self.switch.lock().unwrap().on(slot)
    }

    /// Returns true when the facts changed.
    pub fn set_facts(&self, f: Result<HostFacts, String>) -> bool {
        let mut cur = self.facts.lock().unwrap();
        let changed = cur.as_ref() != Some(&f);
        *cur = Some(f);
        changed
    }

    pub fn has_facts(&self) -> bool {
        self.facts.lock().unwrap().is_some()
    }

    /// simhost answered and nothing is missing.
    pub fn ready(&self) -> bool {
        matches!(&*self.facts.lock().unwrap(), Some(Ok(f)) if f.ready())
    }

    /// `Offer.sim` for `slot` (the caller checked [`Self::on`] and the
    /// server's protocol). `free_bytes`: free memory under the node's cap.
    pub fn facts_for(&self, slot: Slot, free_bytes: u64, remote_update: bool) -> SimFacts {
        let leases = self.leases.lock().unwrap();
        let (host, needs) = match &*self.facts.lock().unwrap() {
            Some(Ok(f)) => (f.clone(), f.needs.clone()),
            Some(Err(e)) => (HostFacts::default(), vec![unreachable_need(e)]),
            None => (HostFacts::default(), vec![unreachable_need("not asked yet")]),
        };
        let ready = needs.is_empty() && host.ready();
        SimFacts {
            ready,
            needs,
            xcode: host.xcode.clone(),
            xcode_path: host.xcode_path.clone(),
            helper_protocol: HELPER_PROTOCOL,
            runtimes: host.runtimes.clone(),
            sessions: leases.sessions_for(slot),
            sessions_total: leases.all().len() as u32,
            bytes: host.bytes,
            reserve_bytes: RESERVE_MEM_BYTES,
            free_bytes,
            paths: self.opts.root.slot_paths(slot, &host.devicetype_roots),
            remote_update,
        }
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.leases.lock().unwrap().reserved_bytes()
    }

    pub fn reserved_millicpu(&self) -> u64 {
        self.leases.lock().unwrap().reserved_millicpu()
    }

    /// A job with no `--mem` gets the cap minus the sessions' reservation (IR-8).
    pub fn default_job_mem(&self, node_cap: u64) -> u64 {
        self.leases.lock().unwrap().default_job_mem(node_cap)
    }

    pub fn busy(&self) -> bool {
        self.leases.lock().unwrap().busy()
    }

    /// `sim:<session>` and its hard end, for the status file: `k2-node`
    /// updates and drains wait for these like running jobs (IR-7).
    pub fn running(&self) -> Vec<(String, u64)> {
        self.leases.lock().unwrap().all().iter().map(|l| (format!("sim:{}", l.session), l.ends_at)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_node_proto::sim::{SimDeviceType, SimOpen, SimRuntime};

    fn ready_facts() -> HostFacts {
        HostFacts {
            xcode: Some("26.6 (17F113)".into()),
            runtimes: vec![SimRuntime {
                identifier: "com.apple.CoreSimulator.SimRuntime.iOS-26-5".into(),
                name: "iOS 26.5".into(),
                version: "26.5".into(),
                device_types: vec![SimDeviceType { identifier: "t".into(), name: "iPhone 17".into(), width_pt: 402, height_pt: 874, scale_milli: 3000, default: true }],
            }],
            helper_built: true,
            ..Default::default()
        }
    }

    fn sim(mac: bool) -> NodeSim {
        NodeSim::new(SimOptions { mac, ..SimOptions::system() })
    }

    #[test]
    fn only_a_mac_with_the_slot_turned_on_runs_iphones() {
        let s = sim(true);
        assert!(!s.on(0) && !s.any_on());
        assert!(s.set_switch(SimSwitch { slots: vec![0] }));
        assert!(s.on(0) && !s.on(1) && s.any_on());
        assert!(!s.set_switch(SimSwitch { slots: vec![0] }), "no change");
        let linux = sim(false);
        linux.set_switch(SimSwitch { slots: vec![0] });
        assert!(!linux.on(0), "never on a Linux node");
    }

    #[test]
    fn facts_are_ready_only_when_simhost_answered_with_nothing_missing() {
        let s = sim(true);
        let f = s.facts_for(0, 8 << 30, false);
        assert!(!f.ready && f.needs[0].command == "sudo k2-node setup --sim", "{f:?}");
        s.set_facts(Err("connection refused".into()));
        let f = s.facts_for(0, 8 << 30, false);
        assert!(!f.ready && f.needs[0].what.contains("connection refused"));
        s.set_facts(Ok(HostFacts { helper_built: false, ..ready_facts() }));
        assert!(!s.ready() && !s.facts_for(0, 0, false).ready);
        s.set_facts(Ok(ready_facts()));
        let f = s.facts_for(2, 8 << 30, true);
        assert!(s.ready() && f.ready && f.needs.is_empty());
        assert_eq!((f.helper_protocol, f.reserve_bytes, f.free_bytes, f.remote_update), (HELPER_PROTOCOL, RESERVE_MEM_BYTES, 8 << 30, true));
        assert_eq!(f.paths.set, "/var/k2sim/sets/2", "each server its own set");
        assert!(f.helper_protocol >= 3, "the server takes protocol 3 frames from a node");
    }

    #[test]
    fn a_session_reserves_memory_and_shows_as_running() {
        let s = sim(true);
        let cap = 8u64 << 30;
        assert_eq!(s.default_job_mem(cap), cap);
        let adm = k2_node_proto::sim::lease::Admission { enabled: true, free_mem: cap, disk_ok: true, ..Default::default() };
        let o = SimOpen { session: "a".repeat(32), workspace_key: "0123456789abcdef01234567".into(), mem_bytes: RESERVE_MEM_BYTES, lease_secs: 3600 };
        s.leases.lock().unwrap().open(0, &o, &adm, 100);
        assert_eq!(s.reserved_bytes(), RESERVE_MEM_BYTES);
        assert_eq!(s.default_job_mem(cap), 5 << 30);
        assert!(s.busy());
        assert_eq!(s.running(), vec![(format!("sim:{}", "a".repeat(32)), 3700)]);
        assert_eq!(s.facts_for(0, 0, false).sessions, vec!["a".repeat(32)]);
        assert!(s.facts_for(1, 0, false).sessions.is_empty(), "another server sees only the count");
        assert_eq!(s.facts_for(1, 0, false).sessions_total, 1);
    }
}
