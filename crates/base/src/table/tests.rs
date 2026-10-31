//! Unit tests of the table module: the model and render rules, the plain applier, the grid
//! ops (K-CSV-10's `sort_is_forward_chain`, `block_move_is_forward_chain`,
//! `clear_is_set_empty`), the hidden-cell-writes report, the aligner, and one test per
//! `merge3` rule (K-BR-3's `table::merge3_*`) with the report's exact shape.

use super::*;
use crate::elem::{chain, Effect, MapApplied};

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn ks(s: &str) -> Vec<Key> {
    s.split(' ').filter(|x| !x.is_empty()).map(k).collect()
}

fn keys(v: &[Key]) -> String {
    v.iter().map(Key::as_str).collect::<Vec<_>>().join(" ")
}

fn values(recs: &[Record]) -> Vec<Vec<String>> {
    recs.iter()
        .map(|r| r.iter().map(|f| f.value.clone()).collect())
        .collect()
}

fn vals(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect()
}

fn table(rows: &[&[&str]], header: bool, shape: Shape) -> Table {
    let (t, edits) = Table::import(&records(rows), header, shape).unwrap();
    let mut again = Table::new(header, shape);
    again.apply(&edits).unwrap();
    assert_eq!(again, t, "the import's edits build the table it returns");
    t
}

fn list_ops<'a>(edits: &'a [Edit], root: &str) -> &'a [ListOp] {
    edits
        .iter()
        .find_map(|e| match e {
            Edit::List { root: r, ops } if r == root => Some(ops.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

fn map_ops<'a>(edits: &'a [Edit], root: &str) -> &'a [MapOp] {
    edits
        .iter()
        .find_map(|e| match e {
            Edit::Map { root: r, ops } if r == root => Some(ops.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

/// Every move after the first of a run is anchored right after the previous one's key.
fn assert_forward_chain(ops: &[ListOp]) {
    for w in ops.windows(2) {
        match (&w[0], &w[1]) {
            (ListOp::Move { key, .. }, ListOp::Move { to, .. }) => {
                assert_eq!(
                    to,
                    &Anchor::After(key.clone()),
                    "not a forward chain: {ops:?}"
                )
            }
            other => panic!("not moves: {other:?}"),
        }
    }
}

// ---------- the model ----------

#[test]
fn keys_quote_places_and_values() {
    let at = cell_key(&k("r12"), &k("c3"));
    assert_eq!(at, "r12\u{1f}c3");
    assert_eq!(parse_cell_key(&at), Some((k("r12"), k("c3"))));
    assert_eq!(parse_cell_key("bump"), None);
    assert_eq!(parse_cell_key("R1\u{1f}c1"), None);
    for q in [QuoteAt::Header(k("c2")), QuoteAt::Cell(k("r1"), k("c2"))] {
        assert_eq!(QuoteAt::parse(&q.map_key()), Some(q));
    }
    assert_eq!(QuoteAt::Header(k("c2")).map_key(), "\u{1e}c2");
    assert!(check_value("any \"text\", even\nlines").is_ok());
    assert_eq!(check_value("a\0b"), Err(ValueError::Nul { at: 1 }));
    assert!(check_value(&"x".repeat(MAX_VALUE_LEN)).is_ok());
    assert!(matches!(
        check_value(&"x".repeat(MAX_VALUE_LEN + 1)),
        Err(ValueError::TooLong { .. })
    ));
    assert_eq!(ROOTS.len(), 6);
}

#[test]
fn shape_is_detected_from_non_blank_records() {
    assert_eq!(
        Shape::detect(&records(&[&["a", "b"], &[], &["c", "d"]])),
        Shape::Rect
    );
    assert_eq!(
        Shape::detect(&records(&[&["a", "b"], &["c"]])),
        Shape::Ragged
    );
    assert_eq!(Shape::detect(&[]), Shape::Rect);
}

#[test]
fn an_import_renders_back_its_records() {
    let cases: &[(&[&[&str]], bool, Shape)] = &[
        (
            &[&["name", "city"], &["Ana", "Oslo"], &["Ben", ""]],
            true,
            Shape::Rect,
        ),
        (
            &[&["a", "b", "c"], &["x"], &[], &["", "", "z"]],
            false,
            Shape::Ragged,
        ),
        (&[&["a"], &[], &["b"]], false, Shape::Rect),
        (&[], true, Shape::Rect),
        (&[&["h1", "h2"]], true, Shape::Rect),
    ];
    for (rows, header, shape) in cases {
        let t = table(rows, *header, *shape);
        assert_eq!(t.render(), records(rows), "{rows:?}");
    }
    // quote exceptions travel with their fields
    let mut recs = records(&[&["a", "b"], &["1", "2"]]);
    recs[0][1].quote = Some(Quote::Quoted);
    recs[1][0].quote = Some(Quote::Plain);
    let (t, _) = Table::import(&recs, true, Shape::Rect).unwrap();
    assert_eq!(t.render(), recs);
    assert_eq!(t.quoting.len(), 2);
}

#[test]
fn import_mints_keys_in_file_order_and_counts_on() {
    let t = table(&[&["a", "b"], &["1", "2"], &["3"]], true, Shape::Ragged);
    assert_eq!(keys(&t.live_rows()), "r1 r2");
    assert_eq!(keys(&t.live_cols()), "c1 c2");
    assert_eq!((t.next_row, t.next_col), (3, 3));
    assert_eq!(t.name(&k("c2")), Some("b"));
    assert_eq!(t.cell(&k("r2"), &k("c1")), Some("3"));
    assert_eq!(
        t.cell(&k("r2"), &k("c2")),
        None,
        "a short ragged row: absent"
    );
}

/// Contract §1.5's example.
#[test]
fn render_rules_rect_and_ragged() {
    let mut t = table(&[&["a", "b", "c"], &["x", "", ""], &[]], false, Shape::Rect);
    // make r2's b and c absent (they were present and empty at import)
    t.apply(&[Edit::Map {
        root: CELLS.into(),
        ops: vec![
            MapOp::Delete {
                key: cell_key(&k("r2"), &k("c2")),
            },
            MapOp::Delete {
                key: cell_key(&k("r2"), &k("c3")),
            },
        ],
    }])
    .unwrap();
    assert_eq!(
        values(&t.render()),
        vals(&[&["a", "b", "c"], &["x", "", ""], &[]])
    );
    t.shape = Shape::Ragged;
    assert_eq!(values(&t.render()), vals(&[&["a", "b", "c"], &["x"], &[]]));
    // a row with only a middle cell, ragged: empty fields up to it
    t.apply(&[Edit::Map {
        root: CELLS.into(),
        ops: vec![MapOp::Delete {
            key: cell_key(&k("r2"), &k("c1")),
        }],
    }])
    .unwrap();
    t.set_cell(&k("r2"), &k("c2"), "m").unwrap();
    assert_eq!(values(&t.render())[1], vec!["", "m"]);
    // deleted rows and columns don't render
    t.delete_col(&k("c1")).unwrap();
    t.delete_row(&k("r1")).unwrap();
    assert_eq!(values(&t.render()), vals(&[&["m"], &[]]));
}

#[test]
fn the_header_renders_by_the_same_rule() {
    let mut t = table(&[&["a", "b"], &["1", "2"]], true, Shape::Rect);
    let (_, _) = t.insert_col(Anchor::End, None).unwrap();
    assert_eq!(
        values(&t.render()),
        vals(&[&["a", "b", ""], &["1", "2", ""]])
    );
    t.shape = Shape::Ragged;
    assert_eq!(values(&t.render()), vals(&[&["a", "b"], &["1", "2"]]));
}

/// C1k (request 4): the header line is written only when the header is on and the table has
/// a live row or a name on a live column, as the codec writes it.
#[test]
fn the_header_line_is_written_only_with_a_row_or_a_name() {
    let mut t = table(&[&["a", "b"], &["1", "2"]], true, Shape::Rect);
    assert!(t.header_renders());
    // no rows, names: the header line alone
    t.delete_row(&k("r1")).unwrap();
    assert_eq!(values(&t.render()), vals(&[&["a", "b"]]));
    assert_eq!(t.line_map().lines, vec![LineOwner::Header]);
    // no rows, no names on the live columns (a column without a name added): nothing
    let mut u = Table::new(true, Shape::Rect);
    u.insert_col(Anchor::End, None).unwrap();
    u.insert_col(Anchor::End, None).unwrap();
    assert!(!u.header_renders());
    assert!(u.render().is_empty(), "no header record: {:?}", u.render());
    assert!(u.line_map().lines.is_empty());
    // a row (even a blank one) makes it render: a blank header line, then the row
    let (r, _) = u.insert_row(Anchor::Start, &[]).unwrap();
    assert!(u.header_renders());
    assert_eq!(u.render(), vec![Vec::new(), Vec::new()]);
    assert_eq!(
        u.line_map().lines,
        vec![LineOwner::Header, LineOwner::Row(r)]
    );
    // a name on a deleted column doesn't count
    let mut v = table(&[&["a"], &["1"]], true, Shape::Rect);
    v.delete_row(&k("r1")).unwrap();
    v.delete_col(&k("c1")).unwrap();
    assert!(!v.header_renders() && v.render().is_empty());
}

/// C1k (request 4): a lone blank line is no header, at an import and at an outside save (the
/// codec reads it so): header off, one blank row.
#[test]
fn a_lone_blank_line_is_no_header() {
    let t = table(&[&[]], true, Shape::Rect);
    assert!(!t.header, "imported header off");
    assert_eq!(t.render(), vec![Vec::new()]);
    assert_eq!(t.live_rows().len(), 1);
    assert!(t.live_cols().is_empty());
    // an outside save to one blank line switches a header-on table's header off
    let t = table(&[&["a", "b"], &["1", "2"]], true, Shape::Rect);
    let (r, u) = reconciled(&t, &[&[]]);
    assert!(!u.header);
    assert_eq!(u.render(), vec![Vec::new()]);
    assert!(r
        .edits
        .iter()
        .any(|e| matches!(e, Edit::Map { root, .. } if root == META)));
    // a blank header line with rows after it stays a header
    let (_, u) = reconciled(&t, &[&[], &["1", "2"]]);
    assert!(u.header);
    assert_eq!(u.name(&k("c1")), None);
}

#[test]
fn line_map_counts_line_breaks_in_values() {
    let t = table(
        &[&["h"], &["one"], &["two\nlines"], &["crlf\r\nand\rcr"], &[]],
        true,
        Shape::Rect,
    );
    let m = t.line_map();
    let o: Vec<String> = m
        .lines
        .iter()
        .map(|o| match o {
            LineOwner::Header => "h".into(),
            LineOwner::Row(r) => r.to_string(),
            LineOwner::Preamble => "p".into(),
        })
        .collect();
    assert_eq!(o.join(" "), "h r1 r2 r2 r3 r3 r3 r4");
    assert_eq!(m.row_at(1), None, "the header");
    assert_eq!(m.row_at(4), Some(&k("r2")));
    assert_eq!(m.row_at(9), None, "past the end");
    assert_eq!(m.lines_of(&k("r3")), Some(5..8));
    assert_eq!(m.owner(0), None);
}

/// C1k (request 6): a codec's own line is the preamble's, no row's and no header's.
#[test]
fn a_preamble_line_belongs_to_no_row() {
    let t = table(&[&["h"], &["one"]], true, Shape::Rect);
    let mut m = t.line_map();
    m.lines.insert(0, LineOwner::Preamble);
    assert_eq!(m.owner(1), Some(&LineOwner::Preamble));
    assert_eq!(m.row_at(1), None);
    assert_eq!(m.owner(2), Some(&LineOwner::Header));
    assert_eq!(m.row_at(3), Some(&k("r1")));
    assert_eq!(m.lines_of(&k("r1")), Some(3..4));
}

// ---------- the plain applier ----------

fn rows_op(ops: Vec<ListOp>) -> Edit {
    Edit::List {
        root: ROWS.into(),
        ops,
    }
}

#[test]
fn apply_follows_the_list_rules() {
    let mut t = table(&[&["a"], &["b"], &["c"], &["d"]], false, Shape::Rect);
    let mv = |key: &str, to: Anchor| ListOp::Move { key: k(key), to };
    // a deleted element keeps its place: a restore puts it back there
    t.apply(&[rows_op(vec![ListOp::Delete { key: k("r2") }])])
        .unwrap();
    assert_eq!(keys(&t.live_rows()), "r1 r3 r4");
    t.apply(&[rows_op(vec![ListOp::Restore { key: k("r2") }])])
        .unwrap();
    assert_eq!(keys(&t.live_rows()), "r1 r2 r3 r4");
    // a move of a deleted element moves where it comes back
    t.apply(&[rows_op(vec![
        ListOp::Delete { key: k("r1") },
        mv("r1", Anchor::End),
    ])])
    .unwrap();
    assert_eq!(keys(&t.live_rows()), "r2 r3 r4");
    t.apply(&[rows_op(vec![ListOp::Restore { key: k("r1") }])])
        .unwrap();
    assert_eq!(keys(&t.live_rows()), "r2 r3 r4 r1");
    // an anchor may be deleted: the insert lands at its place
    t.apply(&[rows_op(vec![ListOp::Delete { key: k("r3") }])])
        .unwrap();
    t.apply(&[rows_op(vec![ListOp::Insert {
        key: k("r9"),
        at: Anchor::After(k("r3")),
        value: Value::Null,
    }])])
    .unwrap();
    assert_eq!(keys(&t.live_rows()), "r2 r9 r4 r1");
    // refusals: all or nothing
    let before = t.clone();
    let bad = [
        rows_op(vec![
            mv("r2", Anchor::End),
            ListOp::Insert {
                key: k("r3"),
                at: Anchor::End,
                value: Value::Null,
            },
        ]),
        rows_op(vec![mv("r2", Anchor::After(k("r2")))]),
        rows_op(vec![mv("r77", Anchor::End)]),
        rows_op(vec![mv("r2", Anchor::After(k("r77")))]),
        Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Set {
                key: "not a cell".into(),
                value: Value::str("x"),
            }],
        },
        Edit::Map {
            root: QUOTING.into(),
            ops: vec![MapOp::Set {
                key: cell_key(&k("r2"), &k("c1")),
                value: Value::str("z"),
            }],
        },
    ];
    for e in &bad {
        assert!(t.apply(std::slice::from_ref(e)).is_err(), "{e:?}");
        assert_eq!(t, before, "a refused edit changed the table: {e:?}");
    }
    assert!(matches!(
        t.apply(&[rows_op(vec![ListOp::Insert {
            key: k("r3"),
            at: Anchor::End,
            value: Value::Null
        }])]),
        Err(TableError::KeyExists { .. })
    ));
}

#[test]
fn minting_never_reuses_a_key() {
    let mut t = table(&[&["a"], &["b"]], false, Shape::Rect);
    t.next_row = 1; // a counter behind the keys in use
    let (r, _) = t.insert_row(Anchor::End, &[]).unwrap();
    assert_eq!(r, k("r3"));
    // a table read without counters starts past every key, tagged keys included
    let mut state = KeyedState::default();
    for (root, kind) in ROOTS {
        state.roots.push((
            root.to_string(),
            match kind {
                Kind::MovableList if *root == ROWS => KeyedRoot::List {
                    live: vec![(k("r4"), Value::Null), (k("r9.w2"), Value::Null)],
                    hidden: [(k("r12"), Value::Null)].into_iter().collect(),
                    places: Vec::new(),
                },
                Kind::MovableList => KeyedRoot::List {
                    live: Vec::new(),
                    hidden: BTreeMap::new(),
                    places: Vec::new(),
                },
                _ => KeyedRoot::Map(BTreeMap::new()),
            },
        ));
    }
    let t = Table::from_keyed(&state).unwrap();
    assert_eq!((t.next_row, t.next_col), (13, 1));
    assert_eq!(keys(&t.live_rows()), "r4 r9.w2");
    assert_eq!(t.rows.state(&k("r12")), ElemState::Deleted);
    assert!(!t.header, "header off by default");
}

// ---------- grid ops ----------

#[test]
fn clear_is_set_empty() {
    let mut t = table(&[&["a", "b"]], false, Shape::Rect);
    let e = t.clear_cell(&k("r1"), &k("c2")).unwrap();
    assert_eq!(
        map_ops(&e, CELLS),
        &[MapOp::Set {
            key: cell_key(&k("r1"), &k("c2")),
            value: Value::str("")
        }]
    );
    assert_eq!(t.cell(&k("r1"), &k("c2")), Some(""));
    // a block clear sets "" too, and only where there's something to clear
    let mut t = table(&[&["a", "b", "c"], &["d", "", "f"]], false, Shape::Rect);
    let e = t
        .clear_block((&k("r2"), &k("c2")), (&k("r1"), &k("c1")))
        .unwrap();
    let ops = map_ops(&e, CELLS);
    assert_eq!(ops.len(), 3, "{ops:?}");
    assert!(ops
        .iter()
        .all(|o| matches!(o, MapOp::Set { value, .. } if *value == Value::str(""))));
    assert_eq!(values(&t.render()), vals(&[&["", "", "c"], &["", "", "f"]]));
}

#[test]
fn cell_set_checks_what_the_verb_checks() {
    let mut t = table(&[&["a", "b"], &["c", "d"]], false, Shape::Rect);
    t.delete_row(&k("r2")).unwrap();
    assert!(matches!(
        t.set_cell(&k("r2"), &k("c1"), "x"),
        Err(TableError::Deleted { root: ROWS, .. })
    ));
    assert!(matches!(
        t.set_cell(&k("r7"), &k("c1"), "x"),
        Err(TableError::Unknown { root: ROWS, .. })
    ));
    assert!(matches!(
        t.set_cell(&k("r1"), &k("c1"), "a\0"),
        Err(TableError::Value(ValueError::Nul { .. }))
    ));
    assert_eq!(
        t.set_cell(&k("r1"), &k("c1"), "a").unwrap(),
        vec![],
        "no change"
    );
    let e = t.set_cell(&k("r1"), &k("c1"), "z").unwrap();
    assert_eq!(e.len(), 1);
}

#[test]
fn sort_is_forward_chain() {
    let mut t = table(
        &[&["n"], &["30"], &["10"], &["x"], &["20"], &[""], &["10"]],
        true,
        Shape::Rect,
    );
    let by = |numeric, desc| {
        vec![SortKey {
            col: k("c1"),
            desc,
            numeric,
        }]
    };
    let e = t.clone().sort(&by(false, false)).unwrap();
    assert_forward_chain(list_ops(&e, ROWS));
    assert_eq!(e.len(), 1, "moves only: {e:?}");
    let mut s = t.clone();
    s.sort(&by(false, false)).unwrap();
    assert_eq!(keys(&s.live_rows()), "r5 r2 r6 r4 r1 r3", "bytes, stable");
    let mut s = t.clone();
    let e = s.sort(&by(true, false)).unwrap();
    assert_forward_chain(list_ops(&e, ROWS));
    assert_eq!(
        keys(&s.live_rows()),
        "r2 r6 r4 r1 r3 r5",
        "numeric: non-numeric and empty last, ties stable"
    );
    let mut s = t.clone();
    s.sort(&by(true, true)).unwrap();
    assert_eq!(
        keys(&s.live_rows()),
        "r1 r4 r2 r6 r3 r5",
        "desc keeps ties' order"
    );
    assert_eq!(
        s.clone().sort(&by(true, true)).unwrap(),
        vec![],
        "sorted: no edit"
    );
    assert!(matches!(t.sort(&[]), Err(TableError::Usage(_))));
    assert_eq!(values(&s.render())[0], vec!["n"], "the header stays");
}

#[test]
fn block_move_is_forward_chain() {
    let mut t = table(
        &[&["a"], &["b"], &["c"], &["d"], &["e"]],
        false,
        Shape::Rect,
    );
    let e = t
        .move_rows(&ks("r4 r1 r2"), Anchor::After(k("r5")))
        .unwrap();
    assert_eq!(
        list_ops(&e, ROWS),
        chain(&ks("r4 r1 r2"), Anchor::After(k("r5"))).as_slice()
    );
    assert_eq!(keys(&t.live_rows()), "r3 r5 r4 r1 r2");
    assert!(matches!(
        t.move_rows(&ks("r1 r1"), Anchor::End),
        Err(TableError::Usage(_))
    ));
    assert!(matches!(
        t.move_rows(&ks("r1 r2"), Anchor::After(k("r2"))),
        Err(TableError::SelfAnchor { .. })
    ));
    assert_eq!(
        t.move_rows(&ks("r1 r2"), Anchor::End).unwrap(),
        vec![],
        "in place"
    );
    // columns too
    let e = t.move_cols(&ks("c1"), Anchor::End).unwrap();
    assert_eq!(e, vec![], "one column: already at the end");
}

#[test]
fn insert_restore_rename_header() {
    let mut t = table(&[&["a", "b"], &["1", "2"]], true, Shape::Rect);
    assert!(matches!(
        t.insert_row(Anchor::Start, &["x".into(), "y".into(), "z".into()]),
        Err(TableError::TooManyValues { values: 3, cols: 2 })
    ));
    let (r, e) = t.insert_row(Anchor::Start, &["x".into()]).unwrap();
    assert_eq!(r, k("r2"));
    assert!(map_ops(&e, META).iter().any(
        |o| matches!(o, MapOp::Set { key, value } if key == "next_row" && *value == Value::str("3"))
    ));
    assert_eq!(
        values(&t.render()),
        vals(&[&["a", "b"], &["x", ""], &["1", "2"]])
    );
    // delete and restore: back where it was, with its cells
    t.delete_row(&k("r1")).unwrap();
    assert!(matches!(
        t.delete_row(&k("r1")),
        Err(TableError::Deleted { .. })
    ));
    t.restore_row(&k("r1")).unwrap();
    assert_eq!(t.restore_row(&k("r1")).unwrap(), vec![], "already live");
    assert_eq!(values(&t.render())[2], vec!["1", "2"]);
    let (c, _) = t.insert_col(Anchor::After(k("c1")), Some("new")).unwrap();
    assert_eq!(c, k("c3"));
    t.rename_col(&k("c2"), "bee").unwrap();
    assert_eq!(values(&t.render())[0], vec!["a", "new", "bee"]);
    // header switches change nothing rendered
    let before = t.render();
    let off = t.set_header(false).unwrap();
    assert!(!off.is_empty());
    assert_eq!(t.render(), before, "header off: same records");
    assert!(t.names.is_empty());
    assert!(matches!(
        t.rename_col(&k("c1"), "x"),
        Err(TableError::HeaderOff)
    ));
    t.set_header(true).unwrap();
    assert_eq!(t.render(), before, "header on again: same records");
    assert_eq!(t.set_header(true).unwrap(), vec![]);
}

/// C1k (request 3): the header switch never changes the rendering, as the codec's rule.
#[test]
fn the_header_switch_follows_the_codec_rule() {
    // off → on refused where the only live row is blank; nothing applied
    let mut t = table(&[&["a"], &[]], false, Shape::Rect);
    t.delete_row(&k("r1")).unwrap();
    let before = t.clone();
    assert_eq!(t.set_header(true), Err(TableError::HeaderOnlyRowBlank));
    assert_eq!(t, before);
    // a blank first row with others after it: allowed (a blank header line, then rows)
    let mut t = table(&[&[], &["b"]], false, Shape::Rect);
    let r = t.render();
    t.set_header(true).unwrap();
    assert_eq!(t.render(), r);
    // on → off where no header line is written: the setting only, no row
    let mut t = Table::new(true, Shape::Rect);
    t.insert_col(Anchor::End, None).unwrap();
    let e = t.set_header(false).unwrap();
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(!t.header && t.live_rows().is_empty() && t.render().is_empty());
    // on → off with names and no rows: the names become a row (the header line was written)
    let mut t = table(&[&["a", "b"]], true, Shape::Rect);
    let r = t.render();
    t.set_header(false).unwrap();
    assert_eq!(t.render(), r);
    assert_eq!(t.live_rows().len(), 1);
    // off → on with no rows: the setting only
    let mut t = Table::new(false, Shape::Rect);
    t.insert_col(Anchor::End, None).unwrap();
    t.set_header(true).unwrap();
    assert!(t.header && t.render().is_empty());
    // off → on with no rows where live columns kept their names (the header switched off
    // while no line was written): refused, nothing applied; the names stay (pt at 10k, the
    // seed in tests/table.proptest-regressions)
    let mut t = table(&[&["", "x,y"]], true, Shape::Rect);
    t.delete_col(&k("c1")).unwrap();
    t.delete_col(&k("c2")).unwrap();
    t.set_header(false).unwrap();
    t.restore_col(&k("c1")).unwrap();
    t.restore_col(&k("c2")).unwrap();
    assert!(!t.header && t.render().is_empty(), "{:?}", t.render());
    let before = t.clone();
    assert_eq!(t.set_header(true), Err(TableError::HeaderNamesUnrendered));
    assert_eq!(t, before);
}

#[test]
fn the_ops_keep_the_view_in_step() {
    // what a builder did to the table equals applying its edits to a copy
    let t0 = table(
        &[&["a", "b"], &["1", "2"], &["3", "4"]],
        true,
        Shape::Ragged,
    );
    let mut t = t0.clone();
    let mut all = Vec::new();
    all.extend(t.set_cell(&k("r1"), &k("c2"), "x").unwrap());
    all.extend(t.insert_row(Anchor::End, &["5".into()]).unwrap().1);
    all.extend(t.move_row(&k("r2"), Anchor::Start).unwrap());
    all.extend(t.delete_col(&k("c1")).unwrap());
    all.extend(
        t.set_block(&k("r1"), &k("c2"), &vec![vec!["p".to_string()]; 4])
            .unwrap(),
    );
    let mut u = t0.clone();
    u.apply(&all).unwrap();
    assert_eq!(u, t);
    assert_eq!(
        values(&t.render()),
        vals(&[&["b"], &["4"], &["p"], &["p"], &["p"], &["p"]])
    );
}

// ---------- hidden cell writes ----------

#[test]
fn hidden_cell_writes_reports_writes_in_deleted_lines() {
    let mut t = table(&[&["a", "b"], &["c", "d"]], false, Shape::Rect);
    t.delete_row(&k("r2")).unwrap();
    t.delete_col(&k("c2")).unwrap();
    let set = |key: String, w: u32| MapApplied {
        kind: MapOpKind::Set,
        key,
        write: w,
        before: None,
        after: Some(Value::str("v")),
        effect: Effect::Visible,
    };
    let stream = vec![
        Applied::Map {
            root: CELLS.into(),
            ops: vec![
                set(cell_key(&k("r1"), &k("c1")), 1),
                set(cell_key(&k("r2"), &k("c1")), 2),
                set(cell_key(&k("r1"), &k("c2")), 3),
                set(cell_key(&k("r2"), &k("c2")), 4),
                set("bump".into(), 5),
            ],
        },
        Applied::Map {
            root: NAMES.into(),
            ops: vec![set(cell_key(&k("r2"), &k("c1")), 6)],
        },
    ];
    let h = t.hidden_cell_writes(&stream);
    let got: Vec<(String, u32, bool, bool)> = h
        .iter()
        .map(|w| (w.cell_key(), w.write, w.row_hidden, w.col_hidden))
        .collect();
    assert_eq!(
        got,
        vec![
            (cell_key(&k("r2"), &k("c1")), 2, true, false),
            (cell_key(&k("r1"), &k("c2")), 3, false, true),
            (cell_key(&k("r2"), &k("c2")), 4, true, true),
        ]
    );
}

// ---------- the aligner ----------

fn reconciled(t: &Table, rows: &[&[&str]]) -> (Reconciled, Table) {
    let r = reconcile(t, &records(rows)).unwrap();
    let mut u = t.clone();
    u.apply(&r.edits).unwrap();
    assert_eq!(
        values(&u.render()),
        vals(rows),
        "the result renders as the file"
    );
    assert_eq!(reconcile(t, &records(rows)).unwrap(), r, "deterministic");
    (r, u)
}

#[test]
fn reconcile_of_the_rendering_is_nothing() {
    let mut ts = vec![
        table(
            &[&["a", "b"], &["1", "2"], &["1", "2"], &[]],
            true,
            Shape::Rect,
        ),
        table(
            &[&["a", "b", "c"], &["x"], &[], &["", "", "z"]],
            false,
            Shape::Ragged,
        ),
        table(&[], true, Shape::Rect),
        table(&[&["d", "d"], &["", ""]], true, Shape::Rect),
    ];
    // absent cells, a hidden row and column, quote exceptions, a trailing absent column
    let mut t = table(
        &[&["a", "b"], &["1", "2"], &["3", "4"]],
        true,
        Shape::Ragged,
    );
    t.insert_col(Anchor::End, None).unwrap();
    t.insert_row(Anchor::Start, &["only".into()]).unwrap();
    t.delete_row(&k("r2")).unwrap();
    t.delete_col(&k("c1")).unwrap();
    t.quoting
        .insert(QuoteAt::Cell(k("r1"), k("c2")), Quote::Quoted);
    ts.push(t);
    for t in &ts {
        let r = reconcile(t, &t.render()).unwrap();
        assert!(r.is_empty(), "{:?}: {:?}", t.render(), r.edits);
        assert_eq!(r.rows, t.live_rows());
    }
}

#[test]
fn a_spreadsheet_sort_arrives_as_moves() {
    let t = table(
        &[
            &["n", "v"],
            &["c", "3"],
            &["a", "1"],
            &["d", "4"],
            &["b", "2"],
        ],
        true,
        Shape::Rect,
    );
    let (r, u) = reconciled(
        &t,
        &[
            &["n", "v"],
            &["a", "1"],
            &["b", "2"],
            &["c", "3"],
            &["d", "4"],
        ],
    );
    assert_eq!(r.edits.len(), 1, "rows only: {:?}", r.edits);
    assert_eq!(keys(&r.rows), "r2 r4 r1 r3", "every row keeps its key");
    assert_eq!(r.moved_rows.len(), 2, "n − LIS moves: {:?}", r.edits);
    assert_eq!(u.cells, t.cells, "no cell written");
}

#[test]
fn edits_inserts_and_deletes_keep_the_other_keys() {
    let t = table(
        &[&["n", "v"], &["a", "1"], &["b", "2"], &["c", "3"]],
        true,
        Shape::Rect,
    );
    // one cell edited
    let (r, _) = reconciled(&t, &[&["n", "v"], &["a", "1"], &["b", "20"], &["c", "3"]]);
    assert_eq!(keys(&r.rows), "r1 r2 r3");
    assert_eq!(map_ops(&r.edits, CELLS).len(), 1);
    assert!(list_ops(&r.edits, ROWS).is_empty());
    // an insert in the middle, a delete at the end
    let (r, u) = reconciled(&t, &[&["n", "v"], &["a", "1"], &["new", "9"], &["b", "2"]]);
    assert_eq!(keys(&r.rows), "r1 r4 r2");
    assert_eq!(keys(&r.inserted_rows), "r4");
    assert_eq!(keys(&r.deleted_rows), "r3");
    assert_eq!(
        u.cell(&k("r3"), &k("c1")),
        Some("c"),
        "a delete hides the cells"
    );
    // a moved row that was edited too: unique content no longer matches, so it is a delete
    // and an insert; an unedited move is a move
    let (r, _) = reconciled(&t, &[&["n", "v"], &["c", "3"], &["a", "1"], &["b", "2"]]);
    assert_eq!(keys(&r.rows), "r3 r1 r2");
    assert_eq!(keys(&r.moved_rows), "r3");
    assert_forward_chain(list_ops(&r.edits, ROWS));
}

#[test]
fn columns_match_by_name_then_content_then_position() {
    let t = table(
        &[&["n", "v", "w"], &["a", "1", "x"], &["b", "2", "y"]],
        true,
        Shape::Rect,
    );
    // a column move with the header on: by name, no cell written
    let (r, u) = reconciled(&t, &[&["w", "n", "v"], &["x", "a", "1"], &["y", "b", "2"]]);
    assert_eq!(keys(&r.cols), "c3 c1 c2");
    assert_eq!(keys(&r.moved_cols), "c3");
    assert_eq!(u.cells, t.cells);
    // a rename in place keeps the key
    let (r, u) = reconciled(
        &t,
        &[&["n", "value", "w"], &["a", "1", "x"], &["b", "2", "y"]],
    );
    assert_eq!(keys(&r.cols), "c1 c2 c3");
    assert_eq!(u.name(&k("c2")), Some("value"));
    assert_eq!(r.edits.len(), 1, "names only: {:?}", r.edits);
    // the header off at the same width: by position (a content match there can be
    // coincidence), so a column move reads as value rewrites (flip, K2 Cores' option 1)
    let t = table(&[&["a", "1", "x"], &["b", "2", "y"]], false, Shape::Rect);
    let (r, u) = reconciled(&t, &[&["1", "x", "a"], &["2", "y", "b"]]);
    assert_eq!(keys(&r.cols), "c1 c2 c3");
    assert_eq!(u.cell(&k("r1"), &k("c1")), Some("1"));
    // the header off with the width changed: a column move is found by content
    let (r, u) = reconciled(&t, &[&["1", "x", "a", "N"], &["2", "y", "b", "M"]]);
    assert_eq!(keys(&r.cols), "c2 c3 c1 c4");
    assert_eq!(keys(&r.inserted_cols), "c4");
    for (at, v) in &t.cells {
        assert_eq!(u.cells.get(at), Some(v), "kept by key: {at:?}");
    }
    // an inserted column
    let (r, _) = reconciled(&t, &[&["a", "", "1", "x"], &["b", "q", "2", "y"]]);
    assert_eq!(keys(&r.cols), "c1 c4 c2 c3");
    assert_eq!(keys(&r.inserted_cols), "c4");
    // a deleted column
    let (r, u) = reconciled(&t, &[&["a", "x"], &["b", "y"]]);
    assert_eq!(keys(&r.deleted_cols), "c2");
    assert_eq!(u.cell(&k("r1"), &k("c2")), Some("1"), "hidden, kept");
}

#[test]
fn ragged_rows_shorten_and_grow() {
    let t = table(&[&["a", "b", "c"], &["d", "e"]], false, Shape::Ragged);
    let (r, u) = reconciled(&t, &[&["a", "b"], &["d", "e", "", "f"]]);
    assert_eq!(keys(&r.rows), "r1 r2");
    let ops = map_ops(&r.edits, CELLS);
    assert!(
        ops.contains(&MapOp::Delete {
            key: cell_key(&k("r1"), &k("c3"))
        }),
        "a shortened ragged row: the cell becomes absent: {ops:?}"
    );
    assert_eq!(
        u.cell(&k("r2"), &k("c3")),
        None,
        "a middle empty field stays absent"
    );
    assert_eq!(u.cell(&k("r2"), &k("c4")), Some("f"));
    // a trailing empty field must be a present cell, so the row renders through it
    let (_, u) = reconciled(&t, &[&["a", "b", "c"], &["d", "e", ""]]);
    assert_eq!(u.cell(&k("r2"), &k("c3")), Some(""));
}

#[test]
fn reconcile_refuses_bad_values_and_keeps_header_and_shape() {
    let t = table(&[&["a"]], true, Shape::Rect);
    assert!(matches!(
        reconcile(&t, &records(&[&["a"], &["x\0"]])),
        Err(TableError::Value(_))
    ));
    let (_, u) = reconciled(&t, &[&["a", "b"], &["1", "2"]]);
    assert!(u.header);
    assert_eq!(u.shape, Shape::Rect);
    // an empty file with the header on: every column and row deleted (hidden, names kept)
    let (_, u) = reconciled(&t, &[]);
    assert!(u.live_cols().is_empty() && u.live_rows().is_empty());
    assert_eq!(u.name(&k("c1")), Some("a"));
}

// ---------- merge3 ----------

/// base: header `n v`, rows a/1 (r1), b/2 (r2), c/3 (r3).
fn base() -> Table {
    table(
        &[&["n", "v"], &["a", "1"], &["b", "2"], &["c", "3"]],
        true,
        Shape::Rect,
    )
}

fn merged(b: &Table, o: &Table, t: &Table) -> (Merge, Table) {
    let m = merge3(b, o, t).unwrap();
    assert_eq!(merge3(b, o, t).unwrap(), m, "deterministic");
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    (m, r)
}

fn rules(m: &Merge) -> Vec<&'static str> {
    m.conflicts.iter().map(|c| c.rule.as_str()).collect()
}

#[test]
fn merge3_cell() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    t.set_cell(&k("r1"), &k("c2"), "y").unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["cell"]);
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("y"), "nothing overwritten");
    // the same value: taken once
    t.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    let (m, _) = merged(&b, &o, &t);
    assert!(m.is_clean() && m.edits.is_empty(), "{m:?}");
}

#[test]
fn merge3_deleted_row() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r2"), &k("c1"), "B").unwrap();
    o.move_row(&k("r3"), Anchor::Start).unwrap();
    t.delete_row(&k("r2")).unwrap();
    t.delete_row(&k("r3")).unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["deleted_row", "deleted_row"]);
    assert_eq!(m.conflicts[0].at, Region::Row(k("r3")), "the move");
    assert_eq!(
        m.conflicts[1].at,
        Region::Cell {
            row: k("r2"),
            col: k("c1")
        }
    );
    assert_eq!(r, t, "nothing of ours applied");
}

#[test]
fn merge3_edited_row() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.delete_row(&k("r1")).unwrap();
    o.delete_row(&k("r2")).unwrap();
    o.delete_row(&k("r3")).unwrap();
    t.set_cell(&k("r1"), &k("c2"), "9").unwrap();
    t.move_row(&k("r3"), Anchor::Start).unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["edited_row", "edited_row"]);
    assert_eq!(
        m.conflicts[0].theirs_writes,
        vec![Region::Cell {
            row: k("r1"),
            col: k("c2")
        }]
    );
    assert_eq!(m.conflicts[1].theirs_writes, vec![Region::Row(k("r3"))]);
    assert_eq!(
        keys(&r.live_rows()),
        "r3 r1",
        "the untouched row's delete applies"
    );
    // both deleted: once
    t = b.clone();
    t.delete_row(&k("r3")).unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert!(m.is_clean());
    assert!(r.live_rows().is_empty());
}

#[test]
fn merge3_moved_row() {
    // four rows, so each move below is the only minimal one
    let mut b = base();
    b.insert_row(Anchor::End, &["d".into(), "4".into()])
        .unwrap();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.move_row(&k("r1"), Anchor::End).unwrap();
    t.move_row(&k("r1"), Anchor::After(k("r3"))).unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["moved_row"]);
    assert_eq!(keys(&r.live_rows()), "r2 r3 r1 r4", "theirs' place kept");
    // moved to the same place: once
    let mut t = b.clone();
    t.move_row(&k("r1"), Anchor::End).unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert!(m.is_clean() && m.edits.is_empty(), "{m:?}");
    assert_eq!(keys(&r.live_rows()), "r2 r3 r4 r1");
    // moves of different rows both apply
    let mut t = b.clone();
    t.move_row(&k("r3"), Anchor::Start).unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert!(m.is_clean());
    assert_eq!(keys(&r.live_rows()), "r3 r2 r4 r1");
}

#[test]
fn merge3_same_gap_inserts_both_land_theirs_first() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    let (o1, _) = o
        .insert_row(Anchor::After(k("r1")), &["o1".into()])
        .unwrap();
    let (o2, _) = o
        .insert_row(Anchor::After(o1.clone()), &["o2".into()])
        .unwrap();
    let (t1, _) = t
        .insert_row(Anchor::After(k("r1")), &["t1".into()])
        .unwrap();
    assert_eq!((o1.as_str(), t1.as_str()), ("r4", "r4"), "both minted r4");
    let (m, r) = merged(&b, &o, &t);
    assert!(m.is_clean(), "{:?}", m.conflicts);
    assert_eq!(o2, k("r5"));
    assert_eq!(
        m.rekeyed,
        vec![(k("r4"), k("r6"))],
        "only the key theirs uses"
    );
    let firsts: Vec<String> = values(&r.render())
        .into_iter()
        .map(|v| v[0].clone())
        .collect();
    assert_eq!(firsts, ["n", "a", "t1", "o1", "o2", "b", "c"]);
    assert_eq!(
        list_ops(&m.edits, ROWS),
        &[
            ListOp::Insert {
                key: k("r6"),
                at: Anchor::Before(k("r2")),
                value: Value::Null
            },
            ListOp::Insert {
                key: k("r5"),
                at: Anchor::After(k("r6")),
                value: Value::Null
            },
        ],
        "one forward chain, before the gap's right end"
    );
    assert_eq!(r.next_row, 7);
    // at the start too
    let (mut o, mut t) = (b.clone(), b.clone());
    o.insert_row(Anchor::Start, &["o".into()]).unwrap();
    t.insert_row(Anchor::Start, &["t".into()]).unwrap();
    let (_, r) = merged(&b, &o, &t);
    let firsts: Vec<String> = values(&r.render())
        .into_iter()
        .map(|v| v[0].clone())
        .collect();
    assert_eq!(firsts, ["n", "t", "o", "a", "b", "c"]);
}

#[test]
fn merge3_name_and_columns() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.rename_col(&k("c1"), "name").unwrap();
    t.rename_col(&k("c1"), "nom").unwrap();
    let (m, _) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["name"]);
    // a column deleted by theirs: our set into it, and our rename of it, conflict
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    o.rename_col(&k("c2"), "w").unwrap();
    t.delete_col(&k("c2")).unwrap();
    let (m, _) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["deleted_col", "deleted_col"]);
    // our column delete vs their write in it
    let (mut o, mut t) = (b.clone(), b.clone());
    o.delete_col(&k("c2")).unwrap();
    t.set_cell(&k("r3"), &k("c2"), "z").unwrap();
    let (m, _) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["edited_col"]);
    // both moved one column, differently (four columns: each move the only minimal one);
    // both appended a column: theirs first
    let b = table(
        &[&["a", "b", "c", "d"], &["1", "2", "3", "4"]],
        true,
        Shape::Rect,
    );
    let (mut o, mut t) = (b.clone(), b.clone());
    o.move_col(&k("c1"), Anchor::End).unwrap();
    o.insert_col(Anchor::End, Some("x")).unwrap();
    t.move_col(&k("c1"), Anchor::After(k("c3"))).unwrap();
    t.insert_col(Anchor::End, Some("y")).unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert_eq!(rules(&m), ["moved_col"]);
    assert_eq!(m.rekeyed, vec![(k("c5"), k("c6"))]);
    assert_eq!(values(&r.render())[0], vec!["b", "c", "a", "d", "y", "x"]);
}

#[test]
fn merge3_other_elements_both_apply() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "10").unwrap();
    o.delete_row(&k("r3")).unwrap();
    o.insert_row(Anchor::End, &["d".into(), "4".into()])
        .unwrap();
    t.set_cell(&k("r2"), &k("c2"), "20").unwrap();
    t.move_row(&k("r1"), Anchor::After(k("r2"))).unwrap();
    t.rename_col(&k("c1"), "name").unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert!(m.is_clean(), "{:?}", m.conflicts);
    assert_eq!(
        values(&r.render()),
        vals(&[&["name", "v"], &["b", "20"], &["a", "10"], &["d", "4"]])
    );
}

/// The report's exact shape (K-BR-8): rule, region by key, base/ours/theirs, theirs' writes.
#[test]
fn merge3_report_shape() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    o.delete_row(&k("r2")).unwrap();
    o.set_cell(&k("r3"), &k("c1"), "C").unwrap();
    t.set_cell(&k("r1"), &k("c2"), "y").unwrap();
    t.set_cell(&k("r2"), &k("c2"), "z").unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert_eq!(
        m.conflicts,
        vec![
            Conflict {
                rule: ConflictRule::EditedRow,
                at: Region::Row(k("r2")),
                base: Held::Elem(ElemAt::Live {
                    index: 1,
                    after: Some(k("r1"))
                }),
                ours: Held::Elem(ElemAt::Deleted),
                theirs: Held::Elem(ElemAt::Live {
                    index: 1,
                    after: Some(k("r1"))
                }),
                theirs_writes: vec![Region::Cell {
                    row: k("r2"),
                    col: k("c2")
                }],
            },
            Conflict {
                rule: ConflictRule::Cell,
                at: Region::Cell {
                    row: k("r1"),
                    col: k("c2")
                },
                base: Held::Value(Some("1".into())),
                ours: Held::Value(Some("x".into())),
                theirs: Held::Value(Some("y".into())),
                theirs_writes: vec![],
            },
        ]
    );
    assert_eq!(
        m.conflicts[0].to_string(),
        "edited_row at row r2: base #2 (after r1), ours deleted, theirs #2 (after r1) \
         (theirs wrote r2/c2)"
    );
    assert_eq!(
        m.conflicts[1].to_string(),
        "cell at r1/c2: base \"1\", ours \"x\", theirs \"y\""
    );
    // the clean part (`--partial`): the other cell
    assert_eq!(
        m.edits,
        vec![Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Set {
                key: cell_key(&k("r3"), &k("c1")),
                value: Value::str("C")
            }]
        }]
    );
    assert_eq!(r.cell(&k("r3"), &k("c1")), Some("C"));
}

#[test]
fn merge3_of_a_reconciled_rewrite() {
    // write --base: ours = the agent's rewrite reconciled against base
    let b = base();
    let rewrite = records(&[
        &["n", "v"],
        &["a", "1"],
        &["b", "22"],
        &["c", "3"],
        &["e", "5"],
    ]);
    let rec = reconcile(&b, &rewrite).unwrap();
    let mut o = b.clone();
    o.apply(&rec.edits).unwrap();
    let mut t = b.clone();
    t.set_cell(&k("r1"), &k("c2"), "11").unwrap();
    t.insert_row(Anchor::End, &["f".into(), "6".into()])
        .unwrap();
    let (m, r) = merged(&b, &o, &t);
    assert!(m.is_clean(), "{:?}", m.conflicts);
    assert_eq!(
        values(&r.render()),
        vals(&[
            &["n", "v"],
            &["a", "11"],
            &["b", "22"],
            &["c", "3"],
            &["f", "6"],
            &["e", "5"]
        ])
    );
}

// ---------- C1c: places of deleted rows, drop-deleted ----------

/// A doc holding `t`'s import, as a session would.
fn doc_of(rows: &[&[&str]], header: bool) -> (crate::doc::Doc, Table) {
    let mut d = crate::doc::Doc::new(crate::schema::Schema {
        name: "t",
        roots: ROOTS,
    })
    .unwrap();
    let (t, edits) = Table::import(&records(rows), header, Shape::Rect).unwrap();
    d.edit_as(7, &edits).unwrap();
    (d, t)
}

#[test]
fn from_keyed_keeps_deleted_rows_at_their_places() {
    let (mut d, mut t) = doc_of(&[&["a"], &["b"], &["c"], &["d"]], false);
    let mut e = t.delete_row(&k("r2")).unwrap();
    e.extend(t.move_row(&k("r4"), Anchor::Start).unwrap());
    d.edit_as(7, &e).unwrap();
    let mut read = Table::from_keyed(&d.keyed_state().unwrap()).unwrap();
    assert_eq!(read, t);
    // a restore on the read table puts r2 back where the doc does
    let e = read.restore_row(&k("r2")).unwrap();
    d.edit_as(7, &e).unwrap();
    let doc_order: Vec<Key> = d.list(ROWS).unwrap().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys(&doc_order), "r4 r1 r2 r3");
    assert_eq!(keys(&read.live_rows()), "r4 r1 r2 r3");
}

#[test]
fn drop_deleted_frees_rows_cols_and_their_cells() {
    let (mut d, mut t) = doc_of(&[&["h1", "h2"], &["a", "b"], &["c", "d"]], true);
    let mut e = t.delete_row(&k("r2")).unwrap();
    e.extend(t.delete_col(&k("c2")).unwrap());
    d.edit_as(7, &e).unwrap();
    // a hidden write into the deleted row, as a stale writer would make
    let e = vec![Edit::Map {
        root: CELLS.into(),
        ops: vec![MapOp::Set {
            key: cell_key(&k("r2"), &k("c1")),
            value: Value::str("late"),
        }],
    }];
    t.apply(&e).unwrap();
    d.edit_as(8, &e).unwrap();
    assert_eq!(t.cells.len(), 4, "hidden cells count");
    let before = t.render();
    let e = t.drop_deleted().unwrap();
    assert!(list_ops(&e, ROWS)
        .iter()
        .all(|o| matches!(o, ListOp::Drop { .. })));
    d.edit_as(7, &e).unwrap();
    assert_eq!(t.render(), before, "nothing shown changes");
    assert_eq!(t.cells.len(), 1, "only live cells stay: {:?}", t.cells);
    assert!(t.name(&k("c2")).is_none());
    assert!(!t.rows.knows(&k("r2")) && !t.cols.knows(&k("c2")));
    assert_eq!(Table::from_keyed(&d.keyed_state().unwrap()).unwrap(), t);
    // the counters keep dropped keys retired
    let again = Table::from_keyed(&d.keyed_state().unwrap()).unwrap();
    assert!(again.next_row > 2 && again.next_col > 2);
    assert!(t.clone().drop_deleted().unwrap().is_empty(), "idempotent");
}

// ---------- C1d part 2: a move and an edit in one save ----------

#[test]
fn reconcile_one_save_moving_a_row_and_editing_another_keeps_both_keys() {
    let t = table(&[&["a", "b"], &["c", "d"], &["e", "f"]], false, Shape::Rect);
    let r = reconcile(&t, &records(&[&["e", "f"], &["a", "b"], &["c", "D"]])).unwrap();
    assert_eq!(keys(&r.rows), "r3 r1 r2", "every row keeps its key");
    assert!(r.inserted_rows.is_empty() && r.deleted_rows.is_empty());
    assert_eq!(keys(&r.moved_rows), "r3");
    assert_eq!(
        map_ops(&r.edits, CELLS),
        &[MapOp::Set {
            key: cell_key(&k("r2"), &k("c2")),
            value: Value::str("D"),
        }]
    );
    // moved and edited in the same save: still one row
    let r = reconcile(&t, &records(&[&["c", "D"], &["a", "b"], &["e", "f"]])).unwrap();
    assert_eq!(keys(&r.rows), "r2 r1 r3");
}

#[test]
fn reconcile_ambiguity_rule() {
    // two old rows equally like one new row: no best match; the stretch pairing decides
    let t = table(&[&["x", "1"], &["x", "2"], &["z", "9"]], false, Shape::Rect);
    let r = reconcile(&t, &records(&[&["z", "9"], &["x", "3"]])).unwrap();
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[0], k("r3"));
    // a row moved and fully rewritten shares nothing with its old self: a new row
    let t = table(&[&["a", "b"], &["c", "d"]], false, Shape::Rect);
    let r = reconcile(&t, &records(&[&["p", "q"], &["a", "b"]])).unwrap();
    assert_eq!(keys(&r.rows), "r3 r1");
    assert_eq!(keys(&r.deleted_rows), "r2");
}

// ---------- C1k: a row in its unchanged slot keeps its key by position ----------

#[test]
fn reconcile_pairs_rows_in_an_unchanged_slot_by_position() {
    // a blank line edited: nothing shared, same slot
    let t = table(&[&["a", "b"], &[], &["c", "d"]], false, Shape::Rect);
    let (r, u) = reconciled(&t, &[&["a", "b"], &["", "z"], &["c", "d"]]);
    assert_eq!(
        keys(&r.rows),
        "r1 r2 r3",
        "the blank line's row keeps its key"
    );
    assert!(r.inserted_rows.is_empty() && r.deleted_rows.is_empty());
    assert_eq!(u.cell(&k("r2"), &k("c2")), Some("z"));
    assert_eq!(
        u.cell(&k("r2"), &k("c1")),
        None,
        "the empty field stays absent"
    );
    // and back to blank
    let (r, _) = reconciled(&u, &[&["a", "b"], &[], &["c", "d"]]);
    assert_eq!(keys(&r.rows), "r1 r2 r3");
    // a one-column table: any edit shares nothing; first, middle, last, two in a row
    let t = table(&[&["a"], &["b"], &["c"], &["d"]], false, Shape::Rect);
    for (file, want) in [
        (&[&["A"][..], &["b"], &["c"], &["d"]][..], "r1 r2 r3 r4"),
        (&[&["a"][..], &["B"], &["c"], &["d"]][..], "r1 r2 r3 r4"),
        (&[&["a"][..], &["b"], &["c"], &["D"]][..], "r1 r2 r3 r4"),
        (&[&["a"][..], &["B"], &["C"], &["d"]][..], "r1 r2 r3 r4"),
        (&[&["A"][..], &["B"], &["C"], &["D"]][..], "r1 r2 r3 r4"),
    ] {
        let (r, _) = reconciled(&t, file);
        assert_eq!(keys(&r.rows), want, "{file:?}");
        assert!(list_ops(&r.edits, ROWS).is_empty(), "cell edits only");
    }
    // the slot changed: a different count between the same neighbours is inserts and deletes
    let (r, _) = reconciled(&t, &[&["a"], &["X"], &["Y"], &["d"]]);
    assert_eq!(keys(&r.rows), "r1 r2 r3 r4", "two for two");
    let (r, _) = reconciled(&t, &[&["a"], &["X"], &["Y"], &["Z"], &["d"]]);
    assert_eq!(keys(&r.rows), "r1 r5 r6 r7 r4", "three for two: new rows");
    assert_eq!(keys(&r.deleted_rows), "r2 r3");
    // moved and rewritten: its neighbours aren't its old ones, so it is a new row
    let (r, _) = reconciled(&t, &[&["a"], &["c"], &["X"], &["d"]]);
    assert_eq!(keys(&r.rows), "r1 r3 r5 r4");
    assert_eq!(keys(&r.deleted_rows), "r2");
    let (r, _) = reconciled(&t, &[&["X"], &["a"], &["c"], &["d"]]);
    assert_eq!(keys(&r.rows), "r5 r1 r3 r4");
    // a whole one-row table rewritten in place
    let t = table(&[&["only"]], false, Shape::Rect);
    let (r, _) = reconciled(&t, &[&["other"]]);
    assert_eq!(keys(&r.rows), "r1");
}

// ---------- v0.10.0: exact rows, and a text's rows merged whole ----------

fn exact_of(r: &Reconciled) -> String {
    keys(&r.exact_rows)
}

#[test]
fn reconcile_reports_the_rows_left_exact() {
    let t = table(&[&["a", "1"], &["b", "2"], &["c", "3"]], false, Shape::Rect);
    // unchanged, moved: exact; edited: not
    let (r, _) = reconciled(&t, &[&["c", "3"], &["a", "1"], &["b", "X"]]);
    assert_eq!(keys(&r.rows), "r3 r1 r2");
    assert_eq!(exact_of(&r), "r3 r1");
    // the whole rendering: every row
    let r = reconcile(&t, &t.render()).unwrap();
    assert_eq!(exact_of(&r), "r1 r2 r3");
}

/// "Unchanged" is byte-exact: no fingerprint, nothing normalised.
#[test]
fn an_exact_row_is_exact_field_for_field() {
    // a blank line and a row of one empty field are not the same
    let t = table(&[&["a", "1"], &[]], false, Shape::Rect);
    let r = reconcile(&t, &records(&[&["a", "1"], &[""]])).unwrap();
    assert_eq!(keys(&r.rows), "r1 r2", "the blank line's row keeps its key");
    assert_eq!(
        exact_of(&r),
        "r1",
        "a blank line edited to \"\" isn't exact"
    );
    // how a field is quoted isn't a change: a tool can requote a whole file on save
    let t = table(&[&["a", "1"], &["b", "2"]], false, Shape::Rect);
    let mut recs = records(&[&["a", "1"], &["b", "2"]]);
    recs[0][1].quote = Some(Quote::Quoted);
    let r = reconcile(&t, &recs).unwrap();
    assert_eq!(keys(&r.rows), "r1 r2");
    assert_eq!(exact_of(&r), "r1 r2");
    // a space counts
    let (r, _) = reconciled(&t, &[&["a ", "1"], &["b", "2"]]);
    assert_eq!(exact_of(&r), "r2");
}

/// A column mapping is exact by its own position or its unique header name; anything else makes
/// no row exact, however alike it looks.
#[test]
fn a_column_mapping_by_content_leaves_no_row_exact() {
    let t = table(&[&["p", "q"], &["r", "s"]], false, Shape::Rect);
    let (r, _) = reconciled(&t, &[&["q", "p"], &["s", "r"]]);
    assert_eq!(
        keys(&r.rows),
        "r1 r2",
        "the rows look unchanged under the mapping"
    );
    assert_eq!(exact_of(&r), "");
    // by header name: exact, moved or not
    let t = table(&[&["n", "v"], &["a", "1"], &["b", "2"]], true, Shape::Rect);
    let (r, _) = reconciled(&t, &[&["v", "n"], &["1", "a"], &["2", "b"]]);
    assert_eq!(exact_of(&r), "r1 r2");
    // a column added: every row renders another field
    let t = table(&[&["a", "1"], &["b", "2"]], false, Shape::Rect);
    let (r, _) = reconciled(&t, &[&["a", "1", "x"], &["b", "2", ""]]);
    assert_eq!(exact_of(&r), "");
}

fn keyset(ks: &[&str]) -> HashSet<Key> {
    ks.iter().map(|s| k(s)).collect()
}

#[test]
fn merge3_text_a_row_both_changed_is_one_conflict_and_nothing_applies() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    o.set_cell(&k("r2"), &k("c2"), "other").unwrap();
    t.set_cell(&k("r1"), &k("c1"), "y").unwrap();
    // different cells of one row: no merge through a text
    let m = merge3_text(&b, &o, &t, &keyset(&["r3"])).unwrap();
    assert_eq!(rules(&m), ["edited_row"]);
    let c = &m.conflicts[0];
    assert_eq!(c.at, Region::Row(k("r1")));
    assert!(c.is_rewritten_row());
    assert_eq!(
        c.theirs_writes,
        [Region::Cell {
            row: k("r1"),
            col: k("c1")
        }]
    );
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert_eq!(r.cell(&k("r1"), &k("c1")), Some("y"), "theirs stays");
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("1"), "none of ours' row");
    assert_eq!(r.cell(&k("r2"), &k("c2")), Some("other"), "the clean part");
    // merge3 (verbs): cell by cell, clean
    let (m, r) = merged(&b, &o, &t);
    assert!(m.conflicts.is_empty());
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("x"));
}

#[test]
fn merge3_text_an_exact_row_keeps_the_live_cells_and_a_row_alone_applies() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    // ours moved r1 (exact) and changed r2; theirs edited r1 and r3
    o.move_row(&k("r1"), Anchor::End).unwrap();
    o.set_cell(&k("r2"), &k("c2"), "x").unwrap();
    t.set_cell(&k("r1"), &k("c1"), "y").unwrap();
    t.set_cell(&k("r3"), &k("c1"), "z").unwrap();
    let m = merge3_text(&b, &o, &t, &keyset(&["r1", "r3"])).unwrap();
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert_eq!(r.cell(&k("r1"), &k("c1")), Some("y"));
    assert_eq!(r.cell(&k("r2"), &k("c2")), Some("x"));
    assert_eq!(r.cell(&k("r3"), &k("c1")), Some("z"));
    assert_eq!(keys(&r.live_rows()), "r2 r3 r1");
}

#[test]
fn merge3_text_a_live_edit_equal_to_the_rewrite_is_no_conflict() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    t.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    let m = merge3_text(&b, &o, &t, &keyset(&[])).unwrap();
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    let m = merge3_outside_text(&b, &o, &t, &keyset(&[])).unwrap();
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
}

#[test]
fn merge3_text_a_live_quote_is_no_change_and_ours_is_held_with_its_row() {
    let b = base();
    let mut o = b.clone();
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    // a live quote exception alone: the text's row applies, nothing conflicts
    let mut t = b.clone();
    let at = QuoteAt::Cell(k("r1"), k("c1"));
    t.quoting.insert(at.clone(), Quote::Quoted);
    let m = merge3_text(&b, &o, &t, &keyset(&[])).unwrap();
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("x"));
    assert_eq!(r.quote(&at), Some(Quote::Quoted), "the live quoting stays");
    // a row both changed: ours' quote exception is held with it
    let mut o2 = o.clone();
    o2.quoting
        .insert(QuoteAt::Cell(k("r1"), k("c2")), Quote::Quoted);
    let mut t = b.clone();
    t.set_cell(&k("r1"), &k("c1"), "y").unwrap();
    let m = merge3_text(&b, &o2, &t, &keyset(&[])).unwrap();
    assert_eq!(rules(&m), ["edited_row"]);
    assert!(m.edits.is_empty(), "{:?}", m.edits);
}

/// A file's delete of a row the live side only moved stands: no conflict, no note. A row also
/// edited follows the edit rule.
#[test]
fn merge3_text_a_delete_stands_over_a_live_move() {
    let b = base();
    let mut o = b.clone();
    o.delete_row(&k("r1")).unwrap();
    let mut t = b.clone();
    t.move_row(&k("r1"), Anchor::End).unwrap();
    for m in [
        merge3_text(&b, &o, &t, &keyset(&["r2", "r3"])).unwrap(),
        merge3_outside_text(&b, &o, &t, &keyset(&["r2", "r3"])).unwrap(),
    ] {
        assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
        assert_eq!(m.moved_then_deleted, [k("r1")]);
        let mut r = t.clone();
        r.apply(&m.edits).unwrap();
        assert!(!r.rows.is_live(&k("r1")));
    }
    t.set_cell(&k("r1"), &k("c2"), "y").unwrap();
    let m = merge3_text(&b, &o, &t, &keyset(&["r2", "r3"])).unwrap();
    assert_eq!(rules(&m), ["edited_row"]);
    // merge3 (a verb's merge) keeps its rule
    let mut t = b.clone();
    t.move_row(&k("r1"), Anchor::End).unwrap();
    let m = merge3(&b, &o, &t).unwrap();
    assert_eq!(rules(&m), ["edited_row"]);
}

/// Live deletes and moves (K2 Cores' rulings).
#[test]
fn merge3_text_live_deletes_and_moves() {
    let b = base();
    // the text changed a row live deleted: a conflict; outside: the row returns
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    t.delete_row(&k("r1")).unwrap();
    let m = merge3_text(&b, &o, &t, &keyset(&[])).unwrap();
    assert_eq!(rules(&m), ["deleted_row"]);
    let m = merge3_outside_text(&b, &o, &t, &keyset(&[])).unwrap();
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert!(r.rows.is_live(&k("r1")), "the file's row returns");
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("x"));
    assert_eq!(rules(&m), ["deleted_row"], "reported");
    // the text left it: it stays deleted
    let o = b.clone();
    for m in [
        merge3_text(&b, &o, &t, &keyset(&["r1"])).unwrap(),
        merge3_outside_text(&b, &o, &t, &keyset(&["r1"])).unwrap(),
    ] {
        assert!(m.conflicts.is_empty() && m.edits.is_empty(), "{m:?}");
    }
    // a move: the file's place wins only if the file moved the row
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    t.move_row(&k("r1"), Anchor::End).unwrap();
    let m = merge3_outside_text(&b, &o, &t, &keyset(&[])).unwrap();
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert_eq!(keys(&r.live_rows()), "r2 r3 r1", "the live place");
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("x"));
}

#[test]
fn merge3_outside_text_the_file_row_wins_every_live_cell() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    t.set_cell(&k("r1"), &k("c1"), "y").unwrap();
    let m = merge3_outside_text(&b, &o, &t, &keyset(&["r2", "r3"])).unwrap();
    assert_eq!(rules(&m), ["cell"], "the live cell, reported as overridden");
    assert_eq!(m.conflicts[0].theirs, Held::Value(Some("y".into())));
    assert_eq!(m.conflicts[0].ours, Held::Value(Some("a".into())));
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert_eq!(r.cell(&k("r1"), &k("c1")), Some("a"), "the file's row");
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("x"));
    // merge3_outside: cell by cell
    let m = merge3_outside(&b, &o, &t).unwrap();
    assert!(m.conflicts.is_empty());
}

#[test]
fn merge3_outside_text_one_report_per_cell_and_the_files_quoting() {
    let b = base();
    let (mut o, mut t) = (b.clone(), b.clone());
    o.set_cell(&k("r1"), &k("c2"), "x").unwrap();
    t.set_cell(&k("r1"), &k("c2"), "y").unwrap();
    let at = QuoteAt::Cell(k("r1"), k("c1"));
    t.quoting.insert(at.clone(), Quote::Quoted);
    let m = merge3_outside_text(&b, &o, &t, &keyset(&[])).unwrap();
    assert_eq!(rules(&m), ["cell"], "how a field is quoted gets no report");
    let mut r = t.clone();
    r.apply(&m.edits).unwrap();
    assert_eq!(r.cell(&k("r1"), &k("c2")), Some("x"));
    assert_eq!(r.quote(&at), None, "the file's row, quoting too");
}

/// Ruling 1's header forms: a column mapping is exact by its own position or a unique exact
/// header name, in every form.
#[test]
fn header_forms_of_an_exact_column_mapping() {
    let hd = || table(&[&["a", "b"], &["1", "2"], &["3", "4"]], true, Shape::Rect);
    // renamed in place: the same positions: exact
    let (r, _) = reconciled(&hd(), &[&["x", "b"], &["1", "2"], &["3", "4"]]);
    assert!(r.cols_exact);
    assert_eq!(exact_of(&r), "r1 r2");
    // reordered: unique names: exact, the rows unchanged under them
    let (r, _) = reconciled(&hd(), &[&["b", "a"], &["2", "1"], &["4", "3"]]);
    assert!(r.cols_exact);
    assert_eq!(exact_of(&r), "r1 r2");
    // reordered and one renamed: the renamed one maps by content, away from its place: not exact
    let (r, _) = reconciled(&hd(), &[&["b", "z"], &["2", "1"], &["4", "3"]]);
    assert!(!r.cols_exact);
    assert_eq!(exact_of(&r), "");
    // all renamed and swapped: by content: not exact, though every row looks the same
    let (r, _) = reconciled(&hd(), &[&["y", "x"], &["2", "1"], &["4", "3"]]);
    assert!(!r.cols_exact);
    assert_eq!(exact_of(&r), "");
    // duplicate names, kept in place: their positions: exact
    let dup = || table(&[&["a", "a"], &["1", "2"], &["3", "4"]], true, Shape::Rect);
    let (r, _) = reconciled(&dup(), &[&["a", "a"], &["1", "X"], &["3", "4"]]);
    assert!(r.cols_exact);
    assert_eq!(exact_of(&r), "r2");
    // duplicate names, swapped: no unique name, mapped by content: not exact
    let (r, _) = reconciled(&dup(), &[&["a", "a"], &["2", "1"], &["4", "3"]]);
    assert!(!r.cols_exact);
    assert_eq!(exact_of(&r), "");
}

/// A column the file deleted changes every row (K2 Cores' ruling: a column delete is the file
/// changing the row): no row is exact, though each still renders what it did.
#[test]
fn a_deleted_column_leaves_no_row_exact() {
    let b = table(&[&[], &["", "", ""]], false, Shape::Rect);
    let r = reconcile(&b, &[vec![]]).unwrap();
    assert_eq!(keys(&r.rows), "r1");
    assert_eq!(exact_of(&r), "");
    let t = table(&[&["a", "1"], &["b", "2"]], false, Shape::Rect);
    let (r, _) = reconciled(&t, &[&["a"], &["b"]]);
    assert_eq!(exact_of(&r), "");
}
