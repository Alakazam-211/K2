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
            "INSERT INTO projects (id, name, path, db_agent_access) VALUES (?1, ?2, ?3, 'write')",
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
        let v = ops::create_database(&ops, &secrets, &pid, 1, Some("idem-1"), None)
            .expect("create");
        let s = v.to_string().to_ascii_lowercase();
        assert!(!s.contains("superuser"), "{s}");
        assert!(!s.contains("postgres://postgres"), "{s}");
        assert_eq!(v["role"], "agent");
        assert!(v["dsn"].as_str().unwrap_or("").contains("_agent"));
        let helper = ops.pg.lock().unwrap().helper_sql.join("\n").to_ascii_uppercase();
        assert!(helper.contains("NOSUPERUSER"));
        assert!(helper.contains("NOCREATEDB"));
        assert!(helper.contains("NOBYPASSRLS"));
        assert!(!helper.contains("FORCE ROW LEVEL"));
        // Idempotent --id
        let v2 = ops::create_database(&ops, &secrets, &pid, 1, Some("idem-1"), None)
            .expect("idempotent");
        assert_eq!(v2["existing"], true);
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
        assert!(first["applied"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("0001_init")));
        let second = ops::migrate(&ops, &secrets, &pid, &path, None).expect("second");
        assert_eq!(second["noop"], true);
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
            assert!(!name.contains(&_id_b.replace('-', "_")), "must not mint B's DB");
        });
        let _ = std::fs::remove_dir_all(dir_a);
        let _ = std::fs::remove_dir_all(dir_b);
    }
}
