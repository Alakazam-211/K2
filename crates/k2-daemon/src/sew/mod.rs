//! Sew on K2 — the daemon's Sew supervisor (Sew S29, work item K12;
//! `plan-sew-s27-s29-on-k2-v1`, `plan-sew-s30-s32-on-k2-v1` K19).
//!
//! K2 installs, runs, watches, upgrades, backs up and removes Sew on a
//! Linux box the way it runs Stalwart, through the one root helper's
//! `engine sew` namespace (protocol 7). Sew is opt-in and never part of the
//! base install; nothing here ever touches hosted mail's lifecycle.
//!
//! - [`ops`]: everything with a side effect (root helper, systemd reads,
//!   box probes, the mgmt socket), behind [`ops::SewOps`] with a recording
//!   fake for tests.
//! - [`secrets`]: the master keys in the K2 vault (system namespace, scope
//!   `engine:sew`), never a file of K2's own.
//! - [`store`]: the singleton `sew_server` row (migration 0166).
//! - [`supervisor`]: preflight, the enable / upgrade / rollback / restore
//!   machines, disable / uninstall / purge, master-key rotation, status
//!   reconcile and the health loop.
//! - [`routes`]: `/cli/sew/*` with the gates (owner/admin, or an agent whose
//!   workspace has "Manage Sew" on; person-only steps need a vault confirm).
//!
//! Not here yet (later work items): the grant hook target and owner
//! provisioning (S27 K2–K4), the chat pass link (S28 K6/K7), the voice
//! media flow and TURN cert (K13), `k2 hostchat` and the `SewChanged` event
//! mirror (K14), host / mirror / ship / app-url routes (S32).

pub mod ops;
pub mod routes;
pub mod secrets;
pub mod store;
pub mod supervisor;

/// Sew runs on Linux only (a runtime check, so the machines are testable
/// on a Mac through the fakes).
pub fn sew_supported() -> bool {
    cfg!(target_os = "linux")
}
