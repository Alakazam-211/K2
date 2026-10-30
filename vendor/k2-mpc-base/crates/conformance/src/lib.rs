//! The kit's primitive conformance suite, generic over engines.
//!
//! A rule is the kit's merge semantics for a primitive, written once against
//! [`TextReplica`]: the minimal surface a rule needs from an engine. Each engine implements
//! it through an adapter, and every rule runs on every engine:
//! - [`yrs::YrsText`]: the kit's own [`k2_mpc_base::Doc`] (Yrs today);
//! - [`model::ModelText`]: the reference model `mpce-model` (FugueMax), the oracle of record.
//!
//! **Capabilities** ([`caps`]): rules that need more than [`TextReplica`] (events, blame,
//! anchors, branches, purge; and at doc level undo, versions, history purge, erasure) are
//! written against an optional trait and run on every engine that implements it. The **coverage matrix** ([`matrix`]) lists every rule, the capability it
//! needs and its status on each engine (pass, pinned deviation, awaiting, never); a test
//! fails when a rule runs on no engine and isn't listed as awaiting, with a reason and a
//! target. It is printed by the suite and kept in `docs/CONFORMANCE.md`.
//!
//! **Deviations are per engine** ([`TextReplica::KNOWN_DEVIATIONS`]). Where an engine can't
//! meet a rule, the rule asserts the deviation **still happens**, so a fix (or an engine
//! swap) fails loudly until the entry is removed. Every listed id must name a rule in
//! [`matrix::RULES`] ([`invalid_deviation_ids`]). A conforming engine's list is empty: the
//! reference model's is, and a test asserts it.
//!
//! Rules that need one engine's machinery (wire bytes, untrusted decoding, the store) are
//! not here: they stay in that engine's crate. Decoding untrusted bytes is the native
//! decoder's (G2): the `sec.*` rules are declared here and awaiting.
//!
//! This crate is never published and nothing depends on it.

pub mod anchors;
pub mod blame;
pub mod branches;
pub mod caps;
pub mod erasure;
pub mod events;
pub mod kit_history;
mod kit_session;
pub mod kit_undo;
pub mod matrix;
pub mod model;
pub mod purge;
pub mod text;
pub mod undo;
pub mod versions;
pub mod yrs;

use std::fmt::Debug;

pub use k2_mpc_base::text::diff::Hunk;

/// The agent that authors a net's seed text, on every engine. Rules' writers use other ids
/// (101, 102, 103...), ordered numerically on every engine.
pub const SEED_AGENT: u64 = 2;

/// (rule id, why this engine deviates).
pub type Deviation = (&'static str, &'static str);

/// An edit refused before anything changed (for example, an offset off a scalar boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

/// One replica of a text doc on some engine: what the rules need, and nothing else.
///
/// - Agent ids are `u64` and order numerically on every engine (tie-breaks are by id).
/// - Offsets are **UTF-8 byte offsets** of the replica's current text, as in
///   [`k2_mpc_base::Edit::Text`]; the unit of text is the Unicode scalar value.
pub trait TextReplica: Sized {
    /// The engine's name, for messages.
    const ENGINE: &'static str;
    /// The rules this engine is known to fail, each with its reason. Empty for a conforming
    /// engine. Each id must name a rule in [`matrix::RULES`].
    const KNOWN_DEVIATIONS: &'static [Deviation];

    /// What a replica has seen, comparable between replicas of this engine (a version
    /// vector, a frontier...). Replicas that saw the same edits have equal versions.
    type Version: PartialEq + Debug + Clone;

    /// A replica that edits as `agent`, holding `seed`. The seed is authored by
    /// [`SEED_AGENT`] in the same way on every replica, so replicas made separately with the
    /// same seed share its content (syncing them adds nothing).
    fn replica(agent: u64, seed: &str) -> Self;

    /// Apply `hunks` as this replica's agent, as **one atomic batch**: sorted,
    /// non-overlapping hunks in UTF-8 byte offsets of the current text (the rules of
    /// [`k2_mpc_base::text::diff::validate`]). An offset off a scalar boundary, out of range
    /// or out of order is refused and nothing changes.
    fn apply(&mut self, hunks: &[Hunk]) -> Result<(), Refused>;

    /// Deliver everything `from` has that this replica lacks.
    fn sync_from(&mut self, from: &Self);

    /// The current text.
    fn text(&self) -> String;

    /// What this replica has seen.
    fn version(&self) -> Self::Version;
}

/// Is `rule` a listed deviation for engine `E`?
pub fn deviates<E: TextReplica>(rule: &str) -> bool {
    E::KNOWN_DEVIATIONS.iter().any(|(r, _)| *r == rule)
}

/// Deviation ids of `E` that name no rule (a misspelling hides nothing: it is reported).
pub fn invalid_deviation_ids<E: TextReplica>() -> Vec<&'static str> {
    E::KNOWN_DEVIATIONS
        .iter()
        .map(|(r, _)| *r)
        .filter(|r| matrix::rule(r).is_none())
        .collect()
}

/// A hunk inserting `s` at byte offset `at`.
pub fn ins(at: usize, s: &str) -> Hunk {
    Hunk {
        start: at,
        end: at,
        insert: s.into(),
    }
}

/// A hunk deleting bytes `start..end`.
pub fn del(start: usize, end: usize) -> Hunk {
    Hunk {
        start,
        end,
        insert: String::new(),
    }
}

/// A hunk replacing bytes `start..end` with `s`.
pub fn rep(start: usize, end: usize, s: &str) -> Hunk {
    Hunk {
        start,
        end,
        insert: s.into(),
    }
}

/// A set of replicas of one doc on engine `E`.
pub struct Net<E: TextReplica> {
    pub r: Vec<E>,
}

impl<E: TextReplica> Net<E> {
    /// One replica per agent, all holding `seed`.
    pub fn new(agents: &[u64], seed: &str) -> Self {
        Net {
            r: agents.iter().map(|&a| E::replica(a, seed)).collect(),
        }
    }

    pub fn text(&self, i: usize) -> String {
        self.r[i].text()
    }

    /// Replica `i` applies `hunks` as one batch; a refusal fails the rule.
    pub fn edit(&mut self, i: usize, hunks: Vec<Hunk>) {
        if let Err(e) = self.r[i].apply(&hunks) {
            panic!("{}: replica {i} refused {hunks:?}: {e:?}", E::ENGINE);
        }
    }

    /// Replica `i` inserts `s` at byte offset `at`.
    pub fn insert(&mut self, i: usize, at: usize, s: &str) {
        self.edit(
            i,
            vec![Hunk {
                start: at,
                end: at,
                insert: s.into(),
            }],
        );
    }

    /// Replica `i` deletes bytes `start..end`.
    pub fn delete(&mut self, i: usize, start: usize, end: usize) {
        self.edit(
            i,
            vec![Hunk {
                start,
                end,
                insert: String::new(),
            }],
        );
    }

    /// Deliver everything replica `from` has to replica `to`.
    pub fn sync(&mut self, from: usize, to: usize) {
        assert_ne!(from, to, "sync a replica with itself");
        let (a, b) = if from < to {
            let (l, r) = self.r.split_at_mut(to);
            (&l[from], &mut r[0])
        } else {
            let (l, r) = self.r.split_at_mut(from);
            (&r[0], &mut l[to])
        };
        b.sync_from(a);
    }

    /// Everyone gets everything; then assert convergence (same text, same version).
    pub fn sync_all(&mut self) -> String {
        for a in 0..self.r.len() {
            for b in 0..self.r.len() {
                if a != b {
                    self.sync(a, b);
                }
            }
        }
        let (t, v) = (self.text(0), self.r[0].version());
        for i in 1..self.r.len() {
            assert_eq!(self.text(i), t, "{}: replica {i} diverged", E::ENGINE);
            assert_eq!(
                self.r[i].version(),
                v,
                "{}: replica {i} has the same text but saw other edits",
                E::ENGINE
            );
        }
        t
    }
}
