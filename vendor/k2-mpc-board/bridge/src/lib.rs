//! The board file verbs on the multiplayer kit's verb features: each refusal of the board crate as
//! a stable error id from the kit's registry, `explain` and `capabilities` through the kit, a JSON
//! report with the file it was about, and dry runs. A host's command line calls it for a board
//! file; the bridge knows no host. See `README.md`.
//!
//! - [`rows`]: which of the kit's rows (MPC-E830 to MPC-E859) each refusal is, by the call it came
//!   from ([`Phase`]).
//! - [`report`]: the kit's [`ErrorReport`] for each refusal, every key its row's message names set.
//! - [`value_out`]: the one conversion from a board value to the kit's, for every value the bridge
//!   gives.
//! - [`HostBug`]: what the board crate can't give, typed, never a user-facing row.
//! - [`Answer`], [`Refused`], [`Why`]: what a call gives back, with the file it was about.
//! - [`parse`]: a host's argv to a call; [`dry_run`]: running it, or a dry run; [`json`]: a call's
//!   JSON; [`registry`]: the file registry for `capabilities`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod answer;
mod bug;
pub mod dry_run;
pub mod json;
pub mod parse;
pub mod registry;
pub mod report;
pub mod rows;
mod value;

pub use answer::{Answer, Refused, Why};
pub use bug::HostBug;
pub use k2_mpc_base as kit;
pub use k2_mpc_base::errors::ErrorReport;
pub use k2_mpc_board as board;
pub use rows::{Clash, Phase, TargetKind};
pub use value::{read_value, value_out};
