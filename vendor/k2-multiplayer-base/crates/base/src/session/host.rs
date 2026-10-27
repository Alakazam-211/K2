//! What the host application provides: rights, the clock, and an audit sink.

use super::participant::{ConnId, Participant};
use crate::doc::ClientId;
use crate::store::DocKey;

/// What a participant may do with one doc, as the host decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rights {
    pub read: bool,
    pub edit: bool,
}

impl Rights {
    pub const NONE: Rights = Rights {
        read: false,
        edit: false,
    };
    pub const READ: Rights = Rights {
        read: true,
        edit: false,
    };
    /// Read and edit (editing without reading is not a thing).
    pub const EDIT: Rights = Rights {
        read: true,
        edit: true,
    };
}

/// The host application, as the kit sees it. Identity, permission and time belong to the
/// host; the kit never decides them itself.
///
/// **Called with a session locked.** Implementations must be quick and must not call back
/// into the kit (a session or the manager): that would deadlock.
pub trait Host: Send + Sync {
    /// Current rights of `who` on the doc at `key`. Asked at join and on **every** call (read
    /// for every call, edit for every edit), and by [`crate::session::Session::recheck_rights`].
    fn rights(&self, who: &Participant, key: &DocKey) -> Rights;

    /// Milliseconds on the host's clock (wall or monotonic; only differences matter, except
    /// that `at_ms` in the log and versions is stored as given). Injectable for tests.
    fn now_ms(&self) -> u64;

    /// An audit or operational event. Metadata only: the kit never puts doc content here.
    fn on_event(&self, ev: &AuditEvent);
}

/// The right a refused call needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Read,
    Edit,
}

/// The call that was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Join,
    Touch,
    Read,
    Roster,
    View,
    Cursor,
    Subscribe,
    Edit,
    WriteBase,
    Undo,
    Redo,
}

/// Why a participant left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveReason {
    /// [`crate::session::Session::leave`].
    Left,
    /// Idle past [`crate::session::Config::idle_leave_ms`] without a hold.
    Idle,
    /// [`crate::session::Session::kick`] by the host (all of the pid's connections leave).
    Kicked,
    /// The host stopped granting read (all of the pid's connections leave).
    RightsLost,
}

/// Something the host may want to audit or log. Never carries doc content.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuditEvent {
    /// A connection joined (one pid may hold several).
    Joined {
        key: DocKey,
        pid: String,
        conn: ConnId,
        client: ClientId,
    },
    /// A connection left.
    Left {
        key: DocKey,
        pid: String,
        conn: ConnId,
        reason: LeaveReason,
    },
    /// A call was refused for lack of a right.
    Refused {
        key: DocKey,
        pid: String,
        action: Action,
        need: Need,
    },
    /// Compaction failed. Nothing is lost (the log is intact); the store just stays larger.
    CompactFailed { key: DocKey, error: String },
    /// A store write failed mid-edit. The session reloaded from the store; carets, viewing
    /// ranges and undo history were reset.
    Recovered { key: DocKey, error: String },
}
