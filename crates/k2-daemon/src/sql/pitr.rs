//! Point-in-time restore of named clusters (prd-app-supabase-compat-v1
//! §11.1, lane E): the daemon's side of the root helper's protocol-19
//! verbs ([`k2_root_proto::pg_restore`]).
//!
//! - `k2 db cluster basebackup <name>`: a base backup now; the daemon also
//!   takes one a day of every archiving named cluster ([`tick`], headless).
//! - `k2 db cluster backups <name>`: what is on the box, the window a
//!   restore can reach, the last backup and drill K2 recorded (and a
//!   warning after 30 days without a good drill).
//! - `k2 db cluster restore <name> --to <time> --as <new>`: a NEW cluster
//!   beside the old one, recorded as K2's; never in place.
//! - `k2 db cluster restore-drill <name> [--to <time>]`: restore into a
//!   throwaway cluster, check it, remove it, record the result. Without
//!   `--to` it writes two marker rows in the source's `postgres` database
//!   around a moment, restores to that moment and proves the first is
//!   there and the second isn't, and compares every table's row count
//!   taken at that moment.
//!
//! K2 Backups ships the spool and the base backups off the box; this
//! module only writes them where it looks.

use serde_json::{json, Value};

use k2_root_proto::pg_named::ClusterName;
use k2_root_proto::pg_restore::UtcTime;

use super::cluster::{self, Cluster, EnsureError};
use super::sysops::SystemOps;

/// Warn when the last good drill is older than this.
pub const DRILL_WARN_DAYS: i64 = 30;
/// One automatic base backup a day per archiving named cluster.
pub const BASEBACKUP_EVERY_SECS: i64 = 86_400;
/// A failed daily base backup is tried again this soon.
pub const BASEBACKUP_RETRY_SECS: i64 = 3600;
/// How often the daemon looks (headless; no client needed).
pub const TICK_SECS: u64 = 1800;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── times ───────────────────────────────────────────────────────────────

/// A restore time as people and agents write it, in UTC unless an offset
/// is given:
/// - `2026-11-22T10:15:00Z`, `2026-11-22T10:15Z`, `2026-11-22 10:15[:00]`;
/// - with an offset: `2026-11-22T12:15:00+02:00`;
/// - relative: `-15m`, `-2h`, `-1d` (before `now`).
pub fn parse_time(s: &str, now: i64) -> Result<UtcTime, String> {
    let s = s.trim();
    let bad = || {
        format!(
            "{s:?} is not a time this reads: 2026-11-22T10:15:00Z (UTC), 2026-11-22 10:15 (UTC), \
             2026-11-22T12:15:00+02:00, or -15m / -2h / -1d"
        )
    };
    // Review L7: every form is ASCII; slicing below never splits a char.
    if !s.is_ascii() {
        return Err(bad());
    }
    if let Some(rel) = s.strip_prefix('-') {
        let (n, unit) = rel.split_at(rel.len().saturating_sub(1));
        let n: i64 = n.parse().map_err(|_| bad())?;
        let mul = match unit {
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            _ => return Err(bad()),
        };
        if n <= 0 || n > 3650 * 86_400 / mul {
            return Err(bad());
        }
        return Ok(UtcTime::from_epoch(now - n * mul));
    }
    let s2 = s.replacen(' ', "T", 1);
    let (body, offset) = if let Some(b) = s2.strip_suffix('Z') {
        (b.to_string(), 0i64)
    } else if s2.len() > 6 && matches!(s2.as_bytes()[s2.len() - 6], b'+' | b'-') && s2.as_bytes()[s2.len() - 3] == b':' {
        let (b, off) = s2.split_at(s2.len() - 6);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let h: i64 = off[1..3].parse().map_err(|_| bad())?;
        let m: i64 = off[4..6].parse().map_err(|_| bad())?;
        if h > 14 || m > 59 {
            return Err(bad());
        }
        (b.to_string(), sign * (h * 3600 + m * 60))
    } else {
        (s2, 0)
    };
    let full = match body.len() {
        16 => format!("{body}:00Z"),
        19 => format!("{body}Z"),
        _ => return Err(bad()),
    };
    let t = UtcTime::parse(&full).ok_or_else(bad)?;
    Ok(UtcTime::from_epoch(t.epoch() - offset))
}

// ── the helper ──────────────────────────────────────────────────────────

/// A helper refusal K2 passes on as the person's (409), else a helper
/// failure (502).
fn helper_err(e: String) -> EnsureError {
    const REFUSALS: &[&str] = &[
        "not_k2_cluster",
        "not_drill",
        "name_taken",
        "no_archive",
        "cluster_down",
        "no_base_backup",
        "wal_gap",
        "target_after_wal",
        "target_in_future",
        "archive_behind",
        "named_full",
        "band_full",
        "busy",
    ];
    if REFUSALS.iter().any(|c| e.contains(c)) {
        EnsureError::Refused(e)
    } else {
        EnsureError::Helper(e)
    }
}

fn call(ops: &dyn SystemOps, argv: &[&str]) -> Result<Value, EnsureError> {
    ops.helper_pgrestore().map_err(EnsureError::Helper)?;
    let raw = ops.run_helper(argv, None).map_err(helper_err)?;
    serde_json::from_str(raw.trim()).map_err(|e| EnsureError::Helper(format!("{}: not JSON ({e})", argv[0])))
}

// ── records (0328 `pg_cluster_pitr`) ─────────────────────────────────────

fn record_backup(c: &Cluster, ok: bool, note: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "INSERT INTO pg_cluster_pitr (major, name, last_basebackup_at, last_basebackup_ok, last_basebackup_note) \
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(major, name) DO UPDATE SET \
         last_basebackup_at = ?3, last_basebackup_ok = ?4, last_basebackup_note = ?5",
        rusqlite::params![c.major as i64, c.name, now(), ok as i64, note],
    );
}

fn record_drill(c: &Cluster, ok: bool, v: &Value) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "INSERT INTO pg_cluster_pitr (major, name, last_drill_at, last_drill_ok, last_drill_json) \
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(major, name) DO UPDATE SET \
         last_drill_at = ?3, last_drill_ok = ?4, last_drill_json = ?5",
        rusqlite::params![c.major as i64, c.name, now(), ok as i64, v.to_string()],
    );
}

/// K2's record of a cluster's last base backup and drill.
pub fn recorded(c: &Cluster) -> Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = conn
        .query_row(
            "SELECT last_basebackup_at, last_basebackup_ok, last_basebackup_note, last_drill_at, last_drill_ok \
             FROM pg_cluster_pitr WHERE major = ?1 AND name = ?2",
            rusqlite::params![c.major as i64, c.name],
            |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .ok();
    let (bb_at, bb_ok, bb_note, dr_at, dr_ok) = row.unwrap_or_default();
    let t = |x: Option<i64>| x.map(|e| UtcTime::from_epoch(e).token());
    let last_good_drill = dr_at.filter(|_| dr_ok == Some(1));
    let warning = match last_good_drill {
        Some(at) if now() - at <= DRILL_WARN_DAYS * 86_400 => None,
        Some(_) => Some(format!(concat!("the last good restore drill is more than {} days old: run `", k2_core::cli_name!(), " db cluster restore-drill {}`"), DRILL_WARN_DAYS, c.name)),
        None => Some(format!(concat!("no good restore drill yet: run `", k2_core::cli_name!(), " db cluster restore-drill {}`"), c.name)),
    };
    json!({
        "lastBasebackupAt": t(bb_at),
        "lastBasebackupOk": bb_ok.map(|v| v == 1),
        "lastBasebackupNote": bb_note,
        "lastDrillAt": t(dr_at),
        "lastDrillOk": dr_ok.map(|v| v == 1),
        "drillWarning": warning,
    })
}

// ── verbs ───────────────────────────────────────────────────────────────

/// `k2 db cluster basebackup <name>`.
pub fn basebackup(ops: &dyn SystemOps, c: &Cluster) -> Result<Value, EnsureError> {
    match call(ops, &["basebackup", &c.wire()]) {
        Ok(v) => {
            record_backup(c, true, v.get("stamp").and_then(Value::as_str).unwrap_or(""));
            Ok(json!({"ok": true, "cluster": c.label(), "backup": v}))
        }
        Err(e) => {
            record_backup(c, false, &e.hint().chars().take(300).collect::<String>());
            Err(e)
        }
    }
}

/// `k2 db cluster backups <name>`.
pub fn backups(ops: &dyn SystemOps, c: &Cluster) -> Result<Value, EnsureError> {
    let v = call(ops, &["backups", &c.wire()])?;
    Ok(json!({"ok": true, "cluster": c.label(), "onBox": v, "recorded": recorded(c)}))
}

fn check_new_name(c: &Cluster, new: &str) -> Result<ClusterName, EnsureError> {
    let n = ClusterName::parse(new).ok_or_else(|| {
        EnsureError::Usage(format!("--as {new:?}: 2-24 of a-z, 0-9 and _, starting with a letter, never main"))
    })?;
    if n.as_str() == c.name {
        return Err(EnsureError::Usage("--as: a restore is always a new cluster beside the old one; give it another name".into()));
    }
    if let Some(k) = cluster::for_name(new) {
        return Err(EnsureError::Refused(format!("the name {new} is already {}: cluster names are one per box", k.label())));
    }
    Ok(n)
}

fn restore_argv(c: &Cluster, new: &ClusterName, target: UtcTime, drill: bool) -> Vec<String> {
    let mut v = vec!["restore-to".to_string(), c.wire(), new.as_str().to_string(), target.stamp()];
    if drill {
        v.push("--drill".into());
    }
    v
}

fn restored_cluster(v: &Value, c: &Cluster, new: &ClusterName) -> Result<Cluster, EnsureError> {
    let mut r = cluster::parse_helper_cluster(v).map_err(EnsureError::Helper)?;
    if r.major != c.major || r.name != new.as_str() || r.k2_named != Some(true) {
        return Err(EnsureError::Helper(format!("restore-to answered {} (k2Named {:?}), not {}/{}", r.label(), r.k2_named, c.major, new.as_str())));
    }
    r.role = c.role.clone();
    r.origin = "created".into();
    r.owner_project = c.owner_project.clone();
    r.wal_archive = false;
    Ok(r)
}

/// `k2 db cluster restore <name> --to <time> --as <new>`: the new cluster
/// is recorded as K2's, in the source's workspace.
pub fn restore(ops: &dyn SystemOps, c: &Cluster, as_name: &str, target: UtcTime) -> Result<Value, EnsureError> {
    let new = check_new_name(c, as_name)?;
    let _busy = Busy::take(c)?;
    let argv = restore_argv(c, &new, target, false);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let v = call(ops, &refs)?;
    let r = restored_cluster(&v, c, &new)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        cluster::record_in(&conn, &r, true).map_err(EnsureError::Helper)?;
    }
    Ok(json!({
        "ok": true,
        "cluster": r.label(),
        "port": r.port,
        "restoredFrom": c.label(),
        "target": target.token(),
        "baseBackup": v.get("baseBackup"),
        "walReplayed": v.get("walReplayed"),
        "next": format!(
            concat!("check it (`", k2_core::cli_name!(), " db cluster backups {}` / psql on port {}), then point the App at it; the old cluster {} is unchanged"),
            r.name, r.port, c.label()
        ),
    }))
}

/// `k2 db cluster drop-drill <name>`: the helper removes only a drill or a
/// failed restore; K2's record of it goes too.
pub fn drop_drill(ops: &dyn SystemOps, major: u32, name: &str) -> Result<Value, EnsureError> {
    let n = ClusterName::parse(name).ok_or_else(|| EnsureError::Usage(format!("{name:?} is not a cluster name")))?;
    let v = call(ops, &["drop-drill", &k2_root_proto::pg_named::ref_token(
        k2_root_proto::pg_major::PgMajor::new(major).ok_or_else(|| EnsureError::Usage(format!("major {major}")))?,
        &n,
    )])?;
    forget(major, name);
    Ok(v)
}

fn forget(major: u32, name: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute("DELETE FROM sql_clusters WHERE major = ?1 AND name = ?2", rusqlite::params![major as i64, name]);
}

// ── the drill ───────────────────────────────────────────────────────────

/// The drill cluster's name: `<name>_d<4 hex>` (review L8): the hash of
/// the source's name, so it never is a name someone picks by hand for
/// another cluster (`feedback_drill`), cut to fit 24.
pub fn drill_name(name: &str) -> String {
    let h = name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    format!("{}_d{:04x}", &name[..name.len().min(18)], h & 0xffff)
}

/// Review L1: the markers live in a schema K2 makes for the drill, owned by
/// the superuser, checked before each write (a plain table, no triggers,
/// both owned by `postgres`), dropped after.
const MARKER_SCHEMA: &str = "k2_restore_drill";
const MARKER_TABLE: &str = "k2_restore_drill.markers";
const MARKER_CHECK_SQL: &str = "SELECT c.relkind::text || '|' || pg_catalog.pg_get_userbyid(c.relowner) || '|' || pg_catalog.pg_get_userbyid(n.nspowner) || '|' || \
(SELECT count(*) FROM pg_catalog.pg_trigger t WHERE t.tgrelid = c.oid) || '|' || \
(SELECT count(*) FROM pg_catalog.pg_rules r WHERE r.schemaname = n.nspname AND r.tablename = c.relname) \
FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
WHERE n.nspname = 'k2_restore_drill' AND c.relname = 'markers';\n";

fn markers_safe(ops: &dyn SystemOps, c: &Cluster) -> Result<(), EnsureError> {
    let got = psql(ops, c, "postgres", MARKER_CHECK_SQL)?;
    let got = got.lines().last().unwrap_or("").trim().to_string();
    if got != "r|postgres|postgres|0|0" {
        return Err(EnsureError::Refused(format!(
            "{}: the drill's marker table isn't a plain table owned by postgres without triggers or rules ({got:?}); drop schema {MARKER_SCHEMA} and run the drill again",
            c.label()
        )));
    }
    Ok(())
}

/// Every user table's row count in one database, `schema.table|count`
/// lines (as postgres: row security doesn't hide rows from the count).
pub const COUNT_SQL: &str = "SELECT n.nspname || '.' || c.relname || '|' || \
(xpath('/row/c/text()', query_to_xml(format('SELECT count(*) AS c FROM %I.%I', n.nspname, c.relname), false, true, '')))[1]::text \
FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
WHERE c.relkind = 'r' AND n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg\\_%' \
ORDER BY 1;\n";
/// The databases of a cluster a drill checks.
pub const DATABASES_SQL: &str =
    "SELECT datname FROM pg_catalog.pg_database WHERE NOT datistemplate AND datname <> 'postgres' ORDER BY 1;\n";

fn psql(ops: &dyn SystemOps, c: &Cluster, db: &str, sql: &str) -> Result<String, EnsureError> {
    c.helper_psql(ops, db, &["-v", "ON_ERROR_STOP=1", "-tA"], Some(sql.as_bytes()))
        .map_err(|e| EnsureError::Helper(format!("{} -d {db}: {e}", c.label())))
}

fn db_name_ok(d: &str) -> bool {
    !d.is_empty() && d.len() <= 63 && d.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

type Counts = Vec<(String, Vec<(String, i64)>)>;

fn counts(ops: &dyn SystemOps, c: &Cluster) -> Result<Counts, EnsureError> {
    let dbs = psql(ops, c, "postgres", DATABASES_SQL)?;
    let mut out = Vec::new();
    for d in dbs.lines().map(str::trim).filter(|d| db_name_ok(d)) {
        let rows = psql(ops, c, d, COUNT_SQL)?;
        let mut t = Vec::new();
        for l in rows.lines().filter(|l| !l.trim().is_empty()) {
            let (name, n) = l.rsplit_once('|').ok_or_else(|| EnsureError::Helper(format!("count answer {l:?}")))?;
            t.push((name.to_string(), n.trim().parse().map_err(|_| EnsureError::Helper(format!("count answer {l:?}")))?));
        }
        out.push((d.to_string(), t));
    }
    Ok(out)
}

/// The two marker rows around the drill's moment; the moment is the next
/// whole second after the first row's commit, and the second row commits
/// after it. Answers the moment.
fn mark(ops: &dyn SystemOps, c: &Cluster, run: &str) -> Result<UtcTime, EnsureError> {
    psql(
        ops,
        c,
        "postgres",
        &format!(
            "CREATE SCHEMA IF NOT EXISTS {MARKER_SCHEMA} AUTHORIZATION postgres;\n\
             CREATE TABLE IF NOT EXISTS {MARKER_TABLE} (id text PRIMARY KEY, at timestamptz NOT NULL DEFAULT clock_timestamp());\n"
        ),
    )?;
    markers_safe(ops, c)?;
    let first = format!(
        "INSERT INTO {MARKER_TABLE} (id) VALUES ('{run}:before');\n\
         SELECT ceil(extract(epoch FROM clock_timestamp()))::bigint;\n"
    );
    let out = psql(ops, c, "postgres", &first)?;
    let t: i64 = out.lines().last().unwrap_or("").trim().parse().map_err(|_| EnsureError::Helper(format!("drill moment answer {out:?}")))?;
    let past = format!("SELECT (extract(epoch FROM clock_timestamp()) > {t} + 1)::text;\n");
    let mut passed = false;
    for _ in 0..40 {
        if psql(ops, c, "postgres", &past)?.trim() == "true" {
            passed = true;
            break;
        }
        ops.sleep_ms(250);
    }
    if !passed {
        return Err(EnsureError::Helper("the source's clock didn't pass the drill's moment".into()));
    }
    markers_safe(ops, c)?;
    psql(ops, c, "postgres", &format!("INSERT INTO {MARKER_TABLE} (id) VALUES ('{run}:after');\n"))?;
    Ok(UtcTime::from_epoch(t))
}

/// Review M4: one restore or drill per cluster at a time, and a drill at
/// most every [`DRILL_MIN_GAP_SECS`] (each copies the whole cluster).
pub const DRILL_MIN_GAP_SECS: i64 = 600;

fn in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Held while a restore or a drill of `c` runs.
struct Busy(String);
impl Busy {
    fn take(c: &Cluster) -> Result<Self, EnsureError> {
        let key = c.label();
        let mut s = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        if !s.insert(key.clone()) {
            return Err(EnsureError::Refused(format!("busy: a restore or drill of {key} is already running")));
        }
        Ok(Self(key))
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        in_flight().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// `k2 db cluster restore-drill <name> [--to <time>]`.
pub fn drill(ops: &dyn SystemOps, c: &Cluster, at: Option<UtcTime>) -> Result<Value, EnsureError> {
    ops.helper_pgrestore().map_err(EnsureError::Helper)?;
    let _busy = Busy::take(c)?;
    let last = recorded(c)["lastDrillAt"].as_str().and_then(UtcTime::parse).map(UtcTime::epoch);
    if let Some(l) = last.filter(|l| now() - l < DRILL_MIN_GAP_SECS) {
        return Err(EnsureError::Refused(format!(
            "busy: the last drill of {} was at {} UTC; drills run at most every {} minutes",
            c.label(),
            UtcTime::from_epoch(l).token(),
            DRILL_MIN_GAP_SECS / 60
        )));
    }
    if c.state == "down" && at.is_none() {
        return Err(EnsureError::Refused(format!("{} isn't running; a drill to now needs it (or give --to)", c.label())));
    }
    let dname = drill_name(&c.name);
    let new = ClusterName::parse(&dname).ok_or_else(|| EnsureError::Helper(format!("drill name {dname}")))?;
    // A drill a crash left behind goes first (the helper refuses anything
    // that isn't one).
    if let Err(e) = drop_drill(ops, c.major, &dname) {
        if !e.hint().contains("not_drill") {
            return Err(e);
        }
    }
    let have = call(ops, &["backups", &c.wire()])?;
    let complete = have["baseBackups"].as_array().is_some_and(|a| a.iter().any(|b| b["complete"] == true));
    if !complete {
        basebackup(ops, c)?;
    }
    let run = format!("k2drill{}", now());
    let (target, before) = match at {
        Some(t) => (t, None),
        None => {
            let before = counts(ops, c)?;
            (mark(ops, c, &run)?, Some(before))
        }
    };
    let argv = restore_argv(c, &new, target, true);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let restored = call(ops, &refs);
    let result = restored.and_then(|v| {
        let r = restored_cluster(&v, c, &new)?;
        let after = counts(ops, &r)?;
        let markers = if at.is_none() {
            let got = psql(ops, &r, "postgres", &format!("SELECT split_part(id, ':', 2) FROM {MARKER_TABLE} WHERE id LIKE '{run}:%' ORDER BY 1;\n"))?;
            Some(got.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect::<Vec<_>>())
        } else {
            None
        };
        Ok((v, after, markers))
    });
    // Always: the throwaway cluster goes, the source's markers go.
    let dropped = drop_drill(ops, c.major, &dname).map(|v| v["dropped"] == true).unwrap_or(false);
    if at.is_none() {
        let _ = psql(ops, c, "postgres", &format!("DROP SCHEMA IF EXISTS {MARKER_SCHEMA} CASCADE;\n"));
    }
    let (v, after, markers) = match result {
        Ok(x) => x,
        Err(e) => {
            let out = json!({"ok": false, "cluster": c.label(), "target": target.token(), "error": e.hint(), "dropped": dropped});
            record_drill(c, false, &out);
            return Err(e);
        }
    };
    let markers_ok = markers.as_ref().map(|m| m == &["before".to_string()]);
    let mut dbs = Vec::new();
    let mut rows_ok = true;
    for (d, tables) in &after {
        let mut entry = json!({"database": d, "tables": tables.len(), "rows": tables.iter().map(|(_, n)| n).sum::<i64>()});
        if let Some(b) = &before {
            let src = b.iter().find(|(n, _)| n == d).map(|(_, t)| t.clone()).unwrap_or_default();
            let diffs: Vec<Value> = src
                .iter()
                .filter_map(|(t, n)| {
                    let got = tables.iter().find(|(x, _)| x == t).map(|(_, m)| *m);
                    (got != Some(*n)).then(|| json!({"table": t, "atTheMoment": n, "restored": got}))
                })
                .collect();
            rows_ok &= diffs.is_empty();
            entry["rowsMatch"] = json!(diffs.is_empty());
            entry["diffs"] = json!(diffs);
        }
        dbs.push(entry);
    }
    let missing: Vec<String> = before
        .as_ref()
        .map(|b| b.iter().filter(|(d, _)| !after.iter().any(|(x, _)| x == d)).map(|(d, _)| d.clone()).collect())
        .unwrap_or_default();
    let ok = markers_ok.unwrap_or(true) && rows_ok && missing.is_empty() && dropped;
    let out = json!({
        "ok": ok,
        "cluster": c.label(),
        "drillCluster": format!("{}/{}", c.major, dname),
        "target": target.token(),
        "baseBackup": v.get("baseBackup"),
        "walReplayed": v.get("walReplayed"),
        "markers": markers.map(|m| json!({"before": m.iter().any(|x| x == "before"), "after": m.iter().any(|x| x == "after")})),
        "databases": dbs,
        "missingDatabases": missing,
        "storageCheck": "not checked: the /storage/v1 door (lane D) isn't built yet",
        "dropped": dropped,
        "hint": if ok { Value::Null } else { json!("row counts are taken just before the moment: writes landing in that second show as diffs; run the drill again in a quiet minute, or give --to for a moment in the past") },
    });
    record_drill(c, ok, &out);
    Ok(out)
}

// ── the daily base backup (headless) ────────────────────────────────────

/// One pass: every K2 named cluster that archives and runs, with no good
/// base backup in the last day, gets one. Answers the clusters it backed up.
pub fn tick(ops: &dyn SystemOps) -> Vec<String> {
    if ops.helper_pgrestore().is_err() {
        return Vec::new();
    }
    let due: Vec<Cluster> = cluster::list()
        .into_iter()
        .filter(|c| c.is_named() && c.holds_k2_databases() && c.origin == "created" && c.wal_archive && c.state != "down")
        .filter(|c| {
            let r = recorded(c);
            let last = r["lastBasebackupAt"].as_str().and_then(UtcTime::parse).map(UtcTime::epoch);
            // Review L2: a failed one is tried again after an hour.
            let every = if r["lastBasebackupOk"] == false { BASEBACKUP_RETRY_SECS } else { BASEBACKUP_EVERY_SECS };
            last.is_none_or(|t| now() - t >= every)
        })
        .collect();
    let mut done = Vec::new();
    for c in due {
        match basebackup(ops, &c) {
            Ok(_) => done.push(c.label()),
            Err(e) => eprintln!("[db] daily base backup of {} failed: {}", c.label(), e.hint()),
        }
    }
    done
}

/// The daily base backups, headless (one look every [`TICK_SECS`]).
pub fn spawn_loop() {
    if !crate::sql::sql_supported() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        let mut iv = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            iv.tick().await;
            let _ = tokio::task::spawn_blocking(|| tick(crate::sql::routes::current_ops())).await;
        }
    });
}

#[cfg(test)]
#[path = "pitr_tests.rs"]
mod tests;
