//! `/cli/mail/address/*` — ADDRESS-concern handlers (mail slice S3:
//! minting, ownership, caps, idempotent `--id`, retire).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! (PRD §10 / §7): create/delete = WORKSPACE token (`token_ok` in the
//! dispatcher arm — agents mint their own addresses), POST-only
//! (`require_post` + `post_allowed`, house rule
//! feedback_post_only_route_guards). The OWNING workspace
//! (`owner_project_id`) is resolved SERVER-SIDE from the caller's
//! `project` (workspace name | path | uuid) via `resolve_workspace` +
//! `resolve_project_id` — a raw project id is NEVER trusted from the
//! body, and ownership checks compare against the RESOLVED id (the
//! declarative-identity model every workspace-scoped route uses). The
//! owner's Settings table retires any address by addressing the
//! HOLDER's workspace (`holderProjectId` from the `?all=true` list).
//!
//! Cap enforcement reads
//! `k2_core::workspace::settings::mail_address_cap_for_path` — the
//! ONLY sanctioned read path (0 = unlimited); over cap → the §11.1.5
//! BINDING error text (code `cap_reached`).
//!
//! `address/list` serves BOTH audiences like S2's `domain/list`:
//! `?project=<ws>` = the caller's ACTIVE addresses + cap usage
//! (`k2 mail addresses`); `?all=true` = the owner table (every
//! address, retired included, with holders — the Settings→Email
//! addresses table). Reads ride the any-authed-token GET chain, the
//! S2 owner-table precedent.
//!
//! Error contract (feedback_routes conventions, camelCase bodies):
//! `{"ok":false,"error":{"code","hint"}}` with stable codes —
//! `usage` (400) · `not_found` (404) · `exists` (409: local-part
//! collision, and delete-of-already-retired) · `cap_reached` (409) ·
//! `not_ready` (503: mail server not installed/running, domain not
//! verified) · `engine` (502, Stalwart said no).

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError, AddressEngine};
use crate::mail::domains;
use crate::mail::secrets::FileSecretStore;

// ── Response helpers ────────────────────────────────────────────────────

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

/// Map an ops-layer error onto the stable wire shapes.
fn addr_error_response(err: AddrError) -> CliResponse {
    match err {
        AddrError::Usage(hint) => error_response("400 Bad Request", "usage", &hint),
        AddrError::NotFound(hint) => error_response("404 Not Found", "not_found", &hint),
        AddrError::Exists(hint) => error_response("409 Conflict", "exists", &hint),
        AddrError::CapReached(hint) => error_response("409 Conflict", "cap_reached", &hint),
        AddrError::NotReady(hint) => {
            error_response("503 Service Unavailable", "not_ready", &hint)
        }
        AddrError::Engine(hint) => error_response("502 Bad Gateway", "engine", &hint),
    }
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// Resolve the caller's workspace to `(path, project_id)` — the
/// server-side identity step shared by create/delete/list.
fn resolve_caller(project: &str) -> Result<(String, String), CliResponse> {
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return Err(crate::workspace_routes::workspace_not_found_response(project));
    };
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    match project_id {
        Some(id) => Ok((path, id)),
        None => Err(error_response(
            "404 Not Found",
            "not_found",
            &format!("workspace not registered: {path}"),
        )),
    }
}

// ── POST handlers ───────────────────────────────────────────────────────

/// `POST /cli/mail/address/create` body (`k2 mail create
/// <localpart>[@<domain>] [--id <client-id>]`). `localPart` may carry
/// the `name@domain` spelling; `domain` is the explicit `--domain`
/// form; neither = the workspace default domain (S2's resolution).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CreateBody {
    project: String,
    local_part: String,
    domain: Option<String>,
    client_id: Option<String>,
}

/// POST `/cli/mail/address/create` — S3: mint on a Verified domain,
/// subject to the effective cap; idempotent via optional `clientId`
/// (same (owner, clientId) returns the existing ACTIVE address with
/// `existing: true`). Stalwart account + vaulted password + K2 row,
/// with compensating destroy on late failure (see `mail::addresses`).
pub fn handle_address_create(body: &[u8]) -> CliResponse {
    let b: CreateBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    if b.project.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'project' (workspace name | path | UUID)",
        );
    }
    if b.local_part.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'localPart' — e.g. research-bot or research-bot@acme.dev",
        );
    }
    let (path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // The ONLY sanctioned cap read path (module header).
    let cap = k2_core::workspace::settings::mail_address_cap_for_path(&path);
    // A live Stalwart is required to mint (S1 owns getting it there).
    let (engine, _hostname) = match domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
    };
    let secrets_store = FileSecretStore::default();
    match addresses::mint_address(
        &engine,
        &secrets_store,
        &project_id,
        cap,
        &b.local_part,
        b.domain.as_deref(),
        b.client_id.as_deref(),
    ) {
        Ok(v) => ok_json(v),
        Err(e) => addr_error_response(e),
    }
}

/// `POST /cli/mail/address/delete` body (`k2 mail delete <address>`).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct DeleteBody {
    project: String,
    address: String,
}

/// POST `/cli/mail/address/delete` — S3: retire an address the
/// caller's workspace owns (Stalwart account DISABLED — never
/// destroyed, retention window §12 — K2 row → `retired` +
/// `retired_at`). Frees the cap slot immediately (§11.1.5, by
/// construction: the cap counts active rows).
pub fn handle_address_delete(body: &[u8]) -> CliResponse {
    let b: DeleteBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    if b.project.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'project' (workspace name | path | UUID)",
        );
    }
    if b.address.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'address' — the full address to retire, e.g. bot@acme.dev",
        );
    }
    let (_path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // The engine is only needed when the row has a Stalwart account;
    // an installed-but-unreachable server fails CLOSED inside
    // retire_address (mirrors the S2 domain-remove contract).
    let engine = domains::engine_from_db().ok();
    match addresses::retire_address(
        engine.as_ref().map(|(e, _)| e as &dyn AddressEngine),
        &project_id,
        &b.address,
    ) {
        Ok(v) => ok_json(v),
        Err(e) => addr_error_response(e),
    }
}

// ── GET handler ─────────────────────────────────────────────────────────

/// GET `/cli/mail/address/list` — two audiences, one route:
///
/// - **Caller view** (`?project=<ws>` — how `k2 mail addresses`
///   calls it): the workspace's ACTIVE addresses + `createdAt` + cap
///   usage `{used, cap}` (cap 0 = unlimited; callers render
///   "unlimited").
/// - **Owner table** (`?all=true`): every address with holder
///   workspace + status, retired included (the Settings→Email table).
pub fn handle_address_list(params: &HashMap<String, String>) -> CliResponse {
    if crate::cli::bool_param(params, "all") {
        return ok_json(addresses::list_all_json());
    }
    let project = match crate::cli::need_project(params) {
        Ok(p) => p,
        Err(_) => {
            return error_response(
                "400 Bad Request",
                "usage",
                "missing 'project' (workspace name | path | UUID) — or pass all=true for \
                 the owner table",
            )
        }
    };
    let (path, project_id) = match resolve_caller(&project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let cap = k2_core::workspace::settings::mail_address_cap_for_path(&path);
    ok_json(addresses::list_for_project_json(&project_id, cap))
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — no network (house rules): validation + gating +
// the no-server 503; deep mint/retire behavior lives in
// mail::addresses with fakes.
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn body_json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    fn unique(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4();
        (
            format!("mail-addr-routes-{label}-{id}"),
            format!("/tmp/mail-addr-routes-{label}-{}-{id}", std::process::id()),
        )
    }

    /// Register a workspace with a DETERMINISTIC per-workspace cap
    /// override (the resolver's global fallback reads the real app
    /// settings — never asserted on in tests).
    fn insert_project(name: &str, path: &str, cap: i64) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, mail_address_cap) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, path, cap],
        )
        .expect("insert project row");
        id
    }

    fn cleanup_project(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    }

    fn seed_address(
        project_id: &str,
        address: &str,
        status: &str,
        stalwart_account_id: Option<&str>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, 'dom-x', ?3, ?4, ?5, 100)",
            rusqlite::params![id, address, stalwart_account_id, project_id, status],
        )
        .expect("seed address");
        id
    }

    // ── create ──

    #[test]
    fn create_validates_body_workspace_and_reports_not_ready_without_server() {
        // Validation happens before any identity/engine work.
        let resp = handle_address_create(b"not json");
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        let resp = handle_address_create(br#"{"project":"/tmp/x"}"#);
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("localPart"));

        let resp = handle_address_create(br#"{"localPart":"bot"}"#);
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("project"));

        // Unknown workspace → the shared 404 shape.
        let resp = handle_address_create(
            br#"{"project":"no-such-workspace-xyz","localPart":"bot"}"#,
        );
        assert_eq!(resp.status, "404 Not Found");

        // Registered workspace, no mail_server row → not_ready with
        // the Settings pointer, never a network dial.
        let (name, path) = unique("create");
        let project_id = insert_project(&name, &path, 5);
        let resp = handle_address_create(
            serde_json::json!({ "project": path, "localPart": "bot" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "not_ready");
        assert!(v["error"]["hint"].as_str().unwrap().contains("Settings → Email"), "{v}");
        cleanup_project(&project_id);
    }

    // ── delete ──

    #[test]
    fn delete_validates_scopes_ownership_and_retires_without_engine_when_possible() {
        let resp = handle_address_delete(b"{}");
        assert_eq!(resp.status, "400 Bad Request");

        let (name, path) = unique("delete");
        let project_id = insert_project(&name, &path, 5);

        // Unknown address → not_found (row lookup precedes any engine
        // need — no mail server required to say so).
        let resp = handle_address_delete(
            serde_json::json!({ "project": path, "address": "ghost@x.example" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found");
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");

        // A row WITH a Stalwart account and no reachable server →
        // fails CLOSED as engine (502).
        seed_address(&project_id, &format!("bound@{name}.example"), "active", Some("acc-9"));
        let resp = handle_address_delete(
            serde_json::json!({ "project": path, "address": format!("bound@{name}.example") })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "engine");

        // A row WITHOUT a Stalwart account retires cleanly (degraded
        // legacy shape — nothing to disable).
        seed_address(&project_id, &format!("loose@{name}.example"), "active", None);
        let resp = handle_address_delete(
            serde_json::json!({ "project": path, "address": format!("loose@{name}.example") })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["ok"], true);
        assert_eq!(v["status"], "retired");
        assert!(v["retiredAt"].as_i64().unwrap() > 0);

        // Again → already-retired (409 exists).
        let resp = handle_address_delete(
            serde_json::json!({ "project": path, "address": format!("loose@{name}.example") })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "409 Conflict");
        assert_eq!(body_json(&resp)["error"]["code"], "exists");

        // A FOREIGN workspace's address answers the masked not_found.
        let (name2, path2) = unique("delete-foreign");
        let project2 = insert_project(&name2, &path2, 5);
        seed_address(&project2, &format!("theirs@{name}.example"), "active", None);
        let resp = handle_address_delete(
            serde_json::json!({ "project": path, "address": format!("theirs@{name}.example") })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");

        cleanup_project(&project_id);
        cleanup_project(&project2);
    }

    // ── list ──

    #[test]
    fn list_serves_caller_view_with_cap_and_owner_table_with_all() {
        // No project and no all=true → usage naming both options.
        let resp = handle_address_list(&HashMap::new());
        assert_eq!(resp.status, "400 Bad Request");
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "usage");
        assert!(v["error"]["hint"].as_str().unwrap().contains("all=true"), "{v}");

        // Unknown workspace → the shared 404 shape.
        let mut params = HashMap::new();
        params.insert("project".to_string(), "no-such-workspace-xyz".to_string());
        let resp = handle_address_list(&params);
        assert_eq!(resp.status, "404 Not Found");

        // Caller view: own ACTIVE rows + the OVERRIDDEN cap (3).
        let (name, path) = unique("list");
        let project_id = insert_project(&name, &path, 3);
        seed_address(&project_id, &format!("mine@{name}.example"), "active", None);
        seed_address(&project_id, &format!("gone@{name}.example"), "retired", None);
        let mut params = HashMap::new();
        params.insert("project".to_string(), path.clone());
        let resp = handle_address_list(&params);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["ok"], true);
        let addrs = v["addresses"].as_array().unwrap();
        assert_eq!(addrs.len(), 1, "active only: {v}");
        assert_eq!(addrs[0]["address"], format!("mine@{name}.example"));
        assert!(addrs[0]["createdAt"].is_i64());
        assert_eq!(v["cap"]["used"], 1);
        assert_eq!(v["cap"]["cap"], 3, "per-workspace override via the effective resolver");

        // Owner table: retired included, holder workspace named.
        let mut params = HashMap::new();
        params.insert("all".to_string(), "true".to_string());
        let resp = handle_address_list(&params);
        assert_eq!(resp.status, "200 OK");
        let v = body_json(&resp);
        let all = v["addresses"].as_array().unwrap();
        let retired = all
            .iter()
            .find(|a| a["address"] == format!("gone@{name}.example"))
            .expect("retired row in the owner table");
        assert_eq!(retired["status"], "retired");
        assert_eq!(retired["holderProjectId"], project_id);
        assert_eq!(retired["holderWorkspace"], name);

        cleanup_project(&project_id);
    }
}
