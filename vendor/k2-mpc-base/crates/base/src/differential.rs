//! **Test-only** (the `differential` feature, and the kit's own unit tests): which path makes
//! each engine edit, chosen per thread, so that one sequence of operations can be run through
//! two paths and compared byte for byte (v0.9.3). No host enables the feature; without it the
//! engine has one path and none of this is compiled.
//!
//! - [`Path`]: how [`crate::Doc::edit_as`] (and every session verb built on it: edits,
//!   `write --base`, undo, redo, revert, restore, import, outside edits) makes its change.
//! - [`Side`]: one side of a comparison, installed on the current thread by [`with_side`]:
//!   its path; an optional **shadow** path (each edit is also made on a twin of the doc
//!   through the shadow path, and any difference in the update, the resulting state, state
//!   vector or texts **panics**); a seed for **deterministic client IDs** (so two sessions
//!   driven alike mint the same IDs); and a record of every edit (client, update or error,
//!   state after).
//!
//! The mutant paths exist so a differential test can show it catches a wrong path: each makes
//! the same text as today's path in the common case and differs in its bytes.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::doc::{ClientId, DocError, MAX_CLIENT_ID};
use crate::engine::{EngineDoc, TextPlan, YrsDoc};

/// A fault planted in the structure index ([`crate::index`], the store's `blob_struct`), so its
/// invariant tests can show they catch each one. Installed per thread by
/// [`with_index_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IndexMutant {
    /// A draft over the node cap is kept (`blob_index_max_nodes`).
    NoNodeCap,
    /// A draft over the depth cap is kept.
    NoDepthCap,
    /// A row over `blob_index_max_stored_bytes` is kept.
    NoStoredCap,
    /// The raw digest is stored unkeyed.
    UnkeyedHash,
    /// A purged blob's index row stays.
    PurgeKeepsRow,
    /// A purge removes the index row of a blob that is still shared.
    SharedLosesRow,
    /// A job commits for a version that was purged or moved.
    StaleCommitAccepted,
    /// A job commits without having read the stream to its verified end.
    NoDrain,
    /// A rewrite copies an index row whose blob is gone.
    OrphanSurvivesRewrite,
    /// No barrier removes a row: not the blob's transaction, the rewrite's filter or the sweep.
    NoRowRemoval,
    /// A key overlay is used whatever pack and format it was minted for.
    KeysIgnoreFormat,
    /// The digest key is derived the way a blob id is (HMAC under the space key).
    DigestKeyInFileDomain,
    /// The orphan sweep scans with a write transaction held.
    SweepHoldsWriter,
    /// The orphan sweep deletes without re-checking that the blob is still gone.
    SweepSkipsRecheck,
    /// Storing a read's minted keys doesn't re-check the version.
    PutKeysNoCheck,
    /// A commit overwrites a row and overlay that are already there.
    KeysOverwritten,
    /// Deleting a blob version leaves its overlay.
    DeleteKeepsKeys,
    /// A stored row's parents aren't checked at decode.
    RowParentsUnchecked,
    /// A worker's result is decoded with no bound on what it can allocate.
    NoDecodeBudget,
    /// A stored row of format 2 drops a node's view geometry (row format 2's items).
    IndexV2FieldDropped,
    /// A spec with own records (`IndexSpec::with_own_records`) is stored under the pack's own
    /// format, not the kit's offset one: an older kit asking for that format decodes a row of
    /// format 3.
    OwnOffsetSkipped,
    // the read side and claims by node
    /// A structure read checks no rights: not read, not history for another version.
    ReadSkipsRights,
    /// A page holds everything, whatever the read budget.
    NoBudget,
    /// A name that names nothing resolves to the first node whose name contains it.
    GuessesNearName,
    /// A name or ID two nodes share resolves to the first of them.
    AmbiguousPicksFirst,
    /// `#N` past the last node names the last one.
    NthClamps,
    /// A cursor is taken without checking its version or tag.
    CursorUnchecked,
    /// `--no-truncate` returns a page instead of refusing.
    NoTruncatePages,
    /// A node read with a missing address exits 0.
    PartialExitsZero,
    /// Another participant's node claim refuses a write.
    ClaimBlocksWrite,
    /// A claim on a node another participant claims is taken.
    ClaimOverlapAllowed,
    /// A claim whose nodes are all gone from a newly read version doesn't end.
    ClaimOutlivesNodes,
    /// An item larger than a page goes out whole.
    ItemsNotShortened,
    /// `--no-truncate` refuses only when more pages follow (not a shortened item).
    NoTruncateOnlyOnMore,
    /// A missing or ambiguous address lists every candidate.
    CandidatesUncapped,
    /// A node read's misses go whole on every page.
    MissingUncapped,
    /// The claims' check against a new version's index decodes it under the session lock.
    ClaimsLoadUnderLock,
}

impl IndexMutant {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "no-node-cap" => Self::NoNodeCap,
            "no-depth-cap" => Self::NoDepthCap,
            "no-stored-cap" => Self::NoStoredCap,
            "unkeyed-hash" => Self::UnkeyedHash,
            "purge-keeps-row" => Self::PurgeKeepsRow,
            "shared-loses-row" => Self::SharedLosesRow,
            "stale-commit-accepted" => Self::StaleCommitAccepted,
            "no-drain" => Self::NoDrain,
            "orphan-survives-rewrite" => Self::OrphanSurvivesRewrite,
            "no-row-removal" => Self::NoRowRemoval,
            "keys-ignore-format" => Self::KeysIgnoreFormat,
            "digest-key-in-file-domain" => Self::DigestKeyInFileDomain,
            "sweep-holds-writer" => Self::SweepHoldsWriter,
            "sweep-skips-recheck" => Self::SweepSkipsRecheck,
            "put-keys-no-check" => Self::PutKeysNoCheck,
            "keys-overwritten" => Self::KeysOverwritten,
            "delete-keeps-keys" => Self::DeleteKeepsKeys,
            "row-parents-unchecked" => Self::RowParentsUnchecked,
            "no-decode-budget" => Self::NoDecodeBudget,
            "index-v2-field-dropped" => Self::IndexV2FieldDropped,
            "own-offset-skipped" => Self::OwnOffsetSkipped,
            "read-skips-rights" => Self::ReadSkipsRights,
            "no-budget" => Self::NoBudget,
            "guesses-near-name" => Self::GuessesNearName,
            "ambiguous-picks-first" => Self::AmbiguousPicksFirst,
            "nth-clamps" => Self::NthClamps,
            "cursor-unchecked" => Self::CursorUnchecked,
            "no-truncate-pages" => Self::NoTruncatePages,
            "partial-exits-zero" => Self::PartialExitsZero,
            "claim-blocks-write" => Self::ClaimBlocksWrite,
            "claim-overlap-allowed" => Self::ClaimOverlapAllowed,
            "claim-outlives-nodes" => Self::ClaimOutlivesNodes,
            "items-not-shortened" => Self::ItemsNotShortened,
            "no-truncate-only-on-more" => Self::NoTruncateOnlyOnMore,
            "candidates-uncapped" => Self::CandidatesUncapped,
            "missing-uncapped" => Self::MissingUncapped,
            "claims-load-under-lock" => Self::ClaimsLoadUnderLock,
            _ => return None,
        })
    }
}

/// Run `f` under the mutant `MPC_INDEX_MUTANT` names (unset: as is; an unknown name stops the
/// test, naming it).
pub fn under_env_index_mutant<R>(f: impl FnOnce() -> R) -> R {
    match std::env::var("MPC_INDEX_MUTANT") {
        Ok(v) => with_index_mutant(
            IndexMutant::named(&v)
                .unwrap_or_else(|| panic!("MPC_INDEX_MUTANT={v}: not an index mutant")),
            f,
        ),
        Err(_) => f(),
    }
}

thread_local! {
    static INDEX_MUTANT: std::cell::Cell<Option<IndexMutant>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    static INDEX_LOADS: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

/// Count one structure index decoded on this thread (`locked`: with its session's lock held).
pub(crate) fn count_index_load(locked: bool) {
    INDEX_LOADS.with(|c| {
        let (n, l) = c.get();
        c.set((n + 1, l + u64::from(locked)));
    });
}

/// The structure indexes decoded on this thread so far, and how many of them with their
/// session's lock held (a test reads the difference across a call).
pub fn index_loads() -> (u64, u64) {
    INDEX_LOADS.with(|c| c.get())
}

/// The structure-index mutant installed on this thread, if any.
pub fn index_mutant() -> Option<IndexMutant> {
    INDEX_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` installed on this thread.
pub fn with_index_mutant<R>(mutant: IndexMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<IndexMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            let back = self.0;
            INDEX_MUTANT.with(|m| m.set(back));
        }
    }
    let _back = Back(INDEX_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in anchors ([`crate::anchor`]) or the annotations ([`crate::annot`]), so
/// their invariant tests can show they catch each one. Installed per thread by
/// [`with_annot_mutant`]; a test reads `MPC_ANNOT_MUTANT` through [`under_env_annot_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnnotMutant {
    /// A key gone with a candidate reads orphaned.
    CandidateReadsOrphaned,
    /// A hidden node hides a kind change (precedence rows 4 and 5 swapped).
    HiddenOverDoubt,
    /// A key carried by position alone is trusted.
    WeakKeyTrusted,
    /// A duplicated key resolves to its first holder.
    DuplicateFirstWins,
    /// The default judge doubts every rename.
    RenameIsDoubt,
    /// Recording an index moves an anchor to its first candidate.
    AutoReanchor,
    /// An agent's re-anchor applies, as a person's does.
    AgentReanchorApplies,
    /// Deleting a comment keeps its body in the row (only hidden from reads).
    DeleteKeepsBody,
    /// An annotation purge leaves the bodies.
    PurgeKeepsBodies,
    /// Participant erasure leaves the erased person's comment bodies.
    ErasureKeepsBodies,
    /// A version purge leaves the anchors' copies of that version's names.
    VersionPurgeKeepsHints,
    /// Commenting needs only read.
    CommentNeedsRead,
    /// Edit doesn't imply comment.
    EditWithoutComment,
    /// Anyone with comment may delete anyone's comment.
    AnyoneDeletes,
    /// A node anchor is written from a version without re-checking it is live.
    AnchorFromPurgedVersion,
    /// A text doc's purge and erasure don't reach the threads it kept from tier 0.
    TextPurgeSkipsAnnotations,
    /// Blob erasure takes any pseudonym (a reserved one, a participant's own pid).
    EraseSkipsChecks,
    /// Erasure leaves the bodies an agent wrote on the erased person's behalf.
    ErasureSkipsBehalf,
    /// Kit-side changes (erasure, a version purge, an index record) don't mark a thread
    /// changed.
    SinceMissesKitChanges,
    /// Attribution fields are written at any length.
    AttrUnbounded,
    /// An erasure doesn't wait for comment writes already admitted.
    NoAnnotGate,
    /// A reader without history sees an older version's node copies.
    HistoryLeak,
    /// An index record puts back a `last_seen` a purge cleared.
    LastSeenReturns,
}

impl AnnotMutant {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "candidate-reads-orphaned" => Self::CandidateReadsOrphaned,
            "hidden-over-doubt" => Self::HiddenOverDoubt,
            "weak-key-trusted" => Self::WeakKeyTrusted,
            "duplicate-first-wins" => Self::DuplicateFirstWins,
            "rename-is-doubt" => Self::RenameIsDoubt,
            "auto-reanchor" => Self::AutoReanchor,
            "agent-reanchor-applies" => Self::AgentReanchorApplies,
            "delete-keeps-body" => Self::DeleteKeepsBody,
            "purge-keeps-bodies" => Self::PurgeKeepsBodies,
            "erasure-keeps-bodies" => Self::ErasureKeepsBodies,
            "version-purge-keeps-hints" => Self::VersionPurgeKeepsHints,
            "comment-needs-read" => Self::CommentNeedsRead,
            "edit-without-comment" => Self::EditWithoutComment,
            "anyone-deletes" => Self::AnyoneDeletes,
            "anchor-from-purged-version" => Self::AnchorFromPurgedVersion,
            "text-purge-skips-annotations" => Self::TextPurgeSkipsAnnotations,
            "erase-skips-checks" => Self::EraseSkipsChecks,
            "erasure-skips-behalf" => Self::ErasureSkipsBehalf,
            "since-misses-kit-changes" => Self::SinceMissesKitChanges,
            "attr-unbounded" => Self::AttrUnbounded,
            "no-annot-gate" => Self::NoAnnotGate,
            "history-leak" => Self::HistoryLeak,
            "last-seen-returns" => Self::LastSeenReturns,
            _ => return None,
        })
    }
}

/// Run `f` under the mutant `MPC_ANNOT_MUTANT` names (unset: as is; an unknown name stops the
/// test, naming it).
pub fn under_env_annot_mutant<R>(f: impl FnOnce() -> R) -> R {
    match std::env::var("MPC_ANNOT_MUTANT") {
        Ok(v) => with_annot_mutant(
            AnnotMutant::named(&v)
                .unwrap_or_else(|| panic!("MPC_ANNOT_MUTANT={v}: not an annotation mutant")),
            f,
        ),
        Err(_) => f(),
    }
}

thread_local! {
    static ANNOT_MUTANT: std::cell::Cell<Option<AnnotMutant>> = const { std::cell::Cell::new(None) };
}

/// The anchor or annotation mutant installed on this thread, if any.
pub fn annot_mutant() -> Option<AnnotMutant> {
    ANNOT_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` installed on this thread.
pub fn with_annot_mutant<R>(mutant: AnnotMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<AnnotMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            let back = self.0;
            ANNOT_MUTANT.with(|m| m.set(back));
        }
    }
    let _back = Back(ANNOT_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A one-shot pause for the next annotation write, after it is admitted and before it
/// commits (a test drives a race through it): the write sends on `paused`, then waits on
/// `resume`. Process-wide: one at a time.
pub fn pause_next_annotation(
    paused: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
) {
    *ANNOT_PAUSE.lock().unwrap_or_else(|p| p.into_inner()) = Some((paused, resume));
}

type Pause = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);
static ANNOT_PAUSE: std::sync::Mutex<Option<Pause>> = std::sync::Mutex::new(None);

/// The pause point ([`pause_next_annotation`]).
pub(crate) fn annotation_pause_point() {
    let armed = ANNOT_PAUSE.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some((paused, resume)) = armed {
        let _ = paused.send(());
        let _ = resume.recv();
    }
}

/// A fault planted in person undo grouping (undo spec U3, v0.9.7), so the grouping
/// tests can show they catch each one. Installed per thread by [`with_group_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GroupMutant {
    /// Groups never close on another participant's change inside or touching their span
    /// (U3 (g)): the edit joins whatever landed in its window.
    NeverClosesOnForeign,
    /// Groups never close on idle (U3 (a)).
    NeverClosesOnIdle,
    /// Groups never close on a word boundary (U3 (c)).
    NeverClosesOnBoundary,
}

/// A planted fault in the outside-save notes (`tests/outside_removal_notes.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteMutant {
    /// No note is queued.
    DropAll,
    /// The roll-up is not kept: past the bound a note is just dropped.
    DropRollup,
    /// Every note goes to the first author of the save, whoever wrote the text.
    WrongAuthor,
    /// The host's edit calls (`edit_with`...) take waiting notes and drop them (the bug the
    /// soak found before v0.9.7.2 shipped).
    EditWithDrains,
}

thread_local! {
    static NOTE_REMOVAL_COST: std::cell::Cell<std::time::Duration> = const { std::cell::Cell::new(std::time::Duration::ZERO) };
}

/// How long the last outside save on this thread spent working out whose text it removes, under
/// the session's lock (the measurement in `tests/outside_removal_notes.rs`).
pub fn last_removal_cost() -> std::time::Duration {
    NOTE_REMOVAL_COST.with(|c| c.get())
}

pub(crate) fn note_removal_cost(d: std::time::Duration) {
    NOTE_REMOVAL_COST.with(|c| c.set(d));
}

thread_local! {
    static NOTE_MUTANT: std::cell::Cell<Option<NoteMutant>> = const { std::cell::Cell::new(None) };
}

/// The notes fault planted on this thread, if any.
pub fn note_mutant() -> Option<NoteMutant> {
    NOTE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_note_mutant<R>(mutant: NoteMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<NoteMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            NOTE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(NOTE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

thread_local! {
    static GROUP_MUTANT: std::cell::Cell<Option<GroupMutant>> = const { std::cell::Cell::new(None) };
}

/// The grouping fault planted on this thread, if any.
pub fn group_mutant() -> Option<GroupMutant> {
    GROUP_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_group_mutant<R>(mutant: GroupMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<GroupMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            GROUP_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(GROUP_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the kit's use of [`crate::session::Pack::check_merged`], so the tests
/// that should catch it (`tests/check_merged.rs`) can show they do. Installed per thread by
/// [`with_check_merged_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckMergedMutant {
    /// The call is skipped: every merge is taken as if the pack accepted it.
    Skipped,
}

thread_local! {
    static CHECK_MERGED_MUTANT: std::cell::Cell<Option<CheckMergedMutant>> = const { std::cell::Cell::new(None) };
}

/// The merged-text check's fault planted on this thread, if any.
pub fn check_merged_mutant() -> Option<CheckMergedMutant> {
    CHECK_MERGED_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_check_merged_mutant<R>(mutant: CheckMergedMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<CheckMergedMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            CHECK_MERGED_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(CHECK_MERGED_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in tier 0's handling of an outside save, so the tests that should catch it
/// (`tests/tier0.rs`, the stale-after-kit-write rows) can show they do. Installed per thread by
/// [`with_tier0_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier0Mutant {
    /// "stale-after-kit-write ignored": an outside save over a version the kit wrote, with no
    /// lease held, is taken whole, the old way (the kit's write left only in history).
    StaleAfterKitWriteIgnored,
}

thread_local! {
    static TIER0_MUTANT: std::cell::Cell<Option<Tier0Mutant>> = const { std::cell::Cell::new(None) };
}

/// Tier 0's fault planted on this thread, if any.
pub fn tier0_mutant() -> Option<Tier0Mutant> {
    TIER0_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_tier0_mutant<R>(mutant: Tier0Mutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<Tier0Mutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            TIER0_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(TIER0_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the tier-0 verb path (`session/tier0/verbs.rs`: a pack's verb the kit
/// runs on a blob doc, its plan, the applier, the checks, the dry run and the record), so the
/// rows of `tests/tier0_verbs.rs` show they catch each one, at their own assertions. Installed
/// per thread by [`with_tier0_verb_mutant`], or named by `MPC_T0V_MUTANT` for a whole test run
/// (an unknown name stops the run, naming it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tier0VerbMutant {
    /// The applier copies one byte twice: the last byte of the stretch before each span but
    /// its final one (a byte the plan never named changes).
    ApplierOffByOne,
    /// A plan's spans are taken unchecked: out of order, overlapping, outside the file, over
    /// the caps.
    PlanUnchecked,
    /// The output's index diff isn't held to what the plan declared (a strayed node lands).
    StrayUnchecked,
    /// An output the pack's index refuses is taken anyway (its diff empty): it lands.
    OutputIndexSkipped,
    /// The result's changed units are the pack's `touched` and `created`, not the index diff.
    UnitsFromPlan,
    /// A `merge: Lease` verb runs without the caller's own lease.
    LeaseOnlyIgnored,
    /// A verb-made version is stored without its verb (`blob_verbs`).
    VerbNotRecordedBlob,
    /// The commit doesn't check that head is still the version the plan was made on.
    CommitWithoutRecheck,
    /// An agent's verb touching another participant's node claim lands under
    /// `ClaimPolicy::Respect`.
    ClaimUnchecked,
    /// A verb on a doc over the merge bound carries no `merge_over_bound` note.
    MergeNoteMissing,
    /// A dry run commits its version.
    Commits,
    /// A dry run takes the auto lease (and leaves it).
    TakesLease,
    /// A dry run is charged to the edit rate bucket.
    ChargesRate,
    /// A dry run moves its caller's presence (their last edit).
    MovesPresence,
    /// A dry run asks for the output's index job.
    StoresIndex,
    /// A dry run leaves its uncommitted put open.
    LeavesPut,
    /// A dry run writes the file back.
    WritesBack,
    /// A dry run skips a guard the real run makes (`--if-version`).
    SkipsGuards,
    /// The test pack's verb reads hidden state (a counter): two identical calls differ.
    PackImpure,
    /// The registry takes a blob pack's verbs and refusals unchecked.
    RegistryUnchecked,
    /// An erasure leaves the erased participant's verbs (`blob_verbs`).
    VerbKeptOnErase,
    /// A verb's output doesn't say the version it made (`ToolOutput::version_made`).
    VersionMadeHidden,
    /// A verb's view names the first changed node's place, not the last's.
    ViewFirstNode,
    /// A seeded index row's minted keys are minted afresh instead of carried from the version
    /// before (what the host's job would record differs).
    SeedWithoutCarry,
    /// A verb's version isn't indexed from its output: the next verb waits for the host's job.
    SeedSkipped,
    /// The judge is skipped across outside saves: a reused key reads as the same node.
    ReuseReadsAttached,
    /// An anchor across an outside save is judged against its original fingerprint, not the
    /// node just before the save.
    JudgeAgainstFingerprint,
    /// A diff's crossed saves don't count the keys they kept.
    KeysKeptUncounted,
    /// Blame rows that end on an outside save aren't labelled.
    BlameLabelMissing,
    /// A save the pack says reissues keys is answered across, as if unknown.
    ReissuesAnswered,
    /// The state an index job records for an anchor is judged by its fingerprint alone, not
    /// across the outside saves as a read judges it.
    StoredJudgeByFingerprint,
    /// The state an index job records skips an outside save whose version before it has no
    /// stored keys yet, where a read would mint them and judge it.
    StoredSkipsUnminted,
    /// `--base V` refuses whenever head moved since `V`, as the whole-doc guard did.
    BaseWholeDoc,
    /// `--base V` is not checked per node: a call lands over a change made since `V`.
    BaseIgnored,
    /// `--base V` checks per node only: two changes to one node always overlap.
    BaseNodeLevelOnly,
    /// `--base V` runs the verb at head only: a node that left the selection since `V` escapes.
    BaseHeadOnly,
    /// Keys a verb creates are conflict candidates: two concurrent adds collide.
    CreatedKeysConflict,
    /// An outside save between `V` and head is taken as keeping node keys.
    PairingAssumed,
    /// Keys the kit minted pass `--base` as if they were the file's own.
    WeakKeyGuarded,
    /// `--base V` compares against `V`'s index where it should read head's: nothing reads as
    /// moved, so a change made since lands twice.
    PlanOnStaleHead,
    /// A verb that finds head's index not yet recorded plans on the version before head: the
    /// edit just committed is lost.
    SeedGapLoses,
    /// A verb's result leaves out its word.
    WordDropped,
    /// Only a held node itself is checked against an agent's change, not its reach.
    ReachIgnored,
    /// A hold's reach is what it covered when taken: members added since escape it.
    ReachAtHoldTime,
    /// The agent-write check is run in the verb path under `AgentWrites::VerbsOnly`: an
    /// agent's verbs are refused as its whole-file writes are.
    VerbsOnlyCheckInVerbPath,
    /// A verb holds the automatic lease for its whole run, planning included, as it once did:
    /// another agent's concurrent verb is refused for the lease.
    LeaseHeldWholeVerb,
    /// An agent's whole-file write under `ClaimPolicy::Respect`, read for what it reaches,
    /// skips the claim check of that reach: a write changing a held node lands.
    WholeWriteSkipsHolds,
    /// An agent's whole-file write trusts what it found at admission: a hold taken while it
    /// uploads (no holds when admitted) isn't checked at the commit.
    HoldDuringUploadMissed,
    /// A batch commits each call as its own version as it goes: three calls make three
    /// versions, and a refusal at a later call leaves the earlier ones applied.
    BatchCommitsEach,
    /// Each call of a batch plans on head, not on what the calls before it staged.
    BatchPlansOnHead,
    /// A batch's ranges are merged by offset as if every call's were head's, a later range
    /// replacing an earlier one it overlaps: a call after one that changed a length lands in
    /// the wrong place.
    BatchComposeWrong,
    /// A batch's `--base V` checks only its last call's nodes against what changed since V.
    BatchBaseLastCallOnly,
    /// A batch's claim check reads only its first call's reach.
    BatchClaimsFirstCall,
    /// A batch holds the automatic lease for its whole run, planning included: another
    /// agent's batch is refused for the lease.
    BatchLeaseWholeRun,
    /// A tier-0 revert takes the whole version before K, not K's change alone: what changed
    /// since is lost.
    RevertTakesWholeVersion,
    /// A tier-0 revert skips its check of what changed since K and, on a merge's conflict,
    /// takes the version before K: a later change to the same field is overwritten.
    RevertOverwrites,
    /// A tier-0 revert of version K lets anyone with edit rights take K's change away.
    RevertIgnoresAuthor,
    /// `undo` takes the newest kit-made version of anyone, not the caller's own.
    UndoLatestAny,
    /// A new change by the caller leaves their redo in place.
    RedoNotCleared,
    /// A tier-0 revert skips the claim check: an agent's revert into a held node lands.
    RevertSkipsClaims,
    /// The overlap checks (`--base`, a revert's) count every change to a container as its
    /// own, as they did before the own record: an edit to one child conflicts with any later
    /// edit to a sibling.
    ContainerAlwaysCounted,
    /// The overlap checks leave out any record-only change to a node whose child changed,
    /// without asking its own record's hash: a container's own change (its children's order, a
    /// field the index doesn't show) is missed.
    ContainerNeverCounted,
}

impl Tier0VerbMutant {
    /// The mutant `MPC_T0V_MUTANT` names: its name in kebab case.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "applier-off-by-one" => Self::ApplierOffByOne,
            "plan-unchecked" => Self::PlanUnchecked,
            "stray-unchecked" => Self::StrayUnchecked,
            "output-index-skipped" => Self::OutputIndexSkipped,
            "units-from-plan" => Self::UnitsFromPlan,
            "lease-only-ignored" => Self::LeaseOnlyIgnored,
            "verb-not-recorded-blob" => Self::VerbNotRecordedBlob,
            "commit-without-recheck" => Self::CommitWithoutRecheck,
            "claim-unchecked" => Self::ClaimUnchecked,
            "merge-note-missing" => Self::MergeNoteMissing,
            "commits" => Self::Commits,
            "takes-lease" => Self::TakesLease,
            "charges-rate" => Self::ChargesRate,
            "moves-presence" => Self::MovesPresence,
            "stores-index" => Self::StoresIndex,
            "leaves-put" => Self::LeavesPut,
            "writes-back" => Self::WritesBack,
            "skips-guards" => Self::SkipsGuards,
            "pack-impure" => Self::PackImpure,
            "registry-unchecked" => Self::RegistryUnchecked,
            "verb-kept-on-erase" => Self::VerbKeptOnErase,
            "version-made-hidden" => Self::VersionMadeHidden,
            "view-first-node" => Self::ViewFirstNode,
            "seed-without-carry" => Self::SeedWithoutCarry,
            "seed-skipped" => Self::SeedSkipped,
            "reuse-reads-attached" => Self::ReuseReadsAttached,
            "judge-against-fingerprint" => Self::JudgeAgainstFingerprint,
            "keys-kept-uncounted" => Self::KeysKeptUncounted,
            "blame-label-missing" => Self::BlameLabelMissing,
            "reissues-answered" => Self::ReissuesAnswered,
            "stored-judge-by-fingerprint" => Self::StoredJudgeByFingerprint,
            "stored-skips-unminted" => Self::StoredSkipsUnminted,
            "base-whole-doc" => Self::BaseWholeDoc,
            "base-ignored" => Self::BaseIgnored,
            "base-node-level-only" => Self::BaseNodeLevelOnly,
            "base-head-only" => Self::BaseHeadOnly,
            "created-keys-conflict" => Self::CreatedKeysConflict,
            "pairing-assumed" => Self::PairingAssumed,
            "weak-key-guarded" => Self::WeakKeyGuarded,
            "plan-on-stale-head" => Self::PlanOnStaleHead,
            "seed-gap-loses" => Self::SeedGapLoses,
            "word-dropped" => Self::WordDropped,
            "reach-ignored" => Self::ReachIgnored,
            "reach-at-hold-time" => Self::ReachAtHoldTime,
            "verbs-only-check-in-verb-path" => Self::VerbsOnlyCheckInVerbPath,
            "lease-held-whole-verb" => Self::LeaseHeldWholeVerb,
            "whole-write-skips-holds" => Self::WholeWriteSkipsHolds,
            "hold-during-upload-missed" => Self::HoldDuringUploadMissed,
            "batch-commits-each" => Self::BatchCommitsEach,
            "batch-plans-on-head" => Self::BatchPlansOnHead,
            "batch-compose-wrong" => Self::BatchComposeWrong,
            "batch-base-last-call-only" => Self::BatchBaseLastCallOnly,
            "batch-claims-first-call" => Self::BatchClaimsFirstCall,
            "batch-lease-whole-run" => Self::BatchLeaseWholeRun,
            "revert-takes-whole-version" => Self::RevertTakesWholeVersion,
            "revert-overwrites" => Self::RevertOverwrites,
            "revert-ignores-author" => Self::RevertIgnoresAuthor,
            "undo-latest-any" => Self::UndoLatestAny,
            "redo-not-cleared" => Self::RedoNotCleared,
            "revert-skips-claims" => Self::RevertSkipsClaims,
            "container-always-counted" => Self::ContainerAlwaysCounted,
            "container-never-counted" => Self::ContainerNeverCounted,
            _ => return None,
        })
    }
}

thread_local! {
    static TIER0_VERB_MUTANT: std::cell::Cell<Option<Tier0VerbMutant>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    static SEED_GAP_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

/// Run `f` with `hook` called once on this thread in a tier-0 verb's gap: after its version is
/// committed and its own lease released, before the version's index (from the verb's output)
/// is recorded. A test runs another participant's call there.
pub fn with_seed_gap_hook<R>(hook: impl FnOnce() + 'static, f: impl FnOnce() -> R) -> R {
    SEED_GAP_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    let r = f();
    SEED_GAP_HOOK.with(|h| *h.borrow_mut() = None);
    r
}

/// How many seed-gap hooks are running (any thread): the planted fault
/// [`Tier0VerbMutant::SeedGapLoses`] bites only inside one, so the rows that race without
/// the hook aren't left to timing.
static IN_SEED_GAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The gap's hook, if one is set: taken, so a call made from it doesn't run it again.
pub(crate) fn seed_gap_hook() {
    let hook = SEED_GAP_HOOK.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        IN_SEED_GAP.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        hook();
        IN_SEED_GAP.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Whether a seed-gap hook is running ([`with_seed_gap_hook`]).
pub(crate) fn in_seed_gap() -> bool {
    IN_SEED_GAP.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// The verb path's fault planted on this thread, else the one `MPC_T0V_MUTANT` names, if any.
pub fn tier0_verb_mutant() -> Option<Tier0VerbMutant> {
    TIER0_VERB_MUTANT.with(|m| m.get()).or_else(|| {
        let v = std::env::var("MPC_T0V_MUTANT").ok()?;
        Some(
            Tier0VerbMutant::named(&v)
                .unwrap_or_else(|| panic!("MPC_T0V_MUTANT={v}: not a tier-0 verb mutant")),
        )
    })
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_tier0_verb_mutant<R>(mutant: Tier0VerbMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<Tier0VerbMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            TIER0_VERB_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(TIER0_VERB_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the text → tier 0 demotion
/// ([`crate::session::SessionManager::demote_text_file`]), so the rows that should catch it
/// (`tests/demote.rs`) can show they do. Installed per thread by [`with_demote_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DemoteMutant {
    /// "demote drops unwritten": the live doc's final state isn't kept; the tier-0 doc starts
    /// from what the file holds (or, without one, the last version), so a change the file
    /// never got is gone.
    DropsUnwritten,
    /// "demote merges by line": an outside save since the last sync is merged into the live
    /// text by lines first (as a text doc takes one in), then demoted, instead of keeping both.
    MergesByLine,
}

thread_local! {
    static DEMOTE_MUTANT: std::cell::Cell<Option<DemoteMutant>> = const { std::cell::Cell::new(None) };
}

/// The demotion's fault planted on this thread, if any.
pub fn demote_mutant() -> Option<DemoteMutant> {
    DEMOTE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_demote_mutant<R>(mutant: DemoteMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<DemoteMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            DEMOTE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(DEMOTE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in a keyed doc that isn't a table (a deck, a board) on the live path, so the
/// rows that should catch it (`tests/keyed_live.rs`) can show they do, by name. Installed per
/// thread by [`with_keyed_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyedMutant {
    /// "keyed limit not applied": `Config::keyed_max_entries` is skipped, at import, for a
    /// change and for an outside save, as before the limit existed.
    LimitNotApplied,
    /// "keyed outside save applied silently": an outside save over a file the kit wrote since
    /// the last outside save is applied straight onto the live doc, the old way: no conflict
    /// copy, no version kept on purpose, no event.
    OutsideSaveAppliedSilently,
    /// "purge skips live copies": a purge of a live doc leaves its recorded conflict copies
    /// on disk and in `live_copies`, unreported, whatever they hold.
    PurgeSkipsLiveCopies,
    /// "carrier stale": a kit write doesn't become the carrier, so `Pack::encode_from` is
    /// handed the bytes from one write earlier.
    CarrierStale,
    /// "purge keeps carrier": the carrier a write replaces is left for blob GC (its count
    /// given back, its bytes in the blob folder until the grace runs out) instead of removed
    /// at once, so a purge's write-back leaves the erased bytes behind.
    PurgeKeepsCarrier,
    /// "copy compared with encode": a live conflict copy of a pack that keeps a carrier is
    /// compared, on `tick` and by a purge, with `Pack::encode` of its kept version instead of
    /// what the kit wrote (the version spliced into its carrier), so it never matches.
    CopyComparedWithEncode,
    /// "file_at uses encode": `Session::file_at` of a pack that keeps a carrier renders the
    /// version with `Pack::encode`, without its opaque bytes.
    FileAtUsesEncode,
    /// "removal ignores waiting puts": a removal at once (a purge's, a replaced carrier's)
    /// takes an unreferenced blob even while a put of it waits for its writer's reference, as
    /// before (the writer's reference then fails, `NoBlob`).
    RemovalIgnoresWaitingPuts,
    /// "read encodes": a structured read verb renders the file (the doc's content, then the
    /// pack's encoding) on every read, as before, though it reads the keyed state only.
    ReadEncodes,
    /// "read visits whole doc": a structured read verb's view is built from every root of the
    /// engine doc on every read, as before, instead of the live view the session keeps in step.
    ReadVisitsWholeDoc,
    /// "open renders to compare": attaching the file renders the doc to compare it with the
    /// file, as before, though the sync point already says whether the doc renders it.
    OpenRendersToCompare,
    /// "list rereads root": a list op's change reads the whole list root afresh from the engine
    /// into the live view, as before, instead of replaying the op on it.
    ListReReadsRoot,
    /// "synced content stale": a kit write keeps the carrier's pairing from the sync
    /// point before, so `Pack::encode_from_paired` is handed keys that don't fit the bytes.
    SyncedContentStale,
    /// "splice without match": when the pack refuses a pairing that doesn't fit, the kit
    /// falls back to the carrier-only hook (`Pack::encode_from`), which splices by guess.
    SpliceWithoutMatch,
    /// "pairing missing on version": a version cut takes the live carrier but not its
    /// pairing, so its file can't be paired (`file_at` is refused).
    PairingMissingOnVersion,
    /// "holds file calls pack": whether a keyed doc holds an outside save is decided by
    /// splicing the live content into the save's bytes, before the replica has taken the save,
    /// as before, instead of comparing keyed content.
    HoldsFileCallsPack,
    /// "dirty save merged by key": an outside save on a keyed doc that isn't a table,
    /// landing while the doc holds changes the file doesn't, is merged by key onto them, as
    /// before, instead of keeping both.
    DirtySaveMergedByKey,
    /// "settle before carrier": an outside save becomes the carrier only after the
    /// settle has rendered the doc that took it, as before, so that rendering splices into the
    /// bytes, and pairs with the keys, of the sync point before the save.
    SettleBeforeCarrier,
    /// "edit check dropped": a live change commits without the pack's check
    /// (`Pack::check_edit`), as before the hook, so a change the write-back can't write is
    /// taken.
    EditCheckDropped,
    /// "read replays render failure": a structured read verb returns the failure the last
    /// rendering found at the doc's version as its error, as before, instead of its result
    /// with a warning.
    ReadReplaysRenderFailure,
}

thread_local! {
    static KEYED_MUTANT: std::cell::Cell<Option<KeyedMutant>> = const { std::cell::Cell::new(None) };
}

/// The keyed fault planted on this thread, if any.
pub fn keyed_mutant() -> Option<KeyedMutant> {
    KEYED_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_keyed_mutant<R>(mutant: KeyedMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<KeyedMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            KEYED_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(KEYED_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in a merge, so the tests that should catch it can show they do. Installed per
/// thread by [`with_merge_mutant`]. Two families share it:
/// - `write --base`'s text merge (v0.9.8), caught by the trace classes ((i) and (ii) below;
///   `spec-undo`'s `traces` plants them by value);
/// - the grid's text merge (`table::merge3_text` and the outside save), so the row-pairing
///   property and its named tests can show they catch each one (mutants (xxxii) to (xxxvii):
///   contract §2.9.1, §3.4 and §4.3, spec S11/S13; selected in the tests by `MPC_MERGE_MUTANT`),
///   and the keyed grid merges (`table::merge3` and `table::merge3_outside`, mutants (xxxviii)
///   and (xxxix), caught by `tests/table.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MergeMutant {
    /// (i) v0.9.7's merge: sides diffed to the character on the texts as they are. Lines that
    /// share a start pair one line off, and two lines can join at the end of the text.
    CharSides,
    /// (ii) The final line break always as theirs left it, whoever wrote the last line.
    EndFollowsTheirs,
    /// (xxxii) An outside save's whole-row loop goes over the live side's live columns, so a
    /// column the file brings back keeps the live side's hidden value beside the file's row.
    OutsideWholeRowLiveCols,
    /// (xxxiii) The both-changed test by cell goes over the live side's live columns, missing a
    /// live write into a column the live side then deleted.
    ByCellLiveCols,
    /// (xxxiv) `write --base` conflicts on a live switch between empty and absent that changes
    /// no byte of the row (no live edit of the cell), as before the per-cell rule reached it.
    WriteBaseNeutralConflicts,
    /// (xxxv) A revert that rewrites a re-paired row whole (S13) restores it even when the doc has
    /// dropped it (S11 skipped): an engine refusal on an unknown key.
    WholeRowRestoresDropped,
    /// (xxxvi) An undo or redo of an op on a re-paired row the doc has dropped says
    /// `rewritten_row` (S13) instead of `dropped` (S11).
    RewrittenRowBeforeDropped,
    /// (xxxvii) A raw keyed cell write under a row or a column the table doesn't know (dropped,
    /// or never in it) is taken as a hidden write, as before the check: an orphan cell nobody
    /// can see, restore or sweep, reported with a restore that can't work.
    KeyedWriteUnderUnknownKey,
    /// (xxxviii) The per-cell neutral rule of `write --base` reaches the keyed merge
    /// (`table::merge3`, no text): theirs' clear of an absent cell in a row that renders every
    /// column is overwritten by ours' value with no conflict, as in v0.10.0.
    KeyedMergeNeutralSwitch,
    /// (xxxix) The outside save's per-cell neutral rule (a byte-neutral live switch between empty
    /// and absent, or a live value the file's row renders anyway) reaches the keyed
    /// `table::merge3_outside` (no text): the file's value goes in with no overridden `cell`
    /// reported, as in v0.10.0.
    KeyedOutsideNeutralSwitch,
}

thread_local! {
    static MERGE_MUTANT: std::cell::Cell<Option<MergeMutant>> = const { std::cell::Cell::new(None) };
}

/// The merge fault planted on this thread, if any.
pub fn merge_mutant() -> Option<MergeMutant> {
    MERGE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_merge_mutant<R>(mutant: MergeMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<MergeMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            MERGE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(MERGE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}
/// A fault planted at the seams between a grid's two line counts (docs/POSITIONS.md), so
/// `tests/grid_lines.rs` can show it catches each one. Installed per thread by
/// [`with_line_mutant`]; selected in the tests by `MPC_LINE_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineMutant {
    /// `LN` stops refusing on a grid holding a lone CR, and counts the line map as before.
    LnKeepsLineMap,
    /// `--ids` reads a printed line's `\n` number as a rendered line, as before.
    IdsCountNlLines,
    /// The lines a read reports (views, `--for-edit`, notes) and `claim --lines` reach the
    /// line map as they are, `\n` numbers read as rendered lines, as before.
    TouchedCountNlLines,
    /// `LN` goes on to the line map on a file that can't be rendered.
    LnGoesOnUnrendered,
    /// `claim --lines` reads the caller's numbers as rendered lines on a file that can't be
    /// rendered.
    ClaimLinesGoOnUnrendered,
    /// A conflict report gives rendered line numbers on a file that can't be rendered.
    ConflictLinesFallBack,
    /// `row list` prints rendered line numbers on a file holding a lone CR, as before.
    RowListKeepsLines,
    /// A conflict report gives rendered line numbers on a file holding a lone CR, as before.
    ConflictKeepsLines,
    /// A read verb that fails after the doc was read still counts as the caller's read, as
    /// before: it logs activity, and their next read loses who changed the doc.
    RefusedReadIsARead,
}

thread_local! {
    static LINE_MUTANT: std::cell::Cell<Option<LineMutant>> = const { std::cell::Cell::new(None) };
}

/// The line fault planted on this thread, if any.
pub fn line_mutant() -> Option<LineMutant> {
    LINE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_line_mutant<R>(mutant: LineMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<LineMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            LINE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(LINE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in a grid's render paths, so `tests/grid_scaling.rs` can show its bound
/// catches it (selected there by `MPC_GRID_COST_MUTANT`). Installed per thread by
/// [`with_grid_cost_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GridCostMutant {
    /// [`crate::table::Table::render`], [`crate::table::Table::line_map`] and a grid's blame
    /// visit every live column of every row, both keys cloned for each lookup (the earlier
    /// walk): their cost grows with rows × columns, blank rows included.
    PerColumn,
    /// A row's or column's place is found by a walk from the first place (as the vector the
    /// axis was), so a change that inserts or moves many rows costs rows × rows.
    AxisWalk,
    /// A merge's conflict listing finds each row's position, lines and changed cells by
    /// walking the axis, the line map and the live columns, per conflict.
    ConflictWalk,
    /// `row list` finds each row's lines by walking the line map, and builds every column's
    /// field as its own string with both keys cloned.
    RowListWalk,
    /// A live write from an older version (or an outside save) checks each element the live
    /// side dropped and the text kept by scanning every cell of both versions and walking
    /// both axes, per element.
    RekeyWalk,
}

thread_local! {
    static VISITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Elements visited on this thread by the table paths that count them (an axis's place
/// lookups and shifts, a position walk, a line map's scan): a timing-free cost for
/// `tests/grid_scaling.rs`.
pub fn visits() -> u64 {
    VISITS.with(|v| v.get())
}

pub(crate) fn visit(n: usize) {
    VISITS.with(|v| v.set(v.get().saturating_add(n as u64)));
}

thread_local! {
    static GRID_COST_MUTANT: std::cell::Cell<Option<GridCostMutant>> = const { std::cell::Cell::new(None) };
}

/// The grid cost fault planted on this thread, if any.
pub fn grid_cost_mutant() -> Option<GridCostMutant> {
    GRID_COST_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, however `f` ends.
pub fn with_grid_cost_mutant<R>(mutant: GridCostMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<GridCostMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            GRID_COST_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(GRID_COST_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault in element presence ([`crate::session::Session::set_selection`]), so its tests can
/// show they catch it (selected by `MPC_STRUCT_MUTANT` in `tests/elem_presence.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionMutant {
    /// A selection keeps the keys of elements that stopped being live.
    KeepsDeleted,
}

thread_local! {
    static SELECTION_MUTANT: std::cell::Cell<Option<SelectionMutant>> = const { std::cell::Cell::new(None) };
}

/// The selection fault planted on this thread, if any.
pub fn selection_mutant() -> Option<SelectionMutant> {
    SELECTION_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_selection_mutant<R>(mutant: SelectionMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<SelectionMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            SELECTION_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(SELECTION_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault in the kit items (the turn, claim events, focus and
/// range, the verb), so their tests can show they catch it (`tests/contract_presence.rs`,
/// `tests/contract_verbs.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContractMutant {
    /// A participant's erasure leaves the turns of their changes in place.
    TurnNotClearedOnErase,
    /// A claim's start leaves the claimant's selection as it was (Q19 says: replaced by the
    /// claimed keys).
    ClaimDoesNotReplaceSelection,
    /// Setting a text range leaves the participant's focus where it was (r1.3 says: the range's
    /// container).
    RangeDoesNotSetFocus,
    /// A change a pack verb made is stored without its verb (it belongs on the change record,
    /// so history and a reload show it).
    VerbNotRecorded,
    /// A pack verb's change moves the presence of a person who ran it too (the rule: the kit
    /// moves an agent's focus and selection only; a person sets their own).
    VerbMovesPersonPresence,
    /// A live revert or restore leaves the host-given turn off the change it makes (K2's ask:
    /// they take the host's turn, as every other write entry does).
    RevertDropsTurn,
}

thread_local! {
    static CONTRACT_MUTANT: std::cell::Cell<Option<ContractMutant>> = const { std::cell::Cell::new(None) };
}

/// The contract fault planted on this thread, if any.
pub fn contract_mutant() -> Option<ContractMutant> {
    CONTRACT_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_contract_mutant<R>(mutant: ContractMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ContractMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            CONTRACT_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(CONTRACT_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault in the view index ([`crate::view_index`]), so its
/// tests can show they catch it (`tests/view_index.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ViewMutant {
    /// The live builder ignores a field the schema names (a unit's `hidden`: every unit shown).
    SchemaFieldIgnored,
    /// The kit's inverse of an address ignores the pack's mapping and reads the address as a
    /// key, so the address it hands out doesn't lead back to the element.
    AddressNotInverse,
    /// The live index hands the derive hook no carrier, so what only the file's bytes hold
    /// is missing from it.
    DeriveIgnoresCarrier,
    /// The kept index is reused while the doc changes: its key is the carrier's id alone, not
    /// (carrier id, rev), so a read after a change gets the index from before it.
    DeriveCacheStale,
    /// A keyed watch's change frame carries no render hash of the containers the change
    /// re-rendered (the kit's, from the view index before and after it).
    FrameMissesRenderHash,
    /// The roster's element presence and claims go out with the live store's keys where the
    /// pack's addresses belong (`Session::roster`, the roster events).
    RosterKeysLeak,
    /// A keyed watch's change frame carries none of the units' derived values the change set
    /// or removed (`derived.units.<unit>.<name>`, from the view index before and after it).
    FrameMissesUnitDerived,
    /// The view index kept through a change keeps a unit the change touched as it was before
    /// (the kit builds it again only when it is new): a field the change wrote is stale.
    UpdateSkipsTouched,
    /// The view index kept through a change builds every unit again, touched or not: right,
    /// but the change costs the whole doc (the kept index's counts catch it).
    UpdateRebuildsAll,
    /// A test build doesn't fail on a container's layout that isn't a layout reference (the
    /// index still leaves it out and records it, as a release build does).
    LayoutUnchecked,
    /// A test build doesn't fail on a container's parent the index refused (the index still
    /// leaves it out and records it, as a release build does).
    ParentUnchecked,
    /// The kit carries a container's parent without checking what it names: one that names
    /// no container of the index, the container itself, or (live) is no key at all.
    ParentNotValidated,
    /// The kit doesn't break a cycle of containers' parents.
    ParentCycleAccepted,
    /// The kit doesn't bound how deep a container sits ([`crate::view_index::CONTAINER_DEPTH_MAX`]).
    ParentDepthUnbounded,
    /// The snapshot leaves a container's `parent` out.
    ParentDroppedOnWire,
    /// A keyed watch's change frame carries no container's `parent_after` (a container that
    /// came, or whose parent changed).
    FrameMissesParent,
    /// The index keeps a container's derived value whose name the schema doesn't declare
    /// ([`crate::view_index::ViewSchema::container_derived`]).
    ContainerDerivedUndeclaredKept,
    /// The index keeps a container's derived values in the order the pack set them, not the
    /// schema's.
    ContainerDerivedOrderLost,
    /// The snapshot leaves a container's `derived` out.
    ContainerDerivedDroppedOnWire,
    /// A keyed watch's change frame carries none of the containers' derived values the change
    /// set or removed (`derived.containers.<container>.<name>`).
    FrameMissesContainerDerived,
}

thread_local! {
    /// The carrier decodes the view index's derive hook ran on this thread
    /// ([`crate::view_index::ViewCarrier::decoded`]). A count, never a timing.
    static CARRIER_DECODES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One carrier decode by the derive hook ([`crate::view_index::ViewCarrier::decoded`]).
pub(crate) fn count_carrier_decode() {
    CARRIER_DECODES.with(|c| c.set(c.get() + 1));
}

/// This thread's carrier decodes since the last take, reset to 0: how many times a view index
/// read ran the pack's decode of a carrier ([`crate::view_index::ViewCarrier::decoded`]).
pub fn take_carrier_decodes() -> u64 {
    CARRIER_DECODES.with(|c| c.replace(0))
}

/// A fault in the 3D placement slot ([`crate::placement`]), so its rows show they catch it
/// (`tests/view_place.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaceMutant {
    /// A unit delta says `place` only when a location moved: a change of rotation or scale
    /// alone goes out with no `place` and no `placement_after`.
    PlaceDroppedOnRotation,
    /// The render hint doesn't name the containers of what is placed through a moved element.
    NoChainHint,
    /// An `F32` slot's numbers are printed as their `f64` digits.
    F32AsF64Digits,
    /// The kept index asks the pack for every unit's placement on every change.
    AskEveryUnit,
    /// A placement relative to nothing is read as relative to its unit's container.
    NoneIsContainerFrame,
    /// A chain's missing link (an element not shown or not placed) is taken as identity.
    MissingLinkIdentity,
    /// A unit that comes into view (`add`, `restore`) has no `placement_after`.
    NoPlacementOnAdd,
    /// A test build doesn't check the placements' chains for a cycle.
    CycleUnchecked,
}

thread_local! {
    static PLACE_MUTANT: std::cell::Cell<Option<PlaceMutant>> = const { std::cell::Cell::new(None) };
    /// The placements the kit asked a pack for on this thread (`Pack::view_placement`). A count.
    static PLACEMENT_ASKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The placement fault planted on this thread, if any.
pub fn place_mutant() -> Option<PlaceMutant> {
    PLACE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_place_mutant<R>(mutant: PlaceMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<PlaceMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            PLACE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(PLACE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// One placement asked of a pack.
pub(crate) fn count_placement_ask() {
    PLACEMENT_ASKS.with(|c| c.set(c.get() + 1));
}

/// This thread's placements asked of a pack since the last take, reset to 0.
pub fn take_placement_asks() -> u64 {
    PLACEMENT_ASKS.with(|c| c.replace(0))
}

thread_local! {
    /// The view index's units the kit built from the keyed state on this thread (a whole
    /// build builds each; a change kept builds the units it touched). A count, never a timing.
    static VIEW_UNIT_BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One unit of a live view index built from the keyed state.
pub(crate) fn count_view_unit_build() {
    VIEW_UNIT_BUILDS.with(|c| c.set(c.get() + 1));
}

/// This thread's view index units built since the last take, reset to 0: a whole build of a
/// doc of N units builds N; keeping the index through a change builds the units it touched.
pub fn take_view_unit_builds() -> u64 {
    VIEW_UNIT_BUILDS.with(|c| c.replace(0))
}

thread_local! {
    /// The carrier pairings the kit made on this thread (`Pack::pairing`). A count, never
    /// a timing.
    static PAIRINGS_MADE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The renderings an outside save's held check made on this thread. A count.
    static HELD_RENDERS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One pairing made by the kit.
pub(crate) fn count_pairing_made() {
    PAIRINGS_MADE.with(|c| c.set(c.get() + 1));
}

/// This thread's pairings made since the last take, reset to 0: how many times the kit
/// asked a pack for a carrier's pairing.
pub fn take_pairings_made() -> u64 {
    PAIRINGS_MADE.with(|c| c.replace(0))
}

/// One rendering made by an outside save's held check.
pub(crate) fn count_held_render() {
    HELD_RENDERS.with(|c| c.set(c.get() + 1));
}

/// This thread's held-check renderings since the last take, reset to 0: a keyed doc that
/// isn't a table makes none.
pub fn take_held_renders() -> u64 {
    HELD_RENDERS.with(|c| c.replace(0))
}

thread_local! {
    static VIEW_MUTANT: std::cell::Cell<Option<ViewMutant>> = const { std::cell::Cell::new(None) };
}

/// The view index fault planted on this thread, if any.
pub fn view_mutant() -> Option<ViewMutant> {
    VIEW_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_view_mutant<R>(mutant: ViewMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ViewMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            VIEW_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(VIEW_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault in planning a single list insert or delete from the engine's slot index
/// (`engine/structured.rs`), installed per thread by [`with_list_plan_mutant`], so its rows show
/// they catch it (`tests/list_plan.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListPlanMutant {
    /// A single insert after an anchor, planned from the index, lands one slot after where
    /// the full read puts it.
    FastInsertAfterOffByOne,
}

thread_local! {
    static LIST_PLAN_MUTANT: std::cell::Cell<Option<ListPlanMutant>> =
        const { std::cell::Cell::new(None) };
    /// Whether a single list insert or delete is planned from the engine's slot index (on by
    /// default, as in a host build); off, every list change is planned by the full read.
    static LIST_PLAN_FAST: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// The list plan fault planted on this thread, if any.
pub fn list_plan_mutant() -> Option<ListPlanMutant> {
    LIST_PLAN_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_list_plan_mutant<R>(mutant: ListPlanMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ListPlanMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            LIST_PLAN_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(LIST_PLAN_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// Whether a single list insert or delete is planned from the slot index on this thread.
pub fn list_plan_fast() -> bool {
    LIST_PLAN_FAST.with(|c| c.get())
}

/// Run `f` with the slot index's plan `on` or off on this thread (off: the full read plans
/// every list change, the reference the differential row compares with); restored afterwards,
/// even if `f` unwinds.
pub fn with_list_plan_fast<R>(on: bool, f: impl FnOnce() -> R) -> R {
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            LIST_PLAN_FAST.with(|c| c.set(self.0));
        }
    }
    let _back = Back(LIST_PLAN_FAST.with(|c| c.replace(on)));
    f()
}

/// A fault in keeping a pack's verb state (`session/live/verb_state.rs`), installed per thread
/// by [`with_verb_state_mutant`], so its rows show the kept state's checks catch it
/// (`tests/verb_state.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VerbStateMutant {
    /// An outside save doesn't run the pack's change hook, but the kept state is marked as in
    /// step with it: a verb after the save reads the state from before it.
    OutsideSaveSkipped,
}

thread_local! {
    static VERB_STATE_MUTANT: std::cell::Cell<Option<VerbStateMutant>> =
        const { std::cell::Cell::new(None) };
    /// The verb states the kit built to keep on this thread (not the checks' fresh builds).
    /// A count, never a timing.
    static VERB_STATE_BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The verb state fault planted on this thread, if any.
pub fn verb_state_mutant() -> Option<VerbStateMutant> {
    VERB_STATE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_verb_state_mutant<R>(mutant: VerbStateMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<VerbStateMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            VERB_STATE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(VERB_STATE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// One verb state built to keep.
pub(crate) fn count_verb_state_build() {
    VERB_STATE_BUILDS.with(|c| c.set(c.get() + 1));
}

/// This thread's verb states built to keep since the last take, reset to 0: one per doc the
/// first time a verb runs on it (and after a change the pack couldn't keep it through); the
/// checks' fresh builds don't count.
pub fn take_verb_state_builds() -> u64 {
    VERB_STATE_BUILDS.with(|c| c.replace(0))
}

/// A fault planted in what an agent gets back as data (`tools/run.rs`, `tools/result.rs`): a
/// pack's refusals and the call's result ([`crate::tools::ToolOutput::result`]), so the rows of
/// `tests/agent_results.rs` show they catch each one. Installed per thread by
/// [`with_ergo_mutant`], or named by `MPC_ERGO_MUTANT` for a whole test run (an unknown name
/// stops the run, naming it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErgoMutant {
    /// A pack's refusal is dropped as data: neither its registered entry nor the text form's
    /// stop-gap makes a report (`refusal: None`, as before the pack refusals were read).
    RefusalDropped,
    /// The result's changed units lose their addresses (`address: null`).
    UnitsDropAddress,
    /// The result leaves out the version the verb computed on (`version.base: null`).
    EnvelopeDropsBase,
    /// The result is written to stdout though the call didn't ask for it.
    JsonLeaksToText,
    /// The result leaves out the name of its typed data's shape (`data_spec: null`).
    DataSpecDropped,
    /// An edit's view is dropped from its result (`view: null`).
    ViewDropped,
    /// An edit's view takes the first container it touched, not the last.
    ViewFirstContainer,
    /// A unit the change deleted is listed live in its view.
    ViewDeletedLive,
    /// A person's verb moves their view address to its change.
    ViewMovesPerson,
    /// A text change's view takes the outermost named span holding it, not the innermost.
    ViewSectionOuter,
    /// A verb's own view address is taken unchecked by the grammar.
    ViewAddressUnchecked,
}

impl ErgoMutant {
    /// The mutant `MPC_ERGO_MUTANT` names: its name in kebab case.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "refusal-dropped" => Self::RefusalDropped,
            "units-drop-address" => Self::UnitsDropAddress,
            "envelope-drops-base" => Self::EnvelopeDropsBase,
            "json-leaks-to-text" => Self::JsonLeaksToText,
            "data-spec-dropped" => Self::DataSpecDropped,
            "view-dropped" => Self::ViewDropped,
            "view-first-container" => Self::ViewFirstContainer,
            "view-deleted-live" => Self::ViewDeletedLive,
            "view-moves-person" => Self::ViewMovesPerson,
            "view-section-outer" => Self::ViewSectionOuter,
            "view-address-unchecked" => Self::ViewAddressUnchecked,
            _ => return None,
        })
    }
}

thread_local! {
    static ERGO_MUTANT: std::cell::Cell<Option<ErgoMutant>> = const { std::cell::Cell::new(None) };
}

/// The agent-result fault planted on this thread, else the one `MPC_ERGO_MUTANT` names, if any.
pub fn ergo_mutant() -> Option<ErgoMutant> {
    ERGO_MUTANT.with(|m| m.get()).or_else(|| {
        let v = std::env::var("MPC_ERGO_MUTANT").ok()?;
        Some(
            ErgoMutant::named(&v)
                .unwrap_or_else(|| panic!("MPC_ERGO_MUTANT={v}: not an agent-result mutant")),
        )
    })
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_ergo_mutant<R>(mutant: ErgoMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ErgoMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            ERGO_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(ERGO_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the version an app opened (`session/opened.rs`, `note_opened`), so the
/// rows of `tests/opened.rs` show they catch each one. Installed per thread by
/// [`with_opened_mutant`], or named by `MPC_OPENED_MUTANT` for a whole test run (an unknown
/// name stops the run, naming it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenedMutant {
    /// The pin is kept in memory only: a restart forgets it.
    OpenedInMemory,
    /// The pin is used without checking its version is still held.
    PinUnchecked,
    /// An outside save taken in leaves the pin where it was.
    PinNeverMoves,
    /// `FileNow` pins the synced version without checking the file first.
    FileNowSkipsCheck,
    /// The write-back hold after a `FileNow` pin is ignored.
    HoldIgnored,
    /// An outside save merges as if there were no pin.
    OpenedIgnored,
    /// With no pin, a save over the kit's write merges from the synced version as if it were
    /// known.
    BaseIsAlwaysKnown,
    /// A keyed doc's field merge takes a clash as the save's value.
    ClashMerged,
    /// A keyed doc's field merge isn't checked by the pack.
    ViewCheckSkipped,
    /// A save's own stamp is ignored.
    StampIgnored,
    /// A text doc's outside save merges from the synced text though a pin is noted (the pin
    /// is otherwise kept and checked).
    TextBaseSynced,
    /// A text doc's outside save merged from the version the app opened reports no kept
    /// pairs.
    KeptPairsDropped,
}

impl OpenedMutant {
    /// The mutant `MPC_OPENED_MUTANT` names: its name in kebab case.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "opened-in-memory" => Self::OpenedInMemory,
            "pin-unchecked" => Self::PinUnchecked,
            "pin-never-moves" => Self::PinNeverMoves,
            "file-now-skips-check" => Self::FileNowSkipsCheck,
            "hold-ignored" => Self::HoldIgnored,
            "opened-ignored" => Self::OpenedIgnored,
            "base-is-always-known" => Self::BaseIsAlwaysKnown,
            "clash-merged" => Self::ClashMerged,
            "view-check-skipped" => Self::ViewCheckSkipped,
            "stamp-ignored" => Self::StampIgnored,
            "text-base-synced" => Self::TextBaseSynced,
            "kept-pairs-dropped" => Self::KeptPairsDropped,
            _ => return None,
        })
    }
}

thread_local! {
    static OPENED_MUTANT: std::cell::Cell<Option<OpenedMutant>> =
        const { std::cell::Cell::new(None) };
}

/// The opened-pin fault planted on this thread, else the one `MPC_OPENED_MUTANT` names, if any.
pub fn opened_mutant() -> Option<OpenedMutant> {
    OPENED_MUTANT.with(|m| m.get()).or_else(|| {
        let v = std::env::var("MPC_OPENED_MUTANT").ok()?;
        Some(
            OpenedMutant::named(&v)
                .unwrap_or_else(|| panic!("MPC_OPENED_MUTANT={v}: not an opened-pin mutant")),
        )
    })
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_opened_mutant<R>(mutant: OpenedMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<OpenedMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            OPENED_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(OPENED_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A keyed watch's planted fault (`session/live/watch.rs`), installed per thread by
/// [`with_watch_mutant`], so the watch's rows show they catch it (`tests/keyed_watch.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchMutant {
    /// After a snapshot at `upto`, a change still queued at exactly `upto` goes out as well (the
    /// comparison is `<`, not `<=`): the watcher applies it twice.
    SnapshotOffByOne,
    /// A unit delta's geometry after is its geometry before (`crate::view_delta`): a watcher
    /// keeps the old box.
    DeltaGeomStale,
}

thread_local! {
    static WATCH_MUTANT: std::cell::Cell<Option<WatchMutant>> = const { std::cell::Cell::new(None) };
}

/// The watch fault planted on this thread, if any.
pub fn watch_mutant() -> Option<WatchMutant> {
    WATCH_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_watch_mutant<R>(mutant: WatchMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<WatchMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            WATCH_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(WATCH_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted at the apply of a structured change (`apply_structured`, inside `commit`), so
/// the tests of what a change leaves behind can drive an exit no public call reaches
/// (`session/live/next_change_tests.rs`). Installed per thread by [`with_apply_fault`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApplyFault {
    /// The apply refuses the change, and the doc stays trusted.
    Refused,
    /// The apply fails with an error that poisons the doc, so `commit` reloads it (`recover`).
    Poisoned,
    /// The apply reports that the change changed nothing, so `commit` returns `Ok`.
    NoChange,
    /// In `commit`, a step after one that applied is refused, so `commit` reloads the doc.
    LaterStepRefused,
    /// `commit`'s store transaction fails, without damaging the store, so `commit` reloads the doc
    /// and the session goes on.
    StoreCommitFails,
    /// The bookkeeping of the structured change this many changes on fails, after the doc took it.
    BookkeepingFailsAfter(u32),
}

thread_local! {
    static APPLY_FAULT: std::cell::Cell<Option<ApplyFault>> = const { std::cell::Cell::new(None) };
}

/// The apply fault planted on this thread, if any.
pub fn apply_fault() -> Option<ApplyFault> {
    APPLY_FAULT.with(|m| m.get())
}

/// Whether the bookkeeping of this structured change fails ([`ApplyFault::BookkeepingFailsAfter`]);
/// one change closer to it otherwise.
pub(crate) fn take_bookkeeping_fault() -> bool {
    APPLY_FAULT.with(|m| match m.get() {
        Some(ApplyFault::BookkeepingFailsAfter(0)) => true,
        Some(ApplyFault::BookkeepingFailsAfter(n)) => {
            m.set(Some(ApplyFault::BookkeepingFailsAfter(n - 1)));
            false
        }
        _ => false,
    })
}

/// Run `f` with `fault` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_apply_fault<R>(fault: ApplyFault, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ApplyFault>);
    impl Drop for Back {
        fn drop(&mut self) {
            APPLY_FAULT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(APPLY_FAULT.with(|m| m.replace(Some(fault))));
    f()
}

/// A fault planted in when a claim is marked as edited (`tests/claim_marks.rs`, selected by
/// `MPC_STRUCT_MUTANT`). Installed per thread by [`with_mark_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MarkMutant {
    /// A grid claim is marked from what a change names, before it applies.
    GridMarkBeforeLanding,
    /// Every text claim and reservation of the member on a root the change names is marked first
    /// thing, whatever the change does.
    TextMarkFirst,
    /// A reload after a failed change leaves the claims in place, marks and all.
    RecoverKeepsClaims,
}

thread_local! {
    static MARK_MUTANT: std::cell::Cell<Option<MarkMutant>> = const { std::cell::Cell::new(None) };
}

/// The claim mark fault planted on this thread, if any.
pub fn mark_mutant() -> Option<MarkMutant> {
    MARK_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_mark_mutant<R>(mutant: MarkMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<MarkMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            MARK_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(MARK_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in what follows a failed render or re-sync of the disk replica
/// (`tests/resync_failure.rs`, selected by `MPC_STRUCT_MUTANT`). Installed per thread by
/// [`with_resync_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResyncMutant {
    /// The change's commit returns the error, though the change is in the store.
    ReturnsTheError,
    /// A purge stops at its re-sync, with the live removal landed and the history not erased.
    PurgeStopsAtTheResync,
    /// A failure after the doc took an outside save leaves the doc holding it, unlogged.
    IngestKeepsTheDoc,
    /// A failure settling after an outside save returns the error, though the save is in the
    /// store.
    SettleReturnsTheError,
}

thread_local! {
    static RESYNC_MUTANT: std::cell::Cell<Option<ResyncMutant>> = const { std::cell::Cell::new(None) };
}

/// The re-sync fault planted on this thread, if any.
pub fn resync_mutant() -> Option<ResyncMutant> {
    RESYNC_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_resync_mutant<R>(mutant: ResyncMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ResyncMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            RESYNC_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(RESYNC_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in client-ID handling in sessions, so its tests can show they catch it.
/// Installed per thread by [`with_client_mutant`]; selected in the tests by
/// `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientMutant {
    /// Agent reuse ignores the kinds the store records (`client_kinds`): every ID from before
    /// the session's load counts as reusable (an own replica's and an older kit's included),
    /// whatever its kind (caught by `tests/client_reuse.rs`'s own-replica and N−1 tests).
    ReuseAcrossReload,
    /// Agent reuse reads no recorded kinds at load: only IDs minted in this load are reused,
    /// so a plain agent draws a new ID after every reload (caught by
    /// `tests/client_reuse.rs`'s plain-agent test).
    NoReuseAfterReload,
    /// An open leaves the `client_kinds` rows of docs that have been deleted (caught by
    /// `store::tests::an_open_removes_the_client_kinds_of_docs_an_older_kit_deleted` and
    /// tools/n1's `n1_kinds_after_delete`).
    NoOrphanSweep,
    /// Person-writer reuse ignores the writers present connections hold, so a second tab of
    /// the same person and device shares the first one's (caught by `tests/person_writers.rs`'s
    /// `r1_two_tabs`, R9).
    ReusePersonHeld,
    /// Person-writer reuse takes a parked writer (its connection ended without a release) for
    /// another connection's plain join, with no expiry (caught by `tests/person_writers.rs`'s
    /// `r3_crash_with_pending_edits`, R11).
    ReuseParked,
    /// A reclaim of a writer a present connection holds hands it over without closing that
    /// connection (caught by `tests/person_writers.rs`'s `r2_reconnect_half_open`, R13).
    HandoverWithoutClose,
    /// A reclaim whose lease is stale may get its own old writer back, not kept, when that
    /// writer is free again (caught by `tests/person_writers.rs`'s
    /// `r5_stale_reclaim_never_takes_its_writer_back_unkept`).
    StaleReclaimTakesOwn,
    /// An agent that gains edit rights after a read-only join gets no kind row, so its ID is
    /// never reused (caught by `tests/client_reuse.rs`'s
    /// `an_agent_gaining_edit_after_a_read_only_join_reuses_its_id`).
    NoAgentRowOnGain,
    /// A stale reclaim after the park expiry isn't told the version its lease ended at
    /// (caught by `tests/person_writers.rs`'s `r5_stale_lease_after_expiry` and
    /// `r5_stale_reclaim_never_takes_its_writer_back_unkept`).
    ExpiryWithoutCap,
    /// A load leaves a lease whose end was never written (the process ended) as it was, so its
    /// writer never expires and its reclaim isn't told when it ended (caught by
    /// `tests/person_writers.rs`'s `r4_ended_process_then_expiry_tells_the_loaded_end`).
    NoLoadEnd,
    /// A parked writer is never let go, whatever `Config::person_writer_park_ms` says. Caught by
    /// `tests/person_writers.rs`, each at its row assert:
    /// - `r5_stale_lease_after_expiry`, `r5_returning_tab_after_expiry_lands_its_hunks_once`,
    ///   `r5_stale_reclaim_never_takes_its_writer_back_unkept`: the expired writer is never free
    ///   for the device's next tab;
    /// - `r4_ended_process_then_expiry_tells_the_loaded_end`: the writer a process left leased
    ///   never goes to the next tab;
    /// - `r8_long_history_with_expiry`: the history holds as many writers as with no expiry;
    /// - `r12_tag_keys_across_hand_over`: the expired writer never goes to the sibling;
    /// - `soak_reconnects_one_person_ids_bounded`: its expiry arm (every end unclean, no
    ///   reclaim) takes a new writer each round.
    ExpiryIgnored,
    /// A join never checks `Config::writers_max`: a connection that could write takes a new
    /// writer past the cap (caught by `tests/writers_cap.rs`'s join rows and
    /// `tests/person_writers.rs`'s `soak_reconnects_one_person_ids_bounded`).
    CapUnchecked,
    /// At the cap, a person's join takes that person's newest parked writer, not the oldest
    /// (caught by `tests/writers_cap.rs`'s `cap_takes_the_persons_free_then_oldest_parked`).
    CapTakesNewestParked,
    /// The outside participant's first writer is refused at the cap, so an outside save is
    /// refused (caught by `tests/writers_cap.rs`'s `cap_exempts_the_outside_save_and_the_import`).
    CapOnExternal,
    /// The cap is checked only at a join: a connection that gains edit rights later edits as a
    /// writer the doc didn't count (caught by `tests/writers_cap.rs`'s `cap_counts_a_rights_gain`).
    CapJoinOnly,
    /// A tier-0 promotion whose history has more authors than the cap is refused (caught by
    /// `tests/writers_cap.rs`'s `cap_promotion_past_it_is_allowed_and_counted`).
    CapOnPromotion,
    /// The near-cap notice comes only with a cap, never against `WRITERS_NOTICE_MAX` (caught by
    /// `tests/writers_cap.rs`'s `near_cap_notice_counts_against_8192_without_a_cap`).
    NoNearCapNotice,
    /// A connection watching past `Config::participants_max` with edit rights takes and counts
    /// a writer at its join, as one that could write (caught by `tests/writers_cap.rs`'s
    /// `cap_a_watcher_holds_no_counted_writer`).
    WatcherCountsWriter,
    /// A reclaim never takes its own writer back. Caught by `tests/person_writers.rs`, each at
    /// its row assert:
    /// - `r2_reconnect_half_open`: the reconnect's reclaim of its held writer isn't kept;
    /// - R3, a tab whose connection dropped with edits unsent: its reclaim of its parked writer
    ///   isn't kept;
    /// - `r4_offline_device_after_reload`: the offline device's reclaim after a reload isn't kept;
    /// - `r12_tag_keys_across_hand_over`: the hand-over to another tab isn't kept;
    /// - `r8_long_history`, `r8_long_history_with_expiry`: every reconnect takes a new writer, so
    ///   the history passes the reuse bound;
    /// - `soak_reconnects_one_person_ids_bounded`: its reclaim arms take a new writer each round.
    ReclaimNeverKeeps,
}

thread_local! {
    static CLIENT_MUTANT: std::cell::Cell<Option<ClientMutant>> = const { std::cell::Cell::new(None) };
}

/// The client-ID fault planted on this thread, if any.
pub fn client_mutant() -> Option<ClientMutant> {
    CLIENT_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_client_mutant<R>(mutant: ClientMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ClientMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            CLIENT_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(CLIENT_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in tagged-key minting ([`crate::mint`]), so the minting tests can show they
/// catch it. Installed per thread by [`with_mint_mutant`]; selected in the tests by
/// `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MintMutant {
    /// [`crate::mint::next_for`] ignores the retired high-water mark, so a key a drop forgot
    /// is minted again (caught by "drop, then mint").
    IgnoreRetired,
    /// The session's writer-tag rule takes any writer's tag, not only the inserting
    /// connection's own (caught by "an insert with another's tag is refused").
    AnyWriterTag,
    /// [`crate::mint::mark_key`] is one shared key per prefix and tag, not one per dropper:
    /// last writer wins, so of two concurrent drops the lower mark can win (caught by the
    /// two-concurrent-drops convergence test).
    SingleLwwMark,
    /// The session's client-ID helper hands out a draw of 0 instead of drawing again (caught
    /// by the client-ID resampling test).
    ClientIdZero,
    /// The client-ID helper moves a draw outside `2..=MAX_KIT_CLIENT` into it (the old
    /// `.max(2)`), so several draws map onto one ID (caught by "a draw of 0 then 7 gives 7").
    ShiftNotResample,
    /// Agent-ID reuse ignores the IDs present connections hold, so two live connections share
    /// one (caught by the join-and-leave churn test).
    ReuseHeldClient,
}

thread_local! {
    static CLIENT_DRAWS: RefCell<std::collections::VecDeque<ClientId>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
}

/// Run `f` with the session's next raw client-ID draws forced to `draws`, in order (any
/// values, 0 and 2^53 - 1 included), so a test can show which the kit draws again; the usual
/// source takes over when they run out, and any left are discarded afterwards.
pub fn with_client_draws<R>(draws: impl IntoIterator<Item = ClientId>, f: impl FnOnce() -> R) -> R {
    struct Back;
    impl Drop for Back {
        fn drop(&mut self) {
            CLIENT_DRAWS.with(|d| d.borrow_mut().clear());
        }
    }
    CLIENT_DRAWS.with(|d| {
        let mut d = d.borrow_mut();
        d.clear();
        d.extend(draws);
    });
    let _back = Back;
    f()
}

/// The next forced raw client-ID draw on this thread ([`with_client_draws`]), if any.
pub(crate) fn forced_client_draw() -> Option<ClientId> {
    CLIENT_DRAWS.with(|d| d.borrow_mut().pop_front())
}

thread_local! {
    static MINT_MUTANT: std::cell::Cell<Option<MintMutant>> = const { std::cell::Cell::new(None) };
}

/// The minting fault planted on this thread, if any.
pub fn mint_mutant() -> Option<MintMutant> {
    MINT_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_mint_mutant<R>(mutant: MintMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<MintMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            MINT_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(MINT_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in element claims, so their tests can show they catch it. Installed per
/// thread by [`with_claim_mutant`]; selected in the tests by `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClaimMutant {
    /// The claim policy reads a change's list ops only: a map write isn't mapped to its element
    /// through [`crate::session::Pack::composite`], so it passes a claim on that element (caught
    /// by "a write to a claimed element's map key is refused").
    MapKeysUnmapped,
    /// An element claim is marked from what a change names, before it applies, so a change that
    /// lands nothing leaves the mark, and the next change by anyone ends the claim (caught by "a
    /// change that doesn't land leaves the claim alone").
    MarkBeforeApply,
    /// An element claim whose last live element an outside save removed ends
    /// [`crate::session::ClaimEnd::Deleted`], as a live delete does, not
    /// [`crate::session::ClaimEnd::GoneOutsideSave`].
    OutsideEndReportedDeleted,
    /// A claim's end events carry no element addresses (`addresses` empty).
    EndAddressesDropped,
}

thread_local! {
    static CLAIM_MUTANT: std::cell::Cell<Option<ClaimMutant>> = const { std::cell::Cell::new(None) };
}

/// The element-claim fault planted on this thread, if any.
pub fn claim_mutant() -> Option<ClaimMutant> {
    CLAIM_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, however `f` ends.
pub fn with_claim_mutant<R>(mutant: ClaimMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ClaimMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            CLAIM_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(CLAIM_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the reference-state helper ([`crate::refs`]), so its tests can show they
/// catch it. Installed per thread by [`with_ref_mutant`]; selected in the tests by
/// `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefMutant {
    /// [`crate::refs::resolve_all`] leaves orphaned references out (caught by "every link
    /// keeps its entry").
    OrphanDropsLink,
    /// [`crate::refs::resolve_parents`] cuts a cycle at its first element in the list's order,
    /// not its smallest key (caught by "the same parents in another order cut the same way").
    CycleByOrder,
}

thread_local! {
    static REF_MUTANT: std::cell::Cell<Option<RefMutant>> = const { std::cell::Cell::new(None) };
}

/// The reference-state fault planted on this thread, if any.
pub fn ref_mutant() -> Option<RefMutant> {
    REF_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_ref_mutant<R>(mutant: RefMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<RefMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            REF_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(REF_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in the doc's blob references (`store/blob_refs.rs`), so the tests that should
/// catch it can show they do. Installed per thread by [`with_blob_ref_mutant`]; selected in the
/// tests by `MPC_STRUCT_MUTANT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlobRefMutant {
    /// The open's orphan sweep leaves the last table on its list (caught by
    /// `store::tests::the_orphan_sweep_covers_every_table_on_its_list`).
    SweepSkipsTable,
    /// A change's references are taken in a second transaction after its own commit, so an
    /// interrupted write leaves a value naming a blob with no reference (caught by
    /// `session::blob_ref_tests::an_interrupted_write_leaves_the_change_and_its_reference_together`).
    RefAfterCommit,
    /// A load takes no references for the live doc (caught by
    /// `session::blob_ref_tests::a_load_takes_a_hold_for_a_name_an_older_kit_wrote`).
    NoHealAtLoad,
    /// Blob GC ignores a name on record with no count taken, so a blob that arrives after its
    /// name is collected while the doc names it (caught by
    /// `session::blob_ref_tests::a_blob_that_arrives_after_its_name_is_never_collected`).
    GcIgnoresNames,
    /// An edit checked for a name, refused once it reaches the engine, leaves its strictness for
    /// the next change, so an undo or restore naming a missing blob is refused (caught by
    /// `session::blob_ref_tests::a_strict_edit_leaves_nothing_behind_for_the_next_change`).
    StaleStrictFlag,
    /// `Session::blob_refs` returns every row whatever the caller's rights (caught by
    /// `session::blob_ref_tests::without_the_history_right_blob_refs_shows_what_a_read_shows`).
    RefsIgnoreRights,
}

thread_local! {
    static BLOB_REF_MUTANT: std::cell::Cell<Option<BlobRefMutant>> = const { std::cell::Cell::new(None) };
}

/// The blob-reference fault planted on this thread, if any.
pub fn blob_ref_mutant() -> Option<BlobRefMutant> {
    BLOB_REF_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_blob_ref_mutant<R>(mutant: BlobRefMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<BlobRefMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            BLOB_REF_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(BLOB_REF_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// How an edit becomes an engine change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Path {
    /// The production path (v0.9.3): the edit made on the doc itself as the client
    /// (`transact_mut_as`), with the sampled canary against the throwaway replica.
    #[default]
    MainDoc,
    /// The path before v0.9.3, kept for the differential test: a throwaway replica of the
    /// doc's full current state, with the editing client's ID, edited, encoded and dropped;
    /// the doc applies the update.
    Throwaway,
    /// Mutant: hunks applied front to back. Same text; insertions take their clocks in
    /// another order, so a change inserting at two places has other bytes.
    MutantFrontToBack,
    /// Mutant: each client edits on a replica kept from its first edit and never synced
    /// after (a stale base, or a kept replica that missed updates).
    MutantStale,
}

/// One edit as a side made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRecord {
    pub client: ClientId,
    /// The client's clock in the doc edited, before the edit.
    pub clock_before: u64,
    /// The update, or the error the path returned.
    pub update: Result<Vec<u8>, DocError>,
    /// The doc's full state after it (v2; `None` when the doc is poisoned).
    pub state: Option<Vec<u8>>,
    /// The doc's state vector after it, sorted by client.
    pub state_vector: Vec<(ClientId, u32)>,
}

/// One side of a comparison: see the module docs.
#[derive(Default)]
pub struct Side {
    path: Path,
    shadow: Option<Path>,
    /// Next deterministic client ID seed (`None`: the kit's random IDs).
    clients: Option<u64>,
    record: bool,
    edits: Vec<EditRecord>,
    shadow_checks: u64,
    kept: HashMap<ClientId, yrs::Doc>,
    /// Plant a divergence in the production path (the canary's test).
    plant: bool,
    /// File write-backs reload the disk replica from a whole encode of the doc, as before
    /// v0.9.7 (the write-back's differential test).
    full_writeback: bool,
    /// A Yrs text doc's disk replica kept decoded, as before v0.23 (the rows that compare the
    /// text-only replica with it, and the catch-up's own rows).
    decoded_replica: bool,
    /// The engine keeps its own text cache (a Yrs doc's), as before v0.24 (the rows that
    /// compare the session-owned texts with it).
    engine_text_cache: bool,
    /// The engine's position index off: every lookup walks from the text's start, as before
    /// k2.3 (the soak's diagnosis, v0.9.7).
    no_position_index: bool,
}

impl Side {
    /// A side whose edits take `path`.
    pub fn new(path: Path) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }

    /// Also make each edit through `reference` on a twin of the doc, and panic on any
    /// difference (update bytes, state bytes, state vector, texts).
    pub fn shadowed_by(self, reference: Path) -> Self {
        Self {
            shadow: Some(reference),
            ..self
        }
    }

    /// Mint client IDs from `seed` (deterministic), not at random.
    pub fn with_client_seed(self, seed: u64) -> Self {
        Self {
            clients: Some(seed),
            ..self
        }
    }

    /// Plant a divergence in the production path ([`Path::MainDoc`]): its edits apply each
    /// root's hunks front to back (the same text, other bytes for an edit inserting at two
    /// places), which the canary must catch.
    pub fn planting_divergence(self) -> Self {
        Self {
            plant: true,
            ..self
        }
    }

    /// The engine's position index off for every doc made while this side is installed:
    /// each lookup walks from the text's start, as before k2.3 (a diagnosis switch).
    pub fn no_position_index(self) -> Self {
        Self {
            no_position_index: true,
            ..self
        }
    }

    /// File write-backs re-encode the whole doc and reload the disk replica from it, as
    /// before v0.9.7, instead of keeping the replica in step (the differential test's
    /// reference side).
    pub fn full_writeback(self) -> Self {
        Self {
            full_writeback: true,
            ..self
        }
    }

    /// A Yrs text doc opened while this side is installed keeps its disk replica decoded, as
    /// before v0.23, instead of its texts only ([`with_decoded_replica`]).
    pub fn decoded_replica(self) -> Self {
        Self {
            decoded_replica: true,
            ..self
        }
    }

    /// A doc loaded while this side is installed keeps the engine's own text cache, as before
    /// v0.24, instead of the session owning its texts ([`with_engine_text_cache`]).
    pub fn engine_text_cache(self) -> Self {
        Self {
            engine_text_cache: true,
            ..self
        }
    }

    /// Keep an [`EditRecord`] of every edit (its state is a full encode: tests only).
    pub fn recording(self) -> Self {
        Self {
            record: true,
            ..self
        }
    }

    pub fn path(&self) -> Path {
        self.path
    }

    /// The edits recorded since the last call.
    pub fn take_edits(&mut self) -> Vec<EditRecord> {
        std::mem::take(&mut self.edits)
    }

    /// How many edits were checked against the shadow path.
    pub fn shadow_checks(&self) -> u64 {
        self.shadow_checks
    }
}

thread_local! {
    static SIDE: RefCell<Option<Side>> = const { RefCell::new(None) };
    /// The installed side's `plant` (read inside an edit, while `SIDE` is borrowed).
    static PLANT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The installed side's `full_writeback`.
    static FULL_WB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The installed side's `no_position_index`.
    static NO_POSINDEX: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The installed side's `decoded_replica`, or [`with_decoded_replica`]'s.
    static DECODED_REPLICA: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The installed side's `engine_text_cache`, or [`with_engine_text_cache`]'s.
    static ENGINE_TEXT_CACHE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a doc loaded now keeps the engine's own text cache, as before v0.24.
pub(crate) fn engine_text_cache_on() -> bool {
    ENGINE_TEXT_CACHE.with(|p| p.get())
}

/// Run `f` with every doc loaded on this thread keeping the engine's own text cache (a Yrs
/// doc's copy of each text beside the session's), as before v0.24: the reference side of the
/// rows that compare the session-owned texts with it. Removed afterwards, however `f` ends.
pub fn with_engine_text_cache<R>(f: impl FnOnce() -> R) -> R {
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            ENGINE_TEXT_CACHE.with(|p| p.set(self.0));
        }
    }
    let _back = Back(ENGINE_TEXT_CACHE.with(|p| p.replace(true)));
    f()
}

/// Whether a Yrs text doc attached to its file now keeps its disk replica decoded, as before
/// v0.23 (the installed side's `decoded_replica`, or inside [`with_decoded_replica`]).
pub(crate) fn decoded_replica_on() -> bool {
    DECODED_REPLICA.with(|p| p.get())
}

/// Run `f` with every Yrs text doc attached to its file on this thread keeping its disk
/// replica decoded, as before v0.23 (a whole Yrs doc beside the live one, brought forward by
/// the changes since each sync), instead of the file's texts only: the reference side of the
/// rows that compare the two, and the catch-up's own rows. Removed afterwards, however `f`
/// ends (an unwinding `f` too).
pub fn with_decoded_replica<R>(f: impl FnOnce() -> R) -> R {
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            DECODED_REPLICA.with(|p| p.set(self.0));
        }
    }
    let _back = Back(DECODED_REPLICA.with(|p| p.replace(true)));
    f()
}

/// Whether the installed side writes back files the old way.
pub(crate) fn full_writeback() -> bool {
    FULL_WB.with(|p| p.get())
}

thread_local! {
    static REOPEN_BY_VERSION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a reopen claims the stored disk replica in step with the doc from the sync point's
/// version alone (the fault [`with_reopen_step_by_version`] plants).
pub(crate) fn reopen_step_by_version() -> bool {
    REOPEN_BY_VERSION.with(|p| p.get())
}

/// Run `f` with every reopen on this thread claiming the stored disk replica in step with the
/// doc from the sync point's version alone, as before v0.10.0.3: the stale merge base the
/// first sync after a reopen then reports as `AuditEvent::DiskReplicaRepaired` and reloads.
/// Removed afterwards, even if `f` panics.
pub fn with_reopen_step_by_version<R>(f: impl FnOnce() -> R) -> R {
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            REOPEN_BY_VERSION.with(|p| p.set(self.0));
        }
    }
    let _back = Back(REOPEN_BY_VERSION.with(|p| p.replace(true)));
    f()
}

/// Whether the installed side turns the engine's position index off.
pub(crate) fn position_index_off() -> bool {
    NO_POSINDEX.with(|p| p.get())
}

/// Whether the installed side plants a divergence in the production path.
pub(crate) fn planted_divergence() -> bool {
    PLANT.with(|p| p.get())
}

/// Run `f` with `side` installed on this thread (every edit `f` makes, directly or through a
/// session, takes its path). The side is handed back afterwards, even if `f` panics.
pub fn with_side<R>(side: &mut Side, f: impl FnOnce() -> R) -> R {
    struct Back<'a>(&'a mut Side, bool, bool);
    impl Drop for Back<'_> {
        fn drop(&mut self) {
            PLANT.with(|p| p.set(false));
            FULL_WB.with(|p| p.set(false));
            NO_POSINDEX.with(|p| p.set(false));
            // back to what it was (an enclosing `with_decoded_replica` keeps it on)
            DECODED_REPLICA.with(|p| p.set(self.1));
            ENGINE_TEXT_CACHE.with(|p| p.set(self.2));
            if let Some(s) = SIDE.with(|t| t.borrow_mut().take()) {
                *self.0 = s;
            }
        }
    }
    PLANT.with(|p| p.set(side.plant));
    FULL_WB.with(|p| p.set(side.full_writeback));
    NO_POSINDEX.with(|p| p.set(side.no_position_index));
    let decoded = DECODED_REPLICA.with(|p| p.replace(side.decoded_replica || p.get()));
    let cache = ENGINE_TEXT_CACHE.with(|p| p.replace(side.engine_text_cache || p.get()));
    let prev = SIDE.with(|t| t.borrow_mut().replace(std::mem::take(side)));
    assert!(prev.is_none(), "with_side: a side is already installed");
    let _back = Back(side, decoded, cache);
    f()
}

/// A deterministic client ID when the installed side mints them, else `None`.
pub(crate) fn next_client() -> Option<ClientId> {
    SIDE.with(|t| {
        let mut t = t.borrow_mut();
        let seed = t.as_mut()?.clients.as_mut()?;
        loop {
            // splitmix64
            *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            let c = (z ^ (z >> 31)) & MAX_CLIENT_ID;
            if c > 1 {
                return Some(c);
            }
        }
    })
}

/// Make `plans` as `client` on `doc` through `path`; on success the doc holds the change.
fn make_with(
    doc: &mut YrsDoc,
    path: Path,
    client: ClientId,
    plans: &[TextPlan],
    kept: &mut HashMap<ClientId, yrs::Doc>,
) -> Result<Vec<u8>, DocError> {
    let update = match path {
        // applies the change itself
        Path::MainDoc => return doc.checked_change(client, plans),
        Path::Throwaway => doc.throwaway_update(client, plans)?,
        Path::MutantFrontToBack => doc.mutant_front_to_back(client, plans)?,
        Path::MutantStale => doc.mutant_stale(client, plans, kept)?,
    };
    doc.apply_trusted(&update)?;
    Ok(update)
}

/// The engine's edit, through the installed side's path (today's path when none is).
pub(crate) fn make(
    doc: &mut YrsDoc,
    client: ClientId,
    plans: &[TextPlan],
) -> Result<Vec<u8>, DocError> {
    SIDE.with(|t| {
        let mut t = t.borrow_mut();
        let Some(side) = t.as_mut() else {
            return make_with(doc, Path::MainDoc, client, plans, &mut HashMap::new());
        };
        let clock_before = doc.clock_of(client);
        let out = match side.shadow {
            None => make_with(doc, side.path, client, plans, &mut side.kept),
            Some(reference) => {
                let mut twin = doc.twin();
                let want = match twin.as_mut() {
                    Ok(tw) => make_with(tw, reference, client, plans, &mut HashMap::new()),
                    Err(e) => Err(e.clone()),
                };
                let got = make_with(doc, side.path, client, plans, &mut side.kept);
                assert_eq!(
                    got, want,
                    "edit paths differ: the update of {:?} (left) against {reference:?} \
                     (right), client {client}",
                    side.path
                );
                if let (Ok(_), Ok(tw)) = (&got, &twin) {
                    assert_eq!(
                        doc.snapshot_for_tests(),
                        tw.snapshot_for_tests(),
                        "edit paths differ: the doc's state after an edit by {client} \
                         through {:?} (left) and {reference:?} (right)",
                        side.path
                    );
                    assert_eq!(
                        doc.texts_for_tests(),
                        tw.texts_for_tests(),
                        "edit paths differ: the texts"
                    );
                }
                side.shadow_checks += 1;
                got
            }
        };
        if side.record {
            let (state, state_vector) = doc.snapshot_for_tests();
            side.edits.push(EditRecord {
                client,
                clock_before,
                update: out.clone(),
                state,
                state_vector,
            });
        }
        out
    })
}

/// The run record's `job` field (`tools/evidence/`): the job id K2 Compute's nodes set in
/// `K2_COMPUTE_JOB`, as a JSON string, or `null` off a node (then the evidence collector fills it in).
/// Only `[A-Za-z0-9_-]` is taken, so the record stays valid JSON.
pub fn runrec_job() -> String {
    match std::env::var("K2_COMPUTE_JOB") {
        Ok(j)
            if !j.is_empty()
                && j.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') =>
        {
            format!("\"{j}\"")
        }
        _ => "null".to_owned(),
    }
}

/// Bugs a test can plant in the engine's position index (v0.9.7), to prove the kit reports
/// what its shadow check finds. Only with the test-only `planted-position-bugs` feature.
#[cfg(feature = "planted-position-bugs")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlantedPositionBug {
    /// A removal moves the markers after it left by one unit too few.
    RemoveOffByOne,
    /// Integrating an item (a remote update, a redo) leaves the text's markers in place.
    KeepMarkersOnIntegrate,
}

#[cfg(feature = "planted-position-bugs")]
impl From<PlantedPositionBug> for yrs::PlantedPositionBug {
    fn from(b: PlantedPositionBug) -> Self {
        match b {
            PlantedPositionBug::RemoveOffByOne => yrs::PlantedPositionBug::RemoveOffByOne,
            PlantedPositionBug::KeepMarkersOnIntegrate => {
                yrs::PlantedPositionBug::KeepMarkersOnIntegrate
            }
        }
    }
}

thread_local! {
    /// Entries the kit read or copied in bulk on this thread (`entry_visits!`): list elements, map
    /// entries and text roots read from the engine doc by a whole-root reader, copied into or
    /// out of a view, or counted. A count, never a timing.
    static ENTRY_VISITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Whether the session checks its live view of a keyed doc against the engine doc, and a
    /// read base it rebuilds against the snapshot taken with it (on by default in test builds).
    static VIEW_CHECKS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

thread_local! {
    /// Entries the engine adapter read in bulk on this thread (`engine_visits!`): a list's
    /// order rebuilt from its slots, a list plan's read of every slot and register. Counted
    /// apart from the kit's own ([`take_entry_visits`]): the engine's work is its side's.
    static ENGINE_VISITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Add `n` to this thread's engine visit count (`engine_visits!`).
#[doc(hidden)]
pub fn count_engine_visits(n: u64) {
    ENGINE_VISITS.with(|v| v.set(v.get() + n));
}

/// This thread's engine visit count since the last take, reset to 0 (the engine adapter's
/// bulk reads: list orders rebuilt, list plans read).
pub fn take_engine_visits() -> u64 {
    ENGINE_VISITS.with(|v| v.replace(0))
}

/// Add `n` to this thread's visit count (`entry_visits!`).
#[doc(hidden)]
pub fn count_entry_visits(n: u64) {
    ENTRY_VISITS.with(|v| v.set(v.get() + n));
}

/// This thread's visit count since the last take, reset to 0: the entries the kit read or
/// copied in bulk (whole-root engine reads, view copies), a count independent of timing.
pub fn take_entry_visits() -> u64 {
    ENTRY_VISITS.with(|v| v.replace(0))
}

/// Switch the live view's checks on this thread (on by default): every hand-out of a keyed
/// doc's live view is compared with the engine doc, and every rebuilt read base with the
/// snapshot taken with it. Off, the kit does only what a host build does (the counts and the
/// measurements switch them off). Returns the previous setting.
pub fn set_view_checks(on: bool) -> bool {
    VIEW_CHECKS.with(|c| c.replace(on))
}

/// Whether the live view's checks are on on this thread ([`set_view_checks`]).
pub fn view_checks() -> bool {
    VIEW_CHECKS.with(|c| c.get())
}

thread_local! {
    /// The profile (v0.9.7): per scope name, (times entered, seconds inside).
    static PROFILE: std::cell::RefCell<std::collections::BTreeMap<&'static str, (u64, f64)>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// A profile scope: the time from `new` to drop is added to its name (this thread's profile).
/// Nested scopes each count their own (inclusive) time.
#[doc(hidden)]
pub struct ProfScope {
    name: &'static str,
    start: std::time::Instant,
}

impl ProfScope {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            start: std::time::Instant::now(),
        }
    }
}

impl Drop for ProfScope {
    fn drop(&mut self) {
        let s = self.start.elapsed().as_secs_f64();
        PROFILE.with(|p| {
            let mut p = p.borrow_mut();
            let e = p.entry(self.name).or_default();
            e.0 += 1;
            e.1 += s;
        });
    }
}

/// This thread's profile since the last call, by scope name: (times entered, seconds
/// inside); then it starts again. Test-only (the soak's profile, v0.9.7).
pub fn profile_take() -> Vec<(&'static str, u64, f64)> {
    PROFILE.with(|p| {
        std::mem::take(&mut *p.borrow_mut())
            .into_iter()
            .map(|(k, (n, s))| (k, n, s))
            .collect()
    })
}

/// The engine's position index counters (k2.3; [`crate::doc::Doc::position_index_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PositionIndexStats {
    /// Index lookups in texts.
    pub lookups: u64,
    /// Lookups that started from a marker.
    pub marker_hits: u64,
    /// Lookups that walked from the text's start.
    pub walks: u64,
    /// Marker lookups the walk also ran for (the shadow check).
    pub shadow_checks: u64,
    /// Shadow checks that differed.
    pub mismatches: u64,
    /// Markers that no longer resolved.
    pub stale_markers: u64,
}

impl std::ops::AddAssign for PositionIndexStats {
    fn add_assign(&mut self, o: Self) {
        self.lookups += o.lookups;
        self.marker_hits += o.marker_hits;
        self.walks += o.walks;
        self.shadow_checks += o.shadow_checks;
        self.mismatches += o.mismatches;
        self.stale_markers += o.stale_markers;
    }
}

/// A fault planted in the blame provenance map's incremental update (v0.9.5), so that the
/// blame differential test and the sampled runtime check
/// ([`crate::session::Config::blame_check_every`]) can show they catch one. Installed per
/// thread by [`with_prov_mutant`]; the rebuild from the engine and the store (the cold path
/// and the oracle) never sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProvMutant {
    /// A change's deletion never reaches the map: it keeps the deleted elements (the map
    /// then has more bytes than the text).
    MissDeletion,
    /// A deleted restoration stays in the index of live restorations by root (the runs are
    /// right; undo's "is this root live?" is not).
    MissDeletionInIndex,
    /// An undo, redo or revert's restored elements are recorded as their own roots, so blame
    /// names the restorer instead of the root's author.
    RestoredAsRestorer,
    /// v0.9.7.1 and before (undo spec U23): an outside save's revert record, and a promoted
    /// version's, keeps no roots, so a revert of it puts back fresh text instead of the roots
    /// the change removed. Planted in the record, not the map.
    OutsideSaveKeepsNoRoots,
}

thread_local! {
    static PROV_MUTANT: std::cell::Cell<Option<ProvMutant>> = const { std::cell::Cell::new(None) };
}

/// The provenance fault planted on this thread, if any.
pub(crate) fn prov_mutant() -> Option<ProvMutant> {
    PROV_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted in the provenance map's incremental update on this thread
/// (every session call `f` makes on this thread); removed afterwards, even if `f` panics.
pub fn with_prov_mutant<R>(mutant: ProvMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ProvMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            PROV_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(PROV_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

thread_local! {
    static BLAME_CHECK_HOOK: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
}

/// Run `f` with `hook` called by every sampled blame check on this thread at its off-lock
/// point: after the rebuild, before the lock is taken again (v0.9.7). The hook may call the
/// session (an edit, a purge) to show what the check does when the doc moves under it.
pub fn with_blame_check_hook<R>(hook: impl FnMut() + 'static, f: impl FnOnce() -> R) -> R {
    struct Back;
    impl Drop for Back {
        fn drop(&mut self) {
            BLAME_CHECK_HOOK.with(|h| h.borrow_mut().take());
        }
    }
    BLAME_CHECK_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    let _back = Back;
    f()
}

/// Call the installed blame check hook, if any (taken out while it runs, so it may blame).
pub(crate) fn blame_check_hook() {
    let hook = BLAME_CHECK_HOOK.with(|h| h.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
        BLAME_CHECK_HOOK.with(|h| {
            let mut h = h.borrow_mut();
            if h.is_none() {
                *h = Some(hook);
            }
        });
    }
}

/// A fault planted in the live-edit rebase (E-LIVE, `edit_at_base`, v0.9.7), so the live
/// tests can show they catch each one (amendment E-LIVE r2's mutants). Installed per thread
/// by [`with_live_mutant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LiveMutant {
    /// The server puts ours left of an insert at the same point (the client mirror doesn't).
    TieLeftForOurs,
    /// A deletion runs over text another change inserted inside its range.
    DeleteSwallowsInsert,
    /// merge3's O5 for live edits: two inserts at one point conflict.
    O5ForLive,
    /// A replacement's text goes at the right end of its range, after others' insertions.
    ReplacementTextPlacedRight,
    /// Two overlapping deletions conflict.
    DeleteDeleteConflicts,
    /// The client sends a base that leaves out its own acknowledged change (the client
    /// model's fault: the rebase itself has nothing to plant).
    OwnChangeInWindow,
    /// The window holds each change's hunks widened to whole words (as D1's token
    /// expansion widens a change inside `edit_as`), not the authored ones (clarification
    /// r2.1): typing inside a multi-author word then bounces.
    WindowHoldsExpandedHunks,
    /// r2.2's rule (1) on the server only: its rebase re-canonicalizes the carried change at
    /// every step, the client's doesn't (E-LIVE r2.3): the two disagree.
    RecanonServerOnly,
    /// r2.2's rule (1) on both sides: rule 4 then judges merged and split shapes, not the
    /// hunks as sent (a deletion bounces, a split replacement passes).
    RecanonBothSides,
    /// "Strictly inside" judged by the tie-break's placement alone (`Bias::Right`), not under
    /// both placements (r2.3 S3): an insertion tied with another at its point bounces when a
    /// later deletion takes the other's text and one neighbour.
    InsideByTieBreak,
}

thread_local! {
    static LIVE_MUTANT: std::cell::Cell<Option<LiveMutant>> = const { std::cell::Cell::new(None) };
}

/// The live-rebase fault planted on this thread, if any.
pub fn live_mutant() -> Option<LiveMutant> {
    LIVE_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` panics.
pub fn with_live_mutant<R>(mutant: LiveMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<LiveMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            LIVE_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(LIVE_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// The D1 amendment's mutants (v1.10), for the kit's integration tests: wrong variants of
/// the authored-ranges rule that the tests must catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum D1Mutant {
    /// The rule as specified.
    #[default]
    None,
    /// `reuse-foreign-elements-in-changed-token`: D1 before the amendment (no widening).
    ReuseForeign,
    /// `rewrite-whole-line`: the authored ranges widened to whole lines.
    RewriteWholeLine,
    /// `rewrite-merged-ranges`: `write --base` also rewrites what merge3 kept from the live
    /// text.
    RewriteMergedRanges,
}

thread_local! {
    static D1_MUTANT: std::cell::Cell<D1Mutant> = const { std::cell::Cell::new(D1Mutant::None) };
    static D1_EXTRA: std::cell::RefCell<Vec<(usize, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Run `f` with the D1 mutant `m` active on this thread.
pub fn with_d1_mutant<R>(m: D1Mutant, f: impl FnOnce() -> R) -> R {
    struct Back(D1Mutant);
    impl Drop for Back {
        fn drop(&mut self) {
            D1_MUTANT.with(|c| c.set(self.0));
        }
    }
    let _back = Back(D1_MUTANT.with(|c| c.replace(m)));
    f()
}

/// The D1 mutant active on this thread.
pub(crate) fn d1_mutant() -> D1Mutant {
    D1_MUTANT.with(|c| c.get())
}

/// The `rewrite-merged-ranges` mutant's extra ranges for the next change on this thread.
pub(crate) fn set_d1_extra(v: Vec<(usize, usize)>) {
    D1_EXTRA.with(|c| *c.borrow_mut() = v);
}

/// Take them (the change consumes them).
pub(crate) fn take_d1_extra() -> Vec<(usize, usize)> {
    D1_EXTRA.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// A live grid's planted fault (`grid.rs`, the session's cells presence and claims), installed
/// per thread by [`with_grid_mutant`], so the grid's rows show they catch it
/// (`tests/grid_view.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GridMutant {
    /// Numbering ignores the gap field: every keyed row and column counts as the next one, so
    /// blank rows vanish from the numbers.
    GapIgnored,
    /// A corner whose form a change broke is moved to the nearest cell in normal form instead
    /// of dropped.
    CornersMoveNotDrop,
    /// A cells claim's start event carries an empty actor, so a view can't word it without the
    /// roster.
    ClaimCellsActorMissing,
}

thread_local! {
    static GRID_MUTANT: std::cell::Cell<Option<GridMutant>> = const { std::cell::Cell::new(None) };
}

/// The live grid fault planted on this thread, if any.
pub fn grid_mutant() -> Option<GridMutant> {
    GRID_MUTANT.with(|m| m.get())
}

/// Run `f` with `mutant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_grid_mutant<R>(mutant: GridMutant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<GridMutant>);
    impl Drop for Back {
        fn drop(&mut self) {
            GRID_MUTANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(GRID_MUTANT.with(|m| m.replace(Some(mutant))));
    f()
}

/// A fault planted in chunked tier-0 versions (`docs/design/version-storage-r2.md`), so the rows
/// that should catch it (`tests/chunked_versions.rs`) can show they do, by name. Installed per
/// thread by [`with_chunk_plant`]; a row reads its name from `MPC_CHUNK_PLANT`
/// ([`ChunkPlant::named`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChunkPlant {
    /// `gc-ignores-waiting-segment`: `gc` removes a segment file with no row even while a
    /// write that is about to commit it marks it waiting.
    GcIgnoresWaitingSegment,
    /// `commit-skips-chunk-check`: a chunked commit doesn't check that the chunk rows the write
    /// reuses are still there.
    CommitSkipsChunkCheck,
    /// `reader-skips-chunk-check`: a chunked read doesn't verify each chunk's keyed id.
    ReaderSkipsChunkCheck,
    /// `cheap-path-beyond-current`: a chunked write reuses any chunk the doc stores, not only
    /// those of the versions its cheap set names.
    CheapPathBeyondCurrent,
    /// `reader-opens-lazily`: a chunked read opens each segment file when it reaches it,
    /// not every one before its first byte.
    ReaderOpensLazily,
    /// `retention-ignores-chunk-marks`: retention removes a chunk row at no references even
    /// while a write that reuses the chunk marks it waiting.
    RetentionIgnoresChunkMarks,
    /// `delete-space-leaves-segments`: deleting a space leaves its docs' segment folders for a
    /// later `gc`.
    DeleteSpaceLeavesSegments,
    /// `purge-skips-compaction`: a purge drops rows but rewrites no segment, so the purged
    /// chunks' bytes stay in the segment files.
    PurgeSkipsCompaction,
    /// `removal-list-not-finished-at-open`: an open leaves the removal list a stop after a
    /// purge's commit left.
    RemovalListNotFinishedAtOpen,
    /// `compaction-skips-dead-ranges`: a purge compacts only the segments holding a chunk it
    /// purged, not those with other dead ranges.
    CompactionSkipsDeadRanges,
    /// `compaction-output-not-waiting`: a compaction's output segment isn't marked waiting
    /// before its commit, so `gc` may remove it.
    CompactionOutputNotWaiting,
    /// `purge-skips-doc-lock`: a purge doesn't take the doc's internal lock, so it runs beside a
    /// write of the doc.
    PurgeSkipsDocLock,
    /// `history-chunk-cheap`: the blob store widens the cheap set a session gives it to every
    /// version's chunks, so a chunk only history holds takes the cheap path for a writer
    /// without the history right.
    HistoryChunkCheap,
    /// `manifest-reuse-skips-full-write`: a write whose whole file equals a stored version's
    /// (its manifest is there) writes nothing and reuses every chunk, whatever the cheap set.
    ManifestReuseSkipsFullWrite,
    /// `cheap-set-ignores-history-right`: a session gives every chunked write the cheap set of
    /// a connection with the history right, whatever the writer's rights.
    CheapSetIgnoresHistoryRight,
    /// `rechunk-keeps-old-blob`: a rechunk leaves the whole-file blob it dropped for `gc`,
    /// instead of removing it at once.
    RechunkKeepsOldBlob,
    /// `hints-required`: a pack's cut hints are the only cut points: each span between two is
    /// one chunk, however long (no content cut inside it, and none at the maximum).
    HintsRequired,
    /// `hints-unmerged`: hints closer than the chunk minimum are all kept as cut points.
    HintsUnmerged,
}

impl ChunkPlant {
    /// The plant `name` names (`MPC_CHUNK_PLANT`'s spelling).
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "gc-ignores-waiting-segment" => Self::GcIgnoresWaitingSegment,
            "commit-skips-chunk-check" => Self::CommitSkipsChunkCheck,
            "reader-skips-chunk-check" => Self::ReaderSkipsChunkCheck,
            "cheap-path-beyond-current" => Self::CheapPathBeyondCurrent,
            "reader-opens-lazily" => Self::ReaderOpensLazily,
            "retention-ignores-chunk-marks" => Self::RetentionIgnoresChunkMarks,
            "delete-space-leaves-segments" => Self::DeleteSpaceLeavesSegments,
            "purge-skips-compaction" => Self::PurgeSkipsCompaction,
            "removal-list-not-finished-at-open" => Self::RemovalListNotFinishedAtOpen,
            "compaction-skips-dead-ranges" => Self::CompactionSkipsDeadRanges,
            "compaction-output-not-waiting" => Self::CompactionOutputNotWaiting,
            "purge-skips-doc-lock" => Self::PurgeSkipsDocLock,
            "history-chunk-cheap" => Self::HistoryChunkCheap,
            "manifest-reuse-skips-full-write" => Self::ManifestReuseSkipsFullWrite,
            "cheap-set-ignores-history-right" => Self::CheapSetIgnoresHistoryRight,
            "rechunk-keeps-old-blob" => Self::RechunkKeepsOldBlob,
            "hints-required" => Self::HintsRequired,
            "hints-unmerged" => Self::HintsUnmerged,
            _ => return None,
        })
    }
}

thread_local! {
    static CHUNK_PLANT: std::cell::Cell<Option<ChunkPlant>> = const { std::cell::Cell::new(None) };
}

/// The chunk fault planted on this thread, if any.
pub fn chunk_plant() -> Option<ChunkPlant> {
    CHUNK_PLANT.with(|m| m.get())
}

/// Run `f` with `plant` planted on this thread; removed afterwards, even if `f` unwinds.
pub fn with_chunk_plant<R>(plant: ChunkPlant, f: impl FnOnce() -> R) -> R {
    struct Back(Option<ChunkPlant>);
    impl Drop for Back {
        fn drop(&mut self) {
            CHUNK_PLANT.with(|m| m.set(self.0));
        }
    }
    let _back = Back(CHUNK_PLANT.with(|m| m.replace(Some(plant))));
    f()
}

/// A step of a chunked write, where a test hook ([`with_chunk_hook`]) runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChunkStep {
    /// The new segment's temp file is written and synced.
    TempWritten,
    /// The temp file is renamed into place.
    Renamed,
    /// The segment's folder is synced.
    FolderSynced,
    /// Just before the transaction that commits the version.
    BeforeCommit,
    /// The version's transaction committed.
    Committed,
    /// The write's waiting marks are released.
    MarksReleased,
    /// A compaction's output segment is renamed into place, before the commit that names it.
    CompactionRenamed,
    /// A purge's transaction committed, before its removal list is performed (a stop there
    /// reports the purge as failed, as a process stop after the commit would leave it).
    PurgeCommitted,
    /// A chunked read has read its rows, before it opens its segments (first attempt only).
    ReaderRowsRead,
    /// A rechunk removed the whole-file blob of the version it converted (a stop there reports
    /// the rechunk as failed).
    OldBlobRemoved,
}

/// A chunked write's test hook: called at each [`ChunkStep`]; `true` stops the write there, as
/// a process stop would (nothing is cleaned up).
type ChunkHook = Box<dyn FnMut(ChunkStep) -> bool>;

thread_local! {
    static CHUNK_HOOK: RefCell<Option<ChunkHook>> = const { RefCell::new(None) };
}

/// Run `f` with `hook` called at each step of every chunked write on this thread; removed
/// afterwards, even if `f` unwinds.
pub fn with_chunk_hook<R>(
    hook: impl FnMut(ChunkStep) -> bool + 'static,
    f: impl FnOnce() -> R,
) -> R {
    struct Back(Option<ChunkHook>);
    impl Drop for Back {
        fn drop(&mut self) {
            let old = self.0.take();
            CHUNK_HOOK.with(|h| *h.borrow_mut() = old);
        }
    }
    let _back = Back(CHUNK_HOOK.with(|h| h.borrow_mut().replace(Box::new(hook))));
    f()
}

/// Run this thread's chunk hook at `step`: whether it stops the write there. The hook is taken
/// out while it runs, so it may call into the blob store.
pub(crate) fn chunk_hook(step: ChunkStep) -> bool {
    let Some(mut h) = CHUNK_HOOK.with(|c| c.borrow_mut().take()) else {
        return false;
    };
    let stop = h(step);
    CHUNK_HOOK.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(h);
        }
    });
    stop
}

/// The work of the chunked writes on this thread since the last [`take_chunk_work`], from
/// test-only counters (counts, never timings): what the counted-work rows of
/// `docs/design/version-storage-r2.md` (section 6) compare field for field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChunkWork {
    /// Bytes written to new segment files.
    pub segment_bytes: u64,
    /// Segment files synced.
    pub file_syncs: u64,
    /// Folders synced: a new segment's folder, and the parent of each folder made for it.
    pub folder_syncs: u64,
    /// Rows written (made or updated) by the transaction that commits a chunked version: the
    /// doc's chunk key, each chunk row, the manifest (made, rewritten or its count raised), the
    /// segment's list, each version, the reference it takes, and the file's sync state.
    pub rows_written: u64,
    /// Transactions committed with a chunked version.
    pub commits: u64,
}

thread_local! {
    static CHUNK_WORK: std::cell::Cell<ChunkWork> = const {
        std::cell::Cell::new(ChunkWork {
            segment_bytes: 0,
            file_syncs: 0,
            folder_syncs: 0,
            rows_written: 0,
            commits: 0,
        })
    };
}

/// Add to this thread's [`ChunkWork`] (the kit's `chunk_work!` counts).
pub(crate) fn count_chunk_work(f: impl FnOnce(&mut ChunkWork)) {
    CHUNK_WORK.with(|c| {
        let mut w = c.get();
        f(&mut w);
        c.set(w);
    });
}

/// This thread's [`ChunkWork`] since the last take, reset to nothing.
pub fn take_chunk_work() -> ChunkWork {
    CHUNK_WORK.with(|c| c.replace(ChunkWork::default()))
}
