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
    /// The participant edits through a replica of its own (an editor holding the doc and
    /// sending updates as its client), not only through the session (v0.9.7). Its client ID
    /// is then never reused for another connection: a stale replica could write as it from
    /// an old clock, and the engine would drop that content without an error. Set by the
    /// host ([`Participant::own_replica`]); default false.
    pub own_replica: bool,
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
            own_replica: false,
        }
    }

    /// Mark this participant as editing through a replica of its own (see
    /// [`Participant::own_replica`](field@Participant::own_replica)): its client IDs are
    /// never reused.
    pub fn own_replica(mut self) -> Self {
        self.own_replica = true;
        self
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

/// A caret or selection on a grid, by identity: the anchor and head cells
/// `(row key, column key)`, equal for a caret. A selection is the rectangle between them:
/// the rows (and columns) between the two keys in the current order. It follows moves and
/// sorts with no transform; when its row or column is deleted it is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CellCursor {
    pub anchor: (crate::elem::Key, crate::elem::Key),
    pub head: (crate::elem::Key, crate::elem::Key),
    /// The head cell's live position now, 1-based (`#12` and the column's `#N`).
    pub at: Option<(usize, usize)>,
    /// The head column's header text, when it has one.
    pub name: Option<String>,
    /// The rendered lines of the rows it covers now (root [`crate::table::ROWS`]).
    pub lines: Option<LineRange>,
}

impl CellCursor {
    /// What it covers, as a claim would ([`super::awareness::ClaimSpan::Cells`]).
    pub fn span(&self) -> super::awareness::ClaimSpan {
        super::awareness::ClaimSpan::Cells {
            rows: (self.anchor.0.clone(), self.head.0.clone()),
            cols: (self.anchor.1.clone(), self.head.1.clone()),
        }
    }
}

impl std::fmt::Display for CellCursor {
    /// `cell r7/c2 (#12, "email")`; a selection `cells r7/c2–r9/c4 (#12, "email")`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (r, c) = &self.head;
        if self.anchor == self.head {
            write!(f, "cell {r}/{c}")?;
        } else {
            write!(f, "cells {}/{}–{r}/{c}", self.anchor.0, self.anchor.1)?;
        }
        match (&self.at, &self.name) {
            (Some((row, _)), Some(n)) => {
                write!(f, " (#{row}, \"{}\")", super::awareness::one_line(n))
            }
            (Some((row, _)), None) => write!(f, " (#{row})"),
            _ => Ok(()),
        }
    }
}

/// At most this many keys in one [`ElemSelection`]; more is refused
/// ([`super::SessionError::SelectionTooLarge`]).
pub const ELEM_SELECTION_MAX: usize = 256;

/// Elements selected by identity: keys of one MovableList root. It follows moves with no
/// mapping, and a key whose element stops being live leaves it
/// ([`super::Session::set_selection`]).
///
/// `addresses` are the same elements by address ([`super::Pack::address_of`]),
/// in the same order: what the wire carries as the selection's `keys`. The kit fills them on
/// every selection it hands out (the roster, its events, the setters' replies); [`Self::new`]
/// starts them as the keys themselves (a pack's default addresses).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ElemSelection {
    pub root: String,
    pub keys: Vec<crate::elem::Key>,
    pub addresses: Vec<String>,
}

/// Equal by identity (root and keys): the addresses follow from the keys through the pack.
impl PartialEq for ElemSelection {
    fn eq(&self, o: &Self) -> bool {
        self.root == o.root && self.keys == o.keys
    }
}
impl Eq for ElemSelection {}

impl ElemSelection {
    pub fn new(root: impl Into<String>, keys: Vec<crate::elem::Key>) -> Self {
        Self {
            root: root.into(),
            addresses: keys.iter().map(|k| k.as_str().to_owned()).collect(),
            keys,
        }
    }
}

/// The container a connection has in view (`focus`): an
/// element of a MovableList root, by key (a slide of a deck's `slides`). Set by the host for a
/// person ([`super::Session::set_focus`]), and by the kit for an agent when it takes a claim
/// ([`super::Session::claim_elems`]); a text range sets it too
/// ([`super::Session::set_range`]). It leaves when its element stops being live.
///
/// The key is the live store's own. The wire carries addresses (`@sld:256`):
/// [`super::Session::set_focus_at`] takes one and returns the focus by address, and
/// [`super::Session::address_of`] converts a stored focus ([`super::Pack::address_of`]).
/// `address` is the element's address, what the wire carries as the focus's `key`: the kit
/// fills it on every focus it hands out; [`Self::new`] starts it as the key.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ElemFocus {
    pub root: String,
    pub key: crate::elem::Key,
    pub address: String,
}

/// Equal by identity (root and key), as [`ElemSelection`].
impl PartialEq for ElemFocus {
    fn eq(&self, o: &Self) -> bool {
        self.root == o.root && self.key == o.key
    }
}
impl Eq for ElemFocus {}

impl ElemFocus {
    pub fn new(root: impl Into<String>, key: crate::elem::Key) -> Self {
        Self {
            root: root.into(),
            address: key.as_str().to_owned(),
            key,
        }
    }
}

/// A caret or a selection inside one element's text field (a
/// `range`): `field` of element `key` of the MovableList root `root`, whose text is the string
/// value of the [composite](super::Pack::composite) map key `<key>\x1f<field>`. `anchor` stays
/// put while `head` moves (equal for a bare caret), in **UTF-8 bytes** of that text (a client
/// shows UTF-16). It is never moved: when the field changes, or the element stops being live,
/// it is dropped, and its owner sends a new one ([`super::Session::set_range`]).
///
/// By address, as for [`ElemFocus`]: [`super::Session::set_range_at`]; `address` is the
/// element's, filled by the kit as on [`ElemFocus`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ElemRange {
    pub root: String,
    pub key: crate::elem::Key,
    pub address: String,
    pub field: String,
    pub anchor: usize,
    pub head: usize,
}

/// Equal by identity (root, key, field and offsets), as [`ElemSelection`].
impl PartialEq for ElemRange {
    fn eq(&self, o: &Self) -> bool {
        (&self.root, &self.key, &self.field, self.anchor, self.head)
            == (&o.root, &o.key, &o.field, o.anchor, o.head)
    }
}
impl Eq for ElemRange {}

impl ElemRange {
    pub fn new(
        root: impl Into<String>,
        key: crate::elem::Key,
        field: impl Into<String>,
        anchor: usize,
        head: usize,
    ) -> Self {
        Self {
            root: root.into(),
            address: key.as_str().to_owned(),
            key,
            field: field.into(),
            anchor,
            head,
        }
    }
}

/// A place in the doc's own coordinates, which the kit doesn't interpret. Always finite, and
/// never `-0.0`, so it compares and encodes one way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}

impl Eq for Point {}

/// `-0.0` as `0.0`; a number that isn't finite is refused.
fn coordinate(what: &str, v: f64) -> super::error::Result<f64> {
    if v.is_finite() {
        Ok(v + 0.0)
    } else {
        Err(super::error::SessionError::BadPosition(format!(
            "{what} is not a finite number"
        )))
    }
}

impl Point {
    /// [`super::SessionError::BadPosition`] unless both are finite.
    pub fn new(x: f64, y: f64) -> super::error::Result<Self> {
        Ok(Self {
            x: coordinate("x", x)?,
            y: coordinate("y", y)?,
        })
    }

    pub fn x(&self) -> f64 {
        self.x
    }

    pub fn y(&self) -> f64 {
        self.y
    }
}

/// The longest address a presence value names ([`TimePosition::within`], a view address,
/// [`super::Session::set_view_address`]), in bytes.
pub const PRESENCE_ADDRESS_MAX: usize = 512;

/// `address` as a presence value names one: 1 to [`PRESENCE_ADDRESS_MAX`] bytes, read by the
/// kit's address grammar ([`crate::addr::parse`] with native IDs; a bare key is one), else
/// [`super::SessionError::BadPosition`] naming `what`.
pub(crate) fn presence_address(what: &str, address: &str) -> super::error::Result<String> {
    let bad = |why: &str| {
        Err(super::error::SessionError::BadPosition(format!(
            "{what} {address:?} {why}"
        )))
    };
    if address.len() > PRESENCE_ADDRESS_MAX {
        return bad(&format!("is over {PRESENCE_ADDRESS_MAX} bytes"));
    }
    if crate::addr::parse(address, crate::addr::Forms::none().native()).is_err() {
        return bad("is not an address");
    }
    Ok(address.to_owned())
}

/// A decimal integer as a time position writes one: an optional `-`, then digits with no
/// leading zero (`0` alone), within an `i64`.
fn decimal_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let canonical = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && s != "-0";
    canonical.then(|| s.parse::<i64>().ok()).flatten()
}

/// Whether someone's time position is moving ([`TimePosition::state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayState {
    Playing,
    Paused,
}

impl PlayState {
    /// `playing` or `paused`, as the wire carries it.
    pub fn as_str(self) -> &'static str {
        match self {
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
        }
    }

    /// The word back (`playing`, `paused`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "playing" => Some(PlayState::Playing),
            "paused" => Some(PlayState::Paused),
            _ => None,
        }
    }
}

/// A participant's place in time (a timeline's playhead), a presence value
/// ([`super::Session::set_time_position`]). Generic: the kit reads no time, it carries the
/// position in the pack's own unit (the view schema's
/// [`crate::view_index::ViewSchema::time_unit`]: a timeline's ticks) as a decimal string, never
/// a floating-point number. A follower extrapolates a playing one from `at_ms` at `rate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimePosition {
    at: String,
    state: PlayState,
    at_ms: u64,
    within: Option<String>,
    rate: Option<String>,
}

impl TimePosition {
    /// At `at` (a decimal integer in the pack's unit: an optional `-`, digits, no leading
    /// zero, within an `i64`), playing or paused. [`super::SessionError::BadPosition`]
    /// otherwise.
    pub fn new(at: &str, state: PlayState) -> super::error::Result<Self> {
        if decimal_int(at).is_none() {
            return Err(super::error::SessionError::BadPosition(format!(
                "the time {at:?} is not a decimal integer (an optional -, digits, no leading \
                 zero, within 64 bits)"
            )));
        }
        Ok(Self {
            at: at.to_owned(),
            state,
            at_ms: 0,
            within: None,
            rate: None,
        })
    }

    /// In what (a sequence, a track), by address ([`PRESENCE_ADDRESS_MAX`] bytes at most, read
    /// by the kit's address grammar).
    pub fn within(mut self, address: &str) -> super::error::Result<Self> {
        self.within = Some(presence_address("the time position's `in`", address)?);
        Ok(self)
    }

    /// The playback rate, a decimal ratio: `n` or `n/d` (`2`, `1/2`, `-1`, `1001/1000`), each a
    /// decimal integer as [`TimePosition::new`] reads one, `d` above 0. None set: 1.
    pub fn rate(mut self, rate: &str) -> super::error::Result<Self> {
        let good = match rate.split_once('/') {
            None => decimal_int(rate).is_some(),
            Some((n, d)) => decimal_int(n).is_some() && decimal_int(d).is_some_and(|d| d > 0),
        };
        if !good {
            return Err(super::error::SessionError::BadPosition(format!(
                "the rate {rate:?} is not a decimal ratio (n or n/d, d above 0)"
            )));
        }
        self.rate = Some(rate.to_owned());
        Ok(self)
    }

    /// The position, a decimal integer in the pack's unit.
    pub fn at(&self) -> &str {
        &self.at
    }

    pub fn state(&self) -> PlayState {
        self.state
    }

    /// When it was set (host clock, ms): stamped by the kit when it stores it; 0 before.
    pub fn at_ms(&self) -> u64 {
        self.at_ms
    }

    /// What it is in, by address; `None`: the doc's own time.
    pub fn in_address(&self) -> Option<&str> {
        self.within.as_deref()
    }

    /// The playback rate as given; `None`: 1.
    pub fn rate_text(&self) -> Option<&str> {
        self.rate.as_deref()
    }

    /// The same, stamped at `now` (the kit's, when it stores it).
    pub(crate) fn stamped(mut self, now: u64) -> Self {
        self.at_ms = now;
        self
    }
}

/// An area in the doc's own coordinates, which the kit doesn't interpret: `(x, y)` and a size.
/// Always finite, never `-0.0`, and the size is never negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Eq for Rect {}

impl Rect {
    /// [`super::SessionError::BadPosition`] unless all four are finite and `w` and `h` aren't
    /// negative.
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> super::error::Result<Self> {
        let (w, h) = (coordinate("w", w)?, coordinate("h", h)?);
        if w < 0.0 || h < 0.0 {
            return Err(super::error::SessionError::BadPosition(format!(
                "a size of {w} by {h} is negative"
            )));
        }
        Ok(Self {
            x: coordinate("x", x)?,
            y: coordinate("y", y)?,
            w,
            h,
        })
    }

    pub fn x(&self) -> f64 {
        self.x
    }

    pub fn y(&self) -> f64 {
        self.y
    }

    pub fn w(&self) -> f64 {
        self.w
    }

    pub fn h(&self) -> f64 {
        self.h
    }
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
    /// The most active connection's caret that has one (a text doc).
    pub cursor: Option<CursorAt>,
    /// On a grid: the most active connection's cell caret or selection that has one.
    pub cell: Option<CellCursor>,
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
    /// A text doc: one [`Device`] per connection this participant has open, in the order
    /// they joined, each with its own caret (v0.10.1). [`RosterEntry::cursor`] is still the
    /// most active connection's caret, for hosts and clients that show one caret a person.
    /// Empty on a grid (its carets are [`RosterEntry::cell`], per participant) and for tier 0.
    pub devices: Vec<Device>,
    /// The most active connection's element selection that has one
    /// ([`super::Session::set_selection`]).
    pub selection: Option<ElemSelection>,
    /// The most active connection's pointer that has one ([`super::Session::set_pointer`]).
    pub pointer: Option<Point>,
    /// The most active connection's viewport that has one ([`super::Session::set_viewport`]).
    pub viewport: Option<Rect>,
    /// The most active connection's focus that has one: the container in view
    /// ([`super::Session::set_focus`]).
    pub focus: Option<ElemFocus>,
    /// The most active connection's text range that has one ([`super::Session::set_range`]).
    pub range: Option<ElemRange>,
    /// On a grid with a view schema's grid slot: the most active connection's cell caret and
    /// ranges that has one ([`super::Session::set_cells`]).
    pub cells: Option<crate::grid::Cells>,
    /// The most active connection's view address that has one: the view they are looking at
    /// ([`super::Session::set_view_address`]).
    pub view_address: Option<String>,
    /// The most active connection's time position that has one: their playhead
    /// ([`super::Session::set_time_position`]).
    pub time: Option<TimePosition>,
}

/// One connection of a participant, on a text doc's roster ([`RosterEntry::devices`]): the
/// same person in two tabs or on two devices is one roster row with two of these (v0.10.1).
/// A client draws every other connection's caret, its own person's other tabs included, and
/// hides only the connection it is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Device {
    /// The connection's client ID ([`super::Joined::client`]): a client knows its own one.
    pub client: crate::doc::ClientId,
    /// This connection's caret, if it placed one.
    pub cursor: Option<CursorAt>,
    /// The host's label for this connection, given at join
    /// ([`JoinOptions::label`](field@super::JoinOptions::label)):
    /// "another tab", "Chrome on Mac". Never doc text; `None` when the host gave none.
    pub label: Option<String>,
    /// This connection's element selection, if it has one.
    pub selection: Option<ElemSelection>,
    /// This connection's pointer, if it has one.
    pub pointer: Option<Point>,
    /// This connection's viewport, if it has one.
    pub viewport: Option<Rect>,
    /// This connection's focus, if it has one.
    pub focus: Option<ElemFocus>,
    /// This connection's text range, if it has one.
    pub range: Option<ElemRange>,
    /// This connection's cell caret and ranges, if it has them.
    pub cells: Option<crate::grid::Cells>,
    /// This connection's view address, if it has one.
    pub view_address: Option<String>,
    /// This connection's time position, if it has one.
    pub time: Option<TimePosition>,
}

/// At most this many characters of a connection's label
/// ([`JoinOptions::label`](field@super::JoinOptions::label)) are kept; the rest is cut.
pub const DEVICE_LABEL_MAX: usize = 64;

/// A host's connection label as the kit keeps it: control characters dropped, whitespace
/// trimmed, at most [`DEVICE_LABEL_MAX`] characters; `None` when nothing is left. A label
/// never fails a join.
pub(crate) fn device_label(label: Option<&str>) -> Option<String> {
    let clean: String = label?.chars().filter(|c| !c.is_control()).collect();
    let cut: String = clean.trim().chars().take(DEVICE_LABEL_MAX).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_owned())
}
