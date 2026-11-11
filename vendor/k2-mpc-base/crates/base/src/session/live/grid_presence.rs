//! Presence on a grid (A10, K-PRES-6, CSV contract §6): carets, views, claims and the last
//! edit **by element identity**. A child of `live`, so it works on the session's own state.
//!
//! - **Positions are keys.** A caret is a cell `(row key, col key)`; a selection, a view, a
//!   claim and a last edit are spans: the rows (or columns) between two keys **in the current
//!   order**, inclusive, whichever comes first ([`ClaimSpan`]). Keys never change, so nothing
//!   is transformed: a span follows moves and sorts, and rows moved between its ends join it.
//! - **Deleted ends keep their place.** A deleted element sits where a restore would show it
//!   (the list's place order), so a span whose end row is deleted still covers the rows
//!   between; when **nothing** it covers is live, a claim ends with [`ClaimEnd::Deleted`] and
//!   a view or last edit is dropped. A caret on a deleted row or column is dropped.
//! - **Anchors expand**, as a text claim's do: a row inserted right after a claim's last live
//!   row, or right before its first, joins it (a hand-off reservation's too). Where two participants' claims meet there, the
//!   inserter's own claim takes it, else the one taken first ([`ClaimId`] order), so claims of
//!   two participants never come to overlap (as [`awareness::claim_fates`] for text).
//! - **Touching a claim** (contract §6): setting or clearing a cell inside it, renaming one of
//!   its columns, inserting, moving, deleting or restoring one of its rows or columns (before
//!   or after the change), or inserting a row between its ends (or at an end it expands to).
//!   Under [`ClaimPolicy::Respect`] an agent's change touching another's claim is refused
//!   (exit 5) before anything applies; the claimant's own change touching it ends it
//!   ([`ClaimEnd::Edited`]) unless it is held.
//!
//! Everything is memory only, as for text (`session::awareness`).

use std::collections::{HashMap, HashSet};

use super::*;
use crate::elem::{
    Anchor, Applied as ElemApplied, Effect, ElemState, Key, ListOp, ListOpKind, MapOp,
};
use crate::session::awareness::{
    Claim, ClaimEnd, ClaimId, ClaimInfo, ClaimSpan, GridSpan, Note, NoteKind, Reservation,
    ReservationEnd, ReservationInfo,
};
use crate::session::participant::CellCursor;
use crate::table::{self, LineMap};

/// A connection's presence on a grid (memory only).
#[derive(Debug, Clone, Default)]
pub(super) struct GridPresence {
    /// The cell caret or selection: anchor and head `(row, col)`.
    pub cell: Option<((Key, Key), (Key, Key))>,
    /// The rows a read showed (first, last), by key.
    pub view: Option<(Key, Key)>,
    /// The rows its latest change touched (first, last in the order then), by key.
    pub edited: Option<(Key, Key)>,
}

/// One axis of a table as it is (or would be): every element in place order, live and
/// deleted, and the live ones.
#[derive(Debug, Clone)]
pub(super) struct AxisNow {
    places: Vec<Key>,
    live: Vec<Key>,
    place_ix: HashMap<Key, usize>,
    live_ix: HashMap<Key, usize>,
}

impl AxisNow {
    fn new(places: Vec<Key>, live: &HashSet<Key>) -> Self {
        let live: Vec<Key> = places
            .iter()
            .filter(|k| live.contains(*k))
            .cloned()
            .collect();
        AxisNow {
            place_ix: places
                .iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i))
                .collect(),
            live_ix: live
                .iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i))
                .collect(),
            places,
            live,
        }
    }

    pub(super) fn of(doc: &Doc, root: &str) -> Result<Self> {
        let live: HashSet<Key> = doc.list(root)?.into_iter().map(|(k, _)| k).collect();
        Ok(Self::new(doc.places(root)?, &live))
    }

    fn is_live(&self, k: &Key) -> bool {
        self.live_ix.contains_key(k)
    }

    /// The live position (0-based).
    fn index(&self, k: &Key) -> Option<usize> {
        self.live_ix.get(k).copied()
    }

    /// The place range of a span (`None`: an end the list no longer knows, dropped).
    fn range(&self, (a, b): &(Key, Key)) -> Option<(usize, usize)> {
        let (x, y) = (*self.place_ix.get(a)?, *self.place_ix.get(b)?);
        Some((x.min(y), x.max(y)))
    }

    /// The live elements a span covers, in order (empty: none live, or an end dropped).
    fn members(&self, span: &(Key, Key)) -> Vec<Key> {
        match self.range(span) {
            Some((lo, hi)) => self.places[lo..=hi]
                .iter()
                .filter(|k| self.is_live(k))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }

    /// The first and last live elements of a span.
    fn live_ends(&self, span: &(Key, Key)) -> Option<(Key, Key)> {
        let (lo, hi) = self.range(span)?;
        let first = self.places[lo..=hi].iter().find(|k| self.is_live(k))?;
        let last = self.places[lo..=hi]
            .iter()
            .rev()
            .find(|k| self.is_live(k))?;
        Some((first.clone(), last.clone()))
    }

    /// Whether `k`'s place is inside the span.
    fn within(&self, span: &(Key, Key), k: &Key) -> bool {
        match (self.range(span), self.place_ix.get(k)) {
            (Some((lo, hi)), Some(p)) => lo <= *p && *p <= hi,
            _ => false,
        }
    }

    /// The axis after `ops` (the plain applier's semantics: what the doc does when one
    /// participant applies them in order). An op it can't place is skipped: the doc refuses
    /// the change anyway.
    fn after(&self, ops: &[&ListOp]) -> Self {
        let mut places = self.places.clone();
        let mut live: HashSet<Key> = self.live.iter().cloned().collect();
        let pos = |places: &Vec<Key>, k: &Key| places.iter().position(|x| x == k);
        let at = |places: &Vec<Key>, a: &Anchor| -> Option<usize> {
            Some(match a {
                Anchor::Start => 0,
                Anchor::End => places.len(),
                Anchor::After(k) => pos(places, k)? + 1,
                Anchor::Before(k) => pos(places, k)?,
            })
        };
        for op in ops {
            match op {
                ListOp::Insert { key, at: a, .. } => {
                    if let Some(p) = at(&places, a) {
                        places.insert(p, key.clone());
                        live.insert(key.clone());
                    }
                }
                ListOp::Move { key, to } => {
                    let (Some(from), Some(mut p)) = (pos(&places, key), at(&places, to)) else {
                        continue;
                    };
                    places.remove(from);
                    if from < p {
                        p -= 1;
                    }
                    places.insert(p, key.clone());
                }
                ListOp::Delete { key } => {
                    live.remove(key);
                }
                ListOp::Restore { key } if pos(&places, key).is_some() => {
                    live.insert(key.clone());
                }
                ListOp::Drop { key } => {
                    if let Some(p) = pos(&places, key) {
                        places.remove(p);
                    }
                    live.remove(key);
                }
                _ => {}
            }
        }
        Self::new(places, &live)
    }
}

/// The grid claims and the grid reservations a change touches, by index.
pub(super) type GridTouched = (Vec<usize>, Vec<usize>);

/// What a change does to a table, by key: the elements its ops name, per root.
#[derive(Debug, Default)]
pub(super) struct GridTouch {
    rows: HashSet<Key>,
    cols: HashSet<Key>,
    /// Cells set or cleared.
    cells: Vec<(Key, Key)>,
    /// Columns renamed (their header text).
    names: HashSet<Key>,
    /// Inserted rows and columns, in op order (they may join a claim at its end).
    ins_rows: Vec<Key>,
    ins_cols: Vec<Key>,
}

impl GridTouch {
    fn of_edits(edits: &[Edit]) -> Self {
        let mut t = GridTouch::default();
        for e in edits {
            match e {
                Edit::List { root, ops } => {
                    for o in ops {
                        let k = o.key().clone();
                        let ins = matches!(o, ListOp::Insert { .. });
                        if root == table::ROWS {
                            if ins {
                                t.ins_rows.push(k.clone());
                            }
                            t.rows.insert(k);
                        } else if root == table::COLS {
                            if ins {
                                t.ins_cols.push(k.clone());
                            }
                            t.cols.insert(k);
                        }
                    }
                }
                Edit::Map { root, ops } => t.map_keys(root, ops.iter().map(MapOp::key)),
                Edit::Text { .. } => {}
            }
        }
        t
    }

    fn of_stream(stream: &[ElemApplied]) -> Self {
        let mut t = GridTouch::default();
        for a in stream {
            match a {
                ElemApplied::List { root, ops } => {
                    for o in ops
                        .iter()
                        .filter(|o| !matches!(o.effect, Effect::None { .. }))
                    {
                        let ins = o.kind == ListOpKind::Insert && o.effect == Effect::Visible;
                        if root == table::ROWS {
                            if ins {
                                t.ins_rows.push(o.key.clone());
                            }
                            t.rows.insert(o.key.clone());
                        } else if root == table::COLS {
                            if ins {
                                t.ins_cols.push(o.key.clone());
                            }
                            t.cols.insert(o.key.clone());
                        }
                    }
                }
                ElemApplied::Map { root, ops } => t.map_keys(
                    root,
                    ops.iter()
                        .filter(|o| !matches!(o.effect, Effect::None { .. }))
                        .map(|o| o.key.as_str()),
                ),
                ElemApplied::Text { .. } => {}
            }
        }
        t
    }

    fn map_keys<'a>(&mut self, root: &str, keys: impl Iterator<Item = &'a str>) {
        for k in keys {
            if root == table::CELLS {
                if let Some(rc) = table::parse_cell_key(k) {
                    self.cells.push(rc);
                }
            } else if root == table::NAMES {
                if let Ok(c) = Key::new(k) {
                    self.names.insert(c);
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
            && self.cols.is_empty()
            && self.cells.is_empty()
            && self.names.is_empty()
    }
}

/// The spans of `claims` after `ins` landed (`rows`, `cols`: the axes after the change):
/// an insert right after a span's last live element, or right before its first, joins it;
/// where claims of two participants meet, `typist`'s own takes it, else the first taken.
fn expanded(
    claims: &[(ClaimId, String, GridSpan)],
    rows: &AxisNow,
    cols: &AxisNow,
    ins_rows: &[Key],
    ins_cols: &[Key],
    typist: Option<&str>,
) -> Vec<GridSpan> {
    let mut spans: Vec<GridSpan> = claims.iter().map(|c| c.2.clone()).collect();
    for (axis, ins, is_rows) in [(rows, ins_rows, true), (cols, ins_cols, false)] {
        for k in ins {
            let Some(ix) = axis.index(k) else { continue };
            let left = ix.checked_sub(1).map(|j| &axis.live[j]);
            let right = axis.live.get(ix + 1);
            // (claim, joins at its end (true) or start (false))
            let mut holders: Vec<(usize, bool)> = Vec::new();
            let mut inside = false;
            for (n, s) in spans.iter().enumerate() {
                let Some(span) = (if is_rows { &s.rows } else { &s.cols }) else {
                    continue;
                };
                if axis.within(span, k) {
                    inside = true;
                    continue;
                }
                let Some((first, last)) = axis.live_ends(span) else {
                    continue;
                };
                if left == Some(&last) {
                    holders.push((n, true));
                } else if right == Some(&first) {
                    holders.push((n, false));
                }
            }
            if inside || holders.is_empty() {
                continue;
            }
            let first_pid = &claims[holders[0].0].1;
            let one = holders.iter().all(|(n, _)| claims[*n].1 == *first_pid);
            let joins: Vec<(usize, bool)> = if one {
                holders
            } else {
                let pick = |own: bool| {
                    holders
                        .iter()
                        .copied()
                        .filter(|(n, _)| !own || Some(claims[*n].1.as_str()) == typist)
                        .min_by_key(|(n, _)| claims[*n].0)
                };
                pick(true).or_else(|| pick(false)).into_iter().collect()
            };
            for (n, at_end) in joins {
                let s = &mut spans[n];
                let span = if is_rows { &mut s.rows } else { &mut s.cols };
                let Some((a, b)) = span.as_mut() else {
                    continue;
                };
                // the end at the higher place moves to `k` (or the lower one, at the start)
                let (pa, pb) = (axis.place_ix.get(a), axis.place_ix.get(b));
                let a_is_high = pa >= pb;
                let target = if at_end == a_is_high { a } else { b };
                *target = k.clone();
            }
        }
    }
    spans
}

/// Whether a change touching `t` touches a claim covering `span` (contract §6), with the
/// axes before and after it and the span after (expanded).
fn touches(
    t: &GridTouch,
    span: &GridSpan,
    after_span: &GridSpan,
    before: (&AxisNow, &AxisNow),
    after: (&AxisNow, &AxisNow),
) -> bool {
    let set =
        |s: &Option<(Key, Key)>, s2: &Option<(Key, Key)>, b: &AxisNow, a: &AxisNow| match (s, s2) {
            (Some(x), Some(y)) => {
                let mut m: HashSet<Key> = b.members(x).into_iter().collect();
                m.extend(a.members(y));
                Some(m)
            }
            _ => None,
        };
    let rows = set(&span.rows, &after_span.rows, before.0, after.0);
    let cols = set(&span.cols, &after_span.cols, before.1, after.1);
    let in_rows = |r: &Key| rows.as_ref().map_or(true, |m| m.contains(r));
    let in_cols = |c: &Key| cols.as_ref().map_or(true, |m| m.contains(c));
    if let Some(m) = &rows {
        if t.rows.iter().any(|k| m.contains(k)) {
            return true;
        }
    }
    if let Some(m) = &cols {
        if t.cols.iter().any(|k| m.contains(k)) {
            return true;
        }
        if t.names.iter().any(|k| m.contains(k)) {
            return true;
        }
    }
    t.cells.iter().any(|(r, c)| in_rows(r) && in_cols(c))
}

/// Every grid span that expands with its anchors (A10): the live grid claims, then the grid
/// reservations (a hand-off holds what the claim held, and grows as it would have), as
/// `(index, is a reservation, (id, holder, span))`. A reservation's id is its ended claim's.
fn grid_spans(st: &State) -> Vec<(usize, bool, (ClaimId, String, GridSpan))> {
    let claims = st
        .aware
        .claims
        .iter()
        .enumerate()
        .filter_map(|(k, c)| Some((k, false, (c.id, c.pid.clone(), c.grid.clone()?))));
    let res = st
        .aware
        .reservations
        .iter()
        .enumerate()
        .filter_map(|(k, r)| Some((k, true, (r.claim, r.pid.clone(), r.grid.clone()?))));
    claims.chain(res).collect()
}

/// Rendered lines of a run of rows, `(first, last)` 1-based, from the line map (or, with
/// none, the rows' live positions plus a header line).
fn row_lines(lines: Option<&LineMap>, rows: &AxisNow, members: &[Key]) -> Option<(usize, usize)> {
    let (first, last) = (members.first()?, members.last()?);
    match lines {
        Some(m) => {
            let a = m.lines_of(first)?;
            let b = m.lines_of(last)?;
            Some((a.start, b.end - 1))
        }
        None => Some((rows.index(first)? + 1, rows.index(last)? + 1)),
    }
}

fn line_range(p: (usize, usize)) -> LineRange {
    LineRange {
        root: table::ROWS.to_owned(),
        first: p.0,
        last: p.1,
    }
}

/// [`Session::read_grid_with`]'s result: the read, or the panic of its `f` (passed on by the
/// caller once the lock is released).
pub(super) type GridRead<R> =
    std::thread::Result<(ReadResult, R, Notes, Option<awareness::ForEdit>)>;

/// A grid as presence reads it: both axes, the line map, the header names.
pub(super) struct GridNow {
    pub rows: AxisNow,
    pub cols: AxisNow,
    pub lines: Option<LineMap>,
    /// The number of rendered lines.
    pub total: usize,
}

impl Session {
    // ----- reading the grid for presence -----

    pub(super) fn is_grid(&self) -> bool {
        table::is_table(&self.schema)
    }

    /// The axes and the line map now (the line map renders the table: O(cells)).
    pub(super) fn grid_now(&self, st: &State) -> Result<GridNow> {
        let rows = AxisNow::of(&st.doc, table::ROWS)?;
        let cols = AxisNow::of(&st.doc, table::COLS)?;
        let view = View::new(st.doc.keyed_state()?);
        let lines = self.pack.line_map(&view, &st.codec);
        let total = lines.as_ref().map_or(rows.live.len(), |m| m.lines.len());
        Ok(GridNow {
            rows,
            cols,
            lines,
            total,
        })
    }

    /// [`Session::grid_now`], kept while the doc's version and codec settings stay as they
    /// were (every change moves the version; a settings-only outside save changes the codec):
    /// the roster's flush reads it for every caret, and a caret move changes neither. O(1)
    /// then; O(doc) once after a change.
    pub(super) fn grid_now_cached(&self, st: &State) -> Result<Arc<GridNow>> {
        if let Some((v, codec, g)) = st.grid_now.borrow().as_ref() {
            if *v == st.version && *codec == st.codec {
                return Ok(g.clone());
            }
        }
        let g = Arc::new(self.grid_now(st)?);
        *st.grid_now.borrow_mut() = Some((st.version, st.codec.clone(), g.clone()));
        Ok(g)
    }

    /// Whether anything on the grid needs presence work after a change.
    fn grid_presence_live(st: &State) -> bool {
        st.aware.claims.iter().any(|c| c.grid.is_some())
            || st.aware.reservations.iter().any(|r| r.grid.is_some())
            || st
                .members
                .iter()
                .any(|m| m.grid.cell.is_some() || m.grid.view.is_some() || m.grid.edited.is_some())
    }

    /// The lines a grid span covers now (a column span: every line).
    fn span_lines_now(g: &GridNow, span: &GridSpan) -> LineRange {
        match &span.rows {
            Some(r) => {
                let m = g.rows.members(r);
                line_range(row_lines(g.lines.as_ref(), &g.rows, &m).unwrap_or((1, 1)))
            }
            None => line_range((1, g.total.max(1))),
        }
    }

    pub(super) fn grid_claim_info(g: &GridNow, c: &Claim, now: u64) -> ClaimInfo {
        let span = c.grid.as_ref().expect("a grid claim");
        let mut info = c.info("", now);
        info.lines = Self::span_lines_now(g, span);
        info.span = span.span();
        info.start = 0;
        info.end = 0;
        info
    }

    /// Claim `k`'s info, a text claim or a grid one.
    pub(super) fn any_claim_info(&self, st: &State, k: usize, now: u64) -> Result<ClaimInfo> {
        let c = &st.aware.claims[k];
        if c.elems.is_some() {
            return Ok(Self::elem_claim_info(st, c, now));
        }
        if c.grid.is_some() {
            let g = self.grid_now(st)?;
            return Ok(Self::grid_claim_info(&g, c, now));
        }
        Ok(c.info(self.text_of(st, &c.root)?.1, now))
    }

    /// The lines a grid claim covers now (its end events).
    pub(super) fn grid_claim_lines(&self, st: &State, c: &Claim) -> LineRange {
        match (self.grid_now(st), &c.grid) {
            (Ok(g), Some(span)) => Self::span_lines_now(&g, span),
            _ => line_range((1, 1)),
        }
    }

    /// A grid reservation's info.
    fn grid_reservation_info(g: &GridNow, r: &Reservation, now: u64) -> ReservationInfo {
        let span = r.grid.as_ref().expect("a grid reservation");
        let mut info = r.info("", now);
        info.lines = Self::span_lines_now(g, span);
        info.span = span.span();
        info.start = 0;
        info.end = 0;
        info
    }

    /// Reservation `k`'s info, a text reservation or a grid one.
    pub(super) fn any_reservation_info(
        &self,
        st: &State,
        k: usize,
        now: u64,
    ) -> Result<ReservationInfo> {
        let r = &st.aware.reservations[k];
        if r.elems.is_some() {
            return Ok(Self::elem_reservation_info(st, r, now));
        }
        if r.grid.is_some() {
            let g = self.grid_now(st)?;
            return Ok(Self::grid_reservation_info(&g, r, now));
        }
        Ok(r.info(self.text_of(st, &r.root)?.1, now))
    }

    /// The lines a grid reservation holds now (its events).
    pub(super) fn grid_reservation_lines(&self, st: &State, r: &Reservation) -> LineRange {
        match (self.grid_now(st), &r.grid) {
            (Ok(g), Some(span)) => Self::span_lines_now(&g, span),
            _ => line_range((1, 1)),
        }
    }

    /// A cell caret or selection as the roster shows it.
    fn cell_cursor(
        &self,
        st: &State,
        g: &GridNow,
        (anchor, head): &((Key, Key), (Key, Key)),
    ) -> CellCursor {
        let at = g
            .rows
            .index(&head.0)
            .zip(g.cols.index(&head.1))
            .map(|(r, c)| (r + 1, c + 1));
        let name = st
            .doc
            .map_get(table::NAMES, head.1.as_str())
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(str::to_owned));
        let span = (anchor.0.clone(), head.0.clone());
        let lines = row_lines(g.lines.as_ref(), &g.rows, &g.rows.members(&span)).map(line_range);
        CellCursor {
            anchor: anchor.clone(),
            head: head.clone(),
            at,
            name,
            lines,
        }
    }

    // ----- the public calls -----

    /// Place `conn`'s caret on a cell, or select the rectangle from `anchor` to `head` (A10,
    /// `cursor --cell R,C [--to R,C]`): by identity, so it follows moves and sorts and is
    /// dropped when its row or column is deleted. Needs edit. Both cells' rows and columns
    /// must be live ([`SessionError::BadPosition`] otherwise). A grid only.
    pub fn set_cell_cursor(
        &self,
        conn: ConnId,
        anchor: (&Key, &Key),
        head: (&Key, &Key),
    ) -> Result<CellCursor> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Cursor, now)?;
        if !self.is_grid() {
            return Err(SessionError::BadPosition(
                "a cell caret is for tables; use a line:col".into(),
            ));
        }
        let g = self.grid_now(st)?;
        for (r, c) in [anchor, head] {
            if !g.rows.is_live(r) || !g.cols.is_live(c) {
                return Err(SessionError::BadPosition(format!(
                    "cell {r}/{c} is not live (unknown or deleted row or column)"
                )));
            }
        }
        let sel = (
            (anchor.0.clone(), anchor.1.clone()),
            (head.0.clone(), head.1.clone()),
        );
        let info = self.cell_cursor(st, &g, &sel);
        let m = &mut st.members[i];
        m.cursor = None;
        m.grid.cell = Some(sel);
        m.last_cursor_ms = Some(now);
        Ok(info)
    }

    /// `set_cursor` with [`At::Elem`] (cells) on a grid.
    pub(super) fn set_cursor_elem(&self, conn: ConnId, anchor: &At, head: &At) -> Result<Cursor> {
        let cell = |a: &At| -> Result<(Key, Key)> {
            match a {
                At::Elem { root, key } if root == table::CELLS => table::parse_cell_key(key)
                    .ok_or_else(|| SessionError::BadPosition(format!("{key:?} is not a cell key"))),
                other => Err(SessionError::BadPosition(format!(
                    "{other:?}: a grid caret is a cell (root {:?})",
                    table::CELLS
                ))),
            }
        };
        let (a, h) = (cell(anchor)?, cell(head)?);
        self.set_cell_cursor(conn, (&a.0, &a.1), (&h.0, &h.1))?;
        Ok(Cursor {
            root: table::CELLS.to_owned(),
            anchor: 0,
            head: 0,
        })
    }

    /// The rendered lines `first..=last` of a grid, as the rows they belong to (the header
    /// line belongs to none): `None` when no row is on them.
    pub(super) fn grid_rows_of_lines(
        &self,
        g: &GridNow,
        first: usize,
        last: usize,
    ) -> Option<(Key, Key)> {
        let rows: Vec<Key> = match &g.lines {
            Some(m) => (first..=last.min(m.lines.len()))
                .filter_map(|n| m.row_at(n).cloned())
                .collect(),
            None => (first..=last)
                .filter_map(|n| g.rows.live.get(n.checked_sub(1)?).cloned())
                .collect(),
        };
        Some((rows.first()?.clone(), rows.last()?.clone()))
    }

    /// Take a grid claim on `span` for member `i` (rights checked, expired claims ended).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_grid_claim(
        &self,
        st: &mut State,
        i: usize,
        span: GridSpan,
        ttl: u64,
        note: Option<String>,
        hold: bool,
        for_edit: bool,
        verb: &str,
        now: u64,
    ) -> Result<ClaimInfo> {
        let ttl = self.claim_ttl(ttl)?;
        let g = self.grid_now(st)?;
        for (axis, ends, what) in [
            (&g.rows, &span.rows, "row"),
            (&g.cols, &span.cols, "column"),
        ] {
            if let Some((a, b)) = ends {
                for k in [a, b] {
                    if !axis.is_live(k) {
                        return Err(SessionError::BadPosition(format!(
                            "{what} {k} is not live (unknown or deleted)"
                        )));
                    }
                }
            }
        }
        let pid = st.members[i].p.pid.clone();
        let overlaps = |other: &GridSpan| -> bool {
            let ax = |a: &Option<(Key, Key)>, b: &Option<(Key, Key)>, axis: &AxisNow| match (a, b) {
                (Some(x), Some(y)) => {
                    let m: HashSet<Key> = axis.members(x).into_iter().collect();
                    axis.members(y).iter().any(|k| m.contains(k))
                }
                _ => true,
            };
            ax(&span.rows, &other.rows, &g.rows) && ax(&span.cols, &other.cols, &g.cols)
        };
        // a reservation handed to someone else binds agents only
        if st.members[i].p.kind == ParticipantKind::Agent {
            if let Some(r) = st
                .aware
                .reservations
                .iter()
                .find(|r| r.pid != pid && r.grid.as_ref().is_some_and(overlaps))
            {
                let l = Self::span_lines_now(&g, r.grid.as_ref().expect("a grid reservation"));
                return Err(SessionError::Claimed {
                    id: r.claim,
                    by: r.pid.clone(),
                    name: r.name.clone(),
                    lines: (l.first, l.last),
                    remaining_ms: r.remaining(now),
                    note: None,
                });
            }
        }
        for c in st.aware.claims.iter() {
            let Some(other) = c.grid.as_ref().filter(|o| overlaps(o)) else {
                continue;
            };
            if c.pid != pid {
                let l = Self::span_lines_now(&g, other);
                return Err(SessionError::Claimed {
                    id: c.id,
                    by: c.pid.clone(),
                    name: c.name.clone(),
                    lines: (l.first, l.last),
                    remaining_ms: c.remaining(now),
                    note: c.note.clone(),
                });
            }
            if c.contended() {
                return Err(SessionError::ClaimContended {
                    id: c.id,
                    waiters: c.waiters.len(),
                    remaining_ms: c.remaining(now),
                });
            }
        }
        let cfg = &self.config;
        if st.aware.claims.len() >= cfg.claims_max {
            return Err(SessionError::BadClaim(format!(
                "this doc has {} live claims (the most)",
                st.aware.claims.len()
            )));
        }
        let mine = st.aware.claims.iter().filter(|c| c.pid == pid).count();
        if mine >= cfg.claims_per_participant {
            return Err(SessionError::BadClaim(format!(
                "you hold {mine} claims here (the most); release one"
            )));
        }
        // the caller's own reservation there is used: the rest of its queue waits for this
        let mut waiters = std::collections::VecDeque::new();
        if let Some(k) = st
            .aware
            .reservations
            .iter()
            .position(|r| r.pid == pid && r.grid.as_ref().is_some_and(overlaps))
        {
            let r = st.aware.reservations.remove(k);
            waiters = r.queue.clone();
            waiters.retain(|(_, p)| *p != pid);
            self.emit(AuditEvent::ReservationEnded {
                key: st.key.clone(),
                claim: r.claim,
                pid: r.pid.clone(),
                how: ReservationEnd::Claimed,
                lines: Self::span_lines_now(&g, r.grid.as_ref().expect("a grid reservation")),
                version: st.version,
                next_waiter: waiters.front().cloned(),
            });
        }
        let m = &st.members[i];
        let claim = Claim {
            id: ClaimId::fresh(),
            pid: pid.clone(),
            name: m.p.name.clone(),
            conn: m.conn,
            root: table::CELLS.to_owned(),
            s: 0,
            e: 0,
            taken_ms: now,
            expires_ms: now.saturating_add(ttl),
            note,
            hold,
            for_edit,
            waiters,
            edited: false,
            grid: Some(span),
            elems: None,
        };
        let info = Self::grid_claim_info(&g, &claim, now);
        st.aware.claims.push(claim);
        // the roster push (K5) tells subscribers, as a line claim's start is told
        st.roster.record(
            RosterEvent::ClaimStarted {
                who: Who::of(&pid),
                claim: info.id,
                lines: info.lines.clone(),
                note: info.note.clone(),
                hold,
                for_edit,
                expires_ms: info.expires_ms,
                version: st.version,
            },
            self.config.roster_queue,
        );
        self.emit(AuditEvent::ClaimTaken {
            key: st.key.clone(),
            claim: info.id,
            pid,
            conn: info.conn,
            lines: info.lines.clone(),
            expires_ms: info.expires_ms,
            hold,
            for_edit,
        });
        self.log_activity(
            st,
            i,
            ActivityKind::Claim,
            verb,
            Some(info.lines.clone()),
            now,
            Some(info.id),
        );
        Ok(info)
    }

    /// Claim `span` (A10, `claim --rows`/`--cols`/`--cells`, or `--lines a,b`) for `ttl_ms`,
    /// as [`Session::claim`] does for lines: advisory, anchored by key (see the module), it
    /// ends on the claimant's own edit touching it (unless `hold`), on release, time or
    /// leave, and, on a grid, with [`ClaimEnd::Deleted`] once nothing it covers is live. On
    /// a grid, [`ClaimSpan::Lines`] is mapped to the rows on those rendered lines. On a text
    /// doc only [`ClaimSpan::Lines`] applies. [`ClaimSpan::Elems`] is
    /// [`Session::claim_elems`]. Needs edit. Errors as [`Session::claim`]; an unknown or deleted
    /// end is [`SessionError::BadPosition`].
    pub fn claim_span(
        &self,
        conn: ConnId,
        span: ClaimSpan,
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        let out = self.claim_span_inner(conn, span, ttl_ms, note, hold);
        // an App viewer (K5) sees the others by handle
        self.for_viewer(conn, out, |l, c| l.claim(c))
    }

    /// [`Session::claim_span`] as the caller's own pids see it ([`Session::claim`] lenses it
    /// once for an App viewer).
    pub(super) fn claim_span_inner(
        &self,
        conn: ConnId,
        span: ClaimSpan,
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        if let ClaimSpan::Elems { root, keys } = &span {
            return self.claim_elems_inner(conn, root, keys, ttl_ms, note, hold);
        }
        if let ClaimSpan::Nodes { .. } = &span {
            return Err(SessionError::BadPosition(format!(
                "{span}: index nodes are a blob doc's; claim lines, rows, columns, cells or elements"
            )));
        }
        if !self.is_grid() {
            return match span {
                ClaimSpan::Lines(l) => self.claim_inner(conn, l, ttl_ms, note, hold),
                other => Err(SessionError::BadPosition(format!(
                    "{other}: rows, columns and cells are a table's; claim lines"
                ))),
            };
        }
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.expire_claims(st, now);
        Self::check_claim_note(note.as_deref())?;
        let gs = match &span {
            ClaimSpan::Lines(l) => {
                let g = self.grid_now(st)?;
                // `claim --lines` names the lines `cat -n` printed; the rows are the map's
                let text = match self.pack.encode(&self.content(st)?, &st.codec) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    // it can't tell which line the caller means: no claim by line
                    Err(_) => {
                        #[cfg(any(test, feature = "differential"))]
                        let goes_on = crate::differential::line_mutant()
                            == Some(crate::differential::LineMutant::ClaimLinesGoOnUnrendered);
                        #[cfg(not(any(test, feature = "differential")))]
                        let goes_on = false;
                        if !goes_on {
                            return Err(SessionError::LinesUnknown {
                                address: format!("lines {}..{}", l.first, l.last),
                            });
                        }
                        String::new()
                    }
                };
                let bridge = table::NlLines::of(&text);
                let (first, last) = nl_to_rendered(&bridge, (l.first.max(1), l.last.max(l.first)));
                let rows = self.grid_rows_of_lines(&g, first, last).ok_or_else(|| {
                    SessionError::BadPosition(format!(
                        "lines {}..{} hold no row of the table",
                        l.first, l.last
                    ))
                })?;
                GridSpan {
                    rows: Some(rows),
                    cols: None,
                }
            }
            s => GridSpan::of(s).expect("a grid span"),
        };
        let ttl = ttl_ms.unwrap_or(self.config.claim_default_ms);
        self.take_grid_claim(st, i, gs, ttl, note, hold, false, "claim", now)
    }

    // ----- changes -----

    /// The grid claims, and the grid reservations, a structured change by member `i`
    /// (`edits`, not applied yet) touches (contract §6), by index. Both expand with their
    /// anchors.
    pub(super) fn grid_touched(&self, st: &State, i: usize, edits: &[Edit]) -> Result<GridTouched> {
        Ok(match self.touch_axes(st)? {
            Some(axes) => self.touched_by(st, i, &GridTouch::of_edits(edits), edits, &axes),
            None => GridTouched::default(),
        })
    }

    /// The rows and columns before a change, for [`Session::grid_landed`]: `None` when no grid
    /// span could be touched.
    pub(super) fn touch_axes(&self, st: &State) -> Result<Option<(AxisNow, AxisNow)>> {
        let any = st.aware.claims.iter().any(|c| c.grid.is_some())
            || st.aware.reservations.iter().any(|r| r.grid.is_some());
        if !self.is_grid() || !any {
            return Ok(None);
        }
        Ok(Some((
            AxisNow::of(&st.doc, table::ROWS)?,
            AxisNow::of(&st.doc, table::COLS)?,
        )))
    }

    /// [`Session::grid_touched`] for a change that has landed in the doc, from what it did
    /// (`stream`): an op with no effect touches nothing. `axes`: as they were before it.
    pub(super) fn grid_landed(
        &self,
        st: &State,
        i: usize,
        stream: &[ElemApplied],
        edits: &[Edit],
        axes: Option<&(AxisNow, AxisNow)>,
    ) -> GridTouched {
        match axes {
            Some(axes) => self.touched_by(st, i, &GridTouch::of_stream(stream), edits, axes),
            None => GridTouched::default(),
        }
    }

    fn touched_by(
        &self,
        st: &State,
        i: usize,
        t: &GridTouch,
        edits: &[Edit],
        (rows, cols): &(AxisNow, AxisNow),
    ) -> GridTouched {
        if t.is_empty() {
            return GridTouched::default();
        }
        let pid = st.members[i].p.pid.clone();
        let ops = |root: &str| -> Vec<&ListOp> {
            edits
                .iter()
                .filter_map(|e| match e {
                    Edit::List { root: r, ops } if r == root => Some(ops.iter()),
                    _ => None,
                })
                .flatten()
                .collect()
        };
        let (rows2, cols2) = (rows.after(&ops(table::ROWS)), cols.after(&ops(table::COLS)));
        let spans = grid_spans(st);
        let plain: Vec<(ClaimId, String, GridSpan)> =
            spans.iter().map(|(_, _, c)| c.clone()).collect();
        let after = expanded(&plain, &rows2, &cols2, &t.ins_rows, &t.ins_cols, Some(&pid));
        let (mut claims, mut reservations) = (Vec::new(), Vec::new());
        for ((k, res, c), a) in spans.iter().zip(&after) {
            if touches(t, &c.2, a, (rows, cols), (&rows2, &cols2)) {
                if *res {
                    reservations.push(*k);
                } else {
                    claims.push(*k);
                }
            }
        }
        (claims, reservations)
    }

    /// The claim policy on a structured change by member `i` (A10): under
    /// [`ClaimPolicy::Respect`], an **agent's** change touching another participant's live
    /// grid claim or element claim, or a reservation of either handed to someone else, is
    /// refused ([`SessionError::Claimed`], [`SessionError::ElemsClaimed`], exit 5) before
    /// anything applies. A person (or any other kind) is never refused.
    pub(super) fn check_grid_claims(
        &self,
        st: &mut State,
        i: usize,
        edits: &[Edit],
        policy: ClaimPolicy,
        now: u64,
    ) -> Result<()> {
        self.expire_claims(st, now);
        if policy == ClaimPolicy::Ignore || st.members[i].p.kind != ParticipantKind::Agent {
            return Ok(());
        }
        self.check_elem_claims(st, i, edits, now)?;
        let pid = st.members[i].p.pid.clone();
        let (touched, reserved) = self.grid_touched(st, i, edits)?;
        // a reservation handed to someone else
        if let Some(&k) = reserved
            .iter()
            .find(|k| st.aware.reservations[**k].pid != pid)
        {
            let g = self.grid_now(st)?;
            let r = &st.aware.reservations[k];
            let l = Self::span_lines_now(&g, r.grid.as_ref().expect("a grid reservation"));
            return Err(SessionError::Claimed {
                id: r.claim,
                by: r.pid.clone(),
                name: r.name.clone(),
                lines: (l.first, l.last),
                remaining_ms: r.remaining(now),
                note: None,
            });
        }
        let Some(&k) = touched.iter().find(|k| st.aware.claims[**k].pid != pid) else {
            return Ok(());
        };
        let g = self.grid_now(st)?;
        let c = &st.aware.claims[k];
        let l = Self::span_lines_now(&g, c.grid.as_ref().expect("a grid claim"));
        Err(SessionError::Claimed {
            id: c.id,
            by: c.pid.clone(),
            name: c.name.clone(),
            lines: (l.first, l.last),
            remaining_ms: c.remaining(now),
            note: c.note.clone(),
        })
    }

    /// A structured change by member `i` has landed in the doc: its own grid claims in `touched`
    /// ([`Session::grid_touched`], read before it applied) end once it is durable
    /// ([`ClaimEnd::Edited`]), unless held, and a grid reservation handed to it there is used
    /// ([`ReservationEnd::Edited`], passed to the next waiter). A commit that fails after this
    /// reloads the doc, which ends every claim as [`ClaimEnd::Reset`].
    pub(super) fn mark_grid_claims(&self, st: &mut State, i: usize, touched: &GridTouched) {
        let pid = st.members[i].p.pid.clone();
        for &k in &touched.0 {
            let c = &mut st.aware.claims[k];
            if c.pid == pid && !c.hold {
                c.edited = true;
            }
        }
        // the reserved waiter's own edit there uses its reservation (it passes on)
        for &k in &touched.1 {
            let r = &mut st.aware.reservations[k];
            if r.pid == pid {
                r.edited = true;
            }
        }
    }

    /// After a structured change (`stream`, applied and logged; `actor`: the member who made
    /// it, `None` for an outside edit): claims expand at their ends and end
    /// [`ClaimEnd::Deleted`] when nothing they cover is live; carets on a deleted row or
    /// column, and views and last edits with nothing live, are dropped; the actor's last edit
    /// is the rows it touched and its caret the last cell it wrote (K-PRES-7).
    pub(super) fn grid_presence_after(
        &self,
        st: &mut State,
        actor: Option<usize>,
        stream: &[ElemApplied],
        now: u64,
    ) {
        self.elem_claims_after(st, stream, now);
        if !self.is_grid() || stream.is_empty() {
            return;
        }
        if actor.is_none() && !Self::grid_presence_live(st) {
            return;
        }
        let touch = GridTouch::of_stream(stream);
        // O(1) per element (the doc caches the live order); the axes (O(rows)) only for
        // claims and reservations, and for a span whose two ends are both gone
        let live_at = |doc: &Doc, root: &str, k: &Key| match doc.elem(root, k) {
            Ok(ElemState::Live(i)) => Some(i),
            _ => None,
        };
        let any_spans = st.aware.claims.iter().any(|c| c.grid.is_some())
            || st.aware.reservations.iter().any(|r| r.grid.is_some());
        if any_spans {
            let (Ok(rows), Ok(cols)) = (
                AxisNow::of(&st.doc, table::ROWS),
                AxisNow::of(&st.doc, table::COLS),
            ) else {
                return;
            };
            let typist = actor.map(|i| st.members[i].p.pid.clone());
            // claims and reservations: expand, then end the ones with nothing live
            let spans = grid_spans(st);
            let plain: Vec<(ClaimId, String, GridSpan)> =
                spans.iter().map(|(_, _, c)| c.clone()).collect();
            let grown = expanded(
                &plain,
                &rows,
                &cols,
                &touch.ins_rows,
                &touch.ins_cols,
                typist.as_deref(),
            );
            for ((k, res, _), g) in spans.iter().zip(grown) {
                if *res {
                    st.aware.reservations[*k].grid = Some(g);
                } else {
                    st.aware.claims[*k].grid = Some(g);
                }
            }
            let gone = |s: &GridSpan| {
                s.rows.as_ref().is_some_and(|r| rows.members(r).is_empty())
                    || s.cols.as_ref().is_some_and(|c| cols.members(c).is_empty())
            };
            while let Some(k) = st
                .aware
                .claims
                .iter()
                .position(|c| c.grid.as_ref().is_some_and(gone))
            {
                self.end_claim(st, k, ClaimEnd::Deleted, now, false);
            }
            while let Some(k) = st
                .aware
                .reservations
                .iter()
                .position(|r| r.grid.as_ref().is_some_and(gone))
            {
                self.end_reservation(st, k, ReservationEnd::Deleted, now);
            }
        }
        // carets on a deleted row or column drop; views and last edits with nothing live
        let mut rows_axis: Option<AxisNow> = None;
        for k in 0..st.members.len() {
            let doc = &st.doc;
            let g = &st.members[k].grid;
            let cell_gone = g.cell.as_ref().is_some_and(|(a, h)| {
                let live = |(r, c): &(Key, Key)| {
                    live_at(doc, table::ROWS, r).is_some() && live_at(doc, table::COLS, c).is_some()
                };
                !live(a) || !live(h)
            });
            let mut empty = |span: &Option<(Key, Key)>| -> bool {
                let Some((a, b)) = span else { return false };
                if live_at(doc, table::ROWS, a).is_some() || live_at(doc, table::ROWS, b).is_some()
                {
                    return false;
                }
                let axis = rows_axis.get_or_insert_with(|| {
                    AxisNow::of(doc, table::ROWS)
                        .unwrap_or_else(|_| AxisNow::new(Vec::new(), &HashSet::new()))
                });
                axis.members(&(a.clone(), b.clone())).is_empty()
            };
            let (view_gone, edited_gone) = (empty(&g.view), empty(&g.edited));
            let g = &mut st.members[k].grid;
            if cell_gone {
                g.cell = None;
            }
            if view_gone {
                g.view = None;
            }
            if edited_gone {
                g.edited = None;
            }
        }
        let Some(i) = actor else { return };
        // the rows it touched, live now, in order: its last edit
        let doc = &st.doc;
        let mut touched: Vec<(usize, Key)> = touch
            .rows
            .iter()
            .chain(touch.cells.iter().map(|(r, _)| r))
            .filter_map(|r| Some((live_at(doc, table::ROWS, r)?, r.clone())))
            .collect();
        touched.sort();
        let edited = match (touched.first(), touched.last()) {
            (Some(a), Some(b)) => Some((a.1.clone(), b.1.clone())),
            _ => None,
        };
        // its caret: the last cell it wrote that is live
        let last_cell = stream
            .iter()
            .filter_map(|a| match a {
                ElemApplied::Map { root, ops } if root == table::CELLS => Some(ops.iter()),
                _ => None,
            })
            .flatten()
            .filter(|o| o.effect == Effect::Visible)
            .filter_map(|o| table::parse_cell_key(&o.key))
            .rfind(|(r, c)| {
                live_at(doc, table::ROWS, r).is_some() && live_at(doc, table::COLS, c).is_some()
            });
        let m = &mut st.members[i];
        if edited.is_some() {
            m.grid.edited = edited;
        }
        if let Some(cell) = last_cell {
            m.cursor = None;
            m.grid.cell = Some((cell.clone(), cell));
        }
    }

    // ----- notes -----

    /// Heads-up notes for member `i` about the elements `t` names (a structured verb's
    /// change, a read's rows): others' claims there, people's carets and selections, views
    /// and recent edits, with their spans by key. Raw (no dedupe, no limit).
    fn collect_grid_notes(
        &self,
        st: &State,
        g: &GridNow,
        reader: &str,
        t: &GridTouch,
        now: u64,
    ) -> Vec<Note> {
        let kind_of = |pid: &str| {
            st.members
                .iter()
                .find(|m| m.p.pid == pid)
                .map_or(ParticipantKind::Agent, |m| m.p.kind)
        };
        let hit = |span: &GridSpan| touches(t, span, span, (&g.rows, &g.cols), (&g.rows, &g.cols));
        let rows_span = |r: &(Key, Key)| GridSpan {
            rows: Some(r.clone()),
            cols: None,
        };
        let note = |pid: &str, name: &str, kind: NoteKind, span: &GridSpan, at: u64| Note {
            pid: pid.to_owned(),
            name: name.to_owned(),
            kind,
            participant_kind: kind_of(pid),
            lines: Self::span_lines_now(g, span),
            age_ms: now.saturating_sub(at),
            claim: None,
            claim_note: None,
            remaining_ms: None,
            span: Some(span.span()),
            overwrite: None,
        };
        let mut out = Vec::new();
        for c in &st.aware.claims {
            let Some(s) = c.grid.as_ref() else { continue };
            if c.pid != reader && hit(s) {
                out.push(Note {
                    claim: Some(c.id),
                    claim_note: c.note.clone(),
                    remaining_ms: Some(c.remaining(now)),
                    ..note(&c.pid, &c.name, NoteKind::Claimed, s, c.taken_ms)
                });
            }
        }
        for m in &st.members {
            if m.p.pid == reader {
                continue;
            }
            if let Some(v) = &m.grid.view {
                let s = rows_span(v);
                if hit(&s) {
                    let at = m.view_ms.unwrap_or(m.last_active_ms);
                    out.push(note(&m.p.pid, &m.p.name, NoteKind::Viewing, &s, at));
                }
            }
            if let (Some(e), Some(at)) = (&m.grid.edited, m.last_edit_ms) {
                let s = rows_span(e);
                if now.saturating_sub(at) < self.config.note_edit_window_ms && hit(&s) {
                    out.push(note(&m.p.pid, &m.p.name, NoteKind::Edited, &s, at));
                }
            }
            if m.p.kind == ParticipantKind::Person {
                if let Some((a, h)) = &m.grid.cell {
                    let s = GridSpan {
                        rows: Some((a.0.clone(), h.0.clone())),
                        cols: Some((a.1.clone(), h.1.clone())),
                    };
                    if hit(&s) {
                        let kind = if a == h {
                            NoteKind::Caret
                        } else {
                            NoteKind::Selection
                        };
                        let at = m.last_cursor_ms.unwrap_or(m.last_active_ms);
                        out.push(note(&m.p.pid, &m.p.name, kind, &s, at));
                    }
                }
            }
        }
        out.sort_by(|x, y| (x.kind, x.age_ms, &x.pid).cmp(&(y.kind, y.age_ms, &y.pid)));
        let mut seen: Vec<(String, NoteKind, Option<ClaimSpan>)> = Vec::new();
        out.retain(|n| {
            let k = (n.pid.clone(), n.kind, n.span.clone());
            if seen.contains(&k) {
                false
            } else {
                seen.push(k);
                true
            }
        });
        out
    }

    /// The heads-up notes for member `i` about a structured change's `edits`, deduped and
    /// rate-limited as text notes are.
    pub(super) fn grid_notes_for_edits(
        &self,
        st: &mut State,
        i: usize,
        edits: &[Edit],
        now: u64,
    ) -> Notes {
        if !self.is_grid() {
            return Notes::default();
        }
        let t = GridTouch::of_edits(edits);
        self.grid_notes(st, i, &t, now)
    }

    /// The heads-up notes for member `i` reading the rows `first..=last` (by key).
    pub(super) fn grid_notes_for_rows(
        &self,
        st: &mut State,
        i: usize,
        rows: &(Key, Key),
        now: u64,
    ) -> Notes {
        let Ok(axis) = AxisNow::of(&st.doc, table::ROWS) else {
            return Notes::default();
        };
        let t = GridTouch {
            rows: axis.members(rows).into_iter().collect(),
            ..GridTouch::default()
        };
        self.grid_notes(st, i, &t, now)
    }

    fn grid_notes(&self, st: &mut State, i: usize, t: &GridTouch, now: u64) -> Notes {
        let reader = st.members[i].p.pid.clone();
        // nobody else is anywhere on the grid: no notes about them, and no rendering for
        // them (notes waiting for the reader still come)
        let others = st
            .aware
            .claims
            .iter()
            .any(|c| c.grid.is_some() && c.pid != reader)
            || st.members.iter().any(|m| {
                m.p.pid != reader
                    && (m.grid.cell.is_some() || m.grid.view.is_some() || m.grid.edited.is_some())
            });
        let raw = if t.is_empty() || !others {
            Vec::new()
        } else {
            match self.grid_now(st) {
                Ok(g) => self.collect_grid_notes(st, &g, &reader, t, now),
                Err(_) => Vec::new(),
            }
        };
        self.batch_notes(st, i, raw, now)
    }

    // ----- views -----

    /// Set member `i`'s grid view to the rows on rendered lines `first..=last` (a read verb's
    /// range, a scrolled tab); `None` when no row is on them.
    pub(super) fn set_grid_view(
        &self,
        st: &mut State,
        i: usize,
        first: usize,
        last: usize,
        now: u64,
    ) -> Option<(Key, Key)> {
        let g = self.grid_now(st).ok()?;
        let rows = self.grid_rows_of_lines(&g, first, last)?;
        let m = &mut st.members[i];
        m.grid.view = Some(rows.clone());
        m.view = None;
        m.view_ms = Some(now);
        Some(rows)
    }

    /// [`Session::read_with`] on a grid (CSV contract §3.2, A10): the read verbs see the
    /// rendered file; the lines they showed become a view of the rows on them, the lines they
    /// looked at get the heads-up notes about those rows, and `--for-edit` takes a short
    /// claim on those rows.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn read_grid_with<R>(
        &self,
        st: &mut State,
        i: usize,
        verb: &str,
        for_edit: Option<Option<u64>>,
        ids: bool,
        now: u64,
        f: impl FnOnce(&ReadResult, &str, Option<&table::LineMap>) -> (R, Shown),
    ) -> Result<GridRead<R>> {
        let read = self.read_unmarked(st, i)?;
        let text = String::from_utf8_lossy(&read.bytes).into_owned();
        // --ids: the rendering's line -> row map, from the same state as `read.bytes`
        let map = if ids {
            self.pack
                .line_map(&View::new(st.doc.keyed_state()?), &st.codec)
        } else {
            None
        };
        let (
            r,
            Shown {
                view,
                touched,
                refused,
            },
        ) = match catch_unwind(AssertUnwindSafe(|| f(&read, &text, map.as_ref()))) {
            Ok(x) => x,
            // the caller releases the lock, then passes the panic on
            Err(payload) => return Ok(Err(payload)),
        };
        if refused_read(refused) {
            return Ok(Ok((read, r, Notes::default(), None)));
        }
        self.mark_read(st, i, read.version, now);
        // the verbs count `\n` lines; views, notes and claims go by the line map
        let bridge = table::NlLines::of(&text);
        let view = view.map(|v| nl_to_rendered(&bridge, v));
        let touched = touched.map(|t| nl_to_rendered(&bridge, t));
        if let Some((first, last)) = view {
            self.set_grid_view(st, i, first, last, now);
        }
        self.expire_claims(st, now);
        let rows = match (touched, self.grid_now(st)) {
            (Some((a, b)), Ok(g)) => self.grid_rows_of_lines(&g, a, b),
            _ => None,
        };
        let span = rows.clone().map(|r| GridSpan {
            rows: Some(r),
            cols: None,
        });
        let for_edit = for_edit.map(|ttl| self.grid_for_edit(st, i, span, ttl, verb, now));
        let notes = match &rows {
            Some(r) => self.grid_notes_for_rows(st, i, r, now),
            None => Notes::default(),
        };
        let lines = touched.map(|(first, last)| line_range((first, last)));
        self.log_activity(st, i, ActivityKind::Read, verb, lines, now, None);
        Ok(Ok((read, r, notes, for_edit)))
    }

    /// A grid read's `--for-edit`: a short claim on the rows (or cells) shown, unless another
    /// participant's claim covers them (then nothing is taken and that claim is reported).
    pub(super) fn grid_for_edit(
        &self,
        st: &mut State,
        i: usize,
        span: Option<GridSpan>,
        ttl_ms: Option<u64>,
        verb: &str,
        now: u64,
    ) -> awareness::ForEdit {
        use awareness::ForEdit;
        let Some(span) = span else {
            return ForEdit::NotTaken(SessionError::BadClaim(
                "the call showed no rows to claim".into(),
            ));
        };
        if !st.members[i].rights.edit {
            return ForEdit::NotTaken(SessionError::Forbidden { need: Need::Edit });
        }
        let ttl = ttl_ms.unwrap_or(self.config.for_edit_ms);
        match self.take_grid_claim(st, i, span, ttl, None, false, true, verb, now) {
            Ok(c) => ForEdit::Claimed(c),
            Err(SessionError::Claimed { id, .. }) => {
                match st.aware.claims.iter().position(|c| c.id == id) {
                    Some(k) => match self.any_claim_info(st, k, now) {
                        Ok(c) => ForEdit::Conflict(c),
                        Err(e) => ForEdit::NotTaken(e),
                    },
                    None => ForEdit::NotTaken(SessionError::BadClaim(format!("{id} ended"))),
                }
            }
            Err(e) => ForEdit::NotTaken(e),
        }
    }

    // ----- the roster -----

    /// Fill a roster row's grid presence from its connections (most active first).
    pub(super) fn grid_roster(
        &self,
        st: &State,
        g: &GridNow,
        conns: &[&Member],
        entry: &mut RosterEntry,
        now: u64,
    ) {
        entry.cell = conns
            .iter()
            .find_map(|m| m.grid.cell.as_ref())
            .map(|sel| self.cell_cursor(st, g, sel));
        let lines = |r: &(Key, Key)| {
            row_lines(g.lines.as_ref(), &g.rows, &g.rows.members(r)).map(line_range)
        };
        entry.view = conns
            .iter()
            .find_map(|m| m.grid.view.as_ref())
            .and_then(lines);
        entry.last_edit = conns
            .iter()
            .find_map(|m| m.grid.edited.as_ref())
            .and_then(lines);
        entry.claims = st
            .aware
            .claims
            .iter()
            .filter(|c| c.pid == entry.participant.pid && c.grid.is_some())
            .map(|c| Self::grid_claim_info(g, c, now))
            .collect();
    }

    // ----- write --base conflicts -----

    /// Who (other than member `i`) holds a live claim covering a merge conflict's region. A
    /// row's conflict touches the row and `row_cells`: the cells in it either side changed since
    /// V (the live side's writes and the text's own), so a claim on a column one of them is in
    /// is named; a column neither side changed in that row is not.
    pub(super) fn claimed_region(
        &self,
        st: &State,
        g: &GridNow,
        i: usize,
        at: &table::Region,
        row_cells: &[(Key, Key)],
    ) -> Vec<String> {
        let me = &st.members[i].p.pid;
        let t = match at {
            table::Region::Cell { row, col } => GridTouch {
                cells: vec![(row.clone(), col.clone())],
                ..GridTouch::default()
            },
            table::Region::Row(r) => GridTouch {
                rows: [r.clone()].into(),
                cells: row_cells.iter().filter(|(x, _)| x == r).cloned().collect(),
                ..GridTouch::default()
            },
            table::Region::Col(c) => GridTouch {
                cols: [c.clone()].into(),
                ..GridTouch::default()
            },
            table::Region::Name(c) => GridTouch {
                names: [c.clone()].into(),
                ..GridTouch::default()
            },
            table::Region::Quote(table::QuoteAt::Cell(r, c)) => GridTouch {
                cells: vec![(r.clone(), c.clone())],
                ..GridTouch::default()
            },
            table::Region::Quote(table::QuoteAt::Header(c)) => GridTouch {
                names: [c.clone()].into(),
                ..GridTouch::default()
            },
        };
        let mut out: Vec<String> = Vec::new();
        for c in &st.aware.claims {
            let Some(s) = c.grid.as_ref() else { continue };
            if c.pid != *me
                && touches(&t, s, s, (&g.rows, &g.cols), (&g.rows, &g.cols))
                && !out.contains(&c.pid)
            {
                out.push(c.pid.clone());
            }
        }
        out
    }
}

/// `\n` lines `(first, last)` as the rendered lines they hold ([`table::NlLines`]).
fn nl_to_rendered(bridge: &table::NlLines, (first, last): (usize, usize)) -> (usize, usize) {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::line_mutant()
        == Some(crate::differential::LineMutant::TouchedCountNlLines)
    {
        return (first, last);
    }
    bridge.rendered(first, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    fn axis(places: &[&str], dead: &[&str]) -> AxisNow {
        let live: HashSet<Key> = places
            .iter()
            .filter(|p| !dead.contains(p))
            .map(|p| k(p))
            .collect();
        AxisNow::new(places.iter().map(|p| k(p)).collect(), &live)
    }

    fn keys(v: &[Key]) -> Vec<&str> {
        v.iter().map(Key::as_str).collect()
    }

    #[test]
    fn members_are_between_the_ends_in_place_order() {
        let a = axis(&["r1", "r2", "r3", "r4", "r5"], &["r3"]);
        assert_eq!(keys(&a.members(&(k("r2"), k("r4")))), ["r2", "r4"]);
        // ends in either order
        assert_eq!(keys(&a.members(&(k("r4"), k("r2")))), ["r2", "r4"]);
        // a deleted end keeps its place
        assert_eq!(keys(&a.members(&(k("r3"), k("r5")))), ["r4", "r5"]);
        assert!(a.members(&(k("r3"), k("r3"))).is_empty());
        // a dropped end: nothing
        assert!(a.members(&(k("r9"), k("r2"))).is_empty());
    }

    #[test]
    fn the_plain_axis_applier_matches_the_ops() {
        let a = axis(&["r1", "r2", "r3"], &[]);
        let ins = ListOp::Insert {
            key: k("r4"),
            at: Anchor::After(k("r1")),
            value: crate::elem::Value::Null,
        };
        let mv = ListOp::Move {
            key: k("r3"),
            to: Anchor::Start,
        };
        let del = ListOp::Delete { key: k("r2") };
        let b = a.after(&[&ins, &mv, &del]);
        assert_eq!(keys(&b.places), ["r3", "r1", "r4", "r2"]);
        assert_eq!(keys(&b.live), ["r3", "r1", "r4"]);
        let c = b.after(&[&ListOp::Restore { key: k("r2") }]);
        assert_eq!(keys(&c.live), ["r3", "r1", "r4", "r2"]);
    }

    #[test]
    fn inserts_at_an_end_join_and_meeting_claims_share_nothing() {
        // claims: p1 on r1..r2, p2 on r3..r4; r9 inserted between r2 and r3
        let rows = axis(&["r1", "r2", "r9", "r3", "r4", "r5"], &[]);
        let cols = axis(&["c1"], &[]);
        let span = |a: &str, b: &str| GridSpan {
            rows: Some((k(a), k(b))),
            cols: None,
        };
        let claims = vec![
            (ClaimId::new(1), "p1".to_owned(), span("r1", "r2")),
            (ClaimId::new(2), "p2".to_owned(), span("r3", "r4")),
        ];
        // nobody's own: the first taken (p1's) gets it
        let s = expanded(&claims, &rows, &cols, &[k("r9")], &[], Some("p3"));
        assert_eq!(s[0].rows, Some((k("r1"), k("r9"))));
        assert_eq!(s[1].rows, Some((k("r3"), k("r4"))));
        // p2's own insert joins p2's claim, at its start
        let s = expanded(&claims, &rows, &cols, &[k("r9")], &[], Some("p2"));
        assert_eq!(s[0].rows, Some((k("r1"), k("r2"))));
        assert_eq!(s[1].rows, Some((k("r9"), k("r4"))));
        // after the last row: p2's claim grows at its end; a row inside doesn't move ends
        let rows = axis(&["r1", "r2", "r3", "r4", "r9", "r5"], &[]);
        let s = expanded(&claims, &rows, &cols, &[k("r9")], &[], None);
        assert_eq!(s[1].rows, Some((k("r3"), k("r9"))));
        let rows = axis(&["r1", "r9", "r2", "r3", "r4"], &[]);
        let s = expanded(&claims, &rows, &cols, &[k("r9")], &[], None);
        assert_eq!(s[0].rows, Some((k("r1"), k("r2"))));
        // ends given high-first keep their roles
        let claims = vec![(ClaimId::new(1), "p1".to_owned(), span("r2", "r1"))];
        let rows = axis(&["r1", "r2", "r9"], &[]);
        let s = expanded(&claims, &rows, &cols, &[k("r9")], &[], None);
        assert_eq!(s[0].rows, Some((k("r9"), k("r1"))));
    }

    #[test]
    fn touching_follows_the_contract() {
        let rows = axis(&["r1", "r2", "r3", "r4"], &[]);
        let cols = axis(&["c1", "c2", "c3"], &[]);
        let rows_claim = GridSpan {
            rows: Some((k("r2"), k("r3"))),
            cols: None,
        };
        let cols_claim = GridSpan {
            rows: None,
            cols: Some((k("c2"), k("c2"))),
        };
        let t = |f: &dyn Fn(&mut GridTouch)| {
            let mut t = GridTouch::default();
            f(&mut t);
            t
        };
        let hit = |t: &GridTouch, s: &GridSpan| touches(t, s, s, (&rows, &cols), (&rows, &cols));
        let cell = |r: &str, c: &str| t(&|x| x.cells.push((k(r), k(c))));
        assert!(hit(&cell("r2", "c1"), &rows_claim));
        assert!(!hit(&cell("r1", "c2"), &rows_claim));
        assert!(hit(&cell("r1", "c2"), &cols_claim));
        assert!(!hit(&cell("r1", "c1"), &cols_claim));
        let row_op = |r: &str| {
            t(&|x| {
                x.rows.insert(k(r));
            })
        };
        assert!(hit(&row_op("r3"), &rows_claim));
        assert!(!hit(&row_op("r4"), &rows_claim));
        assert!(
            !hit(&row_op("r3"), &cols_claim),
            "a row move doesn't touch a column"
        );
        let rename = t(&|x| {
            x.names.insert(k("c2"));
        });
        assert!(hit(&rename, &cols_claim));
        assert!(!hit(&rename, &rows_claim));
    }
}
