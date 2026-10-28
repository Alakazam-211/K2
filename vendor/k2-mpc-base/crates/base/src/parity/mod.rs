//! The agent-parity framework (K-PAR-1…3): everything a person can do in an editor, an
//! agent can do through a verb, with the same effect.
//!
//! - [`PersonAction`]: one variant per parity row (P1…P31). Deliberately exhaustive: a pack
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
mod cases;
mod harness;
mod registry;

pub use action::PersonAction;
pub use cases::{edit_summary, kit_cases, undo_summary};
pub use harness::{
    check, check_list, rename_target, render, run_case, Agent, AgentFn, Case, ChangeObs, EventObs,
    Failure, Observation, Output, ParityRow, Person, PersonFn, PresenceObs, Report, RowStatus,
    SetupFn, Side, Stage, Step, VerbOut, VerbValue, VersionObs, ACTOR, DOC, DOC_PATH, OBSERVER,
};
pub use registry::{
    verb_form, Api, Registry, RegistryError, Source, VerbKind, VerbSpec, KIT_VERBS,
};
