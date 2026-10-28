//! The person actions every text pack maps to verbs (the parity rows).

use std::fmt;

macro_rules! person_actions {
    ($( $(#[$m:meta])* $v:ident = $n:literal, $desc:literal; )*) => {
        /// One thing a person can do in an editor: one parity row (`P1`...). Every pack's
        /// parity list maps each of these to verbs, or marks it N/A or deferred with a
        /// reason (`docs/PARITY.md` in the pack).
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
        }
    };
}

person_actions! {
    /// P1: join; appear in presence.
    Open = 1, "Open the file (join; appear in presence)";
    /// P2: leave.
    Close = 2, "Close it (leave)";
    /// P3.
    ReadAll = 3, "Read the whole doc";
    /// P4: the viewing highlight.
    ViewRange = 4, "Look at a range (viewing highlight)";
    /// P5.
    Find = 5, "Find";
    /// P6.
    Who = 6, "See who's here";
    /// P7.
    Follow = 7, "Follow someone";
    /// P8 (and, later, declaring an intent range: K-PRES-12).
    Caret = 8, "Place the caret / select; say what I'm working on";
    /// P9.
    Type = 9, "Type at a position";
    /// P10.
    TypeOver = 10, "Select and type over";
    /// P11.
    Delete = 11, "Delete a range";
    /// P12.
    FindReplace = 12, "Find and replace";
    /// P13.
    Paste = 13, "Paste a block / apply a change";
    /// P14: `write --base V` (three-way merge; conflict exit 1, expired base exit 8).
    Rewrite = 14, "Rewrite from what I last saw";
    /// P15.
    Format = 15, "Format (bold, heading, list)";
    /// P16: all or nothing; `undo --drop` skips a blocked step.
    Undo = 16, "Undo / redo my own edits";
    /// P17.
    Saved = 17, "See that it saved";
    /// P18.
    Watch = 18, "Watch others edit live";
    /// P19.
    History = 19, "History, diff, show a version";
    /// P20.
    Restore = 20, "Restore / name a version";
    /// P21.
    Rename = 21, "Rename / move";
    /// P22.
    Blame = 22, "Blame";
    /// P23: comment, reply, resolve; suggest; accept / reject per hunk.
    Annotate = 23, "Comment, reply, resolve; suggest; accept / reject";
    /// P24.
    Export = 24, "Export";
    /// P25.
    WhyOff = 25, "See why live editing is off";
    /// P26: Ctrl+Z when others edited part of my change (`undo --partial`).
    UndoPartial = 26, "Undo the regions of my change nobody touched; report the rest";
    /// P27.
    Mention = 27, "Mention someone";
    /// P28: owner only.
    Purge = 28, "Purge a pasted secret or personal data";
    /// P29.
    Approve = 29, "Approve or decline an assistant's edit made on my behalf";
    /// P30.
    Pending = 30, "See which edits are waiting for my approval";
    /// P31: inside the revert window; older changes: `restore --version`.
    Revert = 31, "Reverse one specific change";
}

impl PersonAction {
    /// The action with row number `n`.
    pub fn from_number(n: u8) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.number() == n)
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
        assert_eq!(PersonAction::ALL.len(), 31);
        assert_eq!(PersonAction::Paste.to_string(), "P13");
        assert_eq!(PersonAction::from_number(0), None);
    }
}
