//! **Test-only** (the `differential` feature, and the kit's own unit tests): which path makes
//! each engine edit, chosen per thread, so that one sequence of operations can be run through
//! two paths and compared byte for byte (v0.9.3). No host enables the feature; without it the
//! engine has one path and none of this is compiled.
//!
//! - [`Path`]: how [`crate::Doc::edit_as`] (and every session verb built on it: edits,
//!   `write --base`, undo, redo, revert, restore, import, outside edits) makes its change.
//! - [`Side`]: one side of a comparison, installed on the current thread by [`with_side`]:
//!   its path; an optional **shadow** path (each edit is also made on a twin of the doc
//!   through the shadow path, and any difference in the update, the resulting state, state
//!   vector or texts **panics**); a seed for **deterministic client IDs** (so two sessions
//!   driven alike mint the same IDs); and a record of every edit (client, update or error,
//!   state after).
//!
//! The mutant paths exist so a differential test can show it catches a wrong path: each makes
//! the same text as today's path in the common case and differs in its bytes.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::doc::{ClientId, DocError, MAX_CLIENT_ID};
use crate::engine::{EngineDoc, TextPlan, YrsDoc};

/// A fault planted in person undo grouping (undo spec U3, K4, v0.9.7), so the grouping
/// tests can show they catch each one. Installed per thread by [`with_group_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GroupMutant {
    /// Groups never close on another participant's change inside or touching their span
    /// (U3 (g)): the edit joins whatever landed in its window.
    NeverClosesOnForeign,
    /// Groups never close on idle (U3 (a)).
    NeverClosesOnIdle,
    /// Groups never close on a word boundary (U3 (c)).
    NeverClosesOnBoundary,
}

/// A planted fault in the outside-save notes (`tests/outside_removal_notes.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteMutant {
    /// No note is queued.
    DropAll,
    /// The roll-up is not kept: past the bound a note is just dropped.
    DropRollup,
    /// Every note goes to the first author of the save, whoever wrote the text.
    WrongAuthor,
    /// The host's edit calls (`edit_with`...) take waiting notes and drop them (the bug the
    /// soak found before v0.9.7.2 shipped).
    EditWithDrains,
}

thread_local! {
    static NOTE_REMOVAL_COST: std::cell::Cell<std::time::Duration> = const { std::cell::Cell::new(std::time::Duration::ZERO) };
}

/// How long the last outside save on this thread spent working out whose text it removes, under
/// the session's lock (the measurement in `tests/outside_removal_notes.rs`).
pub fn last_removal_cost() -> std::time::Duration {
    NOTE_REMOVAL_COST.with(|c| c.get())
}

pub(crate) fn note_removal_cost(d: std::time::Duration) {
    NOTE_REMOVAL_COST.with(|c| c.set(d));
}

thread_local! {
    static NOTE_MUTANT: std::cell::Cell<Option<NoteMutant>> = const { std::cell::Cell::new(None) };
}

/// The notes fault planted on this thread, if any.
pub fn note_mutant() -> Option<NoteMutant> {
    NOTE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_note_mutant<R>(mutant: NoteMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<NoteMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            NOTE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(NOTE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

thread_local! {
    static GROUP_MUTANT: std::cell::Cell<Option<GroupMutant>> = const { std::cell::Cell::new(None) };
}

/// The grouping fault planted on this thread, if any.
pub fn group_mutant() -> Option<GroupMutant> {
    GROUP_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_group_mutant<R>(mutant: GroupMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<GroupMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            GROUP_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(GROUP_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in a merge, so the tests that should catch it can show they do. Installed per
/// thread by [`with_merge_mutant`]. Two families share it:
/// - `write --base`'s text merge (v0.9.8), caught by the trace classes ((i) and (ii) below;
///   `spec-undo`'s `traces` plants them by value);
/// - the grid's text merge (`table::merge3_text` and the outside save), so the row-pairing
///   property and its named tests can show they catch each one (mutants (xxxii) to (xxxvii):
///   contract §2.9.1, §3.4 and §4.3, spec S11/S13; selected in the tests by `MPC_MERGE_MUTANT`),
///   and the keyed grid merges (`table::merge3` and `table::merge3_outside`, mutants (xxxviii)
///   and (xxxix), caught by `tests/table.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MergeMutant {
    /// (i) v0.9.7's merge: sides diffed to the character on the texts as they are. Lines that
    /// share a start pair one line off, and two lines can join at the end of the text.
    CharSides,
    /// (ii) The final line break always as theirs left it, whoever wrote the last line.
    EndFollowsTheirs,
    /// (xxxii) An outside save's whole-row loop goes over the live side's live columns, so a
    /// column the file brings back keeps the live side's hidden value beside the file's row.
    OutsideWholeRowLiveCols,
    /// (xxxiii) The both-changed test by cell goes over the live side's live columns, missing a
    /// live write into a column the live side then deleted.
    ByCellLiveCols,
    /// (xxxiv) `write --base` conflicts on a live switch between empty and absent that changes
    /// no byte of the row (no live edit of the cell), as before the per-cell rule reached it.
    WriteBaseNeutralConflicts,
    /// (xxxv) A revert that rewrites a re-paired row whole (S13) restores it even when the doc has
    /// dropped it (S11 skipped): an engine refusal on an unknown key.
    WholeRowRestoresDropped,
    /// (xxxvi) An undo or redo of an op on a re-paired row the doc has dropped says
    /// `rewritten_row` (S13) instead of `dropped` (S11).
    RewrittenRowBeforeDropped,
    /// (xxxvii) A raw keyed cell write under a row or a column the table doesn't know (dropped,
    /// or never in it) is taken as a hidden write, as before the check: an orphan cell nobody
    /// can see, restore or sweep, reported with a restore that can't work.
    KeyedWriteUnderUnknownKey,
    /// (xxxviii) The per-cell neutral rule of `write --base` reaches the keyed merge
    /// (`table::merge3`, no text): theirs' clear of an absent cell in a row that renders every
    /// column is overwritten by ours' value with no conflict, as in v0.10.0.
    KeyedMergeNeutralSwitch,
    /// (xxxix) The outside save's per-cell neutral rule (a byte-neutral live switch between empty
    /// and absent, or a live value the file's row renders anyway) reaches the keyed
    /// `table::merge3_outside` (no text): the file's value goes in with no overridden `cell`
    /// reported, as in v0.10.0.
    KeyedOutsideNeutralSwitch,
}

thread_local! {
    static MERGE_MUTANT: std::cell::Cell<Option<MergeMutant>> = const { std::cell::Cell::new(None) };
}

/// The merge fault planted on this thread, if any.
pub fn merge_mutant() -> Option<MergeMutant> {
    MERGE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_merge_mutant<R>(mutant: MergeMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<MergeMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            MERGE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(MERGE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}
/// A fault planted at the seams between a grid's two line counts (docs/POSITIONS.md), so
/// `tests/grid_lines.rs` can show it catches each one. Installed per thread by
/// [`with_line_mutant`]; selected in the tests by `MPC_LINE_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineMutant {
    /// `LN` stops refusing on a grid holding a lone CR, and counts the line map as before.
    LnKeepsLineMap,
    /// `--ids` reads a printed line's `\n` number as a rendered line, as before.
    IdsCountNlLines,
    /// The lines a read reports (views, `--for-edit`, notes) and `claim --lines` reach the
    /// line map as they are, `\n` numbers read as rendered lines, as before.
    TouchedCountNlLines,
    /// `LN` goes on to the line map on a file that can't be rendered.
    LnGoesOnUnrendered,
    /// `claim --lines` reads the caller's numbers as rendered lines on a file that can't be
    /// rendered.
    ClaimLinesGoOnUnrendered,
    /// A conflict report gives rendered line numbers on a file that can't be rendered.
    ConflictLinesFallBack,
    /// `row list` prints rendered line numbers on a file holding a lone CR, as before.
    RowListKeepsLines,
    /// A conflict report gives rendered line numbers on a file holding a lone CR, as before.
    ConflictKeepsLines,
    /// A read verb that fails after the doc was read still counts as the caller's read, as
    /// before: it logs activity, and their next read loses who changed the doc.
    RefusedReadIsARead,
}

thread_local! {
    static LINE_MUTANT: std::cell::Cell<Option<LineMutant>> = const { std::cell::Cell::new(None) };
}

/// The line fault planted on this thread, if any.
pub fn line_mutant() -> Option<LineMutant> {
    LINE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_line_mutant<R>(mutant: LineMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<LineMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            LINE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(LINE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in a grid's render paths, so `tests/grid_scaling.rs` can show its bound
/// catches it (selected there by `MPC_GRID_COST_MUTANT`). Installed per thread by
/// [`with_grid_cost_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GridCostMutant {
    /// [`crate::table::Table::render`], [`crate::table::Table::line_map`] and a grid's blame
    /// visit every live column of every row, both keys cloned for each lookup (the earlier
    /// walk): their cost grows with rows × columns, blank rows included.
    PerColumn,
    /// A row's or column's place is found by a walk from the first place (as the vector the
    /// axis was), so a change that inserts or moves many rows costs rows × rows.
    AxisWalk,
    /// A merge's conflict listing finds each row's position, lines and changed cells by
    /// walking the axis, the line map and the live columns, per conflict.
    ConflictWalk,
    /// `row list` finds each row's lines by walking the line map, and builds every column's
    /// field as its own string with both keys cloned.
    RowListWalk,
    /// A live write from an older version (or an outside save) checks each element the live
    /// side dropped and the text kept by scanning every cell of both versions and walking
    /// both axes, per element.
    RekeyWalk,
}

thread_local! {
    static VISITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Elements visited on this thread by the table paths that count them (an axis's place
/// lookups and shifts, a position walk, a line map's scan): a timing-free cost for
/// `tests/grid_scaling.rs`.
pub fn visits() -> u64 {
    VISITS.with(|v| v.get())
}

pub(crate) fn visit(n: usize) {
    VISITS.with(|v| v.set(v.get().saturating_add(n as u64)));
}

thread_local! {
    static GRID_COST_MUTANT: std::cell::Cell<Option<GridCostMutant>> = const { std::cell::Cell::new(None) };
}

/// The grid cost fault planted on this thread, if any.
pub fn grid_cost_mutant() -> Option<GridCostMutant> {
    GRID_COST_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, however `f` ends.
pub fn with_grid_cost_mutant<R>(mutant: GridCostMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<GridCostMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            GRID_COST_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(GRID_COST_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault in element presence ([`crate::session::Session::set_selection`]), so its tests can
/// show they catch it (selected by `MPC_STRUCT_MUTANT` in `tests/elem_presence.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionMutant {
    /// A selection keeps the keys of elements that stopped being live.
    KeepsDeleted,
}

thread_local! {
    static SELECTION_MUTANT: std::cell::Cell<Option<SelectionMutant>> = const { std::cell::Cell::new(None) };
}

/// The selection fault planted on this thread, if any.
pub fn selection_mutant() -> Option<SelectionMutant> {
    SELECTION_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_selection_mutant<R>(mutant: SelectionMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<SelectionMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            SELECTION_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(SELECTION_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted at the apply of a structured change (`apply_structured`, inside `commit`), so
/// the tests of what a change leaves behind can drive an exit no public call reaches
/// (`session/live/next_change_tests.rs`). Installed per thread by [`with_apply_fault`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApplyFault {
    /// The apply refuses the change, and the doc stays trusted.
    Refused,
    /// The apply fails with an error that poisons the doc, so `commit` reloads it (`recover`).
    Poisoned,
    /// The apply reports that the change changed nothing, so `commit` returns `Ok`.
    NoChange,
    /// In `commit`, a step after one that applied is refused, so `commit` reloads the doc.
    LaterStepRefused,
    /// `commit`'s store transaction fails, without damaging the store, so `commit` reloads the doc
    /// and the session goes on.
    StoreCommitFails,
    /// The bookkeeping of the structured change this many changes on fails, after the doc took it.
    BookkeepingFailsAfter(u32),
}

thread_local! {
    static APPLY_FAULT: std::cell::Cell<Option<ApplyFault>> = const { std::cell::Cell::new(None) };
}

/// The apply fault planted on this thread, if any.
pub fn apply_fault() -> Option<ApplyFault> {
    APPLY_FAULT.with(|m| m.get())
}

/// Whether the bookkeeping of this structured change fails ([`ApplyFault::BookkeepingFailsAfter`]);
/// one change closer to it otherwise.
pub(crate) fn take_bookkeeping_fault() -> bool {
    APPLY_FAULT.with(|m| match m.get() {
        Some(ApplyFault::BookkeepingFailsAfter(0)) => true,
        Some(ApplyFault::BookkeepingFailsAfter(n)) => {
            m.set(Some(ApplyFault::BookkeepingFailsAfter(n - 1)));
            false
        }
        _ => false,
    })
}

/// Run `f` with `fault` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_apply_fault<R>(fault: ApplyFault, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ApplyFault>);
    impl Drop for Back {
        fn drop(&mut self) {
            APPLY_FAULT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(APPLY_FAULT.with(|m| m.replace(Some(fault))));
    f()
}

/// A fault planted in when a claim is marked as edited (`tests/claim_marks.rs`, selected by
/// `MPC_STRUCT_MUTANT`). Installed per thread by [`with_mark_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MarkMutant {
    /// A grid claim is marked from what a change names, before it applies.
    GridMarkBeforeLanding,
    /// Every text claim and reservation of the member on a root the change names is marked first
    /// thing, whatever the change does.
    TextMarkFirst,
    /// A reload after a failed change leaves the claims in place, marks and all.
    RecoverKeepsClaims,
}

thread_local! {
    static MARK_MUTANT: std::cell::Cell<Option<MarkMutant>> = const { std::cell::Cell::new(None) };
}

/// The claim mark fault planted on this thread, if any.
pub fn mark_mutant() -> Option<MarkMutant> {
    MARK_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_mark_mutant<R>(mutant: MarkMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<MarkMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            MARK_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(MARK_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in what follows a failed render or re-sync of the disk replica
/// (`tests/resync_failure.rs`, selected by `MPC_STRUCT_MUTANT`). Installed per thread by
/// [`with_resync_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResyncMutant {
    /// The change's commit returns the error, though the change is in the store.
    ReturnsTheError,
    /// A purge stops at its re-sync, with the live removal landed and the history not erased.
    PurgeStopsAtTheResync,
    /// A failure after the doc took an outside save leaves the doc holding it, unlogged.
    IngestKeepsTheDoc,
    /// A failure settling after an outside save returns the error, though the save is in the
    /// store.
    SettleReturnsTheError,
}

thread_local! {
    static RESYNC_MUTANT: std::cell::Cell<Option<ResyncMutant>> = const { std::cell::Cell::new(None) };
}

/// The re-sync fault planted on this thread, if any.
pub fn resync_mutant() -> Option<ResyncMutant> {
    RESYNC_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_resync_mutant<R>(mutant: ResyncMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ResyncMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            RESYNC_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(RESYNC_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in client-ID handling in sessions, so its tests can show they catch it.
/// Installed per thread by [`with_client_mutant`]; selected in the tests by
/// `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientMutant {
    /// Agent reuse ignores the kinds the store records (`client_kinds`): every ID from before
    /// the session's load counts as reusable (an own replica's and an older kit's included),
    /// whatever its kind (caught by `tests/client_reuse.rs`'s own-replica and N−1 tests).
    ReuseAcrossReload,
    /// Agent reuse reads no recorded kinds at load: only IDs minted in this load are reused,
    /// so a plain agent draws a new ID after every reload (caught by
    /// `tests/client_reuse.rs`'s plain-agent test).
    NoReuseAfterReload,
    /// An open leaves the `client_kinds` rows of docs that have been deleted (caught by
    /// `store::tests::an_open_removes_the_client_kinds_of_docs_an_older_kit_deleted` and
    /// tools/n1's `n1_kinds_after_delete`).
    NoOrphanSweep,
}

thread_local! {
    static CLIENT_MUTANT: std::cell::Cell<Option<ClientMutant>> = const { std::cell::Cell::new(None) };
}

/// The client-ID fault planted on this thread, if any.
pub fn client_mutant() -> Option<ClientMutant> {
    CLIENT_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_client_mutant<R>(mutant: ClientMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ClientMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            CLIENT_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(CLIENT_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in tagged-key minting ([`crate::mint`]), so the minting tests can show they
/// catch it. Installed per thread by [`with_mint_mutant`]; selected in the tests by
/// `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MintMutant {
    /// [`crate::mint::next_for`] ignores the retired high-water mark, so a key a drop forgot
    /// is minted again (caught by "drop, then mint").
    IgnoreRetired,
    /// The session's writer-tag rule takes any writer's tag, not only the inserting
    /// connection's own (caught by "an insert with another's tag is refused").
    AnyWriterTag,
    /// [`crate::mint::mark_key`] is one shared key per prefix and tag, not one per dropper:
    /// last writer wins, so of two concurrent drops the lower mark can win (caught by the
    /// two-concurrent-drops convergence test).
    SingleLwwMark,
    /// The session's client-ID helper hands out a draw of 0 instead of drawing again (caught
    /// by the client-ID resampling test).
    ClientIdZero,
    /// The client-ID helper moves a draw outside `2..=MAX_KIT_CLIENT` into it (the old
    /// `.max(2)`), so several draws map onto one ID (caught by "a draw of 0 then 7 gives 7").
    ShiftNotResample,
    /// Agent-ID reuse ignores the IDs present connections hold, so two live connections share
    /// one (caught by the join-and-leave churn test).
    ReuseHeldClient,
}

thread_local! {
    static CLIENT_DRAWS: RefCell<std::collections::VecDeque<ClientId>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
}

/// Run `f` with the session's next raw client-ID draws forced to `draws`, in order (any
/// values, 0 and 2^53 - 1 included), so a test can show which the kit draws again; the usual
/// source takes over when they run out, and any left are discarded afterwards.
pub fn with_client_draws<R>(draws: impl IntoIterator<Item = ClientId>, f: impl FnOnce() -> R) -> R {
    struct Back;
    impl Drop for Back {
        fn drop(&mut self) {
            CLIENT_DRAWS.with(|d| d.borrow_mut().clear());
        }
    }
    CLIENT_DRAWS.with(|d| {
        let mut d = d.borrow_mut();
        d.clear();
        d.extend(draws);
    });
    let _back = Back;
    f()
}

/// The next forced raw client-ID draw on this thread ([`with_client_draws`]), if any.
pub(crate) fn forced_client_draw() -> Option<ClientId> {
    CLIENT_DRAWS.with(|d| d.borrow_mut().pop_front())
}

thread_local! {
    static MINT_MUTANT: std::cell::Cell<Option<MintMutant>> = const { std::cell::Cell::new(None) };
}

/// The minting fault planted on this thread, if any.
pub fn mint_mutant() -> Option<MintMutant> {
    MINT_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_mint_mutant<R>(mutant: MintMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<MintMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            MINT_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(MINT_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in element claims, so their tests can show they catch it. Installed per
/// thread by [`with_claim_mutant`]; selected in the tests by `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClaimMutant {
    /// The claim policy reads a change's list ops only: a map write isn't mapped to its element
    /// through [`crate::session::Pack::composite`], so it passes a claim on that element (caught
    /// by "a write to a claimed element's map key is refused").
    MapKeysUnmapped,
    /// An element claim is marked from what a change names, before it applies, so a change that
    /// lands nothing leaves the mark, and the next change by anyone ends the claim (caught by "a
    /// change that doesn't land leaves the claim alone").
    MarkBeforeApply,
}

thread_local! {
    static CLAIM_MUTANT: std::cell::Cell<Option<ClaimMutant>> = const { std::cell::Cell::new(None) };
}

/// The element-claim fault planted on this thread, if any.
pub fn claim_mutant() -> Option<ClaimMutant> {
    CLAIM_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, however `f` ends.
pub fn with_claim_mutant<R>(mutant: ClaimMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ClaimMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            CLAIM_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(CLAIM_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the reference-state helper ([`crate::refs`]), so its tests can show they
/// catch it. Installed per thread by [`with_ref_mutant`]; selected in the tests by
/// `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefMutant {
    /// [`crate::refs::resolve_all`] leaves orphaned references out (caught by "every link
    /// keeps its entry").
    OrphanDropsLink,
    /// [`crate::refs::resolve_parents`] cuts a cycle at its first element in the list's order,
    /// not its smallest key (caught by "the same parents in another order cut the same way").
    CycleByOrder,
}

thread_local! {
    static REF_MUTANT: std::cell::Cell<Option<RefMutant>> = const { std::cell::Cell::new(None) };
}

/// The reference-state fault planted on this thread, if any.
pub fn ref_mutant() -> Option<RefMutant> {
    REF_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_ref_mutant<R>(mutant: RefMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<RefMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            REF_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(REF_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the doc's blob references (`store/blob_refs.rs`), so the tests that should
/// catch it can show they do. Installed per thread by [`with_blob_ref_mutant`]; selected in the
/// tests by `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlobRefMutant {
    /// The open's orphan sweep leaves the last table on its list (caught by
    /// `store::tests::the_orphan_sweep_covers_every_table_on_its_list`).
    SweepSkipsTable,
    /// A change's references are taken in a second transaction after its own commit, so an
    /// interrupted write leaves a value naming a blob with no reference (caught by
    /// `session::blob_ref_tests::an_interrupted_write_leaves_the_change_and_its_reference_together`).
    RefAfterCommit,
    /// A load takes no references for the live doc (caught by
    /// `session::blob_ref_tests::a_load_takes_a_hold_for_a_name_an_older_kit_wrote`).
    NoHealAtLoad,
    /// Blob GC ignores a name on record with no count taken, so a blob that arrives after its
    /// name is collected while the doc names it (caught by
    /// `session::blob_ref_tests::a_blob_that_arrives_after_its_name_is_never_collected`).
    GcIgnoresNames,
    /// An edit checked for a name, refused once it reaches the engine, leaves its strictness for
    /// the next change, so an undo or restore naming a missing blob is refused (caught by
    /// `session::blob_ref_tests::a_strict_edit_leaves_nothing_behind_for_the_next_change`).
    StaleStrictFlag,
    /// `Session::blob_refs` returns every row whatever the caller's rights (caught by
    /// `session::blob_ref_tests::without_the_history_right_blob_refs_shows_what_a_read_shows`).
    RefsIgnoreRights,
}

thread_local! {
    static BLOB_REF_MUTANT: std::cell::Cell<Option<BlobRefMutant>> = const { std::cell::Cell::new(None) };
}

/// The blob-reference fault planted on this thread, if any.
pub fn blob_ref_mutant() -> Option<BlobRefMutant> {
    BLOB_REF_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_blob_ref_mutant<R>(mutant: BlobRefMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<BlobRefMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            BLOB_REF_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(BLOB_REF_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// How an edit becomes an engine change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Path {
    /// The production path (v0.9.3): the edit made on the doc itself as the client
    /// (`transact_mut_as`), with the sampled canary against the throwaway replica.
    #[default]
    MainDoc,
    /// The path before v0.9.3, kept for the differential test: a throwaway replica of the
    /// doc's full current state, with the editing client's ID, edited, encoded and dropped;
    /// the doc applies the update.
    Throwaway,
    /// Mutant: hunks applied front to back. Same text; insertions take their clocks in
    /// another order, so a change inserting at two places has other bytes.
    MutantFrontToBack,
    /// Mutant: each client edits on a replica kept from its first edit and never synced
    /// after (a stale base, or a kept replica that missed updates).
    MutantStale,
}

/// One edit as a side made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRecord {
    pub client: ClientId,
    /// The client's clock in the doc edited, before the edit.
    pub clock_before: u64,
    /// The update, or the error the path returned.
    pub update: Result<Vec<u8>, DocError>,
    /// The doc's full state after it (v2; `None` when the doc is poisoned).
    pub state: Option<Vec<u8>>,
    /// The doc's state vector after it, sorted by client.
    pub state_vector: Vec<(ClientId, u32)>,
}

/// One side of a comparison: see the module docs.
#[derive(Default)]
pub struct Side {
    path: Path,
    shadow: Option<Path>,
    /// Next deterministic client ID seed (`None`: the kit's random IDs).
    clients: Option<u64>,
    record: bool,
    edits: Vec<EditRecord>,
    shadow_checks: u64,
    kept: HashMap<ClientId, yrs::Doc>,
    /// Plant a divergence in the production path (the canary's test).
    plant: bool,
    /// File write-backs reload the disk replica from a whole encode of the doc, as before
    /// v0.9.7 (the write-back's differential test).
    full_writeback: bool,
    /// The engine's position index off: every lookup walks from the text's start, as before
    /// k2.3 (the soak's diagnosis, v0.9.7).
    no_position_index: bool,
}

impl Side {
    /// A side whose edits take `path`.
    pub fn new(path: Path) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }

    /// Also make each edit through `reference` on a twin of the doc, and panic on any
    /// difference (update bytes, state bytes, state vector, texts).
    pub fn shadowed_by(self, reference: Path) -> Self {
        Self {
            shadow: Some(reference),
            ..self
        }
    }

    /// Mint client IDs from `seed` (deterministic), not at random.
    pub fn with_client_seed(self, seed: u64) -> Self {
        Self {
            clients: Some(seed),
            ..self
        }
    }

    /// Plant a divergence in the production path ([`Path::MainDoc`]): its edits apply each
    /// root's hunks front to back (the same text, other bytes for an edit inserting at two
    /// places), which the canary must catch.
    pub fn planting_divergence(self) -> Self {
        Self {
            plant: true,
            ..self
        }
    }

    /// The engine's position index off for every doc made while this side is installed:
    /// each lookup walks from the text's start, as before k2.3 (a diagnosis switch).
    pub fn no_position_index(self) -> Self {
        Self {
            no_position_index: true,
            ..self
        }
    }

    /// File write-backs re-encode the whole doc and reload the disk replica from it, as
    /// before v0.9.7, instead of keeping the replica in step (the differential test's
    /// reference side).
    pub fn full_writeback(self) -> Self {
        Self {
            full_writeback: true,
            ..self
        }
    }

    /// Keep an [`EditRecord`] of every edit (its state is a full encode: tests only).
    pub fn recording(self) -> Self {
        Self {
            record: true,
            ..self
        }
    }

    pub fn path(&self) -> Path {
        self.path
    }

    /// The edits recorded since the last call.
    pub fn take_edits(&mut self) -> Vec<EditRecord> {
        std::mem::take(&mut self.edits)
    }

    /// How many edits were checked against the shadow path.
    pub fn shadow_checks(&self) -> u64 {
        self.shadow_checks
    }
}

thread_local! {
    static SIDE: RefCell<Option<Side>> = const { RefCell::new(None) };
    /// The installed side's `plant` (read inside an edit, while `SIDE` is borrowed).
    static PLANT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The installed side's `full_writeback`.
    static FULL_WB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The installed side's `no_position_index`.
    static NO_POSINDEX: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the installed side writes back files the old way.
pub(crate) fn full_writeback() -> bool {
    FULL_WB.with(|p| p.get())
}

thread_local! {
    static REOPEN_BY_VERSION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a reopen claims the stored disk replica in step with the doc from the sync point's
/// version alone (the fault [`with_reopen_step_by_version`] plants).
pub(crate) fn reopen_step_by_version() -> bool {
    REOPEN_BY_VERSION.with(|p| p.get())
}

/// Run `f` with every reopen on this thread claiming the stored disk replica in step with the
/// doc from the sync point's version alone, as before v0.10.0.3: the stale merge base the
/// first sync after a reopen then reports as `AuditEvent::DiskReplicaRepaired` and reloads.
/// Removed afterwards, even if `f` panics.
pub fn with_reopen_step_by_version<R>(f: impl FnOnce() -> R) -> R {
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            REOPEN_BY_VERSION.with(|p| p.set(self.0));
        }
    }
    let _back = Back(REOPEN_BY_VERSION.with(|p| p.replace(true)));
    f()
}

/// Whether the installed side turns the engine's position index off.
pub(crate) fn position_index_off() -> bool {
    NO_POSINDEX.with(|p| p.get())
}

/// Whether the installed side plants a divergence in the production path.
pub(crate) fn planted_divergence() -> bool {
    PLANT.with(|p| p.get())
}

/// Run `f` with `side` installed on this thread (every edit `f` makes, directly or through a
/// session, takes its path). The side is handed back afterwards, even if `f` panics.
pub fn with_side<R>(side: &mut Side, f: impl FnOnce() -> R) -> R {
    struct Back<'a>(&'a mut Side);
    impl Drop for Back<'_> {
        fn drop(&mut self) {
            PLANT.with(|p| p.set(false));
            FULL_WB.with(|p| p.set(false));
            NO_POSINDEX.with(|p| p.set(false));
            if let Some(s) = SIDE.with(|t| t.borrow_mut().take()) {
                *self.0 = s;
            }
        }
    }
    PLANT.with(|p| p.set(side.plant));
    FULL_WB.with(|p| p.set(side.full_writeback));
    NO_POSINDEX.with(|p| p.set(side.no_position_index));
    let prev = SIDE.with(|t| t.borrow_mut().replace(std::mem::take(side)));
    assert!(prev.is_none(), "with_side: a side is already installed");
    let _back = Back(side);
    f()
}

/// A deterministic client ID when the installed side mints them, else `None`.
pub(crate) fn next_client() -> Option<ClientId> {
    SIDE.with(|t| {
        let mut t = t.borrow_mut();
        let seed = t.as_mut()?.clients.as_mut()?;
        loop {
            // splitmix64
            *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            let c = (z ^ (z >> 31)) & MAX_CLIENT_ID;
            if c > 1 {
                return Some(c);
            }
        }
    })
}

/// Make `plans` as `client` on `doc` through `path`; on success the doc holds the change.
fn make_with(
    doc: &mut YrsDoc,
    path: Path,
    client: ClientId,
    plans: &[TextPlan],
    kept: &mut HashMap<ClientId, yrs::Doc>,
) -> Result<Vec<u8>, DocError> {
    let update = match path {
        // applies the change itself
        Path::MainDoc => return doc.checked_change(client, plans),
        Path::Throwaway => doc.throwaway_update(client, plans)?,
        Path::MutantFrontToBack => doc.mutant_front_to_back(client, plans)?,
        Path::MutantStale => doc.mutant_stale(client, plans, kept)?,
    };
    doc.apply_trusted(&update)?;
    Ok(update)
}

/// The engine's edit, through the installed side's path (today's path when none is).
pub(crate) fn make(
    doc: &mut YrsDoc,
    client: ClientId,
    plans: &[TextPlan],
) -> Result<Vec<u8>, DocError> {
    SIDE.with(|t| {
        let mut t = t.borrow_mut();
        let Some(side) = t.as_mut() else {
            return make_with(doc, Path::MainDoc, client, plans, &mut HashMap::new());
        };
        let clock_before = doc.clock_of(client);
        let out = match side.shadow {
            None => make_with(doc, side.path, client, plans, &mut side.kept),
            Some(reference) => {
                let mut twin = doc.twin();
                let want = match twin.as_mut() {
                    Ok(tw) => make_with(tw, reference, client, plans, &mut HashMap::new()),
                    Err(e) => Err(e.clone()),
                };
                let got = make_with(doc, side.path, client, plans, &mut side.kept);
                assert_eq!(
                    got, want,
                    "edit paths differ: the update of {:?} (left) against {reference:?} \
                     (right), client {client}",
                    side.path
                );
                if let (Ok(_), Ok(tw)) = (&got, &twin) {
                    assert_eq!(
                        doc.snapshot_for_tests(),
                        tw.snapshot_for_tests(),
                        "edit paths differ: the doc's state after an edit by {client} \
                         through {:?} (left) and {reference:?} (right)",
                        side.path
                    );
                    assert_eq!(
                        doc.texts_for_tests(),
                        tw.texts_for_tests(),
                        "edit paths differ: the texts"
                    );
                }
                side.shadow_checks += 1;
                got
            }
        };
        if side.record {
            let (state, state_vector) = doc.snapshot_for_tests();
            side.edits.push(EditRecord {
                client,
                clock_before,
                update: out.clone(),
                state,
                state_vector,
            });
        }
        out
    })
}

/// The run record's `job` field (`tools/evidence/`): the job id K2 Compute's nodes set in
/// `K2_COMPUTE_JOB`, as a JSON string, or `null` off a node (then `collect.py` fills it in).
/// Only `[A-Za-z0-9_-]` is taken, so the record stays valid JSON.
pub fn runrec_job() -> String {
    match std::env::var("K2_COMPUTE_JOB") {
        Ok(j)
            if !j.is_empty()
                && j.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') =>
        {
            format!("\"{j}\"")
        }
        _ => "null".to_owned(),
    }
}

/// Bugs a test can plant in the engine's position index (v0.9.7), to prove the kit reports
/// what its shadow check finds. Only with the test-only `planted-position-bugs` feature.
#[cfg(feature = "planted-position-bugs")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlantedPositionBug {
    /// A removal moves the markers after it left by one unit too few.
    RemoveOffByOne,
    /// Integrating an item (a remote update, a redo) leaves the text's markers in place.
    KeepMarkersOnIntegrate,
}

#[cfg(feature = "planted-position-bugs")]
impl From<PlantedPositionBug> for yrs::PlantedPositionBug {
    fn from(b: PlantedPositionBug) -> Self {
        match b {
            PlantedPositionBug::RemoveOffByOne => yrs::PlantedPositionBug::RemoveOffByOne,
            PlantedPositionBug::KeepMarkersOnIntegrate => {
                yrs::PlantedPositionBug::KeepMarkersOnIntegrate
            }
        }
    }
}

thread_local! {
    /// The profile (v0.9.7): per scope name, (times entered, seconds inside).
    static PROFILE: std::cell::RefCell<std::collections::BTreeMap<&'static str, (u64, f64)>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// A profile scope: the time from `new` to drop is added to its name (this thread's profile).
/// Nested scopes each count their own (inclusive) time.
#[doc(hidden)]
pub struct ProfScope {
    name: &'static str,
    start: std::time::Instant,
}

impl ProfScope {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            start: std::time::Instant::now(),
        }
    }
}

impl Drop for ProfScope {
    fn drop(&mut self) {
        let s = self.start.elapsed().as_secs_f64();
        PROFILE.with(|p| {
            let mut p = p.borrow_mut();
            let e = p.entry(self.name).or_default();
            e.0 += 1;
            e.1 += s;
        });
    }
}

/// This thread's profile since the last call, by scope name: (times entered, seconds
/// inside); then it starts again. Test-only (the soak's profile, v0.9.7).
pub fn profile_take() -> Vec<(&'static str, u64, f64)> {
    PROFILE.with(|p| {
        std::mem::take(&mut *p.borrow_mut())
            .into_iter()
            .map(|(k, (n, s))| (k, n, s))
            .collect()
    })
}

/// The engine's position index counters (k2.3; [`crate::doc::Doc::position_index_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PositionIndexStats {
    /// Index lookups in texts.
    pub lookups: u64,
    /// Lookups that started from a marker.
    pub marker_hits: u64,
    /// Lookups that walked from the text's start.
    pub walks: u64,
    /// Marker lookups the walk also ran for (the shadow check).
    pub shadow_checks: u64,
    /// Shadow checks that differed.
    pub mismatches: u64,
    /// Markers that no longer resolved.
    pub stale_markers: u64,
}

impl std::ops::AddAssign for PositionIndexStats {
    fn add_assign(&mut self, o: Self) {
        self.lookups += o.lookups;
        self.marker_hits += o.marker_hits;
        self.walks += o.walks;
        self.shadow_checks += o.shadow_checks;
        self.mismatches += o.mismatches;
        self.stale_markers += o.stale_markers;
    }
}

/// A fault planted in the blame provenance map's incremental update (v0.9.5), so that the
/// blame differential test and the sampled runtime check
/// ([`crate::session::Config::blame_check_every`]) can show they catch one. Installed per
/// thread by [`with_prov_mutant`]; the rebuild from the engine and the store (the cold path
/// and the oracle) never sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProvMutant {
    /// A change's deletion never reaches the map: it keeps the deleted elements (the map
    /// then has more bytes than the text).
    MissDeletion,
    /// A deleted restoration stays in the index of live restorations by root (the runs are
    /// right; undo's "is this root live?" is not).
    MissDeletionInIndex,
    /// An undo, redo or revert's restored elements are recorded as their own roots, so blame
    /// names the restorer instead of the root's author.
    RestoredAsRestorer,
    /// v0.9.7.1 and before (undo spec U23): an outside save's revert record, and a promoted
    /// version's, keeps no roots, so a revert of it puts back fresh text instead of the roots
    /// the change removed. Planted in the record, not the map.
    OutsideSaveKeepsNoRoots,
}

thread_local! {
    static PROV_MUTANT: std::cell::Cell<Option<ProvMutant>> = const { std::cell::Cell::new(None) };
}

/// The provenance fault planted on this thread, if any.
pub(crate) fn prov_mutant() -> Option<ProvMutant> {
    PROV_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted in the provenance map's incremental update on this thread
/// (every session call `f` makes on this thread); removed afterwards, even if `f` panics.
pub fn with_prov_mutant<R>(mutant: ProvMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ProvMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            PROV_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(PROV_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

thread_local! {
    static BLAME_CHECK_HOOK: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
}

/// Run `f` with `hook` called by every sampled blame check on this thread at its off-lock
/// point: after the rebuild, before the lock is taken again (v0.9.7). The hook may call the
/// session (an edit, a purge) to show what the check does when the doc moves under it.
pub fn with_blame_check_hook<R>(hook: impl FnMut() + 'static, f: impl FnOnce() -> R) -> R {
    struct Back;
    impl Drop for Back {
        fn drop(&mut self) {
            BLAME_CHECK_HOOK.with(|h| h.borrow_mut().take());
        }
    }
    BLAME_CHECK_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    let _back = Back;
    f()
}

/// Call the installed blame check hook, if any (taken out while it runs, so it may blame).
pub(crate) fn blame_check_hook() {
    let hook = BLAME_CHECK_HOOK.with(|h| h.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
        BLAME_CHECK_HOOK.with(|h| {
            let mut h = h.borrow_mut();
            if h.is_none() {
                *h = Some(hook);
            }
        });
    }
}

/// A fault planted in the live-edit rebase (E-LIVE, `edit_at_base`, v0.9.7), so the live
/// tests can show they catch each one (amendment E-LIVE r2's mutants). Installed per thread
/// by [`with_live_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LiveMutant {
    /// The server puts ours left of an insert at the same point (the client mirror doesn't).
    TieLeftForOurs,
    /// A deletion runs over text another change inserted inside its range.
    DeleteSwallowsInsert,
    /// merge3's O5 for live edits: two inserts at one point conflict.
    O5ForLive,
    /// A replacement's text goes at the right end of its range, after others' insertions.
    ReplacementTextPlacedRight,
    /// Two overlapping deletions conflict.
    DeleteDeleteConflicts,
    /// The client sends a base that leaves out its own acknowledged change (the client
    /// model's fault: the rebase itself has nothing to plant).
    OwnChangeInWindow,
    /// The window holds each change's hunks widened to whole words (as D1's token
    /// expansion widens a change inside `edit_as`), not the authored ones (clarification
    /// r2.1, F1): typing inside a multi-author word then bounces.
    WindowHoldsExpandedHunks,
    /// r2.2's rule (1) on the server only: its rebase re-canonicalizes the carried change at
    /// every step, the client's doesn't (E-LIVE r2.3): the two disagree (P1).
    RecanonServerOnly,
    /// r2.2's rule (1) on both sides: rule 4 then judges merged and split shapes, not the
    /// hunks as sent (a deletion bounces, a split replacement passes: P2, P2c).
    RecanonBothSides,
    /// "Strictly inside" judged by the tie-break's placement alone (`Bias::Right`), not under
    /// both placements (r2.3 S3): an insertion tied with another at its point bounces when a
    /// later deletion takes the other's text and one neighbour.
    InsideByTieBreak,
}

thread_local! {
    static LIVE_MUTANT: std::cell::Cell<Option<LiveMutant>> = const { std::cell::Cell::new(None) };
}

/// The live-rebase fault planted on this thread, if any.
pub fn live_mutant() -> Option<LiveMutant> {
    LIVE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_live_mutant<R>(mutant: LiveMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<LiveMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            LIVE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(LIVE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// The D1 amendment's mutants (v1.10), for the kit's integration tests: wrong variants of
/// the authored-ranges rule that the tests must catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum D1Mutant {
    /// The rule as specified.
    #[default]
    None,
    /// `reuse-foreign-elements-in-changed-token`: D1 before the amendment (no widening).
    ReuseForeign,
    /// `rewrite-whole-line`: the authored ranges widened to whole lines.
    RewriteWholeLine,
    /// `rewrite-merged-ranges`: `write --base` also rewrites what merge3 kept from the live
    /// text.
    RewriteMergedRanges,
}

thread_local! {
    static D1_MUTANT: std::cell::Cell<D1Mutant> = const { std::cell::Cell::new(D1Mutant::None) };
    static D1_EXTRA: std::cell::RefCell<Vec<(usize, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Run `f` with the D1 mutant `m` active on this thread.
pub fn with_d1_mutant<R>(m: D1Mutant, f: impl FnOnce() -> R) -> R {
    struct Back(D1Mutant);
    impl Drop for Back {
        fn drop(&mut self) {
            D1_MUTANT.with(|c| c.set(self.0));
        }
    }
    let _back = Back(D1_MUTANT.with(|c| c.replace(m)));
    f()
}

/// The D1 mutant active on this thread.
pub(crate) fn d1_mutant() -> D1Mutant {
    D1_MUTANT.with(|c| c.get())
}

/// The `rewrite-merged-ranges` mutant's extra ranges for the next change on this thread.
pub(crate) fn set_d1_extra(v: Vec<(usize, usize)>) {
    D1_EXTRA.with(|c| *c.borrow_mut() = v);
}

/// Take them (the change consumes them).
pub(crate) fn take_d1_extra() -> Vec<(usize, usize)> {
    D1_EXTRA.with(|c| std::mem::take(&mut *c.borrow_mut()))
}
