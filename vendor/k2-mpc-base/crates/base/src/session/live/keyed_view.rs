//! A keyed doc's **live view** (not a table's): the doc's keyed state, kept in step with every
//! change, so a structured call costs what it touches, not the whole doc.
//!
//! - **Handed out without a copy.** A structured verb's [`View`] shares the live view's state
//!   ([`View::shared`]); building it once costs O(doc) (the first call after a load), and every
//!   call after that O(1).
//! - **Kept in step by every change**, from the change's applied stream: a map op sets or
//!   removes its key (O(log n) each); a list op is replayed on the list from its positions
//!   before and after (an insert, a move, a delete, a restore, a set, a drop: no entry read,
//!   only the slice of the list after the position shifted); a text root a change touched is
//!   read afresh (O(that root)). An op that doesn't fit the view (a kit bug, refused loudly in
//!   test builds) has its list read afresh instead. A local change (`commit`) and an outside
//!   save (`ingest_structured`) keep it; a reload keeps it when the doc is at its version, and
//!   a purge drops it.
//! - **No places.** A list root of the live view carries no `places` (empty, as a version's
//!   view): keeping every element's place in step would need the engine's order, rebuilt after
//!   each change to the list. Its live order and its deleted elements are kept exactly.
//! - **Read bases without copies.** A read records its version as a base for `--base` without
//!   copying the doc: the session keeps, while any read base lives, the reverse of every
//!   change since the oldest one (the [`Undo`] journal: each map key's value before, each list
//!   op as applied, each re-read root's content before, moved out of the view, not copied).
//!   A verb that names
//!   positions at an older base (`#N` under `--base`, R-1) gets that version rebuilt: the live
//!   view copied, the journal undone back to it. The read bases are kept and counted as before
//!   (the view's size per base, `Config::read_bases_max_bytes`), and the journal's re-read
//!   roots count against the same bound.
//! - **The size counter** for `Config::keyed_max_entries` is taken from the view when it is
//!   built (O(roots)), then kept in step by every change as before.
//!
//! In test builds (and with the `differential` feature) every hand-out is checked against the
//! engine doc, and every rebuilt read base against a snapshot taken with it, unless a thread
//! switched the checks off (`differential::set_view_checks`), as the visit counts and the
//! measurements do.

use super::*;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use crate::elem::{
    Applied as ElemApplied, Effect, ElemState, Key, KeyedRoot, KeyedState, ListApplied, ListOpKind,
    Value,
};
use crate::session::view::{root_bytes, state_bytes, value_bytes};
use crate::table;

/// The live view: the doc's keyed state at `seq`, its size in `View::bytes`'s count, and its
/// content digest: a sum of one term per thing a pack renders (a list's live elements, each
/// with the key before it; a map's entries; a text), kept by every change in O(its ops), so
/// "is the doc's content the synced content?" costs O(1) (`resync_if_synced`). `None` when an
/// op didn't fit the view (it is rebuilt with the view).
pub(super) struct LiveView {
    state: Arc<KeyedState>,
    seq: Seq,
    bytes: usize,
    digest: Option<u128>,
}

/// The reverse of one change (or of one commit's steps, which no read sits between): undone
/// newest first, it gives the view before it.
pub(super) struct Undo {
    /// The change's (first) log position: undone for a base older than it.
    seq: Seq,
    parts: Vec<UndoPart>,
    /// The re-read roots' size, counted against the read bases' bound.
    bytes: usize,
}

enum UndoPart {
    /// A map root's keys as they were, in the order they were written.
    Map {
        root: usize,
        before: Vec<(String, Option<Value>)>,
    },
    /// A list root's ops as applied (positions and values before and after), undone last first.
    List { root: usize, ops: Vec<ListApplied> },
    /// A root's whole content before the change (a list or text root, read afresh after it).
    Root { root: usize, before: KeyedRoot },
}

/// The planted fault [`crate::differential::KeyedMutant::ReadVisitsWholeDoc`].
pub(super) fn planted_read_visits_whole_doc() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::ReadVisitsWholeDoc)
    )
}

/// The planted fault [`crate::differential::KeyedMutant::ListReReadsRoot`].
fn planted_list_rereads_root() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::ListReReadsRoot)
    )
}

/// Whether the live view's checks are on (test builds, unless switched off on this thread).
pub(super) fn checks_on() -> bool {
    #[cfg(any(test, feature = "differential"))]
    {
        crate::differential::view_checks()
    }
    #[cfg(not(any(test, feature = "differential")))]
    {
        false
    }
}

impl Session {
    /// Whether this doc keeps a live view: a keyed doc that isn't a table (a table's verbs read
    /// its rendered file and its keyed state as before).
    pub(super) fn keeps_view(&self) -> bool {
        self.structured && !table::is_table(&self.schema)
    }

    /// The view a structured verb computes on: the live view for a keyed doc that isn't a
    /// table (no copy), else the doc's keyed state, read whole.
    pub(super) fn verb_view(&self, st: &mut State) -> Result<View> {
        let view = match self.keeps_view() {
            true => View::shared(self.live_state(st)?),
            false => View::new(st.doc.keyed_state()?),
        };
        Ok(view
            .with_codec(&st.codec)
            .with_max_value(self.config.grid_max_value_len))
    }

    /// The live view's state: built from the doc on first need (O(doc)), then shared (O(1)).
    pub(super) fn live_state(&self, st: &mut State) -> Result<Arc<KeyedState>> {
        if let Some(v) = &st.live_view {
            if v.seq == st.version {
                if checks_on() {
                    let doc = without_places(st.doc.keyed_state()?);
                    assert!(
                        *v.state == doc,
                        "the live view of {} at v{} is not the doc's",
                        st.key.path,
                        v.seq
                    );
                    assert_eq!(v.bytes, state_bytes(&doc), "the live view's size");
                    if let Some(d) = v.digest {
                        assert_eq!(d, state_digest(&doc), "the live view's content digest");
                    }
                }
                return Ok(v.state.clone());
            }
            // a path changed the doc without keeping the view: every path that changes a keyed
            // doc keeps it or drops it, so this is a kit bug; the read bases it served go
            assert!(
                !checks_on(),
                "the live view of {} is at v{}, the doc at v{}: a change didn't keep it",
                st.key.path,
                v.seq,
                st.version
            );
            Self::drop_live_view(st);
        }
        let state = Arc::new(without_places(st.doc.keyed_state()?));
        let bytes = state_bytes(&state);
        let digest = state_digest(&state);
        if st.keyed_size.is_none() {
            st.keyed_size = Some(keyed_entries(&state));
        }
        st.live_view = Some(LiveView {
            state: state.clone(),
            seq: st.version,
            bytes,
            digest: Some(digest),
        });
        // the file holds this content when the doc is at the sync point and known to render
        // the synced bytes (`store::Rendered`)
        if let Some(d) = &st.disk {
            if d.synced_seq == st.version && self.known_to_render(st, st.version, &d.synced_hash) {
                st.synced_digest = Some((st.version, digest));
            }
        }
        Ok(state)
    }

    /// Whether an agent's change of `edits` can cut a version before it (for lists:
    /// more elements touched than `Config::agent_version_elems`, or more than
    /// `Config::agent_version_percent` of the live elements). A change touches at most one
    /// element per list op, so a keyed doc's live view (its lists' lengths, O(roots)) tells
    /// when it can't: then the doc's content isn't copied first. A table: always, as before.
    pub(super) fn cut_possible(&self, st: &mut State, edits: &[Edit]) -> Result<bool> {
        if !self.keeps_view() {
            return Ok(true);
        }
        let ops: usize = edits
            .iter()
            .map(|e| match e {
                Edit::List { ops, .. } => ops.len(),
                _ => 0,
            })
            .sum();
        let c = &self.config;
        if ops > c.agent_version_elems {
            return Ok(true);
        }
        let total: usize = self
            .live_state(st)?
            .roots
            .iter()
            .map(|(_, r)| match r {
                KeyedRoot::List { live, .. } => live.len(),
                _ => 0,
            })
            .sum();
        Ok(ops.saturating_mul(100) > total.saturating_mul(c.agent_version_percent))
    }

    /// The live view's content digest, when it is at the doc's version and has one.
    pub(super) fn view_digest(st: &State) -> Option<u128> {
        st.live_view
            .as_ref()
            .filter(|v| v.seq == st.version)
            .and_then(|v| v.digest)
    }

    /// Whether element `key` of list root `root` is live: from the live view when it is at the
    /// doc's version (not deleted there, and known to the engine, O(log n) and O(1)), so a
    /// change's presence bookkeeping doesn't make the engine rebuild the list's order; else
    /// the engine's order (`Doc::elem`).
    pub(super) fn elem_live(&self, st: &State, root: &str, key: &Key) -> bool {
        let fast = self.keeps_view().then(|| {
            let v = st.live_view.as_ref().filter(|v| v.seq == st.version)?;
            let (_, r) = v.state.roots.iter().find(|(r, _)| r == root)?;
            let KeyedRoot::List { hidden, .. } = r else {
                return Some(false);
            };
            Some(!hidden.contains_key(key) && matches!(st.doc.elem_value(root, key), Ok(Some(_))))
        });
        let slow = || matches!(st.doc.elem(root, key), Ok(ElemState::Live(_)));
        match fast.flatten() {
            Some(live) => {
                if checks_on() {
                    assert_eq!(
                        live,
                        slow(),
                        "{root}/{key}: the live view's liveness is the doc's"
                    );
                }
                live
            }
            None => slow(),
        }
    }

    /// The live view's size in `View::bytes`'s count (the read bases' accounting).
    pub(super) fn live_view_bytes(&self, st: &mut State) -> Result<usize> {
        self.live_state(st)?;
        Ok(st.live_view.as_ref().map_or(0, |v| v.bytes))
    }

    /// Drop the live view (a path that changed the doc without its stream: a purge, a reload
    /// to another version), and with it what only the journal could rebuild: the read bases
    /// older than now.
    pub(super) fn drop_live_view(st: &mut State) {
        st.live_view = None;
        // the view index built from it goes with it (values may be erased)
        st.view_cache = None;
        // and the pack's verb state kept with it
        st.verb_state = None;
        st.undo_journal.clear();
        st.journal_bytes = 0;
        let now = st.version;
        let mut freed = 0;
        st.read_bases.retain(|b| {
            let keep = b.version >= now || b.view.is_some();
            if !keep {
                freed += b.bytes;
            }
            keep
        });
        st.read_base_bytes -= freed;
    }

    /// Keep the live view in step with a change that took the doc from `before` to `after`
    /// (the doc's version already `after`): its applied `stream`, and the text roots it
    /// changed (`texts`). A view at another version than `before`, or none, is dropped, to be
    /// built afresh, with the read bases only the journal could rebuild; so is one a root
    /// can't be read afresh for (the change has landed: it isn't failed for that).
    pub(super) fn view_after(
        &self,
        st: &mut State,
        before: Seq,
        after: Seq,
        stream: &[ElemApplied],
        texts: &[&str],
        from: verb_state::ChangeFrom,
    ) {
        if !self.keeps_view() {
            return;
        }
        if self
            .view_after_inner(st, before, after, stream, texts)
            .is_err()
        {
            Self::drop_live_view(st);
            return;
        }
        // the kept view index learns what the change touched, to be kept through it
        self.view_index_after(st, before, after, stream);
        // the pack's verb state is kept through it now: no verb reads it behind the doc
        self.verb_state_after(st, before, after, stream, from);
    }

    fn view_after_inner(
        &self,
        st: &mut State,
        before: Seq,
        after: Seq,
        stream: &[ElemApplied],
        texts: &[&str],
    ) -> Result<()> {
        match &st.live_view {
            Some(v) if v.seq == before => {}
            _ => {
                Self::drop_live_view(st);
                return Ok(());
            }
        }
        // the journal is needed while a read base older than `after` lives
        let journal = !st.read_bases.is_empty();
        let mut parts: Vec<UndoPart> = Vec::new();
        let mut reread: Vec<usize> = Vec::new();
        let mut undo_bytes = 0;
        let v = st.live_view.as_mut().expect("checked above");
        if Arc::strong_count(&v.state) > 1 {
            // someone still holds the old state (a verb's view, kept): the change copies it
            entry_visits!(crate::session::view::state_entries(&v.state));
        }
        let state = Arc::make_mut(&mut v.state);
        let mut bytes = v.bytes as i64;
        let (mut digest, mut digest_ok) = (v.digest.unwrap_or(0), v.digest.is_some());
        let index =
            |state: &KeyedState, root: &str| state.roots.iter().position(|(r, _)| r == root);
        for x in stream {
            let Some(ix) = index(state, x.root()) else {
                continue;
            };
            match (x, &mut state.roots[ix].1) {
                (ElemApplied::Map { ops, .. }, KeyedRoot::Map(m)) => {
                    let mut before = Vec::new();
                    for o in ops {
                        entry_visits!(1);
                        let old = match &o.after {
                            Some(val) => {
                                bytes += (o.key.len() + value_bytes(val)) as i64;
                                digest = digest.wrapping_add(map_term(ix, &o.key, val));
                                m.insert(o.key.clone(), val.clone())
                            }
                            None => m.remove(&o.key),
                        };
                        if let Some(old) = &old {
                            bytes -= (o.key.len() + value_bytes(old)) as i64;
                            digest = digest.wrapping_sub(map_term(ix, &o.key, old));
                        }
                        if journal {
                            before.push((o.key.clone(), old));
                        }
                    }
                    if !before.is_empty() {
                        parts.push(UndoPart::Map { root: ix, before });
                    }
                }
                (ElemApplied::List { ops, .. }, KeyedRoot::List { live, hidden, .. })
                    if !reread.contains(&ix) && !planted_list_rereads_root() =>
                {
                    prof!("view.list_delta");
                    match apply_list(live, hidden, ops, ix, &mut digest) {
                        Some(delta) => {
                            bytes += delta;
                            if journal {
                                undo_bytes += ops_bytes(ops);
                                parts.push(UndoPart::List {
                                    root: ix,
                                    ops: ops.clone(),
                                });
                            }
                        }
                        None => {
                            assert!(
                                !checks_on(),
                                "a list op on {:?} of {} doesn't fit the live view",
                                x.root(),
                                st.key.path
                            );
                            digest_ok = false;
                            reread.push(ix);
                        }
                    }
                }
                _ => {
                    if !reread.contains(&ix) {
                        reread.push(ix);
                    }
                }
            }
        }
        for t in texts {
            if let Some(ix) = index(state, t) {
                if !reread.contains(&ix) {
                    reread.push(ix);
                }
            }
        }
        for ix in reread {
            prof!("view.reread");
            let fresh = root_without_places(st.doc.keyed_root(&state.roots[ix].0)?);
            bytes += root_bytes(&fresh) as i64;
            digest = digest.wrapping_add(root_digest(ix, &fresh));
            let old = std::mem::replace(&mut state.roots[ix].1, fresh);
            let b = root_bytes(&old);
            bytes -= b as i64;
            digest = digest.wrapping_sub(root_digest(ix, &old));
            if journal {
                undo_bytes += b;
                parts.push(UndoPart::Root {
                    root: ix,
                    before: old,
                });
            }
        }
        v.bytes = bytes.max(0) as usize;
        v.digest = digest_ok.then_some(digest);
        v.seq = after;
        if journal && !parts.is_empty() {
            st.journal_bytes += undo_bytes;
            st.undo_journal.push_back(Undo {
                seq: before + 1,
                parts,
                bytes: undo_bytes,
            });
            self.bound_journal(st);
        }
        Ok(())
    }

    /// Keep the read bases and the journal's re-read roots within
    /// [`Config::read_bases_max_bytes`]: the oldest read bases go first.
    fn bound_journal(&self, st: &mut State) {
        let max = self.config.read_bases_max_bytes;
        while st.read_base_bytes + st.journal_bytes > max {
            let Some(old) = st.read_bases.pop_front() else {
                break;
            };
            st.read_base_bytes -= old.bytes;
            Self::prune_journal(st);
        }
    }

    /// Forget the reverse of changes no read base is older than.
    pub(super) fn prune_journal(st: &mut State) {
        let oldest = st
            .read_bases
            .iter()
            .filter(|b| b.view.is_none())
            .map(|b| b.version)
            .min();
        match oldest {
            None => {
                st.undo_journal.clear();
                st.journal_bytes = 0;
            }
            Some(o) => {
                while st.undo_journal.front().is_some_and(|u| u.seq <= o) {
                    let u = st.undo_journal.pop_front().expect("non-empty");
                    st.journal_bytes -= u.bytes;
                }
            }
        }
    }

    /// The doc at read base `base` (older than the live view): the live view copied (O(doc)),
    /// then the journal undone back to it. `None` when the journal can't reach it.
    pub(super) fn rebuilt_base(&self, st: &mut State, base: Seq) -> Result<Option<KeyedState>> {
        let live = self.live_state(st)?;
        entry_visits!(crate::session::view::state_entries(&live));
        let mut state = (*live).clone();
        for u in st.undo_journal.iter().rev().take_while(|u| u.seq > base) {
            for p in u.parts.iter().rev() {
                match p {
                    UndoPart::Map { root, before } => {
                        let KeyedRoot::Map(m) = &mut state.roots[*root].1 else {
                            unreachable!("a map part names a map root")
                        };
                        for (k, old) in before.iter().rev() {
                            match old {
                                Some(v) => m.insert(k.clone(), v.clone()),
                                None => m.remove(k),
                            };
                        }
                    }
                    UndoPart::List { root, ops } => {
                        let KeyedRoot::List { live, hidden, .. } = &mut state.roots[*root].1 else {
                            unreachable!("a list part names a list root")
                        };
                        for o in ops.iter().rev() {
                            unapply_list_op(live, hidden, o);
                        }
                    }
                    UndoPart::Root { root, before } => state.roots[*root].1 = before.clone(),
                }
            }
        }
        if checks_on() {
            if let Some(Some(snap)) = st
                .read_bases
                .iter()
                .find(|b| b.version == base)
                .map(|b| b.check.clone())
            {
                assert!(
                    *snap == state,
                    "read base v{base} of {}, rebuilt from the journal, is not the doc it read",
                    st.key.path
                );
            }
        }
        Ok(Some(state))
    }
}

/// A keyed state with no list's places (the live view's form).
fn without_places(mut k: KeyedState) -> KeyedState {
    for (_, r) in &mut k.roots {
        if let KeyedRoot::List { places, .. } = r {
            *places = Vec::new();
        }
    }
    k
}

/// A root with no places (the live view's form).
fn root_without_places(mut r: KeyedRoot) -> KeyedRoot {
    if let KeyedRoot::List { places, .. } = &mut r {
        *places = Vec::new();
    }
    r
}

/// The size of `ops` in `View::bytes`'s count (their keys and values), for the journal's
/// bound.
fn ops_bytes(ops: &[ListApplied]) -> usize {
    ops.iter()
        .map(|o| {
            o.key.as_str().len()
                + o.value_before.as_ref().map_or(0, value_bytes)
                + o.value_after.as_ref().map_or(0, value_bytes)
        })
        .sum()
}

/// Replay a list root's applied `ops` on the view's list (live order, deleted elements), in
/// order, keeping `digest` (the view's content digest) in step: the change in `View::bytes`'s
/// count, or `None` when an op doesn't fit the view, with the list as it was (and `digest`
/// left for the caller to drop). One visit per op; the elements after a position shift, none
/// is read.
fn apply_list(
    live: &mut Vec<(Key, Value)>,
    hidden: &mut BTreeMap<Key, Value>,
    ops: &[ListApplied],
    root: usize,
    digest: &mut u128,
) -> Option<i64> {
    let mut delta = 0;
    for (n, o) in ops.iter().enumerate() {
        entry_visits!(1);
        match apply_list_op(live, hidden, o, root, digest) {
            Some(d) => delta += d,
            None => {
                for done in ops[..n].iter().rev() {
                    unapply_list_op(live, hidden, done);
                }
                return None;
            }
        }
    }
    Some(delta)
}

/// The size of an element in `View::bytes`'s count.
fn elem_bytes(k: &Key, v: &Value) -> i64 {
    (k.as_str().len() + value_bytes(v)) as i64
}

/// A digest term: two SipHash-1-3 hashes with fixed keys (process-stable), as one `u128`.
fn term(x: impl Hash) -> u128 {
    let (mut a, mut b) = (DefaultHasher::new(), DefaultHasher::new());
    (0u8, &x).hash(&mut a);
    (1u8, &x).hash(&mut b);
    (u128::from(a.finish()) << 64) | u128::from(b.finish())
}

/// A live list element's digest term: its root, key and value, and the key before it in the
/// live order (the order is the set of these pairs: keys are unique).
fn list_term(root: usize, key: &Key, value: &Value, prev: Option<&Key>) -> u128 {
    term((1u8, root, key, value, prev))
}

/// A map entry's digest term.
fn map_term(root: usize, key: &str, value: &Value) -> u128 {
    term((0u8, root, key, value))
}

/// A root's part of the content digest: what a pack renders from it (a list's live elements in
/// order, a map's entries, a text); a list's deleted elements render nothing. O(root).
fn root_digest(root: usize, r: &KeyedRoot) -> u128 {
    let mut d = 0u128;
    match r {
        KeyedRoot::Text(t) => d = term((2u8, root, t)),
        KeyedRoot::List { live, .. } => {
            let mut prev: Option<&Key> = None;
            for (k, v) in live {
                d = d.wrapping_add(list_term(root, k, v, prev));
                prev = Some(k);
            }
        }
        KeyedRoot::Map(m) => {
            for (k, v) in m {
                d = d.wrapping_add(map_term(root, k, v));
            }
        }
    }
    d
}

/// A state's content digest (every root's part).
fn state_digest(k: &KeyedState) -> u128 {
    k.roots
        .iter()
        .enumerate()
        .fold(0u128, |d, (i, (_, r))| d.wrapping_add(root_digest(i, r)))
}

/// Remove the live element at `i`, keeping `digest`: its term goes, and the next one's
/// predecessor becomes the previous one.
fn remove_live(
    live: &mut Vec<(Key, Value)>,
    i: usize,
    root: usize,
    digest: &mut u128,
) -> (Key, Value) {
    let prev = i.checked_sub(1).map(|p| live[p].0.clone());
    *digest = digest.wrapping_sub(list_term(root, &live[i].0, &live[i].1, prev.as_ref()));
    if let Some((nk, nv)) = live.get(i + 1) {
        *digest = digest
            .wrapping_sub(list_term(root, nk, nv, Some(&live[i].0)))
            .wrapping_add(list_term(root, nk, nv, prev.as_ref()));
    }
    live.remove(i)
}

/// Insert a live element at `i`, keeping `digest`.
fn insert_live(
    live: &mut Vec<(Key, Value)>,
    i: usize,
    e: (Key, Value),
    root: usize,
    digest: &mut u128,
) {
    let prev = i.checked_sub(1).map(|p| live[p].0.clone());
    if let Some((nk, nv)) = live.get(i) {
        *digest = digest
            .wrapping_sub(list_term(root, nk, nv, prev.as_ref()))
            .wrapping_add(list_term(root, nk, nv, Some(&e.0)));
    }
    *digest = digest.wrapping_add(list_term(root, &e.0, &e.1, prev.as_ref()));
    live.insert(i, e);
}

/// One op of [`apply_list`]: `None` (nothing changed) when it doesn't fit.
fn apply_list_op(
    live: &mut Vec<(Key, Value)>,
    hidden: &mut BTreeMap<Key, Value>,
    o: &ListApplied,
    root: usize,
    digest: &mut u128,
) -> Option<i64> {
    let at = |live: &Vec<(Key, Value)>, i: Option<usize>| {
        i.filter(|&i| live.get(i).is_some_and(|(k, _)| *k == o.key))
    };
    match (o.kind, &o.effect) {
        (_, Effect::None { .. }) => Some(0),
        (ListOpKind::Insert, Effect::Visible) => {
            let i = o.index_after.filter(|&i| i <= live.len())?;
            let v = o.value_after.clone()?;
            let d = elem_bytes(&o.key, &v);
            insert_live(live, i, (o.key.clone(), v), root, digest);
            Some(d)
        }
        (ListOpKind::Move, Effect::Visible) => {
            let b = at(live, o.index_before)?;
            let a = o.index_after.filter(|&a| a < live.len())?;
            let e = remove_live(live, b, root, digest);
            insert_live(live, a, e, root, digest);
            Some(0)
        }
        (ListOpKind::Delete, Effect::Visible) => {
            let b = at(live, o.index_before)?;
            let (k, v) = remove_live(live, b, root, digest);
            hidden.insert(k, v);
            Some(0)
        }
        (ListOpKind::Restore, Effect::Visible) => {
            let a = o.index_after.filter(|&a| a <= live.len())?;
            let v = hidden.remove(&o.key)?;
            insert_live(live, a, (o.key.clone(), v), root, digest);
            Some(0)
        }
        (ListOpKind::Set, Effect::Visible) => {
            let a = at(live, o.index_after)?;
            let v = o.value_after.clone()?;
            let prev = a.checked_sub(1).map(|p| live[p].0.clone());
            *digest = digest
                .wrapping_sub(list_term(root, &o.key, &live[a].1, prev.as_ref()))
                .wrapping_add(list_term(root, &o.key, &v, prev.as_ref()));
            let old = std::mem::replace(&mut live[a].1, v);
            Some(elem_bytes(&o.key, &live[a].1) - elem_bytes(&o.key, &old))
        }
        (ListOpKind::Set, Effect::Hidden) => {
            let v = o.value_after.clone()?;
            let slot = hidden.get_mut(&o.key)?;
            let old = std::mem::replace(slot, v);
            Some(elem_bytes(&o.key, slot) - elem_bytes(&o.key, &old))
        }
        // a deleted element's place moved: its place shows only when it comes back
        (ListOpKind::Move, Effect::Hidden) => Some(0),
        (ListOpKind::Drop, _) => {
            let v = hidden.remove(&o.key)?;
            Some(-elem_bytes(&o.key, &v))
        }
        _ => None,
    }
}

/// The reverse of [`apply_list_op`] for an op it applied.
fn unapply_list_op(
    live: &mut Vec<(Key, Value)>,
    hidden: &mut BTreeMap<Key, Value>,
    o: &ListApplied,
) {
    let (b, a) = (o.index_before, o.index_after);
    match (o.kind, &o.effect) {
        (_, Effect::None { .. }) | (ListOpKind::Move, Effect::Hidden) => {}
        (ListOpKind::Insert, _) => {
            if let Some(a) = a {
                live.remove(a);
            }
        }
        (ListOpKind::Move, _) => {
            if let (Some(b), Some(a)) = (b, a) {
                let e = live.remove(a);
                live.insert(b, e);
            }
        }
        (ListOpKind::Delete, _) => {
            if let (Some(b), Some(v)) = (b, hidden.remove(&o.key)) {
                live.insert(b, (o.key.clone(), v));
            }
        }
        (ListOpKind::Restore, _) => {
            if let Some(a) = a {
                let (k, v) = live.remove(a);
                hidden.insert(k, v);
            }
        }
        (ListOpKind::Set, Effect::Hidden) => {
            if let (Some(slot), Some(v)) = (hidden.get_mut(&o.key), o.value_before.clone()) {
                *slot = v;
            }
        }
        (ListOpKind::Set, _) => {
            if let (Some(a), Some(v)) = (a, o.value_before.clone()) {
                live[a].1 = v;
            }
        }
        (ListOpKind::Drop, _) => {
            if let Some(v) = o.value_before.clone() {
                hidden.insert(o.key.clone(), v);
            }
        }
    }
}

/// A keyed state's elements (live or deleted) and map entries, every root's: the count
/// `Config::keyed_max_entries` limits. O(roots).
pub(super) fn keyed_entries(k: &KeyedState) -> usize {
    k.roots
        .iter()
        .map(|(_, x)| match x {
            KeyedRoot::List { live, hidden, .. } => live.len() + hidden.len(),
            KeyedRoot::Map(m) => m.len(),
            KeyedRoot::Text(_) => 0,
        })
        .sum()
}

impl State {
    /// The version the live view is at, if there is one.
    pub(super) fn live_view_seq(&self) -> Option<Seq> {
        self.live_view.as_ref().map(|v| v.seq)
    }
}
