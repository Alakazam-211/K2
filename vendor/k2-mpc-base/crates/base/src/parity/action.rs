//! The person actions every pack maps to verbs (the parity rows), with the tiers each
//! applies to.

use std::fmt;

use super::registry::Tier;

const TEXT: &[Tier] = &[Tier::Text];
const BOTH: &[Tier] = &[Tier::Text, Tier::Blob];
const BLOB: &[Tier] = &[Tier::Blob];

macro_rules! person_actions {
    ($( $(#[$m:meta])* $v:ident = $n:literal, $t:ident, $desc:literal; )*) => {
        /// One thing a person can do in an editor: one parity row (`P1`...). Every pack's
        /// parity list maps each of these to verbs, or marks it N/A or deferred with a
        /// reason (`docs/PARITY.md` in the pack). Each applies to text docs, blob docs
        /// (tier 0), or both ([`PersonAction::tiers`]); a pack's list needs a row for every
        /// action of its tier.
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
    Open = 1, BOTH, "Open the file (join; appear in presence)";
    /// P2: leave.
    Close = 2, BOTH, "Close it (leave)";
    /// P3.
    ReadAll = 3, BOTH, "Read the whole doc";
    /// P4: the viewing highlight.
    ViewRange = 4, BOTH, "Look at a range (viewing highlight)";
    /// P5.
    Find = 5, TEXT, "Find";
    /// P6.
    Who = 6, BOTH, "See who's here";
    /// P7.
    Follow = 7, TEXT, "Follow someone";
    /// P8 (and, later, declaring an intent range: K-PRES-12).
    Caret = 8, TEXT, "Place the caret / select; say what I'm working on";
    /// P9.
    Type = 9, TEXT, "Type at a position";
    /// P10.
    TypeOver = 10, TEXT, "Select and type over";
    /// P11.
    Delete = 11, TEXT, "Delete a range";
    /// P12.
    FindReplace = 12, TEXT, "Find and replace";
    /// P13.
    Paste = 13, TEXT, "Paste a block / apply a change";
    /// P14: `write --base V` (three-way merge; conflict exit 1, expired base exit 8).
    Rewrite = 14, BOTH, "Rewrite from what I last saw";
    /// P15.
    Format = 15, TEXT, "Format (bold, heading, list)";
    /// P16: all or nothing; `undo --drop` skips a blocked step.
    Undo = 16, TEXT, "Undo / redo my own edits";
    /// P17.
    Saved = 17, BOTH, "See that it saved";
    /// P18.
    Watch = 18, BOTH, "Watch others edit live";
    /// P19.
    History = 19, BOTH, "History, diff, show a version";
    /// P20.
    Restore = 20, BOTH, "Restore / name a version";
    /// P21.
    Rename = 21, BOTH, "Rename / move";
    /// P22.
    Blame = 22, BOTH, "Blame";
    /// P23: comment, reply, resolve; suggest; accept / reject per hunk.
    Annotate = 23, TEXT, "Comment, reply, resolve; suggest; accept / reject";
    /// P24.
    Export = 24, BOTH, "Export";
    /// P25.
    WhyOff = 25, BOTH, "See why live editing is off";
    /// P26: Ctrl+Z when others edited part of my change (`undo --partial`).
    UndoPartial = 26, TEXT, "Undo the regions of my change nobody touched; report the rest";
    /// P27.
    Mention = 27, TEXT, "Mention someone";
    /// P28: owner only.
    Purge = 28, BOTH, "Purge a pasted secret or personal data";
    /// P29.
    Approve = 29, BOTH, "Approve or decline an assistant's edit made on my behalf";
    /// P30.
    Pending = 30, BOTH, "See which edits are waiting for my approval";
    /// P31: inside the revert window; older changes: `restore --version`.
    Revert = 31, TEXT, "Reverse one specific change";
    /// P32 (tier 0): take the lease, or extend mine (`lease`, `lease --renew`).
    Lease = 32, BLOB, "Take the lease (lock the file while I change it), or extend it";
    /// P33 (tier 0).
    ReleaseLease = 33, BLOB, "Release my lease";
    /// P34 (tier 0): owner only.
    BreakLease = 34, BLOB, "Break someone else's lease";
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
        assert_eq!(PersonAction::ALL.len(), 34);
        assert!(PersonAction::ALL.iter().all(|a| !a.tiers().is_empty()));
        assert_eq!(PersonAction::for_tier(Tier::Text).count(), 31);
        assert_eq!(
            PersonAction::for_tier(Tier::Blob)
                .map(|a| a.number())
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 6, 14, 17, 18, 19, 20, 21, 22, 24, 25, 28, 29, 30, 32, 33, 34]
        );
        assert_eq!(PersonAction::Paste.to_string(), "P13");
        assert_eq!(PersonAction::from_number(0), None);
    }
}
