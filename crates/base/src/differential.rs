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

/// A fault planted in person undo grouping (undo spec U3, K4, v0.9.7), so the grouping
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
    pub clock_before: u32,
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
}

/// Whether the installed side writes back files the old way.
pub(crate) fn full_writeback() -> bool {
    FULL_WB.with(|p| p.get())
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
    struct Back<'a>(&'a mut Side);
    impl Drop for Back<'_> {
        fn drop(&mut self) {
            PLANT.with(|p| p.set(false));
            FULL_WB.with(|p| p.set(false));
            NO_POSINDEX.with(|p| p.set(false));
            if let Some(s) = SIDE.with(|t| t.borrow_mut().take()) {
                *self.0 = s;
            }
        }
    }
    PLANT.with(|p| p.set(side.plant));
    FULL_WB.with(|p| p.set(side.full_writeback));
    NO_POSINDEX.with(|p| p.set(side.no_position_index));
    let prev = SIDE.with(|t| t.borrow_mut().replace(std::mem::take(side)));
    assert!(prev.is_none(), "with_side: a side is already installed");
    let _back = Back(side);
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
/// `K2_COMPUTE_JOB`, as a JSON string, or `null` off a node (then `collect.py` fills it in).
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
    /// r2.1, F1): typing inside a multi-author word then bounces.
    WindowHoldsExpandedHunks,
    /// r2.2's rule (1) on the server only: its rebase re-canonicalizes the carried change at
    /// every step, the client's doesn't (E-LIVE r2.3): the two disagree (P1).
    RecanonServerOnly,
    /// r2.2's rule (1) on both sides: rule 4 then judges merged and split shapes, not the
    /// hunks as sent (a deletion bounces, a split replacement passes: P2, P2c).
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
