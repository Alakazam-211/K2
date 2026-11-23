//! User namespaces in compute jobs (0.46.18): denied unless the node's
//! owner allows them for the job.
//!
//! - **The default** (Linux job wall): a job can't make a user namespace.
//!   The unit's seccomp filter answers `clone3` with `ENOSYS` (its flags
//!   can't be inspected, so libc falls back to `clone`), refuses `clone`
//!   and `unshare` with `CLONE_NEWUSER` (`EPERM`) and refuses `setns`; the
//!   job runs in a user namespace of its own whose `max_user_namespaces`
//!   is 0 (that namespace only, never the node's sysctl).
//! - **`--userns`** (`k2 compute run --userns`): the job skips that
//!   denial; every other denial stays. Only a workspace the node's owner
//!   granted it (`k2 compute grant <node> --workspace <w> --userns`) may
//!   ask, and only on a node its own server owns: a server that is a
//!   tenant of the node (its jobs run in a slot 1–7) never gets it, and
//!   root refuses it for any slot but the owner's ([`codes::USERNS_OWNER_ONLY`]).
//! - Carried as `Plan.userns` (absent unless asked, so every other plan
//!   and digest is unchanged), sent only to nodes offering
//!   [`FEATURE_JOB_USERNS`] (an older node would compute another digest
//!   and refuse the assign).

/// Offer feature: this node denies user namespaces in jobs and runs a
/// `--userns` job (its owner's) without that denial.
pub const FEATURE_JOB_USERNS: &str = "job-userns";

/// Refusal codes.
pub mod codes {
    /// The workspace asked for `--userns` but the node's owner hasn't
    /// granted it user namespaces on that node.
    pub const USERNS_NOT_GRANTED: &str = "userns_not_granted";
    /// `--userns` is only for the node owner's own server: this server is
    /// a tenant of the node (or a node checked the job came from a
    /// tenant's slot).
    pub const USERNS_OWNER_ONLY: &str = "userns_owner_only";
    /// No node this job may use runs `--userns` jobs yet (k2-node 0.46.18).
    pub const USERNS_NEEDS_UPDATE: &str = "userns_needs_update";
}
