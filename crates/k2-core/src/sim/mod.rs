//! Sim sessions: device emulators / Simulators the daemon runs for a
//! workspace (`prd-mobile-dev-v1`, vs-live M-A1–M-A28, 0.46.6).
//!
//! "Sim session", never "device session" (that is a paired Companion
//! phone, M-A2). One device per workspace in v1.
//!
//! - [`lane`]: the [`lane::SimLane`] trait every backend implements, and
//!   its types. The only lane-specific seam.
//! - [`tree`]: the one accessibility-tree format, snapshot ids, render and
//!   diff.
//! - [`hold`]: person-wins control hold, Take over / Hand back.
//! - [`limits`]: box limits (`~/.k2/sim.json`).
//! - [`stream`]: the `/cli/sim/stream` wire format.
//! - [`api`]: `/cli/sim/*` request / response shapes.
//! - [`android`]: the Android emulator lane.
//!
//! Everything here is lane-blind except [`android`] (and the iOS lane,
//! which plugs in through [`register_lane`]).

pub mod android;
pub mod api;
pub mod encode;
#[cfg(any(test, feature = "test-util"))]
pub mod fake;
pub mod hold;
// The iOS Simulator lane (L-I). Compiles on every unix so its unit tests run
// on Linux nodes; it registers only on macOS.
#[cfg(unix)]
pub mod ios;
pub mod lane;
pub mod licence;
pub mod limits;
pub mod manager;
pub mod store;
pub mod stream;
pub mod tree;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

pub use lane::{LaneKind, SimLane};

/// The boot-status feature key (M-A13): pushed when at least one lane is
/// usable.
pub const BOOT_STATUS_KEY: &str = "mobile-sim-v1";

/// What the tab says about isolation (PRD §8, "Honest limits").
pub const ISOLATION_NOTE: &str =
    "Separate, not sealed: the device runs as the same system user as this server's agents.";

static BASE_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// The K2 home as sims see it: `~/.k2`, or a test's temp folder
/// ([`set_base_for_tests`]) so no test ever writes the real `~/.k2`.
pub fn base() -> PathBuf {
    if let Some(p) = BASE_OVERRIDE.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return p.clone();
    }
    crate::paths::k2_home()
}

/// Point [`base`] at a temp folder (tests only).
#[cfg(any(test, feature = "test-util"))]
pub fn set_base_for_tests(p: PathBuf) {
    *BASE_OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

/// `~/.k2/sim`: SDKs, device data, licence records.
pub fn sim_home() -> PathBuf {
    base().join("sim")
}

/// Where a workspace's device data lives for a lane.
pub fn device_dir(lane: LaneKind, workspace_key: &str) -> PathBuf {
    sim_home().join(lane.as_str()).join("devices").join(workspace_key)
}

/// A filesystem-safe, stable key for a workspace id (lowercase hex of the
/// first 12 bytes of its sha256). Never the handle: handles can be
/// renamed.
pub fn workspace_key(workspace_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(workspace_id.as_bytes());
    d.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

// ── lane registry ────────────────────────────────────────────────────

static LANES: OnceLock<RwLock<Vec<Arc<dyn SimLane>>>> = OnceLock::new();

fn lanes_cell() -> &'static RwLock<Vec<Arc<dyn SimLane>>> {
    LANES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Add (or replace) a lane. The daemon registers the built-in lanes at
/// boot; tests register fakes.
pub fn register_lane(l: Arc<dyn SimLane>) {
    let mut v = lanes_cell().write().unwrap_or_else(|e| e.into_inner());
    v.retain(|x| x.kind() != l.kind());
    v.push(l);
    v.sort_by_key(|x| x.kind());
}

/// Remove every lane (tests).
pub fn clear_lanes() {
    lanes_cell().write().unwrap_or_else(|e| e.into_inner()).clear();
}

pub fn lane(kind: LaneKind) -> Option<Arc<dyn SimLane>> {
    lanes_cell().read().unwrap_or_else(|e| e.into_inner()).iter().find(|l| l.kind() == kind).cloned()
}

pub fn lanes() -> Vec<Arc<dyn SimLane>> {
    lanes_cell().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Register the lanes built into this daemon for this OS.
pub fn register_builtin_lanes() {
    register_lane(Arc::new(android::AndroidLane::new(sim_home().join("android"))));
    #[cfg(target_os = "macos")]
    register_lane(Arc::new(ios::IosLane::new(ios::IosLaneConfig::under(&sim_home()))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_key_is_stable_hex() {
        let k = workspace_key("ws-123");
        assert_eq!(k.len(), 24);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(k, workspace_key("ws-123"));
        assert_ne!(k, workspace_key("ws-124"));
    }
}
