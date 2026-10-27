//! Garden blob tests (prd-garden-board-needs-v1 §14, "Refs and writes",
//! "Downgrade reconcile", "GC", "Quota"). Each test gets its own temp
//! folder; nothing touches `~/.k2`.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use serde_json::json;

use super::super::backend::{Op, StoreBackend};
use super::super::blobs::{ref_of, sha256_hex, BLOB_PENDING_KEEP_SECS, BLOB_UNREF_KEEP_SECS};
use super::*;

struct Fx {
    root: std::path::PathBuf,
    dir: std::path::PathBuf,
    clock: Arc<AtomicI64>,
    store: SqliteStore,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fx_caps(space_cap: i64, store_cap: i64) -> Fx {
    let root = crate::test_env::unique_temp_path("gd-blobstore");
    let dir = root.join("stores");
    let clock = Arc::new(AtomicI64::new(1_760_000_000));
    let c = clock.clone();
    let store = SqliteStore::with_clock(&dir, Arc::new(move || c.load(Ordering::SeqCst))).with_blob_caps(space_cap, store_cap);
    Fx { root, dir, clock, store }
}

fn fx() -> Fx {
    fx_caps(super::super::blobs::MAX_SPACE_BLOB_BYTES, super::super::blobs::MAX_BLOB_STORE_BYTES)
}

impl Fx {
    fn reopen(&self) -> SqliteStore {
        let c = self.clock.clone();
        SqliteStore::with_clock(&self.dir, Arc::new(move || c.load(Ordering::SeqCst)))
    }
    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }
}

fn board() -> Space {
    Space::data("board").expect("space")
}

fn other() -> Space {
    Space::data("notes").expect("space")
}

fn widget() -> Actor {
    Actor::Widget { widget: "board".into() }
}

fn refs_of(s: &SqliteStore, space: &Space, hash: &str) -> i64 {
    s.blob_row(space, hash).expect("row").expect("held").refs
}

#[test]
fn a_put_is_hashed_sniffed_and_deduplicated() {
    let f = fx();
    let png = b"\x89PNG\r\n\x1a\nnot really a png".to_vec();
    let a = f.store.blob_put(&board(), &png, Some("../shot.png"), &widget()).expect("put");
    assert_eq!(a.reference, ref_of(&sha256_hex(&png)));
    assert_eq!((a.size, a.mime.as_str(), a.name.as_deref(), a.new), (png.len() as i64, "image/png", Some("shot.png"), true));
    let again = f.store.blob_put(&board(), &png, None, &widget()).expect("again");
    assert!(!again.new, "the same bytes in the same space: held already");
    let row = f.store.blob_row(&board(), &sha256_hex(&png)).expect("row").expect("held");
    assert!(row.pending, "no key names it yet");
    assert_eq!(row.added_by, "widget:board");
    let (_, bytes, eof) = f.store.blob_read(&board(), &sha256_hex(&png), 0, 1 << 20).expect("read");
    assert_eq!((bytes, eof), (png, true));
}

#[test]
fn save_spaces_keep_no_files_and_big_files_are_refused() {
    let f = fx();
    let save = Space::save("board").expect("save");
    assert_eq!(f.store.blob_put(&save, b"x", None, &widget()).unwrap_err().code(), "bad_request");
    let big = vec![7u8; super::super::blobs::MAX_BLOB_BYTES + 1];
    assert_eq!(f.store.blob_put(&board(), &big, None, &widget()).unwrap_err().code(), "too_large");
    // A ref string in a save: value is just a string (GB4).
    let r = ref_of(&sha256_hex(b"never uploaded"));
    f.store.put(&save, "prefs", &json!({ "bg": r }), None, &widget()).expect("save values aren't scanned");
}

#[test]
fn a_value_naming_an_unheld_blob_is_refused_and_nothing_is_written() {
    let f = fx();
    f.store.put(&board(), "obj/0", &json!(1), None, &Actor::Owner).expect("first");
    let missing = ref_of(&sha256_hex(b"nobody uploaded this"));
    let e = f.store.put(&board(), "obj/1", &json!({ "src": missing }), None, &Actor::Owner).unwrap_err();
    assert_eq!(e.code(), "blob_missing");
    assert_eq!(e.status(), 409);
    assert_eq!(e.extra()["ref"], json!(missing));
    assert_eq!(f.store.info(&board()).expect("info").expect("space").rev, 1, "no rev bump");
    assert!(f.store.get(&board(), "obj/1").expect("get").is_none());
    // A batch fails whole, naming the missing ref.
    let ops = vec![
        Op::Put { key: "obj/2".into(), value: json!(2), if_rev: None },
        Op::Put { key: "obj/3".into(), value: json!([missing]), if_rev: None },
    ];
    assert_eq!(f.store.apply(&board(), &ops, &Actor::Owner).unwrap_err().code(), "blob_missing");
    assert!(f.store.get(&board(), "obj/2").expect("get").is_none());
}

#[test]
fn dedup_never_crosses_spaces() {
    let f = fx();
    let bytes = b"a private photo".to_vec();
    let h = sha256_hex(&bytes);
    f.store.blob_put(&board(), &bytes, None, &widget()).expect("board uploads");
    // Space B can't name A's hash by copying its ref.
    let e = f.store.put(&other(), "n/1", &json!({ "img": ref_of(&h) }), None, &Actor::Owner).unwrap_err();
    assert_eq!(e.code(), "blob_missing");
    assert_eq!(f.store.blob_read(&other(), &h, 0, 10).unwrap_err().code(), "blob_missing");
    // B uploads the same bytes itself: then it may, and the file is shared.
    let b = f.store.blob_put(&other(), &bytes, None, &Actor::Owner).expect("notes uploads");
    assert!(b.new, "new to this space");
    f.store.put(&other(), "n/1", &json!({ "img": ref_of(&h) }), None, &Actor::Owner).expect("now held");
    let (files, _) = super::super::blobs::walk_files(&f.store.blob_dir()).expect("walk");
    assert_eq!(files.len(), 1, "one file on disk");
}

#[test]
fn every_write_kind_keeps_refs_in_its_transaction() {
    let f = fx();
    let a = f.store.blob_put(&board(), b"aaa", None, &widget()).expect("a");
    let b = f.store.blob_put(&board(), b"bbb", None, &widget()).expect("b");
    let (ha, hb) = (sha256_hex(b"aaa"), sha256_hex(b"bbb"));
    // put
    f.store.put(&board(), "obj/1", &json!({ "src": a.reference }), None, &widget()).expect("put");
    assert_eq!(refs_of(&f.store, &board(), &ha), 1);
    assert!(!f.store.blob_row(&board(), &ha).expect("row").expect("held").pending);
    // batch: a second key names a, a third names both
    let ops = vec![
        Op::Put { key: "obj/2".into(), value: json!([a.reference, a.reference]), if_rev: None },
        Op::Put { key: "obj/3".into(), value: json!({ "x": a.reference, "y": b.reference }), if_rev: None },
    ];
    f.store.apply(&board(), &ops, &widget()).expect("batch");
    assert_eq!((refs_of(&f.store, &board(), &ha), refs_of(&f.store, &board(), &hb)), (3, 1));
    // patch swaps obj/1's ref from a to b
    let p = Op::Patch { key: "obj/1".into(), value: json!({ "src": b.reference }), if_rev: None };
    f.store.apply(&board(), std::slice::from_ref(&p), &widget()).expect("patch");
    assert_eq!((refs_of(&f.store, &board(), &ha), refs_of(&f.store, &board(), &hb)), (2, 2));
    // a failed write (conflict) changes no ref rows
    let bad = vec![
        Op::Delete { key: "obj/2".into(), if_rev: None },
        Op::Put { key: "obj/3".into(), value: json!(0), if_rev: Some(999) },
    ];
    assert_eq!(f.store.apply(&board(), &bad, &widget()).unwrap_err().code(), "conflict");
    assert_eq!((refs_of(&f.store, &board(), &ha), refs_of(&f.store, &board(), &hb)), (2, 2));
    // deletes let go
    f.store.delete(&board(), "obj/2", None, &widget()).expect("delete");
    f.store.delete(&board(), "obj/3", None, &widget()).expect("delete");
    assert_eq!((refs_of(&f.store, &board(), &ha), refs_of(&f.store, &board(), &hb)), (0, 1));
    assert_eq!(f.store.blob_holders(&board(), &hb, 10).expect("holders"), vec!["obj/1".to_string()]);
}

#[test]
fn space_delete_and_move_carry_the_blob_rows() {
    let f = fx();
    let a = f.store.blob_put(&board(), b"moving", None, &widget()).expect("a");
    let h = sha256_hex(b"moving");
    f.store.put(&board(), "obj/1", &json!(a.reference), None, &widget()).expect("put");
    let to = Space::data("board2").expect("to");
    f.store.move_space(&board(), &to).expect("move");
    assert!(f.store.blob_row(&board(), &h).expect("row").is_none());
    assert_eq!(refs_of(&f.store, &to, &h), 1, "the row and its count moved");
    // A reopen doesn't think the moved space is stale.
    let again = f.reopen();
    assert_eq!(refs_of(&again, &to, &h), 1);
    again.drop_space(&to).expect("drop");
    assert!(again.blob_row(&to, &h).expect("row").is_none());
    assert_eq!(again.blob_holders(&to, &h, 10).expect("holders"), Vec::<String>::new());
}

#[test]
fn an_older_k2_writing_is_reconciled_on_open() {
    let f = fx();
    let a = f.store.blob_put(&board(), b"kept", None, &widget()).expect("a");
    let b = f.store.blob_put(&board(), b"other", None, &widget()).expect("b");
    f.store.put(&board(), "obj/1", &json!(a.reference), None, &widget()).expect("put");
    // An older K2 (no aux tables in its code) rewrites obj/1 to name b and
    // adds obj/2 naming a blob the space never held: it writes entries and
    // spaces only.
    {
        let conn = Connection::open(f.store.path()).expect("open");
        conn.execute("UPDATE entries SET value = ?1, rev = 2 WHERE space = 'data:board' AND key = 'obj/1'", [json!(b.reference).to_string()])
            .expect("older write");
        let stranger = json!(ref_of(&sha256_hex(b"never here"))).to_string();
        conn.execute(
            "INSERT INTO entries (space, key, rev, deleted, value, size, updated_at, updated_by) VALUES ('data:board', 'obj/2', 2, 0, ?1, 10, 1, 'owner')",
            [stranger],
        )
        .expect("older insert");
        conn.execute("UPDATE spaces SET rev = 2 WHERE space = 'data:board'", []).expect("rev");
    }
    let s = f.reopen();
    let (ha, hb) = (sha256_hex(b"kept"), sha256_hex(b"other"));
    assert_eq!(refs_of(&s, &board(), &ha), 0, "obj/1 no longer names a");
    assert_eq!(refs_of(&s, &board(), &hb), 1, "it names b now");
    assert_eq!(s.blob_holders(&board(), &hb, 10).expect("holders"), vec!["obj/1".to_string()]);
    let unheld = s.blob_holders(&board(), &sha256_hex(b"never here"), 10).expect("holders");
    assert!(unheld.is_empty(), "a ref to a blob the space doesn't hold is dropped from the rebuild");
    assert!(s.blob_row(&board(), &ha).expect("row").expect("held").refs == 0);
}

#[test]
fn quota_counts_every_row_until_gc_and_a_full_space_drops_its_own_let_go_first() {
    // Security review #5: a let-go file keeps counting against its own
    // space; when the space is full it gives up its own oldest copies,
    // never another space's.
    let f = fx_caps(10, 1 << 30);
    let a = f.store.blob_put(&board(), b"123456", None, &widget()).expect("6 bytes");
    let e = f.store.blob_put(&board(), b"abcdef", None, &widget()).unwrap_err();
    assert_eq!((e.code(), e.extra()["what"].as_str()), ("space_full", Some("blobs")), "pending counts");
    f.store.put(&board(), "obj/1", &json!(a.reference), None, &widget()).expect("held");
    assert_eq!(f.store.blob_usage(&board()).expect("usage"), (6, 10));
    f.store.delete(&board(), "obj/1", None, &widget()).expect("let go");
    assert_eq!(f.store.blob_usage(&board()).expect("usage").0, 6, "a let-go blob still counts until GC");
    let n = f.store.blob_put(&other(), b"notes'", None, &Actor::Owner).expect("notes has its own quota");
    f.store.put(&other(), "n", &json!(n.reference), None, &Actor::Owner).expect("held");
    f.store.delete(&other(), "n", None, &Actor::Owner).expect("notes lets go too");
    f.store.blob_put(&board(), b"abcdef", None, &widget()).expect("fits by dropping board's own let-go copy");
    assert!(f.store.blob_row(&board(), &sha256_hex(b"123456")).expect("row").is_none(), "board's own copy went");
    assert!(f.store.blob_row(&other(), &sha256_hex(b"notes'")).expect("row").is_some(), "notes' copy stays");
    assert_eq!(f.store.blob_usage(&board()).expect("usage").0, 6);
}

#[test]
fn pending_uploads_are_capped_per_uploader_across_spaces() {
    let root = crate::test_env::unique_temp_path("gd-pending");
    let dir = root.join("stores");
    let store = SqliteStore::with_clock(&dir, Arc::new(|| 1_760_000_000)).with_pending_cap(8);
    let agent = Actor::Agent { project_id: "w-1".into() };
    store.blob_put(&board(), b"12345", None, &agent).expect("5 pending");
    let e = store.blob_put(&other(), b"abcd", None, &agent).unwrap_err();
    assert_eq!((e.code(), e.extra()["what"].as_str()), ("space_full", Some("pending")), "the cap spans spaces");
    store.blob_put(&other(), b"abcd", None, &Actor::Owner).expect("another uploader has its own");
    let mine = store.blob_put(&board(), b"12345", None, &agent).expect("held already: no new bytes");
    store.put(&board(), "k", &json!(mine.reference), None, &agent).expect("named: no longer pending");
    store.blob_put(&other(), b"abcd", None, &agent).expect("held already in notes (the owner's)");
    store.blob_put(&other(), b"wxyz", None, &agent).expect("fits once its first upload is named");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_dedup_answer_names_the_file_only_to_its_uploader() {
    let f = fx();
    f.store.blob_put(&board(), b"same bytes", Some("secret-plan.png"), &Actor::Owner).expect("person");
    let mine = f.store.blob_put(&board(), b"same bytes", None, &Actor::Owner).expect("again");
    assert_eq!(mine.name.as_deref(), Some("secret-plan.png"));
    let agent = f.store.blob_put(&board(), b"same bytes", None, &Actor::Agent { project_id: "w-1".into() }).expect("agent");
    assert_eq!((agent.new, agent.name), (false, None), "review #6: another uploader never learns the name");
}

#[test]
fn a_deleted_space_keeps_its_files_for_a_restore_and_a_new_widget_sees_none() {
    let f = fx();
    let a = f.store.blob_put(&board(), b"keep me", Some("k.png"), &widget()).expect("a");
    f.store.put(&board(), "obj/1", &json!({ "src": a.reference }), None, &widget()).expect("held");
    f.store.drop_space(&board()).expect("drop");
    let h = sha256_hex(b"keep me");
    assert!(f.store.blob_row(&board(), &h).expect("row").is_none(), "the space holds nothing now");
    assert!(f.store.blob_list(&board(), None, 500, None).expect("list").items.is_empty(), "a new widget sees none");
    // Within the 35 days the file survives GC…
    f.advance(BLOB_PENDING_KEEP_SECS + 10);
    f.store.blob_gc(f.now()).expect("gc");
    assert_eq!(f.store.blob_store_bytes().expect("bytes"), 7, "the file is kept for the restore window");
    // …another space can't claim it…
    let e = f.store.put(&other(), "n", &json!(a.reference), None, &Actor::Owner).unwrap_err();
    assert_eq!(e.code(), "blob_missing");
    // …and a restore into the same space (the old value again) takes it back.
    f.store.put(&board(), "obj/1", &json!({ "src": a.reference }), None, &Actor::Owner).expect("restore");
    assert_eq!(refs_of(&f.store, &board(), &h), 1);
    let (_, bytes, _) = f.store.blob_read(&board(), &h, 0, 100).expect("readable again");
    assert_eq!(bytes, b"keep me");
    // Deleted again and 35 days on, GC takes it.
    f.store.drop_space(&board()).expect("drop again");
    f.advance(BLOB_UNREF_KEEP_SECS + 1);
    f.store.blob_gc(f.now()).expect("gc");
    assert_eq!(f.store.blob_store_bytes().expect("bytes"), 0);
}

#[test]
fn rows_an_older_k2_left_behind_on_delete_become_tombstones() {
    let f = fx();
    let a = f.store.blob_put(&board(), b"left behind", None, &widget()).expect("a");
    f.store.put(&board(), "obj/1", &json!(a.reference), None, &widget()).expect("held");
    f.store.blob_put(&other(), b"fresh paste", None, &widget()).expect("pending in a space with no writes");
    {
        // An older K2 deletes the space: it knows only entries, changes, spaces.
        let conn = Connection::open(f.store.path()).expect("open");
        for t in ["entries", "changes", "spaces"] {
            conn.execute(&format!("DELETE FROM {t} WHERE space = 'data:board'"), []).expect("older delete");
        }
    }
    let s = f.reopen();
    assert!(s.blob_row(&board(), &sha256_hex(b"left behind")).expect("row").is_none(), "a new board sees none");
    assert!(s.blob_list(&board(), None, 500, None).expect("list").items.is_empty());
    assert!(s.blob_row(&other(), &sha256_hex(b"fresh paste")).expect("row").is_some(), "fresh pending uploads stay");
}

#[test]
fn store_pressure_takes_the_asking_spaces_own_let_go_first() {
    let f = fx_caps(1 << 30, 8);
    let n = f.store.blob_put(&other(), b"oth1", None, &Actor::Owner).expect("notes");
    f.store.put(&other(), "n", &json!(n.reference), None, &Actor::Owner).expect("held");
    f.store.delete(&other(), "n", None, &Actor::Owner).expect("notes lets go first (oldest)");
    f.advance(10);
    let b = f.store.blob_put(&board(), b"brd1", None, &widget()).expect("board");
    f.store.put(&board(), "b", &json!(b.reference), None, &widget()).expect("held");
    f.store.delete(&board(), "b", None, &widget()).expect("board lets go later");
    f.store.blob_put(&board(), b"brd2", None, &widget()).expect("board needs 4 more bytes");
    assert!(f.store.blob_row(&board(), &sha256_hex(b"brd1")).expect("row").is_none(), "board's own copy went");
    assert!(f.store.blob_row(&other(), &sha256_hex(b"oth1")).expect("row").is_some(), "notes' older copy stayed");
}

#[test]
fn gc_reaps_pending_and_let_go_blobs_and_never_held_ones() {
    let f = fx();
    let pending = f.store.blob_put(&board(), b"pending", None, &widget()).expect("pending");
    let held = f.store.blob_put(&board(), b"held", None, &widget()).expect("held");
    let gone = f.store.blob_put(&board(), b"let go", None, &widget()).expect("let go");
    let shared = f.store.blob_put(&board(), b"shared", None, &widget()).expect("shared");
    f.store.blob_put(&other(), b"shared", None, &Actor::Owner).expect("other holds it too");
    f.store.put(&board(), "obj/held", &json!(held.reference), None, &widget()).expect("hold");
    f.store.put(&board(), "obj/gone", &json!([gone.reference, shared.reference]), None, &widget()).expect("hold");
    f.store.put(&other(), "n/1", &json!(shared.reference), None, &Actor::Owner).expect("other holds shared");
    f.store.delete(&board(), "obj/gone", None, &widget()).expect("let go");
    // 24 h + 1 s: only the pending upload goes.
    f.advance(BLOB_PENDING_KEEP_SECS + 1);
    let r = f.store.blob_gc(f.now()).expect("gc");
    assert_eq!((r.rows, r.files), (1, 1), "{r:?}");
    assert!(f.store.blob_row(&board(), &sha256_hex(b"pending")).expect("row").is_none());
    let _ = pending;
    // 35 days later: the let-go rows go; `shared` stays on disk for notes.
    f.advance(BLOB_UNREF_KEEP_SECS);
    let r = f.store.blob_gc(f.now()).expect("gc");
    assert_eq!((r.rows, r.files), (2, 1), "{r:?}");
    assert!(f.store.blob_row(&board(), &sha256_hex(b"held")).expect("row").is_some(), "held is never reaped");
    let (_, bytes, _) = f.store.blob_read(&other(), &sha256_hex(b"shared"), 0, 100).expect("still readable from notes");
    assert_eq!(bytes, b"shared");
    let (files, _) = super::super::blobs::walk_files(&f.store.blob_dir()).expect("walk");
    let mut names: Vec<String> = files.into_iter().map(|(h, _)| h).collect();
    names.sort();
    let mut want = vec![sha256_hex(b"held"), sha256_hex(b"shared")];
    want.sort();
    assert_eq!(names, want);
}

#[test]
fn store_pressure_removes_the_oldest_let_go_blob_first_then_refuses() {
    let f = fx_caps(1 << 30, 12);
    let old = f.store.blob_put(&board(), b"old!", None, &widget()).expect("old");
    f.store.put(&board(), "o", &json!(old.reference), None, &widget()).expect("hold");
    f.advance(10);
    let newer = f.store.blob_put(&board(), b"new!", None, &widget()).expect("newer");
    f.store.put(&board(), "n", &json!(newer.reference), None, &widget()).expect("hold");
    f.store.delete(&board(), "o", None, &widget()).expect("let go first");
    f.advance(10);
    f.store.delete(&board(), "n", None, &widget()).expect("let go second");
    let live = f.store.blob_put(&board(), b"live", None, &widget()).expect("12 of 12");
    f.store.put(&board(), "l", &json!(live.reference), None, &widget()).expect("hold");
    // 4 more bytes: the oldest let-go blob (old!) makes room.
    f.store.blob_put(&board(), b"more", None, &widget()).expect("pressure GC");
    assert!(f.store.blob_row(&board(), &sha256_hex(b"old!")).expect("row").is_none(), "oldest let-go first");
    assert!(f.store.blob_row(&board(), &sha256_hex(b"new!")).expect("row").is_some(), "only as much as needed");
    // Nothing left to free but new!; then held and pending are never taken.
    f.store.blob_put(&board(), b"once", None, &widget()).expect("frees new!");
    assert_eq!(f.store.blob_put(&board(), b"full", None, &widget()).unwrap_err().code(), "store_full");
    assert_eq!(f.store.blob_store_bytes().expect("bytes"), 12);
}

#[test]
fn a_list_is_paged_and_filtered_by_the_keys_that_hold_blobs() {
    let f = fx();
    let a = f.store.blob_put(&board(), b"under board", None, &widget()).expect("a");
    let b = f.store.blob_put(&board(), b"under secret", None, &widget()).expect("b");
    f.store.put(&board(), "board/b1/obj/1", &json!(a.reference), None, &widget()).expect("hold");
    f.store.put(&board(), "secret/x", &json!(b.reference), None, &widget()).expect("hold");
    let all = f.store.blob_list(&board(), None, 500, None).expect("all");
    assert_eq!(all.items.len(), 2);
    let ps = vec!["board/".to_string()];
    let mine = f.store.blob_list(&board(), None, 500, Some(&ps)).expect("filtered");
    assert_eq!(mine.items.iter().map(|r| r.reference()).collect::<Vec<_>>(), vec![a.reference.clone()]);
    let p1 = f.store.blob_list(&board(), None, 1, None).expect("page 1");
    assert_eq!(p1.items.len(), 1);
    let p2 = f.store.blob_list(&board(), p1.next.as_deref(), 1, None).expect("page 2");
    assert_eq!(p2.items.len(), 1);
    assert_ne!(p1.items[0].hash, p2.items[0].hash);
    assert!(p2.next.is_none());
    assert_eq!(all.bytes, (b"under board".len() + b"under secret".len()) as i64);
}
