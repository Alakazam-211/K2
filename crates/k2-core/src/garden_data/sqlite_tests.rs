//! `SqliteStore` tests (contract §2.2, §3.3, §12.1). Each test gets its own
//! temp folder; nothing touches `~/.k2`.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::json;

use super::*;

struct Fixture {
    dir: PathBuf,
    clock: Arc<AtomicI64>,
    store: SqliteStore,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(root) = self.dir.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn fixture() -> Fixture {
    let dir = crate::test_env::unique_temp_path("gd-sqlite").join("stores");
    let clock = Arc::new(AtomicI64::new(1_760_000_000));
    let c = clock.clone();
    let store = SqliteStore::with_clock(&dir, Arc::new(move || c.load(Ordering::SeqCst)));
    Fixture { dir, clock, store }
}

fn board() -> Space {
    Space::data("board").unwrap()
}

fn owner() -> Actor {
    Actor::Owner
}

fn put(op_key: &str, v: Json) -> Op {
    Op::Put { key: op_key.into(), value: v, if_rev: None }
}

#[test]
fn file_and_folder_are_private_and_a_newer_schema_is_refused() {
    let f = fixture();
    f.store.put(&board(), "obj/1", &json!({"x": 1}), None, &owner()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dm = std::fs::metadata(&f.dir).unwrap().permissions().mode() & 0o777;
        let fm = std::fs::metadata(f.store.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!((dm, fm), (0o700, 0o600));
    }
    assert!(f.store.is_open());
    // A newer K2 wrote the file: refuse, never downgrade it.
    let conn = Connection::open(f.store.path()).unwrap();
    conn.execute("UPDATE _k2_meta SET v = '2' WHERE k = 'schema_version'", []).unwrap();
    drop(conn);
    let fresh = SqliteStore::new(&f.dir);
    let e = fresh.get(&board(), "obj/1").unwrap_err();
    assert_eq!(e.code(), "store_unavailable", "{e}");
}

#[test]
fn put_get_and_if_rev_rules() {
    let f = fixture();
    let s = board();
    let r1 = f.store.put(&s, "obj/1", &json!({"a": 1}), Some(0), &owner()).unwrap();
    assert_eq!(r1, 1);
    let e = f.store.put(&s, "obj/1", &json!({"a": 2}), Some(0), &owner()).unwrap_err();
    assert_eq!(e, DataError::Conflict { key: "obj/1".into(), rev: 1, op: None }, "create-only on an existing key");
    let e = f.store.put(&s, "obj/1", &json!({"a": 2}), Some(7), &owner()).unwrap_err();
    assert_eq!(e, DataError::Conflict { key: "obj/1".into(), rev: 1, op: None });
    let r2 = f.store.put(&s, "obj/1", &json!({"a": 2}), Some(1), &Actor::Agent { project_id: "w-1".into() }).unwrap();
    assert_eq!(r2, 2);
    let got = f.store.get(&s, "obj/1").unwrap().expect("entry");
    assert_eq!((got.rev, got.value.clone(), got.by.as_str()), (2, json!({"a": 2}), "agent:w-1"));
    assert_eq!(f.store.get(&s, "obj/2").unwrap(), None);
    // ifRev on a missing key: only 0 matches.
    let e = f.store.put(&s, "obj/2", &json!(1), Some(1), &owner()).unwrap_err();
    assert_eq!(e, DataError::Conflict { key: "obj/2".into(), rev: 0, op: None });
}

#[test]
fn delete_leaves_a_tombstone_and_missing_deletes_bump_nothing() {
    let f = fixture();
    let s = board();
    f.store.put(&s, "obj/1", &json!(1), None, &owner()).unwrap();
    let e = f.store.delete(&s, "obj/1", Some(9), &owner()).unwrap_err();
    assert_eq!(e, DataError::Conflict { key: "obj/1".into(), rev: 1, op: None });
    assert_eq!(f.store.delete(&s, "obj/1", Some(1), &owner()).unwrap(), Some(2));
    assert_eq!(f.store.get(&s, "obj/1").unwrap(), None);
    assert_eq!(f.store.delete(&s, "obj/1", None, &owner()).unwrap(), None, "already gone");
    assert_eq!(f.store.delete(&s, "nope", None, &owner()).unwrap(), None);
    let info = f.store.info(&s).unwrap().unwrap();
    assert_eq!((info.rev, info.keys, info.bytes), (2, 0, 0));
    // The tombstone counts as missing for ifRev 0.
    assert_eq!(f.store.put(&s, "obj/1", &json!(3), Some(0), &owner()).unwrap(), 3);
}

#[test]
fn value_and_key_limits() {
    let f = fixture();
    let s = board();
    // Exactly 60 KiB of JSON text passes; one byte more doesn't.
    let fits = "x".repeat(MAX_VALUE_BYTES - 2);
    f.store.put(&s, "big", &json!(fits), None, &owner()).unwrap();
    let over = "x".repeat(MAX_VALUE_BYTES - 1);
    assert_eq!(
        f.store.put(&s, "big", &json!(over), None, &owner()).unwrap_err(),
        DataError::ValueTooLarge { bytes: MAX_VALUE_BYTES + 1 }
    );
    for bad in ["", "/x", "a//b", "a/../b", "a b"] {
        assert_eq!(f.store.put(&s, bad, &json!(1), None, &owner()).unwrap_err().code(), "bad_key", "{bad:?}");
    }
}

#[test]
fn batch_is_all_or_nothing_at_one_rev() {
    let f = fixture();
    let s = board();
    f.store.put(&s, "obj/1", &json!(1), None, &owner()).unwrap();
    let ops = vec![put("obj/2", json!(2)), Op::Put { key: "obj/1".into(), value: json!(9), if_rev: Some(5) }];
    assert_eq!(
        f.store.apply(&s, &ops, &owner()).unwrap_err(),
        DataError::Conflict { key: "obj/1".into(), rev: 1, op: Some(1) }
    );
    assert_eq!(f.store.get(&s, "obj/2").unwrap(), None, "nothing written on a conflict");
    assert_eq!(f.store.info(&s).unwrap().unwrap().rev, 1);
    let ops = vec![put("obj/2", json!(2)), put("obj/3", json!(3)), Op::Delete { key: "obj/1".into(), if_rev: Some(1) }];
    assert_eq!(f.store.apply(&s, &ops, &owner()).unwrap(), 2);
    for k in ["obj/2", "obj/3"] {
        assert_eq!(f.store.get(&s, k).unwrap().unwrap().rev, 2);
    }
    let dup = vec![put("a", json!(1)), put("a", json!(2))];
    assert_eq!(f.store.apply(&s, &dup, &owner()).unwrap_err().code(), "bad_request");
    assert_eq!(f.store.apply(&s, &[], &owner()).unwrap_err().code(), "bad_request");
}

#[test]
fn counters_stay_exact() {
    let f = fixture();
    let s = board();
    f.store.put(&s, "a", &json!("12345"), None, &owner()).unwrap(); // "\"12345\"" = 7
    f.store.put(&s, "b", &json!(1), None, &owner()).unwrap(); // 1
    f.store.put(&s, "a", &json!("1"), None, &owner()).unwrap(); // 3
    f.store.apply(&s, &[put("c", json!([1, 2])), Op::Delete { key: "b".into(), if_rev: None }], &owner()).unwrap();
    let info = f.store.info(&s).unwrap().unwrap();
    assert_eq!((info.keys, info.bytes), (2, 3 + 5));
}

#[test]
fn space_and_save_limits_by_keys() {
    let f = fixture();
    let save = Space::save("arcade").unwrap();
    let ops: Vec<Op> = (0..MAX_SAVE_KEYS).map(|i| put(&format!("k{i}"), json!(1))).collect();
    f.store.apply(&save, &ops, &owner()).unwrap();
    assert_eq!(
        f.store.put(&save, "one-more", &json!(1), None, &owner()).unwrap_err(),
        DataError::SpaceFull { what: "keys" }
    );
    // An overwrite at the cap is fine.
    f.store.put(&save, "k0", &json!(2), None, &owner()).unwrap();
}

#[test]
fn space_limit_by_bytes() {
    let f = fixture();
    let save = Space::save("arcade").unwrap();
    let chunk = json!("y".repeat(50 * 1024));
    let n = (MAX_SAVE_BYTES as usize) / (50 * 1024 + 2);
    for i in 0..n {
        f.store.put(&save, &format!("p{i}"), &chunk, None, &owner()).unwrap();
    }
    assert_eq!(
        f.store.put(&save, "over", &chunk, None, &owner()).unwrap_err(),
        DataError::SpaceFull { what: "bytes" }
    );
}

#[test]
fn store_full_refuses_writes_but_not_deletes() {
    let dir = crate::test_env::unique_temp_path("gd-full").join("stores");
    let store = SqliteStore::new(&dir).with_max_file_bytes(1);
    assert_eq!(store.put(&board(), "a", &json!(1), None, &owner()).unwrap_err(), DataError::StoreFull);
    assert_eq!(store.delete(&board(), "a", None, &owner()).unwrap(), None);
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn list_pages_by_count_and_bytes_and_never_leaks_past_the_filter() {
    let f = fixture();
    let s = board();
    let ops: Vec<Op> = (0..30)
        .map(|i| put(&format!("{}/{i:02}", if i % 2 == 0 { "p7" } else { "p8" }), json!(i)))
        .collect();
    f.store.apply(&s, &ops, &owner()).unwrap();
    let p = f.store.list(&s, "", None, 10, None).unwrap();
    assert_eq!(p.items.len(), 10);
    assert_eq!(p.next.as_deref(), Some(p.items[9].key.as_str()));
    // Walk every page: all 30, in order, once.
    let mut all = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let p = f.store.list(&s, "", after.as_deref(), 7, None).unwrap();
        all.extend(p.items.iter().map(|e| e.key.clone()));
        match p.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!((all.len(), all.clone()), (30, sorted));
    // Prefix and filter.
    let p = f.store.list(&s, "p7/", None, 500, None).unwrap();
    assert_eq!(p.items.len(), 15);
    assert!(p.items.iter().all(|e| e.key.starts_with("p7/")));
    let only = vec!["p8/".to_string()];
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let p = f.store.list(&s, "", after.as_deref(), 4, Some(&only)).unwrap();
        assert!(p.items.iter().all(|e| e.key.starts_with("p8/")), "filter leak");
        seen.extend(p.items.iter().map(|e| e.key.clone()));
        match p.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    assert_eq!(seen.len(), 15);
    assert!(f.store.list(&s, "", None, 500, Some(&[])).unwrap().items.is_empty(), "no grants, no keys");
    // Bytes: 2 × 40 KiB values never share a 60 KiB page.
    let t = Space::data("big").unwrap();
    f.store.put(&t, "a", &json!("z".repeat(40 * 1024)), None, &owner()).unwrap();
    f.store.put(&t, "b", &json!("z".repeat(40 * 1024)), None, &owner()).unwrap();
    let p = f.store.list(&t, "", None, 500, None).unwrap();
    assert_eq!((p.items.len(), p.next.as_deref()), (1, Some("a")));
    // A missing space lists empty.
    assert!(f.store.list(&Space::data("ghost").unwrap(), "", None, 10, None).unwrap().items.is_empty());
}

#[test]
fn changes_follow_rev_order_with_cursors_and_whole_revs() {
    let f = fixture();
    let s = board();
    f.store.put(&s, "a", &json!(1), None, &owner()).unwrap(); // rev 1
    f.store.apply(&s, &[put("b", json!(2)), put("c", json!(3))], &owner()).unwrap(); // rev 2
    f.store.delete(&s, "a", None, &owner()).unwrap(); // rev 3
    let p = f.store.changes(&s, &Cursor { since: 0, after: None }, "", None).unwrap();
    assert_eq!(p.head, 3);
    let got: Vec<(String, i64, bool)> = p.items.iter().map(|c| (c.key.clone(), c.rev, c.deleted)).collect();
    assert_eq!(got, vec![("b".into(), 2, false), ("c".into(), 2, false), ("a".into(), 3, true)]);
    assert_eq!(p.items[2].value, Json::Null);
    assert!(p.next.is_none() && !p.partial);
    let p = f.store.changes(&s, &Cursor { since: 2, after: None }, "", None).unwrap();
    assert_eq!(p.items.len(), 1);
    let p = f.store.changes(&s, &Cursor { since: 2, after: Some("b".into()) }, "", None).unwrap();
    assert_eq!(p.items.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), vec!["c", "a"]);
    let p = f.store.changes(&s, &Cursor { since: 0, after: None }, "", Some(&["b".to_string()])).unwrap();
    assert_eq!(p.items.len(), 1, "filtered");
    let ghost = f.store.changes(&Space::data("ghost").unwrap(), &Cursor { since: 0, after: None }, "", None).unwrap();
    assert_eq!((ghost.head, ghost.items.len()), (0, 0));
}

#[test]
fn changes_pages_never_split_a_rev_unless_it_alone_is_over_a_page() {
    let f = fixture();
    let s = board();
    // rev 1: 300 small keys; rev 2: 300 small keys. 600 > one page of 500.
    let r1: Vec<Op> = (0..300).map(|i| put(&format!("r1/{i:03}"), json!(i))).collect();
    let r2: Vec<Op> = (0..300).map(|i| put(&format!("r2/{i:03}"), json!(i))).collect();
    f.store.apply(&s, &r1, &owner()).unwrap();
    f.store.apply(&s, &r2, &owner()).unwrap();
    let p = f.store.changes(&s, &Cursor { since: 0, after: None }, "", None).unwrap();
    assert_eq!(p.items.len(), 300, "rev 2 doesn't fit whole, so it moves to the next page");
    assert!(!p.partial);
    assert_eq!(p.next, Some(Cursor { since: 1, after: None }));
    let p2 = f.store.changes(&s, p.next.as_ref().unwrap(), "", None).unwrap();
    assert_eq!((p2.items.len(), p2.next.clone(), p2.partial), (300, None, false));
    // One rev of 700 keys: split, partial, then the rest.
    let big: Vec<Op> = (0..700).map(|i| put(&format!("r3/{i:03}"), json!(i))).collect();
    f.store.apply(&s, &big, &owner()).unwrap();
    let p = f.store.changes(&s, &Cursor { since: 2, after: None }, "", None).unwrap();
    assert_eq!(p.items.len(), CHANGES_PAGE_ITEMS);
    assert!(p.partial);
    let rest = f.store.changes(&s, p.next.as_ref().unwrap(), "", None).unwrap();
    assert_eq!((rest.items.len(), rest.partial, rest.next), (200, false, None));
}

#[test]
fn reaper_removes_old_tombstones_and_raises_reaped_rev() {
    let f = fixture();
    let s = board();
    f.store.put(&s, "a", &json!(1), None, &owner()).unwrap();
    f.store.delete(&s, "a", None, &owner()).unwrap(); // tombstone at rev 2
    f.store.put(&s, "b", &json!(1), None, &owner()).unwrap(); // rev 3
    let now = f.clock.load(Ordering::SeqCst);
    let r = f.store.reap(now).unwrap();
    assert_eq!(r.tombstones, 0, "too young");
    f.clock.store(now + TOMBSTONE_KEEP_SECS + 1, Ordering::SeqCst);
    let r = f.store.reap(now + TOMBSTONE_KEEP_SECS + 1).unwrap();
    assert_eq!((r.tombstones, r.log_rows), (1, 3));
    assert_eq!(f.store.info(&s).unwrap().unwrap().reaped_rev, 2);
    assert_eq!(
        f.store.changes(&s, &Cursor { since: 1, after: None }, "", None).unwrap_err(),
        DataError::Resync { reaped_rev: 2 }
    );
    let ok = f.store.changes(&s, &Cursor { since: 2, after: None }, "", None).unwrap();
    assert_eq!(ok.items.len(), 1);
}

#[test]
fn drop_move_and_orphans() {
    let f = fixture();
    let old = Space::data("board-old").unwrap();
    let new = Space::data("board").unwrap();
    f.store.put(&old, "a", &json!(1), None, &owner()).unwrap();
    f.store.put(&new, "x", &json!(1), None, &owner()).unwrap();
    assert_eq!(f.store.move_space(&old, &new).unwrap_err().code(), "space_exists");
    f.store.drop_space(&new).unwrap();
    assert!(f.store.info(&new).unwrap().is_none());
    f.store.move_space(&old, &new).unwrap();
    assert_eq!(f.store.get(&new, "a").unwrap().unwrap().value, json!(1));
    assert!(f.store.info(&old).unwrap().is_none());
    assert_eq!(f.store.move_space(&old, &new).unwrap_err().code(), "space_not_found");
    let marked = f.store.mark_orphans(&["other".to_string()], 5).unwrap();
    assert_eq!(marked, vec!["board".to_string()]);
    assert_eq!(f.store.info(&new).unwrap().unwrap().orphaned_at, Some(5));
    assert!(f.store.mark_orphans(&["board".to_string()], 6).unwrap().is_empty());
    assert_eq!(f.store.info(&new).unwrap().unwrap().orphaned_at, None);
    let s = f.store.spaces().unwrap();
    assert_eq!(s.iter().map(|i| i.widget.as_str()).collect::<Vec<_>>(), vec!["board"]);
}

#[test]
fn writes_since_counts_one_actor_and_everyone_from_the_change_log() {
    let f = fixture();
    let s = board();
    let w = Actor::Widget { widget: "board".into() };
    f.store.put(&s, "obj/1", &json!(1), None, &w).unwrap();
    f.store.put(&s, "obj/2", &json!(2), None, &w).unwrap();
    f.store.put(&s, "obj/3", &json!(3), None, &Actor::Agent { project_id: "w-1".into() }).unwrap();
    f.store.put(&Space::data("other").unwrap(), "obj/1", &json!(1), None, &w).unwrap();
    let start = f.store.now();
    assert_eq!(f.store.writes_since(&s, &w.stored(), start - 10).unwrap(), (2, 3), "this space only");
    f.clock.store(start + 100, Ordering::SeqCst);
    f.store.delete(&s, "obj/1", None, &w).unwrap();
    assert_eq!(f.store.writes_since(&s, &w.stored(), start + 50).unwrap(), (1, 1), "only since the time");
    assert_eq!(f.store.writes_since(&board(), "widget:nobody", 0).unwrap(), (0, 4));
}

// ── 0.46.5: the patch op in batches and transactions (Board via Appa) ──

fn patch_op(op_key: &str, partial: Json, if_rev: Option<i64>) -> Op {
    Op::Patch { key: op_key.into(), value: partial, if_rev }
}

/// Every change-log row after `since`, as `(key, rev, deleted, by)`.
fn feed(f: &Fixture, since: i64) -> Vec<(String, i64, bool, String)> {
    let p = f.store.changes(&board(), &Cursor { since, after: None }, "", None).unwrap();
    assert!(p.next.is_none(), "one page in these tests");
    p.items.into_iter().map(|c| (c.key, c.rev, c.deleted, c.by)).collect()
}

/// The space's live keys and values, for "nothing changed" checks.
fn snapshot(f: &Fixture) -> (i64, Vec<(String, i64, Json)>) {
    let rev = f.store.info(&board()).unwrap().map_or(0, |i| i.rev);
    let items = f.store.list(&board(), "", None, 500, None).unwrap().items;
    (rev, items.into_iter().map(|e| (e.key, e.rev, e.value)).collect())
}

#[test]
fn a_batch_mixing_put_patch_and_delete_lands_at_one_rev() {
    let f = fixture();
    let s = board();
    let r1 = f.store.put(&s, "obj/1", &json!({"t": "hi", "x": 1, "keep": {"a": 1, "b": 2}}), None, &owner()).unwrap();
    let r2 = f.store.put(&s, "obj/2", &json!({"gone": true}), None, &owner()).unwrap();
    let w = Actor::Widget { widget: "board".into() };
    let ops = vec![
        put("obj/3", json!({"n": 3})),
        patch_op("obj/1", json!({"x": 2, "t": null, "keep": {"b": null, "c": 3}}), Some(r1)),
        Op::Delete { key: "obj/2".into(), if_rev: Some(r2) },
        patch_op("obj/new", json!({"a": 1, "z": null}), Some(0)),
    ];
    let rev = f.store.apply(&s, &ops, &w).unwrap();
    assert_eq!(rev, r2 + 1, "one new rev for the whole set");

    // The same merge as a single store.patch: run each partial through
    // patch::patch on a twin space and compare the documents.
    let twin = Space::data("twin").unwrap();
    f.store.put(&twin, "obj/1", &json!({"t": "hi", "x": 1, "keep": {"a": 1, "b": 2}}), None, &owner()).unwrap();
    let single = super::super::patch::patch(&f.store, &twin, "obj/1", &json!({"x": 2, "t": null, "keep": {"b": null, "c": 3}}), None, &w).unwrap();
    let one = f.store.get(&s, "obj/1").unwrap().expect("obj/1");
    assert_eq!(one.value, single.value, "batch patch = single patch");
    assert_eq!(one.value, json!({"x": 2, "keep": {"a": 1, "c": 3}}));
    let fresh = f.store.get(&s, "obj/new").unwrap().expect("obj/new");
    assert_eq!(fresh.value, json!({"a": 1}), "a missing key starts from {{}}");
    assert_eq!(f.store.get(&s, "obj/2").unwrap(), None);
    for k in ["obj/1", "obj/3", "obj/new"] {
        let e = f.store.get(&s, k).unwrap().expect(k);
        assert_eq!((e.rev, e.by.as_str()), (rev, "widget:board"), "{k}");
    }

    // The change log: one row per key, all at the batch's rev, by its actor.
    let mut got = feed(&f, r2);
    got.sort();
    let by = "widget:board".to_string();
    assert_eq!(
        got,
        vec![
            ("obj/1".to_string(), rev, false, by.clone()),
            ("obj/2".to_string(), rev, true, by.clone()),
            ("obj/3".to_string(), rev, false, by.clone()),
            ("obj/new".to_string(), rev, false, by.clone()),
        ]
    );
    assert_eq!(f.store.writes_since(&s, &by, 0).unwrap(), (4, 6), "four ops logged for the widget, six in all");

    // Counters stay exact: keys and bytes equal the stored texts.
    let info = f.store.info(&s).unwrap().unwrap();
    let live = f.store.list(&s, "", None, 500, None).unwrap().items;
    let bytes: i64 = live.iter().map(|e| serde_json::to_string(&e.value).unwrap().len() as i64).sum();
    assert_eq!((info.rev, info.keys, info.bytes), (rev, 3, bytes));
}

#[test]
fn a_stale_if_rev_on_one_patch_rolls_back_the_whole_batch() {
    let f = fixture();
    let s = board();
    let r1 = f.store.put(&s, "obj/1", &json!({"t": 1}), None, &owner()).unwrap();
    let r2 = f.store.put(&s, "obj/2", &json!({"t": 2}), None, &owner()).unwrap();
    let before = snapshot(&f);
    let ops = vec![
        put("obj/9", json!(9)),
        patch_op("obj/2", json!({"t": 22}), Some(r2)),
        patch_op("obj/1", json!({"t": 11}), Some(r1 + 40)),
        Op::Delete { key: "obj/2x".into(), if_rev: None },
    ];
    let e = f.store.apply(&s, &ops, &owner()).unwrap_err();
    assert_eq!(e, DataError::Conflict { key: "obj/1".into(), rev: r1, op: Some(2) });
    let env = e.envelope();
    assert_eq!((env["error"]["op"].as_u64(), env["error"]["key"].as_str()), (Some(2), Some("obj/1")), "{env}");
    assert!(env["error"]["hint"].as_str().unwrap().starts_with("op 2 ('obj/1')"), "{env}");
    assert_eq!(snapshot(&f), before, "nothing written, no new rev");
    assert!(feed(&f, r2).is_empty(), "no change-log rows");
    // ifRev 0 on a patch is create-only, like put.
    let e = f.store.apply(&s, &[put("obj/9", json!(9)), patch_op("obj/1", json!({"a": 1}), Some(0))], &owner()).unwrap_err();
    assert_eq!(e, DataError::Conflict { key: "obj/1".into(), rev: r1, op: Some(1) });
    assert_eq!(snapshot(&f), before);
}

#[test]
fn an_invalid_patch_rolls_back_the_whole_batch_and_names_its_op() {
    let f = fixture();
    let s = board();
    f.store.put(&s, "obj/1", &json!({"a": "x".repeat(40 * 1024)}), None, &owner()).unwrap();
    f.store.put(&s, "list", &json!([1, 2]), None, &owner()).unwrap();
    let before = snapshot(&f);
    let bad = |op: Op| vec![put("obj/9", json!(9)), Op::Delete { key: "list".into(), if_rev: None }, op];
    // A partial that isn't an object (refused before anything is read).
    for partial in [json!([1]), json!("x"), json!(3), json!(null)] {
        match f.store.apply(&s, &bad(patch_op("obj/1", partial.clone(), None)), &owner()) {
            Err(DataError::BadRequest(m)) => {
                assert!(m.starts_with("op 2 ('obj/1'): ") && m.contains("object"), "{partial}: {m}")
            }
            other => panic!("{partial}: want bad_request, got {other:?}"),
        }
    }
    // A negative ifRev.
    match f.store.apply(&s, &bad(patch_op("obj/1", json!({"b": 1}), Some(-1))), &owner()) {
        Err(DataError::BadRequest(m)) => assert!(m.starts_with("op 2 ('obj/1'): ") && m.contains("ifRev"), "{m}"),
        other => panic!("want bad_request, got {other:?}"),
    }
    // A live value that isn't an object (found inside the write).
    let ops = vec![put("obj/9", json!(9)), patch_op("list", json!({"a": 1}), None)];
    match f.store.apply(&s, &ops, &owner()) {
        Err(DataError::BadRequest(m)) => assert!(m.starts_with("op 1 ('list'): ") && m.contains("use store.put"), "{m}"),
        other => panic!("want bad_request, got {other:?}"),
    }
    // A merged document over the value limit, like a single patch.
    let ops = vec![put("obj/9", json!(9)), patch_op("obj/1", json!({"b": "y".repeat(40 * 1024)}), None)];
    match f.store.apply(&s, &ops, &owner()) {
        Err(DataError::ValueTooLarge { bytes }) => assert!(bytes > MAX_VALUE_BYTES, "{bytes}"),
        other => panic!("want value_too_large, got {other:?}"),
    }
    assert_eq!(snapshot(&f), before, "every refusal wrote nothing");
}

#[test]
fn a_patch_that_changes_nothing_writes_no_row_in_a_batch() {
    let f = fixture();
    let s = board();
    let r1 = f.store.put(&s, "obj/1", &json!({"t": 1}), None, &owner()).unwrap();
    // Only a no-op patch: no rev bump, no change-log row (like store.patch).
    assert_eq!(f.store.apply(&s, &[patch_op("obj/1", json!({"t": 1, "gone": null}), Some(r1))], &owner()).unwrap(), r1);
    assert!(feed(&f, r1).is_empty());
    // Beside a real write: the batch moves, the no-op key keeps its rev.
    let rev = f.store.apply(&s, &[patch_op("obj/1", json!({"t": 1}), None), put("obj/2", json!(2))], &owner()).unwrap();
    assert_eq!(rev, r1 + 1);
    assert_eq!(f.store.get(&s, "obj/1").unwrap().unwrap().rev, r1);
    assert_eq!(feed(&f, r1), vec![("obj/2".to_string(), rev, false, "owner".to_string())]);
}
