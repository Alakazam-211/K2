//! iPhone on a Mac compute node: the node's half (prd-ios-on-mac-node-0467-v1
//! §3.2, §3.3, §3.7; review IR-3 to IR-9, IR-15; slice S2).
//!
//! Two processes, two users, one socket between them:
//!
//! - **k2-node** (`_k2node`) holds the node key. It admits or refuses a
//!   server's `sim_open` ([`k2_node_proto::sim::lease`]), dials the session's
//!   encrypted stream to `/cli/compute/sim-stream` ([`dial`]) and relays its
//!   plain messages to the Simulator service. It never runs `simctl` and
//!   never starts a Simulator app: those run outside the job wall, and the
//!   node key must never be within their reach (IN-A16).
//! - **`k2-node simhost`** ([`host`]) runs as the hidden user `_k2sim`
//!   (a LaunchDaemon with `UserName`, `dev.k2.simhost`). It holds no key and
//!   no server credential. It runs only what [`k2_node_proto::sim::policy`]
//!   allows for the session's server slot, argument by argument (IR-5),
//!   builds and runs the Simulator helper, unpacks uploads safely (IR-3),
//!   and keeps frames newest-wins with one in flight (§3.4).
//! - **The socket** ([`local`]): `/var/k2sim/run/simhost.sock`. Root makes
//!   `/var/k2sim` 0710 and `/var/k2sim/run` 0750, both `_k2sim:k2simctl`;
//!   the only member of `k2simctl` is `_k2node`, and simhost also checks the
//!   peer's uid (`getpeereid`, IR-9). Jobs share `_k2node`'s uid, so the job
//!   wall denies `/var/k2sim` and the socket (Seatbelt, §3.3).
//!
//! The service shape (S0's question, IR-10): a LaunchDaemon with
//! `UserName=_k2sim` in the system domain. The node's own smoke on rig1-mac
//! ran a whole Simulator session (create, boot, install, launch, Safari,
//! the helper's frames and taps) as the node's hidden job user, which runs
//! in exactly that shape (dev.k2.node is a LaunchDaemon with `UserName`),
//! with nobody's GUI login involved. `_k2sim` uses the same shape.
//!
//! Per server slot (§3.8): only slots the node owner turned on in
//! `/etc/k2-node/sim.toml` ([`switch`]) see `sim-ios` and `Offer.sim`; a
//! `team` slot, or one whose network is `none` or `no-lan`, can't be turned
//! on. Each slot has its own device set, temp and upload folders.

pub mod cli;
pub mod dial;
pub mod exec;
pub mod helperbin;
pub mod host;
pub mod hostsession;
pub mod local;
pub mod node_side;
pub mod probe;
pub mod switch;

use std::path::{Path, PathBuf};

use k2_node_proto::share::Slot;
use k2_node_proto::sim::policy::SlotPaths;

/// The Simulator user's home: device sets, uploads, temp files, the helper.
pub const SIM_ROOT: &str = "/var/k2sim";
/// The hidden user the Simulator runs as.
pub const SIM_USER: &str = "_k2sim";
/// The group that may reach simhost's socket: `_k2node` only.
pub const SIM_CTL_GROUP: &str = "k2simctl";
/// The node's own user: the only peer simhost answers.
pub const NODE_USER: &str = "_k2node";
/// The service's launchd label.
pub const SIMHOST_LABEL: &str = "dev.k2.simhost";

/// The Simulator user's folders, under one root (tests use a temp dir).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimRoot {
    pub root: PathBuf,
}

impl SimRoot {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn system() -> Self {
        Self::new(SIM_ROOT)
    }

    fn sub(&self, parts: &[&str]) -> PathBuf {
        let mut p = self.root.clone();
        for x in parts {
            p.push(x);
        }
        p
    }

    pub fn run_dir(&self) -> PathBuf {
        self.sub(&["run"])
    }

    pub fn socket(&self) -> PathBuf {
        self.sub(&["run", "simhost.sock"])
    }

    pub fn sets(&self) -> PathBuf {
        self.sub(&["sets"])
    }

    pub fn set(&self, slot: Slot) -> PathBuf {
        self.sets().join(slot.to_string())
    }

    pub fn tmp(&self, slot: Slot) -> PathBuf {
        self.sub(&["tmp", &slot.to_string()])
    }

    pub fn uploads(&self, slot: Slot) -> PathBuf {
        self.sub(&["uploads", &slot.to_string()])
    }

    /// The compiled helpers, keyed by source and Xcode.
    pub fn bin(&self) -> PathBuf {
        self.sub(&["bin"])
    }

    /// The service's own temp (TMPDIR), apart from the slots' folders.
    pub fn cache_tmp(&self) -> PathBuf {
        self.sub(&["cache", "tmp"])
    }

    pub fn log_dir(&self) -> PathBuf {
        self.sub(&["log"])
    }

    /// One slot's places, as `Offer.sim.paths` reports them and the server
    /// passes them back (the node polices every one).
    pub fn slot_paths(&self, slot: Slot, devicetype_roots: &[String]) -> SlotPaths {
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        SlotPaths { set: s(self.set(slot)), tmp: s(self.tmp(slot)), uploads: s(self.uploads(slot)), devicetype_roots: devicetype_roots.to_vec() }
    }
}

/// `true` when `p` is a plain absolute path (no `..`), for paths a test
/// or the owner hands in on the command line.
pub fn plain_abs(p: &Path) -> bool {
    k2_node_proto::sim::policy::clean_abs(&p.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_gets_its_own_set_temp_and_uploads() {
        let r = SimRoot::system();
        let p = r.slot_paths(2, &["/Library/Developer/CoreSimulator/Profiles/DeviceTypes".into()]);
        assert_eq!((p.set.as_str(), p.tmp.as_str(), p.uploads.as_str()), ("/var/k2sim/sets/2", "/var/k2sim/tmp/2", "/var/k2sim/uploads/2"));
        assert_eq!(p.workspace_tmp("0123456789abcdef01234567"), "/var/k2sim/tmp/2/0123456789abcdef01234567");
        assert_eq!(r.socket(), PathBuf::from("/var/k2sim/run/simhost.sock"));
        assert!(plain_abs(Path::new("/var/k2sim")) && !plain_abs(Path::new("/var/../etc")) && !plain_abs(Path::new("rel")));
    }
}
