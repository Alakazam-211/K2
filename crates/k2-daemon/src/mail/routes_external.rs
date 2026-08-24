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
//! - `draft` (POST) is a **workspace-token agent verb**: gated only by
//!   `can_draft` (a draft is not a send). Reply form: the source
//!   message must live in a linked inbox (a K2-hosted source gets a
//!   teaching error pointing at `k2 mail reply`). Compose form: `--to`
//!   + `--subject`, no id; `--from` picks among draftable linked
//!   inboxes. The ONLY effect is an APPEND-\Draft into the account's
//!   real Drafts folder. **No send path exists.**
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
use crate::mail::external::{self, ExtError, ImapOps, Rfc822Attachment};
use crate::mail::external_imap::RealImapOps;
use crate::mail::external_smtp::OutAttachment;
use crate::mail::graph;
use crate::mail::jmap::MailAddr;
use crate::mail::messages::{self, MailBackend, ReadError};
use crate::mail::routes_send;
use crate::mail::secrets::{self, FileSecretStore, SecretStore as _};
use crate::mail::send;
use k2_core::db::schema::MailExternalInbox;

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
    /// Source message id (`m_…` from `k2 mail messages`). Empty = compose.
    id: String,
    body: String,
    to: serde_json::Value,
    subject: String,
    cc: serde_json::Value,
    from: String,
    attachments: serde_json::Value,
}

/// POST `/cli/mail/draft` — agent verb. Two mutually exclusive forms:
/// reply (`id` + `body`) or compose (`to` + `subject` + `body`, no id).
/// Gate is **only** [`access::can_draft`] — a draft is not a send (do
/// not call `require_linked_send_gate` / `mailAgentSend`). IMAP APPEND
/// `\Draft`; Graph compose and Graph+attach are teaching errors.
pub fn handle_draft(body: &[u8]) -> CliResponse {
    handle_draft_with(body, &RealImapOps)
}

fn handle_draft_with(body: &[u8], ops: &dyn ImapOps) -> CliResponse {
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
    if b.body.trim().is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'body' — the draft text",
        );
    }
    if b.body.len() > send::MAX_MESSAGE_BYTES {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!("draft too large — max {} bytes of text", send::MAX_MESSAGE_BYTES),
        );
    }
    let to_raw = match routes_send::string_list(&b.to, "to") {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let cc_raw = match routes_send::string_list(&b.cc, "cc") {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let has_id = !b.id.trim().is_empty();
    let has_to = !to_raw.is_empty();
    let has_subject = !b.subject.trim().is_empty();
    let has_from = !b.from.trim().is_empty();
    if has_id && (has_to || has_subject) {
        return error_response(
            "400 Bad Request",
            "usage",
            "pass either a message id (reply draft) or --to/--subject (compose draft), not both",
        );
    }
    if has_id && !cc_raw.is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "--cc is for compose drafts (k2 mail draft --to … --cc)",
        );
    }
    if has_id && has_from {
        return error_response(
            "400 Bad Request",
            "usage",
            "--from is for compose drafts (picks the linked inbox)",
        );
    }
    if !has_id {
        if !has_to {
            return error_response(
                "400 Bad Request",
                "usage",
                "compose draft requires 'to' and 'subject' — or pass 'id' to draft a reply \
                 (from 'k2 mail messages')",
            );
        }
        if !has_subject {
            return error_response(
                "400 Bad Request",
                "usage",
                "compose draft requires 'subject'",
            );
        }
    }
    let attach_specs = match routes_send::parse_attachment_specs(&serde_json::json!({
        "attachments": b.attachments,
    })) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let (path, project_id) = match resolve_caller(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    enum DraftKind {
        Reply { address: String, email_id: String, source_id: String },
        Compose { to: Vec<String>, cc: Vec<String>, subject: String },
    }
    let kind = if has_id {
        let Some((address, email_id)) = messages::decode_message_id(&b.id) else {
            return error_response(
                "400 Bad Request",
                "usage",
                "invalid message id — use an id from 'k2 mail messages'",
            );
        };
        DraftKind::Reply { address, email_id, source_id: b.id.clone() }
    } else {
        let to = match normalize_draft_recipients(&to_raw) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let cc = match normalize_draft_recipients(&cc_raw) {
            Ok(c) => c,
            Err(resp) => return resp,
        };
        DraftKind::Compose {
            to,
            cc,
            subject: b.subject.trim().to_string(),
        }
    };

    let (inbox, source_id) = match &kind {
        DraftKind::Reply { address, source_id, .. } => {
            match resolve_reply_inbox(&project_id, address, source_id) {
                Ok(inbox) => (inbox, Some(source_id.clone())),
                Err(resp) => return resp,
            }
        }
        DraftKind::Compose { .. } => match resolve_compose_from(&project_id, b.from.as_str()) {
            Ok(inbox) => (inbox, None),
            Err(resp) => return resp,
        },
    };

    let backend = messages::backend_for_address(&inbox.email_address);
    if matches!(backend, MailBackend::Graph) {
        if matches!(kind, DraftKind::Compose { .. }) {
            return error_response(
                "400 Bad Request",
                "usage",
                "composing a new draft is not supported on Microsoft 365 (Graph) inboxes yet \
                 — reply to an existing message with 'k2 mail draft <id> --body' (createReply)",
            );
        }
        if !attach_specs.is_empty() {
            return error_response(
                "400 Bad Request",
                "usage",
                "attachments on drafts aren't supported on Microsoft 365 (Graph) inboxes yet",
            );
        }
    }

    let attachments = match routes_send::read_workspace_attachments(&path, &attach_specs) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let parts: Vec<Rfc822Attachment> = attachments.into_iter().map(out_to_rfc822).collect();

    let ok_draft = |folder: String| {
        let mut v = serde_json::json!({
            "ok": true,
            "address": inbox.email_address,
            "folder": folder,
            "hint": format!(
                "draft saved to '{folder}' in {} — your human can review and send it from \
                 their own mail client",
                inbox.email_address
            ),
        });
        if let Some(id) = &source_id {
            v["id"] = serde_json::json!(id);
        }
        ok_json(v)
    };

    match backend {
        MailBackend::Graph => {
            let DraftKind::Reply { email_id, .. } = &kind else {
                unreachable!("Graph compose refused above");
            };
            let http = std::sync::Arc::new(graph::RealGraphHttp::new(inbox.id.clone()));
            let backend = graph::GraphBackend::new(inbox.clone(), http);
            match backend.save_reply_draft(&inbox.id, email_id, &b.body) {
                Ok(folder) => ok_draft(folder),
                Err(hint) => error_response("502 Bad Gateway", "engine", &hint),
            }
        }
        _ => {
            let password = match linked_draft_password(&inbox) {
                Ok(p) => p,
                Err(resp) => return resp,
            };
            match &kind {
                DraftKind::Reply { email_id, .. } => {
                    match external::save_reply_draft(
                        ops, &inbox, &password, email_id, &b.body, &parts,
                    ) {
                        Ok(folder) => ok_draft(folder),
                        Err(e) => ext_error_response(e),
                    }
                }
                DraftKind::Compose { to, cc, subject } => {
                    let to_addrs = as_mailaddrs(to);
                    let cc_addrs = as_mailaddrs(cc);
                    match external::save_compose_draft(
                        ops, &inbox, &password, &to_addrs, &cc_addrs, subject, &b.body, &parts,
                    ) {
                        Ok(folder) => ok_draft(folder),
                        Err(e) => ext_error_response(e),
                    }
                }
            }
        }
    }
}

fn out_to_rfc822(a: OutAttachment) -> Rfc822Attachment {
    Rfc822Attachment { filename: a.filename, content_type: a.content_type, bytes: a.bytes }
}

fn as_mailaddrs(list: &[String]) -> Vec<MailAddr> {
    list.iter()
        .map(|e| MailAddr { name: None, email: e.clone() })
        .collect()
}

fn normalize_draft_recipients(raw: &[String]) -> Result<Vec<String>, CliResponse> {
    match send::normalize_recipients(raw) {
        Ok(v) => Ok(v),
        Err(send::SendError::Usage(h)) | Err(send::SendError::NotFound(h)) => {
            Err(error_response("400 Bad Request", "usage", &h))
        }
        Err(e) => Err(error_response(
            "400 Bad Request",
            "usage",
            &format!("invalid recipient: {e:?}"),
        )),
    }
}

fn linked_draft_password(inbox: &MailExternalInbox) -> Result<String, CliResponse> {
    let is_oauth = matches!(
        external::read_oauth_fields(&inbox.id),
        Ok(f) if f.auth_kind == external::AUTH_OAUTH
    );
    if is_oauth {
        return Ok(String::new());
    }
    let secrets = FileSecretStore::default();
    match secrets.resolve(&external::vault_key(&inbox.id)) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(error_response(
            "503 Service Unavailable",
            "not_ready",
            &format!(
                "credentials for '{}' are missing from the vault — your human can reconnect \
                 it with 'k2 mail link add'",
                inbox.email_address
            ),
        )),
        Err(hint) => Err(error_response("503 Service Unavailable", "not_ready", &hint)),
    }
}

fn resolve_reply_inbox(
    project_id: &str,
    address: &str,
    source_id: &str,
) -> Result<MailExternalInbox, CliResponse> {
    let teach_reply = || {
        error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "'{address}' is an address on this K2 mail server — 'k2 mail draft' is for \
                 EXTERNAL assistant inboxes. Reply to this message with 'k2 mail reply {source_id}'"
            ),
        )
    };
    let masked = || {
        error_response(
            "404 Not Found",
            "not_found",
            &format!("no message '{source_id}' in this workspace"),
        )
    };
    let resolved = match access::can_draft(project_id, address) {
        Ok(ai) => ai,
        Err(ReadError::Usage(hint)) => {
            return Err(error_response("400 Bad Request", "usage", &hint))
        }
        Err(_) => {
            return match access::can_read(project_id, address) {
                Ok(ai) if ai.source == Source::Hosted => Err(teach_reply()),
                _ => Err(masked()),
            };
        }
    };
    resolved.linked.ok_or_else(teach_reply)
}

/// Compose `--from` via `can_draft` only. 0 draftable linked inboxes →
/// teach; 1 → implicit; N → require `--from`. Does **not** reuse send's
/// hosted-only implicit From resolver.
fn resolve_compose_from(
    project_id: &str,
    explicit: &str,
) -> Result<MailExternalInbox, CliResponse> {
    if let Some(addr) = Some(explicit.trim()).filter(|s| !s.is_empty()) {
        return match access::can_draft(project_id, addr) {
            Ok(ai) => match ai.linked {
                Some(inbox) => Ok(inbox),
                None => Err(error_response(
                    "400 Bad Request",
                    "usage",
                    &format!(
                        "'{addr}' is an address on this K2 mail server — compose drafts are \
                         for linked inboxes. Pass --from with a linked address ('k2 mail inboxes')"
                    ),
                )),
            },
            Err(ReadError::Usage(hint)) => {
                Err(error_response("400 Bad Request", "usage", &hint))
            }
            Err(_) => Err(error_response(
                "404 Not Found",
                "not_found",
                &format!("no address '{addr}' this workspace can draft FROM"),
            )),
        };
    }
    let all = access::draftable_linked(project_id);
    match all.len() {
        0 => Err(error_response(
            "400 Bad Request",
            "usage",
            "this workspace has no linked inbox it can draft FROM — your human can connect \
             one with 'k2 mail link add', or pass --from",
        )),
        1 => match access::can_draft(project_id, &all[0].0) {
            Ok(ai) => ai.linked.ok_or_else(|| {
                error_response(
                    "502 Bad Gateway",
                    "engine",
                    "linked inbox row missing from the resolved sender (unexpected)",
                )
            }),
            Err(ReadError::Usage(hint)) => {
                Err(error_response("400 Bad Request", "usage", &hint))
            }
            Err(ReadError::NotFound(h)) | Err(ReadError::Engine(h)) => {
                Err(error_response("404 Not Found", "not_found", &h))
            }
        },
        n => Err(error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "this workspace can draft from {n} linked inboxes — pass --from to pick the \
                 sender (see 'k2 mail inboxes')"
            ),
        )),
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
            (r#"{"project":"/tmp/x"}"#, "body"),
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

    fn seed_linked(
        project_id: &str,
        address: &str,
        kind: &str,
        auth_kind: &str,
        drafts_folder: Option<&str>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, kind, auth_kind, drafts_folder) \
             VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100, ?4, ?5, ?6)",
            rusqlite::params![id, project_id, address, kind, auth_kind, drafts_folder],
        )
        .expect("seed linked inbox");
        id
    }

    #[test]
    fn draft_compose_xor_and_missing_fields() {
        let resp = handle_draft(
            serde_json::json!({
                "project": "/tmp/x",
                "id": "m_abc",
                "to": "a@b.example",
                "subject": "s",
                "body": "hi",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        let hint = body_json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("not both"), "{hint}");

        let resp = handle_draft(
            serde_json::json!({ "project": "/tmp/x", "to": "a@b.example", "body": "hi" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        let hint = body_json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("subject"), "{hint}");

        let resp = handle_draft(
            serde_json::json!({ "project": "/tmp/x", "subject": "s", "body": "hi" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        let hint = body_json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("to"), "{hint}");

        let (name, path) = unique("compose-none");
        let project_id = insert_project(&name, &path);
        let resp = handle_draft(
            serde_json::json!({
                "project": path,
                "to": "someone@x.example",
                "subject": "Hello",
                "body": "hi",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let hint = body_json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("no linked inbox") || hint.contains("link add"), "{hint}");
        cleanup_project(&project_id);
    }

    #[test]
    fn draft_compose_appends_when_can_draft_even_if_mail_agent_send_off() {
        let (name, path) = unique("compose-ok");
        let project_id = insert_project(&name, &path);
        std::fs::create_dir_all(&path).ok();
        let mine = format!("mine-{}@ext.example", &project_id[..8]);
        seed_linked(&project_id, &mine, "imap", "oauth", Some("Drafts"));

        let ops = crate::mail::external::tests::FakeOps {
            folders: vec![("Drafts".to_string(), true)],
            ..Default::default()
        };
        let resp = handle_draft_with(
            serde_json::json!({
                "project": path,
                "to": "someone@x.example",
                "subject": "Hello",
                "body": "compose me",
            })
            .to_string()
            .as_bytes(),
            &ops,
        );
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_ne!(body_json(&resp)["error"]["code"], "gated");
        let appended = ops.appended.lock().expect("lock");
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].0, "Drafts");
        let text = String::from_utf8(appended[0].1.clone()).expect("ascii");
        assert!(text.contains("To: <someone@x.example>"), "{text}");
        assert!(text.contains("Subject: Hello"), "{text}");
        assert!(!text.contains("In-Reply-To:"), "{text}");
        drop(appended);

        let _ = std::fs::remove_dir_all(&path);
        cleanup_project(&project_id);
    }

    #[test]
    fn draft_graph_compose_and_graph_attach_are_teaching_errors() {
        let (name, path) = unique("graph-compose");
        let project_id = insert_project(&name, &path);
        let mine = format!("mine-{}@ext.example", &project_id[..8]);
        seed_linked(&project_id, &mine, "graph", "oauth", Some("Drafts"));

        let resp = handle_draft(
            serde_json::json!({
                "project": path,
                "to": "someone@x.example",
                "subject": "Hello",
                "body": "nope",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let hint = body_json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("Graph") || hint.contains("createReply"), "{hint}");

        let token = messages::encode_message_id(&mine, "graph:AAA");
        let resp = handle_draft(
            serde_json::json!({
                "project": path,
                "id": token,
                "body": "hi",
                "attachments": ["a.pdf"],
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let hint = body_json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("attachment"), "{hint}");

        cleanup_project(&project_id);
    }

    #[test]
    fn draft_attach_over_cap_is_usage() {
        let too_many: Vec<String> = (0..11).map(|i| format!("f{i}.txt")).collect();
        let resp = handle_draft(
            serde_json::json!({
                "project": "/tmp/x",
                "to": "a@b.example",
                "subject": "s",
                "body": "t",
                "attachments": too_many,
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        let hint = body_json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("too many attachments"), "{hint}");
        assert!(hint.contains("10"), "{hint}");
    }
}
