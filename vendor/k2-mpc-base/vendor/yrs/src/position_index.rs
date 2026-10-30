//! k2: a position index for text.
//!
//! Finding index `i` in a text (`find_position`) walks the text's blocks from its first one,
//! deleted ones included, so its cost grows with the text's whole history. This module keeps,
//! per text branch, a few **markers**: `(index, ID)` pairs, each the content index of one
//! character (a block's unit) known to be live in that branch. A lookup starts at the nearest
//! marker, walks from there, and re-seeds a marker where it lands.
//!
//! # What a marker is, and why it is an ID
//!
//! A marker holds the [ID] of a unit, never an [ItemPtr]: commit squashes neighbouring blocks
//! (`try_squash` drops the right one) and GC replaces blocks, so a stored pointer could go
//! stale.
//! A marker is resolved through the block store's lookup by ID (a binary search in the client's
//! block list), which returns the block holding the unit, wherever splits and squashes have put
//! it; the unit's offset in that block gives the content index of the block's start.
//!
//! # When markers are kept, and when dropped
//!
//! Only local text inserts and removals keep a branch's markers, by shifting them
//! ([Markers::shift_insert], [Markers::shift_remove]). Every other change to a branch's sequence
//! goes through `TransactionMut::integrate_item` or `TransactionMut::delete`, and both drop the
//! branch's markers: remote updates, pending updates, undo and redo, formatting and its
//! cleanup, embeds of shared types, deltas, and anything else. The next lookup walks, as before,
//! and seeds anew. Markers are used only by branches without formatting
//! (`Branch::has_formatting`), because the walk also gathers the formatting attributes in
//! force at the index; formatted branches always walk (Yjs makes the same choice).
//!
//! # Three choices, and why
//!
//! - **GC doesn't drop markers.** GC touches only deleted items: their content becomes
//!   `Deleted`, or, under a deleted parent, the block becomes a GC block. A marker never points
//!   at a deleted unit (a local removal drops the markers on the units it removes, and every
//!   other deletion drops the branch's markers), so GC can't change what a marker means.
//!   Dropping on GC would drop a text's markers after every committed local removal. Where GC
//!   collects blocks outright (under a deleted parent), the parent branch is collected too and
//!   its markers dropped ([crate::branch::Branch::is_collected]); and a marker whose ID resolves
//!   to a GC block counts as stale (it is dropped and the lookup walks).
//! - **Markers drop at the change, not at commit.** `integrate_item` and `delete` drop them
//!   when they happen, so no later lookup in the same transaction can use a marker that a
//!   change has outdated: at least as strict as dropping at commit.
//! - **`ItemPosition::index` from a marker is exact in UTF-16 only.** The old walk sums block
//!   (UTF-16) lengths; a lookup from a marker starts that sum at the marker's content index,
//!   which in byte offsets counts bytes. Nothing reads the field but `Display`, and the shadow
//!   check leaves it out ([Located::same_place]).
//!
//! # Runs of deleted blocks
//!
//! Between markers, a walk still passes every deleted block, and a long-lived text is mostly
//! deleted blocks. So walks also cross a run of deleted blocks in one step by its recorded end
//! ([Runs]), in lookups (from a marker or from the start) and in edits (an insert's skip past
//! deleted blocks, a removal). Deleted blocks count for nothing, so this finds the same places;
//! [crate::Doc::set_deleted_runs] turns it off (the test oracle).
//!
//! # Shadow check
//!
//! On a deterministic sample of marker lookups (every `shadow_every`-th, default 1 in 1000;
//! [crate::Doc::set_position_shadow_rate]) the lookup is checked. Without runs, the full walk
//! runs too and is compared. With runs, two checks run instead, neither costing time in
//! proportion to the text's history: the marker's scan again, stepping through every block (up
//! to [SHADOW_STEPS] of them: it checks the runs it crossed), and the walk from the start,
//! crossing runs (it checks the marker); only if either disagrees does the full walk run. At the
//! same rate, counted over crossings, an edit's run crossing is also stepped through (up to
//! [SHADOW_STEPS] blocks) and compared ([PositionIndexStats::run_checks]). If a check differs,
//! the stepped result is used, the branch's markers and runs are dropped and the audit hook
//! ([crate::Doc::set_position_audit_hook]) is called with a [PositionMismatch] (IDs and indices
//! only, never content).

use crate::block::{Item, ItemContent, ItemPtr, ID};
use crate::block_store::BlockStore;
use crate::branch::{BranchID, BranchPtr};
use crate::types::TypePtr;
use crate::OffsetKind;
use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// Markers kept per text branch.
///
/// 32, against Yjs's 80: each local edit shifts every marker after it and each lookup scans
/// them all, so both are O(markers); 32 is still more hot spots than the editors, agents and
/// cursors that edit one text at once in practice, and costs 32 × 24 bytes per branch, allocated
/// on the branch's first indexed lookup.
pub(crate) const MAX_MARKERS: usize = 32;

/// The default shadow rate: one marker lookup in this many also runs the full walk.
pub(crate) const DEFAULT_SHADOW_EVERY: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Marker {
    /// The content index (in the doc's [OffsetKind]) of the unit `id`.
    pub index: u32,
    /// A live unit of the branch.
    pub id: ID,
    /// When this marker was last used or set (a per-branch counter): the least recently used
    /// marker is the one replaced.
    pub used: u64,
}

/// A text branch's markers.
#[derive(Debug, Default)]
pub(crate) struct Markers {
    list: Vec<Marker>,
    clock: u64,
}

impl Markers {
    /// The marker nearest to `index`, by content index.
    pub fn nearest(&self, index: u32) -> Option<usize> {
        let mut best: Option<(usize, u32)> = None;
        for (i, m) in self.list.iter().enumerate() {
            let d = m.index.abs_diff(index);
            if best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    pub fn get(&self, i: usize) -> Marker {
        self.list[i]
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    pub fn touch(&mut self, i: usize) {
        let t = self.tick();
        self.list[i].used = t;
    }

    /// Records that unit `id` is at content `index`. If `reuse` names a marker (the one the
    /// lookup started from), that marker moves here; else a marker already on `id` is
    /// refreshed, or a new one is added, replacing the least recently used when full.
    pub fn put(&mut self, index: u32, id: ID, reuse: Option<usize>) {
        let used = self.tick();
        let m = Marker { index, id, used };
        if let Some(i) = self.list.iter().position(|m| m.id == id) {
            self.list[i] = m;
            if let Some(r) = reuse {
                if r != i {
                    // two markers on one unit: keep one
                    self.list.swap_remove(r);
                }
            }
            return;
        }
        if let Some(r) = reuse {
            self.list[r] = m;
        } else if self.list.len() < MAX_MARKERS {
            self.list.push(m);
        } else {
            let lru = self
                .list
                .iter()
                .enumerate()
                .min_by_key(|(_, m)| m.used)
                .map(|(i, _)| i)
                .unwrap();
            self.list[lru] = m;
        }
    }

    /// `len` units were inserted at content index `at`: they sit before the unit that was at
    /// `at`, so every marker at or after `at` moves right.
    pub fn shift_insert(&mut self, at: u32, len: u32) {
        for m in self.list.iter_mut() {
            if m.index >= at {
                m.index += len;
            }
        }
    }

    /// Units `at..at + len` were removed: markers on them go, markers after them move left.
    pub fn shift_remove(&mut self, at: u32, len: u32, planted: Option<PlantedPositionBug>) {
        let end = at + len;
        let _ = planted;
        self.list.retain_mut(|m| {
            if m.index < at {
                true
            } else if m.index < end {
                false
            } else {
                #[cfg(feature = "planted-position-bugs")]
                if planted == Some(PlantedPositionBug::RemoveOffByOne) {
                    m.index -= len - 1;
                    return true;
                }
                m.index -= len;
                true
            }
        });
    }

    #[cfg(test)]
    pub fn list(&self) -> &[Marker] {
        &self.list
    }
}

/// Bugs a test can plant in the position index to prove the property test and the shadow
/// check catch them. Compiled only with the test-only `planted-position-bugs` feature; in any
/// other build this type has no values and nothing can plant one.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlantedPositionBug {
    /// A removal moves the markers after it left by one unit too few.
    #[cfg(feature = "planted-position-bugs")]
    RemoveOffByOne,
    /// Integrating an item (a remote update, a redo) leaves the branch's markers in place.
    #[cfg(feature = "planted-position-bugs")]
    KeepMarkersOnIntegrate,
    /// A live block linked in between two deleted ones leaves the branch's runs in place.
    #[cfg(feature = "planted-position-bugs")]
    KeepRunsOnInterior,
}

/// How often the position index was used and checked; see [crate::Doc::position_index_stats].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PositionIndexStats {
    /// Index lookups in texts (inserts, removals, formatting).
    pub lookups: u64,
    /// Lookups that started from a marker.
    pub marker_hits: u64,
    /// Lookups that walked from the text's start: formatted texts, index 0, no marker yet, a
    /// stale marker, or the index turned off.
    pub walks: u64,
    /// Marker lookups that the full walk also ran for (the shadow check).
    pub shadow_checks: u64,
    /// Shadow checks where the two differed (each also called the audit hook).
    pub mismatches: u64,
    /// Markers that no longer resolved to a live unit of their branch (dropped, walked).
    pub stale_markers: u64,
    /// k2: edits' crossings of a run of deleted blocks that the shadow check also stepped
    /// through block by block (a difference counts in `mismatches`).
    pub run_checks: u64,
}

/// Where a lookup put an index: the units either side of it, as IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPosition {
    /// The last unit before the index (`None` at the start).
    pub left: Option<ID>,
    /// The first unit (or block) after the index (`None` at the end).
    pub right: Option<ID>,
    /// The content index of the start of the block holding `left`.
    pub left_start: u32,
    /// Units of the index past the end of the text (0 when the index is in range).
    pub missing: u32,
}

/// A shadow check found the marker path and the full walk disagree; see
/// [crate::Doc::set_position_audit_hook]. It holds IDs and indices only, never content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionMismatch {
    /// The text branch.
    pub branch: BranchID,
    /// The index looked up.
    pub index: u32,
    /// The marker the lookup started from: its content index and unit.
    pub marker: (u32, ID),
    /// What the marker path found.
    pub marker_result: TextPosition,
    /// What the full walk found (and what the lookup used).
    pub walk_result: TextPosition,
}

pub(crate) type AuditHook = Box<dyn Fn(&PositionMismatch) + Send + Sync + 'static>;

/// The store's position index settings and counters.
pub(crate) struct PositionIndex {
    pub enabled: bool,
    /// k2: lookups and edits cross runs of deleted blocks by their recorded ends ([Runs]);
    /// off, they step through every block ([crate::Doc::set_deleted_runs], the test oracle).
    pub runs: bool,
    pub shadow_every: u32,
    /// k2: run crossings made by edits, for the shadow check's sample of them.
    pub crossings: u64,
    pub hook: Option<AuditHook>,
    pub stats: PositionIndexStats,
    pub planted: Option<PlantedPositionBug>,
}

impl Default for PositionIndex {
    fn default() -> Self {
        PositionIndex {
            enabled: true,
            runs: true,
            shadow_every: DEFAULT_SHADOW_EVERY,
            crossings: 0,
            hook: None,
            stats: PositionIndexStats::default(),
            planted: None,
        }
    }
}

impl PositionIndex {
    /// Whether lookups and edits use the branches' [Runs].
    #[inline]
    pub fn uses_runs(&self) -> bool {
        self.enabled && self.runs
    }

    /// Whether this run crossing (by an edit) is one the shadow check samples.
    pub fn sample_crossing(&mut self) -> bool {
        self.crossings += 1;
        self.shadow_every != 0 && self.crossings % self.shadow_every as u64 == 0
    }

    /// Whether this marker lookup is one the shadow check samples.
    pub fn sample(&mut self) -> bool {
        if self.shadow_every == 0 {
            return false;
        }
        self.stats.marker_hits % self.shadow_every as u64 == 0
    }

    #[inline]
    pub fn keeps_runs_on_interior(&self) -> bool {
        #[cfg(feature = "planted-position-bugs")]
        if self.planted == Some(PlantedPositionBug::KeepRunsOnInterior) {
            return true;
        }
        false
    }

    #[inline]
    pub fn keeps_markers_on_integrate(&self) -> bool {
        #[cfg(feature = "planted-position-bugs")]
        if self.planted == Some(PlantedPositionBug::KeepMarkersOnIntegrate) {
            return true;
        }
        false
    }
}

/// Drops a branch's markers: called on every change to its sequence that local text edits
/// don't account for.
#[inline]
pub(crate) fn drop_markers(mut branch: BranchPtr) {
    if branch.markers.is_some() {
        branch.markers = None;
    }
}

/// k2: runs of deleted blocks in a text's sequence, so a walk crosses one in a step or two
/// instead of block by block.
///
/// A text's sequence keeps every deleted block (its tombstones) for good, so a walk that
/// steps through them costs time in proportion to the text's history, not its content. A walk
/// that has stepped through two or more deleted blocks in a row records the run it crossed:
/// in [Runs::right], the run's first block maps to the run's last unit; in [Runs::left], its
/// last block maps to the run's first unit. An end block is recorded under both its first and
/// its last unit ([keys]), so a squash on either side of it keeps one. A later walk arriving
/// at either end goes straight to the other.
///
/// # Why a recorded run stays a run
///
/// A run is a stretch of the sequence where every block is deleted. A block never becomes
/// live again once deleted (undo and redo make new items), and blocks never move in a text.
/// Splitting or squashing deleted blocks keeps the stretch as it is, and the run's ends are
/// kept as unit IDs, resolved through the block store, so they find the block holding the
/// unit wherever a split or squash has put it. The one change that breaks a run is a **live**
/// block linked in between two deleted ones; `TransactionMut::integrate_item` drops the
/// branch's runs whenever that happens ([drop_runs]). A deleted block linked in anywhere, or a
/// live one next to at most one deleted neighbour, leaves every run a run.
///
/// Every end is checked when used: it must resolve to a deleted block of the same branch,
/// other than the block it was looked up from; else the entry is removed and the walk steps.
/// A crossing takes at most [MAX_HOPS] recorded steps and then steps block by block, so it
/// always ends. The shadow check steps through what they cross, on its sample (see the module
/// notes).
#[derive(Debug, Default)]
pub(crate) struct Runs {
    /// The first and the last unit of a run's first block -> the ID of the run's last unit.
    right: IdMap,
    /// The first and the last unit of a run's last block -> the ID of the run's first unit.
    left: IdMap,
}

type IdMap = HashMap<ID, ID, BuildHasherDefault<IdHasher>>;

/// Recorded runs kept per text beyond twice its live length, before they are all dropped (an
/// entry can outlive its use, when its block is squashed into its left neighbour).
const RUNS_SLACK: usize = 4096;

/// Recorded steps one crossing takes at most before stepping block by block.
pub(crate) const MAX_HOPS: u32 = 64;

/// A small multiplicative hasher for IDs (two integers, already well spread).
#[derive(Default)]
pub(crate) struct IdHasher(u64);

impl IdHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for IdHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(b as u64);
        }
    }
    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(n as u64);
    }
    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }
}

impl Runs {
    /// Entries in each map.
    pub fn len(&self) -> (usize, usize) {
        (self.right.len(), self.left.len())
    }
}

/// Drops a branch's recorded runs: a live block was linked in between two deleted ones.
#[inline]
pub(crate) fn drop_runs(mut branch: BranchPtr) {
    branch.run_drops += 1;
    if branch.runs.is_some() {
        branch.runs = None;
    }
}

/// Is `item` a deleted block of `branch`?
#[inline]
fn is_deleted_in(item: &Item, branch: BranchPtr) -> bool {
    item.is_deleted() && matches!(item.parent, TypePtr::Branch(p) if p == branch)
}

/// The recorded other end of the run at `from`, checked; a bad entry is removed.
#[inline]
fn hop(
    map: &mut IdMap,
    key: ID,
    blocks: &BlockStore,
    mut branch: BranchPtr,
    from: ItemPtr,
) -> Option<ItemPtr> {
    let to = *map.get(&key)?;
    match blocks.get_item(&to) {
        Some(item) if item != from && is_deleted_in(&item, branch) => Some(item),
        _ => {
            branch.run_rejects += 1;
            map.remove(&key);
            None
        }
    }
}

/// The keys a run's end block is recorded under: its first unit and its last unit (one key
/// for a block of one unit). A walk enters a run at an end block; when that block is squashed
/// at commit with a neighbour (the block before it, once deleted too, or the next piece of the
/// same split block), the merged block keeps one of the two: its start if it absorbed the
/// block after it, its end if the block before it absorbed it.
#[inline]
fn keys(block: ItemPtr) -> smallvec::SmallVec<[ID; 2]> {
    let (first, last) = (block.id, block.last_id());
    if first == last {
        smallvec::smallvec![first]
    } else {
        smallvec::smallvec![first, last]
    }
}

/// Records the run from `first` to `last` (left to right), and forgets the entries `hops` (the
/// recorded steps the crossing took, from block to block, in its direction) leave inside it: a
/// walk enters a run only at its ends, so an entry inside one is never looked up again.
fn record(
    mut branch: BranchPtr,
    first: ItemPtr,
    last: ItemPtr,
    hops: &[(ItemPtr, ItemPtr)],
    rightwards: bool,
) {
    let cap = RUNS_SLACK + 2 * branch.block_len as usize;
    let runs = branch.runs.get_or_insert_with(Box::default);
    if runs.right.len() > cap || runs.left.len() > cap {
        runs.right.clear();
        runs.left.clear();
    }
    for &(from, to) in hops {
        let (map, keep) = if rightwards {
            (&mut runs.right, first)
        } else {
            (&mut runs.left, last)
        };
        if from != keep {
            for k in keys(from) {
                map.remove(&k);
            }
        }
        let (map, keep) = if rightwards {
            (&mut runs.left, last)
        } else {
            (&mut runs.right, first)
        };
        if to != keep {
            for k in keys(to) {
                map.remove(&k);
            }
        }
    }
    for k in keys(first) {
        runs.right.insert(k, last.last_id());
    }
    for k in keys(last) {
        runs.left.insert(k, first.id);
    }
}

/// The last block of the run of deleted blocks that starts at `first` (deleted), going right:
/// where stepping right from `first` while the next block is deleted ends.
pub(crate) fn cross_right(branch: BranchPtr, blocks: &BlockStore, first: ItemPtr) -> ItemPtr {
    cross(branch, blocks, first, true)
}

/// The first block of the run of deleted blocks that ends at `last` (deleted), going left:
/// where stepping left from `last` while the next block is deleted ends.
pub(crate) fn cross_left(branch: BranchPtr, blocks: &BlockStore, last: ItemPtr) -> ItemPtr {
    cross(branch, blocks, last, false)
}

fn cross(branch: BranchPtr, blocks: &BlockStore, from: ItemPtr, rightwards: bool) -> ItemPtr {
    let mut cur = from;
    let mut moves = 0u32;
    let mut hops: smallvec::SmallVec<[(ItemPtr, ItemPtr); 4]> = smallvec::SmallVec::new();
    loop {
        if (hops.len() as u32) < MAX_HOPS {
            let mut b = branch;
            if let Some(runs) = b.runs.as_deref_mut() {
                let map = if rightwards {
                    &mut runs.right
                } else {
                    &mut runs.left
                };
                // the block's facing end first (going right its last unit, going left its
                // first), then its other end
                let (a, z) = (cur.id, cur.last_id());
                let (k1, k2) = if rightwards { (z, a) } else { (a, z) };
                let found = hop(map, k1, blocks, branch, cur).or_else(|| {
                    (k2 != k1)
                        .then(|| hop(map, k2, blocks, branch, cur))
                        .flatten()
                });
                if let Some(end) = found {
                    b.run_hops += 1;
                    hops.push((cur, end));
                    cur = end;
                    moves += 1;
                }
            }
        }
        let next = if rightwards { cur.right } else { cur.left };
        match next {
            Some(next) if next.is_deleted() => {
                cur = next;
                moves += 1;
            }
            _ => break,
        }
    }
    if moves >= 2 {
        if rightwards {
            record(branch, from, cur, &hops, true);
        } else {
            record(branch, cur, from, &hops, false);
        }
    }
    cur
}

/// How a scan goes through deleted blocks.
#[derive(Clone, Copy)]
pub(crate) enum Steps<'a> {
    /// Block by block.
    All,
    /// Crossing runs by their recorded ends ([cross_right], [cross_left]): the text and the block
    /// store.
    Runs(BranchPtr, &'a BlockStore),
    /// Block by block, at most this many blocks (the shadow check's budget); past it the scan
    /// stops with [Stop::Budget].
    Budget(&'a Cell<u32>),
}

impl Steps<'_> {
    /// Counts one block stepped through; `false` once the budget is spent.
    #[inline]
    fn step(&self) -> bool {
        match self {
            Steps::Budget(left) => match left.get() {
                0 => false,
                n => {
                    left.set(n - 1);
                    true
                }
            },
            _ => true,
        }
    }
}

/// Blocks the shadow check's second scan from the marker steps through at most
/// ([Steps::Budget]).
pub(crate) const SHADOW_STEPS: u32 = 4096;

/// Where an index lands, as the walk in `find_position` finds it, before the walk's split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Located {
    pub left: Option<ItemPtr>,
    /// `left.right` before any split.
    pub right: Option<ItemPtr>,
    /// When > 0, `left` must be split at this (UTF-16) offset, and the split-off part is the
    /// position's right.
    pub split: u32,
    /// Units of the index past the text's end.
    pub remaining: u32,
    /// The content index of `left`'s first unit (when `left` is a counted block).
    pub left_start: u32,
    /// What the old walk accumulates in `ItemPosition::index` (UTF-16 lengths of the counted
    /// blocks passed, the split one's counted as `len - split`), from the scan's start.
    pub index: u32,
}

impl Located {
    /// Whether two lookups found the same place (`index` is diagnostic only and, counted from
    /// a marker in byte offsets, not comparable).
    pub fn same_place(&self, other: &Located) -> bool {
        self.left == other.left
            && self.right == other.right
            && self.split == other.split
            && self.remaining == other.remaining
            && self.left_start == other.left_start
    }

    pub fn position(&self) -> TextPosition {
        let (left, right) = match self.left {
            Some(l) if self.split > 0 => (
                Some(ID::new(l.id.client, l.id.clock + self.split - 1)),
                Some(ID::new(l.id.client, l.id.clock + self.split)),
            ),
            Some(l) => (Some(l.last_id()), self.right.map(|r| r.id)),
            None => (None, self.right.map(|r| r.id)),
        };
        TextPosition {
            left,
            right,
            left_start: self.left_start,
            missing: self.remaining,
        }
    }

    /// The marker this lookup leaves: the start of the block holding the index's last unit.
    pub fn seed(&self) -> Option<(u32, ID)> {
        match self.left {
            Some(l) if self.remaining == 0 => Some((self.left_start, l.id)),
            _ => None,
        }
    }
}

/// Why a scan stopped without a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// A live format item: the branch must walk the old, formatting-aware way.
    Format,
    /// The numbers don't add up: an overflow, or a split that isn't inside the block or on a
    /// character boundary. From a marker, the marker is stale; from the text's start, the
    /// index itself is bad and the old walk handles it as it always has.
    Invalid,
    /// k2: a [Steps::Budget] scan spent its budget.
    Budget,
}

/// The walk of `find_position`, without its split and for a text without formatting: from
/// `right` (whose content index is `start`), find `remaining` units on. Read-only, and all its
/// arithmetic is checked, so a wrong start (a stale marker) gives [Stop::Invalid] or a wrong
/// place, never an overflow or a split outside a block.
///
/// k2: with [Steps::Runs], a run of deleted blocks is crossed by its recorded end
/// ([cross_right]): the same place, as deleted blocks count for nothing (only the recorded runs
/// change).
pub(crate) fn scan(
    mut left: Option<ItemPtr>,
    mut right: Option<ItemPtr>,
    mut remaining: u32,
    start: u32,
    encoding: OffsetKind,
    steps: Steps,
) -> Result<Located, Stop> {
    let mut index: u32 = 0;
    let mut cursor = start;
    let mut left_start = start;
    while let Some(r) = right {
        if remaining == 0 {
            break;
        }
        if !steps.step() {
            return Err(Stop::Budget);
        }
        if r.is_deleted() {
            if let Steps::Runs(branch, blocks) = steps {
                let last = cross_right(branch, blocks, r);
                left = Some(last);
                right = last.right;
                continue;
            }
        } else {
            if let ItemContent::Format(_, _) = &r.content {
                return Err(Stop::Format);
            }
            let block_len = r.len();
            let content_len = r.content_len(encoding);
            if remaining < content_len {
                let offset = split_offset(&r, remaining, encoding).ok_or(Stop::Invalid)?;
                return Ok(Located {
                    left: Some(r),
                    right: r.right,
                    split: offset,
                    remaining: 0,
                    left_start: cursor,
                    index: index.checked_add(block_len - offset).ok_or(Stop::Invalid)?,
                });
            }
            remaining -= content_len;
            index = index.checked_add(block_len).ok_or(Stop::Invalid)?;
            left_start = cursor;
            cursor = cursor.checked_add(content_len).ok_or(Stop::Invalid)?;
        }
        left = Some(r);
        right = r.right;
    }
    Ok(Located {
        left,
        right,
        split: 0,
        remaining,
        left_start,
        index,
    })
}

/// The (UTF-16) offset at which the walk splits `item` to put `remaining` content units on
/// its left: what `SplittableString::block_offset` computes, but `None` unless it falls inside
/// the block (0 < offset < len) and, in byte offsets, on a character boundary (where
/// `block_offset` has no meaningful answer).
fn split_offset(item: &Item, remaining: u32, encoding: OffsetKind) -> Option<u32> {
    let offset = match (&item.content, encoding) {
        (ItemContent::String(s), OffsetKind::Bytes) => {
            let (mut bytes, mut units) = (0u32, 0u32);
            for c in s.as_str().chars() {
                if bytes >= remaining {
                    break;
                }
                bytes += c.len_utf8() as u32;
                units += c.len_utf16() as u32;
            }
            if bytes != remaining {
                return None;
            }
            units
        }
        _ => remaining,
    };
    (offset > 0 && offset < item.len()).then_some(offset)
}

/// Content units (in `encoding`) of the first `clock_offset` units of `item`. `None` if the
/// offset doesn't fall on a character boundary.
fn content_prefix(item: &Item, clock_offset: u32, encoding: OffsetKind) -> Option<u32> {
    if clock_offset == 0 {
        return Some(0);
    }
    match (&item.content, encoding) {
        (ItemContent::String(s), OffsetKind::Bytes) => {
            let (mut units, mut bytes) = (0u32, 0u32);
            for c in s.as_str().chars() {
                if units == clock_offset {
                    break;
                }
                units += c.len_utf16() as u32;
                bytes += c.len_utf8() as u32;
            }
            (units == clock_offset).then_some(bytes)
        }
        // every other content, and strings counted in UTF-16, has as many content units as
        // block (UTF-16) units
        _ => Some(clock_offset),
    }
}

/// Is `item` a live, counted unit of `branch` (what a marker must point at)?
fn is_live_in(item: &Item, branch: BranchPtr) -> bool {
    !item.is_deleted()
        && !matches!(item.content, ItemContent::Format(_, _))
        && matches!(item.parent, TypePtr::Branch(p) if p == branch)
}

/// Resolves a marker to the block holding its unit and that block's content index.
pub(crate) fn resolve(
    blocks: &crate::block_store::BlockStore,
    branch: BranchPtr,
    marker: &Marker,
    encoding: OffsetKind,
) -> Option<(ItemPtr, u32)> {
    let item = blocks.get_item(&marker.id)?;
    if !is_live_in(&item, branch) {
        return None;
    }
    let offset = marker.id.clock - item.id.clock;
    let prefix = content_prefix(&item, offset, encoding)?;
    let start = marker.index.checked_sub(prefix)?;
    Some((item, start))
}

/// The lookup of `index` (> 0) from a resolved marker: the block `anchor`, whose first unit is
/// at content index `start`. [Stop::Invalid] if the numbers don't add up (the walk back runs
/// past the text's start or below 0, an overflow, a bad split): the marker is stale.
pub(crate) fn scan_from(
    anchor: ItemPtr,
    start: u32,
    index: u32,
    encoding: OffsetKind,
    steps: Steps,
) -> Result<Located, Stop> {
    let from = |block: ItemPtr, at: u32| -> Result<Located, Stop> {
        let mut l = scan(block.left, Some(block), index - at, at, encoding, steps)?;
        l.index = l.index.checked_add(at).ok_or(Stop::Invalid)?;
        Ok(l)
    };
    if index > start {
        return from(anchor, start);
    }
    // walk back to the counted block holding unit `index - 1`
    let mut cursor = start;
    let mut block = anchor.left;
    while let Some(b) = block {
        if !steps.step() {
            return Err(Stop::Budget);
        }
        if !b.is_deleted() {
            if let ItemContent::Format(_, _) = &b.content {
                return Err(Stop::Format);
            }
            let len = b.content_len(encoding);
            cursor = cursor.checked_sub(len).ok_or(Stop::Invalid)?;
            if cursor < index {
                return from(b, cursor);
            }
        } else if let Steps::Runs(branch, blocks) = steps {
            block = cross_left(branch, blocks, b).left;
            continue;
        }
        block = b.left;
    }
    Err(Stop::Invalid)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::block::ClientID;
    use crate::types::text::TextRef;
    use crate::types::{Attrs, Delta};
    use crate::undo::{Options as UndoOptions, UndoManager};
    use crate::updates::decoder::Decode;
    use crate::{
        Any, Array, Doc, GetString, Options, ReadTxn, StateVector, Text, TextPrelim, Transact,
        Update, Xml, XmlFragment, XmlTextPrelim, ID,
    };
    use std::sync::{Arc, Mutex};

    fn doc(client: u64, skip_gc: bool, kind: OffsetKind) -> Doc {
        let d = Doc::with_options(Options {
            client_id: ClientID::new(client),
            offset_kind: kind,
            skip_gc,
            ..Options::default()
        });
        d.set_position_shadow_rate(1);
        d
    }

    /// Installs a hook recording every mismatch.
    fn audit(d: &Doc) -> Arc<Mutex<Vec<PositionMismatch>>> {
        let seen: Arc<Mutex<Vec<PositionMismatch>>> = Arc::default();
        let s2 = seen.clone();
        d.set_position_audit_hook(Some(Box::new(move |m| s2.lock().unwrap().push(m.clone()))));
        seen
    }

    fn branch<T: AsRef<crate::branch::Branch>>(t: &T) -> BranchPtr {
        BranchPtr::from(t.as_ref())
    }

    fn markers<T: AsRef<crate::branch::Branch>>(t: &T) -> Vec<Marker> {
        branch(t)
            .markers
            .as_deref()
            .map(|m| m.list().to_vec())
            .unwrap_or_default()
    }

    fn has_markers<T: AsRef<crate::branch::Branch>>(t: &T) -> bool {
        branch(t).markers.is_some()
    }

    fn text(d: &Doc, t: &TextRef) -> String {
        t.get_string(&d.transact())
    }

    /// A lookup at every character boundary of `t` (an insert, then a removal of it, so the
    /// text is unchanged), checked by the shadow (rate 1).
    fn probe_all(d: &Doc, t: &TextRef) {
        let s = text(d, t);
        let kind = d.transact().store().offset_kind;
        let mut at = 0;
        for c in s.chars() {
            at += match kind {
                OffsetKind::Utf16 => c.len_utf16() as u32,
                OffsetKind::Bytes => c.len_utf8() as u32,
            };
            t.insert(&mut d.transact_mut(), at, "#");
            t.remove_range(&mut d.transact_mut(), at, 1);
        }
        assert_eq!(text(d, t), s);
    }

    /// Markers hold IDs: one left inside a block that commit then squashed resolves to that
    /// block at the right content index, and lookups from it match the walk.
    #[test]
    fn a_marker_inside_a_squashed_block_resolves() {
        for kind in [OffsetKind::Utf16, OffsetKind::Bytes] {
            let d = doc(1, false, kind);
            let seen = audit(&d);
            let t = d.get_or_insert_text("t");
            {
                let mut txn = d.transact_mut();
                t.insert(&mut txn, 0, "é"); // 2 bytes, 1 UTF-16 unit
                let w = |k: u32| {
                    if let OffsetKind::Bytes = kind {
                        2 * k
                    } else {
                        k
                    }
                };
                t.insert(&mut txn, w(1), "é");
                t.insert(&mut txn, w(2), "é"); // seeds a marker on (1, 1), the second block
                t.insert(&mut txn, w(3), "é");
            }
            // commit squashed the four blocks into one
            let txn = d.transact();
            let start = branch(&t).start.unwrap();
            assert_eq!(start.len(), 4, "squashed into one block");
            let inside: Vec<Marker> = markers(&t)
                .into_iter()
                .filter(|m| m.id.clock > start.id.clock)
                .collect();
            assert!(!inside.is_empty(), "a marker points inside the block");
            for m in inside {
                let (item, at) = resolve(&txn.store().blocks, branch(&t), &m, kind).unwrap();
                assert_eq!(item, start);
                assert_eq!(at, 0, "the block's start, from a marker at {}", m.index);
            }
            drop(txn);
            let before = d.position_index_stats();
            probe_all(&d, &t);
            let after = d.position_index_stats();
            assert!(after.marker_hits > before.marker_hits);
            assert!(seen.lock().unwrap().is_empty());
        }
    }

    /// GC after a marker is set (the removed block's content becomes `Deleted`, squashed with
    /// its deleted neighbours), and a full GC: the markers resolve, lookups match the walk.
    #[test]
    fn gc_after_a_marker_is_set() {
        let d = doc(1, false, OffsetKind::Utf16);
        let seen = audit(&d);
        let t = d.get_or_insert_text("t");
        t.insert(&mut d.transact_mut(), 0, "hello ");
        t.insert(
            &mut d.transact_mut_as(ClientID::new(2)),
            6,
            "big wide world",
        );
        t.insert(&mut d.transact_mut(), 15, "!"); // a marker on client 2's block
        assert!(has_markers(&t));
        t.remove_range(&mut d.transact_mut(), 1, 3); // committed, then GC'd
        assert!(has_markers(&t), "a local removal keeps the markers");
        assert_eq!(text(&d, &t), "ho big wide !world");
        let before = d.position_index_stats();
        probe_all(&d, &t);
        d.transact_mut().gc(None);
        probe_all(&d, &t);
        assert!(d.position_index_stats().marker_hits > before.marker_hits);
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(text(&d, &t), "ho big wide !world");
    }

    /// A doc and its text, with markers set by one lookup.
    fn primed(skip_gc: bool) -> (Doc, TextRef) {
        let d = doc(1, skip_gc, OffsetKind::Utf16);
        let t = d.get_or_insert_text("t");
        t.insert(&mut d.transact_mut(), 0, "abcdef");
        t.insert(&mut d.transact_mut(), 3, "-");
        assert!(has_markers(&t), "primed");
        (d, t)
    }

    fn peer_update(
        d: &Doc,
        client: u64,
        f: impl FnOnce(&mut crate::TransactionMut, &TextRef),
    ) -> Vec<u8> {
        let p = doc(client, false, OffsetKind::Utf16);
        let pt = p.get_or_insert_text("t");
        let full = d
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        p.transact_mut()
            .apply_update(Update::decode_v1(&full).unwrap())
            .unwrap();
        let mut txn = p.transact_mut();
        f(&mut txn, &pt);
        txn.encode_update_v1()
    }

    fn apply(d: &Doc, u: &[u8]) {
        d.transact_mut()
            .apply_update(Update::decode_v1(u).unwrap())
            .unwrap();
    }

    #[test]
    fn local_edits_keep_the_markers() {
        let (d, t) = primed(false);
        t.insert(&mut d.transact_mut(), 5, "xy");
        t.remove_range(&mut d.transact_mut(), 1, 2);
        t.insert(&mut d.transact_mut_as(ClientID::new(9)), 2, "z");
        t.insert_embed(&mut d.transact_mut(), 4, Any::from(7));
        t.insert_with_attributes(&mut d.transact_mut(), 1, "q", Attrs::new());
        assert!(has_markers(&t));
    }

    #[test]
    fn a_remote_insert_drops_the_markers() {
        let (d, t) = primed(false);
        let u = peer_update(&d, 2, |txn, pt| pt.insert(txn, 1, "R"));
        apply(&d, &u);
        assert!(!has_markers(&t));
    }

    #[test]
    fn a_remote_delete_drops_the_markers() {
        let (d, t) = primed(false);
        let u = peer_update(&d, 2, |txn, pt| pt.remove_range(txn, 1, 1));
        apply(&d, &u);
        assert!(!has_markers(&t));
    }

    #[test]
    fn a_pending_update_drops_the_markers_when_it_integrates() {
        let (d, t) = primed(false);
        let p = doc(2, false, OffsetKind::Utf16);
        let pt = p.get_or_insert_text("t");
        let full = d
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        apply(&p, &full);
        let u1 = {
            let mut txn = p.transact_mut();
            pt.insert(&mut txn, 0, "1");
            txn.encode_update_v1()
        };
        let u2 = {
            let mut txn = p.transact_mut();
            // right after "1" (2, 0): its origin is the block the doc lacks, so it waits (an
            // item whose origins the doc has integrates at once, over a Skip for the gap)
            pt.insert(&mut txn, 1, "2");
            txn.encode_update_v1()
        };
        let before = text(&d, &t);
        apply(&d, &u2); // waits for u1
        assert!(d.transact().store().pending_update().is_some());
        assert_eq!(text(&d, &t), before, "nothing integrated yet");
        assert!(has_markers(&t), "nothing integrated, so the markers stay");
        apply(&d, &u1);
        assert!(!has_markers(&t));
        assert_eq!(text(&d, &t), text(&p, &pt));
    }

    #[test]
    fn undo_and_redo_drop_the_markers() {
        let (d, t) = primed(false);
        let mut mgr = UndoManager::<()>::with_options({
            let mut o = UndoOptions::default();
            o.capture_timeout_millis = 0;
            o
        });
        mgr.expand_scope(&d, &t);
        t.insert(&mut d.transact_mut(), 2, "u");
        assert!(has_markers(&t));
        let lookups = d.position_index_stats().lookups;
        assert!(mgr.undo_blocking());
        assert!(!has_markers(&t), "undo");
        assert_eq!(
            d.position_index_stats().lookups,
            lookups,
            "undo looks nothing up"
        );
        // re-seed in an edit the manager doesn't track (it would clear the redo stack)
        t.insert(&mut d.transact_mut_with("untracked"), 4, "v");
        assert!(has_markers(&t));
        let lookups = d.position_index_stats().lookups;
        assert!(mgr.redo_blocking());
        assert!(!has_markers(&t), "redo");
        assert_eq!(
            d.position_index_stats().lookups,
            lookups,
            "redo looks nothing up"
        );
        assert_eq!(text(&d, &t), "abuc-vdef");
    }

    #[test]
    fn formatting_drops_the_markers_and_then_walks() {
        let (d, t) = primed(false);
        let mut bold = Attrs::new();
        bold.insert(Arc::from("bold"), Any::Bool(true));
        t.format(&mut d.transact_mut(), 1, 2, bold.clone());
        assert!(!has_markers(&t));
        assert!(branch(&t).has_formatting);
        let before = d.position_index_stats();
        probe_all(&d, &t);
        let after = d.position_index_stats();
        assert_eq!(
            after.marker_hits, before.marker_hits,
            "a formatted text walks"
        );
        assert!(after.walks > before.walks);
        assert!(!has_markers(&t));
        // and a formatted insert on a plain text
        let (d, t) = primed(false);
        t.insert_with_attributes(&mut d.transact_mut(), 2, "b", bold);
        assert!(!has_markers(&t));
    }

    #[test]
    fn a_delta_drops_the_markers() {
        let (d, t) = primed(false);
        t.apply_delta(
            &mut d.transact_mut(),
            [
                Delta::Retain(2, None),
                Delta::Inserted(Any::from("D"), None),
            ],
        );
        assert!(!has_markers(&t));
    }

    #[test]
    fn an_embedded_shared_type_drops_the_markers() {
        let (d, t) = primed(false);
        t.insert_embed(&mut d.transact_mut(), 2, TextPrelim::new("inner"));
        assert!(!has_markers(&t));
    }

    /// `remove_range` past the end panics ("Couldn't remove 50 elements…", which the test log
    /// shows) after deleting what it could, and the transaction commits as it unwinds: upstream
    /// behaviour, the same with the index off. The markers, out for the edit, stay dropped.
    #[test]
    fn a_failed_removal_drops_the_markers() {
        let fail = |d: &Doc, t: &TextRef| {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                t.remove_range(&mut d.transact_mut(), 5, 50);
            }));
            assert!(r.is_err(), "removing past the end panics");
        };
        let (d, t) = primed(false);
        let seen = audit(&d);
        let (twin, tt) = primed(false);
        twin.set_position_index(false);
        fail(&d, &t);
        fail(&twin, &tt);
        assert!(!has_markers(&t));
        assert_eq!(
            text(&d, &t),
            "abc-d",
            "the two characters it could remove are gone"
        );
        assert_eq!(
            d.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            twin.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            "the same as the walk's"
        );
        probe_all(&d, &t);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn an_xml_text_attribute_drops_the_markers() {
        let d = doc(1, false, OffsetKind::Utf16);
        let f = d.get_or_insert_xml_fragment("x");
        let xt = f.insert(&mut d.transact_mut(), 0, XmlTextPrelim::new("abcdef"));
        xt.insert(&mut d.transact_mut(), 3, "-");
        assert!(has_markers(&xt));
        xt.insert_attribute(&mut d.transact_mut(), "k", "v");
        assert!(!has_markers(&xt));
    }

    #[test]
    fn deleting_the_parent_drops_the_markers() {
        let d = doc(1, true, OffsetKind::Utf16);
        let a = d.get_or_insert_array("a");
        let t = a.insert(&mut d.transact_mut(), 0, TextPrelim::new("abcdef"));
        t.insert(&mut d.transact_mut(), 3, "-");
        assert!(has_markers(&t));
        a.remove_range(&mut d.transact_mut(), 0, 1);
        assert!(!has_markers(&t));
    }

    /// A formatted text never takes the marker path, from its first lookup.
    #[test]
    fn a_formatted_text_walks() {
        let d = doc(1, false, OffsetKind::Utf16);
        let t = d.get_or_insert_text("t");
        let mut bold = Attrs::new();
        bold.insert(Arc::from("bold"), Any::Bool(true));
        t.insert_with_attributes(&mut d.transact_mut(), 0, "bold", bold);
        t.insert(&mut d.transact_mut(), 4, " plain text");
        probe_all(&d, &t);
        let s = d.position_index_stats();
        assert_eq!(s.marker_hits, 0);
        assert_eq!(s.walks, s.lookups);
        assert!(!has_markers(&t));
    }

    #[test]
    fn turned_off_it_walks_and_keeps_no_markers() {
        let (d, t) = primed(false);
        d.set_position_index(false);
        let before = d.position_index_stats();
        probe_all(&d, &t);
        let after = d.position_index_stats();
        assert_eq!(after.marker_hits, before.marker_hits);
        assert!(!has_markers(&t));
    }

    /// With markers wrong (the planted off-by-one), the 1-in-1 shadow catches the lookup: the
    /// hook fires with IDs and indices, the walk's result is used and the markers drop.
    #[cfg(feature = "planted-position-bugs")]
    #[test]
    fn the_shadow_fires_on_a_planted_bug() {
        for bug in [
            PlantedPositionBug::RemoveOffByOne,
            PlantedPositionBug::KeepMarkersOnIntegrate,
        ] {
            let d = doc(1, false, OffsetKind::Utf16);
            d.plant_position_bug(Some(bug));
            let seen = audit(&d);
            let t = d.get_or_insert_text("t");
            t.insert(&mut d.transact_mut(), 0, "01234");
            t.insert(&mut d.transact_mut_as(ClientID::new(2)), 5, "56789");
            t.insert(&mut d.transact_mut(), 8, "x"); // a marker on (2, 0) at 5
            assert!(markers(&t)
                .iter()
                .any(|m| m.id == ID::new(ClientID::new(2), 0)));
            match bug {
                PlantedPositionBug::RemoveOffByOne => {
                    t.remove_range(&mut d.transact_mut(), 1, 2) // (2, 0) now at 3, marker says 4
                }
                PlantedPositionBug::KeepMarkersOnIntegrate => {
                    let u = peer_update(&d, 3, |txn, pt| pt.insert(txn, 0, "RR"));
                    apply(&d, &u); // (2, 0) now at 7, marker says 5
                }
                PlantedPositionBug::KeepRunsOnInterior => unreachable!(),
            }
            let want = {
                let mut s = text(&d, &t);
                s.insert(6, 'Y');
                s
            };
            t.insert(&mut d.transact_mut(), 6, "Y");
            assert_eq!(text(&d, &t), want, "{bug:?}: the walk's result was used");
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "{bug:?}: the hook fired once");
            let m = &seen[0];
            assert_eq!(m.branch, crate::BranchID::Root("t".into()));
            assert_eq!(m.index, 6);
            assert_eq!(m.marker.1, ID::new(ClientID::new(2), 0));
            assert_ne!(m.marker_result, m.walk_result);
            assert_eq!(d.position_index_stats().mismatches, 1);
        }
    }

    /// A text of `n` one-character blocks, each from its own client (so none squash), all but
    /// the first and last then removed: one run of `n - 2` deleted blocks.
    fn tombstoned(n: u32) -> (Doc, TextRef, Vec<u8>) {
        let d = doc(1, false, OffsetKind::Utf16);
        d.set_position_shadow_rate(1);
        let t = d.get_or_insert_text("t");
        for i in 0..n {
            t.insert(
                &mut d.transact_mut_as(ClientID::new(100 + i as u64)),
                i,
                "x",
            );
        }
        // the state before the removal, for a peer that edits concurrently
        let before = d
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        t.remove_range(&mut d.transact_mut(), 1, n - 2);
        (d, t, before)
    }

    fn has_runs(t: &TextRef) -> bool {
        branch(t).runs.is_some()
    }

    fn hops(t: &TextRef) -> u64 {
        branch(t).run_hops
    }

    /// The same edits with runs off: the oracle.
    fn same_without_runs(d: &Doc, edits: impl Fn(&Doc, &TextRef)) {
        let (o, ot, _) = tombstoned(40);
        o.set_deleted_runs(false);
        edits(&o, &ot);
        assert_eq!(text(d, &d.get_or_insert_text("t")), text(&o, &ot));
        assert_eq!(
            d.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            o.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            "the same block layout as stepping"
        );
        assert_eq!(hops(&ot), 0, "runs off: no hops");
    }

    #[test]
    fn a_crossed_run_is_recorded_and_hopped() {
        let edits = |d: &Doc, t: &TextRef| {
            // the insert skips the 38 deleted blocks after index 1 (and records them)
            t.insert(&mut d.transact_mut(), 1, "a");
            // so does this one, now in one hop
            t.insert(&mut d.transact_mut(), 1, "b");
            // a removal crosses it too
            t.remove_range(&mut d.transact_mut(), 1, 1);
        };
        let (d, t, _) = tombstoned(40);
        edits(&d, &t);
        assert_eq!(text(&d, &t), "xax");
        assert!(has_runs(&t));
        assert!(hops(&t) >= 2, "hops: {}", hops(&t));
        assert_eq!(d.position_index_stats().mismatches, 0);
        same_without_runs(&d, edits);
    }

    /// A run recorded across same-client pieces of one split block, squashed back into a single
    /// block at commit. Keyed by its first block's first unit (as first written), the entry
    /// resolved to the block it was looked up from and was rejected at use (the walk stepped);
    /// keyed by its last unit, no block ends there any more and the entry is never looked up.
    /// Either way: the same place as stepping, the same bytes and state.
    #[test]
    fn a_run_squashed_into_one_block_is_never_hopped_wrongly() {
        let edits = |d: &Doc, t: &TextRef| {
            let c = ClientID::new(7);
            t.insert(&mut d.transact_mut(), 0, "x");
            t.insert(&mut d.transact_mut_as(c), 1, "abcd");
            t.insert(&mut d.transact_mut(), 5, "y");
            {
                // one transaction: "b", "c" and "d" removed one by one (three split pieces,
                // separate until commit), then an insert after "a" crosses them (a recorded run)
                let mut txn = d.transact_mut();
                t.remove_range(&mut txn, 2, 1);
                t.remove_range(&mut txn, 2, 1);
                t.remove_range(&mut txn, 2, 1);
                t.insert(&mut txn, 2, "z");
            } // commit squashes "b", "c" and "d" into one deleted block
              // looked up from that block, the run's end is the block itself: rejected, stepped
            t.insert(&mut d.transact_mut(), 2, "w");
        };
        let d = doc(1, false, OffsetKind::Utf16);
        d.set_position_shadow_rate(1);
        let t = d.get_or_insert_text("t");
        edits(&d, &t);
        assert_eq!(text(&d, &t), "xawzy");
        assert_eq!(d.position_index_stats().mismatches, 0);
        let o = doc(1, false, OffsetKind::Utf16);
        o.set_deleted_runs(false);
        let ot = o.get_or_insert_text("t");
        edits(&o, &ot);
        assert_eq!(text(&o, &ot), "xawzy");
        assert_eq!(
            d.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            o.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            "the same block layout as stepping"
        );
    }

    /// A run's first block squashed into its left neighbour (a piece of the same split block,
    /// deleted later): the merged block still ends with the run's first block's last unit, so
    /// the next crossing still goes by the run in one step instead of block by block.
    #[test]
    fn a_run_keeps_its_entry_when_its_first_block_is_squashed_into_its_left() {
        let edits = |d: &Doc, t: &TextRef| {
            let c = ClientID::new(7);
            t.insert(&mut d.transact_mut(), 0, "x");
            t.insert(&mut d.transact_mut_as(c), 1, "ab");
            for i in 0..30u32 {
                t.insert(
                    &mut d.transact_mut_as(ClientID::new(100 + i as u64)),
                    3 + i,
                    "-",
                );
            }
            t.insert(&mut d.transact_mut(), 33, "y");
            // "b" and the 30 after it removed: "ab" splits, "b" starts a run of 31
            t.remove_range(&mut d.transact_mut(), 2, 31);
            // an insert after "a" crosses the run (and records it)
            t.insert(&mut d.transact_mut(), 2, "z");
            // "a" removed: commit squashes "a" and "b" into one block, which starts at "a"
            t.remove_range(&mut d.transact_mut(), 1, 1);
        };
        let d = doc(1, false, OffsetKind::Utf16);
        let t = d.get_or_insert_text("t");
        edits(&d, &t);
        let before = hops(&t);
        // the insert's skip, from the merged block
        t.insert(&mut d.transact_mut(), 1, "w");
        assert_eq!(text(&d, &t), "xwzy");
        assert!(
            hops(&t) > before,
            "the run was crossed by its entry, not block by block"
        );
        let o = doc(1, false, OffsetKind::Utf16);
        o.set_deleted_runs(false);
        let ot = o.get_or_insert_text("t");
        edits(&o, &ot);
        ot.insert(&mut o.transact_mut(), 1, "w");
        assert_eq!(
            d.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            o.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            "the same block layout as stepping"
        );
    }

    /// A replace (a removal and an insert in one transaction) next to a run: the insert's skip
    /// crosses the block just removed and the run, recording the removed block as the run's
    /// first; at commit that block absorbs its right neighbour (the run's old first block, a
    /// piece of the same split block). The merged block starts where the recorded first block
    /// did, so the next crossing still goes by the run in one step.
    #[test]
    fn a_run_keeps_its_entry_when_its_first_block_absorbs_its_right() {
        let edits = |d: &Doc, t: &TextRef| {
            let c = ClientID::new(7);
            t.insert(&mut d.transact_mut(), 0, "x");
            t.insert(&mut d.transact_mut_as(c), 1, "ab");
            for i in 0..30u32 {
                t.insert(
                    &mut d.transact_mut_as(ClientID::new(100 + i as u64)),
                    3 + i,
                    "-",
                );
            }
            t.insert(&mut d.transact_mut(), 33, "y");
            // "b" and the 30 after it removed: "ab" splits, "b" starts a run of 31
            t.remove_range(&mut d.transact_mut(), 2, 31);
            t.insert(&mut d.transact_mut(), 2, "q");
            {
                // a replace of "a": its insert crosses "a" and the run (recorded from "a")
                let mut txn = d.transact_mut();
                t.remove_range(&mut txn, 1, 1);
                t.insert(&mut txn, 1, "z");
            } // commit: "a" absorbs "b"
        };
        let d = doc(1, false, OffsetKind::Utf16);
        let t = d.get_or_insert_text("t");
        edits(&d, &t);
        let before = hops(&t);
        t.insert(&mut d.transact_mut(), 1, "w");
        assert_eq!(text(&d, &t), "xwzqy");
        assert!(
            hops(&t) > before,
            "the run was crossed by its entry, not block by block"
        );
        let o = doc(1, false, OffsetKind::Utf16);
        o.set_deleted_runs(false);
        let ot = o.get_or_insert_text("t");
        edits(&o, &ot);
        ot.insert(&mut o.transact_mut(), 1, "w");
        assert_eq!(
            d.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            o.transact()
                .encode_state_as_update_v1(&StateVector::default()),
            "the same block layout as stepping"
        );
    }

    /// A concurrent remote insert whose origin is inside a run's first block: the block splits
    /// and the live insert lands between its two deleted pieces, which drops the text's runs
    /// (nothing is crossed by them past the live block). Walks then step, record anew and hop.
    #[test]
    fn a_remote_insert_inside_a_runs_first_block_drops_the_runs() {
        let d = doc(1, false, OffsetKind::Utf16);
        let t = d.get_or_insert_text("t");
        t.insert(&mut d.transact_mut(), 0, "x");
        t.insert(&mut d.transact_mut_as(ClientID::new(7)), 1, "abcd");
        for i in 0..30u32 {
            t.insert(
                &mut d.transact_mut_as(ClientID::new(100 + i as u64)),
                5 + i,
                "-",
            );
        }
        t.insert(&mut d.transact_mut(), 35, "y");
        let before = d
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        // "abcd" and the 30 after it removed: one run, "abcd" its first block (4 units)
        t.remove_range(&mut d.transact_mut(), 1, 34);
        t.insert(&mut d.transact_mut(), 1, "z");
        assert!(has_runs(&t));
        // a peer that hasn't seen the removal types between "b" and "c"
        let p = doc(9, false, OffsetKind::Utf16);
        let pt = p.get_or_insert_text("t");
        apply(&p, &before);
        let u = {
            let mut txn = p.transact_mut();
            pt.insert(&mut txn, 3, "R");
            txn.encode_update_v1()
        };
        apply(&d, &u);
        assert!(
            !has_runs(&t),
            "the live block between two deleted pieces dropped the runs"
        );
        assert_eq!(text(&d, &t), "xRzy");
        // no live block is skipped: an insert after "x" stops before "R"; one after "R"
        // crosses the rest of the run (and records it); the next one goes by it in one step
        t.insert(&mut d.transact_mut(), 1, "w");
        assert_eq!(text(&d, &t), "xwRzy");
        t.insert(&mut d.transact_mut(), 3, "s");
        assert_eq!(text(&d, &t), "xwRszy");
        let before = hops(&t);
        t.insert(&mut d.transact_mut(), 3, "u");
        assert_eq!(text(&d, &t), "xwRuszy");
        assert!(hops(&t) > before, "recorded anew and hopped");
        // the same as stepping
        let o = doc(1, false, OffsetKind::Utf16);
        o.set_deleted_runs(false);
        apply(
            &o,
            &d.transact()
                .encode_state_as_update_v1(&StateVector::default()),
        );
        assert_eq!(text(&o, &o.get_or_insert_text("t")), "xwRuszy");
        assert_eq!(d.position_index_stats().mismatches, 0);
    }

    #[test]
    fn a_live_block_inside_a_run_drops_the_runs() {
        let (d, t, before) = tombstoned(40);
        t.insert(&mut d.transact_mut(), 1, "a");
        assert!(has_runs(&t));
        // a peer that hasn't seen the removal types after the 10th character
        let p = doc(7, false, OffsetKind::Utf16);
        let pt = p.get_or_insert_text("t");
        apply(&p, &before);
        let u = {
            let mut txn = p.transact_mut();
            pt.insert(&mut txn, 10, "P");
            txn.encode_update_v1()
        };
        apply(&d, &u);
        assert!(
            !has_runs(&t),
            "a live block between two deleted ones drops them"
        );
        t.insert(&mut d.transact_mut(), 1, "b");
        assert_eq!(text(&d, &t), "xbPax");
        assert!(has_runs(&t), "recorded anew");
        assert_eq!(d.position_index_stats().mismatches, 0);
    }

    /// With the planted bug, the interior live block is hopped over: the shadow (1-in-1) sees
    /// the marker path disagree with stepping, fires and uses the walk's result.
    #[cfg(feature = "planted-position-bugs")]
    #[test]
    fn the_shadow_fires_on_a_kept_run() {
        let (d, t, before) = tombstoned(40);
        d.plant_position_bug(Some(PlantedPositionBug::KeepRunsOnInterior));
        let seen = audit(&d);
        t.insert(&mut d.transact_mut(), 1, "a");
        let p = doc(7, false, OffsetKind::Utf16);
        let pt = p.get_or_insert_text("t");
        apply(&p, &before);
        let u = {
            let mut txn = p.transact_mut();
            pt.insert(&mut txn, 10, "P");
            txn.encode_update_v1()
        };
        apply(&d, &u);
        assert!(has_runs(&t), "planted: the runs stay");
        // the insert's skip would hop over "P"; the check steps, fires and uses its end
        t.insert(&mut d.transact_mut(), 1, "b");
        assert_eq!(text(&d, &t), "xbPax", "the stepped end was used");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "the hook fired once");
        assert_eq!(
            seen[0].marker.1.client,
            ClientID::new(101),
            "the run's first block"
        );
        assert_ne!(seen[0].marker_result, seen[0].walk_result);
        assert!(!has_runs(&t));
        assert_eq!(d.position_index_stats().mismatches, 1);
    }

    #[test]
    fn markers_replace_the_least_recently_used() {
        let mut ms = Markers::default();
        let id = |c: u32| ID::new(ClientID::new(1), c);
        for i in 0..MAX_MARKERS as u32 {
            ms.put(i * 10, id(i), None);
        }
        assert_eq!(ms.len(), MAX_MARKERS);
        ms.touch(0); // marker 0 is now the most recent; 1 the least
        ms.put(999, id(999), None);
        assert_eq!(ms.len(), MAX_MARKERS);
        assert!(ms.list().iter().any(|m| m.id == id(0)));
        assert!(!ms.list().iter().any(|m| m.id == id(1)));
        // reuse moves the marker; a second marker on one unit is merged
        let i = ms.nearest(20).unwrap();
        ms.put(21, id(2000), Some(i));
        assert_eq!(ms.get(i).index, 21);
        let n = ms.len();
        let j = ms.nearest(30).unwrap();
        ms.put(21, id(2000), Some(j));
        assert_eq!(ms.len(), n - 1);
    }

    #[test]
    fn shifts_follow_the_units() {
        let mut ms = Markers::default();
        let id = |c: u32| ID::new(ClientID::new(1), c);
        for (i, at) in [0u32, 5, 10, 15].iter().enumerate() {
            ms.put(*at, id(i as u32), None);
        }
        ms.shift_insert(5, 3); // before the unit at 5
        let at = |ms: &Markers| {
            let mut v: Vec<u32> = ms.list().iter().map(|m| m.index).collect();
            v.sort();
            v
        };
        assert_eq!(at(&ms), vec![0, 8, 13, 18]);
        ms.shift_remove(8, 5, None); // units 8..13: the marker at 8 goes, 13 → 8
        assert_eq!(at(&ms), vec![0, 8, 13]);
    }
}
