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
//! - [`needs`]: toolchains and system packages a job declares.
//! - [`ticket`]: one-click enroll tickets from the control plane.
//! - [`admin`]: node admin — the signed command, classes, levels, the
//!   verb catalog and refusal codes (prd-k2-node-admin-v1 §7). SSOT.
//! - [`health`]: node health facts (§10.1).
//! - [`results`]: artifact globs, collected artifacts, JUnit test summaries.
//! - [`queue`]: the node queue's rules (order, starts, ETAs, quotes, views).
//! - [`share`]: one node, many servers (protocol 3): slots, classes,
//!   shares, `servers.toml`, quotes and queue views, the `servers.*` verb
//!   arguments and the sharing reason codes.
//! - [`secret`]: compute secret-env (protocol 4): the seal to the node's
//!   one-time key, caps, codes, the stdin pipe payload.

pub mod admin;
pub mod canonical;
pub mod crypto;
pub mod env;
pub mod frames;
pub mod health;
pub mod needs;
pub mod pairing;
pub mod queue;
pub mod refusal;
pub mod results;
pub mod secret;
pub mod share;
pub mod ticket;

pub use frames::PROTOCOL;
