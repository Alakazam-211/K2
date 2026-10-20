#![deny(missing_docs)]
//! Sew wire contracts (slice map C1–C18).
//!
//! Every wire type lives here: ids, chat addresses, errors, HTTP request and response bodies,
//! gateway frames and events, change-log kinds, mgmt API bodies, permission bits, org policies
//! and audit events. Server, clients and K2 share this crate; nothing else defines wire JSON.
//!
//! The crate is pure: it reads the clock (only in [`clock`]) and does no other IO. It builds on
//! macOS as well as Linux (contracts rev 16).

#[macro_use]
mod macros;

pub mod addr;
pub mod api;
pub mod audit;
pub mod calls;
pub mod changes;
pub mod clock;
pub mod e2ee;
pub mod emoji_token;
pub mod error;
pub mod export;
pub mod external_id;
pub mod gateway;
pub mod http;
pub mod id;
pub mod install;
pub mod limits;
pub mod links;
pub mod mention;
pub mod mgmt;
pub mod pass;
pub mod perms;
pub mod policy;
pub mod recording;
pub mod registry;
pub mod routes;
pub mod search;
pub mod time;
pub mod token;
pub mod voice;
pub mod when;

mod examples;

/// Client API and gateway protocol version (C15). Changes only with a breaking change after the
/// first release.
pub const PROTOCOL_VERSION: u32 = 1;

/// Logical export archive format version carried in export manifests (C15).
pub const EXPORT_FORMAT: u32 = 1;

/// Additive revision of export format 1 (contracts rev 37, S23 G2). Readers refuse archives with
/// a higher minor (`format_too_new`); a breaking change bumps [`EXPORT_FORMAT`]. Minor 1 (Hearth
/// HC6, HC4): `spaces.ndjson` records carry the space's default channels, description, rules
/// and welcome channels. Minor 2 (SW13): they also carry the App a space belongs to. Minor 3
/// (Meet MC1): and whether a space is hidden.
pub const EXPORT_FORMAT_MINOR: u32 = 3;

/// The recorder participant's user-facing name (rev 90, Rosson: "Bobbin it is"). Clients label
/// the recorder's roster entry, the recording banner and notices with it. The engine never sees
/// it (only the pseudonymous `p_…` identity, rev 75); the binary, unit and user stay
/// `sew-recorder`.
pub const RECORDER_DISPLAY_NAME: &str = "Bobbin";

pub use addr::{Addr, AddrError, ChannelRef, DmTarget, Handle, Slug, SpaceRef};
pub use clock::{Clock, SystemClock};
pub use error::{ApiError, Code, ErrorBody, Validate, ValidationError};
pub use external_id::ExternalId;
pub use http::{IdempotencyKey, Nonce, Page, PageQuery, Patch};
pub use id::{Id, IdGenerator, IdList, NodeId, id_to_time};
pub use perms::Perms;
pub use time::Timestamp;
