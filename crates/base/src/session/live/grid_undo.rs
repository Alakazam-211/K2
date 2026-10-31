//! Undo, redo and revert of structured changes (spec v1.10 §6a, C1e). A child of `live`.
//!
//! **One mechanism.** Every stack entry is a change (its log position); undoing it, redoing
//! it and reverting it all **reverse that change's record** (revert record v3: its ops by key,
//! each with its write, values before and after, the live key before it, and the facet's
//! holder before it), "own" being that change's writes:
//! - undo of batch B reverses B's record; the undo's own change U goes on the redo stack;
//! - redo reverses U's record (so it inverts exactly what the undo applied: an `already` or
//!   skipped region was never applied, so it is never redone, S3a(i), S9); its change goes
//!   back on the undo stack;
//! - `revert <change>` reverses that change's record; the revert is the caller's batch (U5).
//!
//! **Per region** (one op, S1), newest op first, judged against the doc and the holder index
//! as they stand before the step (S3a, S3, S4): `already` (the live facet equals what the
//! inverse would write: applies nothing, never a conflict), a conflict (the facet's holder,
//! resolved through `restores_write` (S2), isn't the change's own write; or the element was
//! dropped, S11), or the inverse op, recording the write it puts back (S2). A change that
//! wrote the table's header setting is one region (S7). Several steps (`--steps N`) are
//! planned on a scratch copy of the doc, with the holders the earlier steps would leave.

use crate::table::Table;
use std::collections::{BTreeMap, HashMap};

use super::grid::{back_after, resolved, ElemRestores};
use super::*;
use crate::elem::{
    Anchor, Applied as ElemApplied, Effect, ElemState, Key, KeyedRoot, KeyedState, ListOp, MapOp,
    Value, WriteId,
};
use crate::store::{ElemFacet, ElemOpKind, ElemOpRecord, ElemWrite, StoredEffect};
use crate::table;

/// A step being planned's view: the doc (live, or a scratch copy after earlier steps) and
/// the holders (the store's index, overridden by what earlier planned steps wrote).
struct World<'a> {
    doc: Option<Doc>,
    live: &'a Doc,
    keyed: KeyedState,
    overlay: HashMap<(String, String, ElemFacet), Option<ElemWrite>>,
}

/// What one region of a step comes to.
enum Outcome {
    Apply {
        list: Option<ListOp>,
        map: Option<MapOp>,
        restores: Option<(ElemFacet, (Seq, Option<WriteId>))>,
    },
    Already,
    Conflict {
        why: &'static str,
        by: Option<ElemWrite>,
    },
}

struct RegionPlan {
    root: String,
    key: String,
    outcome: Outcome,
}

/// A planned step: its edits, what they put back, the conflicts it skips or reports.
#[derive(Default)]
struct StepPlan {
    edits: Vec<Edit>,
    restores: ElemRestores,
    conflicts: Vec<(String, String, String, Option<ElemWrite>)>,
    applied: usize,
    /// A revert's rows rewritten whole (S13): the live cells it overrides, for the notes.
    overrides: Vec<table::Conflict>,
}

/// Which of a change's rows a later edit or re-pairing keeps from a merge by cell (spec S13).
#[derive(Default)]
struct Rewritten {
    rows: Vec<Key>,
    /// The change remapped the columns and a cell was written since: all of it.
    remap: bool,
}

/// A seq no change has: the holder an earlier planned step leaves for a write it makes.
const PENDING: Seq = u64::MAX;

impl Session {
    fn holder(
        &self,
        st: &State,
        w: &World<'_>,
        root: &str,
        key: &str,
        f: ElemFacet,
    ) -> Result<Option<ElemWrite>> {
        if let Some(x) = w.overlay.get(&(root.to_owned(), key.to_owned(), f)) {
            return Ok(x.clone());
        }
        Ok(self.store(st)?.elem_write(self.doc_id, root, key, f)?)
    }

    /// The holders of a table row's cells, by key (the index by prefix, then the overlay).
    fn row_cell_holders(
        &self,
        st: &State,
        w: &World<'_>,
        row: &str,
    ) -> Result<Vec<(String, ElemWrite)>> {
        let prefix = format!("{row}{}", table::CELL_SEP);
        let mut out: HashMap<String, ElemWrite> = self
            .store(st)?
            .elem_writes_prefixed(self.doc_id, table::CELLS, &prefix)?
            .into_iter()
            .filter(|(_, f, _)| *f == ElemFacet::Value)
            .map(|(k, _, x)| (k, x))
            .collect();
        for ((r, k, f), x) in &w.overlay {
            if r == table::CELLS && *f == ElemFacet::Value && k.starts_with(&prefix) {
                match x {
                    Some(x) => {
                        out.insert(k.clone(), x.clone());
                    }
                    None => {
                        out.remove(k);
                    }
                }
            }
        }
        let mut v: Vec<(String, ElemWrite)> = out.into_iter().collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(v)
    }

    /// One region (op `o` of change `own` on `root`): `already`, a conflict, or its inverse,
    /// judged on `cur` (the doc as the step's earlier regions leave it) and the holders.
    #[allow(clippy::too_many_arguments)]
    fn plan_region(
        &self,
        st: &State,
        w: &World<'_>,
        cur: &KeyedState,
        own: Seq,
        root: &str,
        o: &ElemOpRecord,
        is_table: bool,
    ) -> Result<Outcome> {
        let key = o.key.as_str();
        // own: the holder, resolved, is the change's write for the facet, resolved (S2, S3)
        let mine = o.restores.map_or(own, |r| r.0);
        let not_own = |x: &Option<ElemWrite>| x.as_ref().map_or(true, |x| resolved(x).0 != mine);
        let conflict = |why, by| Outcome::Conflict { why, by };
        let put_back = || o.prev.as_ref().map(|p| (o.kind.facet(), resolved(p)));
        let list_key = || Key::new(key).ok();
        // a purge redacted this op's values (C1f): a value can't be put back once erased,
        // and the record no longer says what it was (U19: never resurrect, never guess)
        if o.redacted
            && matches!(
                o.kind,
                ElemOpKind::Set | ElemOpKind::MapSet | ElemOpKind::MapDelete
            )
        {
            return Ok(conflict("erased", None));
        }
        // a cell (or its quoting) whose row or column the doc has dropped: nothing can show or
        // reach it, so nothing is written under that key (S11); `--partial` skips it
        if is_table && (root == table::CELLS || root == table::QUOTING) {
            if let Some((r, c)) = table::parse_cell_key(key) {
                if elem_state(cur, table::ROWS, &r) == ElemState::Unknown
                    || elem_state(cur, table::COLS, &c) == ElemState::Unknown
                {
                    return Ok(conflict("dropped", None));
                }
            }
        }
        Ok(match o.kind {
            ElemOpKind::Drop => conflict("dropped", None),
            ElemOpKind::MapSet | ElemOpKind::MapDelete => {
                let live = map_value(cur, root, key);
                if live == o.value_before {
                    Outcome::Already
                } else {
                    let h = self.holder(st, w, root, key, ElemFacet::Value)?;
                    if not_own(&h) {
                        conflict("written since", h)
                    } else {
                        Outcome::Apply {
                            list: None,
                            map: Some(match &o.value_before {
                                Some(v) => MapOp::Set {
                                    key: key.to_owned(),
                                    value: v.clone(),
                                },
                                None => MapOp::Delete {
                                    key: key.to_owned(),
                                },
                            }),
                            restores: o.value_before.as_ref().and(put_back()),
                        }
                    }
                }
            }
            kind => {
                let Some(k) = list_key() else {
                    return Ok(conflict("dropped", None));
                };
                let state = elem_state(cur, root, &k);
                if state == ElemState::Unknown {
                    return Ok(conflict("dropped", None));
                }
                match kind {
                    ElemOpKind::Insert => {
                        if state == ElemState::Deleted {
                            return Ok(Outcome::Already);
                        }
                        for f in [ElemFacet::Position, ElemFacet::Value, ElemFacet::Existence] {
                            let h = self.holder(st, w, root, key, f)?;
                            // a value never written since the insert has no holder: fine
                            if h.is_some() && not_own(&h) {
                                let why = if f == ElemFacet::Existence {
                                    "restored since"
                                } else {
                                    "written since"
                                };
                                return Ok(conflict(why, h));
                            }
                        }
                        if is_table && root == table::ROWS {
                            for (_, h) in self.row_cell_holders(st, w, key)? {
                                // a cell held by a later write that isn't own
                                if h.seq > own && resolved(&h).0 != own && h.seq != own {
                                    return Ok(conflict("cell written since", Some(h)));
                                }
                            }
                        }
                        Outcome::Apply {
                            list: Some(ListOp::Delete { key: k }),
                            map: None,
                            restores: put_back(),
                        }
                    }
                    ElemOpKind::Delete => {
                        if matches!(state, ElemState::Live(_)) {
                            Outcome::Already
                        } else {
                            Outcome::Apply {
                                list: Some(ListOp::Restore { key: k }),
                                map: None,
                                restores: put_back(),
                            }
                        }
                    }
                    ElemOpKind::Restore => {
                        if state == ElemState::Deleted {
                            return Ok(Outcome::Already);
                        }
                        let h = self.holder(st, w, root, key, ElemFacet::Existence)?;
                        if not_own(&h) {
                            conflict("existence written since", h)
                        } else {
                            Outcome::Apply {
                                list: Some(ListOp::Delete { key: k }),
                                map: None,
                                restores: put_back(),
                            }
                        }
                    }
                    ElemOpKind::Move => {
                        if state == ElemState::Deleted {
                            let h = self.holder(st, w, root, key, ElemFacet::Existence)?;
                            return Ok(conflict("deleted since", h));
                        }
                        let to = back_after(cur, root, &o.left, &k);
                        if sits_after(cur, root, &k, &to) {
                            return Ok(Outcome::Already);
                        }
                        let h = self.holder(st, w, root, key, ElemFacet::Position)?;
                        if not_own(&h) {
                            conflict("moved since", h)
                        } else {
                            Outcome::Apply {
                                list: Some(ListOp::Move { key: k, to }),
                                map: None,
                                restores: put_back(),
                            }
                        }
                    }
                    ElemOpKind::Set => {
                        let live = elem_value(cur, root, &k).unwrap_or_default();
                        let before = o.value_before.clone().unwrap_or_default();
                        if live == before {
                            return Ok(Outcome::Already);
                        }
                        let h = self.holder(st, w, root, key, ElemFacet::Value)?;
                        if not_own(&h) {
                            conflict("written since", h)
                        } else {
                            Outcome::Apply {
                                list: Some(ListOp::Set {
                                    key: k,
                                    value: before,
                                }),
                                map: None,
                                restores: put_back(),
                            }
                        }
                    }
                    ElemOpKind::MapSet | ElemOpKind::MapDelete | ElemOpKind::Drop => {
                        unreachable!("handled above")
                    }
                }
            }
        })
    }

    /// Plan one step: change `own`'s record reversed on `w` (newest op first). A change that
    /// wrote the table's header setting is one region (S7): any conflict in it holds back
    /// all of it. With `partial`, the clean regions apply; otherwise the caller applies
    /// nothing when any region conflicts.
    fn plan_step(
        &self,
        st: &State,
        w: &World<'_>,
        own: Seq,
        rec: &RevertRecord,
        is_table: bool,
        kind: ChangeKind,
    ) -> Result<StepPlan> {
        // S13: a row a text re-paired is a guess about identity: with later edits on it, no merge
        // by cell. An undo or redo conflicts; a revert is a whole rewrite (below)
        let rewritten = if is_table {
            self.rows_rewritten_since(st, own, rec)?
        } else {
            Rewritten::default()
        };
        let row_of = |root: &str, key: &str| -> Option<Key> {
            match root {
                table::ROWS => Key::new(key).ok(),
                table::CELLS => table::parse_cell_key(key).map(|(r, _)| r),
                table::QUOTING => table::parse_cell_key(key).map(|(r, _)| r),
                _ => None,
            }
        };
        let revert_whole = kind == ChangeKind::Revert && !rewritten.remap;
        let held_row =
            |root: &str, key: &str| row_of(root, key).is_some_and(|r| rewritten.rows.contains(&r));
        // newest op first, each judged on what the earlier ones leave (a sort's moves
        // depend on each other)
        let mut cur = w.keyed.clone();
        let mut regions = Vec::new();
        for root in rec.elems.iter().rev() {
            for o in root.ops.iter().rev() {
                if o.effect == StoredEffect::None {
                    continue;
                }
                if rewritten.remap || held_row(&root.root, &o.key) {
                    if revert_whole {
                        continue;
                    }
                    // S11 before S13: an op on a row (or its cells) the doc has dropped is
                    // `dropped`, whatever re-paired it
                    let dropped = !planted_rewritten_row_before_dropped()
                        && row_of(&root.root, &o.key).is_some_and(|r| !row_known(w, &r));
                    regions.push(RegionPlan {
                        root: root.root.clone(),
                        key: o.key.clone(),
                        outcome: Outcome::Conflict {
                            why: if dropped { "dropped" } else { "rewritten_row" },
                            by: None,
                        },
                    });
                    continue;
                }
                let outcome = self.plan_region(st, w, &cur, own, &root.root, o, is_table)?;
                if let Outcome::Apply { list, map, .. } = &outcome {
                    apply_keyed(&mut cur, &root.root, list.as_ref(), map.as_ref());
                }
                regions.push(RegionPlan {
                    root: root.root.clone(),
                    key: o.key.clone(),
                    outcome,
                });
            }
        }
        let header = is_table
            && rec.elems.iter().any(|r| {
                r.root == table::META
                    && r.ops
                        .iter()
                        .any(|o| o.key == "header" && o.effect != StoredEffect::None)
            });
        let held = header
            && regions
                .iter()
                .any(|r| matches!(r.outcome, Outcome::Conflict { .. }));
        let mut out = StepPlan::default();
        let mut roots: Vec<(String, Vec<ListOp>, Vec<MapOp>)> = Vec::new();
        for r in regions {
            match r.outcome {
                Outcome::Already => {}
                Outcome::Conflict { why, by } => {
                    out.conflicts.push((r.root, r.key, why.to_owned(), by));
                }
                Outcome::Apply { .. } if held => {}
                Outcome::Apply {
                    list,
                    map,
                    restores,
                } => {
                    out.applied += 1;
                    if let Some((f, x)) = restores {
                        out.restores.insert((r.root.clone(), r.key.clone(), f), x);
                    }
                    let at = match roots.iter().position(|(x, ..)| *x == r.root) {
                        Some(at) => at,
                        None => {
                            roots.push((r.root.clone(), Vec::new(), Vec::new()));
                            roots.len() - 1
                        }
                    };
                    roots[at].1.extend(list);
                    roots[at].2.extend(map);
                }
            }
        }
        if revert_whole {
            for k in &rewritten.rows {
                // S11 before S13: a row the doc has dropped can't come back (a dropped key is
                // never reused): a `dropped` conflict, skipped under `--partial`; never a restore
                // of an unknown key
                if !planted_whole_row_restores_dropped() && !row_known(w, k) {
                    out.conflicts.push((
                        table::ROWS.to_owned(),
                        k.to_string(),
                        "dropped".to_owned(),
                        None,
                    ));
                    continue;
                }
                match self.row_before_change(st, w, own, k)? {
                    Some(before) => {
                        let (list, map, overrides) = self.whole_row_back(st, w, k, &before)?;
                        out.applied += usize::from(!list.is_empty() || !map.is_empty());
                        out.overrides.extend(overrides);
                        let at = match roots.iter().position(|(x, ..)| x == table::ROWS) {
                            Some(at) => at,
                            None => {
                                roots.push((table::ROWS.to_owned(), Vec::new(), Vec::new()));
                                roots.len() - 1
                            }
                        };
                        roots[at].1.extend(list);
                        let at = match roots.iter().position(|(x, ..)| x == table::CELLS) {
                            Some(at) => at,
                            None => {
                                roots.push((table::CELLS.to_owned(), Vec::new(), Vec::new()));
                                roots.len() - 1
                            }
                        };
                        roots[at].2.extend(map);
                    }
                    // its record left the window, or a purge took a value: it can't be known
                    None => out.conflicts.push((
                        table::ROWS.to_owned(),
                        k.to_string(),
                        "rewritten_row".to_owned(),
                        None,
                    )),
                }
            }
        }
        for (root, list, map) in roots {
            if !list.is_empty() {
                out.edits.push(Edit::List {
                    root: root.clone(),
                    ops: list,
                });
            }
            if !map.is_empty() {
                out.edits.push(Edit::Map { root, ops: map });
            }
        }
        Ok(out)
    }

    /// Reverse `changes` (each `(own, its record, the stack entry's on-behalf-of)`), newest
    /// first, as one request (U13, U14): all or nothing, or with `partial` every clean region
    /// of each step until one whose every region conflicts (which stays, `blocked`). Each
    /// step is its own change of kind `kind`. Returns the outcome and, per step taken, the
    /// change it made (`None`: every region was `already`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reverse_records(
        &self,
        st: &mut State,
        i: usize,
        changes: Vec<(Seq, RevertRecord, Option<String>)>,
        partial: bool,
        kind: ChangeKind,
        of: Option<u64>,
        taken: Option<&mut Vec<Option<Seq>>>,
        now: u64,
    ) -> Result<UndoOutcome> {
        let is_table = table::is_table(&self.schema);
        let live_keyed = st.doc.keyed_state()?;
        let mut world = World {
            doc: None,
            live: &st.doc,
            keyed: live_keyed.clone(),
            overlay: HashMap::new(),
        };
        let n = changes.len();
        let mut plans: Vec<StepPlan> = Vec::with_capacity(n);
        let mut blocked = None;
        for (k, (own, rec, _)) in changes.iter().enumerate() {
            let p = self.plan_step(st, &world, *own, rec, is_table, kind)?;
            if !p.conflicts.is_empty() && !partial {
                let regions = self.elem_conflict_regions(st, &live_keyed, p.conflicts)?;
                return Ok(UndoOutcome::Conflicts {
                    step: k + 1,
                    conflicts: regions,
                });
            }
            if partial && p.applied == 0 && !p.conflicts.is_empty() {
                // every region conflicts: this step stays, and planning stops (U14)
                blocked = Some(self.elem_conflict_regions(st, &live_keyed, p.conflicts)?);
                break;
            }
            if k + 1 < n && !p.edits.is_empty() {
                // the next step sees what this one leaves
                let mut d = match world.doc.take() {
                    Some(d) => d,
                    None => Doc::load(self.schema, &world.live.encode_state()?)?,
                };
                let made = d.edit_applied(st.members[i].client, &p.edits)?;
                for (key, x) in holder_updates(&made.applied, &p.restores) {
                    world.overlay.insert(key, x);
                }
                world.keyed = d.keyed_state()?;
                world.doc = Some(d);
            }
            plans.push(p);
        }
        drop(world);
        if plans.is_empty() {
            return Ok(UndoOutcome::Blocked {
                conflicts: blocked.unwrap_or_default(),
            });
        }
        let mut skipped = Vec::new();
        let mut steps = Vec::with_capacity(plans.len());
        let mut overrides = Vec::new();
        for p in plans {
            skipped.extend(p.conflicts);
            overrides.extend(p.overrides);
            steps.push(Step {
                edits: p.edits,
                kind,
                of,
                restores: None,
                elem_restores: p.restores,
            });
        }
        let skipped = self.elem_conflict_regions(st, &live_keyed, skipped)?;
        // a revert's whole rows that override live cells: the notes, in the change's transaction,
        // and a version that holds what they override
        let before = st.version;
        let mut versions = Vec::new();
        let mut finish = None;
        if !overrides.is_empty() {
            let t =
                Table::from_keyed(&live_keyed).map_err(|e| SessionError::BadEdit(e.to_string()))?;
            let pid = st.members[i].p.pid.clone();
            let mut overridden = self.overridden(st, &t, &overrides)?;
            grid::drop_own(&mut overridden, &pid);
            let plan = self.plan_overrides(st, &overridden, "a revert", before, before + 1, now)?;
            st.next_note_rows = plan.note_rows();
            if st.last_version_seq != st.version {
                let snap = self.snapshot(st)?;
                versions.push(Self::version_of(
                    st,
                    st.version,
                    VersionReason::BeforeRestore,
                    None,
                    snap,
                    now,
                ));
            }
            finish = Some((overridden, plan));
        }
        let (result, done) = self.commit(st, i, steps, now, versions)?;
        if let Some((overridden, plan)) = finish {
            self.finish_overrides(st, overridden, plan, before, before + 1)?;
        }
        let steps = done.len();
        if let Some(t) = taken {
            *t = done.into_iter().map(|d| d.map(|x| x.0)).collect();
        }
        Ok(UndoOutcome::Applied {
            steps,
            result,
            skipped,
            blocked,
        })
    }

    /// Spec S13: the rows change `own` touched that a merge by cell can't undo. A row whose mark
    /// is `own` or later, with a later edit on it (a cell's value, or its existence, held by a
    /// change after `own`), or a later text's mark on it. A mark an undo, redo or revert set
    /// isn't a later text: its writes are judged themselves.
    fn rows_rewritten_since(&self, st: &State, own: Seq, rec: &RevertRecord) -> Result<Rewritten> {
        let store = self.store(st)?;
        let mut out = Rewritten::default();
        if store.col_remap(self.doc_id)? == Some(own) {
            out.remap = store
                .elem_writes(self.doc_id)?
                .iter()
                .any(|((root, _, f), w)| {
                    root == table::CELLS && *f == ElemFacet::Value && resolved(w).0 > own
                });
            if out.remap {
                return Ok(out);
            }
        }
        for k in grid::rows_of_elems(&rec.elems) {
            let Some(mark) = store.row_mark(self.doc_id, k.as_str())? else {
                continue;
            };
            if mark < own {
                continue;
            }
            let later_text = mark > own
                && store
                    .change(self.doc_id, mark)?
                    .is_some_and(|c| matches!(c.kind, ChangeKind::Edit | ChangeKind::External));
            let prefix = format!("{k}{}", table::CELL_SEP);
            let later_cell = store
                .elem_writes_prefixed(self.doc_id, table::CELLS, &prefix)?
                .iter()
                .any(|(_, f, w)| *f == ElemFacet::Value && resolved(w).0 > own);
            let later_existence = store
                .elem_write(self.doc_id, table::ROWS, k.as_str(), ElemFacet::Existence)?
                .is_some_and(|w| resolved(&w).0 > own);
            if later_text || later_cell || later_existence {
                out.rows.push(k);
            }
        }
        Ok(out)
    }

    /// Row `k`'s cells just before change `own`: the live row with every later record's ops on it
    /// undone, newest first. `None`: a record left the window or a purge took a value.
    fn row_before_change(
        &self,
        st: &State,
        w: &World<'_>,
        own: Seq,
        k: &Key,
    ) -> Result<Option<HashMap<Key, Value>>> {
        let store = self.store(st)?;
        let t = Table::from_keyed(&w.keyed).map_err(|e| SessionError::BadEdit(e.to_string()))?;
        let mut row: HashMap<Key, Value> = HashMap::new();
        for ((r, c), v) in &t.cells {
            if r == k {
                row.insert(c.clone(), Value::str(v));
            }
        }
        for seq in (own..=st.version).rev() {
            let Some(rec) = store.revert_record(self.doc_id, seq)? else {
                return Ok(None);
            };
            for root in rec.elems.iter().rev().filter(|r| r.root == table::CELLS) {
                for o in root.ops.iter().rev() {
                    let Some((r, c)) = table::parse_cell_key(&o.key) else {
                        continue;
                    };
                    if r != *k || o.effect == StoredEffect::None {
                        continue;
                    }
                    if o.redacted {
                        return Ok(None);
                    }
                    match &o.value_before {
                        Some(v) if *v != Value::Null => {
                            row.insert(c, v.clone());
                        }
                        _ => {
                            row.remove(&c);
                        }
                    }
                }
            }
        }
        Ok(Some(row))
    }

    /// The ops that put row `k` back to `before` whole: a deleted row restored, every cell that
    /// differs set or deleted, each a live cell someone else wrote reported.
    #[allow(clippy::type_complexity)]
    fn whole_row_back(
        &self,
        st: &State,
        w: &World<'_>,
        k: &Key,
        before: &HashMap<Key, Value>,
    ) -> Result<(Vec<ListOp>, Vec<MapOp>, Vec<table::Conflict>)> {
        let t = Table::from_keyed(&w.keyed).map_err(|e| SessionError::BadEdit(e.to_string()))?;
        let mut list = Vec::new();
        if !t.rows.is_live(k) {
            list.push(ListOp::Restore { key: k.clone() });
        }
        let text = |v: Option<&Value>| match v {
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        };
        let mut cols: Vec<Key> = t
            .cells
            .keys()
            .filter(|(r, _)| r == k)
            .map(|(_, c)| c.clone())
            .collect();
        cols.extend(before.keys().cloned());
        cols.sort();
        cols.dedup();
        let mut map = Vec::new();
        let mut overrides = Vec::new();
        for c in cols {
            let (now, was) = (t.cell(k, &c).map(str::to_owned), text(before.get(&c)));
            if now == was {
                continue;
            }
            let key = table::cell_key(k, &c);
            map.push(match &was {
                Some(v) => MapOp::Set {
                    key: key.clone(),
                    value: Value::str(v),
                },
                None => MapOp::Delete { key: key.clone() },
            });
            overrides.push(table::Conflict {
                rule: table::ConflictRule::Cell,
                at: table::Region::Cell {
                    row: k.clone(),
                    col: c.clone(),
                },
                base: table::Held::Value(was.clone()),
                ours: table::Held::Value(was),
                theirs: table::Held::Value(now),
                theirs_writes: Vec::new(),
            });
        }
        let _ = st;
        Ok((list, map, overrides))
    }

    /// Undo or redo on a structured doc (spec v1.10): the stack's entries are changes, each
    /// reversed by its record ([`Session::reverse_records`]).
    pub(super) fn undo_redo_structured(
        &self,
        st: &mut State,
        i: usize,
        req: &UndoRequest,
        origin: Origin,
        now: u64,
    ) -> Result<UndoOutcome> {
        let pid = st.members[i].p.pid.clone();
        let h = st.hist.entry(pid.clone()).or_default();
        let stack = if origin == Origin::Undo {
            &h.undo
        } else {
            &h.redo
        };
        let take = if req.mode == UndoMode::Drop {
            1
        } else {
            req.steps.max(1)
        };
        let picked: Vec<usize> = (0..stack.len())
            .rev()
            .filter(|&k| {
                req.for_person
                    .as_ref()
                    .map_or(true, |p| stack[k].on_behalf_of.as_ref() == Some(p))
            })
            .take(take)
            .collect();
        if picked.is_empty() {
            if h.cleared {
                h.cleared = false;
                return Ok(UndoOutcome::ClearedByPurge);
            }
            if origin == Origin::Undo && stack.is_empty() && self.history_lost(st, &pid)? {
                return Ok(UndoOutcome::HistoryLost);
            }
            return Ok(UndoOutcome::Nothing);
        }
        if req.mode == UndoMode::Drop {
            let b = Self::take_batches(st, &pid, origin, &picked).remove(0);
            return Ok(UndoOutcome::Dropped { change: b.seq });
        }
        let entries: Vec<(Seq, Option<String>)> = picked
            .iter()
            .map(|&k| (stack[k].seq, stack[k].on_behalf_of.clone()))
            .collect();
        let store = self.store(st)?.clone();
        let mut changes = Vec::with_capacity(entries.len());
        for (seq, obo) in entries {
            // the record is inside the revert window (stacks live for a session; the
            // window is 30 days): a missing one is history lost
            let Some(rec) = store.revert_record(self.doc_id, seq)? else {
                return Ok(UndoOutcome::HistoryLost);
            };
            changes.push((seq, rec, obo));
        }
        let kind = if origin == Origin::Undo {
            ChangeKind::Undo
        } else {
            ChangeKind::Redo
        };
        let partial = req.mode == UndoMode::Partial;
        let mut taken = Vec::new();
        let out =
            self.reverse_records(st, i, changes, partial, kind, None, Some(&mut taken), now)?;
        if let UndoOutcome::Applied { .. } = &out {
            let consumed = Self::take_batches(st, &pid, origin, &picked[..taken.len()]);
            for (made, b) in taken.into_iter().zip(consumed) {
                // a step whose every region was `already` made no change: nothing to redo
                if let Some(seq) = made {
                    let batch = Batch {
                        seq,
                        on_behalf_of: b.on_behalf_of,
                        regions: Vec::new(),
                        touched: false,
                    };
                    self.push_batch(st, &pid, batch, origin);
                }
            }
        }
        Ok(out)
    }
}

/// A list root's live elements, deleted ones and places.
type ListParts<'k> = (
    &'k Vec<(Key, Value)>,
    &'k BTreeMap<Key, Value>,
    &'k Vec<Key>,
);

fn list_of<'k>(cur: &'k KeyedState, root: &str) -> Option<ListParts<'k>> {
    match cur.roots.iter().find(|(r, _)| r == root) {
        Some((
            _,
            KeyedRoot::List {
                live,
                hidden,
                places,
            },
        )) => Some((live, hidden, places)),
        _ => None,
    }
}

fn elem_state(cur: &KeyedState, root: &str, k: &Key) -> ElemState {
    let Some((live, hidden, _)) = list_of(cur, root) else {
        return ElemState::Unknown;
    };
    if let Some(i) = live.iter().position(|(x, _)| x == k) {
        ElemState::Live(i)
    } else if hidden.contains_key(k) {
        ElemState::Deleted
    } else {
        ElemState::Unknown
    }
}

fn elem_value(cur: &KeyedState, root: &str, k: &Key) -> Option<Value> {
    let (live, hidden, _) = list_of(cur, root)?;
    live.iter()
        .find(|(x, _)| x == k)
        .map(|(_, v)| v.clone())
        .or_else(|| hidden.get(k).cloned())
}

fn map_value(cur: &KeyedState, root: &str, key: &str) -> Option<Value> {
    match cur.roots.iter().find(|(r, _)| r == root) {
        Some((_, KeyedRoot::Map(m))) => m.get(key).cloned(),
        _ => None,
    }
}

/// Apply one planned inverse op to the step's working state (the doc's sequential rules).
fn apply_keyed(cur: &mut KeyedState, root: &str, list: Option<&ListOp>, map: Option<&MapOp>) {
    let Some((_, x)) = cur.roots.iter_mut().find(|(r, _)| r == root) else {
        return;
    };
    match (x, list, map) {
        (KeyedRoot::Map(m), _, Some(MapOp::Set { key, value })) => {
            m.insert(key.clone(), value.clone());
        }
        (KeyedRoot::Map(m), _, Some(MapOp::Delete { key })) => {
            m.remove(key);
        }
        (
            KeyedRoot::List {
                live,
                hidden,
                places,
            },
            Some(op),
            _,
        ) => {
            let mut vals: HashMap<Key, Value> = live.drain(..).collect();
            let mut dead: HashMap<Key, Value> = std::mem::take(hidden).into_iter().collect();
            match op {
                ListOp::Move { key, to } => {
                    if let Some(at) = places.iter().position(|x| x == key) {
                        places.remove(at);
                    }
                    let at = match to {
                        Anchor::Start => 0,
                        Anchor::End => places.len(),
                        Anchor::After(a) => places.iter().position(|x| x == a).map_or(0, |i| i + 1),
                        Anchor::Before(a) => places.iter().position(|x| x == a).unwrap_or(0),
                    };
                    places.insert(at, key.clone());
                }
                ListOp::Delete { key } => {
                    if let Some(v) = vals.remove(key) {
                        dead.insert(key.clone(), v);
                    }
                }
                ListOp::Restore { key } => {
                    if let Some(v) = dead.remove(key) {
                        vals.insert(key.clone(), v);
                    }
                }
                ListOp::Set { key, value } => {
                    if let Some(v) = vals.get_mut(key).or_else(|| dead.get_mut(key)) {
                        *v = value.clone();
                    }
                }
                _ => {}
            }
            *live = places
                .iter()
                .filter_map(|k| vals.get(k).map(|v| (k.clone(), v.clone())))
                .collect();
            *hidden = dead.into_iter().collect();
        }
        _ => {}
    }
}

/// Whether `k` already sits where `to` puts it: right after `to`'s key, or first.
fn sits_after(keyed: &KeyedState, root: &str, k: &Key, to: &Anchor) -> bool {
    // S3a: right after S4's anchor in place order (the anchor may be deleted)
    let Some((_, KeyedRoot::List { places, .. })) = keyed.roots.iter().find(|(r, _)| r == root)
    else {
        return false;
    };
    let Some(at) = places.iter().position(|x| x == k) else {
        return false;
    };
    match to {
        Anchor::Start => at == 0,
        Anchor::After(a) => at > 0 && places[at - 1] == *a,
        _ => false,
    }
}

/// The holders a step leaves (for the next planned step): each written facet held by a
/// pending write, counting as what it restores.
fn holder_updates(
    stream: &[ElemApplied],
    restores: &ElemRestores,
) -> Vec<((String, String, ElemFacet), Option<ElemWrite>)> {
    let mut out = Vec::new();
    let row = |kind, f: ElemFacet, root: &str, key: &str| {
        let restores = restores.get(&(root.to_owned(), key.to_owned(), f)).copied();
        Some(ElemWrite {
            seq: PENDING,
            write: None,
            kind,
            restores,
        })
    };
    for a in stream {
        match a {
            ElemApplied::List { root, ops } => {
                for o in ops
                    .iter()
                    .filter(|o| !matches!(o.effect, Effect::None { .. }))
                {
                    let k = o.key.as_str();
                    let kind = ElemOpKind::of_list(o.kind);
                    let mut put = |f: ElemFacet| {
                        out.push(((root.clone(), k.to_owned(), f), row(kind, f, root, k)))
                    };
                    match kind {
                        ElemOpKind::Insert => {
                            put(ElemFacet::Position);
                            put(ElemFacet::Existence);
                            if o.value_after.as_ref().is_some_and(|v| *v != Value::Null) {
                                put(ElemFacet::Value);
                            }
                        }
                        k2 => put(k2.facet()),
                    }
                }
            }
            ElemApplied::Map { root, ops } => {
                for o in ops
                    .iter()
                    .filter(|o| !matches!(o.effect, Effect::None { .. }))
                {
                    let kind = ElemOpKind::of_map(o.kind);
                    out.push((
                        (root.clone(), o.key.clone(), ElemFacet::Value),
                        row(kind, ElemFacet::Value, root, &o.key),
                    ));
                }
            }
            ElemApplied::Text { .. } => {}
        }
    }
    out
}

/// Whether the doc still knows row `k` (live or deleted, not dropped).
fn row_known(w: &World<'_>, k: &Key) -> bool {
    Table::from_keyed(&w.keyed).is_ok_and(|t| t.rows.knows(k))
}

/// The S13 faults planted for the tests ([`crate::differential::MergeMutant`]): (xxxv) a
/// whole-row revert restores a dropped row; (xxxvi) an undo or redo calls an op on a dropped row
/// `rewritten_row`. Without the `differential` feature there is no hook: both are `false`.
#[cfg(any(test, feature = "differential"))]
fn planted_whole_row_restores_dropped() -> bool {
    crate::differential::merge_mutant()
        == Some(crate::differential::MergeMutant::WholeRowRestoresDropped)
}

#[cfg(any(test, feature = "differential"))]
fn planted_rewritten_row_before_dropped() -> bool {
    crate::differential::merge_mutant()
        == Some(crate::differential::MergeMutant::RewrittenRowBeforeDropped)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_whole_row_restores_dropped() -> bool {
    false
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_rewritten_row_before_dropped() -> bool {
    false
}
