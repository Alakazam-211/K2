//! Agent presence (amendment 13): claims, heads-up notes and the activity ring.
//!
//! **Ephemeral awareness, not locking and not history.** Everything here lives in a loaded
//! session's memory only: nothing is written to the store, the update log, versions or the
//! engine, and it is gone when the doc unloads. The kit never blocks or delays anyone: a
//! claim refuses an edit only when the **host** asks for that on the edit
//! ([`ClaimPolicy::Respect`]), and never a person's ([`ParticipantKind::Person`]).
//!
//! **Ranges are anchors, expanding.** A claim's (and a view's, and a last edit's) byte span
//! is carried through every change by [`position::transform`]: its start with
//! [`Bias::Before`] (it sticks to the character before: Yrs `Assoc::Before`), its end with
//! [`Bias::After`] (the character after: `Assoc::After`). Text typed at either edge lands
//! inside, and the span never inverts (the opposite convention inverts an empty or
//! near-empty range on the first keystroke at its edge). Where two participants' claims
//! meet, text typed there joins only one of them ([`claim_fates`]): claims never overlap.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use super::participant::{ConnId, ParticipantKind};
use super::position::{self, Bias, LineRange};
use crate::elem::Key;
use crate::text::diff::Hunk;

/// A claim's id: unique within the process, so an id from another doc (or an ended claim)
/// is simply not live. Shown as `claim-12`; [`ClaimId::parse`] reads that or `12`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClaimId(u64);

impl ClaimId {
    pub(crate) fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        ClaimId(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The id with this number (as a host parses it from a command line).
    pub const fn new(n: u64) -> Self {
        ClaimId(n)
    }

    /// The raw number.
    pub fn get(self) -> u64 {
        self.0
    }

    /// `claim-12` or `12`.
    pub fn parse(s: &str) -> Option<Self> {
        s.strip_prefix("claim-")
            .unwrap_or(s)
            .parse()
            .ok()
            .map(ClaimId)
    }
}

impl fmt::Display for ClaimId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "claim-{}", self.0)
    }
}

/// What an edit does about other participants' claims. **The host chooses**, per edit
/// ([`super::Session::edit_with_policy`], [`super::Session::write_base_with`],
/// [`crate::tools::ToolOptions`]); the kit's default is [`ClaimPolicy::Ignore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ClaimPolicy {
    /// Claims never refuse anything (awareness only: notes still say who is there).
    #[default]
    Ignore,
    /// An **agent's** (`ParticipantKind::Agent`) edit that touches another participant's
    /// live claim, or a hand-off reservation held for someone else, is refused with
    /// [`super::SessionError::Claimed`] (exit 5) and nothing is applied. **Claims coordinate
    /// agents; they never block anyone else:** a person, an app guest or any other kind is
    /// never refused, whatever the policy.
    Respect,
}

/// How a claim ended ([`super::AuditEvent::ClaimEnded`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClaimEnd {
    /// Its owner released it.
    Released,
    /// Its owner's own edit touching it landed (a claim without `hold`).
    Edited,
    /// Its time ran out on the host's clock.
    Expired,
    /// The connection that took it left, was kicked, idled out or lost read.
    Left,
    /// A store write failed mid-edit and the session reloaded (positions can't be trusted).
    Reset,
    /// Every row (or column) it covered was deleted (a grid claim, A10, K-PRES-6), or none of
    /// its elements is live (an element claim), so the claimant knows why.
    Deleted,
}

impl ClaimEnd {
    /// `released`, `edited`, `expired`, `left`, `reset`, `deleted`.
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimEnd::Released => "released",
            ClaimEnd::Edited => "edited",
            ClaimEnd::Expired => "expired",
            ClaimEnd::Left => "left",
            ClaimEnd::Reset => "reset",
            ClaimEnd::Deleted => "deleted",
        }
    }
}

/// What a claim covers (A10, K-PRES-6): lines of a text root, or on a grid ([`crate::table`]
/// doc) rows, columns or a rectangle of cells **by identity**: the elements between two keys
/// in the current order, inclusive (whichever comes first). Keys are stable, so a grid span
/// needs no transform: it follows moves and sorts, rows moved between its ends join it, and
/// a row inserted right at one of its ends joins it (anchors expand, as a text claim's do).
/// A deleted end still marks its place (where a restore would show it); the claim ends with
/// [`ClaimEnd::Deleted`] when nothing it covers is live.
///
/// On a keyed doc that isn't a table, a claim covers named elements of a list root
/// ([`ClaimSpan::Elems`]): those keys and no others. It never grows, and a map key of a
/// [composite](super::Pack::composite) root counts as its element.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClaimSpan {
    /// Lines of a text root (`claim --lines a,b`; on a grid, mapped to rows when taken).
    Lines(LineRange),
    /// Rows `first..last` (`claim --rows R1..R2`).
    Rows { first: Key, last: Key },
    /// Columns `first..last` (`claim --cols C1..C2`).
    Cols { first: Key, last: Key },
    /// The cells of rows `rows.0..rows.1` and columns `cols.0..cols.1` (`claim --cells
    /// R1,C1..R2,C2`).
    Cells { rows: (Key, Key), cols: (Key, Key) },
    /// Elements of a MovableList root by key, in key order, at most [`CLAIM_KEYS_MAX`]
    /// ([`super::Session::claim_elems`]).
    Elems { root: String, keys: Vec<Key> },
    /// Tier 0: nodes of a blob doc's structure index by key, in key order, at most
    /// [`CLAIM_KEYS_MAX`] ([`super::BlobSession::claim_nodes`]). Keys carry across versions;
    /// the claim never grows.
    Nodes { keys: Vec<crate::index::NodeKey> },
}

/// The most elements one claim names ([`ClaimSpan::Elems`]).
pub const CLAIM_KEYS_MAX: usize = 256;

/// `items i1`, `items i1, i2`, `items i1, i2 (+3)`.
pub(crate) fn elems_text(root: &str, keys: &[Key]) -> String {
    let shown: Vec<&str> = keys.iter().take(2).map(Key::as_str).collect();
    let more = keys.len().saturating_sub(2);
    if more == 0 {
        format!("{root} {}", shown.join(", "))
    } else {
        format!("{root} {} (+{more})", shown.join(", "))
    }
}

/// `node @n:1`, `nodes @n:1, @n:2`, `nodes @n:1, @n:2 (+3)`.
pub(crate) fn nodes_text(keys: &[crate::index::NodeKey]) -> String {
    let shown: Vec<&str> = keys.iter().take(2).map(|k| k.as_str()).collect();
    let more = keys.len().saturating_sub(2);
    let w = if keys.len() == 1 { "node" } else { "nodes" };
    if more == 0 {
        format!("{w} {}", shown.join(", "))
    } else {
        format!("{w} {} (+{more})", shown.join(", "))
    }
}

/// The longest note any claim carries, in bytes: it comes from the host or off the wire, and every
/// roster subscriber is told it ([`super::Session::claim`], [`super::Session::claim_elems`]).
pub const CLAIM_NOTE_MAX: usize = 1024;

impl fmt::Display for ClaimSpan {
    /// `lines 3–5`, `rows r12–r40`, `cols c2`, `cells r1/c1–r4/c3`, `items i1, i2 (+3)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pair = |a: &Key, b: &Key| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}–{b}")
            }
        };
        match self {
            ClaimSpan::Lines(l) => f.write_str(&lines_text(l)),
            ClaimSpan::Rows { first, last } => {
                let w = if first == last { "row" } else { "rows" };
                write!(f, "{w} {}", pair(first, last))
            }
            ClaimSpan::Cols { first, last } => {
                let w = if first == last { "col" } else { "cols" };
                write!(f, "{w} {}", pair(first, last))
            }
            ClaimSpan::Cells { rows, cols } => {
                if rows.0 == rows.1 && cols.0 == cols.1 {
                    write!(f, "cell {}/{}", rows.0, cols.0)
                } else {
                    write!(f, "cells {}/{}–{}/{}", rows.0, cols.0, rows.1, cols.1)
                }
            }
            ClaimSpan::Elems { root, keys } => f.write_str(&elems_text(root, keys)),
            ClaimSpan::Nodes { keys } => f.write_str(&nodes_text(keys)),
        }
    }
}

/// A live claim, as [`super::Session::claim`], [`super::Session::claims`] and the roster
/// report it. The range is anchored: `lines` and `start..end` are where it is **now**.
///
/// Field order is the stable order for a host's JSON (`--presence json`); the kit has no
/// serde.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClaimInfo {
    pub id: ClaimId,
    /// Who holds it.
    pub pid: String,
    pub name: String,
    /// The connection that took it (it ends when that connection leaves).
    pub conn: ConnId,
    /// The lines it covers now (1-based, inclusive). On a grid: the rendered lines of the
    /// rows it covers (root [`crate::table::ROWS`]; a column claim covers every line). An
    /// element claim has no lines: this is the list root and the 1-based live positions of its
    /// first and last live elements (`1, 1` with none live), for display only; `span` says
    /// what it covers.
    pub lines: LineRange,
    /// What it covers: [`ClaimSpan::Lines`] (the same as `lines`) on a text doc; on a grid,
    /// its rows, columns or cells by key (A10); on other keyed docs, [`ClaimSpan::Elems`].
    pub span: ClaimSpan,
    /// Its byte span in the root's text now (`start <= end`; 0 on a grid or for elements).
    pub start: usize,
    pub end: usize,
    /// When it was taken (host clock).
    pub taken_ms: u64,
    /// When it runs out (host clock).
    pub expires_ms: u64,
    /// `expires_ms - now`, on the host's clock at the call (deterministic).
    pub remaining_ms: u64,
    /// The claimant's note ("rewriting the intro").
    pub note: Option<String>,
    /// Survives its owner's edits (ends only on release, time or leave).
    pub hold: bool,
    /// Taken implicitly by a read verb's `--for-edit`.
    pub for_edit: bool,
    /// Pids waiting for it, first in line first ([`super::Session::register_waiter`]).
    pub waiters: Vec<String>,
}

/// [`super::Session::register_waiter`]: the claim waited on, and the caller's place in line
/// (1 = next).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Waiting {
    pub claim: ClaimInfo,
    pub position: usize,
}

/// How a hand-off reservation ended ([`super::AuditEvent::ReservationEnded`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReservationEnd {
    /// The reserved waiter's edit touching the range landed (it passes to the next waiter).
    Edited,
    /// The reserved waiter took a claim on the range (the rest of the queue now waits for
    /// that claim).
    Claimed,
    /// Its deadline passed (it passes to the next waiter).
    Lapsed,
    /// The reserved waiter cancelled ([`super::Session::cancel_waiter`]); it passes on.
    Cancelled,
    /// The reserved waiter's connection left; it passes on.
    Left,
    /// A reload after a failed write, or the session closing: nothing passes on.
    Reset,
    /// Every row (or column) it held was deleted (a grid reservation), or none of its elements
    /// is live: nothing passes on.
    Deleted,
}

impl ReservationEnd {
    /// `edited`, `claimed`, `lapsed`, `cancelled`, `left`, `reset`, `deleted`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReservationEnd::Edited => "edited",
            ReservationEnd::Claimed => "claimed",
            ReservationEnd::Lapsed => "lapsed",
            ReservationEnd::Cancelled => "cancelled",
            ReservationEnd::Left => "left",
            ReservationEnd::Reset => "reset",
            ReservationEnd::Deleted => "deleted",
        }
    }
}

/// A hand-off reservation, as [`super::Session::reservations`] reports it: when a claim
/// with waiters ends, its range is held for the first waiter for
/// [`super::Config::claim_handoff_ms`], so the owner (or another agent) can't take it back
/// before the waiter acts. Field order is the stable JSON order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReservationInfo {
    /// The claim that ended (the id the waiters waited on).
    pub claim: ClaimId,
    /// The waiter it is reserved for.
    pub pid: String,
    pub name: String,
    pub conn: ConnId,
    /// Its range now (anchored, expanding). On a grid: the rendered lines of its rows. For
    /// elements: as [`ClaimInfo::lines`].
    pub lines: LineRange,
    /// What it holds: [`ClaimSpan::Lines`] (as `lines`) on a text doc; on a grid, the ended
    /// claim's rows, columns or cells by key; for elements, [`ClaimSpan::Elems`].
    pub span: ClaimSpan,
    /// Its byte span in the root's text now (0 on a grid or for elements).
    pub start: usize,
    pub end: usize,
    pub until_ms: u64,
    pub remaining_ms: u64,
    /// The rest of the queue, first in line first.
    pub waiters: Vec<String>,
}

/// What a read verb's `--for-edit` did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForEdit {
    /// An implicit claim on the lines shown (it ends at the caller's next edit touching it).
    Claimed(ClaimInfo),
    /// Another participant's live claim overlaps the lines shown: nothing was taken. The
    /// read itself succeeded.
    Conflict(ClaimInfo),
    /// Nothing was taken, for the reason given: no lines shown (`wc`, a byte count), no edit
    /// right, or a claim refusal (`BadClaim`, `ClaimContended`).
    NotTaken(super::SessionError),
}

impl fmt::Display for ForEdit {
    /// One line for stderr, starting `note: `.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForEdit::Claimed(c) => write!(
                f,
                "note: for-edit: claimed {} for {} ({})",
                lines_text(&c.lines),
                duration(c.remaining_ms),
                c.id
            ),
            ForEdit::Conflict(c) => {
                write!(
                    f,
                    "note: for-edit: not claimed: {} {} claimed by {}",
                    lines_text(&c.lines),
                    if c.lines.first == c.lines.last {
                        "is"
                    } else {
                        "are"
                    },
                    who(&c.name, &c.pid)
                )?;
                if let Some(n) = &c.note {
                    write!(f, " {}", quoted(n))?;
                }
                write!(f, " ({} left)", duration(c.remaining_ms))
            }
            ForEdit::NotTaken(e) => write!(
                f,
                "note: for-edit: not claimed: {}",
                one_line(&e.to_string())
            ),
        }
    }
}

/// What a heads-up note is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum NoteKind {
    /// Another participant's live claim.
    Claimed,
    /// A person's selection.
    Selection,
    /// A person's caret.
    Caret,
    /// Another participant's recent edit ([`super::Config::note_edit_window_ms`]).
    Edited,
    /// Another participant's viewing range.
    Viewing,
    /// An outside save overrode your live work ([`Note::overwrite`]): on a grid, your live
    /// value (contract §2.9.1); on a text doc, it may have removed text you wrote.
    Overwritten,
}

impl NoteKind {
    /// `claimed`, `selection`, `caret`, `edited`, `viewing`, `overwritten` (for a host's
    /// JSON).
    pub fn as_str(self) -> &'static str {
        match self {
            NoteKind::Claimed => "claimed",
            NoteKind::Selection => "selection",
            NoteKind::Caret => "caret",
            NoteKind::Edited => "edited",
            NoteKind::Viewing => "viewing",
            NoteKind::Overwritten => "overwritten",
        }
    }
}

/// A heads-up: someone else is viewing, just edited, claimed, or (a person) has a caret or
/// selection on lines this call touched.
///
/// Field order is the stable order for a host's JSON (`--presence json`); `kind` as
/// [`NoteKind::as_str`]. [`fmt::Display`] is one line starting `note: `, for **stderr only**.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Note {
    /// The other participant.
    pub pid: String,
    pub name: String,
    pub kind: NoteKind,
    /// Person, agent, app.
    pub participant_kind: ParticipantKind,
    /// Their lines now.
    pub lines: LineRange,
    /// How long ago they set it (the view, the edit, the claim, the caret move).
    pub age_ms: u64,
    /// For a claim: its id, note and time left.
    pub claim: Option<ClaimId>,
    pub claim_note: Option<String>,
    pub remaining_ms: Option<u64>,
    /// On a grid: what the other participant is on, by key (their claim's span, the cell
    /// they have a caret on, the rows they view or last edited). `lines` are its rendered
    /// lines now. `None` on a text doc.
    pub span: Option<ClaimSpan>,
    /// For [`NoteKind::Overwritten`]: what the change did to your work (a grid: your value;
    /// a text doc: what the outside save may have removed).
    pub overwrite: Option<Overwrite>,
}

/// The most bytes of one value an [`Overwrite`] carries (cut at a character boundary, with
/// `cut` set): on a grid, one cell's value (the full value is in the version the note names);
/// on a text doc, what the file now has, per place.
pub const OVERWRITE_VALUE_MAX: usize = 256;
/// The most cells (a grid: one row's, for one author) or places (a text doc) one [`Overwrite`]
/// lists; the rest are counted in `more`. So one grid event is at most about 20 KiB.
pub const OVERWRITE_CELLS_MAX: usize = 32;
/// The most notes waiting for one participant on one doc: past it, the last is a roll-up.
pub const OVERWRITE_PENDING_MAX: usize = 64;
/// The most text one text-doc note carries, all its places together: the author's own removed
/// text and what the file now has. The rest is counted in [`Overwrite::not_shown`].
pub const OVERWRITE_TEXT_MAX: usize = 16 * 1024;
/// The most note text waiting on one text doc, everyone's together, loaded or not. Past it the
/// oldest notes lose their text (the fact stays, and says so). One author's full queue fits
/// (64 x 16 KiB); so does a 32-participant doc's normal load, whose notes are a few lines.
pub const OVERWRITE_DOC_TEXT_MAX: usize = 1 << 20;

/// `s` capped to [`OVERWRITE_VALUE_MAX`] bytes, and whether it was cut.
pub(crate) fn cap_value(s: &str) -> (String, bool) {
    if s.len() <= OVERWRITE_VALUE_MAX {
        return (s.to_owned(), false);
    }
    let mut end = OVERWRITE_VALUE_MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_owned(), true)
}

/// One more cell (a grid: another live cell of the same row and author) or place (a text doc)
/// of an [`Overwrite`]. `Debug` doesn't print the values.
#[derive(Clone, PartialEq, Eq)]
pub struct OverwriteCell {
    pub region: String,
    pub col: Option<usize>,
    pub yours: String,
    pub file: String,
    /// `yours` or `file` was cut to [`OVERWRITE_VALUE_MAX`].
    pub cut: bool,
}

impl fmt::Debug for OverwriteCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverwriteCell")
            .field("region", &self.region)
            .field("col", &self.col)
            .field("yours", &format_args!("<{} bytes>", self.yours.len()))
            .field("file", &format_args!("<{} bytes>", self.file.len()))
            .field("cut", &self.cut)
            .finish()
    }
}

/// What a change did to someone's live work, and where to get it back. Two kinds, one per doc
/// kind (a doc only ever makes its own kind):
///
/// - **A grid doc** (contract §2.9.1): an outside save, a stale write, a revert or a restore
///   won over a live value someone wrote, and the live value is kept in a version. One per row
///   per author: the first cell in the fields here, the row's others in `also`. Kept in the
///   store until delivered.
/// - **A text doc**: an outside save may have removed or replaced text someone wrote through
///   the kit (their own, or an assistant's on their behalf): one note per change per person,
///   the first place in the fields here, the others in `also`. "May have": the kit can't tell a
///   removal from a move, or from the author deleting their own line in another program. It
///   carries **the person's own removed text**, so they can paste it back: only their own
///   characters, never someone else's in the same removed block, up to [`OVERWRITE_TEXT_MAX`]
///   for the whole note (`not_shown` counts the rest). In memory only.
///
/// Past the bound per person, a **roll-up** ([`Overwrite::is_rollup`]) counts the rest in
/// `more`; a note is never dropped silently. `Debug` doesn't print the values (a host's
/// catch-all log stays free of document text); `Display` does, for the person it is for.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Overwrite {
    /// The rule. A grid: the merge rule (`cell`, `name`, `quoting`, `edited_row`,
    /// `deleted_row`, `moved_row`, the column forms, `dropped_row`, `dropped_col`). A text doc:
    /// `removed_text`, `replaced_text`. `rollup` for a roll-up, on either.
    pub rule: &'static str,
    /// What did it: `an outside save`, `a write`, `a revert`, `a restore`, `rows drop-deleted`
    /// (a text doc: `an outside save` only).
    pub source: &'static str,
    /// Where. A grid: the element by key (`r2/c3`, `row r4`, `col c2`, `name of c2`). A text
    /// doc: `L12` or `L12–L14`, in the text before the change; with more than one root,
    /// `root:L12`.
    pub region: String,
    /// A grid: its row's key, for a cell or a row; a client dedupes on `(change, row)`. `None`
    /// on a text doc, where `(change, row)` is unique because a change makes one note per
    /// person.
    pub row: Option<String>,
    /// Its rendered line (1-based) and column (1-based) in the live file before the change,
    /// when it had them (a text doc: its first line in the text before the change).
    pub line: Option<usize>,
    pub col: Option<usize>,
    /// A grid: the live value (or `deleted`, a place `#3 (after r2)`). A text doc: this
    /// person's own removed text there, exactly as it was (newlines included): what they can
    /// paste back; empty for text someone else wrote that this person put back by an undo.
    pub yours: String,
    /// A grid: the file's value. A text doc: those lines as they read after the change (empty
    /// when nothing is left of them).
    pub file: String,
    /// `yours` or `file` was cut to [`OVERWRITE_VALUE_MAX`] (a text doc: see `not_shown`).
    pub cut: bool,
    /// The change that did it.
    pub change: crate::store::Seq,
    /// The version that holds the live value (`show --version`). On a text doc, **one this
    /// reader can open** ([`super::Session::show`]): `None` when they have no history right
    /// ([`Overwrite::no_history`]), or it can't be named any more (a purge, or retention).
    pub version: Option<crate::store::VersionId>,
    /// A text doc: that version, for telling someone with history access; set whether this
    /// reader can open it or not. Never link a reader to this one: that's `version`. `None` on
    /// a grid.
    pub restore_from: Option<crate::store::VersionId>,
    /// The other cells of this row (a grid) or places (a text doc), for this person.
    pub also: Vec<OverwriteCell>,
    /// More cells or places not listed (past [`OVERWRITE_CELLS_MAX`], or a roll-up's count).
    pub more: usize,
    /// A text doc: characters of this person's own removed text the note doesn't carry (past
    /// [`OVERWRITE_TEXT_MAX`], or in places past [`OVERWRITE_CELLS_MAX`]). 0 on a grid.
    pub not_shown: usize,
    /// The values aren't here: the reader can't read the doc now, a purge took them, or (a
    /// text doc) the hold expired or the doc's total cap took them. `yours` and `file` are
    /// empty, the fact and the count stay.
    pub withheld: bool,
    /// A text doc: the reader has no history right: `version` is `None`, and they're told who
    /// can help. `false` on a grid.
    pub no_history: bool,
    pub(crate) before: crate::store::Seq,
    pub(crate) at_ms: u64,
    /// A text doc's note (its rules, its sentences, memory only); a grid's otherwise.
    pub(crate) text: bool,
}

impl fmt::Debug for Overwrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Overwrite")
            .field("rule", &self.rule)
            .field("source", &self.source)
            .field("region", &self.region)
            .field("line", &self.line)
            .field("col", &self.col)
            .field("yours", &format_args!("<{} bytes>", self.yours.len()))
            .field("file", &format_args!("<{} bytes>", self.file.len()))
            .field("cut", &self.cut)
            .field("change", &self.change)
            .field("version", &self.version)
            .field("also", &self.also)
            .field("more", &self.more)
            .field("not_shown", &self.not_shown)
            .field("restore_from", &self.restore_from)
            .field("withheld", &self.withheld)
            .field("no_history", &self.no_history)
            .finish()
    }
}

impl Overwrite {
    /// A grid note: the version that holds the live value, given the newest one before the
    /// change: none for a drop (a version holds live elements only, and a dropped one wasn't
    /// live): the note itself carries the value, up to [`OVERWRITE_VALUE_MAX`] (`cut` says when
    /// it was cut).
    pub(crate) fn version_of(
        &self,
        before: Option<crate::store::VersionId>,
    ) -> Option<crate::store::VersionId> {
        if self.source == "rows drop-deleted" {
            None
        } else {
            before
        }
    }

    /// The roll-up form: no cell or place, `more` counts them.
    pub fn is_rollup(&self) -> bool {
        self.rule == "rollup"
    }

    /// What the other value is called in a grid note's line.
    fn theirs_word(&self) -> &'static str {
        match self.source {
            "an outside save" | "a write" => "the file's",
            _ => "now",
        }
    }

    /// How many cells or places this note speaks for.
    pub fn cells(&self) -> usize {
        usize::from(!self.is_rollup()) + self.also.len() + self.more
    }

    /// How much text it holds.
    pub(crate) fn text_bytes(&self) -> usize {
        self.yours.len()
            + self.file.len()
            + self
                .also
                .iter()
                .map(|c| c.yours.len() + c.file.len())
                .sum::<usize>()
    }

    /// Without its values (the hold's expiry, the doc's total cap): the fact and the count stay,
    /// and the version is still named.
    pub(crate) fn strip(&mut self) {
        self.yours.clear();
        self.file.clear();
        self.cut = false;
        self.withheld = true;
        for c in &mut self.also {
            c.yours.clear();
            c.file.clear();
            c.cut = false;
        }
    }

    /// A purge, or a reader who can't read the doc now: the values go, the fact and the count
    /// stay. The note also loses its version, which can't be vouched for any more: no
    /// `version`, no `restore_from`, both kinds (one rule for hosts; a grid's delivery names
    /// none for a withheld note either).
    pub(crate) fn withhold(&mut self) {
        self.strip();
        self.version = None;
        self.restore_from = None;
    }

    /// A grid note, for the store (`overwrite_notes`). A text doc's notes are never stored.
    pub(crate) fn encode(&self, w: &mut Vec<u8>) {
        fn st(w: &mut Vec<u8>, s: &str) {
            w.extend_from_slice(&(s.len() as u64).to_le_bytes());
            w.extend_from_slice(s.as_bytes());
        }
        fn n(w: &mut Vec<u8>, v: u64) {
            w.extend_from_slice(&v.to_le_bytes());
        }
        fn opt(w: &mut Vec<u8>, v: Option<usize>) {
            n(w, v.map_or(u64::MAX, |v| v as u64));
        }
        debug_assert!(!self.text, "a text doc's notes are never stored");
        st(w, self.rule);
        st(w, self.source);
        st(w, &self.region);
        st(w, self.row.as_deref().unwrap_or(""));
        opt(w, self.line);
        opt(w, self.col);
        st(w, &self.yours);
        st(w, &self.file);
        n(w, u64::from(self.cut));
        n(w, self.change);
        n(w, self.more as u64);
        n(w, u64::from(self.withheld));
        n(w, self.before);
        n(w, self.at_ms);
        n(w, self.also.len() as u64);
        for c in &self.also {
            st(w, &c.region);
            opt(w, c.col);
            st(w, &c.yours);
            st(w, &c.file);
            n(w, u64::from(c.cut));
        }
    }

    /// One grid note from `b` at `*at`; `None`: not a note this build wrote.
    pub(crate) fn decode(b: &[u8], at: &mut usize) -> Option<Overwrite> {
        fn n(b: &[u8], at: &mut usize) -> Option<u64> {
            let s = b.get(*at..at.checked_add(8)?)?;
            *at += 8;
            Some(u64::from_le_bytes(s.try_into().ok()?))
        }
        fn st(b: &[u8], at: &mut usize) -> Option<String> {
            let l = usize::try_from(n(b, at)?).ok()?;
            let s = b.get(*at..at.checked_add(l)?)?;
            *at += l;
            String::from_utf8(s.to_vec()).ok()
        }
        fn opt(b: &[u8], at: &mut usize) -> Option<Option<usize>> {
            let v = n(b, at)?;
            Some((v != u64::MAX).then_some(v as usize))
        }
        const RULES: &[&str] = &[
            "cell",
            "name",
            "quoting",
            "edited_row",
            "deleted_row",
            "moved_row",
            "edited_col",
            "deleted_col",
            "moved_col",
            "dropped_row",
            "dropped_col",
            "rollup",
        ];
        const SOURCES: &[&str] = &[
            "an outside save",
            "a write",
            "a revert",
            "a restore",
            "rows drop-deleted",
        ];
        let rule = st(b, at)?;
        let rule = *RULES.iter().find(|r| **r == rule)?;
        let source = st(b, at)?;
        let source = *SOURCES.iter().find(|r| **r == source)?;
        let region = st(b, at)?;
        let row = Some(st(b, at)?).filter(|r| !r.is_empty());
        let line = opt(b, at)?;
        let col = opt(b, at)?;
        let yours = st(b, at)?;
        let file = st(b, at)?;
        let cut = n(b, at)? != 0;
        let change = n(b, at)?;
        let more = usize::try_from(n(b, at)?).ok()?;
        let withheld = n(b, at)? != 0;
        let before = n(b, at)?;
        let at_ms = n(b, at)?;
        let len = usize::try_from(n(b, at)?).ok()?;
        let mut also = Vec::new();
        for _ in 0..len {
            also.push(OverwriteCell {
                region: st(b, at)?,
                col: opt(b, at)?,
                yours: st(b, at)?,
                file: st(b, at)?,
                cut: n(b, at)? != 0,
            });
        }
        Some(Overwrite {
            rule,
            source,
            region,
            row,
            line,
            col,
            yours,
            file,
            cut,
            change,
            version: None,
            restore_from: None,
            also,
            more,
            not_shown: 0,
            withheld,
            no_history: false,
            before,
            at_ms,
            text: false,
        })
    }

    /// A grid note's line (contract §2.9.1), pinned in the grid tests.
    fn fmt_grid(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_rollup() {
            write!(f, "{} more cells overwritten, see the log", self.more)?;
            if let Some(v) = self.version {
                write!(f, "; yours are in version {v} or earlier")?;
            }
            return Ok(());
        }
        let what = match self.rule {
            "edited_row" | "edited_col" => "deleted",
            "deleted_row" | "deleted_col" => "brought back and wrote",
            "moved_row" | "moved_col" => "moved",
            "dropped_row" | "dropped_col" => "removed",
            _ => "overwrote",
        };
        write!(f, "{} {what} {}", self.source, one_line(&self.region))?;
        match (self.line, self.col) {
            (Some(l), Some(c)) => write!(f, " (L{l}, col {c})")?,
            (Some(l), None) => write!(f, " (L{l})")?,
            (None, Some(c)) => write!(f, " (col {c})")?,
            (None, None) => {}
        }
        if self.withheld {
            write!(f, " (change {})", self.change)?;
        } else {
            write!(
                f,
                ": yours {}, {} {} (change {})",
                one_line(&self.yours),
                self.theirs_word(),
                one_line(&self.file),
                self.change
            )?;
        }
        for c in &self.also {
            if self.withheld {
                write!(f, "; also {}", one_line(&c.region))?;
            } else {
                write!(
                    f,
                    "; also {}: yours {}, {} {}",
                    one_line(&c.region),
                    one_line(&c.yours),
                    self.theirs_word(),
                    one_line(&c.file)
                )?;
            }
        }
        if self.more > 0 {
            write!(f, "; and {} more cells", self.more)?;
        }
        if let Some(v) = self.version {
            write!(f, "; yours is in version {v}")?;
        }
        Ok(())
    }

    /// A text doc's note: its sentences. "May have" throughout: a moved line reads as removed.
    /// It never tells someone to open a version they can't. Pinned in
    /// `tests/outside_removal_notes.rs`.
    fn fmt_text(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_rollup() {
            write!(
                f,
                "An outside save may have removed or replaced your text in {} more places. See change {} in the log.",
                self.more, self.change
            )?;
            return match (self.version, self.restore_from) {
                (Some(v), _) => write!(f, " Version {v} holds the latest of it."),
                (None, Some(r)) => write!(
                    f,
                    " Someone with history access can restore the latest of it from version {r}."
                ),
                (None, None) => Ok(()),
            };
        }
        let verb = if self.rule == "replaced_text" {
            "replaced"
        } else {
            "removed"
        };
        write!(
            f,
            "An outside save may have {verb} your text ({}",
            one_line(&self.region)
        )?;
        for c in &self.also {
            write!(f, ", {}", one_line(&c.region))?;
        }
        if self.more > 0 {
            write!(f, ", and {} more places", self.more)?;
        }
        write!(f, ").")?;
        if self.withheld {
            write!(f, " The text is no longer held here.")?;
        }
        match (self.version, self.restore_from) {
            (Some(v), _) => write!(
                f,
                " Version {v} holds it. Copy it back from there: restoring that version replaces the outside save's change and everything since."
            )?,
            // no history: who can help, once (a cut note's last sentence says it instead)
            (None, Some(r)) if self.withheld || self.not_shown == 0 => write!(
                f,
                " Someone with history access can restore it from version {r}."
            )?,
            (None, Some(_)) => {}
            (None, None) => write!(f, " See change {} in the log.", self.change)?,
        }
        if self.withheld {
            return Ok(());
        }
        let one = |f: &mut fmt::Formatter<'_>, yours: &str, file: &str, cut: bool| {
            if !yours.is_empty() {
                write!(f, "It read {}", value_text(yours, cut))?;
                if !file.is_empty() {
                    write!(f, "; the file now has {}", value_text(file, false))?;
                }
                write!(f, ".")?;
            }
            Ok::<(), fmt::Error>(())
        };
        if self.also.is_empty() {
            if !self.yours.is_empty() {
                write!(f, " ")?;
            }
            one(f, &self.yours, &self.file, self.cut)?;
        } else {
            if !self.yours.is_empty() {
                write!(f, " {}: ", one_line(&self.region))?;
            }
            one(f, &self.yours, &self.file, self.cut)?;
            for c in &self.also {
                if !c.yours.is_empty() {
                    write!(f, " {}: ", one_line(&c.region))?;
                }
                one(f, &c.yours, &c.file, c.cut)?;
            }
        }
        if self.not_shown > 0 {
            write!(f, " {} more characters not shown", self.not_shown)?;
            match (self.version, self.restore_from) {
                (Some(v), _) => write!(f, "; version {v} has them.")?,
                (None, Some(r)) => write!(
                    f,
                    "; someone with history access can restore it from version {r}."
                )?,
                (None, None) => write!(f, ".")?,
            }
        }
        Ok(())
    }
}

/// `s` on one line and exactly recoverable: `\\`, `\"`, `\n`, `\r`, `\t` escaped, so an agent
/// reading the note on stderr can paste its text back as it was.
fn escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = fmt::Write::write_fmt(&mut out, format_args!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn value_text(s: &str, cut: bool) -> String {
    if cut {
        format!("\"{}…\"", escaped(s))
    } else {
        format!("\"{}\"", escaped(s))
    }
}

impl fmt::Display for Overwrite {
    /// One line, without the `note: ` prefix: a grid note's line or a text note's sentences,
    /// by the note's kind.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.text {
            self.fmt_text(f)
        } else {
            self.fmt_grid(f)
        }
    }
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let w = who(&self.name, &self.pid);
        let l = match &self.span {
            Some(s) => s.to_string(),
            None => lines_text(&self.lines),
        };
        let age = duration(self.age_ms);
        match self.kind {
            NoteKind::Claimed => {
                write!(f, "note: {w} claimed {l}")?;
                if let Some(n) = &self.claim_note {
                    write!(f, " {}", quoted(n))?;
                }
                write!(f, " ({age} ago")?;
                if let Some(r) = self.remaining_ms {
                    write!(f, ", {} left", duration(r))?;
                }
                write!(f, ")")
            }
            NoteKind::Selection => write!(f, "note: {w} selected {l} ({age} ago)"),
            NoteKind::Caret => write!(f, "note: {w} has a caret at {l} ({age} ago)"),
            NoteKind::Edited => write!(f, "note: {w} edited {l} ({age} ago)"),
            NoteKind::Viewing => write!(f, "note: {w} viewing {l} ({age} ago)"),
            NoteKind::Overwritten => match &self.overwrite {
                Some(o) => write!(f, "note: {o}"),
                None if self.lines.root == crate::table::ROWS => {
                    write!(f, "note: an outside save overrode your change at {l}")
                }
                None => write!(f, "note: an outside save may have removed your text at {l}"),
            },
        }
    }
}

/// The heads-up notes of one call. Deduped per (reader, other, kind, range) within
/// [`super::Config::note_repeat_ms`], and rate-limited per (reader, doc): at most one batch
/// per [`super::Config::note_interval_ms`], of at most [`super::Config::note_batch_max`]
/// notes (the rest counted in `more`). Notes the rate limit holds back are counted in
/// `held` and come in a later batch if they still apply.
///
/// [`fmt::Display`] is one line per note, each starting `note: `, then `note: +N more`
/// when `more > 0`; nothing for an empty batch. **Print it on stderr only**: the kit never
/// puts a note in `stdout`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Notes {
    pub notes: Vec<Note>,
    /// Notes over the batch cap, not shown in this batch.
    pub more: usize,
    /// Notes held back by the rate limit (none shown in this batch).
    pub held: usize,
}

impl Notes {
    /// No line to print.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.more == 0
    }

    /// The lines [`fmt::Display`] prints, each starting `note: `.
    pub fn lines(&self) -> Vec<String> {
        let mut v: Vec<String> = self.notes.iter().map(Note::to_string).collect();
        if self.more > 0 {
            v.push(format!("note: +{} more", self.more));
        }
        v
    }
}

impl fmt::Display for Notes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.lines().join("\n"))
    }
}

/// What an [`ActivityEntry`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ActivityKind {
    /// A read verb (through `run_tool`).
    Read,
    /// A change (any edit path, undo, redo, revert, restore).
    Edit,
    /// A claim taken (explicitly, or by `--for-edit`).
    Claim,
    /// A claim ended, and how.
    ClaimEnded(ClaimEnd),
}

impl ActivityKind {
    /// `read`, `edit`, `claim`, `claim-ended`.
    pub fn as_str(self) -> &'static str {
        match self {
            ActivityKind::Read => "read",
            ActivityKind::Edit => "edit",
            ActivityKind::Claim => "claim",
            ActivityKind::ClaimEnded(_) => "claim-ended",
        }
    }
}

/// One row of [`super::Session::activity`]: who did what, where, when. Line numbers are as
/// they were at the time (a log, not anchors). Field order is the stable JSON order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ActivityEntry {
    pub pid: String,
    pub name: String,
    pub kind: ActivityKind,
    /// The verb (`grep`, `sed -i`, `claim`...), or the change kind for a direct API call
    /// (`edit`, `undo`, `restore`...).
    pub verb: String,
    pub lines: Option<LineRange>,
    pub at_ms: u64,
    /// The claim, for claim rows.
    pub claim: Option<ClaimId>,
}

// ---------- internal state ----------

/// A live claim (session state).
pub(crate) struct Claim {
    pub id: ClaimId,
    pub pid: String,
    pub name: String,
    pub conn: ConnId,
    pub root: String,
    /// Byte span, carried: start `Bias::Before`, end `Bias::After`; `s <= e`.
    pub s: usize,
    pub e: usize,
    pub taken_ms: u64,
    pub expires_ms: u64,
    pub note: Option<String>,
    pub hold: bool,
    pub for_edit: bool,
    /// FIFO.
    pub waiters: VecDeque<(ConnId, String)>,
    /// Touched by its owner's edit that is being committed (ends it once durable).
    pub edited: bool,
    /// A grid claim (A10): its rows and/or columns by key (`root` is
    /// [`crate::table::CELLS`], `s`/`e` unused).
    pub grid: Option<GridSpan>,
    /// An element claim: its list root's keys (`root` is that list, `s`/`e` unused).
    pub elems: Option<ElemSpan>,
}

/// An element claim's keys in one list root. A key stays when its element is deleted: a restore
/// brings it back under the claim, and the claim ends only when none is live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElemSpan {
    pub root: String,
    pub keys: BTreeSet<Key>,
}

impl ElemSpan {
    pub fn overlaps(&self, other: &ElemSpan) -> bool {
        self.root == other.root && !self.keys.is_disjoint(&other.keys)
    }

    pub fn span(&self) -> ClaimSpan {
        ClaimSpan::Elems {
            root: self.root.clone(),
            keys: self.keys.iter().cloned().collect(),
        }
    }
}

/// A grid claim's ends by key, per axis (`None`: the whole axis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GridSpan {
    pub rows: Option<(Key, Key)>,
    pub cols: Option<(Key, Key)>,
}

impl GridSpan {
    pub fn of(span: &ClaimSpan) -> Option<Self> {
        Some(match span {
            ClaimSpan::Lines(_) | ClaimSpan::Elems { .. } | ClaimSpan::Nodes { .. } => return None,
            ClaimSpan::Rows { first, last } => GridSpan {
                rows: Some((first.clone(), last.clone())),
                cols: None,
            },
            ClaimSpan::Cols { first, last } => GridSpan {
                rows: None,
                cols: Some((first.clone(), last.clone())),
            },
            ClaimSpan::Cells { rows, cols } => GridSpan {
                rows: Some(rows.clone()),
                cols: Some(cols.clone()),
            },
        })
    }

    pub fn span(&self) -> ClaimSpan {
        match (&self.rows, &self.cols) {
            (Some(r), None) => ClaimSpan::Rows {
                first: r.0.clone(),
                last: r.1.clone(),
            },
            (None, Some(c)) => ClaimSpan::Cols {
                first: c.0.clone(),
                last: c.1.clone(),
            },
            (Some(r), Some(c)) => ClaimSpan::Cells {
                rows: r.clone(),
                cols: c.clone(),
            },
            (None, None) => unreachable!("a grid span has an axis"),
        }
    }
}

impl Claim {
    pub fn remaining(&self, now: u64) -> u64 {
        self.expires_ms.saturating_sub(now)
    }

    /// Its span overlaps `[s, e]` of `root` (inclusive: line spans never share a byte).
    pub fn overlaps(&self, root: &str, s: usize, e: usize) -> bool {
        self.root == root && overlap((self.s, self.e), (s, e))
    }

    /// Carry it through `hunks` with the biases [`claim_fates`] gave it.
    pub fn carry(&mut self, hunks: &[Hunk], fate: Fate) {
        let s = position::transform(self.s, hunks, fate.s);
        self.e = position::transform(self.e, hunks, fate.e).max(s);
        self.s = s;
    }

    /// Waiters other than the owner.
    pub fn contended(&self) -> bool {
        self.waiters.iter().any(|(_, p)| *p != self.pid)
    }

    pub fn info(&self, text: &str, now: u64) -> ClaimInfo {
        ClaimInfo {
            id: self.id,
            pid: self.pid.clone(),
            name: self.name.clone(),
            conn: self.conn,
            lines: span_lines(&self.root, text, self.s, self.e),
            span: ClaimSpan::Lines(span_lines(&self.root, text, self.s, self.e)),
            start: self.s,
            end: self.e,
            taken_ms: self.taken_ms,
            expires_ms: self.expires_ms,
            remaining_ms: self.remaining(now),
            note: self.note.clone(),
            hold: self.hold,
            for_edit: self.for_edit,
            waiters: self.waiters.iter().map(|(_, p)| p.clone()).collect(),
        }
    }
}

/// A hand-off reservation (session state).
pub(crate) struct Reservation {
    /// The claim that ended.
    pub claim: ClaimId,
    pub pid: String,
    pub name: String,
    pub conn: ConnId,
    pub root: String,
    pub s: usize,
    pub e: usize,
    pub until_ms: u64,
    /// The waiters after this one, FIFO.
    pub queue: VecDeque<(ConnId, String)>,
    /// Touched by the reserved waiter's change being committed (ends it once durable).
    pub edited: bool,
    /// A grid reservation: the ended claim's span by key (`root` is
    /// [`crate::table::CELLS`], `s`/`e` unused).
    pub grid: Option<GridSpan>,
    /// An element reservation: the ended claim's keys (`root` is the list, `s`/`e` unused).
    pub elems: Option<ElemSpan>,
}

impl Reservation {
    pub fn remaining(&self, now: u64) -> u64 {
        self.until_ms.saturating_sub(now)
    }

    pub fn overlaps(&self, root: &str, s: usize, e: usize) -> bool {
        self.root == root && overlap((self.s, self.e), (s, e))
    }

    pub fn touched_by(&self, root: &str, hunks: &[Hunk]) -> bool {
        self.root == root && hunks.iter().any(|h| touches(h, self.s, self.e))
    }

    pub fn carry(&mut self, hunks: &[Hunk]) {
        (self.s, self.e) = carry_span(self.s, self.e, hunks);
    }

    pub fn info(&self, text: &str, now: u64) -> ReservationInfo {
        ReservationInfo {
            claim: self.claim,
            pid: self.pid.clone(),
            name: self.name.clone(),
            conn: self.conn,
            lines: span_lines(&self.root, text, self.s, self.e),
            span: ClaimSpan::Lines(span_lines(&self.root, text, self.s, self.e)),
            start: self.s,
            end: self.e,
            until_ms: self.until_ms,
            remaining_ms: self.remaining(now),
            waiters: self.queue.iter().map(|(_, p)| p.clone()).collect(),
        }
    }
}

/// Dedupe key: (reader, other, kind, root, first line, last line).
pub(crate) type NoteKey = (String, String, NoteKind, String, usize, usize);

/// A session's presence extras. Never persisted.
#[derive(Default)]
pub(crate) struct Aware {
    pub claims: Vec<Claim>,
    pub reservations: Vec<Reservation>,
    pub activity: VecDeque<ActivityEntry>,
    /// When a note was last delivered, per dedupe key.
    pub noted: HashMap<NoteKey, u64>,
    /// When a reader (pid) last got a non-empty batch.
    pub batches: HashMap<String, u64>,
    /// The [`NoteKind::Overwritten`] notes waiting for each participant (by pid), whether they
    /// are here or not, delivered with its next verb (a grid: its next batch) or by
    /// `take_notes`, whatever the rate limit. A grid doc's are a mirror of the store's
    /// `overwrite_notes`, read once (`notes_loaded`). A text doc's are in memory: they survive
    /// an unload (without their values, through the manager) and not a restart.
    pub pending: HashMap<String, Vec<Note>>,
    pub notes_loaded: bool,
}

/// Most dedupe entries kept per doc (the oldest go first past it).
const NOTED_MAX: usize = 4096;

impl Aware {
    pub fn push_activity(&mut self, e: ActivityEntry, max: usize) {
        if max == 0 {
            return;
        }
        while self.activity.len() >= max {
            self.activity.pop_front();
        }
        self.activity.push_back(e);
    }

    /// Forget dedupe and rate-limit entries older than `window`, and keep the map bounded.
    pub fn prune_noted(&mut self, now: u64, repeat_ms: u64, interval_ms: u64) {
        self.noted.retain(|_, t| now.saturating_sub(*t) < repeat_ms);
        self.batches
            .retain(|_, t| now.saturating_sub(*t) < interval_ms);
        if self.noted.len() > NOTED_MAX {
            let mut ts: Vec<u64> = self.noted.values().copied().collect();
            ts.sort_unstable();
            let cut = ts[self.noted.len() - NOTED_MAX];
            self.noted.retain(|_, t| *t > cut);
        }
    }

    /// Participant erasure: drop every trace of `pid` (activity, dedupe, rate limit).
    pub fn forget(&mut self, pid: &str) {
        self.pending.remove(pid);
        self.activity.retain(|a| a.pid != pid);
        self.noted.retain(|k, _| k.0 != pid && k.1 != pid);
        self.batches.remove(pid);
        self.pending.remove(pid);
    }
}

// ---------- helpers ----------

/// Inclusive overlap of two byte spans.
pub(crate) fn overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// Whether hunk `h` touches span `[s, e]`.
pub(crate) fn touches(h: &Hunk, s: usize, e: usize) -> bool {
    if h.start == h.end {
        s <= h.start && h.start <= e
    } else {
        h.start < e.max(s + 1) && s < h.end
    }
}

/// How one change meets one live claim ([`claim_fates`]): the biases its start and end are
/// carried with, and whether the change touches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fate {
    pub s: Bias,
    pub e: Bias,
    pub touched: bool,
}

/// How a change by `typist` to `root` (`hunks`, sorted, in the claims' coordinates) meets
/// each live claim, in `claims` order.
///
/// Anchors expand (text typed at a claim's edge lands inside), except where claims of
/// **more than one participant** meet: text inserted at a point two participants' claims
/// both hold (they share an edge there, once the text between them was deleted, or one is
/// empty there) joins **one** claim: the typist's own if they hold one there (the first
/// they acquired), otherwise the one acquired first ([`ClaimId`] order). Every other claim
/// there carries the insertion outside it: an end there stays before it, a start there
/// moves past it, an empty claim goes to the side away from the one that took it; and the
/// insertion doesn't touch it (no `Edited` end, no `Respect` refusal for it). So two
/// participants' claims never come to overlap. One participant's own claims meeting there
/// (and nobody else's) all expand, as before.
pub(crate) fn claim_fates(claims: &[Claim], root: &str, hunks: &[Hunk], typist: &str) -> Vec<Fate> {
    let mut fates: Vec<Fate> = claims
        .iter()
        .map(|_| Fate {
            s: Bias::Before,
            e: Bias::After,
            touched: false,
        })
        .collect();
    // per claim, the insertions it gives up (they join another participant's claim)
    let mut gives_up: Vec<Vec<usize>> = vec![Vec::new(); claims.len()];
    for (j, h) in hunks.iter().enumerate() {
        if h.start != h.end || h.insert.is_empty() {
            continue;
        }
        let p = h.start;
        let holders: Vec<usize> = (0..claims.len())
            .filter(|&k| claims[k].root == root && claims[k].s <= p && p <= claims[k].e)
            .collect();
        let Some(&first) = holders.first() else {
            continue;
        };
        if holders.iter().all(|&k| claims[k].pid == claims[first].pid) {
            continue;
        }
        let pick = |own: bool| {
            holders
                .iter()
                .copied()
                .filter(|&k| !own || claims[k].pid == typist)
                .min_by_key(|&k| claims[k].id)
        };
        let w = pick(true)
            .or_else(|| pick(false))
            .expect("holders is not empty");
        let winner_before = claims[w].s < p;
        for &k in holders.iter().filter(|&&k| k != w) {
            let c = &claims[k];
            let f = &mut fates[k];
            if c.s == p && c.e == p {
                let b = if winner_before {
                    Bias::After
                } else {
                    Bias::Before
                };
                (f.s, f.e) = (b, b);
            } else if c.e == p {
                f.e = Bias::Before;
            } else if c.s == p {
                f.s = Bias::After;
            } else {
                // p inside it: it already overlaps the winner (not reachable while claims
                // never overlap); it keeps expanding
                continue;
            }
            gives_up[k].push(j);
        }
    }
    for (k, c) in claims.iter().enumerate() {
        fates[k].touched = c.root == root
            && hunks
                .iter()
                .enumerate()
                .any(|(j, h)| !gives_up[k].contains(&j) && touches(h, c.s, c.e));
    }
    fates
}

/// Carry an expanding span through `hunks`.
pub(crate) fn carry_span(s: usize, e: usize, hunks: &[Hunk]) -> (usize, usize) {
    let s2 = position::transform(s, hunks, Bias::Before);
    let e2 = position::transform(e, hunks, Bias::After).max(s2);
    (s2, e2)
}

/// Carry a participant's last-edit span through a change, **not expanding**: text inserted
/// at either edge lands outside, a replaced or deleted edge moves inward past it. `None`
/// when a span that held text holds none any more (its text was all removed or replaced:
/// what is there now is someone else's). An empty span (an edit that left no text, a
/// deletion) stays a point. Claims and views keep expanding anchors ([`carry_span`]).
pub(crate) fn carry_edit_span(s: usize, e: usize, hunks: &[Hunk]) -> Option<(usize, usize)> {
    // the first character still in the span: past anything inserted at or replacing `s`
    let start = |pos: usize| -> usize {
        let mut delta: isize = 0;
        for h in hunks {
            if pos < h.start {
                break;
            }
            let grow = h.insert.len() as isize - (h.end - h.start) as isize;
            if h.start < h.end && pos < h.end {
                return (h.start as isize + delta) as usize + h.insert.len();
            }
            delta += grow;
        }
        (pos as isize + delta) as usize
    };
    // the end (exclusive): before anything inserted at or replacing up to `e`
    let end = |pos: usize| -> usize {
        let mut delta: isize = 0;
        for h in hunks {
            if pos <= h.start {
                break;
            }
            let grow = h.insert.len() as isize - (h.end - h.start) as isize;
            if pos <= h.end && h.start < h.end {
                return (h.start as isize + delta) as usize;
            }
            delta += grow;
        }
        (pos as isize + delta) as usize
    };
    let (s2, e2) = (start(s), end(e));
    if s < e {
        (s2 < e2).then_some((s2, e2))
    } else {
        // a point: where an insertion at it leaves the text after it
        Some((s2, s2))
    }
}

/// The lines a last-edit span covers now, clamped to the lines the text has (never the
/// empty line after a final `\n`); `None` for an empty text.
pub(crate) fn edit_span_lines(root: &str, text: &str, s: usize, e: usize) -> Option<LineRange> {
    if text.is_empty() {
        return None;
    }
    let lines = position::line_of(text, text.len()) - usize::from(text.ends_with('\n'));
    let first = position::line_of(text, s).min(lines);
    let last = position::line_of(text, e).clamp(first, lines);
    Some(LineRange {
        root: root.to_owned(),
        first,
        last,
    })
}

/// The byte span of lines `first..=last` of `text` (`last` clamped), or `None` when the
/// lines don't exist.
pub(crate) fn line_span(text: &str, first: usize, last: usize) -> Option<(usize, usize, usize)> {
    let count = position::line_count(text);
    if first == 0 || first > last || first > count {
        return None;
    }
    let last = last.min(count);
    let s = position::line_start(text, first)?;
    let e = position::line_end_from(text, position::line_start(text, last)?);
    Some((s, e, last))
}

/// The lines a byte span covers now.
pub(crate) fn span_lines(root: &str, text: &str, s: usize, e: usize) -> LineRange {
    LineRange {
        root: root.to_owned(),
        first: position::line_of(text, s),
        last: position::line_of(text, e),
    }
}

/// `line 5` or `lines 5–8`.
pub(crate) fn lines_text(l: &LineRange) -> String {
    pair_text((l.first, l.last))
}

/// `line 5` or `lines 5–8`, from `(first, last)`.
pub(crate) fn pair_text((first, last): (usize, usize)) -> String {
    if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}–{last}")
    }
}

/// `8 s`, `3 min`, `2 h` (rounded down; under a second is `0 s`).
pub(crate) fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h", s / 3600)
    }
}

/// Control characters escaped, so a note stays on one line.
pub(crate) fn one_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", one_line(s).replace('"', "\\\""))
}

/// `Name (pid)`, or just the pid when the name is it or empty.
fn who(name: &str, pid: &str) -> String {
    if name.is_empty() || name == pid {
        one_line(pid)
    } else {
        format!("{} ({})", one_line(name), one_line(pid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(start: usize, end: usize, insert: &str) -> Hunk {
        Hunk {
            start,
            end,
            insert: insert.into(),
        }
    }

    /// Last-edit spans don't expand: text typed at an edge stays out, and a span whose text
    /// is all removed or replaced is gone (finding F2: notes naming someone's newer text).
    #[test]
    fn edit_spans_do_not_expand() {
        // "aaa|BBB|ccc": the span is BBB, bytes 3..6
        let (s, e) = (3, 6);
        assert_eq!(
            carry_edit_span(s, e, &[h(3, 3, "xx")]),
            Some((5, 8)),
            "typed before"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(6, 6, "xx")]),
            Some((3, 6)),
            "typed after"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(4, 4, "xx")]),
            Some((3, 8)),
            "typed inside"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(0, 1, "")]),
            Some((2, 5)),
            "before it"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(7, 9, "")]),
            Some((3, 6)),
            "after it"
        );
        assert_eq!(carry_edit_span(s, e, &[h(3, 6, "")]), None, "all deleted");
        assert_eq!(
            carry_edit_span(s, e, &[h(3, 6, "YYY")]),
            None,
            "all replaced"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(2, 7, "Z")]),
            None,
            "replaced with more"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(2, 4, "zz")]),
            Some((4, 6)),
            "start replaced"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(5, 8, "zz")]),
            Some((3, 5)),
            "end replaced"
        );
        assert_eq!(
            carry_edit_span(s, e, &[h(0, 0, "q"), h(3, 3, "r"), h(6, 6, "t")]),
            Some((5, 8))
        );
        // a point stays a point, after text typed at it
        assert_eq!(carry_edit_span(3, 3, &[h(3, 3, "xx")]), Some((5, 5)));
        assert_eq!(carry_edit_span(3, 3, &[h(1, 5, "")]), Some((1, 1)));
        // expanding claims, for contrast
        assert_eq!(carry_span(s, e, &[h(6, 6, "xx")]), (3, 8));
    }

    /// Last-edit lines never name the empty line after a final newline, nor a line past
    /// the end.
    #[test]
    fn edit_lines_stay_in_the_text() {
        let t = "one\ntwo\n";
        let l = |s, e| edit_span_lines("r", t, s, e).map(|r| (r.first, r.last));
        assert_eq!(l(0, 3), Some((1, 1)));
        assert_eq!(l(4, 7), Some((2, 2)));
        assert_eq!(
            l(4, 8),
            Some((2, 2)),
            "not the empty line after the last newline"
        );
        assert_eq!(l(8, 8), Some((2, 2)));
        assert_eq!(l(50, 60), Some((2, 2)), "clamped");
        assert_eq!(edit_span_lines("r", "", 0, 0), None);
        assert_eq!(
            edit_span_lines("r", "a\nb", 3, 3).map(|r| (r.first, r.last)),
            Some((2, 2))
        );
    }

    /// The expanding convention keeps an empty range valid and takes in text typed at its
    /// edges; the opposite (start After, end Before) inverts on the first keystroke.
    #[test]
    fn expanding_anchors_never_invert() {
        // empty range at 3, typing "xy" there
        let (s, e) = carry_span(3, 3, &[h(3, 3, "xy")]);
        assert_eq!((s, e), (3, 5), "typed text lands inside");
        // the opposite convention inverts
        let s2 = position::transform(3, &[h(3, 3, "xy")], Bias::After);
        let e2 = position::transform(3, &[h(3, 3, "xy")], Bias::Before);
        assert!(s2 > e2, "the contracting convention inverts: {s2} > {e2}");
        // a one-char range, typing at each edge
        assert_eq!(carry_span(3, 4, &[h(3, 3, "a")]), (3, 5));
        assert_eq!(carry_span(3, 4, &[h(4, 4, "a")]), (3, 5));
        // deleted from under it: a point, never inverted
        assert_eq!(carry_span(3, 6, &[h(2, 8, "")]), (2, 2));
    }

    #[test]
    fn touching_rules() {
        // span [4, 9] ("line two" content, its \n at 9 not counted as content)
        assert!(touches(&h(4, 4, "x"), 4, 9), "insert at start edge");
        assert!(touches(&h(9, 9, "x"), 4, 9), "insert at end edge");
        assert!(!touches(&h(10, 10, "x"), 4, 9), "insert after the newline");
        assert!(touches(&h(5, 6, ""), 4, 9), "delete inside");
        assert!(!touches(&h(9, 10, ""), 4, 9), "deleting its newline only");
        assert!(!touches(&h(0, 4, ""), 4, 9), "deleting the line before");
        assert!(touches(&h(0, 5, ""), 4, 9), "deleting into it");
        // an empty span is touched by a hunk across it
        assert!(touches(&h(2, 6, ""), 4, 4));
    }

    #[test]
    fn grid_spans_render_by_key() {
        let k = |s: &str| Key::new(s).unwrap();
        let rows = ClaimSpan::Rows {
            first: k("r12"),
            last: k("r40"),
        };
        assert_eq!(rows.to_string(), "rows r12–r40");
        let one = ClaimSpan::Cols {
            first: k("c2"),
            last: k("c2"),
        };
        assert_eq!(one.to_string(), "col c2");
        let cells = ClaimSpan::Cells {
            rows: (k("r1"), k("r4")),
            cols: (k("c1"), k("c3")),
        };
        assert_eq!(cells.to_string(), "cells r1/c1–r4/c3");
        assert_eq!(
            ClaimSpan::Cells {
                rows: (k("r1"), k("r1")),
                cols: (k("c2"), k("c2")),
            }
            .to_string(),
            "cell r1/c2"
        );
        for s in [rows, one, cells] {
            assert_eq!(GridSpan::of(&s).unwrap().span(), s);
        }
        assert_eq!(ClaimEnd::Deleted.as_str(), "deleted");
        // the contract's note (§6)
        let n = Note {
            pid: "agent-b".into(),
            name: "Agent B".into(),
            kind: NoteKind::Claimed,
            participant_kind: ParticipantKind::Agent,
            lines: LineRange {
                root: "rows".into(),
                first: 13,
                last: 41,
            },
            age_ms: 20_000,
            claim: Some(ClaimId::new(3)),
            claim_note: Some("dedupe emails".into()),
            remaining_ms: Some(40_000),
            span: Some(ClaimSpan::Rows {
                first: k("r12"),
                last: k("r40"),
            }),
            overwrite: None,
        };
        assert_eq!(
            n.to_string(),
            "note: Agent B (agent-b) claimed rows r12–r40 \"dedupe emails\" (20 s ago, 40 s left)"
        );
    }

    #[test]
    fn rendering_is_one_line_with_prefix() {
        let n = Note {
            pid: "agent-b".into(),
            name: "B\nevil".into(),
            kind: NoteKind::Claimed,
            participant_kind: ParticipantKind::Agent,
            lines: LineRange {
                root: "text".into(),
                first: 40,
                last: 60,
            },
            age_ms: 12_400,
            claim: Some(ClaimId::new(3)),
            claim_note: Some("rewriting \"intro\"\n".into()),
            remaining_ms: Some(18_000),
            span: None,
            overwrite: None,
        };
        let s = n.to_string();
        assert!(s.starts_with("note: "), "{s}");
        assert!(!s.contains('\n'), "{s}");
        assert_eq!(
            s,
            "note: B\\nevil (agent-b) claimed lines 40–60 \"rewriting \\\"intro\\\"\\n\" (12 s ago, 18 s left)"
        );
        let all = Notes {
            notes: vec![n.clone(), n],
            more: 3,
            held: 0,
        };
        let lines = all.lines();
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.starts_with("note: ")));
        assert_eq!(lines[2], "note: +3 more");
        assert_eq!(all.to_string().lines().count(), 3);
        assert_eq!(Notes::default().to_string(), "");
        assert_eq!(ClaimId::parse("claim-7"), Some(ClaimId::new(7)));
        assert_eq!(ClaimId::parse("7"), Some(ClaimId::new(7)));
        assert_eq!(ClaimId::parse("x"), None);
        assert_eq!(duration(59_999), "59 s");
        assert_eq!(duration(60_000), "1 min");
    }
}
