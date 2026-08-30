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
    use crate::sql::secrets::MemSecretStore;
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
        assert!(v["dsn"].as_str().unwrap_or("").contains("_agent"));
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
        ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let first = ops::migrate(&ops, &secrets, &pid, &path, None).expect("migrate");
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
        ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        ops::store_create(&ops, &secrets, &pid, "items").expect("coll");
        let doc = serde_json::json!({"n": 1});
        ops::store_put(&ops, &secrets, &pid, "items", "a", &doc).expect("put");
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
        let granted = ops::grant_access(&ops, None, &db_name, &id_b, "read", false).expect("grant");
        assert_eq!(granted["level"], "read");
        assert_eq!(granted["canManage"], false);
        let role = granted["role"].as_str().unwrap();
        assert!(role.contains("_agent"), "{role}");
        assert!(!role.contains("postgres"), "{role}");
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
        let err = ops::grant_access(&ops, Some(&id_b), &db_name, &id_c, "read", false)
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
        let ops = FakeSystemOps::baked();
        let secrets = MemSecretStore::default();
        let created = ops::create_database(&ops, &secrets, &pid, 1, None, None).expect("create");
        let db_name = created["name"].as_str().unwrap().to_string();
        let v = ops::bind_role(Some(&db_name), Some(&pid), "app_reader").expect("bind");
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
        let r = routes::handle_bind(body.to_string().as_bytes());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let out: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        let s = out.to_string().to_ascii_lowercase();
        assert!(!s.contains("password"), "{s}");
        assert!(!s.contains("dsn"), "{s}");
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
        ops::grant_access(&ops, None, &db_a, &id_b, "write", false).expect("grant B");
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

    fn json_body(r: &crate::cli_response::CliResponse) -> serde_json::Value {
        serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {}: {}", e, r.body))
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
        ops::grant_access(fake, None, &db_name, &id_b, "read", false).expect("grant");
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
                assert_eq!(put.status, "403 Forbidden", "{}", put.body);
                let put_err = json_body(&put);
                let put_hint = put_err["error"]["hint"].as_str().expect("put hint");
                assert!(put_hint.contains("sql_grants"), "{put_hint}");
                assert!(!put_hint.contains("db_agent_access"), "{put_hint}");

                let mig = routes::handle_migrate(
                    serde_json::json!({ "project": path_b })
                        .to_string()
                        .as_bytes(),
                );
                assert_eq!(mig.status, "403 Forbidden", "{}", mig.body);
                let mig_err = json_body(&mig);
                let mig_hint = mig_err["error"]["hint"].as_str().expect("mig hint");
                assert!(mig_hint.contains("sql_grants"), "{mig_hint}");
                assert!(!mig_hint.contains("db_agent_access"), "{mig_hint}");
            });
        });
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
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
        });
        let _ = std::fs::remove_dir_all(dir);
    }
}
