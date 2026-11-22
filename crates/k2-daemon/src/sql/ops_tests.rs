use super::*;

/// 0.46.4 drill: `k2 db drop` takes the database's own two roles with
/// it (never a bind role or a grantee); one still holding something stays.
#[test]
fn a_dropped_databases_own_roles_go_too() {
    let sql = drop_database_roles_sql("ws_jobs");
    assert_eq!(sql.matches("DROP ROLE IF EXISTS").count(), 2, "{sql}");
    assert!(sql.contains("DROP ROLE IF EXISTS \"ws_jobs_migrator\";") && sql.contains("DROP ROLE IF EXISTS \"ws_jobs_agent\";"), "{sql}");
    assert_eq!(sql.matches("EXCEPTION WHEN dependent_objects_still_exist").count(), 2);
    assert!(!sql.contains("_bind"), "{sql}");
    refuse_dangerous_role_sql(&sql).unwrap();
}

#[test]
fn seal_sql_puts_row_security_on_both_k2_tables_for_guest_principals() {
    let sql = seal_k2_internal_tables_sql("ws_abc");
    for t in K2_INTERNAL_TABLES {
        assert!(sql.contains(&format!("'{t}'")), "{t}: {sql}");
    }
    assert!(sql.contains("ENABLE ROW LEVEL SECURITY"), "{sql}");
    assert!(!sql.contains("FORCE ROW LEVEL SECURITY"), "owner (migrator) keeps access: {sql}");
    assert!(sql.contains("CREATE POLICY k2_internal_no_guest"), "{sql}");
    assert!(sql.contains("FOR ALL TO PUBLIC"), "{sql}");
    assert!(
        sql.contains("USING (k2.internal_table_ok()) WITH CHECK (k2.internal_table_ok())"),
        "{sql}"
    );
    assert!(sql.contains("IF k2.skin_uid() IS NOT NULL THEN"), "{sql}");
    assert!(sql.contains("ERRCODE = 'insufficient_privilege'"), "{sql}");
    assert!(sql.contains("OWNER TO \"ws_abc_migrator\""), "{sql}");
    assert!(sql.contains("IS NULL THEN\nCONTINUE"), "missing tables are skipped: {sql}");
    assert!(!sql.to_ascii_uppercase().contains("CREATE ROLE"), "{sql}");
    assert!(!sql.to_ascii_uppercase().contains("REVOKE"), "agent keeps its grants: {sql}");
}

#[test]
fn exec_as_role_with_set_role_sends_the_statement_as_c_not_stdin() {
    let (args, stdin) = role_psql_args("5433", "db", "u", Some("SET ROLE \"r\""), "SELECT 1");
    assert!(args.join(" ").starts_with("-h 127.0.0.1 -p 5433 -U u -d db "), "the resolver's port: {args:?}");
    assert!(stdin.is_none(), "psql never reads stdin once -c is set");
    let joined = args.join(" ");
    let role = joined.find("-c SET ROLE").expect("set role");
    let stmt = joined.find("-c SELECT 1").expect("statement as -c");
    assert!(role < stmt, "{joined}");
    assert!(args.contains(&"-q"), "{joined}");
    let (args, stdin) = role_psql_args("5432", "db", "u", None, "SELECT 1");
    assert_eq!(stdin, Some("SELECT 1".as_bytes()));
    assert!(!args.contains(&"-c"), "{args:?}");
}

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

#[test]
fn dump_table_ident_rejects_catalog_and_k2_prefix() {
    assert_eq!(validate_dump_table("Example").expect("ok"), "example");
    assert_eq!(validate_dump_table("notes_1").expect("ok"), "notes_1");
    for bad in [
        "",
        "1bad",
        "has-dash",
        "has space",
        "has.dot",
        "_k2_store",
        "_K2_store",
        "pg_class",
        "PG_roles",
        "information_schema",
        "INFORMATION_SCHEMA",
    ] {
        let err = validate_dump_table(bad).expect_err(bad);
        let hint = err.hint();
        if bad.is_empty() {
            assert_eq!(hint, "missing table", "{bad}");
        } else {
            assert_eq!(hint, "invalid table name", "{bad} → {hint}");
        }
    }
}

#[test]
fn dump_json_to_sql_typed_binds() {
    assert_eq!(json_to_sql(&serde_json::Value::Null), "NULL");
    assert_eq!(json_to_sql(&serde_json::json!(true)), "TRUE");
    assert_eq!(json_to_sql(&serde_json::json!(false)), "FALSE");
    assert_eq!(json_to_sql(&serde_json::json!(3)), "3");
    assert_eq!(json_to_sql(&serde_json::json!(1.5)), "1.5");
    assert_eq!(json_to_sql(&serde_json::json!("o'reilly")), "'o''reilly'");
    let obj = json_to_sql(&serde_json::json!({"a": 1}));
    assert!(obj.ends_with("::jsonb"), "{obj}");
    assert!(obj.starts_with("'"), "{obj}");
    let arr = json_to_sql(&serde_json::json!([1, 2]));
    assert!(arr.ends_with("::jsonb"), "{arr}");
}

#[test]
fn strip_sql_comments_removes_line_and_block() {
    let s = strip_sql_comments(
        "-- FORCE ROW LEVEL SECURITY\nCREATE TABLE t (id int);\n/* CREATE ROLE app_x */\n",
    );
    let up = s.to_ascii_uppercase();
    assert!(!up.contains("FORCE ROW LEVEL"), "{s}");
    assert!(!up.contains("CREATE ROLE"), "{s}");
    assert!(up.contains("CREATE TABLE T"), "{s}");
}

#[test]
fn refuse_user_migration_sql_comments_only_ok_executable_usage() {
    refuse_user_migration_sql(
        "-- ALTER TABLE t FORCE ROW LEVEL SECURITY;\nCREATE TABLE t (id int);\n",
    )
    .expect("comments-only FORCE");
    refuse_user_migration_sql("/* CREATE ROLE app_x */\nCREATE TABLE t (id int);\n")
        .expect("comment CREATE ROLE");
    let err = refuse_user_migration_sql("ALTER TABLE t FORCE ROW LEVEL SECURITY;\n")
        .expect_err("executable FORCE");
    assert_eq!(err.code(), "usage", "{}", err.hint());
    let err = refuse_user_migration_sql("CREATE ROLE app_x;\n").expect_err("CREATE ROLE");
    assert_eq!(err.code(), "usage", "{}", err.hint());
    assert!(err.hint().contains("cluster-wide"), "{}", err.hint());
    let err = refuse_user_migration_sql("CREATE USER app_x;\n").expect_err("CREATE USER");
    assert_eq!(err.code(), "usage", "{}", err.hint());
    let err = refuse_user_migration_sql("$tag$ CREATE ROLE inside $tag$;\n")
        .expect_err("dollar-quote leftover is fail-closed");
    assert_eq!(err.code(), "usage", "{}", err.hint());
}

#[test]
fn helpers_include_the_actor_helpers_and_owner_sql_covers_both_definers() {
    let sql = ensure_k2_helpers_sql();
    let schema = sql.find("CREATE SCHEMA IF NOT EXISTS k2").expect("schema");
    let actor = sql.find("FUNCTION k2.actor_info()").expect("actor helper");
    assert!(schema < actor, "schema first: {sql}");
    assert_eq!(
        sql.matches("SECURITY DEFINER").count(),
        2,
        "only skin_uid and actor_info are definers: {sql}"
    );
    let owner = k2_helper_owner_sql("ws_abc");
    assert_eq!(
        owner,
        "ALTER FUNCTION k2.skin_uid() OWNER TO \"ws_abc_migrator\";\n\
             ALTER FUNCTION k2.actor_info() OWNER TO \"ws_abc_migrator\";"
    );
}

#[test]
fn guest_lock_sql_locks_principal_then_actor_and_grants_only_the_principal_read() {
    let actor = GuestActor::session("1b4e28ba-2fa1-11d2-883f-0016d3cca427", "pass-1");
    let sql = guest_lock_sql(&actor, "app_reader");
    let p = sql.find("INSERT INTO pg_temp.k2_principal_lock").expect("principal");
    let a = sql.find("INSERT INTO pg_temp.k2_actor_lock").expect("actor");
    assert!(p < a, "{sql}");
    assert_eq!(sql.matches("GRANT").count(), 1, "{sql}");
    assert!(sql.contains("GRANT SELECT ON TABLE pg_temp.k2_principal_lock TO \"app_reader\""), "{sql}");
}

#[test]
fn guest_conn_refuses_a_reserved_or_mismatched_actor() {
    let ops = super::super::sysops::FakeSystemOps::baked();
    let secrets = super::super::secrets::MemSecretStore::default();
    let uid = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
    let mut forged = GuestActor::session(uid, "p");
    forged.actor = "guest:00000000-0000-0000-0000-000000000000".into();
    let err = guest_conn(&ops, &secrets, "no-project", &forged).err().expect("mismatch");
    assert_eq!(err.hint(), "internal: unsupported actor");
    let mut agent = GuestActor::session(uid, "p");
    agent.kind = super::super::actor::ACTOR_KIND_AGENT;
    agent.actor = format!("agent:{uid}");
    let err = guest_conn(&ops, &secrets, "no-project", &agent)
        .err()
        .expect("agent kind with no marked workspace");
    assert_eq!(err.hint(), "internal: unsupported actor");
    let mut delegated = GuestActor::marked_agent(
        uid,
        "p",
        "6f9619ff-8b86-4011-b42d-00c04fc964ff",
        Some("sales"),
    );
    delegated.on_behalf_of = Some(format!("guest:{uid}"));
    let err = guest_conn(&ops, &secrets, "no-project", &delegated)
        .err()
        .expect("S43 delegated shape is not built");
    assert_eq!(err.hint(), "internal: unsupported actor");
}

#[test]
fn ensure_k2_helpers_sql_is_set_local_uuid_no_create_role() {
    let sql = ensure_k2_helpers_sql();
    let up = sql.to_ascii_uppercase();
    assert!(!up.contains("CREATE ROLE"), "{sql}");
    assert!(!up.contains("FORCE ROW LEVEL"), "{sql}");
    assert!(sql.contains("k2.set_principal"), "{sql}");
    assert!(sql.contains("k2.clear_principal"), "{sql}");
    assert!(sql.contains("k2.principal_hygiene"), "{sql}");
    assert!(sql.contains("k2.skin_uid"), "{sql}");
    assert!(sql.contains("set_config('k2.skin_principal'"), "{sql}");
    assert!(
        sql.contains("set_config('k2.skin_principal', COALESCE(p::text, ''), true)"),
        "set_principal must be SET LOCAL: {sql}"
    );
    assert!(sql.contains("p uuid"), "{sql}");
    assert!(sql.contains("GRANT USAGE ON SCHEMA k2 TO PUBLIC"), "{sql}");
    assert!(
        sql.contains("GRANT EXECUTE ON FUNCTION k2.set_principal(uuid) TO PUBLIC"),
        "{sql}"
    );
    assert!(sql.contains("SECURITY DEFINER"), "{sql}");
    assert!(sql.contains("pg_temp.k2_principal_lock"), "{sql}");
    assert!(sql.contains("undefined_table"), "{sql}");
    assert!(
        sql.contains("SET search_path = pg_catalog, pg_temp"),
        "{sql}"
    );
    // The lock counts only when the definer (the migrator) owns it.
    let skin = &sql[sql.find("FUNCTION k2.skin_uid()").expect("skin_uid")..sql.find("FUNCTION k2.set_principal").expect("set_principal")];
    assert!(
        skin.contains("c.relowner = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)"),
        "skin_uid must check the lock's owner: {skin}"
    );
    assert!(skin.contains("c.relpersistence = 't'"), "{skin}");
    assert!(!sql.contains("k2.lock_principal"), "{sql}");
    assert!(
        sql.contains("nullif(current_setting('k2.skin_principal', true), '')::uuid"),
        "{sql}"
    );
}

#[test]
fn privilege_sync_sql_walks_schemas_write_and_read() {
    let sql = privilege_sync_sql(
        "ws_docs",
        &[
            SyncGrant {
                role: "ws_sales_agent".into(),
                level: "write".into(),
                relations: vec![],
            },
            SyncGrant {
                role: "ws_read_agent".into(),
                level: "read".into(),
                relations: vec![],
            },
        ],
    );
    let up = sql.to_ascii_uppercase();
    assert!(up.contains("PG_NAMESPACE"), "{sql}");
    assert!(up.contains("PG_CATALOG"), "{sql}");
    assert!(up.contains("INFORMATION_SCHEMA"), "{sql}");
    assert!(up.contains("PG_TOAST"), "{sql}");
    assert!(up.contains("PG_TEMP"), "{sql}");
    assert!(up.contains("GRANT USAGE ON SCHEMA"), "{sql}");
    assert!(
        up.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES"),
        "{sql}"
    );
    assert!(
        up.contains("GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES"),
        "{sql}"
    );
    assert!(up.contains("ALTER DEFAULT PRIVILEGES FOR ROLE"), "{sql}");
    assert!(up.contains("GRANT SELECT ON ALL TABLES"), "{sql}");
    assert!(up.contains("GRANT SELECT ON ALL SEQUENCES"), "{sql}");
    assert!(!up.contains("GRANT CREATE ON SCHEMA"), "{sql}");
    assert!(!up.contains("WS_*_AGENT"), "{sql}");
    for stmt in up.split(';') {
        let grant_on_sequences = stmt.contains("GRANT") && stmt.contains("ON ALL SEQUENCES");
        assert!(
            !grant_on_sequences || (!stmt.contains("INSERT") && !stmt.contains("DELETE")),
            "sequence GRANT must not include INSERT/DELETE: {stmt}"
        );
    }
    assert!(sql.contains("ws_docs_agent"), "{sql}");
    assert!(sql.contains("ws_sales_agent"), "{sql}");
    assert!(sql.contains("ws_read_agent"), "{sql}");
}

#[test]
fn relation_names_fold_public_and_reject_reserved() {
    let ok = normalize_relations(&[
        "Notes".into(),
        "public.Notes".into(),
        "App.Tasks".into(),
    ])
    .expect("fold");
    assert_eq!(
        ok,
        vec![
            ("public".into(), "notes".into()),
            ("app".into(), "tasks".into()),
        ]
    );
    for bad in [
        "pg_stat",
        "information_schema.t",
        "_k2_migrations",
        "9no",
        "a.b.c",
        "",
        "public.",
    ] {
        assert!(
            normalize_relations(&[bad.into()]).is_err(),
            "{bad} must be rejected before SQL"
        );
    }
}

#[test]
fn privilege_sync_sql_listed_is_select_not_all_tables() {
    let sql = privilege_sync_sql(
        "ws_docs",
        &[SyncGrant {
            role: "ws_patty_agent".into(),
            level: "read".into(),
            relations: vec![
                ("public".into(), "notes".into()),
                ("app".into(), "tasks".into()),
            ],
        }],
    );
    assert!(sql.contains("GRANT SELECT ON TABLE"), "{sql}");
    assert!(sql.contains("'notes'"), "{sql}");
    assert!(sql.contains("'tasks'"), "{sql}");
    assert!(sql.contains("REVOKE ALL ON ALL TABLES"), "{sql}");
    assert!(sql.contains("REVOKE ALL ON SEQUENCES"), "{sql}");
    for stmt in sql.split(';') {
        if stmt.contains("'ws_patty_agent'") && stmt.to_ascii_uppercase().contains("GRANT") {
            assert!(
                !stmt.to_ascii_uppercase().contains("ON ALL TABLES"),
                "listed grantee must not receive ALL TABLES: {stmt}"
            );
        }
    }
    assert!(
        sql.contains("ws_docs_agent")
            && sql.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES"),
        "owner agent stays broad: {sql}"
    );
    let write = privilege_sync_sql(
        "ws_docs",
        &[SyncGrant {
            role: "ws_patty_agent".into(),
            level: "write".into(),
            relations: vec![("public".into(), "invoices".into())],
        }],
    );
    assert!(
        write.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE"),
        "{write}"
    );
    assert!(write.contains("deptype IN ('a', 'i')"), "{write}");
    assert!(
        write.contains("GRANT USAGE, SELECT, UPDATE ON SEQUENCE"),
        "{write}"
    );
    for stmt in write.split(';') {
        let up = stmt.to_ascii_uppercase();
        if stmt.contains("'ws_patty_agent'")
            && up.contains("GRANT")
            && up.contains("ON ALL SEQUENCES")
        {
            panic!("write list must not grant every sequence: {stmt}");
        }
    }
}

#[test]
fn refuse_user_migration_does_not_scan_ensure_role_sql() {
    let role = ensure_role_sql("ws_docs_agent", "secret");
    assert!(role.to_ascii_uppercase().contains("CREATE ROLE"), "{role}");
    refuse_user_migration_sql("-- platform CREATE ROLE lives elsewhere\nSELECT 1;\n")
        .expect("comment mentioning CREATE ROLE applies");
}
