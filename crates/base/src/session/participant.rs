//! Participants and presence.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use super::position::{Cursor, LineCol, LineRange};

/// One connection to a session: a tab's socket, a `watch`, a command-line caller. Returned
/// by [`super::Session::join`] and passed to every per-participant call.
///
/// A plain `Copy` id: cheap to keep in per-socket or per-request state, and it doesn't
/// borrow the session. Unique within the process, so an id from another session (or from
/// before an unload) is simply not joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnId(u64);

impl ConnId {
    pub(crate) fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        ConnId(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The raw number, for logs.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conn-{}", self.0)
    }
}

/// Participant ids starting with this are reserved for the kit itself.
pub const RESERVED_PID_PREFIX: &str = "kit:";

/// The participant that imports a file's first content into a new doc.
pub const IMPORT_PID: &str = "kit:import";

/// What kind of participant this is. Shown in presence and in the "contributors" fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticipantKind {
    Person,
    Agent,
    /// A guest acting through an App.
    App,
    /// A change made outside the kit (the file edited on disk by another program).
    External,
    /// A restore of an older version.
    Restore,
    /// The first import of a file.
    Import,
}

/// The participant that applies changes made to the file outside the kit (another program
/// saving it). It has its own client ID per doc, and its changes are logged `via: "disk"`.
pub const EXTERNAL_PID: &str = "kit:external";

/// Blame's author for text whose client the store has no owner for (never a guess).
pub const UNKNOWN_PID: &str = "kit:unknown";

/// Someone (or something) in a doc. **Identity is stamped by the host** from its own
/// authentication; the kit never takes it from a client.
///
/// `#[non_exhaustive]`: build it with [`Participant::new`] and the setters
/// ([`Participant::home`], [`Participant::on_behalf_of`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Participant {
    /// The host's stable id for who this is. Must not be empty or start with `kit:`.
    pub pid: String,
    /// Display name.
    pub name: String,
    /// Display colour, e.g. `#7c3aed`.
    pub colour: String,
    pub kind: ParticipantKind,
    /// Surface they came through: `"cli"`, `"tab"`, `"app"`, `"disk"`... (logged per change).
    pub via: String,
    /// Home server when the participant is remote.
    pub home: Option<String>,
    /// The person (pid) an assistant agent is acting for. Set by the host, only for such
    /// agents. The change stays the agent's own (its pid, client ID, undo stack, rights);
    /// this is metadata on it, and each change made this way starts a `Pending` approval
    /// that only that person can settle ([`super::Session::set_approval`]). Must not be
    /// empty, reserved, or the participant's own pid.
    pub on_behalf_of: Option<String>,
}

impl Participant {
    /// A participant with no home server, acting for nobody else.
    pub fn new(
        pid: impl Into<String>,
        name: impl Into<String>,
        colour: impl Into<String>,
        kind: ParticipantKind,
        via: impl Into<String>,
    ) -> Self {
        Self {
            pid: pid.into(),
            name: name.into(),
            colour: colour.into(),
            kind,
            via: via.into(),
            home: None,
            on_behalf_of: None,
        }
    }

    /// Set the home server (a remote participant).
    pub fn home(mut self, home: impl Into<String>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Mark this participant as an assistant agent acting for the person `pid`.
    pub fn on_behalf_of(mut self, pid: impl Into<String>) -> Self {
        self.on_behalf_of = Some(pid.into());
        self
    }
}

/// How long a connection stays without activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stay {
    /// Leaves after [`super::Config::idle_leave_ms`] with no call (command-line verbs).
    UntilIdle,
    /// Stays until [`super::Session::leave`] or a kick: an editor tab's connection, or a
    /// `watch` / `open` verb. A hold is sticky: [`super::Session::join_shared`] with
    /// `UntilIdle` keeps it; use [`super::Session::set_stay`] to drop it.
    Held,
}

/// A participant's role in presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Has edit rights and edited, or placed a caret, within
    /// [`super::Config::editor_window_ms`].
    Editor,
    Viewer,
}

/// A caret with its `line:col` form (for `follow` and `who`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorAt {
    pub cursor: Cursor,
    pub anchor: LineCol,
    pub head: LineCol,
}

/// One row of [`super::Session::roster`]: one participant (pid), however many connections
/// they have. Per-connection state is folded in: role, caret and view come from the most
/// active connection; away only when every connection is away.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RosterEntry {
    /// As stamped on the most active connection (its `via` may differ from the others').
    pub participant: Participant,
    pub role: Role,
    /// Every connection has been not visible for [`super::Config::away_after_ms`] (see
    /// [`super::Session::set_visible`]).
    pub away: bool,
    /// The most active connection's caret that has one.
    pub cursor: Option<CursorAt>,
    /// The range they are looking at (readers' "viewing highlight"), from the most active
    /// connection that has one.
    pub view: Option<LineRange>,
    /// Latest edit across their connections.
    pub last_edit_ms: Option<u64>,
    /// `Held` when any connection is held.
    pub stay: Stay,
    /// How many connections this participant has open.
    pub connections: usize,
    /// Tier 0: the opaque view region (a page number, a slide) from the most active
    /// connection that set one ([`super::BlobSession::set_region`]). `None` for text docs.
    pub region: Option<String>,
    /// Tier 0: this participant holds the doc's lease (they are shown as an editor).
    pub holds_lease: bool,
    /// Their live claims in this doc, oldest first (amendment 13). Empty for tier 0.
    pub claims: Vec<super::awareness::ClaimInfo>,
    /// The lines their latest change touched (anchored: where those lines are now), from
    /// the most active connection that has one. `None` for tier 0.
    pub last_edit: Option<LineRange>,
}
