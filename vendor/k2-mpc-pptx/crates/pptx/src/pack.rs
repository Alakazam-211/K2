//! [`PptxPack`]: what a deck adds to the kit's tier 0: the structure index, and the merge of
//! an app's save by shape ([`crate::merge`]). A clash is a whole-file conflict until the
//! kit's durable held conflicts land ([`HOLD_CLASHES`]).

use std::io::Read;

use k2_mpc_base::errors::ErrorEntry;
use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, IndexSpec};
use k2_mpc_base::session::{BlobMerge, BlobPack, BlobVerb, HoldReach, SaveBase, SaveMerge};
use k2_mpc_base::view_index::{ViewIndex, ViewSchema, ViewSource};

use crate::index::{index_pptx, FORMAT, OLDER_FORMAT, PACK, VERBS};
use crate::merge::{merge3, Conflicts, Outcome};
use crate::mutant;
use crate::Limits;

/// The tier 0.5 pack for `.pptx`: one per deck doc (it holds that deck's conflicts).
#[derive(Debug, Default)]
pub struct PptxPack {
    pub limits: Limits,
    /// The agent's side of each clash a merge kept the person's side of. Empty while
    /// [`HOLD_CLASHES`] is off: a clash is then a whole-file conflict.
    pub conflicts: Conflicts,
}

/// What the caller of [`PptxPack::merge3_since`] knows of the file between the last outside
/// save and this one: whether the save can be taken as made on `base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Since {
    /// The kit wrote nothing to the file since the last outside save: `base` is what that
    /// save left, and (one outside writer at a time) the new save was made on it or later.
    NoKitWrite,
    /// The kit may have written the file since the last outside save, so the app's buffer may
    /// be older than `base`. What the kit's [`BlobPack::merge3_bytes`] call amounts to today:
    /// it hands over no history.
    KitMayHaveWritten,
}

impl PptxPack {
    /// The merge of an app's save `theirs` with the doc's head `ours`, from `base` (what the
    /// file last held), told what the caller knows of the file since the last outside save.
    ///
    /// **Never a silent loss: when the pack can't know, keep both.**
    /// - `base == ours` says nothing about what the app had open (its save may be made on an
    ///   older version, and a merge would read that as undoing the agent's write): a
    ///   conflict, as the docx pack does.
    /// - [`Since::KitMayHaveWritten`]: the kit may have written an agent's change to the file
    ///   after the app read it, and a merge from `base` would read the save as the person
    ///   undoing that change and drop it. The save's base isn't known exactly, so a save that
    ///   both sides changed is a conflict. The kit keeps both: the save stays current; the
    ///   head, with every agent change, is a version and a conflict copy.
    /// - A clash (both sides changed one shape, or one deleted what the other changed) is a
    ///   whole-file conflict too, while [`HOLD_CLASHES`] is off.
    ///
    /// The kit's planned outside-save bridge replaces both guards with its base test: it hands
    /// over the version the app's save was made on, and plugs in here.
    pub fn merge3_since(&self, base: &[u8], ours: &[u8], theirs: &[u8], since: Since) -> BlobMerge {
        if base == ours && ours != theirs && !mutant::active("pack_no_stale_guard") {
            return BlobMerge::Conflict;
        }
        let both_changed = base != ours && base != theirs && ours != theirs;
        if since == Since::KitMayHaveWritten
            && both_changed
            && !mutant::active("pack_no_kit_write_guard")
        {
            return BlobMerge::Conflict;
        }
        self.merge3_exact(base, ours, theirs)
    }

    /// The merge by shape with neither stale guard, for three versions the caller knows
    /// exactly: the kit's revert ([`SaveBase::Revert`]: `base` the version taken away, `ours`
    /// head, `theirs` the version before it, all three the doc's own versions). No app's buffer
    /// is in play, so `base == ours` (the revert of head's own version) is that version taken
    /// away, and a version both sides changed merges by shape. A clash is a whole-file conflict
    /// while [`HOLD_CLASHES`] is off, as in [`PptxPack::merge3_since`].
    pub fn merge3_exact(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        match merge3(base, ours, theirs, &self.limits) {
            Outcome::Merged(m) if m.conflicts.is_empty() => BlobMerge::Merged(m.bytes),
            Outcome::Merged(m) if HOLD_CLASHES || mutant::active("pack_holds_clashes") => {
                self.conflicts.hold(m.conflicts);
                BlobMerge::Merged(m.bytes)
            }
            Outcome::Merged(_) | Outcome::Conflict(_) => BlobMerge::Conflict,
            Outcome::Unsupported(_) => BlobMerge::Unsupported,
        }
    }
}

/// **Off until the kit's durable held conflicts land: until then a clash is a whole-file
/// conflict, which a restart can't lose.** On, a merge that holds the
/// agent's side of a clash returns the merged file (the person's side in it) and keeps the
/// agent's in [`PptxPack::conflicts`], for `conflicts list` and `conflict take`. That record
/// lives in this pack's memory only, so a restart would lose it. Off, a clash is a whole-file
/// conflict: the kit keeps both, the save current and the head (with the agent's side) a
/// version and a conflict copy. The held path ([`Conflicts`], `take`) is kept as it was, so
/// moving it onto the kit's tables and turning this back on restores it.
pub const HOLD_CLASHES: bool = false;

impl BlobPack for PptxPack {
    fn can_merge(&self) -> bool {
        true
    }

    /// The kit's call. `base` is what the file last held, and the kit doesn't say whether it
    /// wrote the file since the last outside save, so this is [`Since::KitMayHaveWritten`]
    /// ([`PptxPack::merge3_since`]): a save both sides changed is a conflict, and the kit
    /// keeps both. The kit's outside-save bridge plugs in here.
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        self.merge3_since(base, ours, theirs, Since::KitMayHaveWritten)
    }

    /// The kit's revert, undo and redo ([`SaveBase::Revert`]) hand over the doc's own three
    /// versions, after the kit's own check that nothing the reverted version changed changed
    /// since: [`PptxPack::merge3_exact`], with no stale guard. Any other known base (a save the
    /// app opened or stamped) is [`BlobPack::merge3_bytes`]'s, as before.
    fn merge3_known(&self, m: &SaveMerge<'_>) -> BlobMerge {
        if m.base_is == SaveBase::Revert {
            return self.merge3_exact(m.base, m.ours, m.theirs);
        }
        self.merge3_bytes(m.base, m.ours, m.theirs)
    }

    /// Format 2 carries the view items, and reads format 1's rows (the same nodes) until a
    /// version is indexed again ([`crate::index::FORMAT`]); slides and groups carry their own
    /// record digests, declared, so the kit stores the rows under its offset format.
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(
            IndexSpec::new(PACK, FORMAT, &VERBS)
                .with_older_format(OLDER_FORMAT)
                .with_own_records(),
        )
    }

    fn index(
        &self,
        src: &mut dyn Read,
        size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        index_pptx(src, size, limits, &self.limits)
    }

    /// The deck's view schema ([`crate::live::view_schema::VIEW`]), with its mapping of the
    /// structure index: the kit's view index on the blob tier.
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(crate::live::view_schema::VIEW)
    }

    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        crate::live::view_schema::derive(source, index);
    }

    /// `slide list` and the seven edits ([`crate::blob_verbs`]), each under the caller's own
    /// lease until this pack merges a save against the version the app opened.
    fn verbs(&self) -> &'static [BlobVerb] {
        crate::blob_verbs::VERB_SET
    }

    /// A slide's hold covers its shapes and notes; a group's, its shapes; a master's, its
    /// layouts ([`crate::blob_verbs::hold_reach`]).
    fn hold_reach(&self, kind: &str) -> HoldReach {
        crate::blob_verbs::hold_reach(kind)
    }

    fn errors(&self) -> &'static [ErrorEntry] {
        crate::blob_verbs::PPTX_ERRORS
    }
}
