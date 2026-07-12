//! `/cli/mail/external/*` + `/cli/mail/draft` — EXTERNAL-inbox
//! handlers (mail slice S9, PRD §17.5).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract:
//! - `external/add` / `external/remove` / `external/grant` /
//!   `external/revoke` (POSTs) and `external/list` (GET) are
//!   **owner-level** — the dispatcher gates the POSTs via
//!   `is_owner_level_mutation` + `token_is_owner_or_admin` (the
//!   `/cli/mail/external/` prefix), and the list GET has its own
//!   owner-gated dispatcher clause (the S5 `approvals/list` precedent;
//!   §11.1.3: owner verbs hard-fail for agent tokens SERVER-SIDE).
//! - S10: the OWNER workspace (`mail_external_inboxes.owner_project_id`)
//!   keeps full read+draft + sole management; `grant`/`revoke` give a
//!   SECOND workspace `read` or `draft` access (a grant row). Reads
//!   (`can_read`) and drafts (`can_draft`) enforce owner-OR-grant with
//!   the SAME masked `not_found` for a workspace with no access.
//! - `draft` (POST) is a **workspace-token agent verb**: the calling
//!   workspace must be the inbox's BOUND workspace (masked `not_found`
//!   otherwise — the S3 rule), the source message must live in that
//!   external inbox (a K2-hosted source gets a teaching error pointing
//!   at `k2 mail reply`), and the ONLY effect is an APPEND-\Draft into
//!   the account's real Drafts folder. **No send path exists.**
//!
//! Credentials: the add body carries `password` (from `--pass-stdin`)
//! or `passwordRef` (`env:<VAR>` / absolute file path — resolved ONCE
//! at add time); the value is vaulted under the deterministic
//! `ext-inbox-<row-id>` key and NEVER echoed, listed, or logged. No
//! response from any handler here contains a credential or a secret
//! ref — `external/list` omits even the username.
//!
//! Reads of external MESSAGES do not live here: they flow through the
//! existing `routes_messages` handlers via the §17.5 seam
//! (`backend_for_address`) — one read pipeline, two backends.

use crate::cli_response::CliResponse;
use crate::mail::access::{self, Source};
use crate::mail::external::{self, ExtError};
use crate::mail::external_imap::RealImapOps;
use crate::mail::graph;
use crate::mail::messages::{self, MailBackend, ReadError};
use crate::mail::secrets::{self, FileSecretStore, SecretStore as _};
use crate::mail::send;

// ── Response helpers (the shared error contract) ────────────────────────

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

fn ext_error_response(err: ExtError) -> CliResponse {
    match err {
        ExtError::Usage(hint) => error_response("400 Bad Request", "usage", &hint),
        ExtError::NotFound(hint) => error_response("404 Not Found", "not_found", &hint),
        ExtError::Exists(hint) => error_response("409 Conflict", "exists", &hint),
        ExtError::Engine(hint) => error_response("502 Bad Gateway", "engine", &hint),
    }
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// Resolve the caller's workspace to `(path, project_id)` — identical
/// to routes_addresses (identity from the resolved workspace, never
/// raw params).
fn resolve_caller(project: &str) -> Result<(String, String), CliResponse> {
    // Wave 0: prefers scoped principal over client project= claim.
    crate::mail::identity::resolve_caller(project)
}

// ── POST /cli/mail/external/add (owner) ─────────────────────────────────

/// `k2 mail external add` body. `project` names the workspace the
/// inbox binds to (exactly one, at add time — Rosson's contract).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AddBody {
    project: String,
    address: String,
    display_name: Option<String>,
    kind: Option<String>,
    host: String,
    port: Option<i64>,
    tls: Option<String>,
    username: Option<String>,
    drafts_folder: Option<String>,
    /// OPTIONAL SMTP submission override (§17.5 linked send). Omitted =
    /// derive from the provider / IMAP host at send time.
    smtp_host: Option<String>,
    smtp_port: Option<i64>,
    smtp_tls: Option<String>,
    /// The app-password itself (`--pass-stdin`). Never logged.
    password: Option<String>,
    /// Alternative: `env:<VAR>` or an absolute file path, resolved
    /// once here (`--pass-ref`).
    password_ref: Option<String>,
}

/// POST `/cli/mail/external/add` — owner-level: validate, live
/// connect-check (login + drafts-folder survey through the production
/// IMAP ops), vault the credential, insert the row bound to the named
/// workspace. Runs in the dispatcher's `spawn_blocking` (it dials the
/// user's mail host).
pub fn handle_external_add(body: &[u8]) -> CliResponse {
    let b: AddBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    if b.project.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'project' — the ONE workspace this inbox binds to",
        );
    }
    if b.address.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'address' — the external account, e.g. you@gmail.com",
        );
    }
    if b.host.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'host' — the IMAP server, e.g. imap.gmail.com",
        );
    }
    let spec = match external::validate_new_inbox(
        &b.address,
        b.display_name.as_deref(),
        b.kind.as_deref(),
        &b.host,
        b.port,
        b.tls.as_deref(),
        b.username.as_deref(),
        b.drafts_folder.as_deref(),
        b.smtp_host.as_deref(),
        b.smtp_port,
        b.smtp_tls.as_deref(),
    ) {
        Ok(s) => s,
        Err(e) => return ext_error_response(e),
    };
    let (_path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // Exactly one credential spelling; a ref resolves ONCE, here.
    let password = match (b.password.as_deref(), b.password_ref.as_deref()) {
        (Some(p), None) => p.to_string(),
        (None, Some(r)) => match secrets::resolve_secret_ref(r) {
            Ok(p) => p,
            Err(hint) => return error_response("400 Bad Request", "usage", &hint),
        },
        (None, None) => {
            return error_response(
                "400 Bad Request",
                "usage",
                "missing credential — pipe the app-password via --pass-stdin or point \
                 --pass-ref at env:<VAR> / an absolute file path",
            )
        }
        (Some(_), Some(_)) => {
            return error_response(
                "400 Bad Request",
                "usage",
                "pass EITHER the password or a password ref, not both",
            )
        }
    };
    match external::add_inbox(
        &RealImapOps,
        &FileSecretStore::default(),
        &project_id,
        &spec,
        &password,
    ) {
        Ok(v) => ok_json(v),
        Err(e) => ext_error_response(e),
    }
}

// ── POST /cli/mail/external/remove (owner) ──────────────────────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RemoveBody {
    address: String,
}

/// POST `/cli/mail/external/remove` — owner-level: delete the row AND
/// its vault credential. Owner surface, so no masking.
pub fn handle_external_remove(body: &[u8]) -> CliResponse {
    let b: RemoveBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    if b.address.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'address' — the connected account to remove ('k2 mail external list')",
        );
    }
    match external::remove_inbox(&FileSecretStore::default(), &b.address) {
        Ok(v) => ok_json(v),
        Err(e) => ext_error_response(e),
    }
}

// (S11: GRANT / REVOKE / SET-PRIMARY / SET-LEVEL and the unified
// `/cli/mail/inboxes` catalog moved to `routes_access.rs` — one access
// surface over hosted + linked. `external/grant` and `external/revoke`
// are retired; `external/add` + `external/remove` are the LINK
// provisioning the CLI now spells `k2 mail link add|remove`.)

// ── POST /cli/mail/draft (workspace token — the agent verb) ─────────────

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct DraftBody {
    project: String,
    /// Source message id (`m_…` from `k2 mail messages`).
    id: String,
    body: String,
}

/// POST `/cli/mail/draft` — agent verb (`k2 mail draft <message-id>
/// --body <t>`): compose a reply to a message in the workspace's
/// EXTERNAL inbox and APPEND it, `\Draft`-flagged, into the account's
/// real Drafts folder. The user sees it in their own mail client,
/// edits, and sends themself. This DRAFT route never sends — sending
/// FROM a linked account is the separate 'send'-level path
/// (`external_smtp`). A K2-hosted source message gets
/// the teaching error pointing at `k2 mail reply`; a foreign inbox
/// gets the masked `not_found`. Runs in the dispatcher's
/// `spawn_blocking` (dials the user's mail host).
pub fn handle_draft(body: &[u8]) -> CliResponse {
    let b: DraftBody = match serde_json::from_slice(body) {
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
    if b.id.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — the source message id from 'k2 mail messages'",
        );
    }
    if b.body.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'body' — the draft reply text",
        );
    }
    if b.body.len() > send::MAX_MESSAGE_BYTES {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!("draft too large — max {} bytes of text", send::MAX_MESSAGE_BYTES),
        );
    }
    let (_path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some((address, email_id)) = messages::decode_message_id(&b.id) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "invalid message id — use an id from 'k2 mail messages'",
        );
    };
    // S11 unified DRAFT gate. `k2 mail draft` is LINKED-only (it APPENDs
    // a \Draft into the account's real Drafts folder); a hosted address
    // is replied to with `k2 mail reply` (governed send). The masked
    // message-level not_found covers every no-access case.
    let teach_reply = || {
        error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "'{address}' is an address on this K2 mail server — 'k2 mail draft' is for \
                 EXTERNAL assistant inboxes. Reply to this message with 'k2 mail reply {}'",
                b.id
            ),
        )
    };
    let masked = || {
        error_response(
            "404 Not Found",
            "not_found",
            &format!("no message '{}' in this workspace", b.id),
        )
    };
    let resolved = match access::can_draft(&project_id, &address) {
        Ok(ai) => ai,
        Err(ReadError::Usage(hint)) => {
            return error_response("400 Bad Request", "usage", &hint)
        }
        Err(_) => {
            // A hosted message the caller can read → teach reply; else
            // masked (no existence leak).
            return match access::can_read(&project_id, &address) {
                Ok(ai) if ai.source == Source::Hosted => teach_reply(),
                _ => masked(),
            };
        }
    };
    // A hosted inbox at draft+ still can't be "drafted into" — teach reply.
    let Some(inbox) = resolved.linked else {
        return teach_reply();
    };
    // The stable "draft saved" success shape (identical for both backends).
    let ok_draft = |folder: String| {
        ok_json(serde_json::json!({
            "ok": true,
            "id": b.id,
            "address": inbox.email_address,
            "folder": folder,
            "hint": format!(
                "draft saved to '{folder}' in {} — your human can review and send it from \
                 their own mail client",
                inbox.email_address
            ),
        }))
    };
    // §17.5 seam: DISPATCH the draft to the row's backend (O4). A Graph row
    // (Microsoft 365, kind='graph') lands the reply via
    // createReply+PATCH-body — its Bearer token is minted inside
    // `RealGraphHttp`, so there is NO vault password to resolve. Every
    // other linked row is IMAP (Gmail XOAUTH2 or app-password) and APPENDs
    // a `\Draft` with `external::save_reply_draft`. This is the DRAFT route
    // only — sending is the separate 'send'-level path (external_smtp for
    // IMAP; Graph send is not yet built).
    match messages::backend_for_address(&inbox.email_address) {
        MailBackend::Graph => {
            let http = std::sync::Arc::new(graph::RealGraphHttp::new(inbox.id.clone()));
            let backend = graph::GraphBackend::new(inbox.clone(), http);
            match backend.save_reply_draft(&inbox.id, &email_id, &b.body) {
                Ok(folder) => ok_draft(folder),
                Err(hint) => error_response("502 Bad Gateway", "engine", &hint),
            }
        }
        // IMAP: Gmail XOAUTH2 (auth_kind='oauth') OR generic app-password.
        _ => {
            let secrets = FileSecretStore::default();
            // For an OAuth-IMAP row (Gmail XOAUTH2) `login()` mints the
            // access token itself and IGNORES this param, so there is no
            // app-password to require; a `password` row must still have its
            // vaulted credential (503 → re-link).
            let is_oauth = matches!(
                external::read_oauth_fields(&inbox.id),
                Ok(f) if f.auth_kind == external::AUTH_OAUTH
            );
            let password = if is_oauth {
                String::new()
            } else {
                match secrets.resolve(&external::vault_key(&inbox.id)) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        return error_response(
                            "503 Service Unavailable",
                            "not_ready",
                            &format!(
                                "credentials for '{}' are missing from the vault — your human \
                                 can reconnect it with 'k2 mail link add'",
                                inbox.email_address
                            ),
                        )
                    }
                    Err(hint) => {
                        return error_response("503 Service Unavailable", "not_ready", &hint)
                    }
                }
            };
            match external::save_reply_draft(&RealImapOps, &inbox, &password, &email_id, &b.body) {
                Ok(folder) => ok_draft(folder),
                Err(e) => ext_error_response(e),
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — validation + gating + masking + the teaching
// error, no network (house rules): every path below stops BEFORE any
// IMAP dial (bad input, foreign ownership, or a missing vault
// credential). Deep add/draft behavior lives in mail::external with
// fakes; the wire conversation lives in mail::external_imap's
// loopback mock.
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
        (
            format!("mext-{label}-{short}"),
            format!("/tmp/mail-ext-routes-{label}-{}-{short}", std::process::id()),
        )
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

    fn cleanup_project(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // Grant rows keyed by this project (grantee) or by any inbox it
        // owns — purge both so no orphan lingers in the shared test DB.
        let _ = conn.execute(
            "DELETE FROM mail_inbox_grants WHERE project_id = ?1 \
             OR inbox_id IN (SELECT id FROM mail_external_inboxes WHERE owner_project_id = ?1)",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    }

    fn seed_external(project_id: &str, address: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at) VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100)",
            rusqlite::params![id, project_id, address],
        )
        .expect("seed external inbox");
        id
    }

    // ── add ──

    #[test]
    fn add_validates_everything_before_any_dial() {
        let resp = handle_external_add(b"not json");
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        // Field-by-field guidance.
        for (body, needle) in [
            (r#"{}"#, "project"),
            (r#"{"project":"/tmp/x"}"#, "address"),
            (r#"{"project":"/tmp/x","address":"a@b.example"}"#, "host"),
        ] {
            let resp = handle_external_add(body.as_bytes());
            assert_eq!(resp.status, "400 Bad Request", "{body}");
            assert!(
                body_json(&resp)["error"]["hint"].as_str().unwrap().contains(needle),
                "{body}: {}",
                resp.body
            );
        }

        // Spec validation fires before workspace resolution: V2 kinds
        // teach, plaintext TLS does not exist.
        let resp = handle_external_add(
            br#"{"project":"x","address":"a@b.example","host":"h","kind":"gmail-api"}"#,
        );
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("OAuth2"));
        let resp = handle_external_add(
            br#"{"project":"x","address":"a@b.example","host":"h","tls":"none"}"#,
        );
        assert_eq!(resp.status, "400 Bad Request");

        // Unknown workspace → the shared 404 shape.
        let resp = handle_external_add(
            br#"{"project":"no-such-workspace-xyz","address":"a@b.example","host":"h.example"}"#,
        );
        assert_eq!(resp.status, "404 Not Found");

        // Registered workspace but NO credential → usage naming both
        // spellings (still no dial).
        let (name, path) = unique("add");
        let project_id = insert_project(&name, &path);
        let resp = handle_external_add(
            serde_json::json!({
                "project": path, "address": "a@b.example", "host": "imap.example.com",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let hint = body_json(&resp)["error"]["hint"].as_str().unwrap().to_string();
        assert!(hint.contains("--pass-stdin") && hint.contains("--pass-ref"), "{hint}");

        // Both spellings at once → usage.
        let resp = handle_external_add(
            serde_json::json!({
                "project": path, "address": "a@b.example", "host": "imap.example.com",
                "password": "x", "passwordRef": "env:X",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("not both"));

        // A dead password ref fails loudly at add time.
        let resp = handle_external_add(
            serde_json::json!({
                "project": path, "address": "a@b.example", "host": "imap.example.com",
                "passwordRef": "env:K2_TEST_EXT_INBOX_MISSING_VAR",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("K2_TEST_EXT_INBOX_MISSING_VAR"));

        cleanup_project(&project_id);
    }

    // ── remove ──

    #[test]
    fn remove_validates_and_answers_not_found_with_the_list_pointer() {
        let resp = handle_external_remove(b"{}");
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("address"));

        let resp = handle_external_remove(br#"{"address":"ghost@nowhere.example"}"#);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("k2 mail external list"));
    }

    // (S11: the unified catalog + grant/revoke/set-primary/set-level
    // tests live in `routes_access` and `mail::access`.)

    // ── draft ──

    #[test]
    fn draft_validates_masks_and_teaches_before_any_dial() {
        let resp = handle_draft(b"not json");
        assert_eq!(resp.status, "400 Bad Request");
        for (body, needle) in [
            (r#"{}"#, "project"),
            (r#"{"project":"/tmp/x"}"#, "id"),
            (r#"{"project":"/tmp/x","id":"m_x"}"#, "body"),
        ] {
            let resp = handle_draft(body.as_bytes());
            assert_eq!(resp.status, "400 Bad Request", "{body}");
            assert!(
                body_json(&resp)["error"]["hint"].as_str().unwrap().contains(needle),
                "{body}: {}",
                resp.body
            );
        }

        let (name, path) = unique("draft");
        let project_id = insert_project(&name, &path);

        // Malformed id → usage.
        let resp = handle_draft(
            serde_json::json!({ "project": path, "id": "garbage", "body": "hi" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        // A message id naming a K2-HOSTED owned address → the teaching
        // error pointing at `k2 mail reply` (drafts are external-only).
        let local = format!("bot@{name}.example");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
                 owner_project_id, status, created_at) \
                 VALUES (?1, ?2, 'dom-x', 'acc-1', ?3, 'active', 100)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), local, project_id],
            )
            .expect("seed local address");
        }
        let token = messages::encode_message_id(&local, "M1");
        let resp = handle_draft(
            serde_json::json!({ "project": path, "id": token, "body": "hi" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let hint = body_json(&resp)["error"]["hint"].as_str().unwrap().to_string();
        assert!(hint.contains("k2 mail reply"), "{hint}");

        // A FOREIGN external inbox answers the masked not_found (never
        // reveals the inbox exists).
        let (name2, path2) = unique("draft-foreign");
        let project2 = insert_project(&name2, &path2);
        let foreign = format!("theirs-{}@ext.example", &project2[..8]);
        seed_external(&project2, &foreign);
        let token = messages::encode_message_id(&foreign, "uid:1:1");
        let resp = handle_draft(
            serde_json::json!({ "project": path, "id": token, "body": "hi" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(
            !body_json(&resp)["error"]["hint"].as_str().unwrap().contains(&foreign),
            "masked hint must not leak the address"
        );

        // The BOUND workspace with no vault credential → 503 not_ready
        // with the reconnect pointer (stops before the dial).
        let mine = format!("mine-{}@ext.example", &project_id[..8]);
        seed_external(&project_id, &mine);
        let token = messages::encode_message_id(&mine, "uid:1:1");
        let resp = handle_draft(
            serde_json::json!({ "project": path, "id": token, "body": "hi" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "not_ready");

        cleanup_project(&project_id);
        cleanup_project(&project2);
    }
}
