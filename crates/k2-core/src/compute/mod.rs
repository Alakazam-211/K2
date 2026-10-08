//! K2 compute nodes, controller side (`prd-k2-compute-nodes-v1`).
//!
//! The controller is K2's own capability, so it lives in-process in the
//! daemon (cores review): this module holds the pure and SQLite parts
//! (store, enroll codes, the fair queue, code sync, the log store, the
//! completion text, the node machine's local supervisor). The live
//! sockets and routes are `k2-daemon`'s `compute_ws` / `compute_routes`.
//! The node runtime is the separate `k2-node` binary; both ends share
//! only `k2-node-proto`.
//!
//! Dark until `K2_COMPUTE=1` or the Settings switch "Compute nodes
//! (preview)" (`computePreview`) is on (CN30): every compute route 404s.

use std::sync::atomic::{AtomicBool, Ordering};

pub mod enroll;
pub mod local;
pub mod logs;
pub mod message;
pub mod scheduler;
pub mod store;
pub mod sync;

pub use k2_node_proto as proto;

static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `computePreview` setting into this process (boot and
/// every settings update/reset).
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// True when compute is on: `K2_COMPUTE` (`1`/`true`/`yes`/`on`) or the
/// persisted preview switch. Default OFF.
pub fn enabled() -> bool {
    if let Ok(v) = std::env::var("K2_COMPUTE") {
        if matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    SETTING_ENABLED.load(Ordering::Relaxed)
}

/// Unix seconds.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The controller's signing key: the existing tunnel key (CN3), also the
/// `<sub>.k2.dev` TLS key and the federation identity.
pub fn controller_key() -> Result<proto::crypto::SigningKey, String> {
    let kp = crate::tunnel::tls::load_or_generate_keypair()?;
    proto::crypto::SigningKey::from_pkcs8_der(&kp.serialize_der())
}

/// `~/.k2/compute/`.
pub fn compute_dir() -> std::path::PathBuf {
    crate::paths::k2_home().join("compute")
}

/// Grant defaults (§13).
pub const DEFAULT_MAX_JOB_SECS: i64 = 7200;
pub const DEFAULT_MAX_DISK_GB: i64 = 60;
pub const DEFAULT_MAX_PARALLEL: i64 = 1;
pub const DEFAULT_MAX_QUEUED: i64 = 10;
/// Per-job log cap (§11.3).
pub const LOG_CAP_BYTES: u64 = 200 << 20;
/// Relay-path caps (§12.4, CN17). LAN and tailnet aren't capped by us.
pub const RELAY_MAX_BUNDLE_BYTES: u64 = 50 << 20;
pub const MAX_DIRTY_BYTES: u64 = 50 << 20;

/// Stable error codes the CLI turns into exit 3 with teaching text.
pub mod codes {
    pub const COMPUTE_OFF: &str = "compute_off";
    pub const NOT_GRANTED: &str = "not_granted";
    pub const NODE_OFFLINE: &str = "node_offline";
    pub const NODE_PAUSED: &str = "node_paused";
    pub const NODE_NOT_FOUND: &str = "node_not_found";
    pub const QUEUE_FULL: &str = "queue_full";
    pub const OWNER_ONLY: &str = "owner_only";
    pub const IDEMPOTENCY_CONFLICT: &str = "idempotency_conflict";
    pub const JOB_NOT_FOUND: &str = "job_not_found";
    pub const BAD_REQUEST: &str = "bad_request";
    pub const NOT_A_REPO: &str = "not_a_repo";
    pub const DIRTY_TOO_LARGE: &str = "dirty_too_large";
    pub const NO_NODE_FITS: &str = "no_node_fits";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_defaults_off_and_env_or_setting_turns_it_on() {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os("K2_COMPUTE");
        std::env::remove_var("K2_COMPUTE");
        set_enabled(false);
        assert!(!enabled(), "compute must default OFF");
        std::env::set_var("K2_COMPUTE", "1");
        assert!(enabled());
        std::env::set_var("K2_COMPUTE", "off");
        assert!(!enabled());
        std::env::remove_var("K2_COMPUTE");
        set_enabled(true);
        assert!(enabled(), "computePreview setting turns it on");
        set_enabled(false);
        assert!(!enabled());
        match prev {
            Some(p) => std::env::set_var("K2_COMPUTE", p),
            None => std::env::remove_var("K2_COMPUTE"),
        }
    }

    #[test]
    fn prod_reach_vars_match_k2_core() {
        assert_eq!(proto::env::PROD_REACH_VARS, crate::test_isolation::PROD_REACH_VARS);
    }
}
