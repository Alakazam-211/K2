//! Sew chat for agents: the daemon half of `k2 chat` (slice S30 core,
//! plan-sew-s30-s32-on-k2-v1 K17 agent routes + K18 approval queue).
//!
//! - [`sew`]: the [`sew::SewChat`] trait — every Sew call, always act-as the
//!   agent — and the backend registry.
//! - [`client`]: the real backend (K15), Sew 0.3.0's vendored `sew-client`
//!   over the mgmt socket (Unix only; installed with the Sew lifecycle).
//! - [`routes`]: `/cli/sew/chat/*` (agent verbs) and the shared plumbing.
//! - [`routes_approvals`]: `/cli/sew/approvals*` (the queue, the per-box
//!   `approvals.by` policy, vault-confirmed person approvals, labelled
//!   agent approvals). K18.
//! - [`routes_admin`]: `/cli/sew/admin/*` (agent access, people, the audit
//!   log; the Manage Sew gate with no self-loosening) and
//!   `/cli/sew/on-behalf*` (a person's opt-in). K19.
//! - [`routes_manage`]: `/cli/sew/chat/manage/*`, community management
//!   through act-as (Manage Sew + level manage, then Sew S12). K20.
//!
//! The data model (levels, approval modes, limits, the store) is
//! `k2_core::sew_chat` (migration 0209).

#[cfg(unix)]
pub mod client;
pub mod routes;
pub mod routes_admin;
pub mod routes_approvals;
pub mod routes_manage;
pub mod sew;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
