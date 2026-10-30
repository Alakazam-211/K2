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
//! - [`store`]: persistence (snapshot + update log + attribution + engine-neutral versions),
//!   with erasure: redacted records, the file rewrite and the byte scan.
//! - [`codec`]: reusable file codecs (plain text with BOM/EOL round trip).
//! - [`blob`]: the blob service (tier 0): content-addressed bytes under a host folder,
//!   keyed per space, streamed and verified, with GC and purge.
//! - [`session`]: loaded docs with participants: rights, presence, edits, `write --base`,
//!   undo, change events, the memory-bounded pool (`docs/SESSIONS.md`); tier 0 for any
//!   file ([`session::BlobSession`]: presence, a lease, versions and restore, conflict
//!   copies); purge and participant erasure ([`Session::purge`], `docs/SESSIONS.md`,
//!   "Erasure"); blame per line at the character's grain ([`Session::blame`],
//!   `docs/SESSIONS.md`, "Blame"); agent presence: anchored claims with FIFO waiters, a
//!   host-chosen claim policy on edits, heads-up notes and an activity ring, memory only
//!   (`docs/SESSIONS.md`, "Agent presence").
//! - [`conformance`]: this engine's known deviations from the primitive rules (the rules
//!   themselves are the `k2-mpc-conformance` crate, generic over engines).
//! - [`tools`]: the text verbs agents run on a live doc (`cat`, `grep`, `sed`, `patch`...),
//!   as pure functions and through [`Session::run_tool`]; packs add their own
//!   ([`session::Pack::verbs`]).
//! - [`parity`]: the agent-parity framework: person actions, the verb registry, and the
//!   harness that runs each action the person's way and the agent's way and compares.
//!
//! Disk sync (sessions opened with a file): atomic write-back, outside edits merged in as a
//! CRDT change, echo suppression; the host watches and drives the clock.
//!
//! Planned here (see `docs/ARCHITECTURE.md`): sync transport.

pub mod blob;
pub mod codec;
pub mod conformance;
#[cfg(any(test, feature = "differential"))]
pub mod differential;
pub mod doc;
mod engine;
mod hash;
pub mod parity;
pub mod schema;
pub mod session;
pub mod store;
pub mod text;
pub mod tools;

pub use doc::{ClientId, Doc, DocError, Edit, Neutral, UpdateInfo, MAX_CLIENT_ID};
pub use schema::{Kind, Schema};
pub use session::{
    Host, Pack, Participant, ParticipantKind, Rights, Session, SessionError, SessionManager,
    TextFilePack,
};
