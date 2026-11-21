//! K2's own tables in a workspace database (`public._k2_*`: `_k2_migrations`,
//! `_k2_store`) keep K2's own grants and nothing else (0.46.17).
//!
//! The privilege sync and `k2 db grant` reach every table in `public`
//! (Postgres 15+ owns `public` as `pg_database_owner`, so the old
//! migrator-owned-schemas walk never reached it), and a default privilege
//! for the migrator in `public` reaches a table K2 makes later. Postgres
//! can't leave a table out of either by name, so after each of them K2
//! resets its own tables: every grant to anyone but the table's owner (the
//! migrator) goes, then the database's agent role gets back exactly what
//! K2's own code uses:
//!
//! - `_k2_store`: SELECT, INSERT, UPDATE, DELETE (`k2 store`, the guest
//!   doors' seal reads it as the agent);
//! - `_k2_migrations`: SELECT only. K2 reads and writes the migration
//!   history as the migrator (`sql::ops::migrate_db`); SELECT lets the
//!   agent's own tools (and an App through the agent's role) read which
//!   version is applied, and can't change it. Before 0.46.17 the agent held
//!   INSERT, UPDATE and DELETE on it too (the create SQL and its repair).
//!
//! No other workspace's role (a read or write grantee) and no App login
//! holds anything on them directly. Run as the migrator (the privilege sync)
//! or the root helper's superuser (create, the repair); either way the
//! grants are recorded with the owner as grantor, so the owner can revoke
//! them.

/// The reset, as one `DO` block. `agent`: the database's agent role,
/// unquoted (`<db>_agent`).
pub(crate) fn fence_sql(agent: &str) -> String {
    let agent_lit = crate::sql::ops::pg_quote_literal(agent);
    format!(
        "DO $k2fence$\n\
         DECLARE\n\
           r record;\n\
         BEGIN\n\
           FOR r IN\n\
             SELECT DISTINCT c.oid::regclass::text AS rel, a.grantee\n\
             FROM pg_catalog.pg_class c\n\
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace\n\
             CROSS JOIN LATERAL pg_catalog.aclexplode(c.relacl) a\n\
             WHERE n.nspname = 'public' AND c.relname LIKE '\\_k2\\_%' AND a.grantee <> c.relowner\n\
           LOOP\n\
             EXECUTE format('REVOKE ALL ON TABLE %s FROM %s', r.rel,\n\
               CASE WHEN r.grantee = 0 THEN 'PUBLIC' ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(r.grantee)) END);\n\
           END LOOP;\n\
           IF pg_catalog.to_regclass('public._k2_store') IS NOT NULL THEN\n\
             EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public._k2_store TO %I', {agent_lit});\n\
           END IF;\n\
           IF pg_catalog.to_regclass('public._k2_migrations') IS NOT NULL THEN\n\
             EXECUTE format('GRANT SELECT ON TABLE public._k2_migrations TO %I', {agent_lit});\n\
           END IF;\n\
         END\n\
         $k2fence$;"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_fence_resets_k2s_own_tables_and_gives_the_agent_only_what_k2_uses() {
        let sql = super::fence_sql("ws_a_agent");
        assert!(sql.contains("c.relname LIKE '\\_k2\\_%'"), "{sql}");
        assert!(sql.contains("a.grantee <> c.relowner"), "the owner (migrator) keeps its own: {sql}");
        assert!(sql.contains("REVOKE ALL ON TABLE %s FROM %s"), "{sql}");
        assert!(sql.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public._k2_store TO %I', 'ws_a_agent'"), "{sql}");
        assert!(sql.contains("GRANT SELECT ON TABLE public._k2_migrations TO %I', 'ws_a_agent'"), "{sql}");
        assert!(!sql.contains("INSERT, UPDATE, DELETE ON TABLE public._k2_migrations"), "never write on the history: {sql}");
        // A quote in the role name can't break out of the literal.
        assert!(super::fence_sql("x'; DROP TABLE y; --").contains("'x''; DROP TABLE y; --'"));
    }
}
