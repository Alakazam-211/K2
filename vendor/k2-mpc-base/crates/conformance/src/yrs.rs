//! [`TextReplica`] on the kit's own [`Doc`] (Yrs today), through its public API plus the
//! test-only hooks of the `conformance` feature ([`k2_mpc_base::conformance::replica`]).
//!
//! Capabilities, all through those hooks:
//! - [`Events`]: a batch is a Yrs update the kit's doc made (`events_since`, a diff against a
//!   state vector); delivery runs the kit's per-client continuity check, then applies it
//!   ([`replica::deliver`]). No other bytes reach the engine.
//! - [`Blame`]: the kit's blame runs, per character; a character's ID is (client, clock).
//! - [`Anchors`]: Yrs relative positions.
//! - [`Branches`]: a branch is a replica loaded from the doc's encoded state (public API),
//!   editing as its own client ID; merging delivers what it has ([`replica::sync`]). No
//!   [`crate::caps::BranchAt`]: Yrs keeps no state of past versions (see the matrix).
//! - [`Purge`]: what a session's purge does to its doc for a live span (`purge --text
//!   --and-remove`): the span deleted as the purger's change, then the doc re-encoded and
//!   loaded again. Yrs garbage collection is on (K-PURGE-11), so deleted content is dropped
//!   at the delete's commit, here and on every replica that applies it; the item keeps its
//!   ID and length. The byte scan is the kit's (`scan_bytes`, whole or any 8-byte window)
//!   over the encoded state, what the store would keep.
//!
//! Undo is the kit's, on a session: [`crate::kit_undo::KitUndo`]; versions, history purge
//! and erasure too: [`crate::kit_history::KitHistory`].

use k2_mpc_base::conformance::replica::{self, Anchor, Stick};
use k2_mpc_base::{ClientId, Doc, DocError, Edit, Kind, Schema};

use k2_mpc_base::store::scan_bytes;

use crate::caps::{Anchors, Blame, Blamed, Branches, EventRefusal, Events, Purge, Side};
use crate::{Deviation, Hunk, Refused, TextReplica, SEED_AGENT};

const SCHEMA: Schema = Schema {
    name: "conformance-text",
    roots: &[("t", Kind::Text)],
};
const ROOT: &str = "t";

/// One replica: a kit [`Doc`] with one text root, edited as one agent.
pub struct YrsText {
    doc: Doc,
    agent: ClientId,
}

impl TextReplica for YrsText {
    const ENGINE: &'static str = "kit Doc (yrs)";
    /// The kit's list for its engine, unchanged.
    const KNOWN_DEVIATIONS: &'static [Deviation] = k2_mpc_base::conformance::KNOWN_DEVIATIONS;

    type Version = Vec<(ClientId, u32)>;

    fn replica(agent: u64, seed: &str) -> Self {
        // The seed doc is built and encoded, and the replica loads that state: every replica
        // holds the same seed items, authored by SEED_AGENT.
        let mut first = Doc::new(SCHEMA).expect("conformance schema");
        if !seed.is_empty() {
            first
                .replace_text_as(SEED_AGENT, ROOT, seed)
                .expect("seed the doc");
        }
        let state = first.encode_state().expect("encode the seed");
        YrsText {
            doc: Doc::load(SCHEMA, &state).expect("load the seed"),
            agent,
        }
    }

    fn apply(&mut self, hunks: &[Hunk]) -> Result<(), Refused> {
        self.doc
            .edit_as(
                self.agent,
                &[Edit::Text {
                    root: ROOT.into(),
                    hunks: hunks.to_vec(),
                }],
            )
            .map(|_| ())
            .map_err(|e| Refused(e.to_string()))
    }

    fn sync_from(&mut self, from: &Self) {
        replica::sync(&from.doc, &mut self.doc).expect("sync between kit docs");
    }

    fn text(&self) -> String {
        self.doc.text(ROOT).expect("text root")
    }

    fn version(&self) -> Self::Version {
        replica::state(&self.doc)
    }
}

impl Events for YrsText {
    /// A Yrs update made by a kit doc.
    type Batch = Vec<u8>;

    fn events_since(&self, since: &Self::Version) -> Vec<u8> {
        replica::events_since(&self.doc, since).expect("encode the events since a version")
    }

    fn missing_for(&self, peer: &Self) -> Vec<u8> {
        self.events_since(&peer.version())
    }

    fn deliver(&mut self, batch: &Vec<u8>) -> Result<(), EventRefusal> {
        replica::deliver(&mut self.doc, batch).map_err(refusal)
    }
}

fn refusal(e: DocError) -> EventRefusal {
    match e {
        DocError::MissingDeps { .. } => EventRefusal::MissingDeps,
        other => EventRefusal::Other(other.to_string()),
    }
}

impl Branches for YrsText {
    fn branch(&self, agent: u64) -> Self {
        let state = self.doc.encode_state().expect("encode the doc");
        YrsText {
            doc: Doc::load(SCHEMA, &state).expect("load the branch"),
            agent,
        }
    }

    fn merge(&mut self, other: &Self) -> Result<(), EventRefusal> {
        replica::sync(&other.doc, &mut self.doc).map_err(refusal)
    }
}

impl Purge for YrsText {
    fn purge(&mut self, start: usize, end: usize) -> Result<Vec<u8>, Refused> {
        let before = self.version();
        self.apply(&[crate::del(start, end)])?;
        // a session's purge then re-encodes its doc and loads it again
        let state = self
            .doc
            .encode_state()
            .map_err(|e| Refused(e.to_string()))?;
        self.doc = Doc::load(SCHEMA, &state).map_err(|e| Refused(e.to_string()))?;
        replica::events_since(&self.doc, &before).map_err(|e| Refused(e.to_string()))
    }

    fn holds(&self, needle: &str) -> bool {
        let state = self.doc.encode_state().expect("encode the doc");
        !scan_bytes(&state, &[needle.as_bytes()]).is_empty()
    }
}

impl Blame for YrsText {
    /// (client, clock); clocks count UTF-16 code units.
    type CharId = (ClientId, u32);

    fn blame(&self) -> Vec<Blamed<(ClientId, u32)>> {
        let text = self.text();
        let runs = replica::blame(&self.doc, ROOT).expect("blame the text root");
        let mut out = Vec::with_capacity(text.len());
        let mut at = 0;
        for (bytes, client, clock) in runs {
            let mut k = clock;
            for ch in text[at..at + bytes].chars() {
                out.push(Blamed {
                    ch,
                    agent: client,
                    id: (client, k),
                });
                k += ch.len_utf16() as u32;
            }
            at += bytes;
        }
        assert_eq!(at, text.len(), "the kit's blame runs cover the text");
        out
    }
}

impl Anchors for YrsText {
    type Anchor = Anchor;

    fn anchor(&self, at: usize, side: Side) -> Option<Anchor> {
        let stick = match side {
            Side::Left => Stick::Left,
            Side::Right => Stick::Right,
        };
        replica::anchor(&self.doc, ROOT, at, stick).expect("anchor in the text root")
    }

    fn resolve(&self, anchor: &Anchor) -> Option<usize> {
        replica::resolve(&self.doc, ROOT, anchor).expect("resolve in the text root")
    }
}
