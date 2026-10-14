//! `k2-app-engine` (`prd-k2-app-runtime-v1` §10.6 option C, S11/S12).
//!
//! A small binary that runs as the `k2app` user (a lingering user unit
//! installed by the root helper's `apps install-engine`) and drives
//! rootless Podman + Quadlet for guarded Apps. It listens on
//! `/run/k2app/engine.sock` and admits only the K2 daemon's main process
//! ([`peer`]); agents share the daemon's uid but never its main pid.
//!
//! - [`server`]: the socket and the closed verb list;
//! - [`quadlet`]: the per-deployment `.container` from a canned template;
//! - [`seccomp`]: Podman's default profile + K2's denies;
//! - [`seeds`]: Seeds unpacked defensively under `k2app`'s home;
//! - [`slots`]: each App's pinned id range and gateway port.
//!
//! Signed with the root-helper key (trusted comment
//! `file:k2-app-engine-linux-<arch>`), installed and updated only through
//! the root helper. Links nothing from k2-daemon or k2-core.

pub mod peer;
pub mod quadlet;
pub mod seccomp;
pub mod seeds;
pub mod server;
pub mod slots;
pub mod sys;

/// The marker a signed build must carry exactly once.
#[cfg(target_arch = "x86_64")]
#[used]
pub static BUILD_MARKER: &str = concat!("K2_APP_ENGINE_BUILD v=", env!("K2_PRODUCT_VERSION"), " proto=1 arch=x86_64;");
#[cfg(target_arch = "aarch64")]
#[used]
pub static BUILD_MARKER: &str = concat!("K2_APP_ENGINE_BUILD v=", env!("K2_PRODUCT_VERSION"), " proto=1 arch=aarch64;");

/// Present only in a `test-keys` build; release signing refuses it.
#[cfg(feature = "test-keys")]
#[used]
pub static TEST_KEYS_FLAG: &str = "K2_APP_ENGINE_TEST_KEYS_ENABLED";

/// Referenced from `version` so the linker keeps both strings.
pub fn build_strings() -> (&'static str, Option<&'static str>) {
    #[cfg(feature = "test-keys")]
    let t = Some(TEST_KEYS_FLAG);
    #[cfg(not(feature = "test-keys"))]
    let t = None;
    (BUILD_MARKER, t)
}

#[cfg(test)]
mod tests {
    #[test]
    fn marker_matches_the_proto_and_protocol() {
        let (m, _) = super::build_strings();
        let found = k2_root_proto::engine::find_markers(m.as_bytes()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].protocol, k2_root_proto::engine::ENGINE_PROTOCOL);
        assert_eq!(found[0].version.to_string(), env!("K2_PRODUCT_VERSION"));
    }
}
