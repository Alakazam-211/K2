//! Workspace database + JSONB store operations (create/migrate/dump/store).

use std::path::{Path, PathBuf};

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
    #[allow(dead_code)]
    Forbidden(String),
    Engine(String),
}

impl OpsError {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Usage(_) => "400 Bad Request",
            Self::NotFound(_) => "404 Not Found",
            Self::CapReached(_) | Self::NotReady(_) => "409 Conflict",
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
            | Self::Forbidden(h)
            | Self::Engine(h) => h,
        }
    }
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
    agent_secret_ref: Option<String>,
    migrator_secret_ref: Option<String>,
    status: String,
}

fn load_active_by_client(
    conn: &rusqlite::Connection,
    project_id: &str,
    client_id: &str,
) -> Option<DbRow> {
    conn.query_row(
        "SELECT id, name, agent_secret_ref, migrator_secret_ref, status FROM sql_databases \
         WHERE project_id = ?1 AND client_id = ?2",
        rusqlite::params![project_id, client_id],
        |r| {
            Ok(DbRow {
                id: r.get(0)?,
                name: r.get(1)?,
                agent_secret_ref: r.get(2)?,
                migrator_secret_ref: r.get(3)?,
                status: r.get(4)?,
            })
        },
    )
    .ok()
}

fn load_active_by_name(
    conn: &rusqlite::Connection,
    project_id: &str,
    name: &str,
) -> Option<DbRow> {
    conn.query_row(
        "SELECT id, name, agent_secret_ref, migrator_secret_ref, status FROM sql_databases \
         WHERE project_id = ?1 AND name = ?2 AND status = 'active'",
        rusqlite::params![project_id, name],
        |r| {
            Ok(DbRow {
                id: r.get(0)?,
                name: r.get(1)?,
                agent_secret_ref: r.get(2)?,
                migrator_secret_ref: r.get(3)?,
                status: r.get(4)?,
            })
        },
    )
    .ok()
}

fn load_active_default(conn: &rusqlite::Connection, project_id: &str) -> Option<DbRow> {
    conn.query_row(
        "SELECT id, name, agent_secret_ref, migrator_secret_ref, status FROM sql_databases \
         WHERE project_id = ?1 AND status = 'active' ORDER BY created_at ASC LIMIT 1",
        rusqlite::params![project_id],
        |r| {
            Ok(DbRow {
                id: r.get(0)?,
                name: r.get(1)?,
                agent_secret_ref: r.get(2)?,
                migrator_secret_ref: r.get(3)?,
                status: r.get(4)?,
            })
        },
    )
    .ok()
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
        return Err(OpsError::Usage("internal db name must start with ws_".into()));
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
        "{}\n{}",
        create_role_sql(&migrator, &migrator_pw),
        create_role_sql(&agent, &agent_pw),
    );
    let up = role_sql.to_ascii_uppercase();
    if up.contains("SUPERUSER") && !up.contains("NOSUPERUSER") {
        return Err(OpsError::Engine("internal: superuser leaked into role SQL".into()));
    }
    if up.contains("CREATEDB") && !up.contains("NOCREATEDB") {
        return Err(OpsError::Engine("internal: createdb leaked into role SQL".into()));
    }
    if up.contains("BYPASSRLS") && !up.contains("NOBYPASSRLS") {
        return Err(OpsError::Engine("internal: bypassrls leaked into role SQL".into()));
    }
    if up.contains("FORCE ROW LEVEL") {
        return Err(OpsError::Engine("internal: FORCE RLS is not v1".into()));
    }

    ops.run_helper(&["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1"], Some(role_sql.as_bytes()))
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
           applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\n\
         );\n\
         CREATE TABLE IF NOT EXISTS _k2_store (\n\
           collection TEXT NOT NULL,\n\
           id TEXT NOT NULL,\n\
           doc JSONB NOT NULL,\n\
           updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
           PRIMARY KEY (collection, id)\n\
         );\n",
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
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, status, created_at FROM sql_databases \
             WHERE project_id = ?1 ORDER BY created_at",
        )
        .expect("prepare");
    let rows = stmt
        .query_map(rusqlite::params![project_id], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "status": r.get::<_, String>(2)?,
                "createdAt": r.get::<_, i64>(3)?,
            }))
        })
        .ok();
    let mut dbs = Vec::new();
    if let Some(rows) = rows {
        for r in rows.flatten() {
            dbs.push(r);
        }
    }
    let used = count_active(&conn, project_id);
    serde_json::json!({ "ok": true, "databases": dbs, "used": used })
}

pub fn dsn_for_project(
    secrets: &dyn SecretStore,
    project_id: &str,
    cap: u32,
) -> Result<serde_json::Value, OpsError> {
    require_running()?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = load_active_default(&conn, project_id).ok_or_else(|| {
        OpsError::NotFound("no database for this workspace — run 'k2 db create' first".into())
    })?;
    let used = count_active(&conn, project_id);
    dsn_json(secrets, &row, true, used, cap)
}

fn active_row(project_id: &str) -> Result<DbRow, OpsError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    load_active_default(&conn, project_id).ok_or_else(|| {
        OpsError::NotFound("no database for this workspace — run 'k2 db create' first".into())
    })
}

fn migrator_creds(
    secrets: &dyn SecretStore,
    row: &DbRow,
) -> Result<(String, String), OpsError> {
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
            ],
            &[("PGPASSWORD", password)],
            Some(sql.as_bytes()),
        )
        .map_err(OpsError::Engine)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
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
    let mig_dir = Path::new(ws_path).join(rel);
    if !mig_dir.is_dir() {
        return Err(OpsError::NotFound(format!(
            "migrations directory not found: {rel}"
        )));
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&mig_dir)
        .map_err(|e| OpsError::Engine(format!("read migrations: {e}")))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some("sql")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| {
                        n.len() >= 5
                            && n.as_bytes().iter().take(4).all(|b| b.is_ascii_digit())
                            && n.as_bytes().get(4) == Some(&b'_')
                    })
                    .unwrap_or(false)
        })
        .collect();
    files.sort();

    exec_as(
        ops,
        &row.name,
        &user,
        &pw,
        "CREATE TABLE IF NOT EXISTS _k2_migrations (\n\
           version TEXT PRIMARY KEY,\n\
           applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\n\
         );",
    )?;
    let applied_raw = exec_as(ops, &row.name, &user, &pw, "SELECT version FROM _k2_migrations;")?;
    let applied: Vec<&str> = applied_raw
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let mut ran = Vec::new();
    for path in files {
        let version = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if applied.iter().any(|a| *a == version) {
            continue;
        }
        let sql = std::fs::read_to_string(&path)
            .map_err(|e| OpsError::Engine(format!("read {}: {e}", path.display())))?;
        if sql.to_ascii_uppercase().contains("FORCE ROW LEVEL") {
            return Err(OpsError::Usage(
                "FORCE RLS is not enabled in v1 — remove FORCE ROW LEVEL SECURITY from migrations"
                    .into(),
            ));
        }
        exec_as(ops, &row.name, &user, &pw, &sql)?;
        let insert = format!(
            "INSERT INTO _k2_migrations (version) VALUES ({});",
            pg_quote_literal(&version)
        );
        exec_as(ops, &row.name, &user, &pw, &insert)?;
        ran.push(version);
    }
    Ok(serde_json::json!({
        "ok": true,
        "applied": ran,
        "noop": ran.is_empty(),
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
    let row = active_row(project_id)?;
    let (user, pw) = migrator_creds(secrets, &row)?;
    let src = match resolve_in_path(ws_path, file) {
        Ok(p) => p,
        Err(InPathError::Usage(h)) => return Err(OpsError::Usage(h)),
        Err(InPathError::NotFound(h)) => return Err(OpsError::NotFound(h)),
    };
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

pub fn drop_database(
    ops: &dyn SystemOps,
    project_id: &str,
) -> Result<serde_json::Value, OpsError> {
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
            "UPDATE sql_databases SET status = 'dropped', dropped_at = ?1 WHERE id = ?2",
            rusqlite::params![now_secs(), row.id],
        )
        .map_err(|e| OpsError::Engine(format!("catalog drop: {e}")))?;
    }
    Ok(serde_json::json!({ "ok": true, "dropped": row.name }))
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

pub fn store_create(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    project_id: &str,
    collection: &str,
) -> Result<serde_json::Value, OpsError> {
    let name = validate_collection(collection)?;
    let (row, user, pw) = require_store_db(secrets, project_id)?;
    exec_as(
        ops,
        &row.name,
        &user,
        &pw,
        "CREATE TABLE IF NOT EXISTS _k2_store (\n\
           collection TEXT NOT NULL,\n\
           id TEXT NOT NULL,\n\
           doc JSONB NOT NULL,\n\
           updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
           PRIMARY KEY (collection, id)\n\
         );",
    )?;
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
    let names: Vec<&str> = raw.lines().map(str::trim).filter(|s| !s.is_empty()).collect();
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
    fn pg_ident_sanitizes_uuid() {
        let id = pg_ident_for_project("01234567-89ab-cdef-0123-456789abcdef");
        assert!(id.starts_with("ws_"));
        assert!(!id.contains('-'));
    }
}
