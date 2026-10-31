//! Grid ops: every grid verb's edits (contract §3.1), built against a [`Table`] and applied to
//! it, so the table stays the doc's view after the doc applies the same edits (one change).
//!
//! The builders check what a verb checks (keys known and live, values, the header) and refuse
//! with a [`TableError`]; a no-op returns no edits. Keys they mint come from the table's
//! counters, and the change writes the counters back (`meta.next_row` / `next_col`), so
//! several builders on one table make one change with distinct keys.
//!
//! The op rules (K-CSV-10): clear is a set to `""`; a multi-element move (a sort, a block
//! move) is one forward chain ([`crate::elem::chain`], [`crate::elem::reorder`]); a deleted
//! row or column comes back by restore.

use std::cmp::Ordering;

use super::*;
use crate::elem::{chain, reorder};

/// Edits being built, per root, applied to the table as they are added.
pub(crate) struct Change {
    lists: Vec<(&'static str, Vec<ListOp>)>,
    maps: Vec<(&'static str, Vec<MapOp>)>,
    counters: (u64, u64),
}

impl Change {
    pub(crate) fn new(t: &Table) -> Self {
        Change {
            lists: Vec::new(),
            maps: Vec::new(),
            counters: (t.next_row, t.next_col),
        }
    }

    pub(crate) fn list(
        &mut self,
        t: &mut Table,
        root: &'static str,
        op: ListOp,
    ) -> Result<(), TableError> {
        t.apply_list(root, &op)?;
        match self.lists.iter_mut().find(|(r, _)| *r == root) {
            Some((_, ops)) => ops.push(op),
            None => self.lists.push((root, vec![op])),
        }
        Ok(())
    }

    fn map(&mut self, t: &mut Table, root: &'static str, op: MapOp) {
        t.apply_map(root, &op)
            .expect("a table's own map op applies");
        match self.maps.iter_mut().find(|(r, _)| *r == root) {
            Some((_, ops)) => ops.push(op),
            None => self.maps.push((root, vec![op])),
        }
    }

    /// Set (or, with `None`, delete) a cell, if that changes it.
    pub(crate) fn cell(&mut self, t: &mut Table, r: &Key, c: &Key, v: Option<&str>) {
        if t.cell(r, c) != v {
            self.map(t, CELLS, map_op(cell_key(r, c), v));
        }
    }

    pub(crate) fn name(&mut self, t: &mut Table, c: &Key, v: Option<&str>) {
        if t.name(c) != v {
            self.map(t, NAMES, map_op(c.as_str().to_owned(), v));
        }
    }

    pub(crate) fn quote(&mut self, t: &mut Table, at: QuoteAt, q: Option<Quote>) {
        if t.quote(&at) != q {
            let key = at.map_key();
            self.map(t, QUOTING, map_op(key, q.map(Quote::spelling)));
        }
    }

    pub(crate) fn meta(&mut self, t: &mut Table, k: &str, v: &str) {
        self.map(t, META, map_op(k.to_owned(), Some(v)));
    }

    /// The edits, roots in [`ROOTS`] order; the key counters written if minting moved them.
    pub(crate) fn finish(mut self, t: &mut Table) -> Vec<Edit> {
        if t.next_row != self.counters.0 {
            let n = t.next_row.to_string();
            self.meta(t, "next_row", &n);
        }
        if t.next_col != self.counters.1 {
            let n = t.next_col.to_string();
            self.meta(t, "next_col", &n);
        }
        let mut out = Vec::new();
        for (root, _) in ROOTS {
            if let Some((_, ops)) = self.lists.iter_mut().find(|(r, _)| r == root) {
                out.push(Edit::List {
                    root: (*root).to_owned(),
                    ops: std::mem::take(ops),
                });
            }
            if let Some((_, ops)) = self.maps.iter_mut().find(|(r, _)| r == root) {
                out.push(Edit::Map {
                    root: (*root).to_owned(),
                    ops: std::mem::take(ops),
                });
            }
        }
        out
    }
}

/// One sort key (`sort --by C [--desc] [--numeric]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub col: Key,
    pub desc: bool,
    /// Compare a leading decimal number; empty and non-numeric cells go last.
    pub numeric: bool,
}

impl SortKey {
    pub fn by(col: Key) -> Self {
        SortKey {
            col,
            desc: false,
            numeric: false,
        }
    }
}

/// A leading decimal number (`[+-]?digits[.digits]`, or `.digits`), if the value starts with
/// one.
fn leading_number(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let int = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int;
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        frac = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if frac > 0 {
            i += 1 + frac;
        }
    }
    if int == 0 && frac == 0 {
        return None;
    }
    s[..i].parse().ok()
}

impl Table {
    fn live(&self, root: &'static str, k: &Key) -> Result<(), TableError> {
        let a = if root == ROWS { &self.rows } else { &self.cols };
        match a.state(k) {
            ElemState::Live(_) => Ok(()),
            ElemState::Deleted => Err(TableError::Deleted {
                root,
                key: k.clone(),
            }),
            ElemState::Unknown => Err(TableError::Unknown {
                root,
                key: k.clone(),
            }),
        }
    }

    /// A verb's anchor: `After`/`Before` must name a live element other than `moving`.
    fn anchor(&self, root: &'static str, at: &Anchor, moving: &[Key]) -> Result<(), TableError> {
        if let Anchor::After(k) | Anchor::Before(k) = at {
            if moving.contains(k) {
                return Err(TableError::SelfAnchor {
                    root,
                    key: k.clone(),
                });
            }
            self.live(root, k)?;
        }
        Ok(())
    }

    /// `cell set`: one cell of a live row and column.
    pub fn set_cell(&mut self, row: &Key, col: &Key, value: &str) -> Result<Vec<Edit>, TableError> {
        self.live(ROWS, row)?;
        self.live(COLS, col)?;
        self.check(value)?;
        let mut ch = Change::new(self);
        ch.cell(self, row, col, Some(value));
        Ok(ch.finish(self))
    }

    /// Clear a cell: a set to `""` (K-CSV-10a), never a delete.
    pub fn clear_cell(&mut self, row: &Key, col: &Key) -> Result<Vec<Edit>, TableError> {
        self.set_cell(row, col, "")
    }

    /// `cells set --from R,C`: paste `block` (rows of values) with its top-left at (`row`,
    /// `col`), in the live order; rows past the end are added at the end. A block wider than
    /// the columns from `col` on is refused.
    pub fn set_block(
        &mut self,
        row: &Key,
        col: &Key,
        block: &[Vec<String>],
    ) -> Result<Vec<Edit>, TableError> {
        self.live(ROWS, row)?;
        self.live(COLS, col)?;
        let cols: Vec<Key> = self
            .cols
            .live()
            .skip_while(|c| *c != col)
            .cloned()
            .collect();
        let width = block.iter().map(Vec::len).max().unwrap_or(0);
        if width > cols.len() {
            return Err(TableError::TooManyValues {
                values: width,
                cols: cols.len(),
            });
        }
        for v in block.iter().flatten() {
            self.check(v)?;
        }
        let mut rows: Vec<Key> = self
            .rows
            .live()
            .skip_while(|r| *r != row)
            .cloned()
            .collect();
        let mut ch = Change::new(self);
        while rows.len() < block.len() {
            let k = self.mint_row();
            ch.list(
                self,
                ROWS,
                ListOp::Insert {
                    key: k.clone(),
                    at: Anchor::End,
                    value: Value::Null,
                },
            )?;
            rows.push(k);
        }
        for (vals, r) in block.iter().zip(&rows) {
            for (v, c) in vals.iter().zip(&cols) {
                ch.cell(self, r, c, Some(v));
            }
        }
        Ok(ch.finish(self))
    }

    /// `cells clear --from R,C --to R,C`: every present, non-empty cell of the rectangle
    /// (corners by key, in either order) set to `""`.
    pub fn clear_block(
        &mut self,
        from: (&Key, &Key),
        to: (&Key, &Key),
    ) -> Result<Vec<Edit>, TableError> {
        for (r, c) in [from, to] {
            self.live(ROWS, r)?;
            self.live(COLS, c)?;
        }
        let span = |a: &Axis, x: &Key, y: &Key| -> Vec<Key> {
            let live = a.live_keys();
            let i = live.iter().position(|k| k == x).expect("live");
            let j = live.iter().position(|k| k == y).expect("live");
            live[i.min(j)..=i.max(j)].to_vec()
        };
        let rows = span(&self.rows, from.0, to.0);
        let cols = span(&self.cols, from.1, to.1);
        let mut ch = Change::new(self);
        for r in &rows {
            for c in &cols {
                if self.cell(r, c).is_some_and(|v| !v.is_empty()) {
                    ch.cell(self, r, c, Some(""));
                }
            }
        }
        Ok(ch.finish(self))
    }

    /// `row insert`: a new row at `at`, its cells the `values` over the live columns in order
    /// (each given value a present cell). Returns its key.
    pub fn insert_row(
        &mut self,
        at: Anchor,
        values: &[String],
    ) -> Result<(Key, Vec<Edit>), TableError> {
        self.anchor(ROWS, &at, &[])?;
        let cols = self.live_cols();
        if values.len() > cols.len() {
            return Err(TableError::TooManyValues {
                values: values.len(),
                cols: cols.len(),
            });
        }
        for v in values {
            self.check(v)?;
        }
        let mut ch = Change::new(self);
        let k = self.mint_row();
        ch.list(
            self,
            ROWS,
            ListOp::Insert {
                key: k.clone(),
                at,
                value: Value::Null,
            },
        )?;
        for (v, c) in values.iter().zip(&cols) {
            ch.cell(self, &k, c, Some(v));
        }
        Ok((k, ch.finish(self)))
    }

    /// `col insert`: a new column at `at`, with a name if given (header on). Returns its key.
    pub fn insert_col(
        &mut self,
        at: Anchor,
        name: Option<&str>,
    ) -> Result<(Key, Vec<Edit>), TableError> {
        self.anchor(COLS, &at, &[])?;
        if let Some(n) = name {
            if !self.header {
                return Err(TableError::HeaderOff);
            }
            self.check(n)?;
        }
        let mut ch = Change::new(self);
        let k = self.mint_col();
        ch.list(
            self,
            COLS,
            ListOp::Insert {
                key: k.clone(),
                at,
                value: Value::Null,
            },
        )?;
        if let Some(n) = name {
            ch.name(self, &k, Some(n));
        }
        Ok((k, ch.finish(self)))
    }

    fn move_block(
        &mut self,
        root: &'static str,
        keys: &[Key],
        to: Anchor,
    ) -> Result<Vec<Edit>, TableError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut seen = HashSet::new();
        for k in keys {
            self.live(root, k)?;
            if !seen.insert(k) {
                return Err(TableError::Usage(format!("{root}: {k} given twice")));
            }
        }
        self.anchor(root, &to, keys)?;
        let ops = chain(keys, to);
        let mut after = self.clone();
        for op in &ops {
            after.apply_list(root, op)?;
        }
        let axis = |t: &Table| -> Vec<Key> {
            if root == ROWS {
                t.live_rows()
            } else {
                t.live_cols()
            }
        };
        if axis(&after) == axis(self) {
            return Ok(Vec::new());
        }
        let mut ch = Change::new(self);
        for op in ops {
            ch.list(self, root, op)?;
        }
        Ok(ch.finish(self))
    }

    /// `row move`: one live row to `to` (a live anchor). A move that changes nothing is no
    /// edit.
    pub fn move_row(&mut self, row: &Key, to: Anchor) -> Result<Vec<Edit>, TableError> {
        self.move_block(ROWS, std::slice::from_ref(row), to)
    }

    /// A block move of rows: `rows` (live, distinct), in this order, to `to`, as one forward
    /// chain (K-CSV-10b).
    pub fn move_rows(&mut self, rows: &[Key], to: Anchor) -> Result<Vec<Edit>, TableError> {
        self.move_block(ROWS, rows, to)
    }

    pub fn move_col(&mut self, col: &Key, to: Anchor) -> Result<Vec<Edit>, TableError> {
        self.move_block(COLS, std::slice::from_ref(col), to)
    }

    pub fn move_cols(&mut self, cols: &[Key], to: Anchor) -> Result<Vec<Edit>, TableError> {
        self.move_block(COLS, cols, to)
    }

    fn delete(&mut self, root: &'static str, k: &Key) -> Result<Vec<Edit>, TableError> {
        self.live(root, k)?;
        let mut ch = Change::new(self);
        ch.list(self, root, ListOp::Delete { key: k.clone() })?;
        Ok(ch.finish(self))
    }

    fn restore(&mut self, root: &'static str, k: &Key) -> Result<Vec<Edit>, TableError> {
        match self.live(root, k) {
            Ok(()) => Ok(Vec::new()),
            Err(TableError::Deleted { .. }) => {
                let mut ch = Change::new(self);
                ch.list(self, root, ListOp::Restore { key: k.clone() })?;
                Ok(ch.finish(self))
            }
            Err(e) => Err(e),
        }
    }

    /// `row delete`: hide a live row; its cells are kept (contract §1.6).
    pub fn delete_row(&mut self, row: &Key) -> Result<Vec<Edit>, TableError> {
        self.delete(ROWS, row)
    }

    /// `row restore`: show a deleted row again where it was, with its cells; no edit if it is
    /// live.
    pub fn restore_row(&mut self, row: &Key) -> Result<Vec<Edit>, TableError> {
        self.restore(ROWS, row)
    }

    pub fn delete_col(&mut self, col: &Key) -> Result<Vec<Edit>, TableError> {
        self.delete(COLS, col)
    }

    pub fn restore_col(&mut self, col: &Key) -> Result<Vec<Edit>, TableError> {
        self.restore(COLS, col)
    }

    /// `col rename`: set a live column's header text (header on).
    pub fn rename_col(&mut self, col: &Key, name: &str) -> Result<Vec<Edit>, TableError> {
        if !self.header {
            return Err(TableError::HeaderOff);
        }
        self.live(COLS, col)?;
        self.check(name)?;
        let mut ch = Change::new(self);
        ch.name(self, col, Some(name));
        Ok(ch.finish(self))
    }

    /// `sort --by`: the live rows, stably sorted by `by` (UTF-8 bytes, or a leading decimal
    /// number with empty and non-numeric cells last; `desc` reverses the comparison, never
    /// the order of ties), as **one change of one forward chain** of moves
    /// ([`crate::elem::reorder`]: the rows between the common prefix and suffix, in their new
    /// order). A sorted table is no edit. The header is not a row: it stays.
    pub fn sort(&mut self, by: &[SortKey]) -> Result<Vec<Edit>, TableError> {
        if by.is_empty() {
            return Err(TableError::Usage("sort needs a column".into()));
        }
        for k in by {
            self.live(COLS, &k.col)?;
        }
        let current = self.live_rows();
        let mut target = current.clone();
        let val = |r: &Key, c: &Key| self.cell(r, c).unwrap_or("");
        target.sort_by(|a, b| {
            for k in by {
                let (x, y) = (val(a, &k.col), val(b, &k.col));
                let o = if k.numeric {
                    match (leading_number(x), leading_number(y)) {
                        (Some(x), Some(y)) => {
                            let o = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
                            if k.desc {
                                o.reverse()
                            } else {
                                o
                            }
                        }
                        (Some(_), None) => Ordering::Less,
                        (None, Some(_)) => Ordering::Greater,
                        (None, None) => Ordering::Equal,
                    }
                } else if k.desc {
                    y.as_bytes().cmp(x.as_bytes())
                } else {
                    x.as_bytes().cmp(y.as_bytes())
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        });
        let ops = reorder(&current, &target).expect("a permutation of the live rows");
        let mut ch = Change::new(self);
        for op in ops {
            ch.list(self, ROWS, op)?;
        }
        Ok(ch.finish(self))
    }

    /// `header on|off` (contract §1.4): the rendering doesn't change, only how it is read
    /// (the codec's rule, so a CSV file's bytes never change). `on → off`: the names become
    /// a new first row (minted; names and header quoting move to its cells), or, when the
    /// header line isn't written ([`Table::header_renders`]: no live row, no name), only the
    /// setting changes. `off → on`: the first live row's cells become the names, and that
    /// row is deleted (hidden, as every delete); refused when it is the only live row and
    /// blank ([`TableError::HeaderOnlyRowBlank`], nothing applied), and, with no live row,
    /// when live columns hold names the header line would then show
    /// ([`TableError::HeaderNamesUnrendered`], nothing applied). No edit when it is already
    /// so.
    pub fn set_header(&mut self, on: bool) -> Result<Vec<Edit>, TableError> {
        if self.header == on {
            return Ok(Vec::new());
        }
        let cols = self.live_cols();
        if on {
            let mut live = self.rows.live();
            match (live.next(), live.next()) {
                (Some(only), None) if !cols.iter().any(|c| self.cell(only, c).is_some()) => {
                    return Err(TableError::HeaderOnlyRowBlank);
                }
                // no row: names kept from before would become a header line
                (None, _) if cols.iter().any(|c| self.names.contains_key(c)) => {
                    return Err(TableError::HeaderNamesUnrendered);
                }
                _ => {}
            }
        }
        let mut ch = Change::new(self);
        if on {
            let first = self.rows.live().next().cloned();
            if let Some(first) = first {
                for c in &cols {
                    let v = self.cell(&first, c).map(str::to_owned);
                    ch.name(self, c, v.as_deref());
                    let q = self.quote(&QuoteAt::Cell(first.clone(), c.clone()));
                    ch.quote(self, QuoteAt::Header(c.clone()), q);
                }
                ch.list(self, ROWS, ListOp::Delete { key: first })?;
            }
            ch.meta(self, "header", "on");
        } else if !self.header_renders() {
            // no header line is written: switching it off adds no line
            ch.meta(self, "header", "off");
        } else {
            let k = self.mint_row();
            ch.list(
                self,
                ROWS,
                ListOp::Insert {
                    key: k.clone(),
                    at: Anchor::Start,
                    value: Value::Null,
                },
            )?;
            for c in &cols {
                let v = self.name(c).map(str::to_owned);
                ch.cell(self, &k, c, v.as_deref());
                let q = self.quote(&QuoteAt::Header(c.clone()));
                ch.quote(self, QuoteAt::Cell(k.clone(), c.clone()), q);
                ch.name(self, c, None);
                ch.quote(self, QuoteAt::Header(c.clone()), None);
            }
            ch.meta(self, "header", "off");
        }
        Ok(ch.finish(self))
    }

    /// Re-identify this table (the grid as a file holds it: an outside save's base) onto
    /// `live`, a later state of the same doc, wherever `live` gave elements the file shows
    /// new identities (contract §2.9.1): a header switch (on → off makes the names a new row,
    /// off → on a row the names), and rows or columns deleted (or dropped) with others
    /// inserted in their place, with the same values or with edits since (a row or column
    /// deleted and inserted again, a version restore minting fresh keys for dropped rows, a
    /// header switched both ways). Returns the edits that do it on this table (applied to
    /// it): what it renders doesn't change, and every element it shows that `live` shows
    /// under another key takes that key, with its cells, hidden ones too, its name and its
    /// quote exceptions.
    ///
    /// Columns first, then rows. An element of ours `live` doesn't show (`gone`) is paired
    /// with an element `live` has that we never had (`fresh`), in order, by what it holds in
    /// rendered order (keys may differ on both axes): the same values; else at least half of
    /// them the same (an element edited since, the aligner's pairing rule, §2.9 step 3); else
    /// the one new element at its place. An element left unpaired keeps its key: genuinely
    /// deleted on the live side.
    pub(crate) fn reidentify_onto(&mut self, live: &Table) -> Result<Vec<Edit>, TableError> {
        let mut out = Vec::new();
        if self.header != live.header {
            match self.set_header(live.header) {
                Ok(e) => out.extend(e),
                // the switch would change what this table renders: keep its header
                Err(TableError::HeaderOnlyRowBlank | TableError::HeaderNamesUnrendered) => {}
                Err(e) => return Err(e),
            }
        }
        out.extend(self.reidentify_axis(live, COLS)?);
        out.extend(self.reidentify_axis(live, ROWS)?);
        Ok(out)
    }

    /// One axis of [`Table::reidentify_onto`].
    fn reidentify_axis(
        &mut self,
        live: &Table,
        root: &'static str,
    ) -> Result<Vec<Edit>, TableError> {
        let is_rows = root == ROWS;
        fn axis_in(t: &Table, rows: bool) -> &Axis {
            if rows {
                &t.rows
            } else {
                &t.cols
            }
        }
        let mine = |t| axis_in(t, is_rows);
        // what an element holds, in rendered order of the other axis (keys may differ)
        let held = |t: &Table, k: &Key| -> Vec<Option<String>> {
            if is_rows {
                t.live_cols()
                    .iter()
                    .map(|c| t.cell(k, c).map(str::to_owned))
                    .collect()
            } else {
                let mut v = vec![t.name(k).map(str::to_owned)];
                v.extend(
                    t.live_rows()
                        .iter()
                        .map(|r| t.cell(r, k).map(str::to_owned)),
                );
                v
            }
        };
        let gone: Vec<Key> = mine(self)
            .live()
            .filter(|k| !mine(live).is_live(k))
            .cloned()
            .collect();
        let fresh: Vec<Key> = mine(live)
            .live()
            .filter(|k| !mine(self).knows(k))
            .cloned()
            .collect();
        if gone.is_empty() || fresh.is_empty() {
            return Ok(Vec::new());
        }
        let fresh_held: Vec<(Key, Vec<Option<String>>, Option<usize>)> = fresh
            .iter()
            .map(|k| {
                let at = mine(live).live().position(|x| x == k);
                (k.clone(), held(live, k), at)
            })
            .collect();
        let mut pairs: Vec<(Key, Key)> = Vec::new();
        let mut used: HashSet<Key> = HashSet::new();
        for b in &gone {
            let want = held(self, b);
            if want.iter().all(Option::is_none) {
                continue;
            }
            let at = mine(self).live().position(|k| k == b);
            let free = || fresh_held.iter().filter(|(k, _, _)| !used.contains(k));
            let same = |v: &Vec<Option<String>>| {
                let n = want.len().max(v.len());
                let eq = want
                    .iter()
                    .zip(v)
                    .filter(|(a, b)| a == b && a.is_some())
                    .count();
                n > 0 && 2 * eq >= n
            };
            let pick = free()
                .find(|(_, v, _)| *v == want)
                .or_else(|| free().find(|(_, v, _)| same(v)))
                .or_else(|| {
                    let mut c = free().filter(|(_, _, x)| *x == at);
                    match (c.next(), c.next()) {
                        (Some(x), None) => Some(x),
                        _ => None,
                    }
                })
                .map(|(k, _, _)| k.clone());
            if let Some(l) = pick {
                used.insert(l.clone());
                pairs.push((b.clone(), l));
            }
        }
        let mut ch = Change::new(self);
        for (b, l) in pairs {
            ch.list(
                self,
                root,
                ListOp::Insert {
                    key: l.clone(),
                    at: Anchor::Before(b.clone()),
                    value: Value::Null,
                },
            )?;
            // every cell keyed by it, hidden ones too, and its quote exceptions
            let cells: Vec<((Key, Key), String)> = self
                .cells
                .iter()
                .filter(|((r, c), _)| if is_rows { *r == b } else { *c == b })
                .map(|(rc, v)| (rc.clone(), v.clone()))
                .collect();
            for ((r, c), v) in cells {
                let (r2, c2) = if is_rows {
                    (l.clone(), c.clone())
                } else {
                    (r.clone(), l.clone())
                };
                ch.cell(self, &r2, &c2, Some(&v));
                let q = self.quote(&QuoteAt::Cell(r.clone(), c.clone()));
                ch.quote(self, QuoteAt::Cell(r2, c2), q);
            }
            if !is_rows {
                let n = self.name(&b).map(str::to_owned);
                ch.name(self, &l, n.as_deref());
                let q = self.quote(&QuoteAt::Header(b.clone()));
                ch.quote(self, QuoteAt::Header(l.clone()), q);
            }
            ch.list(self, root, ListOp::Delete { key: b })?;
        }
        Ok(ch.finish(self))
    }

    /// `rows drop-deleted` (K-CSV-7): remove every deleted row and column, with every cell,
    /// name and quote exception keyed by one, from the live sheet ([`ListOp::Drop`], map
    /// deletes), so they stop counting toward the size limit. They can't be restored from
    /// the sheet afterwards; versions and change records still hold them (not erasure). The
    /// key counters are written as they stand, so a dropped key is never minted again. No
    /// edit when nothing is deleted.
    pub fn drop_deleted(&mut self) -> Result<Vec<Edit>, TableError> {
        let rows: Vec<Key> = self.rows.deleted().cloned().collect();
        let cols: Vec<Key> = self.cols.deleted().cloned().collect();
        if rows.is_empty() && cols.is_empty() {
            return Ok(Vec::new());
        }
        let gone_r: BTreeSet<&Key> = rows.iter().collect();
        let gone_c: BTreeSet<&Key> = cols.iter().collect();
        let mut ch = Change::new(self);
        let cells: Vec<(Key, Key)> = self
            .cells
            .keys()
            .filter(|(r, c)| gone_r.contains(r) || gone_c.contains(c))
            .cloned()
            .collect();
        for (r, c) in &cells {
            ch.cell(self, r, c, None);
        }
        let quotes: Vec<QuoteAt> = self
            .quoting
            .keys()
            .filter(|at| match at {
                QuoteAt::Header(c) => gone_c.contains(c),
                QuoteAt::Cell(r, c) => gone_r.contains(r) || gone_c.contains(c),
            })
            .cloned()
            .collect();
        for at in quotes {
            ch.quote(self, at, None);
        }
        for c in &cols {
            ch.name(self, c, None);
            ch.list(self, COLS, ListOp::Drop { key: c.clone() })?;
        }
        for r in &rows {
            ch.list(self, ROWS, ListOp::Drop { key: r.clone() })?;
        }
        let (nr, nc) = (self.next_row.to_string(), self.next_col.to_string());
        ch.meta(self, "next_row", &nr);
        ch.meta(self, "next_col", &nc);
        Ok(ch.finish(self))
    }
}

#[cfg(test)]
mod tests {
    use super::leading_number;

    #[test]
    fn leading_numbers() {
        assert_eq!(leading_number("12"), Some(12.0));
        assert_eq!(leading_number("-3.5kg"), Some(-3.5));
        assert_eq!(leading_number(".5"), Some(0.5));
        assert_eq!(leading_number("7."), Some(7.0));
        assert_eq!(leading_number("+"), None);
        assert_eq!(leading_number(""), None);
        assert_eq!(leading_number("x1"), None);
        assert_eq!(leading_number(" 1"), None);
    }
}
