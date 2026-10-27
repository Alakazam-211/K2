//! What the host application provides: rights, the clock, and an audit sink.

use super::participant::{ConnId, Participant};
use crate::doc::ClientId;
use crate::store::DocKey;

/// What a participant may do with one doc, as the host decides it.
///
/// `#[non_exhaustive]`: build it with the constructors ([`Rights::none`],
/// [`Rights::read_only`], [`Rights::editor`], [`Rights::full`]) and adjust with
/// [`Rights::with_history`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Rights {
    /// Read the doc, be present in it, subscribe. Every call needs it.
    pub read: bool,
    /// Change the doc (and place a caret). Meaningless without `read`.
    pub edit: bool,
    /// Read its history: versions, log, diff, show, blame (C7a), and the approval state of
    /// other people's on-behalf changes. **Never implied** by read or edit: grant it
    /// explicitly. Meaningless without `read`.
    pub history: bool,
}

impl Rights {
    /// No access.
    pub const fn none() -> Self {
        Self {
            read: false,
            edit: false,
            history: false,
        }
    }

    /// Read only. History is not included; add it with [`Rights::with_history`].
    pub const fn read_only() -> Self {
        Self {
            read: true,
            ..Self::none()
        }
    }

    /// Read and edit (editing without reading is not a thing). No history.
    pub const fn editor() -> Self {
        Self {
            read: true,
            edit: true,
            history: false,
        }
    }

    /// Read, edit and history.
    pub const fn full() -> Self {
        Self {
            read: true,
            edit: true,
            history: true,
        }
    }

    /// The same rights with `history` set or cleared.
    pub const fn with_history(self, history: bool) -> Self {
        Self { history, ..self }
    }

    /// Same as [`Rights::none`] (kept from before amendment 1).
    pub const NONE: Rights = Rights::none();
    /// Same as [`Rights::read_only`] (kept from before amendment 1).
    pub const READ: Rights = Rights::read_only();
    /// Same as [`Rights::editor`] (kept from before amendment 1).
    pub const EDIT: Rights = Rights::editor();
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
#[non_exhaustive]
pub enum Need {
    Read,
    Edit,
    /// [`Rights::history`]: reading history, or someone else's approval state.
    History,
    /// Not a right the host grants: only the person a change was made on behalf of
    /// ([`super::Participant::on_behalf_of`]) may approve or decline it.
    Approver,
}

/// The call that was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
    /// [`crate::session::Session::set_approval`].
    Approve,
    /// [`crate::session::Session::approval`] / [`crate::session::Session::approvals`].
    ReadApproval,
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
    /// The person `pid` settled the approval of on-behalf change `seq`.
    Approval {
        key: DocKey,
        seq: crate::store::Seq,
        pid: String,
        state: crate::store::ApprovalState,
    },
}
