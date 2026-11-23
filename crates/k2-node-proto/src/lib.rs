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
//! - [`tiers`]: compute job tiers (protocol 5): CI, Priority, Normal, the
//!   rules every orderer shares, and their codes.
//! - [`cache`]: per-workspace job caches (protocol 6): names, the
//!   workspace key, `cache_get` / `cache_view`.
//! - [`host`]: host facts (protocol 7): distro, version, package manager,
//!   kernel, libc, GPU; `--distro`.
//! - [`pkgmap`]: distro-neutral package names (protocol 7).
//! - [`profiles`]: K2-shipped package profiles (protocol 7).
//! - [`pkgpolicy`]: the per-server package policy, the deny list, package
//!   requests and their frames (protocol 7).
//! - [`gitcred`]: compute git access (protocol 8): repo keys, the sealed
//!   git token, its codes.
//! - [`ci`]: CI on compute for clients (protocol 9): the features, signing
//!   domains, enums, the signed envelope, receipt v2's run facts
//!   (`docs/ci-formats-v1.md`).
//! - [`statement`]: the node self-statement and key rotation (protocol 9).
//! - [`node_roster`]: the owner-signed node roster, seats, attestations.
//! - [`lanes`]: the node's lanes file, the `lanes.write` and
//!   `node_roster.set` verbs, the roster the node relays (protocol 9).
//! - [`evidence`]: the evidence bundle v2 the signer checks.
//! - [`signer_policy`]: the signer's per-project policy file (TOML).
//! - [`ci_config`]: `.k2/ci.toml`, the `k2 ci` config (TOML).
//! - [`gpu`]: GPU jobs (0.46.11): the ask, the facts, the codes.
//! - [`jobnet`]: a job's own network ask (0.46.14): `--network none`,
//!   `--netns-dummy`, their features and checks.
//! - [`gens`]: cache generations, setup snapshots, idle cores, job stats
//!   (protocol 10 contracts, prd-compute-blacksmith-class-v1).
//! - [`boxes`]: warm boxes for agents (protocol 10 frames).
//! - [`provenance`]: the node-signed build statement (in-toto Statement
//!   v1, SLSA Provenance v1, DSSE; protocol-10 feature `provenance-v1`).
//! - [`sim`]: iPhone on a Mac node (protocol 7): sim session frames, the
//!   encrypted stream, the node's run policy, upload rules, leases, and
//!   the Simulator helper's source.

pub mod admin;
pub mod boxes;
pub mod cache;
pub mod canonical;
pub mod ci;
pub mod ci_config;
pub mod crypto;
pub mod env;
pub mod evidence;
pub mod frames;
pub mod gens;
pub mod gitcred;
pub mod gpu;
pub mod health;
pub mod host;
pub mod jobnet;
pub mod lanes;
pub mod needs;
pub mod node_roster;
pub mod pairing;
pub mod pkgmap;
pub mod pkgpolicy;
pub mod profiles;
pub mod provenance;
pub mod queue;
pub mod refusal;
pub mod results;
pub mod secret;
pub mod share;
pub mod signer_policy;
pub mod statement;
pub mod sim;
pub mod stall;
pub mod ticket;
pub mod tiers;
pub mod userns;

pub use frames::PROTOCOL;
