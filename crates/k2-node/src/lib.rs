//! k2-node: the compute node runtime (`prd-k2-compute-nodes-v1` §8).
//!
//! Runs as its own OS user (`k2node` / `_k2node`), opens no port, dials
//! out to one controller, and runs that controller's jobs within the
//! limits the machine's owner set. It never links `k2-core`; the wire is
//! `k2-node-proto`.

pub mod admin;
pub mod artifacts;
pub mod cgroup;
pub mod config;
pub mod enroll;
pub mod fairq;
pub mod identity;
pub mod install;
pub mod journal;
pub mod ledger;
pub mod locks;
pub mod node;
pub mod packages;
pub mod paths;
pub mod queue_store;
pub mod request;
pub mod runner;
pub mod session;
pub mod setup;
pub mod slots;
pub mod status;
pub mod sync;
pub mod sysinfo;
pub mod toolchain;
pub mod url;
pub mod util;
pub mod wall;

/// The product version this build reports (the daemon's; build.rs).
pub const PRODUCT_VERSION: &str = env!("K2_PRODUCT_VERSION");

/// The marker a signed build carries exactly once
/// (`k2_root_proto::node_build`, prd-k2-node-admin-v1 §8.2). `proto=` is
/// `k2_node_proto::PROTOCOL` (a test pins them together).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[used]
pub static BUILD_MARKER: &str = concat!("K2_NODE_BUILD v=", env!("K2_PRODUCT_VERSION"), " proto=4 os=linux arch=x86_64;");
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[used]
pub static BUILD_MARKER: &str = concat!("K2_NODE_BUILD v=", env!("K2_PRODUCT_VERSION"), " proto=4 os=linux arch=aarch64;");
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[used]
pub static BUILD_MARKER: &str = concat!("K2_NODE_BUILD v=", env!("K2_PRODUCT_VERSION"), " proto=4 os=macos arch=x86_64;");
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[used]
pub static BUILD_MARKER: &str = concat!("K2_NODE_BUILD v=", env!("K2_PRODUCT_VERSION"), " proto=4 os=macos arch=aarch64;");

/// Present only in a `test-keys` build (it also trusts the TEST minisign
/// key); release signing must refuse a binary that contains it.
#[cfg(feature = "test-keys")]
#[used]
pub static TEST_KEYS_FLAG: &str = "K2_NODE_TEST_KEYS_ENABLED";

/// The build marker (referenced so the linker keeps it).
pub fn build_marker() -> &'static str {
    BUILD_MARKER
}

#[cfg(test)]
mod build_tests {
    #[test]
    fn marker_matches_protocol_and_parses() {
        let m = k2_root_proto::node_build::find_markers(super::BUILD_MARKER.as_bytes()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].protocol, k2_node_proto::PROTOCOL, "bump proto= in BUILD_MARKER with PROTOCOL");
        assert_eq!(m[0].version.to_string(), super::PRODUCT_VERSION);
        assert_eq!(m[0].os, crate::sysinfo::os_label());
        assert_eq!(m[0].arch, std::env::consts::ARCH, "asset arch names are Rust arch names (aarch64), not labels (arm64)");
    }
}
