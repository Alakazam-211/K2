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
            if let Some(s) = SIDE.with(|t| t.borrow_mut().take()) {
                *self.0 = s;
            }
        }
    }
    PLANT.with(|p| p.set(side.plant));
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
