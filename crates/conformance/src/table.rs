//! Table rules: the CSV contract's composition (§1.1: rows and columns as MovableLists keyed
//! by row and column keys, cells as a Map keyed `"<row key>\x1f<col key>"`), over
//! [`Tables`], and with [`Merged`] where a rule needs what a merge did (contract §7.1; engine
//! answer E3).
//!
//! - `table.cell_kept`: deleting a row or a column never touches cells: they stay, hidden,
//!   and come back with a restore.
//! - `table.cell_vs_row_delete`: a cell set concurrent with its row's delete: the row stays
//!   deleted, the value is kept (hidden), the merge that brings the set to the deleter's
//!   replica reports it (the kit's report, [`k2_mpc_base::table::hidden_cell_writes`], over
//!   the merge's stream, E3), and a restore shows it.
//! - `table.cell_vs_col_delete`: the same for a column.
//! - `table.sort_atomic`: a sort is one change; a replica that receives it sees all its moves
//!   or none.
//! - `table.block_contiguous`: two concurrent row block moves into one gap stay contiguous,
//!   and every cell stays on its row (a cell set concurrently too).
//! - `table.cell_lww`: concurrent sets of one cell: the higher `(lamport, agent)` wins (as
//!   `map.lww`).
//! - `table.row_identity`: a row sort, a column move and a cell set, all concurrent: every
//!   cell stays on its row and its column.
//! - `table.sort_vs_cell`: a cell set concurrent with a sort stays on its row.
//! - `table.row_move_keeps_cells` / `table.col_move_keeps_cells`: concurrent row (column)
//!   moves and a cell set: every cell stays on its row (column).
//! - `table.col_move_no_duplicate`: two concurrent moves of one column leave one copy.
//! - `table.delete_wins_restorable`: a row delete concurrent with a newer move of it, by the
//!   higher agent: deleted; its cells kept; a restore brings it back at the move's place
//!   with its cells.
//! - `table.insert_beside_moved_row`: a row inserted (with cells) after a row moved
//!   concurrently stays at the old spot; its neighbours don't move.
//! - `table.ragged_render`: a row added concurrently with a column renders by the kit's
//!   render rules (contract §1.5): `rect`, the full width (absent cells as empty fields);
//!   `ragged`, through the last present cell ([`ragged_render_with`] takes the renderer, so
//!   a broken one can be planted).
//!
//! The seed's cells name their cell ([`cell_value`]), so "every cell on its row and column"
//! is checked by rendering the grid.

use std::collections::BTreeMap;

use k2_mpc_base::elem::{chain, reorder, Anchor, Applied, ElemState, KeyedRoot, KeyedState};
use k2_mpc_base::table::{self as kit_table, Record, Shape, Table};
use k2_mpc_base::{Key, Value};

use crate::caps::{Merged, Tables, CELLS, COLS, ROWS};
use crate::grid::*;
use crate::{deviates, TextReplica};

/// Every table rule, by id.
pub const RULES: &[&str] = &[
    "table.cell_kept",
    "table.cell_vs_row_delete",
    "table.cell_vs_col_delete",
    "table.sort_atomic",
    "table.block_contiguous",
    "table.cell_lww",
    "table.row_identity",
    "table.sort_vs_cell",
    "table.row_move_keeps_cells",
    "table.col_move_keeps_cells",
    "table.col_move_no_duplicate",
    "table.delete_wins_restorable",
    "table.insert_beside_moved_row",
    "table.ragged_render",
];

/// The table rules that also need [`Merged`] ([`run_merged`]); the others run with
/// [`run`].
pub const MERGED_RULES: &[&str] = &[
    "table.cell_vs_row_delete",
    "table.cell_vs_col_delete",
    "table.sort_atomic",
];

/// Run rule `id` (one that needs only [`Tables`]) on engine `S`. An unknown id panics.
pub fn run<S: Tables>(id: &str) {
    run_as::<S>(id, deviates::<S::Engine>(id));
}

/// Run rule `id`'s conforming check, ignoring `S`'s deviation list.
pub fn run_strict<S: Tables>(id: &str) {
    run_as::<S>(id, false);
}

fn run_as<S: Tables>(id: &str, dev: bool) {
    match id {
        "table.cell_kept" => cell_kept::<S>(),
        "table.block_contiguous" => block_contiguous::<S>(),
        "table.cell_lww" => cell_lww::<S>(dev),
        "table.row_identity" => row_identity::<S>(),
        "table.sort_vs_cell" => sort_vs_cell::<S>(),
        "table.row_move_keeps_cells" => line_move_keeps_cells::<S>(ROWS),
        "table.col_move_keeps_cells" => line_move_keeps_cells::<S>(COLS),
        "table.col_move_no_duplicate" => col_move_no_duplicate::<S>(),
        "table.delete_wins_restorable" => delete_wins_restorable::<S>(),
        "table.insert_beside_moved_row" => insert_beside_moved_row::<S>(),
        "table.ragged_render" => ragged_render_with::<S>(Table::render),
        _ => panic!("no table rule {id:?} over Tables alone"),
    }
}

/// Run rule `id` (one of [`MERGED_RULES`]) on engine `S`. An unknown id panics.
pub fn run_merged<S: Tables + Merged>(id: &str) {
    match id {
        "table.cell_vs_row_delete" => cell_vs_line_delete::<S>(ROWS),
        "table.cell_vs_col_delete" => cell_vs_line_delete::<S>(COLS),
        "table.sort_atomic" => sort_atomic::<S>(),
        _ => panic!("no table rule {id:?} over Tables + Merged"),
    }
}

/// The cells a stream sets that are hidden on `r` now (their row or column is not live), by
/// cell key: the kit's report ([`kit_table::hidden_cell_writes`]) to a writer whose cell
/// landed in a deleted row (contract §1.6, E3), over this replica's readers.
pub fn hidden_cell_writes<S: Tables, W: Clone>(applied: &[Applied<W>], r: &S) -> Vec<String> {
    kit_table::hidden_cell_writes(applied, |root, k| {
        matches!(r.elem(root, k), ElemState::Live(_))
    })
    .iter()
    .map(|h| h.cell_key())
    .collect()
}

/// The kit's view of `r`'s table (its live rows and columns, and its cells), rendered as
/// `shape`.
pub fn table_of<S: Tables>(r: &S, shape: Shape) -> Table {
    let list = |root: &str| KeyedRoot::List {
        live: r.list(root),
        hidden: BTreeMap::new(),
        places: Vec::new(),
    };
    let map = |m: BTreeMap<String, Value>| KeyedRoot::Map(m);
    let cells = r
        .map(CELLS)
        .into_iter()
        .filter(|(k, _)| kit_table::parse_cell_key(k).is_some())
        .collect();
    let shape = match shape {
        Shape::Rect => "rect",
        _ => "ragged",
    };
    let meta = [("shape".to_owned(), Value::str(shape))]
        .into_iter()
        .collect();
    let state = KeyedState {
        roots: vec![
            (kit_table::ROWS.into(), list(ROWS)),
            (kit_table::COLS.into(), list(COLS)),
            (kit_table::CELLS.into(), map(cells)),
            (kit_table::NAMES.into(), map(BTreeMap::new())),
            (kit_table::QUOTING.into(), map(BTreeMap::new())),
            (kit_table::META.into(), map(meta)),
        ],
    };
    Table::from_keyed(&state).expect("the rules' tables read as tables")
}

/// Every live cell of `r` holds [`cell_value`] of its row and column, except `changed`
/// (row, col, value).
fn assert_cells_on_their_rows<S: Tables>(r: &S, changed: &[(&str, &str, &str)], what: &str) {
    let e = S::Engine::ENGINE;
    let map = r.map(CELLS);
    for (row, _) in r.list(ROWS) {
        for (col, _) in r.list(COLS) {
            let (row, col) = (row.as_str(), col.as_str());
            let want = changed
                .iter()
                .find(|(r, c, _)| *r == row && *c == col)
                .map(|(_, _, v)| v.to_string())
                .unwrap_or_else(|| cell_value(row, col));
            assert_eq!(
                map.get(&cell(row, col)).and_then(|v| v.as_str()),
                Some(want.as_str()),
                "{e}: {what}: cell {row}/{col}"
            );
        }
    }
}

/// `table.cell_kept`
pub fn cell_kept<S: Tables>() {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102], &seed_table("r1 r2 r3", "c1 c2 c3"));
    let all = g.r[0].map(CELLS);
    assert_eq!(all.len(), 9, "{e}: the seed");
    g.edit(0, vec![rows(vec![del("r2")])]);
    assert_eq!(order(&g.r[0], ROWS), "r1 r3", "{e}");
    assert_eq!(g.r[0].map(CELLS), all, "{e}: a row delete touched cells");
    g.edit(0, vec![cols(vec![del("c2")])]);
    assert_eq!(g.r[0].map(CELLS), all, "{e}: a column delete touched cells");
    g.sync_all();
    assert_converged(&g, "after the deletes");
    assert_eq!(
        g.r[1].map(CELLS),
        all,
        "{e}: the deletes, merged, touched cells"
    );
    g.edit(1, vec![rows(vec![res("r2")]), cols(vec![res("c2")])]);
    g.sync_all();
    assert_converged(&g, "after the restores");
    assert_eq!(
        order(&g.r[0], ROWS),
        "r1 r2 r3",
        "{e}: the row back in place"
    );
    assert_eq!(
        order(&g.r[0], COLS),
        "c1 c2 c3",
        "{e}: the column back in place"
    );
    assert_cells_on_their_rows(&g.r[0], &[], "restored with their cells");
}

/// `table.cell_vs_row_delete` (`line` = [`ROWS`]) and `table.cell_vs_col_delete` ([`COLS`]).
pub fn cell_vs_line_delete<S: Tables + Merged>(line: &str) {
    let e = S::Engine::ENGINE;
    let (gone, set_at) = if line == ROWS {
        ("r2", ("r2", "c1"))
    } else {
        ("c2", ("r1", "c2"))
    };
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2 r3", "c1 c2"));
    g.edit(0, vec![list_edit(line, vec![del(gone)])]);
    g.edit(1, vec![cells(vec![mset(&cell(set_at.0, set_at.1), "new")])]);
    // the deleter receives the set: the merge reports a write hidden by its delete
    let (y, x) = pair(&mut g.r, 1, 0);
    let stream = x.sync_applied(y);
    assert_eq!(
        hidden_cell_writes(&stream, &*x),
        vec![cell(set_at.0, set_at.1)],
        "{e}: the merge of a cell set into a deleted {line} line is reported: {stream:?}"
    );
    g.sync_all();
    assert_converged(&g, "cell set vs delete");
    for (i, r) in g.r.iter().enumerate() {
        assert_eq!(
            r.elem(line, &k(gone)),
            ElemState::Deleted,
            "{e}: replica {i}: the {line} line stays deleted"
        );
        assert_eq!(
            r.map(CELLS)
                .get(&cell(set_at.0, set_at.1))
                .and_then(|v| v.as_str()),
            Some("new"),
            "{e}: replica {i}: the set value is kept (hidden)"
        );
    }
    g.edit(2, vec![list_edit(line, vec![res(gone)])]);
    g.sync_all();
    assert_converged(&g, "after the restore");
    assert_eq!(order(&g.r[0], ROWS), "r1 r2 r3", "{e}");
    assert_eq!(order(&g.r[0], COLS), "c1 c2", "{e}");
    assert_cells_on_their_rows(
        &g.r[0],
        &[(set_at.0, set_at.1, "new")],
        "the restore shows the set value and the line's other cells",
    );
}

/// `table.sort_atomic`
pub fn sort_atomic<S: Tables + Merged>() {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102], &seed_table("r1 r2 r3 r4 r5 r6", "c1"));
    let old = ks("r1 r2 r3 r4 r5 r6");
    let new = ks("r6 r3 r5 r1 r4 r2");
    g.edit(0, vec![rows(reorder(&old, &new).expect("a permutation"))]);
    let changes = g.r[0].own_changes();
    let keys = |r: &S| -> Vec<Key> { r.list(ROWS).into_iter().map(|(k, _)| k).collect() };
    for (i, c) in changes.iter().enumerate() {
        g.r[1]
            .deliver(c)
            .unwrap_or_else(|r| panic!("{e}: change {i} refused: {r:?}"));
        let now = keys(&g.r[1]);
        assert!(
            now == old || now == new,
            "{e}: after {} of the sort's {} change(s), a replica sees part of it: {now:?}",
            i + 1,
            changes.len()
        );
    }
    assert_eq!(changes.len(), 1, "{e}: a sort is one change");
    assert_eq!(keys(&g.r[1]), new, "{e}: the sort arrived");
    assert_cells_on_their_rows(&g.r[1], &[], "after the sort");
}

/// `table.block_contiguous`
pub fn block_contiguous<S: Tables>() {
    let e = S::Engine::ENGINE;
    let seed = seed_table("r1 r2 r3 r4 r5 r6 r7 r8", "c1 c2");
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed);
    g.edit(0, vec![rows(chain(&ks("r1 r2 r3"), after("r8")))]);
    g.edit(1, vec![rows(chain(&ks("r4 r5 r6"), after("r8")))]);
    g.edit(2, vec![cells(vec![mset(&cell("r2", "c2"), "z")])]);
    g.sync_all();
    assert_converged(&g, "two block moves");
    let o = format!(" {} ", order(&g.r[0], ROWS));
    assert!(
        o.contains(" r1 r2 r3 ") && o.contains(" r4 r5 r6 "),
        "{e}: the blocks interleaved: {o}"
    );
    assert_cells_on_their_rows(&g.r[0], &[("r2", "c2", "z")], "after the block moves");
}

/// `table.cell_lww`: x (101) sets the cell after three changes of its own; y (102) once.
pub fn cell_lww<S: Tables>(dev: bool) {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2", "c1 c2"));
    g.bump(0, 3);
    g.edit(0, vec![cells(vec![mset(&cell("r1", "c1"), "x")])]);
    g.edit(1, vec![cells(vec![mset(&cell("r1", "c1"), "y")])]);
    g.sync_all();
    assert_converged(&g, "two sets of one cell");
    let r = &g.r[0];
    let v = r.map(CELLS).get(&cell("r1", "c1")).cloned();
    let w = r
        .map_winner(CELLS, &cell("r1", "c1"))
        .map(|w| S::agent_of(&w));
    let v = v.as_ref().and_then(|v| v.as_str());
    if dev {
        assert_eq!(
            (v, w),
            (Some("y"), Some(102)),
            "{e}: the pinned tie-break changed (y won by agent ID): re-check table.cell_lww"
        );
    } else {
        assert_eq!(
            (v, w),
            (Some("x"), Some(101)),
            "{e}: the higher (lamport, agent) set wins"
        );
    }
}

/// `table.row_identity`
pub fn row_identity<S: Tables>() {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2 r3 r4", "c1 c2 c3"));
    let sorted = ks("r4 r3 r2 r1");
    g.edit(
        0,
        vec![rows(
            reorder(&ks("r1 r2 r3 r4"), &sorted).expect("a permutation"),
        )],
    );
    g.edit(
        1,
        vec![cols(vec![mv("c3", k2_mpc_base::elem::Anchor::Start)])],
    );
    g.edit(2, vec![cells(vec![mset(&cell("r2", "c3"), "z")])]);
    g.sync_all();
    assert_converged(&g, "row sort, column move, cell set");
    assert_eq!(order(&g.r[0], ROWS), "r4 r3 r2 r1", "{e}");
    assert_eq!(order(&g.r[0], COLS), "c3 c1 c2", "{e}");
    assert_cells_on_their_rows(
        &g.r[0],
        &[("r2", "c3", "z")],
        "every cell on its row and column",
    );
}

/// `table.sort_vs_cell`
pub fn sort_vs_cell<S: Tables>() {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102], &seed_table("r1 r2 r3 r4", "c1 c2"));
    let sorted = ks("r3 r1 r4 r2");
    g.edit(
        0,
        vec![rows(
            reorder(&ks("r1 r2 r3 r4"), &sorted).expect("a permutation"),
        )],
    );
    g.edit(1, vec![cells(vec![mset(&cell("r2", "c1"), "y")])]);
    g.sync_all();
    assert_converged(&g, "a sort and a cell set");
    assert_eq!(order(&g.r[0], ROWS), "r3 r1 r4 r2", "{e}");
    assert_cells_on_their_rows(&g.r[0], &[("r2", "c1", "y")], "the set stays on its row");
}

/// `table.row_move_keeps_cells` (`line` = [`ROWS`]) and `table.col_move_keeps_cells`
/// ([`COLS`]): two concurrent moves on that line, and a cell set, all concurrent.
pub fn line_move_keeps_cells<S: Tables>(line: &str) {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2 r3 r4", "c1 c2 c3 c4"));
    let (a, b, set) = if line == ROWS {
        ("r1", "r3", ("r1", "c2"))
    } else {
        ("c1", "c3", ("r2", "c1"))
    };
    g.edit(0, vec![list_edit(line, vec![mv(a, Anchor::End)])]);
    g.edit(1, vec![list_edit(line, vec![mv(b, Anchor::Start)])]);
    g.edit(2, vec![cells(vec![mset(&cell(set.0, set.1), "z")])]);
    g.sync_all();
    assert_converged(&g, "concurrent moves");
    let o = order(&g.r[0], line);
    let mut keys: Vec<&str> = o.split(' ').collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), 4, "{e}: {line}: every line once: {o}");
    assert_cells_on_their_rows(&g.r[0], &[(set.0, set.1, "z")], "after concurrent moves");
}

/// `table.col_move_no_duplicate`
pub fn col_move_no_duplicate<S: Tables>() {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2", "c1 c2 c3 c4"));
    g.edit(0, vec![cols(vec![mv("c1", Anchor::End)])]);
    g.edit(1, vec![cols(vec![mv("c1", after("c2"))])]);
    g.sync_all();
    assert_converged(&g, "two moves of c1");
    let o = order(&g.r[0], COLS);
    assert_eq!(o.split(' ').filter(|x| *x == "c1").count(), 1, "{e}: {o}");
    assert_eq!(o.split(' ').count(), 4, "{e}: {o}");
    assert_cells_on_their_rows(&g.r[0], &[], "after two moves of one column");
}

/// `table.delete_wins_restorable`: the mover is the higher agent and has made more changes,
/// so its move is the newer write by every last-writer-wins order; the delete still wins.
pub fn delete_wins_restorable<S: Tables>() {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2 r3", "c1 c2"));
    g.bump(1, 3);
    g.edit(1, vec![rows(vec![mv("r2", Anchor::Start)])]);
    g.edit(0, vec![rows(vec![del("r2")])]);
    g.sync_all();
    assert_converged(&g, "delete vs a newer move");
    for (i, r) in g.r.iter().enumerate() {
        assert_eq!(
            r.elem(ROWS, &k("r2")),
            ElemState::Deleted,
            "{e}: replica {i}: r2 deleted"
        );
    }
    assert_eq!(order(&g.r[0], ROWS), "r1 r3", "{e}");
    assert_eq!(
        g.r[0].map(CELLS).get(&cell("r2", "c1")),
        Some(&Value::str(cell_value("r2", "c1"))),
        "{e}: the deleted row's cells are kept"
    );
    g.edit(2, vec![rows(vec![res("r2")])]);
    g.sync_all();
    assert_converged(&g, "after the restore");
    assert_eq!(
        order(&g.r[0], ROWS),
        "r2 r1 r3",
        "{e}: restored at the move's place"
    );
    assert_cells_on_their_rows(&g.r[0], &[], "restored with its cells");
}

/// `table.insert_beside_moved_row`
pub fn insert_beside_moved_row<S: Tables>() {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102, 103], &seed_table("r1 r2 r3 r4", "c1 c2"));
    g.edit(0, vec![rows(vec![mv("r2", Anchor::End)])]);
    g.edit(
        1,
        vec![
            rows(vec![ins("r5", after("r2"))]),
            cells(vec![
                mset(&cell("r5", "c1"), &cell_value("r5", "c1")),
                mset(&cell("r5", "c2"), &cell_value("r5", "c2")),
            ]),
        ],
    );
    g.sync_all();
    assert_converged(&g, "an insert beside a moved row");
    assert_eq!(order(&g.r[0], ROWS), "r1 r5 r3 r4 r2", "{e}: the old spot");
    assert_cells_on_their_rows(&g.r[0], &[], "the new row's cells on it");
}

/// `table.ragged_render`, rendering with `render` (the kit's: [`Table::render`]).
pub fn ragged_render_with<S: Tables>(render: fn(&Table) -> Vec<Record>) {
    let e = S::Engine::ENGINE;
    let mut g = Grid::<S>::new(&[101, 102], &seed_table("r1 r2", "c1 c2"));
    g.edit(0, vec![cols(vec![ins("c3", Anchor::End)])]);
    g.edit(
        1,
        vec![
            rows(vec![ins("r3", after("r2"))]),
            cells(vec![mset(&cell("r3", "c1"), "x")]),
        ],
    );
    g.sync_all();
    assert_converged(&g, "a row and a column added at once");
    let shown = |r: &S, shape| -> Vec<Vec<String>> {
        render(&table_of(r, shape))
            .into_iter()
            .map(|r| r.into_iter().map(|f| f.value).collect())
            .collect()
    };
    let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let (a1, a2, b1, b2) = (
        cell_value("r1", "c1"),
        cell_value("r1", "c2"),
        cell_value("r2", "c1"),
        cell_value("r2", "c2"),
    );
    assert_eq!(
        shown(&g.r[0], Shape::Rect),
        vec![v(&[&a1, &a2, ""]), v(&[&b1, &b2, ""]), v(&["x", "", ""])],
        "{e}: rect: every row renders the full width"
    );
    assert_eq!(
        shown(&g.r[0], Shape::Ragged),
        vec![v(&[&a1, &a2]), v(&[&b1, &b2]), v(&["x"])],
        "{e}: ragged: through the last present cell"
    );
    g.edit(0, vec![cells(vec![mset(&cell("r3", "c3"), "z")])]);
    g.sync_all();
    for shape in [Shape::Rect, Shape::Ragged] {
        assert_eq!(
            shown(&g.r[0], shape)[2],
            v(&["x", "", "z"]),
            "{e}: {shape:?}: an absent cell before a present one is an empty field"
        );
    }
}
