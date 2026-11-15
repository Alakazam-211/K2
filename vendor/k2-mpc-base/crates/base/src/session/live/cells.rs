//! A connection's cell caret and ranges on a live grid (`cells`), and claims on one range of
//! cells. A child of `live`, so it works on the session's own state. Only for a pack whose view
//! schema has a grid slot ([`crate::grid::GridSlot`]).
//!
//! - **Corners follow keys** ([`crate::grid::Corner`]): an insert inside a range grows it with
//!   no event; the kit never moves a corner.
//! - **Dropped, never moved:** after a change, a range whose corner's row or column stopped
//!   being live, or whose form the change broke (a keyed row now stands inside its offset), is
//!   dropped, the caret on its own the same way; the roster then carries what is left.
//! - **One container:** setting cells sets the connection's focus to their container, and a
//!   focus on another container drops them.
//! - **An agent's cells follow its verbs** ([`crate::grid::CellsReport`]) and its cells claims;
//!   a person's are never moved by the kit.
//! - **A cells claim** holds off other agents' writes to a cell inside it and deletes of a row
//!   or column inside it, under [`ClaimPolicy::Respect`]; its owner's own write inside ends it
//!   unless held; it ends ([`ClaimEnd::Deleted`]) when a corner breaks.
//!
//! Everything is memory only.

use super::*;
use crate::elem::{Applied as ElemApplied, Effect, Key};
use crate::grid::{
    Axis, BandOp, CellRange, Cells, CellsReport, Corner, CornerError, GridNumbers, GridSlot,
    CELLS_RANGES_MAX,
};
use crate::session::awareness::{CellsSpan, Claim, ClaimEnd, ClaimInfo};
use crate::session::error::ElemsClaim;
use crate::session::participant::ElemFocus;
use crate::table::CELL_SEP;
use crate::view_index::ViewSchema;

fn field_key(key: &Key, field: &str) -> String {
    format!("{key}{CELL_SEP}{field}")
}

fn bad(e: impl std::fmt::Display) -> SessionError {
    SessionError::BadPosition(e.to_string())
}

/// A range's rows and columns as numbers, low to high.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    rows: (u64, u64),
    cols: (u64, u64),
}

impl Rect {
    fn of(g: &GridNumbers, r: &CellRange) -> std::result::Result<Rect, CornerError> {
        let (a, b) = (g.cell(&r.start)?, g.cell(&r.end)?);
        Ok(Rect {
            rows: (a.0.min(b.0), a.0.max(b.0)),
            cols: (a.1.min(b.1), a.1.max(b.1)),
        })
    }

    fn holds(&self, row: u64, col: u64) -> bool {
        (self.rows.0..=self.rows.1).contains(&row) && (self.cols.0..=self.cols.1).contains(&col)
    }

    fn meets(&self, o: &Rect) -> bool {
        self.rows.0 <= o.rows.1
            && o.rows.0 <= self.rows.1
            && self.cols.0 <= o.cols.1
            && o.cols.0 <= self.cols.1
    }
}

impl Session {
    /// The view schema and its grid slot, when the pack has both.
    pub(super) fn grid_slot(&self) -> Option<(ViewSchema, GridSlot)> {
        let vs = self.pack.view_schema()?;
        let g = vs.grid?;
        Some((vs, g))
    }

    /// The value of a container's id (its `container_id_field`, else its key).
    fn container_id(&self, st: &State, vs: &ViewSchema, container: &Key) -> Option<String> {
        match vs.container_id_field {
            None => Some(container.as_str().to_owned()),
            Some(f) => {
                let map = self
                    .pack
                    .composite()
                    .iter()
                    .find(|(_, l)| *l == vs.containers)
                    .map(|(m, _)| *m)?;
                match st.doc.map_get(map, &field_key(container, f)) {
                    Ok(Some(crate::elem::Value::Str(s))) => Some(s),
                    _ => None,
                }
            }
        }
    }

    /// Container `container`'s keyed rows and columns with their numbers, read from the live
    /// doc; `None` without a grid slot or when the container isn't live.
    pub(super) fn grid_numbers(&self, st: &State, container: &Key) -> Option<GridNumbers> {
        let (vs, g) = self.grid_slot()?;
        if !self.elem_live(st, vs.containers, container) {
            return None;
        }
        let id = self.container_id(st, &vs, container)?;
        let read = |map: &str, k: &Key, f: &str| match st.doc.map_get(map, &field_key(k, f)) {
            Ok(Some(crate::elem::Value::Str(s))) => Some(s),
            _ => None,
        };
        let flag = |v: Option<String>| v.is_some_and(|v| !matches!(v.trim(), "" | "0" | "false"));
        let axis = |list: &str, map: &str, owner: &str, gap: &str, hide: Option<&str>| {
            let mut hidden: Vec<Key> = Vec::new();
            let els: Vec<(Key, u64)> = st
                .doc
                .list(list)
                .unwrap_or_default()
                .into_iter()
                .filter(|(k, _)| read(map, k, owner).as_deref() == Some(id.as_str()))
                .map(|(k, _)| {
                    let n = read(map, &k, gap)
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .unwrap_or(0);
                    if hide.is_some_and(|h| flag(read(map, &k, h))) {
                        hidden.push(k.clone());
                    }
                    (k, n)
                })
                .collect();
            (els, hidden)
        };
        let (rows, hidden_rows) = axis(
            g.rows,
            g.cells,
            vs.unit_container_field,
            g.row_numbering.gap_field,
            g.row_hidden,
        );
        let (cols, hidden_cols) = axis(
            g.cols,
            g.col_fields,
            g.col_container_field,
            g.col_numbering.gap_field,
            g.col_hidden,
        );
        Some(
            GridNumbers::new(&g, rows, cols)
                .hide(Axis::Rows, &hidden_rows)
                .hide(Axis::Cols, &hidden_cols),
        )
    }

    /// The rows and columns of the container at `container` (an address) with their numbers
    /// now, for a doc whose pack has a grid slot: what corners and A1 names are worked out
    /// from. `None`: no grid, or no live container there.
    pub fn grid_numbers_at(&self, container: &str) -> Result<Option<GridNumbers>> {
        let Some((vs, _)) = self.grid_slot() else {
            return Ok(None);
        };
        let Some(k) = self.pack.key_of(vs.containers, container) else {
            return Ok(None);
        };
        let st = self.lock()?;
        Ok(self.grid_numbers(&st, &k))
    }

    /// `cells`, checked against the live doc: its container live, at most
    /// [`CELLS_RANGES_MAX`] ranges, every corner in normal form within the sheet, and the caret
    /// inside a range when ranges are given.
    fn checked_cells(&self, st: &State, c: &Cells) -> Result<()> {
        let Some((vs, _)) = self.grid_slot() else {
            return Err(bad("this doc has no grid: no cells to set"));
        };
        if c.ranges.len() > CELLS_RANGES_MAX {
            return Err(SessionError::SelectionTooLarge {
                len: c.ranges.len(),
                max: CELLS_RANGES_MAX,
            });
        }
        let Some(container) = &c.container else {
            return Err(bad("cells name no container"));
        };
        if !self.elem_live(st, vs.containers, container) {
            return Err(bad(format!(
                "container {container} of {:?} is not live (unknown or deleted)",
                vs.containers
            )));
        }
        let g = self
            .grid_numbers(st, container)
            .ok_or_else(|| bad("the container's grid can't be read"))?;
        let mut rects = Vec::with_capacity(c.ranges.len());
        for r in &c.ranges {
            rects.push(Rect::of(&g, r).map_err(bad)?);
        }
        if let Some(a) = &c.active {
            let (row, col) = g.cell(a).map_err(bad)?;
            if !rects.is_empty() && !rects.iter().any(|x| x.holds(row, col)) {
                return Err(bad("the active cell is outside every range"));
            }
        }
        Ok(())
    }

    /// Fill a cells value's addresses through the pack.
    fn addressed(&self, mut c: Cells) -> Cells {
        address_cells(&*self.pack, &mut c);
        c
    }

    /// Set this connection's cell caret and ranges on a grid, or clear them with `None` (or a
    /// value with no caret and no range). Needs read, as a selection; a watcher is refused
    /// ([`SessionError::Watching`]). Refused, the value as it was: more than
    /// [`CELLS_RANGES_MAX`] ranges ([`SessionError::SelectionTooLarge`]); a container that isn't
    /// live, a corner whose row or column isn't a live keyed one of that container, a corner not
    /// in normal form or past the sheet's last row or column, a caret outside every range
    /// ([`SessionError::BadPosition`]). It **sets the focus** to the container. Returns the value
    /// as stored, by address.
    pub fn set_cells(&self, conn: ConnId, cells: Option<Cells>) -> Result<Option<Cells>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Cursor, now)?;
        if st.members[i].watcher {
            return Err(SessionError::Watching {
                max: self.config.participants_max,
            });
        }
        let cells = cells.filter(|c| c.active.is_some() || !c.ranges.is_empty());
        let Some(c) = cells else {
            st.members[i].elem.cells = None;
            return Ok(None);
        };
        self.checked_cells(st, &c)?;
        let (vs, _) = self.grid_slot().expect("checked");
        let container = c.container.clone().expect("checked");
        let m = &mut st.members[i];
        m.last_cursor_ms = Some(now);
        m.elem
            .cells_moved(ElemFocus::new(vs.containers, container), Some(c.clone()));
        Ok(Some(self.addressed(c)))
    }

    /// [`Session::set_cells`] by address: the container and every corner's row and column are
    /// given by address ([`Cells::at`], [`Corner::at`]); a corner's `None` is "no keyed row (or
    /// column) at or before the cell". An address that names no element of the grid's
    /// containers, rows or columns is refused ([`SessionError::BadPosition`]).
    pub fn set_cells_at(&self, conn: ConnId, cells: Option<&Cells>) -> Result<Option<Cells>> {
        let Some(c) = cells else {
            return self.set_cells(conn, None);
        };
        let (vs, g) = self
            .grid_slot()
            .ok_or_else(|| bad("this doc has no grid: no cells to set"))?;
        let key = |root: &str, a: &str| {
            self.pack
                .key_of(root, a)
                .ok_or_else(|| bad(format!("{a:?} names no element of {root:?}")))
        };
        let corner = |x: &Corner| -> Result<Corner> {
            let row = x
                .row_address
                .as_deref()
                .map(|a| key(g.rows, a))
                .transpose()?;
            let col = x
                .col_address
                .as_deref()
                .map(|a| key(g.cols, a))
                .transpose()?;
            Ok(Corner::new(row, x.row_off, col, x.col_off))
        };
        let ranges = c
            .ranges
            .iter()
            .map(|r| Ok(CellRange::new(corner(&r.start)?, corner(&r.end)?)))
            .collect::<Result<Vec<_>>>()?;
        let active = c.active.as_ref().map(corner).transpose()?;
        let keyed = Cells::new(key(vs.containers, &c.address)?, active, ranges);
        self.set_cells(conn, Some(keyed))
    }

    /// After anything that changed the doc (`changed`: what it applied; `None`: anything may
    /// have): every connection's cells are checked again when the change touched a grid's rows
    /// or columns (their lists, their gap or container fields). A range with a broken corner is
    /// dropped, and the caret on its own; cells left with neither are gone.
    pub(super) fn cells_after(&self, st: &mut State, changed: Option<&[ElemApplied]>) {
        let Some((vs, g)) = self.grid_slot() else {
            return;
        };
        if !st.members.iter().any(|m| m.elem.cells.is_some()) {
            return;
        }
        if let Some(stream) = changed {
            if !touches_axes(&vs, &g, stream) {
                return;
            }
        }
        let mut numbers: Vec<(Key, Option<GridNumbers>)> = Vec::new();
        for i in 0..st.members.len() {
            let Some(c) = st.members[i].elem.cells.clone() else {
                continue;
            };
            let Some(container) = c.container.clone() else {
                continue;
            };
            if !numbers.iter().any(|(k, _)| *k == container) {
                let n = self.grid_numbers(st, &container);
                numbers.push((container.clone(), n));
            }
            let g = numbers
                .iter()
                .find(|(k, _)| *k == container)
                .and_then(|(_, n)| n.as_ref());
            let kept = g.and_then(|g| kept_cells(g, &c));
            st.members[i].elem.cells = kept;
        }
    }

    /// After a pack verb's change by member `i` landed: an **agent's** cells follow what the
    /// verb wrote ([`CellsReport`]), and its focus goes to that container. A person's, a
    /// watcher's or a verb's that reports nothing are left as they are.
    pub(super) fn verb_cells(&self, st: &mut State, i: usize, report: Option<CellsReport>) {
        let Some(report) = report else {
            return;
        };
        let m = &st.members[i];
        if m.p.kind != ParticipantKind::Agent || m.watcher {
            return;
        }
        let Some((vs, _)) = self.grid_slot() else {
            return;
        };
        let container = report.container().clone();
        let Some(g) = self.grid_numbers(st, &container) else {
            return;
        };
        let (max_rows, max_cols) = g.max();
        let cells = match report {
            CellsReport::Written { ranges, .. } => {
                let ranges: Vec<CellRange> = ranges
                    .into_iter()
                    .filter(|r| Rect::of(&g, r).is_ok())
                    .take(CELLS_RANGES_MAX)
                    .collect();
                let active = ranges.first().map(|r| r.start.clone());
                (!ranges.is_empty()).then(|| Cells::new(container.clone(), active, ranges))
            }
            CellsReport::Band { band, .. } => {
                let (from, to) = (band.from.max(1), band.to.max(band.from).max(1));
                match (band.op, band.axis) {
                    (BandOp::Insert, Axis::Rows) => {
                        let r = CellRange::new(g.corner(from, 1), g.corner(to, max_cols));
                        Some(Cells::new(
                            container.clone(),
                            Some(r.start.clone()),
                            vec![r],
                        ))
                    }
                    (BandOp::Insert, Axis::Cols) => {
                        let r = CellRange::new(g.corner(1, from), g.corner(max_rows, to));
                        Some(Cells::new(
                            container.clone(),
                            Some(r.start.clone()),
                            vec![r],
                        ))
                    }
                    // the first visible cell after the band (else the last before it)
                    (BandOp::Delete, Axis::Rows) => Some(Cells::new(
                        container.clone(),
                        Some(g.corner(
                            g.visible_near(Axis::Rows, from),
                            g.visible_near(Axis::Cols, 1),
                        )),
                        Vec::new(),
                    )),
                    (BandOp::Delete, Axis::Cols) => Some(Cells::new(
                        container.clone(),
                        Some(g.corner(
                            g.visible_near(Axis::Rows, 1),
                            g.visible_near(Axis::Cols, from),
                        )),
                        Vec::new(),
                    )),
                }
            }
            CellsReport::Container { .. } => match &st.members[i].elem.cells {
                Some(c) if c.container.as_ref() == Some(&container) => Some(c.clone()),
                _ => Some(Cells::new(
                    container.clone(),
                    Some(g.corner(1, 1)),
                    Vec::new(),
                )),
            },
        };
        st.members[i]
            .elem
            .cells_moved(ElemFocus::new(vs.containers, container), cells);
    }

    // ----- claims -----

    /// Claim one range of a grid's cells on `container` for `ttl_ms`, as
    /// [`Session::claim_elems`] claims elements (the same defaults, limits, note and `hold`).
    /// Needs edit. Advisory: under [`ClaimPolicy::Respect`] it refuses another **agent's** write
    /// to a cell inside it and delete of a row or column inside it ([`SessionError::ElemsClaimed`],
    /// exit 5), never a person's; inserts inside grow it. Its owner's own write inside ends it
    /// unless held. It ends ([`ClaimEnd::Deleted`]) when a corner's row or column goes or a
    /// change breaks a corner's form.
    ///
    /// An agent's claim moves its focus to the container and its cells to the range (caret at
    /// its start); its selection is left alone.
    ///
    /// Errors: another participant's live cells claim meeting the range gives
    /// [`SessionError::ElemsClaimed`]; a bad container or corner gives
    /// [`SessionError::BadPosition`]; a doc with no grid, a note too long or the limits give
    /// [`SessionError::BadClaim`].
    pub fn claim_cells(
        &self,
        conn: ConnId,
        container: &Key,
        range: CellRange,
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        let out = self.claim_cells_inner(conn, container, range, ttl_ms, note, hold);
        self.for_viewer(conn, out, |l, c| l.claim(c))
    }

    /// [`Session::claim_cells`] by address: `cells` given as for [`Session::set_cells_at`],
    /// with exactly one range (more is [`SessionError::BadClaim`]) and no caret.
    pub fn claim_cells_at(
        &self,
        conn: ConnId,
        cells: &Cells,
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        if cells.ranges.len() != 1 {
            return Err(SessionError::BadClaim(format!(
                "a cells claim is one range, not {}",
                cells.ranges.len()
            )));
        }
        let (vs, g) = self
            .grid_slot()
            .ok_or_else(|| SessionError::BadClaim("this doc has no grid".into()))?;
        let key = |root: &str, a: &str| {
            self.pack
                .key_of(root, a)
                .ok_or_else(|| bad(format!("{a:?} names no element of {root:?}")))
        };
        let corner = |x: &Corner| -> Result<Corner> {
            let row = x
                .row_address
                .as_deref()
                .map(|a| key(g.rows, a))
                .transpose()?;
            let col = x
                .col_address
                .as_deref()
                .map(|a| key(g.cols, a))
                .transpose()?;
            Ok(Corner::new(row, x.row_off, col, x.col_off))
        };
        let r = &cells.ranges[0];
        let range = CellRange::new(corner(&r.start)?, corner(&r.end)?);
        let container = key(vs.containers, &cells.address)?;
        self.claim_cells(conn, &container, range, ttl_ms, note, hold)
    }

    fn claim_cells_inner(
        &self,
        conn: ConnId,
        container: &Key,
        range: CellRange,
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.expire_claims(st, now);
        let ttl = self.claim_ttl(ttl_ms.unwrap_or(self.config.claim_default_ms))?;
        let Some((vs, g)) = self.grid_slot() else {
            return Err(SessionError::BadClaim("this doc has no grid".into()));
        };
        Self::check_claim_note(note.as_deref())?;
        let Some(numbers) = self.grid_numbers(st, container) else {
            return Err(bad(format!(
                "container {container} of {:?} is not live (unknown or deleted)",
                vs.containers
            )));
        };
        let rect = Rect::of(&numbers, &range).map_err(bad)?;
        let pid = st.members[i].p.pid.clone();
        for c in &st.aware.claims {
            let Some(s) = c.cells.as_ref().filter(|s| s.container == *container) else {
                continue;
            };
            let Ok(other) = Rect::of(&numbers, &s.range) else {
                continue;
            };
            if !other.meets(&rect) {
                continue;
            }
            if c.pid != pid {
                return Err(cells_refusal(&g, s, c, now));
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
        let m = &st.members[i];
        let span = CellsSpan {
            container: container.clone(),
            range: range.clone(),
        };
        let claim = Claim {
            id: awareness::ClaimId::fresh(),
            pid: pid.clone(),
            name: m.p.name.clone(),
            kind: m.p.kind,
            colour: m.p.colour.clone(),
            conn: m.conn,
            root: g.rows.to_owned(),
            s: 0,
            e: 0,
            taken_ms: now,
            expires_ms: now.saturating_add(ttl),
            note,
            hold,
            for_edit: false,
            waiters: std::collections::VecDeque::new(),
            edited: false,
            grid: None,
            elems: None,
            cells: Some(span),
        };
        let info = self.cell_claim_info(st, &claim, now);
        let mut actor = claim.actor();
        if planted_claim_actor_missing() {
            actor.name = String::new();
            actor.colour = String::new();
        }
        st.aware.claims.push(claim);
        let mut event_range = range.clone();
        address_range(&*self.pack, &g, &mut event_range);
        st.roster.record(
            RosterEvent::ClaimStartedCells {
                who: Who::of(&pid),
                claim: info.id,
                container: container.clone(),
                address: self.pack.address_of(vs.containers, container),
                range: event_range,
                note: info.note.clone(),
                hold,
                expires_ms: info.expires_ms,
                version: st.version,
                actor,
                started_ms: now,
            },
            self.config.roster_queue,
        );
        if st.members[i].p.kind == ParticipantKind::Agent {
            let cells = Cells::new(container.clone(), Some(range.start.clone()), vec![range]);
            st.members[i].elem.cells_moved(
                ElemFocus::new(vs.containers, container.clone()),
                Some(cells),
            );
        }
        self.emit(AuditEvent::ClaimTaken {
            key: st.key.clone(),
            claim: info.id,
            pid,
            conn: info.conn,
            lines: info.lines.clone(),
            expires_ms: info.expires_ms,
            hold,
            for_edit: false,
        });
        self.log_activity(
            st,
            i,
            ActivityKind::Claim,
            "claim",
            Some(info.lines.clone()),
            now,
            Some(info.id),
        );
        Ok(info)
    }

    /// A cells claim's `lines`: the grid's rows root and the range's first and last row
    /// numbers now (`1, 1` when a corner can't be read), for display only.
    pub(super) fn cell_lines(&self, st: &State, s: &CellsSpan) -> LineRange {
        let root = self
            .grid_slot()
            .map_or_else(String::new, |(_, g)| g.rows.to_owned());
        let rows = self
            .grid_numbers(st, &s.container)
            .and_then(|g| Rect::of(&g, &s.range).ok())
            .map_or((1, 1), |r| r.rows);
        LineRange {
            root,
            first: rows.0 as usize,
            last: rows.1 as usize,
        }
    }

    pub(super) fn cell_claim_info(&self, st: &State, c: &Claim, now: u64) -> ClaimInfo {
        let span = c.cells.as_ref().expect("a cells claim");
        let mut info = c.info("", now);
        info.lines = self.cell_lines(st, span);
        let mut s = span.clone();
        if let Some((vs, g)) = self.grid_slot() {
            address_range(&*self.pack, &g, &mut s.range);
            // a cells claim's container by address: what the wire's `container` carries
            info.addresses = vec![self.pack.address_of(vs.containers, &span.container)];
        }
        info.span = s.span();
        info.start = 0;
        info.end = 0;
        info
    }

    /// For each cell write, row delete and column delete in `edits`: `(container, row number,
    /// col number)` per cell, `(container, Some(row), None)` per row, `(container, None,
    /// Some(col))` per column, numbered in the doc as it is (before the edits).
    fn cells_named(&self, st: &State, edits: &[Edit]) -> Vec<(Key, Option<u64>, Option<u64>)> {
        let Some((vs, g)) = self.grid_slot() else {
            return Vec::new();
        };
        let containers: Vec<Key> = st
            .aware
            .claims
            .iter()
            .filter_map(|c| c.cells.as_ref().map(|s| s.container.clone()))
            .collect();
        let mut out = Vec::new();
        for container in containers.iter().fold(Vec::<Key>::new(), |mut a, k| {
            if !a.contains(k) {
                a.push(k.clone());
            }
            a
        }) {
            let Some(numbers) = self.grid_numbers(st, &container) else {
                continue;
            };
            let caddr = self.pack.address_of(vs.containers, &container);
            for e in edits {
                match e {
                    Edit::Map { root, ops } if root == g.cells => {
                        for o in ops {
                            let Some((row, field)) = o.key().split_once(CELL_SEP) else {
                                continue;
                            };
                            let Ok(row) = Key::new(row) else {
                                continue;
                            };
                            let (Some(rn), Some(ca)) = (
                                numbers.number(Axis::Rows, &row),
                                g.col_address(&caddr, field),
                            ) else {
                                continue;
                            };
                            let cn = self
                                .pack
                                .key_of(g.cols, &ca)
                                .and_then(|k| numbers.number(Axis::Cols, &k));
                            if let Some(cn) = cn {
                                out.push((container.clone(), Some(rn), Some(cn)));
                            }
                        }
                    }
                    Edit::List { root, ops } if root == g.rows || root == g.cols => {
                        let axis = if root == g.rows {
                            Axis::Rows
                        } else {
                            Axis::Cols
                        };
                        for o in ops {
                            if !matches!(o, crate::elem::ListOp::Delete { .. }) {
                                continue;
                            }
                            if let Some(n) = numbers.number(axis, o.key()) {
                                out.push(match axis {
                                    Axis::Rows => (container.clone(), Some(n), None),
                                    Axis::Cols => (container.clone(), None, Some(n)),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// The claim policy for cells claims, for an agent (the caller checked the policy and the
    /// kind): a change writing a cell inside another participant's cells claim, or deleting a
    /// row or column inside one, is refused before anything applies.
    pub(super) fn check_cell_claims(
        &self,
        st: &State,
        i: usize,
        edits: &[Edit],
        now: u64,
    ) -> Result<()> {
        if !st.aware.claims.iter().any(|c| c.cells.is_some()) {
            return Ok(());
        }
        let Some((_, g)) = self.grid_slot() else {
            return Ok(());
        };
        let named = self.cells_named(st, edits);
        if named.is_empty() {
            return Ok(());
        }
        let pid = &st.members[i].p.pid;
        for c in &st.aware.claims {
            let Some(s) = c.cells.as_ref().filter(|_| c.pid != *pid) else {
                continue;
            };
            let Some(rect) = self
                .grid_numbers(st, &s.container)
                .and_then(|n| Rect::of(&n, &s.range).ok())
            else {
                continue;
            };
            let hit = named.iter().any(|(k, r, col)| {
                *k == s.container
                    && match (r, col) {
                        (Some(r), Some(col)) => rect.holds(*r, *col),
                        (Some(r), None) => (rect.rows.0..=rect.rows.1).contains(r),
                        (None, Some(col)) => (rect.cols.0..=rect.cols.1).contains(col),
                        (None, None) => false,
                    }
            });
            if hit {
                return Err(cells_refusal(&g, s, c, now));
            }
        }
        Ok(())
    }

    /// Member `i`'s own cells claims (not held) that `edits` write inside or delete a row or
    /// column of: what ends them once the change is durable ([`Session::mark_cell_claims`]).
    pub(super) fn own_cell_claims_touched(
        &self,
        st: &State,
        i: usize,
        edits: &[Edit],
    ) -> Vec<awareness::ClaimId> {
        let pid = &st.members[i].p.pid;
        if !st
            .aware
            .claims
            .iter()
            .any(|c| c.cells.is_some() && c.pid == *pid && !c.hold)
        {
            return Vec::new();
        }
        let named = self.cells_named(st, edits);
        let mut out = Vec::new();
        for c in &st.aware.claims {
            let Some(s) = c.cells.as_ref().filter(|_| c.pid == *pid && !c.hold) else {
                continue;
            };
            let Some(rect) = self
                .grid_numbers(st, &s.container)
                .and_then(|n| Rect::of(&n, &s.range).ok())
            else {
                continue;
            };
            if named.iter().any(|(k, r, col)| {
                *k == s.container
                    && match (r, col) {
                        (Some(r), Some(col)) => rect.holds(*r, *col),
                        (Some(r), None) => (rect.rows.0..=rect.rows.1).contains(r),
                        (None, Some(col)) => (rect.cols.0..=rect.cols.1).contains(col),
                        (None, None) => false,
                    }
            }) {
                out.push(c.id);
            }
        }
        out
    }

    /// Mark the claims [`Session::own_cell_claims_touched`] named as edited (they end once the
    /// change is durable).
    pub(super) fn mark_cell_claims(&self, st: &mut State, ids: &[awareness::ClaimId]) {
        for c in &mut st.aware.claims {
            if ids.contains(&c.id) {
                c.edited = true;
            }
        }
    }

    /// After a structured change (`stream`): a cells claim on a grid whose rows or columns the
    /// change touched ends ([`ClaimEnd::Deleted`]) when a corner's row or column stopped being
    /// live, its form broke, or its container went.
    pub(super) fn cell_claims_after(&self, st: &mut State, stream: &[ElemApplied], now: u64) {
        if !st.aware.claims.iter().any(|c| c.cells.is_some()) {
            return;
        }
        let Some((vs, g)) = self.grid_slot() else {
            return;
        };
        if !touches_axes(&vs, &g, stream) {
            return;
        }
        let gone: Vec<awareness::ClaimId> = st
            .aware
            .claims
            .iter()
            .filter(|c| {
                c.cells.as_ref().is_some_and(|s| {
                    self.grid_numbers(st, &s.container)
                        .and_then(|n| Rect::of(&n, &s.range).ok())
                        .is_none()
                })
            })
            .map(|c| c.id)
            .collect();
        for id in gone {
            if let Some(k) = st.aware.claims.iter().position(|c| c.id == id) {
                self.end_claim(st, k, ClaimEnd::Deleted, now, false);
            }
        }
    }
}

/// What is left of `c` on numbers `g`: ranges whose corners both hold, the caret when it holds;
/// `None` when nothing does. (Planted [`crate::differential::GridMutant::CornersMoveNotDrop`]:
/// a corner whose form broke is moved to the cell its numbers named, in normal form.)
fn kept_cells(g: &GridNumbers, c: &Cells) -> Option<Cells> {
    let fix = |x: &Corner| -> Option<Corner> {
        match g.cell(x) {
            Ok(_) => Some(x.clone()),
            Err(CornerError::NotNormal(_)) if planted_corners_move() => {
                let row = match &x.row {
                    Some(k) => g.number(Axis::Rows, k)? + x.row_off,
                    None => x.row_off,
                };
                let col = match &x.col {
                    Some(k) => g.number(Axis::Cols, k)? + x.col_off,
                    None => x.col_off,
                };
                Some(g.corner(row, col))
            }
            Err(_) => None,
        }
    };
    let ranges: Vec<CellRange> = c
        .ranges
        .iter()
        .filter_map(|r| Some(CellRange::new(fix(&r.start)?, fix(&r.end)?)))
        .collect();
    let active = c.active.as_ref().and_then(fix);
    if ranges.is_empty() && active.is_none() {
        return None;
    }
    let mut out = c.clone();
    out.ranges = ranges;
    out.active = active;
    Some(out)
}

/// A change touched a grid's rows or columns: a list op on either, or a map op on a gap or
/// container field of either.
fn touches_axes(vs: &ViewSchema, g: &GridSlot, stream: &[ElemApplied]) -> bool {
    let field_of = |k: &str| k.split_once(CELL_SEP).map(|(_, f)| f.to_owned());
    stream.iter().any(|a| match a {
        ElemApplied::List { root, ops } => {
            (root == g.rows || root == g.cols || root == vs.containers)
                && ops.iter().any(|o| !matches!(o.effect, Effect::None { .. }))
        }
        ElemApplied::Map { root, ops } => {
            let fields: &[&str] = if root == g.cells {
                &[g.row_numbering.gap_field, vs.unit_container_field]
            } else if root == g.col_fields {
                &[g.col_numbering.gap_field, g.col_container_field]
            } else {
                return false;
            };
            ops.iter().any(|o| {
                !matches!(o.effect, Effect::None { .. })
                    && field_of(&o.key).is_some_and(|f| fields.contains(&f.as_str()))
            })
        }
        _ => false,
    })
}

/// The refusal a cells claim gives another agent's write ([`SessionError::ElemsClaimed`],
/// exit 5): the claim, its holder and its note; `root` is the grid's cells, `keys` the
/// container.
fn cells_refusal(g: &GridSlot, s: &CellsSpan, c: &Claim, now: u64) -> SessionError {
    SessionError::ElemsClaimed(Box::new(ElemsClaim {
        id: c.id,
        by: c.pid.clone(),
        name: c.name.clone(),
        root: g.cells.to_owned(),
        keys: vec![s.container.clone()],
        remaining_ms: c.remaining(now),
        note: c.note.clone(),
    }))
}

/// A corner's addresses filled through the pack, on the grid's rows and columns.
fn address_corner<P: Pack + ?Sized>(pack: &P, g: &GridSlot, c: &mut Corner) {
    c.row_address = c.row.as_ref().map(|k| pack.address_of(g.rows, k));
    c.col_address = c.col.as_ref().map(|k| pack.address_of(g.cols, k));
}

fn address_range<P: Pack + ?Sized>(pack: &P, g: &GridSlot, r: &mut CellRange) {
    address_corner(pack, g, &mut r.start);
    address_corner(pack, g, &mut r.end);
}

/// A cells value's addresses (its container's and every corner's) filled through the pack.
pub(super) fn address_cells<P: Pack + ?Sized>(pack: &P, c: &mut Cells) {
    let Some(vs) = pack.view_schema() else {
        return;
    };
    let Some(g) = vs.grid else {
        return;
    };
    if let Some(k) = &c.container {
        c.address = pack.address_of(vs.containers, k);
    }
    if let Some(a) = c.active.as_mut() {
        address_corner(pack, &g, a);
    }
    for r in &mut c.ranges {
        address_range(pack, &g, r);
    }
}

fn planted_corners_move() -> bool {
    plant_on!(
        crate::differential::grid_mutant()
            == Some(crate::differential::GridMutant::CornersMoveNotDrop)
    )
}

fn planted_claim_actor_missing() -> bool {
    plant_on!(
        crate::differential::grid_mutant()
            == Some(crate::differential::GridMutant::ClaimCellsActorMissing)
    )
}
