//! Garden data routes: export (live and `at` a snapshot date) and import (contract §4.5; PRD §5.8). C0 → B3.
//!
//! The gates in `zen_data_routes::handle` already ran (credential kind,
//! person-only rows, POST-only, body cap). Handlers answer the 200 body or
//! a `DataError` (contract §4.6).
//!
//! - `GET /cli/zen/data/export?widget=&at=&after=`: one JSONL page as
//!   JSON (`{header, items, next}`). A person sees every key; an agent only
//!   keys under its **current** read prefixes, even in an old snapshot.
//!   `at` reads the newest dated snapshot on or before that date, read-only.
//! - `POST /cli/zen/data/import` (person): `{widget, mode, items}`. A
//!   pre-op snapshot first, then one write at one new rev.
//!
//! Also the snapshot hooks the rest of the daemon calls: [`snapshot_tick`]
//! (the hourly ticker: today's daily snapshot) and [`pre_op_snapshot`]
//! (before a space delete or move).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use k2_core::garden_data::export::{self, ExportPage, ImportItem, ImportMode};
use k2_core::garden_data::snapshots::{self, PreOp};
use k2_core::garden_data::{Actor, DataError, Space, StoreBackend};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::garden_data as gd;
use crate::zen_data_routes::DataWho;

/// The read filter for a caller: `None` = everything (the person),
/// `Some(prefixes)` = an agent's current read prefixes.
type FilterFn = fn(&DataWho, &Space) -> Result<Option<Vec<String>>, DataError>;

/// Everything a handler touches, so tests can use temp files and fakes.
pub(crate) struct Ctx<'a> {
    pub store: &'a dyn StoreBackend,
    /// `gardens.sqlite` (read directly for export pages).
    pub store_file: PathBuf,
    /// `~/.k2/stores/.snapshots/`.
    pub snap_dir: PathBuf,
    pub now: i64,
    pub filter: FilterFn,
    /// Stored actor → wire `by` (contract §5.3).
    pub wire_by: fn(&str, &mut HashMap<String, String>) -> String,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `~/.k2/stores/.snapshots/`.
pub fn snapshot_dir() -> PathBuf {
    gd::stores_dir().join(snapshots::DIR_NAME)
}

fn live_ctx() -> Ctx<'static> {
    let store = gd::store();
    Ctx {
        store,
        store_file: store.path().to_path_buf(),
        snap_dir: snapshot_dir(),
        now: now_secs(),
        filter: grant_filter,
        wire_by: wire_by_live,
    }
}

/// The person reads everything; an agent or shell tab reads its current
/// read prefixes (re-read on every call, contract §3.4), or everything for
/// its own widget under the `gardens_manage` switch (0.46.1, gap 7).
fn grant_filter(who: &DataWho, space: &Space) -> Result<Option<Vec<String>>, DataError> {
    match who.project_id() {
        None if who.is_owner() => Ok(None),
        None => Err(DataError::NotGranted { key: None }),
        Some(_) => Ok(crate::garden_data::agent_routes::access(who, space)?.read_prefixes()),
    }
}

/// Stored → wire (contract §5.3): `owner` → `you`; `agent:<id>` →
/// `agent:<handle>` (`agent:gone` when the workspace is deleted);
/// `widget:<w>` → `you via <w>`.
fn wire_by_live(stored: &str, cache: &mut HashMap<String, String>) -> String {
    wire_by_with(stored, cache, |id| crate::compute_routes::ws_by_id(id).map(|w| w.handle))
}

pub(crate) fn wire_by_with(
    stored: &str,
    cache: &mut HashMap<String, String>,
    handle_of: impl Fn(&str) -> Option<String>,
) -> String {
    match Actor::parse_stored(stored) {
        Some(Actor::Owner) => "you".to_string(),
        Some(Actor::Widget { widget }) => format!("you via {widget}"),
        Some(Actor::Agent { project_id }) => cache
            .entry(project_id.clone())
            .or_insert_with(|| format!("agent:{}", handle_of(&project_id).unwrap_or_else(|| "gone".to_string())))
            .clone(),
        None => stored.to_string(),
    }
}

pub fn handle(
    path: &str,
    who: &DataWho,
    body: &[u8],
    params: &HashMap<String, String>,
) -> Result<serde_json::Value, DataError> {
    let ctx = live_ctx();
    match path {
        "/cli/zen/data/export" => export_route(&ctx, who, params),
        "/cli/zen/data/import" => import_route(&ctx, who, body),
        _ => Err(DataError::NotFound("route_not_found")),
    }
}

fn param<'p>(params: &'p HashMap<String, String>, k: &str) -> Option<&'p str> {
    params.get(k).map(String::as_str).filter(|v| !v.is_empty())
}

/// `widget=` → its data space (a built-in's `@version` is dropped).
fn data_space(widget: Option<&str>) -> Result<Space, DataError> {
    let w = widget.ok_or_else(|| DataError::BadRequest("widget is required (the widget's folder name)".into()))?;
    let w = if w.starts_with("k2:") { w.split('@').next().unwrap_or(w) } else { w };
    Space::data(w)
}

/// True when the dev-build hook asks for today's snapshot now.
fn snapshot_now_hook() -> bool {
    cfg!(debug_assertions) && std::env::var(snapshots::SNAPSHOT_NOW_ENV).is_ok_and(|v| v == "1")
}

pub(crate) fn export_route(ctx: &Ctx<'_>, who: &DataWho, params: &HashMap<String, String>) -> Result<Value, DataError> {
    let space = data_space(param(params, "widget"))?;
    let after = param(params, "after");
    if let Some(a) = after {
        k2_core::garden_data::keys::validate_key(a)?;
    }
    let filter = (ctx.filter)(who, &space)?;
    let at = param(params, "at");
    let page: ExportPage = match at {
        Some(date) => {
            snapshots::check_date(date)?;
            if snapshot_now_hook() && date == snapshots::local_date(ctx.now) {
                snapshots::take_daily_with(&ctx.store_file, &ctx.snap_dir, date, true)?;
            }
            let snap = snapshots::open_at(&ctx.snap_dir, date)?;
            export::export_page(&snap.path, &space, after, filter.as_deref(), Some(&snap.date), ctx.now)?
        }
        None => export::export_page(&ctx.store_file, &space, after, filter.as_deref(), None, ctx.now)?,
    };
    if after.is_none() {
        gd::audit::line(
            "zen.data.export",
            &who.audit_label(),
            &format!("space={} at={}", space, page.header.snapshot.as_deref().unwrap_or("live")),
        );
    }
    let mut cache = HashMap::new();
    let items: Vec<Value> = page
        .items
        .iter()
        .map(|r| {
            json!({
                "key": r.key,
                "rev": r.rev,
                "value": r.value,
                "updatedAt": r.updated_at,
                "by": (ctx.wire_by)(&r.by, &mut cache),
            })
        })
        .collect();
    Ok(json!({ "header": page.header, "items": items, "next": page.next }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBody {
    widget: String,
    mode: ImportMode,
    items: Vec<ImportItem>,
}

pub(crate) fn import_route(ctx: &Ctx<'_>, who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    if !who.is_owner() {
        return Err(DataError::PersonOnly);
    }
    let req: ImportBody = serde_json::from_slice(body)
        .map_err(|e| DataError::BadRequest(format!("import takes {{widget, mode, items:[{{key, value}}]}}: {e}")))?;
    let space = data_space(Some(&req.widget))?;
    let existing =
        if req.mode == ImportMode::Replace { export::live_keys(&ctx.store_file, &space)? } else { Vec::new() };
    let ops = export::import_plan(&req.items, req.mode, &existing)?;
    let written = ops.iter().filter(|o| matches!(o, k2_core::garden_data::Op::Put { .. })).count();
    let deleted = ops.len() - written;
    let snapshot = if ctx.store_file.exists() {
        Some(snapshots::take_pre_op(&ctx.store_file, &ctx.snap_dir, PreOp::Import, &snapshots::local_stamp(ctx.now))?)
    } else {
        None
    };
    let rev = if ops.is_empty() {
        ctx.store.info(&space)?.map(|i| i.rev).unwrap_or(0)
    } else {
        let rev = ctx.store.apply(&space, &ops, &Actor::Owner)?;
        gd::notify_changed(space.as_str(), rev);
        rev
    };
    gd::audit::line(
        "zen.data.import",
        &who.audit_label(),
        &format!(
            "space={space} mode={} written={written} deleted={deleted} rev={rev} snapshot={}",
            if req.mode == ImportMode::Replace { "replace" } else { "merge" },
            snapshot.as_deref().unwrap_or("-")
        ),
    );
    Ok(json!({ "rev": rev, "written": written, "deleted": deleted }))
}

/// The hourly ticker's snapshot step: today's dated snapshot unless one
/// exists or nothing changed (contract §2.3). B1's `garden_data::start`
/// calls this on every tick; the first tick after local midnight takes it.
pub fn snapshot_tick() -> Result<Option<PathBuf>, DataError> {
    let today = snapshots::local_date(now_secs());
    snapshots::take_daily_with(gd::store().path(), &snapshot_dir(), &today, false)
}

/// A pre-op snapshot before a space delete or move (B1's routes). Answers
/// its file name (`space/delete` returns it), or `None` when no store
/// file exists yet (nothing to keep).
pub fn pre_op_snapshot(op: PreOp) -> Result<Option<String>, DataError> {
    take_pre_op_at(gd::store().path(), &snapshot_dir(), op, now_secs())
}

pub(crate) fn take_pre_op_at(store: &Path, dir: &Path, op: PreOp, now: i64) -> Result<Option<String>, DataError> {
    if !store.exists() {
        return Ok(None);
    }
    snapshots::take_pre_op(store, dir, op, &snapshots::local_stamp(now)).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::garden_data::sqlite::GARDENS_SCHEMA_V1;
    use k2_core::garden_data::{
        ChangesPage, Cursor, Entry, Filter, Json, Op, Page, ReapReport, SpaceInfo,
    };
    use rusqlite::Connection;
    use std::sync::Mutex;

    /// A backend that applies ops straight to the test's store file (the
    /// part of B1's write algorithm import needs: one new rev for all).
    struct FileStore {
        file: PathBuf,
        applied: Mutex<Vec<(String, usize)>>,
    }

    impl StoreBackend for FileStore {
        fn get(&self, _: &Space, _: &str) -> Result<Option<Entry>, DataError> {
            Err(DataError::NotBuilt)
        }
        fn put(&self, _: &Space, _: &str, _: &Json, _: Option<i64>, _: &Actor) -> Result<i64, DataError> {
            Err(DataError::NotBuilt)
        }
        fn delete(&self, _: &Space, _: &str, _: Option<i64>, _: &Actor) -> Result<Option<i64>, DataError> {
            Err(DataError::NotBuilt)
        }
        fn apply(&self, space: &Space, ops: &[Op], by: &Actor) -> Result<i64, DataError> {
            let mut c = Connection::open(&self.file)?;
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO spaces (space, rev, created_at, updated_at) VALUES (?1, 0, 1, 1) ON CONFLICT(space) DO NOTHING",
                [space.as_str()],
            )?;
            let rev: i64 = tx.query_row("SELECT rev + 1 FROM spaces WHERE space = ?1", [space.as_str()], |r| r.get(0))?;
            for op in ops {
                match op {
                    Op::Put { key, value, .. } => {
                        tx.execute(
                            "INSERT OR REPLACE INTO entries VALUES (?1, ?2, ?3, 0, ?4, 1, 1, ?5)",
                            rusqlite::params![space.as_str(), key, rev, value.to_string(), by.stored()],
                        )?;
                    }
                    Op::Delete { key, .. } => {
                        tx.execute(
                            "INSERT OR REPLACE INTO entries VALUES (?1, ?2, ?3, 1, NULL, 0, 1, ?4)",
                            rusqlite::params![space.as_str(), key, rev, by.stored()],
                        )?;
                    }
                    Op::Patch { .. } => unreachable!("an import plans puts and deletes only"),
                }
            }
            tx.execute("UPDATE spaces SET rev = ?2, updated_at = ?2 WHERE space = ?1", rusqlite::params![space.as_str(), rev])?;
            tx.commit()?;
            self.applied.lock().expect("lock").push((space.as_str().to_string(), ops.len()));
            Ok(rev)
        }
        fn list(&self, _: &Space, _: &str, _: Option<&str>, _: usize, _: Filter) -> Result<Page, DataError> {
            Err(DataError::NotBuilt)
        }
        fn changes(&self, _: &Space, _: &Cursor, _: &str, _: Filter) -> Result<ChangesPage, DataError> {
            Err(DataError::NotBuilt)
        }
        fn info(&self, space: &Space) -> Result<Option<SpaceInfo>, DataError> {
            let c = Connection::open(&self.file)?;
            let rev: Option<i64> = c
                .query_row("SELECT rev FROM spaces WHERE space = ?1", [space.as_str()], |r| r.get(0))
                .ok();
            Ok(rev.map(|rev| SpaceInfo {
                space: space.as_str().into(),
                widget: space.widget().into(),
                rev,
                reaped_rev: 0,
                bytes: 0,
                keys: 0,
                created_at: 1,
                updated_at: 1,
                orphaned_at: None,
            }))
        }
        fn spaces(&self) -> Result<Vec<SpaceInfo>, DataError> {
            Err(DataError::NotBuilt)
        }
        fn drop_space(&self, _: &Space) -> Result<(), DataError> {
            Err(DataError::NotBuilt)
        }
        fn move_space(&self, _: &Space, _: &Space) -> Result<(), DataError> {
            Err(DataError::NotBuilt)
        }
        fn mark_orphans(&self, _: &[String], _: i64) -> Result<Vec<String>, DataError> {
            Err(DataError::NotBuilt)
        }
        fn reap(&self, _: i64) -> Result<ReapReport, DataError> {
            Err(DataError::NotBuilt)
        }
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-gd-export-routes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    fn seed(file: &Path) {
        let c = Connection::open(file).expect("store");
        // WAL, like the live store B1 opens.
        c.execute_batch("PRAGMA journal_mode = WAL;").expect("wal");
        c.execute_batch(GARDENS_SCHEMA_V1).expect("schema");
        c.execute_batch(
            "INSERT INTO spaces (space, rev, created_at, updated_at) VALUES ('data:board', 4, 1, 4);
             INSERT INTO entries VALUES ('data:board', 'obj/1', 2, 0, '{\"t\":\"a\"}', 9, 2, 'owner');
             INSERT INTO entries VALUES ('data:board', 'project/7/obj/9', 3, 0, '1', 1, 3, 'agent:w-keeper');
             INSERT INTO entries VALUES ('data:board', 'project/8/obj/1', 4, 0, '2', 1, 4, 'widget:board');",
        )
        .expect("rows");
    }

    /// The fake grant table: agent `w-keeper` reads `project/7/` only.
    fn fake_filter(who: &DataWho, _: &Space) -> Result<Option<Vec<String>>, DataError> {
        match who.project_id() {
            None => Ok(None),
            Some("w-keeper") => Ok(Some(vec!["project/7/".into()])),
            Some(_) => Ok(Some(Vec::new())),
        }
    }

    fn fake_by(stored: &str, cache: &mut HashMap<String, String>) -> String {
        wire_by_with(stored, cache, |id| (id == "w-keeper").then(|| "board-keeper".to_string()))
    }

    fn ctx<'a>(dir: &Path, store: &'a FileStore, now: i64) -> Ctx<'a> {
        Ctx {
            store,
            store_file: dir.join("gardens.sqlite"),
            snap_dir: dir.join(snapshots::DIR_NAME),
            now,
            filter: fake_filter,
            wire_by: fake_by,
        }
    }

    fn params(kv: &[(&str, &str)]) -> HashMap<String, String> {
        kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn keys(v: &Value) -> Vec<String> {
        v["items"].as_array().expect("items").iter().map(|i| i["key"].as_str().expect("key").to_string()).collect()
    }

    #[test]
    fn export_live_person_sees_all_with_wire_by() {
        let d = tmp();
        seed(&d.join("gardens.sqlite"));
        let fs = FileStore { file: d.join("gardens.sqlite"), applied: Mutex::new(Vec::new()) };
        let c = ctx(&d, &fs, 1_000);
        let v = export_route(&c, &DataWho::Owner, &params(&[("widget", "board")])).expect("export");
        assert_eq!(v["header"]["space"], "data:board");
        assert_eq!(v["header"]["rev"], 4);
        assert_eq!(v["header"]["exportedAt"], 1_000);
        assert_eq!(v["header"]["snapshot"], Value::Null);
        assert_eq!(keys(&v), ["obj/1", "project/7/obj/9", "project/8/obj/1"]);
        let by: Vec<&str> = v["items"].as_array().expect("items").iter().map(|i| i["by"].as_str().expect("by")).collect();
        assert_eq!(by, ["you", "agent:board-keeper", "you via board"]);
        assert_eq!(v["items"][0]["updatedAt"], 2);
        assert_eq!(v["next"], Value::Null);
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn export_agent_is_filtered_by_current_grants_even_in_a_snapshot() {
        let d = tmp();
        let file = d.join("gardens.sqlite");
        seed(&file);
        let fs = FileStore { file: file.clone(), applied: Mutex::new(Vec::new()) };
        let c = ctx(&d, &fs, 1_000);
        snapshots::take_daily(&file, &c.snap_dir, "2026-10-13").expect("snap").expect("written");
        let keeper = DataWho::Agent { project_id: "w-keeper".into() };
        let live = export_route(&c, &keeper, &params(&[("widget", "board")])).expect("live");
        assert_eq!(keys(&live), ["project/7/obj/9"]);
        // Yesterday's snapshot holds project/8/… too: still never shown.
        let old = export_route(&c, &keeper, &params(&[("widget", "board"), ("at", "2026-10-14")])).expect("at");
        assert_eq!(keys(&old), ["project/7/obj/9"]);
        assert_eq!(old["header"]["snapshot"], "2026-10-13");
        let shell = DataWho::Shell { project_id: "w-other".into() };
        assert!(keys(&export_route(&c, &shell, &params(&[("widget", "board")])).expect("none")).is_empty());
        // The person sees the snapshot's every key, frozen: a later write isn't in it.
        let conn = Connection::open(&file).expect("open");
        conn.execute("INSERT INTO entries VALUES ('data:board', 'late', 5, 0, '1', 1, 5, 'owner')", []).expect("late");
        let frozen = export_route(&c, &DataWho::Owner, &params(&[("widget", "board"), ("at", "2026-10-13")])).expect("at");
        assert_eq!(keys(&frozen), ["obj/1", "project/7/obj/9", "project/8/obj/1"]);
        // Windows can't delete a file that is still open.
        drop(conn);
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn export_refusals() {
        let d = tmp();
        let fs = FileStore { file: d.join("gardens.sqlite"), applied: Mutex::new(Vec::new()) };
        let c = ctx(&d, &fs, 1_000);
        let e = export_route(&c, &DataWho::Owner, &params(&[])).expect_err("no widget");
        assert_eq!(e.code(), "bad_request");
        let e = export_route(&c, &DataWho::Owner, &params(&[("widget", "board"), ("at", "yesterday")])).expect_err("date");
        assert_eq!(e.code(), "bad_request");
        let e = export_route(&c, &DataWho::Owner, &params(&[("widget", "board"), ("at", "2026-10-13")])).expect_err("none");
        assert_eq!(e, DataError::NoSnapshot { oldest: None });
        assert_eq!(e.status(), 404);
        let e = export_route(&c, &DataWho::Owner, &params(&[("widget", "../x")])).expect_err("widget");
        assert_eq!(e.code(), "bad_request");
        let e = export_route(&c, &DataWho::Owner, &params(&[("widget", "board"), ("after", "/bad")])).expect_err("after");
        assert_eq!(e.code(), "bad_key");
        // No store yet: an empty export, rev 0 (never a probe).
        let v = export_route(&c, &DataWho::Owner, &params(&[("widget", "board")])).expect("empty");
        assert_eq!((v["header"]["rev"].as_i64(), keys(&v).len()), (Some(0), 0));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn import_merge_and_replace_write_one_rev_after_a_pre_op_snapshot() {
        let d = tmp();
        let file = d.join("gardens.sqlite");
        seed(&file);
        let fs = FileStore { file: file.clone(), applied: Mutex::new(Vec::new()) };
        let c = ctx(&d, &fs, 1_760_423_520);
        let merge = br#"{"widget":"board","mode":"merge","items":[{"key":"obj/1","value":{"t":"b"}},{"key":"obj/2","value":2}]}"#;
        let v = import_route(&c, &DataWho::Owner, merge).expect("merge");
        assert_eq!(v, json!({"rev": 5, "written": 2, "deleted": 0}));
        assert_eq!(snapshots::list_pre_op(&c.snap_dir).expect("pre-ops").len(), 1);
        let replace = br#"{"widget":"board","mode":"replace","items":[{"key":"obj/2","value":3}]}"#;
        let v = import_route(&c, &DataWho::Owner, replace).expect("replace");
        assert_eq!(v, json!({"rev": 6, "written": 1, "deleted": 3}));
        assert_eq!(*fs.applied.lock().expect("lock"), [("data:board".to_string(), 2), ("data:board".to_string(), 4)]);
        let live = export_route(&c, &DataWho::Owner, &params(&[("widget", "board")])).expect("export");
        assert_eq!(keys(&live), ["obj/2"]);
        assert_eq!(live["items"][0]["value"], 3);
        // Nothing to do: no write, the current rev.
        let v = import_route(&c, &DataWho::Owner, br#"{"widget":"board","mode":"merge","items":[]}"#).expect("noop");
        assert_eq!(v, json!({"rev": 6, "written": 0, "deleted": 0}));
        assert_eq!(fs.applied.lock().expect("lock").len(), 2);
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn import_refusals() {
        let d = tmp();
        let fs = FileStore { file: d.join("gardens.sqlite"), applied: Mutex::new(Vec::new()) };
        let c = ctx(&d, &fs, 1);
        let agent = DataWho::Agent { project_id: "w-keeper".into() };
        assert_eq!(import_route(&c, &agent, b"{}").expect_err("agent"), DataError::PersonOnly);
        let code = |b: &[u8]| import_route(&c, &DataWho::Owner, b).expect_err("refused").code();
        assert_eq!(code(b"not json"), "bad_request");
        assert_eq!(code(br#"{"widget":"board","mode":"merge","items":[],"extra":1}"#), "bad_request");
        assert_eq!(code(br#"{"widget":"board","mode":"swap","items":[]}"#), "bad_request");
        assert_eq!(code(br#"{"widget":"board","mode":"merge","items":[{"key":"a","value":1,"rev":3}]}"#), "bad_request");
        assert_eq!(code(br#"{"widget":"board","mode":"merge","items":[{"key":"a//b","value":1}]}"#), "bad_key");
        assert!(fs.applied.lock().expect("lock").is_empty());
        assert!(!c.store_file.exists(), "a refused import never creates the store");
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn pre_op_needs_a_store_file() {
        let d = tmp();
        let snaps = d.join(snapshots::DIR_NAME);
        assert_eq!(take_pre_op_at(&d.join("gardens.sqlite"), &snaps, PreOp::Delete, 1).expect("none"), None);
        seed(&d.join("gardens.sqlite"));
        let name = take_pre_op_at(&d.join("gardens.sqlite"), &snaps, PreOp::Move, 1_760_423_520).expect("move");
        assert!(name.expect("name").ends_with("-move.sqlite"));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn wire_by_forms() {
        let mut cache = HashMap::new();
        let f = |id: &str| (id == "w-1").then(|| "builder".to_string());
        assert_eq!(wire_by_with("owner", &mut cache, f), "you");
        assert_eq!(wire_by_with("widget:board", &mut cache, f), "you via board");
        assert_eq!(wire_by_with("agent:w-1", &mut cache, f), "agent:builder");
        assert_eq!(wire_by_with("agent:w-gone", &mut cache, f), "agent:gone");
        assert_eq!(data_space(Some("k2:diary@3")).expect("built-in").as_str(), "data:k2:diary");
    }
}
