//! What the host application provides: rights, the clock, and an audit sink.

use super::awareness::{ClaimEnd, ClaimId, ReservationEnd};
use super::participant::{ConnId, Participant};
use super::position::LineRange;
use crate::doc::ClientId;
use crate::store::{DocKey, Seq};

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
    /// The doc's owner (the host decides who: e.g. the workspace owner and admins):
    /// breaking someone else's tier-0 lease, purge and participant erasure. **Never implied** by any other right, not even
    /// [`Rights::full`]: grant it with [`Rights::with_owner`]. Meaningless without `read`.
    pub owner: bool,
}

impl Rights {
    /// No access.
    pub const fn none() -> Self {
        Self {
            read: false,
            edit: false,
            history: false,
            owner: false,
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
            ..Self::none()
        }
    }

    /// Read, edit and history (not owner).
    pub const fn full() -> Self {
        Self {
            read: true,
            edit: true,
            history: true,
            owner: false,
        }
    }

    /// The same rights with `history` set or cleared.
    pub const fn with_history(self, history: bool) -> Self {
        Self { history, ..self }
    }

    /// The same rights with `owner` set or cleared.
    pub const fn with_owner(self, owner: bool) -> Self {
        Self { owner, ..self }
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

    /// Tier 0: the folder for conflict copies of `file` (its real path, inside its root),
    /// e.g. a folder the host keeps out of version control. The kit creates it if missing.
    /// Default `None`: beside the file. Called with the session locked, when a copy is about
    /// to be written: quick, no I/O needed (the kit does the I/O).
    fn conflict_dir(&self, file: &std::path::Path) -> Option<std::path::PathBuf> {
        let _ = file;
        None
    }
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
    /// [`Rights::owner`]: breaking someone else's lease, purging.
    Owner,
    /// `revert <change>`: only the change's author, the person it was made for
    /// ([`super::Participant::on_behalf_of`]), or an owner ([`Rights::owner`]) may revert it.
    /// The host can say so: "only the author, the person it was made for, or an owner can
    /// revert this".
    AuthorOrOwner,
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
    /// [`crate::session::Session::set_approval`], [`crate::session::BlobSession::set_approval`].
    Approve,
    /// [`crate::session::Session::approval`] / [`crate::session::Session::approvals`], and
    /// the [`crate::session::BlobSession`] calls of those names.
    ReadApproval,
    /// [`crate::session::Session::flush`].
    Flush,
    /// Tier 0: acquiring, renewing or releasing a lease.
    Lease,
    /// Taking, renewing, releasing or waiting on a claim (amendment 13).
    Claim,
    /// [`crate::session::Session::activity`].
    Activity,
    /// Tier 0: breaking someone else's lease (owner only).
    BreakLease,
    /// Tier 0: restoring an older version.
    Restore,
    /// Listing or reading versions, naming one (tier 0 and text docs).
    Versions,
    /// `revert <change>`.
    Revert,
    /// `log`, `diff --version`, `show --version`.
    History,
    /// `purge` and participant erasure (owner only).
    Purge,
    /// `blame` (text and tier 0).
    Blame,
}

/// What a purge targeted, as [`AuditEvent::Purged`] records it: metadata only (never the
/// text, never a hash of it; an erased pid appears only as its pseudonym).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PurgedScope {
    /// The whole history before the purge.
    History,
    /// Changes `from..=to`.
    Changes {
        from: crate::store::Seq,
        to: crate::store::Seq,
    },
    /// Versions `from..=to`.
    Versions {
        from: crate::store::VersionId,
        to: crate::store::VersionId,
    },
    /// A text the owner named, of `bytes` bytes.
    Text { bytes: usize },
    /// Participant erasure, to `pseudonym`.
    Participant { pseudonym: String },
    /// Tier 0: blob versions (`None`: every one but the current).
    BlobVersions {
        version: Option<crate::store::VersionId>,
    },
}

/// How a tier-0 lease ended ([`AuditEvent::LeaseEnded`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeaseEnd {
    /// The holder released it (or the write that took it automatically finished).
    Released,
    /// It ran out on the host's clock.
    Expired,
    /// An owner broke it ([`AuditEvent::LeaseBroken`] carries who).
    Broken,
    /// The holder was kicked or lost read.
    HolderRemoved,
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
    /// The file was edited outside the kit; the edit was merged in as change `seq` (by
    /// [`crate::session::EXTERNAL_PID`]). For a tier-0 blob doc, `seq` is the version the
    /// save became.
    ExternalEdit { key: DocKey, seq: crate::store::Seq },
    /// The file is gone (deleted, replaced by a folder...). Write-back stops; the doc is
    /// kept. `unflushed`: it had changes the file never got. Tell participants.
    FileMissing { key: DocKey, unflushed: bool },
    /// The file can't be a live doc any more (`code` as in
    /// [`crate::session::Declined`]). Write-back stops and edits are refused. Tell
    /// participants.
    FileDeclined { key: DocKey, code: &'static str },
    /// The file is back (after missing or declined).
    FileRestored { key: DocKey },
    /// The file came back holding one of the kit's own recent writes, made since the last
    /// outside save (an editor saved a stale buffer, or a late echo): it was not taken as an
    /// edit, and the next write-back puts the doc back (v0.9.5; before, suppressed silently).
    EchoSuppressed { key: DocKey },
    /// Reading or writing the file failed (write-back retries).
    DiskError { key: DocKey, error: String },
    /// The person `pid` settled the approval of on-behalf change `seq`. For a tier-0 blob
    /// doc, `seq` is the version.
    Approval {
        key: DocKey,
        seq: crate::store::Seq,
        pid: String,
        state: crate::store::ApprovalState,
    },
    /// Tier 0: `pid` took (or renewed) the doc's lease until `expires_ms`. `auto`: taken for
    /// the length of one write.
    LeaseAcquired {
        key: DocKey,
        pid: String,
        expires_ms: u64,
        auto: bool,
    },
    /// Tier 0: `pid`'s lease ended.
    LeaseEnded {
        key: DocKey,
        pid: String,
        how: LeaseEnd,
    },
    /// Tier 0: the owner `by` broke `holder`'s lease (which ran until `expires_ms`).
    LeaseBroken {
        key: DocKey,
        by: String,
        holder: String,
        expires_ms: u64,
    },
    /// Tier 0: the file was saved by another program while the lease was held or while the
    /// doc had a version the file didn't have yet. Nothing is lost: the file's content is
    /// version `current` (by [`crate::session::EXTERNAL_PID`]), and the content that was
    /// current before is version `kept`, also written to the conflict copy `copy` (its path
    /// relative to the file's folder: a plain file name when beside it, or a path into
    /// [`Host::conflict_dir`]'s folder; `None` if writing it failed). `lease_holder` is who
    /// held the lease, if anyone. Tell both parties.
    BlobConflict {
        key: DocKey,
        current: crate::store::VersionId,
        kept: crate::store::VersionId,
        copy: Option<String>,
        lease_holder: Option<String>,
    },
    /// Tier 0.5: a conflicting save was merged by the pack ([`crate::session::BlobPack`]);
    /// the merge is version `version`.
    BlobMerged {
        key: DocKey,
        version: crate::store::VersionId,
    },
    /// [`crate::session::SessionManager::recheck_rights`]: re-checking this loaded doc
    /// failed (it was skipped; the others were re-checked). `error` is the session error's
    /// message.
    RecheckFailed { key: DocKey, error: String },
    /// [`crate::session::SessionManager::delete_doc`] / `delete_space`: the doc was deleted
    /// while loaded. Its session closed: `connections` (each with its pid) were in it and are
    /// gone, with their subscriptions and any lease; later calls on the session fail with
    /// [`crate::session::SessionError::Deleted`]. Tell those participants.
    DocDeleted {
        key: DocKey,
        connections: Vec<(ConnId, String)>,
    },
    /// The tier-0 blob doc at `key` became a text doc, keeping its key and its history
    /// (v0.9.2, [`crate::session::SessionManager::promote_blob_file`]). Its blob session
    /// closed: `connections` (each with its pid) were in it and are gone; later calls on it
    /// fail with [`crate::session::SessionError::Unloaded`]. Tell those tabs to open the
    /// text doc. `text_versions` blob versions became text versions (through `changes` log
    /// changes); `tier0_kept` stay as the doc's tier-0 history (non-text or purged).
    Promoted {
        key: DocKey,
        connections: Vec<(ConnId, String)>,
        text_versions: u64,
        changes: u64,
        tier0_kept: u64,
    },
    /// The edit path's runtime check (v0.9.3, [`crate::session::Config::edit_check_every`])
    /// found that change `change`'s update, made on the doc as `client`, differed from the
    /// same edit made through a throwaway replica. The replica's update was applied (never
    /// the other), and the doc uses that path until it is reloaded. Metadata only. A bug in
    /// the engine: report it.
    EditPathMismatch {
        key: DocKey,
        client: ClientId,
        change: Seq,
    },
    /// The runtime check found the engine's cached text of `root` different from a fresh
    /// read, at change `change` (checked before and after it). The fresh text was used, and
    /// the doc reads its texts afresh until it is reloaded. Metadata only.
    TextCacheMismatch {
        key: DocKey,
        root: String,
        change: Seq,
    },
    /// `pid` took claim `claim` on `lines` until `expires_ms` (amendment 13). `for_edit`:
    /// implicitly, by a read verb's `--for-edit`. Metadata only (the claim's note is not
    /// here: read it with [`crate::session::Session::claims`]).
    ClaimTaken {
        key: DocKey,
        claim: ClaimId,
        pid: String,
        conn: ConnId,
        lines: LineRange,
        expires_ms: u64,
        hold: bool,
        for_edit: bool,
    },
    /// `pid`'s claim `claim` ended (`how`). `waiters` were waiting for it, first in line
    /// first; `next_waiter` is the first of them. A host that holds agents' edits until a
    /// claim ends ([`crate::session::ClaimPolicy::Respect`] plus its own wait) wakes them
    /// from this event, in order: it never needs to poll. Called with the session locked,
    /// like every event: don't call back into the kit from it.
    ///
    /// `lines` (amendment 14) is the claim's range as it ends: its current lines, after the
    /// edits made while it was live moved its anchors (as `ClaimTaken`'s, 1-based,
    /// inclusive). `version` is the doc's version as it ends: the same number reads report
    /// (`ReadResult::version`, `ToolOutput::version`) and `--if-version` compares, so "lines
    /// X–Y are free; the doc is at `version`; re-read" and the agent's next guarded edit
    /// agree.
    ClaimEnded {
        key: DocKey,
        claim: ClaimId,
        pid: String,
        how: ClaimEnd,
        lines: LineRange,
        version: crate::store::Seq,
        next_waiter: Option<(ConnId, String)>,
        waiters: Vec<(ConnId, String)>,
    },
    /// Claim `claim`'s range is reserved for the waiter `pid` (`conn`) until `until_ms`
    /// ([`crate::session::Config::claim_handoff_ms`]): granted to the first waiter when the
    /// claim ends, and passed to the next when a reservation is used by an edit, lapses, is
    /// cancelled or its holder leaves. Wake `conn`'s held edit on this. `waiters`: the rest
    /// of the queue, in order.
    Reserved {
        key: DocKey,
        claim: ClaimId,
        pid: String,
        conn: ConnId,
        lines: LineRange,
        until_ms: u64,
        waiters: Vec<(ConnId, String)>,
    },
    /// The reservation of claim `claim`'s range for `pid` ended (`how`). Unless `how` is
    /// `Claimed` or `Reset`, a `Reserved` for `next_waiter` follows when there is one.
    /// `lines` and `version` (amendment 14): the reserved range's current lines and the
    /// doc's version as it ends, as in `ClaimEnded`.
    ReservationEnded {
        key: DocKey,
        claim: ClaimId,
        pid: String,
        how: ReservationEnd,
        lines: LineRange,
        version: crate::store::Seq,
        next_waiter: Option<(ConnId, String)>,
    },
    /// The owner `by` purged `scope` from the doc (K-PURGE-1): what changed, as counts. No
    /// content and no content hash, ever.
    Purged {
        key: DocKey,
        by: String,
        scope: PurgedScope,
        counts: crate::store::PurgeCounts,
    },
}
