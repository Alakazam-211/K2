//! K2 compute nodes: the shared wire (`prd-k2-compute-nodes-v1` §6).
//!
//! The controller (in `k2-core` / `k2-daemon`) and the node runtime
//! (`k2-node`, a separate binary that must never link `k2-core`) both
//! depend on this tiny crate and nothing else of each other's.
//!
//! - [`crypto`]: P-256 keys, fingerprints, domain-separated signatures.
//! - [`pairing`]: enroll codes, the enroll string, the nonce SAS (CN1).
//! - [`frames`]: handshake frames, the signed envelope, session frames,
//!   plans, offers, receipts.
//! - [`refusal`]: the one refusal function both ends use.
//! - [`env`]: job env rules (the daemon-reaching names, the PATH rule).
//! - [`canonical`]: canonical JSON for digests and signatures.

pub mod canonical;
pub mod crypto;
pub mod env;
pub mod frames;
pub mod pairing;
pub mod refusal;

pub use frames::PROTOCOL;
