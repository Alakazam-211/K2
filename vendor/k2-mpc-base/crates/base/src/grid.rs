//! A live grid (a sheet of rows, columns and cells), generically: what a view needs to draw one
//! with no pack code, and the places presence and claims name in it.
//!
//! - **The grid slot** ([`GridSlot`], in a pack's [`crate::view_index::ViewSchema::grid`]):
//!   which roots hold the rows, columns and cells, and the **numbering rule**. Rows and columns
//!   are keyed only where something stands; each keyed element's `gap` field says how many
//!   blank ones come before it. Within one container, in list order,
//!   `number = previous number + gap + 1` (the first's `gap + first`). So a row insert is one
//!   list op and at most one gap op, and every row below renumbers with no op. **A number is never read out of a key or an
//!   address.**
//! - **Corners** ([`Corner`]): a cell named by the last keyed row and column at or before it,
//!   plus offsets, so a blank cell has a name and the name follows the content. A corner must
//!   be in **normal form**: no keyed row (column) of its container lies strictly between its
//!   row (column) and the cell, so one cell has one spelling and two corners compare by list
//!   order. A range ([`CellRange`]) is two corners; a connection's [`Cells`] is a caret and up
//!   to [`CELLS_RANGES_MAX`] ranges on one container.
//! - **Numbers for one container** ([`GridNumbers`]): its keyed rows and columns with their
//!   numbers, read by the kit from the live doc; corners are checked and placed through it.
//! - **What a verb wrote** ([`CellsReport`]): a grid verb hands it to the kit with its ops, and
//!   the kit moves the acting agent's cells to it.
//! - **A change frame's derived values, typed** ([`FrameDerived`]): what a grid pack puts in a
//!   keyed watch's change frame (`a1` text, merges, style hints, the written rectangle, the
//!   inserted or deleted bands), built through types rather than free JSON.

use std::collections::BTreeMap;

use crate::elem::Key;
use crate::errors::Value as Json;

/// The most ranges one [`Cells`] holds; more is refused
/// ([`crate::session::SessionError::SelectionTooLarge`]).
pub const CELLS_RANGES_MAX: usize = 16;

/// How a view labels columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColLabel {
    /// 1 = A, 26 = Z, 27 = AA, 16,384 = XFD.
    Letters,
    /// 1, 2, 3...
    Numbers,
}

impl ColLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            ColLabel::Letters => "letters",
            ColLabel::Numbers => "numbers",
        }
    }
}

/// One axis's numbering: the element's field that holds how many blank lines come before it
/// (missing or not a number: 0), and the first line's number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numbering {
    pub gap_field: &'static str,
    pub first: u64,
}

/// Where a cell's value parts are, when a cell holds JSON (a register `{"t": .., "v": ..}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellParts {
    /// `"json"`.
    pub encoding: &'static str,
    pub ty: &'static str,
    pub value: &'static str,
    pub formula: Option<&'static str>,
    pub formula_kind: Option<&'static str>,
    pub style: Option<&'static str>,
    /// The type words and what each means.
    pub types: &'static [(&'static str, &'static str)],
}

/// How a cell finds its style hint: its `field` (`default` when absent), looked up in the
/// container's derived `table`, whose entries have `parts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleHintSpec {
    pub field: &'static str,
    pub default: &'static str,
    pub table: &'static str,
    pub parts: &'static [&'static str],
}

/// The grid slot of a view schema: everything a view needs to draw a sparse grid with no pack
/// code. Declared as a `const`, starting from [`GridSlot::NONE`].
///
/// The rows are the schema's units (a row's container is found as a unit's is); the columns are
/// another MovableList root whose container is named by `col_container_field` in `col_fields`,
/// matched the same way. `cells` is the rows' composite map: a field starting with
/// `row_fields_start` is a row field, any other a cell, whose column's address is
/// `col_of_cell_field` with `{container}` and `{field}` filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSlot {
    pub rows: &'static str,
    pub cols: &'static str,
    pub cells: &'static str,
    pub col_fields: &'static str,
    pub col_container_field: &'static str,
    pub row_fields_start: &'static str,
    pub col_of_cell_field: &'static str,
    pub row_numbering: Numbering,
    pub col_numbering: Numbering,
    pub col_label: ColLabel,
    /// The highest row and column number.
    pub max_rows: u64,
    pub max_cols: u64,
    pub cell: Option<CellParts>,
    pub row_hidden: Option<&'static str>,
    pub row_height: Option<&'static str>,
    /// What a row height counts in (`"pt"`).
    pub height_unit: &'static str,
    pub col_hidden: Option<&'static str>,
    pub col_width: Option<&'static str>,
    pub width_unit: &'static str,
    /// How to read a peer's viewport: `"cells"` (x, y the first column and row numbers on the
    /// focus container, w and h their counts).
    pub viewport: &'static str,
    pub style_hint: Option<StyleHintSpec>,
    /// The pack's derived values that are maps by cell field (`a1`, a formula's text): a
    /// snapshot page of the cells carries each cell's own beside its entry
    /// ([`crate::session::Pack::page_derived`]).
    pub cell_derived: &'static [&'static str],
}

impl GridSlot {
    /// Nothing declared.
    pub const NONE: GridSlot = GridSlot {
        rows: "",
        cols: "",
        cells: "",
        col_fields: "",
        col_container_field: "",
        row_fields_start: "",
        col_of_cell_field: "",
        row_numbering: Numbering {
            gap_field: "",
            first: 1,
        },
        col_numbering: Numbering {
            gap_field: "",
            first: 1,
        },
        col_label: ColLabel::Letters,
        max_rows: 0,
        max_cols: 0,
        cell: None,
        row_hidden: None,
        row_height: None,
        height_unit: "",
        col_hidden: None,
        col_width: None,
        width_unit: "",
        viewport: "",
        style_hint: None,
        cell_derived: &[],
    };

    /// The slot as the watch snapshot's schema carries it (`schema.grid`).
    pub fn to_value(&self) -> Json {
        let num = |n: Numbering| {
            Json::obj([
                ("gap_field", Json::from(n.gap_field)),
                ("first", Json::from(n.first)),
            ])
        };
        let cell = self.cell.map_or(Json::Null, |c| {
            Json::obj([
                ("encoding", Json::from(c.encoding)),
                ("type", Json::from(c.ty)),
                ("value", Json::from(c.value)),
                ("formula", Json::from(c.formula)),
                ("formula_kind", Json::from(c.formula_kind)),
                ("style", Json::from(c.style)),
                (
                    "types",
                    Json::obj(c.types.iter().map(|(t, m)| (*t, Json::from(*m)))),
                ),
            ])
        });
        let hint = self.style_hint.map_or(Json::Null, |h| {
            Json::obj([
                ("field", Json::from(h.field)),
                ("default", Json::from(h.default)),
                ("table", Json::from(h.table)),
                ("parts", Json::list(h.parts.iter().copied())),
            ])
        });
        Json::obj([
            ("rows", Json::from(self.rows)),
            ("cols", Json::from(self.cols)),
            ("cells", Json::from(self.cells)),
            ("col_fields", Json::from(self.col_fields)),
            ("col_container_field", Json::from(self.col_container_field)),
            ("row_fields_start", Json::from(self.row_fields_start)),
            ("col_of_cell_field", Json::from(self.col_of_cell_field)),
            (
                "numbering",
                Json::obj([
                    ("rows", num(self.row_numbering)),
                    ("cols", num(self.col_numbering)),
                ]),
            ),
            ("col_label", Json::from(self.col_label.as_str())),
            (
                "max",
                Json::obj([
                    ("rows", Json::from(self.max_rows)),
                    ("cols", Json::from(self.max_cols)),
                ]),
            ),
            ("cell", cell),
            (
                "row_fields",
                Json::obj([
                    ("hidden", Json::from(self.row_hidden)),
                    ("height", Json::from(self.row_height)),
                    ("height_unit", Json::from(self.height_unit)),
                ]),
            ),
            (
                "col_field_names",
                Json::obj([
                    ("hidden", Json::from(self.col_hidden)),
                    ("width", Json::from(self.col_width)),
                    ("width_unit", Json::from(self.width_unit)),
                ]),
            ),
            ("viewport", Json::from(self.viewport)),
            ("style_hint", hint),
            (
                "cell_derived",
                Json::list(self.cell_derived.iter().copied()),
            ),
        ])
    }

    /// Whether `field` of a row (a key of `cells` after the separator) is a cell, not a row
    /// field.
    pub fn is_cell_field(&self, field: &str) -> bool {
        self.row_fields_start.is_empty() || !field.starts_with(self.row_fields_start)
    }

    /// The address of the column a cell field names, on the container at `container` (an
    /// address): `col_of_cell_field` filled in. `None` for a row field.
    pub fn col_address(&self, container: &str, field: &str) -> Option<String> {
        if !self.is_cell_field(field) {
            return None;
        }
        Some(
            self.col_of_cell_field
                .replace("{container}", container)
                .replace("{field}", field),
        )
    }
}

/// Rows or columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Axis {
    Rows,
    Cols,
}

impl Axis {
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::Rows => "rows",
            Axis::Cols => "cols",
        }
    }
}

/// Column `n`'s letters: 1 = A, 26 = Z, 27 = AA, 16,384 = XFD (`""` for 0).
pub fn col_letters(mut n: u64) -> String {
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII letters")
}

/// A cell's A1 name: its column's letters and its row number.
pub fn a1(row: u64, col: u64) -> String {
    format!("{}{row}", col_letters(col))
}

/// A cell named by the last keyed row and column of its container at or before it, plus
/// offsets. `row: None` means the container has no keyed row at or before the cell: then
/// `row_off` is the row number itself (so at least the first). The same for columns.
///
/// The keys are the live store's; the addresses ([`crate::session::Pack::address_of`]) are what
/// the wire carries: the kit fills them on every corner it hands out. A corner given by address
/// ([`crate::session::Session::set_cells_at`]) has its addresses set and its keys left `None`.
/// Equal by keys and offsets.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Corner {
    pub row: Option<Key>,
    pub row_off: u64,
    pub col: Option<Key>,
    pub col_off: u64,
    pub row_address: Option<String>,
    pub col_address: Option<String>,
}

impl PartialEq for Corner {
    fn eq(&self, o: &Self) -> bool {
        (&self.row, self.row_off, &self.col, self.col_off) == (&o.row, o.row_off, &o.col, o.col_off)
    }
}
impl Eq for Corner {}

impl Corner {
    /// A corner by key (its addresses start as the keys).
    pub fn new(row: Option<Key>, row_off: u64, col: Option<Key>, col_off: u64) -> Self {
        Corner {
            row_address: row.as_ref().map(|k| k.as_str().to_owned()),
            col_address: col.as_ref().map(|k| k.as_str().to_owned()),
            row,
            row_off,
            col,
            col_off,
        }
    }

    /// A corner by address, for [`crate::session::Session::set_cells_at`].
    pub fn at(row: Option<&str>, row_off: u64, col: Option<&str>, col_off: u64) -> Self {
        Corner {
            row: None,
            row_off,
            col: None,
            col_off,
            row_address: row.map(str::to_owned),
            col_address: col.map(str::to_owned),
        }
    }

    pub(crate) fn key(&self, axis: Axis) -> (Option<&Key>, u64) {
        match axis {
            Axis::Rows => (self.row.as_ref(), self.row_off),
            Axis::Cols => (self.col.as_ref(), self.col_off),
        }
    }

    /// The corner as the wire carries it: `{row, row_off, col, col_off}`, by address.
    pub fn to_value(&self) -> Json {
        Json::obj([
            ("row", Json::from(self.row_address.as_deref())),
            ("row_off", Json::from(self.row_off)),
            ("col", Json::from(self.col_address.as_deref())),
            ("col_off", Json::from(self.col_off)),
        ])
    }
}

/// A rectangle of cells: where it began (`start`) and where it is now (`end`); either may be
/// above or left of the other.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellRange {
    pub start: Corner,
    pub end: Corner,
}

impl CellRange {
    pub fn new(start: Corner, end: Corner) -> Self {
        CellRange { start, end }
    }

    pub fn to_value(&self) -> Json {
        Json::obj([
            ("start", self.start.to_value()),
            ("end", self.end.to_value()),
        ])
    }
}

/// A connection's cell caret and range selections on one container (its `cells` presence):
/// `active` (the caret, or none) and up to [`CELLS_RANGES_MAX`] ranges. `container` is the
/// container's key in the view schema's containers root; `address` its address (the kit fills
/// it, as on corners). Equal by keys.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Cells {
    pub container: Option<Key>,
    pub address: String,
    pub active: Option<Corner>,
    pub ranges: Vec<CellRange>,
}

impl PartialEq for Cells {
    fn eq(&self, o: &Self) -> bool {
        (&self.container, &self.active, &self.ranges) == (&o.container, &o.active, &o.ranges)
    }
}
impl Eq for Cells {}

impl Cells {
    /// Cells by key.
    pub fn new(container: Key, active: Option<Corner>, ranges: Vec<CellRange>) -> Self {
        Cells {
            address: container.as_str().to_owned(),
            container: Some(container),
            active,
            ranges,
        }
    }

    /// Cells by address, for [`crate::session::Session::set_cells_at`].
    pub fn at(container: &str, active: Option<Corner>, ranges: Vec<CellRange>) -> Self {
        Cells {
            container: None,
            address: container.to_owned(),
            active,
            ranges,
        }
    }

    /// `{container, active, ranges}`, by address.
    pub fn to_value(&self) -> Json {
        Json::obj([
            ("container", Json::from(&self.address)),
            (
                "active",
                self.active.as_ref().map_or(Json::Null, Corner::to_value),
            ),
            (
                "ranges",
                Json::List(self.ranges.iter().map(CellRange::to_value).collect()),
            ),
        ])
    }
}

/// An inserted or deleted run of rows or columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandOp {
    Insert,
    Delete,
}

impl BandOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BandOp::Insert => "insert",
            BandOp::Delete => "delete",
        }
    }
}

/// A run of rows or columns a verb inserted or deleted, by number: **an insert's after the
/// change, a delete's before it** (a blank row's delete has no list op: this is the only place
/// that says which one went).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    pub axis: Axis,
    pub op: BandOp,
    pub from: u64,
    pub to: u64,
}

/// What a grid verb wrote, handed to the kit with its ops
/// ([`crate::tools::StructOutput::writing`]). The kit
/// moves the acting **agent's** cells to it after the change applied (a person's never):
///
/// - [`CellsReport::Written`]: the ranges it wrote, by corner at the change's rev; `active` at
///   the first range's start.
/// - [`CellsReport::Band`] insert: the inserted rows (or columns) as whole-band ranges, to the
///   last column (row); delete: a caret on the first **visible** cell after the band (for rows,
///   the first row after it that isn't hidden, in the first column that isn't; for columns, the
///   same across), else the last visible cell before it; no ranges.
/// - [`CellsReport::Container`] (a sheet added or renamed): the agent's focus goes to it; its
///   cells are kept if they are on it, else a caret on its first cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellsReport {
    Written {
        container: Key,
        ranges: Vec<CellRange>,
    },
    Band {
        container: Key,
        band: Band,
    },
    Container {
        container: Key,
    },
}

impl CellsReport {
    /// The container it is on.
    pub fn container(&self) -> &Key {
        match self {
            CellsReport::Written { container, .. }
            | CellsReport::Band { container, .. }
            | CellsReport::Container { container } => container,
        }
    }
}

/// Why a corner doesn't name a cell of its container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CornerError {
    /// Its row or column isn't a keyed element of the container.
    NotKeyed(Axis),
    /// A keyed element lies strictly between its element and the cell.
    NotNormal(Axis),
    /// Past the last row or column, or before the first.
    OutsideSheet(Axis),
}

impl std::fmt::Display for CornerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CornerError::NotKeyed(a) => write!(
                f,
                "the corner's {} element isn't a live one of its container",
                a.as_str()
            ),
            CornerError::NotNormal(a) => write!(
                f,
                "the corner's {} offset reaches past a keyed element (not in normal form)",
                a.as_str()
            ),
            CornerError::OutsideSheet(a) => {
                write!(f, "the corner is outside the sheet's {}", a.as_str())
            }
        }
    }
}

/// One container's keyed rows and columns, in list order, with their numbers
/// ([`GridSlot`]'s rule).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GridNumbers {
    rows: Vec<(Key, u64)>,
    cols: Vec<(Key, u64)>,
    max_rows: u64,
    max_cols: u64,
    /// The numbers of hidden keyed rows and columns (a blank line is never hidden).
    hidden_rows: Vec<u64>,
    hidden_cols: Vec<u64>,
}

impl GridNumbers {
    /// From each axis's keyed elements in list order with their gaps.
    pub fn new(slot: &GridSlot, rows: Vec<(Key, u64)>, cols: Vec<(Key, u64)>) -> Self {
        let number = |n: Numbering, v: Vec<(Key, u64)>| {
            let mut out = Vec::with_capacity(v.len());
            let mut prev: Option<u64> = None;
            for (k, gap) in v {
                let gap = if planted_gap_ignored() { 0 } else { gap };
                let at = match prev {
                    None => gap.saturating_add(n.first),
                    Some(p) => p.saturating_add(gap).saturating_add(1),
                };
                out.push((k, at));
                prev = Some(at);
            }
            out
        };
        GridNumbers {
            rows: number(slot.row_numbering, rows),
            cols: number(slot.col_numbering, cols),
            max_rows: slot.max_rows,
            max_cols: slot.max_cols,
            hidden_rows: Vec::new(),
            hidden_cols: Vec::new(),
        }
    }

    /// Mark keyed elements hidden (`keys` of an axis; unknown ones are ignored).
    pub fn hide(mut self, axis: Axis, keys: &[Key]) -> Self {
        let ns: Vec<u64> = keys.iter().filter_map(|k| self.number(axis, k)).collect();
        match axis {
            Axis::Rows => self.hidden_rows.extend(ns),
            Axis::Cols => self.hidden_cols.extend(ns),
        }
        self
    }

    /// Whether line `n` of an axis is a hidden keyed one.
    pub fn is_hidden(&self, axis: Axis, n: u64) -> bool {
        match axis {
            Axis::Rows => self.hidden_rows.contains(&n),
            Axis::Cols => self.hidden_cols.contains(&n),
        }
    }

    /// The first visible line of an axis at or after `n`; else the last one before `n`; else
    /// `n` (every line hidden), all within the axis's bounds.
    pub fn visible_near(&self, axis: Axis, n: u64) -> u64 {
        let max = self.axis(axis).1.max(1);
        let n = n.clamp(1, max);
        let mut x = n;
        // hidden lines are keyed, so at most that many are skipped
        while x <= max && self.is_hidden(axis, x) {
            x += 1;
        }
        if x <= max {
            return x;
        }
        let mut y = n - 1;
        while y >= 1 && self.is_hidden(axis, y) {
            y -= 1;
        }
        if y >= 1 {
            y
        } else {
            n
        }
    }

    fn axis(&self, axis: Axis) -> (&[(Key, u64)], u64) {
        match axis {
            Axis::Rows => (&self.rows, self.max_rows),
            Axis::Cols => (&self.cols, self.max_cols),
        }
    }

    /// The keyed elements of an axis with their numbers, in list order.
    pub fn keyed(&self, axis: Axis) -> &[(Key, u64)] {
        self.axis(axis).0
    }

    /// Keyed element `key`'s number, when it is one of the container's.
    pub fn number(&self, axis: Axis, key: &Key) -> Option<u64> {
        self.axis(axis)
            .0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, n)| *n)
    }

    /// The number a corner's part names, checked: keyed in this container, in normal form,
    /// within bounds.
    pub(crate) fn line(&self, axis: Axis, at: Option<&Key>, off: u64) -> Result<u64, CornerError> {
        let (els, max) = self.axis(axis);
        let (n, next) = match at {
            None => (off, els.first().map(|(_, n)| *n)),
            Some(k) => {
                let i = els
                    .iter()
                    .position(|(x, _)| x == k)
                    .ok_or(CornerError::NotKeyed(axis))?;
                (
                    els[i].1.saturating_add(off),
                    els.get(i + 1).map(|(_, n)| *n),
                )
            }
        };
        if n == 0 || n > max {
            return Err(CornerError::OutsideSheet(axis));
        }
        if next.is_some_and(|x| n >= x) {
            return Err(CornerError::NotNormal(axis));
        }
        Ok(n)
    }

    /// A corner's (row, column) numbers, checked.
    pub(crate) fn cell(&self, c: &Corner) -> Result<(u64, u64), CornerError> {
        let (r, ro) = c.key(Axis::Rows);
        let (k, co) = c.key(Axis::Cols);
        Ok((self.line(Axis::Rows, r, ro)?, self.line(Axis::Cols, k, co)?))
    }

    /// A corner's (row, column) numbers, when it names a cell of this container in normal
    /// form within the sheet; `None` otherwise.
    pub fn cell_of(&self, c: &Corner) -> Option<(u64, u64)> {
        self.cell(c).ok()
    }

    /// The normal-form part naming line `n` of an axis: the last keyed element at or before
    /// it and the offset.
    pub fn place(&self, axis: Axis, n: u64) -> (Option<Key>, u64) {
        let els = self.axis(axis).0;
        match els.iter().rev().find(|(_, x)| *x <= n) {
            Some((k, x)) => (Some(k.clone()), n - x),
            None => (None, n),
        }
    }

    /// The normal-form corner of cell (`row`, `col`).
    pub fn corner(&self, row: u64, col: u64) -> Corner {
        let (r, ro) = self.place(Axis::Rows, row);
        let (c, co) = self.place(Axis::Cols, col);
        Corner::new(r, ro, c, co)
    }

    /// The last row and column numbers.
    pub fn max(&self) -> (u64, u64) {
        (self.max_rows, self.max_cols)
    }
}

/// Planted ([`crate::differential::GridMutant::GapIgnored`]): numbering ignores the gap
/// field. Without the `differential` feature: `false`.
fn planted_gap_ignored() -> bool {
    plant_on!(
        crate::differential::grid_mutant() == Some(crate::differential::GridMutant::GapIgnored)
    )
}

// ----- a change frame's derived values, typed -----

/// One unit's derived values of one name, by field (a row's formula cells' A1 text under
/// `a1`), as [`crate::session::Pack::page_derived`] gives them for a snapshot page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDerived {
    pub unit: Key,
    pub name: &'static str,
    /// `(field, value)`, each field once.
    pub values: Vec<(String, String)>,
}

/// A style hint: bold, a solid fill colour (`#RRGGBB`, or `None`) and the number-format code
/// as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleHint {
    pub bold: bool,
    pub fill: Option<String>,
    pub numfmt: String,
}

impl StyleHint {
    pub fn to_value(&self) -> Json {
        Json::obj([
            ("bold", Json::from(self.bold)),
            ("fill", Json::from(self.fill.as_deref())),
            ("numfmt", Json::from(&self.numfmt)),
        ])
    }
}

/// A container's derived values: its merged ranges and its style table (by style index).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerDerived {
    pub merges: Option<Vec<CellRange>>,
    pub styles: Option<BTreeMap<String, StyleHint>>,
}

/// What a change frame's `derived` carries for a grid pack, by address (every corner's
/// addresses filled), typed. Built by the pack in [`crate::session::Pack::watch_extras`]
/// and put on the frame with [`crate::session::WatchExtras::grid`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameDerived {
    /// Per unit (by address): per cell field, its new A1 text, `None` when it stopped being a formula.
    pub a1: BTreeMap<String, BTreeMap<String, Option<String>>>,
    /// Per container (by address): its merges and style table, whole, when either changed.
    pub containers: BTreeMap<String, ContainerDerived>,
    /// What the verb wrote: its container's address and ranges.
    pub written: Option<(String, Vec<CellRange>)>,
    /// The bands it inserted or deleted, each with its container's address.
    pub bands: Vec<(String, Band)>,
}

impl FrameDerived {
    /// `{units, containers, written, bands}`, each only when it has something; `None` when
    /// nothing does (the frame's `derived` is then `null`).
    pub fn to_value(&self) -> Option<Json> {
        let mut out: Vec<(String, Json)> = Vec::new();
        if !self.a1.is_empty() {
            let units = self.a1.iter().map(|(u, cells)| {
                let a1 = Json::obj(
                    cells
                        .iter()
                        .map(|(f, t)| (f.as_str(), Json::from(t.as_deref()))),
                );
                (u.as_str(), Json::obj([("a1", a1)]))
            });
            out.push(("units".into(), Json::obj(units)));
        }
        if !self.containers.is_empty() {
            let cs = self.containers.iter().map(|(c, d)| {
                let mut v: Vec<(String, Json)> = Vec::new();
                if let Some(m) = &d.merges {
                    v.push((
                        "merges".into(),
                        Json::List(m.iter().map(CellRange::to_value).collect()),
                    ));
                }
                if let Some(s) = &d.styles {
                    v.push((
                        "styles".into(),
                        Json::obj(s.iter().map(|(i, h)| (i.as_str(), h.to_value()))),
                    ));
                }
                (c.as_str(), Json::Obj(v))
            });
            out.push(("containers".into(), Json::obj(cs)));
        }
        if let Some((c, ranges)) = &self.written {
            out.push((
                "written".into(),
                Json::obj([
                    ("container", Json::from(c)),
                    (
                        "ranges",
                        Json::List(ranges.iter().map(CellRange::to_value).collect()),
                    ),
                ]),
            ));
        }
        if !self.bands.is_empty() {
            let bands = self.bands.iter().map(|(c, b)| {
                Json::obj([
                    ("container", Json::from(c)),
                    ("axis", Json::from(b.axis.as_str())),
                    ("op", Json::from(b.op.as_str())),
                    ("from", Json::from(b.from)),
                    ("to", Json::from(b.to)),
                    (
                        "numbers",
                        Json::from(match b.op {
                            BandOp::Insert => "after",
                            BandOp::Delete => "before",
                        }),
                    ),
                ])
            });
            out.push(("bands".into(), Json::List(bands.collect())));
        }
        (!out.is_empty()).then_some(Json::Obj(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        for (n, l) in [
            (1, "A"),
            (26, "Z"),
            (27, "AA"),
            (52, "AZ"),
            (53, "BA"),
            (16384, "XFD"),
        ] {
            assert_eq!(col_letters(n), l, "column {n}");
        }
        assert_eq!(a1(18, 5), "E18");
    }

    fn k(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    const SLOT: GridSlot = GridSlot {
        row_numbering: Numbering {
            gap_field: "@gap",
            first: 1,
        },
        col_numbering: Numbering {
            gap_field: "gap",
            first: 1,
        },
        max_rows: 100,
        max_cols: 10,
        ..GridSlot::NONE
    };

    #[test]
    fn numbers_and_normal_form() {
        // rows 1, 3, 4; cols 1, 2
        let g = GridNumbers::new(
            &SLOT,
            vec![(k("r1"), 0), (k("r2"), 1), (k("r3"), 0)],
            vec![(k("c1"), 0), (k("c2"), 0)],
        );
        assert_eq!(g.number(Axis::Rows, &k("r2")), Some(3));
        assert_eq!(g.line(Axis::Rows, Some(&k("r1")), 1), Ok(2));
        assert_eq!(
            g.line(Axis::Rows, Some(&k("r1")), 2),
            Err(CornerError::NotNormal(Axis::Rows))
        );
        assert_eq!(g.line(Axis::Rows, Some(&k("r3")), 96), Ok(100));
        assert_eq!(
            g.line(Axis::Rows, Some(&k("r3")), 97),
            Err(CornerError::OutsideSheet(Axis::Rows))
        );
        assert_eq!(
            g.corner(2, 5),
            Corner::new(Some(k("r1")), 1, Some(k("c2")), 3)
        );
        let empty = GridNumbers::new(&SLOT, vec![], vec![]);
        assert_eq!(empty.corner(1, 1), Corner::new(None, 1, None, 1));
        assert_eq!(
            empty.line(Axis::Rows, None, 0),
            Err(CornerError::OutsideSheet(Axis::Rows))
        );
    }
}
