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
use crate::mail::access::{self, Source};
use crate::mail::domains;
use crate::mail::messages::{self, ReadError};
use crate::mail::routes_messages::WaitSlot;
use crate::mail::secrets::FileSecretStore;
use crate::mail::send::{
    self, DbOutboundStore, Gate, OutboundMessage, OutboundRequest, OutboundStore, SendError,
    SendOutcome, SubmitBackend,
};

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

/// A teaching 403 when the caller CAN see the address (reads it) but
/// lacks SEND — it is read/draft-only for them (or a linked account,
/// which can never send). `gated` → CLI exit 3 (ask the primary/human).
/// If they can't even read it, we mask (no existence leak).
fn not_sendable_response(project_id: &str, address: &str) -> CliResponse {
    match access::can_read(project_id, address) {
        Ok(inbox) if inbox.source == Source::Linked => send_error_response(SendError::Gated(
            "linked accounts can't send — there is no send path from an external account. \
             Save a reply draft with 'k2 mail draft' instead."
                .to_string(),
        )),
        Ok(_) => send_error_response(SendError::Gated(
            "this inbox is read/draft-only for your workspace — ask the primary workspace \
             for 'send' access (k2 mail access set-level)."
                .to_string(),
        )),
        Err(e) => read_error_response(e),
    }
}

/// The server-stamped From: an explicit address must pass the S11 SEND
/// gate (hosted + effective level 'send'); none → the workspace's
/// single SENDABLE address (two+ is a usage error naming the fix; zero
/// is the teaching not_found). Returns `(address, stalwart account id)`.
fn resolve_from(
    project_id: &str,
    explicit: Option<&str>,
) -> Result<(String, String), CliResponse> {
    if let Some(addr) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return match access::can_send(project_id, addr) {
            Ok(inbox) => {
                let Some(account_id) = inbox.account_id else {
                    return Err(error_response(
                        "502 Bad Gateway",
                        "engine",
                        &format!("address '{}' has no mailbox on the mail server", inbox.address),
                    ));
                };
                Ok((inbox.address, account_id))
            }
            Err(ReadError::Usage(hint)) => {
                Err(error_response("400 Bad Request", "usage", &hint))
            }
            // Masked, unless they can read it — then teach send is denied.
            Err(_) => Err(not_sendable_response(project_id, addr)),
        };
    }
    let mut all = access::sendable_hosted(project_id);
    match all.len() {
        0 => Err(error_response(
            "404 Not Found",
            "not_found",
            "this workspace has no address it can send FROM — mint one with 'k2 mail create', \
             or ask the primary for 'send' access (k2 mail inboxes shows yourLevel)",
        )),
        1 => Ok(all.remove(0)),
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
    // V1 daemon surface: no attachments on send yet — refuse loudly
    // rather than silently dropping them.
    if v.get("attach").is_some() || v.get("attachments").is_some() {
        return error_response(
            "400 Bad Request",
            "usage",
            "attachments are not supported on 'k2 mail send' yet — include a link instead",
        );
    }
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
    let (path, project_id) = match resolve_caller(project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // The D4 gate first — fail-closed reader (tested in mail::send).
    let gate = match send::effective_gate(Ok(
        k2_core::workspace::settings::mail_agent_send_for_path(&path),
    )) {
        Ok(g) => g,
        Err(e) => return send_error_response(e),
    };
    if gate == Gate::Off {
        return send_error_response(SendError::Gated(
            "outbound email is disabled for this workspace. Your human can enable it in \
             Settings → Email → Sending"
                .to_string(),
        ));
    }

    let (from_address, account_id) =
        match resolve_from(&project_id, v["from"].as_str()) {
            Ok(f) => f,
            Err(resp) => return resp,
        };

    // Always-on recipient + size caps (§8.4) — before any normalize
    // churn on absurd lists.
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
    // The FROM domain's send mode (D1: receive-only refuses; relay
    // validates its config at use time — fail-closed).
    if let Err(e) = send::check_domain_send_mode(&FileSecretStore::default(), &from_address) {
        return send_error_response(e);
    }

    let agent_name = k2_core::workspace::display::agent_display_name(&path);
    let msg = OutboundMessage {
        from_name: Some(agent_name.clone()),
        from: from_address,
        to,
        cc,
        subject: subject.to_string(),
        text_body: body_text.to_string(),
        in_reply_to: None,
        references: None,
    };
    // The always-on message-size cap (§8.4) — on the composed text.
    if msg.message_bytes() > send::MAX_MESSAGE_BYTES {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!(
                "message too large ({} bytes) — max {} bytes of text",
                msg.message_bytes(),
                send::MAX_MESSAGE_BYTES
            ),
        );
    }
    dispatch_and_respond(&project_id, &path, &agent_name, &account_id, &msg, gate, wait_timeout)
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
    let (path, project_id) = match resolve_caller(project) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Gate first: off refuses BEFORE any engine dial.
    let gate = match send::effective_gate(Ok(
        k2_core::workspace::settings::mail_agent_send_for_path(&path),
    )) {
        Ok(g) => g,
        Err(e) => return send_error_response(e),
    };
    if gate == Gate::Off {
        return send_error_response(SendError::Gated(
            "outbound email is disabled for this workspace. Your human can enable it in \
             Settings → Email → Sending"
                .to_string(),
        ));
    }

    // §8.4: reply only to a message received at an OWNED address — the
    // address rides inside the opaque id (S4) and re-passes the masked
    // ownership gate here.
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
    // S11: replying IS sending — the receiving address must be one this
    // workspace can SEND from (hosted + effective 'send'). A read/draft
    // grant (or a linked inbox) can't reply — teaching 403 when they can
    // at least read it, masked not_found otherwise.
    let (from_address, account_id) = match access::can_send(&project_id, &address) {
        Ok(inbox) => match inbox.account_id {
            Some(account_id) => (inbox.address, account_id),
            None => return masked(),
        },
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

    // Fetch the original's reply context (sender, threading headers,
    // auth verdicts) from its backend.
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
    if let Err(e) = send::check_domain_send_mode(&FileSecretStore::default(), &from_address) {
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
    dispatch_and_respond(&project_id, &path, &agent_name, &account_id, &msg, gate, wait_timeout)
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
                serde_json::json!({ "project": "/tmp/x", "to": "a@b.c", "subject": "s",
                                    "body": "b", "attach": ["f.pdf"] }),
                "attachments are not supported",
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
}
