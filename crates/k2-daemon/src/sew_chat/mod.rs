//! Sew chat for agents: the daemon half of `k2 chat` (slice S30 core,
//! plan-sew-s30-s32-on-k2-v1 K17 agent routes + K18 approval queue).
//!
//! - [`sew`]: the [`sew::SewChat`] trait — every Sew call, always act-as the
//!   agent — and the backend registry. The real `sew-client` impl is K15.
//! - [`routes`]: `/cli/sew/chat/*` (agent verbs) and the shared plumbing.
//! - [`routes_approvals`]: `/cli/sew/approvals*` (the queue, the per-box
//!   `approvals.by` policy, vault-confirmed person approvals, labelled
//!   agent approvals).
//!
//! The data model (levels, approval modes, limits, the store) is
//! `k2_core::sew_chat` (migration 0167).

pub mod routes;
pub mod routes_approvals;
pub mod sew;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
