//! Client API request and response bodies (C4–C8, C12).
//!
//! Owning slices may add optional fields to these types. Renames or removals need a contract
//! change (S1 §9.4).

pub mod audit;
pub mod auth;
pub mod changes;
pub mod channels;
pub mod compliance;
pub mod exports;
pub mod extras;
pub mod files;
pub mod guests;
pub mod media;
pub mod meetings;
pub mod members;
pub mod messages;
pub mod moderation;
pub mod notify;
pub mod overwrites;
pub mod people;
pub mod read_state;
pub mod resolve;
pub mod roles;
pub mod search;
pub mod spaces;
