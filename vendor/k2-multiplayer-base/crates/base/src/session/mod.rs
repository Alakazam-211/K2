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

mod config;
mod error;
mod host;
mod live;
mod manager;
mod pack;
mod participant;
pub mod position;
mod ratelimit;
mod summary;
mod undo;

pub use crate::store::{Approval, ApprovalState};
pub use config::Config;
pub use error::{EditWithError, SessionError};
pub use host::{Action, AuditEvent, Host, LeaveReason, Need, Rights};
pub use live::{
    ChangeEvent, EditResult, Joined, ReadResult, RightsChange, Session, Status, UndoOutcome,
    WriteOutcome,
};
pub use manager::{PoolStats, SessionManager};
pub use pack::{Declined, Pack, PackContent, PackError, TextFilePack};
pub use participant::{
    ConnId, CursorAt, Participant, ParticipantKind, Role, RosterEntry, Stay, EXTERNAL_PID,
    IMPORT_PID, RESERVED_PID_PREFIX,
};
pub use position::{At, Cursor, LineCol, LineRange};

// Compile-time check: sessions and the manager are shared across threads.
const _: fn() = || {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Session>();
    send_sync::<SessionManager>();
};
