//! Tool errors and the exit codes every verb maps to, in one place.

use crate::session::SessionError;

/// Exit codes (the PRD's §9.1 table).
pub mod exit {
    /// Success. For `diff`: no differences.
    pub const OK: u8 = 0;
    /// The tool's own "no": grep found nothing, a patch didn't apply, diff found
    /// differences, a write conflicted.
    pub const NO: u8 = 1;
    /// Bad usage, an unsupported flag, a refused command (`sed e/w/r/R/W`, a backup
    /// suffix, a backreference in a pattern).
    pub const USAGE: u8 = 2;
    /// Refused: the caller lacks the right.
    pub const REFUSED: u8 = 3;
    /// Not found: no such doc, or live editing is off for it.
    pub const NOT_FOUND: u8 = 4;
    /// Held: a permission card pending or `approvals_backlog` (the same code as usage).
    pub const HELD: u8 = 2;
    /// Leased by someone else (tier 0), or claimed by someone else (amendment 13: a new
    /// overlapping claim, or an agent's edit under `ClaimPolicy::Respect`). The same
    /// meaning: another participant holds it for now; retry after the time left, or wait.
    pub const LEASED: u8 = 5;
    /// The store is unavailable (disk full, fsync failure): nothing acked.
    pub const STORE: u8 = 6;
    /// Link lost mid-verb (federation; the host's).
    pub const LINK_LOST: u8 = 7;
    /// Too old: `revert_too_old`, `base_expired`. Never retry the same request.
    pub const TOO_OLD: u8 = 8;
}

/// Why a tool call was refused before it changed anything. Every variant exits 2
/// ([`ToolError::exit`]); nothing is ever applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolError {
    /// A flag, command or syntax outside the supported subset. `hint` says what to use.
    #[error("{flag}: not supported ({hint})")]
    Unsupported { flag: String, hint: String },
    /// Supported syntax the tools refuse on purpose: `sed` commands that touch other files
    /// or run commands (`e`, `w`, `r`, `R`, `W`), backup suffixes (`-i.bak`: versions replace
    /// backups), backreferences in a pattern.
    #[error("{what}: refused ({reason})")]
    Refused { what: String, reason: String },
    /// Bad usage: a missing operand, a bad number, a malformed script, pattern or patch.
    #[error("{0}")]
    Usage(String),
    /// The call would exceed a bound (output size, regex size, work).
    #[error("limit: {0}")]
    Limit(String),
}

impl ToolError {
    /// The exit code: always [`exit::USAGE`] (2).
    pub fn exit(&self) -> u8 {
        exit::USAGE
    }

    /// [`ToolError::Unsupported`] (for packs' own verbs too).
    pub fn unsupported(flag: impl Into<String>, hint: impl Into<String>) -> Self {
        ToolError::Unsupported {
            flag: flag.into(),
            hint: hint.into(),
        }
    }

    /// [`ToolError::Refused`].
    pub fn refused(what: impl Into<String>, reason: impl Into<String>) -> Self {
        ToolError::Refused {
            what: what.into(),
            reason: reason.into(),
        }
    }

    /// [`ToolError::Usage`].
    pub fn usage(msg: impl Into<String>) -> Self {
        ToolError::Usage(msg.into())
    }
}

/// Error from [`crate::session::Session::run_tool`]: the tool refused the call, or the
/// session did. Nothing was applied either way.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolRunError {
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    Session(#[from] SessionError),
}

impl ToolRunError {
    /// The exit code for this error (see [`session_exit`]).
    pub fn exit(&self) -> u8 {
        match self {
            ToolRunError::Tool(e) => e.exit(),
            ToolRunError::Session(e) => session_exit(e),
        }
    }
}

/// The exit code for a session error: the requirements' one table (K-EXIT-1, `docs/SESSIONS.md`).
/// Total: every variant is listed, so a new one doesn't compile until it is mapped.
///
/// Anything that says "the thing you named doesn't exist" (a doc, version, change, approval,
/// blob or blob space) is **4**, wherever it comes from (the session, the store, the blob
/// store); real store failures (I/O, a damaged record, a newer format, the lock) are **6**
/// (`store_exit`).
pub fn session_exit(e: &SessionError) -> u8 {
    use crate::blob::BlobError;
    match e {
        SessionError::Forbidden { .. } => exit::REFUSED,
        SessionError::NotFound(_)
        | SessionError::Declined(_)
        | SessionError::PackMismatch { .. }
        | SessionError::NoSuchChange(_)
        | SessionError::NoApproval { .. }
        | SessionError::NoBlobStore
        | SessionError::Deleted
        | SessionError::NoClaim(_)
        | SessionError::Blob(BlobError::NotFound(_) | BlobError::TooLarge { .. }) => {
            exit::NOT_FOUND
        }
        SessionError::NotJoined { .. }
        | SessionError::RateLimited { .. }
        | SessionError::BadParticipant(_)
        | SessionError::BadPosition(_)
        | SessionError::BadEdit(_)
        | SessionError::BadApproval(_)
        | SessionError::BadPath(_)
        | SessionError::NoLease
        | SessionError::BadClaim(_)
        | SessionError::Doc(_) => exit::USAGE,
        SessionError::Held { .. } | SessionError::ClaimContended { .. } => exit::HELD,
        SessionError::Leased { .. } | SessionError::Claimed { .. } => exit::LEASED,
        SessionError::Store(e) | SessionError::Blob(BlobError::Store(e)) => store_exit(e),
        SessionError::RevertTooOld { .. } => exit::TOO_OLD,
        SessionError::Stale { .. }
        | SessionError::Disk(_)
        | SessionError::Blob(_)
        | SessionError::Pack(_)
        | SessionError::Unloaded
        | SessionError::Released
        | SessionError::Broken(_) => exit::NO,
    }
}

/// The exit code for a store error (inside [`SessionError::Store`], or the blob store's
/// `BlobError::Store`): **4** when the thing named doesn't exist (no doc,
/// version, approval, blob or blob space; a purged version), **2** for the current blob
/// version (purge: write or restore another first), **6** for real store failures.
fn store_exit(e: &crate::store::StoreError) -> u8 {
    use crate::store::StoreError as S;
    match e {
        S::NoDoc(_)
        | S::NoVersion(..)
        | S::NoApproval(..)
        | S::NoBlobSpace
        | S::NoBlob(_)
        | S::PurgedVersion(..) => exit::NOT_FOUND,
        S::CurrentVersion(..) => exit::USAGE,
        S::Db(_)
        | S::Newer { .. }
        | S::Corrupt(_)
        | S::KeyTaken(_)
        | S::Locked
        | S::NamedVersion(..)
        | S::KeyChange
        | S::LogMoved(..)
        | S::NoRandom(_) => exit::STORE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Need, SessionError};

    /// K-EXIT-1: one table. `session_exit` lists every variant (a new one doesn't compile
    /// until mapped); this pins the codes.
    #[test]
    fn exit_code_table_is_total() {
        use crate::blob::BlobError;
        use crate::store::StoreError as S;
        let cases: Vec<(SessionError, u8)> = vec![
            (
                SessionError::Forbidden {
                    need: Need::History,
                },
                3,
            ),
            (SessionError::NoSuchChange(9), 4),
            (SessionError::Deleted, 4),
            (SessionError::NoBlobStore, 4),
            (SessionError::RateLimited { retry_after_ms: 1 }, 2),
            (
                SessionError::Held {
                    reason: "approvals_backlog",
                },
                2,
            ),
            (SessionError::NoLease, 2),
            (
                SessionError::Leased {
                    holder: "h".into(),
                    name: "H".into(),
                    expires_ms: 1,
                },
                5,
            ),
            (SessionError::Store(S::Locked), 6),
            // purge: the current blob version is refused (2); a purged one is gone (4)
            (SessionError::Store(S::CurrentVersion(1, 2)), 2),
            (SessionError::Store(S::PurgedVersion(1, 2)), 4),
            (SessionError::RevertTooOld { seq: 1 }, 8),
            (
                SessionError::Stale {
                    expected: 1,
                    actual: 2,
                },
                1,
            ),
            (SessionError::Unloaded, 1),
            (SessionError::BadApproval("x".into()), 2),
            (
                SessionError::NotJoined {
                    conn: crate::session::ConnId::fresh(),
                },
                2,
            ),
            (SessionError::Blob(BlobError::TooLarge { limit: 1 }), 4),
            (SessionError::Blob(BlobError::Damaged("ab".into())), 1),
            (SessionError::Blob(BlobError::Io("x".into())), 1),
            (SessionError::Blob(BlobError::NoRandom("x".into())), 1),
            // amendment 13: claims
            (
                SessionError::Claimed {
                    id: crate::session::ClaimId::new(1),
                    by: "b".into(),
                    name: "B".into(),
                    lines: (1, 2),
                    remaining_ms: 5,
                    note: None,
                },
                5,
            ),
            (SessionError::NoClaim(None), 4),
            (
                SessionError::NoClaim(Some(crate::session::ClaimId::new(3))),
                4,
            ),
            (
                SessionError::ClaimContended {
                    id: crate::session::ClaimId::new(1),
                    waiters: 1,
                    remaining_ms: 5,
                },
                2,
            ),
            (SessionError::BadClaim("x".into()), 2),
        ];
        for (e, code) in cases {
            assert_eq!(session_exit(&e), code, "{e:?}");
        }
        assert_eq!(exit::TOO_OLD, 8);
    }

    /// v0.3.1: everything that means "the thing you named doesn't exist" exits 4 (not 6),
    /// from the session, the store or the blob store; real store failures stay 6.
    #[test]
    fn missing_things_exit_4_and_store_failures_exit_6() {
        use crate::blob::BlobError;
        use crate::store::{DocKey, StoreError as S};
        let missing: Vec<SessionError> = vec![
            SessionError::NotFound(DocKey {
                space: "s".into(),
                path: "p".into(),
            }),
            SessionError::NoSuchChange(3),
            SessionError::NoApproval { seq: 3 },
            SessionError::NoBlobStore,
            SessionError::Deleted,
            SessionError::Blob(BlobError::NotFound("ab".into())),
        ];
        let missing_in_store = [
            S::NoDoc(1),
            S::NoVersion(1, 2),
            S::NoApproval(1, 2),
            S::NoBlobSpace,
            S::NoBlob("ab".into()),
            S::PurgedVersion(1, 2),
        ];
        let failures = [
            S::Db("disk full".into()),
            S::Newer { found: 9 },
            S::Corrupt("log entry"),
            S::KeyTaken(DocKey {
                space: "s".into(),
                path: "p".into(),
            }),
            S::Locked,
            S::NamedVersion(1, 2),
            S::KeyChange,
            S::LogMoved(1, 2, 3),
            S::NoRandom("x".into()),
        ];
        for e in &missing {
            assert_eq!(session_exit(e), exit::NOT_FOUND, "{e:?}");
        }
        for e in missing_in_store {
            assert_eq!(
                session_exit(&SessionError::Store(e.clone())),
                exit::NOT_FOUND,
                "Store({e:?})"
            );
            assert_eq!(
                session_exit(&SessionError::Blob(BlobError::Store(e.clone()))),
                exit::NOT_FOUND,
                "Blob(Store({e:?}))"
            );
        }
        for e in failures {
            assert_eq!(
                session_exit(&SessionError::Store(e.clone())),
                exit::STORE,
                "Store({e:?})"
            );
            assert_eq!(
                session_exit(&SessionError::Blob(BlobError::Store(e.clone()))),
                exit::STORE,
                "Blob(Store({e:?}))"
            );
        }
        assert_eq!(
            session_exit(&SessionError::Store(S::CurrentVersion(1, 2))),
            exit::USAGE
        );
    }
}
