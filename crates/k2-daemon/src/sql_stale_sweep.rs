//! Rights an older K2 left on a database's `public` schema for workspaces it
//! no longer grants, cleared by K2 itself (0.46.17).
//!
//! Up to 0.41.5 `k2 db grant` granted ALL TABLES and ALL SEQUENCES IN SCHEMA
//! public to the grantee and set default privileges for the migrator in
//! `public`; that era's revoke never took the default privileges back, and
//! from 0.41.6 the revoke skipped `public` (Postgres 15+ owns it as
//! `pg_database_owner`). So a workspace granted then and revoked later may
//! still read `public`'s tables, and every table made since.
//!
//! **Only roles K2 made.** A workspace's role is `ws_<workspace>_agent`
//! (`sql::ops::default_agent_role`), the only kind `k2 db grant` ever named.
//! The sweep looks only at roles of that shape that hold a right in
//! `public` (a table or sequence ACL, a default privilege, the schema's
//! own ACL) and that are neither this database's own agent role nor one of
//! its recorded grantees (`sql_grants`). A role the owner made by hand keeps
//! whatever they gave it. Only what the migrator (the owner of every table
//! migrations make) granted is revoked: its tables' and sequences' rights,
//! its default privileges in `public`, and an explicit right on the schema.
//!
//! **When:** at the end of every privilege sync (migrate, restore, a move,
//! grant), after `k2 db revoke`, and once per database at the first daemon
//! start with this version ([`boot_once`], recorded in
//! `~/.k2/db/stale-grant-sweep-v1.json`). It runs as the migrator, never a
//! superuser, and only takes rights away. Each cleared role is logged by a
//! short hash, never its name.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::sql::ops::{self, OpsError};
use crate::sql::sysops::SystemOps;

/// The marker the sweep's last statement prints.
const MARK: &str = "k2swept:";

/// The `LIKE` pattern of the roles K2 makes for a workspace.
const K2_ROLE_LIKE: &str = "ws\\_%\\_agent";

/// The sweep, run as the migrator: a temp table of the roles it cleared, the
/// `DO` block, and a last `SELECT` that prints them after [`MARK`].
/// `keep`: the recorded grantees' roles (the database's own agent role is
/// always kept).
pub(crate) fn sweep_sql(db_name: &str, keep: &[String]) -> String {
    let mut kept: BTreeSet<String> = keep.iter().cloned().collect();
    kept.insert(format!("{db_name}_agent"));
    kept.insert(format!("{db_name}_migrator"));
    let keep_lits: Vec<String> = kept.iter().map(|r| ops::pg_quote_literal(r)).collect();
    let like = ops::pg_quote_literal(K2_ROLE_LIKE);
    format!(
        "CREATE TEMP TABLE IF NOT EXISTS k2_swept (rolname text);\n\
         TRUNCATE pg_temp.k2_swept;\n\
         DO $k2sweep$\n\
         DECLARE\n\
           keep text[] := ARRAY[{keep}]::text[];\n\
           r record;\n\
           t record;\n\
         BEGIN\n\
           FOR r IN\n\
             SELECT DISTINCT pg_catalog.pg_get_userbyid(g.grantee) AS rolname, g.grantee AS roid FROM (\n\
               SELECT (pg_catalog.aclexplode(c.relacl)).grantee FROM pg_catalog.pg_class c WHERE c.relnamespace = 'public'::pg_catalog.regnamespace\n\
               UNION SELECT (pg_catalog.aclexplode(d.defaclacl)).grantee FROM pg_catalog.pg_default_acl d WHERE d.defaclnamespace = 'public'::pg_catalog.regnamespace\n\
               UNION SELECT (pg_catalog.aclexplode(n.nspacl)).grantee FROM pg_catalog.pg_namespace n WHERE n.nspname = 'public'\n\
             ) g\n\
             WHERE g.grantee <> 0\n\
               AND pg_catalog.pg_get_userbyid(g.grantee) LIKE {like}\n\
               AND NOT (pg_catalog.pg_get_userbyid(g.grantee) = ANY (keep))\n\
           LOOP\n\
             FOR t IN\n\
               SELECT DISTINCT c.oid::pg_catalog.regclass::text AS rel, c.relkind\n\
               FROM pg_catalog.pg_class c CROSS JOIN LATERAL pg_catalog.aclexplode(c.relacl) x\n\
               WHERE c.relnamespace = 'public'::pg_catalog.regnamespace\n\
                 AND pg_catalog.pg_get_userbyid(c.relowner) = current_user AND x.grantee = r.roid\n\
             LOOP\n\
               IF t.relkind = 'S' THEN\n\
                 EXECUTE pg_catalog.format('REVOKE ALL ON SEQUENCE %s FROM %I', t.rel, r.rolname);\n\
               ELSE\n\
                 EXECUTE pg_catalog.format('REVOKE ALL ON TABLE %s FROM %I', t.rel, r.rolname);\n\
               END IF;\n\
             END LOOP;\n\
             IF EXISTS (\n\
               SELECT 1 FROM pg_catalog.pg_default_acl d CROSS JOIN LATERAL pg_catalog.aclexplode(d.defaclacl) x\n\
               WHERE d.defaclnamespace = 'public'::pg_catalog.regnamespace\n\
                 AND pg_catalog.pg_get_userbyid(d.defaclrole) = current_user AND x.grantee = r.roid\n\
             ) THEN\n\
               EXECUTE pg_catalog.format('ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public REVOKE ALL ON TABLES FROM %I', current_user, r.rolname);\n\
               EXECUTE pg_catalog.format('ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public REVOKE ALL ON SEQUENCES FROM %I', current_user, r.rolname);\n\
             END IF;\n\
             IF EXISTS (\n\
               SELECT 1 FROM pg_catalog.pg_namespace n CROSS JOIN LATERAL pg_catalog.aclexplode(n.nspacl) x\n\
               WHERE n.nspname = 'public' AND x.grantee = r.roid\n\
             ) THEN\n\
               EXECUTE pg_catalog.format('REVOKE ALL ON SCHEMA public FROM %I', r.rolname);\n\
             END IF;\n\
             INSERT INTO pg_temp.k2_swept (rolname) VALUES (r.rolname);\n\
           END LOOP;\n\
         END\n\
         $k2sweep$;\n\
         SELECT '{MARK}' || coalesce(pg_catalog.string_agg(rolname, ',' ORDER BY rolname), '') FROM pg_temp.k2_swept;",
        keep = keep_lits.join(", "),
    )
}

/// The roles a sweep's output names (after [`MARK`]).
pub(crate) fn swept_of(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.trim().strip_prefix(MARK))
        .flat_map(|rest| rest.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// A role's short hash for the log (never its name).
pub(crate) fn role_hash(role: &str) -> String {
    let d = Sha256::digest(role.as_bytes());
    d.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// The one log line for a database.
pub(crate) fn log_line(db_name: &str, swept: &[String]) -> String {
    let hashes: Vec<String> = swept.iter().map(|r| role_hash(r)).collect();
    format!(
        "{db_name}: cleared rights an older version left on public from {} role(s) no longer granted{}",
        swept.len(),
        if hashes.is_empty() { String::new() } else { format!(" ({})", hashes.join(", ")) }
    )
}

/// The privilege sync's output (`sql::ops::apply_privilege_sync`): one log
/// line when it cleared any role.
pub(crate) fn log_swept(db_name: &str, out: &str) {
    let swept = swept_of(out);
    if !swept.is_empty() {
        eprintln!("[db] {}", log_line(db_name, &swept));
    }
}

/// The sweep alone (no grant is applied or widened), as the migrator, on
/// one active database, with K2's own tables reset after it. The roles it
/// cleared.
pub(crate) fn sweep_db(sys: &dyn SystemOps, db_name: &str) -> Result<Vec<String>, OpsError> {
    let row = ops::active_row_by_name(db_name)?;
    let keep: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ops::sync_grants_for(&conn, &row.id)?.into_iter().map(|g| g.role).collect()
    };
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let (user, pw) = ops::migrator_creds(&secrets, &row)?;
    let sql = format!("{}\n{}", crate::sql_k2_tables::fence_sql(&format!("{db_name}_agent")), sweep_sql(db_name, &keep));
    let out = ops::exec_as(sys, db_name, &user, &pw, &sql)?;
    Ok(swept_of(&out))
}

fn marker_path() -> std::path::PathBuf {
    k2_core::paths::k2_home().join("db").join("stale-grant-sweep-v1.json")
}

fn done_ids() -> BTreeSet<String> {
    std::fs::read(marker_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("done").and_then(|d| d.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

fn record_done(done: &BTreeSet<String>) -> Result<(), String> {
    let p = marker_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let body = serde_json::json!({ "done": done.iter().collect::<Vec<_>>() }).to_string();
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Once per database, at the first start with this version: [`sweep_db`] on
/// every active database not swept before, each recorded by its id as it
/// succeeds (a failure is tried again at the next start). One line per
/// database it ran on.
pub(crate) fn boot_once(sys: &dyn SystemOps) -> Vec<String> {
    let dbs: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.prepare("SELECT id, name FROM sql_databases WHERE status = 'active' ORDER BY created_at")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>())
            .unwrap_or_default()
    };
    let mut done = done_ids();
    let mut lines = Vec::new();
    for (id, name) in dbs {
        if done.contains(&id) || crate::sql::pgmove::k2::moving(&name) || crate::sql::pgmove::dual_k2::running(&name) {
            continue;
        }
        match sweep_db(sys, &name) {
            Ok(swept) => {
                lines.push(log_line(&name, &swept));
                done.insert(id);
                if let Err(e) = record_done(&done) {
                    lines.push(format!("{name}: swept, but the record wasn't kept ({e}); it runs again next start"));
                }
            }
            Err(e) => lines.push(format!("{name}: older-version rights not checked yet ({}); next start tries again", e.hint())),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_names_only_k2s_own_roles_and_keeps_the_grantees_and_the_database_s_own() {
        let sql = sweep_sql("ws_a", &["ws_b_agent".to_string()]);
        assert!(sql.contains("LIKE 'ws\\_%\\_agent'"), "only K2's workspace roles: {sql}");
        assert!(sql.contains("ARRAY['ws_a_agent', 'ws_a_migrator', 'ws_b_agent']::text[]"), "{sql}");
        assert!(sql.contains("pg_catalog.pg_get_userbyid(c.relowner) = current_user"), "only what the migrator owns: {sql}");
        assert!(sql.contains("ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public REVOKE ALL ON TABLES FROM %I"), "{sql}");
        assert!(sql.trim_end().ends_with("FROM pg_temp.k2_swept;"), "the last statement prints the cleared roles: {sql}");
        assert!(!sql.to_ascii_uppercase().contains(" GRANT "), "the sweep only takes away: {sql}");
        assert!(sweep_sql("x'y", &["a'b".into()]).contains("'a''b'"), "names are quoted literals");
    }

    #[test]
    fn its_output_logs_hashes_never_names() {
        let swept = swept_of("DO\nk2swept:ws_old_agent,ws_older_agent\n");
        assert_eq!(swept, ["ws_old_agent", "ws_older_agent"]);
        assert!(swept_of("k2swept:\n").is_empty());
        let line = log_line("ws_a_db", &swept);
        assert!(line.starts_with("ws_a_db: cleared rights an older version left on public from 2 role(s)"), "{line}");
        assert!(!line.contains("ws_old_agent") && line.contains(&role_hash("ws_old_agent")), "{line}");
        assert_eq!(role_hash("ws_old_agent").len(), 8);
    }
}
