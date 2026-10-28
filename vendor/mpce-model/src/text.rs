//! Plain text as a FugueMax list (the reference model): events with `(agent, seq)` IDs and
//! parents, causal delivery, the Fugue tree (`crate::tree`), tombstones.
//!
//! Positions: `insert`/`delete` take char (Unicode scalar) indices; `edit_as`, `anchor` and
//! `resolve` take UTF-8 byte offsets into the visible text and refuse offsets that are not
//! scalar boundaries (E-TEXT-9).
//!
//! An event is an insert, a delete or a purge. Its parents are the creating replica's
//! frontier. `seq` counts the agent's events (all kinds) from 0, as in the Quint model.
//! An insert carries what Algorithm 1 broadcasts (the node's parent and side, and, for
//! FugueMax, its right origin) and also its left origin, which Def. 4 needs.
//!
//! Versions (E-HIST): a [`Version`] is a set of event IDs standing for its causal closure
//! (normally a frontier). `text_at`/`list_at`/`blame_at` read the state of exactly that
//! closure; `fork_at` makes a replica holding it; `restore_as` moves the text forward to it
//! with new events; `merge_from` merges a branch.
//!
//! Purge (E-ERASE): `purge_as` emits a `Op::Purge` event (IDs only) that deletes its targets
//! and erases their characters everywhere it is delivered: the insert events keep their IDs,
//! parents, origins and placement, and their `value` becomes `None`. A replica stores and
//! sends purged inserts content-less, so `log()`, `event()` and `diff_events` never carry
//! purged content. Purge knowledge only grows: a content-less copy of a known insert erases
//! the local content too, and a content-bearing copy of a purged insert is accepted as
//! already known and dropped (never re-admitted, never a `Conflict`). A content-less insert
//! received before its purge event is invisible (its character is unavailable) though not
//! yet deleted; once every replica has every event, all agree. No digest of purged content is
//! kept anywhere.

use crate::tree::{Origin, Parent, Side, SiblingRule, Tree, TreeError};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// `(agent, seq)`, ordered lexicographically (the paper's ID order).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id {
    pub agent: u64,
    pub seq: u64,
}

impl Id {
    pub fn new(agent: u64, seq: u64) -> Id {
        Id { agent, seq }
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({},{})", self.agent, self.seq)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Op {
    Insert {
        /// the character; `None` once its content is purged (E-ERASE)
        value: Option<char>,
        left: Origin<Id>,
        right: Origin<Id>,
        parent: Parent<Id>,
        side: Side,
    },
    Delete {
        target: Id,
    },
    /// E-ERASE: delete every target that is still live and erase the content of each (their
    /// insert events become content-less). Targets are inserts in the event's causal past.
    Purge {
        targets: BTreeSet<Id>,
    },
}

/// A version: a set of event IDs standing for its causal closure (normally a frontier, as
/// `Doc::version()` returns).
pub type Version = BTreeSet<Id>;

/// What a purge does to content. `KeepContent` is a mutant for the byte scan's planted
/// negative (as `SiblingRule::Fugue` is for FugueMax checks); real replicas use `Erase`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PurgeRule {
    /// E-ERASE: a purge deletes its targets and erases their characters
    Erase,
    /// mutant: a purge deletes its targets but keeps their characters
    KeepContent,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Event {
    pub id: Id,
    pub parents: BTreeSet<Id>,
    pub op: Op,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DocError {
    /// a parent of the event is not known yet (causal delivery)
    MissingDeps,
    /// a position beyond the visible text
    OutOfRange,
    /// a delete whose target insert is not known
    UnknownTarget,
    /// an event whose ID is known with different content (equivocation)
    Conflict,
    /// a byte offset that is not a UTF-8 scalar boundary of the visible text (E-TEXT-9)
    NotCharBoundary { offset: usize },
    /// hunks not sorted by start, or overlapping
    BadHunks,
    /// `insert`/`delete` on a replica created with `Doc::replica()` (use `edit_as`)
    NoAgent,
    /// a version names an event this replica has not received
    UnknownVersion { id: Id },
    /// a purge with no targets
    EmptyPurge,
    Tree(TreeError),
}

/// One hunk of an edit: replace the visible bytes `start..end` with `insert`. Offsets are UTF-8
/// byte offsets into the visible text before the edit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

/// A stable position (E-TEXT-10): an element plus a side. `Side::Right`: the gap just before
/// the element (the anchor sticks to the character after it); `Side::Left`: the gap just after
/// the element (it sticks to the character before it). `elem: None` is the start of the
/// document (`Side::Left`) or its end (`Side::Right`). A deleted element resolves to the gap
/// where it was.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Anchor {
    pub elem: Option<Id>,
    pub side: Side,
}

impl From<TreeError> for DocError {
    fn from(e: TreeError) -> DocError {
        DocError::Tree(e)
    }
}

/// One replica.
#[derive(Clone, Debug)]
pub struct Doc {
    agent: Option<u64>,
    tree: Tree<Id>,
    events: BTreeMap<Id, Event>,
    /// every event in delivery order (a causal order); purged inserts are stored content-less
    log: Vec<Event>,
    /// each known insert's character (`None`: purged)
    values: BTreeMap<Id, Option<char>>,
    deleted: BTreeSet<Id>,
    frontier: BTreeSet<Id>,
    purge_rule: PurgeRule,
    /// lowest next seq per agent, carried by `fork_at` (so an agent editing a branch does not
    /// reuse seqs its origin replica already had)
    seq_floor: BTreeMap<u64, u64>,
}

impl Doc {
    /// A FugueMax replica that edits as `agent`.
    pub fn new(agent: u64) -> Doc {
        Doc::with_rule(agent, SiblingRule::FugueMax)
    }

    /// A replica with an explicit right-sibling rule (`SiblingRule::Fugue` is plain Fugue).
    pub fn with_rule(agent: u64, rule: SiblingRule) -> Doc {
        Doc { agent: Some(agent), ..Doc::replica_with_rule(rule) }
    }

    /// A replica with no fixed agent: edits go through `edit_as(agent, ..)`.
    pub fn replica() -> Doc {
        Doc::replica_with_rule(SiblingRule::FugueMax)
    }

    pub fn replica_with_rule(rule: SiblingRule) -> Doc {
        Doc {
            agent: None,
            tree: Tree::new(rule),
            events: BTreeMap::new(),
            log: Vec::new(),
            values: BTreeMap::new(),
            deleted: BTreeSet::new(),
            frontier: BTreeSet::new(),
            purge_rule: PurgeRule::Erase,
            seq_floor: BTreeMap::new(),
        }
    }

    /// Set the purge rule (`PurgeRule::KeepContent` is a mutant for tests).
    pub fn set_purge_rule(&mut self, rule: PurgeRule) {
        self.purge_rule = rule;
    }

    pub fn purge_rule(&self) -> PurgeRule {
        self.purge_rule
    }

    /// The fixed agent (`None` for `Doc::replica()`).
    pub fn agent(&self) -> Option<u64> {
        self.agent
    }

    /// Insert `ch` at visible position `pos` (a char index; Algorithm 1 `insert(i, x)`) as this
    /// replica's agent; returns the event, already applied here, to be delivered elsewhere.
    pub fn insert(&mut self, pos: usize, ch: char) -> Result<Event, DocError> {
        let agent = self.agent.ok_or(DocError::NoAgent)?;
        self.insert_as(agent, pos, ch)
    }

    /// Delete the visible element at char index `pos` (Algorithm 1 `delete(i)`) as this
    /// replica's agent.
    pub fn delete(&mut self, pos: usize) -> Result<Event, DocError> {
        let agent = self.agent.ok_or(DocError::NoAgent)?;
        self.delete_as(agent, pos)
    }

    fn insert_as(&mut self, agent: u64, pos: usize, ch: char) -> Result<Event, DocError> {
        let vis = self.visible();
        if pos > vis.len() {
            return Err(DocError::OutOfRange);
        }
        // line 23: leftOrigin = node of the (i-1)-th value, or the root (start)
        let left = if pos == 0 { Origin::Start } else { Origin::Elem(vis[pos - 1]) };
        // line 24: rightOrigin = next node after leftOrigin, tombstones included
        let right = self.tree.next_after(left);
        // lines 25-28
        let (parent, side) = self.tree.placement(left, right)?;
        let e = Event { id: self.fresh_id(agent), parents: self.frontier.clone(), op: Op::Insert { value: Some(ch), left, right, parent, side } };
        self.deliver(&e)?;
        Ok(e)
    }

    fn delete_as(&mut self, agent: u64, pos: usize) -> Result<Event, DocError> {
        let vis = self.visible();
        let target = *vis.get(pos).ok_or(DocError::OutOfRange)?;
        let e = Event { id: self.fresh_id(agent), parents: self.frontier.clone(), op: Op::Delete { target } };
        self.deliver(&e)?;
        Ok(e)
    }

    /// The next ID for `agent`: one past the highest seq of its events known here, so seq
    /// continues for a returning agent (and at least the fork's floor, see `fork_at`).
    fn fresh_id(&self, agent: u64) -> Id {
        let next = self.events.range(Id::new(agent, 0)..=Id::new(agent, u64::MAX)).next_back().map_or(0, |(id, _)| id.seq + 1);
        Id::new(agent, next.max(self.seq_floor.get(&agent).copied().unwrap_or(0)))
    }

    /// Apply an edit as `agent`: `hunks` replace byte ranges of the current visible text,
    /// sorted by `start` (strictly increasing) and non-overlapping (`end <= next.start`). Each
    /// hunk deletes its range, then inserts its text there; every character is one event, with
    /// consecutive seqs for `agent`. One atomic batch: on any error nothing changes.
    pub fn edit_as(&mut self, agent: u64, hunks: &[Hunk]) -> Result<Vec<Event>, DocError> {
        let text = self.text();
        for (i, h) in hunks.iter().enumerate() {
            if h.start > h.end {
                return Err(DocError::BadHunks);
            }
            for off in [h.start, h.end] {
                if off > text.len() {
                    return Err(DocError::OutOfRange);
                }
                if !text.is_char_boundary(off) {
                    return Err(DocError::NotCharBoundary { offset: off });
                }
            }
            if let Some(n) = hunks.get(i + 1) {
                // Same rule as the kit's diff::validate: hunks may touch (n.start == h.end) but not
                // overlap, and two pure inserts at one offset are refused (their order would be
                // ambiguous). Touching hunks apply in order, the same as one coalesced hunk (spec D2).
                let both_pure_inserts_same_spot =
                    h.start == h.end && n.start == n.end && n.start == h.start;
                if n.start < h.end || n.start < h.start || both_pure_inserts_same_spot {
                    return Err(DocError::BadHunks);
                }
            }
        }
        let mut work = self.clone();
        let mut out = Vec::new();
        let mut shift: isize = 0; // chars inserted minus deleted so far
        for h in hunks {
            let cstart = text[..h.start].chars().count();
            let del = text[h.start..h.end].chars().count();
            let pos = (cstart as isize + shift) as usize;
            for _ in 0..del {
                out.push(work.delete_as(agent, pos)?);
            }
            for (k, ch) in h.insert.chars().enumerate() {
                out.push(work.insert_as(agent, pos + k, ch)?);
            }
            shift += h.insert.chars().count() as isize - del as isize;
        }
        *self = work;
        Ok(out)
    }

    /// Deliver an event (causal broadcast). Ok(true) if it was new, Ok(false) if it was
    /// already known. Re-delivery changes nothing, except that purge knowledge only grows: a
    /// content-less copy of a known insert erases the local content, and a content-bearing
    /// copy of a purged insert is accepted and its content dropped (E-ERASE-6). The same ID
    /// with any other difference is a `Conflict`.
    pub fn deliver(&mut self, e: &Event) -> Result<bool, DocError> {
        if let Some(old) = self.events.get(&e.id) {
            if old == e {
                return Ok(false);
            }
            let (ov, nv) = (insert_value(old), insert_value(e));
            if (ov == Some(None) || nv == Some(None)) && without_content(old) == without_content(e) {
                if nv == Some(None) {
                    self.erase(e.id);
                }
                return Ok(false);
            }
            return Err(DocError::Conflict);
        }
        if !e.parents.iter().all(|p| self.events.contains_key(p)) {
            return Err(DocError::MissingDeps);
        }
        match &e.op {
            Op::Insert { value, right, parent, side, .. } => {
                self.tree.integrate(e.id, *parent, *side, *right)?;
                self.values.insert(e.id, *value);
            }
            Op::Delete { target } => {
                if !self.values.contains_key(target) {
                    return Err(DocError::UnknownTarget);
                }
                self.deleted.insert(*target);
            }
            Op::Purge { targets } => {
                if targets.is_empty() {
                    return Err(DocError::EmptyPurge);
                }
                if !targets.iter().all(|t| self.values.contains_key(t)) {
                    return Err(DocError::UnknownTarget);
                }
            }
        }
        for p in &e.parents {
            self.frontier.remove(p);
        }
        self.frontier.insert(e.id);
        self.events.insert(e.id, e.clone());
        self.log.push(e.clone());
        if let Op::Purge { targets } = &e.op {
            for t in targets {
                self.deleted.insert(*t);
                if self.purge_rule == PurgeRule::Erase {
                    self.erase(*t);
                }
            }
        }
        Ok(true)
    }

    /// Drop the character of insert `id` everywhere this replica keeps it: its value and its
    /// stored event (in the map and in the log). IDs, parents and origins stay.
    fn erase(&mut self, id: Id) {
        if let Some(v) = self.values.get_mut(&id) {
            *v = None;
        }
        if let Some(Event { op: Op::Insert { value, .. }, .. }) = self.events.get_mut(&id) {
            *value = None;
        }
        for e in self.log.iter_mut().filter(|e| e.id == id) {
            if let Op::Insert { value, .. } = &mut e.op {
                *value = None;
            }
        }
    }

    /// Deliver a batch of events in a causal order (any input order, duplicates allowed;
    /// known events are skipped). Atomic: if some event cannot be delivered (missing
    /// dependencies, conflict, ...) nothing changes. Returns the number of new events.
    pub fn deliver_all(&mut self, events: &[Event]) -> Result<usize, DocError> {
        let mut work = self.clone();
        let mut pending: Vec<&Event> = events.iter().collect();
        let mut n = 0;
        loop {
            let before = pending.len();
            let mut rest = Vec::new();
            for e in pending {
                if work.has(e.id) || e.parents.iter().all(|p| work.has(*p)) {
                    if work.deliver(e)? {
                        n += 1;
                    }
                } else {
                    rest.push(e);
                }
            }
            pending = rest;
            if pending.is_empty() {
                break;
            }
            if pending.len() == before {
                return Err(DocError::MissingDeps);
            }
        }
        *self = work;
        Ok(n)
    }

    /// The events this replica has that a replica whose frontier is `remote_frontier` lacks,
    /// in causal (delivery) order. IDs of `remote_frontier` unknown here are ignored (the
    /// remote may then receive events it already has; delivery is idempotent).
    pub fn diff_events(&self, remote_frontier: &BTreeSet<Id>) -> Vec<Event> {
        let mut known: BTreeSet<Id> = BTreeSet::new();
        let mut stack: Vec<Id> = remote_frontier.iter().copied().filter(|id| self.has(*id)).collect();
        while let Some(id) = stack.pop() {
            if known.insert(id) {
                stack.extend(self.events[&id].parents.iter().copied());
            }
        }
        self.log.iter().filter(|e| !known.contains(&e.id)).cloned().collect()
    }

    /// Each visible character with the event that inserted it (blame).
    pub fn blame(&self) -> Vec<(char, Id)> {
        self.visible().into_iter().filter_map(|x| self.ch(x).map(|c| (c, x))).collect()
    }

    /// The current version (the frontier).
    pub fn version(&self) -> Version {
        self.frontier.clone()
    }

    /// The causal closure of `v`; `UnknownVersion` if `v` names an event not received here.
    fn closure(&self, v: &Version) -> Result<BTreeSet<Id>, DocError> {
        if let Some(id) = v.iter().find(|id| !self.has(**id)) {
            return Err(DocError::UnknownVersion { id: *id });
        }
        let mut c = BTreeSet::new();
        let mut stack: Vec<Id> = v.iter().copied().collect();
        while let Some(id) = stack.pop() {
            if c.insert(id) {
                stack.extend(self.events[&id].parents.iter().copied());
            }
        }
        Ok(c)
    }

    /// The visible elements of the state holding exactly the (causally closed) event set `c`:
    /// `order` (this replica's list order, tombstones included) restricted to the inserts of
    /// `c` that no delete or purge of `c` removes and whose content is not purged here.
    fn visible_in(&self, order: &[Id], c: &BTreeSet<Id>) -> Vec<Id> {
        let mut gone = BTreeSet::new();
        for id in c {
            match &self.events[id].op {
                Op::Delete { target } => {
                    gone.insert(*target);
                }
                Op::Purge { targets } => gone.extend(targets.iter().copied()),
                Op::Insert { .. } => {}
            }
        }
        order.iter().copied().filter(|x| c.contains(x) && !gone.contains(x) && self.ch(*x).is_some()).collect()
    }

    /// The visible elements at version `v` (E-HIST-1): the state of exactly the causal closure
    /// of `v`, minus content purged as far as this replica knows (E-ERASE-7). The same as
    /// `fork_at(v)?.visible()` (the list order of a causally closed subset is the restriction
    /// of the full order; tested).
    pub fn list_at(&self, v: &Version) -> Result<Vec<Id>, DocError> {
        let c = self.closure(v)?;
        Ok(self.visible_in(&self.tree.order(), &c))
    }

    /// The visible text at version `v` (see `list_at`).
    pub fn text_at(&self, v: &Version) -> Result<String, DocError> {
        Ok(self.list_at(v)?.into_iter().filter_map(|x| self.ch(x)).collect())
    }

    /// Blame at version `v` (E-HIST-4): each character visible at `v` with its inserting event.
    pub fn blame_at(&self, v: &Version) -> Result<Vec<(char, Id)>, DocError> {
        Ok(self.list_at(v)?.into_iter().filter_map(|x| self.ch(x).map(|c| (c, x))).collect())
    }

    /// A replica holding exactly the causal closure of `v` (a branch, E-HIST-8), with this
    /// replica's agent, sibling rule and purge rule. Content purged here stays purged there.
    /// Edit it with `edit_as`; merge it back with `merge_from` (either way). Its seqs for each
    /// agent start after the highest this replica had, so an agent may edit the branch without
    /// reusing seqs of events the origin already had; an agent must still not edit two
    /// replicas concurrently (that reuses seqs: `Conflict` on merge).
    pub fn fork_at(&self, v: &Version) -> Result<Doc, DocError> {
        let c = self.closure(v)?;
        let mut seq_floor = self.seq_floor.clone();
        for id in self.events.keys() {
            let f = seq_floor.entry(id.agent).or_insert(0);
            *f = (*f).max(id.seq + 1);
        }
        let mut d = Doc { agent: self.agent, purge_rule: self.purge_rule, seq_floor, ..Doc::replica_with_rule(self.tree.rule()) };
        for e in self.log.iter().filter(|e| c.contains(&e.id)) {
            d.deliver(e)?;
        }
        Ok(d)
    }

    /// Merge another replica (a branch, or any replica) into this one (E-HIST-9): deliver
    /// every event `other` has that this replica lacks. The same as
    /// `deliver_all(&other.diff_events(self.frontier()))`; idempotent; atomic. Returns the
    /// number of new events.
    pub fn merge_from(&mut self, other: &Doc) -> Result<usize, DocError> {
        let diff = other.diff_events(&self.frontier);
        self.deliver_all(&diff)
    }

    /// Restore forward to version `v` (E-HIST-2): new events by `agent` (history is never
    /// rewritten) after which, with no concurrent events, `text() == text_at(v)`. Visible
    /// characters not visible at `v` are deleted; characters visible at `v` but deleted now
    /// are inserted again as new content (new IDs, attributed to `agent`) at the gap where
    /// they were. Purged content is not visible at `v`, so it comes back as nothing
    /// (E-ERASE-7). One atomic `edit_as` batch; nothing to do gives no events.
    pub fn restore_as(&mut self, agent: u64, v: &Version) -> Result<Vec<Event>, DocError> {
        let order = self.tree.order();
        let then: BTreeSet<Id> = self.visible_in(&order, &self.closure(v)?).into_iter().collect();
        let mut hunks: Vec<Hunk> = Vec::new();
        let mut cur: Option<Hunk> = None;
        let mut off = 0; // byte offset in the current visible text
        for x in order {
            let now = self.shows(x);
            match (now, then.contains(&x)) {
                (true, true) => {
                    hunks.extend(cur.take());
                    off += self.ch(x).map_or(0, char::len_utf8);
                }
                (true, false) => {
                    let h = cur.get_or_insert(Hunk { start: off, end: off, insert: String::new() });
                    off += self.ch(x).map_or(0, char::len_utf8);
                    h.end = off;
                }
                (false, true) => {
                    let h = cur.get_or_insert(Hunk { start: off, end: off, insert: String::new() });
                    h.insert.extend(self.ch(x));
                }
                (false, false) => {}
            }
        }
        hunks.extend(cur);
        self.edit_as(agent, &hunks)
    }

    /// Purge (E-ERASE-1): one event by `agent` (parents: the current frontier) that deletes
    /// every still-live element of `ids` and erases the content of every one of them (their
    /// insert events keep IDs, parents and origins, and become content-less here and wherever
    /// the purge is delivered). `ids` must be known inserts (`UnknownTarget`) and not empty
    /// (`EmptyPurge`); already purged or deleted ones are fine. On error nothing changes.
    pub fn purge_as(&mut self, agent: u64, ids: &[Id]) -> Result<Event, DocError> {
        if ids.is_empty() {
            return Err(DocError::EmptyPurge);
        }
        if !ids.iter().all(|t| self.values.contains_key(t)) {
            return Err(DocError::UnknownTarget);
        }
        let e = Event { id: self.fresh_id(agent), parents: self.frontier.clone(), op: Op::Purge { targets: ids.iter().copied().collect() } };
        self.deliver(&e)?;
        Ok(e)
    }

    /// Every occurrence of `needle` in all content this replica holds, tombstones included (for
    /// a purge by text): the element IDs of each occurrence, in list order, sorted by where
    /// the occurrence starts. An occurrence is consecutive content-bearing elements spelling
    /// `needle` in (a) the list order with tombstones, or (b) the visible text of any state
    /// this replica passed through (each prefix of its log), or (c) the visible text each
    /// event's author saw right after making it (the event's causal closure). (b) and (c)
    /// find text that was contiguous on screen with a since-deleted character between (a typo
    /// fixed mid-word). Purged content has no characters, so it is never found.
    pub fn find_content(&self, needle: &str) -> Vec<Vec<Id>> {
        let pat: Vec<char> = needle.chars().collect();
        if pat.is_empty() {
            return Vec::new();
        }
        let order = self.tree.order();
        let mut found: BTreeSet<Vec<Id>> = BTreeSet::new();
        let mut scan = |seq: &[Id]| {
            for w in seq.windows(pat.len()) {
                if w.iter().zip(&pat).all(|(x, c)| self.ch(*x) == Some(*c)) {
                    found.insert(w.to_vec());
                }
            }
        };
        let all: Vec<Id> = order.iter().copied().filter(|x| self.ch(*x).is_some()).collect();
        scan(&all);
        let mut prefix = BTreeSet::new();
        for e in &self.log {
            prefix.insert(e.id);
            scan(&self.visible_in(&order, &prefix));
        }
        for e in &self.log {
            let c = self.closure(&BTreeSet::from([e.id])).expect("own event");
            scan(&self.visible_in(&order, &c));
        }
        let pos: BTreeMap<Id, usize> = order.iter().enumerate().map(|(i, x)| (*x, i)).collect();
        let mut out: Vec<Vec<Id>> = found.into_iter().collect();
        out.sort_by_key(|occ| (pos[&occ[0]], occ.clone()));
        out
    }

    /// A stable anchor at a byte offset of the visible text (E-TEXT-10). `Side::Right` sticks
    /// to the character after the offset, `Side::Left` to the character before it; with no such
    /// character it is the document's end or start.
    pub fn anchor(&self, byte_offset: usize, side: Side) -> Result<Anchor, DocError> {
        let text = self.text();
        if byte_offset > text.len() {
            return Err(DocError::OutOfRange);
        }
        if !text.is_char_boundary(byte_offset) {
            return Err(DocError::NotCharBoundary { offset: byte_offset });
        }
        let ci = text[..byte_offset].chars().count();
        let vis = self.visible();
        let elem = match side {
            Side::Right => vis.get(ci).copied(),
            Side::Left => ci.checked_sub(1).map(|i| vis[i]),
        };
        Ok(Anchor { elem, side })
    }

    /// The byte offset an anchor stands for now: the gap before (`Side::Right`) or after
    /// (`Side::Left`) its element, or, if the element is deleted, the gap where it was.
    /// `UnknownTarget` if this replica has not received the element.
    pub fn resolve(&self, a: &Anchor) -> Result<usize, DocError> {
        let id = match a.elem {
            None => return Ok(if a.side == Side::Left { 0 } else { self.text().len() }),
            Some(id) => id,
        };
        if !self.values.contains_key(&id) {
            return Err(DocError::UnknownTarget);
        }
        let mut off = 0;
        for x in self.tree.order() {
            if x == id {
                if a.side == Side::Left && self.shows(x) {
                    off += self.ch(x).map_or(0, char::len_utf8);
                }
                return Ok(off);
            }
            if self.shows(x) {
                off += self.ch(x).map_or(0, char::len_utf8);
            }
        }
        Err(DocError::UnknownTarget)
    }

    pub fn has(&self, id: Id) -> bool {
        self.events.contains_key(&id)
    }

    pub fn event(&self, id: Id) -> Option<&Event> {
        self.events.get(&id)
    }

    /// Every received event, in delivery order (a causal order).
    pub fn log(&self) -> &[Event] {
        &self.log
    }

    pub fn frontier(&self) -> &BTreeSet<Id> {
        &self.frontier
    }

    /// All elements in list order, tombstones included.
    pub fn list_with_tombstones(&self) -> Vec<Id> {
        self.tree.order()
    }

    /// The visible elements in list order (`values()` as IDs).
    pub fn visible(&self) -> Vec<Id> {
        self.tree.order().into_iter().filter(|x| self.shows(*x)).collect()
    }

    pub fn is_deleted(&self, id: Id) -> bool {
        self.deleted.contains(&id)
    }

    /// Whether `id` is a known insert whose content is purged here.
    pub fn is_purged(&self, id: Id) -> bool {
        self.values.get(&id) == Some(&None)
    }

    /// The character of a known insert, unless purged.
    fn ch(&self, id: Id) -> Option<char> {
        self.values.get(&id).copied().flatten()
    }

    /// Visible: not deleted, and the content is not purged.
    fn shows(&self, id: Id) -> bool {
        !self.deleted.contains(&id) && self.ch(id).is_some()
    }

    /// The visible text (`values()`).
    pub fn text(&self) -> String {
        self.visible().into_iter().filter_map(|x| self.ch(x)).collect()
    }

    /// Left and right origins of every element (for `crate::check`).
    pub fn origins(&self) -> BTreeMap<Id, (Origin<Id>, Origin<Id>)> {
        self.events
            .values()
            .filter_map(|e| match e.op {
                Op::Insert { left, right, .. } => Some((e.id, (left, right))),
                Op::Delete { .. } | Op::Purge { .. } => None,
            })
            .collect()
    }

    /// Deliver every event of `from` that this replica lacks, in `from`'s delivery order.
    pub fn sync_from(&mut self, from: &Doc) -> Result<usize, DocError> {
        let mut n = 0;
        for e in from.log() {
            if !self.has(e.id) {
                self.deliver(e)?;
                n += 1;
            }
        }
        Ok(n)
    }
}

/// An insert's value (`Some(None)`: a content-less insert), or `None` for other events.
fn insert_value(e: &Event) -> Option<Option<char>> {
    match e.op {
        Op::Insert { value, .. } => Some(value),
        _ => None,
    }
}

/// The event with its content dropped (other events unchanged).
fn without_content(e: &Event) -> Event {
    let mut e = e.clone();
    if let Op::Insert { value, .. } = &mut e.op {
        *value = None;
    }
    e
}
