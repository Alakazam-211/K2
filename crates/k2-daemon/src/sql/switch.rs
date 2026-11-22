//! `k2 db cluster switch <db> --to <cluster>` (prd-app-supabase-compat-v1
//! §11.1 addendum): point one database at a cluster `restore-to` made from
//! its current one, or back at the one it was switched away from.
//!
//! K2 finds a database by name: the newest active `sql_databases` row with
//! that name names its cluster ([`super::cluster::for_db`]); every DSN, App
//! URL, door and psql follows it. A restored cluster holds the same
//! database names, so nothing reaches it until that row moves. The switch:
//!
//! 1. **Checks**: the target is K2's, on the same major, and either a
//!    promoted restore of the database's current cluster (forward) or the
//!    cluster the database was switched away from (rollback); the database
//!    exists there, owned by `<db>_migrator`.
//! 2. **Shows the moment** the data goes back to and what is lost; nothing
//!    changes until the caller confirms (`--yes`).
//! 3. **Closes the old copy** (`ALLOW_CONNECTIONS false` and its sessions
//!    ended: no write lands on both) and opens the target's.
//! 4. **Repoints the row in one transaction** with the switch's own record:
//!    exactly one active row per name before, during and after.
//! 4b. **Sets K2's own logins again** on the target (`<db>_migrator`,
//!    `<db>_agent`, each granted workspace's login) to their passwords now:
//!    a restored cluster keeps no login until K2 does (security review M3).
//! 5. **Re-creates each App's side** on the target: its login and password,
//!    `DATABASE_URL` / `DATABASE_MIGRATE_URL` with the new port, its door
//!    and nft ports, a re-host (`k2 app db connect` again, same login).
//! 6. **Archiving** on a restored target, and a base backup, so it can be
//!    restored to a time in turn.
//! 7. The old cluster is **kept, closed**, for the rollback
//!    (`switch <db> --to <old>`).
//!
//! No new root-helper verb: the SQL runs through the helper's `psql
//! --cluster <M>-<name>` as every other K2 cluster change does.

use serde_json::{json, Value};

use super::cluster::{self, Cluster, EnsureError};
use super::secrets::SecretStore;
use super::sysops::SystemOps;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn token(t: i64) -> String {
    k2_root_proto::pg_restore::UtcTime::from_epoch(t).token()
}

/// A database name the helper's psql takes (`[a-z0-9_]{1,63}`).
fn db_name_ok(d: &str) -> bool {
    !d.is_empty() && d.len() <= 63 && d.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn psql(ops: &dyn SystemOps, c: &Cluster, db: &str, sql: &str) -> Result<String, EnsureError> {
    c.helper_psql(ops, db, &["-v", "ON_ERROR_STOP=1", "-tA"], Some(sql.as_bytes()))
        .map_err(|e| EnsureError::Helper(format!("{} -d {db}: {e}", c.label())))
}

/// Which way a switch goes, and the moment its data goes back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direction {
    /// To a restore of the current cluster: data after the restore's moment
    /// stays behind in the old cluster.
    Restore { moment: i64 },
    /// Back to the cluster it was switched away from: writes since that
    /// switch stay behind in the restored one.
    Rollback { since: i64 },
}

/// The database's active row: (id, project).
fn active_row(db: &str) -> Result<(String, String), EnsureError> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut st = conn
        .prepare("SELECT id, project_id FROM sql_databases WHERE name = ?1 AND status = 'active'")
        .map_err(|e| EnsureError::Helper(e.to_string()))?;
    let rows: Vec<(String, String)> = st
        .query_map(rusqlite::params![db], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| EnsureError::Helper(e.to_string()))?
        .flatten()
        .collect();
    match rows.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(EnsureError::NotFound(format!("no active database named {db}"))),
        _ => Err(EnsureError::Refused(format!("{} active rows are named {db}: the switch won't guess which one moves", rows.len()))),
    }
}

/// How many active rows carry `db`'s name (tests read it).
pub fn active_rows_named(db: &str) -> i64 {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row("SELECT count(*) FROM sql_databases WHERE name = ?1 AND status = 'active'", rusqlite::params![db], |r| r.get(0))
        .unwrap_or(-1)
}

/// `restore-to`'s record of a restored cluster: (source name, moment).
fn restored_of(c: &Cluster) -> Option<(String, i64)> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row(
        "SELECT restored_from, restored_to FROM pg_cluster_pitr WHERE major = ?1 AND name = ?2 AND restored_from IS NOT NULL",
        rusqlite::params![c.major as i64, c.name],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )
    .ok()
}

/// The database's last switch: (from cluster, to cluster, at).
fn last_switch(db: &str) -> Option<(String, String, i64)> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row(
        "SELECT from_cluster, to_cluster, at FROM pg_db_switches WHERE db_name = ?1 ORDER BY at DESC, rowid DESC LIMIT 1",
        rusqlite::params![db],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

#[cfg(test)]
pub(crate) mod test_seam {
    //! Tests: fail the repoint after its UPDATE, inside the transaction.
    use std::cell::Cell;
    thread_local! { pub static FAIL_REPOINT: Cell<bool> = const { Cell::new(false) }; }
}

/// Step 4: the row and the switch's record, in one transaction. A failure
/// anywhere rolls both back.
fn repoint(id: &str, db: &str, from: &Cluster, to: &Cluster, dir: &Direction, by: &str) -> Result<(), EnsureError> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let tx = conn.unchecked_transaction().map_err(|e| EnsureError::Helper(format!("begin: {e}")))?;
    let n = tx
        .execute(
            "UPDATE sql_databases SET cluster_major = ?2, cluster_name = ?3 WHERE id = ?1 AND status = 'active'",
            rusqlite::params![id, to.major as i64, to.name],
        )
        .map_err(|e| EnsureError::Helper(format!("repoint: {e}")))?;
    if n != 1 {
        return Err(EnsureError::Helper(format!("repoint touched {n} rows; nothing changed")));
    }
    #[cfg(test)]
    if test_seam::FAIL_REPOINT.with(|f| f.get()) {
        return Err(EnsureError::Helper("injected failure after the UPDATE".into()));
    }
    let (kind, moment) = match dir {
        Direction::Restore { moment } => ("restore", *moment),
        Direction::Rollback { since } => ("rollback", *since),
    };
    tx.execute(
        "INSERT INTO pg_db_switches (db_name, database_id, from_cluster, to_cluster, kind, moment, at, by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![db, id, from.name, to.name, kind, moment, now(), by],
    )
    .map_err(|e| EnsureError::Helper(format!("record the switch: {e}")))?;
    tx.commit().map_err(|e| EnsureError::Helper(format!("commit: {e}")))
}

/// K2's own logins of `db` with their passwords now (LOGIN again): the
/// migrator, the workspace's agent, each granted workspace's login. The
/// SQL goes to the helper's psql on stdin, never argv; a role whose secret
/// K2 doesn't hold is left as it is (named in the answer).
fn rekey_sql(secrets: &dyn SecretStore, db: &str, db_id: &str) -> Result<(String, Vec<String>, Vec<String>), EnsureError> {
    let (agent_ref, migrator_ref, grants) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let (a, m): (Option<String>, Option<String>) = conn
            .query_row("SELECT agent_secret_ref, migrator_secret_ref FROM sql_databases WHERE id = ?1", rusqlite::params![db_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(|e| EnsureError::Helper(format!("read {db}'s logins: {e}")))?;
        let mut st = conn
            .prepare("SELECT project_id, agent_secret_ref FROM sql_grants WHERE database_id = ?1 AND agent_secret_ref IS NOT NULL ORDER BY project_id")
            .map_err(|e| EnsureError::Helper(e.to_string()))?;
        let g: Vec<(String, String)> = st
            .query_map(rusqlite::params![db_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| EnsureError::Helper(e.to_string()))?
            .flatten()
            .collect();
        (a, m, g)
    };
    let mut want: Vec<(String, Option<String>)> = vec![(format!("{db}_migrator"), migrator_ref), (format!("{db}_agent"), agent_ref)];
    want.extend(grants.into_iter().map(|(p, r)| (super::ops::default_agent_role(&p), Some(r))));
    let (mut sql, mut done, mut skipped) = (String::new(), Vec::new(), Vec::new());
    for (role, sref) in want {
        match sref.map(|r| secrets.resolve(&r)) {
            Some(Ok(Some(pw))) => {
                sql.push_str(&super::ops::alter_role_password_sql(&role, &pw));
                sql.push('\n');
                done.push(role);
            }
            _ => skipped.push(role),
        }
    }
    Ok((sql, done, skipped))
}

fn open_sql(db: &str, open: bool) -> String {
    if open {
        format!("ALTER DATABASE \"{db}\" ALLOW_CONNECTIONS true;\nALTER DATABASE \"{db}\" CONNECTION LIMIT -1;\nSELECT 0;\n")
    } else {
        format!(
            "ALTER DATABASE \"{db}\" ALLOW_CONNECTIONS false;\n\
             SELECT count(pg_catalog.pg_terminate_backend(pid)) FROM pg_catalog.pg_stat_activity WHERE datname = '{db}' AND pid <> pg_catalog.pg_backend_pid();\n"
        )
    }
}

fn switching() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Re-review N4: one switch of a database at a time (held for the switch).
pub(crate) struct DbBusy(String);
impl DbBusy {
    pub(crate) fn take(db: &str) -> Result<Self, EnsureError> {
        let mut s = switching().lock().unwrap_or_else(|e| e.into_inner());
        if !s.insert(db.to_string()) {
            return Err(EnsureError::Refused(format!("busy: a switch of {db} is already running")));
        }
        Ok(Self(db.to_string()))
    }
}
impl Drop for DbBusy {
    fn drop(&mut self) {
        switching().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// `k2 db cluster switch <db> --to <cluster> [--yes]`. Without `confirm`
/// it changes nothing and answers the moment and what would be lost.
pub fn switch(ops: &dyn SystemOps, secrets: &dyn SecretStore, db: &str, to_spec: &str, confirm: bool, by: &str) -> Result<Value, EnsureError> {
    if !db_name_ok(db) {
        return Err(EnsureError::Usage(format!("{db:?} is not a K2 database name")));
    }
    let (id, project) = active_row(db)?;
    let from = cluster::for_db(db).map_err(EnsureError::NotFound)?;
    let to = cluster::named_target(to_spec, None)?;
    if to.major != from.major || to.name == from.name {
        return Err(EnsureError::Usage(format!(
            "{db} is on {}; --to names {}: a switch goes to another cluster of the same major",
            from.label(),
            to.label()
        )));
    }
    if to.owner_project.as_deref().is_some_and(|o| o != project) {
        return Err(EnsureError::NotYours(format!("{} is another workspace's cluster", to.label())));
    }
    let dir = match (restored_of(&to), last_switch(db)) {
        (Some((src, moment)), _) if src == from.name => Direction::Restore { moment },
        (_, Some((f, t, at))) if f == to.name && t == from.name => Direction::Rollback { since: at },
        _ => {
            return Err(EnsureError::Refused(format!(
                concat!("{} is neither a restore of {} (`", k2_core::cli_name!(), " db cluster restore {} --to <time> --as <new>`) nor the cluster {} was switched away from; nothing changed"),
                to.label(),
                from.label(),
                from.name,
                db
            )))
        }
    };
    let owner = psql(ops, &to, "postgres", &format!("SELECT pg_catalog.pg_get_userbyid(datdba) FROM pg_catalog.pg_database WHERE datname = '{db}';\n"))?;
    if owner.trim() != format!("{db}_migrator") {
        return Err(EnsureError::Refused(format!(
            "{} has no database {db} owned by {db}_migrator (it answered {:?}); nothing changed",
            to.label(),
            owner.trim()
        )));
    }
    let apps = crate::apps::db_connect::connections_on_db(db);
    let (moment, lost) = match &dir {
        Direction::Restore { moment } => (
            *moment,
            format!(
                "{db} goes back to {} UTC: every write to it on {} after that moment is left behind there (the cluster is kept, closed, for a rollback); logins and passwords go back to that moment too, so K2 sets each App's again",
                token(*moment),
                from.label()
            ),
        ),
        Direction::Rollback { since } => (
            *since,
            format!(
                "{db} goes back to {} as it was when it was switched away at {} UTC: every write to it on {} since then is left behind there (kept, closed)",
                to.label(),
                token(*since),
                from.label()
            ),
        ),
    };
    let kind = if matches!(dir, Direction::Restore { .. }) { "restore" } else { "rollback" };
    let preview = json!({
        "db": db, "from": from.label(), "to": to.label(), "kind": kind, "moment": token(moment), "lost": lost,
        "apps": apps.iter().map(|(a, n)| json!({"app": a, "as": n})).collect::<Vec<_>>(),
    });
    if !confirm {
        let mut v = preview;
        v["ok"] = json!(false);
        v["needsConfirm"] = json!(true);
        return Ok(v);
    }
    let _busy = DbBusy::take(db)?;
    // 3. The old copy closes; the target's opens.
    psql(ops, &from, "postgres", &open_sql(db, false))?;
    if let Err(e) = psql(ops, &to, "postgres", &open_sql(db, true)) {
        let _ = psql(ops, &from, "postgres", &open_sql(db, true));
        return Err(e);
    }
    // 4b. K2's own logins on the target, with their passwords now.
    let (rekey, rekeyed, not_rekeyed) = match rekey_sql(secrets, db, &id) {
        Ok(x) => x,
        Err(e) => {
            let _ = psql(ops, &to, "postgres", &open_sql(db, false));
            let _ = psql(ops, &from, "postgres", &open_sql(db, true));
            return Err(e);
        }
    };
    if !rekey.is_empty() {
        if let Err(e) = psql(ops, &to, "postgres", &rekey) {
            let _ = psql(ops, &to, "postgres", &open_sql(db, false));
            let _ = psql(ops, &from, "postgres", &open_sql(db, true));
            return Err(EnsureError::Helper(format!("setting {db}'s logins on {}: {}", to.label(), e.hint().replace(&rekey, "[sql]"))));
        }
    }
    // 4. The row, in one transaction.
    if let Err(e) = repoint(&id, db, &from, &to, &dir, by) {
        let _ = psql(ops, &to, "postgres", &open_sql(db, false));
        let _ = psql(ops, &from, "postgres", &open_sql(db, true));
        return Err(e);
    }
    // 5. Each App's side on the target.
    let mut app_results = Vec::new();
    let mut apps_ok = true;
    for (app, as_name) in &apps {
        match crate::apps::db_connect::reconnect_for_switch(by, app, as_name) {
            Ok(v) => app_results.push(json!({"app": app, "as": as_name, "ok": true, "check": v.get("check"), "rehost": v.get("rehost")})),
            Err(e) => {
                apps_ok = false;
                app_results.push(json!({"app": app, "as": as_name, "ok": false, "error": e.code, "hint": e.hint}));
            }
        }
    }
    // 6. A restored target archives and gets a base backup.
    let mut archive = Value::Null;
    if kind == "restore" && !to.wal_archive {
        archive = match cluster::archive_named(ops, &to, true) {
            Ok(_) => match super::pitr::basebackup(ops, &to) {
                Ok(b) => json!({"walArchive": true, "baseBackup": b["backup"]["stamp"]}),
                Err(e) => json!({"walArchive": true, "baseBackupError": e.hint()}),
            },
            Err(e) => json!({"walArchive": false, "error": e.hint()}),
        };
    }
    eprintln!("[db] switch {db} {} -> {} ({kind}) by {by}", from.label(), to.label());
    let mut v = preview;
    v["ok"] = json!(apps_ok);
    v["switched"] = json!(true);
    v["apps"] = json!(app_results);
    v["archive"] = archive;
    v["loginsSet"] = json!(rekeyed);
    if !not_rekeyed.is_empty() {
        v["loginsNotSet"] = json!(not_rekeyed);
    }
    v["rollback"] = json!(format!(
        concat!(k2_core::cli_name!(), " db cluster switch {} --to {} (kept, closed, until then)"),
        db, from.name
    ));
    Ok(v)
}

#[cfg(test)]
#[path = "switch_tests.rs"]
mod tests;
