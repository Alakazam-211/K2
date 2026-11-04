//! A connection's element selection, pointer and viewport, on a keyed doc that isn't a table and
//! on a table. They need read, stay outside the doc's numbering, reach subscribers through the
//! roster, and a selection loses the keys of elements that stop being live, on every path that
//! changes the doc. Public API only.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::elem::{Anchor, ListOp};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, DiskCheck, EditOptions, ElemSelection, Host, Joined, Participant,
    ParticipantKind, Point, Rect, Rights, Role, RosterEntry, RosterEvent, Session, SessionError,
    SessionManager, Stay, TextFilePack, Who, ELEM_SELECTION_MAX,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::{table, Edit, Key, Value};

#[path = "grid_pack/mod.rs"]
mod grid_pack;
#[path = "keyed_pack/mod.rs"]
mod keyed_pack;

use keyed_pack::{Keyed, ITEMS, PROPS};

/// `MPC_STRUCT_MUTANT` runs `f` with a selection fault planted on this thread
/// (`k2_mpc_base::differential::SelectionMutant`, the `differential` feature, on for these
/// tests): `selection-keeps-deleted` is `SelectionMutant::KeepsDeleted`. Unset, `f` runs as is.
/// Any other value stops the suite.
fn under_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_selection_mutant, SelectionMutant};
    match std::env::var("MPC_STRUCT_MUTANT").as_deref() {
        Ok("selection-keeps-deleted") => with_selection_mutant(SelectionMutant::KeepsDeleted, f),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::editor())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(PathBuf);

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    dir: TmpDir,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

fn fx_with(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-elem-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store, host.clone(), c);
    Fx {
        dir: TmpDir(dir),
        host,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, |_| {})
}

fn doc_key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

impl Fx {
    /// A keyed doc holding `items` i1..i4.
    fn keyed(&self) -> Arc<Session> {
        self.mgr
            .open(
                Arc::new(Keyed),
                &doc_key("doc.keyed"),
                Some(b"i1\ni2\ni3\ni4\n"),
            )
            .unwrap()
    }
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
    fn reader(&self, pid: &str) {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::read_only());
    }
}

fn join(s: &Session, pid: &str) -> Joined {
    s.join(
        Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab"),
        Stay::Held,
    )
    .unwrap()
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn sel(root: &str, keys: &[&str]) -> Option<ElemSelection> {
    Some(ElemSelection::new(
        root,
        keys.iter().map(|x| k(x)).collect(),
    ))
}

fn list_op(s: &Session, c: ConnId, op: ListOp) {
    s.edit_ops(
        c,
        vec![Edit::List {
            root: ITEMS.into(),
            ops: vec![op],
        }],
        &EditOptions::new(),
    )
    .unwrap();
}

fn delete(s: &Session, c: ConnId, key: &str) {
    list_op(s, c, ListOp::Delete { key: k(key) });
}

fn entry(s: &Session, c: ConnId, pid: &str) -> RosterEntry {
    s.roster(c)
        .unwrap()
        .into_iter()
        .find(|e| e.participant.pid == pid)
        .unwrap_or_else(|| panic!("{pid} isn't in the roster"))
}

/// `pid`'s selected keys in the roster, as strings; empty when there is no selection.
fn selected(s: &Session, c: ConnId, pid: &str) -> Vec<String> {
    entry(s, c, pid).selection.map_or(Vec::new(), |x| {
        x.keys.iter().map(|k| k.to_string()).collect()
    })
}

fn drain(rx: &Receiver<RosterEvent>) -> Vec<RosterEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn id_of(e: SessionError) -> String {
    e.report().id().to_owned()
}

// ---------- rights and refusals ----------

#[test]
fn a_reader_can_show_all_three_and_it_does_not_make_them_an_editor() {
    let f = fx("reader");
    let s = f.keyed();
    f.reader("r");
    let r = join(&s, "r").conn;
    s.set_selection(r, sel(ITEMS, &["i1"])).unwrap();
    s.set_pointer(r, Some(Point::new(1.5, 2.5).unwrap()))
        .unwrap();
    s.set_viewport(r, Some(Rect::new(0.0, 0.0, 10.0, 20.0).unwrap()))
        .unwrap();
    let e = entry(&s, r, "r");
    assert_eq!(e.selection, sel(ITEMS, &["i1"]));
    assert_eq!(e.pointer, Some(Point::new(1.5, 2.5).unwrap()));
    assert_eq!(e.viewport, Some(Rect::new(0.0, 0.0, 10.0, 20.0).unwrap()));
    assert_eq!(e.role, Role::Viewer);
}

/// A selection is editing intent, as a caret is, for someone who can edit. A pointer and a
/// viewport aren't.
#[test]
fn a_writers_selection_shows_them_as_an_editor_and_a_pointer_or_viewport_does_not() {
    let f = fx("writer");
    let s = f.keyed();
    let a = join(&s, "a").conn;
    assert_eq!(entry(&s, a, "a").role, Role::Viewer, "after joining");
    s.set_pointer(a, Some(Point::new(1.0, 2.0).unwrap()))
        .unwrap();
    s.set_viewport(a, Some(Rect::new(0.0, 0.0, 1.0, 1.0).unwrap()))
        .unwrap();
    assert_eq!(
        entry(&s, a, "a").role,
        Role::Viewer,
        "a pointer and a viewport"
    );
    s.set_selection(a, sel(ITEMS, &["i1"])).unwrap();
    assert_eq!(entry(&s, a, "a").role, Role::Editor, "a selection");
}

#[test]
fn someone_with_no_read_right_is_refused() {
    let f = fx("noread");
    let s = f.keyed();
    let c = join(&s, "gone").conn;
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("gone".into(), Rights::none());
    let e = s.set_pointer(c, None).unwrap_err();
    assert!(matches!(e, SessionError::Forbidden { .. }), "{e:?}");
}

#[test]
fn a_watcher_is_refused_and_shows_nothing() {
    let f = fx_with("watcher", |c| c.participants_max = 1);
    let s = f.keyed();
    let a = join(&s, "a").conn;
    let w = join(&s, "w").conn;
    let at = Some(Point::new(1.0, 1.0).unwrap());
    let view = Some(Rect::new(0.0, 0.0, 1.0, 1.0).unwrap());
    assert_eq!(id_of(s.set_pointer(w, at).unwrap_err()), "MPC-E305");
    assert_eq!(id_of(s.set_viewport(w, view).unwrap_err()), "MPC-E305");
    assert_eq!(
        id_of(s.set_selection(w, sel(ITEMS, &["i1"])).unwrap_err()),
        "MPC-E305"
    );
    assert_eq!(s.roster(a).unwrap().len(), 1);
}

#[test]
fn a_bad_selection_is_refused_and_leaves_the_selection_as_it_was() {
    let f = fx("bad");
    let s = f.keyed();
    let a = join(&s, "a").conn;
    s.set_selection(a, sel(ITEMS, &["i1"])).unwrap();
    delete(&s, a, "i4");

    for (why, r) in [
        ("unknown", s.set_selection(a, sel(ITEMS, &["nope"]))),
        ("deleted", s.set_selection(a, sel(ITEMS, &["i2", "i4"]))),
        ("a map root", s.set_selection(a, sel(PROPS, &["i1"]))),
        ("no such root", s.set_selection(a, sel("nope", &["i1"]))),
    ] {
        assert_eq!(id_of(r.unwrap_err()), "MPC-E114", "{why}");
    }

    // what a reader of the refusal is told
    let said = |r: Result<Option<ElemSelection>, SessionError>| {
        r.unwrap_err().report().message().to_owned()
    };
    assert_eq!(
        said(s.set_selection(a, sel(PROPS, &["i1"]))),
        "bad position: \"props\" is not a list of elements in this doc"
    );
    assert_eq!(
        said(s.set_selection(a, sel(ITEMS, &["i4"]))),
        "bad position: element i4 of \"items\" is not live (unknown or deleted)"
    );

    let too_many = (0..=ELEM_SELECTION_MAX)
        .map(|i| k(&format!("x{i}")))
        .collect();
    let e = s
        .set_selection(a, Some(ElemSelection::new(ITEMS, too_many)))
        .unwrap_err();
    assert!(
        matches!(e, SessionError::SelectionTooLarge { len: 257, max: 256 }),
        "{e:?}"
    );
    assert_eq!(id_of(e), "MPC-E310");

    assert_eq!(selected(&s, a, "a"), ["i1"]);
}

#[test]
fn exactly_the_most_keys_is_taken_and_repeats_count_once() {
    let f = fx("max");
    let body: String = (0..ELEM_SELECTION_MAX).map(|i| format!("k{i}\n")).collect();
    let s = f
        .mgr
        .open(
            Arc::new(Keyed),
            &doc_key("big.keyed"),
            Some(body.as_bytes()),
        )
        .unwrap();
    let a = join(&s, "a").conn;
    let keys: Vec<Key> = (0..ELEM_SELECTION_MAX)
        .map(|i| k(&format!("k{i}")))
        .collect();
    let got = s
        .set_selection(a, Some(ElemSelection::new(ITEMS, keys)))
        .unwrap()
        .unwrap();
    assert_eq!(got.keys.len(), ELEM_SELECTION_MAX);
    let got = s
        .set_selection(a, sel(ITEMS, &["k1", "k2", "k1"]))
        .unwrap()
        .unwrap();
    assert_eq!(got.keys, [k("k1"), k("k2")]);
}

#[test]
fn no_keys_clears_the_selection() {
    let f = fx("clear");
    let s = f.keyed();
    let a = join(&s, "a").conn;
    s.set_selection(a, sel(ITEMS, &["i1"])).unwrap();
    assert_eq!(s.set_selection(a, sel(ITEMS, &[])).unwrap(), None);
    assert_eq!(entry(&s, a, "a").selection, None);
    s.set_selection(a, sel(ITEMS, &["i1"])).unwrap();
    assert_eq!(s.set_selection(a, None).unwrap(), None);
    assert_eq!(entry(&s, a, "a").selection, None);
}

#[test]
fn a_text_doc_has_no_elements_to_select_but_takes_a_pointer_and_a_viewport() {
    let f = fx("text");
    let s = f
        .mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &doc_key("a.md"),
            Some(b"one\n"),
        )
        .unwrap();
    let a = join(&s, "a").conn;
    assert_eq!(
        id_of(
            s.set_selection(a, sel(TextFilePack::ROOT, &["i1"]))
                .unwrap_err()
        ),
        "MPC-E114"
    );
    s.set_pointer(a, Some(Point::new(1.0, 2.0).unwrap()))
        .unwrap();
    s.set_viewport(a, Some(Rect::new(0.0, 0.0, 1.0, 1.0).unwrap()))
        .unwrap();
    let e = entry(&s, a, "a");
    assert!(e.pointer.is_some() && e.viewport.is_some());
}

#[test]
fn coordinates_must_be_finite_and_a_size_not_negative() {
    for (why, bad) in [
        ("x nan", Point::new(f64::NAN, 0.0).err()),
        ("y inf", Point::new(0.0, f64::INFINITY).err()),
        ("x nan", Rect::new(f64::NAN, 0.0, 1.0, 1.0).err()),
        ("w inf", Rect::new(0.0, 0.0, f64::INFINITY, 1.0).err()),
        ("w negative", Rect::new(0.0, 0.0, -1.0, 1.0).err()),
        ("h negative", Rect::new(0.0, 0.0, 1.0, -0.5).err()),
    ] {
        assert_eq!(id_of(bad.expect(why)), "MPC-E114", "{why}");
    }
    // one way to compare and encode
    assert_eq!(
        Point::new(-0.0, 1.0).unwrap(),
        Point::new(0.0, 1.0).unwrap()
    );
    assert!(Point::new(-0.0, 1.0).unwrap().x().is_sign_positive());
    assert!(
        Rect::new(0.0, 0.0, -0.0, 1.0).is_ok(),
        "negative zero is zero"
    );
}

// ---------- a selection follows its elements ----------

#[test]
fn a_deleted_element_leaves_every_selection_and_comes_back_only_if_selected_again() {
    under_mutant(|| {
        let f = fx("deleted");
        let s = f.keyed();
        let a = join(&s, "a").conn;
        let b = join(&s, "b").conn;
        s.set_selection(a, sel(ITEMS, &["i1", "i2"])).unwrap();
        s.set_selection(b, sel(ITEMS, &["i2", "i3"])).unwrap();
        let roster = s.subscribe_roster(a).unwrap();
        f.tick();
        drain(&roster);

        delete(&s, b, "i2");
        assert_eq!(selected(&s, a, "a"), ["i1"], "a's selection");
        assert_eq!(selected(&s, a, "b"), ["i3"], "b's selection");
        f.tick();
        let told: BTreeSet<(String, Vec<String>)> = drain(&roster)
            .into_iter()
            .filter_map(|e| match e {
                RosterEvent::Selection {
                    who,
                    selection: Some(x),
                    ..
                } => Some((
                    who.to_string(),
                    x.keys.iter().map(|k| k.to_string()).collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            told,
            BTreeSet::from([
                ("a".to_owned(), vec!["i1".to_owned()]),
                ("b".to_owned(), vec!["i3".to_owned()]),
            ]),
            "subscribers are told"
        );

        // the last key gone: no selection at all
        delete(&s, a, "i1");
        assert_eq!(entry(&s, a, "a").selection, None, "a's last key");
        f.tick();
        assert!(
            drain(&roster).iter().any(|e| matches!(
                e,
                RosterEvent::Selection { who, selection: None, .. } if *who == Who::Pid("a".into())
            )),
            "subscribers are told it is gone"
        );

        // brought back: live again, not selected again until someone selects it
        list_op(&s, b, ListOp::Restore { key: k("i2") });
        assert_eq!(selected(&s, a, "b"), ["i3"], "a restore selects nothing");
        s.set_selection(a, sel(ITEMS, &["i2"])).unwrap();
        assert_eq!(selected(&s, a, "a"), ["i2"]);
    })
}

#[test]
fn an_undo_that_removes_an_element_takes_it_out_of_a_selection() {
    under_mutant(|| {
        let f = fx("undo");
        let s = f.keyed();
        let a = join(&s, "a").conn;
        let b = join(&s, "b").conn;
        list_op(
            &s,
            b,
            ListOp::Insert {
                key: k("i9"),
                at: Anchor::End,
                value: Value::Null,
            },
        );
        s.set_selection(a, sel(ITEMS, &["i1", "i9"])).unwrap();
        s.undo(b, 1).unwrap();
        assert_eq!(selected(&s, a, "a"), ["i1"]);
    })
}

#[test]
fn a_move_keeps_a_selection_and_a_map_write_leaves_it_alone() {
    under_mutant(|| {
        let f = fx("move");
        let s = f.keyed();
        let a = join(&s, "a").conn;
        s.set_selection(a, sel(ITEMS, &["i1", "i3"])).unwrap();
        list_op(
            &s,
            a,
            ListOp::Move {
                key: k("i3"),
                to: Anchor::Start,
            },
        );
        assert_eq!(selected(&s, a, "a"), ["i1", "i3"], "after a move");
        s.edit_ops(
            a,
            vec![Edit::Map {
                root: PROPS.into(),
                ops: vec![k2_mpc_base::elem::MapOp::Set {
                    key: "i1\u{1f}x".into(),
                    value: Value::str("1"),
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
        assert_eq!(selected(&s, a, "a"), ["i1", "i3"], "after a map write");
    })
}

/// A file behind a table, and a session on it with `a` joined as its owner.
fn table_on_file(f: &Fx, body: &str) -> (Arc<Session>, ConnId, PathBuf) {
    let ws = std::fs::canonicalize(f.dir.0.join("ws")).unwrap();
    let path = ws.join("t.csv");
    std::fs::write(&path, body).unwrap();
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("a".into(), Rights::full().with_owner(true));
    let s = f
        .mgr
        .open_file(Arc::new(grid_pack::GridLite), &doc_key("t.csv"), &path, &ws)
        .unwrap();
    let a = join(&s, "a").conn;
    (s, a, path)
}

const THREE_ROWS: &str = "id,city\nk1,Oslo\nk2,Rome\nk3,Lima\n";

fn delete_row(s: &Session, c: ConnId, key: &str) {
    s.edit_ops(
        c,
        vec![Edit::List {
            root: table::ROWS.into(),
            ops: vec![ListOp::Delete { key: k(key) }],
        }],
        &EditOptions::new(),
    )
    .unwrap();
}

/// A table's rows are elements too.
#[test]
fn a_row_deleted_on_a_table_leaves_a_selection() {
    under_mutant(|| {
        let f = fx("table");
        let (s, a, _) = table_on_file(&f, THREE_ROWS);
        s.set_selection(a, sel(table::ROWS, &["r1", "r2", "r3"]))
            .unwrap();
        delete_row(&s, a, "r2");
        assert_eq!(selected(&s, a, "a"), ["r1", "r3"], "a row deleted");
    })
}

/// An outside save goes through its own path.
#[test]
fn a_row_removed_by_an_outside_save_leaves_a_selection() {
    under_mutant(|| {
        let f = fx("outside");
        let (s, a, path) = table_on_file(&f, THREE_ROWS);
        s.set_selection(a, sel(table::ROWS, &["r1", "r2", "r3"]))
            .unwrap();
        std::fs::write(&path, "id,city\nk1,Oslo\n").unwrap();
        let check = s.file_changed().unwrap();
        assert!(matches!(check, DiskCheck::Ingested { .. }), "{check:?}");
        assert_eq!(selected(&s, a, "a"), ["r1"], "an outside save");
    })
}

/// Only a deleted element can be dropped, so it has left every selection by then. A drop keeps
/// it out, and its key can't be selected again.
#[test]
fn a_dropped_row_stays_out_of_a_selection_and_cant_be_selected() {
    under_mutant(|| {
        let f = fx("drop");
        let (s, a, _) = table_on_file(&f, THREE_ROWS);
        let b = join(&s, "b").conn;
        s.set_selection(b, sel(table::ROWS, &["r1", "r2"])).unwrap();
        delete_row(&s, a, "r2");
        s.drop_deleted(a).unwrap();
        assert_eq!(selected(&s, a, "b"), ["r1"], "after a drop");
        assert_eq!(
            id_of(s.set_selection(b, sel(table::ROWS, &["r2"])).unwrap_err()),
            "MPC-E114"
        );
        assert_eq!(selected(&s, a, "b"), ["r1"], "a refusal changes nothing");
    })
}

// ---------- outside the doc's numbering ----------

#[test]
fn nothing_here_takes_or_moves_the_docs_numbering() {
    let f = fx("unnumbered");
    let s = f.keyed();
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("a".into(), Rights::full());
    let a = join(&s, "a").conn;
    let b = join(&s, "b").conn;
    let changes = s.subscribe(b).unwrap();
    let roster = s.subscribe_roster(b).unwrap();
    f.tick();
    let before = s.status().unwrap();
    let log_before = s.log(a, &Default::default()).unwrap().len();

    for i in 0..200 {
        let x = f64::from(i);
        s.set_pointer(a, Some(Point::new(x, x).unwrap())).unwrap();
        s.set_viewport(a, Some(Rect::new(x, 0.0, 10.0, 10.0).unwrap()))
            .unwrap();
        let key = ["i1", "i2", "i3"][i as usize % 3];
        s.set_selection(a, sel(ITEMS, &[key])).unwrap();
        if i % 20 == 0 {
            f.tick();
        }
    }
    s.set_pointer(a, None).unwrap();
    s.set_viewport(a, None).unwrap();
    s.set_selection(a, None).unwrap();
    f.tick();

    let after = s.status().unwrap();
    assert_eq!(after.version, before.version, "the version");
    assert_eq!(after.log_entries, before.log_entries, "the store's log");
    assert_eq!(after.disk, before.disk, "nothing to write back");
    assert_eq!(
        s.log(a, &Default::default()).unwrap().len(),
        log_before,
        "the history"
    );
    assert!(
        s.events_since(a, before.version).unwrap().is_empty(),
        "the live window"
    );
    assert!(changes.try_recv().is_err(), "no change event");
    assert!(
        drain(&roster)
            .iter()
            .any(|e| matches!(e, RosterEvent::Pointer { .. })),
        "the roster did carry them"
    );

    // the next real change takes exactly the next number
    list_op(&s, a, ListOp::Delete { key: k("i4") });
    assert_eq!(s.status().unwrap().version, before.version + 1);
}

// ---------- the roster ----------

#[test]
fn a_pointer_moved_many_times_between_flushes_is_one_event_with_the_latest() {
    let f = fx("coalesce");
    let s = f.keyed();
    let a = join(&s, "a").conn;
    let roster = s.subscribe_roster(a).unwrap();
    f.tick();
    drain(&roster);
    for i in 0..100 {
        let x = f64::from(i);
        s.set_pointer(a, Some(Point::new(x, 0.0).unwrap())).unwrap();
    }
    f.tick();
    let got: Vec<Option<Point>> = drain(&roster)
        .into_iter()
        .filter_map(|e| match e {
            RosterEvent::Pointer { pointer, .. } => Some(pointer),
            _ => None,
        })
        .collect();
    assert_eq!(got, [Some(Point::new(99.0, 0.0).unwrap())]);
}

#[test]
fn each_connection_has_its_own_values_and_the_participants_follow_the_most_active() {
    let f = fx("conns");
    let s = f.keyed();
    let t1 = join(&s, "a");
    let t2 = join(&s, "a");
    let roster = s.subscribe_roster(t1.conn).unwrap();
    f.tick();
    drain(&roster);

    s.set_selection(t1.conn, sel(ITEMS, &["i1"])).unwrap();
    f.host.now.fetch_add(10, Ordering::SeqCst);
    s.set_selection(t2.conn, sel(ITEMS, &["i2"])).unwrap();
    let e = entry(&s, t1.conn, "a");
    assert_eq!(e.selection, sel(ITEMS, &["i2"]), "the most active");
    let of = |c| e.devices.iter().find(|d| d.client == c).unwrap();
    assert_eq!(of(t1.client).selection, sel(ITEMS, &["i1"]));
    assert_eq!(of(t2.client).selection, sel(ITEMS, &["i2"]));

    f.tick();
    let told: Vec<_> = drain(&roster)
        .into_iter()
        .filter_map(|e| match e {
            RosterEvent::Selection {
                selection, client, ..
            } => Some((client, selection)),
            _ => None,
        })
        .collect();
    assert_eq!(
        told,
        [
            (Some(t1.client), sel(ITEMS, &["i1"])),
            (Some(t2.client), sel(ITEMS, &["i2"]))
        ],
        "one a connection, the participant's value last"
    );

    // the most active connection leaves: its value goes, then the participant's own
    s.leave(t2.conn).unwrap();
    f.tick();
    let told: Vec<_> = drain(&roster)
        .into_iter()
        .filter_map(|e| match e {
            RosterEvent::Selection {
                selection, client, ..
            } => Some((client, selection)),
            _ => None,
        })
        .collect();
    assert_eq!(told, [(Some(t2.client), None), (None, sel(ITEMS, &["i1"]))],);
}

#[test]
fn a_late_subscribers_snapshot_holds_them_and_a_grid_row_has_them_with_no_devices() {
    let f = fx("snapshot");
    let s = f.keyed();
    let a = join(&s, "a").conn;
    s.set_selection(a, sel(ITEMS, &["i1"])).unwrap();
    s.set_pointer(a, Some(Point::new(3.0, 4.0).unwrap()))
        .unwrap();
    let late = s.subscribe_roster(a).unwrap();
    let first = late.try_recv().unwrap();
    let RosterEvent::Snapshot { members, .. } = first else {
        panic!("{first:?}");
    };
    assert_eq!(members[0].entry.selection, sel(ITEMS, &["i1"]));
    assert_eq!(members[0].entry.devices[0].selection, sel(ITEMS, &["i1"]));

    let t = f
        .mgr
        .open(
            Arc::new(grid_pack::GridLite),
            &doc_key("g.csv"),
            Some(b"id\nk1\nk2\n"),
        )
        .unwrap();
    let g = join(&t, "g").conn;
    let roster = t.subscribe_roster(g).unwrap();
    f.tick();
    drain(&roster);
    t.set_selection(g, sel(table::ROWS, &["r1"])).unwrap();
    assert_eq!(entry(&t, g, "g").selection, sel(table::ROWS, &["r1"]));
    assert!(entry(&t, g, "g").devices.is_empty());
    f.tick();
    let got: Vec<_> = drain(&roster)
        .into_iter()
        .filter_map(|e| match e {
            RosterEvent::Selection { client, .. } => Some(client),
            _ => None,
        })
        .collect();
    assert_eq!(got, [None], "a grid's event names no connection");
}

#[test]
fn an_app_viewer_sees_a_handle_never_the_pid() {
    let f = fx("app");
    let s = f.keyed();
    let a = join(&s, "someone").conn;
    let v = s
        .join(
            Participant::new("widget", "W", "#123456", ParticipantKind::App, "embed"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let roster = s.subscribe_roster(v).unwrap();
    f.tick();
    drain(&roster);
    s.set_pointer(a, Some(Point::new(1.0, 2.0).unwrap()))
        .unwrap();
    f.tick();
    let who = drain(&roster)
        .into_iter()
        .find_map(|e| match e {
            RosterEvent::Pointer { who, .. } => Some(who),
            _ => None,
        })
        .expect("a pointer event");
    assert!(who.handle().is_some(), "{who:?}");
}
