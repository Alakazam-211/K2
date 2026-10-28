//! Markdown's parity list as data (`docs/PARITY.md` is the same list for people).
//!
//! One exhaustive `match`: when the kit adds a person action, this stops compiling until
//! the action is mapped. `tests/agent_parity.rs` runs it through the kit's harness.

use k2_mpc_base::parity::{ParityRow, PersonAction};

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
        P::Who => ParityRow::live(a, &["who"]),
        P::Follow => ParityRow::live(a, &["follow"]),
        // `intent --lines a,b` (K-PRES-12) is still to come
        P::Caret => ParityRow::live_but(a, &["cursor"], &["intent"]),
        P::Type => ParityRow::live(a, &["insert"]),
        P::TypeOver => ParityRow::live(a, &["replace"]),
        P::Delete => ParityRow::live(a, &["delete", "sed -i"]),
        P::FindReplace => ParityRow::live(a, &["sed -i"]),
        P::Paste => ParityRow::live(a, &["patch", "apply", "append", "replace-section"]),
        P::Rewrite => ParityRow::live(a, &["write"]),
        // Markdown formatting is text
        P::Format => ParityRow::live(a, &["insert", "replace", "sed -i"]),
        P::Undo => ParityRow::live(a, &["undo", "redo", "undo --drop"]),
        P::Saved => ParityRow::live(a, &["status", "flush"]),
        P::Watch => ParityRow::live(a, &["watch", "tail -f"]),
        P::History => ParityRow::live(a, &["log", "diff --version", "show --version", "diff"]),
        P::Restore => ParityRow::live(a, &["restore", "version --name"]),
        P::Rename => ParityRow::live(a, &["mv"]),
        P::Blame => ParityRow::deferred(
            a,
            &["blame"],
            "C7b (the release after MD-v1)",
            "blame lands with its UI; the client-to-pid mapping is recorded now",
        ),
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
    }
}

/// Every row, P1 to P31.
pub fn parity() -> Vec<ParityRow> {
    PersonAction::ALL.iter().map(|a| parity_row(*a)).collect()
}
