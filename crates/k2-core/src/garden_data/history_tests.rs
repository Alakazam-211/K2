//! History, restore and snapshot tests (prd-garden-board-needs-v1 §14,
//! "History"). Each test gets its own temp folder; nothing touches `~/.k2`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use serde_json::json;

use super::super::backend::{Cursor, StoreBackend};
use super::*;

struct Fx {
    root: std::path::PathBuf,
    clock: Arc<AtomicI64>,
    store: SqliteStore,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fx() -> Fx {
    let root = crate::test_env::unique_temp_path("gd-history");
    let dir = root.join("stores");
    let clock = Arc::new(AtomicI64::new(1_760_000_000));
    let c = clock.clone();
    let store = SqliteStore::with_clock(&dir, Arc::new(move || c.load(Ordering::SeqCst)));
    Fx { root, clock, store }
}

impl Fx {
    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn put(&self, key: &str, v: Json) -> i64 {
        self.store.put(&board(), key, &v, None, &Actor::Owner).expect("put")
    }
    fn del(&self, key: &str) -> i64 {
        self.store.delete(&board(), key, None, &Actor::Owner).expect("delete").expect("it existed")
    }
    /// Every live key and value.
    fn state(&self) -> BTreeMap<String, Json> {
        let mut out = BTreeMap::new();
        let mut after: Option<String> = None;
        loop {
            let p = self.store.list(&board(), "", after.as_deref(), 500, None).expect("list");
            for e in p.items {
                out.insert(e.key, e.value);
            }
            match p.next {
                Some(n) => after = Some(n),
                None => return out,
            }
        }
    }
    fn conn(&self) -> Connection {
        Connection::open(self.store.path()).expect("open the store file")
    }
}

fn board() -> Space {
    Space::data("board").unwrap()
}

fn all() -> KeySet {
    KeySet::default()
}

fn set(prefixes: &[&str], except: &[&str]) -> KeySet {
    KeySet::new(prefixes.iter().map(|s| s.to_string()).collect(), except.iter().map(|s| s.to_string()).collect(), vec![])
        .expect("a key set")
}

fn anyone(_: &str) -> bool {
    true
}

fn restore(f: &Fx, target: RestoreTarget, s: &KeySet) -> Restored {
    f.store.restore(&board(), &target, s, &anyone, &Actor::Widget { widget: "board".into() }).expect("restore")
}

fn hist(f: &Fx, q: HistoryQuery) -> HistoryPage {
    f.store.history(&board(), &HistoryQuery { limit: q.limit.max(1), ..q }, None).expect("history")
}

fn only<T>(r: Result<T, DataError>) -> DataError {
    match r {
        Ok(_) => panic!("expected a refusal"),
        Err(e) => e,
    }
}

#[test]
fn every_change_keeps_its_before_value() {
    let f = fx();
    f.put("obj/1", json!({"x": 1}));
    f.put("obj/1", json!({"x": 2}));
    let p = super::super::patch::patch(&f.store, &board(), "obj/1", &json!({"y": 3}), None, &Actor::Owner).expect("patch");
    assert!(p.changed);
    f.del("obj/1");
    let h = hist(&f, HistoryQuery { limit: 50, ..Default::default() });
    let rows: Vec<(i64, bool, Json, Json, PrevKind)> =
        h.items.iter().map(|i| (i.rev, i.deleted, i.value.clone(), i.prev.clone(), i.prev_kind)).collect();
    assert_eq!(
        rows,
        vec![
            (4, true, Json::Null, json!({"x": 2, "y": 3}), PrevKind::Value),
            (3, false, json!({"x": 2, "y": 3}), json!({"x": 2}), PrevKind::Value),
            (2, false, json!({"x": 2}), json!({"x": 1}), PrevKind::Value),
            (1, false, json!({"x": 1}), Json::Null, PrevKind::None),
        ],
        "newest first, each with the value it wrote and the one before"
    );
    assert_eq!((h.head, h.oldest), (4, 0), "a whole log restores back to the empty space");
    assert!(h.next.is_none());
}

#[test]
fn history_pages_cross_a_big_rev_without_losing_a_row() {
    let f = fx();
    let ops: Vec<Op> = (0..30).map(|i| Op::Put { key: format!("k/{i:02}"), value: json!(i), if_rev: None }).collect();
    f.store.apply(&board(), &ops, &Actor::Owner).expect("batch");
    f.put("k/00", json!("again"));
    let mut seen = Vec::new();
    let mut q = HistoryQuery { limit: 7, ..Default::default() };
    loop {
        let page = hist(&f, q.clone());
        assert!(page.items.len() <= 7);
        seen.extend(page.items.iter().map(|i| (i.rev, i.key.clone())));
        match page.next {
            Some((before, after)) => q = HistoryQuery { before: Some(before), after, limit: 7, ..Default::default() },
            None => break,
        }
    }
    assert_eq!(seen.len(), 31, "every change once: {seen:?}");
    assert_eq!(seen[0], (2, "k/00".to_string()));
    let mut sorted = seen.clone();
    sorted.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    assert_eq!(seen, sorted, "newest rev first, keys in order inside a rev");
    // Narrowed to one key, or a prefix.
    let one = hist(&f, HistoryQuery { key: Some("k/00".into()), limit: 50, ..Default::default() });
    assert_eq!(one.items.iter().map(|i| i.rev).collect::<Vec<_>>(), vec![2, 1]);
    let pre = hist(&f, HistoryQuery { prefix: "k/2".into(), limit: 50, ..Default::default() });
    assert_eq!(pre.items.len(), 10);
}

#[test]
fn a_filter_hides_keys_outside_the_read_prefixes() {
    let f = fx();
    f.put("pub/a", json!(1));
    f.put("private/b", json!(2));
    let mine = vec!["pub/".to_string()];
    let h = f.store.history(&board(), &HistoryQuery { limit: 50, ..Default::default() }, Some(&mine)).expect("history");
    assert_eq!(h.items.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), vec!["pub/a"]);
}

/// A tiny deterministic generator (no new dependency).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

#[test]
fn restoring_to_any_rev_or_time_gives_exactly_that_state_under_the_prefix() {
    let f = fx();
    let mut g = Lcg(7);
    // The model: the whole state after each rev, and the time of each rev.
    let mut states: Vec<BTreeMap<String, Json>> = vec![BTreeMap::new()];
    let mut times: Vec<i64> = vec![f.now()];
    for step in 0..120 {
        f.advance(60);
        let key = format!("{}/{}", ["board", "settings", "access"][g.next(3) as usize], g.next(6));
        let mut next = states.last().cloned().expect("a state");
        if g.next(4) == 0 && next.contains_key(&key) {
            f.del(&key);
            next.remove(&key);
        } else {
            let v = json!({"step": step, "n": g.next(1000)});
            f.put(&key, v.clone());
            next.insert(key, v);
        }
        states.push(next);
        times.push(f.now());
    }
    let head = (states.len() - 1) as i64;
    for to in [0i64, 1, 17, 59, 60, 99, head - 1, head] {
        let before = f.state();
        let r = restore(&f, RestoreTarget::Rev(to), &set(&["board/"], &[]));
        let after = f.state();
        for (k, v) in &after {
            if k.starts_with("board/") {
                assert_eq!(states[to as usize].get(k), Some(v), "toRev {to}: {k}");
            } else {
                assert_eq!(before.get(k), Some(v), "toRev {to}: {k} is outside the prefix and stays");
            }
        }
        for k in states[to as usize].keys().filter(|k| k.starts_with("board/")) {
            assert!(after.contains_key(k), "toRev {to}: {k} comes back");
        }
        assert_eq!(r.plan.to_rev, Some(to));
        // Put the head back (the restore is in the history too: undo it).
        if let Some(rev) = r.rev {
            restore(&f, RestoreTarget::Rev(rev - 1), &all());
            assert_eq!(f.state(), before, "the restore undoes");
        }
    }
    // By time: the last rev at or before it.
    let t = times[42] + 30;
    let r = restore(&f, RestoreTarget::At(t), &all());
    assert_eq!(r.plan.to_rev, Some(42));
    assert_eq!(f.state(), states[42]);
}

#[test]
fn a_restore_is_marked_everywhere_a_change_is_read() {
    let f = fx();
    f.put("board/b1/obj/1", json!({"x": 1}));
    f.put("board/b1/obj/1", json!({"x": 2}));
    f.put("board/b1/obj/2", json!({"y": 1}));
    let r = restore(&f, RestoreTarget::Rev(1), &set(&["board/b1/"], &[]));
    let id = r.id.clone().expect("a restore id");
    assert!(id.starts_with("rs-") && id.len() == 15, "{id}");
    assert_eq!((r.rev, r.plan.written, r.plan.deleted), (Some(4), 1, 1));
    assert_eq!(r.plan.families, vec!["board/".to_string()]);
    // store.changes (subscribe reads it) carries the mark and the restorer.
    let page = f.store.changes(&board(), &Cursor { since: 3, after: None }, "", None).expect("changes");
    assert_eq!(page.items.len(), 2);
    for c in &page.items {
        assert_eq!(c.restore.as_deref(), Some(id.as_str()), "{c:?}");
        assert_eq!(c.by, "widget:board");
    }
    let plain = f.store.changes(&board(), &Cursor { since: 0, after: None }, "", None).expect("changes");
    assert!(plain.items.iter().filter(|c| c.rev < 4).all(|c| c.restore.is_none()));
    // So does the history.
    let h = hist(&f, HistoryQuery { limit: 10, ..Default::default() });
    assert!(h.items.iter().filter(|i| i.rev == 4).all(|i| i.restore.as_deref() == Some(id.as_str())));
    assert!(h.items.iter().filter(|i| i.rev < 4).all(|i| i.restore.is_none()));
    // And one restores row.
    let n: i64 = f.conn().query_row("SELECT COUNT(*) FROM restores WHERE id = ?1 AND rev = 4", [&id], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn prefixes_and_except_leave_the_versions_alone() {
    let f = fx();
    f.put("board/b1/obj/1", json!(1));
    f.put("board/b1/versions/v1", json!({"label": "first"}));
    f.put("board/b1/obj/1", json!(2));
    f.put("board/b1/versions/v2", json!({"label": "second"}));
    let r = restore(&f, RestoreTarget::Rev(1), &set(&["board/b1/"], &["board/b1/versions/"]));
    assert_eq!((r.plan.written, r.plan.deleted), (1, 0));
    let s = f.state();
    assert_eq!(s.get("board/b1/obj/1"), Some(&json!(1)));
    assert_eq!(s.get("board/b1/versions/v1"), Some(&json!({"label": "first"})));
    assert_eq!(s.get("board/b1/versions/v2"), Some(&json!({"label": "second"})), "except keeps the newer version");
}

#[test]
fn restore_skips_stay_unless_a_prefix_names_them_and_a_dry_run_lists_families() {
    let f = fx();
    f.put("access/boards/b1", json!({"who": ["ann"]}));
    f.put("settings/access", json!({"open": false}));
    f.put("board/b1/obj/1", json!(1));
    f.put("access/boards/b1", json!({"who": ["ann", "bob"]}));
    f.put("settings/access", json!({"open": true}));
    f.put("board/b1/obj/1", json!(2));
    let skips = vec!["access/".to_string(), "settings/access".to_string()];
    let whole = KeySet::new(vec![], vec![], skips.clone()).unwrap();
    let plan = f.store.restore_plan(&board(), &RestoreTarget::Rev(3), &whole, &anyone).expect("plan");
    assert_eq!(plan.families, vec!["board/".to_string()], "skipped families aren't rewritten");
    assert_eq!(plan.keys, vec!["board/b1/obj/1".to_string()]);
    assert_eq!(f.state().get("board/b1/obj/1"), Some(&json!(2)), "a plan writes nothing");
    // Named explicitly in prefixes, a skipped family is restored.
    let named = KeySet::new(vec!["access/".into()], vec![], skips).unwrap();
    restore(&f, RestoreTarget::Rev(3), &named);
    assert_eq!(f.state().get("access/boards/b1"), Some(&json!({"who": ["ann"]})));
    assert_eq!(f.state().get("settings/access"), Some(&json!({"open": true})), "not under the named prefix");
    // Without skips, every family a whole-space restore rewrites is listed.
    let plan = f.store.restore_plan(&board(), &RestoreTarget::Rev(0), &all(), &anyone).expect("plan");
    assert_eq!(plan.families, vec!["access/".to_string(), "board/".to_string(), "settings/".to_string()]);
    assert_eq!(plan.deleted, 3);
}

#[test]
fn a_restore_outside_the_callers_write_rights_writes_nothing() {
    let f = fx();
    f.put("pub/a", json!(1));
    f.put("private/b", json!(1));
    f.put("pub/a", json!(2));
    f.put("private/b", json!(2));
    let only_pub = |k: &str| k.starts_with("pub/");
    let e = only(f.store.restore(&board(), &RestoreTarget::Rev(2), &all(), &only_pub, &Actor::Owner));
    assert_eq!(e.code(), "not_granted");
    assert_eq!(f.state().get("pub/a"), Some(&json!(2)), "nothing was written");
    let r = f.store.restore(&board(), &RestoreTarget::Rev(2), &set(&["pub/"], &[]), &only_pub, &Actor::Owner).expect("pub only");
    assert_eq!(r.plan.written, 1);
}

#[test]
fn unknown_before_values_and_a_reaped_log_bound_the_restore() {
    let f = fx();
    f.put("a", json!(1));
    f.put("a", json!(2));
    // An older K2 wrote rev 3 (its insert names only the six old columns).
    {
        let c = f.conn();
        c.execute("UPDATE spaces SET rev = 3 WHERE space = 'data:board'", []).unwrap();
        c.execute(
            "UPDATE entries SET rev = 3, value = '3', size = 1 WHERE space = 'data:board' AND key = 'a'",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO changes (space, rev, key, deleted, by, at) VALUES ('data:board', 3, 'a', 0, 'owner', ?1)", [f.now()])
            .unwrap();
    }
    f.put("a", json!(4));
    let (head, oldest) = f.store.restorable(&board()).unwrap();
    assert_eq!((head, oldest), (4, 3), "rev 3's before-value is unknown, so 3 is as far back as it goes");
    let e = only(f.store.restore(&board(), &RestoreTarget::Rev(2), &all(), &anyone, &Actor::Owner));
    assert_eq!((e.code(), e.status()), ("no_history", 409));
    assert!(e.hint().contains("k2 zen data export --at"), "{}", e.hint());
    restore(&f, RestoreTarget::Rev(3), &all());
    assert_eq!(f.state().get("a"), Some(&json!(3)));
    // The reaper takes the log by age: what's left is whole from its first row.
    f.advance(CHANGE_LOG_KEEP_SECS + 10);
    f.put("a", json!(6));
    f.store.reap(f.now()).expect("reap");
    let (head, oldest) = f.store.restorable(&board()).unwrap();
    assert_eq!((head, oldest), (6, 5));
    // A time before the first change kept, once rows were reaped: unknown.
    let e = only(f.store.restore(&board(), &RestoreTarget::At(f.now() - 365 * 86_400), &all(), &anyone, &Actor::Owner));
    assert_eq!(e.code(), "no_history");
}

#[test]
fn the_trim_clears_the_oldest_before_values_and_moves_oldest_up() {
    let f = fx();
    let big = "x".repeat(10_000);
    for i in 0..10 {
        f.put("doc", json!({"i": i, "pad": big}));
    }
    let (_, oldest) = f.store.restorable(&board()).unwrap();
    assert_eq!(oldest, 0);
    // About 9 before-values of ~10 KB; keep 40 KB.
    let n = f.store.trim_history(40_000).expect("trim");
    assert!(n >= 5, "{n} rows trimmed");
    let kept: i64 = f
        .conn()
        .query_row("SELECT SUM(LENGTH(CAST(prev AS BLOB))) FROM changes WHERE space = 'data:board'", [], |r| r.get(0))
        .unwrap();
    assert!(kept <= 40_000, "{kept}");
    let rows: i64 = f.conn().query_row("SELECT COUNT(*) FROM changes WHERE space = 'data:board'", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 10, "the rows stay; only their before-values go");
    let (_, oldest) = f.store.restorable(&board()).unwrap();
    assert!(oldest >= 5, "{oldest}");
    let h = hist(&f, HistoryQuery { limit: 50, ..Default::default() });
    assert_eq!(h.items.last().map(|i| i.prev_kind), Some(PrevKind::Unknown));
    assert_eq!(h.oldest, oldest);
    // Under budget: nothing to trim.
    assert_eq!(f.store.trim_history(MAX_HISTORY_BYTES).unwrap(), 0);
}

#[test]
fn a_snapshot_round_trips_and_restores_only_its_own_keys() {
    let f = fx();
    f.put("board/b1/obj/1", json!({"x": 1}));
    f.put("board/b1/obj/2", json!({"x": 2}));
    f.put("board/b1/versions/v0", json!({"label": "old"}));
    f.put("prefs/zoom", json!(1));
    let s = set(&["board/b1/"], &["board/b1/versions/"]);
    let (bytes, rev, n) = f.store.snapshot_doc(&board(), &s, Some("before cleanup"), &anyone).expect("snapshot");
    assert_eq!((rev, n), (4, 2));
    let info = f.store.blob_put(&board(), &bytes, Some("snapshot.json"), &Actor::Owner).expect("kept as a blob");
    assert_eq!(info.mime, "application/json");
    // Change everything: inside, a version, and outside.
    f.put("board/b1/obj/1", json!({"x": 100}));
    f.del("board/b1/obj/2");
    f.put("board/b1/obj/3", json!({"x": 3}));
    f.put("board/b1/versions/v1", json!({"label": "new"}));
    f.put("prefs/zoom", json!(2));
    let doc = parse_snapshot(&info.hash(), &bytes, &board()).expect("a snapshot");
    assert_eq!(doc.items.len(), 2);
    let r = restore(&f, RestoreTarget::Snapshot(doc.clone()), &all());
    assert_eq!((r.plan.written, r.plan.deleted), (2, 1));
    let st = f.state();
    assert_eq!(st.get("board/b1/obj/1"), Some(&json!({"x": 1})));
    assert_eq!(st.get("board/b1/obj/2"), Some(&json!({"x": 2})));
    assert!(!st.contains_key("board/b1/obj/3"), "a key made after the snapshot, inside its set, goes");
    assert_eq!(st.get("board/b1/versions/v1"), Some(&json!({"label": "new"})), "except: the versions survive");
    assert_eq!(st.get("prefs/zoom"), Some(&json!(2)), "outside the set: untouched");
    assert_eq!(r.plan.source["snapshot"], json!(ref_of_hash(&info.hash())));
    // R1: another key set is refused; the same one is fine.
    let e = only(f.store.restore_plan(&board(), &RestoreTarget::Snapshot(doc.clone()), &set(&["board/"], &[]), &anyone));
    assert_eq!(e.code(), "bad_request");
    assert!(e.hint().contains("board/b1/"), "{}", e.hint());
    assert!(f.store.restore_plan(&board(), &RestoreTarget::Snapshot(doc), &s, &anyone).is_ok());
    // Another widget's snapshot, or not a snapshot at all.
    let other = Space::data("notes").unwrap();
    assert_eq!(only(parse_snapshot("h", &bytes, &other)).code(), "bad_request");
    assert_eq!(only(parse_snapshot("h", b"{\"format\":\"x\"}", &board())).code(), "bad_request");
}

fn ref_of_hash(h: &str) -> String {
    super::super::blobs::ref_of(h)
}

trait HashOf {
    fn hash(&self) -> String;
}
impl HashOf for super::super::blobs::BlobInfo {
    fn hash(&self) -> String {
        super::super::blobs::hash_of_ref(&self.reference).expect("a ref").to_string()
    }
}

#[test]
fn a_snapshot_reader_must_read_every_key_it_covers() {
    let f = fx();
    f.put("pub/a", json!(1));
    f.put("private/b", json!(1));
    let e = only(f.store.snapshot_doc(&board(), &all(), None, &|k: &str| k.starts_with("pub/")));
    assert_eq!(e.code(), "not_granted");
    let (_, _, n) = f.store.snapshot_doc(&board(), &set(&["pub/"], &[]), None, &|k: &str| k.starts_with("pub/")).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn restored_blob_refs_pass_the_held_check() {
    let f = fx();
    let png = b"\x89PNG\r\n\x1a\nfake image bytes".to_vec();
    let info = f.store.blob_put(&board(), &png, Some("a.png"), &Actor::Owner).unwrap();
    f.put("board/b1/obj/1", json!({"src": info.reference}));
    f.del("board/b1/obj/1");
    // Still held as let-go (35 days): the restore names it again.
    restore(&f, RestoreTarget::Rev(1), &all());
    assert_eq!(f.state().get("board/b1/obj/1"), Some(&json!({"src": info.reference})));
    let row = f.store.blob_row(&board(), &info.hash()).unwrap().expect("held");
    assert_eq!(row.refs, 1, "the restore moved the ref count like any write");
}

#[test]
fn key_sets_check_their_lists() {
    assert_eq!(only(KeySet::new(vec![String::new()], vec![], vec![])).code(), "bad_request");
    let many: Vec<String> = (0..17).map(|i| format!("p{i}/")).collect();
    assert_eq!(only(KeySet::new(many, vec![], vec![])).code(), "bad_request");
    let s = KeySet::new(vec!["b/".into(), "a/".into(), "a/".into()], vec![], vec![]).unwrap();
    assert_eq!(s.prefixes, vec!["a/".to_string(), "b/".to_string()], "sorted, once each");
    assert_eq!(family("board/b1/obj"), "board/");
    assert_eq!(family("prefs"), "prefs");
}

#[test]
fn the_columns_are_added_beside_schema_version_one() {
    let f = fx();
    f.put("a", json!(1));
    let c = f.conn();
    let v: String = c.query_row("SELECT v FROM _k2_meta WHERE k = 'schema_version'", [], |r| r.get(0)).unwrap();
    assert_eq!(v, "1", "V6: the schema version stays 1, so an older K2 still opens the file");
    for col in ["prev", "prev_kind", "restore_id"] {
        let n: i64 =
            c.query_row("SELECT COUNT(*) FROM pragma_table_info('changes') WHERE name = ?1", [col], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "{col}");
    }
    // An older K2's insert (the six version-1 columns) still works.
    c.execute("INSERT INTO changes (space, rev, key, deleted, by, at) VALUES ('data:board', 9, 'z', 0, 'owner', 1)", []).unwrap();
}

#[test]
fn deleting_and_moving_a_space_carries_its_restores() {
    let f = fx();
    f.put("a", json!(1));
    f.put("a", json!(2));
    restore(&f, RestoreTarget::Rev(1), &all());
    let to = Space::data("board2").unwrap();
    f.store.move_space(&board(), &to).expect("move");
    let n = |space: &str| -> i64 {
        f.conn().query_row("SELECT COUNT(*) FROM restores WHERE space = ?1", [space], |r| r.get(0)).unwrap()
    };
    assert_eq!((n("data:board"), n("data:board2")), (0, 1));
    f.store.drop_space(&to).expect("drop");
    assert_eq!(n("data:board2"), 0);
}
