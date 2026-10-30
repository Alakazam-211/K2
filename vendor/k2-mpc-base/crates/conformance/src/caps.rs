//! Optional capabilities: what a rule needs beyond [`TextReplica`].
//!
//! An engine implements the capabilities it has; a rule that needs one runs on every engine
//! that implements it (the coverage matrix, [`crate::matrix`], records which). The traits
//! hold only what the rules need, in the kit's terms (UTF-8 byte offsets, `u64` agents):
//!
//! - [`Events`]: export what a replica has past a version or what a peer lacks, deliver a
//!   batch (a set, a subset, a re-delivery), with typed refusals ([`EventRefusal`]).
//! - [`Blame`]: each live character, with the agent and the event that inserted it.
//! - [`Anchors`]: a relative position that follows a character through concurrent edits;
//!   a deleted character's anchor resolves to the gap where it was.
//! - [`Undo`]: a participant's undo and redo (all or nothing, `--partial`, `--drop`) over a
//!   doc with two text roots, applied as one attributed batch.
//! - [`Branches`]: a replica forked from this one now, editing as another agent, merged back
//!   by delivering what it has ([`BranchAt`]: forked at any version this replica has had).
//! - [`Purge`]: erase a live span as an event that keeps IDs, lengths and positions, and a
//!   byte scan of everything the replica keeps ([`BlameAt`]: blame at a past version).
//! - [`SpanIds`]: purge a span by the element IDs it covers, deleted elements strictly
//!   inside it included.
//!
//! **Doc-level capabilities** are not [`TextReplica`]s: they are one doc that several agents
//! edit through the engine's history machinery (the kit's is a session over its store; the
//! model's is one replica), like [`Undo`]. Each names its engine's replica type (`Engine`),
//! whose deviation list applies, and keeps its own version identity (`VersionId`):
//!
//! - [`Versions`]: name the current state, show and restore any named state.
//! - [`HistoryPurge`]: purge every copy of a text (live, deleted, in versions), and a byte
//!   scan of everything the doc's history keeps.
//! - [`Erasure`]: participant erasure, the kit's mapping from agents to people.

use std::fmt::Debug;
use std::ops::Range;

use crate::{Hunk, Refused, TextReplica};

/// Why a batch of events was refused. A refused batch changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventRefusal {
    /// An event's parent is neither known nor in the batch.
    MissingDeps,
    /// The batch's events' parents form a cycle.
    Cycle,
    /// An event's ID is known with different content.
    Equivocation,
    /// Any other refusal, with the engine's message.
    Other(String),
}

/// Events: what a replica ships to others, and delivering it.
///
/// A batch is the engine's own unit of shipping (an update, a list of events), in the
/// exporter's causal order. Rules make batches only through this trait (or
/// [`Events::cyclic_batch`]): they never build engine bytes.
pub trait Events: TextReplica {
    type Batch: Clone + Debug;

    /// Everything this replica has past `since`, a version this replica has had (from
    /// [`TextReplica::version`]).
    fn events_since(&self, since: &Self::Version) -> Self::Batch;

    /// Everything this replica has that `peer` lacks.
    fn missing_for(&self, peer: &Self) -> Self::Batch;

    /// Deliver `batch` as one update: all of it, or (refused) none of it. Events already
    /// known are skipped.
    fn deliver(&mut self, batch: &Self::Batch) -> Result<(), EventRefusal>;

    /// Test input: a batch whose events' parents form a cycle, built from the engine's own
    /// event type. `None` for an engine whose events are only wire bytes: hand-made bytes
    /// belong to the native decoder's own tests (G2), not to this suite.
    fn cyclic_batch() -> Option<Self::Batch> {
        None
    }
}

/// One live character and who inserted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blamed<I> {
    pub ch: char,
    /// The agent that inserted it.
    pub agent: u64,
    /// The inserting event's ID (unique per character, the same on every replica).
    pub id: I,
}

/// Blame at the character's grain.
pub trait Blame: TextReplica {
    type CharId: Clone + Ord + Debug;

    /// Every live character of [`TextReplica::text`], in order, with its inserting event.
    fn blame(&self) -> Vec<Blamed<Self::CharId>>;
}

/// Which character an anchor follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The character before the offset: the anchor stays just after it.
    Left,
    /// The character after the offset: the anchor stays just before it.
    Right,
}

/// Relative positions (cursors, comment anchors).
pub trait Anchors: TextReplica {
    /// Valid on every replica of the doc on this engine.
    type Anchor: Clone + Debug;

    /// An anchor at byte offset `at` of the current text, following the character on side
    /// `side` of it. `None` when `at` isn't a character boundary. At an edge of the text
    /// with no character on that side, an engine may give `None` or an edge anchor: what
    /// an edge anchor does is not a rule.
    fn anchor(&self, at: usize, side: Side) -> Option<Self::Anchor>;

    /// Where `anchor` is now, as a byte offset of the current text: just after (`Left`) or
    /// just before (`Right`) its character, or, if that character was deleted, the gap where
    /// it was. `None` when this replica doesn't have the character yet.
    fn resolve(&self, anchor: &Self::Anchor) -> Option<usize>;
}

/// One atomic change as other participants see it: one engine batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The agent it is attributed to.
    pub author: u64,
    /// The roots it touched (indices into [`Undo::texts`]), sorted.
    pub roots: Vec<usize>,
}

/// What an undo or redo applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Steps (the participant's earlier batches) undone or redone.
    pub steps: usize,
    /// The atomic changes it made, in order.
    pub changes: Vec<Change>,
}

/// One region an undo can't turn back: someone changed it since (undo spec U11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// The root (index into [`Undo::texts`]).
    pub root: usize,
    /// Its byte range in that root's current text.
    pub range: Range<usize>,
    /// What the undo wanted to put back.
    pub ours: String,
    /// The current text of the region.
    pub theirs: String,
}

/// Why an undo or redo applied nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoRefusal {
    /// All or nothing: step `step` (1-based, newest first) conflicts, in these regions.
    Conflicts { step: usize, regions: Vec<Region> },
    /// `--partial`: the top step's every region conflicts; it stays on top.
    Blocked(Vec<Region>),
    /// Nothing to undo or redo.
    Nothing,
    /// Any other refusal, with the engine's message.
    Other(String),
}

/// What `undo --partial` applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialUndo {
    pub applied: Applied,
    /// Conflicting regions of the applied steps, left alone.
    pub skipped: Vec<Region>,
    /// The next step's regions when it was blocked (all conflicting): it stays on top.
    pub blocked: Option<Vec<Region>>,
}

/// One live character of an [`Undo`] doc and the element that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElemBlame {
    pub ch: char,
    /// The agent whose change inserted it.
    pub agent: u64,
    /// The element's identity (unique per character, never reused).
    pub id: (u64, u32),
    /// The change (log position) that inserted it.
    pub change: Option<u64>,
    /// Its provenance root (undo spec v1.8, U23): itself, or for an element an undo, redo
    /// or revert put back, the element it restores (path-compressed).
    pub root: (u64, u32),
    /// The agent whose change inserted the root.
    pub root_agent: u64,
    /// The change that inserted the root.
    pub root_change: Option<u64>,
}

/// Undo and redo (the kit's undo spec): a doc with two text roots, shared by agents that
/// each undo their own batches.
///
/// Not a [`TextReplica`]: undo is per participant on one shared doc (a session), not per
/// replica, and it needs more than one root. `Engine` names the replica type of the same
/// engine, whose deviation list applies.
pub trait Undo: Sized {
    type Engine: TextReplica;

    /// A doc whose two text roots hold `seeds`.
    fn open(seeds: [&str; 2]) -> Self;

    /// `agent` applies `edits` (root index, hunks against that root's current text) as one
    /// batch.
    fn edit(&mut self, agent: u64, edits: &[(usize, Vec<Hunk>)]) -> Result<(), Refused>;

    /// Undo `agent`'s last `steps` batches, all or nothing. Refused (nothing changes) when
    /// any step conflicts or there is nothing to undo.
    fn undo(&mut self, agent: u64, steps: usize) -> Result<Applied, UndoRefusal>;

    /// `undo --partial`: undo `agent`'s last `steps` batches newest first, applying every
    /// region that turns back cleanly and skipping the others; a step whose every region
    /// conflicts is not applied and stops the walk (it stays on top). Refused with
    /// [`UndoRefusal::Blocked`] when that is the first step.
    fn undo_partial(&mut self, agent: u64, steps: usize) -> Result<PartialUndo, UndoRefusal>;

    /// `undo --drop`: remove `agent`'s top step without applying it.
    fn drop_top(&mut self, agent: u64) -> Result<(), UndoRefusal>;

    /// Redo `agent`'s last `steps` undone batches, all or nothing: each reapplies exactly
    /// the regions its undo applied.
    fn redo(&mut self, agent: u64, steps: usize) -> Result<Applied, UndoRefusal>;

    /// Both roots' current text.
    fn texts(&self) -> [String; 2];

    /// `revert <change>` by `agent`, all or nothing (undo spec U22).
    fn revert(&mut self, agent: u64, change: u64) -> Result<Applied, UndoRefusal>;

    /// Every live character of root `root`, in order, with its element and its provenance
    /// root (blame at the element's grain).
    fn blame(&self, root: usize) -> Vec<ElemBlame>;
}

// ---------- branches and purge (replicas) ----------

/// Branches (E-HIST-8): a replica forked from this one, editing as another agent; merging
/// it is delivering what it has.
pub trait Branches: Events {
    /// A replica editing as `agent` (an agent no other replica edits as) that holds exactly
    /// this replica's events now.
    fn branch(&self, agent: u64) -> Self;

    /// Merge everything `other` has that this replica lacks.
    fn merge(&mut self, other: &Self) -> Result<(), EventRefusal>;
}

/// Branches from the past (E-HIST-9): a fork at any version this replica has had.
pub trait BranchAt: Branches {
    /// A replica editing as `agent` that holds exactly this replica's events up to `at`, a
    /// version this replica has had (from [`TextReplica::version`]).
    fn branch_at(&self, at: &Self::Version, agent: u64) -> Result<Self, Refused>;
}

/// Purge (E-ERASE-1): erase content as an attributed event that keeps IDs, lengths and
/// positions.
pub trait Purge: Events {
    /// Erase bytes `start..end` of the current text as this replica's agent: the span is
    /// deleted, and its content erased here and wherever the returned batch is delivered.
    fn purge(&mut self, start: usize, end: usize) -> Result<Self::Batch, Refused>;

    /// Whether `needle`, whole or by any 8-byte window, is in anything this replica keeps
    /// (a byte scan: what it would store or send, tombstones included).
    fn holds(&self, needle: &str) -> bool;
}

/// Span purge (E-ERASE-1, `erase.span_includes_tombstones`): purge a selection by the
/// element IDs it covers, so a deleted character inside it (a typo fixed mid-secret) is
/// erased with it.
pub trait SpanIds: Purge {
    /// Erase bytes `start..end` (`start < end`) of the current text as this replica's
    /// agent, as one event: the elements from the span's first character's through its
    /// last's, plus every deleted element strictly between them; deleted elements just
    /// outside either end are left alone. The returned batch carries it to other replicas.
    /// Refused, changing nothing, for an empty or reversed span or an offset out of range
    /// or off a character boundary.
    fn purge_span(&mut self, start: usize, end: usize) -> Result<Self::Batch, Refused>;
}

/// Blame at a past version (E-HIST-4).
pub trait BlameAt: Blame + Purge {
    /// [`Blame::blame`] of the state at `v`, a version this replica has had, minus content
    /// purged as far as this replica knows.
    fn blame_at(&self, v: &Self::Version) -> Result<Vec<Blamed<Self::CharId>>, Refused>;
}

// ---------- history (doc level) ----------

/// Versions (E-HIST-1, -2): one doc that agents edit, whose states can be named, shown and
/// restored.
pub trait Versions: Sized {
    /// The replica type of the same engine, whose deviation list applies.
    type Engine: TextReplica;
    /// The engine's own version identity (the kit's version ids, the model's frontiers).
    type VersionId: Clone + Debug + PartialEq;

    /// A doc holding `seed`, as the engine's first content (the kit imports it from a file;
    /// no rule attributes it).
    fn open(seed: &str) -> Self;

    /// `agent` applies `hunks` (against the current text) as one batch.
    fn edit(&mut self, agent: u64, hunks: &[Hunk]) -> Result<(), Refused>;

    /// The current text.
    fn text(&self) -> String;

    /// A version standing for the current state (`agent` names it, where the engine
    /// records that).
    fn version(&mut self, agent: u64) -> Self::VersionId;

    /// The text at `v`, minus content purged since.
    fn show(&self, v: &Self::VersionId) -> Result<String, Refused>;

    /// Restore forward: make the text equal `show(v)` with new events by `agent` (history
    /// is never rewritten). Returns the change it made, or `None` when there was nothing to
    /// do.
    fn restore(&mut self, agent: u64, v: &Self::VersionId) -> Result<Option<Change>, Refused> {
        Ok(self.restore_applied(agent, v)?.map(|(c, _)| c))
    }

    /// [`Versions::restore`], with the hunks the engine applied (against the text before
    /// it), as the engine reports them. **Restore is a kit operation on every engine:**
    /// `edit_as(agent, pipeline(text(), show(v)))`, the kit's edit pipeline (undo spec D1,
    /// `align_insertions(diff::hunks(..))`); an engine's own restore is not used.
    fn restore_applied(
        &mut self,
        agent: u64,
        v: &Self::VersionId,
    ) -> Result<Option<(Change, Vec<Hunk>)>, Refused>;
}

/// Purge on a doc's history (E-ERASE-1, -7): every copy of a text, live, deleted or in a
/// version.
pub trait HistoryPurge: Versions {
    /// As `owner`: delete every live copy of `needle` (at least 8 bytes), then erase every
    /// copy of it the doc holds (deleted text, versions, the log).
    fn purge_text(&mut self, owner: u64, needle: &str) -> Result<(), Refused>;

    /// Whether `needle`, whole or by any 8-byte window, is in anything the doc keeps: its
    /// live text and its history (a byte scan).
    fn holds(&self, needle: &str) -> bool;
}

/// Participant erasure (E-ERASE-5): the kit's mapping from opaque agent IDs to people.
pub trait Erasure: HistoryPurge {
    /// The identifier the doc records for `agent` (what erasure must remove).
    fn label(&self, agent: u64) -> String;

    /// As `owner`: replace `agent` by `pseudonym` in attribution, keeping counts, and erase
    /// the content of every change it made.
    fn erase(&mut self, owner: u64, agent: u64, pseudonym: &str) -> Result<(), Refused>;

    /// Each live line (without its `\n`) with the label of its most recent author.
    fn line_authors(&self) -> Vec<(String, String)>;

    /// How many changes the history attributes to `label`.
    fn changes_by(&self, label: &str) -> usize;
}
