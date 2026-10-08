//! k2-node: the compute node runtime (`prd-k2-compute-nodes-v1` §8).
//!
//! Runs as its own OS user (`k2node` / `_k2node`), opens no port, dials
//! out to one controller, and runs that controller's jobs within the
//! limits the machine's owner set. It never links `k2-core`; the wire is
//! `k2-node-proto`.

pub mod cgroup;
pub mod config;
pub mod enroll;
pub mod identity;
pub mod install;
pub mod journal;
pub mod ledger;
pub mod locks;
pub mod node;
pub mod paths;
pub mod runner;
pub mod session;
pub mod slots;
pub mod status;
pub mod sync;
pub mod sysinfo;
pub mod url;
pub mod util;
