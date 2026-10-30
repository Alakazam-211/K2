//! Sessions: a loaded doc with people and agents in it.
//!
//! The contract a host codes against is in `docs/SESSIONS.md`. In short:
//!
//! - The host implements [`Host`] (rights, clock, audit) and each file type a [`Pack`]
//!   (schema and codec; [`TextFilePack`] covers plain-text types).
//! - A [`SessionManager`] owns one [`crate::store::DocStore`] and the loaded [`Session`]s:
//!   [`SessionManager::open`] loads a stored doc or imports a file.
//! - Participants are host-stamped ([`Participant`]). Each joins through one or more
//!   connections ([`ConnId`]: two tabs, a CLI call during a `watch`), each with its own fresh
//!   client ID, so every change is attributed to the participant.
//! - Agents and people send **text edits**: [`Session::edit_with`] (read live, compute,
//!   apply atomically), [`Session::edit`] with an `if_version` guard, or
//!   [`Session::write_base`]. The kit makes every engine update itself. A change is durable
//!   in the store before the call returns.
//! - Rights are per participant and asked of the host on every call; edit again on every
//!   edit. Undo and the rate limit are per participant too.
//! - Presence: roles, carets and viewing ranges carried through every change, idle expiry,
//!   per-participant undo/redo, change subscriptions, rate limits, a memory-bounded pool.
//! - Agent presence (amendment 13): anchored, expiring claims with FIFO waiters, a
//!   host-chosen [`ClaimPolicy`] per edit, heads-up [`Notes`], and an activity ring. Memory
//!   only: never in the store, the log or the engine.
//! - The roster pushed to subscribers ([`Session::subscribe_roster`]): a snapshot, then
//!   coalesced deltas ([`RosterEvent`]).

mod awareness;
mod carry;
mod config;
mod disk;
#[cfg(feature = "fuzzing")]
pub(crate) use disk::fuzz_file_stat;
mod error;
mod foreign;
mod handles;
mod host;
mod live;
mod manager;
mod pack;
mod participant;
pub mod position;
mod presence;
mod prov;
mod ratelimit;
pub mod rebase;
mod roster;
mod summary;
mod tier0;
mod undo;

pub use crate::store::{Approval, ApprovalState};
pub use crate::store::{ChangeInfo, ChangeKind, VersionInfo};
pub(crate) use awareness::pair_text;
pub use awareness::{
    ActivityEntry, ActivityKind, ClaimEnd, ClaimId, ClaimInfo, ClaimPolicy, ForEdit, Note,
    NoteKind, Notes, Overwrite, OverwriteCell, ReservationEnd, ReservationInfo, Waiting,
    OVERWRITE_CELLS_MAX, OVERWRITE_DOC_TEXT_MAX, OVERWRITE_PENDING_MAX, OVERWRITE_TEXT_MAX,
    OVERWRITE_VALUE_MAX,
};
pub use config::{
    Config, DEFAULT_BLAME_CHECK_EVERY, DEFAULT_BLAME_CHECK_EVERY_LARGE,
    DEFAULT_BLAME_CHECK_LARGE_AT,
};
pub use disk::{file_identity, is_write_temp, DiskCheck, DiskState, FileIdentity, FlushOutcome};
pub use error::{EditWithError, SessionError};
pub use host::{Action, AuditEvent, Host, LeaseEnd, LeaveReason, Need, PurgedScope, Rights};
#[cfg(feature = "conformance")]
pub use live::CharBlame;
pub(crate) use live::Shown;
pub use live::{
    Blame, BlameLine, ChangeEvent, DiffOutput, EditAtBase, EditOptions, EditResult, FileBytes,
    FileCodec, FileCopy, Joined, LogItem, LogQuery, NotReached, PurgeOutcome, PurgeRequest,
    PurgeScope, ReadResult, RightsChange, Session, Status, SyncDrift, UndoMode, UndoOutcome,
    UndoRequest, WriteOutcome, WriteReport,
};
pub use manager::{PoolStats, SessionManager, TickReport};
pub use pack::{Declined, Pack, PackContent, PackError, TextFilePack};
pub use participant::{
    ConnId, CursorAt, Participant, ParticipantKind, Role, RosterEntry, Stay, EXTERNAL_PID,
    IMPORT_PID, RESERVED_PID_PREFIX, UNKNOWN_PID,
};
pub use position::{At, Cursor, LineCol, LineRange};
pub use roster::{RosterEvent, RosterMember, Who};
pub use tier0::{
    conflict_copy_name, AnyFile, BlobApproval, BlobAuthor, BlobBlame, BlobLogEntry, BlobLogKind,
    BlobMerge, BlobPack, BlobRead, BlobSession, BlobStatus, BlobWriteResult, ConflictCopy,
    LeaseInfo, Promoted, Promotion, MERGE_PID, REGION_MAX_BYTES,
};
pub use undo::{ConflictKind, ConflictRegion};

#[cfg(test)]
mod disk_crash_tests;
#[cfg(test)]
mod engine_check_tests;
#[cfg(test)]
mod manager_tests;
#[cfg(test)]
mod one_writer_tests;
#[cfg(test)]
mod promote_crash_tests;
#[cfg(test)]
mod tier0_crash_tests;
#[cfg(test)]
mod writeback_cost_tests;
#[cfg(test)]
mod writeback_diff_tests;

// Compile-time check: sessions and the manager are shared across threads.
const _: fn() = || {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Session>();
    send_sync::<BlobSession>();
    send_sync::<SessionManager>();
};
