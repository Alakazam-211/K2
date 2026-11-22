//! `k2 db import supabase <dump> --db <db> [--allow-unresolved] [--dry-run]`
//! (prd-app-supabase-compat-v1 §9.1, slice S2): a Supabase project's
//! `pg_dump -Fc` restored into a database this workspace owns, recognisably
//! the same (§1 R0): its tables, functions, row policies and grants to
//! `anon` / `authenticated` / `service_role`, with K2's own `auth` and
//! `storage` in place of Supabase's.
//!
//! **What it does, in order.** Nothing is written until every check passed.
//! 1. Reads the dump's table of contents ([`toc::classify`]): Supabase's own
//!    schemas (`auth`, `storage`, `realtime`, `supabase_functions`,
//!    `graphql*`, `pgsodium`, `vault`, `extensions`, `net`, `pgbouncer`,
//!    `cron`, …) and extensions are dropped; extensions the target lets a
//!    database owner create (pgcrypto, uuid-ossp, pg_trgm, …) are carried
//!    into a K2-made `extensions` schema; ACLs go to step 3.
//! 2. Scans what is restored for references it can't carry ([`scan`]:
//!    `vault.decrypted_secrets`, `net.http_post`, `cron.schedule`, an `auth`
//!    or `storage` object K2's schemas lack, a policy for another role) and
//!    **stops** with the list unless `--allow-unresolved`. It never guesses.
//! 3. Reads the grants ([`acl::rewrite`]): kept for the standard roles,
//!    dropped for Supabase's own; default privileges `FOR ROLE postgres`
//!    become the migrator's.
//! 4. Reads `auth.users` (never the password), `storage.buckets` /
//!    `storage.objects` rows and `supabase_migrations.schema_migrations`.
//! 5. Refuses a target that isn't empty, and a migrations folder that already
//!    has numbered files when the dump brings migrations.
//! 6. Writes: the standard roles and K2's `auth` / `storage` schemas
//!    ([`standin`]), the `auth.users` projection rows, then `pg_restore
//!    --no-owner` **as the migrator** (so it owns everything and SECURITY
//!    DEFINER functions keep bypassing row security as under `postgres`),
//!    `FORCE ROW LEVEL SECURITY` lifted with a warning naming each table
//!    (K2 refuses FORCE in migrations), the data, Storage's rows, post-data
//!    (indexes, foreign keys, triggers, policies), the rewritten grants, the
//!    migration map ([`migrations`]), and K2's privilege sync.
//!
//! A failure after step 6 began leaves the database half-loaded: the answer
//! says to drop it and import into a fresh one (an import never runs over a
//! non-empty database).
//!
//! **Not done here:** the App's users (`k2 app user import`, the same ids as
//! `auth.users` here), `auth.users` kept in step afterwards (lane A), the
//! Storage bytes (lane D's `k2 app storage import`).

pub mod acl;
pub mod copy;
pub mod migrations;
pub mod scan;
pub mod standin;
pub mod toc;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value as J};

use super::cluster::Cluster;
use super::ops::{self, DbRow, OpsError};
use super::paths::{resolve_in_path, InPathError};
use super::secrets::{FileSecretStore, SecretStore};
use super::sysops::SystemOps;
use crate::cli_response::CliResponse;
use copy::CopyTable;
use k2_core::brand::CLI_NAME;
use toc::Extract;

pub const ROUTE: &str = "/cli/db/import/supabase";
/// Where the migration map is written (workspace-relative).
pub const MAP_FILE: &str = ".k2/db/supabase-migrations.json";
const MIGRATIONS_DIR: &str = ".k2/db/migrations";
/// Rows per INSERT when K2 writes rows it read from the dump.
const CHUNK: usize = 500;

/// What to import, and how.
pub struct Opts<'a> {
    pub project_id: &'a str,
    pub ws_path: &'a str,
    pub file: &'a str,
    pub db: &'a str,
    pub allow_unresolved: bool,
    pub dry_run: bool,
}

/// Why an import stopped.
#[derive(Debug)]
pub enum Refusal {
    /// Before anything was written.
    Ops(OpsError),
    /// References the import can't carry (nothing written).
    Unresolved(J),
    /// A step after writing began failed (the database is half-loaded).
    Failed { step: &'static str, error: String, report: J },
}

impl From<OpsError> for Refusal {
    fn from(e: OpsError) -> Self {
        Refusal::Ops(e)
    }
}

/// A private folder for `pg_restore`'s entry lists, gone when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Result<Self, OpsError> {
        let p = std::env::temp_dir().join(format!("k2-supabase-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).map_err(|e| OpsError::Engine(format!("temp folder: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).map_err(|e| OpsError::Engine(format!("temp folder: {e}")))?;
        }
        Ok(Tmp(p))
    }
    fn write(&self, name: &str, lines: &[String]) -> Result<String, OpsError> {
        let f = self.0.join(name);
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(&f, text).map_err(|e| OpsError::Engine(format!("write {name}: {e}")))?;
        Ok(f.to_string_lossy().into_owned())
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The first `ERROR:` (or last) line of a tool's failure, without row values.
fn first_error(e: &str) -> String {
    let line = e.lines().find(|l| l.contains("ERROR:") || l.contains("error:")).or_else(|| e.lines().last()).unwrap_or(e);
    line.trim().chars().take(400).collect()
}

/// `pg_restore` without a database (`-l`, `-f -`): its stdout.
fn restore_text(ops: &dyn SystemOps, bin: &str, args: &[&str]) -> Result<String, String> {
    ops.run_cmd(bin, args, &[], None).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| first_error(&e))
}

fn quote_literal(v: &str) -> String {
    ops::pg_quote_literal(v)
}

/// `INSERT INTO <table> (<cols>) VALUES …;` for `rows` (at most [`CHUNK`]),
/// with only `cols` K2 fills; text values cast by the column's own type.
fn insert_sql(table: &str, t: &CopyTable, rows: &[Vec<Option<String>>], cols: &[&str], not_null_false: &[&str]) -> String {
    let have: Vec<&str> = cols.iter().copied().filter(|c| t.col(c).is_some()).collect();
    let mut sql = format!("INSERT INTO {table} ({}) VALUES\n", have.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "));
    let values: Vec<String> = rows
        .iter()
        .map(|r| {
            let vals: Vec<String> = have
                .iter()
                .map(|c| match t.get(r, c) {
                    Some(v) => quote_literal(v),
                    None if not_null_false.contains(c) => "false".into(),
                    None => "NULL".into(),
                })
                .collect();
            format!("({})", vals.join(", "))
        })
        .collect();
    sql.push_str(&values.join(",\n"));
    sql.push_str(";\n");
    sql
}

/// The tables `ALTER TABLE … FORCE ROW LEVEL SECURITY` names in a script.
fn forced_in(script: &str) -> Vec<String> {
    script
        .lines()
        .filter_map(|l| l.trim().strip_suffix(" FORCE ROW LEVEL SECURITY;"))
        .filter_map(|l| l.strip_prefix("ALTER TABLE ONLY ").or_else(|| l.strip_prefix("ALTER TABLE ")))
        .map(|t| t.trim().to_string())
        .collect()
}

/// Is the target database empty of the App's objects (the two `_k2_*`
/// tables aside; the helpers' own schema holds functions only)?
const TARGET_EMPTY_SQL: &str = "SELECT count(*) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
     WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S') AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
       AND n.nspname NOT LIKE 'pg\\_%' AND NOT (n.nspname = 'public' AND c.relname IN ('_k2_migrations', '_k2_store'));";

/// The numbered migration files already in the workspace's migrations folder.
fn numbered_files(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.ends_with(".sql") && n.len() > 5 && n.as_bytes()[..4].iter().all(u8::is_ascii_digit) && n.as_bytes()[4] == b'_')
        .collect();
    v.sort();
    v
}

/// Everything read before writing.
struct Prepared {
    plan: toc::Plan,
    acl: acl::AclOut,
    forced: Vec<String>,
    users: Option<CopyTable>,
    buckets: Option<CopyTable>,
    objects: Option<CopyTable>,
    mapped: Vec<migrations::Mapped>,
}

fn extract(ops: &dyn SystemOps, bin: &str, tmp: &Tmp, src: &str, plan: &toc::Plan, which: Extract, qualified: &str) -> Result<Option<CopyTable>, OpsError> {
    let Some(line) = plan.extract.get(&which) else { return Ok(None) };
    let list = tmp.write(&format!("{which:?}.list"), std::slice::from_ref(line))?;
    let script = restore_text(ops, bin, &["-f", "-", "-a", "-L", &list, src]).map_err(|e| OpsError::Engine(format!("reading {qualified} from the dump: {e}")))?;
    copy::parse_copy(&script, qualified).map_err(OpsError::Usage)
}

fn prepare(ops: &dyn SystemOps, bin: &str, tmp: &Tmp, src: &str, trusted: &BTreeSet<String>, migrator: &str) -> Result<Prepared, OpsError> {
    let toc_text = restore_text(ops, bin, &["-l", src])
        .map_err(|e| OpsError::Usage(format!("not a dump pg_restore can read (make it with pg_dump -Fc): {e}")))?;
    let mut plan = toc::classify(&toc_text, trusted);
    if !plan.pending_triggers.is_empty() {
        let list = tmp.write("triggers.list", &plan.pending_triggers)?;
        let defs = restore_text(ops, bin, &["-f", "-", "-s", "--no-owner", "-L", &list, src]).map_err(|e| OpsError::Engine(format!("reading triggers on auth/storage: {e}")))?;
        toc::decide_triggers(&mut plan, &defs);
    }
    let keep = tmp.write("keep.list", &plan.keep)?;
    let schema = restore_text(ops, bin, &["-f", "-", "-s", "--no-owner", "-L", &keep, src]).map_err(|e| OpsError::Engine(format!("reading the dump's schema: {e}")))?;
    plan.unresolved.extend(scan::scan(&schema));
    let forced = forced_in(&schema);
    let acl = if plan.acl.is_empty() {
        acl::AclOut::default()
    } else {
        let list = tmp.write("acl.list", &plan.acl)?;
        let script = restore_text(ops, bin, &["-f", "-", "--no-owner", "-L", &list, src]).map_err(|e| OpsError::Engine(format!("reading the dump's grants: {e}")))?;
        acl::rewrite(&script, migrator).map_err(OpsError::Usage)?
    };
    let users = extract(ops, bin, tmp, src, &plan, Extract::AuthUsers, "auth.users")?;
    let buckets = extract(ops, bin, tmp, src, &plan, Extract::StorageBuckets, "storage.buckets")?;
    let objects = extract(ops, bin, tmp, src, &plan, Extract::StorageObjects, "storage.objects")?;
    let mapped = match extract(ops, bin, tmp, src, &plan, Extract::Migrations, "supabase_migrations.schema_migrations")? {
        Some(t) => migrations::plan(&t).map_err(OpsError::Usage)?,
        None => Vec::new(),
    };
    Ok(Prepared { plan, acl, forced, users, buckets, objects, mapped })
}

fn not_carried(t: Option<&CopyTable>, cols: &[&str], table: &str) -> Vec<String> {
    t.map(|t| t.columns.iter().filter(|c| !cols.contains(&c.as_str()) && c.as_str() != "path_tokens").map(|c| format!("{table}.{c}")).collect()).unwrap_or_default()
}

fn report(db: &str, p: &Prepared, dry_run: bool) -> J {
    let n = |t: &Option<CopyTable>| t.as_ref().map_or(0, |t| t.rows.len());
    let mut cols = not_carried(p.users.as_ref(), standin::AUTH_USER_COLUMNS, "auth.users");
    cols.retain(|c| c != "auth.users.encrypted_password");
    cols.extend(not_carried(p.buckets.as_ref(), standin::BUCKET_COLUMNS, "storage.buckets"));
    cols.extend(not_carried(p.objects.as_ref(), standin::OBJECT_COLUMNS, "storage.objects"));
    let mut notes: Vec<String> = p.plan.notes.iter().cloned().collect();
    if !p.forced.is_empty() {
        notes.push(format!(
            "FORCE ROW LEVEL SECURITY lifted on {}: migrations here may not use FORCE; the owner (the migrator) bypasses these tables' policies, as postgres did on Supabase without FORCE",
            p.forced.join(", ")
        ));
    }
    if !p.plan.carried_extensions.is_empty() {
        notes.push("the database's search_path is \"$user\", public, extensions (Supabase's), so unqualified extension calls keep working".into());
    }
    json!({
        "ok": true,
        "db": db,
        "dryRun": dry_run,
        "dropped": {
            "schemas": p.plan.dropped_schemas,
            "extensions": p.plan.dropped_extensions,
        },
        "carried": {
            "extensions": p.plan.carried_extensions,
            "tables": p.plan.tables,
            "functions": p.plan.functions,
            "policies": p.plan.policies,
            "onSupabaseTables": p.plan.kept_on_supabase_tables,
        },
        "grants": { "kept": p.acl.kept, "droppedSupabase": p.acl.dropped_internal, "droppedRoles": p.acl.dropped_roles },
        "forceLifted": p.forced,
        "authUsers": n(&p.users),
        "storage": { "buckets": n(&p.buckets), "objects": n(&p.objects) },
        "columnsNotCarried": cols,
        "migrations": {
            "count": p.mapped.len(),
            "mapFile": if p.mapped.is_empty() { J::Null } else { json!(MAP_FILE) },
            "map": migrations::map_json(&p.mapped)["migrations"],
        },
        "unresolved": p.plan.unresolved.iter().map(|u| json!({ "object": u.object, "reference": u.reference })).collect::<Vec<_>>(),
        "notes": notes,
    })
}

/// The whole import (see the module docs).
pub fn import(ops: &dyn SystemOps, secrets: &dyn SecretStore, o: &Opts<'_>) -> Result<J, Refusal> {
    ops::require_running()?;
    let src = match resolve_in_path(o.ws_path, o.file) {
        Ok(p) => p,
        Err(InPathError::Usage(h)) => return Err(OpsError::Usage(h).into()),
        Err(InPathError::NotFound(h)) => return Err(OpsError::NotFound(h).into()),
    };
    let src = src.to_string_lossy().into_owned();
    let row = ops::target_row(o.project_id, Some(o.db))?;
    ops::refuse_if_moving(&row.name)?;
    let (mig, pw) = ops::migrator_creds(secrets, &row)?;
    let cl = ops::cluster_of(&row.name)?;
    let (bin, _) = cl.restore_bin(ops);
    let trusted: BTreeSet<String> = ops::exec_as(ops, &row.name, &mig, &pw, "SELECT DISTINCT name FROM pg_catalog.pg_available_extension_versions WHERE trusted ORDER BY 1;")?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let tmp = Tmp::new()?;
    let p = prepare(ops, &bin, &tmp, &src, &trusted, &mig)?;
    let mut rep = report(&row.name, &p, o.dry_run);
    let left = ops::exec_as(ops, &row.name, &mig, &pw, TARGET_EMPTY_SQL)?;
    if left.trim() != "0" {
        return Err(OpsError::Refused(
            "import_target_not_empty",
            format!("{} already has {} relation(s) of its own: import into a fresh database ({CLI_NAME} db create --cluster <name> --name <db>), never over one", row.name, left.trim()),
        )
        .into());
    }
    let mig_dir = Path::new(o.ws_path).join(MIGRATIONS_DIR);
    let existing = numbered_files(&mig_dir);
    if !p.mapped.is_empty() && !existing.is_empty() {
        return Err(OpsError::Refused(
            "import_migrations_folder",
            format!(
                "the dump brings {} Supabase migration(s), but {MIGRATIONS_DIR} already has numbered files ({}): move them aside so the renamed ones start at 0001",
                p.mapped.len(),
                existing.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            ),
        )
        .into());
    }
    if !p.plan.unresolved.is_empty() && !o.allow_unresolved {
        rep["ok"] = json!(false);
        return Err(Refusal::Unresolved(rep));
    }
    if o.dry_run {
        return Ok(rep);
    }
    write(ops, secrets, o, &row, &cl, &mig, &pw, &bin, &tmp, &src, &p)
        .map_err(|(step, e)| Refusal::Failed { step, error: e.hint().to_string(), report: rep.clone() })?;
    if !p.mapped.is_empty() {
        rep["migrations"]["files"] = json!(p.mapped.iter().map(|m| format!("{MIGRATIONS_DIR}/{}.sql", m.k2_version)).collect::<Vec<_>>());
    }
    rep["next"] = json!([
        format!("bring the App's users over with their ids: {CLI_NAME} app user import <app> --file users.jsonl --apply (auth.users here already lists those ids)"),
        "the Storage bytes: copy them before the App serves files (storage.objects lists every object)",
    ]);
    Ok(rep)
}

type Step = (&'static str, OpsError);

#[allow(clippy::too_many_arguments)]
fn write(
    ops: &dyn SystemOps,
    secrets: &dyn SecretStore,
    o: &Opts<'_>,
    row: &DbRow,
    cl: &Cluster,
    mig: &str,
    pw: &str,
    bin: &str,
    tmp: &Tmp,
    src: &str,
    p: &Prepared,
) -> Result<(), Step> {
    let db = row.name.as_str();
    let on = |step: &'static str| move |e: OpsError| (step, e);
    let stop = ["-v", "ON_ERROR_STOP=1"];
    let as_mig = |sql: &str| ops::exec_as(ops, db, mig, pw, sql);
    // The standard roles (once per cluster) and K2's schemas.
    ops::su_psql(ops, cl, "postgres", &stop, Some(standin::roles_sql().as_bytes())).map_err(on("roles"))?;
    let roles = ops::su_psql(ops, cl, "postgres", &["-v", "ON_ERROR_STOP=1", "-tA"], Some(standin::ROLES_CHECK.as_bytes())).map_err(on("roles"))?;
    standin::roles_ok(&roles).map_err(|e| ("roles", OpsError::Refused("roles_conflict", e)))?;
    let search_path = if p.plan.carried_extensions.is_empty() {
        String::new()
    } else {
        format!("ALTER DATABASE {} SET search_path = \"$user\", public, extensions;\n", ops::pg_quote_ident(db))
    };
    as_mig(&format!(
        "CREATE SCHEMA IF NOT EXISTS extensions;\nGRANT USAGE ON SCHEMA extensions TO anon, authenticated, service_role;\n{search_path}"
    ))
    .map_err(on("the extensions schema"))?;
    ops::su_psql(ops, cl, db, &stop, Some(standin::auth_sql(mig, &format!("{db}_agent")).as_bytes())).map_err(on("the auth schema"))?;
    as_mig(&standin::storage_sql()).map_err(on("the storage schema"))?;
    // auth.users before anything refers to it (no trigger exists yet, so no
    // App code runs as the superuser writing these rows).
    if let Some(t) = &p.users {
        for chunk in t.rows.chunks(CHUNK) {
            let sql = insert_sql("auth.users", t, chunk, standin::AUTH_USER_COLUMNS, &["is_anonymous"]);
            ops::su_psql(ops, cl, db, &stop, Some(sql.as_bytes())).map_err(|e| ("auth.users rows", OpsError::Engine(first_error(e.hint()))))?;
        }
    }
    // The App's objects, as the migrator.
    let keep = tmp.write("keep.list", &p.plan.keep).map_err(on("the restore list"))?;
    let port = cl.port_arg();
    let restore = |section: &str| -> Result<(), OpsError> {
        let sec = format!("--section={section}");
        ops.run_cmd(
            bin,
            &["-h", "127.0.0.1", "-p", &port, "-U", mig, "-d", db, "--no-owner", "--exit-on-error", &sec, "-L", &keep, src],
            &[("PGPASSWORD", pw), ("PGOPTIONS", "-c statement_timeout=0 -c lock_timeout=0")],
            None,
        )
        .map(|_| ())
        .map_err(|e| OpsError::Engine(format!("pg_restore ({section}): {}", first_error(&e))))
    };
    // FORCE off as soon as the tables exist (it would hide rows from the
    // owner loading them), and again after post-data, whichever section
    // the dump put it in.
    let lift_force = || -> Result<(), OpsError> {
        let forced = as_mig("SELECT c.oid::pg_catalog.regclass::text FROM pg_catalog.pg_class c WHERE c.relforcerowsecurity ORDER BY 1;")?;
        for t in forced.lines().map(str::trim).filter(|t| !t.is_empty()) {
            as_mig(&format!("ALTER TABLE {t} NO FORCE ROW LEVEL SECURITY;"))?;
        }
        Ok(())
    };
    restore("pre-data").map_err(on("pre-data"))?;
    lift_force().map_err(on("FORCE"))?;
    restore("data").map_err(on("data"))?;
    // Storage's rows before post-data: the App's triggers on them don't exist
    // yet, so nothing fires twice.
    for (t, table, cols) in [(&p.buckets, "storage.buckets", standin::BUCKET_COLUMNS), (&p.objects, "storage.objects", standin::OBJECT_COLUMNS)] {
        if let Some(t) = t {
            for chunk in t.rows.chunks(CHUNK) {
                as_mig(&insert_sql(table, t, chunk, cols, &[])).map_err(|e| ("storage rows", OpsError::Engine(first_error(e.hint()))))?;
            }
        }
    }
    restore("post-data").map_err(on("post-data"))?;
    lift_force().map_err(on("FORCE"))?;
    if !p.acl.statements.is_empty() {
        as_mig(&p.acl.statements.join("\n")).map_err(on("grants"))?;
    }
    // The migration history, renamed.
    ops::ensure_migrations_table(ops, db, mig, pw).map_err(on("the migrations table"))?;
    if !p.mapped.is_empty() {
        let dir = Path::new(o.ws_path).join(MIGRATIONS_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| ("migration files", OpsError::Engine(e.to_string())))?;
        let mut rows = Vec::new();
        for m in &p.mapped {
            std::fs::write(dir.join(format!("{}.sql", m.k2_version)), &m.sql).map_err(|e| ("migration files", OpsError::Engine(e.to_string())))?;
            rows.push(format!("({}, {})", quote_literal(&m.k2_version), quote_literal(&ops::migration_checksum(m.sql.as_bytes()))));
        }
        as_mig(&format!("INSERT INTO _k2_migrations (version, checksum) VALUES {} ON CONFLICT (version) DO NOTHING;", rows.join(", "))).map_err(on("the migrations table"))?;
        let map = serde_json::to_vec_pretty(&migrations::map_json(&p.mapped)).map_err(|e| ("the migration map", OpsError::Engine(e.to_string())))?;
        std::fs::write(Path::new(o.ws_path).join(MAP_FILE), map).map_err(|e| ("the migration map", OpsError::Engine(e.to_string())))?;
    }
    // K2's grants for the workspace's own roles, then the seal.
    ops::privilege_sync_for_row(ops, secrets, row).map_err(on("privilege sync"))?;
    ops::seal_k2_internal_tables(ops, cl, db).map_err(on("privilege sync"))?;
    Ok(())
}

// ── the route ────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Body {
    project: String,
    file: String,
    db: String,
    allow_unresolved: bool,
    dry_run: bool,
}

fn answer(status: &'static str, code: &str, hint: String, report: Option<J>) -> CliResponse {
    let mut v = json!({ "ok": false, "error": { "code": code, "hint": hint } });
    if let Some(r) = report {
        v["report"] = r;
    }
    CliResponse { status, content_type: "application/json", body: v.to_string() }
}

/// `POST /cli/db/import/supabase {project, file, db, allowUnresolved?, dryRun?}`.
pub fn handle(body: &[u8]) -> CliResponse {
    let b: Body = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return super::routes::err_json("400 Bad Request", "usage", format!("invalid JSON body: {e}")),
    };
    if b.file.trim().is_empty() || b.db.trim().is_empty() {
        return super::routes::err_json("400 Bad Request", "usage", format!("{CLI_NAME} db import supabase <dump> --db <name>: name the dump and the database it goes into"));
    }
    let (path, project_id) = match super::identity::resolve_caller(&b.project) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let o = Opts { project_id: &project_id, ws_path: &path, file: b.file.trim(), db: b.db.trim(), allow_unresolved: b.allow_unresolved, dry_run: b.dry_run };
    match import(super::routes::ops(), &FileSecretStore::default(), &o) {
        Ok(v) => super::routes::ok_json(v),
        Err(Refusal::Ops(e)) => super::routes::ops_err(e),
        Err(Refusal::Unresolved(r)) => {
            let n = r["unresolved"].as_array().map_or(0, Vec::len);
            answer(
                "409 Conflict",
                "import_unresolved",
                format!("{n} thing(s) in the dump refer to what this server doesn't carry (listed); nothing was changed. Change them in the source and dump again, or pass --allow-unresolved to restore anyway (objects that need them may then fail)"),
                Some(r),
            )
        }
        Err(Refusal::Failed { step, error, report }) => answer(
            "502 Bad Gateway",
            "import_failed",
            format!("{step}: {error}. The database is half-loaded: drop it ({CLI_NAME} db drop --db {} --yes), make a fresh one and import again", o.db),
            Some(report),
        ),
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod real_pg;
