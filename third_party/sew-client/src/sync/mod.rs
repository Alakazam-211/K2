//! The sync position model (S25 §13): where the client is, not what it holds.
//!
//! - [`SyncState`] and the pure planner [`SyncState::plan`] against a [`SyncBoot`] (from READY or,
//!   in S25.3, `GET /api/v1/me/sync`).
//! - [`CursorStore`]: [`JsonFileStore`] (atomic, `0600`) and [`MemoryStore`]; K2 implements the
//!   trait over its own database.
//! - [`DeliveryCursor`] (feature `mgmt` too): the agent delivery feed cursor for K2 (S31).
//!
//! The catch-up driver (`SyncDriver`) lands in S25.3 with S14's change routes.

mod state;
mod store;

#[cfg(all(feature = "mgmt", unix))]
mod deliveries;

pub use state::{BootChannel, ChannelCursor, SYNC_STATE_FORMAT, SyncBoot, SyncPlan, SyncState};
pub use store::{CursorStore, JsonFileStore, MemoryStore};

#[cfg(all(feature = "mgmt", unix))]
pub use deliveries::{DeliveryCursor, FeedStep, ResetReason};
