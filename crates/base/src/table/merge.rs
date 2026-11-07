//! The grid three-way merge of `write --base` (contract §3.4, K-BR-3, K7): only what `ours`
//! changed since `base` applies to `theirs` (the live table), element by element, by key;
//! overlaps are **reported, never last-writer-wins**.
//!
//! The element rules (the grid form of O1–O5):
//!
//! | ours | theirs | result |
//! |---|---|---|
//! | set cell X | set X to another value | conflict `cell` |
//! | set cell X | set X to the same value | taken once |
//! | set a cell in row R (or column C) | deleted R (C) | conflict `deleted_row` (`deleted_col`): the value would be hidden |
//! | moved R (C) | deleted it | conflict `deleted_row` (`deleted_col`) |
//! | deleted R (C) | set a cell in it, or moved it (or renamed C) | conflict `edited_row` (`edited_col`) |
//! | a text's row, not exact ([`merge3_text`]) | set a cell or a quote in it, to anything but ours' row | conflict `edited_row`, none of ours' writes to R applied |
//! | moved R (C) | moved it elsewhere | conflict `moved_row` (`moved_col`); to the same place: once |
//! | renamed C | renamed C differently | conflict `name`; renamed C vs deleted C: `deleted_col` |
//! | a quote exception | another one for the same field | conflict `quoting` |
//! | inserted rows (columns) at one gap | inserted at the same gap | **both land, theirs first** (K7) |
//! | deleted R | deleted R | once |
//! | anything | anything on other elements | both apply |
//!
//! A side's **moves** are the minimal set: the rows outside a longest run of rows it kept in
//! their base order (n − LIS), each with its new left neighbour (where a swap is ambiguous,
//! the LIS tie-break decides which row moved, the same way on both sides). Ours'
//! placements (inserts, moves, restores) go by runs: each run of consecutive placed rows in
//! ours' order lands right before the nearest row after it in ours' order that is live in
//! the result (or at the end), as one forward chain. So rows theirs inserted (or moved) into
//! the same gap come first, and ours' appends stay at the end. A conflicted element is left
//! as theirs has it. Ours' new keys that theirs already uses are re-minted
//! ([`Merge::rekeyed`]), and the key counters cover both sides.
//!
//! The result is the clean part ([`Merge::edits`], one change on `theirs`) and the conflict
//! report ([`Merge::conflicts`]). `write --base` applies nothing when there are conflicts
//! (exit 1) and the clean part with `--partial` (K-BR-9).

use std::collections::{BTreeMap, HashMap, HashSet};

use super::ops::Change;
use super::*;
use crate::elem::lis;

/// Which rule a conflict is (contract §3.4's names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConflictRule {
    /// Both set one cell, to different values.
    Cell,
    /// Ours wrote into (or moved) a row theirs deleted.
    DeletedRow,
    /// Ours deleted a row theirs wrote into or moved.
    EditedRow,
    /// Both moved one row, to different places.
    MovedRow,
    DeletedCol,
    EditedCol,
    MovedCol,
    /// Both renamed one column, differently.
    Name,
    /// Both gave one field different quote exceptions.
    Quoting,
}

impl ConflictRule {
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictRule::Cell => "cell",
            ConflictRule::DeletedRow => "deleted_row",
            ConflictRule::EditedRow => "edited_row",
            ConflictRule::MovedRow => "moved_row",
            ConflictRule::DeletedCol => "deleted_col",
            ConflictRule::EditedCol => "edited_col",
            ConflictRule::MovedCol => "moved_col",
            ConflictRule::Name => "name",
            ConflictRule::Quoting => "quoting",
        }
    }
}

impl fmt::Display for ConflictRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a conflict (or a write it met) is about, by key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Region {
    Cell {
        row: Key,
        col: Key,
    },
    /// A row's place or existence.
    Row(Key),
    Col(Key),
    /// A column's header text.
    Name(Key),
    Quote(QuoteAt),
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Region::Cell { row, col } => write!(f, "{row}/{col}"),
            Region::Row(r) => write!(f, "row {r}"),
            Region::Col(c) => write!(f, "col {c}"),
            Region::Name(c) => write!(f, "name of {c}"),
            Region::Quote(at) => write!(f, "quoting of {at}"),
        }
    }
}

/// Where a row or column is in one version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ElemAt {
    Unknown,
    Deleted,
    /// Live at `index` (0-based, in the live order), right after `after` (`None`: first).
    Live {
        index: usize,
        after: Option<Key>,
    },
}

/// [`ElemAt::of`] for many keys of one axis: its live keys and their indexes, read once, so
/// each lookup is O(1) (a merge's conflicts, one per row or column).
pub(crate) struct ElemIndex<'a> {
    axis: &'a Axis,
    keys: Vec<&'a Key>,
    at: HashMap<&'a Key, usize>,
}

impl<'a> ElemIndex<'a> {
    pub(crate) fn new(axis: &'a Axis) -> Self {
        let keys: Vec<&Key> = axis.live().collect();
        let at = axis.live_index();
        ElemIndex { axis, keys, at }
    }

    /// What [`ElemAt::of`] gives for `k` (the planted
    /// [`crate::differential::GridCostMutant::ConflictWalk`]: by its walk).
    pub(crate) fn of(&self, k: &Key) -> ElemAt {
        if super::conflict_walk() {
            return ElemAt::of(self.axis, k);
        }
        match self.at.get(k) {
            Some(&index) => ElemAt::Live {
                index,
                after: index.checked_sub(1).map(|i| self.keys[i].clone()),
            },
            None if self.axis.knows(k) => ElemAt::Deleted,
            None => ElemAt::Unknown,
        }
    }
}

impl ElemAt {
    pub fn of(a: &Axis, k: &Key) -> ElemAt {
        match a.state(k) {
            ElemState::Unknown => ElemAt::Unknown,
            ElemState::Deleted => ElemAt::Deleted,
            ElemState::Live(index) => ElemAt::Live {
                index,
                after: index.checked_sub(1).and_then(|i| a.at(i)).cloned(),
            },
        }
    }
}

/// What one version holds for a conflict's region.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Held {
    /// A cell's value, a name, or a quote exception (`q`/`p`); `None`: absent.
    Value(Option<String>),
    /// A row's or column's place.
    Elem(ElemAt),
}

impl fmt::Display for Held {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Held::Value(Some(v)) => write!(f, "{v:?}"),
            Held::Value(None) => f.write_str("absent"),
            Held::Elem(ElemAt::Unknown) => f.write_str("unknown"),
            Held::Elem(ElemAt::Deleted) => f.write_str("deleted"),
            Held::Elem(ElemAt::Live { index, after }) => match after {
                Some(a) => write!(f, "#{} (after {a})", index + 1),
                None => write!(f, "#{} (first)", index + 1),
            },
        }
    }
}

/// One conflict (K-BR-8): the rule, the region by key, what base, ours and theirs hold
/// there, and the writes of theirs it met when they are elsewhere (the row a cell's row
/// delete, the cells and move that make a row delete conflict; empty when theirs' write is
/// the region itself). Positions and rendered lines, and who wrote or claims the region, are
/// the session's to add (from the live table's line map and presence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub rule: ConflictRule,
    pub at: Region,
    pub base: Held,
    pub ours: Held,
    pub theirs: Held,
    pub theirs_writes: Vec<Region>,
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}: base {}, ours {}, theirs {}",
            self.rule, self.at, self.base, self.ours, self.theirs
        )?;
        if !self.theirs_writes.is_empty() {
            let w: Vec<String> = self.theirs_writes.iter().map(|r| r.to_string()).collect();
            write!(f, " (theirs wrote {})", w.join(", "))?;
        }
        if self.is_rewritten_row() {
            f.write_str("; your text changed this row and someone else changed it since")?;
        }
        Ok(())
    }
}

impl Conflict {
    /// An `edited_row` where ours kept the row: the text changed it too ([`merge3_text`]),
    /// not deleted it.
    pub fn is_rewritten_row(&self) -> bool {
        self.rule == ConflictRule::EditedRow
            && matches!(self.at, Region::Row(_))
            && matches!(self.ours, Held::Elem(ElemAt::Live { .. }))
    }
}

/// [`merge3()`]'s result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Merge {
    /// Ours' clean changes, as one change on `theirs`.
    pub edits: Vec<Edit>,
    /// Columns' structure, rows' structure, names, cells (by row then column, in ours'
    /// order), quoting.
    pub conflicts: Vec<Conflict>,
    /// Ours' new keys theirs already used, and the keys they get (rows, then columns).
    pub rekeyed: Vec<(Key, Key)>,
    /// Through a text: rows ours deleted that theirs only moved. The delete stands, with no
    /// conflict and no note; the session records them for the audit.
    pub moved_then_deleted: Vec<Key>,
}

impl Merge {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// One side's structural changes to an axis, against base.
struct Sided {
    live: Vec<Key>,
    idx: HashMap<Key, usize>,
    deleted: BTreeSet<Key>,
    restored: BTreeSet<Key>,
    inserted: BTreeSet<Key>,
    moved: BTreeSet<Key>,
}

impl Sided {
    fn new(base: &Axis, side: &Axis) -> Self {
        let live = side.live_keys();
        let idx: HashMap<Key, usize> = live
            .iter()
            .enumerate()
            .map(|(i, k)| (k.clone(), i))
            .collect();
        let base_live = base.live_keys();
        let base_idx: HashMap<&Key, usize> =
            base_live.iter().enumerate().map(|(i, k)| (k, i)).collect();
        let deleted = base_live
            .iter()
            .filter(|k| !idx.contains_key(*k))
            .cloned()
            .collect();
        let restored = live
            .iter()
            .filter(|k| base.knows(k) && !base.is_live(k))
            .cloned()
            .collect();
        let inserted = live.iter().filter(|k| !base.knows(k)).cloned().collect();
        let common: Vec<&Key> = live.iter().filter(|k| base_idx.contains_key(k)).collect();
        let seq: Vec<usize> = common.iter().map(|k| base_idx[k]).collect();
        let stay: HashSet<&Key> = lis(&seq).into_iter().map(|i| common[i]).collect();
        let moved = common
            .into_iter()
            .filter(|k| !stay.contains(k))
            .cloned()
            .collect();
        Sided {
            live,
            idx,
            deleted,
            restored,
            inserted,
            moved,
        }
    }

    /// The key right before `k` in this side's live order.
    fn after(&self, k: &Key) -> Option<&Key> {
        let i = *self.idx.get(k)?;
        i.checked_sub(1).map(|p| &self.live[p])
    }
}

/// The keys whose value differs from `base` to `side`, with the side's value.
fn changed<K: Ord + Clone, V: PartialEq + Clone>(
    base: &BTreeMap<K, V>,
    side: &BTreeMap<K, V>,
) -> BTreeMap<K, Option<V>> {
    let mut out = BTreeMap::new();
    for (k, v) in side {
        if base.get(k) != Some(v) {
            out.insert(k.clone(), Some(v.clone()));
        }
    }
    for k in base.keys() {
        if !side.contains_key(k) {
            out.insert(k.clone(), None);
        }
    }
    out
}

/// What ours does to one axis cleanly.
#[derive(Default)]
struct Plan {
    delete: Vec<Key>,
    restore: Vec<Key>,
    /// Placed by ours (inserted, moved, restored).
    placed: HashSet<Key>,
    fresh: HashSet<Key>,
}

/// Ours' new keys that theirs already knows, re-minted.
fn rekey(base: &Table, ours: &Table, theirs: &Table) -> (Table, Vec<(Key, Key)>) {
    let mut out = Vec::new();
    let mut remint = |b: &Axis, o: &Axis, t: &Axis, next: &mut u64, prefix: char| {
        let mut map = HashMap::new();
        for k in o.places() {
            if b.knows(k) || !t.knows(k) {
                continue;
            }
            let n = loop {
                let c = Key::new(format!("{prefix}{next}")).expect("a minted key");
                *next = next.saturating_add(1);
                if !t.knows(&c) && !o.knows(&c) {
                    break c;
                }
            };
            out.push((k.clone(), n.clone()));
            map.insert(k.clone(), n);
        }
        map
    };
    let mut nr = ours.next_row.max(theirs.next_row);
    let mut nc = ours.next_col.max(theirs.next_col);
    let rows = remint(&base.rows, &ours.rows, &theirs.rows, &mut nr, 'r');
    let cols = remint(&base.cols, &ours.cols, &theirs.cols, &mut nc, 'c');
    let mut t = if rows.is_empty() && cols.is_empty() {
        ours.clone()
    } else {
        ours.renamed(
            &|k| rows.get(k).cloned().unwrap_or_else(|| k.clone()),
            &|k| cols.get(k).cloned().unwrap_or_else(|| k.clone()),
        )
    };
    t.next_row = t.next_row.max(nr);
    t.next_col = t.next_col.max(nc);
    (t, out)
}

/// The three-way merge of grid versions by key: `ours`' changes since `base`, applied to
/// `theirs` (the live table), with the element rules of the module docs. `base`, `ours` and
/// `theirs` are one table's versions (keys shared). Pure: the same inputs give the same
/// edits and report.
pub fn merge3(base: &Table, ours: &Table, theirs: &Table) -> Result<Merge, TableError> {
    merge3_with(base, ours, theirs, false, None)
}

/// [`merge3`] where `ours` came from a **text** (`write --base`, contract §3.4): a text has no
/// keys, so which row is which was a guess, and a row is merged as one unit. A row `ours` didn't
/// leave `exact` ([`super::Reconciled::exact_rows`]) that `theirs` changed too, to anything
/// other than `ours`' row, is one `edited_row` conflict at the row (`ours` live: rewritten, not
/// deleted), and none of `ours`' writes to it apply, with `--partial` either. An exact row
/// keeps `theirs`' cells. `exact` empty is the strict reading: no row is exact.
pub fn merge3_text(
    base: &Table,
    ours: &Table,
    theirs: &Table,
    exact: &HashSet<Key>,
) -> Result<Merge, TableError> {
    merge3_with(base, ours, theirs, false, Some(exact))
}

/// The merge of an **outside save** (contract §2.9.1): as [`merge3`], but nothing is held
/// back. Where both sides changed one element, the file's side (`ours`, applied after the
/// live change as the external participant: the later write) wins, and the overlap is
/// reported in [`Merge::conflicts`] as **overridden**, not left out:
/// - `cell`, `name`, `quoting`: the file's value is set;
/// - `edited_row` (`edited_col`): the file deleted an element the live side edited or
///   moved: the delete applies (the element stays restorable);
/// - `deleted_row` (`deleted_col`): the file wrote into (or moved) an element the live side
///   deleted: the element is restored, then the file's writes (and move) apply;
/// - `moved_row` (`moved_col`): the file's place.
///
/// [`Merge::edits`] is the whole merged change on `theirs`. An element `theirs` no longer
/// knows at all (dropped, `rows drop-deleted`) can't be restored: a caller that wants the
/// file's changes to it kept gives it a fresh key in `ours` first (the session does).
pub fn merge3_outside(base: &Table, ours: &Table, theirs: &Table) -> Result<Merge, TableError> {
    merge3_with(base, ours, theirs, true, None)
}

/// [`merge3_outside`] where the file is a text, as it always is on a save (contract §2.9.1):
/// a row the file didn't leave `exact` ([`super::Reconciled::exact_rows`]) that the live side
/// changed too **wins whole**. Every cell the live side changed is set to the file's value, each
/// reported as an overridden `cell` (the session's Overwritten note, audit event and the
/// version before the save keep the live value), whether or not the file changed that cell.
pub fn merge3_outside_text(
    base: &Table,
    ours: &Table,
    theirs: &Table,
    exact: &HashSet<Key>,
) -> Result<Merge, TableError> {
    merge3_with(base, ours, theirs, true, Some(exact))
}

/// The planted merge faults ([`crate::differential::MergeMutant`]): (the outside save's
/// whole-row loop over the live side's columns, the both-changed test by cell over the live
/// side's columns, `write --base` conflicting on a byte-neutral live switch, the keyed merge
/// taking ours' value over one, the keyed outside save not reporting its override of one).
/// Without the `differential` feature there is no hook: all are `false`.
#[cfg(any(test, feature = "differential"))]
fn planted() -> (bool, bool, bool, bool, bool) {
    use crate::differential::{merge_mutant, MergeMutant};
    let m = merge_mutant();
    (
        m == Some(MergeMutant::OutsideWholeRowLiveCols),
        m == Some(MergeMutant::ByCellLiveCols),
        m == Some(MergeMutant::WriteBaseNeutralConflicts),
        m == Some(MergeMutant::KeyedMergeNeutralSwitch),
        m == Some(MergeMutant::KeyedOutsideNeutralSwitch),
    )
}

#[cfg(not(any(test, feature = "differential")))]
fn planted() -> (bool, bool, bool, bool, bool) {
    (false, false, false, false, false)
}

fn merge3_with(
    base: &Table,
    ours: &Table,
    theirs: &Table,
    file_wins: bool,
    exact: Option<&HashSet<Key>>,
) -> Result<Merge, TableError> {
    let (
        planted_whole_row,
        planted_by_cell,
        planted_neutral_conflicts,
        planted_keyed_neutral,
        planted_keyed_outside,
    ) = planted();
    let (ours, rekeyed) = rekey(base, ours, theirs);
    let ours = &ours;
    // each live column's position, by key: a row's rendering compared in O(its cells)
    let (base_cols_at, ours_cols_at) = (base.col_positions(), ours.col_positions());
    let mut w = theirs.clone();
    let mut ch = Change::new(&w);
    w.next_row = w.next_row.max(ours.next_row);
    w.next_col = w.next_col.max(ours.next_col);
    let mut conflicts = Vec::new();

    let rows = (
        Sided::new(&base.rows, &ours.rows),
        Sided::new(&base.rows, &theirs.rows),
    );
    let cols = (
        Sided::new(&base.cols, &ours.cols),
        Sided::new(&base.cols, &theirs.cols),
    );
    let cells = (
        changed(&base.cells, &ours.cells),
        changed(&base.cells, &theirs.cells),
    );
    let names = (
        changed(&base.names, &ours.names),
        changed(&base.names, &theirs.names),
    );
    let quotes = (
        changed(&base.quoting, &ours.quoting),
        changed(&base.quoting, &theirs.quoting),
    );

    // the rows and columns ours writes into (a file save restores them if theirs deleted
    // them)
    let mut o_rows: HashSet<Key> = HashSet::new();
    let mut o_cols: HashSet<Key> = HashSet::new();
    if file_wins || exact.is_some() {
        for (r, c) in cells.0.keys() {
            o_rows.insert(r.clone());
            o_cols.insert(c.clone());
        }
        o_cols.extend(names.0.keys().cloned());
        for at in quotes.0.keys() {
            match at {
                QuoteAt::Cell(r, c) => {
                    o_rows.insert(r.clone());
                    o_cols.insert(c.clone());
                }
                QuoteAt::Header(c) => {
                    o_cols.insert(c.clone());
                }
            }
        }
    }

    // theirs' writes by row and by column
    let mut t_row: HashMap<&Key, Vec<Region>> = HashMap::new();
    let mut t_col: HashMap<&Key, Vec<Region>> = HashMap::new();
    for (r, c) in cells.1.keys() {
        let at = Region::Cell {
            row: r.clone(),
            col: c.clone(),
        };
        t_row.entry(r).or_default().push(at.clone());
        t_col.entry(c).or_default().push(at);
    }
    for c in names.1.keys() {
        t_col.entry(c).or_default().push(Region::Name(c.clone()));
    }
    // a text's rows that aren't exact and that theirs changed too, to anything but ours' row:
    // one unit each. "Changed" is by the rendered fields' decoded values, exactly (a switch
    // between empty and absent that renders the same is no change; a blank line and `""` differ;
    // how a field is quoted is no change), over the columns ours keeps: a column ours deleted
    // takes its cells with it (restorable), and theirs moving the row is no change to it
    let mut whole: BTreeMap<Key, Vec<Region>> = BTreeMap::new();
    // a live switch between empty and absent in one cell isn't a live edit of that cell when the
    // base row, with only that cell switched, renders the same bytes (a blank row given a `""`
    // cell, blank to `,`, changes bytes and is an edit)
    let neutral_live = |r: &Key, c: &Key| -> bool {
        let (b, t) = (base.cell(r, c), theirs.cell(r, c));
        b != t
            && b.unwrap_or("").is_empty()
            && t.unwrap_or("").is_empty()
            && base.rows.is_live(r)
            && base.row_same_with(r, c, t, b, &base_cols_at)
    };
    // through a text, the rows ours and theirs render the same (the same decoded fields: an
    // empty field and an absent cell alike): the same row, so no conflict on any of its cells,
    // and its cells are ours' (contract §3.4 "to anything but ours' row", §2.9.1)
    let mut same_rows: HashSet<Key> = HashSet::new();
    // an outside save: a live value between empty and absent that the file's row renders anyway
    // (the file's row with that cell taken as the live value renders the same bytes): it already
    // stands in the result, so the file's value goes in with no override
    let file_renders_live = |r: &Key, c: &Key| -> bool {
        let (o, t) = (ours.cell(r, c), theirs.cell(r, c));
        o != t
            && o.unwrap_or("").is_empty()
            && t.unwrap_or("").is_empty()
            && ours.rows.is_live(r)
            && ours.row_same_with(r, c, t, o, &ours_cols_at)
    };
    if let Some(exact) = exact {
        let shown = |t: &Table| -> HashMap<Key, Vec<String>> {
            let recs = t.render();
            let rows = t.live_rows();
            let skip = recs.len() - rows.len();
            rows.into_iter()
                .zip(
                    recs.into_iter()
                        .skip(skip)
                        .map(|r| r.into_iter().map(|f| f.value).collect()),
                )
                .collect()
        };
        let (s_base, s_ours, s_theirs) = (shown(base), shown(ours), shown(theirs));
        let same_cols = same_cols_bt;
        let (bt_cols, ot_cols) = (same_cols(base, theirs), same_cols(ours, theirs));
        if ot_cols {
            for (k, r) in &s_ours {
                if theirs.rows.is_live(k) && s_theirs.get(k) == Some(r) {
                    same_rows.insert(k.clone());
                }
            }
        }
        for k in ours.rows.live() {
            if exact.contains(k) || !base.rows.is_live(k) {
                continue;
            }
            // a row theirs edited and then deleted: both changed, for a file (its row comes back
            // whole). `write --base` refuses a deleted row anyway (`deleted_row`)
            let gone = !theirs.rows.is_live(k);
            if gone && !file_wins {
                continue;
            }
            let kept = |c: &Key| ours.cols.is_live(c);
            // over the columns the text keeps that theirs knows: a column the live side deleted
            // after writing into it is still a change of theirs (theirs' live columns would miss
            // it); a column new in the text is not a change of either here
            let by_cell = |a: &Table, b: &Table| {
                if planted_by_cell {
                    // (xxxiii), planted: the live side's live columns
                    return theirs
                        .cols
                        .live()
                        .all(|c| !kept(c) || a.cell(k, c) == b.cell(k, c));
                }
                // the cells either side holds in the row (absent on both is the same), O(the
                // row's cells): every other column's cell is absent on both sides
                let kept_known = |c: &Key| kept(c) && theirs.cols.knows(c);
                a.row_cells(k)
                    .filter(|(c, _)| kept_known(c))
                    .all(|(c, v)| b.cell(k, c) == Some(v))
                    && b.row_cells(k)
                        .filter(|(c, _)| kept_known(c))
                        .all(|(c, v)| a.cell(k, c) == Some(v))
            };
            let unchanged = if bt_cols && !gone {
                s_base.get(k) == s_theirs.get(k)
            } else {
                by_cell(base, theirs)
            };
            let same = if ot_cols && !gone {
                s_ours.get(k) == s_theirs.get(k)
            } else {
                by_cell(ours, theirs)
            };
            if unchanged || same {
                continue;
            }
            let mut tw = t_row.get(k).cloned().unwrap_or_default();
            tw.retain(|w| !matches!(w, Region::Cell { col, .. } if !kept(col)));
            if !tw.is_empty() {
                tw.sort();
                whole.insert(k.clone(), tw);
            }
        }
    }

    let moved_then_deleted = std::cell::RefCell::new(Vec::new());
    // where a conflict's row or column is in each version: each axis's live index read once,
    // on first need, so each conflict is O(1) however many there are
    let tables = [base, ours, theirs];
    let idx: [std::cell::OnceCell<ElemIndex<'_>>; 6] = Default::default();
    let elem_at = |side: usize, row: bool, k: &Key| -> ElemAt {
        let t = tables[side];
        idx[side * 2 + usize::from(row)]
            .get_or_init(|| ElemIndex::new(if row { &t.rows } else { &t.cols }))
            .of(k)
    };
    let elem = |a: fn(&Table) -> &Axis, k: &Key| {
        let row = std::ptr::eq(a(base), &base.rows);
        (
            Held::Elem(elem_at(0, row, k)),
            Held::Elem(elem_at(1, row, k)),
            Held::Elem(elem_at(2, row, k)),
        )
    };
    let structure = |root: &'static str,
                     (o, t): &(Sided, Sided),
                     writes: &HashMap<&Key, Vec<Region>>,
                     written: &HashSet<Key>,
                     conflicts: &mut Vec<Conflict>|
     -> Plan {
        let is_row = root == ROWS;
        let get: fn(&Table) -> &Axis = if is_row { rows_of } else { cols_of };
        let b_axis = get(base);
        let line = |k: &Key| {
            if is_row {
                Region::Row(k.clone())
            } else {
                Region::Col(k.clone())
            }
        };
        let rule = |r: ConflictRule, c: ConflictRule| if is_row { r } else { c };
        let mut conflict = |rule: ConflictRule, k: &Key, theirs_writes: Vec<Region>| {
            let (b, ou, th) = elem(get, k);
            conflicts.push(Conflict {
                rule,
                at: line(k),
                base: b,
                ours: ou,
                theirs: th,
                theirs_writes,
            });
        };
        let mut plan = Plan::default();
        for k in b_axis.live() {
            if !o.deleted.contains(k) || t.deleted.contains(k) {
                continue;
            }
            let mut tw = writes.get(k).cloned().unwrap_or_default();
            // through a text, a move alone doesn't stop the delete (a row's place isn't its
            // content): no conflict, no note
            if t.moved.contains(k) && !(is_row && exact.is_some()) {
                tw.push(line(k));
            }
            if tw.is_empty() {
                plan.delete.push(k.clone());
                if t.moved.contains(k) && is_row {
                    moved_then_deleted.borrow_mut().push(k.clone());
                }
            } else {
                conflict(
                    rule(ConflictRule::EditedRow, ConflictRule::EditedCol),
                    k,
                    tw,
                );
                if file_wins {
                    // the file's delete wins; the element stays restorable
                    plan.delete.push(k.clone());
                }
            }
        }
        for k in &o.live {
            let text_exact = is_row && exact.is_some_and(|e| e.contains(k));
            let text_row = is_row && exact.is_some();
            // a text's move is no change: an exact row theirs deleted stays deleted
            if text_exact && t.deleted.contains(k) && (o.moved.contains(k) || !written.contains(k))
            {
                continue;
            }
            // a text changed the row only through a column (not exact, no cell of its own), and
            // theirs deleted it: a conflict; a file's row comes back
            if text_row && !text_exact && t.deleted.contains(k) && !written.contains(k) {
                conflict(ConflictRule::DeletedRow, k, vec![line(k)]);
                if file_wins && get(theirs).knows(k) {
                    plan.restore.push(k.clone());
                    if o.moved.contains(k) {
                        plan.placed.insert(k.clone());
                    }
                }
                continue;
            }
            if o.moved.contains(k) {
                if t.deleted.contains(k) {
                    let r = rule(ConflictRule::DeletedRow, ConflictRule::DeletedCol);
                    conflict(r, k, vec![line(k)]);
                    // (an element theirs dropped can't come back: the caller gives the
                    // file's a fresh key first)
                    if file_wins && get(theirs).knows(k) {
                        // back where it is kept, then to the file's place
                        plan.restore.push(k.clone());
                        plan.placed.insert(k.clone());
                    }
                } else if t.moved.contains(k) {
                    if text_row {
                        // both moved it, through a text: the text's place, no conflict
                        if o.after(k) != t.after(k) {
                            plan.placed.insert(k.clone());
                        }
                    } else if o.after(k) != t.after(k) {
                        let r = rule(ConflictRule::MovedRow, ConflictRule::MovedCol);
                        conflict(r, k, Vec::new());
                        if file_wins {
                            plan.placed.insert(k.clone());
                        }
                    }
                } else {
                    plan.placed.insert(k.clone());
                }
            } else if o.inserted.contains(k) {
                plan.placed.insert(k.clone());
                plan.fresh.insert(k.clone());
            } else if o.restored.contains(k) {
                if !t.idx.contains_key(k) {
                    plan.restore.push(k.clone());
                }
                plan.placed.insert(k.clone());
            } else if file_wins
                && t.deleted.contains(k)
                && written.contains(k)
                && get(theirs).knows(k)
            {
                // the file writes into an element theirs deleted: it comes back where it is
                // kept (the writes' conflicts are reported where they are)
                plan.restore.push(k.clone());
            }
        }
        plan
    };
    let col_plan = structure(COLS, &cols, &t_col, &o_cols, &mut conflicts);
    let mut row_plan = structure(ROWS, &rows, &t_row, &o_rows, &mut conflicts);
    if !file_wins {
        for (k, tw) in &whole {
            conflicts.push(Conflict {
                rule: ConflictRule::EditedRow,
                at: Region::Row(k.clone()),
                base: Held::Elem(elem_at(0, true, k)),
                ours: Held::Elem(elem_at(1, true, k)),
                theirs: Held::Elem(elem_at(2, true, k)),
                theirs_writes: tw.clone(),
            });
            // the row stays where theirs has it
            row_plan.placed.remove(k);
        }
    }
    let placer = if file_wins { place_after } else { place };
    placer(&mut ch, &mut w, COLS, &ours.cols, &col_plan)?;
    placer(&mut ch, &mut w, ROWS, &ours.rows, &row_plan)?;

    // names
    let mut held_names: HashSet<&Key> = HashSet::new();
    let deleted_by_theirs =
        |a: fn(&Table) -> &Axis, k: &Key| a(base).is_live(k) && !a(theirs).is_live(k);
    let value = |v: Option<&str>| Held::Value(v.map(str::to_owned));
    // changes to elements ours inserted and deleted again aren't carried (nothing shows them)
    let carried = |w: &Table, r: Option<&Key>, c: &Key| {
        r.map_or(true, |r| w.rows.knows(r)) && w.cols.knows(c)
    };
    for (c, v) in &names.0 {
        if !carried(&w, None, c) {
            continue;
        }
        let held = (
            value(base.name(c)),
            value(ours.name(c)),
            value(theirs.name(c)),
        );
        let mut push = |rule, theirs_writes| {
            conflicts.push(Conflict {
                rule,
                at: Region::Name(c.clone()),
                base: held.0.clone(),
                ours: held.1.clone(),
                theirs: held.2.clone(),
                theirs_writes,
            })
        };
        match names.1.get(c) {
            Some(tv) if tv == v => {}
            Some(_) => {
                push(ConflictRule::Name, Vec::new());
                if file_wins {
                    ch.name(&mut w, c, v.as_deref());
                } else {
                    held_names.insert(c);
                }
            }
            None if deleted_by_theirs(cols_of, c) => {
                push(ConflictRule::DeletedCol, vec![Region::Col(c.clone())]);
                if file_wins {
                    ch.name(&mut w, c, v.as_deref());
                } else {
                    held_names.insert(c);
                }
            }
            None => ch.name(&mut w, c, v.as_deref()),
        }
    }

    // cells, by row then column in ours' order (else the base's)
    let live_at = |a: &Axis| -> HashMap<Key, usize> {
        a.live().enumerate().map(|(i, k)| (k.clone(), i)).collect()
    };
    let (base_rows_at, base_cols_live_at) = (live_at(&base.rows), live_at(&base.cols));
    let pos = |s: &Sided, b: &HashMap<Key, usize>, k: &Key| {
        s.idx
            .get(k)
            .or_else(|| b.get(k))
            .copied()
            .unwrap_or(usize::MAX)
    };
    let mut order: Vec<(&(Key, Key), &Option<String>)> = cells.0.iter().collect();
    order.sort_by_key(|((r, c), _)| {
        (
            pos(&rows.0, &base_rows_at, r),
            pos(&cols.0, &base_cols_live_at, c),
        )
    });
    let mut held_cells: HashSet<(Key, Key)> = HashSet::new();
    for ((r, c), v) in order {
        if !carried(&w, Some(r), c) {
            continue;
        }
        if !file_wins && whole.contains_key(r) {
            // a rewritten row theirs changed: held whole (its one conflict is the row's)
            held_cells.insert((r.clone(), c.clone()));
            continue;
        }
        let held = (
            value(base.cell(r, c)),
            value(ours.cell(r, c)),
            value(theirs.cell(r, c)),
        );
        let (rule, theirs_writes) = match cells.1.get(&(r.clone(), c.clone())) {
            // through a text, a row theirs edited and then deleted is a row theirs changed, even
            // where the edit equals ours' value: `deleted_row` (a file's row comes back, noted)
            Some(_) if exact.is_some() && deleted_by_theirs(rows_of, r) => {
                (ConflictRule::DeletedRow, vec![Region::Row(r.clone())])
            }
            Some(tv) if tv == v => continue,
            // a row ours and theirs render the same: ours' cell, no conflict
            Some(_) if same_rows.contains(r) => {
                ch.cell(&mut w, r, c, v.as_deref());
                continue;
            }
            // `write --base` over a live switch between empty and absent that is no live edit of
            // the cell (`neutral_live`): the text's value, no conflict. Through a text only: a
            // text can't tell an empty field from an absent cell, so it never saw that switch.
            // By key (`merge3`) the switch is theirs' write (a clear is a set to `""`), and ours
            // setting the cell too is a `cell` conflict, never theirs' write overwritten
            Some(_)
                if !file_wins
                    // (xxxviii), planted: the keyed merge too
                    && (exact.is_some() || planted_keyed_neutral)
                    && !planted_neutral_conflicts
                    && neutral_live(r, c) =>
            {
                ch.cell(&mut w, r, c, v.as_deref());
                continue;
            }
            // an outside save through a text (`merge3_outside_text`) over a live switch between
            // empty and absent that is no live edit of the cell (`neutral_live`), or a live value
            // the file's row renders anyway (`file_renders_live`): the file's value, no override.
            // Through a text only: a text can't tell an empty field from an absent cell. By key
            // (`merge3_outside`) the switch is the live side's write (a clear is a set to `""`),
            // and the file setting the cell too is an overridden `cell`, reported
            Some(_)
                if file_wins
                    // (xxxix), planted: the keyed outside save too
                    && (exact.is_some() || planted_keyed_outside)
                    && (neutral_live(r, c) || file_renders_live(r, c)) =>
            {
                ch.cell(&mut w, r, c, v.as_deref());
                continue;
            }
            Some(_) => (ConflictRule::Cell, Vec::new()),
            None if deleted_by_theirs(rows_of, r) => {
                (ConflictRule::DeletedRow, vec![Region::Row(r.clone())])
            }
            None if deleted_by_theirs(cols_of, c) => {
                (ConflictRule::DeletedCol, vec![Region::Col(c.clone())])
            }
            None => {
                ch.cell(&mut w, r, c, v.as_deref());
                continue;
            }
        };
        conflicts.push(Conflict {
            rule,
            at: Region::Cell {
                row: r.clone(),
                col: c.clone(),
            },
            base: held.0,
            ours: held.1,
            theirs: held.2,
            theirs_writes,
        });
        if file_wins {
            // the file's value, after the live one (its row or column restored above)
            ch.cell(&mut w, r, c, v.as_deref());
        } else {
            held_cells.insert((r.clone(), c.clone()));
        }
    }

    // an outside save's rewritten rows: the file's row whole, over every live cell change
    if file_wins {
        for k in whole.keys() {
            // the file's columns, not the live side's, which would miss a column the live side
            // deleted
            let file_cols: Vec<Key> = if planted_whole_row {
                // (xxxii), planted: the live side's live columns
                theirs.cols.live().cloned().collect()
            } else {
                // the file's kept columns the result knows: live (one the file brings back
                // included), or one the live side deleted, whose hidden cell takes the file's
                // value too (the file's row wins whole; restoring the column shows it). Only
                // the columns of the cells the live side changed in this row can take part
                // (below), in ours' column order: O(those cells), whatever the width
                let mut at: Vec<(usize, Key)> = cells
                    .1
                    .range((k.clone(), Key::lowest())..)
                    .take_while(|((r, _), _)| r == k)
                    .filter(|((_, c), _)| w.cols.knows(c))
                    .filter_map(|((_, c), _)| ours_cols_at.get(c).map(|&i| (i, c.clone())))
                    .collect();
                at.sort_unstable();
                at.into_iter().map(|(_, c)| c).collect()
            };
            for c in &file_cols {
                let at = (k.clone(), c.clone());
                if cells.0.contains_key(&at) || !cells.1.contains_key(&at) {
                    continue; // the file's own change (above), or a cell live didn't change
                }
                let (o, t) = (ours.cell(k, c), theirs.cell(k, c));
                if o == t || !ours.cols.is_live(c) || !carried(&w, Some(k), c) {
                    continue;
                }
                // a live switch between empty and absent that is no live edit of the cell
                // (`neutral_live`): the file's, no override
                if neutral_live(k, c) || file_renders_live(k, c) {
                    ch.cell(&mut w, k, c, o);
                    continue;
                }
                conflicts.push(Conflict {
                    rule: ConflictRule::Cell,
                    at: Region::Cell {
                        row: k.clone(),
                        col: c.clone(),
                    },
                    base: value(base.cell(k, c)),
                    ours: value(o),
                    theirs: value(t),
                    theirs_writes: Vec::new(),
                });
                ch.cell(&mut w, k, c, o);
            }
        }
    }

    // quoting: follows its field; skipped where the field is in conflict
    for (at, q) in &quotes.0 {
        let skip = match at {
            QuoteAt::Cell(r, c) => {
                held_cells.contains(&(r.clone(), c.clone()))
                    || (!file_wins && whole.contains_key(r))
            }
            QuoteAt::Header(c) => held_names.contains(c),
        };
        let (r, c) = match at {
            QuoteAt::Cell(r, c) => (Some(r), c),
            QuoteAt::Header(c) => (None, c),
        };
        if skip || !carried(&w, r, c) {
            continue;
        }
        match quotes.1.get(at) {
            Some(tq) if tq == q => {}
            Some(_) => {
                let spell = |t: &Table| Held::Value(t.quote(at).map(|q| q.spelling().to_owned()));
                conflicts.push(Conflict {
                    rule: ConflictRule::Quoting,
                    at: Region::Quote(at.clone()),
                    base: spell(base),
                    ours: spell(ours),
                    theirs: spell(theirs),
                    theirs_writes: Vec::new(),
                });
                if file_wins {
                    ch.quote(&mut w, at.clone(), *q);
                }
            }
            None => ch.quote(&mut w, at.clone(), *q),
        }
    }

    // an outside save's rewritten rows: the file's quoting too, over a live change the file
    // didn't make itself
    if file_wins {
        for at in quotes.1.keys() {
            let QuoteAt::Cell(r, c) = at else { continue };
            if !whole.contains_key(r)
                || quotes.0.contains_key(at)
                || !ours.cols.is_live(c)
                || !carried(&w, Some(r), c)
            {
                continue;
            }
            // (how a field is quoted is no live edit: no report)
            let o = ours.quote(at);
            if o == theirs.quote(at) {
                continue;
            }
            ch.quote(&mut w, at.clone(), o);
        }
    }

    // settings: two-valued, so both sides changing one agree
    if ours.header != base.header && theirs.header == base.header {
        ch.meta(&mut w, "header", if ours.header { "on" } else { "off" });
    }
    if ours.shape != base.shape && theirs.shape == base.shape {
        ch.meta(&mut w, "shape", ours.shape.spelling());
    }

    Ok(Merge {
        edits: ch.finish(&mut w),
        conflicts,
        rekeyed,
        moved_then_deleted: moved_then_deleted.into_inner(),
    })
}

fn same_cols_bt(a: &Table, b: &Table) -> bool {
    a.live_cols() == b.live_cols()
}

fn rows_of(t: &Table) -> &Axis {
    &t.rows
}

fn cols_of(t: &Table) -> &Axis {
    &t.cols
}

fn axis_of<'a>(t: &'a Table, root: &str) -> &'a Axis {
    if root == ROWS {
        &t.rows
    } else {
        &t.cols
    }
}

/// An outside save's structure on one axis of `w` (theirs, [`merge3_outside`]): deletes,
/// restores, then each placed element (insert, move, restore) right after its left neighbour
/// in ours' order that is live in the result (or at the start), left to right: the anchors the
/// aligner gives a file's own edits ([`super::reconcile`]), so a moved row lands where the
/// file has it relative to the rows around it, and before any deleted row kept there. One
/// already there is left alone.
fn place_after(
    ch: &mut Change,
    w: &mut Table,
    root: &'static str,
    ours: &Axis,
    plan: &Plan,
) -> Result<(), TableError> {
    for k in &plan.delete {
        ch.list(w, root, ListOp::Delete { key: k.clone() })?;
    }
    for k in &plan.restore {
        ch.list(w, root, ListOp::Restore { key: k.clone() })?;
    }
    let mut left: Option<Key> = None;
    for k in ours.live() {
        if !plan.placed.contains(k) {
            if axis_of(w, root).is_live(k) {
                left = Some(k.clone());
            }
            continue;
        }
        let at = left.clone().map_or(Anchor::Start, Anchor::After);
        if plan.fresh.contains(k) {
            ch.list(
                w,
                root,
                ListOp::Insert {
                    key: k.clone(),
                    at,
                    value: Value::Null,
                },
            )?;
        } else if axis_of(w, root).next_live_after(left.as_ref()) != Some(k)
            || plan.restore.contains(k)
        {
            ch.list(
                w,
                root,
                ListOp::Move {
                    key: k.clone(),
                    to: at,
                },
            )?;
        }
        left = Some(k.clone());
    }
    Ok(())
}

/// Ours' clean structure on one axis of `w` (theirs): deletes, restores, then ours'
/// placements (see the module docs): each run of consecutive placed elements in ours' order
/// goes right before the nearest element after it in ours' order that is live in the result
/// (or at the end), as one forward chain; a run already there is left alone. Runs are placed
/// right to left, so that element is always where it ends up.
fn place(
    ch: &mut Change,
    w: &mut Table,
    root: &'static str,
    ours: &Axis,
    plan: &Plan,
) -> Result<(), TableError> {
    for k in &plan.delete {
        ch.list(w, root, ListOp::Delete { key: k.clone() })?;
    }
    for k in &plan.restore {
        ch.list(w, root, ListOp::Restore { key: k.clone() })?;
    }
    let order: Vec<&Key> = ours.live().collect();
    let mut right: Option<Key> = None;
    let mut j = order.len();
    while j > 0 {
        let k = order[j - 1];
        if !plan.placed.contains(k) {
            if axis_of(w, root).is_live(k) {
                right = Some(k.clone());
            }
            j -= 1;
            continue;
        }
        let mut s = j - 1;
        while s > 0 && plan.placed.contains(order[s - 1]) {
            s -= 1;
        }
        let run = &order[s..j];
        let settled = run
            .iter()
            .all(|k| !plan.fresh.contains(*k) && !plan.restore.contains(*k))
            && {
                let live = axis_of(w, root).live_keys();
                let end = match &right {
                    Some(r) => live.iter().position(|x| x == r).expect("live"),
                    None => live.len(),
                };
                end >= run.len() && live[end - run.len()..end].iter().eq(run.iter().copied())
            };
        if !settled {
            let mut at = right.clone().map_or(Anchor::End, Anchor::Before);
            for k in run {
                let op = if plan.fresh.contains(*k) {
                    ListOp::Insert {
                        key: (*k).clone(),
                        at,
                        value: Value::Null,
                    }
                } else {
                    ListOp::Move {
                        key: (*k).clone(),
                        to: at,
                    }
                };
                ch.list(w, root, op)?;
                at = Anchor::After((*k).clone());
            }
        }
        right = Some(run[0].clone());
        j = s;
    }
    Ok(())
}
