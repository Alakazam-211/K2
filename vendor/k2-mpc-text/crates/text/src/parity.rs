//! The text pack's parity list as data (`docs/PARITY.md` is the same list for people).
//!
//! One exhaustive `match`: when the kit adds a person action, this stops compiling until
//! the action is mapped. `tests/agent_parity.rs` runs it through the kit's harness.

use k2_mpc_base::parity::{ParityRow, PersonAction, Tier};

/// The row for `action`: its verbs, or why it has none yet / at all.
pub fn parity_row(action: PersonAction) -> ParityRow {
    use PersonAction as P;
    let a = action;
    match action {
        P::Open => ParityRow::live(a, &["open"]),
        P::Close => ParityRow::live(a, &["leave"]),
        P::ReadAll => ParityRow::live(a, &["cat", "wc"]),
        P::ViewRange => ParityRow::live(a, &["head", "tail", "sed", "view"]),
        P::Find => ParityRow::live(a, &["grep"]),
        P::Who => ParityRow::live(a, &["who", "activity"]),
        P::Follow => ParityRow::live(a, &["follow"]),
        // saying what I'm working on (K-PRES-12) is `claim` (base v0.7.0)
        P::Caret => ParityRow::live(a, &["cursor", "claim", "claim --renew", "claim --release"]),
        P::Type => ParityRow::live(a, &["insert"]),
        P::TypeOver => ParityRow::live(a, &["replace"]),
        P::Delete => ParityRow::live(a, &["delete", "sed -i"]),
        P::FindReplace => ParityRow::live(a, &["sed -i", "replace"]),
        // no pack verb: the kit's patch tools paste into any text
        P::Paste => ParityRow::live(a, &["patch", "apply", "append"]),
        P::Rewrite => ParityRow::live(a, &["write"]),
        // formatting a text file is text: indentation, comment toggles, wrapping
        // (a formatter's whole-file output is P14's `write --base V`)
        P::Format => ParityRow::live(a, &["insert", "replace", "sed -i"]),
        P::Undo => ParityRow::live(a, &["undo", "redo", "undo --drop"]),
        P::Saved => ParityRow::live(a, &["status", "flush"]),
        P::Watch => ParityRow::live(a, &["watch", "tail -f"]),
        P::History => ParityRow::live(a, &["log", "diff --version", "show --version", "diff"]),
        P::Restore => ParityRow::live(a, &["restore", "version --name"]),
        P::Rename => ParityRow::live(a, &["mv"]),
        // the tab's blame gutter and `blame -L a,b` both call `Session::blame`
        P::Blame => ParityRow::live(a, &["blame"]),
        P::Annotate => ParityRow::deferred(
            a,
            &[
                "comment", "reply", "resolve", "suggest", "accept", "reject", "comments",
            ],
            "M5",
            "the kit's annotation layer (K-ANN-1..7)",
        ),
        P::Export => ParityRow::not_applicable(
            a,
            &["cat"],
            &["export"],
            "the file is the export: cat gives its exact bytes",
        ),
        P::WhyOff => ParityRow::live(a, &["status"]),
        P::UndoPartial => ParityRow::live(a, &["undo --partial"]),
        P::Mention => ParityRow::deferred(
            a,
            &["comment"],
            "M5",
            "mentions ride on comments (K-ANN-1..7)",
        ),
        P::Purge => ParityRow::deferred(
            a,
            &["purge"],
            "the purge slice (K-PURGE-1..12)",
            "redaction of history and revert records is not built",
        ),
        P::Approve => ParityRow::live(a, &["approve", "decline"]),
        P::Pending => ParityRow::live(a, &["log --pending"]),
        P::Revert => ParityRow::live(a, &["revert"]),
        // tier 0's rows (any file): a text doc merges live, it has no lease
        P::Lease | P::ReleaseLease | P::BreakLease => ParityRow::not_applicable(
            a,
            &[],
            &["lease", "lease --renew", "lease --release", "lease --break"],
            "a text doc merges edits; leases are tier 0's",
        ),
    }
}

/// Every row of a text pack, P1 to P31 (the text tier's actions).
pub fn parity() -> Vec<ParityRow> {
    PersonAction::for_tier(Tier::Text).map(parity_row).collect()
}
