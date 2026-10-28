//! [`TextReplica`] on the reference model `mpce-model` (FugueMax, the oracle of record),
//! through its native API (v0.3.0), with no semantics of the adapter's own:
//!
//! - edits: `edit_as(agent, hunks)` (UTF-8 byte offsets, one atomic batch; it refuses
//!   offsets off a scalar boundary, out of range, out of order or overlapping);
//! - sync and [`Events`]: a batch is the model's events in causal order, from
//!   `diff_events(frontier)`, delivered with `deliver_all` (all or nothing, any order,
//!   duplicates skipped). The model's `MissingDeps` and `Conflict` map to
//!   [`EventRefusal::MissingDeps`] and [`EventRefusal::Equivocation`];
//! - [`Blame`]: `blame()`, a character's ID is the model's `Id(agent, seq)`;
//! - [`Anchors`]: `anchor(byte offset, side)` and `resolve` (`UnknownTarget`, a character
//!   this replica hasn't received, is `None`).
//! - [`Branches`] and [`BranchAt`]: `fork_at(version)` (a fork's seqs start after the
//!   origin's), merged with `merge_from`.
//! - [`Purge`]: `purge_as(agent, ids)` of the span's characters (one event); the byte scan is
//!   the model's `find_content`, for the needle and each of its 8-byte windows (every
//!   content-bearing element it holds, tombstones and past states included).
//! - [`BlameAt`]: the model's `blame_at(version)`.
//! - [`ModelHistory`], the doc-level capabilities ([`Versions`], [`HistoryPurge`]): one
//!   replica that every agent edits with `edit_as`; a version is its frontier, shown with
//!   `text_at`; a restore is the kit's operation, `edit_as` of the kit's edit pipeline from
//!   the live text to the version's (the model's own `restore_as` is model-internal); a
//!   purge by text is `find_content` then `purge_as`.
//!
//! Replicas are `Doc::replica()` (no fixed agent); the adapter keeps the agent it edits as.
//!
//! Beyond the trait, the model can check a final state against the Fugue paper's Def. 4 and
//! the strong list specification ([`check_lists`]): the suite runs them on every rule's final
//! state, an extra assertion.

use std::cell::Cell;
use std::collections::BTreeSet;

use mpce_model::check;
use mpce_model::text::{Anchor, Doc, DocError, Event, Hunk as ModelHunk, Id, Op, PurgeRule};
use mpce_model::tree::{Origin, Parent, Side as TreeSide};

use crate::caps::{
    Anchors, Blame, BlameAt, Blamed, BranchAt, Branches, Change, EventRefusal, Events,
    HistoryPurge, Purge, Side, Versions,
};
use crate::{Deviation, Hunk, Refused, TextReplica, SEED_AGENT};
use k2_mpc_base::text::{diff, lines::align_insertions};

/// One replica of the reference model, editing as one agent.
#[derive(Clone)]
pub struct ModelText {
    doc: Doc,
    agent: u64,
}

impl ModelText {
    /// The model's own replica, for its checkers.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }
}

thread_local! {
    /// The planted negative for the byte scans: replicas made while it is set purge with
    /// `PurgeRule::KeepContent` (delete, keep the characters).
    static KEEP_CONTENT: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` with every model replica it makes purging with the mutant rule
/// `PurgeRule::KeepContent`: a purge rule run inside must fail (its byte scan finds the
/// content). Test-only; this thread only.
pub fn with_keep_content_mutant<R>(f: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            KEEP_CONTENT.with(|k| k.set(false));
        }
    }
    KEEP_CONTENT.with(|k| k.set(true));
    let _reset = Reset;
    f()
}

/// A replica holding `seed`, typed by SEED_AGENT the same way on every replica (the same
/// events).
fn seeded(seed: &str) -> Doc {
    let mut doc = Doc::replica();
    if KEEP_CONTENT.with(Cell::get) {
        doc.set_purge_rule(PurgeRule::KeepContent);
    }
    if !seed.is_empty() {
        let mut s = Doc::replica();
        let events = s
            .edit_as(
                SEED_AGENT,
                &[ModelHunk {
                    start: 0,
                    end: 0,
                    insert: seed.into(),
                }],
            )
            .expect("seed the model");
        doc.deliver_all(&events).expect("deliver the seed");
    }
    doc
}

fn model_hunks(hunks: &[Hunk]) -> Vec<ModelHunk> {
    hunks
        .iter()
        .map(|h| ModelHunk {
            start: h.start,
            end: h.end,
            insert: h.insert.clone(),
        })
        .collect()
}

/// `needle` and each of its 8-byte windows ([`k2_mpc_base::store::MIN_MATCH`]), as the
/// kit's byte scan matches them.
fn windows(needle: &str) -> Vec<&str> {
    const W: usize = k2_mpc_base::store::MIN_MATCH;
    let mut out = vec![needle];
    let b: Vec<usize> = (0..=needle.len())
        .filter(|i| needle.is_char_boundary(*i))
        .collect();
    for (k, &s) in b.iter().enumerate() {
        if let Some(&e) = b[k..].iter().find(|&&e| e >= s + W) {
            out.push(&needle[s..e]);
        }
    }
    out
}

/// Does `doc` hold any content of `needle` (whole or by a window), anywhere?
fn doc_holds(doc: &Doc, needle: &str) -> bool {
    windows(needle)
        .into_iter()
        .any(|w| !doc.find_content(w).is_empty())
}

fn refusal(e: DocError) -> EventRefusal {
    match e {
        DocError::MissingDeps => EventRefusal::MissingDeps,
        DocError::Conflict => EventRefusal::Equivocation,
        other => EventRefusal::Other(format!("{other:?}")),
    }
}

impl TextReplica for ModelText {
    const ENGINE: &'static str = "mpce-model (FugueMax)";
    /// Empty: the reference model conforms (asserted by `model_list_is_empty`).
    const KNOWN_DEVIATIONS: &'static [Deviation] = &[];

    /// The frontier: deletes included, so equal frontiers mean the same events.
    type Version = BTreeSet<Id>;

    fn replica(agent: u64, seed: &str) -> Self {
        ModelText {
            doc: seeded(seed),
            agent,
        }
    }

    fn apply(&mut self, hunks: &[Hunk]) -> Result<(), Refused> {
        self.doc
            .edit_as(self.agent, &model_hunks(hunks))
            .map(|_| ())
            .map_err(|e| Refused(format!("model edit_as: {e:?}")))
    }

    fn sync_from(&mut self, from: &Self) {
        let events = from.doc.diff_events(self.doc.frontier());
        self.doc.deliver_all(&events).expect("causal delivery");
    }

    fn text(&self) -> String {
        self.doc.text()
    }

    fn version(&self) -> Self::Version {
        self.doc.frontier().clone()
    }
}

impl Events for ModelText {
    /// The model's events, in causal order.
    type Batch = Vec<Event>;

    fn events_since(&self, since: &Self::Version) -> Vec<Event> {
        self.doc.diff_events(since)
    }

    fn missing_for(&self, peer: &Self) -> Vec<Event> {
        self.doc.diff_events(peer.doc.frontier())
    }

    fn deliver(&mut self, batch: &Vec<Event>) -> Result<(), EventRefusal> {
        self.doc.deliver_all(batch).map(|_| ()).map_err(refusal)
    }

    /// Two inserts by an agent no rule edits as, each naming the other as its parent.
    fn cyclic_batch() -> Option<Vec<Event>> {
        let (x, y) = (Id::new(999, 0), Id::new(999, 1));
        let insert = |value| Op::Insert {
            value: Some(value),
            left: Origin::Start,
            right: Origin::End,
            parent: Parent::Root,
            side: TreeSide::Right,
        };
        Some(vec![
            Event {
                id: x,
                parents: [y].into(),
                op: insert('x'),
            },
            Event {
                id: y,
                parents: [x].into(),
                op: insert('y'),
            },
        ])
    }
}

impl Blame for ModelText {
    type CharId = Id;

    fn blame(&self) -> Vec<Blamed<Id>> {
        self.doc
            .blame()
            .into_iter()
            .map(|(ch, id)| Blamed {
                ch,
                agent: id.agent,
                id,
            })
            .collect()
    }
}

impl Anchors for ModelText {
    type Anchor = Anchor;

    fn anchor(&self, at: usize, side: Side) -> Option<Anchor> {
        let side = match side {
            Side::Left => TreeSide::Left,
            Side::Right => TreeSide::Right,
        };
        self.doc.anchor(at, side).ok()
    }

    fn resolve(&self, anchor: &Anchor) -> Option<usize> {
        match self.doc.resolve(anchor) {
            Ok(at) => Some(at),
            Err(DocError::UnknownTarget) => None,
            Err(e) => panic!("model resolve: {e:?}"),
        }
    }
}

impl Branches for ModelText {
    fn branch(&self, agent: u64) -> Self {
        ModelText {
            doc: self.doc.fork_at(&self.doc.version()).expect("fork now"),
            agent,
        }
    }

    fn merge(&mut self, other: &Self) -> Result<(), EventRefusal> {
        self.doc.merge_from(&other.doc).map(|_| ()).map_err(refusal)
    }
}

impl BranchAt for ModelText {
    fn branch_at(&self, at: &Self::Version, agent: u64) -> Result<Self, Refused> {
        let doc = self
            .doc
            .fork_at(at)
            .map_err(|e| Refused(format!("model fork_at: {e:?}")))?;
        Ok(ModelText { doc, agent })
    }
}

impl Purge for ModelText {
    fn purge(&mut self, start: usize, end: usize) -> Result<Vec<Event>, Refused> {
        let text = self.doc.text();
        if start > end || end > text.len() || !text.is_char_boundary(start) {
            return Err(Refused(format!(
                "purge {start}..{end} of {} bytes",
                text.len()
            )));
        }
        let mut at = 0;
        let mut ids = Vec::new();
        for (ch, id) in self.doc.blame() {
            if at >= start && at < end {
                ids.push(id);
            }
            at += ch.len_utf8();
        }
        let e = self
            .doc
            .purge_as(self.agent, &ids)
            .map_err(|e| Refused(format!("model purge_as: {e:?}")))?;
        Ok(vec![e])
    }

    fn holds(&self, needle: &str) -> bool {
        doc_holds(&self.doc, needle)
    }
}

impl BlameAt for ModelText {
    fn blame_at(&self, v: &Self::Version) -> Result<Vec<Blamed<Id>>, Refused> {
        let b = self
            .doc
            .blame_at(v)
            .map_err(|e| Refused(format!("model blame_at: {e:?}")))?;
        Ok(b.into_iter()
            .map(|(ch, id)| Blamed {
                ch,
                agent: id.agent,
                id,
            })
            .collect())
    }
}

/// The doc-level capabilities on the model: one replica every agent edits.
pub struct ModelHistory {
    doc: Doc,
}

impl Versions for ModelHistory {
    type Engine = ModelText;
    /// A frontier.
    type VersionId = BTreeSet<Id>;

    fn open(seed: &str) -> Self {
        ModelHistory { doc: seeded(seed) }
    }

    fn edit(&mut self, agent: u64, hunks: &[Hunk]) -> Result<(), Refused> {
        self.doc
            .edit_as(agent, &model_hunks(hunks))
            .map(|_| ())
            .map_err(|e| Refused(format!("model edit_as: {e:?}")))
    }

    fn text(&self) -> String {
        self.doc.text()
    }

    fn version(&mut self, _agent: u64) -> BTreeSet<Id> {
        self.doc.version()
    }

    fn show(&self, v: &BTreeSet<Id>) -> Result<String, Refused> {
        self.doc
            .text_at(v)
            .map_err(|e| Refused(format!("model text_at: {e:?}")))
    }

    /// The kit's restore (`edit_as` of the pipeline's hunks from the live text to `v`'s);
    /// the model's own `restore_as` is not used.
    fn restore_applied(
        &mut self,
        agent: u64,
        v: &BTreeSet<Id>,
    ) -> Result<Option<(Change, Vec<Hunk>)>, Refused> {
        let target = self.show(v)?;
        let live = self.doc.text();
        let hunks = align_insertions(&live, diff::hunks(&live, &target));
        if hunks.is_empty() {
            return Ok(None);
        }
        let events = self
            .doc
            .edit_as(agent, &model_hunks(&hunks))
            .map_err(|e| Refused(format!("model edit_as: {e:?}")))?;
        // one batch: attributed to whoever made its events (all of them, the restorer)
        if let Some(e) = events.iter().find(|e| e.id.agent != agent) {
            return Err(Refused(format!(
                "a restore's event by another agent: {e:?}"
            )));
        }
        let change = Change {
            author: agent,
            roots: vec![0],
        };
        Ok(Some((change, hunks)))
    }
}

impl HistoryPurge for ModelHistory {
    fn purge_text(&mut self, owner: u64, needle: &str) -> Result<(), Refused> {
        let ids: BTreeSet<Id> = self
            .doc
            .find_content(needle)
            .into_iter()
            .flatten()
            .collect();
        let ids: Vec<Id> = ids.into_iter().collect();
        self.doc
            .purge_as(owner, &ids)
            .map(|_| ())
            .map_err(|e| Refused(format!("model purge_as: {e:?}")))
    }

    fn holds(&self, needle: &str) -> bool {
        doc_holds(&self.doc, needle)
    }
}

/// Check the replicas' final lists (tombstones included) against Def. 4, with Lemma 5's
/// exception, on each replica, and the strong list specification across all of them.
/// The replicas must have converged (the same origins on each).
pub fn check_lists(replicas: &[ModelText]) -> Result<(), String> {
    let origins = replicas[0].doc.origins();
    let lists: Vec<Vec<Id>> = replicas
        .iter()
        .map(|r| r.doc.list_with_tombstones())
        .collect();
    for (i, l) in lists.iter().enumerate() {
        check::def4(l, &origins).map_err(|v| format!("replica {i}: Def. 4: {v:?}"))?;
    }
    check::strong_list_spec(&lists, &origins).map_err(|v| format!("strong list spec: {v:?}"))
}
