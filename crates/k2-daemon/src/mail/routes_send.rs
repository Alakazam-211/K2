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
use crate::mail::external;
use crate::mail::external_smtp;
use crate::mail::messages::{self, ReadError};
use crate::mail::routes_messages::WaitSlot;
use crate::mail::secrets::{FileSecretStore, SecretStore as _};
use crate::mail::send::{
    self, DbOutboundStore, Gate, OutboundMessage, OutboundRequest, OutboundStore, SendError,
    SendOutcome, SubmitBackend,
};
use k2_core::db::schema::MailExternalInbox;

// ── Response helpers (the S2/S3/S4 error contract + the S5 codes) ───────

pub(crate) fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
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
            "this workspace has no address it can send FROM — mint one with 'k2 hostmail create', \
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
pub(crate) fn string_list(v: &serde_json::Value, field: &str) -> Result<Vec<String>, CliResponse> {
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
    send_after: Option<i64>,
) -> CliResponse {
    let store = DbOutboundStore::default();
    let now = now_secs();
    let future_schedule = send_after.filter(|t| *t > now);
    let engine_client;
    let backend: Option<&dyn SubmitBackend> = match (gate, future_schedule) {
        (Gate::On, None) => match domains::engine_from_db() {
            Ok((client, _)) => {
                engine_client = client;
                Some(&engine_client)
            }
            Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
        },
        _ => None,
    };
    if wait_timeout.is_some() && future_schedule.is_some() {
        return error_response(
            "400 Bad Request",
            "usage",
            "--wait cannot combine with --at/--in — schedule the send, then poll \
             'k2 mail outbox <id>' (or cancel with 'k2 mail outbox cancel <id>')",
        );
    }
    let _slot = match wait_slot(project_id, gate, wait_timeout) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let req = OutboundRequest {
        project_id,
        agent_name,
        account_id,
        message: msg,
        send_after,
    };
    let outcome = match send::gate_and_dispatch(&store, backend, gate, &req, now) {
        Ok(o) => o,
        Err(e) => return send_error_response(e),
    };
    respond_outcome(outcome, &store, project_path, agent_name, wait_timeout)
}

/// The `--wait` slot for an approval-mode send (bounded per workspace).
fn wait_slot(
    project_id: &str,
    gate: Gate,
    wait_timeout: Option<u64>,
) -> Result<Option<WaitSlot>, CliResponse> {
    if wait_timeout.is_none() || gate != Gate::Approval {
        return Ok(None);
    }
    match WaitSlot::try_acquire(project_id) {
        Some(s) => Ok(Some(s)),
        None => Err(CliResponse {
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
        }),
    }
}

/// Shape the gate's outcome (shared by hosted and linked sends): queued
/// (optionally held until decided), scheduled, submitted or failed.
fn respond_outcome(
    outcome: SendOutcome,
    store: &DbOutboundStore,
    project_path: &str,
    agent_name: &str,
    wait_timeout: Option<u64>,
) -> CliResponse {
    match outcome {
        SendOutcome::Scheduled { id, send_after } => {
            let when = crate::mail::schedule::format_send_after(send_after);
            ok_json(serde_json::json!({
                "ok": true,
                "queued": true,
                "scheduled": true,
                "id": id,
                "status": "scheduled",
                "sendAfter": send_after,
                "sendAfterRfc3339": when,
                "hint": format!(
                    "scheduled ({id}) — will leave after {when}; track with \
                     'k2 mail outbox {id}', cancel with 'k2 mail outbox cancel {id}'"
                ),
            }))
        }
        SendOutcome::Queued { id, send_after } => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::MailSendApprovalRequested,
                serde_json::json!({
                    "outboundId": id,
                    "projectPath": project_path,
                    "agentName": agent_name,
                }),
            );
            let Some(timeout_secs) = wait_timeout else {
                let mut body = serde_json::json!({
                    "ok": true,
                    "queued": true,
                    "id": id,
                    "status": "pending_approval",
                    "hint": format!(
                        "queued for approval ({id}) — your human decides in Settings → \
                         Email → Approvals; track with 'k2 mail outbox {id}'"
                    ),
                });
                if let Some(after) = send_after {
                    body["sendAfter"] = serde_json::json!(after);
                    body["sendAfterRfc3339"] =
                        serde_json::json!(crate::mail::schedule::format_send_after(after));
                    body["hint"] = serde_json::json!(format!(
                        "queued for approval ({id}) — after approve, sends after {}; \
                         track with 'k2 mail outbox {id}'",
                        crate::mail::schedule::format_send_after(after)
                    ));
                }
                return ok_json(body);
            };
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

// ── LINKED send (§17.5, GATED — same mail_agent_send as hosted) ─────────

/// The workspace `mail_agent_send` gate for a LINKED send (E4 / hole #3):
/// off → 403 `gated`; approval → queue (MCS2's linked approval hand-off);
/// on → send now. Fail-closed on an unreadable/unrecognized mode.
fn linked_send_gate(project_path: &str) -> Result<Gate, CliResponse> {
    let gate = send::gate_for_path(project_path).map_err(send_error_response)?;
    send::allow_linked_send(gate).map_err(send_error_response)?;
    Ok(gate)
}

/// A submission error → the shared {code, hint} shapes.
fn submit_error_response(err: crate::mail::submit::SubmitError) -> CliResponse {
    use crate::mail::submit::SubmitError;
    match err {
        SubmitError::Usage { code, hint } => error_response("400 Bad Request", code, &hint),
        SubmitError::NotFound(h) => error_response("404 Not Found", "not_found", &h),
        SubmitError::Engine(h) => error_response("502 Bad Gateway", "engine", &h),
    }
}

/// The send backend for a linked inbox (SMTP or Microsoft Graph). A
/// missing vaulted credential is 503 `not_ready` (never a leaked secret).
fn linked_submit(
    inbox: &MailExternalInbox,
) -> Result<Box<dyn crate::mail::submit::MailSubmit>, CliResponse> {
    crate::mail::submit::submit_for_linked(inbox)
        .map_err(|h| error_response("503 Service Unavailable", "not_ready", &h))
}

/// Microsoft 365 takes attachments inline only up to 3 MB each / 4 MB a
/// request in this K2 version: refuse BEFORE anything is audited, queued
/// or sent (400 `attachment_too_large`).
fn graph_attachment_precheck(
    inbox: &MailExternalInbox,
    attachments: &[external_smtp::OutAttachment],
    text_len: usize,
) -> Result<(), CliResponse> {
    if inbox.kind != "graph" {
        return Ok(());
    }
    let atts: Vec<crate::mail::graph::GraphAttachment> = attachments
        .iter()
        .map(|a| crate::mail::graph::GraphAttachment {
            filename: &a.filename,
            content_type: &a.content_type,
            bytes: &a.bytes,
        })
        .collect();
    crate::mail::graph::check_inline_attachments(&atts, text_len)
        .map_err(|e| submit_error_response(e.into()))
}

/// MCS2: a LINKED send or reply through the governed pipeline — rate
/// limits + audit row first, then approval queue or the provider (SMTP or
/// Graph) — with the same answer shapes as before for a direct send.
#[allow(clippy::too_many_arguments)]
fn dispatch_linked(
    project_id: &str,
    project_path: &str,
    agent_name: &str,
    inbox: &MailExternalInbox,
    msg: &OutboundMessage,
    submit: &dyn crate::mail::submit::MailSubmit,
    gate: Gate,
    wait_timeout: Option<u64>,
    attachments: &[external_smtp::OutAttachment],
) -> CliResponse {
    let store = DbOutboundStore::default();
    let _slot = match wait_slot(project_id, gate, wait_timeout) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let req = OutboundRequest { project_id, agent_name, account_id: &inbox.id, message: msg, send_after: None };
    let (outcome, receipt) =
        match send::gate_and_dispatch_linked(&store, submit, gate, &req, attachments, now_secs()) {
            Ok(o) => o,
            Err(e) => return send_error_response(e),
        };
    let via = if submit.backend() == "graph" { "Microsoft Graph" } else { "SMTP" };
    match (outcome, receipt) {
        (SendOutcome::Submitted { id }, Some(r)) => {
            let mut v = serde_json::json!({
                "ok": true,
                "id": id,
                "status": "submitted",
                "from": inbox.email_address,
                "via": submit.backend(),
                "hint": format!(
                    "submitted from {} via {via} — accepted by the provider (K2 doesn't confirm \
                     final delivery); see it in 'k2 mail outbox {id}'",
                    inbox.email_address
                ),
            });
            if msg.reply_source.is_some() {
                v["to"] = serde_json::json!(r.to.first().cloned().unwrap_or_default());
            }
            ok_json(v)
        }
        (outcome, _) => respond_outcome(outcome, &store, project_path, agent_name, wait_timeout),
    }
}

/// Resolve a linked inbox's vaulted app-password (same key as IMAP).
/// Missing/unreadable → 503 not_ready with the reconnect pointer (never
/// a leaked secret).
pub(crate) fn linked_password(inbox: &MailExternalInbox) -> Result<String, CliResponse> {
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

// ── Attachments (§17.5 send/reply): daemon reads workspace-relative ─────

/// Max attachments per message, and the per-file / total size caps
/// (enforced daemon-side after resolving each path; the CLI mirrors the
/// count/path shape but does NO file I/O — the daemon owns the bytes).
/// Shared with `k2 mail draft` (same numbers; extract, don't copy).
pub(crate) const MAX_ATTACHMENTS: usize = 10;
pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
pub(crate) const MAX_ATTACHMENTS_TOTAL_BYTES: u64 = 25 * 1024 * 1024;

/// Parse the `attachments` array (workspace-relative path STRINGS — the
/// CLI sends paths, never bytes). Enforces the count cap and the
/// non-empty-string shape BEFORE any file is touched. Absent/null → none.
pub(crate) fn parse_attachment_specs(v: &serde_json::Value) -> Result<Vec<String>, CliResponse> {
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

/// A filename that is safe to show an approver and to put in a message: the
/// last path segment, with control and invisible formatting characters (line
/// breaks, bidi overrides that make `gpj.exe` read as `exe.jpg`) replaced, at
/// most 200 characters. Workspace files and a forwarded message's attachment
/// names both come through here. Never empty.
pub(crate) fn safe_attachment_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = last
        .chars()
        .map(|c| if c.is_control() || send::is_spoofy_format_char(c) { '_' } else { c })
        .take(200)
        .collect();
    let clean = clean.trim();
    if clean.is_empty() || clean == "." || clean == ".." {
        "attachment".to_string()
    } else {
        clean.to_string()
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// The one size refusal (workspace files and forwards).
pub(crate) fn attachment_too_large(what: &str, bytes: u64, per: &str) -> CliResponse {
    error_response(
        "400 Bad Request",
        "attachment_too_large",
        &format!(
            "{what} is too large ({}) — the limit is 25 MiB {per}. Send a link to the file instead",
            mib(bytes)
        ),
    )
}

/// Why a workspace path may not leave in an email, whatever its content: K2's
/// private folder (`.k2/`, and the old `.k2so/`, at any depth: notes, vault,
/// tokens), or a secret by its path ([`k2_core::secret_paths`]: `.env*`,
/// `.ssh/`, `.auth/`, keys and token files).
fn attachment_path_refusal(rel: &str) -> Option<&'static str> {
    let in_k2 = rel
        .split(['/', '\\'])
        .any(|p| p.eq_ignore_ascii_case(".k2") || p.eq_ignore_ascii_case(".k2so"));
    if in_k2 {
        return Some("it is in K2's private .k2 folder. Copy what you mean to send out of .k2/ first");
    }
    if k2_core::secret_paths::is_secret_rel(rel) {
        return Some("it looks like a secret. Secrets travel through the vault (k2 vault), not email");
    }
    None
}

fn attachment_refused(spec: &str, why: &str) -> CliResponse {
    error_response(
        "400 Bad Request",
        "attachment_secret",
        &format!("attachment '{spec}' can't be sent: {why}"),
    )
}

/// Read one workspace file for mail. The ONE reader for every mail attachment
/// (send, reply and draft, hosted and linked). Resolves workspace-relative
/// ([`messages::resolve_in_path`]: no absolute path, no `..`, no symlink out),
/// refuses private and secret paths ([`attachment_path_refusal`], on the path
/// as written AND as resolved, so a link can't rename its way past it), then
/// opens the resolved path with no symlink at any step and works from that one
/// handle: its size, its bytes and the content scan all see the same file, so
/// a path swapped after the checks can't change what is read. `budget` is what
/// the message's total still allows.
fn read_one_attachment(
    ws_root: &str,
    spec: &str,
    budget: u64,
) -> Result<external_smtp::OutAttachment, CliResponse> {
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
    let unreadable = |e: &dyn std::fmt::Display| {
        error_response("400 Bad Request", "usage", &format!("cannot read attachment '{spec}': {e}"))
    };
    let root = dunce::canonicalize(ws_root).map_err(|e| unreadable(&e))?;
    let rel_path = path.strip_prefix(&root).map_err(|e| unreadable(&e))?;
    let rel = rel_path.to_string_lossy().replace('\\', "/");
    if let Some(why) = attachment_path_refusal(spec).or_else(|| attachment_path_refusal(&rel)) {
        return Err(attachment_refused(spec, why));
    }
    let parts: Vec<String> = rel_path.iter().map(|c| c.to_string_lossy().into_owned()).collect();
    let mut file = crate::apps::data_files::open_nofollow(&root, &parts).map_err(|e| unreadable(&e))?;
    let meta = file.metadata().map_err(|e| unreadable(&e))?;
    if !meta.is_file() {
        return Err(error_response(
            "400 Bad Request",
            "usage",
            &format!("attachment '{spec}' is not a regular file"),
        ));
    }
    let size = meta.len();
    if size > MAX_ATTACHMENT_BYTES {
        return Err(attachment_too_large(&format!("attachment '{spec}'"), size, "per file"));
    }
    if size > budget {
        return Err(attachment_too_large(
            "these attachments together",
            MAX_ATTACHMENTS_TOTAL_BYTES - budget + size,
            "in total",
        ));
    }
    // Bounded: a file growing after the size check still can't pass the cap.
    let mut bytes = Vec::with_capacity(size as usize);
    std::io::Read::read_to_end(&mut std::io::Read::take(&mut file, budget + 1), &mut bytes)
        .map_err(|e| unreadable(&e))?;
    if bytes.len() as u64 > budget {
        return Err(attachment_too_large(&format!("attachment '{spec}'"), bytes.len() as u64, "in total"));
    }
    let scan = k2_core::secret_paths::scan_attachment(&rel, &bytes);
    if scan.hit.is_some() {
        return Err(attachment_refused(
            spec,
            "its content looks like it holds a credential (a private key, a token, or a `password=` \
             style line). If it does, secrets travel through the vault (k2 vault), not email. If it \
             doesn't, reword that line, or ask your human to send the file",
        ));
    }
    let filename = safe_attachment_name(path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
    let content_type = content_type_for(&filename);
    Ok(external_smtp::OutAttachment { scanned: scan.read_as_text, filename, content_type, bytes })
}

/// Read every requested file ([`read_one_attachment`]) under the per-message
/// total. Any failure names the path and aborts the whole send (nothing is
/// audited, queued or submitted). Bytes are never logged.
pub(crate) fn read_workspace_attachments(
    ws_root: &str,
    specs: &[String],
) -> Result<Vec<external_smtp::OutAttachment>, CliResponse> {
    let mut out = Vec::with_capacity(specs.len());
    let mut left = MAX_ATTACHMENTS_TOTAL_BYTES;
    for spec in specs {
        let a = read_one_attachment(ws_root, spec, left)?;
        left -= a.bytes.len() as u64;
        out.push(a);
    }
    Ok(out)
}

// ── POST /cli/mail/send ─────────────────────────────────────────────────

/// S5: gated outbound. Body: `{project, to (string|[string]), subject,|[string]), subject,
/// body, cc?, from?, fromName?, wait?, timeout?}`. From is SERVER-STAMPED
/// (owned address; the workspace's single one when `from` is omitted).
/// `fromName` is an optional display name — cosmetic only, validated by
/// [`send::validate_from_name`], HOSTED senders only; without it the
/// workspace's agent name is used.
/// off → 403 `gated` (CLI exit 3); approval → pending row + owner
/// notification (`--wait` blocks until decided); on → audit row +
/// submit ("accepted-for-delivery").
pub fn handle_send(body: &[u8]) -> CliResponse {
    handle_send_with(body, None)
}

/// The thread a send answers: the reply headers it carries.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReplyThread {
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
}

/// [`handle_send`] for a drafted reply a person sends from the mail reader
/// with the recipients they chose: the same gate, outbox and approvals,
/// plus the reply's thread headers. Only the reader calls this (a request
/// body can't ask for it), so an agent's replies still meet `k2 mail
/// reply`'s locked recipient and loop caps.
pub(crate) fn handle_send_in_thread(body: &[u8], thread: &ReplyThread) -> CliResponse {
    handle_send_with(body, Some(thread))
}

fn handle_send_with(body: &[u8], thread: Option<&ReplyThread>) -> CliResponse {
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
    // Agent-chosen blind copies — envelope only, never a header.
    let bcc_raw = match string_list(&v["bcc"], "bcc") {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let reply_to = match string_list(&v["replyTo"], "replyTo") {
        Ok(l) => match send::normalize_reply_to(&l) {
            Ok(r) => r,
            Err(e) => return send_error_response(e),
        },
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
    // Optional From DISPLAY NAME — cosmetic only (the address is still
    // server-stamped below). Validated before any identity work.
    let from_name_explicit = match from_name_param(&v) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let (path, project_id) = match resolve_caller(project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Resolve the sender FIRST — its SOURCE decides the whole pipeline:
    // hosted → Stalwart + mail_agent_send gate; LINKED → same
    // mail_agent_send gate then SMTP (E4). `can_send` masks any
    // no-access case identically for both sources.
    let from_inbox = match resolve_from_inbox(&project_id, v["from"].as_str()) {
        Ok(i) => i,
        Err(resp) => return resp,
    };

    // Always-on recipient + size caps (§8.4) — shared by both sources,
    // before any normalize churn on absurd lists. to + cc + bcc count.
    let raw_total = to_raw.len() + cc_raw.len() + bcc_raw.len();
    if raw_total > send::MAX_RECIPIENTS {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "too many recipients ({raw_total}) — max {} per message, counting to + cc + bcc",
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
    let bcc = match send::normalize_recipients(&bcc_raw) {
        Ok(b) => b,
        Err(e) => return send_error_response(e),
    };
    // The owner's "always BCC" policy — server-stamped from the
    // workspace setting, never from the body (fail-closed read). It
    // joins the envelope and the cap below: over the cap refuses, it
    // never silently drops a recipient.
    let policy_bcc = match send::policy_bcc_for_path(&path) {
        Ok(p) => p,
        Err(e) => return send_error_response(e),
    };
    let agent_name = k2_core::workspace::display::agent_display_name(&path);
    let mut msg = OutboundMessage {
        from_name: Some(from_name_explicit.clone().unwrap_or_else(|| agent_name.clone())),
        from: from_inbox.address.clone(),
        to,
        cc,
        bcc,
        policy_bcc: Vec::new(),
        reply_to,
        subject: subject.to_string(),
        text_body: body_text.to_string(),
        in_reply_to: thread.and_then(|t| t.in_reply_to.clone()),
        references: thread.and_then(|t| t.references.clone()),
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    };
    send::apply_policy_bcc(&mut msg, policy_bcc);
    if let Err(e) = send::check_recipient_limit(&msg) {
        return send_error_response(e);
    }
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

    let schedule_requested = v["sendAt"].as_str().or_else(|| v["send_at"].as_str()).is_some()
        || v["sendIn"].as_str().or_else(|| v["send_in"].as_str()).is_some();

    match from_inbox.source {
        // ── LINKED: mail_agent_send gate, then the governed pipeline over
        // SMTP or Microsoft Graph (§17.5 / E4; MCS2 approval hand-off) ──
        Source::Linked => {
            if from_name_explicit.is_some() {
                return linked_from_name_refused();
            }
            if !msg.reply_to.is_empty() {
                return error_response(
                    "400 Bad Request",
                    "usage",
                    "--reply-to works on hosted K2 mailboxes only — a linked inbox is your human's \
                     own account, and answers go to it. Nothing was sent",
                );
            }
            if schedule_requested {
                return error_response(
                    "400 Bad Request",
                    "usage",
                    "--at/--in scheduling is for hosted K2 mailboxes; a linked inbox sends \
                     now or queues for approval (no schedule queue)",
                );
            }
            let gate = match linked_send_gate(&path) {
                Ok(g) => g,
                Err(resp) => return resp,
            };
            let Some(inbox) = from_inbox.linked else {
                return error_response(
                    "502 Bad Gateway",
                    "engine",
                    "linked inbox row missing from the resolved sender (unexpected)",
                );
            };
            // MCS4: a link consented for its calendar only sends no mail.
            if let Err(resp) = crate::mail::capabilities::mail_scope_gate(&inbox, "mail.send") {
                return resp;
            }
            if let Err(e) = send::check_rate_limits(&DbOutboundStore::default(), &inbox.email_address, now_secs()) {
                return send_error_response(e);
            }
            // Read the attachment files from THIS workspace (caps enforced).
            let attachments = match read_workspace_attachments(&path, &attach_specs) {
                Ok(a) => a,
                Err(resp) => return resp,
            };
            if let Err(resp) =
                graph_attachment_precheck(&inbox, &attachments, subject.len() + body_text.len())
            {
                return resp;
            }
            let submit = match linked_submit(&inbox) {
                Ok(s) => s,
                Err(resp) => return resp,
            };
            dispatch_linked(
                &project_id,
                &path,
                &agent_name,
                &inbox,
                &msg,
                submit.as_ref(),
                gate,
                wait_timeout,
                &attachments,
            )
        }
        // ── HOSTED: the existing governed Stalwart path ──
        Source::Hosted => {
            if let Err(e) = send::check_hosted_text_size(subject, body_text) {
                return send_error_response(e);
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
            let gate = match send::gate_for_path(&path) {
                Ok(g) => g,
                Err(e) => return send_error_response(e),
            };
            if gate == Gate::Off {
                return send_error_response(SendError::Gated(send::agent_send_off_hint()));
            }
            // The FROM domain's send mode (D1: receive-only refuses; relay
            // validates its config at use time — fail-closed).
            if let Err(e) =
                send::check_domain_send_mode(&FileSecretStore::default(), &from_inbox.address)
            {
                return send_error_response(e);
            }
            let send_after = match crate::mail::schedule::resolve_send_after(
                v["sendAt"].as_str().or_else(|| v["send_at"].as_str()),
                v["sendIn"].as_str().or_else(|| v["send_in"].as_str()),
                now_secs(),
            ) {
                Ok(t) => t,
                Err(e) => return send_error_response(e),
            };
            if let Err(e) = send::check_rate_limits(&DbOutboundStore::default(), &from_inbox.address, now_secs()) {
                return send_error_response(e);
            }
            // The files, only now that the send is allowed (a workspace with
            // sending off never has one read), carried on the message itself
            // so the approval queue and a schedule hold exactly these bytes.
            msg.attachments = match read_workspace_attachments(&path, &attach_specs) {
                Ok(a) => a.iter().map(send::StoredAttachment::from_out).collect(),
                Err(resp) => return resp,
            };
            dispatch_and_respond(
                &project_id,
                &path,
                &agent_name,
                &account_id,
                &msg,
                gate,
                wait_timeout,
                send_after,
            )
        }
    }
}

/// `--from-name` on a LINKED sender (send or reply) — a usage refusal:
/// the human's own account sends under its own display name.
fn linked_from_name_refused() -> CliResponse {
    error_response(
        "400 Bad Request",
        "usage",
        "--from-name works on hosted K2 mailboxes only — a linked inbox is your \
         human's own account and sends under the display name set on that \
         account",
    )
}

/// The optional `fromName` body field: absent/null → `None`; a string →
/// validated ([`send::validate_from_name`]); anything else → usage.
fn from_name_param(v: &serde_json::Value) -> Result<Option<String>, CliResponse> {
    match &v["fromName"] {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(raw) => {
            send::validate_from_name(raw).map(Some).map_err(send_error_response)
        }
        _ => Err(error_response(
            "400 Bad Request",
            "usage",
            "'fromName' must be a string (the From display name)",
        )),
    }
}

// ── POST /cli/mail/reply ────────────────────────────────────────────────

/// S5: guardrailed reply (§8.4). Body: `{project, id, body, fromName?,
/// wait?, timeout?}`. Recipient LOCKED to the original sender, From LOCKED
/// to the receiving address, In-Reply-To/References stamped, loop caps
/// (no self-chains, ≤100 References, ≤4 replies/thread/hour),
/// DMARC-fail gated outside approval mode. `fromName` is the same
/// optional display name as on `handle_send` (`k2 mail reply
/// --from-name`): cosmetic only, HOSTED senders only; without it the
/// workspace's agent name is used.
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
    // A reply stays locked to the original sender, and refuses (never drops)
    // an attempt to point its answers elsewhere.
    if !v["replyTo"].is_null() {
        return error_response(
            "400 Bad Request",
            "usage",
            "k2 mail reply goes back to the original sender and takes no --reply-to — send a new \
             message with k2 mail send if answers should go elsewhere",
        );
    }
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
    // Optional From DISPLAY NAME (`--from-name`) — cosmetic only; the
    // address stays LOCKED to the receiving one. Validated before any
    // identity work, exactly like `handle_send`.
    let from_name_explicit = match from_name_param(&v) {
        Ok(n) => n,
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
    // LINKED → same mail_agent_send gate then SMTP, E4) — like
    // `handle_send`.
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
    // The owner's "always BCC" policy rides replies too (server-stamped,
    // fail-closed read). The reply's recipient stays locked; the policy
    // copies are envelope-only.
    let policy_bcc = match send::policy_bcc_for_path(&path) {
        Ok(p) => p,
        Err(e) => return send_error_response(e),
    };
    if 1 + policy_bcc.len() > send::MAX_RECIPIENTS {
        return send_error_response(SendError::Usage(format!(
            "too many recipients ({}) — max {} per message. This workspace's owner policy \
             always BCCs {} address(es), which count toward the limit and can't be dropped",
            1 + policy_bcc.len(),
            send::MAX_RECIPIENTS,
            policy_bcc.len()
        )));
    }

    match from_inbox.source {
        // ── LINKED: mail_agent_send gate, then the governed pipeline; the
        // provider threads and locks the recipient (SMTP re-reads the
        // source over IMAP; Graph uses createReply) (E4; MCS2) ──
        Source::Linked => {
            if from_name_explicit.is_some() {
                return linked_from_name_refused();
            }
            let gate = match linked_send_gate(&path) {
                Ok(g) => g,
                Err(resp) => return resp,
            };
            let Some(inbox) = from_inbox.linked else {
                return error_response(
                    "502 Bad Gateway",
                    "engine",
                    "linked inbox row missing from the resolved sender (unexpected)",
                );
            };
            // MCS4: a link consented for its calendar only sends no mail.
            if let Err(resp) = crate::mail::capabilities::mail_scope_gate(&inbox, "mail.send") {
                return resp;
            }
            if let Err(e) = send::check_rate_limits(&DbOutboundStore::default(), &inbox.email_address, now_secs()) {
                return send_error_response(e);
            }
            let attachments = match read_workspace_attachments(&path, &attach_specs) {
                Ok(a) => a,
                Err(resp) => return resp,
            };
            if let Err(resp) = graph_attachment_precheck(&inbox, &attachments, body_text.len()) {
                return resp;
            }
            let submit = match linked_submit(&inbox) {
                Ok(s) => s,
                Err(resp) => return resp,
            };
            // Who the reply goes to (the source's sender, locked) and its
            // subject — read BEFORE anything is audited, queued or sent, so
            // an approval shows the real recipient. `email_id` is the
            // linked message's backend id (`uid:…` / `graph:…`).
            let (recipient, subject) = match submit.reply_preview(&email_id) {
                Ok(Some(p)) => p,
                Ok(None) => return masked(),
                Err(e) => return submit_error_response(e),
            };
            let agent_name = k2_core::workspace::display::agent_display_name(&path);
            let mut msg = OutboundMessage {
                from_name: Some(agent_name.clone()),
                from: inbox.email_address.clone(),
                to: vec![recipient],
                cc: Vec::new(),
                bcc: Vec::new(),
                policy_bcc: Vec::new(),
                reply_to: Vec::new(),
                subject,
                text_body: body_text.to_string(),
                in_reply_to: None,
                references: None,
                reply_source: Some(email_id.clone()),
                attachments: Vec::new(),
                ephemeral: false,
                notify: None,
                forward_of: None,
                source_dmarc: None,
                source_check: None,
            };
            send::apply_policy_bcc(&mut msg, policy_bcc);
            if let Err(e) = send::check_recipient_limit(&msg) {
                return send_error_response(e);
            }
            dispatch_linked(
                &project_id,
                &path,
                &agent_name,
                &inbox,
                &msg,
                submit.as_ref(),
                gate,
                wait_timeout,
                &attachments,
            )
        }
        // ── HOSTED: the existing governed reply path ──
        Source::Hosted => {
            let from_address = from_inbox.address.clone();
            let Some(account_id) = from_inbox.account_id.clone() else {
                return masked();
            };
            // Gate: off refuses BEFORE any engine dial.
            let gate = match send::gate_for_path(&path) {
                Ok(g) => g,
                Err(e) => return send_error_response(e),
            };
            if gate == Gate::Off {
                return send_error_response(SendError::Gated(send::agent_send_off_hint()));
            }
            if let Err(e) = send::check_hosted_text_size("", body_text) {
                return send_error_response(e);
            }
            if let Err(e) = send::check_rate_limits(&DbOutboundStore::default(), &from_address, now_secs()) {
                return send_error_response(e);
            }
            // The files first: a refused file must not use up the thread's
            // reply budget below, and needs no engine dial.
            let attachments: Vec<send::StoredAttachment> =
                match read_workspace_attachments(&path, &attach_specs) {
                    Ok(a) => a.iter().map(send::StoredAttachment::from_out).collect(),
                    Err(resp) => return resp,
                };
            // Fetch the original's reply context (sender, threading
            // headers, auth verdicts) from its backend.
            let (client, hostname) = match domains::engine_from_db() {
                Ok(c) => c,
                Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
            };
            let ctx = match client.email_get_reply_context(&account_id, &email_id) {
                Ok(Some(ctx)) => ctx,
                Ok(None) => return masked(),
                Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
            };

            // §8.4 guardrails: locked recipient, loop caps, DMARC gate.
            let authority = messages::AuthAuthority::ThisBox(hostname);
            let on_box = send::sender_on_this_box(&ctx.from);
            let recipient = match send::check_reply_guardrails(&ctx, &from_address, gate, &authority, on_box) {
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
            let mut msg = OutboundMessage {
                from_name: Some(from_name_explicit.clone().unwrap_or_else(|| agent_name.clone())),
                from: from_address.clone(),
                to: vec![recipient],
                cc: Vec::new(),
                bcc: Vec::new(),
                policy_bcc: Vec::new(),
                reply_to: Vec::new(),
                subject: send::reply_subject(&ctx.subject),
                text_body: body_text.to_string(),
                in_reply_to: ctx.message_id.clone(),
                references: send::build_out_references(
                    ctx.references.as_deref(),
                    ctx.message_id.as_deref(),
                ),
                reply_source: None,
                attachments,
                ephemeral: false,
                notify: None,
                forward_of: None,
                source_dmarc: messages::parse_auth_results(&ctx.auth_results, &authority)["dmarc"]
                    .as_str()
                    .map(String::from),
                source_check: match messages::sender_check(&ctx.auth_results, &authority, &ctx.from, on_box) {
                    messages::SenderCheck::Clear => None,
                    messages::SenderCheck::NeedsPerson(why) => Some(why),
                },
            };
            send::apply_policy_bcc(&mut msg, policy_bcc);
            if let Err(e) = send::check_recipient_limit(&msg) {
                return send_error_response(e);
            }
            let send_after = match crate::mail::schedule::resolve_send_after(
                v["sendAt"].as_str().or_else(|| v["send_at"].as_str()),
                v["sendIn"].as_str().or_else(|| v["send_in"].as_str()),
                now_secs(),
            ) {
                Ok(t) => t,
                Err(e) => return send_error_response(e),
            };
            dispatch_and_respond(
                &project_id,
                &path,
                &agent_name,
                &account_id,
                &msg,
                gate,
                wait_timeout,
                send_after,
            )
        }
    }
}

// ── POST /cli/mail/forward-message ──────────────────────────────────────

/// The refusal for a linked inbox on either end of a forward.
fn forward_linked_refused(which: &str) -> CliResponse {
    error_response(
        "400 Bad Request",
        "usage",
        &format!(
            "k2 mail forward-message works on hosted K2 mailboxes only for now, and {which} is a \
             linked inbox. Nothing was sent. Read the message with k2 mail read and send what \
             matters with k2 mail send, or ask your human to forward it from that account"
        ),
    )
}

/// Send one existing message on to someone else ([`crate::mail::fwd_message`]).
/// Body: `{project, id, to, cc?, bcc?, from?, fromName?, note?, asEml?,
/// replyTo?, replyToOriginal?, wait?, timeout?}`. Same pipeline as
/// `handle_send`. READ is checked exactly as `k2 mail read` checks it
/// ([`access::can_read`], the same masked answer for a message that isn't
/// there and one the caller may not see), and first, so no other answer can
/// tell the two apart. SEND is checked on the From: by default the mailbox
/// holding the original; a workspace that may only read it names its own
/// mailbox with `from`. The From is always a mailbox the caller may send from,
/// never the original sender.
pub fn handle_forward_message(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(project) = v["project"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response("400 Bad Request", "usage", "missing 'project' (workspace name | path | UUID)");
    };
    let Some(token) = v["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response("400 Bad Request", "usage", "missing 'id' — a message id from 'k2 mail messages'");
    };
    let to_raw = match string_list(&v["to"], "to") {
        Ok(l) if !l.is_empty() => l,
        Ok(_) => return error_response("400 Bad Request", "usage", "missing 'to' — at least one recipient"),
        Err(resp) => return resp,
    };
    let cc_raw = match string_list(&v["cc"], "cc") {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let bcc_raw = match string_list(&v["bcc"], "bcc") {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let reply_to_raw = match string_list(&v["replyTo"], "replyTo") {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let reply_to_original = v["replyToOriginal"].as_bool().unwrap_or(false);
    if reply_to_original && !reply_to_raw.is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "pass only one of --reply-to <addr> or --reply-to-original",
        );
    }
    let mut reply_to = match send::normalize_reply_to(&reply_to_raw) {
        Ok(r) => r,
        Err(e) => return send_error_response(e),
    };
    let note = match &v["note"] {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.as_str()),
        _ => return error_response("400 Bad Request", "usage", "'note' must be a string"),
    };
    let as_eml = v["asEml"].as_bool().unwrap_or(false);
    let wait_timeout = match wait_params(&v) {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let from_name_explicit = match from_name_param(&v) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let (path, project_id) = match resolve_caller(project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // The read gate, as `k2 mail read`: the address inside the id must be one
    // this workspace can read. Every miss answers alike.
    let Some((address, email_id)) = messages::decode_message_id(token) else {
        return error_response("400 Bad Request", "usage", "invalid message id — use an id from 'k2 mail messages'");
    };
    let masked = || error_response("404 Not Found", "not_found", &format!("no message '{token}' in this workspace"));
    let source = match access::can_read(&project_id, &address) {
        Ok(inbox) => inbox,
        Err(ReadError::Usage(hint)) => return error_response("400 Bad Request", "usage", &hint),
        Err(_) => return masked(),
    };
    if source.source == Source::Linked {
        return forward_linked_refused("this message's inbox");
    }
    let Some(source_account) = source.account_id.clone() else {
        return masked();
    };

    // The sender: an explicit address passes the send gate as on `k2 mail
    // send`; otherwise the mailbox holding the message.
    let from_inbox = match v["from"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(addr) => match resolve_from_inbox(&project_id, Some(addr)) {
            Ok(i) => i,
            Err(resp) => return resp,
        },
        None => match access::can_send(&project_id, &address) {
            Ok(i) => i,
            Err(ReadError::Usage(hint)) => return error_response("400 Bad Request", "usage", &hint),
            Err(_) => {
                return send_error_response(SendError::Gated(format!(
                    "you can read {address} but not send from it — pass --from with a mailbox of \
                     yours, or ask the primary workspace for 'send' access (k2 mail access set-level)"
                )))
            }
        },
    };
    if from_inbox.source == Source::Linked {
        return forward_linked_refused("the sending address");
    }
    let Some(account_id) = from_inbox.account_id.clone() else {
        return error_response(
            "502 Bad Gateway",
            "engine",
            &format!("address '{}' has no mailbox on the mail server", from_inbox.address),
        );
    };

    // The gate and the sender's domain, before anything is fetched.
    let gate = match send::gate_for_path(&path) {
        Ok(g) => g,
        Err(e) => return send_error_response(e),
    };
    if gate == Gate::Off {
        return send_error_response(SendError::Gated(send::agent_send_off_hint()));
    }
    if let Err(e) = send::check_domain_send_mode(&FileSecretStore::default(), &from_inbox.address) {
        return send_error_response(e);
    }

    // Recipients, as `handle_send`.
    let raw_total = to_raw.len() + cc_raw.len() + bcc_raw.len();
    if raw_total > send::MAX_RECIPIENTS {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!("too many recipients ({raw_total}) — max {} per message, counting to + cc + bcc", send::MAX_RECIPIENTS),
        );
    }
    let (to, cc, bcc) = match (
        send::normalize_recipients(&to_raw),
        send::normalize_recipients(&cc_raw),
        send::normalize_recipients(&bcc_raw),
    ) {
        (Ok(t), Ok(c), Ok(b)) => (t, c, b),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return send_error_response(e),
    };
    let policy_bcc = match send::policy_bcc_for_path(&path) {
        Ok(p) => p,
        Err(e) => return send_error_response(e),
    };
    let agent_name = k2_core::workspace::display::agent_display_name(&path);
    let mut msg = OutboundMessage {
        from_name: Some(from_name_explicit.clone().unwrap_or_else(|| agent_name.clone())),
        from: from_inbox.address.clone(),
        to,
        cc,
        bcc,
        policy_bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: String::new(),
        text_body: String::new(),
        in_reply_to: None,
        references: None,
        reply_source: None,
        attachments: Vec::new(),
        ephemeral: false,
        notify: None,
        forward_of: None,
        source_dmarc: None,
        source_check: None,
    };
    send::apply_policy_bcc(&mut msg, policy_bcc);
    if let Err(e) = send::check_recipient_limit(&msg) {
        return send_error_response(e);
    }

    if let Err(e) = send::check_rate_limits(&DbOutboundStore::default(), &from_inbox.address, now_secs()) {
        return send_error_response(e);
    }
    // Everything allowed so far: now read the original.
    let (client, hostname) = match domains::engine_from_db() {
        Ok(c) => c,
        Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
    };
    let read = messages::StalwartReadBackend::new(client, hostname);
    let src = match messages::ReadBackend::fetch_for_forward(&read, &source_account, &email_id) {
        Ok(Some(s)) => s,
        Ok(None) => return masked(),
        Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
    };
    let on_box = send::sender_on_this_box(&src.full.summary.from);
    if let Err(e) = forward_sender_gate(
        &src.full.auth_results,
        &messages::ReadBackend::auth_authority(&read),
        &src.full.summary.from,
        on_box,
        gate,
    ) {
        return send_error_response(e);
    }
    let original_sender = src.full.summary.from.first().map(|a| a.email.trim().to_string());
    if reply_to_original {
        let Some(sender) = original_sender.as_deref().filter(|s| !s.is_empty()) else {
            return error_response("400 Bad Request", "usage", "--reply-to-original: the original names no sender");
        };
        reply_to = match send::normalize_reply_to(&[sender.to_string()]) {
            Ok(r) => r,
            Err(_) => {
                return error_response(
                    "400 Bad Request",
                    "usage",
                    "--reply-to-original: the original's sender isn't a usable address, so nothing was sent",
                )
            }
        };
    }
    let composed = match crate::mail::fwd_message::compose(&read, &source_account, &src, note, as_eml) {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(e) = send::check_hosted_text_size(&composed.subject, &composed.text_body) {
        return send_error_response(e);
    }
    msg.reply_to = reply_to;
    msg.subject = composed.subject;
    msg.text_body = composed.text_body;
    msg.attachments = composed.attachments;
    msg.source_check = match messages::sender_check(
        &src.full.auth_results,
        &messages::ReadBackend::auth_authority(&read),
        &src.full.summary.from,
        on_box,
    ) {
        messages::SenderCheck::Clear => None,
        messages::SenderCheck::NeedsPerson(why) => Some(why),
    };
    msg.source_dmarc = messages::parse_auth_results(
        &src.full.auth_results,
        &messages::ReadBackend::auth_authority(&read),
    )["dmarc"]
        .as_str()
        .map(String::from);
    msg.forward_of = Some(send::ForwardOf {
        mailbox: address.clone(),
        sender: crate::mail::fwd_message::one_line(original_sender.as_deref().unwrap_or("")),
        as_eml,
    });
    let resp = dispatch_and_respond(&project_id, &path, &agent_name, &account_id, &msg, gate, wait_timeout, None);
    audit_forward(&agent_name, &msg, &resp);
    resp
}

/// One line per forward in the audit log, whatever became of it: who, from where,
/// to whom, and what it carried. Never a subject, a body or a file's bytes.
fn audit_forward(agent_name: &str, msg: &OutboundMessage, resp: &CliResponse) {
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
    let (mailbox, sender, as_eml) = match msg.forward_of.as_ref() {
        Some(f) => (f.mailbox.as_str(), f.sender.as_str(), f.as_eml),
        None => ("?", "?", false),
    };
    crate::thread_attachments::audit(
        "mail.forward_message",
        &format!("agent:{agent_name}"),
        format!(
            "id={} status={} from={} source={mailbox} source_sender={sender} as_eml={as_eml} files={} to={:?} cc={:?} bcc={:?} reply_to={:?}",
            v["id"].as_str().unwrap_or("-"),
            v["status"].as_str().unwrap_or("refused"),
            msg.from,
            msg.attachments.len(),
            msg.to,
            msg.cc,
            msg.all_bcc(),
            msg.reply_to,
        ),
    );
}

/// As for a reply (§8.4): passing on a message whose sender nothing vouches for,
/// under this mailbox's name, needs a person looking at it.
fn forward_sender_gate(
    auth_results: &[String],
    authority: &messages::AuthAuthority,
    from: &[crate::mail::jmap::MailAddr],
    on_box: bool,
    gate: Gate,
) -> Result<(), SendError> {
    if gate == Gate::Approval {
        return Ok(());
    }
    match messages::sender_check(auth_results, authority, from, on_box) {
        messages::SenderCheck::Clear => Ok(()),
        messages::SenderCheck::NeedsPerson(why) => Err(SendError::Guardrail(format!(
            "the original message {why}, so its sender may be forged — forwarding it is blocked \
             unless send gating is 'approval' (your human reviews it). Ask your human via \
             'k2 tickets ask' with an HTML brief (k2 study ticket-brief)"
        ))),
    }
}

// ── GET /cli/mail/outbox ────────────────────────────────────────────────

/// S5 (§11.1.9): the caller's outbound rows, newest first — or one row
/// via `?id=out_…` (masked `not_found` outside the workspace). Params:
/// `project` (required) · `id` · `limit` (default 20, max 100).
/// Expired pending items lazily auto-deny on every read (§12).
pub fn handle_outbox(params: &HashMap<String, String>) -> CliResponse {
    // Wave 0: principal-or-claim identity (stamped scoped principal wins
    // over client project= / K2_PROJECT_PATH).
    let (_path, project_id) = match crate::mail::identity::resolve_caller_params(params) {
        Ok(v) => v,
        Err(resp) => {
            if crate::caller_workspace::principal_from_params(params).is_none()
                && crate::cli::need_project(params).is_err()
            {
                return error_response(
                    "400 Bad Request",
                    "usage",
                    "missing 'project' (workspace name | path | UUID)",
                );
            }
            return resp;
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
    send::auto_deny_expired(now_secs());

    if let Some(id) = params.get("id").map(String::as_str).map(str::trim).filter(|s| !s.is_empty())
    {
        let store = DbOutboundStore::default();
        let row = match store.load(id) {
            Ok(r) => r,
            Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
        };
        // Masked: a foreign row answers exactly like a missing one — and so
        // does a row sent from a person's mailbox this login may not open
        // (OA0).
        let Some(row) = row.filter(|r| {
            r.owner_project_id == project_id
                && crate::mail::mailbox_guard::check_address(Some(project_id.as_str()), &r.from_address)
        }) else {
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
    let list: Vec<serde_json::Value> = rows
        .iter()
        .filter(|r| crate::mail::mailbox_guard::check_address(Some(project_id.as_str()), &r.from_address))
        .map(send::outbound_json)
        .collect();
    ok_json(serde_json::json!({
        "ok": true,
        "count": list.len(),
        "outbox": list,
    }))
}

// ── POST /cli/mail/outbox/cancel ────────────────────────────────────────

pub fn handle_outbox_cancel(body: &[u8]) -> CliResponse {
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
    let Some(id) = v["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — a scheduled outbound id (out_…)",
        );
    };
    let (path, project_id) = match resolve_caller(project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let agent_name = k2_core::workspace::display::agent_display_name(&path);
    let store = DbOutboundStore::default();
    // OA0: a send from a person's mailbox this login may not open answers
    // like a missing one.
    if let Ok(Some(row)) = store.load(id) {
        if row.owner_project_id == project_id
            && !crate::mail::mailbox_guard::check_address(Some(project_id.as_str()), &row.from_address)
        {
            return send_error_response(SendError::NotFound(format!("no outbound message '{id}'")));
        }
    }
    match crate::mail::schedule::cancel_scheduled(
        &store,
        id,
        &project_id,
        &format!("agent:{agent_name}"),
        now_secs(),
    ) {
        Ok(()) => ok_json(serde_json::json!({
            "ok": true,
            "id": id,
            "status": "rejected",
            "hint": format!("cancelled scheduled send {id} — it will not leave the box"),
        })),
        Err(e) => send_error_response(e),
    }
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
    // Email Notifications (security review L4): a held App email is the
    // owner's to decide, so a mail-manage agent's queue doesn't list it
    // (nor preview its body): its From is a notification sender.
    let agent = crate::mail::agent_creds::mail_caller().is_agent();
    let list: Vec<serde_json::Value> = rows
        .iter()
        .filter_map(|row| {
            // One read per row: a body names its files, it doesn't hold them.
            // A missing/corrupt body file lists WITHOUT a preview (the
            // decision buttons still work — approve would then fail loudly
            // on the unreadable body).
            let stored = store.load_message(&row.id).ok();
            let hidden_from_agent = agent
                && (crate::email_notify::send::is_sender_address(&row.from_address)
                    || stored.as_ref().is_some_and(|m| m.notify.is_some()));
            if hidden_from_agent {
                return None;
            }
            let mut v = send::outbound_json(row);
            let preview: Option<String> =
                stored.as_ref().map(|m| send::text_for_approver(m).chars().take(280).collect());
            v["bodyPreview"] = serde_json::json!(preview);
            if let Some(d) = stored.as_ref().and_then(|m| m.source_dmarc.as_ref()) {
                v["sourceDmarc"] = serde_json::json!(d);
            }
            // Every file that would leave, with its size. `k2 hostmail approvals
            // show` opens it (GET /cli/mail/approvals/show).
            if let Some(m) = stored.as_ref().filter(|m| !m.attachments.is_empty()) {
                v["files"] = serde_json::Value::Array(
                    m.attachments.iter().enumerate().map(|(i, f)| f.view_json(i + 1)).collect(),
                );
            }
            // The From display name the recipient will see (agent name
            // or the agent's --from-name) — the approver sees it first.
            v["fromName"] = serde_json::json!(stored.as_ref().and_then(|m| m.from_name.clone()));
            v["workspace"] = serde_json::json!(project_name(&row.owner_project_id));
            v["expiresAt"] = serde_json::json!(row.updated_at + send::APPROVAL_EXPIRE_SECS);
            Some(v)
        })
        .collect();
    ok_json(serde_json::json!({
        "ok": true,
        "count": list.len(),
        "pending": list,
    }))
}

/// One waiting message as the approver decides it: the whole text (the list
/// carries 280 characters) and the files; with `file=<n>` (1-based, as the
/// list numbers them) that file's bytes, base64. Read-only; same gate as the
/// list (Admin route, mail-manage surface), and a mail-manage agent can't open
/// a held App email here, as it can't see one in the list.
pub fn handle_approvals_show(params: &HashMap<String, String>) -> CliResponse {
    let Some(id) = params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — an outbound id from the approvals list (out_…)",
        );
    };
    let file_n: Option<usize> = match params.get("file").map(|v| v.parse::<usize>()) {
        None => None,
        Some(Ok(n)) if n >= 1 => Some(n),
        _ => {
            return error_response(
                "400 Bad Request",
                "usage",
                "invalid 'file' — the file number from the approvals list (1, 2, …)",
            )
        }
    };
    send::auto_deny_expired(now_secs());
    let store = DbOutboundStore::default();
    let masked = || error_response("404 Not Found", "not_found", &format!("no outbound message '{id}'"));
    let row = match store.load(id) {
        Ok(Some(r)) => r,
        Ok(None) => return masked(),
        Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
    };
    if row.status != "pending" {
        return send_error_response(SendError::Conflict(format!(
            "outbound '{id}' is {} — only a message waiting for approval can be opened here",
            send::wire_status(&row.status)
        )));
    }
    let msg = match store.load_message(id) {
        Ok(m) => m,
        Err(hint) => return error_response("502 Bad Gateway", "engine", &hint),
    };
    let agent = crate::mail::agent_creds::mail_caller().is_agent();
    if agent && (crate::email_notify::send::is_sender_address(&row.from_address) || msg.notify.is_some()) {
        return masked();
    }
    if let Some(n) = file_n {
        use base64::Engine as _;
        return match store.load_file(id, n - 1) {
            Ok(Some((f, bytes))) => ok_json(serde_json::json!({
                "ok": true,
                "id": id,
                "n": n,
                "name": f.filename,
                "size": f.size(),
                "contentType": f.content_type,
                "scanned": f.scanned,
                "dataBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
            })),
            Ok(None) => error_response(
                "404 Not Found",
                "not_found",
                &format!("outbound '{id}' has no file {n} ({} attached)", msg.attachments.len()),
            ),
            Err(hint) => error_response("502 Bad Gateway", "engine", &hint),
        };
    }
    let mut v = send::outbound_json(&row);
    v["fromName"] = serde_json::json!(msg.from_name);
    if let Some(d) = msg.source_dmarc.as_ref() {
        v["sourceDmarc"] = serde_json::json!(d);
    }
    v["text"] = serde_json::json!(send::text_for_approver(&msg));
    v["files"] = serde_json::Value::Array(
        msg.attachments.iter().enumerate().map(|(i, f)| f.view_json(i + 1)).collect(),
    );
    v["workspace"] = serde_json::json!(project_name(&row.owner_project_id));
    v["expiresAt"] = serde_json::json!(row.updated_at + send::APPROVAL_EXPIRE_SECS);
    ok_json(serde_json::json!({ "ok": true, "message": v }))
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

/// Who a decision is recorded under. An IT agent is recorded as itself, whatever
/// its request says; only a person's route may name who decided.
fn decided_by_for(caller: &crate::mail::agent_creds::MailCaller, asked: Option<&str>) -> String {
    use crate::mail::agent_creds::MailCaller;
    match caller {
        MailCaller::ItAgent { workspace_uuid } | MailCaller::Agent { workspace_uuid } => {
            format!("agent:{}", project_name(workspace_uuid).unwrap_or_else(|| workspace_uuid.clone()))
        }
        MailCaller::Owner => asked.unwrap_or("owner").to_string(),
    }
}

/// S5: owner-or-admin approve → ATOMIC `pending → approved` → submit
/// via the LOCAL Stalwart (the audit row has existed since queue
/// time). Body: `{id, note?, decidedBy?}`. Already-decided/unknown ids
/// refuse WITHOUT submitting (pre-mortem #11).
pub fn handle_approvals_approve(body: &[u8]) -> CliResponse {
    // 0.45.0: an agent without mail-manage never decides a queued send.
    let caller = crate::mail::agent_creds::mail_caller();
    if let crate::mail::agent_creds::MailCaller::Agent { .. } = caller {
        return crate::mail::agent_creds::refuse_needs_mail_manage(
            "approve or reject agent mail",
        );
    }
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
    // An IT agent decides other workspaces' sends, never its own.
    if let Err(resp) =
        crate::mail::agent_creds::approval_gate(&caller, outbound_sender(id).as_deref())
    {
        return resp;
    }
    // Email Notifications (E15, E4): a held App email leaves from a
    // notification sender; deciding it is the owner's or an Admin's.
    if caller.is_agent() && outbound_from(id).is_some_and(|f| crate::email_notify::send::is_sender_address(&f)) {
        return crate::email_notify::fence::owner_only("approve an App's held email");
    }
    let note = v["note"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let decided_by = decided_by_for(&caller, v["decidedBy"].as_str().map(str::trim).filter(|s| !s.is_empty()));
    let now = now_secs();
    // Expired items flip to denied FIRST — a 7-day-old approve answers
    // the conflict, it never sends (§12).
    send::auto_deny_expired(now);

    let store = DbOutboundStore::default();
    // MCS2: the approval hand-off resolves the backend from the From
    // address — a LINKED inbox leaves through its own provider (SMTP or
    // Microsoft Graph); a hosted one through the local Stalwart (only
    // dialled when the message is hosted).
    let engine = std::cell::OnceCell::new();
    // The invites' test seam (never set in production): hosted mail goes
    // to it instead of the local Stalwart.
    let override_backend = crate::app_user_invites::backend_override();
    let mut resolve = |msg: &OutboundMessage| -> Result<Box<dyn crate::mail::submit::MailSubmit + '_>, String> {
        // Email Notifications (E15): a held App email leaves through its
        // sender's path, never a mailbox's.
        if let Some(s) = crate::email_notify::send::held_submitter(msg) {
            return Ok(s);
        }
        if let Some(inbox) = external::inbox_for_address(&msg.from) {
            return crate::mail::submit::submit_for_linked(&inbox);
        }
        let account = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT stalwart_account_id FROM mail_addresses \
                 WHERE address = ?1 AND status = 'active'",
                rusqlite::params![msg.from],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
        };
        let account = account.ok_or_else(|| {
            format!("sender address unavailable: sender address '{}' is no longer active", msg.from)
        })?;
        if let Some(b) = override_backend.as_ref() {
            return Ok(Box::new(crate::mail::submit::HostedSubmit {
                backend: &**b as &dyn SubmitBackend,
                account_id: account,
            }));
        }
        let client = match engine.get_or_init(domains::engine_from_db) {
            Ok((client, _)) => client,
            Err(e) => return Err(format!("mail server unavailable: {e}")),
        };
        Ok(Box::new(crate::mail::submit::HostedSubmit {
            backend: client as &dyn SubmitBackend,
            account_id: account,
        }))
    };
    match send::approve_and_submit_with(
        &store,
        &mut resolve,
        id,
        &decided_by,
        note,
        now,
    ) {
        Ok(send::ApproveOutcome::Submitted) => {
            // agent-app-users: a held invite's link starts its clock now.
            crate::app_user_invites::outbound_sent(id);
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
        Ok(send::ApproveOutcome::Scheduled { send_after }) => {
            ok_json(serde_json::json!({
                "ok": true,
                "id": id,
                "status": "scheduled",
                "sendAfter": send_after,
                "sendAfterRfc3339": crate::mail::schedule::format_send_after(send_after),
                "hint": format!(
                    "approved — held until {} (scheduled)",
                    crate::mail::schedule::format_send_after(send_after)
                ),
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

/// The sending workspace (`mail_outbound.owner_project_id`) of a queued
/// item; `None` for an unknown id (the decision then 404s as before).
fn outbound_sender(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT owner_project_id FROM mail_outbound WHERE id = ?1",
        rusqlite::params![id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// The From address of a queued message (Email Notifications E15: a held
/// App email's From is a notification sender).
fn outbound_from(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT from_address FROM mail_outbound WHERE id = ?1", rusqlite::params![id], |r| r.get::<_, String>(0)).ok()
}

/// S5: owner-or-admin deny with a REQUIRED note that flows back to the
/// agent's outbox (§8.4/§11: `deny <id> --note …`). Body:
/// `{id, note, decidedBy?}`.
pub fn handle_approvals_deny(body: &[u8]) -> CliResponse {
    // 0.45.0: same gate as approve (see mail::agent_creds).
    let caller = crate::mail::agent_creds::mail_caller();
    if let crate::mail::agent_creds::MailCaller::Agent { .. } = caller {
        return crate::mail::agent_creds::refuse_needs_mail_manage(
            "approve or reject agent mail",
        );
    }
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
    if let Err(resp) =
        crate::mail::agent_creds::approval_gate(&caller, outbound_sender(id).as_deref())
    {
        return resp;
    }
    let Some(note) = v["note"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'note' — the reason flows back to the agent (required on deny)",
        );
    };
    let decided_by = decided_by_for(&caller, v["decidedBy"].as_str().map(str::trim).filter(|s| !s.is_empty()));
    let now = now_secs();
    send::auto_deny_expired(now);
    let store = DbOutboundStore::default();
    match send::deny(&store, id, &decided_by, note, now) {
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
#[path = "routes_send_tests.rs"]
mod tests;
