//! The person actions every pack maps to verbs (the parity rows), with the tiers each
//! applies to.

use std::fmt;

use super::registry::Tier;

const TEXT: &[Tier] = &[Tier::Text];
const BLOB: &[Tier] = &[Tier::Blob];
const GRID: &[Tier] = &[Tier::Grid];
/// Text and grid docs (CSV contract §3.5: "as for text", or mapped onto the grid).
const TG: &[Tier] = &[Tier::Text, Tier::Grid];
/// Every tier.
const ALL3: &[Tier] = &[Tier::Text, Tier::Blob, Tier::Grid];

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
        /// mapped. That is the parity rule (K-PAR-3): a new action without a mapping must
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

            /// The row number (`13` for P13).
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
    /// P1: join; appear in presence.
    Open = 1, ALL3, "Open the file (join; appear in presence)";
    /// P2: leave.
    Close = 2, ALL3, "Close it (leave)";
    /// P3.
    ReadAll = 3, ALL3, "Read the whole doc";
    /// P4: the viewing highlight.
    ViewRange = 4, ALL3, "Look at a range (viewing highlight)";
    /// P5.
    Find = 5, TG, "Find";
    /// P6.
    Who = 6, ALL3, "See who's here";
    /// P7.
    Follow = 7, TG, "Follow someone";
    /// P8 (and, later, declaring an intent range: K-PRES-12).
    Caret = 8, TG, "Place the caret / select; say what I'm working on";
    /// P9.
    Type = 9, TG, "Type at a position";
    /// P10.
    TypeOver = 10, TG, "Select and type over";
    /// P11.
    Delete = 11, TG, "Delete a range";
    /// P12.
    FindReplace = 12, TG, "Find and replace";
    /// P13.
    Paste = 13, TG, "Paste a block / apply a change";
    /// P14: `write --base V` (three-way merge; conflict exit 1, expired base exit 8).
    Rewrite = 14, ALL3, "Rewrite from what I last saw";
    /// P15.
    Format = 15, TEXT, "Format (bold, heading, list)";
    /// P16: all or nothing; `undo --drop` skips a blocked step.
    Undo = 16, TG, "Undo / redo my own edits";
    /// P17.
    Saved = 17, ALL3, "See that it saved";
    /// P18.
    Watch = 18, ALL3, "Watch others edit live";
    /// P19.
    History = 19, ALL3, "History, diff, show a version";
    /// P20.
    Restore = 20, ALL3, "Restore / name a version";
    /// P21.
    Rename = 21, ALL3, "Rename / move";
    /// P22.
    Blame = 22, ALL3, "Blame";
    /// P23: comment, reply, resolve; suggest; accept / reject per hunk.
    Annotate = 23, TG, "Comment, reply, resolve; suggest; accept / reject";
    /// P24.
    Export = 24, ALL3, "Export";
    /// P25.
    WhyOff = 25, ALL3, "See why live editing is off";
    /// P26: Ctrl+Z when others edited part of my change (`undo --partial`).
    UndoPartial = 26, TG, "Undo the regions of my change nobody touched; report the rest";
    /// P27.
    Mention = 27, TG, "Mention someone";
    /// P28: owner only.
    Purge = 28, ALL3, "Purge a pasted secret or personal data";
    /// P29.
    Approve = 29, ALL3, "Approve or decline an assistant's edit made on my behalf";
    /// P30.
    Pending = 30, ALL3, "See which edits are waiting for my approval";
    /// P31: inside the revert window; older changes: `restore --version`.
    Revert = 31, TG, "Reverse one specific change";
    /// P32 (tier 0): take the lease, or extend mine (`lease`, `lease --renew`).
    Lease = 32, BLOB, "Take the lease (lock the file while I change it), or extend it";
    /// P33 (tier 0).
    ReleaseLease = 33, BLOB, "Release my lease";
    /// P34 (tier 0): owner only.
    BreakLease = 34, BLOB, "Break someone else's lease";
    // the grid tier (CSV contract §3.5, K-CSV-3, K-CSV-7): a structured doc's own actions
    /// P35: `row insert`.
    RowInsert = 35, GRID, "Insert a row";
    /// P36: `row move`.
    RowMove = 36, GRID, "Drag a row to another place";
    /// P37: `row delete` (hides the row; its cells stay, hidden).
    RowDelete = 37, GRID, "Delete rows";
    /// P38: `col insert`.
    ColInsert = 38, GRID, "Insert a column";
    /// P39: `col move`.
    ColMove = 39, GRID, "Drag a column";
    /// P40: `col delete`.
    ColDelete = 40, GRID, "Delete a column";
    /// P41: `col rename`.
    ColRename = 41, GRID, "Rename a column (edit the header)";
    /// P42: `sort --by` (one change: the minimal moves).
    Sort = 42, GRID, "Sort by a column";
    /// P43: `row restore`, `col restore` (also `undo`, `revert`).
    RestoreElement = 43, GRID, "Bring back a deleted row or column";
    /// P44: `rows drop-deleted`, owner only (K-CSV-7: frees the size limit; not erasure,
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
        // the grid tier (CSV contract §3.5): every text row but P15 (CSV has no formatting),
        // tier 0's own rows left out (leases), then P35-P44
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
