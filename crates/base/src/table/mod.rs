//! Tables: the CSV contract's composition (§1.1) over the kit's structured kinds, as plain
//! data (A7). Engine-free and codec-free: it works on keys, cells and rendered records
//! (fields), never on engine IDs or file bytes, so the CSV pack, the grid editor (G7) and any
//! other table-shaped pack share it.
//!
//! **Six roots on three primitives** ([`ROOTS`]): `rows` and `cols` (MovableLists; an
//! element's key is the row or column key, its value unused), `cells` (a Map keyed
//! `"<row>\x1f<col>"`, [`cell_key`]), `names` (column key → header text), `quoting` (sparse
//! per-field exceptions to the file's quote policy, [`QuoteAt`]) and `meta` (`header`,
//! `shape`, and the key counters `next_row` / `next_col`).
//!
//! - [`Table`]: the table as readers see it ([`Table::from_keyed`] over
//!   [`crate::Doc::keyed_state`]), with a plain applier of the kit's ops
//!   ([`Table::apply`], the sequential semantics of [`crate::elem`]) and key minting.
//! - **Render rules** (contract §1.5): [`Table::render`] gives the records a codec writes
//!   (`rect`: a row with a present cell renders every column; `ragged`: through its last
//!   present cell; no present cell: a blank line); [`Table::line_map`] maps rendered lines to
//!   rows.
//! - **Grid ops** ([`ops`]): every grid verb as edits, built against the table and applied
//!   to it (cell set and clear, block set and clear, row and column insert, move, delete,
//!   restore, rename, sort as one forward chain, block moves, the header switch).
//! - [`hidden_cell_writes`]: the cells a change wrote into deleted rows or columns (kept,
//!   hidden, and reported: contract §1.6, engine answer E3).
//! - [`reconcile()`]: the aligner (A8): a new rendering (an outside save, an agent's
//!   `write --base` body) → minimal keyed grid ops.
//! - [`merge3()`]: the grid three-way merge of `write --base` (contract §3.4), with a
//!   conflict report.
//!
//! The kit's op rules (K-CSV-10): clearing a cell is a set to `""` (a map delete only where a
//! cell must become absent); multi-element moves are forward chains ([`crate::elem::chain`]);
//! keys are never reused, and a deleted row or column comes back by restore, never by
//! re-inserting its key. Values are checked here ([`check_value`]: no NUL, at most
//! [`MAX_VALUE_LEN`] bytes), the per-op check that is the pack's, not the kit Doc's (E10).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::ops::Range;

use crate::doc::Edit;
use crate::elem::{
    Anchor, Applied, ElemState, Key, KeyedRoot, KeyedState, ListOp, MapOp, MapOpKind, Value,
};
use crate::schema::Kind;

mod apply;
pub mod merge;
pub mod ops;
pub mod reconcile;
pub mod verbs;

#[cfg(test)]
mod tests;

pub use merge::{
    merge3, merge3_outside, merge3_outside_text, merge3_text, Conflict, ConflictRule, ElemAt, Held,
    Merge, Region,
};
pub use ops::SortKey;
pub use reconcile::{reconcile, Reconciled};

/// The rows: a MovableList of row keys.
pub const ROWS: &str = "rows";
/// The columns: a MovableList of column keys.
pub const COLS: &str = "cols";
/// The cells: a Map, [`cell_key`] → the cell's value.
pub const CELLS: &str = "cells";
/// The header text: a Map, column key → name.
pub const NAMES: &str = "names";
/// Quote exceptions: a Map, [`QuoteAt::map_key`] → `q` (quoted) or `p` (plain).
pub const QUOTING: &str = "quoting";
/// Settings that are content: `header` (`on`/`off`), `shape` (`rect`/`ragged`), `next_row`
/// and `next_col` (the next key numbers to mint).
pub const META: &str = "meta";

/// A table pack's roots, in this order: `Schema { name, roots: table::ROOTS }`.
pub const ROOTS: &[(&str, Kind)] = &[
    (ROWS, Kind::MovableList),
    (COLS, Kind::MovableList),
    (CELLS, Kind::Map),
    (NAMES, Kind::Map),
    (QUOTING, Kind::Map),
    (META, Kind::Map),
];

/// Between a cell key's row and column keys.
pub const CELL_SEP: char = '\u{1f}';
/// In front of a column key: that column's header field, in [`QUOTING`].
pub const HEADER_MARK: char = '\u{1e}';
/// True when `schema` is a table pack's: exactly [`ROOTS`], in order.
pub fn is_table(schema: &crate::schema::Schema) -> bool {
    schema.roots == ROOTS
}

/// The longest cell value or name, in bytes, by default (contract §1.8): a host changes it
/// with `Config::grid_max_value_len` (raise or lower), which the session applies to every
/// edit and to the tables of the views it hands out ([`Table::max_value`]).
pub const DEFAULT_MAX_VALUE_LEN: usize = 64 * 1024;

/// The default value limit, under its earlier name ([`DEFAULT_MAX_VALUE_LEN`]).
pub const MAX_VALUE_LEN: usize = DEFAULT_MAX_VALUE_LEN;

/// A cell's map key: `"<row>\x1f<col>"`.
pub fn cell_key(row: &Key, col: &Key) -> String {
    let mut s = String::with_capacity(row.as_str().len() + col.as_str().len() + 1);
    s.push_str(row.as_str());
    s.push(CELL_SEP);
    s.push_str(col.as_str());
    s
}

/// The row and column keys of a cell's map key; `None` if it isn't one.
pub fn parse_cell_key(s: &str) -> Option<(Key, Key)> {
    let (r, c) = s.split_once(CELL_SEP)?;
    Some((Key::new(r).ok()?, Key::new(c).ok()?))
}

/// The per-op value check (the pack's, E10): no NUL, at most [`DEFAULT_MAX_VALUE_LEN`]
/// bytes. A table checks against its own limit ([`check_value_max`], [`Table::max_value`]).
pub fn check_value(v: &str) -> Result<(), ValueError> {
    check_value_max(v, DEFAULT_MAX_VALUE_LEN)
}

/// [`check_value`] with a limit of `max` bytes.
pub fn check_value_max(v: &str, max: usize) -> Result<(), ValueError> {
    if v.len() > max {
        return Err(ValueError::TooLong { len: v.len(), max });
    }
    match v.bytes().position(|b| b == 0) {
        Some(at) => Err(ValueError::Nul { at }),
        None => Ok(()),
    }
}

/// Why a value is refused (exit 2 at a verb).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    #[error("a value may not hold NUL (byte {at})")]
    Nul { at: usize },
    #[error("a value is {len} bytes; the limit is {max}")]
    TooLong { len: usize, max: usize },
}

/// How rows render (contract §1.5). Fixed at import; changed only by a verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Shape {
    /// A row with a present cell renders every column (absent cells as empty fields).
    #[default]
    Rect,
    /// A row renders through its last present cell.
    Ragged,
}

impl Shape {
    /// `rect` when every non-blank record has the same number of fields, else `ragged` (what
    /// an import decides).
    pub fn detect(records: &[Record]) -> Shape {
        let mut widths = records.iter().filter(|r| !r.is_empty()).map(Vec::len);
        match widths.next() {
            Some(w) if widths.any(|x| x != w) => Shape::Ragged,
            _ => Shape::Rect,
        }
    }

    fn spelling(self) -> &'static str {
        match self {
            Shape::Rect => "rect",
            Shape::Ragged => "ragged",
        }
    }
}

/// A field's exception to the file's quote policy (the codec decides what the policy is).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Quote {
    /// Written quoted although the policy wouldn't quote it.
    Quoted,
    /// Written plain although the policy would quote it.
    Plain,
}

impl Quote {
    fn spelling(self) -> &'static str {
        match self {
            Quote::Quoted => "q",
            Quote::Plain => "p",
        }
    }

    fn parse(s: &str) -> Option<Quote> {
        match s {
            "q" => Some(Quote::Quoted),
            "p" => Some(Quote::Plain),
            _ => None,
        }
    }
}

/// Where a [`Quote`] exception is: a cell, or a column's header field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QuoteAt {
    Header(Key),
    Cell(Key, Key),
}

impl QuoteAt {
    /// Its key in [`QUOTING`]: the cell's key, or `"\x1e<col>"`.
    pub fn map_key(&self) -> String {
        match self {
            QuoteAt::Header(c) => format!("{HEADER_MARK}{c}"),
            QuoteAt::Cell(r, c) => cell_key(r, c),
        }
    }

    pub fn parse(s: &str) -> Option<QuoteAt> {
        match s.strip_prefix(HEADER_MARK) {
            Some(c) => Some(QuoteAt::Header(Key::new(c).ok()?)),
            None => parse_cell_key(s).map(|(r, c)| QuoteAt::Cell(r, c)),
        }
    }

    pub(crate) fn renamed(
        &self,
        rows: &dyn Fn(&Key) -> Key,
        cols: &dyn Fn(&Key) -> Key,
    ) -> QuoteAt {
        match self {
            QuoteAt::Header(c) => QuoteAt::Header(cols(c)),
            QuoteAt::Cell(r, c) => QuoteAt::Cell(rows(r), cols(c)),
        }
    }
}

/// One rendered field: its value and its quote exception, if any. What a codec reads from a
/// file and writes for a cell; the codec owns the bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Field {
    pub value: String,
    pub quote: Option<Quote>,
}

impl Field {
    pub fn new(value: impl Into<String>) -> Self {
        Field {
            value: value.into(),
            quote: None,
        }
    }
}

/// One rendered record: its fields. An empty record is a blank line.
pub type Record = Vec<Field>;

/// Records from plain strings (no quote exceptions): each line a record, fields as given.
pub fn records(rows: &[&[&str]]) -> Vec<Record> {
    rows.iter()
        .map(|r| r.iter().map(|v| Field::new(*v)).collect())
        .collect()
}

/// One MovableList root of a table (rows or columns): its elements in place order, live and
/// deleted, and which are deleted.
///
/// A table read from a doc's keyed state ([`Table::from_keyed`]) knows where deleted
/// elements are (the state's `places`), so a [`Table::apply`] of a restore shows them where
/// the doc does. One read from a state without places (a version, a hand-built state) puts
/// them after the live ones. Equality compares what readers see: the live order and the
/// deleted keys.
#[derive(Debug, Clone, Default)]
pub struct Axis {
    order: Vec<Key>,
    dead: BTreeSet<Key>,
    known: HashSet<Key>,
}

impl PartialEq for Axis {
    fn eq(&self, other: &Self) -> bool {
        self.dead == other.dead && self.live().eq(other.live())
    }
}

impl Eq for Axis {}

impl Axis {
    /// Live keys, in order.
    pub fn from_live(keys: impl IntoIterator<Item = Key>) -> Self {
        let mut a = Axis::default();
        for k in keys {
            a.push(k, false);
        }
        a
    }

    fn push(&mut self, k: Key, dead: bool) {
        if self.known.insert(k.clone()) {
            if dead {
                self.dead.insert(k.clone());
            }
            self.order.push(k);
        }
    }

    /// The live keys, in order.
    pub fn live(&self) -> impl DoubleEndedIterator<Item = &Key> + '_ {
        self.order.iter().filter(|k| !self.dead.contains(*k))
    }

    pub fn live_keys(&self) -> Vec<Key> {
        self.live().cloned().collect()
    }

    pub fn live_len(&self) -> usize {
        self.order.len() - self.dead.len()
    }

    /// The deleted keys.
    pub fn deleted(&self) -> impl Iterator<Item = &Key> + '_ {
        self.dead.iter()
    }

    /// Ever inserted (live or deleted).
    pub fn knows(&self, k: &Key) -> bool {
        self.known.contains(k)
    }

    pub fn is_live(&self, k: &Key) -> bool {
        self.known.contains(k) && !self.dead.contains(k)
    }

    /// Where `k` is: live at an index, deleted, or never inserted. O(n).
    pub fn state(&self, k: &Key) -> ElemState {
        if !self.known.contains(k) {
            ElemState::Unknown
        } else if self.dead.contains(k) {
            ElemState::Deleted
        } else {
            ElemState::Live(self.live().position(|x| x == k).expect("a live key"))
        }
    }

    /// The live key at `index`.
    pub fn at(&self, index: usize) -> Option<&Key> {
        self.live().nth(index)
    }

    fn raw(&self, k: &Key) -> Option<usize> {
        self.order.iter().position(|x| x == k)
    }

    fn rename(&mut self, f: &dyn Fn(&Key) -> Key) {
        self.order = self.order.iter().map(f).collect();
        self.dead = self.dead.iter().map(f).collect();
        self.known = self.known.iter().map(f).collect();
    }
}

/// A table as plain data: what its six roots hold, by key (the view a verb, the aligner and
/// the merge work on). Build one with [`Table::from_keyed`] or [`Table::import`]; keep it in
/// step with the doc by applying the same edits ([`Table::apply`]; the [`ops`] builders do
/// it for you).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub rows: Axis,
    pub cols: Axis,
    /// Present cells, live or hidden.
    pub cells: BTreeMap<(Key, Key), String>,
    pub names: BTreeMap<Key, String>,
    pub quoting: BTreeMap<QuoteAt, Quote>,
    /// The file's first record is the header (`meta.header = on`).
    pub header: bool,
    pub shape: Shape,
    /// The next row key number to mint (`meta.next_row`); at least 1.
    pub next_row: u64,
    pub next_col: u64,
    /// The longest value its ops and the aligner accept, in bytes (not content: never in
    /// the doc). [`DEFAULT_MAX_VALUE_LEN`] unless set ([`Table::with_max_value`]; a
    /// session's views carry the host's `Config::grid_max_value_len`).
    max_value: usize,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            rows: Axis::default(),
            cols: Axis::default(),
            cells: BTreeMap::new(),
            names: BTreeMap::new(),
            quoting: BTreeMap::new(),
            header: false,
            shape: Shape::default(),
            next_row: 0,
            next_col: 0,
            max_value: DEFAULT_MAX_VALUE_LEN,
        }
    }
}

/// Why a table read, op or builder is refused (nothing is applied).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    #[error("{root}: {key} is unknown")]
    Unknown { root: &'static str, key: Key },
    #[error("{root}: {key} is deleted")]
    Deleted { root: &'static str, key: Key },
    #[error("{root}: {key} already exists (keys are never reused; restore it)")]
    KeyExists { root: &'static str, key: Key },
    #[error("{root}: {key} can't be placed next to itself")]
    SelfAnchor { root: &'static str, key: Key },
    #[error("{0}")]
    Value(#[from] ValueError),
    #[error("{values} values for {cols} columns (add a column first)")]
    TooManyValues { values: usize, cols: usize },
    #[error("the header is off: columns have no names")]
    HeaderOff,
    /// `header on` where the only live row is blank: it would become a blank header line
    /// with nothing after it, which isn't written ([`Table::header_renders`]), so the file
    /// would change. `MPC-E807` at the verb; exit 2; nothing applied.
    #[error("header on: the only row is a blank line, and a blank header line with nothing after it isn't written (the file would change); add a non-blank row first")]
    HeaderOnlyRowBlank,
    /// `header on` with no live row where live columns hold names (kept unrendered while
    /// the header was off): they would appear as a header line, so the file would change.
    /// Refused, nothing applied: the kit never clears someone's names as a side effect.
    /// `MPC-E806` at the verb; exit 2.
    #[error(
        "the columns have names that would appear as a header line; clear them or add a row first"
    )]
    HeaderNamesUnrendered,
    #[error("{0}")]
    Usage(String),
    #[error("not a table: {0}")]
    Malformed(String),
}

fn malformed(s: impl Into<String>) -> TableError {
    TableError::Malformed(s.into())
}

fn str_value(root: &str, key: &str, v: &Value) -> Result<String, TableError> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| malformed(format!("{root}[{key:?}] holds no string")))
}

/// The root of a [`TableError`]: `rows` or `cols`.
fn axis_root(root: &str) -> &'static str {
    if root == ROWS {
        ROWS
    } else {
        COLS
    }
}

impl Table {
    /// An empty table.
    pub fn new(header: bool, shape: Shape) -> Self {
        Table {
            header,
            shape,
            next_row: 1,
            next_col: 1,
            ..Table::default()
        }
    }

    /// The same table, with ops and the aligner refusing values over `max` bytes
    /// ([`ValueError::TooLong`]).
    pub fn with_max_value(mut self, max: usize) -> Self {
        self.max_value = max;
        self
    }

    /// The longest value this table's ops and the aligner accept, in bytes.
    pub fn max_value(&self) -> usize {
        self.max_value
    }

    /// [`check_value_max`] under this table's limit.
    pub(crate) fn check(&self, v: &str) -> Result<(), ValueError> {
        check_value_max(v, self.max_value)
    }

    /// The table a doc holds ([`crate::Doc::keyed_state`], or any engine's keyed state).
    /// Every root of [`ROOTS`] must be there. Missing meta keys take their defaults (header
    /// off, `rect`, counters past the highest key number in use).
    pub fn from_keyed(state: &KeyedState) -> Result<Self, TableError> {
        let root = |name: &str| {
            state
                .roots
                .iter()
                .find(|(r, _)| r == name)
                .map(|(_, x)| x)
                .ok_or_else(|| malformed(format!("no root {name}")))
        };
        let axis = |name: &str| -> Result<Axis, TableError> {
            match root(name)? {
                KeyedRoot::List {
                    live,
                    hidden,
                    places,
                } => {
                    let mut a = Axis::default();
                    // places first (where deleted elements sit), when they are the elements
                    let exact = places.len() == live.len() + hidden.len()
                        && places
                            .iter()
                            .filter(|k| !hidden.contains_key(*k))
                            .eq(live.iter().map(|(k, _)| k));
                    if exact {
                        for k in places {
                            a.push(k.clone(), hidden.contains_key(k));
                        }
                    }
                    for (k, _) in live {
                        a.push(k.clone(), false);
                    }
                    for k in hidden.keys() {
                        a.push(k.clone(), true);
                    }
                    Ok(a)
                }
                _ => Err(malformed(format!("{name} is not a list"))),
            }
        };
        let map = |name: &str| -> Result<&BTreeMap<String, Value>, TableError> {
            match root(name)? {
                KeyedRoot::Map(m) => Ok(m),
                _ => Err(malformed(format!("{name} is not a map"))),
            }
        };
        let mut t = Table::new(false, Shape::Rect);
        t.rows = axis(ROWS)?;
        t.cols = axis(COLS)?;
        for (k, v) in map(CELLS)? {
            let at = parse_cell_key(k).ok_or_else(|| malformed(format!("cell key {k:?}")))?;
            t.cells.insert(at, str_value(CELLS, k, v)?);
        }
        for (k, v) in map(NAMES)? {
            let c = Key::new(k.as_str()).map_err(|_| malformed(format!("name key {k:?}")))?;
            t.names.insert(c, str_value(NAMES, k, v)?);
        }
        for (k, v) in map(QUOTING)? {
            let at = QuoteAt::parse(k).ok_or_else(|| malformed(format!("quoting key {k:?}")))?;
            let q = Quote::parse(&str_value(QUOTING, k, v)?)
                .ok_or_else(|| malformed(format!("quoting[{k:?}] = {v:?}")))?;
            t.quoting.insert(at, q);
        }
        for (k, v) in map(META)? {
            t.set_meta(k, Some(&str_value(META, k, v)?))?;
        }
        t.next_row = t.next_row.max(next_free(&t.rows, 'r'));
        t.next_col = t.next_col.max(next_free(&t.cols, 'c'));
        Ok(t)
    }

    /// A table from decoded records, and the edits that make it in an empty doc (one change:
    /// the import). Keys `r1…rN` and `c1…cM` in file order (M: the widest record); every
    /// field is a present cell; with `header`, the first record gives the names, unless it
    /// is a lone blank line: that is no header (the codec's rule, [`Table::header_renders`]),
    /// and the table is header off with one blank row.
    /// Values are checked under the default limit ([`DEFAULT_MAX_VALUE_LEN`]);
    /// [`Table::import_with_max_value`] takes another.
    pub fn import(
        records: &[Record],
        header: bool,
        shape: Shape,
    ) -> Result<(Table, Vec<Edit>), TableError> {
        Table::import_with_max_value(records, header, shape, DEFAULT_MAX_VALUE_LEN)
    }

    /// [`Table::import`] with values of at most `max` bytes (a host's
    /// `Config::grid_max_value_len`); the table keeps that limit.
    pub fn import_with_max_value(
        records: &[Record],
        header: bool,
        shape: Shape,
        max: usize,
    ) -> Result<(Table, Vec<Edit>), TableError> {
        let header = header && !lone_blank(records);
        let mut t = Table::new(header, shape).with_max_value(max);
        let mut e = ops::Change::new(&t);
        e.meta(&mut t, "header", if header { "on" } else { "off" });
        e.meta(&mut t, "shape", shape.spelling());
        let width = records.iter().map(Vec::len).max().unwrap_or(0);
        let mut at = Anchor::Start;
        let mut cols = Vec::with_capacity(width);
        for _ in 0..width {
            let c = t.mint_col();
            e.list(
                &mut t,
                COLS,
                ListOp::Insert {
                    key: c.clone(),
                    at: at.clone(),
                    value: Value::Null,
                },
            )?;
            at = Anchor::After(c.clone());
            cols.push(c);
        }
        let (head, body) = match (header, records.split_first()) {
            (true, Some((h, rest))) => (Some(h), rest),
            _ => (None, records),
        };
        if let Some(h) = head {
            for (f, c) in h.iter().zip(&cols) {
                t.check(&f.value)?;
                e.name(&mut t, c, Some(&f.value));
                e.quote(&mut t, QuoteAt::Header(c.clone()), f.quote);
            }
        }
        let mut at = Anchor::Start;
        for rec in body {
            let r = t.mint_row();
            e.list(
                &mut t,
                ROWS,
                ListOp::Insert {
                    key: r.clone(),
                    at: at.clone(),
                    value: Value::Null,
                },
            )?;
            for (f, c) in rec.iter().zip(&cols) {
                t.check(&f.value)?;
                e.cell(&mut t, &r, c, Some(&f.value));
                e.quote(&mut t, QuoteAt::Cell(r.clone(), c.clone()), f.quote);
            }
            at = Anchor::After(r);
        }
        let edits = e.finish(&mut t);
        Ok((t, edits))
    }

    /// The live rows' keys, in order.
    pub fn live_rows(&self) -> Vec<Key> {
        self.rows.live_keys()
    }

    pub fn live_cols(&self) -> Vec<Key> {
        self.cols.live_keys()
    }

    /// A cell's value (`None`: absent).
    pub fn cell(&self, row: &Key, col: &Key) -> Option<&str> {
        self.cells
            .get(&(row.clone(), col.clone()))
            .map(String::as_str)
    }

    /// A column's header text (`None`: absent).
    pub fn name(&self, col: &Key) -> Option<&str> {
        self.names.get(col).map(String::as_str)
    }

    /// The live columns whose name is exactly `name` (header on).
    pub fn cols_named(&self, name: &str) -> Vec<Key> {
        self.cols
            .live()
            .filter(|c| self.name(c) == Some(name))
            .cloned()
            .collect()
    }

    fn quote(&self, at: &QuoteAt) -> Option<Quote> {
        self.quoting.get(at).copied()
    }

    /// The records a codec writes, in file order: the header (when on) and each live row
    /// (contract §1.5). `rect`: a row with at least one present cell renders every live
    /// column (an absent cell as an empty field); `ragged`: through its last present cell;
    /// a row with no present cell renders as a blank line (an empty record). The header
    /// renders by the same rule over the names, when it renders at all
    /// ([`Table::header_renders`]).
    pub fn render(&self) -> Vec<Record> {
        let cols = self.live_cols();
        let mut out = Vec::with_capacity(self.rows.live_len() + 1);
        if self.header_renders() {
            out.push(self.render_line(&cols, |c| {
                (self.name(c), self.quote(&QuoteAt::Header(c.clone())))
            }));
        }
        for r in self.rows.live() {
            out.push(self.render_line(&cols, |c| {
                (
                    self.cell(r, c),
                    self.quote(&QuoteAt::Cell(r.clone(), c.clone())),
                )
            }));
        }
        out
    }

    /// Row `r`'s record, as [`Table::render`] gives it, with cell `c` taken as `v` (`None`:
    /// absent): a row renders on its own, so this is the row's bytes with only that cell set.
    pub(crate) fn render_row_with(&self, r: &Key, c: &Key, v: Option<&str>) -> Record {
        let cols = self.live_cols();
        self.render_line(&cols, |x| {
            (
                if x == c { v } else { self.cell(r, x) },
                self.quote(&QuoteAt::Cell(r.clone(), x.clone())),
            )
        })
    }

    /// Whether the header line is written (the codec's rule): the header is on, and the
    /// table has a live row or a name on a live column. A header-on table with neither is
    /// empty: no header line (a blank header line with nothing after it would read back as
    /// a blank row, not a header), so its file is empty.
    pub fn header_renders(&self) -> bool {
        self.header
            && (self.rows.live_len() > 0 || self.cols.live().any(|c| self.names.contains_key(c)))
    }

    /// One record over `cols` by the shape's rule; `get` gives each column's value (`None`:
    /// absent) and quote exception.
    fn render_line<'a>(
        &self,
        cols: &[Key],
        get: impl Fn(&Key) -> (Option<&'a str>, Option<Quote>),
    ) -> Record {
        let vals: Vec<_> = cols.iter().map(get).collect();
        let width = match self.shape {
            Shape::Rect if vals.iter().any(|(v, _)| v.is_some()) => vals.len(),
            Shape::Rect => 0,
            Shape::Ragged => vals
                .iter()
                .rposition(|(v, _)| v.is_some())
                .map_or(0, |i| i + 1),
        };
        vals[..width]
            .iter()
            .map(|(v, q)| match v {
                Some(v) => Field {
                    value: (*v).to_owned(),
                    quote: *q,
                },
                None => Field::default(),
            })
            .collect()
    }

    /// Which row each rendered line belongs to (`L42` addressing, blame lines, views): one
    /// entry per line of the rendered records, the header's first. A record spans one line
    /// plus one per line break inside its values (LF, CR or CRLF, kept verbatim in quoted
    /// fields). A codec preamble (`sep=;`) is the codec's to add in front, as
    /// [`LineOwner::Preamble`].
    pub fn line_map(&self) -> LineMap {
        let mut lines = Vec::new();
        let span = |rec: &Record| 1 + rec.iter().map(|f| line_breaks(&f.value)).sum::<usize>();
        let rendered = self.render();
        let mut it = rendered.iter();
        if rendered.len() > self.rows.live_len() {
            let h = it.next().expect("the header record");
            lines.extend(std::iter::repeat(LineOwner::Header).take(span(h)));
        }
        for (rec, r) in it.zip(self.rows.live()) {
            lines.extend(std::iter::repeat(LineOwner::Row(r.clone())).take(span(rec)));
        }
        LineMap { lines }
    }

    /// The cells a change wrote that are hidden here (their row or column is deleted): a
    /// write that landed in a deleted row (contract §1.6, E3; [`hidden_cell_writes`]).
    pub fn hidden_cell_writes<W: Clone>(&self, applied: &[Applied<W>]) -> Vec<HiddenWrite<W>> {
        hidden_cell_writes(applied, |root, k| {
            if root == ROWS {
                self.rows.is_live(k)
            } else {
                self.cols.is_live(k)
            }
        })
    }

    /// A new row key (`r<n>`), never used in this table; advances [`Table::next_row`].
    pub fn mint_row(&mut self) -> Key {
        mint(&self.rows, &mut self.next_row, 'r')
    }

    pub fn mint_col(&mut self) -> Key {
        mint(&self.cols, &mut self.next_col, 'c')
    }

    fn set_meta(&mut self, k: &str, v: Option<&str>) -> Result<(), TableError> {
        let bad = || malformed(format!("meta {k} = {v:?}"));
        match (k, v) {
            ("header", Some("on")) => self.header = true,
            ("header", Some("off") | None) => self.header = false,
            ("shape", Some("rect") | None) => self.shape = Shape::Rect,
            ("shape", Some("ragged")) => self.shape = Shape::Ragged,
            ("next_row" | "next_col", Some(n)) => {
                let n: u64 = n.parse().map_err(|_| bad())?;
                if k == "next_row" {
                    self.next_row = n.max(1);
                } else {
                    self.next_col = n.max(1);
                }
            }
            ("next_row" | "next_col", None) => {}
            ("header" | "shape", _) => return Err(bad()),
            // other meta keys are a later version's: kept by the doc, not read here
            _ => {}
        }
        Ok(())
    }

    /// Every key renamed by `rows` / `cols` (the merge's re-keying).
    pub(crate) fn renamed(&self, rows: &dyn Fn(&Key) -> Key, cols: &dyn Fn(&Key) -> Key) -> Table {
        let mut t = self.clone();
        t.rows.rename(rows);
        t.cols.rename(cols);
        t.cells = self
            .cells
            .iter()
            .map(|((r, c), v)| ((rows(r), cols(c)), v.clone()))
            .collect();
        t.names = self
            .names
            .iter()
            .map(|(c, v)| (cols(c), v.clone()))
            .collect();
        t.quoting = self
            .quoting
            .iter()
            .map(|(at, q)| (at.renamed(rows, cols), *q))
            .collect();
        t
    }
}

/// The lowest number `n` such that no key `<prefix><m>` or `<prefix><m>.<tag>` with `m >= n`
/// is in `axis` (so minting from it never reuses a key, whatever the counter says).
fn next_free(axis: &Axis, prefix: char) -> u64 {
    axis.known
        .iter()
        .filter_map(|k| {
            let s = k.as_str().strip_prefix(prefix)?;
            let n = s.split('.').next()?;
            n.parse::<u64>().ok()
        })
        .max()
        .map_or(1, |m| m.saturating_add(1))
}

fn mint(axis: &Axis, next: &mut u64, prefix: char) -> Key {
    loop {
        let k = Key::new(format!("{prefix}{}", *next)).expect("a minted key");
        *next = next.saturating_add(1);
        if !axis.knows(&k) {
            return k;
        }
    }
}

/// One blank record and nothing else: a file that is one blank line. It is never a header.
pub(crate) fn lone_blank(records: &[Record]) -> bool {
    matches!(records, [only] if only.is_empty())
}

fn line_breaks(s: &str) -> usize {
    let b = s.as_bytes();
    let mut n = 0;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\n' => n += 1,
            b'\r' if b.get(i + 1) != Some(&b'\n') => n += 1,
            _ => {}
        }
    }
    n
}

/// Who a rendered line belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LineOwner {
    /// A line the codec writes of its own, in front of the records (CSV's `sep=;`): no row
    /// and no header; a pack's [`crate::session::Pack::line_map`] adds it. The kit's own
    /// line map has none.
    Preamble,
    Header,
    Row(Key),
}

/// Rendered line → owner ([`Table::line_map`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LineMap {
    /// One per rendered line, line 1 first.
    pub lines: Vec<LineOwner>,
}

impl LineMap {
    /// The owner of line `n` (1-based, as `L42`).
    pub fn owner(&self, n: usize) -> Option<&LineOwner> {
        n.checked_sub(1).and_then(|i| self.lines.get(i))
    }

    /// The row line `n` belongs to (`None`: the header, a preamble line, or past the end).
    pub fn row_at(&self, n: usize) -> Option<&Key> {
        match self.owner(n)? {
            LineOwner::Row(k) => Some(k),
            LineOwner::Header | LineOwner::Preamble => None,
        }
    }

    /// The lines (1-based, end exclusive) row `row` renders on.
    pub fn lines_of(&self, row: &Key) -> Option<Range<usize>> {
        let is = |o: &LineOwner| matches!(o, LineOwner::Row(k) if k == row);
        let start = self.lines.iter().position(is)?;
        let len = self.lines[start..].iter().take_while(|o| is(o)).count();
        Some(start + 1..start + 1 + len)
    }
}

/// A cell write that landed in a deleted row or column ([`hidden_cell_writes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenWrite<W> {
    pub row: Key,
    pub col: Key,
    /// The write (as the stream names it).
    pub write: W,
    /// The row is deleted (else only the column is).
    pub row_hidden: bool,
    pub col_hidden: bool,
}

impl<W> HiddenWrite<W> {
    /// The cell's map key.
    pub fn cell_key(&self) -> String {
        cell_key(&self.row, &self.col)
    }
}

/// The cells `applied` sets (in [`CELLS`]) whose row or column is not live now, by `live(root,
/// key)` (`root` is [`ROWS`] or [`COLS`]): a cell written into a deleted row, concurrently or
/// by a stale writer, is kept, hidden, and reported to the writer (contract §1.6, §4.3; engine
/// answer E3). Map keys that aren't cell keys are skipped. In stream order.
pub fn hidden_cell_writes<W: Clone>(
    applied: &[Applied<W>],
    mut live: impl FnMut(&str, &Key) -> bool,
) -> Vec<HiddenWrite<W>> {
    let mut out = Vec::new();
    for a in applied {
        let Applied::Map { root, ops } = a else {
            continue;
        };
        if root != CELLS {
            continue;
        }
        for o in ops.iter().filter(|o| o.kind == MapOpKind::Set) {
            let Some((row, col)) = parse_cell_key(&o.key) else {
                continue;
            };
            let row_hidden = !live(ROWS, &row);
            let col_hidden = !live(COLS, &col);
            if row_hidden || col_hidden {
                out.push(HiddenWrite {
                    row,
                    col,
                    write: o.write.clone(),
                    row_hidden,
                    col_hidden,
                });
            }
        }
    }
    out
}

impl fmt::Display for QuoteAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuoteAt::Header(c) => write!(f, "header/{c}"),
            QuoteAt::Cell(r, c) => write!(f, "{r}/{c}"),
        }
    }
}

/// A map op setting `key` to `v`, or deleting it (`None`).
fn map_op(key: String, v: Option<&str>) -> MapOp {
    match v {
        Some(v) => MapOp::Set {
            key,
            value: Value::str(v),
        },
        None => MapOp::Delete { key },
    }
}
