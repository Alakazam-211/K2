//! Workspace data sidecar — supervised Postgres (prd-workspace-data-sidecar-v1).
//!
//! The daemon does not *be* Postgres; it supervises a distro unit. Linux-only
//! at runtime (`sql_supported`); compiles + unit-tests everywhere via
//! [`sysops::SystemOps`] fakes. Never vendors, patches, or links libpq server.

pub mod identity;
pub mod ops;
pub mod paths;
pub mod routes;
pub mod secrets;
pub mod supervisor;
pub mod sysops;

/// Optional `skin_*` dump template (not auto-applied by create_database).
/// Commented `CREATE ROLE skin_<id> NOLOGIN` + POLICY; no FORCE RLS.
#[allow(dead_code)]
pub const SKIN_RLS_TEMPLATE: &str = include_str!("skin_rls_template.sql");

/// Help/doctor pointer — copy into `.k2/db/migrations` only if wanted.
pub const SKIN_RLS_TEMPLATE_HINT: &str = "crates/k2-daemon/src/sql/skin_rls_template.sql — commented CREATE ROLE skin_<id> NOLOGIN + POLICY; no FORCE RLS; not auto-applied; no SET ROLE.";

/// The single capability gate for the SQL sidecar (D4): V1 runs on
/// **Linux deployments only**. RUNTIME `cfg!`, not a compile-time
/// `#[cfg]` on the module — compiles + unit-tests on macOS.
pub fn sql_supported() -> bool {
    cfg!(target_os = "linux")
}

/// Serializes tests that touch the SINGLETON `sql_server` row.
#[cfg(test)]
pub(crate) fn sql_server_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller_workspace::with_request_principal;
    use crate::session_token::HookPrincipal;
    use crate::sql::secrets::{MemSecretStore, SecretStore};
    use crate::sql::sysops::FakeSystemOps;
    use crate::sql_routes;

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project");
        id
    }

    #[test]
    fn sql_supported_matches_target_os() {
        assert_eq!(sql_supported(), cfg!(target_os = "linux"));
    }

    #[test]
    fn skin_rls_template_is_commented_nologin_policy_without_force() {
        let t = SKIN_RLS_TEMPLATE;
        let up = t.to_ascii_uppercase();
        assert!(t.contains("CREATE ROLE skin_<id> NOLOGIN"), "{t}");
        assert!(up.contains("NOLOGIN"), "{t}");
        assert!(up.contains("CREATE POLICY"), "{t}");
        assert!(t.contains("principal_id"), "{t}");
        assert!(
            !up.contains("FORCE ROW LEVEL"),
            "template must not contain FORCE ROW LEVEL (migrate refuses that substring even in comments): {t}"
        );
        assert!(t.contains("Do not FORCE RLS"), "{t}");
        assert!(
            SKIN_RLS_TEMPLATE_HINT.contains("skin_rls_template.sql"),
            "{SKIN_RLS_TEMPLATE_HINT}"
        );
        assert!(
            SKIN_RLS_TEMPLATE_HINT.contains("no FORCE RLS"),
            "{SKIN_RLS_TEMPLATE_HINT}"
        );
    }

    #[test]
    fn get_create_is_405() {
        let r = sql_routes::dispatch("/cli/db/create", &Default::default()).expect("claimed");
        assert_eq!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn create_catalog_row_json_has_no_superuser() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM sql_server", []);
            let _ = conn.execute("DELETE FROM sql_databases", []);
            conn.execute(
                "INSERT INTO sql_server (id, status, installed_major, listen, updated_at) \
                 VALUES (1, 'running', 16, 'localhost', 1)",
                [],
            )
            .unwrap();
        }
        let dir = std::env::temp_dir().join(format!("k2-sql-create-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-create", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let v =
            ops::create_database(&ops, &secrets, &pid, 1, Some("idem-1"), None).expect("create");
        let s = v.to_string().to_ascii_lowercase();
        assert!(!s.contains("superuser"), "{s}");
        assert!(!s.contains("postgres://postgres"), "{s}");
        assert_eq!(v["role"], "agent");
        assert!(
            v["dsn"].as_str().expect("dsn").contains("_agent"),
            "{}",
            v["dsn"]
        );
        let helper = ops
            .pg
            .lock()
            .unwrap()
            .helper_sql
            .join("\n")
            .to_ascii_uppercase();
        assert!(helper.contains("NOSUPERUSER"));
        assert!(helper.contains("NOCREATEDB"));
        assert!(helper.contains("NOBYPASSRLS"));
        assert!(!helper.contains("FORCE ROW LEVEL"));
        assert!(
            helper.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE")
                && helper.contains("_K2_STORE"),
            "create_database must GRANT DML on _k2_store to agent; helper_sql={helper}"
        );
        // Idempotent --id
        let v2 = ops::create_database(&ops, &secrets, &pid, 1, Some("idem-1"), None)
            .expect("idempotent");
        assert_eq!(v2["existing"], true);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn create_database_grants_agent_on_k2_store() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-grant-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-grant-store", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let v = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let helper = ops.pg.lock().unwrap().helper_sql.join("\n");
        assert!(
            helper.contains(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE _k2_migrations, _k2_store TO"
            ),
            "explicit GRANT on _k2_store missing from helper_sql: {helper}"
        );
        assert!(helper.contains("_k2_store"), "{helper}");
        assert!(
            helper.contains("_agent"),
            "create_database GRANT path must still target ws_*_agent: {helper}"
        );
        assert!(
            !helper.to_ascii_uppercase().contains("CREATE ROLE SKIN_"),
            "create_database must not mint skin_* roles: {helper}"
        );
        assert!(
            !helper.to_ascii_uppercase().contains("FORCE ROW LEVEL"),
            "{helper}"
        );
        let cat = ops::catalog_json(None);
        let dbs = cat["databases"].as_array().expect("databases");
        let row = dbs.iter().find(|d| d["name"] == v["name"]).expect("listed");
        assert_eq!(row["documents"], true, "{row}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_applies_once_second_run_noop() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM sql_server", []);
            let _ = conn.execute("DELETE FROM sql_databases", []);
            conn.execute(
                "INSERT INTO sql_server (id, status, updated_at) VALUES (1, 'running', 1)",
                [],
            )
            .unwrap();
        }
        let dir = std::env::temp_dir().join(format!("k2-sql-mig-{}", std::process::id()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        std::fs::write(mig.join("0001_init.sql"), b"CREATE TABLE t (id int);\n").unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-mig", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        let n_helper_before = ops.pg.lock().unwrap().helper_sql.len();
        let first = ops::migrate(&ops, &secrets, &pid, &path, None).expect("migrate");
        let helper = ops.pg.lock().unwrap().helper_sql[n_helper_before..].join("\n");
        assert!(
            helper.contains("GRANT USAGE ON SCHEMA public TO")
                && helper.contains(
                    "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE _k2_migrations, _k2_store TO"
                ),
            "ensure GRANT via helper missing: {helper}"
        );
        assert!(
            helper.contains(&format!("{db_name}_agent")),
            "GRANT must target DSN user {{db}}_agent; helper={helper}"
        );
        assert_eq!(first["noop"], false);
        assert_eq!(first["discovered"], 1, "{first}");
        assert!(first["applied"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("0001_init")));
        let second = ops::migrate(&ops, &secrets, &pid, &path, None).expect("second");
        assert_eq!(second["noop"], true);
        assert_eq!(second["discovered"], 1, "{second}");
        assert!(second["applied"].as_array().unwrap().is_empty(), "{second}");
        let status = ops::database_status(&ops, &secrets, &pid).expect("status");
        let migrations = status["migrations"].as_array().expect("migrations");
        assert!(
            migrations
                .iter()
                .any(|m| m["version"].as_str() == Some("0001_init")),
            "{migrations:?}"
        );
        let checksum = migrations[0]["checksum"].as_str().expect("checksum");
        assert_eq!(
            checksum,
            ops::migration_checksum(b"CREATE TABLE t (id int);\n")
        );
        let size = status["sizeBytes"].as_i64().expect("sizeBytes");
        assert!(size > 0, "sizeBytes={size}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_checksum_mismatch_refuses_same_checksum_is_ok() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM sql_server", []);
            let _ = conn.execute("DELETE FROM sql_databases", []);
            conn.execute(
                "INSERT INTO sql_server (id, status, updated_at) VALUES (1, 'running', 1)",
                [],
            )
            .unwrap();
        }
        let dir = std::env::temp_dir().join(format!("k2-sql-cksum-{}", std::process::id()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        std::fs::write(mig.join("0001_init.sql"), b"CREATE TABLE t (id int);\n").unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-cksum", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        ops::migrate(&ops, &secrets, &pid, &path, None).expect("first");
        let again = ops::migrate(&ops, &secrets, &pid, &path, None).expect("same checksum");
        assert_eq!(again["noop"], true, "{again}");
        assert_eq!(again["discovered"], 1, "{again}");
        std::fs::write(
            mig.join("0001_init.sql"),
            b"CREATE TABLE t (id int, n int);\n",
        )
        .unwrap();
        let err = ops::migrate(&ops, &secrets, &pid, &path, None).expect_err("mismatch");
        assert_eq!(err.code(), "checksum_mismatch");
        assert!(
            err.hint().contains("0001_init"),
            "hint names the file: {}",
            err.hint()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_pipe_ledger_second_run_noop_tamper_is_checksum_mismatch() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-pipe-{}", uuid::Uuid::new_v4()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        let body = b"CREATE TABLE t (id int);\n";
        std::fs::write(mig.join("0001_init.sql"), body).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-pipe", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        let checksum = ops::migration_checksum(body);
        {
            let mut pg = ops.pg.lock().unwrap();
            pg.migrations.insert(
                db_name.clone(),
                vec![("0001_init".into(), checksum.clone())],
            );
            let out = pg
                .exec_sql(
                    Some(&db_name),
                    "SELECT version, checksum FROM _k2_migrations;",
                )
                .expect("select");
            assert_eq!(out, format!("0001_init|{checksum}"));
            assert!(
                !out.contains('\t'),
                "fake -tA default must be pipe: {out:?}"
            );
        }
        let second = ops::migrate(&ops, &secrets, &pid, &path, None).expect("pipe ledger skip");
        assert_eq!(second["noop"], true, "{second}");
        assert!(second["applied"].as_array().unwrap().is_empty(), "{second}");
        std::fs::write(
            mig.join("0001_init.sql"),
            b"CREATE TABLE t (id int, n int);\n",
        )
        .unwrap();
        let err = ops::migrate(&ops, &secrets, &pid, &path, None).expect_err("mismatch");
        assert_eq!(err.code(), "checksum_mismatch", "{}", err.hint());
        let ledger = ops
            .pg
            .lock()
            .unwrap()
            .migrations
            .get(&db_name)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            ledger,
            vec![("0001_init".into(), checksum)],
            "tamper must not INSERT a duplicate ledger row"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_tab_ledger_still_skips_and_refuses_tamper() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-tab-{}", uuid::Uuid::new_v4()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        let body = b"CREATE TABLE t (id int);\n";
        std::fs::write(mig.join("0001_init.sql"), body).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-tab", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        let checksum = ops::migration_checksum(body);
        {
            let mut pg = ops.pg.lock().unwrap();
            pg.select_field_sep = "\t";
            pg.migrations.insert(
                db_name.clone(),
                vec![("0001_init".into(), checksum.clone())],
            );
            let out = pg
                .exec_sql(
                    Some(&db_name),
                    "SELECT version, checksum FROM _k2_migrations;",
                )
                .expect("select");
            assert_eq!(out, format!("0001_init\t{checksum}"));
        }
        let second = ops::migrate(&ops, &secrets, &pid, &path, None).expect("tab ledger skip");
        assert_eq!(second["noop"], true, "{second}");
        std::fs::write(
            mig.join("0001_init.sql"),
            b"CREATE TABLE t (id int, n int);\n",
        )
        .unwrap();
        let err = ops::migrate(&ops, &secrets, &pid, &path, None).expect_err("mismatch");
        assert_eq!(err.code(), "checksum_mismatch");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_ws_path_with_space_applies_0001_init() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir()
            .join("AI Projects")
            .join(format!("k2-sql-space-{}", uuid::Uuid::new_v4()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        std::fs::write(mig.join("0001_init.sql"), b"CREATE TABLE t (id int);\n").unwrap();
        let path = dir.to_string_lossy().into_owned();
        assert!(path.contains("AI Projects"), "{path}");
        let pid = insert_project("sql-space", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let first = ops::migrate(&ops, &secrets, &pid, &path, None).expect("migrate");
        assert_eq!(first["noop"], false, "{first}");
        assert_eq!(first["discovered"], 1, "{first}");
        assert!(
            first["applied"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some("0001_init")),
            "{first}"
        );
        let abs = mig.to_string_lossy().into_owned();
        assert!(std::path::Path::new(&abs).is_absolute(), "{abs}");
        let via_abs = ops::migrate(&ops, &secrets, &pid, "/tmp/not the workspace", Some(&abs))
            .expect("absolute dir replaces ws_path");
        assert_eq!(via_abs["noop"], true, "{via_abs}");
        assert_eq!(via_abs["discovered"], 1, "{via_abs}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_init_sql_without_nnnn_is_usage_not_noop() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-init-{}", uuid::Uuid::new_v4()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        std::fs::write(mig.join("init.sql"), b"CREATE TABLE t (id int);\n").unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-init-only", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let err = ops::migrate(&ops, &secrets, &pid, &path, None).expect_err("init.sql");
        assert_eq!(err.code(), "usage", "{}", err.hint());
        assert!(err.hint().contains("0001_init.sql"), "{}", err.hint());
        assert!(err.hint().contains("init.sql"), "{}", err.hint());
        assert!(
            err.hint().contains(&mig.to_string_lossy().into_owned()),
            "hint names the dir: {}",
            err.hint()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_empty_matching_set_is_usage_not_noop() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-empty-mig-{}", uuid::Uuid::new_v4()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-empty-mig", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let err = ops::migrate(&ops, &secrets, &pid, &path, None).expect_err("empty");
        assert_eq!(err.code(), "usage", "{}", err.hint());
        assert!(err.hint().contains("0001_init.sql"), "{}", err.hint());
        assert!(
            err.hint().contains(&mig.to_string_lossy().into_owned()),
            "hint names the dir: {}",
            err.hint()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrate_refuses_force_rls() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-force-rls-{}", uuid::Uuid::new_v4()));
        let mig = dir.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig).unwrap();
        std::fs::write(
            mig.join("0001_init.sql"),
            b"ALTER TABLE t FORCE ROW LEVEL SECURITY;\n",
        )
        .unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-force-rls", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let err = ops::migrate(&ops, &secrets, &pid, &path, None).expect_err("force rls");
        assert_eq!(err.code(), "usage", "{}", err.hint());
        assert!(
            err.hint().contains("FORCE RLS"),
            "must refuse FORCE RLS: {}",
            err.hint()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_jail_rejects_dotdot_and_abs_happy_path_ok() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM sql_server", []);
            let _ = conn.execute("DELETE FROM sql_databases", []);
            conn.execute(
                "INSERT INTO sql_server (id, status, updated_at) VALUES (1, 'running', 1)",
                [],
            )
            .unwrap();
        }
        let dir = std::env::temp_dir().join(format!("k2-sql-restore-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".k2/db/dumps")).unwrap();
        std::fs::write(dir.join(".k2/db/dumps/ok.dump"), b"FAKE-PG-DUMP").unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-restore", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let err = ops::restore(&ops, &secrets, &pid, &path, "../outside.dump").expect_err("..");
        assert_eq!(err.code(), "usage", "{}", err.hint());
        assert!(err.hint().contains(".."), "{}", err.hint());
        let err = ops::restore(&ops, &secrets, &pid, &path, "/tmp/outside.dump").expect_err("abs");
        assert_eq!(err.code(), "usage", "{}", err.hint());
        assert!(err.hint().contains("absolute"), "{}", err.hint());
        let ok = ops::restore(&ops, &secrets, &pid, &path, ".k2/db/dumps/ok.dump")
            .expect("fresh workspace restore");
        assert_eq!(ok["ok"], true, "{ok}");
        assert_eq!(ok["restored"], ".k2/db/dumps/ok.dump", "{ok}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn persist_db_access_write() {
        k2_core::db::init_for_tests();
        let dir = std::env::temp_dir().join(format!("k2-sql-acc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, "sql-acc", path],
            )
            .expect("insert project default access");
        }
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path),
            "off"
        );
        ops::persist_db_access(&path, Some("write")).expect("write");
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path),
            "write"
        );
        let err = ops::persist_db_access(&path, Some("admin")).expect_err("bad");
        assert_eq!(err.code(), "usage");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_put_get_query() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM sql_server", []);
            let _ = conn.execute("DELETE FROM sql_databases", []);
            conn.execute(
                "INSERT INTO sql_server (id, status, updated_at) VALUES (1, 'running', 1)",
                [],
            )
            .unwrap();
        }
        let dir = std::env::temp_dir().join(format!("k2-sql-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-store", &path);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        ops::store_create(&ops, &secrets, &pid, "items").expect("coll");
        let n_rec = ops.recorded().len();
        let doc = serde_json::json!({"n": 1});
        ops::store_put(&ops, &secrets, &pid, "items", "a", &doc).expect("put");
        let rec = ops.recorded()[n_rec..].join("\n");
        assert!(
            rec.contains(&format!("-U {db_name}_agent")),
            "store_put must connect as owner agent, got {rec}"
        );
        assert!(
            !rec.contains("_migrator"),
            "store_put DML must not use migrator, got {rec}"
        );
        let got = ops::store_get(&ops, &secrets, &pid, "items", "a").expect("get");
        assert_eq!(got["doc"]["n"], 1);
        let q = ops::store_query(&ops, &secrets, &pid, "items", 10).expect("query");
        assert!(q["docs"].as_array().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn principal_a_with_project_b_uses_a_db() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM sql_server", []);
            let _ = conn.execute("DELETE FROM sql_databases", []);
            conn.execute(
                "INSERT INTO sql_server (id, status, updated_at) VALUES (1, 'running', 1)",
                [],
            )
            .unwrap();
        }
        let dir_a = std::env::temp_dir().join(format!("k2-sql-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-b-{}", std::process::id()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let path_a = dir_a.to_string_lossy().into_owned();
        let path_b = dir_b.to_string_lossy().into_owned();
        let id_a = insert_project("ws-a", &path_a);
        let _id_b = insert_project("ws-b", &path_b);
        let principal = HookPrincipal {
            workspace_uuid: id_a.clone(),
            agent_address: "a".into(),
        };
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        with_request_principal(Some(principal), || {
            let (path, pid) = identity::resolve_caller(&path_b)
                .unwrap_or_else(|r| panic!("resolve failed: {} {}", r.status, r.body));
            assert_eq!(pid, id_a, "HookPrincipal A must win over project=B");
            assert_eq!(path, path_a);
            let v = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
            let name = v["name"].as_str().unwrap();
            assert!(
                name.contains(&id_a.replace('-', "_")),
                "DB name must be A's: {name}"
            );
            assert!(
                !name.contains(&_id_b.replace('-', "_")),
                "must not mint B's DB"
            );
        });
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    fn seed_running_sidecar() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM sql_grants", []);
        let _ = conn.execute("DELETE FROM sql_server", []);
        let _ = conn.execute("DELETE FROM sql_databases", []);
        conn.execute(
            "INSERT INTO sql_server (id, status, installed_major, listen, updated_at) \
             VALUES (1, 'running', 16, 'localhost', 1)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn sql_grants_round_trip_via_their_role() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-ga-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-gb-{}", std::process::id()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-ga", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-gb", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        let granted =
            ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "read", false).expect("grant");
        assert_eq!(granted["level"], "read");
        assert_eq!(granted["canManage"], false);
        let role = granted["role"].as_str().unwrap();
        assert!(role.contains("_agent"), "{role}");
        assert!(!role.contains("postgres"), "{role}");
        let granted_s = granted.to_string().to_ascii_lowercase();
        assert!(!granted_s.contains("password"), "{granted}");
        assert!(!granted_s.contains("dbsec_"), "{granted}");
        let sref: Option<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT agent_secret_ref FROM sql_grants WHERE project_id = ?1",
                rusqlite::params![id_b],
                |r| r.get(0),
            )
            .expect("grant secret_ref")
        };
        assert!(
            sref.as_deref().is_some_and(|s| s.starts_with("dbsec_")),
            "grant must vault agent_secret_ref, got {sref:?}"
        );
        let helper = ops
            .pg
            .lock()
            .unwrap()
            .helper_sql
            .join("\n")
            .to_ascii_uppercase();
        assert!(helper.contains("GRANT CONNECT"), "{helper}");
        assert!(helper.contains("NOSUPERUSER"), "{helper}");
        assert!(!helper.contains("FORCE ROW LEVEL"), "{helper}");
        let cat = ops::catalog_json(None);
        let dbs = cat["databases"].as_array().unwrap();
        let row = dbs.iter().find(|d| d["name"] == db_name).expect("listed");
        assert_eq!(row["grants"].as_array().unwrap().len(), 1);
        assert_eq!(row["grants"][0]["projectId"], id_b);
        assert_eq!(row["grants"][0]["level"], "read");
        assert_eq!(row["dbAgentAccess"], "off", "{row}");
        ops::revoke_access(&ops, None, &db_name, &id_b).expect("revoke");
        let cat2 = ops::catalog_json(None);
        let row2 = cat2["databases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == db_name)
            .expect("listed");
        assert!(row2["grants"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn agent_cannot_grant_without_can_manage() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-ag-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-bg-{}", std::process::id()));
        let dir_c = std::env::temp_dir().join(format!("k2-sql-cg-{}", std::process::id()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::create_dir_all(&dir_c).unwrap();
        let id_a = insert_project("sql-ag", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-bg", &dir_b.to_string_lossy());
        let id_c = insert_project("sql-cg", &dir_c.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        let err = ops::grant_access(&ops, &secrets, Some(&id_b), &db_name, &id_c, "read", false)
            .expect_err("foreign agent must not grant");
        assert_eq!(err.code(), "forbidden");
        let principal = HookPrincipal {
            workspace_uuid: id_b.clone(),
            agent_address: "b".into(),
        };
        let fake = Box::leak(Box::new(FakeSystemOps::baked()));
        with_request_principal(Some(principal), || {
            routes::with_fake_ops(fake, || {
                let body = serde_json::json!({
                    "project": id_c,
                    "db": db_name,
                    "level": "read",
                });
                let r = routes::handle_grant(body.to_string().as_bytes());
                assert_eq!(r.status, "403 Forbidden", "{}", r.body);
                let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
                assert_eq!(v["error"]["code"], "forbidden");
            });
        });
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
        let _ = std::fs::remove_dir_all(dir_c);
    }

    #[test]
    fn bind_does_not_print_secrets() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-bind-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-bind", &path);
        let fake = Box::leak(Box::new(FakeSystemOps::baked()));
        let secrets = MemSecretStore::default();
        let created = ops::create_database(fake, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        let v = ops::bind_role(fake, Some(&db_name), Some(&pid), "app_reader").expect("bind");
        let s = v.to_string().to_ascii_lowercase();
        assert!(!s.contains("password"), "{s}");
        assert!(!s.contains("dsn"), "{s}");
        assert!(!s.contains("dbsec_"), "{s}");
        assert!(!s.contains("superuser"), "{s}");
        assert_eq!(v["bindRole"], "app_reader");
        let cat = ops::catalog_json(None);
        let row = cat["databases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == db_name)
            .expect("listed");
        assert_eq!(row["bindRole"], "app_reader");
        let body = serde_json::json!({ "project": pid, "db": db_name, "role": "app_reader" });
        routes::with_fake_ops(fake, || {
            let r = routes::handle_bind(body.to_string().as_bytes());
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let out: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            let s = out.to_string().to_ascii_lowercase();
            assert!(!s.contains("password"), "{s}");
            assert!(!s.contains("dsn"), "{s}");
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn get_grant_is_405() {
        let r = sql_routes::dispatch("/cli/db/grant", &Default::default()).expect("claimed");
        assert_eq!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn grant_then_create_does_not_fail_on_duplicate_role() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-gtc-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-gtc-b-{}", std::process::id()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-gtc-a", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-gtc-b", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created_a =
            ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create A");
        let db_a = created_a["name"].as_str().unwrap().to_string();
        ops::grant_access(&ops, &secrets, None, &db_a, &id_b, "write", false).expect("grant B");
        let n_helper_before = ops.pg.lock().unwrap().helper_sql.len();
        let created_b = ops::create_database(&ops, &secrets, &id_b, 1, None, None)
            .expect("create B after grant must not fail on duplicate ROLE");
        assert_eq!(created_b["ok"], true);
        let s = created_b.to_string().to_ascii_lowercase();
        assert!(!s.contains("superuser"), "{s}");
        let helper = ops.pg.lock().unwrap().helper_sql[n_helper_before..]
            .join("\n")
            .to_ascii_uppercase();
        assert!(
            helper.contains("EXCEPTION WHEN DUPLICATE_OBJECT"),
            "create must use idempotent CREATE ROLE, got {helper}"
        );
        assert!(
            helper.contains("ALTER ROLE"),
            "create must ALTER ROLE to reset password after grant, got {helper}"
        );
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn write_grant_uses_usage_select_update_on_sequences() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-seq-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-seq-b-{}", std::process::id()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-seq-a", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-seq-b", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"]
            .as_str()
            .expect("create_database must return name")
            .to_string();
        let granted = ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "write", false)
            .expect("grant write");
        assert_eq!(granted["level"], "write", "{granted}");
        let helper = ops
            .pg
            .lock()
            .expect("pg lock")
            .helper_sql
            .join("\n")
            .to_ascii_uppercase();
        assert!(
            helper.contains("GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES"),
            "write grant must use sequence privileges USAGE/SELECT/UPDATE (not INSERT/DELETE), got {helper}"
        );
        assert!(
            !helper.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL SEQUENCES"),
            "Postgres sequences reject INSERT/DELETE; helper_sql must not GRANT them, got {helper}"
        );
        assert!(
            helper.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES"),
            "write grant must still DML tables, got {helper}"
        );
        assert!(
            helper.contains("ALTER DEFAULT PRIVILEGES FOR ROLE")
                && helper.contains("GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO"),
            "write grant must ALTER DEFAULT PRIVILEGES to the grantee, got {helper}"
        );
        for stmt in helper.split(';') {
            let grant_on_sequences = stmt.contains("GRANT") && stmt.contains("ON ALL SEQUENCES");
            assert!(
                !grant_on_sequences || (!stmt.contains("INSERT") && !stmt.contains("DELETE")),
                "sequence GRANT must not include INSERT/DELETE: {stmt}"
            );
        }
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    fn json_body(r: &crate::cli_response::CliResponse) -> serde_json::Value {
        serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {}: {}", e, r.body))
    }

    fn assert_grant_403(
        resp: &crate::cli_response::CliResponse,
        db_id: &str,
        db_name: &str,
        need: &str,
    ) {
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        let err = json_body(resp);
        assert_eq!(err["error"]["code"], "forbidden", "{err}");
        assert_eq!(err["error"]["dbId"], db_id, "{err}");
        assert_eq!(err["error"]["dbName"], db_name, "{err}");
        assert_eq!(err["error"]["resolvedVia"], "grant", "{err}");
        let hint = err["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains("sql_grants"), "{hint}");
        assert!(hint.contains(db_name), "{hint}");
        assert!(hint.contains(db_id), "{hint}");
        assert!(hint.contains("resolvedVia=grant"), "{hint}");
        assert!(
            hint.contains(&format!("need {need}")),
            "hint must name need={need}: {hint}"
        );
        assert!(!hint.contains("db_agent_access"), "{hint}");
        assert!(!resp.body.contains("db_agent_access"), "{}", resp.body);
    }

    #[test]
    fn agent_off_who_owns_db_lists_and_stores_create_still_403() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-off-own-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-off-own", &path);
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path),
            "off"
        );
        let fake = Box::leak(Box::new(FakeSystemOps::baked()));
        let secrets = crate::sql::secrets::FileSecretStore::default();
        ops::create_database(fake, &secrets, &pid, 1, None, None).expect("create");
        ops::store_create(fake, &secrets, &pid, "items").expect("coll");
        ops::store_put(
            fake,
            &secrets,
            &pid,
            "items",
            "a",
            &serde_json::json!({"n": 1}),
        )
        .expect("seed put");
        let principal = HookPrincipal {
            workspace_uuid: pid.clone(),
            agent_address: "julie".into(),
        };
        with_request_principal(Some(principal), || {
            routes::with_fake_ops(fake, || {
                let mut params = std::collections::HashMap::new();
                params.insert("project".into(), path.clone());
                let list = routes::handle_list(&params);
                assert_eq!(list.status, "200 OK", "{}", list.body);
                let listed = json_body(&list);
                let dbs = listed["databases"].as_array().expect("databases");
                assert_eq!(dbs.len(), 1, "{listed}");
                assert_eq!(dbs[0]["ownerProjectId"], pid, "{listed}");
                assert_eq!(dbs[0]["dbAgentAccess"], "off", "{listed}");

                params.insert("name".into(), "items".into());
                params.insert("id".into(), "a".into());
                let got = routes::handle_store_get(&params);
                assert_eq!(got.status, "200 OK", "{}", got.body);
                let doc = json_body(&got);
                assert_eq!(doc["doc"]["n"], 1, "{doc}");

                let put_body = serde_json::json!({
                    "project": path,
                    "name": "items",
                    "id": "b",
                    "json": {"n": 2},
                });
                let put = routes::handle_store_put(put_body.to_string().as_bytes());
                assert_eq!(put.status, "200 OK", "{}", put.body);

                let create = routes::handle_create(
                    serde_json::json!({ "project": path })
                        .to_string()
                        .as_bytes(),
                );
                assert_eq!(create.status, "403 Forbidden", "{}", create.body);
                let err = json_body(&create);
                let hint = err["error"]["hint"].as_str().expect("hint");
                assert!(hint.contains("db_agent_access"), "{hint}");
                assert!(hint.contains("need write"), "{hint}");
                assert!(hint.contains("create new databases"), "{hint}");
            });
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn grant_read_lists_and_gets_put_migrate_are_grant_403() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-gr-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-gr-b-{}", std::process::id()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let path_a = dir_a.to_string_lossy().into_owned();
        let path_b = dir_b.to_string_lossy().into_owned();
        let id_a = insert_project("sql-gr-a", &path_a);
        let id_b = insert_project("sql-gr-b", &path_b);
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path_b),
            "off"
        );
        let fake = Box::leak(Box::new(FakeSystemOps::baked()));
        let secrets = crate::sql::secrets::FileSecretStore::default();
        let created = ops::create_database(fake, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        ops::store_create(fake, &secrets, &id_a, "items").expect("coll");
        ops::store_put(
            fake,
            &secrets,
            &id_a,
            "items",
            "a",
            &serde_json::json!({"n": 1}),
        )
        .expect("seed put");
        let granted =
            ops::grant_access(fake, &secrets, None, &db_name, &id_b, "read", false).expect("grant");
        let db_id = granted["databaseId"]
            .as_str()
            .expect("databaseId")
            .to_string();
        let principal = HookPrincipal {
            workspace_uuid: id_b.clone(),
            agent_address: "grantee".into(),
        };
        with_request_principal(Some(principal), || {
            routes::with_fake_ops(fake, || {
                let mut params = std::collections::HashMap::new();
                params.insert("project".into(), path_b.clone());
                let list = routes::handle_list(&params);
                assert_eq!(list.status, "200 OK", "{}", list.body);
                let listed = json_body(&list);
                let dbs = listed["databases"].as_array().expect("databases");
                assert_eq!(dbs.len(), 1, "{listed}");
                assert_eq!(dbs[0]["name"], db_name, "{listed}");
                assert_eq!(dbs[0]["yourLevel"], "read", "{listed}");

                params.insert("name".into(), "items".into());
                params.insert("id".into(), "a".into());
                let got = routes::handle_store_get(&params);
                assert_eq!(got.status, "200 OK", "{}", got.body);
                let doc = json_body(&got);
                assert_eq!(doc["doc"]["n"], 1, "{doc}");

                let put_body = serde_json::json!({
                    "project": path_b,
                    "name": "items",
                    "id": "b",
                    "json": {"n": 2},
                });
                let put = routes::handle_store_put(put_body.to_string().as_bytes());
                assert_grant_403(&put, &db_id, &db_name, "write");

                let mig = routes::handle_migrate(
                    serde_json::json!({ "project": path_b })
                        .to_string()
                        .as_bytes(),
                );
                assert_grant_403(&mig, &db_id, &db_name, "write");
            });
        });
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn older_read_then_newer_write_grant_unscoped_put_is_403() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-old-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-old-b-{}", std::process::id()));
        let dir_c = std::env::temp_dir().join(format!("k2-sql-old-c-{}", std::process::id()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::create_dir_all(&dir_c).unwrap();
        let path_a = dir_a.to_string_lossy().into_owned();
        let path_b = dir_b.to_string_lossy().into_owned();
        let path_c = dir_c.to_string_lossy().into_owned();
        let id_a = insert_project("sql-old-a", &path_a);
        let id_b = insert_project("sql-old-b", &path_b);
        let id_c = insert_project("sql-old-c", &path_c);
        let fake = Box::leak(Box::new(FakeSystemOps::baked()));
        let secrets = crate::sql::secrets::FileSecretStore::default();
        let created_a =
            ops::create_database(fake, &secrets, &id_a, 1, None, None).expect("create A");
        let created_c =
            ops::create_database(fake, &secrets, &id_c, 1, None, None).expect("create C");
        let name_a = created_a["name"].as_str().expect("name A").to_string();
        let name_c = created_c["name"].as_str().expect("name C").to_string();
        let grant_read = ops::grant_access(fake, &secrets, None, &name_a, &id_b, "read", false)
            .expect("read grant");
        let grant_write = ops::grant_access(fake, &secrets, None, &name_c, &id_b, "write", false)
            .expect("write grant");
        let id_read = grant_read["databaseId"]
            .as_str()
            .expect("read databaseId")
            .to_string();
        let id_write = grant_write["databaseId"]
            .as_str()
            .expect("write databaseId")
            .to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE sql_grants SET created_at = 1 WHERE database_id = ?1 AND project_id = ?2",
                rusqlite::params![id_read, id_b],
            )
            .expect("stamp older read grant");
            conn.execute(
                "UPDATE sql_grants SET created_at = 2 WHERE database_id = ?1 AND project_id = ?2",
                rusqlite::params![id_write, id_b],
            )
            .expect("stamp newer write grant");
        }
        let principal = HookPrincipal {
            workspace_uuid: id_b.clone(),
            agent_address: "grantee".into(),
        };
        with_request_principal(Some(principal), || {
            routes::with_fake_ops(fake, || {
                let put = routes::handle_store_put(
                    serde_json::json!({
                        "project": path_b,
                        "name": "items",
                        "id": "b",
                        "json": {"n": 2},
                    })
                    .to_string()
                    .as_bytes(),
                );
                assert_grant_403(&put, &id_read, &name_a, "write");
                let put_err = json_body(&put);
                assert_ne!(
                    put_err["error"]["dbId"].as_str().unwrap_or(""),
                    id_write,
                    "unscoped put must not ride the newer write grant: {put_err}"
                );
                assert_ne!(
                    put_err["error"]["dbName"].as_str().unwrap_or(""),
                    name_c,
                    "unscoped put must not name the write-granted DB: {put_err}"
                );

                let mig = routes::handle_migrate(
                    serde_json::json!({ "project": path_b })
                        .to_string()
                        .as_bytes(),
                );
                assert_grant_403(&mig, &id_read, &name_a, "write");
            });
        });
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
        let _ = std::fs::remove_dir_all(dir_c);
    }

    #[test]
    fn owner_interact_put_write_without_grant_row() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-own-put-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-own-put", &path);
        let fake = Box::leak(Box::new(FakeSystemOps::baked()));
        let secrets = crate::sql::secrets::FileSecretStore::default();
        ops::create_database(fake, &secrets, &pid, 1, None, None).expect("create");
        ops::store_create(fake, &secrets, &pid, "items").expect("coll");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM sql_grants", [], |r| r.get(0))
                .expect("grant count");
            assert_eq!(n, 0, "owner write must not require a grant row");
        }
        let principal = HookPrincipal {
            workspace_uuid: pid.clone(),
            agent_address: "owner".into(),
        };
        with_request_principal(Some(principal), || {
            routes::with_fake_ops(fake, || {
                let put = routes::handle_store_put(
                    serde_json::json!({
                        "project": path,
                        "name": "items",
                        "id": "a",
                        "json": {"n": 1},
                    })
                    .to_string()
                    .as_bytes(),
                );
                assert_eq!(put.status, "200 OK", "{}", put.body);
                assert!(!put.body.contains("db_agent_access"), "{}", put.body);

                let mut params = std::collections::HashMap::new();
                params.insert("project".into(), path.clone());
                params.insert("name".into(), "items".into());
                params.insert("id".into(), "a".into());
                let got = routes::handle_store_get(&params);
                assert_eq!(got.status, "200 OK", "{}", got.body);
                let doc = json_body(&got);
                assert_eq!(doc["doc"]["n"], 1, "{doc}");
            });
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn agent_off_with_no_db_lists_empty_not_passport_403() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-off-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let pid = insert_project("sql-off-empty", &path);
        let principal = HookPrincipal {
            workspace_uuid: pid,
            agent_address: "empty".into(),
        };
        with_request_principal(Some(principal), || {
            let mut params = std::collections::HashMap::new();
            params.insert("project".into(), path.clone());
            let list = routes::handle_list(&params);
            assert_eq!(list.status, "200 OK", "{}", list.body);
            let listed = json_body(&list);
            let dbs = listed["databases"].as_array().expect("databases");
            assert!(dbs.is_empty(), "{listed}");
            assert!(!list.body.contains("db_agent_access"), "{}", list.body);

            params.insert("name".into(), "items".into());
            params.insert("id".into(), "a".into());
            let got = routes::handle_store_get(&params);
            assert_eq!(got.status, "404 Not Found", "{}", got.body);
            let err = json_body(&got);
            let hint = err["error"]["hint"].as_str().expect("hint");
            assert!(hint.contains("ask your human to create one"), "{hint}");
            assert!(!hint.contains("db_agent_access"), "{hint}");
            assert!(
                err["error"].get("dbId").is_none(),
                "true empty 404 must omit dbId: {err}"
            );
            assert!(
                err["error"].get("dbName").is_none(),
                "true empty 404 must omit dbName: {err}"
            );
            assert!(
                err["error"].get("resolvedVia").is_none(),
                "true empty 404 must omit resolvedVia: {err}"
            );
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    fn dsn_url_user(v: &serde_json::Value) -> &str {
        let dsn = v["dsn"].as_str().expect("dsn");
        let rest = dsn.strip_prefix("postgres://").expect("postgres scheme");
        rest.split(':').next().expect("dsn user")
    }

    fn grant_secret_ref(grantee: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT agent_secret_ref FROM sql_grants WHERE project_id = ?1",
            rusqlite::params![grantee],
            |r| r.get(0),
        )
        .ok()
        .flatten()
    }

    #[test]
    fn grant_vaults_secret_and_default_privileges_to_grantee() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-ga-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-gb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-128-ga", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-128-gb", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        let granted = ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "write", false)
            .expect("grant");
        let role_b = ops::default_agent_role(&id_b);
        assert_eq!(granted["role"].as_str().expect("role"), role_b);
        let body = granted.to_string().to_ascii_lowercase();
        assert!(!body.contains("password"), "{granted}");
        assert!(!body.contains("dbsec_"), "{granted}");
        let sref = grant_secret_ref(&id_b);
        assert!(
            sref.as_deref().is_some_and(|s| s.starts_with("dbsec_")),
            "agent_secret_ref must be set, got {sref:?}"
        );
        let helper = ops.pg.lock().expect("pg").helper_sql.join("\n");
        assert!(
            helper.contains(&format!("CREATE ROLE \"{role_b}\"")),
            "grant must CREATE ROLE {role_b}: {helper}"
        );
        assert!(
            helper.contains(&format!(
                "GRANT CONNECT ON DATABASE \"{db_name}\" TO \"{role_b}\""
            )),
            "grant must GRANT CONNECT to B: {helper}"
        );
        assert!(
            helper.contains("ALTER DEFAULT PRIVILEGES")
                && helper.contains(&format!("TO \"{role_b}\"")),
            "grant must DEFAULT PRIVILEGES to B, not only owner agent: {helper}"
        );
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn dsn_as_grantee_uses_workspace_agent_not_dbname_agent() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-da-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-db-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-128-da", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-128-db", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "write", false).expect("grant");
        let dsn_b = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("dsn B");
        assert_eq!(dsn_b["role"], "agent", "{dsn_b}");
        let user_b = dsn_url_user(&dsn_b);
        assert_eq!(user_b, ops::default_agent_role(&id_b), "{dsn_b}");
        assert_ne!(user_b, format!("{db_name}_agent"), "{dsn_b}");
        assert!(
            dsn_b["dsn"]
                .as_str()
                .expect("dsn")
                .contains(&format!("/{db_name}")),
            "grantee dsn must target A's db: {dsn_b}"
        );
        let dsn_a = ops::dsn_for_project(&ops, &secrets, &id_a, 1).expect("dsn A");
        assert_eq!(dsn_a["role"], "agent", "{dsn_a}");
        assert_eq!(dsn_url_user(&dsn_a), format!("{db_name}_agent"), "{dsn_a}");
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn two_grantees_get_different_dsn_users() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-2a-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-2b-{}", uuid::Uuid::new_v4()));
        let dir_c = std::env::temp_dir().join(format!("k2-sql-128-2c-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::create_dir_all(&dir_c).unwrap();
        let id_a = insert_project("sql-128-2a", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-128-2b", &dir_b.to_string_lossy());
        let id_c = insert_project("sql-128-2c", &dir_c.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "write", false).expect("grant B");
        ops::grant_access(&ops, &secrets, None, &db_name, &id_c, "read", false).expect("grant C");
        let dsn_b = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("dsn B");
        let dsn_c = ops::dsn_for_project(&ops, &secrets, &id_c, 1).expect("dsn C");
        let user_b = dsn_url_user(&dsn_b);
        let user_c = dsn_url_user(&dsn_c);
        assert_eq!(user_b, ops::default_agent_role(&id_b), "{dsn_b}");
        assert_eq!(user_c, ops::default_agent_role(&id_c), "{dsn_c}");
        assert_ne!(user_b, user_c, "two grantees must not share a LOGIN");
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
        let _ = std::fs::remove_dir_all(dir_c);
    }

    #[test]
    fn store_put_as_grantee_uses_workspace_agent_not_migrator() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-sa-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-sb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let path_a = dir_a.to_string_lossy().into_owned();
        let id_a = insert_project("sql-128-sa", &path_a);
        let id_b = insert_project("sql-128-sb", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "write", false).expect("grant");
        let n = ops.recorded().len();
        ops::store_put(
            &ops,
            &secrets,
            &id_b,
            "items",
            "b1",
            &serde_json::json!({"n": 1}),
        )
        .expect("put as B");
        let rec = ops.recorded()[n..].join("\n");
        let role_b = ops::default_agent_role(&id_b);
        assert!(
            rec.contains(&format!("-U {role_b}")),
            "store_put as B must use -U {role_b}, got {rec}"
        );
        assert!(
            !rec.contains("_migrator"),
            "store_put as B must not use migrator, got {rec}"
        );
        assert!(
            !rec.contains(&format!("-U {db_name}_agent")),
            "store_put as B must not use owner agent, got {rec}"
        );
        let n = ops.recorded().len();
        let mig_dir = dir_a.join(".k2/db/migrations");
        std::fs::create_dir_all(&mig_dir).unwrap();
        std::fs::write(mig_dir.join("0001_init.sql"), b"CREATE TABLE t (id int);\n").unwrap();
        ops::migrate(&ops, &secrets, &id_a, &path_a, None).expect("migrate");
        let rec = ops.recorded()[n..].join("\n");
        assert!(
            rec.contains(&format!("-U {db_name}_migrator")),
            "migrate must use migrator, got {rec}"
        );
        let n = ops.recorded().len();
        ops::dump(&ops, &secrets, &id_a, &path_a, Some(".k2/db/dumps/x.dump")).expect("dump");
        let rec = ops.recorded()[n..].join("\n");
        assert!(
            rec.contains(&format!("-U {db_name}_migrator")),
            "dump must use migrator, got {rec}"
        );
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn store_put_grantee_missing_table_is_409() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-409a-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-409b-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-128-409a", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-128-409b", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "write", false).expect("grant");
        {
            let mut pg = ops.pg.lock().expect("pg");
            pg.tables.remove(&(db_name.clone(), "_k2_store".into()));
        }
        let n = ops.recorded().len();
        let err = ops::store_put(
            &ops,
            &secrets,
            &id_b,
            "items",
            "x",
            &serde_json::json!({"n": 1}),
        )
        .expect_err("grantee missing table");
        assert_eq!(err.code(), "not_ready", "{}", err.hint());
        assert_eq!(err.status(), "409 Conflict");
        assert!(
            err.hint().contains("_k2_store") || err.hint().contains("migrate"),
            "{}",
            err.hint()
        );
        let rec = ops.recorded()[n..].join("\n");
        assert!(
            !rec.contains("_migrator"),
            "grantee must not CREATE as migrator, got {rec}"
        );
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn owner_first_put_missing_table_migrator_ensure_then_agent_insert() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-128-own-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid = insert_project("sql-128-own", &dir.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        {
            let mut pg = ops.pg.lock().expect("pg");
            pg.tables.remove(&(db_name.clone(), "_k2_store".into()));
        }
        let n = ops.recorded().len();
        ops::store_put(
            &ops,
            &secrets,
            &pid,
            "items",
            "a",
            &serde_json::json!({"n": 1}),
        )
        .expect("owner put");
        let rec = ops.recorded()[n..].join("\n");
        assert!(
            rec.contains(&format!("-U {db_name}_migrator")),
            "missing table must CREATE as migrator, got {rec}"
        );
        assert!(
            rec.contains(&format!("-U {db_name}_agent")),
            "INSERT must be owner agent, got {rec}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn foreign_project_on_scoped_token_dsn_uses_stamped_caller() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-fa-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-fb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let path_a = dir_a.to_string_lossy().into_owned();
        let path_b = dir_b.to_string_lossy().into_owned();
        let id_a = insert_project("sql-128-fa", &path_a);
        let id_b = insert_project("sql-128-fb", &path_b);
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        let principal = HookPrincipal {
            workspace_uuid: id_a.clone(),
            agent_address: "a".into(),
        };
        with_request_principal(Some(principal), || {
            let (_path, pid) = identity::resolve_caller(&path_b)
                .unwrap_or_else(|r| panic!("resolve failed: {} {}", r.status, r.body));
            assert_eq!(pid, id_a, "HookPrincipal A must win over project=B");
            let v = ops::dsn_for_project(&ops, &secrets, &pid, 1).expect("dsn");
            assert_eq!(dsn_url_user(&v), format!("{db_name}_agent"), "{v}");
            assert_ne!(
                dsn_url_user(&v),
                ops::default_agent_role(&id_b),
                "must not dsn as B"
            );
        });
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn unscoped_dsn_flips_to_owned_after_grantee_creates() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-fla-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-flb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-128-fla", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-128-flb", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created_a =
            ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create A");
        let db_a = created_a["name"].as_str().expect("name").to_string();
        ops::grant_access(&ops, &secrets, None, &db_a, &id_b, "write", false).expect("grant");
        let dsn_grant = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("dsn grant");
        assert_eq!(
            dsn_url_user(&dsn_grant),
            ops::default_agent_role(&id_b),
            "{dsn_grant}"
        );
        assert_eq!(dsn_grant["name"].as_str().expect("name"), db_a);
        let created_b =
            ops::create_database(&ops, &secrets, &id_b, 1, None, None).expect("create B");
        let db_b = created_b["name"].as_str().expect("name B").to_string();
        let dsn_owned = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("dsn owned");
        assert_eq!(
            dsn_url_user(&dsn_owned),
            format!("{db_b}_agent"),
            "unscoped dsn must flip to B's owned DB: {dsn_owned}"
        );
        assert_eq!(dsn_owned["name"].as_str().expect("name"), db_b);
        assert_ne!(
            dsn_owned["name"].as_str().expect("name"),
            dsn_grant["name"].as_str().expect("name"),
            "unscoped dsn must leave A's DB after B creates: grant={dsn_grant} owned={dsn_owned}"
        );
        assert!(
            dsn_owned["dsn"]
                .as_str()
                .expect("dsn")
                .contains(&format!("/{db_b}")),
            "owned dsn URL must target B's database: {dsn_owned}"
        );
        assert!(
            !dsn_owned["dsn"]
                .as_str()
                .expect("dsn")
                .contains(&format!("/{db_a}")),
            "owned dsn must not still point at A's database: {dsn_owned}"
        );
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn grant_then_create_does_not_rotate_the_other_vaulted_secret() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-rot-a-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-rot-b-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-128-rot-a", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-128-rot-b", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created_a =
            ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create A");
        let db_a = created_a["name"].as_str().expect("name").to_string();
        ops::grant_access(&ops, &secrets, None, &db_a, &id_b, "write", false).expect("grant");
        let grant_ref = grant_secret_ref(&id_b).expect("grant secret_ref");
        let pw_before = secrets
            .resolve(&grant_ref)
            .expect("resolve")
            .expect("vaulted");
        let dsn_before = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("dsn before");
        let created_b = ops::create_database(&ops, &secrets, &id_b, 1, None, None)
            .expect("create B after grant");
        let grant_ref_after = grant_secret_ref(&id_b).expect("grant secret_ref after");
        let pw_after = secrets
            .resolve(&grant_ref_after)
            .expect("resolve after")
            .expect("vaulted after");
        assert_eq!(
            pw_before, pw_after,
            "create must not rotate the grant vaulted password"
        );
        let db_ref: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT agent_secret_ref FROM sql_databases WHERE project_id = ?1 AND status = 'active'",
                rusqlite::params![id_b],
                |r| r.get(0),
            )
            .expect("owned secret_ref")
        };
        assert_eq!(
            db_ref, grant_ref_after,
            "owned row must share the grant vault key"
        );
        let dsn_owned = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("dsn owned");
        assert_eq!(
            dsn_url_user(&dsn_owned),
            format!("{}_agent", created_b["name"].as_str().expect("name"))
        );
        let dsn_grant = ops::dsn_for_project(&ops, &secrets, &id_a, 1).expect("dsn A still owner");
        assert_eq!(dsn_url_user(&dsn_grant), format!("{db_a}_agent"));
        assert_eq!(dsn_url_user(&dsn_before), ops::default_agent_role(&id_b));
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }

    #[test]
    fn bind_set_grants_membership_and_set_role_postgres_still_400() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir = std::env::temp_dir().join(format!("k2-sql-128-bind-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid = insert_project("sql-128-bind", &dir.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        let err = ops::bind_role(&ops, Some(&db_name), Some(&pid), "postgres")
            .expect_err("postgres bind");
        assert_eq!(err.code(), "usage", "{}", err.hint());
        assert_eq!(err.status(), "400 Bad Request");
        let err = ops::bind_role(&ops, Some(&db_name), Some(&pid), "k2_admin")
            .expect_err("k2_admin bind");
        assert_eq!(err.code(), "usage");
        let err = ops::bind_role(&ops, Some(&db_name), Some(&pid), "my_superuser")
            .expect_err("superuser substring");
        assert_eq!(err.code(), "usage");
        ops::bind_role(&ops, Some(&db_name), Some(&pid), "app_reader").expect("bind");
        let helper = ops.pg.lock().expect("pg").helper_sql.join("\n");
        assert!(
            helper.contains("GRANT \"app_reader\" TO")
                && helper.contains(&format!("\"{db_name}_agent\"")),
            "bind must GRANT membership to owner agent: {helper}"
        );
        assert!(
            helper.to_ascii_uppercase().contains("CREATE ROLE")
                && helper.contains("\"app_reader\""),
            "bind must CREATE ROLE bind if missing: {helper}"
        );
        let n = ops.recorded().len();
        ops::store_put(
            &ops,
            &secrets,
            &pid,
            "items",
            "a",
            &serde_json::json!({"n": 1}),
        )
        .expect("put with bind");
        let rec = ops.recorded()[n..].join("\n");
        assert!(
            rec.contains("SET ROLE"),
            "owned store with bind must SET ROLE, got {rec}"
        );
        assert!(
            rec.contains(&format!("-U {db_name}_agent")),
            "SET ROLE session still logs in as owner agent, got {rec}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn upgrade_old_grant_alters_and_vaults_once() {
        let _g = sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running_sidecar();
        let dir_a = std::env::temp_dir().join(format!("k2-sql-128-up-a-{}", uuid::Uuid::new_v4()));
        let dir_b = std::env::temp_dir().join(format!("k2-sql-128-up-b-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let id_a = insert_project("sql-128-up-a", &dir_a.to_string_lossy());
        let id_b = insert_project("sql-128-up-b", &dir_b.to_string_lossy());
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &id_a, 1, None, None).expect("create");
        let db_name = created["name"].as_str().expect("name").to_string();
        ops::grant_access(&ops, &secrets, None, &db_name, &id_b, "write", false).expect("grant");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE sql_grants SET agent_secret_ref = NULL WHERE project_id = ?1",
                rusqlite::params![id_b],
            )
            .expect("simulate pre-128 grant");
        }
        let n_helper = ops.pg.lock().expect("pg").helper_sql.len();
        let dsn_b = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("upgrade dsn");
        assert_eq!(
            dsn_url_user(&dsn_b),
            ops::default_agent_role(&id_b),
            "{dsn_b}"
        );
        let helper = ops.pg.lock().expect("pg").helper_sql[n_helper..].join("\n");
        assert!(
            helper.to_ascii_uppercase().contains("ALTER ROLE"),
            "upgrade must ALTER ROLE + vault, got {helper}"
        );
        let sref = grant_secret_ref(&id_b);
        assert!(
            sref.as_deref().is_some_and(|s| s.starts_with("dbsec_")),
            "upgrade must vault, got {sref:?}"
        );
        let n_helper2 = ops.pg.lock().expect("pg").helper_sql.len();
        let dsn_b2 = ops::dsn_for_project(&ops, &secrets, &id_b, 1).expect("second dsn");
        assert_eq!(dsn_url_user(&dsn_b2), dsn_url_user(&dsn_b));
        let helper2 = ops.pg.lock().expect("pg").helper_sql[n_helper2..].join("\n");
        assert!(
            !helper2.to_ascii_uppercase().contains("ALTER ROLE"),
            "second dsn must not rotate, got {helper2}"
        );
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }
}
