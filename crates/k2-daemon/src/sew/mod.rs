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
//! - [`voice`]: `sew-media` with TURN over TLS (K13): ports preflight,
//!   node IP, the TURN host + cert from K2 Connect's turn mode, renewal,
//!   `voice.public_url`, status mapping.
//! - [`users`]: Sew's own users on a standalone install (setup links).
//! - [`service_user`]: the preflight hard stop that keeps Sew from adopting
//!   a login account named `sew` as its service user.
//!
//! `k2 hostchat` (K14) drives all of it; [`supervisor::changed`] fires the
//! `sew:changed` hook, mirrored as the app-level `sew_changed` event.
//!
//! S27's grant hook (0.46.1) follows the install: enable, a finished
//! upgrade and daemon boot point its chat target at Sew
//! ([`supervisor::grant_hook_on`]; the worker's mgmt token comes from the
//! helper through [`supervisor::mgmt_token`], never from Sew's token
//! file); disable pauses it; uninstall pauses or drops it; purge drops it
//! ([`supervisor::grant_hook_off`]).
//!
//! Not here yet: host / mirror / ship / app-url routes (S32).

pub mod ops;
pub mod routes;
pub mod secrets;
pub mod service_user;
pub mod sew_wire;
pub mod store;
pub mod supervisor;
pub mod users;
pub mod voice;

#[cfg(test)]
mod grant_hook_tests;

/// Sew runs on Linux only (a runtime check, so the machines are testable
/// on a Mac through the fakes).
pub fn sew_supported() -> bool {
    cfg!(target_os = "linux")
}
