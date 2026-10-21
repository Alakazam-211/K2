//! `k2-root-proto` — the shared contract between the K2 daemon and
//! `/usr/local/libexec/k2-root-helper` (`prd-k2-root-helper-v1` §5.1).
//!
//! The helper is the only root door K2 has on a Linux box. Everything both
//! sides must agree on lives here, and nothing else does:
//!
//! - [`paths`]: every fixed path (helper, sudoers, config, state, keep
//!   store, Stalwart, Postgres, legacy helpers). Never redirected at
//!   runtime: there is no env var and no argv path.
//! - [`stalwart`]: the Stalwart pin + previous sha256 per arch.
//! - [`templates`]: the canned bytes the helper renders (Stalwart unit +
//!   hardening drop-in, Postgres RAM fence).
//! - [`verbs`]: the verb catalog — argv codec, `open`/`daemon` class,
//!   per-verb stdin caps, exit codes.
//! - [`version`]: `PROTOCOL`, the min-protocol constants, the build
//!   marker, semver compare, the `helper version` JSON shape.
//! - [`keys`]: the helper signing public keys (separate from the app
//!   updater key, RH7) plus the test key.
//! - [`sig`]: the trusted-comment format and the one signed-build check
//!   both sides run (the helper before installing, the daemon before
//!   spending a root call).
//! - [`apps`]: the App runtime's box prep (protocol 6): the `k2app` user,
//!   its subordinate ids, slice names, port ranges, and the canned unit,
//!   drop-in, sysctl and nftables bytes the helper renders.
//! - [`podman`]: K2's pinned Podman 5 bundle for boxes whose distro ships
//!   Podman < 5 (protocol 8): the sha256 table, the K2 prefix, which
//!   `podman`/`crun` k2app runs, and the k2app-only config bytes.
//! - [`caller`]: `daemon.toml` and the "is this the daemon's main
//!   process" match, shared by the helper and `k2-app-engine` (AR25).
//! - [`engine`]: `k2-app-engine`'s socket wire, paths, validators and its
//!   signed builds (root-helper key, like `k2-node`; S11).
//! - [`node_build`]: the same key and check for signed `k2-node` builds
//!   (prd-k2-node-admin-v1 §8.1: node updates are root-class code).
//! - [`cli`]: which `/usr/local/bin/k2` is the K2 CLI and its version
//!   (`helper cli-refresh`, protocol 9).
//! - [`sew`]: Sew on K2 (protocol 7): the Sew pin table, Sew's fixed paths
//!   and the stdin-JSON validators of the `engine sew` verbs.

pub mod apps;
pub mod caller;
pub mod cli;
pub mod engine;
pub mod keys;
pub mod node_build;
pub mod paths;
pub mod sew;
pub mod podman;
pub mod sig;
pub mod stalwart;
pub mod templates;
pub mod verbs;
pub mod version;

#[cfg(feature = "test-signer")]
pub mod testsign;

pub use verbs::{Class, Verb};
pub use version::PROTOCOL;
