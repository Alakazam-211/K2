//! `/cli/mail/inboxes` + `/cli/mail/access/*` — the S11 unified ACCESS
//! surface over BOTH provisioning sources (PRD §17.5).
//!
//! - `GET /cli/mail/inboxes` — the unified catalog. **Agent view**: viewer
//!   identity from Wave 0 `resolve_caller_params` (stamped scoped principal
//!   / proven `workspace_uuid` wins over client `project=`). Only inboxes
//!   that workspace can access, each with `yourLevel`. **Owner/admin view**:
//!   no principal and no project claim → ALL inboxes with full primary +
//!   grants; owner-or-admin-gated in the dispatcher's dedicated `inboxes`
//!   clause (dual-auth: `token_or_scoped_hook_auth` + stamp). SQLite-only —
//!   no engine dial.
//! - `POST /cli/mail/access/grant|revoke|set-primary|set-level|set-manage` —
//!   PRIMARY/owner MANAGEMENT surface (owner-or-admin-gated via
//!   `is_owner_level_mutation`'s `/cli/mail/access/` prefix; also on
//!   `is_agent_verb` DENY_PREFIXES so scoped agents cannot self-grant).
//!   These act BY ADDRESS across hosted + linked; the ops layer resolves
//!   which provisioning row the address names, accepts 'send' on either
//!   source (§17.5 — linked send is the SMTP opt-in), and never touches
//!   credentials.
//!
//! Masking: these are the primary's own surface (unmasked not_found with
//! a pointer). The AGENT read/draft/send gates that DO mask live in
//! `crate::mail::access`.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::access::{self, AccessError};

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

fn access_error_response(err: AccessError) -> CliResponse {
    match err {
        AccessError::Usage(hint) => error_response("400 Bad Request", "usage", &hint),
        AccessError::NotFound(hint) => error_response("404 Not Found", "not_found", &hint),
        AccessError::Engine(hint) => error_response("502 Bad Gateway", "engine", &hint),
    }
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// Resolve a workspace spec (name | path | UUID) to `project_id`.
fn resolve_project(project: &str) -> Result<String, CliResponse> {
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return Err(crate::workspace_routes::workspace_not_found_response(project));
    };
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    project_id.ok_or_else(|| {
        error_response("404 Not Found", "not_found", &format!("workspace not registered: {path}"))
    })
}

// ── GET /cli/mail/inboxes ───────────────────────────────────────────────

/// The unified inbox catalog.
///
/// * **Agent view** — a scoped principal is stamped / request-scoped, or
///   an owner-token residual supplies `project=`. Viewer is resolved via
///   [`crate::mail::identity::resolve_caller_params`] so a passport's
///   proven `workspace_uuid` always wins over a spoofed `project=` claim.
/// * **Owner view** — no principal and no project claim → ALL inboxes
///   (owner-or-admin gate lives in the dispatcher).
pub fn handle_inboxes(params: &HashMap<String, String>) -> CliResponse {
    match crate::mail::identity::resolve_caller_params(params) {
        Ok((_path, project_id)) => ok_json(access::catalog_json(Some(&project_id))),
        Err(resp) => {
            // Missing identity → owner catalog. Any other resolve error
            // (unknown claim, unresolvable principal) surfaces as-is so
            // agents never fall open into the all-inboxes view.
            let no_identity = crate::caller_workspace::principal_from_params(params).is_none()
                && params
                    .get("project")
                    .or_else(|| params.get("project_path"))
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true);
            if no_identity {
                ok_json(access::catalog_json(None))
            } else {
                resp
            }
        }
    }
}

// ── POST /cli/mail/access/grant ─────────────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct GrantBody {
    address: String,
    project: String,
    level: String,
}

/// POST `/cli/mail/access/grant` — Primary/owner: grant a workspace
/// read|draft|send access to any inbox (upsert). 'send' works on hosted (governed) and linked (SMTP + same mail_agent_send gate, E4);
/// granting the PRIMARY teaches.
pub fn handle_grant(body: &[u8]) -> CliResponse {
    let b: GrantBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'address' — the inbox to share ('k2 mail inboxes')");
    }
    if b.project.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'project' — the workspace to grant (name | path | UUID)");
    }
    if b.level.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'level' — read | draft | send");
    }
    let grantee_id = match resolve_project(&b.project) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match access::grant_access(&b.address, &grantee_id, &b.level) {
        Ok((address, source)) => ok_json(serde_json::json!({
            "ok": true,
            "address": address,
            "source": source.as_str(),
            "project": b.project,
            "projectId": grantee_id,
            "level": b.level.trim(),
            "hint": format!(
                "granted '{}' on {} to '{}' — see 'k2 mail inboxes'",
                b.level.trim(), address, b.project
            ),
        })),
        Err(e) => access_error_response(e),
    }
}

// ── POST /cli/mail/access/revoke ────────────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RevokeBody {
    address: String,
    project: String,
}

/// POST `/cli/mail/access/revoke` — Primary/owner: remove a workspace's
/// grant (idempotent). Revoking the PRIMARY teaches (transfer/remove).
pub fn handle_revoke(body: &[u8]) -> CliResponse {
    let b: RevokeBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'address' — the inbox ('k2 mail inboxes')");
    }
    if b.project.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'project' — the workspace to revoke (name | path | UUID)");
    }
    let grantee_id = match resolve_project(&b.project) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match access::revoke_access(&b.address, &grantee_id) {
        Ok(address) => ok_json(serde_json::json!({
            "ok": true,
            "address": address,
            "project": b.project,
            "projectId": grantee_id,
            "revoked": true,
        })),
        Err(e) => access_error_response(e),
    }
}

// ── POST /cli/mail/access/set-primary ───────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SetPrimaryBody {
    address: String,
    project: String,
}

/// POST `/cli/mail/access/set-primary` — TRANSFER the managing
/// workspace. The old primary is demoted to a grant at its prior
/// ceiling; the new primary's grant row (if any) is cleared.
pub fn handle_set_primary(body: &[u8]) -> CliResponse {
    let b: SetPrimaryBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'address' — the inbox ('k2 mail inboxes')");
    }
    if b.project.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'project' — the new primary workspace (name | path | UUID)");
    }
    let new_id = match resolve_project(&b.project) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match access::set_primary(&b.address, &new_id) {
        Ok((address, source)) => ok_json(serde_json::json!({
            "ok": true,
            "address": address,
            "source": source.as_str(),
            "project": b.project,
            "projectId": new_id,
            "hint": format!("'{}' now manages {}", b.project, address),
        })),
        Err(e) => access_error_response(e),
    }
}

// ── POST /cli/mail/access/set-level ─────────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SetLevelBody {
    address: String,
    project: String,
    level: String,
}

/// POST `/cli/mail/access/set-level` — set a workspace's level. When it
/// is the PRIMARY, updates the primary's own ceiling; otherwise upserts
/// its grant. 'send' works on both sources (§ 17.5).
pub fn handle_set_level(body: &[u8]) -> CliResponse {
    let b: SetLevelBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'address' — the inbox ('k2 mail inboxes')");
    }
    if b.project.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'project' — the workspace (name | path | UUID)");
    }
    if b.level.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'level' — read | draft | send");
    }
    let project_id = match resolve_project(&b.project) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match access::set_level(&b.address, &project_id, &b.level) {
        Ok((address, source, is_primary)) => ok_json(serde_json::json!({
            "ok": true,
            "address": address,
            "source": source.as_str(),
            "project": b.project,
            "projectId": project_id,
            "level": b.level.trim(),
            "isPrimary": is_primary,
        })),
        Err(e) => access_error_response(e),
    }
}

// ── POST /cli/mail/access/set-manage ────────────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SetManageBody {
    address: String,
    project: String,
    can_manage: bool,
    can_delete: bool,
}

/// POST `/cli/mail/access/set-manage` — set a workspace's MANAGEMENT +
/// DELETE caps (0081; ORTHOGONAL to level). `canDelete` REQUIRES
/// `canManage` (a usage error otherwise; the ops layer also normalizes
/// clearing manage → clearing delete). When it names the PRIMARY, the
/// primary's own caps change; otherwise the workspace's grant caps.
pub fn handle_set_manage(body: &[u8]) -> CliResponse {
    let b: SetManageBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'address' — the inbox ('k2 mail inboxes')");
    }
    if b.project.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'project' — the workspace (name | path | UUID)");
    }
    let project_id = match resolve_project(&b.project) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match access::set_manage(&b.address, &project_id, b.can_manage, b.can_delete) {
        Ok((address, source, is_primary)) => ok_json(serde_json::json!({
            "ok": true,
            "address": address,
            "source": source.as_str(),
            "project": b.project,
            "projectId": project_id,
            "canManage": b.can_manage,
            "canDelete": b.can_delete && b.can_manage,
            "isPrimary": is_primary,
        })),
        Err(e) => access_error_response(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — validation + resolution + masking-free management
// surface, shared test DB, no network (house rules).
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn body_json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    fn unique(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let short = &id[..12];
        (format!("macc-{label}-{short}"), format!("/tmp/mail-access-{label}-{}-{short}", std::process::id()))
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project row");
        id
    }

    fn seed_hosted(owner: &str, address: &str) {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at, primary_level) \
             VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100, 'send')",
            rusqlite::params![id, address, format!("acct-{}", &id[..8]), owner],
        )
        .expect("seed hosted");
    }

    fn cleanup(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_inbox_grants WHERE project_id = ?1", rusqlite::params![project_id]);
        let _ = conn.execute("DELETE FROM mail_addresses WHERE owner_project_id = ?1", rusqlite::params![project_id]);
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    }

    #[test]
    fn access_mutations_validate_before_resolution() {
        for (h, body, needle) in [
            ("grant", r#"{}"#, "address"),
            ("grant", r#"{"address":"a@b.example"}"#, "project"),
            ("grant", r#"{"address":"a@b.example","project":"x"}"#, "level"),
            ("revoke", r#"{}"#, "address"),
            ("set-primary", r#"{"address":"a@b.example"}"#, "project"),
            ("set-level", r#"{"address":"a@b.example","project":"x"}"#, "level"),
        ] {
            let resp = match h {
                "grant" => handle_grant(body.as_bytes()),
                "revoke" => handle_revoke(body.as_bytes()),
                "set-primary" => handle_set_primary(body.as_bytes()),
                _ => handle_set_level(body.as_bytes()),
            };
            assert_eq!(resp.status, "400 Bad Request", "{h} {body}");
            assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains(needle), "{h}: {}", resp.body);
        }
    }

    #[test]
    fn grant_resolves_workspace_and_lands_visible_in_inboxes() {
        let (oname, opath) = unique("owner");
        let owner_id = insert_project(&oname, &opath);
        let (gname, gpath) = unique("grantee");
        let grantee_id = insert_project(&gname, &gpath);
        let addr = format!("ops-{}@acc.example", &owner_id[..8]);
        seed_hosted(&owner_id, &addr);

        // Unknown grantee workspace → shared 404.
        let resp = handle_grant(
            serde_json::json!({ "address": addr, "project": "no-such-ws", "level": "read" })
                .to_string().as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);

        // A valid send grant lands and shows in the owner catalog.
        let resp = handle_grant(
            serde_json::json!({ "address": addr, "project": gpath, "level": "send" })
                .to_string().as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(body_json(&resp)["source"], "hosted");

        let list = body_json(&handle_inboxes(&HashMap::new()));
        let row = list["inboxes"].as_array().unwrap().iter()
            .find(|i| i["address"] == addr.as_str()).expect("listed");
        assert_eq!(row["grants"][0]["level"], "send");
        assert_eq!(row["grants"][0]["projectId"], grantee_id);

        // Agent view for the grantee shows yourLevel; owner view hides it.
        let mut p = HashMap::new();
        p.insert("project".to_string(), gpath.clone());
        let mine = body_json(&handle_inboxes(&p));
        let row = mine["inboxes"].as_array().unwrap().iter()
            .find(|i| i["address"] == addr.as_str()).expect("granted sees it");
        assert_eq!(row["yourLevel"], "send");

        cleanup(&owner_id);
        cleanup(&grantee_id);
    }

    /// E1: stamped principal wins over a spoofed `project=` claim for the
    /// agent catalog view (passport identity, never raw project=).
    #[test]
    fn inboxes_agent_view_uses_proven_principal_not_spoofed_project() {
        let (aname, apath) = unique("prinA");
        let a_id = insert_project(&aname, &apath);
        let (bname, bpath) = unique("prinB");
        let b_id = insert_project(&bname, &bpath);
        let addr_a = format!("a-{}@acc.example", &a_id[..8]);
        let addr_b = format!("b-{}@acc.example", &b_id[..8]);
        seed_hosted(&a_id, &addr_a);
        seed_hosted(&b_id, &addr_b);

        // Grant B read on A's inbox so a B-viewer would see addr_a if spoof worked.
        // Principal is A; spoofed project= points at B — catalog must stay A's.
        let mut params = HashMap::new();
        params.insert(
            crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(),
            "1".to_string(),
        );
        params.insert("project_id".to_string(), a_id.clone());
        params.insert("from".to_string(), "agent-A".to_string());
        // Hostile claim: pretend to be workspace B.
        params.insert("project".to_string(), bpath.clone());

        let mine = body_json(&handle_inboxes(&params));
        let rows = mine["inboxes"].as_array().expect("inboxes array");
        assert!(
            rows.iter().any(|i| i["address"] == addr_a.as_str()),
            "principal A must see its own inbox: {}",
            mine
        );
        assert!(
            !rows.iter().any(|i| i["address"] == addr_b.as_str()),
            "principal A must NOT see B's inbox via spoofed project=: {}",
            mine
        );
        let row = rows
            .iter()
            .find(|i| i["address"] == addr_a.as_str())
            .expect("A's row");
        assert_eq!(row["yourLevel"], "send", "primary of A has send: {}", mine);

        // No identity → owner catalog (dispatcher still gates owner-or-admin).
        let all = body_json(&handle_inboxes(&HashMap::new()));
        let all_rows = all["inboxes"].as_array().expect("owner inboxes");
        assert!(
            all_rows.iter().any(|i| i["address"] == addr_a.as_str())
                && all_rows.iter().any(|i| i["address"] == addr_b.as_str()),
            "owner view lists both: {}",
            all
        );

        cleanup(&a_id);
        cleanup(&b_id);
    }

    #[test]
    fn set_manage_validates_delete_requires_manage_and_lands_in_catalog() {
        // Missing fields validate before resolution.
        for (body, needle) in [(r#"{}"#, "address"), (r#"{"address":"a@b.example"}"#, "project")] {
            let resp = handle_set_manage(body.as_bytes());
            assert_eq!(resp.status, "400 Bad Request", "{body}");
            assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains(needle));
        }

        let (oname, opath) = unique("mowner");
        let owner_id = insert_project(&oname, &opath);
        let addr = format!("mng-{}@acc.example", &owner_id[..8]);
        seed_hosted(&owner_id, &addr);

        // canDelete without canManage → usage (delete requires manage).
        let resp = handle_set_manage(
            serde_json::json!({ "address": addr, "project": opath, "canManage": false, "canDelete": true })
                .to_string().as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().to_lowercase().contains("delete requires manage"));

        // Valid primary caps land in the owner catalog.
        let resp = handle_set_manage(
            serde_json::json!({ "address": addr, "project": opath, "canManage": true, "canDelete": true })
                .to_string().as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(body_json(&resp)["isPrimary"], true);
        let list = body_json(&handle_inboxes(&HashMap::new()));
        let row = list["inboxes"].as_array().unwrap().iter()
            .find(|i| i["address"] == addr.as_str()).expect("listed");
        assert_eq!(row["primary"]["canManage"], true);
        assert_eq!(row["primary"]["canDelete"], true);

        cleanup(&owner_id);
    }
}
