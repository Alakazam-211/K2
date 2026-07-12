//! `/cli/mail/send|reply|outbox|approvals/*` — SEND-concern handlers
//! (mail slice S5: gating, approvals queue, outbox, reply guardrails,
//! rate limits, audit).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! (PRD §8.4 / §10): send/reply = WORKSPACE token (`token_ok`), gated
//! by the effective `mail_agent_send` mode
//! (`k2_core::workspace::settings::mail_agent_send_for_path` — off →
//! 403 `gated`, CLI exit 3 with the Settings pointer); approvals
//! approve/deny = OWNER-OR-ADMIN (`token_is_owner_or_admin`, enforced
//! in the dispatcher's POST arm), and the approvals-list GET carries
//! the same owner gate in its own dispatcher clause (§11.1.3: owner
//! verbs hard-fail for agent tokens server-side). All mutations
//! POST-only (`require_post` + `post_allowed`,
//! feedback_post_only_route_guards); the POST arm runs these in
//! `spawn_blocking` (SQLite writes + blocking Stalwart dials +
//! `--wait` holds the request up to 900 s).
//!
//! FAIL-CLOSED invariants (pre-mortem #11, tested in [`send`]):
//! - every send writes a `mail_outbound` audit row BEFORE anything is
//!   handed to Stalwart — no row, no send;
//! - an unreachable queue table / settings read / rate counter NEVER
//!   bypasses governance;
//! - sender identity is stamped server-side: From resolves through the
//!   S3 ownership model ([`messages::owned_active_address`], masked
//!   `not_found`) — never trusted from the body; the display name is
//!   the workspace's agent display name, also server-chosen;
//! - Stalwart's queue owns retries; `send` reports
//!   "accepted-for-delivery", never "delivered" (pre-mortem #9).
//!
//! Bodies/subjects are never logged (pre-mortem #16); recipient lists
//! live in the audit rows only.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::access::{self, AccessInbox, Source};
use crate::mail::domains;
use crate::mail::external::{self, ExtError};
use crate::mail::external_imap::RealImapOps;
use crate::mail::external_smtp::{self, RealSmtpOps};
use crate::mail::messages::{self, ReadError};
use crate::mail::routes_messages::WaitSlot;
use crate::mail::secrets::{FileSecretStore, SecretStore as _};
use crate::mail::send::{
    self, DbOutboundStore, Gate, OutboundMessage, OutboundRequest, OutboundStore, SendError,
    SendOutcome, SubmitBackend,
};
use k2_core::db::schema::MailExternalInbox;

// ── Response helpers (the S2/S3/S4 error contract + the S5 codes) ───────

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

fn send_error_response(err: SendError) -> CliResponse {
    match err {
        SendError::Usage(h) => error_response("400 Bad Request", "usage", &h),
        SendError::NotFound(h) => error_response("404 Not Found", "not_found", &h),
        // The D4 gate: CLI maps `gated` to exit 3.
        SendError::Gated(h) => error_response("403 Forbidden", "gated", &h),
        SendError::SendMode(h) => error_response("409 Conflict", "send_mode", &h),
        SendError::Guardrail(h) => error_response("409 Conflict", "guardrail", &h),
        SendError::Conflict(h) => error_response("409 Conflict", "conflict", &h),
        SendError::RateLimited { window, hint } => CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "error": { "code": "rate_limited", "hint": hint, "window": window },
            })
            .to_string(),
        },
        SendError::Engine(h) => error_response("502 Bad Gateway", "engine", &h),
    }
}

fn read_error_response(err: ReadError) -> CliResponse {
    match err {
        ReadError::Usage(hint) => error_response("400 Bad Request", "usage", &hint),
        ReadError::NotFound(hint) => error_response("404 Not Found", "not_found", &hint),
        ReadError::Engine(hint) => error_response("502 Bad Gateway", "engine", &hint),
    }
}

/// LINKED-send ops errors → the shared {code, hint} shapes. `NotFound`
/// stays masked (a stale source id answers like a missing message).
fn ext_error_response(err: ExtError) -> CliResponse {
    match err {
        ExtError::Usage(h) => error_response("400 Bad Request", "usage", &h),
        ExtError::NotFound(h) => error_response("404 Not Found", "not_found", &h),
        ExtError::Exists(h) => error_response("409 Conflict", "exists", &h),
        ExtError::Engine(h) => error_response("502 Bad Gateway", "engine", &h),
    }
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Server-side identity: caller's workspace → `(path, project_id)`
/// (mirrors routes_messages — never trusted from raw params).
fn resolve_caller(project: &str) -> Result<(String, String), CliResponse> {
    // Wave 0: prefers scoped principal over client project= claim.
    crate::mail::identity::resolve_caller(project)
}

/// A teaching 403 when the caller CAN see the address (reads it) but
/// lacks SEND — it is read/draft-only for them. `gated` → CLI exit 3
/// (ask the primary/human). If they can't even read it, we mask (no
/// existence leak). Linked inboxes now CAN send once granted the 'send'
/// level (§17.5) — the teaching points at raising the level, and adds
/// the draft alternative for linked.
fn not_sendable_response(project_id: &str, address: &str) -> CliResponse {
    match access::can_read(project_id, address) {
        Ok(inbox) => {
            let draft_hint = if inbox.source == Source::Linked {
                ", or save a reply draft with 'k2 mail draft'"
            } else {
                ""
            };
            send_error_response(SendError::Gated(format!(
                "this inbox is read/draft-only for your workspace — ask the primary workspace \
                 for 'send' access (k2 mail access set-level){draft_hint}."
            )))
        }
        Err(e) => read_error_response(e),
    }
}

/// The server-stamped From, source-aware: an explicit address must pass
/// the S11 SEND gate (effective level 'send', EITHER source); none →
/// the workspace's single SENDABLE HOSTED address (linked send requires
/// an explicit `--from <linked-address>` — the implicit resolver stays
/// hosted-only). Returns the resolved [`AccessInbox`] so the caller can
/// branch on `source` (hosted → Stalwart+governance; linked → SMTP).
fn resolve_from_inbox(
    project_id: &str,
    explicit: Option<&str>,
) -> Result<AccessInbox, CliResponse> {
    if let Some(addr) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return match access::can_send(project_id, addr) {
            Ok(inbox) => Ok(inbox),
            Err(ReadError::Usage(hint)) => {
                Err(error_response("400 Bad Request", "usage", &hint))
            }
            // Masked, unless they can read it — then teach send is denied.
            Err(_) => Err(not_sendable_response(project_id, addr)),
        };
    }
    let all = access::sendable_hosted(project_id);
    match all.len() {
        0 => Err(error_response(
            "404 Not Found",
            "not_found",
            "this workspace has no address it can send FROM — mint one with 'k2 mail create', \
             ask the primary for 'send' access (k2 mail inboxes shows yourLevel), or pass \
             'from' with a linked address you can send from",
        )),
        1 => match access::can_send(project_id, &all[0].0) {
            Ok(inbox) => Ok(inbox),
            Err(e) => Err(read_error_response(e)),
        },
        n => Err(error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "this workspace can send from {n} addresses — pass 'from' to pick the sender \
                 (see 'k2 mail inboxes')"
            ),
        )),
    }
}

/// Coerce a JSON `string | [string]` field into a list.
fn string_list(v: &serde_json::Value, field: &str) -> Result<Vec<String>, CliResponse> {
    match v {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::String(s) => Ok(vec![s.clone()]),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|i| {
                i.as_str().map(String::from).ok_or_else(|| {
                    error_response(
                        "400 Bad Request",
                        "usage",
                        &format!("'{field}' must be an address string or a list of them"),
                    )
                })
            })
            .collect(),
        _ => Err(error_response(
            "400 Bad Request",
            "usage",
            &format!("'{field}' must be an address string or a list of them"),
        )),
    }
}

/// The §11.1.4 `--wait` params: `Ok(None)` = no wait; `Ok(Some(secs))`
/// = block-until-decided with that timeout (default 300, max 900 —
/// the S4 wait bounds).
fn wait_params(v: &serde_json::Value) -> Result<Option<u64>, CliResponse> {
    if !v["wait"].as_bool().unwrap_or(false) {
        return Ok(None);
    }
    match v.get("timeout") {
        None | Some(serde_json::Value::Null) => Ok(Some(messages::WAIT_DEFAULT_TIMEOUT_SECS)),
        Some(t) => match t.as_u64() {
            Some(n) if (1..=messages::WAIT_MAX_TIMEOUT_SECS).contains(&n) => Ok(Some(n)),
            _ => Err(error_response(
                "400 Bad Request",
                "usage",
                &format!(
                    "invalid 'timeout' — seconds from 1 to {} (default {})",
                    messages::WAIT_MAX_TIMEOUT_SECS,
                    messages::WAIT_DEFAULT_TIMEOUT_SECS
                ),
            )),
        },
    }
}

/// The shared pipeline tail for send + reply: acquire the wait slot
/// (when asked), run the gate dispatch, emit events, shape the reply.
/// The submit backend is constructed ONLY for on-mode (approval/off
/// never dial the engine — queueing works while Stalwart is down).
fn dispatch_and_respond(
    project_id: &str,
    project_path: &str,
    agent_name: &str,
    account_id: &str,
    msg: &OutboundMessage,
    gate: Gate,
    wait_timeout: Option<u64>,
) -> CliResponse {
    let store = DbOutboundStore::default();
    let engine_client;
    let backend: Option<&dyn SubmitBackend> = match gate {
        Gate::On => match domains::engine_from_db() {
            Ok((client, _)) => {
                engine_client = client;
                Some(&engine_client)
            }
            Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
        },
        _ => None,
    };
    // The wait slot is claimed BEFORE queueing: a fanned-out caller is
    // refused without consuming queue/audit budget (pre-mortem #10 —
    // same RAII cap as the S4 wait).
    let _slot = if wait_timeout.is_some() && gate == Gate::Approval {
        match WaitSlot::try_acquire(project_id) {
            Some(s) => Some(s),
            None => {
                return CliResponse {
                    status: "429 Too Many Requests",
                    content_type: "application/json",
                    body: serde_json::json!({
                        "ok": false,
                        "error": {
                            "code": "rate_limited",
                            "hint": "this workspace already has the maximum wait calls open — \
                                     retry without 'wait', then poll 'k2 mail outbox'",
                        },
                    })
                    .to_string(),
                }
            }
        }
    } else {
        None
    };
    let req = OutboundRequest { project_id, agent_name, account_id, message: msg };
    let outcome = match send::gate_and_dispatch(&store, backend, gate, &req, now_secs()) {
        Ok(o) => o,
        Err(e) => return send_error_response(e),
    };
    match outcome {
        SendOutcome::Queued { id } => {
            // Notify the owner (content-free payload — the Approvals
            // tab refetches; feedback-notification conventions).
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendApprovalRequested,
                serde_json::json!({
                    "outboundId": id,
                    "projectPath": project_path,
                    "agentName": agent_name,
                }),
            );
            let Some(timeout_secs) = wait_timeout else {
                return ok_json(serde_json::json!({
                    "ok": true,
                    "queued": true,
                    "id": id,
                    "status": "pending_approval",
                    "hint": format!(
                        "queued for approval ({id}) — your human decides in Settings → \
                         Email → Approvals; track with 'k2 mail outbox {id}'"
                    ),
                }));
            };
            // §11.1.4: block until decided (bounded in-handler poll —
            // the dispatcher runs this arm in spawn_blocking).
            let mut poll = || -> Result<(String, Option<String>), String> {
                match store.load(&id)? {
                    Some(row) => Ok((row.status, row.note)),
                    None => Err(format!("outbound '{id}' vanished while waiting")),
                }
            };
            let mut now = now_secs;
            let mut sleep = |d: std::time::Duration| std::thread::sleep(d);
            match send::wait_for_decision(
                &mut poll,
                timeout_secs,
                send::DECISION_POLL_SECS,
                &mut now,
                &mut sleep,
            ) {
                Ok(Some((db_status, note))) => ok_json(serde_json::json!({
                    "ok": true,
                    "id": id,
                    "timedOut": false,
                    "status": send::wire_status(&db_status),
                    "statusNote": send::status_note(&db_status),
                    "note": note,
                })),
                // Timeout: the message is STILL QUEUED (exit 2 at the
                // CLI; §11.1.4 wording).
                Ok(None) => ok_json(serde_json::json!({
                    "ok": true,
                    "id": id,
                    "timedOut": true,
                    "status": "pending_approval",
                    "hint": format!(
                        "not decided within {timeout_secs}s — the message is still queued; \
                         check 'k2 mail outbox {id}'"
                    ),
                })),
                Err(hint) => error_response("502 Bad Gateway", "engine", &hint),
            }
        }
        SendOutcome::Submitted { id } => ok_json(serde_json::json!({
            "ok": true,
            "id": id,
            "status": "submitted",
            "hint": format!(
                "accepted-for-delivery — final delivery is the receiving server's \
                 business; track with 'k2 mail outbox {id}'"
            ),
        })),
        SendOutcome::SubmitFailed { id, error } => CliResponse {
            status: "502 Bad Gateway",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "id": id,
                "status": "failed",
                "error": { "code": "engine", "hint": format!("submission failed: {error}") },
            })
            .to_string(),
        },
    }
}

// ── LINKED send (§17.5, UNGATED — SMTP submission) ──────────────────────

/// Resolve a linked inbox's vaulted app-password (same key as IMAP).
/// Missing/unreadable → 503 not_ready with the reconnect pointer (never
/// a leaked secret).
fn linked_password(inbox: &MailExternalInbox) -> Result<String, CliResponse> {
    // For an OAuth-IMAP row (Gmail XOAUTH2) the backend mints the access
    // token itself and IGNORES this param, so there is no app-password to
    // require; a `password` row must still have its vaulted credential.
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

/// UNGATED linked send over SMTP. `can_send` (effective 'send' on the
/// linked inbox) has ALREADY passed. Linked send is deliberately NOT
/// behind the `mail_agent_send` off/approval/on gate for now — Rosson:
/// unified gating for linked lands with the wider email layer.
///
/// ⚠ FUTURE GATE HOME: a per-message linked-send governance check (and
/// any audit row) belongs right HERE, before the SMTP submission — the
/// hosted path's queue/audit/rate-limit machinery is intentionally NOT
/// applied to linked yet.
// ── Attachments (§17.5 send/reply): daemon reads workspace-relative ─────

/// Max attachments per message, and the per-file / total size caps
/// (enforced daemon-side after resolving each path; the CLI mirrors the
/// count/path shape but does NO file I/O — the daemon owns the bytes).
const MAX_ATTACHMENTS: usize = 10;
const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
const MAX_ATTACHMENTS_TOTAL_BYTES: u64 = 25 * 1024 * 1024;

/// Parse the `attachments` array (workspace-relative path STRINGS — the
/// CLI sends paths, never bytes). Enforces the count cap and the
/// non-empty-string shape BEFORE any file is touched. Absent/null → none.
fn parse_attachment_specs(v: &serde_json::Value) -> Result<Vec<String>, CliResponse> {
    let raw = match v.get("attachments") {
        None | Some(serde_json::Value::Null) => return Ok(Vec::new()),
        Some(serde_json::Value::Array(a)) => a,
        Some(_) => {
            return Err(error_response(
                "400 Bad Request",
                "usage",
                "'attachments' must be an array of workspace-relative file paths",
            ))
        }
    };
    if raw.len() > MAX_ATTACHMENTS {
        return Err(error_response(
            "400 Bad Request",
            "usage",
            &format!("too many attachments ({}) — max {}", raw.len(), MAX_ATTACHMENTS),
        ));
    }
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        let Some(p) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(error_response(
                "400 Bad Request",
                "usage",
                "each attachment must be a non-empty workspace-relative file path",
            ));
        };
        out.push(p.to_string());
    }
    Ok(out)
}

/// A content-type from the filename extension (fallback
/// `application/octet-stream`). Small deliberate table — the transfer is
/// base64 either way, so an unknown type just rides as a generic blob.
fn content_type_for(filename: &str) -> String {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let ct = match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "xml" => "application/xml",
        "ics" => "text/calendar; charset=utf-8",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "doc" => "application/msword",
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    };
    ct.to_string()
}

/// Resolve each spec workspace-relative (the read-side mirror of the
/// `--out` guard — rejects absolute/`..`/symlink escapes), enforce the
/// per-file + total size caps (checking metadata BEFORE reading the
/// bytes), and read them into [`external_smtp::OutAttachment`]s. The
/// filename is the basename; the content-type is derived from it. Bytes
/// are never logged. Any resolve/read/cap failure names the path and
/// aborts the whole send (nothing is submitted).
fn read_workspace_attachments(
    ws_root: &str,
    specs: &[String],
) -> Result<Vec<external_smtp::OutAttachment>, CliResponse> {
    let mut out = Vec::with_capacity(specs.len());
    let mut total: u64 = 0;
    for spec in specs {
        let path = match messages::resolve_in_path(ws_root, spec) {
            Ok(p) => p,
            Err(messages::InPathError::Usage(hint)) => {
                return Err(error_response("400 Bad Request", "usage", &hint))
            }
            Err(messages::InPathError::NotFound(p)) => {
                return Err(error_response(
                    "404 Not Found",
                    "not_found",
                    &format!("attachment file not found in this workspace: {p}"),
                ))
            }
        };
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                return Err(error_response(
                    "400 Bad Request",
                    "usage",
                    &format!("cannot read attachment '{spec}': {e}"),
                ))
            }
        };
        if !meta.is_file() {
            return Err(error_response(
                "400 Bad Request",
                "usage",
                &format!("attachment '{spec}' is not a regular file"),
            ));
        }
        let size = meta.len();
        if size > MAX_ATTACHMENT_BYTES {
            return Err(error_response(
                "400 Bad Request",
                "usage",
                &format!(
                    "attachment '{spec}' is too large ({size} bytes) — max {} bytes per file",
                    MAX_ATTACHMENT_BYTES
                ),
            ));
        }
        total = total.saturating_add(size);
        if total > MAX_ATTACHMENTS_TOTAL_BYTES {
            return Err(error_response(
                "400 Bad Request",
                "usage",
                &format!(
                    "attachments total too large ({total} bytes) — max {} bytes across all files",
                    MAX_ATTACHMENTS_TOTAL_BYTES
                ),
            ));
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                return Err(error_response(
                    "400 Bad Request",
                    "usage",
                    &format!("cannot read attachment '{spec}': {e}"),
                ))
            }
        };
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "attachment".to_string());
        let content_type = content_type_for(&filename);
        out.push(external_smtp::OutAttachment { filename, content_type, bytes });
    }
    Ok(out)
}

/// The stable refusal when a HOSTED (K2 mailbox) send carries
/// attachments — the linked SMTP path is fully built, the hosted JMAP
/// blob-upload path is a documented follow-up (§17.5).
fn hosted_attachments_unsupported() -> CliResponse {
    error_response(
        "400 Bad Request",
        "usage",
        "attachments aren't supported yet when sending from a hosted K2 mailbox — send from a \
         linked inbox (see 'k2 mail access'), or include a link in the body",
    )
}

fn dispatch_linked_send(
    project_id: &str,
    agent_name: &str,
    inbox: MailExternalInbox,
    to: &[String],
    cc: &[String],
    subject: &str,
    body: &str,
    attachments: &[external_smtp::OutAttachment],
) -> CliResponse {
    let password = match linked_password(&inbox) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match external_smtp::send_linked_message(
        &RealSmtpOps,
        &inbox,
        &password,
        to,
        cc,
        subject,
        body,
        attachments,
    ) {
        Ok(receipt) => {
            record_linked_outbox(project_id, agent_name, &inbox.email_address, &receipt, body);
            ok_json(serde_json::json!({
                "ok": true,
                "status": "submitted",
                "from": inbox.email_address,
                "hint": format!(
                    "submitted from {} via SMTP — accepted by the provider (K2 doesn't confirm \
                     final delivery); see it in 'k2 mail outbox'",
                    inbox.email_address
                ),
            }))
        }
        Err(e) => ext_error_response(e),
    }
}

/// #31.5: record a SUCCESSFUL linked SMTP submission in the outbox so the
/// agent has a "what did I just send" trail (`k2 mail outbox`). The mail
/// is already gone by the time we get here — a failed audit write is
/// logged, NEVER fatal (never turns a real send into a reported failure).
/// The row lands `submitted` (accepted-for-delivery — never "delivered").
fn record_linked_outbox(
    project_id: &str,
    agent_name: &str,
    from: &str,
    receipt: &external_smtp::LinkedReceipt,
    body: &str,
) {
    let msg = OutboundMessage {
        from_name: Some(agent_name.to_string()),
        from: from.to_string(),
        to: receipt.to.clone(),
        cc: receipt.cc.clone(),
        subject: receipt.subject.clone(),
        text_body: body.to_string(),
        in_reply_to: None,
        references: None,
    };
    let store = DbOutboundStore::default();
    if let Err(e) = send::record_linked_submitted(
        &store,
        project_id,
        agent_name,
        &msg,
        &receipt.attachments,
        now_secs(),
    ) {
        k2_core::log_debug!(
            "[mail/send] linked outbox audit write failed (the send itself succeeded): {e}"
        );
    }
}

/// UNGATED linked REPLY over SMTP (same governance stance as
/// [`dispatch_linked_send`]). Fetches the source over IMAP, threads, and
/// submits. `source_uid_token` is the linked message's `uid:…` id.
fn dispatch_linked_reply(
    project_id: &str,
    agent_name: &str,
    inbox: MailExternalInbox,
    source_uid_token: &str,
    body: &str,
    attachments: &[external_smtp::OutAttachment],
) -> CliResponse {
    let password = match linked_password(&inbox) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match external_smtp::send_linked_reply(
        &RealImapOps,
        &RealSmtpOps,
        &inbox,
        &password,
        source_uid_token,
        body,
        attachments,
    ) {
        Ok(receipt) => {
            record_linked_outbox(project_id, agent_name, &inbox.email_address, &receipt, body);
            let recipient = receipt.to.first().cloned().unwrap_or_default();
            ok_json(serde_json::json!({
                "ok": true,
                "status": "submitted",
                "from": inbox.email_address,
                "to": recipient,
                "hint": format!(
                    "reply submitted from {} to {} via SMTP — accepted by the provider; see it \
                     in 'k2 mail outbox'",
                    inbox.email_address, recipient
                ),
            }))
        }
        Err(e) => ext_error_response(e),
    }
}

// ── POST /cli/mail/send ─────────────────────────────────────────────────

/// S5: gated outbound. Body: `{project, to (string|[string]), subject,
/// body, cc?, from?, wait?, timeout?}`. From is SERVER-STAMPED (owned
/// address; the workspace's single one when `from` is omitted).
/// off → 403 `gated` (CLI exit 3); approval → pending row + owner
/// notification (`--wait` blocks until decided); on → audit row +
/// submit ("accepted-for-delivery").
pub fn handle_send(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(project) = v["project"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'project' (workspace name | path | UUID)",
        );
    };
    let to_raw = match string_list(&v["to"], "to") {
        Ok(l) if !l.is_empty() => l,
        Ok(_) => {
            return error_response(
                "400 Bad Request",
                "usage",
                "missing 'to' — at least one recipient",
            )
        }
        Err(resp) => return resp,
    };
    let Some(subject) = v["subject"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response("400 Bad Request", "usage", "missing 'subject'");
    };
    let Some(body_text) = v["body"].as_str().filter(|s| !s.trim().is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'body' — the message text",
        );
    };
    let cc_raw = match string_list(&v["cc"], "cc") {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let wait_timeout = match wait_params(&v) {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    // Attachment PATHS (the daemon reads them workspace-relative below):
    // parse + count-cap now, before any engine/identity work.
    let attach_specs = match parse_attachment_specs(&v) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let (path, project_id) = match resolve_caller(project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Resolve the sender FIRST — its SOURCE decides the whole pipeline:
    // hosted goes through Stalwart submission + the mail_agent_send gate;
    // LINKED goes out over SMTP and is UNGATED (§17.5). `can_send` masks
    // any no-access case identically for both sources.
    let from_inbox = match resolve_from_inbox(&project_id, v["from"].as_str()) {
        Ok(i) => i,
        Err(resp) => return resp,
    };

    // Always-on recipient + size caps (§8.4) — shared by both sources,
    // before any normalize churn on absurd lists.
    if to_raw.len() + cc_raw.len() > send::MAX_RECIPIENTS {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "too many recipients ({}) — max {} per message",
                to_raw.len() + cc_raw.len(),
                send::MAX_RECIPIENTS
            ),
        );
    }
    let to = match send::normalize_recipients(&to_raw) {
        Ok(t) => t,
        Err(e) => return send_error_response(e),
    };
    let cc = match send::normalize_recipients(&cc_raw) {
        Ok(c) => c,
        Err(e) => return send_error_response(e),
    };
    // The always-on message-size cap (§8.4) — on the composed text
    // (subject + body). Shared by both sources.
    if subject.len() + body_text.len() > send::MAX_MESSAGE_BYTES {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "message too large ({} bytes) — max {} bytes of text",
                subject.len() + body_text.len(),
                send::MAX_MESSAGE_BYTES
            ),
        );
    }

    match from_inbox.source {
        // ── LINKED: ungated SMTP submission (§17.5) ──
        Source::Linked => {
            let Some(inbox) = from_inbox.linked else {
                return error_response(
                    "502 Bad Gateway",
                    "engine",
                    "linked inbox row missing from the resolved sender (unexpected)",
                );
            };
            // Read the attachment files from THIS workspace (caps enforced).
            let attachments = match read_workspace_attachments(&path, &attach_specs) {
                Ok(a) => a,
                Err(resp) => return resp,
            };
            let agent_name = k2_core::workspace::display::agent_display_name(&path);
            dispatch_linked_send(
                &project_id,
                &agent_name,
                inbox,
                &to,
                &cc,
                subject,
                body_text,
                &attachments,
            )
        }
        // ── HOSTED: the existing governed Stalwart path ──
        Source::Hosted => {
            // Hosted attachments are a documented follow-up (JMAP blob
            // upload) — refuse cleanly rather than silently drop them.
            if !attach_specs.is_empty() {
                return hosted_attachments_unsupported();
            }
            let Some(account_id) = from_inbox.account_id.clone() else {
                return error_response(
                    "502 Bad Gateway",
                    "engine",
                    &format!(
                        "address '{}' has no mailbox on the mail server",
                        from_inbox.address
                    ),
                );
            };
            // The D4 gate — fail-closed reader (tested in mail::send).
            let gate = match send::effective_gate(Ok(
                k2_core::workspace::settings::mail_agent_send_for_path(&path),
            )) {
                Ok(g) => g,
                Err(e) => return send_error_response(e),
            };
            if gate == Gate::Off {
                return send_error_response(SendError::Gated(
                    "outbound email is disabled for this workspace. Your human can enable it \
                     in Settings → Email → Sending"
                        .to_string(),
                ));
            }
            // The FROM domain's send mode (D1: receive-only refuses; relay
            // validates its config at use time — fail-closed).
            if let Err(e) =
                send::check_domain_send_mode(&FileSecretStore::default(), &from_inbox.address)
            {
                return send_error_response(e);
            }
            let agent_name = k2_core::workspace::display::agent_display_name(&path);
            let msg = OutboundMessage {
                from_name: Some(agent_name.clone()),
                from: from_inbox.address,
                to,
                cc,
                subject: subject.to_string(),
                text_body: body_text.to_string(),
                in_reply_to: None,
                references: None,
            };
            dispatch_and_respond(
                &project_id,
                &path,
                &agent_name,
                &account_id,
                &msg,
                gate,
                wait_timeout,
            )
        }
    }
}

// ── POST /cli/mail/reply ────────────────────────────────────────────────

/// S5: guardrailed reply (§8.4). Body: `{project, id, body, wait?,
/// timeout?}`. Recipient LOCKED to the original sender, From LOCKED to
/// the receiving address, In-Reply-To/References stamped, loop caps
/// (no self-chains, ≤100 References, ≤4 replies/thread/hour),
/// DMARC-fail gated outside approval mode.
pub fn handle_reply(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(project) = v["project"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'project' (workspace name | path | UUID)",
        );
    };
    let Some(token) = v["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — a message id from 'k2 mail messages'",
        );
    };
    let Some(body_text) = v["body"].as_str().filter(|s| !s.trim().is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'body' — the reply text",
        );
    };
    let wait_timeout = match wait_params(&v) {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    // Attachment PATHS (read workspace-relative below): parse + count-cap
    // now, before any identity/engine work.
    let attach_specs = match parse_attachment_specs(&v) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let (path, project_id) = match resolve_caller(project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // §8.4: reply only to a message received at an address this workspace
    // can SEND from — the address rides inside the opaque id (S4) and
    // re-passes the masked ownership gate here. Resolving the sender's
    // SOURCE first decides the pipeline (hosted → governed Stalwart;
    // LINKED → ungated SMTP, §17.5) — exactly like `handle_send`.
    let Some((address, email_id)) = messages::decode_message_id(token) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "invalid message id — use an id from 'k2 mail messages'",
        );
    };
    let masked = || {
        error_response(
            "404 Not Found",
            "not_found",
            &format!("no message '{token}' in this workspace"),
        )
    };
    let from_inbox = match access::can_send(&project_id, &address) {
        Ok(inbox) => inbox,
        Err(ReadError::Usage(hint)) => {
            return error_response("400 Bad Request", "usage", &hint)
        }
        Err(_) => {
            return match access::can_read(&project_id, &address) {
                Ok(_) => not_sendable_response(&project_id, &address),
                Err(_) => masked(),
            }
        }
    };
    // The size cap fires before the engine dial (the subject the
    // reply inherits is negligible next to the 10 MB budget).
    if body_text.len() > send::MAX_MESSAGE_BYTES {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!("message too large — max {} bytes of text", send::MAX_MESSAGE_BYTES),
        );
    }

    match from_inbox.source {
        // ── LINKED: ungated threaded SMTP reply (§17.5) ──
        Source::Linked => {
            let Some(inbox) = from_inbox.linked else {
                return error_response(
                    "502 Bad Gateway",
                    "engine",
                    "linked inbox row missing from the resolved sender (unexpected)",
                );
            };
            // `email_id` is the linked message's `uid:…` token (the
            // reply threading + IMAP source-fetch happen in external_smtp).
            let attachments = match read_workspace_attachments(&path, &attach_specs) {
                Ok(a) => a,
                Err(resp) => return resp,
            };
            let agent_name = k2_core::workspace::display::agent_display_name(&path);
            dispatch_linked_reply(
                &project_id,
                &agent_name,
                inbox,
                &email_id,
                body_text,
                &attachments,
            )
        }
        // ── HOSTED: the existing governed reply path ──
        Source::Hosted => {
            // Hosted attachments are a documented follow-up (JMAP blob
            // upload) — refuse cleanly rather than silently drop them.
            if !attach_specs.is_empty() {
                return hosted_attachments_unsupported();
            }
            let from_address = from_inbox.address.clone();
            let Some(account_id) = from_inbox.account_id.clone() else {
                return masked();
            };
            // Gate: off refuses BEFORE any engine dial.
            let gate = match send::effective_gate(Ok(
                k2_core::workspace::settings::mail_agent_send_for_path(&path),
            )) {
                Ok(g) => g,
                Err(e) => return send_error_response(e),
            };
            if gate == Gate::Off {
                return send_error_response(SendError::Gated(
                    "outbound email is disabled for this workspace. Your human can enable it \
                     in Settings → Email → Sending"
                        .to_string(),
                ));
            }
            // Fetch the original's reply context (sender, threading
            // headers, auth verdicts) from its backend.
            let (client, _hostname) = match domains::engine_from_db() {
                Ok(c) => c,
                Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
            };
            let ctx = match client.email_get_reply_context(&account_id, &email_id) {
                Ok(Some(ctx)) => ctx,
                Ok(None) => return masked(),
                Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
            };

            // §8.4 guardrails: locked recipient, loop caps, DMARC gate.
            let recipient = match send::check_reply_guardrails(&ctx, &from_address, gate) {
                Ok(r) => r,
                Err(e) => return send_error_response(e),
            };
            if let Err(e) =
                send::check_domain_send_mode(&FileSecretStore::default(), &from_address)
            {
                return send_error_response(e);
            }
            let now = now_secs();
            let thread_key = format!(
                "{project_id}/{}",
                ctx.thread_id.clone().unwrap_or_else(|| email_id.clone())
            );
            if let Err(e) = send::check_and_record_thread_reply(&thread_key, now) {
                return send_error_response(e);
            }

            let agent_name = k2_core::workspace::display::agent_display_name(&path);
            let msg = OutboundMessage {
                from_name: Some(agent_name.clone()),
                from: from_address.clone(),
                to: vec![recipient],
                cc: Vec::new(),
                subject: send::reply_subject(&ctx.subject),
                text_body: body_text.to_string(),
                in_reply_to: ctx.message_id.clone(),
                references: send::build_out_references(
                    ctx.references.as_deref(),
                    ctx.message_id.as_deref(),
                ),
            };
            dispatch_and_respond(
                &project_id,
                &path,
                &agent_name,
                &account_id,
                &msg,
                gate,
                wait_timeout,
            )
        }
    }
}

// ── GET /cli/mail/outbox ────────────────────────────────────────────────

/// S5 (§11.1.9): the caller's outbound rows, newest first — or one row
/// via `?id=out_…` (masked `not_found` outside the workspace). Params:
/// `project` (required) · `id` · `limit` (default 20, max 100).
/// Expired pending items lazily auto-deny on every read (§12).
pub fn handle_outbox(params: &HashMap<String, String>) -> CliResponse {
    let project = match crate::cli::need_project(params) {
        Ok(p) => p,
        Err(_) => {
            return error_response(
                "400 Bad Request",
                "usage",
                "missing 'project' (workspace name | path | UUID)",
            )
        }
    };
    let limit: usize = match params.get("limit").map(|v| v.parse::<usize>()) {
        None => 20,
        Some(Ok(n)) if n >= 1 => n.min(100),
        _ => {
            return error_response(
                "400 Bad Request",
                "usage",
                "invalid 'limit' — a number from 1 to 100 (default 20)",
            )
        }
    };
    let (_path, project_id) = match resolve_caller(&project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    send::auto_deny_expired(now_secs());

    if let Some(id) = params.get("id").map(String::as_str).map(str::trim).filter(|s| !s.is_empty())
    {
        let store = DbOutboundStore::default();
        let row = match store.load(id) {
            Ok(r) => r,
            Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
        };
        // Masked: a foreign row answers exactly like a missing one.
        let Some(row) = row.filter(|r| r.owner_project_id == project_id) else {
            return error_response(
                "404 Not Found",
                "not_found",
                &format!("no outbound message '{id}' in this workspace"),
            );
        };
        return ok_json(serde_json::json!({
            "ok": true,
            "outbound": send::outbound_json(&row),
        }));
    }

    let rows = send::list_for_project(&project_id, limit);
    let list: Vec<serde_json::Value> = rows.iter().map(send::outbound_json).collect();
    ok_json(serde_json::json!({
        "ok": true,
        "count": list.len(),
        "outbox": list,
    }))
}

// ── GET /cli/mail/approvals/list ────────────────────────────────────────

/// S5: the owner's pending-outbound queue (Settings→Email Approvals
/// tab), oldest first, with a to/subject/body preview per item.
/// OWNER-OR-ADMIN — enforced in the dispatcher's approvals-list clause
/// (this handler never runs for a plain workspace token). Expired
/// items lazily auto-deny (§12).
pub fn handle_approvals_list(_params: &HashMap<String, String>) -> CliResponse {
    let now = now_secs();
    send::auto_deny_expired(now);
    let store = DbOutboundStore::default();
    let rows = send::list_pending();
    let list: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut v = send::outbound_json(row);
            // Body preview from the stored composed message; a
            // missing/corrupt body file lists WITHOUT a preview (the
            // decision buttons still work — approve would then fail
            // loudly on the unreadable body).
            let preview: Option<String> = store
                .load_message(&row.id)
                .ok()
                .map(|m| m.text_body.chars().take(280).collect());
            v["bodyPreview"] = serde_json::json!(preview);
            v["workspace"] = serde_json::json!(project_name(&row.owner_project_id));
            v["expiresAt"] = serde_json::json!(row.created_at + send::APPROVAL_EXPIRE_SECS);
            v
        })
        .collect();
    ok_json(serde_json::json!({
        "ok": true,
        "count": list.len(),
        "pending": list,
    }))
}

/// Workspace display name for the Approvals queue rows.
fn project_name(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT name FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get(0),
    )
    .ok()
}

// ── POST /cli/mail/approvals/approve ────────────────────────────────────

/// S5: owner-or-admin approve → ATOMIC `pending → approved` → submit
/// via the LOCAL Stalwart (the audit row has existed since queue
/// time). Body: `{id, note?, decidedBy?}`. Already-decided/unknown ids
/// refuse WITHOUT submitting (pre-mortem #11).
pub fn handle_approvals_approve(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(id) = v["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — an outbound id from the approvals list (out_…)",
        );
    };
    let note = v["note"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let decided_by = v["decidedBy"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let now = now_secs();
    // Expired items flip to denied FIRST — a 7-day-old approve answers
    // the conflict, it never sends (§12).
    send::auto_deny_expired(now);

    let store = DbOutboundStore::default();
    let engine = domains::engine_from_db();
    let backend: Result<&dyn SubmitBackend, String> = match &engine {
        Ok((client, _)) => Ok(client),
        Err(e) => Err(e.clone()),
    };
    let mut account_for_from = |from: &str| -> Result<String, String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT stalwart_account_id FROM mail_addresses \
             WHERE address = ?1 AND status = 'active'",
            rusqlite::params![from],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .ok_or_else(|| format!("sender address '{from}' is no longer active"))
    };
    match send::approve_and_submit(
        &store,
        backend,
        &mut account_for_from,
        id,
        decided_by.unwrap_or("owner"),
        note,
        now,
    ) {
        Ok(send::ApproveOutcome::Submitted) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendDecided,
                serde_json::json!({ "outboundId": id, "status": "submitted" }),
            );
            ok_json(serde_json::json!({
                "ok": true,
                "id": id,
                "status": "submitted",
                "hint": "approved and accepted-for-delivery",
            }))
        }
        Ok(send::ApproveOutcome::FailedToSubmit(error)) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendDecided,
                serde_json::json!({ "outboundId": id, "status": "failed" }),
            );
            CliResponse {
                status: "502 Bad Gateway",
                content_type: "application/json",
                body: serde_json::json!({
                    "ok": false,
                    "id": id,
                    "status": "failed",
                    "error": {
                        "code": "engine",
                        "hint": format!("approved, but submission failed: {error}"),
                    },
                })
                .to_string(),
            }
        }
        Err(e) => send_error_response(e),
    }
}

// ── POST /cli/mail/approvals/deny ───────────────────────────────────────

/// S5: owner-or-admin deny with a REQUIRED note that flows back to the
/// agent's outbox (§8.4/§11: `deny <id> --note …`). Body:
/// `{id, note, decidedBy?}`.
pub fn handle_approvals_deny(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(id) = v["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — an outbound id from the approvals list (out_…)",
        );
    };
    let Some(note) = v["note"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'note' — the reason flows back to the agent (required on deny)",
        );
    };
    let decided_by = v["decidedBy"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("owner");
    let now = now_secs();
    send::auto_deny_expired(now);
    let store = DbOutboundStore::default();
    match send::deny(&store, id, decided_by, note, now) {
        Ok(()) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendDecided,
                serde_json::json!({ "outboundId": id, "status": "rejected" }),
            );
            ok_json(serde_json::json!({
                "ok": true,
                "id": id,
                "status": "rejected",
            }))
        }
        Err(e) => send_error_response(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — validation + gating + masking through the real
// handlers against the shared test DB; NO network, NO real sends
// (approval-mode round trips never dial an engine; on-mode tests stop
// at the no-server 503). Deep pipeline behavior lives in mail::send.
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::send::NewOutbound;

    fn body_json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    // ── attachment reading (workspace-relative, caps) ──

    #[test]
    fn read_workspace_attachments_reads_files_and_derives_name_and_type() {
        let root = std::env::temp_dir().join(format!(
            "mail-att-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("report.pdf"), b"PDF-CONTENT").unwrap();
        std::fs::write(root.join("out/notes.txt"), b"hi").unwrap();
        let root_s = root.to_string_lossy().to_string();

        let atts = match read_workspace_attachments(
            &root_s,
            &["report.pdf".to_string(), "out/notes.txt".to_string()],
        ) {
            Ok(a) => a,
            Err(resp) => panic!("expected both files to read: {}", resp.body),
        };
        assert_eq!(atts.len(), 2);
        assert_eq!(atts[0].filename, "report.pdf");
        assert_eq!(atts[0].content_type, "application/pdf");
        assert_eq!(atts[0].bytes, b"PDF-CONTENT");
        assert_eq!(atts[1].filename, "notes.txt");
        assert!(atts[1].content_type.starts_with("text/plain"));

        // A missing file → 404 not_found naming the path, nothing read.
        let resp = read_workspace_attachments(&root_s, &["ghost.bin".to_string()])
            .err()
            .expect("missing file must be rejected");
        assert_eq!(resp.status, "404 Not Found");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("ghost.bin"));

        // A traversal path → 400 usage.
        let resp = read_workspace_attachments(&root_s, &["../escape.bin".to_string()])
            .err()
            .expect("traversal must be rejected");
        assert_eq!(resp.status, "400 Bad Request");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_workspace_attachments_rejects_oversize_files() {
        let root = std::env::temp_dir().join(format!(
            "mail-att-big-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        // One byte over the per-file cap — rejected via metadata, no read.
        let big = vec![0u8; (MAX_ATTACHMENT_BYTES + 1) as usize];
        std::fs::write(root.join("huge.bin"), &big).unwrap();
        let root_s = root.to_string_lossy().to_string();

        let resp = read_workspace_attachments(&root_s, &["huge.bin".to_string()])
            .err()
            .expect("oversize must be rejected");
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("too large"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Short names double as DOMAIN labels (`bot@<name>.example`).
    fn unique(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let short = &id[..12];
        (
            format!("msr-{label}-{short}"),
            format!("/tmp/mail-send-routes-{label}-{}-{short}", std::process::id()),
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

    fn set_gating(path: &str, mode: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE projects SET mail_agent_send = ?2 WHERE path = ?1",
            rusqlite::params![path, mode],
        )
        .expect("set gating mode");
    }

    fn seed_address(project_id: &str, address: &str, account: Option<&str>) {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100)",
            rusqlite::params![id, address, account, project_id],
        )
        .expect("seed address");
    }

    fn seed_domain(domain: &str, send_mode: &str) {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_domains (id, domain, send_mode, status, created_at) \
             VALUES (?1, ?2, ?3, 'verified', 100)",
            rusqlite::params![id, domain, send_mode],
        )
        .expect("seed domain");
    }

    fn cleanup(project_id: &str, domain: Option<&str>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_outbound WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
        if let Some(d) = domain {
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", rusqlite::params![d]);
        }
    }

    fn outbound_count(project_id: &str) -> i64 {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM mail_outbound WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )
        .unwrap_or(-1)
    }

    fn send_body(path: &str, extra: serde_json::Value) -> Vec<u8> {
        let mut v = serde_json::json!({
            "project": path,
            "to": "human@example.com",
            "subject": "Digest",
            "body": "All green.",
        });
        if let (Some(base), Some(add)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in add {
                base.insert(k.clone(), val.clone());
            }
        }
        serde_json::to_vec(&v).expect("body")
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// The no-server tests must not race a seeded `mail_server` row.
    fn clear_mail_server() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
    }

    // ── send validation ──

    #[test]
    fn send_validates_body_shape_before_any_identity_work() {
        let resp = handle_send(b"not json");
        assert_eq!(resp.status, "400 Bad Request");

        for (bad, needle) in [
            (serde_json::json!({}), "project"),
            (serde_json::json!({ "project": "/tmp/x" }), "to"),
            (serde_json::json!({ "project": "/tmp/x", "to": "a@b.c" }), "subject"),
            (
                serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s" }),
                "body",
            ),
            (
                serde_json::json!({ "project": "/tmp/x", "to": 42,
                                    "subject": "s", "body": "b" }),
                "to",
            ),
            (
                // `attachments` must be an array of path strings (shape
                // validated before any identity work).
                serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s",
                                    "body": "b", "attachments": "f.pdf" }),
                "must be an array",
            ),
            (
                // The count cap (≤10) fires before identity work too.
                serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s",
                                    "body": "b",
                                    "attachments": (0..11).map(|i| format!("f{i}.pdf"))
                                        .collect::<Vec<_>>() }),
                "too many attachments",
            ),
            (
                serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s",
                                    "body": "b", "wait": true, "timeout": 901 }),
                "timeout",
            ),
        ] {
            let resp = handle_send(&serde_json::to_vec(&bad).unwrap());
            assert_eq!(resp.status, "400 Bad Request", "{bad}");
            let v = body_json(&resp);
            assert!(
                v["error"]["hint"].as_str().unwrap().contains(needle),
                "{bad} → {}",
                resp.body
            );
        }
    }

    /// D4 DEFAULT: send is OFF — the teaching 403 fires and NO audit
    /// row exists (the gate precedes everything).
    #[test]
    fn send_defaults_gated_off_with_teaching_error_and_no_row() {
        let (name, path) = unique("off");
        let project_id = insert_project(&name, &path);
        seed_address(&project_id, &format!("bot@{name}.example"), Some("acc-1"));
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "gated");
        assert!(
            v["error"]["hint"].as_str().unwrap().contains("Settings → Email → Sending"),
            "{v}"
        );
        assert_eq!(outbound_count(&project_id), 0, "off mode never writes");
        cleanup(&project_id, None);
    }

    /// The full approval round trip through the REAL handlers: queue →
    /// outbox pending → approvals list preview → deny-with-note →
    /// note reaches the agent's outbox → re-decide conflicts.
    #[test]
    fn approval_round_trip_queue_outbox_deny_note() {
        let (name, path) = unique("appr");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        let from = format!("bot@{domain}");
        seed_domain(&domain, "direct");
        seed_address(&project_id, &from, Some("acc-1"));
        set_gating(&path, "approval");

        // Queue (no engine dial happens in approval mode — no
        // mail_server row exists and this still succeeds).
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["ok"], true);
        assert_eq!(v["queued"], true);
        assert_eq!(v["status"], "pending_approval");
        let id = v["id"].as_str().expect("id").to_string();
        assert!(id.starts_with("out_"), "{id}");
        assert!(
            v["hint"].as_str().unwrap().contains(&format!("k2 mail outbox {id}")),
            "the track-with-outbox hint names the id: {v}"
        );

        // Outbox: the caller sees it pending, with the status note.
        let resp = handle_outbox(&params(&[("project", &path)]));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["count"], 1);
        assert_eq!(v["outbox"][0]["id"], id.as_str());
        assert_eq!(v["outbox"][0]["status"], "pending_approval");
        assert_eq!(v["outbox"][0]["from"], from.as_str());

        // Point lookup (§11.1.9).
        let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
        assert_eq!(resp.status, "200 OK");
        assert_eq!(body_json(&resp)["outbound"]["subject"], "Digest");

        // The owner's queue carries the preview fields.
        let resp = handle_approvals_list(&HashMap::new());
        assert_eq!(resp.status, "200 OK");
        let v = body_json(&resp);
        let ours = v["pending"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id.as_str())
            .expect("our item queued");
        assert_eq!(ours["bodyPreview"], "All green.");
        assert_eq!(ours["workspace"], name.as_str());
        assert_eq!(ours["to"][0], "human@example.com");

        // Deny requires a note; the note reaches the agent.
        let resp = handle_approvals_deny(
            format!(r#"{{"id":"{id}"}}"#).as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "note is required: {}", resp.body);
        let resp = handle_approvals_deny(
            format!(r#"{{"id":"{id}","note":"use the newsletter address"}}"#).as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(body_json(&resp)["status"], "rejected");
        let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
        let v = body_json(&resp);
        assert_eq!(v["outbound"]["status"], "rejected");
        assert_eq!(v["outbound"]["note"], "use the newsletter address");
        assert_eq!(v["outbound"]["decidedBy"], "owner");

        // Re-deciding a decided row conflicts — and never submits.
        let resp = handle_approvals_deny(
            format!(r#"{{"id":"{id}","note":"again"}}"#).as_bytes(),
        );
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        let resp = handle_approvals_approve(format!(r#"{{"id":"{id}"}}"#).as_bytes());
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("rejected"));

        cleanup(&project_id, Some(&domain));
    }

    /// #31.5: a SUCCESSFUL linked SMTP send records an outbox row (no
    /// network — we call the recorder the linked success path calls, then
    /// read it back through the real outbox handler). The row shows as
    /// `submitted` (never "delivered") with `sent_at` stamped.
    #[test]
    fn linked_send_records_a_submitted_outbox_row() {
        let (name, path) = unique("linked");
        let project_id = insert_project(&name, &path);
        assert_eq!(outbound_count(&project_id), 0);

        let store = DbOutboundStore::default();
        let msg = OutboundMessage {
            from_name: Some("Agent".to_string()),
            from: "me@gmail.com".to_string(),
            to: vec!["pat@dest.example".to_string()],
            cc: Vec::new(),
            subject: "Hi via linked".to_string(),
            text_body: "the linked body".to_string(),
            in_reply_to: None,
            references: None,
        };
        let id = send::record_linked_submitted(&store, &project_id, "Agent", &msg, &[], now_secs())
            .expect("record linked send");
        assert!(id.starts_with("out_"), "{id}");
        assert_eq!(outbound_count(&project_id), 1, "one audit row after a linked send");

        // The agent sees it via `k2 mail outbox <id>` — submitted, to the
        // recipient, with a stamped sent_at, never "delivered".
        let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["outbound"]["status"], "submitted");
        assert_ne!(v["outbound"]["status"], "delivered");
        assert_eq!(v["outbound"]["to"][0], "pat@dest.example");
        assert_eq!(v["outbound"]["subject"], "Hi via linked");
        assert!(
            v["outbound"]["sentAt"].as_i64().is_some(),
            "sent_at is stamped: {}",
            resp.body
        );
        // A no-attachment send surfaces NO attachments summary.
        assert!(
            v["outbound"].get("attachments").is_none(),
            "no attachments key for a plain send: {}",
            resp.body
        );

        cleanup(&project_id, None);
    }

    /// #31.5 follow-up: a linked send WITH attachments records the
    /// filenames in the outbox trail (`attachments_ref` = the JSON of the
    /// basenames), and the outbox read surfaces `{count, names}`. Never
    /// bytes, never paths — filenames only.
    #[test]
    fn linked_send_records_attachment_filenames_in_the_outbox_trail() {
        let (name, path) = unique("linkatt");
        let project_id = insert_project(&name, &path);

        let store = DbOutboundStore::default();
        let msg = OutboundMessage {
            from_name: Some("Agent".to_string()),
            from: "me@gmail.com".to_string(),
            to: vec!["pat@dest.example".to_string()],
            cc: Vec::new(),
            subject: "Here are the files".to_string(),
            text_body: "see attached".to_string(),
            in_reply_to: None,
            references: None,
        };
        let names = vec!["invoice.pdf".to_string(), "photo.jpg".to_string()];
        let id = send::record_linked_submitted(
            &store, &project_id, "Agent", &msg, &names, now_secs(),
        )
        .expect("record linked send with attachments");

        // The stored row's attachments_ref is EXACTLY the two basenames —
        // no bytes, no paths.
        let stored: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT attachments_ref FROM mail_outbound WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .expect("attachments_ref present")
        };
        assert_eq!(stored, r#"["invoice.pdf","photo.jpg"]"#, "basenames only: {stored}");

        // The outbox read surfaces the {count, names} summary.
        let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["outbound"]["attachments"]["count"], 2);
        assert_eq!(v["outbound"]["attachments"]["names"][0], "invoice.pdf");
        assert_eq!(v["outbound"]["attachments"]["names"][1], "photo.jpg");

        cleanup(&project_id, None);
    }

    /// Approving with NO mail server: the row flips approved → failed
    /// (loud, audited) and nothing dials out — never silently
    /// approved-forever, never a phantom send.
    #[test]
    fn approve_without_engine_marks_failed_loudly() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        let (name, path) = unique("apfail");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        let from = format!("bot@{domain}");
        seed_domain(&domain, "direct");
        seed_address(&project_id, &from, Some("acc-1"));
        set_gating(&path, "approval");

        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let id = body_json(&resp)["id"].as_str().unwrap().to_string();

        let resp = handle_approvals_approve(format!(r#"{{"id":"{id}"}}"#).as_bytes());
        assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["status"], "failed");
        assert!(v["error"]["hint"].as_str().unwrap().contains("submission failed"));
        let resp = handle_outbox(&params(&[("project", &path), ("id", &id)]));
        let v = body_json(&resp);
        assert_eq!(v["outbound"]["status"], "failed");
        assert!(
            v["outbound"]["note"].as_str().unwrap().contains("mail server unavailable"),
            "{v}"
        );

        // Unknown id → masked not_found.
        let resp = handle_approvals_approve(br#"{"id":"out_nope00000001"}"#);
        assert_eq!(resp.status, "404 Not Found");

        cleanup(&project_id, Some(&domain));
    }

    /// On-mode with no mail server: 503 BEFORE any audit row (nothing
    /// submitted, nothing phantom-recorded).
    #[test]
    fn on_mode_without_engine_answers_503_and_writes_nothing() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        let (name, path) = unique("onmode");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        seed_domain(&domain, "direct");
        seed_address(&project_id, &format!("bot@{domain}"), Some("acc-1"));
        set_gating(&path, "on");
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "not_ready");
        assert_eq!(outbound_count(&project_id), 0);
        cleanup(&project_id, Some(&domain));
    }

    /// D1: a receive-only domain refuses in EVERY gating mode, with the
    /// teaching error, before any row.
    #[test]
    fn receive_only_domain_refuses_sends() {
        let (name, path) = unique("ro");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        seed_domain(&domain, "receive-only");
        seed_address(&project_id, &format!("bot@{domain}"), Some("acc-1"));
        set_gating(&path, "approval");
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "send_mode");
        assert!(v["error"]["hint"].as_str().unwrap().contains("receive-only"), "{v}");
        assert_eq!(outbound_count(&project_id), 0);
        cleanup(&project_id, Some(&domain));
    }

    /// Sender identity is server-stamped: a foreign `from` answers the
    /// S3 masked not_found; no addresses is the teaching not_found;
    /// multiple addresses demand an explicit `from`.
    #[test]
    fn from_resolution_masks_foreign_and_demands_explicit_on_ambiguity() {
        let (name, path) = unique("from");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        seed_domain(&domain, "direct");
        set_gating(&path, "approval");

        // No addresses yet → teaching not_found.
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("k2 mail create"));

        // A FOREIGN from → masked.
        let (name2, path2) = unique("from-f");
        let project2 = insert_project(&name2, &path2);
        let foreign = format!("theirs@{name2}.example");
        seed_address(&project2, &foreign, Some("acc-f"));
        let resp = handle_send(&send_body(&path, serde_json::json!({ "from": foreign })));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);

        // Two addresses without `from` → usage naming the fix.
        seed_address(&project_id, &format!("a@{domain}"), Some("acc-1"));
        seed_address(&project_id, &format!("b@{domain}"), Some("acc-2"));
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("from"));

        // Recipient cap: 11 recipients refuse.
        let many: Vec<String> = (0..11).map(|i| format!("r{i}@example.com")).collect();
        let resp = handle_send(&send_body(
            &path,
            serde_json::json!({ "from": format!("a@{domain}"), "to": many }),
        ));
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("max 10"));

        cleanup(&project_id, Some(&domain));
        cleanup(&project2, None);
    }

    /// The always-on hourly cap through the real store: the 21st send
    /// inside an hour answers 429 with the distinct window marker.
    #[test]
    fn hourly_rate_limit_enforced_through_the_route() {
        let (name, path) = unique("rate");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        let from = format!("bot@{domain}");
        seed_domain(&domain, "direct");
        seed_address(&project_id, &from, Some("acc-1"));
        set_gating(&path, "approval");

        // 20 audit rows in the current hour, straight through the
        // production store.
        let store = DbOutboundStore::default();
        let msg = OutboundMessage {
            from_name: None,
            from: from.clone(),
            to: vec!["x@example.com".to_string()],
            cc: vec![],
            subject: "s".to_string(),
            text_body: "b".to_string(),
            in_reply_to: None,
            references: None,
        };
        let now = now_secs();
        for _ in 0..send::RATE_LIMIT_HOURLY {
            store
                .insert(&NewOutbound {
                    owner_project_id: &project_id,
                    agent_name: "bot",
                    message: &msg,
                    status: "pending",
                    decided_by: None,
                    attachment_names: &[],
                    now,
                })
                .expect("seed audit row");
        }
        let resp = handle_send(&send_body(&path, serde_json::json!({})));
        assert_eq!(resp.status, "429 Too Many Requests", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "rate_limited");
        assert_eq!(v["error"]["window"], "hour");
        cleanup(&project_id, Some(&domain));
    }

    /// `--wait` claims a slot BEFORE queueing: with the workspace's
    /// slots held, nothing is queued and the caller gets the 429.
    #[test]
    fn send_wait_refuses_without_a_slot_and_queues_nothing() {
        let (name, path) = unique("waitslot");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        seed_domain(&domain, "direct");
        seed_address(&project_id, &format!("bot@{domain}"), Some("acc-1"));
        set_gating(&path, "approval");
        let _held: Vec<_> = (0..4)
            .map(|_| WaitSlot::try_acquire(&project_id).expect("slot"))
            .collect();
        let resp = handle_send(&send_body(&path, serde_json::json!({ "wait": true })));
        assert_eq!(resp.status, "429 Too Many Requests", "{}", resp.body);
        assert_eq!(outbound_count(&project_id), 0, "no phantom queue item");
        cleanup(&project_id, Some(&domain));
    }

    // ── reply ──

    #[test]
    fn reply_gates_validates_and_masks_before_any_engine_dial() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        let resp = handle_reply(b"{}");
        assert_eq!(resp.status, "400 Bad Request");

        let (name, path) = unique("reply");
        let project_id = insert_project(&name, &path);
        let domain = format!("{name}.example");
        seed_domain(&domain, "direct");
        let mine = format!("bot@{domain}");
        seed_address(&project_id, &mine, Some("acc-1"));

        let token = messages::encode_message_id(&mine, "M1");
        // Default gating (off) refuses the reply FIRST — even though no
        // mail server exists (no engine dial before the gate).
        let resp = handle_reply(
            serde_json::to_vec(&serde_json::json!({
                "project": path, "id": token, "body": "thanks!",
            }))
            .unwrap()
            .as_slice(),
        );
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "gated");

        set_gating(&path, "approval");
        // Malformed id → usage.
        let resp = handle_reply(
            serde_json::to_vec(&serde_json::json!({
                "project": path, "id": "garbage", "body": "x",
            }))
            .unwrap()
            .as_slice(),
        );
        assert_eq!(resp.status, "400 Bad Request");

        // Foreign id → masked (ownership precedes the engine).
        let (name2, path2) = unique("reply-f");
        let project2 = insert_project(&name2, &path2);
        let foreign = format!("theirs@{name2}.example");
        seed_address(&project2, &foreign, Some("acc-f"));
        let ftoken = messages::encode_message_id(&foreign, "M1");
        let resp = handle_reply(
            serde_json::to_vec(&serde_json::json!({
                "project": path, "id": ftoken, "body": "x",
            }))
            .unwrap()
            .as_slice(),
        );
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(
            !body_json(&resp)["error"]["hint"].as_str().unwrap().contains(&foreign),
            "masked hint must not leak the address"
        );

        // Owned id + no mail server → 503 (the reply context needs the
        // engine); nothing queued.
        let resp = handle_reply(
            serde_json::to_vec(&serde_json::json!({
                "project": path, "id": token, "body": "x",
            }))
            .unwrap()
            .as_slice(),
        );
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        assert_eq!(outbound_count(&project_id), 0);

        cleanup(&project_id, Some(&domain));
        cleanup(&project2, None);
    }

    // ── outbox reads ──

    #[test]
    fn outbox_validates_and_masks_point_lookups() {
        let resp = handle_outbox(&HashMap::new());
        assert_eq!(resp.status, "400 Bad Request");

        let (name, path) = unique("outbox");
        let project_id = insert_project(&name, &path);
        for bad in ["0", "abc"] {
            let resp = handle_outbox(&params(&[("project", &path), ("limit", bad)]));
            assert_eq!(resp.status, "400 Bad Request", "limit={bad}");
        }
        // Empty outbox is a 200, not an error.
        let resp = handle_outbox(&params(&[("project", &path)]));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(body_json(&resp)["count"], 0);

        // Unknown id and FOREIGN id answer the same masked not_found.
        let resp = handle_outbox(&params(&[("project", &path), ("id", "out_nope")]));
        assert_eq!(resp.status, "404 Not Found");
        let (name2, path2) = unique("outbox-f");
        let project2 = insert_project(&name2, &path2);
        let store = DbOutboundStore::default();
        let msg = OutboundMessage {
            from_name: None,
            from: format!("theirs@{name2}.example"),
            to: vec!["x@example.com".to_string()],
            cc: vec![],
            subject: "their secret subject".to_string(),
            text_body: "b".to_string(),
            in_reply_to: None,
            references: None,
        };
        let foreign_id = store
            .insert(&NewOutbound {
                owner_project_id: &project2,
                agent_name: "bot",
                message: &msg,
                status: "pending",
                decided_by: None,
                attachment_names: &[],
                now: now_secs(),
            })
            .expect("seed foreign row");
        let resp = handle_outbox(&params(&[("project", &path), ("id", &foreign_id)]));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(
            !resp.body.contains("secret subject"),
            "masked lookups never leak foreign content"
        );

        cleanup(&project_id, None);
        cleanup(&project2, None);
    }

    // ── LINKED send (§17.5) — routing + ungated + masking ──

    /// Seed a linked inbox bound to `project_id` at `primary_level`.
    fn seed_linked(project_id: &str, address: &str, primary_level: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, primary_level) \
             VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100, ?4)",
            rusqlite::params![id, project_id, address, primary_level],
        )
        .expect("seed linked inbox");
        id
    }

    fn cleanup_linked(project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_inbox_grants WHERE inbox_id IN \
             (SELECT id FROM mail_external_inboxes WHERE owner_project_id = ?1)",
            rusqlite::params![project_id],
        );
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
            rusqlite::params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id]);
    }

    /// §17.5: a linked inbox at 'send' is UNGATED — gating 'off' does NOT
    /// block it (the hosted-only D4 gate never runs on the linked path).
    /// With no vaulted credential the send stops at 503 not_ready, having
    /// already proven the linked branch was taken and the gate skipped.
    #[test]
    fn linked_send_is_ungated_and_reaches_the_smtp_stage() {
        let (name, path) = unique("lsend");
        let project_id = insert_project(&name, &path);
        let linked = format!("me-{}@linked.example", &project_id[..8]);
        seed_linked(&project_id, &linked, "send");
        // Gating OFF — a hosted send would 403 here; linked must not.
        set_gating(&path, "off");
        let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked })));
        assert_ne!(resp.status, "403 Forbidden", "linked send must not be gated: {}", resp.body);
        // No vaulted credential → 503 not_ready (the SMTP stage was
        // reached, gating skipped). Mirrors the S9 draft test pattern.
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "not_ready");
        cleanup_linked(&project_id);
    }

    /// A linked inbox the workspace can only READ/DRAFT (default 'draft')
    /// can't send — the teaching 403 points at raising the level AND at
    /// 'k2 mail draft' (the linked-specific alternative).
    #[test]
    fn linked_send_without_the_send_level_teaches_draft_alternative() {
        let (name, path) = unique("ldraft");
        let project_id = insert_project(&name, &path);
        let linked = format!("me-{}@linked.example", &project_id[..8]);
        seed_linked(&project_id, &linked, "draft"); // default: can't send
        set_gating(&path, "on");
        let resp = handle_send(&send_body(&path, serde_json::json!({ "from": linked })));
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "gated");
        let hint = v["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("read/draft-only"), "{hint}");
        assert!(hint.contains("k2 mail draft"), "linked teaches the draft alternative: {hint}");
        cleanup_linked(&project_id);
    }

    /// Seed a linked inbox whose credential is an OAuth token (Gmail
    /// XOAUTH2) — `auth_kind='oauth'`, `provider='gmail'` — matching the
    /// shape `external::add_oauth_inbox` writes. No app-password is ever
    /// vaulted for such a row (the credential is the token blob).
    fn seed_linked_oauth(project_id: &str, address: &str, primary_level: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, primary_level, auth_kind, provider) \
             VALUES (?1, ?2, ?3, 'imap.gmail.com', 993, ?3, 100, ?4, 'oauth', 'gmail')",
            rusqlite::params![id, project_id, address, primary_level],
        )
        .expect("seed oauth linked inbox");
        id
    }

    /// REGRESSION (the seam the original tests missed): an OAuth-linked
    /// inbox has its token vaulted under the `-oauth` key and NO
    /// app-password. `linked_password` MUST NOT 503 on the absent
    /// app-password — it recognises `auth_kind='oauth'` and returns an
    /// empty password (the SMTP/IMAP backend mints the token itself).
    #[test]
    fn linked_password_skips_app_password_resolve_for_oauth_rows() {
        let (name, path) = unique("oauthpw");
        let project_id = insert_project(&name, &path);
        let linked = format!("me-{}@gmail.com", &project_id[..8]);
        let id = seed_linked_oauth(&project_id, &linked, "send");

        // Present: the OAuth token bundle (the real credential). Absent:
        // any app-password under `vault_key(id)`.
        let secrets = FileSecretStore::default();
        let tokens = crate::mail::oauth::Tokens {
            access_token: "ya29.test-access".to_string(),
            refresh_token: Some("1//test-refresh".to_string()),
            scope: Some("https://mail.google.com/".to_string()),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
        };
        crate::mail::oauth::store_tokens(&secrets, &id, &tokens, 1_000)
            .expect("vault oauth tokens");

        let inbox = external::inbox_for_address(&linked).expect("row loads");
        // No app-password vaulted → a `password` row would 503 here; the
        // oauth row must instead PROCEED with an empty password.
        match linked_password(&inbox) {
            Ok(pw) => assert!(pw.is_empty(), "oauth row yields empty password, got {} bytes", pw.len()),
            Err(resp) => panic!(
                "oauth row wrongly 503'd on a missing app-password: {} {}",
                resp.status, resp.body
            ),
        }

        // Clean up the vaulted token (shared secrets file).
        let _ = secrets.delete(&crate::mail::oauth::oauth_vault_key(&id));
        cleanup_linked(&project_id);
    }

    /// Counterpart: a NON-oauth (app-password) row with NO vaulted
    /// credential still 503s not_ready — the fix narrows the skip to
    /// oauth rows only, it does not silence the real missing-credential
    /// case.
    #[test]
    fn linked_password_still_503s_for_password_rows_missing_the_vault_entry() {
        let (name, path) = unique("pwmiss");
        let project_id = insert_project(&name, &path);
        let linked = format!("me-{}@linked.example", &project_id[..8]);
        seed_linked(&project_id, &linked, "send"); // default auth_kind (not oauth)

        let inbox = external::inbox_for_address(&linked).expect("row loads");
        match linked_password(&inbox) {
            Ok(pw) => panic!("password row with no vault entry must 503, got Ok({pw:?})"),
            Err(resp) => {
                assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
                assert_eq!(body_json(&resp)["error"]["code"], "not_ready");
                let hint = body_json(&resp)["error"]["hint"].as_str().unwrap().to_string();
                assert!(hint.contains("k2 mail link add"), "corrected verb in hint: {hint}");
            }
        }
        cleanup_linked(&project_id);
    }
}
