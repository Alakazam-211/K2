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

/// A profile scope (v0.9.7, test-only): with the `differential` feature (or in the kit's
/// tests) it times the rest of the enclosing block under `$name`
/// ([`differential::profile_take`]); otherwise it is nothing.
macro_rules! prof {
    ($name:literal) => {
        #[cfg(any(test, feature = "differential"))]
        let _prof = $crate::differential::ProfScope::new($name);
    };
}

// K10, the build-time guard: decoding remote clients' bytes stays off until the six gates hold
// (docs/HOST-WIRE.md). A host that has checked them acknowledges it in its own build:
// `RUSTFLAGS="--cfg k2_mpc_untrusted_wire_ack"` (or `[build] rustflags` in its cargo config).
#[cfg(all(feature = "untrusted-wire", not(k2_mpc_untrusted_wire_ack)))]
compile_error!(
    "k2-mpc-base: the `untrusted-wire` feature decodes bytes from remote clients, which is \
     off until the six gates in docs/HOST-WIRE.md hold (the engine fork's line, the strict \
     decoder, per-connection client IDs, relaying original bytes, the wire fuzz target, the \
     engine owner's sign-off). Build with `--cfg k2_mpc_untrusted_wire_ack` only once they do."
);

pub mod blob;
pub mod codec;
pub mod conformance;
#[cfg(any(test, feature = "differential"))]
pub mod differential;
pub mod doc;
mod engine;
pub mod errors;
pub mod export;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
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
