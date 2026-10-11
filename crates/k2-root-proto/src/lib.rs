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

pub mod keys;
pub mod paths;
pub mod sig;
pub mod stalwart;
pub mod templates;
pub mod verbs;
pub mod version;

#[cfg(feature = "test-signer")]
pub mod testsign;

pub use verbs::{Class, Verb};
pub use version::PROTOCOL;
