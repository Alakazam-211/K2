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
//!   PRIMARY/owner MANAGEMENT surface: the owner token, Owner/Admin
//!   logins, and an agent whose workspace has mail-manage on (C5b: the
//!   dispatcher's extra gate refuses every other passport). Such an agent
//!   manages OTHER workspaces' access but never widens its own: grant,
//!   set-level, set-manage and set-primary naming its own workspace answer
//!   403 `owner_only` (M0, vs-live W24; [`refuse_own_workspace`]).
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

/// M0 (prd-routines-reminders-and-agent-mail-v1 §8; vs-live W24): an agent
/// never widens its OWN mail access. A passport whose workspace is
/// `target_id` (the resolved grantee: by path, name or id) is refused on
/// grant, set-level, set-manage and set-primary (taking an inbox over) with
/// 403 `owner_only`; an IT agent (mail-manage on) still manages other
/// workspaces' access, and a plain agent can't change access at all (the
/// dispatcher's extra gate refuses it first; this keeps the handler safe
/// on its own). The owner token and Owner/Admin logins carry no passport
/// and are never refused here.
fn refuse_own_workspace(target_id: &str, what: &str) -> Option<CliResponse> {
    use crate::mail::agent_creds::{self, MailCaller};
    match agent_creds::mail_caller() {
        MailCaller::Owner => None,
        MailCaller::ItAgent { workspace_uuid } | MailCaller::Agent { workspace_uuid } if workspace_uuid == target_id => {
            Some(agent_creds::refuse_self_widening(&format!("{what} for this agent's own workspace")))
        }
        MailCaller::Agent { .. } => Some(agent_creds::refuse_needs_mail_manage("change who can use an inbox")),
        MailCaller::ItAgent { .. } => None,
    }
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

// ── GET /cli/mail/capabilities (MCS1) ───────────────────────────────────

/// GET `/cli/mail/capabilities[?address=]` — what each inbox can do
/// (PRD §3.1). Same identity rule as `inboxes`: a scoped agent (or a
/// `project=` claim) sees only the inboxes it has access to, at its level;
/// no identity = the owner view of every inbox (owner-or-admin, enforced in
/// the dispatcher's shared clause). With `address`, one inbox — an inbox
/// the caller can't see answers the masked `not_found` (CAL19: capability
/// answers never probe what exists).
pub fn handle_capabilities(params: &HashMap<String, String>) -> CliResponse {
    let catalog = handle_inboxes(params);
    if catalog.status != "200 OK" {
        return catalog;
    }
    let v: serde_json::Value = match serde_json::from_str(&catalog.body) {
        Ok(v) => v,
        Err(e) => {
            return error_response("500 Internal Server Error", "engine", &format!("catalog: {e}"))
        }
    };
    let rows = v["inboxes"].as_array().cloned().unwrap_or_default();
    let shape = |r: &serde_json::Value| {
        serde_json::json!({
            "address": r["address"],
            "source": r["source"],
            "yourLevel": r["yourLevel"],
            "capabilities": r["capabilities"],
        })
    };
    let wanted = params.get("address").map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty());
    match wanted {
        Some(addr) => match rows
            .iter()
            .find(|r| r["address"].as_str().map(|a| a.eq_ignore_ascii_case(&addr)).unwrap_or(false))
        {
            Some(r) => {
                let mut out = shape(r);
                out["ok"] = serde_json::json!(true);
                ok_json(out)
            }
            None => error_response(
                "404 Not Found",
                "not_found",
                &format!("no inbox '{addr}' in this workspace — 'k2 mail inboxes' lists yours"),
            ),
        },
        None => {
            let list: Vec<serde_json::Value> = rows.iter().map(shape).collect();
            ok_json(serde_json::json!({ "ok": true, "count": list.len(), "inboxes": list }))
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
    if let Some(refused) = refuse_own_workspace(&grantee_id, "granting inbox access") {
        return refused;
    }
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
    if let Some(refused) = refuse_own_workspace(&new_id, "making an inbox's primary") {
        return refused;
    }
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
    if let Some(refused) = refuse_own_workspace(&project_id, "setting an inbox's access level") {
        return refused;
    }
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
    if let Some(refused) = refuse_own_workspace(&project_id, "setting who manages an inbox") {
        return refused;
    }
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

    /// A workspace with mail-manage on (an "IT agent").
    fn insert_mail_manager(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, mail_manage_enabled) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![id, name, path],
        )
        .expect("insert mail-manage project row");
        id
    }

    fn own_grants(project_id: &str) -> i64 {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM mail_inbox_grants WHERE project_id = ?1", rusqlite::params![project_id], |r| r.get(0))
            .expect("count grants")
    }

    fn primary_of(address: &str) -> String {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT owner_project_id FROM mail_addresses WHERE address = ?1", rusqlite::params![address], |r| r.get(0))
            .expect("primary")
    }

    /// M0 (prd-routines-reminders-and-agent-mail-v1 §8, vs-live W24): an
    /// agent with mail-manage on can manage other workspaces' access, but
    /// never widen its OWN: grant, set-level, set-manage and set-primary
    /// (taking an inbox over) naming its own workspace (by path or
    /// id) are the owner's. To another workspace stays allowed, and the
    /// owner token (an Admin login is the same caller here: no passport)
    /// may do all four.
    #[test]
    fn an_agent_never_widens_its_own_mail_access_m0() {
        let (oname, opath) = unique("m0own");
        let owner_id = insert_project(&oname, &opath);
        let (iname, ipath) = unique("m0it");
        let it_id = insert_mail_manager(&iname, &ipath);
        let (gname, gpath) = unique("m0oth");
        let other_id = insert_project(&gname, &gpath);
        let addr = format!("ops-{}@acc.example", &owner_id[..8]);
        seed_hosted(&owner_id, &addr);
        let it = crate::session_token::HookPrincipal { workspace_uuid: it_id.clone(), agent_address: "agent".to_string() };
        let as_it = |f: &dyn Fn() -> CliResponse| crate::caller_workspace::with_request_principal(Some(it.clone()), f);
        let b = |v: serde_json::Value| v.to_string().into_bytes();

        for spec in [ipath.as_str(), it_id.as_str()] {
            let tries: [(&str, CliResponse); 4] = [
                ("grant", as_it(&|| handle_grant(&b(serde_json::json!({"address": addr, "project": spec, "level": "send"}))))),
                ("set-level", as_it(&|| handle_set_level(&b(serde_json::json!({"address": addr, "project": spec, "level": "send"}))))),
                (
                    "set-manage",
                    as_it(&|| handle_set_manage(&b(serde_json::json!({"address": addr, "project": spec, "canManage": true, "canDelete": true})))),
                ),
                ("set-primary", as_it(&|| handle_set_primary(&b(serde_json::json!({"address": addr, "project": spec}))))),
            ];
            for (verb, resp) in tries {
                assert_eq!(resp.status, "403 Forbidden", "{verb} to itself ({spec}): {}", resp.body);
                assert_eq!(body_json(&resp)["error"]["code"], "owner_only", "{verb}: {}", resp.body);
                assert!(
                    body_json(&resp)["error"]["hint"].as_str().expect("hint").contains("can't widen its own access"),
                    "{verb}: {}",
                    resp.body
                );
            }
        }
        assert_eq!(own_grants(&it_id), 0, "no grant landed for the agent's own workspace");
        assert_eq!(primary_of(&addr), owner_id, "the inbox wasn't taken over");

        // Another workspace: today's behaviour (an IT agent manages others' access).
        let resp = as_it(&|| handle_grant(&b(serde_json::json!({"address": addr, "project": gpath, "level": "read"}))));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let resp = as_it(&|| handle_set_level(&b(serde_json::json!({"address": addr, "project": gpath, "level": "draft"}))));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);

        // The owner token (and an Admin login: no passport) may widen any workspace.
        let resp = handle_grant(&b(serde_json::json!({"address": addr, "project": ipath, "level": "read"})));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let resp = handle_set_primary(&b(serde_json::json!({"address": addr, "project": ipath})));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(primary_of(&addr), it_id);

        cleanup(&owner_id);
        cleanup(&it_id);
        cleanup(&other_id);
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

    /// MCS1: `capabilities` rides every catalog row and the dedicated route;
    /// the agent view sees only its own inboxes at its level, and an inbox
    /// it can't see is the masked not_found (no existence oracle).
    #[test]
    fn capabilities_route_projects_the_catalog_with_masking() {
        let (aname, apath) = unique("capA");
        let a_id = insert_project(&aname, &apath);
        let (bname, bpath) = unique("capB");
        let b_id = insert_project(&bname, &bpath);
        let addr_a = format!("cap-a-{}@acc.example", &a_id[..8]);
        let addr_ms = format!("cap-ms-{}@contoso.example", &b_id[..8]);
        seed_hosted(&a_id, &addr_a);
        // B's Microsoft link, consented for mail only (MCS1 scope set).
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, kind, \
                 host, port, username, created_at, auth_kind, provider, features, \
                 granted_scopes, account_kind, account_checked_at) \
                 VALUES (?1, ?2, ?3, 'graph', 'graph.microsoft.com', 443, ?3, 1, 'oauth', \
                 'microsoft', 'mail', ?4, 'work', 5)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    b_id,
                    addr_ms,
                    crate::mail::oauth::scope_for(
                        crate::mail::oauth::OauthProvider::Microsoft,
                        crate::mail::oauth::Features::MAIL
                    ),
                ],
            )
            .expect("seed ms link");
        }

        // Owner view: both inboxes, capabilities on the inboxes rows too.
        let all = body_json(&handle_inboxes(&HashMap::new()));
        let ms_row = all["inboxes"].as_array().unwrap().iter()
            .find(|i| i["address"] == addr_ms.as_str()).expect("listed");
        assert_eq!(ms_row["capabilities"]["source"], "microsoft");
        assert_eq!(ms_row["capabilities"]["account"]["kind"], "work");

        // One inbox by address (owner).
        let mut p = HashMap::new();
        p.insert("address".to_string(), addr_ms.to_uppercase());
        let one = handle_capabilities(&p);
        assert_eq!(one.status, "200 OK", "{}", one.body);
        let one = body_json(&one);
        assert_eq!(one["address"], addr_ms.as_str());
        assert_eq!(one["capabilities"]["mail"]["read"], true);
        let cal = one["capabilities"]["missing"].as_array().unwrap().iter()
            .find(|m| m["cap"] == "calendar.read").expect("calendar.read missing");
        assert_eq!(cal["reason"], "scope_not_granted");

        // Agent A: sees its hosted inbox only; B's link is masked.
        let mut pa = HashMap::new();
        pa.insert("project".to_string(), apath.clone());
        let mine = body_json(&handle_capabilities(&pa));
        let rows = mine["inboxes"].as_array().unwrap();
        assert!(rows.iter().any(|r| r["address"] == addr_a.as_str()));
        assert!(!rows.iter().any(|r| r["address"] == addr_ms.as_str()), "{mine}");
        let row_a = rows.iter().find(|r| r["address"] == addr_a.as_str()).unwrap();
        assert_eq!(row_a["yourLevel"], "send");
        assert_eq!(row_a["capabilities"]["mail"]["send"], true);
        pa.insert("address".to_string(), addr_ms.clone());
        let masked = handle_capabilities(&pa);
        assert_eq!(masked.status, "404 Not Found", "{}", masked.body);
        assert_eq!(body_json(&masked)["error"]["code"], "not_found");

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
                rusqlite::params![b_id],
            );
        }
        cleanup(&a_id);
        cleanup(&b_id);
        let _ = bpath;
        let _ = bname;
        let _ = aname;
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
