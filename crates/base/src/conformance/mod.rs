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
//! - with the off-by-default `conformance` feature, [`replica`]: the two hooks the suite
//!   needs to drive several replicas of a [`crate::Doc`] in one process. No host enables it.
//!
//! Rules that need this crate's own machinery (wire bytes, `untrusted-wire`, the store,
//! sessions) stay in this crate's tests.

/// (rule id, why [`crate::Doc`]'s engine deviates). Empty for a conforming engine.
///
/// Five entries today: the two YATA orders, two delivery rules where yrs identifies and
/// checks content by (client, clock) only, and `undo --steps N` committed as N batches.
/// An earlier entry, `text.no_interleave_backward_multi`, was wrong: its expectation (three
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
        "core.no_equivocation",
        "yrs identifies content by (client, clock) only: a known ID with different content \
         is skipped as already known, silently (the first content stays), not refused and \
         reported. The native engine keeps a keyed digest per run (E-CORE-4).",
    ),
    (
        "hist.undo_atomic",
        "a session applies `undo --steps N` (and redo) as N engine batches, N versions and N \
         log entries, committed together in one store transaction under the session lock; \
         the rule asks one batch (one log append). A single step over several roots is one \
         batch attributed to the undoer, as asserted.",
    ),
];

/// Test-only hooks for driving several replicas of a [`crate::Doc`] in one process
/// (feature `conformance`, off by default). Not part of the kit's API for hosts.
///
/// Every byte these hooks decode was made by one of this kit's docs in the same process
/// ([`sync`], [`events_since`]); nothing from outside is decoded, and `untrusted-wire` is not
/// needed.
#[cfg(feature = "conformance")]
pub mod replica {
    use crate::{ClientId, Doc, DocError};

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
    pub fn events_since(doc: &Doc, version: &[(ClientId, u32)]) -> Result<Vec<u8>, DocError> {
        doc.events_since(version)
    }

    /// Deliver an update from [`events_since`] (made by another of this kit's docs). An
    /// update that skips ahead of a client's history is refused with
    /// [`DocError::MissingDeps`] and changes nothing; otherwise it is applied, and anything
    /// already known in it is skipped.
    pub fn deliver(doc: &mut Doc, update: &[u8]) -> Result<(), DocError> {
        doc.deliver_local(update)
    }

    /// Text root `root`'s live text as runs `(bytes, client, clock)`: the client that
    /// inserted each run and the clock of its first character (clocks count UTF-16 code
    /// units), in order, covering the text exactly (what the kit's blame reads).
    pub fn blame(doc: &Doc, root: &str) -> Result<Vec<(usize, ClientId, u32)>, DocError> {
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

    /// What `doc` has, as sorted `(client, clock)` pairs: equal on replicas that hold the
    /// same inserts. Comparable across replicas of one engine only.
    pub fn state(doc: &Doc) -> Vec<(ClientId, u32)> {
        doc.state_clocks()
    }
}
