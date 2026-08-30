//! Workspace database + JSONB store operations (create/migrate/dump/store).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::paths::{resolve_in_path, resolve_out_path, InPathError};
use super::secrets::{generate_secret, SecretStore};
use super::supervisor::current_status;
use super::sysops::{SystemOps, PG_DUMP_PATH, PG_RESTORE_PATH, PSQL_PATH};

#[derive(Debug)]
pub enum OpsError {
    Usage(String),
    NotFound(String),
    CapReached(String),
    NotReady(String),
    ChecksumMismatch(String),
    #[allow(dead_code)]
    Forbidden(String),
    Engine(String),
}

impl OpsError {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Usage(_) => "400 Bad Request",
            Self::NotFound(_) => "404 Not Found",
            Self::CapReached(_) | Self::NotReady(_) | Self::ChecksumMismatch(_) => "409 Conflict",
            Self::Forbidden(_) => "403 Forbidden",
            Self::Engine(_) => "502 Bad Gateway",
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::NotFound(_) => "not_found",
            Self::CapReached(_) => "cap_reached",
            Self::NotReady(_) => "not_ready",
            Self::ChecksumMismatch(_) => "checksum_mismatch",
            Self::Forbidden(_) => "forbidden",
            Self::Engine(_) => "engine",
        }
    }
    pub fn hint(&self) -> &str {
        match self {
            Self::Usage(h)
            | Self::NotFound(h)
            | Self::CapReached(h)
            | Self::NotReady(h)
            | Self::ChecksumMismatch(h)
            | Self::Forbidden(h)
            | Self::Engine(h) => h,
        }
    }
}

/// SHA-256 hex of a migration file. Used to refuse silently-rewritten
/// already-applied versions (Julie Stage 2).
pub fn migration_checksum(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Persist `projects.db_agent_access` (`off`/`read`/`write`). Empty = no-op
/// (default stays fail-closed `off`). This flag only gates **creating**
/// new databases (`k2 db create`); list/dsn/store use ownership or grants.
pub fn persist_db_access(project_path: &str, access: Option<&str>) -> Result<(), OpsError> {
    let Some(access) = access.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    if !k2_core::workspace::settings::DB_AGENT_ACCESS_MODES.contains(&access) {
        return Err(OpsError::Usage(
            "access must be 'off', 'read', or 'write'".into(),
        ));
    }
    k2_core::workspace::settings::update_project_setting(project_path, "db_agent_access", access)
        .map_err(OpsError::Engine)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn require_running() -> Result<(), OpsError> {
    match current_status().as_deref() {
        Some("running") => Ok(()),
        Some(s) => Err(OpsError::NotReady(format!(
            "SQL sidecar is '{s}' — ask your human to run 'k2 db enable'"
        ))),
        None => Err(OpsError::NotReady(
            "SQL sidecar is not enabled — ask your human to run 'k2 db enable' (bake --with-db first)"
                .into(),
        )),
    }
}

/// Sanitize a project UUID into a Postgres identifier fragment.
pub fn pg_ident_for_project(project_id: &str) -> String {
    let sanitized: String = project_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("ws_{sanitized}")
}

fn pg_quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
fn pg_quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// CREATE ROLE SQL — no SUPERUSER / CREATEDB / CREATEROLE / BYPASSRLS.
pub fn create_role_sql(role: &str, password: &str) -> String {
    format!(
        "CREATE ROLE {role} LOGIN PASSWORD {pw} NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOINHERIT;",
        role = pg_quote_ident(role),
        pw = pg_quote_literal(password),
    )
}

/// Idempotent CREATE ROLE. A prior `k2 db grant` may have minted the
/// grantee's `ws_*_agent` role; a later `k2 db create` must not fail.
pub fn ensure_role_sql(role: &str, password: &str) -> String {
    let create = create_role_sql(role, password);
    // CREATE ROLE must keep its trailing semicolon so PL/pgSQL does not
    // parse EXCEPTION as a role option (AX41: "unrecognized role option").
    format!("DO $$\nBEGIN\n  {create}\nEXCEPTION WHEN duplicate_object THEN NULL;\nEND $$;")
}

/// Reset LOGIN + password on an existing role (grant-then-create). The
/// helper runs as postgres, so this never prints the password.
pub fn alter_role_password_sql(role: &str, password: &str) -> String {
    format!(
        "ALTER ROLE {role} WITH LOGIN PASSWORD {pw} NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOINHERIT;",
        role = pg_quote_ident(role),
        pw = pg_quote_literal(password),
    )
}

fn count_active(conn: &rusqlite::Connection, project_id: &str) -> u32 {
    conn.query_row(
        "SELECT COUNT(*) FROM sql_databases WHERE project_id = ?1 AND status = 'active'",
        rusqlite::params![project_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
    .max(0) as u32
}

struct DbRow {
    id: String,
    name: String,
    project_id: String,
    agent_secret_ref: Option<String>,
    migrator_secret_ref: Option<String>,
    status: String,
    bind_role: Option<String>,
}

const DB_ROW_COLS: &str =
    "id, name, project_id, agent_secret_ref, migrator_secret_ref, status, bind_role";

fn db_row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<DbRow> {
    Ok(DbRow {
        id: r.get(0)?,
        name: r.get(1)?,
        project_id: r.get(2)?,
        agent_secret_ref: r.get(3)?,
        migrator_secret_ref: r.get(4)?,
        status: r.get(5)?,
        bind_role: r.get(6)?,
    })
}

fn load_active_by_client(
    conn: &rusqlite::Connection,
    project_id: &str,
    client_id: &str,
) -> Option<DbRow> {
    conn.query_row(
        &format!(
            "SELECT {DB_ROW_COLS} FROM sql_databases WHERE project_id = ?1 AND client_id = ?2"
        ),
        rusqlite::params![project_id, client_id],
        db_row_from,
    )
    .ok()
}

fn load_active_by_name(conn: &rusqlite::Connection, project_id: &str, name: &str) -> Option<DbRow> {
    conn.query_row(
        &format!(
            "SELECT {DB_ROW_COLS} FROM sql_databases \
             WHERE project_id = ?1 AND name = ?2 AND status = 'active'"
        ),
        rusqlite::params![project_id, name],
        db_row_from,
    )
    .ok()
}

fn load_active_default(conn: &rusqlite::Connection, project_id: &str) -> Option<DbRow> {
    conn.query_row(
        &format!(
            "SELECT {DB_ROW_COLS} FROM sql_databases \
             WHERE project_id = ?1 AND status = 'active' ORDER BY created_at ASC LIMIT 1"
        ),
        rusqlite::params![project_id],
        db_row_from,
    )
    .ok()
}

/// First active database this workspace is granted (not owned).
fn load_granted_active(conn: &rusqlite::Connection, project_id: &str) -> Option<DbRow> {
    conn.query_row(
        "SELECT d.id, d.name, d.project_id, d.agent_secret_ref, d.migrator_secret_ref, \
                d.status, d.bind_role \
         FROM sql_databases d \
         JOIN sql_grants g ON g.database_id = d.id \
         WHERE g.project_id = ?1 AND d.status = 'active' \
         ORDER BY g.created_at ASC LIMIT 1",
        rusqlite::params![project_id],
        db_row_from,
    )
    .ok()
}

/// Effective SQL interaction level for a workspace: `write` if it owns an
/// active DB, else the highest `sql_grants.level` on an active DB, else None.
/// Does **not** consult `projects.db_agent_access` (that flag is create-only).
pub fn project_sql_level(project_id: &str) -> Option<&'static str> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if count_active(&conn, project_id) > 0 {
        return Some("write");
    }
    let mut stmt = conn
        .prepare(
            "SELECT g.level FROM sql_grants g \
             JOIN sql_databases d ON d.id = g.database_id \
             WHERE g.project_id = ?1 AND d.status = 'active'",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![project_id], |r| r.get::<_, String>(0))
        .ok()?;
    let mut saw_read = false;
    for level in rows.filter_map(Result::ok) {
        if level == "write" {
            return Some("write");
        }
        if level == "read" {
            saw_read = true;
        }
    }
    if saw_read {
        Some("read")
    } else {
        None
    }
}

fn project_db_agent_access(conn: &rusqlite::Connection, project_id: &str) -> String {
    conn.query_row(
        "SELECT db_agent_access FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .filter(|v| k2_core::workspace::settings::DB_AGENT_ACCESS_MODES.contains(&v.as_str()))
    .unwrap_or_else(|| "off".to_string())
}

fn dsn_for(name: &str, user: &str, password: &str) -> String {
    format!("postgres://{user}:{password}@127.0.0.1:5432/{name}")
}

fn json_create(name: &str, dsn: &str, existing: bool, used: u32, cap: u32) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "name": name,
        "dsn": dsn,
        "role": "agent",
        "existing": existing,
        "cap": { "used": used, "cap": cap },
    })
}

fn assert_no_superuser_json(v: &serde_json::Value) {
    let s = v.to_string().to_ascii_lowercase();
    debug_assert!(!s.contains("superuser"));
    debug_assert!(!s.contains("postgres://postgres"));
}

/// Mint (or idempotently return) the workspace DB.
pub fn create_database(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    cap: u32,
    client_id: Option<&str>,
    name_override: Option<&str>,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let default_name = pg_ident_for_project(project_id);
    let name = name_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .unwrap_or(default_name);
    if !name.starts_with("ws_") && name_override.is_none() {
        return Err(OpsError::Usage(
            "internal db name must start with ws_".into(),
        ));
    }
    let client_id = client_id.map(str::trim).filter(|s| !s.is_empty());

    let (used, existing) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(cid) = client_id {
            if let Some(row) = load_active_by_client(&conn, project_id, cid) {
                if row.status == "active" {
                    let used = count_active(&conn, project_id);
                    return dsn_json(secrets, &row, true, used, cap);
                }
            }
        }
        if let Some(row) = load_active_by_name(&conn, project_id, &name) {
            let used = count_active(&conn, project_id);
            return dsn_json(secrets, &row, true, used, cap);
        }
        let used = count_active(&conn, project_id);
        if cap != 0 && used >= cap {
            return Err(OpsError::CapReached(format!(
                "database cap reached ({used}/{cap}). Drop one with 'k2 db drop --yes' \
                 or ask your human to raise the cap."
            )));
        }
        (used, false)
    };
    let _ = existing;

    let migrator = format!("{name}_migrator");
    let agent = format!("{name}_agent");
    let migrator_pw = generate_secret().map_err(OpsError::Engine)?;
    let agent_pw = generate_secret().map_err(OpsError::Engine)?;

    let role_sql = format!(
        "{}\n{}\n{}\n{}",
        ensure_role_sql(&migrator, &migrator_pw),
        alter_role_password_sql(&migrator, &migrator_pw),
        ensure_role_sql(&agent, &agent_pw),
        alter_role_password_sql(&agent, &agent_pw),
    );
    let up = role_sql.to_ascii_uppercase();
    if up.contains("SUPERUSER") && !up.contains("NOSUPERUSER") {
        return Err(OpsError::Engine(
            "internal: superuser leaked into role SQL".into(),
        ));
    }
    if up.contains("CREATEDB") && !up.contains("NOCREATEDB") {
        return Err(OpsError::Engine(
            "internal: createdb leaked into role SQL".into(),
        ));
    }
    if up.contains("BYPASSRLS") && !up.contains("NOBYPASSRLS") {
        return Err(OpsError::Engine(
            "internal: bypassrls leaked into role SQL".into(),
        ));
    }
    if up.contains("FORCE ROW LEVEL") {
        return Err(OpsError::Engine("internal: FORCE RLS is not v1".into()));
    }

    ops.run_helper(
        &["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1"],
        Some(role_sql.as_bytes()),
    )
    .map_err(OpsError::Engine)?;

    let create_db = format!(
        "CREATE DATABASE {db} OWNER {owner};",
        db = pg_quote_ident(&name),
        owner = pg_quote_ident(&migrator),
    );
    ops.run_helper(
        &["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1"],
        Some(create_db.as_bytes()),
    )
    .map_err(OpsError::Engine)?;

    let grants = format!(
        "GRANT CONNECT ON DATABASE {db} TO {agent};\n\
         GRANT USAGE ON SCHEMA public TO {agent};\n\
         GRANT CREATE ON SCHEMA public TO {migrator};\n\
         ALTER DEFAULT PRIVILEGES FOR ROLE {migrator} IN SCHEMA public \
           GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO {agent};\n\
         CREATE TABLE IF NOT EXISTS _k2_migrations (\n\
           version TEXT PRIMARY KEY,\n\
           checksum TEXT NOT NULL DEFAULT '',\n\
           applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\n\
         );\n\
         CREATE TABLE IF NOT EXISTS _k2_store (\n\
           collection TEXT NOT NULL,\n\
           id TEXT NOT NULL,\n\
           doc JSONB NOT NULL,\n\
           updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
           PRIMARY KEY (collection, id)\n\
         );\n\
         ALTER TABLE _k2_migrations OWNER TO {migrator};\n\
         ALTER TABLE _k2_store OWNER TO {migrator};\n\
         GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE _k2_migrations, _k2_store TO {agent};\n",
        db = pg_quote_ident(&name),
        agent = pg_quote_ident(&agent),
        migrator = pg_quote_ident(&migrator),
    );
    ops.run_helper(
        &["psql", "-d", &name, "-v", "ON_ERROR_STOP=1"],
        Some(grants.as_bytes()),
    )
    .map_err(OpsError::Engine)?;

    let agent_ref = secrets
        .store("agent", &agent_pw)
        .map_err(OpsError::Engine)?;
    let migrator_ref = secrets
        .store("migrator", &migrator_pw)
        .map_err(OpsError::Engine)?;
    let id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO sql_databases (id, project_id, name, client_id, status, \
             agent_secret_ref, migrator_secret_ref, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7)",
            rusqlite::params![
                id,
                project_id,
                name,
                client_id,
                agent_ref,
                migrator_ref,
                now_secs()
            ],
        )
        .map_err(|e| OpsError::Engine(format!("catalog insert: {e}")))?;
    }
    let used = used + 1;
    let dsn = dsn_for(&name, &agent, &agent_pw);
    let v = json_create(&name, &dsn, false, used, cap);
    assert_no_superuser_json(&v);
    Ok(v)
}

fn dsn_json(
    secrets: &dyn SecretStore,
    row: &DbRow,
    existing: bool,
    used: u32,
    cap: u32,
) -> Result<serde_json::Value, OpsError> {
    let sref = row
        .agent_secret_ref
        .as_deref()
        .ok_or_else(|| OpsError::Engine("agent secret ref missing".into()))?;
    let pw = secrets
        .resolve(sref)
        .map_err(OpsError::Engine)?
        .ok_or_else(|| OpsError::Engine("agent secret missing from vault".into()))?;
    let agent = format!("{}_agent", row.name);
    let dsn = dsn_for(&row.name, &agent, &pw);
    let v = json_create(&row.name, &dsn, existing, used, cap);
    assert_no_superuser_json(&v);
    Ok(v)
}

pub fn list_databases(project_id: &str) -> serde_json::Value {
    catalog_json(Some(project_id))
}

pub fn dsn_for_project(
    secrets: &dyn SecretStore,
    project_id: &str,
    cap: u32,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let row = active_row(project_id)?;
    let used = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        count_active(&conn, project_id)
    };
    dsn_json(secrets, &row, true, used, cap)
}

fn active_row(project_id: &str) -> Result<DbRow, OpsError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(row) = load_active_default(&conn, project_id) {
        return Ok(row);
    }
    if let Some(row) = load_granted_active(&conn, project_id) {
        return Ok(row);
    }
    Err(OpsError::NotFound(
        "no database for this workspace — run 'k2 db create' first".into(),
    ))
}

fn migrator_creds(secrets: &dyn SecretStore, row: &DbRow) -> Result<(String, String), OpsError> {
    let sref = row
        .migrator_secret_ref
        .as_deref()
        .ok_or_else(|| OpsError::Engine("migrator secret ref missing".into()))?;
    let pw = secrets
        .resolve(sref)
        .map_err(OpsError::Engine)?
        .ok_or_else(|| OpsError::Engine("migrator secret missing from vault".into()))?;
    Ok((format!("{}_migrator", row.name), pw))
}

fn exec_as(
    ops: &dyn SystemOps,
    db: &str,
    user: &str,
    password: &str,
    sql: &str,
) -> Result<String, OpsError> {
    // `-tA` without `-F` uses `|` as the unaligned field separator.
    // Force tab so SELECT version, checksum is unambiguous; the parser
    // still accepts `|` for older fakes / forgotten `-F`.
    let out = ops
        .run_cmd(
            PSQL_PATH,
            &[
                "-h",
                "127.0.0.1",
                "-U",
                user,
                "-d",
                db,
                "-v",
                "ON_ERROR_STOP=1",
                "-tA",
                "-F",
                "\t",
            ],
            &[("PGPASSWORD", password)],
            Some(sql.as_bytes()),
        )
        .map_err(OpsError::Engine)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Split a `psql -tA` row. Tabs first (exec_as `-F`); `|` is the unaligned default.
fn split_psql_fields(line: &str) -> Vec<&str> {
    if line.contains('\t') {
        line.split('\t').map(str::trim).collect()
    } else {
        line.split('|').map(str::trim).collect()
    }
}

/// Repair agent DML on catalog tables as helper superuser. Migrator GRANT
/// is a no-op when OWNER TO did not stick (tables created by helper).
fn grant_agent_k2_tables(ops: &dyn SystemOps, db: &str) -> Result<(), OpsError> {
    let agent = pg_quote_ident(&agent_role_for_db(db));
    let sql = format!(
        "GRANT USAGE ON SCHEMA public TO {agent};\n\
         GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE _k2_migrations, _k2_store TO {agent};"
    );
    ops.run_helper(
        &["psql", "-d", db, "-v", "ON_ERROR_STOP=1"],
        Some(sql.as_bytes()),
    )
    .map_err(OpsError::Engine)?;
    Ok(())
}

fn ensure_migrations_table(
    ops: &dyn SystemOps,
    db: &str,
    user: &str,
    password: &str,
) -> Result<(), OpsError> {
    exec_as(
        ops,
        db,
        user,
        password,
        "CREATE TABLE IF NOT EXISTS _k2_migrations (\n\
           version TEXT PRIMARY KEY,\n\
           checksum TEXT NOT NULL DEFAULT '',\n\
           applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\n\
         );\n\
         DO $$\nBEGIN\n\
           ALTER TABLE _k2_migrations ADD COLUMN checksum TEXT;\n\
         EXCEPTION WHEN duplicate_column THEN NULL;\n\
         END $$;",
    )?;
    // Helper GRANT even when CREATE is IF NOT EXISTS. Before migrate SQL so
    // a later checksum/apply failure still leaves the agent DSN usable.
    grant_agent_k2_tables(ops, db)?;
    Ok(())
}

fn load_applied_checksums(
    ops: &dyn SystemOps,
    db: &str,
    user: &str,
    password: &str,
) -> Result<HashMap<String, String>, OpsError> {
    let applied_raw = exec_as(
        ops,
        db,
        user,
        password,
        "SELECT version, checksum FROM _k2_migrations;",
    )?;
    let mut applied = HashMap::new();
    for line in applied_raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts = split_psql_fields(line);
        let ver = parts.first().copied().unwrap_or("");
        let sum = parts.get(1).copied().unwrap_or("");
        if !ver.is_empty() {
            applied.insert(ver.to_string(), sum.to_string());
        }
    }
    Ok(applied)
}

/// `0001_init.sql` — four digits, underscore, rest, `.sql`. Never `init.sql`.
fn is_versioned_sql_filename(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".sql") else {
        return false;
    };
    let b = stem.as_bytes();
    b.len() >= 5 && b[..4].iter().all(|c| c.is_ascii_digit()) && b[4] == b'_'
}

pub fn migrate(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    ws_path: &str,
    dir: Option<&str>,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let row = active_row(project_id)?;
    let (user, pw) = migrator_creds(secrets, &row)?;
    let rel = dir.unwrap_or(".k2/db/migrations");
    // Absolute `dir` replaces `ws_path` (std::path::Path::join).
    let mig_dir = Path::new(ws_path).join(rel);
    if mig_dir.exists() && !mig_dir.is_dir() {
        return Err(OpsError::Usage(format!(
            "migrations path is not a directory: {}",
            mig_dir.display()
        )));
    }
    if !mig_dir.is_dir() {
        return Err(OpsError::NotFound(format!(
            "migrations directory not found: {}",
            mig_dir.display()
        )));
    }
    let mut matching: Vec<PathBuf> = Vec::new();
    let mut non_matching_sql: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&mig_dir)
        .map_err(|e| OpsError::Engine(format!("read migrations {}: {e}", mig_dir.display())))?
    {
        let entry = entry
            .map_err(|e| OpsError::Engine(format!("read migrations {}: {e}", mig_dir.display())))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return Err(OpsError::Usage(format!(
                "non-utf8 filename in {}",
                mig_dir.display()
            )));
        };
        if path.extension().and_then(|x| x.to_str()) != Some("sql") {
            continue;
        }
        if is_versioned_sql_filename(name) {
            matching.push(path);
        } else {
            non_matching_sql.push(name.to_string());
        }
    }
    matching.sort();
    if matching.is_empty() {
        non_matching_sql.sort();
        let found = if non_matching_sql.is_empty() {
            "none".to_string()
        } else {
            non_matching_sql.join(", ")
        };
        return Err(OpsError::Usage(format!(
            "no NNNN_name.sql migrations in {} (expected 0001_init.sql); found: {found}",
            mig_dir.display()
        )));
    }
    let discovered = matching.len();

    ensure_migrations_table(ops, &row.name, &user, &pw)?;
    let applied = load_applied_checksums(ops, &row.name, &user, &pw)?;

    let mut ran = Vec::new();
    for path in matching {
        let version = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let sql = std::fs::read_to_string(&path)
            .map_err(|e| OpsError::Engine(format!("read {}: {e}", path.display())))?;
        let checksum = migration_checksum(sql.as_bytes());
        if let Some(prev) = applied.get(&version) {
            if prev == &checksum {
                continue;
            }
            return Err(OpsError::ChecksumMismatch(format!(
                "migration {version} already applied with checksum {prev}, file is {checksum} — refuse"
            )));
        }
        if sql.to_ascii_uppercase().contains("FORCE ROW LEVEL") {
            return Err(OpsError::Usage(
                "FORCE RLS is not enabled in v1 — remove FORCE ROW LEVEL SECURITY from migrations"
                    .into(),
            ));
        }
        exec_as(ops, &row.name, &user, &pw, &sql)?;
        let insert = format!(
            "INSERT INTO _k2_migrations (version, checksum) VALUES ({ver}, {sum});",
            ver = pg_quote_literal(&version),
            sum = pg_quote_literal(&checksum),
        );
        exec_as(ops, &row.name, &user, &pw, &insert)?;
        ran.push(version);
    }
    Ok(serde_json::json!({
        "ok": true,
        "applied": ran,
        "discovered": discovered,
        "noop": ran.is_empty(),
    }))
}

/// Workspace DB status for org-box verify (Julie Stage 2 A): applied
/// migrations + size. Fails loud when this workspace has no database.
pub fn database_status(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let row = active_row(project_id)?;
    let (user, pw) = migrator_creds(secrets, &row)?;
    ensure_migrations_table(ops, &row.name, &user, &pw)?;
    let applied_raw = exec_as(
        ops,
        &row.name,
        &user,
        &pw,
        "SELECT version, checksum, applied_at FROM _k2_migrations ORDER BY version;",
    )?;
    let mut migrations = Vec::new();
    for line in applied_raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts = split_psql_fields(line);
        let version = parts.first().copied().unwrap_or("");
        if version.is_empty() {
            continue;
        }
        let checksum = parts.get(1).copied().unwrap_or("");
        let applied_at = parts.get(2).copied().unwrap_or("");
        migrations.push(serde_json::json!({
            "version": version,
            "checksum": checksum,
            "appliedAt": applied_at,
        }));
    }
    let size_raw = exec_as(
        ops,
        &row.name,
        &user,
        &pw,
        "SELECT pg_database_size(current_database());",
    )?;
    let size_bytes: i64 = size_raw.parse().map_err(|e| {
        OpsError::Engine(format!(
            "pg_database_size returned {size_raw:?} (not an integer): {e}"
        ))
    })?;
    Ok(serde_json::json!({
        "ok": true,
        "name": row.name,
        "migrations": migrations,
        "sizeBytes": size_bytes,
    }))
}

pub fn dump(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    ws_path: &str,
    out: Option<&str>,
) -> Result<(serde_json::Value, Option<Vec<u8>>), OpsError> {
    require_running()?;
    let row = active_row(project_id)?;
    let (user, pw) = migrator_creds(secrets, &row)?;
    let rel = match out.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            let ts = now_secs();
            format!(".k2/db/dumps/{ts}.dump")
        }
    };
    let dest = resolve_out_path(ws_path, &rel).map_err(OpsError::Usage)?;
    let dest_s = dest.to_string_lossy().into_owned();
    ops.run_cmd(
        PG_DUMP_PATH,
        &[
            "-Fc",
            "-h",
            "127.0.0.1",
            "-U",
            &user,
            "-d",
            &row.name,
            "-f",
            &dest_s,
        ],
        &[("PGPASSWORD", pw.as_str())],
        None,
    )
    .map_err(OpsError::Engine)?;
    let bytes = ops.read_file(&dest_s).ok();
    Ok((
        serde_json::json!({
            "ok": true,
            "path": rel,
            "format": "custom",
        }),
        bytes,
    ))
}

pub fn restore(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    ws_path: &str,
    file: &str,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    // Jail first (D17) so `..` / abs fail loud even on a fresh workspace.
    let src = match resolve_in_path(ws_path, file) {
        Ok(p) => p,
        Err(InPathError::Usage(h)) => return Err(OpsError::Usage(h)),
        Err(InPathError::NotFound(h)) => return Err(OpsError::NotFound(h)),
    };
    let row = match active_row(project_id) {
        Ok(r) => r,
        Err(OpsError::NotFound(_)) => {
            let cap = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                project_cap(&conn, project_id)
            };
            create_database(ops, secrets, project_id, cap, None, None)?;
            active_row(project_id)?
        }
        Err(e) => return Err(e),
    };
    let (user, pw) = migrator_creds(secrets, &row)?;
    ops.run_cmd(
        PG_RESTORE_PATH,
        &[
            "-h",
            "127.0.0.1",
            "-U",
            &user,
            "-d",
            &row.name,
            "--clean",
            "--if-exists",
            &src.to_string_lossy(),
        ],
        &[("PGPASSWORD", pw.as_str())],
        None,
    )
    .map_err(OpsError::Engine)?;
    Ok(serde_json::json!({ "ok": true, "restored": file }))
}

pub fn drop_database(ops: &dyn SystemOps, project_id: &str) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let row = active_row(project_id)?;
    let sql = format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = {name};\n\
         DROP DATABASE IF EXISTS {ident};",
        name = pg_quote_literal(&row.name),
        ident = pg_quote_ident(&row.name),
    );
    ops.run_helper(
        &["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1"],
        Some(sql.as_bytes()),
    )
    .map_err(OpsError::Engine)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "DELETE FROM sql_grants WHERE database_id = ?1",
            rusqlite::params![row.id],
        )
        .map_err(|e| OpsError::Engine(format!("catalog grant drop: {e}")))?;
        conn.execute(
            "UPDATE sql_databases SET status = 'dropped', dropped_at = ?1 WHERE id = ?2",
            rusqlite::params![now_secs(), row.id],
        )
        .map_err(|e| OpsError::Engine(format!("catalog drop: {e}")))?;
    }
    Ok(serde_json::json!({ "ok": true, "dropped": row.name }))
}

/// Default PG role for a workspace (`ws_<id>_agent`). D22 bind overrides
/// the *catalog* name shown to owners; DSN still uses this role's vault
/// secret (bind does not mint or print a password).
pub fn default_agent_role(project_id: &str) -> String {
    format!("{}_agent", pg_ident_for_project(project_id))
}

fn project_name(conn: &rusqlite::Connection, project_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT name FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

fn project_cap(conn: &rusqlite::Connection, project_id: &str) -> u32 {
    let cap: Option<i64> = conn
        .query_row(
            "SELECT db_active_cap FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    match cap {
        Some(n) if n >= 0 => n as u32,
        _ => 1,
    }
}

/// Locate an active database by id or name. When `prefer_project` is set,
/// a name match prefers that workspace's row.
fn find_database(spec: &str, prefer_project: Option<&str>) -> Result<DbRow, OpsError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(OpsError::Usage(
            "missing 'db' — database id or name (k2 db list)".into(),
        ));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(row) = conn.query_row(
        &format!("SELECT {DB_ROW_COLS} FROM sql_databases WHERE id = ?1 AND status = 'active'"),
        rusqlite::params![spec],
        db_row_from,
    ) {
        return Ok(row);
    }
    if let Some(pid) = prefer_project {
        if let Some(row) = load_active_by_name(&conn, pid, spec) {
            return Ok(row);
        }
    }
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {DB_ROW_COLS} FROM sql_databases WHERE name = ?1 AND status = 'active'"
        ))
        .map_err(|e| OpsError::Engine(format!("prepare: {e}")))?;
    let rows: Vec<DbRow> = stmt
        .query_map(rusqlite::params![spec], db_row_from)
        .map_err(|e| OpsError::Engine(format!("query: {e}")))?
        .filter_map(Result::ok)
        .collect();
    match rows.len() {
        0 => Err(OpsError::NotFound(format!(
            "database not found: {spec} — see 'k2 db list'"
        ))),
        1 => Ok(rows.into_iter().next().unwrap()),
        _ => Err(OpsError::Usage(format!(
            "name '{spec}' matches more than one database — pass the id from 'k2 db list'"
        ))),
    }
}

fn grant_can_manage(conn: &rusqlite::Connection, database_id: &str, project_id: &str) -> bool {
    conn.query_row(
        "SELECT can_manage FROM sql_grants WHERE database_id = ?1 AND project_id = ?2",
        rusqlite::params![database_id, project_id],
        |r| r.get::<_, i64>(0),
    )
    .ok()
    .map(|n| n != 0)
    .unwrap_or(false)
}

/// Owner token (no principal) always; owning workspace; or a grant with
/// `can_manage`.
fn caller_can_manage(caller_project: Option<&str>, row: &DbRow) -> bool {
    match caller_project {
        None => true,
        Some(p) if p == row.project_id => true,
        Some(p) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            grant_can_manage(&conn, &row.id, p)
        }
    }
}

fn validate_level(level: &str) -> Result<&str, OpsError> {
    match level.trim() {
        "read" | "write" => Ok(level.trim()),
        other => Err(OpsError::Usage(format!(
            "level must be read or write, got {other:?}"
        ))),
    }
}

fn apply_pg_grant(
    ops: &dyn SystemOps,
    db_name: &str,
    role: &str,
    level: &str,
) -> Result<(), OpsError> {
    let pw = generate_secret().map_err(OpsError::Engine)?;
    let ensure = ensure_role_sql(role, &pw);
    let connect = format!(
        "{ensure}\nGRANT CONNECT ON DATABASE {db} TO {role};",
        db = pg_quote_ident(db_name),
        role = pg_quote_ident(role),
    );
    let up = connect.to_ascii_uppercase();
    if up.contains("SUPERUSER") && !up.contains("NOSUPERUSER") {
        return Err(OpsError::Engine(
            "internal: superuser leaked into grant SQL".into(),
        ));
    }
    ops.run_helper(
        &["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1"],
        Some(connect.as_bytes()),
    )
    .map_err(OpsError::Engine)?;
    let dml = if level == "write" {
        format!(
            "GRANT USAGE ON SCHEMA public TO {role};\n\
             GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role};\n\
             GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO {role};",
            role = pg_quote_ident(role),
        )
    } else {
        format!(
            "GRANT USAGE ON SCHEMA public TO {role};\n\
             GRANT SELECT ON ALL TABLES IN SCHEMA public TO {role};\n\
             GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO {role};",
            role = pg_quote_ident(role),
        )
    };
    ops.run_helper(
        &["psql", "-d", db_name, "-v", "ON_ERROR_STOP=1"],
        Some(dml.as_bytes()),
    )
    .map_err(OpsError::Engine)?;
    Ok(())
}

fn apply_pg_revoke(ops: &dyn SystemOps, db_name: &str, role: &str) -> Result<(), OpsError> {
    let connect = format!(
        "REVOKE CONNECT ON DATABASE {db} FROM {role};",
        db = pg_quote_ident(db_name),
        role = pg_quote_ident(role),
    );
    let _ = ops.run_helper(
        &["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1"],
        Some(connect.as_bytes()),
    );
    let dml = format!(
        "REVOKE ALL ON SCHEMA public FROM {role};\n\
         REVOKE ALL ON ALL TABLES IN SCHEMA public FROM {role};\n\
         REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM {role};",
        role = pg_quote_ident(role),
    );
    let _ = ops.run_helper(
        &["psql", "-d", db_name, "-v", "ON_ERROR_STOP=1"],
        Some(dml.as_bytes()),
    );
    Ok(())
}

/// Grant another workspace read|write on this database via **their** PG
/// role. Never shares superuser. Same-workspace (the owner) is rejected
/// with teaching — they already have manage/write.
pub fn grant_access(
    ops: &dyn SystemOps,
    caller_project: Option<&str>,
    db_spec: &str,
    grantee_project_id: &str,
    level: &str,
    can_manage: bool,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let level = validate_level(level)?;
    let row = find_database(db_spec, caller_project)?;
    if !caller_can_manage(caller_project, &row) {
        return Err(OpsError::Forbidden(
            "requires owner or can_manage on this database — ask your human".into(),
        ));
    }
    if grantee_project_id == row.project_id {
        return Err(OpsError::Usage(
            "that workspace owns this database — it already has manage/write (cross-workspace grants only)"
                .into(),
        ));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if project_name(&conn, grantee_project_id).is_none() {
            return Err(OpsError::NotFound(format!(
                "workspace not registered: {grantee_project_id}"
            )));
        }
    }
    let role = default_agent_role(grantee_project_id);
    apply_pg_grant(ops, &row.name, &role, level)?;
    let now = now_secs();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO sql_grants (database_id, project_id, level, can_manage, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT (database_id, project_id) DO UPDATE SET \
               level = excluded.level, can_manage = excluded.can_manage, updated_at = excluded.updated_at",
            rusqlite::params![row.id, grantee_project_id, level, if can_manage { 1 } else { 0 }, now],
        )
        .map_err(|e| OpsError::Engine(format!("catalog grant: {e}")))?;
    }
    let v = serde_json::json!({
        "ok": true,
        "databaseId": row.id,
        "name": row.name,
        "projectId": grantee_project_id,
        "level": level,
        "canManage": can_manage,
        "role": role,
    });
    assert_no_superuser_json(&v);
    Ok(v)
}

pub fn revoke_access(
    ops: &dyn SystemOps,
    caller_project: Option<&str>,
    db_spec: &str,
    grantee_project_id: &str,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let row = find_database(db_spec, caller_project)?;
    if !caller_can_manage(caller_project, &row) {
        return Err(OpsError::Forbidden(
            "requires owner or can_manage on this database — ask your human".into(),
        ));
    }
    if grantee_project_id == row.project_id {
        return Err(OpsError::Usage(
            "cannot revoke the owning workspace — drop the database instead".into(),
        ));
    }
    let role = default_agent_role(grantee_project_id);
    apply_pg_revoke(ops, &row.name, &role)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "DELETE FROM sql_grants WHERE database_id = ?1 AND project_id = ?2",
            rusqlite::params![row.id, grantee_project_id],
        )
        .map_err(|e| OpsError::Engine(format!("catalog revoke: {e}")))?;
    }
    Ok(serde_json::json!({
        "ok": true,
        "databaseId": row.id,
        "name": row.name,
        "projectId": grantee_project_id,
        "revoked": true,
    }))
}

fn validate_bind_role(role: &str) -> Result<Option<String>, OpsError> {
    let role = role.trim();
    if role.is_empty() {
        return Ok(None);
    }
    let lower = role.to_ascii_lowercase();
    if lower == "postgres" || lower == "k2_admin" || lower.contains("superuser") {
        return Err(OpsError::Usage(
            "bind role cannot be postgres, k2_admin, or a superuser name".into(),
        ));
    }
    if role.len() > 63
        || !role.chars().enumerate().all(|(i, c)| {
            if i == 0 {
                c.is_ascii_alphabetic() || c == '_'
            } else {
                c.is_ascii_alphanumeric() || c == '_'
            }
        })
    {
        return Err(OpsError::Usage(
            "bind role must be a Postgres identifier (letter/underscore, then alnum/_ , ≤63)"
                .into(),
        ));
    }
    Ok(Some(role.to_string()))
}

/// D22: persist the PG role the workspace assistant uses. Owner/admin.
/// Does **not** mint RLS, does **not** print a DSN or password.
pub fn bind_role(
    db_spec: Option<&str>,
    project_id: Option<&str>,
    role: &str,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let row = if let Some(spec) = db_spec.map(str::trim).filter(|s| !s.is_empty()) {
        find_database(spec, project_id)?
    } else if let Some(pid) = project_id {
        let db = k2_core::db::shared();
        let conn = db.lock();
        load_active_default(&conn, pid).ok_or_else(|| {
            OpsError::NotFound("no database for this workspace — run 'k2 db create' first".into())
        })?
    } else {
        return Err(OpsError::Usage(
            "bind requires --db or a workspace (k2 db bind --role <pg_role>)".into(),
        ));
    };
    let bind = validate_bind_role(role)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE sql_databases SET bind_role = ?1 WHERE id = ?2",
            rusqlite::params![bind.as_deref(), row.id],
        )
        .map_err(|e| OpsError::Engine(format!("catalog bind: {e}")))?;
    }
    let shown = bind
        .clone()
        .unwrap_or_else(|| default_agent_role(&row.project_id));
    let v = serde_json::json!({
        "ok": true,
        "databaseId": row.id,
        "name": row.name,
        "bindRole": shown,
        "default": bind.is_none(),
    });
    let s = v.to_string().to_ascii_lowercase();
    if s.contains("password") || s.contains("\"dsn\"") || s.contains("dbsec_") {
        return Err(OpsError::Engine(
            "refusing to return secrets from bind".into(),
        ));
    }
    assert_no_superuser_json(&v);
    Ok(v)
}

struct GrantRow {
    project_id: String,
    level: String,
    can_manage: bool,
}

fn grants_for(conn: &rusqlite::Connection, database_id: &str) -> Vec<GrantRow> {
    conn.prepare(
        "SELECT project_id, level, can_manage FROM sql_grants \
         WHERE database_id = ?1 ORDER BY created_at, project_id",
    )
    .ok()
    .and_then(|mut stmt| {
        stmt.query_map(rusqlite::params![database_id], |r| {
            Ok(GrantRow {
                project_id: r.get(0)?,
                level: r.get(1)?,
                can_manage: r.get::<_, i64>(2)? != 0,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .ok()
    })
    .unwrap_or_default()
}

fn participant_json(
    conn: &rusqlite::Connection,
    project_id: &str,
    level: &str,
    can_manage: bool,
) -> serde_json::Value {
    serde_json::json!({
        "projectId": project_id,
        "workspace": project_name(conn, project_id),
        "level": level,
        "canManage": can_manage,
    })
}

/// Owner view (`viewer = None`): every database. Agent view: owned + granted.
pub fn catalog_json(viewer: Option<&str>) -> serde_json::Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(&format!(
        "SELECT {DB_ROW_COLS}, created_at FROM sql_databases ORDER BY created_at, name"
    )) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({ "ok": true, "databases": [] }),
    };
    let loaded: Vec<(DbRow, i64)> = stmt
        .query_map([], |r| {
            let row = db_row_from(r)?;
            let created: i64 = r.get(7)?;
            Ok((row, created))
        })
        .ok()
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (row, created_at) in loaded {
        let grants = grants_for(&conn, &row.id);
        let your = if let Some(v) = viewer {
            if v == row.project_id {
                Some("write")
            } else {
                grants
                    .iter()
                    .find(|g| g.project_id == v)
                    .map(|g| g.level.as_str())
            }
        } else {
            None
        };
        if let Some(v) = viewer {
            if your.is_none() && v != row.project_id {
                continue;
            }
        }
        let used = count_active(&conn, &row.project_id);
        let cap = project_cap(&conn, &row.project_id);
        let bind = row
            .bind_role
            .clone()
            .unwrap_or_else(|| default_agent_role(&row.project_id));
        let grant_json: Vec<serde_json::Value> = grants
            .iter()
            .map(|g| participant_json(&conn, &g.project_id, &g.level, g.can_manage))
            .collect();
        out.push(serde_json::json!({
            "id": row.id,
            "name": row.name,
            "status": row.status,
            "createdAt": created_at,
            "type": "sql",
            "documents": true,
            "ownerProjectId": row.project_id,
            "ownerWorkspace": project_name(&conn, &row.project_id),
            "bindRole": bind,
            "cap": { "used": used, "cap": cap },
            "owner": participant_json(&conn, &row.project_id, "write", true),
            "grants": grant_json,
            "yourLevel": your,
            "dbAgentAccess": project_db_agent_access(&conn, &row.project_id),
        }));
    }
    serde_json::json!({ "ok": true, "databases": out })
}

fn require_store_db(
    secrets: &dyn SecretStore,
    project_id: &str,
) -> Result<(DbRow, String, String), OpsError> {
    require_running()?;
    let row = active_row(project_id)?;
    let (user, pw) = migrator_creds(secrets, &row)?;
    Ok((row, user, pw))
}

fn agent_role_for_db(db_name: &str) -> String {
    format!("{db_name}_agent")
}

fn ensure_store_table(
    ops: &dyn SystemOps,
    db: &str,
    user: &str,
    password: &str,
) -> Result<(), OpsError> {
    exec_as(
        ops,
        db,
        user,
        password,
        "CREATE TABLE IF NOT EXISTS _k2_store (\n\
           collection TEXT NOT NULL,\n\
           id TEXT NOT NULL,\n\
           doc JSONB NOT NULL,\n\
           updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
           PRIMARY KEY (collection, id)\n\
         );",
    )?;
    grant_agent_k2_tables(ops, db)?;
    Ok(())
}

pub fn store_create(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    collection: &str,
) -> Result<serde_json::Value, OpsError> {
    let name = validate_collection(collection)?;
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    ensure_store_table(ops, &row.name, &user, &pw)?;
    Ok(serde_json::json!({ "ok": true, "collection": name }))
}

pub fn store_list(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
) -> Result<serde_json::Value, OpsError> {
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    let raw = exec_as(
        ops,
        &row.name,
        &user,
        &pw,
        "SELECT collection FROM _k2_store GROUP BY collection ORDER BY collection;",
    )
    .unwrap_or_default();
    let names: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    Ok(serde_json::json!({ "ok": true, "collections": names }))
}

pub fn store_put(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    collection: &str,
    id: &str,
    doc: &serde_json::Value,
) -> Result<serde_json::Value, OpsError> {
    let name = validate_collection(collection)?;
    let id = id.trim();
    if id.is_empty() {
        return Err(OpsError::Usage("missing document id".into()));
    }
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    ensure_store_table(ops, &row.name, &user, &pw)?;
    let sql = format!(
        "INSERT INTO _k2_store (collection, id, doc) VALUES ({coll}, {id}, {doc}::jsonb) \
         ON CONFLICT (collection, id) DO UPDATE SET doc = EXCLUDED.doc, updated_at = now();",
        coll = pg_quote_literal(&name),
        id = pg_quote_literal(id),
        doc = pg_quote_literal(&doc.to_string()),
    );
    exec_as(ops, &row.name, &user, &pw, &sql)?;
    Ok(serde_json::json!({ "ok": true, "id": id, "collection": name }))
}

pub fn store_get(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    collection: &str,
    id: &str,
) -> Result<serde_json::Value, OpsError> {
    let name = validate_collection(collection)?;
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    let sql = format!(
        "SELECT doc FROM _k2_store WHERE collection = {coll} AND id = {id};",
        coll = pg_quote_literal(&name),
        id = pg_quote_literal(id.trim()),
    );
    let raw = exec_as(ops, &row.name, &user, &pw, &sql)?;
    if raw.is_empty() {
        return Err(OpsError::NotFound(format!(
            "document '{id}' not in collection '{name}'"
        )));
    }
    let doc: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
    Ok(serde_json::json!({ "ok": true, "id": id, "collection": name, "doc": doc }))
}

pub fn store_query(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    collection: &str,
    limit: u32,
) -> Result<serde_json::Value, OpsError> {
    let name = validate_collection(collection)?;
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    let lim = limit.max(1).min(500);
    let sql = format!(
        "SELECT id::text || E'\\t' || doc::text FROM _k2_store WHERE collection = {coll} LIMIT {lim};",
        coll = pg_quote_literal(&name),
    );
    let raw = exec_as(ops, &row.name, &user, &pw, &sql).unwrap_or_default();
    let mut docs = Vec::new();
    for line in raw.lines() {
        if let Some((id, doc)) = line.split_once('\t') {
            let v: serde_json::Value =
                serde_json::from_str(doc).unwrap_or(serde_json::Value::String(doc.to_string()));
            docs.push(serde_json::json!({ "id": id, "doc": v }));
        }
    }
    Ok(serde_json::json!({ "ok": true, "collection": name, "docs": docs }))
}

pub fn store_rm(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    collection: &str,
    id: &str,
) -> Result<serde_json::Value, OpsError> {
    let name = validate_collection(collection)?;
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    let sql = format!(
        "DELETE FROM _k2_store WHERE collection = {coll} AND id = {id};",
        coll = pg_quote_literal(&name),
        id = pg_quote_literal(id.trim()),
    );
    exec_as(ops, &row.name, &user, &pw, &sql)?;
    Ok(serde_json::json!({ "ok": true, "removed": id }))
}

pub fn store_drop(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    collection: &str,
) -> Result<serde_json::Value, OpsError> {
    let name = validate_collection(collection)?;
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    let sql = format!(
        "DELETE FROM _k2_store WHERE collection = {coll};",
        coll = pg_quote_literal(&name),
    );
    exec_as(ops, &row.name, &user, &pw, &sql)?;
    Ok(serde_json::json!({ "ok": true, "dropped": name }))
}

fn validate_collection(name: &str) -> Result<String, OpsError> {
    let n = name.trim();
    if n.is_empty()
        || n.len() > 64
        || !n
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(OpsError::Usage(
            "collection name must be 1–64 [A-Za-z0-9_-]".into(),
        ));
    }
    Ok(n.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_role_sql_has_no_superuser_createdb_bypassrls() {
        let sql = create_role_sql("ws_abc_agent", "secret");
        let up = sql.to_ascii_uppercase();
        assert!(up.contains("NOSUPERUSER"));
        assert!(up.contains("NOCREATEDB"));
        assert!(up.contains("NOBYPASSRLS"));
        assert!(!up.contains("FORCE ROW LEVEL"));
        assert!(!up.split_whitespace().any(|w| w == "SUPERUSER"));
    }

    #[test]
    fn ensure_role_sql_is_idempotent_duplicate_object() {
        let sql = ensure_role_sql("ws_abc_agent", "secret");
        let up = sql.to_ascii_uppercase();
        assert!(up.contains("EXCEPTION WHEN DUPLICATE_OBJECT"));
        assert!(up.contains("NOSUPERUSER"));
        assert!(!up.split_whitespace().any(|w| w == "SUPERUSER"));
        // Semicolon must precede EXCEPTION (PL/pgSQL), not be stripped.
        let before_ex = up.split("EXCEPTION").next().unwrap();
        assert!(
            before_ex.trim_end().ends_with(';'),
            "CREATE ROLE inside DO must end with ; before EXCEPTION, got {sql}"
        );
        let alter = alter_role_password_sql("ws_abc_agent", "secret");
        let aup = alter.to_ascii_uppercase();
        assert!(aup.contains("ALTER ROLE"));
        assert!(aup.contains("NOSUPERUSER"));
    }

    #[test]
    fn pg_ident_sanitizes_uuid() {
        let id = pg_ident_for_project("01234567-89ab-cdef-0123-456789abcdef");
        assert!(id.starts_with("ws_"));
        assert!(!id.contains('-'));
    }

    #[test]
    fn versioned_sql_filename_requires_four_digits_and_underscore() {
        assert!(is_versioned_sql_filename("0001_init.sql"));
        assert!(is_versioned_sql_filename("0099_add_users.sql"));
        assert!(!is_versioned_sql_filename("init.sql"));
        assert!(!is_versioned_sql_filename("1_init.sql"));
        assert!(!is_versioned_sql_filename("0001.sql"));
        assert!(!is_versioned_sql_filename("0001_init.sql.bak"));
    }

    #[test]
    fn split_psql_fields_accepts_pipe_and_tab() {
        assert_eq!(
            split_psql_fields("0001_dogfood|deadbeef"),
            vec!["0001_dogfood", "deadbeef"]
        );
        assert_eq!(
            split_psql_fields("0001_dogfood\tdeadbeef"),
            vec!["0001_dogfood", "deadbeef"]
        );
        assert_eq!(
            split_psql_fields("0001_init|abc|2026-01-01 00:00:00+00"),
            vec!["0001_init", "abc", "2026-01-01 00:00:00+00"]
        );
        assert_eq!(
            split_psql_fields("0001_init\tabc\t2026-01-01 00:00:00+00"),
            vec!["0001_init", "abc", "2026-01-01 00:00:00+00"]
        );
    }
}
