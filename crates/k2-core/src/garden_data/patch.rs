//! `store.patch`: a field-level merge on one Garden data document
//! (0.46.4; Baden's Board via Appa 2026-10-23).
//!
//! The partial is a JSON Merge Patch (RFC 7396): each field it names
//! replaces the stored field, an object merges into an object field the
//! same way, and `null` removes the field. Every field the partial doesn't
//! name is kept as it is, so a widget (or an agent) that only knows some
//! fields of a document never drops the ones another writer added.
//!
//! - **Concurrency.** With `ifRev`, the patch applies only if the key's rev
//!   is still that (`0` = only if the key doesn't exist), else `conflict`
//!   with the live rev, exactly like `store.put`. Without `ifRev` the merge
//!   is still never lost: K2 reads, merges and writes with the rev it read,
//!   and on a race re-reads and merges again (up to [`MAX_ATTEMPTS`]).
//! - **Shape.** The partial is an object. The stored value, when there is
//!   one, must be an object too (`bad_request` otherwise: use `store.put`).
//!   A missing key starts from `{}`.
//! - **No-op.** A patch that changes nothing writes nothing and answers the
//!   key's current rev.
//! - The merged document obeys the usual limits (`value_too_large` past
//!   60 KiB, `space_full`, `store_full`).
//! - **In a batch or transaction** (0.46.5, Board via Appa): an
//!   `{op: "patch", key, value, ifRev?}` op runs [`check_partial`] and
//!   [`merged`] inside the store's one write (`SqliteStore::write`), so it
//!   lands at the batch's rev with the other ops or not at all. A conflict
//!   or a refused partial names the op's index and key.

use super::backend::{Json, StoreBackend};
use super::{Actor, DataError, Space};

/// Read-merge-write rounds before a race without `ifRev` answers `conflict`.
pub const MAX_ATTEMPTS: usize = 5;

/// What a patch did.
#[derive(Debug, Clone, PartialEq)]
pub struct Patched {
    /// The key's rev now (unchanged when the patch changed nothing).
    pub rev: i64,
    /// The whole document after the merge.
    pub value: Json,
    /// False when the patch changed nothing (no write, no new rev).
    pub changed: bool,
}

/// RFC 7396 JSON Merge Patch, applied in place. `patch` must be an object
/// at the top (callers check); nested non-objects replace.
pub fn merge_patch(target: &mut Json, patch: &Json) {
    let Json::Object(fields) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Json::Object(serde_json::Map::new());
    }
    let Json::Object(t) = target else { unreachable!("made an object above") };
    for (k, v) in fields {
        if v.is_null() {
            t.remove(k);
        } else if v.is_object() {
            merge_patch(t.entry(k.clone()).or_insert(Json::Null), v);
        } else {
            t.insert(k.clone(), v.clone());
        }
    }
}

/// The partial's own checks, before anything is read: it is an object, and
/// `ifRev` (when given) is 0 or more. Shared with the `patch` op of
/// `store.batch` and transactions (0.46.5), which run it per op.
pub fn check_partial(partial: &Json, if_rev: Option<i64>) -> Result<(), DataError> {
    if !partial.is_object() {
        return Err(DataError::BadRequest(
            "store.patch takes an object of fields (null removes a field); use store.put to replace a whole value".into(),
        ));
    }
    if if_rev.is_some_and(|r| r < 0) {
        return Err(DataError::BadRequest("store.patch: ifRev is 0 or more".into()));
    }
    Ok(())
}

/// The document after merging `partial` into `current` (the key's live
/// value; `None` when the key doesn't exist, which starts from `{}`).
/// Answers `None` when the merge changes nothing (no write, no new rev).
/// A live value that isn't an object is `bad_request`. Shared with the
/// `patch` op of `store.batch` and transactions (0.46.5), which merge
/// inside their one write.
pub fn merged(key: &str, current: Option<&Json>, partial: &Json) -> Result<Option<Json>, DataError> {
    let base = match current {
        Some(v) if v.is_object() => Some(v),
        Some(_) => {
            return Err(DataError::BadRequest(format!(
                "the value at '{key}' isn't an object, so it has no fields to patch; use store.put"
            )))
        }
        None => None,
    };
    let mut out = base.cloned().unwrap_or_else(|| Json::Object(serde_json::Map::new()));
    merge_patch(&mut out, partial);
    Ok(if base == Some(&out) { None } else { Some(out) })
}

/// Merge `partial` into the document at `key` (see the module doc).
pub fn patch(
    store: &dyn StoreBackend,
    space: &Space,
    key: &str,
    partial: &Json,
    if_rev: Option<i64>,
    by: &Actor,
) -> Result<Patched, DataError> {
    check_partial(partial, if_rev)?;
    let mut last = DataError::Conflict { key: key.to_string(), rev: 0, op: None };
    for _ in 0..MAX_ATTEMPTS {
        let cur = store.get(space, key)?;
        let live = cur.as_ref().map_or(0, |e| e.rev);
        if let Some(want) = if_rev {
            if want != live {
                return Err(DataError::Conflict { key: key.to_string(), rev: live, op: None });
            }
        }
        let Some(merged) = merged(key, cur.as_ref().map(|e| &e.value), partial)? else {
            let value = cur.map_or_else(|| Json::Object(serde_json::Map::new()), |e| e.value);
            return Ok(Patched { rev: live, value, changed: false });
        };
        match store.put(space, key, &merged, Some(live), by) {
            Ok(rev) => return Ok(Patched { rev, value: merged, changed: true }),
            // Someone wrote between our read and our write. With the
            // caller's ifRev that's their conflict; without it, merge again.
            Err(e @ DataError::Conflict { .. }) if if_rev.is_none() => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use serde_json::json;

    use super::super::backend::{ChangesPage, Cursor, Entry, Filter, Op, Page, ReapReport, SpaceInfo};
    use super::super::sqlite::SqliteStore;
    use super::*;

    struct Fx {
        dir: PathBuf,
        store: SqliteStore,
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            if let Some(root) = self.dir.parent() {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }

    fn fx() -> Fx {
        let dir = crate::test_env::unique_temp_path("gd-patch").join("stores");
        let store = SqliteStore::with_clock(&dir, Arc::new(|| 1_760_000_000));
        Fx { dir, store }
    }

    fn board() -> Space {
        Space::data("board").expect("space")
    }

    #[test]
    fn merge_patch_is_rfc_7396() {
        // The RFC's own example (§3), plus a nested merge.
        let mut t = json!({"title": "Goodbye!", "author": {"givenName": "John", "familyName": "Doe"}, "tags": ["example", "sample"], "content": "This will be unchanged"});
        let p = json!({"title": "Hello!", "phoneNumber": "+01-123-456-7890", "author": {"familyName": null}, "tags": ["example"]});
        merge_patch(&mut t, &p);
        assert_eq!(
            t,
            json!({"title": "Hello!", "author": {"givenName": "John"}, "tags": ["example"], "content": "This will be unchanged", "phoneNumber": "+01-123-456-7890"})
        );
        let mut s = json!("text");
        merge_patch(&mut s, &json!({"a": {"b": null, "c": 1}}));
        assert_eq!(s, json!({"a": {"c": 1}}), "a non-object target starts from {{}}");
    }

    #[test]
    fn patch_keeps_fields_it_does_not_name() {
        let f = fx();
        let rev = f.store.put(&board(), "obj/1", &json!({"t": "hi", "x": 1, "newer": {"k": true}}), None, &Actor::Owner).expect("put");
        let p = patch(&f.store, &board(), "obj/1", &json!({"t": "hello", "x": null}), Some(rev), &Actor::Owner).expect("patch");
        assert!(p.changed);
        assert!(p.rev > rev, "a new rev: {} after {rev}", p.rev);
        assert_eq!(p.value, json!({"t": "hello", "newer": {"k": true}}));
        let e = f.store.get(&board(), "obj/1").expect("get").expect("entry");
        assert_eq!((e.rev, e.value), (p.rev, json!({"t": "hello", "newer": {"k": true}})));
    }

    #[test]
    fn if_rev_conflicts_like_put() {
        let f = fx();
        let rev = f.store.put(&board(), "obj/1", &json!({"t": 1}), None, &Actor::Owner).expect("put");
        match patch(&f.store, &board(), "obj/1", &json!({"t": 2}), Some(rev + 7), &Actor::Owner) {
            Err(DataError::Conflict { key, rev: live, op: None }) => assert_eq!((key.as_str(), live), ("obj/1", rev)),
            other => panic!("want conflict, got {other:?}"),
        }
        match patch(&f.store, &board(), "obj/1", &json!({"t": 2}), Some(0), &Actor::Owner) {
            Err(DataError::Conflict { rev: live, .. }) => assert_eq!(live, rev, "ifRev 0 = only if missing"),
            other => panic!("want conflict, got {other:?}"),
        }
        assert_eq!(f.store.get(&board(), "obj/1").expect("get").expect("entry").value, json!({"t": 1}), "nothing written");
    }

    #[test]
    fn a_missing_key_starts_empty_and_if_rev_0_creates() {
        let f = fx();
        let p = patch(&f.store, &board(), "obj/new", &json!({"a": 1, "gone": null}), Some(0), &Actor::Owner).expect("patch");
        assert_eq!(p.value, json!({"a": 1}));
        assert!(p.changed && p.rev >= 1);
        let again = patch(&f.store, &board(), "obj/new2", &json!({"b": 2}), None, &Actor::Owner).expect("patch");
        assert_eq!(again.value, json!({"b": 2}));
    }

    #[test]
    fn a_no_op_patch_writes_nothing() {
        let f = fx();
        let rev = f.store.put(&board(), "obj/1", &json!({"t": 1}), None, &Actor::Owner).expect("put");
        let p = patch(&f.store, &board(), "obj/1", &json!({"t": 1, "absent": null}), None, &Actor::Owner).expect("patch");
        assert_eq!((p.rev, p.changed), (rev, false));
        let info = f.store.info(&board()).expect("info").expect("space");
        assert_eq!(info.rev, rev, "the space's rev didn't move");
    }

    #[test]
    fn shapes_are_checked() {
        let f = fx();
        for bad in [json!([1]), json!("x"), json!(1), json!(null)] {
            match patch(&f.store, &board(), "obj/1", &bad, None, &Actor::Owner) {
                Err(DataError::BadRequest(m)) => assert!(m.contains("object"), "{m}"),
                other => panic!("{bad}: want bad_request, got {other:?}"),
            }
        }
        f.store.put(&board(), "list", &json!([1, 2]), None, &Actor::Owner).expect("put");
        match patch(&f.store, &board(), "list", &json!({"a": 1}), None, &Actor::Owner) {
            Err(DataError::BadRequest(m)) => assert!(m.contains("use store.put"), "{m}"),
            other => panic!("want bad_request, got {other:?}"),
        }
        match patch(&f.store, &board(), "obj/1", &json!({"a": 1}), Some(-1), &Actor::Owner) {
            Err(DataError::BadRequest(m)) => assert!(m.contains("ifRev"), "{m}"),
            other => panic!("want bad_request, got {other:?}"),
        }
    }

    #[test]
    fn the_merged_value_obeys_the_value_limit() {
        let f = fx();
        f.store.put(&board(), "obj/1", &json!({"a": "x".repeat(40 * 1024)}), None, &Actor::Owner).expect("put");
        match patch(&f.store, &board(), "obj/1", &json!({"b": "y".repeat(40 * 1024)}), None, &Actor::Owner) {
            Err(DataError::ValueTooLarge { bytes }) => assert!(bytes > super::super::MAX_VALUE_BYTES, "{bytes}"),
            other => panic!("want value_too_large, got {other:?}"),
        }
    }

    /// A store that lets another writer in between the patch's read and its
    /// write, once.
    struct Racy<'a> {
        inner: &'a SqliteStore,
        raced: AtomicBool,
    }

    impl StoreBackend for Racy<'_> {
        fn get(&self, space: &Space, key: &str) -> Result<Option<Entry>, DataError> {
            self.inner.get(space, key)
        }
        fn put(&self, space: &Space, key: &str, value: &Json, if_rev: Option<i64>, by: &Actor) -> Result<i64, DataError> {
            if !self.raced.swap(true, Ordering::SeqCst) {
                let mut other = self.inner.get(space, key)?.map(|e| e.value).expect("raced key exists");
                other["fromOther"] = json!(true);
                self.inner.put(space, key, &other, None, &Actor::Agent { project_id: "w-other".into() })?;
            }
            self.inner.put(space, key, value, if_rev, by)
        }
        fn delete(&self, space: &Space, key: &str, if_rev: Option<i64>, by: &Actor) -> Result<Option<i64>, DataError> {
            self.inner.delete(space, key, if_rev, by)
        }
        fn apply(&self, space: &Space, ops: &[Op], by: &Actor) -> Result<i64, DataError> {
            self.inner.apply(space, ops, by)
        }
        fn list(&self, space: &Space, prefix: &str, after: Option<&str>, limit: usize, filter: Filter) -> Result<Page, DataError> {
            self.inner.list(space, prefix, after, limit, filter)
        }
        fn changes(&self, space: &Space, from: &Cursor, prefix: &str, filter: Filter) -> Result<ChangesPage, DataError> {
            self.inner.changes(space, from, prefix, filter)
        }
        fn info(&self, space: &Space) -> Result<Option<SpaceInfo>, DataError> {
            self.inner.info(space)
        }
        fn spaces(&self) -> Result<Vec<SpaceInfo>, DataError> {
            self.inner.spaces()
        }
        fn drop_space(&self, space: &Space) -> Result<(), DataError> {
            self.inner.drop_space(space)
        }
        fn move_space(&self, from: &Space, to: &Space) -> Result<(), DataError> {
            self.inner.move_space(from, to)
        }
        fn mark_orphans(&self, live_widgets: &[String], now: i64) -> Result<Vec<String>, DataError> {
            self.inner.mark_orphans(live_widgets, now)
        }
        fn reap(&self, now: i64) -> Result<ReapReport, DataError> {
            self.inner.reap(now)
        }
    }

    #[test]
    fn a_race_without_if_rev_merges_again_and_keeps_both() {
        let f = fx();
        f.store.put(&board(), "obj/1", &json!({"t": 1}), None, &Actor::Owner).expect("put");
        let racy = Racy { inner: &f.store, raced: AtomicBool::new(false) };
        let p = patch(&racy, &board(), "obj/1", &json!({"mine": 2}), None, &Actor::Owner).expect("patch");
        assert_eq!(p.value, json!({"t": 1, "fromOther": true, "mine": 2}), "the other writer's field survives");
        assert_eq!(f.store.get(&board(), "obj/1").expect("get").expect("entry").value, p.value);
    }

    #[test]
    fn a_race_with_if_rev_is_the_callers_conflict() {
        let f = fx();
        let rev = f.store.put(&board(), "obj/1", &json!({"t": 1}), None, &Actor::Owner).expect("put");
        let racy = Racy { inner: &f.store, raced: AtomicBool::new(false) };
        match patch(&racy, &board(), "obj/1", &json!({"mine": 2}), Some(rev), &Actor::Owner) {
            Err(DataError::Conflict { rev: live, .. }) => assert!(live > rev, "the live rev: {live}"),
            other => panic!("want conflict, got {other:?}"),
        }
        assert_eq!(
            f.store.get(&board(), "obj/1").expect("get").expect("entry").value,
            json!({"t": 1, "fromOther": true}),
            "the patch wrote nothing"
        );
    }
}
