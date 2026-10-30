//! Session errors.

use super::awareness::ClaimId;
use super::host::Need;
use super::pack::{Declined, PackError};
use super::participant::ConnId;
use crate::doc::DocError;
use crate::store::{DocKey, Seq, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// The participant lacks a right. When it was read, they were also removed from the doc.
    #[error("refused: needs {need:?} rights")]
    Forbidden { need: Need },
    /// The connection isn't (or is no longer) in this doc: it left, was kicked or expired,
    /// or belongs to another session. Join again.
    #[error("{conn} has not joined this doc")]
    NotJoined { conn: ConnId },
    /// The connection joined past [`super::Config::participants_max`] (K11): it watches,
    /// read-only, until a slot frees and the host promotes it
    /// ([`super::Session::promote_watcher`]). Nothing was applied. Exit 3.
    #[error("watching: the doc has its {max} participants; read-only until the host lets you in")]
    Watching { max: usize },
    /// `edit(.., if_version: Some(expected))` (or an edit verb's `--if-version`, or a tier 0
    /// write from an old base) but the doc is at `actual`. Nothing applied. One line, no doc
    /// text: `doc changed since 12 (now 14)`. Exit 1.
    #[error("doc changed since {expected} (now {actual})")]
    Stale { expected: Seq, actual: Seq },
    #[error("too many edits; retry in {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    /// The file can't be a live doc, or live editing was turned off for it.
    #[error("live editing is off: {0}")]
    Declined(Declined),
    /// No stored doc at this key, and no file bytes to create one from.
    #[error("no doc at {0:?}")]
    NotFound(DocKey),
    #[error("doc was stored by pack {stored:?}, not {pack:?}")]
    PackMismatch { stored: String, pack: String },
    #[error("bad participant: {0}")]
    BadParticipant(String),
    #[error("bad position: {0}")]
    BadPosition(String),
    #[error("bad edit: {0}")]
    BadEdit(String),
    /// [`super::Session::set_approval`] / [`super::Session::approval`]: change `seq` was not
    /// made on anyone's behalf (or doesn't exist). From
    /// [`super::BlobSession::set_approval`], `seq` is the blob version.
    #[error("change {seq} has no approval (it wasn't made on anyone's behalf)")]
    NoApproval { seq: Seq },
    /// An approval can only go from pending to approved or declined, once.
    #[error("bad approval: {0}")]
    BadApproval(String),
    /// [`super::SessionManager::open_file`] / [`super::Session::file_moved`]: the path isn't
    /// a regular file inside the root (a folder, a symlink resolving outside it...).
    #[error("bad path: {0}")]
    BadPath(String),
    /// Refused for now, not for good: `approvals_backlog` (too many pending on-behalf
    /// changes; the person approves or declines some first). Exit 2.
    #[error("held: {reason}")]
    Held { reason: &'static str },
    /// `revert`: the change is older than the revert window (K-HIST-23). Exit 8; never
    /// retry. The way back is `restore --version`.
    #[error("change {seq} is too old to revert (revert_too_old); use restore --version")]
    RevertTooOld { seq: Seq },
    /// No change at that log position (`revert`), or no such version. Exit 4.
    #[error("no change or version {0} in this doc")]
    NoSuchChange(u64),
    /// Reading or writing the file failed. Nothing is lost: the doc keeps its changes and
    /// write-back retries.
    #[error("disk: {0}")]
    Disk(String),
    /// Tier 0: someone else holds the doc's lease (until `expires_ms`, host clock). Nothing
    /// was written. Suggested exit **5** (`leased`), printing the holder and expiry.
    #[error("leased by {name} ({holder}) until {expires_ms}")]
    Leased {
        holder: String,
        name: String,
        expires_ms: u64,
    },
    /// Tier 0: renewing a lease the caller doesn't hold (none is held).
    #[error("no lease held")]
    NoLease,
    /// Another participant (`by`) holds a live claim on these lines (amendment 13): a new
    /// overlapping claim, or an agent's edit under [`super::ClaimPolicy::Respect`]. Nothing
    /// was applied. Exit **5**, as `Leased`: someone else holds it for now; retry after
    /// `remaining_ms`, or wait ([`super::Session::register_waiter`]). `lines` are the
    /// claim's lines now (1-based, inclusive) in the root the call named.
    #[error("{} claimed by {name} ({by}){}; {remaining_ms} ms left",
        crate::session::awareness::pair_text(*.lines),
        .note.as_deref().map(|n| format!(" \"{}\"", crate::session::awareness::one_line(n))).unwrap_or_default())]
    Claimed {
        id: ClaimId,
        by: String,
        name: String,
        lines: (usize, usize),
        remaining_ms: u64,
        note: Option<String>,
    },
    /// No live claim by that id (it ended, or never existed here), or, with no id, the
    /// caller holds none. Exit 4.
    #[error("{}", match .0 { Some(id) => format!("no live claim {id}"), None => "you hold no claim here".to_owned() })]
    NoClaim(Option<ClaimId>),
    /// Renewing a claim, or extending it with `hold`, while other participants wait for it
    /// (anti-starvation): refused; the claim still ends normally. Exit 2.
    #[error("{id} has {waiters} waiting; it can't be renewed or held ({remaining_ms} ms left)")]
    ClaimContended {
        id: ClaimId,
        waiters: usize,
        remaining_ms: u64,
    },
    /// A claim call the kit refuses: not your claim, waiting on your own claim, a zero TTL,
    /// too many claims, several claims and no id. Exit 2.
    #[error("bad claim: {0}")]
    BadClaim(String),
    /// Tier 0: the blob store failed (or a blob is damaged; it is never served).
    #[error(transparent)]
    Blob(#[from] crate::blob::BlobError),
    /// Tier 0: the manager has no blob store ([`super::SessionManager::with_blob_store`]).
    #[error("no blob store configured")]
    NoBlobStore,
    #[error(transparent)]
    Doc(#[from] DocError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Pack(#[from] PackError),
    /// This session handle was unloaded from the pool; open the doc again.
    #[error("session unloaded; open the doc again")]
    Unloaded,
    /// The manager was released (process hand-off).
    #[error("sessions released")]
    Released,
    /// The doc was deleted ([`super::SessionManager::delete_doc`] / `delete_space`) while
    /// this session was loaded. Exit 4.
    #[error("the doc was deleted")]
    Deleted,
    /// A store write failed and reloading from the store failed too. The session is closed;
    /// open the doc again once the store is healthy.
    #[error("session failed: {0}")]
    Broken(String),
}

pub type Result<T, E = SessionError> = std::result::Result<T, E>;

/// Error from [`super::Session::edit_with`]: the session refused or failed, or the caller's
/// own computation returned an error (nothing applied either way).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditWithError<E> {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("{0}")]
    Compute(E),
}

impl SessionError {
    /// This error with every absolute path in its text made safe to show (K7, v0.9.8):
    /// [`crate::errors::redact`] over `root` (inside it, relative; elsewhere, `<outside>`).
    /// The variant, and with it the id and exit code, doesn't change.
    pub fn redacted(self, root: &std::path::Path) -> SessionError {
        use crate::blob::BlobError;
        use crate::errors::redact as r;
        use crate::store::StoreError;
        let store = |e: StoreError| match e {
            StoreError::Db(m) => StoreError::Db(r(&m, root)),
            other => other,
        };
        match self {
            SessionError::BadPath(m) => SessionError::BadPath(r(&m, root)),
            SessionError::Disk(m) => SessionError::Disk(r(&m, root)),
            SessionError::Broken(m) => SessionError::Broken(r(&m, root)),
            SessionError::Declined(d) => SessionError::Declined(Declined {
                message: r(&d.message, root),
                ..d
            }),
            SessionError::Pack(p) => SessionError::Pack(PackError(r(&p.0, root))),
            SessionError::Store(e) => SessionError::Store(store(e)),
            SessionError::Blob(BlobError::Io(m)) => SessionError::Blob(BlobError::Io(r(&m, root))),
            SessionError::Blob(BlobError::Store(e)) => {
                SessionError::Blob(BlobError::Store(store(e)))
            }
            other => other,
        }
    }
}
