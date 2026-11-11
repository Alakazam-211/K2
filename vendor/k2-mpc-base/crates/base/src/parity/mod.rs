//! The agent-parity framework (K-PAR-1…3): everything a person can do in an editor, an
//! agent can do through a verb, with the same effect.
//!
//! - [`PersonAction`]: one variant per parity row (P1…P44), each for some of text docs, blob
//!   docs (tier 0) and grid docs ([`Tier`]). Deliberately exhaustive: a pack
//!   maps it with a `match`, so a new row doesn't compile until every pack maps it.
//! - [`Registry`]: every verb ([`VerbSpec`]: name, kind, rights, the session calls it makes,
//!   the actions it serves, flags, exit codes): the kit's [`KIT_VERBS`] plus a pack's own
//!   ([`crate::session::Pack::verbs`]).
//! - [`ParityRow`]: a pack's parity list as data (live, deferred, or N/A with a reason).
//! - The harness ([`check`]): for every action, the **person path** (a tab: the session API
//!   directly, byte positions, `undo` as Ctrl+Z) and the **agent path** (registered verbs
//!   only, through a reference binding to the session API) run on two identical docs, and
//!   the observable effects are compared: text, file, attribution, versions, approvals,
//!   presence (role, caret, viewing range), exit code and output. The kit's cases are one
//!   exhaustive `match` ([`kit_cases`]); a pack adds cases for its own verbs.
//! - Tier 0 (any file): the same check on blob docs, [`check_blob`] with
//!   [`Registry::for_tier`]`(Tier::Blob)` and [`kit_blob_cases`]. A text pack's list needs
//!   rows only for text actions ([`PersonAction::for_tier`]).
//! - The grid tier (a [`crate::table`] pack, A11): [`check_grid`] with the pack's registry
//!   ([`Registry::for_pack`]: the kit's [`Tier::Grid`] verbs and the pack's structured
//!   verbs), [`kit_grid_cases`] and the pack's [`GridCase`]s; it compares the grid **by key**
//!   ([`GridObservation`]) as well as the rendered bytes. P35-P44 are grid rows; text and
//!   blob lists don't carry them (a text pack maps them N/A in its exhaustive `match`).
//! - A blob pack with a structure index: [`Registry::for_blob_pack`] adds its verbs and node
//!   claims, and [`kit_index_cases`] checks them on two versions of a file the pack gives.
//! - [`positions`]: how each verb counts the positions it takes or prints (K-ADDR-4), the
//!   table `capabilities` states (`docs/POSITIONS.md`).
//!
//! A pack's test:
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use k2_mpc_base::parity::{self, ParityRow, Registry};
//! # use k2_mpc_base::session::{Pack, TextFilePack};
//! # let rows: Vec<ParityRow> = Vec::new();
//! let pack: Arc<dyn Pack> = Arc::new(TextFilePack::new("markdown", 4 << 20));
//! let registry = Registry::for_pack(pack.as_ref()).unwrap();
//! if let Err(f) = parity::check(pack, &rows, &registry, &[]) {
//!     panic!("{}", parity::render(&f));
//! }
//! ```

mod action;
mod annot;
mod blob;
mod blob_cases;
mod cases;
mod grid;
mod grid_cases;
mod harness;
mod index_cases;
pub mod positions;
mod registry;

pub use action::PersonAction;
pub use annot::{
    check_annotations, check_annotations_with, kit_annotation_cases, run_annotation_case,
    AnnotArgvFn, AnnotCase, AnnotPersonFn, AnnotSetupFn, AnnotWorld,
};
pub use blob::{
    check_blob, check_blob_list, run_blob_case, BlobAgent, BlobAgentFn, BlobCase, BlobObservation,
    BlobPerson, BlobPersonFn, BlobPresenceObs, BlobSetupFn, BlobStage, BlobVersionObs, BLOB_DOC,
    BLOB_MAX_BYTES, BLOB_PATH,
};
pub use blob_cases::kit_blob_cases;
pub use cases::{edit_summary, kit_cases, undo_summary};
pub use grid::{
    check_grid, check_grid_list, observe_grid_case, run_grid_case, GridCase, GridChangeObs,
    GridClaimObs, GridObservation, GridPresenceObs, GRID_DOC, GRID_PATH,
};
pub use grid_cases::kit_grid_cases;
pub use harness::{
    check, check_list, rename_target, render, run_case, Agent, AgentFn, Case, ChangeObs, ClaimObs,
    EventObs, Failure, Observation, Output, ParityRow, Person, PersonFn, PresenceObs, Report,
    RowStatus, SetupFn, Side, Stage, Step, VerbOut, VerbValue, VersionObs, ACTOR, DOC, DOC_PATH,
    OBSERVER,
};
pub use index_cases::kit_index_cases;
pub use registry::{
    verb_form, Api, FileVerb, Registry, RegistryError, Source, Tier, VerbKind, VerbSpec, ALIASES,
    ANNOTATION_VERBS, KIT_VERBS, RUN_TOOL_VERBS,
};
