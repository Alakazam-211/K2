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
    /// Leased by someone else (tier 0).
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
pub fn session_exit(e: &SessionError) -> u8 {
    use crate::blob::BlobError;
    use crate::store::StoreError;
    match e {
        SessionError::Forbidden { .. } => exit::REFUSED,
        SessionError::NotFound(_)
        | SessionError::Declined(_)
        | SessionError::PackMismatch { .. }
        | SessionError::NoSuchChange(_)
        | SessionError::NoBlobStore
        | SessionError::Deleted
        | SessionError::Store(StoreError::PurgedVersion(..))
        | SessionError::Blob(BlobError::TooLarge { .. }) => exit::NOT_FOUND,
        SessionError::NotJoined { .. }
        | SessionError::RateLimited { .. }
        | SessionError::BadParticipant(_)
        | SessionError::BadPosition(_)
        | SessionError::BadEdit(_)
        | SessionError::NoApproval { .. }
        | SessionError::BadApproval(_)
        | SessionError::BadPath(_)
        | SessionError::NoLease
        | SessionError::Store(StoreError::CurrentVersion(..))
        | SessionError::Doc(_) => exit::USAGE,
        SessionError::Held { .. } => exit::HELD,
        SessionError::Leased { .. } => exit::LEASED,
        SessionError::Store(_) => exit::STORE,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Need, SessionError};

    /// K-EXIT-1: one table. `session_exit` lists every variant (a new one doesn't compile
    /// until mapped); this pins the codes.
    #[test]
    fn exit_code_table_is_total() {
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
            (SessionError::Store(crate::store::StoreError::Locked), 6),
            // purge: the current blob version is refused (2); a purged one is gone (4)
            (
                SessionError::Store(crate::store::StoreError::CurrentVersion(1, 2)),
                2,
            ),
            (
                SessionError::Store(crate::store::StoreError::PurgedVersion(1, 2)),
                4,
            ),
            (SessionError::RevertTooOld { seq: 1 }, 8),
            (
                SessionError::Stale {
                    expected: 1,
                    actual: 2,
                },
                1,
            ),
            (SessionError::Unloaded, 1),
        ];
        for (e, code) in cases {
            assert_eq!(session_exit(&e), code, "{e:?}");
        }
        assert_eq!(exit::TOO_OLD, 8);
    }
}
