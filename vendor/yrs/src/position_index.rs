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
//! # Shadow check
//!
//! On a deterministic sample of marker lookups (every `shadow_every`-th, default 1 in 1000;
//! [crate::Doc::set_position_shadow_rate]) the full walk runs too. If the two differ, the walk's
//! result is used, the branch's markers are dropped and the audit hook
//! ([crate::Doc::set_position_audit_hook]) is called with a [PositionMismatch] (IDs and indices
//! only, never content). A shadowed lookup costs what a lookup cost before this index (one full
//! walk) on top of the marker path's own cost, so at the default rate the shadow adds about a
//! thousandth of the old cost of lookups.

use crate::block::{Item, ItemContent, ItemPtr, ID};
use crate::branch::{BranchID, BranchPtr};
use crate::types::TypePtr;
use crate::OffsetKind;

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
    pub shadow_every: u32,
    pub hook: Option<AuditHook>,
    pub stats: PositionIndexStats,
    pub planted: Option<PlantedPositionBug>,
}

impl Default for PositionIndex {
    fn default() -> Self {
        PositionIndex {
            enabled: true,
            shadow_every: DEFAULT_SHADOW_EVERY,
            hook: None,
            stats: PositionIndexStats::default(),
            planted: None,
        }
    }
}

impl PositionIndex {
    /// Whether this marker lookup is one the shadow check samples.
    pub fn sample(&mut self) -> bool {
        if self.shadow_every == 0 {
            return false;
        }
        self.stats.marker_hits % self.shadow_every as u64 == 0
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
}

/// The walk of `find_position`, without its split and for a text without formatting: from
/// `right` (whose content index is `start`), find `remaining` units on. Read-only, and all its
/// arithmetic is checked, so a wrong start (a stale marker) gives [Stop::Invalid] or a wrong
/// place, never an overflow or a split outside a block.
pub(crate) fn scan(
    mut left: Option<ItemPtr>,
    mut right: Option<ItemPtr>,
    mut remaining: u32,
    start: u32,
    encoding: OffsetKind,
) -> Result<Located, Stop> {
    let mut index: u32 = 0;
    let mut cursor = start;
    let mut left_start = start;
    while let Some(r) = right {
        if remaining == 0 {
            break;
        }
        if !r.is_deleted() {
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
) -> Result<Located, Stop> {
    let from = |block: ItemPtr, at: u32| -> Result<Located, Stop> {
        let mut l = scan(block.left, Some(block), index - at, at, encoding)?;
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
        if !b.is_deleted() {
            if let ItemContent::Format(_, _) = &b.content {
                return Err(Stop::Format);
            }
            let len = b.content_len(encoding);
            cursor = cursor.checked_sub(len).ok_or(Stop::Invalid)?;
            if cursor < index {
                return from(b, cursor);
            }
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
