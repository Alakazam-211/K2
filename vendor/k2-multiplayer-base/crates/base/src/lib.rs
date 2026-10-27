//! Multiplayer kit: the layer every file-type pack builds on.
//!
//! A pack declares a [`Schema`] made of K2-defined primitives, and gets a [`Doc`] that
//! merges concurrent edits from people and agents. The CRDT engine underneath is an
//! implementation detail: nothing public names it, and edits are plain data
//! ([`Edit`]), so the engine can be replaced without changing any pack.
//!
//! - [`doc`]: the loaded doc, wire and store bytes, edits as a participant.
//! - [`schema`]: primitive kinds and pack schemas.
//! - [`text`]: text algorithms (minimal diff, three-way merge) used by text edits.
//! - [`store`]: persistence (snapshot + update log + attribution + engine-neutral versions).
//! - [`codec`]: reusable file codecs (plain text with BOM/EOL round trip).
//! - [`session`]: loaded docs with participants: rights, presence, edits, `write --base`,
//!   undo, change events, the memory-bounded pool (`docs/SESSIONS.md`).
//! - [`conformance`]: K2's primitive semantics as tests any engine must pass.
//!
//! Disk sync (sessions opened with a file): atomic write-back, outside edits merged in as a
//! CRDT change, echo suppression; the host watches and drives the clock.
//!
//! Planned here (see `docs/ARCHITECTURE.md`): sync transport, versions/history/blame, the
//! parity framework, tier-0 support for any file.

pub mod codec;
pub mod conformance;
pub mod doc;
mod engine;
mod hash;
pub mod schema;
pub mod session;
pub mod store;
pub mod text;

pub use doc::{ClientId, Doc, DocError, Edit, Neutral, UpdateInfo, MAX_CLIENT_ID};
pub use schema::{Kind, Schema};
pub use session::{
    Host, Pack, Participant, ParticipantKind, Rights, Session, SessionError, SessionManager,
    TextFilePack,
};
