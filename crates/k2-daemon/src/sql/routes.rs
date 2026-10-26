//! `/cli/db/*` and `/cli/store/*` handlers.

use std::collections::HashMap;

use crate::caller_workspace::{principal_from_params, request_principal};
use crate::cli_response::CliResponse;
use k2_core::skin::SkinPass;

use super::actor::GuestActor;
use super::identity::{resolve_caller, resolve_caller_params};
use super::ops::{self, OpsError};
use super::secrets::FileSecretStore;
use super::sql_supported;
use super::supervisor;
use super::sysops::{RealSystemOps, SystemOps};

#[cfg(test)]
use super::sysops::FakeSystemOps;

fn err_json(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint.into() },
        })
        .to_string(),
    }
}

fn ops_err(e: OpsError) -> CliResponse {
    err_json(e.status(), e.code(), e.hint())
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn unsupported() -> CliResponse {
    err_json(
        "409 Conflict",
        "unsupported",
        "the SQL sidecar only works on Linux deployments; this daemon is not Linux",
    )
}

/// Create-only passport. `db_agent_access=write` is required for
/// `k2 db create`; list/dsn/store use [`interact_for`] instead.
fn access_for(path: &str, need: &str) -> Result<(), CliResponse> {
    // Owner / no principal: always allowed.
    if request_principal().is_none() {
        return Ok(());
    }
    let mode = k2_core::workspace::settings::db_agent_access_for_path(path);
    let ok = match need {
        "read" => mode == "read" || mode == "write",
        "write" => mode == "write",
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(err_json(
            "403 Forbidden",
            "forbidden",
            format!(
                "workspace db_agent_access is '{mode}' (need {need} to create new databases) — ask your human"
            ),
        ))
    }
}

fn err_forbidden_on_db(need: &str, access: &ops::UnscopedAccess) -> CliResponse {
    let via = access.resolved_via.as_str();
    let hint = format!(
        "sql_grants level is '{level}' (need {need}) on {name} (id={id}) resolvedVia={via} — ask your human",
        level = access.level,
        name = access.name,
        id = access.id,
    );
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "forbidden",
                "hint": hint,
                "dbId": access.id,
                "dbName": access.name,
                "resolvedVia": via,
            },
        })
        .to_string(),
    }
}

/// Interaction gate (list is ungated). Owner token always; else resolve
/// `active_row` once (first owned active, else first grant) and gate
/// **that** row's level — owning it ⇒ write, else that grant. No row →
/// 404, never a passport 403.
fn interact_for(project_id: &str, need: &str) -> Result<(), CliResponse> {
    if request_principal().is_none() {
        return Ok(());
    }
    match ops::unscoped_access(project_id) {
        Some(access) => {
            let ok = match need {
                "read" => access.level == "read" || access.level == "write",
                "write" => access.level == "write",
                _ => false,
            };
            if ok {
                Ok(())
            } else {
                Err(err_forbidden_on_db(need, &access))
            }
        }
        None => Err(err_json(
            "404 Not Found",
            "not_found",
            "no database for this workspace — ask your human to create one",
        )),
    }
}

fn cap_for(path: &str) -> u32 {
    k2_core::workspace::settings::db_active_cap_for_path(path)
}

/// Injected ops for tests; production uses RealSystemOps.
pub(crate) fn current_ops() -> &'static dyn SystemOps {
    #[cfg(test)]
    {
        TEST_OPS.with(|c| {
            if c.borrow().is_some() {
                // Safety: tests install a leaked Fake for the thread.
            }
        });
        if let Some(ops) = test_ops() {
            return ops;
        }
    }
    &RealSystemOps
}

/// K2 vault Q23: rotate DB role passwords once they moved into the vault.
pub fn rotate_after_vault_adoption() -> Vec<String> {
    ops::rotate_db_role_passwords(ops())
}

/// An App handover: new passwords for database `name`'s roles; the new
/// agent DSN (`ops::rotate_database_for_handover`).
pub fn rotate_for_handover(name: &str) -> Result<(Vec<String>, String), String> {
    ops::rotate_database_for_handover(ops(), name)
}

fn ops() -> &'static dyn SystemOps {
    current_ops()
}

#[cfg(test)]
thread_local! {
    static TEST_OPS: std::cell::RefCell<Option<&'static dyn SystemOps>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn test_ops() -> Option<&'static dyn SystemOps> {
    TEST_OPS.with(|c| *c.borrow())
}

#[cfg(test)]
#[allow(dead_code)]
pub fn with_fake_ops<R>(fake: &'static FakeSystemOps, f: impl FnOnce() -> R) -> R {
    with_test_ops(fake, f)
}

/// Route handlers on any injected ops (the fake, or the real-Postgres
/// smoke's throwaway cluster).
#[cfg(test)]
#[allow(dead_code)]
pub fn with_test_ops<R>(ops: &'static dyn SystemOps, f: impl FnOnce() -> R) -> R {
    TEST_OPS.with(|c| *c.borrow_mut() = Some(ops));
    let r = f();
    TEST_OPS.with(|c| *c.borrow_mut() = None);
    r
}

pub fn handle_status(params: &HashMap<String, String>) -> CliResponse {
    let health = params.get("health").map(String::as_str) == Some("1");
    ok_json(supervisor::status_json(health))
}

pub fn handle_doctor(_params: &HashMap<String, String>) -> CliResponse {
    ok_json(supervisor::doctor_with(ops()))
}

pub fn handle_server_enable(body: &[u8]) -> CliResponse {
    if !body.is_empty() {
        if let Err(e) = serde_json::from_slice::<serde_json::Value>(body) {
            return CliResponse::bad_request(format!("invalid JSON body: {e}"));
        }
    }
    if !sql_supported() {
        return unsupported();
    }
    match supervisor::enable_with(ops()) {
        Ok(v) => ok_json(v),
        Err(e) => err_json(e.status_code(), e.code(), e.hint()),
    }
}

pub fn handle_server_disable(_body: &[u8]) -> CliResponse {
    if !sql_supported() {
        return unsupported();
    }
    match supervisor::disable_with(ops()) {
        Ok(()) => ok_json(serde_json::json!({ "ok": true, "state": "disabled" })),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

pub fn handle_server_uninstall(_body: &[u8]) -> CliResponse {
    if !sql_supported() {
        return unsupported();
    }
    match supervisor::disable_with(ops()) {
        Ok(()) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM sql_server WHERE id = 1", []); let _ = conn.execute("DELETE FROM sql_clusters", []);
            ok_json(serde_json::json!({ "ok": true, "state": "not-installed" }))
        }
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// `--major` other than the box default is a version choice: an agent
/// passport needs "Allow this agent to manage Postgres versions" (S6). The
/// owner token and people logins pass (the route's own floor already ran).
pub(crate) fn may_choose_major(project_id: &str, major: u32) -> Result<(), CliResponse> {
    let (default, _) = super::cluster::default_major();
    if major == default {
        return Ok(());
    }
    let agent = crate::caller_workspace::request_principal().is_some();
    if agent && !super::versions_access::switch_on(project_id) {
        return Err(err_json(
            "403 Forbidden",
            "db_upgrade_off",
            format!(
                "creating on Postgres {major} (not this box's default {default}) needs \"Allow this agent to manage Postgres versions\" — ask your human to turn it on, or leave out --major"
            ),
        ));
    }
    Ok(())
}

/// `POST /cli/db/server/default-major {"major": N}` (owner only: the
/// `/cli/db/server/` prefix): what `k2 db create` uses without --major.
pub fn handle_server_default_major(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct B {
        major: u32,
    }
    let b: B = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return err_json("400 Bad Request", "usage", format!("body is {{\"major\": N}}: {e}")),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let installed: bool = conn
        .query_row("SELECT COUNT(*) FROM sql_server WHERE id = 1", [], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .unwrap_or(false);
    if !installed {
        return err_json("409 Conflict", "not_ready", "the SQL sidecar is not enabled (k2 db enable)");
    }
    match super::cluster::set_default_major_in(&conn, b.major) {
        Ok(()) => ok_json(serde_json::json!({ "ok": true, "defaultMajor": b.major, "pinned": true })),
        Err(r) => err_json("409 Conflict", r.code(), r.hint()),
    }
}

pub fn handle_doctor_run(_body: &[u8]) -> CliResponse {
    ok_json(supervisor::doctor_with(ops()))
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CreateBody {
    project: String,
    client_id: Option<String>,
    name: Option<String>,
    /// `off` | `read` | `write` — persist create-only `db_agent_access`.
    #[serde(alias = "dbAccess", alias = "db_access")]
    access: Option<String>,
    /// Disposable cap-exempt test database (P11).
    test: bool,
    /// `k2 db create --major N` (0.46.3): the Postgres major to create on.
    /// None = the box default (`sql_server.default_major`; unset, the 5432
    /// cluster's major; 17 only on a box with no Postgres yet).
    major: Option<u32>,
}

fn truthy_param(params: &HashMap<String, String>, key: &str) -> bool {
    params
        .get(key)
        .map(|s| {
            let t = s.trim();
            t == "1" || t.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

fn grant_path_blocks_test(project_id: &str) -> bool {
    matches!(
        ops::unscoped_access(project_id),
        Some(a) if a.resolved_via == ops::ResolvedVia::Grant
    )
}

fn test_db_forbidden() -> CliResponse {
    err_json(
        "403 Forbidden",
        "forbidden",
        "k2 db --test is owner or owning-workspace cell only — Connect members and grants cannot mint or fetch a migrator DSN",
    )
}

pub fn handle_create(body: &[u8]) -> CliResponse {
    let b: CreateBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let (path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if b.test {
        if b.name
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
        {
            return err_json(
                "400 Bad Request",
                "usage",
                "k2 db create --test does not take --name (name is {workspace}_test)",
            );
        }
        if grant_path_blocks_test(&project_id) {
            return test_db_forbidden();
        }
    }
    if let Err(resp) = access_for(&path, "write") {
        return resp;
    }
    if let Err(e) = ops::persist_db_access(&path, b.access.as_deref()) {
        return ops_err(e);
    }
    let secrets = FileSecretStore::default();
    if let Some(m) = b.major {
        if let Err(resp) = may_choose_major(&project_id, m) {
            return resp;
        }
    }
    let created = if b.test {
        ops::create_test_database_on(
            ops(),
            &secrets,
            &project_id,
            cap_for(&path),
            b.client_id.as_deref(),
            b.major,
        )
    } else {
        ops::create_database_on(
            ops(),
            &secrets,
            &project_id,
            cap_for(&path),
            b.client_id.as_deref(),
            b.name.as_deref(),
            b.major,
        )
    };
    match created {
        Ok(v) => {
            let s = v.to_string().to_ascii_lowercase();
            if s.contains("superuser") || s.contains("postgres://postgres") {
                return err_json(
                    "500 Internal Server Error",
                    "engine",
                    "refusing to return a superuser DSN",
                );
            }
            ok_json(v)
        }
        Err(e) => ops_err(e),
    }
}

pub fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    match resolve_caller_params(params) {
        Ok((path, project_id)) => {
            // Passport does not gate list — owned + granted rows, or empty.
            let mut v = ops::list_databases(&project_id);
            v["cap"] = serde_json::json!(cap_for(&path));
            ok_json(v)
        }
        Err(resp) => {
            let no_identity = principal_from_params(params).is_none()
                && params
                    .get("project")
                    .or_else(|| params.get("project_path"))
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true);
            if no_identity {
                ok_json(ops::catalog_json(None))
            } else {
                resp
            }
        }
    }
}

pub fn handle_dsn(params: &HashMap<String, String>) -> CliResponse {
    let (path, project_id) = match resolve_caller_params(params) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let test = truthy_param(params, "test");
    let actor = params
        .get("actor")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let migrator = if let Some(ref a) = actor {
        if a != "migrator" {
            return err_json(
                "400 Bad Request",
                "usage",
                "actor must be 'migrator' (or omit for the agent DSN)",
            );
        }
        true
    } else {
        false
    };
    if test || migrator {
        if grant_path_blocks_test(&project_id) {
            return test_db_forbidden();
        }
    } else if let Err(resp) = interact_for(&project_id, "read") {
        return resp;
    }
    let secrets = FileSecretStore::default();
    match ops::dsn_for_project_opts(ops(), &secrets, &project_id, cap_for(&path), test, migrator) {
        Ok(v) => {
            if !test {
                let s = v.to_string();
                if s.contains("migratorDsn") || s.contains("_migrator:") {
                    return err_json(
                        "500 Internal Server Error",
                        "engine",
                        "refusing to return a migrator DSN on the live path",
                    );
                }
            }
            if truthy_param(params, "ref") && !migrator {
                return dsn_ref(&project_id, &v, test);
            }
            ok_json(v)
        }
        Err(e) => ops_err(e),
    }
}

/// `k2 db dsn --ref` (K2 vault Q12): keep the DSN as a `user` secret
/// `DATABASE_URL` in the workspace and return only its `k2v://`
/// reference (for `.env.k2`, publish bindings). The plain `k2 db dsn`
/// keeps returning the agent's own DSN.
fn dsn_ref(project_id: &str, v: &serde_json::Value, test: bool) -> CliResponse {
    use k2_vault::SecretStore as _;
    let Some(dsn) = v.get("dsn").and_then(|d| d.as_str()).filter(|d| !d.is_empty()) else {
        return err_json("500 Internal Server Error", "engine", "no DSN to keep");
    };
    let vault = match crate::vault::get() {
        Ok(x) => x,
        Err(e) => return err_json("503 Service Unavailable", "vault_not_open", e.hint),
    };
    let name = if test { "DATABASE_URL_TEST" } else { "DATABASE_URL" };
    let mut r = k2_vault::PutRequest::new(k2_vault::Namespace::User, k2_vault::Scope::workspace(project_id), name, k2_vault::Kind::Dsn);
    r.default_delivery = Some(k2_vault::Delivery::Env);
    r.delivery_ceiling = Some(k2_vault::Delivery::Env);
    r.origin = "system:db".into();
    r.description = "this workspace's database (k2 db dsn --ref)".into();
    r.mode = k2_vault::PutMode::Upsert;
    r.idempotent = true;
    r.human_given_override = Some(false);
    match vault.put(r, k2_vault::SecretBytes::from_slice(dsn.as_bytes()), &k2_vault::Actor::System("db".into())) {
        Ok(out) => ok_json(serde_json::json!({
            "ok": true,
            "ref": crate::vault::display_ref(&out.meta),
            "name": name,
        })),
        Err(e) => err_json("500 Internal Server Error", "vault", e.hint),
    }
}

/// `k2 app install … --pg-major M` / a Seed's `requires.postgres` with no
/// workspace database yet (prd-app-pg-choice-dualrun-v1 PA7, Q2): create
/// the workspace's database on `major` and vault its `DATABASE_URL`
/// reference, as `k2 db create --major M` then `k2 db dsn --ref` do.
/// `agent` = the caller is an agent passport: then, as `k2 db create`,
/// `db_agent_access` must be `write` and a major other than the box
/// default needs the versions switch (PA8). Answers `{name, major, ref}`.
pub(crate) fn create_for_app_install(project_id: &str, path: &str, major: u32, agent: bool) -> Result<serde_json::Value, (u16, &'static str, String)> {
    if !sql_supported() {
        return Err((409, "requires_unmet", "the App needs a Postgres database, and K2 runs Postgres on Linux boxes only".into()));
    }
    if agent {
        let mode = k2_core::workspace::settings::db_agent_access_for_path(path);
        if mode != "write" {
            return Err((
                403,
                "forbidden",
                format!("the App needs a database and workspace db_agent_access is '{mode}' (need write to create one) — ask your human"),
            ));
        }
        let (default, _) = super::cluster::default_major();
        if major != default && !super::versions_access::switch_on(project_id) {
            return Err((
                403,
                "db_upgrade_off",
                format!(
                    "creating the App's database on Postgres {major} (not this box's default {default}) needs \"Allow this agent to manage Postgres versions\" — ask your human to turn it on, or install with --pg-major {default} if the App allows it"
                ),
            ));
        }
    }
    let secrets = FileSecretStore::default();
    let v = ops::create_database_on(ops(), &secrets, project_id, cap_for(path), None, None, Some(major))
        .map_err(|e| (e.status().get(..3).and_then(|s| s.parse().ok()).unwrap_or(500), e.code(), e.hint().to_string()))?;
    let name = v["name"].as_str().unwrap_or_default().to_string();
    let r = dsn_ref(project_id, &v, false);
    let body: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    if !r.status.starts_with("200") {
        return Err((
            500,
            "vault",
            format!("database {name} was created on Postgres {major}, but its DATABASE_URL reference wasn't kept: {} — run k2 db dsn --ref", body["hint"].as_str().unwrap_or(&r.body)),
        ));
    }
    Ok(serde_json::json!({ "name": name, "major": v["major"], "ref": body["ref"], "created": true }))
}

/// `k2 db upgrade`'s re-point (V35): the same `k2 db dsn --ref` upsert,
/// from the record (already on the new cluster), under the same ref.
pub(crate) fn dsn_ref_for_move(project_id: &str, v: &serde_json::Value, test: bool) -> CliResponse {
    dsn_ref(project_id, v, test)
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ProjectBody {
    project: String,
    dir: Option<String>,
    #[serde(rename = "out")]
    out: Option<String>,
    file: Option<String>,
    yes: Option<bool>,
    test: bool,
}

pub fn handle_migrate(body: &[u8]) -> CliResponse {
    let b: ProjectBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let (path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    let secrets = FileSecretStore::default();
    match ops::migrate(ops(), &secrets, &project_id, &path, b.dir.as_deref()) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_dump(body: &[u8]) -> CliResponse {
    let b: ProjectBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let (path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    let secrets = FileSecretStore::default();
    match ops::dump(ops(), &secrets, &project_id, &path, b.out.as_deref()) {
        Ok((v, _)) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_restore(body: &[u8]) -> CliResponse {
    let b: ProjectBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let file = b.file.as_deref().unwrap_or("").trim();
    if file.is_empty() {
        return err_json("400 Bad Request", "usage", "missing 'file'");
    }
    let (path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    let secrets = FileSecretStore::default();
    match ops::restore(ops(), &secrets, &project_id, &path, file) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_drop(body: &[u8]) -> CliResponse {
    let b: ProjectBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    if b.yes != Some(true) {
        return err_json(
            "400 Bad Request",
            "usage",
            "drop requires {\"yes\": true} (CLI: k2 db drop --yes)",
        );
    }
    let (_path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if b.test {
        if grant_path_blocks_test(&project_id) {
            return test_db_forbidden();
        }
        match ops::drop_test_database(ops(), &project_id) {
            Ok(v) => ok_json(v),
            Err(e) => ops_err(e),
        }
    } else {
        if let Err(resp) = interact_for(&project_id, "write") {
            return resp;
        }
        match ops::drop_database(ops(), &project_id) {
            Ok(v) => ok_json(v),
            Err(e) => ops_err(e),
        }
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct StoreBody {
    project: String,
    name: Option<String>,
    id: Option<String>,
    json: Option<serde_json::Value>,
    #[serde(rename = "where")]
    where_: Option<serde_json::Value>,
    limit: Option<u32>,
}

fn store_ident(body: &[u8]) -> Result<(String, String, StoreBody), CliResponse> {
    let b: StoreBody = serde_json::from_slice(body).map_err(|e| {
        err_json(
            "400 Bad Request",
            "usage",
            format!("invalid JSON body: {e}"),
        )
    })?;
    let (path, project_id) = resolve_caller(&b.project)?;
    Ok((path, project_id, b))
}

pub fn handle_store_create(body: &[u8]) -> CliResponse {
    let (_path, project_id, b) = match store_ident(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    let name = b.name.as_deref().unwrap_or("");
    let secrets = FileSecretStore::default();
    match ops::store_create(ops(), &secrets, &project_id, name) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_store_list(params: &HashMap<String, String>) -> CliResponse {
    let (_path, project_id) = match resolve_caller_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "read") {
        return resp;
    }
    let secrets = FileSecretStore::default();
    match ops::store_list(ops(), &secrets, &project_id, false) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_store_put(body: &[u8]) -> CliResponse {
    let (_path, project_id, b) = match store_ident(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    let name = b.name.as_deref().unwrap_or("");
    let id = b.id.as_deref().unwrap_or("");
    let doc = b.json.unwrap_or(serde_json::Value::Null);
    let secrets = FileSecretStore::default();
    match ops::store_put(ops(), &secrets, &project_id, name, id, &doc) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_store_get(params: &HashMap<String, String>) -> CliResponse {
    let (_path, project_id) = match resolve_caller_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "read") {
        return resp;
    }
    let name = params.get("name").map(String::as_str).unwrap_or("");
    let id = params.get("id").map(String::as_str).unwrap_or("");
    let secrets = FileSecretStore::default();
    match ops::store_get(ops(), &secrets, &project_id, name, id, false) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_store_query(params: &HashMap<String, String>) -> CliResponse {
    let (_path, project_id) = match resolve_caller_params(params) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "read") {
        return resp;
    }
    let name = params.get("name").map(String::as_str).unwrap_or("");
    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let secrets = FileSecretStore::default();
    match ops::store_query(ops(), &secrets, &project_id, name, limit, false) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

/// Skin store GET list. Owner/Connect use [`handle_store_list`].
pub fn handle_store_list_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_store_list(params, &pass),
        None => handle_store_list(params),
    }
}

pub fn handle_store_get_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_store_get(params, &pass),
        None => handle_store_get(params),
    }
}

pub fn handle_store_query_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_store_query(params, &pass),
        None => handle_store_query(params),
    }
}

fn need_skin_workspace(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    params
        .get("workspace")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliResponse::bad_request("missing workspace query parameter"))
}

/// Store doors: session passes only. The store is K2's own table; the
/// door's rule is the room's store cap, not the app's row security.
fn skin_store_session(pass: &SkinPass) -> Result<(), CliResponse> {
    if !pass.session {
        return Err(crate::skin_routes::platform_store_forbidden());
    }
    Ok(())
}

/// Rows doors run like `/cli/db/query`: session pass with a uuid
/// principal; principal and actor (from the pass only) are locked into
/// the connection before SET ROLE.
fn skin_dump_actor(pass: &SkinPass) -> Result<GuestActor, CliResponse> {
    skin_store_session(pass)?;
    GuestActor::from_pass(pass).map_err(ops_err)
}

fn skin_store_room(
    params: &HashMap<String, String>,
    pass: &SkinPass,
) -> Result<String, CliResponse> {
    let ws = need_skin_workspace(params)?;
    let resolved = crate::fs_routes::resolve_skin_workspace(pass, &ws)?;
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::STORE_READ) {
        return Err(crate::skin_routes::missing_cap_response(
            crate::skin_routes::STORE_READ,
        ));
    }
    Ok(resolved.project_id)
}

fn handle_skin_store_list(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    if let Err(e) = skin_store_session(pass) {
        return e;
    }
    let project_id = match skin_store_room(params, pass) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let secrets = FileSecretStore::default();
    match ops::store_list(ops(), &secrets, &project_id, true) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

fn handle_skin_store_get(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    if let Err(e) = skin_store_session(pass) {
        return e;
    }
    let project_id = match skin_store_room(params, pass) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let name = params.get("name").map(String::as_str).unwrap_or("");
    let id = params.get("id").map(String::as_str).unwrap_or("");
    let secrets = FileSecretStore::default();
    match ops::store_get(ops(), &secrets, &project_id, name, id, true) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

fn handle_skin_store_query(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    if let Err(e) = skin_store_session(pass) {
        return e;
    }
    let project_id = match skin_store_room(params, pass) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let name = params.get("name").map(String::as_str).unwrap_or("");
    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let secrets = FileSecretStore::default();
    match ops::store_query(ops(), &secrets, &project_id, name, limit, true) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

fn dump_linux_gate() -> Result<(), CliResponse> {
    if sql_supported() {
        return Ok(());
    }
    #[cfg(test)]
    if test_ops().is_some() {
        return Ok(());
    }
    Err(unsupported())
}

fn dump_owner_project(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    if let Some(ws) = params
        .get("workspace")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return resolve_caller(ws).map(|(_, id)| id);
    }
    resolve_caller_params(params).map(|(_, id)| id)
}

fn skin_dump_room(
    params: &HashMap<String, String>,
    pass: &SkinPass,
    cap: &str,
) -> Result<String, CliResponse> {
    let ws = need_skin_workspace(params)?;
    let resolved = crate::fs_routes::resolve_skin_workspace(pass, &ws)?;
    if !pass.has_cap_in_room(&resolved.project_id, cap) {
        return Err(crate::skin_routes::missing_cap_response(cap));
    }
    // A move of this workspace's database is pausing its guest doors.
    if let Some(r) = super::pgmove::k2::maintenance_response(&resolved.project_id) {
        return Err(r);
    }
    Ok(resolved.project_id)
}

fn skin_dump_workspace_body(
    workspace: &str,
    pass: &SkinPass,
    cap: &str,
) -> Result<String, CliResponse> {
    let ws = workspace.trim();
    if ws.is_empty() {
        return Err(CliResponse::bad_request("missing workspace"));
    }
    let resolved = crate::fs_routes::resolve_skin_workspace(pass, ws)?;
    if !pass.has_cap_in_room(&resolved.project_id, cap) {
        return Err(crate::skin_routes::missing_cap_response(cap));
    }
    // A move of this workspace's database is pausing its guest doors.
    if let Some(r) = super::pgmove::k2::maintenance_response(&resolved.project_id) {
        return Err(r);
    }
    Ok(resolved.project_id)
}

pub fn handle_dump_tables(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match dump_owner_project(params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "read") {
        return resp;
    }
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    let secrets = FileSecretStore::default();
    match ops::dump_list_tables(ops(), &secrets, &project_id, None) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_dump_rows(params: &HashMap<String, String>) -> CliResponse {
    let project_id = match dump_owner_project(params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "read") {
        return resp;
    }
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    dump_rows_for(params, &project_id, None)
}

pub fn handle_dump_tables_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_dump_tables(params, &pass),
        None => handle_dump_tables(params),
    }
}

pub fn handle_dump_rows_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_dump_rows(params, &pass),
        None => handle_dump_rows(params),
    }
}

fn handle_skin_dump_tables(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let guest = match skin_dump_actor(pass) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let project_id = match skin_dump_room(params, pass, crate::skin_routes::STORE_READ) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    let secrets = FileSecretStore::default();
    match ops::dump_list_tables(ops(), &secrets, &project_id, Some(&guest)) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

fn handle_skin_dump_rows(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let guest = match skin_dump_actor(pass) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let project_id = match skin_dump_room(params, pass, crate::skin_routes::STORE_READ) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let table = params.get("table").map(String::as_str).unwrap_or("");
    if table.trim().is_empty() {
        return CliResponse::bad_request("missing table");
    }
    if params.contains_key("id") {
        let id = params.get("id").map(String::as_str).unwrap_or("");
        if id.trim().is_empty() {
            return CliResponse::bad_request("missing id");
        }
    }
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    dump_rows_for(params, &project_id, Some(&guest))
}

fn dump_rows_for(
    params: &HashMap<String, String>,
    project_id: &str,
    guest: Option<&GuestActor>,
) -> CliResponse {
    let table = params.get("table").map(String::as_str).unwrap_or("");
    if table.trim().is_empty() {
        return CliResponse::bad_request("missing table");
    }
    let secrets = FileSecretStore::default();
    if params.contains_key("id") {
        let id = params.get("id").map(String::as_str).unwrap_or("");
        if id.trim().is_empty() {
            return CliResponse::bad_request("missing id");
        }
        match ops::dump_get_row(ops(), &secrets, project_id, table, id, guest) {
            Ok(v) => ok_json(v),
            Err(e) => ops_err(e),
        }
    } else {
        let limit = params
            .get("limit")
            .and_then(|s| s.parse().ok())
            .unwrap_or(50);
        match ops::dump_list_rows(ops(), &secrets, project_id, table, limit, guest) {
            Ok(v) => ok_json(v),
            Err(e) => ops_err(e),
        }
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct DumpBody {
    workspace: Option<String>,
    project: Option<String>,
    table: Option<String>,
    id: Option<String>,
    row: Option<serde_json::Value>,
}

fn parse_dump_body(body: &[u8]) -> Result<DumpBody, CliResponse> {
    serde_json::from_slice(body).map_err(|e| {
        err_json(
            "400 Bad Request",
            "usage",
            format!("invalid JSON body: {e}"),
        )
    })
}

fn dump_owner_project_body(b: &DumpBody) -> Result<String, CliResponse> {
    let claim = b
        .workspace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            b.project
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("");
    resolve_caller(claim).map(|(_, id)| id)
}

pub fn handle_dump_insert(body: &[u8]) -> CliResponse {
    let b = match parse_dump_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let project_id = match dump_owner_project_body(&b) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    dump_insert_for(&b, &project_id, None)
}

pub fn handle_dump_update(body: &[u8]) -> CliResponse {
    let b = match parse_dump_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let project_id = match dump_owner_project_body(&b) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    dump_update_for(&b, &project_id, None)
}

pub fn handle_dump_delete(body: &[u8]) -> CliResponse {
    let b = match parse_dump_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let project_id = match dump_owner_project_body(&b) {
        Ok(id) => id,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    dump_delete_for(&b, &project_id, None)
}

pub fn handle_dump_insert_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_dump_write(body, &pass, "insert"),
        None => handle_dump_insert(body),
    }
}

pub fn handle_dump_update_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_dump_write(body, &pass, "update"),
        None => handle_dump_update(body),
    }
}

pub fn handle_dump_delete_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_dump_write(body, &pass, "delete"),
        None => handle_dump_delete(body),
    }
}

fn handle_skin_dump_write(body: &[u8], pass: &SkinPass, kind: &str) -> CliResponse {
    let guest = match skin_dump_actor(pass) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let b = match parse_dump_body(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ws = b.workspace.as_deref().unwrap_or("");
    let project_id = match skin_dump_workspace_body(ws, pass, crate::skin_routes::STORE_WRITE) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    match kind {
        "insert" => dump_insert_for(&b, &project_id, Some(&guest)),
        "update" => dump_update_for(&b, &project_id, Some(&guest)),
        _ => dump_delete_for(&b, &project_id, Some(&guest)),
    }
}

fn dump_insert_for(b: &DumpBody, project_id: &str, guest: Option<&GuestActor>) -> CliResponse {
    let table = b.table.as_deref().unwrap_or("");
    if table.trim().is_empty() {
        return CliResponse::bad_request("missing table");
    }
    let Some(row) = b.row.as_ref() else {
        return CliResponse::bad_request("row must be a JSON object");
    };
    let secrets = FileSecretStore::default();
    match ops::dump_insert(ops(), &secrets, project_id, table, row, guest) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

fn dump_update_for(b: &DumpBody, project_id: &str, guest: Option<&GuestActor>) -> CliResponse {
    let table = b.table.as_deref().unwrap_or("");
    if table.trim().is_empty() {
        return CliResponse::bad_request("missing table");
    }
    let id = b.id.as_deref().unwrap_or("");
    if id.trim().is_empty() {
        return CliResponse::bad_request("missing id");
    }
    let Some(row) = b.row.as_ref() else {
        return CliResponse::bad_request("row must be a JSON object");
    };
    let secrets = FileSecretStore::default();
    match ops::dump_update(ops(), &secrets, project_id, table, id, row, guest) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

fn dump_delete_for(b: &DumpBody, project_id: &str, guest: Option<&GuestActor>) -> CliResponse {
    let table = b.table.as_deref().unwrap_or("");
    if table.trim().is_empty() {
        return CliResponse::bad_request("missing table");
    }
    let id = b.id.as_deref().unwrap_or("");
    if id.trim().is_empty() {
        return CliResponse::bad_request("missing id");
    }
    let secrets = FileSecretStore::default();
    match ops::dump_delete(ops(), &secrets, project_id, table, id, guest) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_store_rm(body: &[u8]) -> CliResponse {
    let (_path, project_id, b) = match store_ident(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    let secrets = FileSecretStore::default();
    match ops::store_rm(
        ops(),
        &secrets,
        &project_id,
        b.name.as_deref().unwrap_or(""),
        b.id.as_deref().unwrap_or(""),
    ) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

pub fn handle_store_drop(body: &[u8]) -> CliResponse {
    let (_path, project_id, b) = match store_ident(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(resp) = interact_for(&project_id, "write") {
        return resp;
    }
    let secrets = FileSecretStore::default();
    match ops::store_drop(
        ops(),
        &secrets,
        &project_id,
        b.name.as_deref().unwrap_or(""),
    ) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

/// Resolve a workspace spec (name | path | UUID) to `project_id` — the
/// **grantee** (a target resource, not caller identity).
fn resolve_grantee(project: &str) -> Result<String, CliResponse> {
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return Err(crate::workspace_routes::workspace_not_found_response(
            project,
        ));
    };
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    project_id.ok_or_else(|| {
        err_json(
            "404 Not Found",
            "not_found",
            format!("workspace not registered: {path}"),
        )
    })
}

fn caller_project_id() -> Option<String> {
    request_principal().map(|p| p.workspace_uuid.clone())
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct GrantBody {
    /// Grantee workspace (name | path | UUID). Not caller identity.
    project: String,
    db: String,
    level: String,
    manage: Option<bool>,
    can_manage: Option<bool>,
    /// Replaces the stored relation fence. Empty or absent clears it.
    #[serde(default)]
    relations: Vec<String>,
}

pub fn handle_grant(body: &[u8]) -> CliResponse {
    let b: GrantBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    if b.project.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'project' — the workspace to grant (name | path | UUID)",
        );
    }
    if b.db.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'db' — database id or name ('k2 db list')",
        );
    }
    if b.level.trim().is_empty() {
        return err_json("400 Bad Request", "usage", "missing 'level' — read | write");
    }
    let grantee = match resolve_grantee(&b.project) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let can_manage = b.manage.or(b.can_manage).unwrap_or(false);
    let secrets = FileSecretStore::default();
    match ops::grant_access(
        ops(),
        &secrets,
        caller_project_id().as_deref(),
        &b.db,
        &grantee,
        &b.level,
        can_manage,
        &b.relations,
    ) {
        Ok(v) => {
            let s = v.to_string().to_ascii_lowercase();
            if s.contains("superuser") || s.contains("postgres://postgres") {
                return err_json(
                    "500 Internal Server Error",
                    "engine",
                    "refusing to return a superuser DSN",
                );
            }
            ok_json(v)
        }
        Err(e) => ops_err(e),
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RevokeBody {
    project: String,
    db: String,
}

pub fn handle_revoke(body: &[u8]) -> CliResponse {
    let b: RevokeBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    if b.project.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'project' — the workspace to revoke (name | path | UUID)",
        );
    }
    if b.db.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'db' — database id or name ('k2 db list')",
        );
    }
    let grantee = match resolve_grantee(&b.project) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match ops::revoke_access(ops(), caller_project_id().as_deref(), &b.db, &grantee) {
        Ok(v) => ok_json(v),
        Err(e) => ops_err(e),
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct BindBody {
    project: String,
    db: Option<String>,
    role: String,
}

/// POST `/cli/db/bind` — owner/admin. Persist bind_role. Never prints secrets.
pub fn handle_bind(body: &[u8]) -> CliResponse {
    let b: BindBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    if b.role.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'role' — k2 db bind --role <pg_role>",
        );
    }
    let project_id = if b.project.trim().is_empty() {
        caller_project_id()
    } else {
        match resolve_caller(&b.project) {
            Ok((_path, id)) => Some(id),
            Err(resp) => return resp,
        }
    };
    match ops::bind_role(ops(), b.db.as_deref(), project_id.as_deref(), &b.role) {
        Ok(v) => {
            let s = v.to_string().to_ascii_lowercase();
            if s.contains("password") || s.contains("\"dsn\"") || s.contains("dbsec_") {
                return err_json(
                    "500 Internal Server Error",
                    "engine",
                    "refusing to return secrets from bind",
                );
            }
            ok_json(v)
        }
        Err(e) => ops_err(e),
    }
}

fn session_token_required() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: r#"{"error":"session token required"}"#.to_string(),
    }
}

/// `POST /cli/db/query`. Session token only. One parsed statement, run as
/// the migrator with a lock row then SET ROLE. GET must not reach this.
pub fn handle_db_query(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    if let Err(r) = dump_linux_gate() {
        return r;
    }
    let Some(pass) = skin else {
        return session_token_required();
    };
    if !pass.session {
        return crate::skin_routes::platform_store_forbidden();
    }
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = v
        .get("workspace")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if workspace.is_empty() {
        return CliResponse::bad_request("missing workspace");
    }
    let sql = v.get("sql").and_then(|x| x.as_str()).unwrap_or("");
    if sql.trim().is_empty() {
        return CliResponse::bad_request("missing sql");
    }
    let params = match v.get("params") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => items.clone(),
        Some(_) => {
            return ops_err(OpsError::Usage("params must be an array".into()));
        }
    };
    let stmt = match super::query::classify_guest_sql(sql) {
        Ok(s) => s,
        Err(e) => return ops_err(e),
    };
    // Principal and actor come from the pass only. Body fields such as
    // `actor` / `principal` are never read.
    let actor = match GuestActor::from_pass(&pass) {
        Ok(a) => a,
        Err(e) => return ops_err(e),
    };
    let cap = if stmt.read {
        crate::skin_routes::STORE_READ
    } else {
        crate::skin_routes::STORE_WRITE
    };
    let project_id = match skin_dump_workspace_body(&workspace, &pass, cap) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let secrets = FileSecretStore::default();
    match ops::exec_guest_query(ops(), &secrets, &project_id, &actor, &stmt, &params) {
        Ok(v) => {
            let rendered = v.to_string();
            if rendered.contains("postgres://") || rendered.contains("\"dsn\"") {
                return err_json(
                    "500 Internal Server Error",
                    "engine",
                    "refusing to return a database URL",
                );
            }
            ok_json(v)
        }
        Err(e) => ops_err(e),
    }
}
