//! D19/D20 — `/v1/w/<ws>/db` create + migrate, GET status + dump, POST restore.
//! API key + workspace grant + `cap_db` (fail-closed). Create is NOT on hire.

use crate::cli_response::CliResponse;
use crate::routes::http::{V1Capability, V1Principal};
use crate::sql::ops;
use crate::sql::routes::current_ops;
use crate::sql::secrets::FileSecretStore;
use crate::v1_sandboxes::{decode_and_validate_segment, resolve_authorized_workspace, uniform_ws_404};

pub(crate) fn require_db(principal: &V1Principal) -> Result<(), CliResponse> {
    if principal.has_capability(V1Capability::Db) {
        Ok(())
    } else {
        Err(uniform_ws_404())
    }
}

fn ops_err(e: ops::OpsError) -> CliResponse {
    CliResponse {
        status: e.status(),
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": e.code(), "hint": e.hint() },
        })
        .to_string(),
    }
}

fn project_id_for_path(path: &str) -> Result<String, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, path)
        .ok_or_else(uniform_ws_404)
}

fn authorize_ws(principal: &V1Principal, ws_raw: &str) -> Result<String, CliResponse> {
    if let Err(resp) = require_db(principal) {
        return Err(resp);
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return Err(uniform_ws_404());
    };
    resolve_authorized_workspace(principal, &slug)
}

/// `GET /v1/w/<ws>/db` — applied migrations + size. Fail loud if no DB.
pub(crate) fn handle_v1_db_status(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let secrets = FileSecretStore::default();
    match ops::database_status(current_ops(), &secrets, &project_id) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => ops_err(e),
    }
}

/// `POST /v1/w/<ws>/db` — create (NOT on hire).
pub(crate) fn handle_v1_db_create(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: serde_json::Value = match serde_json::from_slice(if body.is_empty() { b"{}" } else { body }) {
        Ok(v) => v,
        Err(e) => {
            return CliResponse::bad_request(format!("invalid JSON body: {e}"));
        }
    };
    let client_id = parsed["clientId"].as_str().or_else(|| parsed["id"].as_str());
    let name = parsed["name"].as_str();
    let access = parsed["access"]
        .as_str()
        .or_else(|| parsed["dbAccess"].as_str())
        .or_else(|| parsed["db_access"].as_str());
    if let Err(e) = ops::persist_db_access(&ws_path, access) {
        return ops_err(e);
    }
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let cap = k2_core::workspace::settings::db_active_cap_for_path(&ws_path);
    let secrets = FileSecretStore::default();
    match ops::create_database(
        current_ops(),
        &secrets,
        &project_id,
        cap,
        client_id,
        name,
    ) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => ops_err(e),
    }
}

/// `POST /v1/w/<ws>/db/migrate`
pub(crate) fn handle_v1_db_migrate(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: serde_json::Value = match serde_json::from_slice(if body.is_empty() { b"{}" } else { body }) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let dir = parsed["dir"].as_str();
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let secrets = FileSecretStore::default();
    match ops::migrate(current_ops(), &secrets, &project_id, &ws_path, dir) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => ops_err(e),
    }
}

/// `GET /v1/w/<ws>/db/dump` — org-box pull (D20).
pub(crate) fn handle_v1_db_dump(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let secrets = FileSecretStore::default();
    match ops::dump(current_ops(), &secrets, &project_id, &ws_path, None) {
        Ok((mut v, bytes)) => {
            if let Some(b) = bytes {
                v["bytesBase64"] = serde_json::Value::String(
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b),
                );
            }
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => ops_err(e),
    }
}

/// `POST /v1/w/<ws>/db/restore` — org-box restore. Jail = `resolve_in_path`.
pub(crate) fn handle_v1_db_restore(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: serde_json::Value = match serde_json::from_slice(if body.is_empty() { b"{}" } else { body }) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let file = parsed["file"].as_str().unwrap_or("").trim();
    if file.is_empty() {
        return CliResponse::bad_request("missing 'file'".to_string());
    }
    let project_id = match project_id_for_path(&ws_path) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let secrets = FileSecretStore::default();
    match ops::restore(current_ops(), &secrets, &project_id, &ws_path, file) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => ops_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::api_keys::ApiCapabilities;

    fn apik_caps(id: &str, grant: Option<&str>, capabilities: ApiCapabilities) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: None,
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: grant.map(str::to_string),
            capabilities,
        })
    }

    #[test]
    fn db_cap_fail_closed_is_uniform_404() {
        k2_core::db::init_for_tests();
        let no_db = apik_caps(
            "k-db-nocap",
            Some("*"),
            ApiCapabilities {
                host_sessions: true,
                canonical_message: true,
                sandboxes: true,
                db: false,
            },
        );
        for call in [
            handle_v1_db_create(&no_db, "any", b"{}"),
            handle_v1_db_migrate(&no_db, "any", b"{}"),
            handle_v1_db_dump(&no_db, "any"),
            handle_v1_db_status(&no_db, "any"),
            handle_v1_db_restore(&no_db, "any", b"{\"file\":\"x.dump\"}"),
        ] {
            assert_eq!(call.status, "404 Not Found", "body={}", call.body);
            assert!(call.body.contains("no such workspace"));
        }
    }

    fn seed_ws(name: &str) -> (std::path::PathBuf, String, String) {
        let dir = std::env::temp_dir().join(format!(
            "k2-v1db-{}-{}",
            name,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, name, path],
            )
            .unwrap();
            k2_core::workspace::handle::backfill_workspace_handles(&conn);
        }
        let handle = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::handle::project_handle_for_path(&conn, &path).expect("handle")
        };
        (dir, path, handle)
    }

    fn seed_running() {
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

    #[test]
    fn get_v1_db_status_missing_db_fails_loud() {
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running();
        let (dir, _path, handle) = seed_ws("v1-status-missing");
        let r = handle_v1_db_status(&V1Principal::Owner, &handle);
        assert_eq!(r.status, "404 Not Found", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["error"]["code"], "not_found", "{v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(
            hint.contains("k2 db create"),
            "missing DB must teach create: {hint}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn get_v1_db_status_after_create_has_migrations_and_size() {
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running();
        let (dir, path, handle) = seed_ws("v1-status-ok");
        let pid = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::agent_identity::resolve_project_id(&conn, &path).expect("pid")
        };
        let fake = Box::leak(Box::new(crate::sql::sysops::FakeSystemOps::baked()));
        crate::sql::routes::with_fake_ops(fake, || {
            let secrets = crate::sql::secrets::FileSecretStore::default();
            ops::create_database(fake, &secrets, &pid, 1, None, None).expect("create");
            let mig = dir.join(".k2/db/migrations");
            std::fs::create_dir_all(&mig).unwrap();
            std::fs::write(mig.join("0001_init.sql"), b"CREATE TABLE t (id int);\n").unwrap();
            ops::migrate(fake, &secrets, &pid, &path, None).expect("migrate");
            let r = handle_v1_db_status(&V1Principal::Owner, &handle);
            assert_eq!(r.status, "200 OK", "body={}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
            assert_eq!(v["ok"], true, "{v}");
            let migrations = v["migrations"].as_array().expect("migrations array");
            assert!(
                migrations
                    .iter()
                    .any(|m| m["version"].as_str() == Some("0001_init")),
                "applied list: {migrations:?}"
            );
            let checksum = migrations[0]["checksum"].as_str().expect("checksum");
            assert!(!checksum.is_empty(), "checksum present: {v}");
            let size = v["sizeBytes"].as_i64().expect("sizeBytes integer");
            assert!(size > 0, "sizeBytes must be present and positive: {v}");
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn v1_restore_jail_rejects_dotdot_and_abs() {
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running();
        let (dir, _path, handle) = seed_ws("v1-restore-jail");
        for (file, needle) in [
            ("../outside.dump", ".."),
            ("/tmp/outside.dump", "absolute"),
        ] {
            let body = serde_json::json!({ "file": file });
            let r = handle_v1_db_restore(&V1Principal::Owner, &handle, body.to_string().as_bytes());
            assert_eq!(r.status, "400 Bad Request", "file={file} body={}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
            assert_eq!(v["error"]["code"], "usage", "file={file} {v}");
            let hint = v["error"]["hint"].as_str().expect("hint");
            assert!(
                hint.contains(needle),
                "file={file} hint={hint} expected {needle}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn v1_restore_happy_path_fresh_workspace() {
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running();
        let (dir, _path, handle) = seed_ws("v1-restore-ok");
        std::fs::create_dir_all(dir.join(".k2/db/dumps")).unwrap();
        std::fs::write(dir.join(".k2/db/dumps/x.dump"), b"FAKE-PG-DUMP").unwrap();
        let fake = Box::leak(Box::new(crate::sql::sysops::FakeSystemOps::baked()));
        crate::sql::routes::with_fake_ops(fake, || {
            let body = serde_json::json!({ "file": ".k2/db/dumps/x.dump" });
            let r = handle_v1_db_restore(&V1Principal::Owner, &handle, body.to_string().as_bytes());
            assert_eq!(r.status, "200 OK", "body={}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
            assert_eq!(v["ok"], true, "{v}");
            assert_eq!(v["restored"], ".k2/db/dumps/x.dump", "{v}");
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn v1_create_access_write_persists() {
        let _g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        seed_running();
        let (dir, path, handle) = seed_ws("v1-access-write");
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path),
            "off"
        );
        let fake = Box::leak(Box::new(crate::sql::sysops::FakeSystemOps::baked()));
        crate::sql::routes::with_fake_ops(fake, || {
            let body = serde_json::json!({ "access": "write" });
            let r = handle_v1_db_create(&V1Principal::Owner, &handle, body.to_string().as_bytes());
            assert_eq!(r.status, "200 OK", "body={}", r.body);
        });
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path),
            "write"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
