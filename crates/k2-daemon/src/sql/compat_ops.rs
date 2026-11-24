//! Postgres work for the Supabase-compatible layer (prd-app-supabase-compat-v1
//! §5, §9.3; lane A): the standard roles, the door's login, the `auth`
//! schema and the claims key on one App database, and the daemon's writes
//! of the App-user projection (`auth.users`). The canned SQL is
//! [`k2_root_proto::k2_auth`]; this file only sends it.
//!
//! Superuser work goes through the root helper's existing `pg psql` verb
//! (no new verb). `service_role` is the one BYPASSRLS role K2 makes, and
//! the role-SQL check allows exactly its statement
//! ([`super::ops::refuse_dangerous_role_sql_compat`]).

use k2_root_proto::k2_auth;

use super::ops::{self, OpsError};
use super::secrets::{generate_secret, SecretStore};
use super::sysops::SystemOps;
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};

/// The door's login: at most this many connections (lane C's pool is 10;
/// room for a restart overlap and the daemon's own sync).
pub const AUTHENTICATOR_CONN_LIMIT: u32 = 30;

/// A database this workspace owns, by name (active only).
pub(crate) fn owned_db(project_id: &str, name: &str) -> Result<(String, String), OpsError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = ops::load_active_by_name(&conn, project_id, name)
        .ok_or_else(|| OpsError::NotFound(format!("no database {name} in the App's workspace ({CLI_NAME} db list)")))?;
    Ok((row.id.clone(), row.name.clone()))
}

/// What turning the layer on made or kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnResult {
    pub authenticator: String,
    pub authenticator_secret_ref: String,
    pub claims_key_ref: String,
    /// Memberships of the standard roles that weren't K2's, revoked
    /// (`member>role`, comma-separated; empty when none).
    pub stripped: String,
}

fn su(ops: &dyn SystemOps, db: &str, sql: &str) -> Result<String, OpsError> {
    let cl = ops::cluster_of(db)?;
    ops::su_psql(ops, &cl, db, &["-v", "ON_ERROR_STOP=1", "-tA"], Some(sql.as_bytes()))
}

/// Cluster-wide SQL (roles) in database `postgres` of the cluster `db`
/// lives on (never the box's primary by default).
fn su_cluster(ops: &dyn SystemOps, db: &str, sql: &str) -> Result<String, OpsError> {
    let cl = ops::cluster_of(db)?;
    ops::su_psql(ops, &cl, "postgres", &["-v", "ON_ERROR_STOP=1", "-tA"], Some(sql.as_bytes()))
}

/// Schema `auth` here: `none`, K2's (`k2`) or someone else's (`other`).
pub(crate) fn auth_schema_owner(ops: &dyn SystemOps, db: &str) -> Result<String, OpsError> {
    Ok(su(ops, db, k2_auth::AUTH_SCHEMA_OWNER_QUERY)?.lines().last().unwrap_or("").trim().to_string())
}

/// Turn the layer on for `db` (idempotent). `existing`: the refs a
/// previous turn-on kept (the same password and key again). Order: refuse
/// a foreign `auth` schema before anything changes; the cluster's standard
/// roles; the door's login and the CONNECT wall; the `auth` schema; the
/// claims key.
/// `others`: the other compat databases of the box (their door logins and
/// migrators keep their memberships of the standard roles).
pub(crate) fn turn_on(ops: &dyn SystemOps, secrets: &dyn SecretStore, db: &str, existing: Option<(&str, &str)>, others: &[String]) -> Result<OnResult, OpsError> {
    ops::require_running()?;
    ops::refuse_if_moving(db)?;
    let login = k2_auth::authenticator_role(db).ok_or_else(|| OpsError::Usage(format!("{db} can't have a door login (name)")))?;
    match auth_schema_owner(ops, db)?.as_str() {
        "none" | "k2" => {}
        other => {
            return Err(OpsError::Refused(
                "compat_auth_schema_taken",
                format!(
                    "{db} already has a schema named auth that {PRODUCT_NAME} didn't make ({other}): rename or drop it first \
                     (a Supabase dump: {CLI_NAME} db import supabase maps it)"
                ),
            ))
        }
    }
    // Memberships first (a pre-existing service_role's members must not
    // inherit BYPASSRLS), then K2's attributes, in one session.
    let mut authenticators = vec![login.clone()];
    let mut migrators = vec![format!("{db}_migrator")];
    for o in others.iter().filter(|o| o.as_str() != db) {
        authenticators.extend(k2_auth::authenticator_role(o));
        migrators.push(format!("{o}_migrator"));
    }
    let strip = k2_auth::standard_roles_strip_sql(&authenticators, &migrators).ok_or_else(|| OpsError::Usage(format!("{db}: not a role name")))?;
    let roles = format!("{strip}{}", k2_auth::standard_roles_sql());
    ops::refuse_dangerous_role_sql_compat(&roles)?;
    let out = su_cluster(ops, db, &roles)?;
    let stripped = out.lines().find_map(|l| l.trim().strip_prefix("k2-stripped:")).unwrap_or("").to_string();

    let resolve = |sref: &str| -> Result<Option<String>, OpsError> { secrets.resolve(sref).map_err(OpsError::Engine) };
    let (pw, pw_ref) = match existing.map(|(p, _)| p).map(|r| (r, resolve(r))) {
        Some((r, Ok(Some(pw)))) => (pw, r.to_string()),
        _ => {
            let pw = generate_secret().map_err(OpsError::Engine)?;
            let r = secrets.store("authenticator", &pw).map_err(OpsError::Engine)?;
            (pw, r)
        }
    };
    let login_sql = k2_auth::authenticator_sql(db, &login, &pw, AUTHENTICATOR_CONN_LIMIT)
        .ok_or_else(|| OpsError::Usage(format!("{db}: not a role name")))?;
    ops::refuse_dangerous_role_sql(&login_sql)?;
    su_cluster(ops, db, &login_sql)?;

    let schema = k2_auth::auth_schema_sql(db, &login).ok_or_else(|| OpsError::Usage(format!("{db}: not a role name")))?;
    su(ops, db, &schema)?;

    let (key, key_ref) = match existing.map(|(_, k)| k).map(|r| (r, resolve(r))) {
        Some((r, Ok(Some(k)))) => (k, r.to_string()),
        _ => {
            let k = generate_secret().map_err(OpsError::Engine)?;
            let r = secrets.store("claims-key", &k).map_err(OpsError::Engine)?;
            (k, r)
        }
    };
    let key_sql = k2_auth::claims_key_sql(&key).ok_or_else(|| OpsError::Engine("claims key is not hex".into()))?;
    // As the key's owner, never the superuser: a trigger the App put on
    // the table must not run with superuser rights.
    let row = ops::active_row_by_name(db)?;
    let (mig_user, mig_pw) = ops::migrator_creds(secrets, &row)?;
    ops::exec_as(ops, db, &mig_user, &mig_pw, &key_sql)?;
    Ok(OnResult { authenticator: login, authenticator_secret_ref: pw_ref, claims_key_ref: key_ref, stripped })
}

/// Turn the door's login off (the `auth` schema, `auth.users` and the
/// CONNECT wall stay: the App's foreign keys point at `auth.users`).
pub(crate) fn turn_off(ops: &dyn SystemOps, db: &str, login: &str) -> Result<(), OpsError> {
    let sql = k2_auth::authenticator_off_sql(login).ok_or_else(|| OpsError::Usage(format!("{login}: not a role name")))?;
    su_cluster(ops, db, &sql).map(|_| ())
}

/// Other compat databases in `db`'s cluster (the CLI warns: role names are
/// shared per cluster; a moved App should get its own).
pub(crate) fn neighbours_in_cluster(db: &str, compat_dbs: &[String]) -> Vec<String> {
    let Ok(mine) = ops::cluster_of(db) else { return Vec::new() };
    compat_dbs
        .iter()
        .filter(|d| d.as_str() != db)
        .filter(|d| ops::cluster_of(d).is_ok_and(|c| c.port == mine.port))
        .cloned()
        .collect()
}

/// What the standard roles can reach in the exposed schemas (for the
/// turn-on and anon warnings, §5.5): tables and views `anon` can read,
/// those it can write, and tables without row security that `anon` or
/// `authenticated` can read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Exposure {
    pub anon_read: u64,
    pub anon_write: u64,
    pub no_rls_readable: u64,
}

pub(crate) fn exposure(ops: &dyn SystemOps, db: &str, schemas: &[String]) -> Result<Exposure, OpsError> {
    let list = schemas.iter().map(|s| ops::pg_quote_literal(s)).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT count(*) FILTER (WHERE has_table_privilege('anon', c.oid, 'SELECT')) || '|' || \
           count(*) FILTER (WHERE has_table_privilege('anon', c.oid, 'INSERT,UPDATE,DELETE')) || '|' || \
           count(*) FILTER (WHERE c.relkind IN ('r', 'p') AND NOT c.relrowsecurity \
             AND (has_table_privilege('anon', c.oid, 'SELECT') OR has_table_privilege('authenticated', c.oid, 'SELECT'))) \
         FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r', 'p', 'v', 'm') AND n.nspname IN ({list});"
    );
    let out = su(ops, db, &sql)?;
    let n: Vec<u64> = out.lines().last().unwrap_or("").split('|').filter_map(|x| x.trim().parse().ok()).collect();
    match n.as_slice() {
        [r, w, x] => Ok(Exposure { anon_read: *r, anon_write: *w, no_rls_readable: *x }),
        _ => Err(OpsError::Engine(format!("exposure: unexpected answer {out:?}"))),
    }
}

/// The counts one projection write made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct UsersWrite {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
}

impl UsersWrite {
    pub fn changed(&self) -> bool {
        self.inserted + self.updated + self.deleted > 0
    }
}

/// The SQL that makes `auth.users` equal `rows` (a JSON array of
/// `apps::compat::users_sync::UserRow`: times in unix seconds).
/// `full`: rows not listed are deleted (the App's `ON DELETE` rules fire);
/// else only the listed ids are written. Prints `inserted|updated|deleted`.
pub(crate) fn users_write_sql(rows_json: &str, full: bool, delete_ids: &[String]) -> String {
    let rows = ops::pg_quote_literal(rows_json);
    let del_ids = ops::pg_quote_literal(&serde_json::to_string(delete_ids).unwrap_or_else(|_| "[]".into()));
    let del = if full {
        "DELETE FROM auth.users u WHERE NOT EXISTS (SELECT 1 FROM want w WHERE w.id = u.id) RETURNING 1".to_string()
    } else {
        format!("DELETE FROM auth.users u WHERE u.id IN (SELECT (x #>> '{{}}')::uuid FROM jsonb_array_elements({del_ids}::jsonb) x) RETURNING 1")
    };
    format!(
        "WITH want AS (\n\
           SELECT * FROM jsonb_to_recordset({rows}::jsonb) AS x(\n\
             id uuid, email text, raw_app_meta_data jsonb, raw_user_meta_data jsonb, created_at bigint,\n\
             last_sign_in_at bigint, email_confirmed boolean, banned boolean)\n\
         ), del AS (\n\
           {del}\n\
         ), up AS (\n\
           INSERT INTO auth.users AS u (id, email, raw_app_meta_data, raw_user_meta_data, created_at, updated_at,\n\
               last_sign_in_at, email_confirmed_at, banned_until)\n\
           SELECT id, email, coalesce(raw_app_meta_data, '{{}}'), coalesce(raw_user_meta_data, '{{}}'), to_timestamp(created_at), now(),\n\
               to_timestamp(last_sign_in_at), CASE WHEN email_confirmed THEN to_timestamp(created_at) END,\n\
               CASE WHEN banned THEN 'infinity'::timestamptz END FROM want\n\
           ON CONFLICT (id) DO UPDATE SET email = excluded.email, raw_app_meta_data = excluded.raw_app_meta_data,\n\
               raw_user_meta_data = excluded.raw_user_meta_data, created_at = excluded.created_at, updated_at = now(),\n\
               last_sign_in_at = excluded.last_sign_in_at, email_confirmed_at = excluded.email_confirmed_at,\n\
               banned_until = excluded.banned_until\n\
             WHERE (u.email, u.raw_app_meta_data, u.raw_user_meta_data, u.created_at, u.last_sign_in_at,\n\
                    u.email_confirmed_at, u.banned_until)\n\
               IS DISTINCT FROM (excluded.email, excluded.raw_app_meta_data, excluded.raw_user_meta_data, excluded.created_at,\n\
                    excluded.last_sign_in_at, excluded.email_confirmed_at, excluded.banned_until)\n\
           RETURNING (xmax = 0) AS inserted\n\
         )\n\
         SELECT (SELECT count(*) FROM up WHERE inserted) || '|' || (SELECT count(*) FROM up WHERE NOT inserted) || '|' || (SELECT count(*) FROM del);"
    )
}

/// Write the projection as the database's migrator (it owns `auth.users`;
/// no App role can write it).
pub(crate) fn write_users(ops: &dyn SystemOps, secrets: &dyn SecretStore, project_id: &str, db: &str, rows_json: &str, full: bool, delete_ids: &[String]) -> Result<UsersWrite, OpsError> {
    let row = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        ops::load_active_by_name(&conn, project_id, db).ok_or_else(|| OpsError::NotFound(format!("no database {db}")))?
    };
    let (user, pw) = ops::migrator_creds(secrets, &row)?;
    let out = ops::exec_as(ops, db, &user, &pw, &users_write_sql(rows_json, full, delete_ids))?;
    let line = out.lines().last().unwrap_or("").trim();
    let n: Vec<u64> = line.split('|').filter_map(|x| x.trim().parse().ok()).collect();
    match n.as_slice() {
        [i, u, d] => Ok(UsersWrite { inserted: *i, updated: *u, deleted: *d }),
        _ => Err(OpsError::Engine(format!("auth.users write: unexpected answer {line:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_write_is_one_statement_with_quoted_rows() {
        let s = users_write_sql(r#"[{"id":"00000000-0000-0000-0000-000000000001","email":"o'neil@example.com"}]"#, true, &[]);
        assert!(s.contains("o''neil@example.com"), "{s}");
        assert!(s.contains("DELETE FROM auth.users u WHERE NOT EXISTS"), "{s}");
        assert!(!s.contains("{{") && !s.contains("}}"), "{s}");
        let p = users_write_sql("[]", false, &["00000000-0000-0000-0000-000000000002".into()]);
        assert!(p.contains("WHERE u.id IN (SELECT (x #>> '{}')::uuid"), "{p}");
        assert!(p.contains("00000000-0000-0000-0000-000000000002"), "{p}");
        assert!(!p.contains("NOT EXISTS (SELECT 1 FROM want"), "a partial write deletes only what it names: {p}");
    }

    #[test]
    fn only_k2s_service_role_statement_may_bypass_rls() {
        let roles = k2_auth::standard_roles_sql();
        assert!(ops::refuse_dangerous_role_sql(k2_auth::SERVICE_ROLE_BYPASS_SQL).is_err(), "the plain check still refuses BYPASSRLS");
        assert!(ops::refuse_dangerous_role_sql_compat(&roles).is_ok());
        for bad in [
            &format!("{roles}\nALTER ROLE x BYPASSRLS;") as &str,
            &format!("{roles}\nCREATE ROLE y LOGIN SUPERUSER;"),
            &format!("{roles}\nALTER ROLE z CREATEROLE;"),
            "ALTER ROLE service_role WITH LOGIN BYPASSRLS;",
            "ALTER ROLE anon WITH NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION BYPASSRLS;",
            &format!("{}\nALTER ROLE x BYPASSRLS;", k2_auth::SERVICE_ROLE_BYPASS_SQL),
            &format!("{} SUPERUSER", k2_auth::SERVICE_ROLE_BYPASS_SQL),
        ] {
            assert!(ops::refuse_dangerous_role_sql_compat(bad).is_err(), "{bad}");
        }
    }
}
