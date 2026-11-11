//! **Annotations** (slice A4): comment threads pinned to anchors, per doc
//! (`docs/SESSIONS.md`, "Annotations").
//!
//! One generic table set, keyed by doc and thread, so any doc kind can carry comments: the
//! table, its operations, rights, history, events and purge never look at what an anchor
//! points at ([`crate::anchor::AnchorTarget`]); a new target needs only its resolver. Blob
//! docs (tier 0) have them now ([`crate::session::BlobSession::comment`] and the calls
//! beside it).
//!
//! - **Outside the doc's versions and undo.** A comment changes no content: it makes no
//!   version and no undo step, and an agent's comment on someone's behalf starts no approval.
//! - **One copy of each body.** A comment isn't editable (a correction is a reply or a
//!   delete), and the history ([`AnnotEvent`]) never holds a body. Deleting a comment clears
//!   its body at once, with no undo; purge ([`AnnotPurge`]) and participant erasure clear
//!   bodies and rewrite the store file, so no byte is left.
//! - **Never moved silently.** Only [`crate::session::BlobSession::reanchor`] changes where a
//!   thread points, and only a person's applies: an agent's is a proposal
//!   ([`PendingAnchor`]) a person confirms.
//! - **The host renders and notifies.** The kit stores [`Comment::mentions`] and
//!   [`Comment::attachments`] as data and emits [`crate::session::AuditEvent::Annotation`]
//!   (no content); resolving names to pids, notifications and showing a proposal are the
//!   host's.

use std::fmt;

use crate::anchor::{Anchor, AnchorResolution, AnchorState, AnchorTarget};
use crate::store::VersionId;

pub(crate) mod rec;
pub mod render;
mod verbs;

pub use verbs::{AnnotCall, AnnotOutput};

/// Most mentions on one comment.
pub const ANNOT_MENTIONS_MAX: usize = 32;
/// Longest mentioned pid, in bytes; also the longest participant id (and erasure pseudonym)
/// a comment write takes (`MPC-E115` past it: the host's to fix).
pub const ANNOT_PID_MAX: usize = 256;
/// Longest display name or surface stored in an attribution, in bytes: a longer one is cut
/// at a character boundary.
pub const ANNOT_NAME_MAX: usize = 256;
/// Most attachments on one comment.
pub const ANNOT_ATTACHMENTS_MAX: usize = 8;
/// Longest attachment target, in bytes (opaque to the kit: a proposal's path, for one).
pub const ANNOT_ATTACHMENT_MAX: usize = 512;
/// Threads in one page of [`crate::session::BlobSession::comments`] when the query names no
/// limit, and the most it takes.
pub const ANNOT_PAGE_DEFAULT: usize = 50;
pub const ANNOT_PAGE_MAX: usize = 200;
/// A page stops adding threads once its bodies pass this many bytes (it always holds one).
pub const ANNOT_PAGE_BYTES: usize = 1 << 20;
/// Most history rows one [`crate::session::BlobSession::annotation_log`] call returns.
pub const ANNOT_LOG_MAX: usize = 1_000;
/// How many versions back a comment's anchor looks for an index when the version asked for
/// has none yet ([`AnchorResolution::current`] `false`).
pub const ANNOT_RESOLVE_LOOKBACK: usize = 32;

/// A thread's id: per doc, never reused. Displays `ann-12`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadId(pub(crate) u64);

impl ThreadId {
    pub fn get(self) -> u64 {
        self.0
    }

    /// `ann-12` or `12`.
    pub fn parse(s: &str) -> Option<Self> {
        let n = s.strip_prefix("ann-").unwrap_or(s);
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || n.starts_with('0') {
            return None;
        }
        n.parse().ok().map(ThreadId)
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ann-{}", self.0)
    }
}

/// Who did something, as the host stamped the connection ([`crate::session::Participant`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Attribution {
    pub pid: String,
    pub name: String,
    /// The surface (`tab`, `cli`...).
    pub via: String,
    /// The person an agent acted for.
    pub on_behalf_of: Option<String>,
    /// The host's clock.
    pub at_ms: u64,
}

/// Open, or resolved by someone.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThreadStatus {
    Open,
    Resolved { by: Attribution },
}

impl ThreadStatus {
    pub fn is_open(&self) -> bool {
        matches!(self, ThreadStatus::Open)
    }
}

/// What an attachment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttachKind {
    /// A proposed new file (an agent's change it may not write itself): the host checks and
    /// shows the path, and places it on a person's say.
    Proposal,
}

impl AttachKind {
    pub const fn name(self) -> &'static str {
        match self {
            AttachKind::Proposal => "proposal",
        }
    }
}

/// Something a comment points at, opaque to the kit.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Attachment {
    pub kind: AttachKind,
    /// At most [`ANNOT_ATTACHMENT_MAX`] bytes.
    pub target: String,
}

impl Attachment {
    pub fn proposal(target: impl Into<String>) -> Self {
        Self {
            kind: AttachKind::Proposal,
            target: target.into(),
        }
    }
}

/// One comment of a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Comment {
    /// 0 is the thread's first comment.
    pub n: u32,
    pub by: Attribution,
    /// `None` once deleted or purged: the only copy is gone.
    pub body: Option<String>,
    /// Pids the host resolved (the kit doesn't parse text). Cleared with the body.
    pub mentions: Vec<String>,
    /// Cleared with the body.
    pub attachments: Vec<Attachment>,
    /// Who deleted it, when.
    pub deleted: Option<Attribution>,
}

/// A re-anchor an agent proposed: a person confirms it by re-anchoring to the same place.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingAnchor {
    pub to: AnchorTarget,
    /// The version the target was named in.
    pub at: VersionId,
    pub by: Attribution,
}

/// A thread: its anchor, state and comments.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Thread {
    pub id: ThreadId,
    pub anchor: Anchor,
    /// The anchor resolved at the version asked for (or the newest earlier one with an index,
    /// [`AnchorResolution::current`] `false`); `None` when no index applies yet.
    pub resolution: Option<AnchorResolution>,
    pub status: ThreadStatus,
    pub created: Attribution,
    /// Oldest first.
    pub comments: Vec<Comment>,
    pub pending_anchor: Option<PendingAnchor>,
}

/// Where a new comment (or a re-anchor) goes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NewAnchor {
    /// The whole doc.
    Doc,
    /// A node, by its key (`@n:42`, `@k:r4.1`), in `version` (`None`: the current one), with
    /// the pack's opaque `hint` (at most [`crate::anchor::ANCHOR_HINT_MAX`] bytes).
    Node {
        key: String,
        version: Option<VersionId>,
        hint: Option<String>,
    },
}

impl NewAnchor {
    pub fn node(key: impl Into<String>, version: Option<VersionId>) -> Self {
        NewAnchor::Node {
            key: key.into(),
            version,
            hint: None,
        }
    }

    /// The same, with the pack's hint.
    pub fn with_hint(self, h: impl Into<String>) -> Self {
        match self {
            NewAnchor::Node { key, version, .. } => NewAnchor::Node {
                key,
                version,
                hint: Some(h.into()),
            },
            other => other,
        }
    }
}

/// A new thread's first comment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NewComment {
    pub anchor: NewAnchor,
    pub body: String,
    pub mentions: Vec<String>,
    pub attachments: Vec<Attachment>,
}

impl NewComment {
    pub fn new(anchor: NewAnchor, body: impl Into<String>) -> Self {
        Self {
            anchor,
            body: body.into(),
            mentions: Vec::new(),
            attachments: Vec::new(),
        }
    }

    pub fn with_mentions(mut self, pids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.mentions = pids.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_attachment(mut self, a: Attachment) -> Self {
        self.attachments.push(a);
        self
    }
}

/// What a history row records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnnotOp {
    Created,
    Replied,
    Resolved,
    Reopened,
    Deleted,
    Reanchored,
    ReanchorProposed,
    /// An owner's purge cleared it (or the thread's bodies).
    Purged,
}

impl AnnotOp {
    pub const fn name(self) -> &'static str {
        match self {
            AnnotOp::Created => "created",
            AnnotOp::Replied => "replied",
            AnnotOp::Resolved => "resolved",
            AnnotOp::Reopened => "reopened",
            AnnotOp::Deleted => "deleted",
            AnnotOp::Reanchored => "reanchored",
            AnnotOp::ReanchorProposed => "reanchor_proposed",
            AnnotOp::Purged => "purged",
        }
    }

    pub(crate) fn code(self) -> u8 {
        match self {
            AnnotOp::Created => 1,
            AnnotOp::Replied => 2,
            AnnotOp::Resolved => 3,
            AnnotOp::Reopened => 4,
            AnnotOp::Deleted => 5,
            AnnotOp::Reanchored => 6,
            AnnotOp::ReanchorProposed => 7,
            AnnotOp::Purged => 8,
        }
    }

    pub(crate) fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            1 => AnnotOp::Created,
            2 => AnnotOp::Replied,
            3 => AnnotOp::Resolved,
            4 => AnnotOp::Reopened,
            5 => AnnotOp::Deleted,
            6 => AnnotOp::Reanchored,
            7 => AnnotOp::ReanchorProposed,
            8 => AnnotOp::Purged,
            _ => return None,
        })
    }
}

/// One history row: who did what to which thread. **Never a body**, a hint or a name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AnnotEvent {
    /// Per doc, from 1, never reused.
    pub seq: u64,
    pub thread: ThreadId,
    pub comment: Option<u32>,
    pub op: AnnotOp,
    pub by: Attribution,
    /// A re-anchor's (or a proposal's) target.
    pub to: Option<AnchorTarget>,
    /// The version a re-anchor named.
    pub version: Option<VersionId>,
}

/// Which threads [`crate::session::BlobSession::comments`] lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ThreadQuery {
    /// `Some(true)`: open ones only; `Some(false)`: resolved ones only.
    pub open: Option<bool>,
    /// Resolve anchors at this version (`None`: the current one). An older version needs
    /// history.
    pub version: Option<VersionId>,
    /// Only threads anchored to this node key.
    pub on: Option<String>,
    /// Only threads changed after this change seq ([`ThreadPage::head`] of an earlier page):
    /// what changed since. Reads at most one row a thread.
    pub since: Option<u64>,
    /// Threads after this one (the cursor: [`ThreadPage::next`]).
    pub after: Option<ThreadId>,
    /// At most this many (default [`ANNOT_PAGE_DEFAULT`], at most [`ANNOT_PAGE_MAX`]).
    pub limit: Option<usize>,
}

impl ThreadQuery {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }
    pub fn at_version(mut self, v: VersionId) -> Self {
        self.version = Some(v);
        self
    }
    pub fn on(mut self, key: impl Into<String>) -> Self {
        self.on = Some(key.into());
        self
    }
    pub fn since(mut self, seq: u64) -> Self {
        self.since = Some(seq);
        self
    }
    pub fn after(mut self, t: ThreadId) -> Self {
        self.after = Some(t);
        self
    }
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }
}

/// One page of threads.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ThreadPage {
    /// In id order.
    pub threads: Vec<Thread>,
    /// The cursor for the next page ([`ThreadQuery::after`]); `None`: this was the last.
    pub next: Option<ThreadId>,
    /// The doc's newest change seq when it was read: pass it as [`ThreadQuery::since`] to
    /// get only the threads that changed after (any change: a comment, a status, a purge's or
    /// an erasure's, an anchor's new state). Not a history seq.
    pub head: u64,
}

/// What an owner's annotation purge clears.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnnotPurge {
    /// One comment's body.
    Comment(ThreadId, u32),
    /// Every body of one thread, and its anchor's hint and fingerprint.
    Thread(ThreadId),
    /// Every body, hint and fingerprint name that holds this text, whole or by any 8-byte
    /// window (at least [`crate::store::MIN_MATCH`] bytes).
    Text(String),
    /// Every body, hint and fingerprint of the doc.
    All,
}

/// How a re-anchor went.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reanchored {
    /// A person's: the thread points at the new place.
    Applied(Thread),
    /// An agent's: the thread still points where it did, with [`Thread::pending_anchor`] set
    /// for a person to confirm.
    Proposed(Thread),
}

/// One anchor's state change, when an index was recorded ([`crate::session::AuditEvent::AnchorState`]).
pub(crate) type StateChange = (ThreadId, Option<AnchorState>, AnchorState);
