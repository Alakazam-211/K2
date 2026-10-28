//! Session errors.

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
    /// `edit(.., if_version: Some(expected))` but the doc is at `actual`. Nothing applied.
    #[error("doc moved: expected version {expected}, it is at {actual}")]
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
    /// made on anyone's behalf (or doesn't exist).
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
