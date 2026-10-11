//! Node admin, the node side (prd-k2-node-admin-v1): the root executor
//! (`k2-node admin serve`) and the unprivileged node's courier role.
//!
//! **Agents propose, owners approve, the node checks.** The executor is
//! the only privileged code; it trusts nothing it receives until the
//! controller signature (and, for privileged verbs, the owner approval
//! signature) check out against keys root pinned at the machine. The
//! wire is `k2_node_proto::admin` (SSOT; `.k2/prds/node-admin-wire-v1.md`).
//!
//! - [`config`]: `admin.toml` (level, pinned controller, approval keys).
//! - [`store`]: root-owned nonce store + hash-chained audit log.
//! - [`exec`]: the ordered checks and the verb dispatch.
//! - [`server`]: the local Unix socket, peer check, framing, client.
//! - [`policy`]: remote policy show/set/narrow and the widen classifier (S3).
//! - [`update`]: signed self-update, health gate, rollback (S5).
//! - [`courier`]: the node relaying admin frames to the executor.
//! - [`units`]: systemd/launchd files for the executor.

pub mod cli;
pub mod config;
pub mod courier;
pub mod exec;
#[cfg(target_os = "macos")]
pub mod launchd;
pub mod policy;
pub mod server;
pub mod store;
pub mod units;
pub mod update;

#[cfg(test)]
pub mod tests;
