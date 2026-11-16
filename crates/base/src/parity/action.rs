//! The person actions every pack maps to verbs (the parity rows), with the tiers each
//! applies to.

use std::fmt;

use super::registry::Tier;

const TEXT: &[Tier] = &[Tier::Text];
const BLOB: &[Tier] = &[Tier::Blob];
const GRID: &[Tier] = &[Tier::Grid];
/// Text and grid docs (CSV contract §3.5: "as for text", or mapped onto the grid).
const TG: &[Tier] = &[Tier::Text, Tier::Grid];
/// Text, blob and grid docs.
const ALL3: &[Tier] = &[Tier::Text, Tier::Blob, Tier::Grid];
/// Every tier: the doc's presence and records (who's here, versions, approvals), whatever its
/// content; a keyed doc's too.
const EVERY: &[Tier] = Tier::ALL;

macro_rules! person_actions {
    ($( $(#[$m:meta])* $v:ident = $n:literal, $t:ident, $desc:literal; )*) => {
        /// One thing a person can do in an editor: one parity row (`P1`...). Every pack's
        /// parity list maps each of these to verbs, or marks it N/A or deferred with a
        /// reason (`docs/PARITY.md` in the pack). Each applies to some of text docs, blob
        /// docs (tier 0) and grid docs ([`PersonAction::tiers`]); a pack's list needs a row
        /// for every action of its tier.
        ///
        /// **Deliberately exhaustive** (not `#[non_exhaustive]`): a pack maps actions with a
        /// `match`, so adding a row here is a compile error in every pack until it is
        /// mapped. That is the parity rule: a new action without a mapping must
        /// not compile.
        ///
        /// ```compile_fail
        /// use k2_mpc_base::parity::PersonAction;
        /// // Only one row mapped: every other action is unmapped, so this doesn't compile.
        /// fn verbs(a: PersonAction) -> &'static [&'static str] {
        ///     match a {
        ///         PersonAction::Open => &["open"],
        ///     }
        /// }
        /// ```
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum PersonAction {
            $( $(#[$m])* $v, )*
        }

        impl PersonAction {
            /// Every action, in row order.
            pub const ALL: &'static [PersonAction] = &[ $( PersonAction::$v, )* ];

            /// The row number (`13` for the row printed `P13`).
            pub const fn number(self) -> u8 {
                match self { $( PersonAction::$v => $n, )* }
            }

            /// The person action, in words.
            pub const fn describe(self) -> &'static str {
                match self { $( PersonAction::$v => $desc, )* }
            }

            /// The kinds of doc a person can do it on.
            pub const fn tiers(self) -> &'static [Tier] {
                match self { $( PersonAction::$v => $t, )* }
            }
        }
    };
}

person_actions! {
    /// Join; appear in presence.
    Open = 1, EVERY, "Open the file (join; appear in presence)";
    /// Leave.
    Close = 2, EVERY, "Close it (leave)";
    /// Read the whole doc.
    ReadAll = 3, ALL3, "Read the whole doc";
    /// The viewing highlight.
    ViewRange = 4, ALL3, "Look at a range (viewing highlight)";
    /// Find.
    Find = 5, TG, "Find";
    /// See who's here.
    Who = 6, EVERY, "See who's here";
    /// Follow someone.
    Follow = 7, TG, "Follow someone";
    /// Place the caret or select (and, later, declaring an intent range).
    Caret = 8, TG, "Place the caret / select; say what I'm working on";
    /// Type at a position.
    Type = 9, TG, "Type at a position";
    /// Select and type over.
    TypeOver = 10, TG, "Select and type over";
    /// Delete a range.
    Delete = 11, TG, "Delete a range";
    /// Find and replace.
    FindReplace = 12, TG, "Find and replace";
    /// Paste a block or apply a change.
    Paste = 13, TG, "Paste a block / apply a change";
    /// `write --base V` (three-way merge; conflict exit 1, expired base exit 8).
    Rewrite = 14, ALL3, "Rewrite from what I last saw";
    /// Format (bold, heading, list).
    Format = 15, TEXT, "Format (bold, heading, list)";
    /// Undo / redo my own edits: all or nothing; `undo --drop` skips a blocked step.
    Undo = 16, TG, "Undo / redo my own edits";
    /// See that it saved.
    Saved = 17, EVERY, "See that it saved";
    /// Watch others edit live.
    Watch = 18, ALL3, "Watch others edit live";
    /// History, diff, show a version.
    History = 19, EVERY, "History, diff, show a version";
    /// Restore or name a version.
    Restore = 20, EVERY, "Restore / name a version";
    /// Rename or move.
    Rename = 21, ALL3, "Rename / move";
    /// Blame.
    Blame = 22, ALL3, "Blame";
    /// Comment, reply, resolve; suggest; accept / reject per hunk.
    Annotate = 23, TG, "Comment, reply, resolve; suggest; accept / reject";
    /// Export.
    Export = 24, ALL3, "Export";
    /// See why live editing is off.
    WhyOff = 25, EVERY, "See why live editing is off";
    /// Ctrl+Z when others edited part of my change (`undo --partial`).
    UndoPartial = 26, TG, "Undo the regions of my change nobody touched; report the rest";
    /// Mention someone.
    Mention = 27, TG, "Mention someone";
    /// Purge a pasted secret or personal data: owner only.
    Purge = 28, ALL3, "Purge a pasted secret or personal data";
    /// Approve or decline an assistant's edit made on my behalf.
    Approve = 29, EVERY, "Approve or decline an assistant's edit made on my behalf";
    /// See which edits are waiting for my approval.
    Pending = 30, EVERY, "See which edits are waiting for my approval";
    /// Reverse one change, inside the revert window; older changes: `restore --version`.
    Revert = 31, TG, "Reverse one specific change";
    /// Tier 0: take the lease, or extend mine (`lease`, `lease --renew`).
    Lease = 32, BLOB, "Take the lease (lock the file while I change it), or extend it";
    /// Tier 0: release my lease.
    ReleaseLease = 33, BLOB, "Release my lease";
    /// Tier 0: break someone else's lease; owner only.
    BreakLease = 34, BLOB, "Break someone else's lease";
    // the grid tier (CSV contract §3.5): a structured doc's own actions
    /// `row insert`.
    RowInsert = 35, GRID, "Insert a row";
    /// `row move`.
    RowMove = 36, GRID, "Drag a row to another place";
    /// `row delete` (hides the row; its cells stay, hidden).
    RowDelete = 37, GRID, "Delete rows";
    /// `col insert`.
    ColInsert = 38, GRID, "Insert a column";
    /// `col move`.
    ColMove = 39, GRID, "Drag a column";
    /// `col delete`.
    ColDelete = 40, GRID, "Delete a column";
    /// `col rename`.
    ColRename = 41, GRID, "Rename a column (edit the header)";
    /// `sort --by` (one change: the minimal moves).
    Sort = 42, GRID, "Sort by a column";
    /// `row restore`, `col restore` (also `undo`, `revert`).
    RestoreElement = 43, GRID, "Bring back a deleted row or column";
    /// `rows drop-deleted`, owner only (frees the size limit; not erasure,
    /// history keeps the rows; anyone else gets exit 3).
    DropDeleted = 44, GRID, "Remove deleted rows to free space (owner only)";
}

impl PersonAction {
    /// The action with row number `n`.
    pub fn from_number(n: u8) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.number() == n)
    }

    /// Whether a person can do it on `tier`'s docs.
    pub fn in_tier(self, tier: Tier) -> bool {
        self.tiers().contains(&tier)
    }

    /// The actions of `tier`, in row order: the rows a pack of that tier lists.
    pub fn for_tier(tier: Tier) -> impl Iterator<Item = PersonAction> {
        Self::ALL.iter().copied().filter(move |a| a.in_tier(tier))
    }
}

impl fmt::Display for PersonAction {
    /// `P13`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P{}", self.number())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_numbered_in_order_without_gaps() {
        for (i, a) in PersonAction::ALL.iter().enumerate() {
            assert_eq!(usize::from(a.number()), i + 1, "{a:?}");
            assert_eq!(PersonAction::from_number(a.number()), Some(*a));
            assert!(!a.describe().is_empty());
        }
        assert_eq!(PersonAction::ALL.len(), 44);
        assert!(PersonAction::ALL.iter().all(|a| !a.tiers().is_empty()));
        assert_eq!(PersonAction::for_tier(Tier::Text).count(), 31);
        assert!(
            PersonAction::for_tier(Tier::Text).all(|a| a.number() <= 31),
            "the grid rows are not text rows"
        );
        assert_eq!(
            PersonAction::for_tier(Tier::Blob)
                .map(|a| a.number())
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 6, 14, 17, 18, 19, 20, 21, 22, 24, 25, 28, 29, 30, 32, 33, 34]
        );
        // the grid tier (CSV contract §3.5): every text row but formatting (CSV has none),
        // tier 0's own rows left out (leases), then the grid rows' own actions
        let grid: Vec<u8> = PersonAction::for_tier(Tier::Grid)
            .map(|a| a.number())
            .collect();
        let mut want: Vec<u8> = (1..=31).filter(|n| *n != 15).collect();
        want.extend(35..=44);
        assert_eq!(grid, want);
        assert_eq!(
            PersonAction::from_number(44),
            Some(PersonAction::DropDeleted)
        );
        assert_eq!(
            PersonAction::DropDeleted.describe(),
            "Remove deleted rows to free space (owner only)"
        );
        assert_eq!(PersonAction::Paste.to_string(), "P13");
        assert_eq!(PersonAction::from_number(0), None);
    }
}
