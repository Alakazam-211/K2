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
//! CI on compute (0.46.10) has its own preview on top, [`ci_enabled`]
//! (`ciPreview` / `K2_CI`), default off.

use std::sync::atomic::{AtomicBool, Ordering};

pub mod access;
pub mod admin;
pub mod allow;
pub mod advertise;
pub mod enroll;
pub mod git;
pub mod lane_secrets;
pub mod local;
pub mod logs;
pub mod message;
pub mod pkgreq;
pub mod result;
pub mod retry;
pub mod rollout;
pub mod scheduler;
pub mod shards;
pub mod share;
pub mod store;
pub mod sync;
pub mod ticket;
pub mod trust;

pub use k2_node_proto as proto;

static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `computePreview` setting into this process (boot and
/// every settings update/reset).
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// The in-process setting alone (no env force), for `test_env`'s per-test
/// snapshot of process switches.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn setting_raw() -> bool {
    SETTING_ENABLED.load(Ordering::Relaxed)
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

// ── CI on compute: its own preview (0.46.10 ships it dark) ───────────

/// The env var that forces the CI-on-compute preview on (`1`/`true`/`yes`/
/// `on`), like `K2_COMPUTE` for compute itself.
pub const CI_ENV: &str = "K2_CI";

/// The refusal code every CI-on-compute surface answers while its preview
/// is off (409, the `voice_tunnel_preview_off` pattern: a preview on top
/// of a preview that is on).
pub const CI_PREVIEW_OFF: &str = "ci_preview_off";

/// How the owner turns it on (help, study pages and every refusal say it).
pub const CI_PREVIEW_HOW: &str = "k2 settings set ciPreview true";

static CI_SETTING: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `ciPreview` setting into this process (boot and
/// every settings update/reset).
pub fn set_ci_enabled(on: bool) {
    CI_SETTING.store(on, Ordering::Relaxed);
}

/// The in-process CI setting alone (no env force), for `test_env`.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn ci_setting_raw() -> bool {
    CI_SETTING.load(Ordering::Relaxed)
}

/// True when CI on compute is on (prd-ci-on-compute-for-clients-v1: `k2
/// ci` and its loop, node rosters, lanes, seats, owner trust and consent,
/// signer pairing, lane secrets, the V27 artifact delete, receipt v2 on
/// submit, the node roster / statement / rotation / seat-enrol hooks on the
/// node sockets): compute is on ([`enabled`]) **and** `K2_CI` or the
/// persisted `ciPreview` switch. Default OFF: 0.46.10 ships it dark, and
/// compute without CI is exactly 0.46.9's.
pub fn ci_enabled() -> bool {
    if !enabled() {
        return false;
    }
    if let Ok(v) = std::env::var(CI_ENV) {
        if matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    CI_SETTING.load(Ordering::Relaxed)
}

/// The one line a CI-on-compute refusal carries: what was refused and how
/// the owner turns the preview on.
pub fn ci_preview_hint(what: &str) -> String {
    format!(
        "{what} is part of CI on K2 Compute, a preview that is off on this server: the owner turns it on ({CI_PREVIEW_HOW}, or {CI_ENV}=1 in the daemon's environment; compute must be on too)"
    )
}

/// How auto-placement ranks the nodes that fit (`computePlacement`,
/// prd-compute-auto-placement-v1 §8, CP-A18). `V2` (the default): load
/// bands, a warm slot worth one band, bursts spread. `V1`: the 0.46.5
/// order (warm slot first, then free CPU), kept until v2 is proven
/// (`feedback_keep_old_path_until_proven`). Only the ranking switches:
/// no node named = auto and the placement record hold either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementPolicy {
    V1,
    V2,
}

impl PlacementPolicy {
    pub fn as_wire(self) -> &'static str {
        match self {
            PlacementPolicy::V1 => "v1",
            PlacementPolicy::V2 => "v2",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "v1" => Some(PlacementPolicy::V1),
            "v2" => Some(PlacementPolicy::V2),
            _ => None,
        }
    }
}

static PLACEMENT_V1: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `computePlacement` setting into this process (boot
/// and every settings update/reset). Anything but `v1` is v2.
pub fn set_placement_policy(p: PlacementPolicy) {
    PLACEMENT_V1.store(p == PlacementPolicy::V1, Ordering::Relaxed);
}

/// The ranking auto-placement uses right now.
pub fn placement_policy() -> PlacementPolicy {
    if PLACEMENT_V1.load(Ordering::Relaxed) {
        PlacementPolicy::V1
    } else {
        PlacementPolicy::V2
    }
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
/// A job without `--mem` reserves this much (Rosson 10-28: `--mem` is
/// required; missing, the default plus a warning), clamped to a smaller
/// node's cap. The node stops a job that goes over its reservation
/// (`failed/mem_cap`).
pub const DEFAULT_MEM_BYTES: u64 = 4 << 30;
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
        // The one env lock (also serializes the computePreview setting);
        // `_unset` restores the caller's K2_COMPUTE on drop.
        let _env = crate::test_env::lock();
        let _unset = crate::test_env::EnvVar::remove("K2_COMPUTE");
        set_enabled(false);
        assert!(!enabled(), "compute must default OFF");
        {
            let _on = crate::test_env::EnvVar::set("K2_COMPUTE", "1");
            assert!(enabled());
        }
        {
            let _off = crate::test_env::EnvVar::set("K2_COMPUTE", "off");
            assert!(!enabled());
        }
        set_enabled(true);
        assert!(enabled(), "computePreview setting turns it on");
        set_enabled(false);
        assert!(!enabled());
    }

    #[test]
    fn ci_preview_defaults_off_and_needs_compute_on_too() {
        let _env = crate::test_env::lock();
        let _unset = crate::test_env::EnvVar::remove("K2_COMPUTE");
        let _unset_ci = crate::test_env::EnvVar::remove(CI_ENV);
        assert!(!ci_enabled(), "CI on compute must default OFF");
        // Compute on alone (0.46.9's switch) never turns CI on.
        set_enabled(true);
        assert!(!ci_enabled(), "computePreview alone leaves CI off");
        set_ci_enabled(true);
        assert!(ci_enabled(), "ciPreview with compute on turns it on");
        // CI without compute is off: every CI surface rides compute.
        set_enabled(false);
        assert!(!ci_enabled(), "ciPreview without compute stays off");
        set_ci_enabled(false);
        {
            let _c = crate::test_env::EnvVar::set("K2_COMPUTE", "1");
            assert!(!ci_enabled());
            let _ci = crate::test_env::EnvVar::set(CI_ENV, "1");
            assert!(ci_enabled(), "K2_CI=1 forces it on (with compute on)");
            let _off = crate::test_env::EnvVar::set(CI_ENV, "off");
            assert!(!ci_enabled());
        }
        let hint = ci_preview_hint("k2 ci");
        assert!(hint.contains(CI_PREVIEW_HOW) && hint.contains("K2_CI=1"), "{hint}");
    }

    #[test]
    fn prod_reach_vars_match_k2_core() {
        assert_eq!(proto::env::PROD_REACH_VARS, crate::test_isolation::PROD_REACH_VARS);
    }
}
