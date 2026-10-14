//! Daily snapshots of `gardens.sqlite` (contract §2.3; PRD §5.8, BG7).
//! C0 → B3.
//!
//! `~/.k2/stores/.snapshots/gardens-YYYY-MM-DD.sqlite` via `VACUUM INTO`
//! (0600), once a day, skipped when nothing changed; 7 daily + 4 weekly
//! kept. Pre-op snapshots (`…THHMMSS-<op>.sqlite`) before a space delete,
//! import or move, kept 14 days / at most 10. `export --at` reads only the
//! dated files, read-only.
//!
//! Every snapshot is written to a dot-named temp file in the 0700 folder,
//! made 0600, switched out of WAL (so a read-only open never creates
//! `-wal`/`-shm` files beside it), then renamed into place: a reader never
//! sees half a file.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

use super::{DataError, SNAPSHOT_DAILY_KEEP, SNAPSHOT_WEEKLY_KEEP};

/// The folder under `~/.k2/stores/`.
pub const DIR_NAME: &str = ".snapshots";

/// Pre-op snapshots are kept this long …
pub const PRE_OP_KEEP_DAYS: i64 = 14;
/// … and at most this many.
pub const PRE_OP_KEEP_MAX: usize = 10;

/// The dev-build hook that forces today's snapshot (contract §12.4 step 13).
pub const SNAPSHOT_NOW_ENV: &str = "K2_GARDEN_DATA_SNAPSHOT_NOW";

const PREFIX: &str = "gardens-";
const SUFFIX: &str = ".sqlite";

/// Why a pre-op snapshot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreOp {
    Delete,
    Import,
    Move,
}

impl PreOp {
    pub fn as_str(self) -> &'static str {
        match self {
            PreOp::Delete => "delete",
            PreOp::Import => "import",
            PreOp::Move => "move",
        }
    }
}

/// One dated snapshot file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// `YYYY-MM-DD` (local date).
    pub date: String,
    pub path: PathBuf,
}

// ── dates ────────────────────────────────────────────────────────────

/// Parse a `YYYY-MM-DD` date (exactly that shape).
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    if s.len() != 10 {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The `--at` check: `bad_request` unless `YYYY-MM-DD`.
pub fn check_date(s: &str) -> Result<NaiveDate, DataError> {
    parse_date(s).ok_or_else(|| DataError::BadRequest("a date looks like 2026-10-13 (YYYY-MM-DD)".into()))
}

/// This computer's local date for `now` (seconds since the epoch).
pub fn local_date(now: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(now, 0).single() {
        Some(t) => t.format("%Y-%m-%d").to_string(),
        None => chrono::Local::now().format("%Y-%m-%d").to_string(),
    }
}

/// A pre-op stamp in local time: `YYYY-MM-DDTHHMMSS`.
pub fn local_stamp(now: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(now, 0).single() {
        Some(t) => t.format("%Y-%m-%dT%H%M%S").to_string(),
        None => chrono::Local::now().format("%Y-%m-%dT%H%M%S").to_string(),
    }
}

fn parse_stamp(s: &str) -> Option<NaiveDateTime> {
    if s.len() != 17 {
        return None;
    }
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H%M%S").ok()
}

/// The dated file name for `date`.
pub fn dated_name(date: &str) -> String {
    format!("{PREFIX}{date}{SUFFIX}")
}

/// `gardens-YYYY-MM-DD.sqlite` → the date; anything else → None.
fn date_of(name: &str) -> Option<String> {
    let d = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    parse_date(d).map(|_| d.to_string())
}

/// `gardens-<stamp>-<op>[-n].sqlite` → the stamp; anything else → None.
fn pre_op_stamp_of(name: &str) -> Option<String> {
    let rest = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let stamp = rest.get(..17)?;
    parse_stamp(stamp)?;
    let tail = rest.get(17..)?;
    let tail = tail.strip_prefix('-')?;
    let op = tail.split('-').next().unwrap_or("");
    if ["delete", "import", "move"].contains(&op) {
        Some(stamp.to_string())
    } else {
        None
    }
}

// ── retention (pure) ─────────────────────────────────────────────────

/// Pure: which dated snapshots to keep (7 newest + the newest of each of
/// the 4 ISO weeks before them). `dates` are `YYYY-MM-DD`; anything else
/// is never kept. Answers the kept dates, oldest first.
///
/// "Before them": the dates older than the 7 newest, grouped by ISO week;
/// the 4 newest of those weeks each keep their newest date. A week that
/// also holds one of the 7 dailies counts as one of the 4.
pub fn keep_set(dates: &[String]) -> Vec<String> {
    let mut valid: Vec<(NaiveDate, String)> =
        dates.iter().filter_map(|d| parse_date(d).map(|p| (p, d.clone()))).collect();
    valid.sort_by(|a, b| b.0.cmp(&a.0));
    valid.dedup_by(|a, b| a.0 == b.0);
    let mut keep: Vec<(NaiveDate, String)> = valid.iter().take(SNAPSHOT_DAILY_KEEP).cloned().collect();
    let mut weeks: Vec<(i32, u32)> = Vec::new();
    for (d, s) in valid.iter().skip(SNAPSHOT_DAILY_KEEP) {
        let w = d.iso_week();
        let key = (w.year(), w.week());
        if weeks.contains(&key) {
            continue;
        }
        if weeks.len() == SNAPSHOT_WEEKLY_KEEP {
            break;
        }
        weeks.push(key);
        keep.push((*d, s.clone()));
    }
    keep.sort_by(|a, b| a.0.cmp(&b.0));
    keep.into_iter().map(|(_, s)| s).collect()
}

/// Pure: which pre-op files to delete. `names` are file names; `today` is
/// the local date. Older than 14 days goes, then all but the newest 10.
pub fn pre_op_drop_set(names: &[String], today: NaiveDate) -> Vec<String> {
    let mut ops: Vec<(NaiveDateTime, String)> =
        names.iter().filter_map(|n| pre_op_stamp_of(n).and_then(|s| parse_stamp(&s)).map(|t| (t, n.clone()))).collect();
    ops.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let mut drop = Vec::new();
    let mut kept = 0usize;
    for (t, n) in ops {
        let age = (today - t.date()).num_days();
        if age > PRE_OP_KEEP_DAYS || kept >= PRE_OP_KEEP_MAX {
            drop.push(n);
        } else {
            kept += 1;
        }
    }
    drop
}

// ── files ────────────────────────────────────────────────────────────

fn io(what: &str, e: std::io::Error) -> DataError {
    DataError::Unavailable(format!("Garden data snapshot: can't {what} ({})", e.kind()))
}

fn names_in(dir: &Path) -> Result<Vec<String>, DataError> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io("read the snapshot folder", e)),
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e.map_err(|e| io("read the snapshot folder", e))?;
        out.push(e.file_name().to_string_lossy().to_string());
    }
    out.sort();
    Ok(out)
}

/// Every dated snapshot in `dir`, oldest first.
pub fn list_dated(dir: &Path) -> Result<Vec<Snapshot>, DataError> {
    Ok(names_in(dir)?
        .into_iter()
        .filter_map(|n| date_of(&n).map(|date| Snapshot { date, path: dir.join(&n) }))
        .collect())
}

/// Every pre-op snapshot file name in `dir`, oldest first.
pub fn list_pre_op(dir: &Path) -> Result<Vec<String>, DataError> {
    Ok(names_in(dir)?.into_iter().filter(|n| pre_op_stamp_of(n).is_some()).collect())
}

fn ensure_dir(dir: &Path) -> Result<(), DataError> {
    fs::create_dir_all(dir).map_err(|e| io("create the snapshot folder", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| io("lock the snapshot folder", e))?;
    }
    Ok(())
}

/// Open a store file for reading (never creates it).
fn open_source(store: &Path) -> Result<Connection, DataError> {
    let c = Connection::open_with_flags(store, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(c)
}

/// Open a snapshot read-only; it is never written.
pub fn open_read_only(path: &Path) -> Result<Connection, DataError> {
    Ok(Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?)
}

/// What changed-or-not means: every space's name, rev and updated time.
fn fingerprint(c: &Connection) -> Result<String, DataError> {
    let mut st = c.prepare("SELECT space, rev, updated_at FROM spaces ORDER BY space")?;
    let mut rows = st.query([])?;
    let mut h = Sha256::new();
    let mut n = 0u64;
    while let Some(r) = rows.next()? {
        let s: String = r.get(0)?;
        let rev: i64 = r.get(1)?;
        let at: i64 = r.get(2)?;
        h.update(format!("{s}\t{rev}\t{at}\n").as_bytes());
        n += 1;
    }
    h.update(n.to_le_bytes());
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn has_spaces(c: &Connection) -> Result<bool, DataError> {
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM spaces)", [], |r| r.get::<_, bool>(0))?)
}

/// `VACUUM INTO` a temp file, 0600, out of WAL, renamed to `dest`.
fn vacuum_to(src: &Connection, dir: &Path, dest_name: &str) -> Result<PathBuf, DataError> {
    let tmp = dir.join(format!(".{dest_name}.tmp"));
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io("clear a leftover temp snapshot", e)),
    }
    src.execute("VACUUM INTO ?1", [tmp.to_string_lossy().to_string()])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|e| io("lock a snapshot", e))?;
    }
    {
        let c = Connection::open_with_flags(&tmp, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        let mode: String = c.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(DataError::Unavailable(format!("Garden data snapshot: journal mode stayed {mode}")));
        }
    }
    let dest = dir.join(dest_name);
    fs::rename(&tmp, &dest).map_err(|e| io("move a snapshot into place", e))?;
    Ok(dest)
}

/// Delete dated snapshots outside [`keep_set`]. Answers the names removed.
pub fn prune_dated(dir: &Path) -> Result<Vec<String>, DataError> {
    let all = list_dated(dir)?;
    let dates: Vec<String> = all.iter().map(|s| s.date.clone()).collect();
    let keep = keep_set(&dates);
    let mut gone = Vec::new();
    for s in all {
        if !keep.contains(&s.date) {
            fs::remove_file(&s.path).map_err(|e| io("remove an old snapshot", e))?;
            gone.push(dated_name(&s.date));
        }
    }
    Ok(gone)
}

/// Delete pre-op snapshots past 14 days or beyond the newest 10.
pub fn prune_pre_op(dir: &Path, today: NaiveDate) -> Result<Vec<String>, DataError> {
    let names = list_pre_op(dir)?;
    let drop = pre_op_drop_set(&names, today);
    for n in &drop {
        fs::remove_file(dir.join(n)).map_err(|e| io("remove an old pre-op snapshot", e))?;
    }
    Ok(drop)
}

/// Take today's snapshot unless one exists or nothing changed since the
/// last. Answers the file written, if any.
pub fn take_daily(store: &Path, dir: &Path, today: &str) -> Result<Option<PathBuf>, DataError> {
    take_daily_with(store, dir, today, false)
}

/// [`take_daily`], or with `force` today's file is (re)written even when
/// one exists or nothing changed (the dev-build hook, [`SNAPSHOT_NOW_ENV`]).
pub fn take_daily_with(store: &Path, dir: &Path, today: &str, force: bool) -> Result<Option<PathBuf>, DataError> {
    check_date(today)?;
    if !store.exists() {
        return Ok(None);
    }
    let name = dated_name(today);
    if !force && dir.join(&name).exists() {
        return Ok(None);
    }
    let src = open_source(store)?;
    if !force {
        let live = fingerprint(&src)?;
        let last = list_dated(dir)?.into_iter().rev().find(|s| s.date.as_str() < today);
        match last {
            Some(prev) => {
                let old = fingerprint(&open_read_only(&prev.path)?)?;
                if old == live {
                    return Ok(None);
                }
            }
            None => {
                if !has_spaces(&src)? {
                    return Ok(None);
                }
            }
        }
    }
    ensure_dir(dir)?;
    let path = vacuum_to(&src, dir, &name)?;
    drop(src);
    prune_dated(dir)?;
    Ok(Some(path))
}

/// A pre-op snapshot. Answers its file name. `stamp` is
/// `YYYY-MM-DDTHHMMSS` ([`local_stamp`]). A store that doesn't exist yet
/// has nothing to keep: `store_unavailable`.
pub fn take_pre_op(store: &Path, dir: &Path, op: PreOp, stamp: &str) -> Result<String, DataError> {
    let at = parse_stamp(stamp)
        .ok_or_else(|| DataError::BadRequest("a snapshot stamp looks like 2026-10-13T091500".into()))?;
    if !store.exists() {
        return Err(DataError::Unavailable("Garden data: there's no store to snapshot yet".into()));
    }
    ensure_dir(dir)?;
    let base = format!("{PREFIX}{stamp}-{}", op.as_str());
    let mut name = format!("{base}{SUFFIX}");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{base}-{n}{SUFFIX}");
        n += 1;
    }
    let src = open_source(store)?;
    vacuum_to(&src, dir, &name)?;
    drop(src);
    prune_pre_op(dir, at.date())?;
    Ok(name)
}

/// The newest dated snapshot on or before `date`, or `no_snapshot` (with
/// the oldest date there is). Pre-op snapshots are never read here.
pub fn open_at(dir: &Path, date: &str) -> Result<Snapshot, DataError> {
    let want = check_date(date)?;
    let all = list_dated(dir)?;
    let oldest = all.first().map(|s| s.date.clone());
    all.into_iter()
        .rev()
        .find(|s| parse_date(&s.date).is_some_and(|d| d <= want))
        .ok_or(DataError::NoSnapshot { oldest })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::garden_data::sqlite::GARDENS_SCHEMA_V1;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-gd-snap-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).expect("temp dir");
        d
    }

    fn days(from: &str, n: i64) -> Vec<String> {
        let start = parse_date(from).expect("start date");
        (0..n).map(|i| (start + chrono::Duration::days(i)).format("%Y-%m-%d").to_string()).collect()
    }

    /// A store file with one space at `rev`.
    fn store_with(dir: &Path, rev: i64, at: i64) -> PathBuf {
        let p = dir.join("gardens.sqlite");
        let c = Connection::open(&p).expect("store");
        c.execute_batch("PRAGMA journal_mode = WAL;").expect("wal");
        c.execute_batch(GARDENS_SCHEMA_V1).expect("schema");
        c.execute(
            "INSERT INTO spaces (space, rev, bytes, keys, created_at, updated_at) VALUES ('data:board', ?1, 1, 1, 1, ?2)
             ON CONFLICT(space) DO UPDATE SET rev = excluded.rev, updated_at = excluded.updated_at",
            rusqlite::params![rev, at],
        )
        .expect("space row");
        c.execute(
            "INSERT OR REPLACE INTO entries (space, key, rev, deleted, value, size, updated_at, updated_by)
             VALUES ('data:board', 'obj/1', ?1, 0, ?2, 3, ?3, 'owner')",
            rusqlite::params![rev, format!("{rev}"), at],
        )
        .expect("entry");
        p
    }

    #[test]
    fn keep_set_keeps_seven_dailies_and_four_weeklies() {
        // 2026-08-01 (Sat) … 2026-10-14 (Wed): 75 days.
        let all = days("2026-08-01", 75);
        let keep = keep_set(&all);
        assert_eq!(keep.len(), SNAPSHOT_DAILY_KEEP + SNAPSHOT_WEEKLY_KEEP, "{keep:?}");
        // The 7 newest: 10-08 … 10-14.
        assert_eq!(&keep[4..], &days("2026-10-08", 7)[..]);
        // 10-08 is a Thursday (ISO week 41, Mon 10-05). The days before the
        // dailies: 10-05..10-07 (week 41) → 10-07; week 40 → 10-04 (Sun);
        // week 39 → 09-27; week 38 → 09-20.
        assert_eq!(&keep[..4], &["2026-09-20", "2026-09-27", "2026-10-04", "2026-10-07"].map(String::from)[..]);
    }

    #[test]
    fn keep_set_with_gaps_and_junk() {
        let mut all: Vec<String> = ["2026-01-05", "2026-03-02", "nope", "2026-13-01", "2026-02-02"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        all.push("2026-01-05".into());
        let keep = keep_set(&all);
        assert_eq!(keep, vec!["2026-01-05", "2026-02-02", "2026-03-02"]);
        // Fewer than 7: every one is a daily.
        assert_eq!(keep_set(&days("2026-10-10", 3)), days("2026-10-10", 3));
        assert!(keep_set(&[]).is_empty());
        // Year boundary: ISO week of 2026-01-01 (Thu) is 2026-W01; 2025-12-29 too.
        let span = days("2025-12-01", 40); // to 2026-01-09
        let keep = keep_set(&span);
        assert_eq!(&keep[4..], &days("2026-01-03", 7)[..]);
        // Before 01-03 (Sat, W01): W01 → 01-02, W52 → 12-28, W51 → 12-21, W50 → 12-14.
        assert_eq!(&keep[..4], &["2025-12-14", "2025-12-21", "2025-12-28", "2026-01-02"].map(String::from)[..]);
    }

    #[test]
    fn pre_op_retention_is_fourteen_days_and_ten_files() {
        let today = parse_date("2026-10-14").expect("date");
        let mut names: Vec<String> = (0..12)
            .map(|i| format!("gardens-2026-10-{:02}T120000-delete.sqlite", 2 + i))
            .collect();
        names.push("gardens-2026-09-29T120000-import.sqlite".into()); // 15 days old
        names.push("gardens-2026-10-13.sqlite".into()); // dated: never a pre-op
        names.push("gardens-2026-10-13T120000-other.sqlite".into()); // not an op
        let drop = pre_op_drop_set(&names, today);
        assert!(drop.contains(&"gardens-2026-09-29T120000-import.sqlite".to_string()));
        // 12 within 14 days: the 2 oldest go.
        assert!(drop.contains(&"gardens-2026-10-02T120000-delete.sqlite".to_string()));
        assert!(drop.contains(&"gardens-2026-10-03T120000-delete.sqlite".to_string()));
        assert_eq!(drop.len(), 3, "{drop:?}");
    }

    #[test]
    fn daily_snapshot_skips_when_unchanged_and_never_twice_a_day() {
        let d = tmp();
        let snaps = d.join(DIR_NAME);
        // No store yet: nothing.
        assert_eq!(take_daily(&d.join("gardens.sqlite"), &snaps, "2026-10-12").expect("no store"), None);
        let store = store_with(&d, 3, 100);
        let first = take_daily(&store, &snaps, "2026-10-12").expect("first").expect("written");
        assert!(first.ends_with("gardens-2026-10-12.sqlite"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&first).expect("meta").permissions().mode() & 0o777, 0o600);
            assert_eq!(fs::metadata(&snaps).expect("meta").permissions().mode() & 0o777, 0o700);
        }
        // Same day again: no.
        assert_eq!(take_daily(&store, &snaps, "2026-10-12").expect("again"), None);
        // Next day, nothing changed: skipped.
        assert_eq!(take_daily(&store, &snaps, "2026-10-13").expect("unchanged"), None);
        // A write: taken.
        store_with(&d, 4, 200);
        assert!(take_daily(&store, &snaps, "2026-10-14").expect("changed").is_some());
        // Forced: rewrites today's file even though nothing changed.
        assert!(take_daily_with(&store, &snaps, "2026-10-14", true).expect("forced").is_some());
        // The snapshot is out of WAL and opening it read-only leaves no side files.
        let s = open_at(&snaps, "2026-10-14").expect("open_at");
        let c = open_read_only(&s.path).expect("ro");
        let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).expect("mode");
        assert_eq!(mode, "delete");
        let rev: i64 = c.query_row("SELECT rev FROM spaces", [], |r| r.get(0)).expect("rev");
        assert_eq!(rev, 4);
        drop(c);
        let names = names_in(&snaps).expect("names");
        assert!(names.iter().all(|n| !n.ends_with("-wal") && !n.ends_with("-shm") && !n.ends_with(".tmp")), "{names:?}");
        fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn daily_prunes_to_the_keep_set() {
        let d = tmp();
        let snaps = d.join(DIR_NAME);
        let store = store_with(&d, 1, 1);
        let all = days("2026-08-01", 30);
        for (i, day) in all.iter().enumerate() {
            store_with(&d, 2 + i as i64, 10 + i as i64);
            take_daily(&store, &snaps, day).expect("daily").expect("written");
        }
        let kept: Vec<String> = list_dated(&snaps).expect("list").into_iter().map(|s| s.date).collect();
        assert_eq!(kept, keep_set(&all));
        assert_eq!(kept.len(), SNAPSHOT_DAILY_KEEP + SNAPSHOT_WEEKLY_KEEP);
        fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn pre_op_snapshots_never_count_for_at() {
        let d = tmp();
        let snaps = d.join(DIR_NAME);
        let store = store_with(&d, 1, 1);
        let a = take_pre_op(&store, &snaps, PreOp::Delete, "2026-10-13T091500").expect("pre-op");
        assert_eq!(a, "gardens-2026-10-13T091500-delete.sqlite");
        let b = take_pre_op(&store, &snaps, PreOp::Delete, "2026-10-13T091500").expect("same second");
        assert_eq!(b, "gardens-2026-10-13T091500-delete-2.sqlite");
        assert_eq!(list_pre_op(&snaps).expect("pre-ops").len(), 2);
        // Only pre-op files: --at finds nothing.
        assert_eq!(open_at(&snaps, "2026-10-14"), Err(DataError::NoSnapshot { oldest: None }));
        assert!(matches!(take_pre_op(&store, &snaps, PreOp::Move, "yesterday"), Err(DataError::BadRequest(_))));
        assert!(matches!(
            take_pre_op(&d.join("missing.sqlite"), &snaps, PreOp::Import, "2026-10-13T091500"),
            Err(DataError::Unavailable(_))
        ));
        fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn open_at_picks_the_newest_on_or_before() {
        let d = tmp();
        for day in ["2026-10-08", "2026-10-10", "2026-10-12"] {
            fs::write(d.join(dated_name(day)), b"").expect("file");
        }
        fs::write(d.join("gardens-2026-10-11T100000-import.sqlite"), b"").expect("pre-op");
        assert_eq!(open_at(&d, "2026-10-11").expect("10-11").date, "2026-10-10");
        assert_eq!(open_at(&d, "2026-10-12").expect("10-12").date, "2026-10-12");
        assert_eq!(open_at(&d, "2027-01-01").expect("later").date, "2026-10-12");
        assert_eq!(open_at(&d, "2026-10-07"), Err(DataError::NoSnapshot { oldest: Some("2026-10-08".into()) }));
        assert!(matches!(open_at(&d, "10/11/2026"), Err(DataError::BadRequest(_))));
        assert!(matches!(open_at(&d, "2026-02-30"), Err(DataError::BadRequest(_))));
        assert_eq!(
            open_at(&d.join("none"), "2026-10-11"),
            Err(DataError::NoSnapshot { oldest: None }),
            "a missing folder is no snapshot"
        );
        fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn local_dates_have_the_contract_shapes() {
        assert!(parse_date(&local_date(1_760_423_520)).is_some());
        assert!(parse_stamp(&local_stamp(1_760_423_520)).is_some());
    }
}
