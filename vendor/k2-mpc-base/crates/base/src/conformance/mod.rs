//! Primitive conformance: the kit's merge semantics, as rules every engine must pass.
//!
//! The rules live in the separate, never-published crate `k2-mpc-conformance`
//! (`crates/conformance`), generic over engines: the same rules run on this crate's
//! [`crate::Doc`] (Yrs) and on the reference model `mpce-model`. Where an engine can't meet
//! a rule, the deviation is in **that engine's** list with its reason, and the rule asserts
//! the deviation **still happens**: fixing it (or swapping the engine) makes the test fail
//! until the entry is removed. A new engine must have an empty list.
//!
//! This module keeps what belongs to this crate's engine:
//! - [`KNOWN_DEVIATIONS`], the list for [`crate::Doc`];
//! - with the off-by-default `conformance` feature, `replica` (no link: the module exists only
//!   with that feature): the two hooks the suite
//!   needs to drive several replicas of a [`crate::Doc`] in one process. No host enables it.
//!
//! Rules that need this crate's own machinery (wire bytes, `untrusted-wire`, the store,
//! sessions) stay in this crate's tests.

/// (rule id, why [`crate::Doc`]'s engine deviates). Empty for a conforming engine.
///
/// Sixteen entries today. Text: the two YATA orders, three delivery rules, `core.authorship`,
/// `core.steady_plain`, a change
/// record without the author's kind (`prov.record`), and `undo --steps N` committed as N
/// batches.
/// Structured kinds (CSV seam design §5.3): concurrent register writes ordered by client ID
/// (`mlist.move_lww`, `map.lww`, `table.cell_lww`), a set surviving a concurrent delete
/// (`map.delete_vs_set`), overwrite stubs (`map.steady_bounded`), YATA's order of list slots
/// (`mlist.yata_separating_case`), and no write identity for a merged removal
/// (`stream.merged_writes`). None of the structured ones reaches production on the interim:
/// a structured doc has no CRDT path there (every change is the kit's, applied by key in
/// daemon order), so they show only in this suite. An earlier entry, `text.no_interleave_backward_multi`, was wrong: its expectation (three
/// contiguous runs) is unsatisfiable for any engine meeting the strong list spec on that
/// script, and Yrs, Yjs, Diamond Types and Loro all give the same order (cross-checked
/// 2026-10-27). The real Yrs-vs-FugueMax separating case is `text.yata_separating_case`.
pub const KNOWN_DEVIATIONS: &[(&str, &str)] = &[
    (
        "text.yata_separating_case",
        "yrs (YATA) orders the separating script [pfae], splitting one writer's run `fe`; \
     FugueMax gives [apfe]. Wire-format bound: fixed only by a FugueMax-class engine.",
    ),
    (
        "text.yata_paper_a19",
        "yrs (YATA) orders the Fugue paper's A.1.9 example `axb` (as the paper reports for \
         Yjs); FugueMax gives `xab`. Wire-format bound.",
    ),
    (
        "core.causal",
        "the kit checks an update's dependencies per client (each client's clocks must \
         continue the doc's), which refuses a skipped own predecessor with MissingDeps; an \
         event whose parent is another client's event the doc lacks passes that check, yrs \
         holds it as pending, and the kit then poisons the doc (Pending: reload from the \
         store) instead of refusing it with nothing changed. The native engine checks every \
         parent.",
    ),
    (
        "core.refused_no_trace",
        "Hardening; pinned on Yrs. A host calling Doc::apply_client_update directly must reload \
         on Pending; a session does it itself.",
    ),
    (
        "core.no_equivocation",
        "Pinned on Yrs. The native engine conforms (E-CORE-4).",
    ),
    (
        "core.authorship",
        "Hardening; pinned on Yrs. The native engine conforms (E-CORE-5).",
    ),
    (
        "core.steady_plain",
        "Pinned on Yrs: E-TEXT-13's budget is the native engine's target.",
    ),
    (
        "prov.record",
        "the kit's change record (`ChangeInfo`, one per change: the author's pid, on whose \
         behalf, host time, the change's kind and surface) doesn't say whether the author \
         is a person or an agent: the host knows that from the pid. Interface r1.7's typed \
         core carries it (review C9).",
    ),
    (
        "hist.undo_atomic",
        "a session applies `undo --steps N` (and redo) as N engine batches, N versions and N \
         log entries, committed together in one store transaction under the session lock; \
         the rule asks one batch (one log append). A single step over several roots is one \
         batch attributed to the undoer, as asserted.",
    ),
    (
        "mlist.move_lww",
        "a move writes the element's position register, a yrs map entry, and yrs orders \
         concurrent writes of one map entry by client ID (YATA's tie-break), not by \
         (lamport, agent): a move made after more of its writer's own changes can lose. \
         Wire-format bound.",
    ),
    (
        "mlist.yata_separating_case",
        "the list's slots are a yrs array, so YATA orders them: the separating script gives \
         [lb p f a e rb], splitting one writer's run `f e`; FugueMax gives [lb a p f e rb] \
         (as text.yata_separating_case). Wire-format bound.",
    ),
    (
        "map.lww",
        "yrs orders concurrent sets of one map key by client ID (YATA's tie-break), not by \
         (lamport, agent). Wire-format bound.",
    ),
    (
        "map.delete_vs_set",
        "a yrs map delete removes only the entry it saw, so a concurrent set always survives \
         it, whatever (lamport, agent) say. (A list element's delete is a set of its delete \
         flag, and wins, as E-MLIST-2 asks.)",
    ),
    (
        "map.steady_bounded",
        "each overwrite of a map key leaves a content-free item stub (content is \
         garbage-collected, K-PURGE-11), in the live doc and in a doc loaded from its state \
         (the stored v2 state compresses a regular run of them). Bounded on the interim by \
         the CSV caps.",
    ),
    (
        "table.cell_lww",
        "cells are a kit Map, so concurrent sets of one cell are ordered by client ID \
         (map.lww).",
    ),
    (
        "stream.merged_writes",
        "a merged update's stream (`Doc::applied_since`, a keyed diff) names each write by \
         the facet's winner; on yrs a restore removes the delete flag and a map delete \
         removes the entry, and yrs keeps no identity for a removal, so those two ops carry \
         no write (`None`), as `winner(Existence)` of a live element is `None`. The native \
         engine reports every merged op's event ID (E4).",
    ),
];

/// Test-only hooks for driving several replicas of a [`crate::Doc`] in one process
/// (feature `conformance`, off by default). Not part of the kit's API for hosts.
///
/// Every byte these hooks decode was made by one of this kit's docs in the same process
/// ([`replica::sync`], [`replica::events_since`]); nothing from outside is decoded, and
/// `untrusted-wire` is not needed.
#[cfg(feature = "conformance")]
pub mod replica {
    use crate::{ClientId, Doc, DocError, Key};

    /// A relative position in a text root, from [`anchor`]: valid on every replica of the
    /// doc. Opaque.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Anchor(crate::engine::Sticky);

    /// Which character an [`Anchor`] follows.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Stick {
        /// The character before the offset: the anchor stays just after it.
        Left,
        /// The character after the offset: the anchor stays just before it.
        Right,
    }

    /// Everything `doc` has past `version` (a state `doc` has seen, as from [`state`]), as
    /// one update: the inserts past each client's clock, plus every delete `doc` knows.
    pub fn events_since(doc: &Doc, version: &[(ClientId, u64)]) -> Result<Vec<u8>, DocError> {
        doc.events_since(version)
    }

    /// Deliver an update from [`events_since`], or a change's own update (`Doc::edit_as`),
    /// made by another of this kit's docs in this process. An update that skips ahead of a
    /// client's history is refused with [`DocError::MissingDeps`] and changes nothing;
    /// otherwise it is applied, and anything already known in it is skipped.
    pub fn deliver(doc: &mut Doc, update: &[u8]) -> Result<(), DocError> {
        doc.deliver_local(update)
    }

    /// Text root `root`'s live text as runs `(bytes, client, clock)`: the client that
    /// inserted each run and the clock of its first character (in the engine's units:
    /// UTF-16 code units on Yrs), in order, covering the text exactly (what the kit's blame
    /// reads).
    pub fn blame(doc: &Doc, root: &str) -> Result<Vec<(usize, ClientId, u64)>, DocError> {
        Ok(doc
            .blame_runs(root)?
            .into_iter()
            .map(|r| (r.bytes, r.client, r.clock))
            .collect())
    }

    /// A relative position at byte offset `at` of text root `root`, following the
    /// character on side `stick` of it. `None` when `at` isn't a character boundary of the
    /// text or there is no character on that side to follow.
    pub fn anchor(
        doc: &Doc,
        root: &str,
        at: usize,
        stick: Stick,
    ) -> Result<Option<Anchor>, DocError> {
        Ok(doc.sticky(root, at, stick == Stick::Right)?.map(Anchor))
    }

    /// Where `anchor` is now in text root `root`, as a byte offset. A deleted character's
    /// anchor resolves to the gap where it was.
    pub fn resolve(doc: &Doc, root: &str, anchor: &Anchor) -> Result<Option<usize>, DocError> {
        doc.resolve_sticky(root, &anchor.0)
    }

    /// Deliver everything `from` has that `to` lacks. Both docs are this kit's: the diff is
    /// encoded against `to`'s own state vector and applied as trusted bytes (no
    /// `untrusted-wire` needed, nothing from outside is decoded).
    pub fn sync(from: &Doc, to: &mut Doc) -> Result<(), DocError> {
        to.sync_from_local(from)
    }

    /// Check an update from [`events_since`] (made by another of this kit's docs in this
    /// process) as if it came from a connection that owns client `allowed`, without applying
    /// it: the kit's own client check, the same code as `Doc::check_client_update` (which
    /// exists only with `untrusted-wire`), not a copy of it. New content only from
    /// `allowed` ([`DocError::ForeignClient`] otherwise); `inserts_from` lists the clients
    /// with new content. For `core.authorship`; nothing from outside is decoded.
    pub fn check_from(
        doc: &Doc,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<crate::UpdateInfo, DocError> {
        doc.client_checked(update, allowed)
    }

    /// [`check_from`], then apply: the same code as `Doc::apply_client_update`. Nothing is
    /// applied unless every check passed.
    pub fn deliver_from(
        doc: &mut Doc,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<crate::UpdateInfo, DocError> {
        doc.client_applied(update, allowed)
    }

    /// What `doc` has, as sorted `(client, clock)` pairs: equal on replicas that hold the
    /// same inserts (the native engine: the same events, deletes included). Comparable
    /// across replicas of one engine only.
    pub fn state(doc: &Doc) -> Vec<(ClientId, u64)> {
        doc.state_clocks()
    }

    /// `doc` loaded from what it keeps at rest (a steady state, `core.steady_plain`): on Yrs
    /// its encoded state, on the native engine its records.
    pub fn reloaded(doc: &Doc) -> Result<Doc, DocError> {
        doc.reloaded()
    }

    /// Hardening (test-only): on a doc on the native engine, the next apply after a passed
    /// check (a delivery, a client's update) errs after the engine applied it, as an engine
    /// error would: the doc must drop its replica and reopen it from its records, with
    /// nothing of that apply left. `false` on a doc on Yrs (no such hook there).
    pub fn fail_next_apply(doc: &mut Doc) -> bool {
        doc.fail_next_apply()
    }

    /// How many times a native doc's replica was reopened after an apply error; `None` on
    /// Yrs.
    pub fn reopens(doc: &Doc) -> Option<u32> {
        doc.reopens()
    }

    /// How many runs of the engine's ID index the current places of `keys` (in this order)
    /// take in MovableList root `root`: 1 when they are adjacent slots with consecutive
    /// engine IDs (a forward chain, `elem::chain` / `elem::reorder`). `None` when a key was
    /// never inserted.
    pub fn slot_runs(doc: &Doc, root: &str, keys: &[Key]) -> Result<Option<usize>, DocError> {
        doc.slot_runs(root, keys)
    }
}
