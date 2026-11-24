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
//!   denial and may mount (mount, umount2, pivot_root, the new mount API)
//!   and setns in a namespace of its own, so the owner's trusted jobs can
//!   build containers there; the always-denied list (bpf, perf events,
//!   userfaultfd, the keyring, io_uring, kexec, modules, file handles)
//!   stays. Only a workspace the node's owner
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

/// Offer feature: this node runs a `--userns --network none` job of its
/// owner's with `--no-addr-filter`: no cgroup address filter
/// (`IPAddressDeny=`/`IPAddressAllow=`) on the unit, so packets between
/// the job's own namespaces reach their interfaces; the job's network
/// namespace (loopback only, no path to the node's interfaces) stays,
/// and the shim checks it has no interface but `lo` before anything runs.
pub const FEATURE_NO_ADDR_FILTER: &str = "no-addr-filter";

/// Refusal codes.
pub mod codes {
    /// The workspace asked for `--userns` but the node's owner hasn't
    /// granted it user namespaces on that node.
    pub const USERNS_NOT_GRANTED: &str = "userns_not_granted";
    /// `--userns` is only for the node owner's own server: this server is
    /// a tenant of the node (or a node checked the job came from a
    /// tenant's slot).
    pub const USERNS_OWNER_ONLY: &str = "userns_owner_only";
    /// The node's kernel restricts user namespaces for unconfined processes
    /// (Ubuntu 23.10+) and root couldn't load K2's AppArmor profile for
    /// `--userns` units: the start is refused with why and the fix.
    pub const USERNS_BLOCKED_BY_APPARMOR: &str = "userns_blocked_by_apparmor";
    /// No node this job may use runs `--userns` jobs yet (k2-node 0.46.18).
    pub const USERNS_NEEDS_UPDATE: &str = "userns_needs_update";
    /// `--no-addr-filter` without `--userns` and `--network none` (or
    /// beside `--netns-dummy`, whose namespace has more than loopback).
    pub const NO_ADDR_FILTER_INVALID: &str = "no_addr_filter_invalid";
}
