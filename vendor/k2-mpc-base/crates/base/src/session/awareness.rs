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
//! near-empty range on the first keystroke at its edge).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use super::participant::{ConnId, ParticipantKind};
use super::position::{self, Bias, LineRange};
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
}

impl ClaimEnd {
    /// `released`, `edited`, `expired`, `left`, `reset`.
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimEnd::Released => "released",
            ClaimEnd::Edited => "edited",
            ClaimEnd::Expired => "expired",
            ClaimEnd::Left => "left",
            ClaimEnd::Reset => "reset",
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
    /// The lines it covers now (1-based, inclusive).
    pub lines: LineRange,
    /// Its byte span in the root's text now (`start <= end`).
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
}

impl ReservationEnd {
    /// `edited`, `claimed`, `lapsed`, `cancelled`, `left`, `reset`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReservationEnd::Edited => "edited",
            ReservationEnd::Claimed => "claimed",
            ReservationEnd::Lapsed => "lapsed",
            ReservationEnd::Cancelled => "cancelled",
            ReservationEnd::Left => "left",
            ReservationEnd::Reset => "reset",
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
    /// Its range now (anchored, expanding).
    pub lines: LineRange,
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
}

impl NoteKind {
    /// `claimed`, `selection`, `caret`, `edited`, `viewing` (for a host's JSON).
    pub fn as_str(self) -> &'static str {
        match self {
            NoteKind::Claimed => "claimed",
            NoteKind::Selection => "selection",
            NoteKind::Caret => "caret",
            NoteKind::Edited => "edited",
            NoteKind::Viewing => "viewing",
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
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let w = who(&self.name, &self.pid);
        let l = lines_text(&self.lines);
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
}

impl Claim {
    pub fn remaining(&self, now: u64) -> u64 {
        self.expires_ms.saturating_sub(now)
    }

    /// Its span overlaps `[s, e]` of `root` (inclusive: line spans never share a byte).
    pub fn overlaps(&self, root: &str, s: usize, e: usize) -> bool {
        self.root == root && overlap((self.s, self.e), (s, e))
    }

    /// A change's `hunks` (old-text coordinates) to `root` touch it: a replacement or
    /// deletion changes a character inside it, or an insertion lands inside it (at either
    /// edge too: the anchors are expanding).
    pub fn touched_by(&self, root: &str, hunks: &[Hunk]) -> bool {
        self.root == root && hunks.iter().any(|h| touches(h, self.s, self.e))
    }

    pub fn carry(&mut self, hunks: &[Hunk]) {
        let (s, e) = carry_span(self.s, self.e, hunks);
        self.s = s;
        self.e = e;
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
        self.activity.retain(|a| a.pid != pid);
        self.noted.retain(|k, _| k.0 != pid && k.1 != pid);
        self.batches.remove(pid);
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

/// Carry an expanding span through `hunks`.
pub(crate) fn carry_span(s: usize, e: usize, hunks: &[Hunk]) -> (usize, usize) {
    let s2 = position::transform(s, hunks, Bias::Before);
    let e2 = position::transform(e, hunks, Bias::After).max(s2);
    (s2, e2)
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
