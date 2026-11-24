//! Role SQL guards and the grantee revoke statement, moved out of ops.rs as
//! they were (0.46.18 queue, ratchet): the refusal of a role statement
//! that would grant SUPERUSER, CREATEDB or BYPASSRLS (and the compat
//! layer's one narrow service_role allow), and the DO block that revokes a
//! grantee's privileges on every migrator-owned schema.

use super::*;

/// [`refuse_dangerous_role_sql`] with one narrow allow: K2's own
/// `service_role` statement ([`k2_root_proto::k2_auth::SERVICE_ROLE_BYPASS_SQL`],
/// NOLOGIN, exact text) may carry BYPASSRLS. Anything else is checked as usual.
/// Stricter than the plain check, which passes any text that also says
/// NOBYPASSRLS somewhere: here every SUPERUSER, CREATEDB and BYPASSRLS in
/// the rest must be the NO… form.
pub(crate) fn refuse_dangerous_role_sql_compat(sql: &str) -> Result<(), OpsError> {
    let rest = sql.replace(k2_root_proto::k2_auth::SERVICE_ROLE_BYPASS_SQL, "");
    let up = rest.to_ascii_uppercase();
    for word in ["SUPERUSER", "CREATEDB", "BYPASSRLS", "CREATEROLE", "REPLICATION"] {
        if up.matches(word).count() > up.matches(&format!("NO{word}")).count() {
            return Err(OpsError::Engine(format!("internal: {} leaked into role SQL", word.to_ascii_lowercase())));
        }
    }
    refuse_dangerous_role_sql(&rest)
}

pub(crate) fn refuse_dangerous_role_sql(sql: &str) -> Result<(), OpsError> {
    let up = sql.to_ascii_uppercase();
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
    Ok(())
}

/// Revoke tables, sequences, and default privileges on every
/// migrator-owned schema. Runs as superuser so FOR ROLE names the
/// migrator (not current_user).
pub(super) fn revoke_grantee_privileges_sql(db_name: &str, role: &str) -> String {
    let migrator = format!("{db_name}_migrator");
    let mig_lit = pg_quote_literal(&migrator);
    let role_lit = pg_quote_literal(role);
    format!(
        "DO $k2revoke$\n\
         DECLARE\n\
           sch text;\n\
         BEGIN\n\
           FOR sch IN\n\
             SELECT n.nspname\n\
             FROM pg_namespace n\n\
             JOIN pg_roles r ON r.oid = n.nspowner\n\
             WHERE (r.rolname = {mig_lit} OR n.nspname = 'public')\n\
               AND n.nspname <> 'pg_catalog'\n\
               AND n.nspname <> 'information_schema'\n\
               AND n.nspname NOT LIKE 'pg_toast%'\n\
               AND n.nspname NOT LIKE 'pg_temp%'\n\
           LOOP\n\
             EXECUTE format('REVOKE ALL ON SCHEMA %I FROM %I', sch, {role_lit});\n\
             EXECUTE format('REVOKE ALL ON ALL TABLES IN SCHEMA %I FROM %I', sch, {role_lit});\n\
             EXECUTE format('REVOKE ALL ON ALL SEQUENCES IN SCHEMA %I FROM %I', sch, {role_lit});\n\
             EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA %I REVOKE ALL ON TABLES FROM %I', {mig_lit}, sch, {role_lit});\n\
             EXECUTE format('ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA %I REVOKE ALL ON SEQUENCES FROM %I', {mig_lit}, sch, {role_lit});\n\
           END LOOP;\n\
         END\n\
         $k2revoke$;"
    )
}
