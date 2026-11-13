//! Checks a build statement against the build machine owner's roster and
//! the files you hold, offline.
//!
//! A **build statement** is an [in-toto Statement v1] with a [SLSA
//! Provenance v1] predicate, in a [DSSE envelope], signed by the key of the
//! machine (the *node*) that ran the build: "I built these bytes from this
//! commit with this command, walled off this way". The node's **owner**
//! signs a **roster**: its list of node keys, each with its machine, status
//! and dates. You pin the owner's roster key fingerprint, got from the owner
//! out of band; everything else is checked from there:
//!
//! ```text
//! pinned owner key ─▶ roster ─▶ node key ─▶ DSSE signature ─▶ statement ─▶ subjects ─▶ your files
//!                                   └──▶ node self-statement (wall, key exposure, taint)
//! ```
//!
//! The rules and their order are in [`verify`]. Formats: `docs/provenance-v1.md`
//! and `docs/ci-formats-v1.md` in the source repository.
//!
//! **What a pass means.** The owner vouches for a machine, and that machine
//! says it built these bytes from that commit. It doesn't prove the owner
//! honest, the source safe or the build reproducible; a twin (a second
//! build on another machine with the same result, [`verify::Input::twin`])
//! narrows what one bad machine can do.
//!
//! **Time.** Every time in a statement is the node's own clock. For a key
//! that is no longer active in the roster, a statement could have been
//! signed after the key was retired and back-dated, so the verdict is
//! [`verify::Verdict::TimeNotProven`] unless you supply proof of when the
//! statement existed ([`verify::Policy::existed_before`]). A roster is only
//! as current as its date: a key revoked after it was issued still looks
//! active in it, so the report warns when the roster predates the build.
//!
//! [in-toto Statement v1]: https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md
//! [SLSA Provenance v1]: https://slsa.dev/spec/v1.0/provenance
//! [DSSE envelope]: https://github.com/secure-systems-lab/dsse/blob/master/envelope.md

#![forbid(unsafe_code)]

pub mod canonical;
pub mod dsse;
pub mod load;
pub mod node_statement;
pub mod roster;
pub mod statement;
pub mod trust;
pub mod util;
pub mod verify;

pub use dsse::{Envelope, NodeKey};
pub use statement::Statement;
pub use verify::{verify, Input, Policy, Report, Verdict};
