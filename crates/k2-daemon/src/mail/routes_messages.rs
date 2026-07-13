//! `/cli/mail/messages|read|attachments|wait` — READ-concern handlers
//! (mail slice S4: the RFC 8621 read proxy + the long-poll `wait`
//! verification-code primitive).
//!
//! Dispatched by the `crate::mail_routes` shim (GETs, `token_ok`); the
//! dispatcher runs these four paths in `spawn_blocking` because they
//! dial Stalwart over blocking reqwest and `wait` holds the request
//! open for up to 900 s. AUTH/GATING contract (PRD §8.1): reads are
//! always allowed for the OWNING workspace — the caller's `project`
//! resolves server-side to `owner_project_id` (never trusted from
//! params) and every explicitly named address/message passes
//! [`messages::owned_active_address`], whose failure is S3's MASKED
//! `not_found` (a workspace never learns what exists outside it).
//!
//! §17.5 seam: each address resolves its backend through
//! [`messages::backend_for_address`] — local Stalwart via
//! `domains::engine_from_db` (503 `not_ready` when the server isn't
//! up), or (S9) the user's OWN external IMAP inbox via
//! [`external::ExternalImapBackend`] when the address has a
//! `mail_external_inboxes` row bound to the calling workspace.
//! Ownership masking, §8.1 markers, and never-log-bodies apply
//! IDENTICALLY to both — external content flows through the same
//! handlers below, no duplication. No handler talks to a protocol
//! client directly.
//!
//! UNTRUSTED-CONTENT contract (§8.1): bodies come back wrapped in the
//! exact BEGIN/END EXTERNAL EMAIL markers (applied in
//! [`messages::shape_full_json`], NOT in the JMAP client). Attachments
//! are never auto-opened — `attachments --get` writes bytes to a path
//! resolved INSIDE the calling workspace ([`messages::
//! resolve_out_path`], §10) and nothing more. Bodies are never logged
//! (pre-mortem #16).
//!
//! `wait` (§8.2 BINDING): filters to/from/subject, timeout default
//! 300 s / max 900 s, match-from = call time − 60 s grace, poll every
//! 2 s inside the held request; match → the FULL message in `read`
//! format, marked read; timeout → `{ok:true, timedOut:true}` 200 (the
//! CLI maps it to exit 2). Per-workspace concurrent-wait cap wired
//! from day one (pre-mortem #10).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

use crate::cli_response::CliResponse;
use crate::mail::messages::{
    self, ListError, MailBackend, MailFolder, ManageBackend, ReadBackend, ReadError, WatchAddress,
};
use crate::mail::{access, domains, external, external_imap, graph};

// ── Response helpers (the S2/S3 error contract) ─────────────────────────

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

fn read_error_response(err: ReadError) -> CliResponse {
    match err {
        ReadError::Usage(hint) => error_response("400 Bad Request", "usage", &hint),
        ReadError::NotFound(hint) => error_response("404 Not Found", "not_found", &hint),
        ReadError::Engine(hint) => error_response("502 Bad Gateway", "engine", &hint),
    }
}

/// Map a list failure onto the wire contract: an engine/transport fault
/// is a 502 `engine`; a mistyped `--folder`/`--junk` is a 400 `usage`
/// that TEACHES with the folders that actually exist. Both backends
/// classify identically (§17.5 uniform seam).
fn list_error_response(err: ListError) -> CliResponse {
    match err {
        ListError::Engine(hint) => error_response("502 Bad Gateway", "engine", &hint),
        ListError::UnknownFolder { requested, available } => {
            let list = if available.is_empty() {
                "(none visible)".to_string()
            } else {
                available.join(", ")
            };
            error_response(
                "400 Bad Request",
                "usage",
                &format!("no folder '{requested}' here — available: {list}"),
            )
        }
    }
}

/// #31.4: fold a per-inbox failure into a compact `{address, code,
/// hint}` entry for the all-inbox sweep. `code`/`hint` are lifted from
/// the SAME error body a single-address call would return, so nothing
/// new is exposed — and every address that reaches this loop is one the
/// caller can already read, so naming it leaks no existence (masking is
/// enforced upstream in `watch_list`, never here).
fn inbox_error_json(address: &str, resp: &CliResponse) -> serde_json::Value {
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
    let code = parsed["error"]["code"].as_str().unwrap_or("engine");
    let hint = parsed["error"]["hint"].as_str().unwrap_or("");
    serde_json::json!({ "address": address, "code": code, "hint": hint })
}

/// Read-page defaults: a sane default page and a hard per-page ceiling
/// (an agent walks beyond it with the `offset` cursor, never a giant
/// single fetch).
const DEFAULT_PAGE: usize = 25;
const MAX_PAGE: usize = 200;

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// Resolve the caller's workspace to `(path, project_id)` — the
/// server-side identity step every workspace-scoped route uses
/// (mirrors routes_addresses; identity from the resolved workspace,
/// never from raw params).
fn resolve_caller(project: &str) -> Result<(String, String), CliResponse> {
    // Wave 0: prefers scoped principal over client project= claim.
    crate::mail::identity::resolve_caller(project)
}

/// Resolve the app-password for a LINKED inbox from the vault, mapping
/// the two failure shapes onto detail-free 503 `not_ready`s. An
/// OAuth-IMAP row (Gmail XOAUTH2) needs no app-password here — `login()`
/// mints the access token itself and IGNORES this param — so it resolves
/// to an empty string. The vault's own `Err` text can carry a filesystem
/// path / keychain detail, so it is NEVER forwarded to the caller (M3):
/// the read-failure case reuses the same generic reconnect guidance as
/// the missing-credential case. The one seam both `backend_for` and
/// `manage_backend_for` share.
fn resolve_linked_password(
    secrets: &dyn crate::mail::secrets::SecretStore,
    row: &k2_core::db::schema::MailExternalInbox,
    address: &str,
) -> Result<String, CliResponse> {
    let is_oauth = matches!(
        external::read_oauth_fields(&row.id),
        Ok(f) if f.auth_kind == external::AUTH_OAUTH
    );
    if is_oauth {
        return Ok(String::new());
    }
    match secrets.resolve(&external::vault_key(&row.id)) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(error_response(
            "503 Service Unavailable",
            "not_ready",
            &format!(
                "credentials for '{address}' are missing from the vault — your \
                 human can reconnect it with 'k2 mail link add'"
            ),
        )),
        Err(_) => Err(error_response(
            "503 Service Unavailable",
            "not_ready",
            &format!(
                "credentials for '{address}' could not be read from the vault — \
                 your human can reconnect it with 'k2 mail link add'"
            ),
        )),
    }
}

/// Client `from` filter for messages/wait — NOT the identity stamp.
///
/// Wave 0 / dual-auth `stamp_principal` writes the agent address into
/// `from=`. Mail list/wait treat `from` as an IMAP/JMAP From: substring
/// filter. Under a scoped passport that became `FROM "<workspace-uuid>"`
/// and always returned empty while folder list (no filter) still worked
/// (#38 agent AKZM retest). Dispatcher restores client --from after stamp;
/// this belt also drops a remaining stamp when `principal_bound` is set
/// and `from` equals the stamped `project_id` (common agent_address shape).
fn mail_from_filter(params: &HashMap<String, String>) -> Option<String> {
    let from = crate::cli::opt_param(params, "from")?;
    if params.get("principal_bound").map(|s| s == "1").unwrap_or(false) {
        if let Some(pid) = params.get("project_id") {
            if from == *pid {
                return None;
            }
        }
    }
    Some(from)
}

/// §17.5: resolve one address's backend and construct it — the ONLY
/// place a backend is instantiated. `LocalStalwart` → the production
/// JMAP client from the `mail_server` row (503 `not_ready` when
/// absent/stopped); `ExternalImap` (S9) → the address's
/// `mail_external_inboxes` row + its vault credential (503 when the
/// credential is gone — re-add fixes it). Callers only ever see
/// `dyn ReadBackend`.
fn backend_for(address: &str) -> Result<Box<dyn ReadBackend>, CliResponse> {
    match messages::backend_for_address(address) {
        MailBackend::LocalStalwart => {
            let (client, _hostname) = domains::engine_from_db().map_err(|hint| {
                error_response("503 Service Unavailable", "not_ready", &hint)
            })?;
            Ok(Box::new(messages::StalwartReadBackend::new(client)))
        }
        MailBackend::ExternalImap => {
            // A remove can race the seam answer — mask, like any
            // unknown address.
            let Some(row) = external::inbox_for_address(address) else {
                return Err(error_response(
                    "404 Not Found",
                    "not_found",
                    &format!("no address '{address}' in this workspace"),
                ));
            };
            let secrets = crate::mail::secrets::FileSecretStore::default();
            let password = resolve_linked_password(&secrets, &row, address)?;
            Ok(Box::new(external::ExternalImapBackend::new(
                row,
                password,
                std::sync::Arc::new(external_imap::RealImapOps),
            )))
        }
        MailBackend::Graph => {
            // Microsoft Graph (O3): no vault password to resolve — the
            // Bearer token is minted per-request INSIDE RealGraphHttp via
            // the O1 engine, so the seam here only needs the row.
            let Some(row) = external::inbox_for_address(address) else {
                return Err(error_response(
                    "404 Not Found",
                    "not_found",
                    &format!("no address '{address}' in this workspace"),
                ));
            };
            let http = std::sync::Arc::new(graph::RealGraphHttp::new(row.id.clone()));
            Ok(Box::new(graph::GraphBackend::new(row, http)))
        }
    }
}

/// The (address, account) watch list for a request: an explicit
/// address must pass the masked ownership gate; no address = every
/// ACTIVE address the workspace owns PLUS (S9) every external inbox
/// bound to it — external handles "just work" in messages/read/wait,
/// no new flags. Rows minted in a degraded state (no Stalwart account)
/// fail loudly when explicitly targeted and are skipped in the
/// all-addresses sweep (doc'd on `mail::addresses`).
fn watch_list(
    project_id: &str,
    explicit: Option<&str>,
) -> Result<Vec<WatchAddress>, CliResponse> {
    if let Some(addr) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        // S11: ONE read gate over hosted + linked. A workspace reads an
        // inbox when it is the PRIMARY or holds a read+ grant; anything
        // else answers the SAME masked not_found (no existence leak,
        // and the failure never reveals which world the address lives
        // in). `account_id` is the backend handle (hosted = Stalwart
        // account; linked = the external row id).
        return match crate::mail::access::can_read(project_id, addr) {
            Ok(inbox) => {
                let Some(account_id) = inbox.account_id else {
                    return Err(error_response(
                        "502 Bad Gateway",
                        "engine",
                        &format!("address '{}' has no mailbox on the mail server", inbox.address),
                    ));
                };
                Ok(vec![WatchAddress { address: inbox.address, account_id }])
            }
            Err(e @ ReadError::Usage(_)) => Err(read_error_response(e)),
            Err(e) => Err(read_error_response(e)),
        };
    }
    // No explicit address: every inbox the workspace can READ — owned OR
    // granted, across BOTH sources (S11 unified sweep).
    let mut watch: Vec<WatchAddress> = crate::mail::access::readable_hosted(project_id)
        .into_iter()
        .map(|(address, account_id)| WatchAddress { address, account_id })
        .collect();
    watch.extend(
        crate::mail::access::readable_linked(project_id)
            .into_iter()
            .map(|(address, inbox_id)| WatchAddress { address, account_id: inbox_id }),
    );
    Ok(watch)
}

/// Decode + ownership-gate an opaque message id: the address inside
/// the token must be an ACTIVE address of the CALLING workspace — or
/// (S9) an external inbox BOUND to it (the backend handle is then the
/// external row id). Every mismatch answers the same masked
/// message-level not_found.
fn owned_message(
    project_id: &str,
    token: &str,
) -> Result<(String, String, String), CliResponse> {
    let Some((address, email_id)) = messages::decode_message_id(token) else {
        return Err(error_response(
            "400 Bad Request",
            "usage",
            "invalid message id — use an id from 'k2 mail messages'",
        ));
    };
    let masked = || {
        error_response(
            "404 Not Found",
            "not_found",
            &format!("no message '{token}' in this workspace"),
        )
    };
    // S11 unified read gate (hosted + linked): the address in the token
    // must be one the caller can READ. Every mismatch answers the same
    // masked message-level not_found.
    match crate::mail::access::can_read(project_id, &address) {
        Ok(inbox) => {
            let Some(account_id) = inbox.account_id else {
                return Err(masked());
            };
            Ok((inbox.address, account_id, email_id))
        }
        Err(ReadError::Usage(hint)) => Err(error_response("400 Bad Request", "usage", &hint)),
        Err(_) => Err(masked()),
    }
}

// ── GET /cli/mail/messages ──────────────────────────────────────────────

/// Newest-first summaries for owned addresses (`k2 mail messages`).
/// Params: `project` (required) · `address` (optional; default = all
/// the caller's addresses) · `unread` · `limit` (default 25, max 200)
/// · `offset` (pagination cursor; response carries `nextOffset` when a
/// page is full) · `query` (subject AND body) · `from` (From header
/// address + display name, §11.1.8) · `since`/`before` (date window:
/// YYYY-MM-DD or a relative age like 7d/24h/30m) · `folder` (a mailbox
/// other than the Inbox) / `junk` (the Junk/Spam folder). Summaries
/// only — no bodies ride this route.
pub fn handle_messages(params: &HashMap<String, String>) -> CliResponse {
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
        None => DEFAULT_PAGE,
        Some(Ok(n)) if n >= 1 => n.min(MAX_PAGE), // clamp to the per-page ceiling
        _ => {
            return error_response(
                "400 Bad Request",
                "usage",
                &format!("invalid 'limit' — a number from 1 to {MAX_PAGE} (default {DEFAULT_PAGE})"),
            )
        }
    };
    // Pagination cursor (§ triage): 0-based offset into the newest-first
    // list — an agent walks a large mailbox by re-calling with the
    // `nextOffset` we return.
    let offset: usize = match params.get("offset").map(|v| v.parse::<usize>()) {
        None => 0,
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            return error_response(
                "400 Bad Request",
                "usage",
                "invalid 'offset' — a whole number (0 = first page; use the nextOffset we return)",
            )
        }
    };
    // Date window: `since`/`before` accept YYYY-MM-DD or a relative age
    // (7d/24h/30m). These are the read filter's OWN window — distinct
    // from the wait grace boundary.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let after_unix = match params.get("since").map(|s| messages::parse_date_bound(s, now)) {
        None => None,
        Some(Ok(t)) => Some(t),
        Some(Err(hint)) => return error_response("400 Bad Request", "usage", &hint),
    };
    let before_unix = match params.get("before").map(|s| messages::parse_date_bound(s, now)) {
        None => None,
        Some(Ok(t)) => Some(t),
        Some(Err(hint)) => return error_response("400 Bad Request", "usage", &hint),
    };
    // Folder selection: `--junk` convenience OR an explicit `--folder`
    // (never both). Default = the Inbox.
    let junk = crate::cli::bool_param(params, "junk");
    let named = crate::cli::opt_param(params, "folder");
    let folder = match (junk, named) {
        (true, Some(_)) => {
            return error_response(
                "400 Bad Request",
                "usage",
                "use either --junk or --folder <name>, not both",
            )
        }
        (true, None) => messages::MailFolder::Junk,
        (false, Some(name)) => messages::MailFolder::Named(name),
        (false, None) => messages::MailFolder::Inbox,
    };
    // Did the caller name ONE explicit address? That path surfaces an
    // error directly; the all-inbox sweep (no address) degrades per
    // inbox instead (#31.4).
    let explicit_address = params
        .get("address")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .is_some();
    let watch = match watch_list(&project_id, params.get("address").map(String::as_str)) {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if watch.is_empty() {
        // Nothing minted yet — an empty inbox, not an error (and no
        // engine dial: readable even while the server is down).
        return ok_json(serde_json::json!({ "ok": true, "messages": [], "count": 0 }));
    }
    let filter = messages::ListFilter {
        unread_only: crate::cli::bool_param(params, "unread"),
        query: crate::cli::opt_param(params, "query"),
        // #38: never treat identity stamp `from=` as a From: filter.
        from: mail_from_filter(params),
        since_unix: None,
        after_unix,
        before_unix,
        folder,
        offset,
    };
    let mut shaped: Vec<(i64, serde_json::Value)> = Vec::new();
    // A backend that returns a FULL page may have more behind it — the
    // cursor's "there is a next page" signal (exact for one inbox; a
    // safe over-estimate when the sweep merges several).
    let mut maybe_more = false;
    // #31.4: the all-inbox sweep DEGRADES per inbox — one inbox's engine
    // fault, credential expiry, or auth blip must not sink the whole read.
    // A single EXPLICIT address still surfaces its error directly (the
    // `explicit_address` early-returns below preserve S3 behavior).
    let mut inbox_errors: Vec<serde_json::Value> = Vec::new();
    // #38: track whether ANY backend list completed (Ok) so a total
    // failure cannot look like a successful empty inbox.
    let mut lists_ok: usize = 0;
    for w in &watch {
        let backend = match backend_for(&w.address) {
            Ok(b) => b,
            Err(resp) => {
                if explicit_address {
                    return resp;
                }
                inbox_errors.push(inbox_error_json(&w.address, &resp));
                continue;
            }
        };
        let summaries = match backend.list_inbox(&w.account_id, &filter, limit) {
            Ok(s) => {
                lists_ok += 1;
                s
            }
            Err(e) => {
                if explicit_address {
                    return list_error_response(e);
                }
                inbox_errors.push(inbox_error_json(&w.address, &list_error_response(e)));
                continue;
            }
        };
        if summaries.len() >= limit {
            maybe_more = true;
        }
        for s in &summaries {
            shaped.push((
                messages::iso_to_unix(&s.received_at),
                messages::shape_summary_json(&w.address, s),
            ));
        }
    }
    // #38: if every watched inbox errored (no successful list), surface
    // a hard error — never `{count:0, ok:true}` for "all backends failed".
    // Agents branch on exit code; silent empty teaches the wrong fix.
    if lists_ok == 0 && !inbox_errors.is_empty() {
        let first = inbox_errors.first().cloned().unwrap_or_else(|| {
            serde_json::json!({"code": "engine", "hint": "all inboxes failed to list"})
        });
        let code = first
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or("engine");
        let hint = first
            .get("hint")
            .and_then(|h| h.as_str())
            .unwrap_or("all inboxes failed to list messages");
        let status = if code == "not_ready" {
            "503 Service Unavailable"
        } else {
            "502 Bad Gateway"
        };
        return CliResponse {
            status,
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "error": {
                    "code": code,
                    "hint": format!(
                        "{hint} ({} inbox(es) failed; none returned a list)",
                        inbox_errors.len()
                    ),
                },
                "inboxErrors": inbox_errors,
            })
            .to_string(),
        };
    }
    // Merge across addresses newest-first, then apply the limit.
    shaped.sort_by(|a, b| b.0.cmp(&a.0));
    shaped.truncate(limit);
    let list: Vec<serde_json::Value> = shaped.into_iter().map(|(_, v)| v).collect();
    let mut body = serde_json::json!({ "ok": true, "count": list.len(), "messages": list });
    if maybe_more {
        // The next page cursor — the CLI prints a "more:" hint from it.
        body["nextOffset"] = serde_json::json!(offset + limit);
    }
    if !inbox_errors.is_empty() {
        // Partial success: the healthy inboxes' messages rode above; the
        // ones that failed are named here (address + code + hint) so the
        // caller can act without the whole call 5xx-ing.
        body["inboxErrors"] = serde_json::json!(inbox_errors);
    }
    ok_json(body)
}

// ── GET /cli/mail/read ──────────────────────────────────────────────────

/// One full message by id (§8.1 shape, §11.1.6: marks read). Params:
/// `project` + `id` (required) · `html` (include the wrapped HTML
/// body) · `raw` (the RFC 822 blob, base64 — still marks read).
pub fn handle_read(params: &HashMap<String, String>) -> CliResponse {
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
    let Some(token) = params.get("id").map(String::as_str).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — a message id from 'k2 mail messages'",
        );
    };
    let (_path, project_id) = match resolve_caller(&project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id, email_id) = match owned_message(&project_id, token) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match backend_for(&address) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let full = match backend.fetch_full(&account_id, &email_id) {
        Ok(Some(f)) => f,
        // The backend not knowing the id answers the same masked
        // not_found as a foreign id.
        Ok(None) => {
            return error_response(
                "404 Not Found",
                "not_found",
                &format!("no message '{token}' in this workspace"),
            )
        }
        Err(hint) => return read_error_response(ReadError::Engine(hint)),
    };
    if let Err(hint) = backend.mark_seen(&account_id, &email_id) {
        return read_error_response(ReadError::Engine(hint));
    }
    if crate::cli::bool_param(params, "raw") {
        let Some(blob_id) = full.blob_id.as_deref() else {
            return error_response(
                "502 Bad Gateway",
                "engine",
                "the server exposed no raw blob for this message",
            );
        };
        let bytes = match backend.fetch_blob(&account_id, blob_id, "message.eml", "message/rfc822")
        {
            Ok(b) => b,
            Err(hint) => return read_error_response(ReadError::Engine(hint)),
        };
        return ok_json(serde_json::json!({
            "ok": true,
            "id": token,
            "rawBase64": B64.encode(&bytes),
        }));
    }
    let include_html = crate::cli::bool_param(params, "html");
    ok_json(serde_json::json!({
        "ok": true,
        "message": messages::shape_full_json(&address, &full, include_html),
    }))
}

// ── GET /cli/mail/attachments ───────────────────────────────────────────

/// Attachment size ceiling for `--get` (matches the fs layer's 100 MB
/// single-shot cap — the daemon never buffers more than this for one
/// attachment).
const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;

/// List a message's attachments, or (`?get=<n>&out=<path>`) fetch one.
/// `get` is 1-BASED (§11.1.11). SECURITY (§10): `out` resolves inside
/// the calling workspace's root — absolute paths, traversal, and
/// symlink escapes are refused; the daemon writes the bytes and does
/// NOTHING else with them. Listing does not mark the message read.
pub fn handle_attachments(params: &HashMap<String, String>) -> CliResponse {
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
    let Some(token) = params.get("id").map(String::as_str).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'id' — a message id from 'k2 mail messages'",
        );
    };
    // Validate the get/out pair BEFORE any identity/engine work.
    let get_n: Option<usize> = match params.get("get").map(|v| v.parse::<usize>()) {
        None => None,
        Some(Ok(n)) if n >= 1 => Some(n),
        _ => {
            return error_response(
                "400 Bad Request",
                "usage",
                "invalid 'get' — the 1-based attachment number from the list",
            )
        }
    };
    let out = crate::cli::opt_param(params, "out");
    if get_n.is_some() && out.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'out' — the workspace-relative path to write the attachment to",
        );
    }
    let (path, project_id) = match resolve_caller(&project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id, email_id) = match owned_message(&project_id, token) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // Path shape is refused before the engine dials (cheap + testable).
    let target = match get_n {
        Some(_) => match messages::resolve_out_path(&path, out.as_deref().unwrap_or("")) {
            Ok(t) => Some(t),
            Err(hint) => return error_response("400 Bad Request", "usage", &hint),
        },
        None => None,
    };
    let backend = match backend_for(&address) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let full = match backend.fetch_full(&account_id, &email_id) {
        Ok(Some(f)) => f,
        Ok(None) => {
            return error_response(
                "404 Not Found",
                "not_found",
                &format!("no message '{token}' in this workspace"),
            )
        }
        Err(hint) => return read_error_response(ReadError::Engine(hint)),
    };
    let Some(n) = get_n else {
        let list: Vec<serde_json::Value> = full
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| messages::attachment_json(i, a))
            .collect();
        return ok_json(serde_json::json!({
            "ok": true,
            "id": token,
            "count": list.len(),
            "attachments": list,
        }));
    };
    let Some(meta) = full.attachments.get(n - 1) else {
        return error_response(
            "404 Not Found",
            "not_found",
            &format!(
                "no attachment #{n} — this message has {} (1-based)",
                full.attachments.len()
            ),
        );
    };
    if meta.size > MAX_ATTACHMENT_BYTES {
        return error_response(
            "400 Bad Request",
            "usage",
            &format!("attachment #{n} is larger than the 100 MB fetch cap"),
        );
    }
    let name = meta.filename.clone().unwrap_or_else(|| format!("attachment-{n}"));
    let bytes = match backend.fetch_blob(&account_id, &meta.blob_id, &name, &meta.mime) {
        Ok(b) => b,
        Err(hint) => return read_error_response(ReadError::Engine(hint)),
    };
    let target = target.expect("resolved above when get is set");
    if let Err(e) = std::fs::write(&target, &bytes) {
        return error_response("502 Bad Gateway", "engine", &format!("write attachment: {e}"));
    }
    ok_json(serde_json::json!({
        "ok": true,
        "id": token,
        "index": n,
        "filename": name,
        "mime": meta.mime,
        "bytes": bytes.len(),
        "wrote": target.to_string_lossy(),
    }))
}

// ── GET /cli/mail/wait ──────────────────────────────────────────────────

/// Max concurrent `wait` long-polls per workspace (pre-mortem #10:
/// per-agent limits wired from day one — an agent loops ONE wait, it
/// never fans out dozens).
const MAX_CONCURRENT_WAITS: usize = 4;

fn active_waits() -> &'static Mutex<HashMap<String, usize>> {
    static WAITS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    WAITS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// RAII slot in the per-workspace concurrent-wait counter.
pub(crate) struct WaitSlot {
    project_id: String,
}

impl WaitSlot {
    pub(crate) fn try_acquire(project_id: &str) -> Option<Self> {
        let mut map = active_waits().lock().unwrap_or_else(|p| p.into_inner());
        let n = map.entry(project_id.to_string()).or_insert(0);
        if *n >= MAX_CONCURRENT_WAITS {
            return None;
        }
        *n += 1;
        Some(Self { project_id: project_id.to_string() })
    }
}

impl Drop for WaitSlot {
    fn drop(&mut self) {
        let mut map = active_waits().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = map.get_mut(&self.project_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&self.project_id);
            }
        }
    }
}

/// Long-poll for a matching incoming message (§8.2). Params: `project`
/// (required) · `to` (an owned address; default all) · `from`
/// (substring, address + display name) · `subject` (substring) ·
/// `timeout` (secs, default 300, max 900). Match → 200
/// `{ok, timedOut:false, message}` with the FULL message in read
/// format, marked read (§11.1.6); timeout → 200 `{ok, timedOut:true}`
/// (CLI exit 2). The request is HELD — the dispatcher runs this in
/// `spawn_blocking`.
pub fn handle_wait(params: &HashMap<String, String>) -> CliResponse {
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
    let timeout_secs: u64 = match params.get("timeout").map(|v| v.parse::<u64>()) {
        None => messages::WAIT_DEFAULT_TIMEOUT_SECS,
        Some(Ok(n)) if (1..=messages::WAIT_MAX_TIMEOUT_SECS).contains(&n) => n,
        Some(Ok(n)) if n > messages::WAIT_MAX_TIMEOUT_SECS => {
            return error_response(
                "400 Bad Request",
                "usage",
                &format!(
                    "timeout is capped at {} s per call — loop wait calls for longer \
                     (the blessed pattern, §11.1.6)",
                    messages::WAIT_MAX_TIMEOUT_SECS
                ),
            )
        }
        _ => {
            return error_response(
                "400 Bad Request",
                "usage",
                "invalid 'timeout' — seconds from 1 to 900 (default 300)",
            )
        }
    };
    let (_path, project_id) = match resolve_caller(&project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let watch = match watch_list(&project_id, params.get("to").map(String::as_str)) {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if watch.is_empty() {
        return error_response(
            "404 Not Found",
            "not_found",
            "this workspace has no active mail addresses — mint one with 'k2 mail create' \
             before waiting",
        );
    }
    let Some(_slot) = WaitSlot::try_acquire(&project_id) else {
        return error_response(
            "429 Too Many Requests",
            "rate_limited",
            &format!(
                "this workspace already has {MAX_CONCURRENT_WAITS} wait calls open — \
                 loop ONE 'k2 mail wait', don't fan out"
            ),
        );
    };
    // §17.5: EVERY watched address resolves its own backend through
    // the seam — since S9 a workspace can watch local Stalwart
    // addresses and external IMAP inboxes in the same wait.
    let mut backends: Vec<Box<dyn ReadBackend>> = Vec::with_capacity(watch.len());
    for w in &watch {
        match backend_for(&w.address) {
            Ok(b) => backends.push(b),
            Err(resp) => return resp,
        }
    }
    let pairs: Vec<(&WatchAddress, &dyn ReadBackend)> = watch
        .iter()
        .zip(backends.iter().map(|b| b.as_ref()))
        .collect();
    let filters = messages::WaitFilters {
        // #38: same stamp-vs-filter collision as messages.
        from: mail_from_filter(params),
        subject: crate::cli::opt_param(params, "subject"),
    };
    let mut now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    };
    let mut sleep = |d: std::time::Duration| std::thread::sleep(d);
    match messages::wait_for_match(
        &pairs,
        &filters,
        timeout_secs,
        messages::WAIT_GRACE_SECS,
        messages::WAIT_POLL_SECS,
        &mut now,
        &mut sleep,
    ) {
        Ok(Some(message)) => ok_json(serde_json::json!({
            "ok": true,
            "timedOut": false,
            "message": message,
        })),
        Ok(None) => ok_json(serde_json::json!({ "ok": true, "timedOut": true })),
        Err(hint) => read_error_response(ReadError::Engine(hint)),
    }
}

// ── S11 MANAGEMENT + DELETE (move / flag / archive / delete / folders) ──
//
// These are workspace-token POSTs (agent verbs, like `draft`), gated by
// the ORTHOGONAL can_manage / can_delete caps (0081) — INDEPENDENT of the
// read/draft/send level. Every deny masks the SAME `not_found` a foreign
// address gets. DELETE is ALWAYS a MOVE to Trash — never an EXPUNGE.

/// §17.5 seam for MANAGEMENT: the same backends as reads, but exposing
/// the [`ManageBackend`] surface. `LocalStalwart` → the JMAP manage
/// backend; `ExternalImap` → the linked IMAP inbox + its vault
/// credential (mirrors [`backend_for`]).
fn manage_backend_for(address: &str) -> Result<Box<dyn ManageBackend>, CliResponse> {
    match messages::backend_for_address(address) {
        MailBackend::LocalStalwart => {
            let (client, _hostname) = domains::engine_from_db().map_err(|hint| {
                error_response("503 Service Unavailable", "not_ready", &hint)
            })?;
            Ok(Box::new(messages::StalwartManageBackend::new(client)))
        }
        MailBackend::ExternalImap => {
            let Some(row) = external::inbox_for_address(address) else {
                return Err(error_response(
                    "404 Not Found",
                    "not_found",
                    &format!("no address '{address}' in this workspace"),
                ));
            };
            let secrets = crate::mail::secrets::FileSecretStore::default();
            let password = resolve_linked_password(&secrets, &row, address)?;
            Ok(Box::new(external::ExternalImapBackend::new(
                row,
                password,
                std::sync::Arc::new(external_imap::RealImapOps),
            )))
        }
        MailBackend::Graph => {
            // Microsoft Graph manage surface (move/flag/archive/delete-to-
            // DeletedItems/folder) — same row-only construction as reads;
            // the token is minted inside RealGraphHttp.
            let Some(row) = external::inbox_for_address(address) else {
                return Err(error_response(
                    "404 Not Found",
                    "not_found",
                    &format!("no address '{address}' in this workspace"),
                ));
            };
            let http = std::sync::Arc::new(graph::RealGraphHttp::new(row.id.clone()));
            Ok(Box::new(graph::GraphBackend::new(row, http)))
        }
    }
}

/// Decode + MANAGE/DELETE-gate a message id → `(address, account_id,
/// email_id)`. `need_delete` picks the `can_delete` gate (whose
/// manage-but-no-delete deny is a TEACHING usage error, not masked —
/// the workspace already sees the inbox); otherwise `can_manage`. Every
/// other deny answers the masked message-level not_found.
fn managed_message(
    project_id: &str,
    token: &str,
    need_delete: bool,
) -> Result<(String, String, String), CliResponse> {
    let Some((address, email_id)) = messages::decode_message_id(token) else {
        return Err(error_response(
            "400 Bad Request",
            "usage",
            "invalid message id — use an id from 'k2 mail messages'",
        ));
    };
    let masked = || {
        error_response(
            "404 Not Found",
            "not_found",
            &format!("no message '{token}' in this workspace"),
        )
    };
    let gate = if need_delete {
        access::can_delete(project_id, &address)
    } else {
        access::can_manage(project_id, &address)
    };
    match gate {
        Ok(inbox) => {
            let Some(account_id) = inbox.account_id else {
                return Err(masked());
            };
            Ok((inbox.address, account_id, email_id))
        }
        // can_delete's manage-but-no-delete teaching error surfaces as a
        // usage 400 (the caller can already see the inbox — nothing leaks).
        Err(ReadError::Usage(hint)) => Err(error_response("400 Bad Request", "usage", &hint)),
        Err(_) => Err(masked()),
    }
}

/// MANAGE-gate an inbox by ADDRESS (folder ops) → `(address,
/// account_id)`. Deny → masked not_found; malformed address → usage.
fn managed_inbox(project_id: &str, raw_address: &str) -> Result<(String, String), CliResponse> {
    match access::can_manage(project_id, raw_address) {
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
        Err(ReadError::Usage(hint)) => Err(error_response("400 Bad Request", "usage", &hint)),
        Err(_) => Err(error_response(
            "404 Not Found",
            "not_found",
            &format!("no address '{raw_address}' in this workspace"),
        )),
    }
}

/// A destination folder string → [`MailFolder`] (Inbox/Junk/Named).
fn parse_dest_folder(name: &str) -> MailFolder {
    let t = name.trim();
    if t.eq_ignore_ascii_case("inbox") {
        MailFolder::Inbox
    } else if t.eq_ignore_ascii_case("junk") || t.eq_ignore_ascii_case("spam") {
        MailFolder::Junk
    } else {
        MailFolder::Named(t.to_string())
    }
}

fn need_project_body(project: &str) -> Result<(String, String), CliResponse> {
    if project.trim().is_empty() {
        return Err(error_response(
            "400 Bad Request",
            "usage",
            "missing 'project' (workspace name | path | UUID)",
        ));
    }
    resolve_caller(project)
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct MoveBody {
    project: String,
    id: String,
    folder: String,
}

/// POST `/cli/mail/move` — move a message to a folder (can_manage).
pub fn handle_move(body: &[u8]) -> CliResponse {
    let b: MoveBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.id.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'id' — a message id from 'k2 mail messages'");
    }
    if b.folder.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'folder' — the destination folder name");
    }
    let (_path, project_id) = match need_project_body(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id, email_id) = match managed_message(&project_id, &b.id, false) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match manage_backend_for(&address) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    match backend.move_message(&account_id, &email_id, &parse_dest_folder(&b.folder)) {
        Ok(o) => ok_json(serde_json::json!({ "ok": true, "id": b.id, "folder": o.folder })),
        Err(e) => list_error_response(e),
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct FlagBody {
    project: String,
    id: String,
    read: Option<bool>,
    flagged: Option<bool>,
}

/// POST `/cli/mail/flag` — set/clear \Seen (read) / \Flagged
/// (flagged) on a message (can_manage). At least one of read/flagged.
pub fn handle_flag(body: &[u8]) -> CliResponse {
    let b: FlagBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.id.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'id' — a message id from 'k2 mail messages'");
    }
    if b.read.is_none() && b.flagged.is_none() {
        return error_response(
            "400 Bad Request",
            "usage",
            "nothing to change — pass at least one of read (--read/--unread) or flagged (--star/--unstar)",
        );
    }
    let (_path, project_id) = match need_project_body(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id, email_id) = match managed_message(&project_id, &b.id, false) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match manage_backend_for(&address) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    match backend.set_flags(&account_id, &email_id, b.read, b.flagged) {
        Ok(()) => ok_json(serde_json::json!({
            "ok": true, "id": b.id, "read": b.read, "flagged": b.flagged,
        })),
        Err(hint) => read_error_response(ReadError::Engine(hint)),
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct IdBody {
    project: String,
    id: String,
}

/// POST `/cli/mail/archive` — move a message to the Archive folder
/// (can_manage).
pub fn handle_archive(body: &[u8]) -> CliResponse {
    let b: IdBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.id.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'id' — a message id from 'k2 mail messages'");
    }
    let (_path, project_id) = match need_project_body(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id, email_id) = match managed_message(&project_id, &b.id, false) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match manage_backend_for(&address) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    match backend.archive_message(&account_id, &email_id) {
        Ok(o) => ok_json(serde_json::json!({ "ok": true, "id": b.id, "folder": o.folder })),
        Err(e) => list_error_response(e),
    }
}

/// POST `/cli/mail/delete` — DELETE a message = move it to Trash
/// (can_delete; NEVER a permanent expunge).
pub fn handle_delete(body: &[u8]) -> CliResponse {
    let b: IdBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.id.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'id' — a message id from 'k2 mail messages'");
    }
    let (_path, project_id) = match need_project_body(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id, email_id) = match managed_message(&project_id, &b.id, true) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match manage_backend_for(&address) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    match backend.trash_message(&account_id, &email_id) {
        Ok(o) => ok_json(serde_json::json!({
            "ok": true, "id": b.id, "folder": o.folder, "trashed": true,
        })),
        Err(e) => list_error_response(e),
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct FolderCreateBody {
    project: String,
    address: String,
    name: String,
}

/// POST `/cli/mail/folder/create` — create a folder in an inbox
/// (can_manage).
pub fn handle_folder_create(body: &[u8]) -> CliResponse {
    let b: FolderCreateBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'address' — the inbox ('k2 mail inboxes')");
    }
    if b.name.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'name' — the folder to create");
    }
    let (_path, project_id) = match need_project_body(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id) = match managed_inbox(&project_id, &b.address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match manage_backend_for(&address) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    match backend.folder_create(&account_id, b.name.trim()) {
        Ok(()) => ok_json(serde_json::json!({
            "ok": true, "address": address, "folder": b.name.trim(), "created": true,
        })),
        Err(hint) => read_error_response(ReadError::Engine(hint)),
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct FolderRenameBody {
    project: String,
    address: String,
    from: String,
    to: String,
}

/// POST `/cli/mail/folder/rename` — rename a folder (can_manage).
pub fn handle_folder_rename(body: &[u8]) -> CliResponse {
    let b: FolderRenameBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.address.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'address' — the inbox ('k2 mail inboxes')");
    }
    if b.from.trim().is_empty() || b.to.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "rename needs 'from' and 'to' folder names");
    }
    let (_path, project_id) = match need_project_body(&b.project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id) = match managed_inbox(&project_id, &b.address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match manage_backend_for(&address) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    match backend.folder_rename(&account_id, b.from.trim(), b.to.trim()) {
        Ok(()) => ok_json(serde_json::json!({
            "ok": true, "address": address, "from": b.from.trim(), "to": b.to.trim(), "renamed": true,
        })),
        Err(e) => list_error_response(e),
    }
}

/// GET `/cli/mail/folder/list?project=&address=` — list an inbox's
/// folders (can_manage).
pub fn handle_folder_list(params: &HashMap<String, String>) -> CliResponse {
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
    let Some(address) = params.get("address").map(String::as_str).filter(|s| !s.is_empty()) else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'address' — the inbox to list folders for ('k2 mail inboxes')",
        );
    };
    let (_path, project_id) = match resolve_caller(&project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let (address, account_id) = match managed_inbox(&project_id, address) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let backend = match manage_backend_for(&address) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    match backend.folder_list(&account_id) {
        Ok(folders) => ok_json(serde_json::json!({
            "ok": true, "address": address, "count": folders.len(), "folders": folders,
        })),
        Err(hint) => read_error_response(ReadError::Engine(hint)),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — validation + gating + masking + the no-server
// 503, no network (house rules); deep read/wait behavior lives in
// mail::messages with fakes.
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn body_json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    /// Short names: they double as DOMAIN labels in seeded addresses
    /// (`mine@<name>.example`), and DNS labels cap at 63 chars.
    fn unique(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let short = &id[..12];
        (
            format!("mmr-{label}-{short}"),
            format!("/tmp/mail-msg-routes-{label}-{}-{short}", std::process::id()),
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
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at) VALUES (?1, ?2, 'dom-x', ?3, ?4, ?5, 100)",
            rusqlite::params![id, address, stalwart_account_id, project_id, status],
        )
        .expect("seed address");
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn mail_from_filter_drops_principal_stamp_collision() {
        // Identity stamp shape: principal_bound + from == project_id.
        let p = params(&[
            ("principal_bound", "1"),
            ("project_id", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
            ("from", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
        ]);
        assert_eq!(
            mail_from_filter(&p),
            None,
            "stamped agent uuid must not become IMAP From: filter"
        );
        // Real client --from survives.
        let p = params(&[
            ("principal_bound", "1"),
            ("project_id", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
            ("from", "boss@example.com"),
        ]);
        assert_eq!(
            mail_from_filter(&p).as_deref(),
            Some("boss@example.com")
        );
        // Owner path (no principal_bound): from is always the filter.
        let p = params(&[("from", "boss@example.com")]);
        assert_eq!(
            mail_from_filter(&p).as_deref(),
            Some("boss@example.com")
        );
    }

    /// These tests assert the no-server 503 — hold the singleton lock
    /// and clear the `mail_server` row so a concurrently seeded row
    /// can't turn the assertion into a live engine dial (house rule:
    /// no network in tests).
    fn clear_mail_server() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
    }

    // ── messages ──

    #[test]
    fn messages_validates_scopes_and_answers_before_any_engine_dial() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        // Missing project → usage.
        let resp = handle_messages(&HashMap::new());
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        // Unknown workspace → the shared 404 shape (identity resolves
        // before flag validation — a registered project is required to
        // exercise limit/usage checks).
        let resp = handle_messages(&params(&[("project", "no-such-workspace-xyz")]));
        assert_eq!(resp.status, "404 Not Found");

        let (name, path) = unique("messages");
        let project_id = insert_project(&name, &path);

        // Bad limit → usage (0 and non-numeric both die) on a registered ws.
        for bad in ["0", "abc"] {
            let resp = handle_messages(&params(&[("project", &path), ("limit", bad)]));
            assert_eq!(resp.status, "400 Bad Request", "limit={bad}");
        }

        // No addresses minted → an EMPTY inbox (200), never an error,
        // never an engine dial.
        let resp = handle_messages(&params(&[("project", &path)]));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["ok"], true);
        assert_eq!(v["count"], 0);

        // A FOREIGN address answers the masked not_found.
        let (name2, path2) = unique("messages-foreign");
        let project2 = insert_project(&name2, &path2);
        seed_address(&project2, &format!("theirs@{name2}.example"), "active", Some("acc-f"));
        let resp = handle_messages(&params(&[
            ("project", &path),
            ("address", &format!("theirs@{name2}.example")),
        ]));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");

        // A RETIRED own address answers the same masked not_found.
        seed_address(&project_id, &format!("gone@{name}.example"), "retired", Some("acc-r"));
        let resp = handle_messages(&params(&[
            ("project", &path),
            ("address", &format!("gone@{name}.example")),
        ]));
        assert_eq!(resp.status, "404 Not Found");

        // An owned ACTIVE address with no mail server → 503 not_ready
        // with the Settings pointer (the §17.5 seam's not-ready path).
        seed_address(&project_id, &format!("mine@{name}.example"), "active", Some("acc-1"));
        let resp = handle_messages(&params(&[
            ("project", &path),
            ("address", &format!("mine@{name}.example")),
        ]));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "not_ready");
        assert!(v["error"]["hint"].as_str().unwrap().contains("Settings → Email"), "{v}");

        cleanup_project(&project_id);
        cleanup_project(&project2);
    }

    /// #31.4 + #38: the ALL-INBOX sweep degrades per inbox when at least
    /// one list succeeds. When EVERY watched inbox fails, #38 hard-fails
    /// (never `{count:0, ok:true}` for total engine failure). An EXPLICIT
    /// single address still surfaces the error directly.
    #[test]
    fn all_inbox_sweep_degrades_per_inbox_instead_of_hard_failing() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        let (name, path) = unique("sweep");
        let project_id = insert_project(&name, &path);

        // Two owned ACTIVE hosted addresses. With no mail server up, each
        // one's backend is a 503 not_ready.
        let addr_a = format!("a@{name}.example");
        let addr_b = format!("b@{name}.example");
        seed_address(&project_id, &addr_a, "active", Some("acc-a"));
        seed_address(&project_id, &addr_b, "active", Some("acc-b"));

        // Single EXPLICIT address → the 503 surfaces DIRECTLY (the S3
        // single-address contract is preserved, no partial shape).
        let resp = handle_messages(&params(&[("project", &path), ("address", &addr_a)]));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "not_ready");
        assert!(v.get("inboxErrors").is_none(), "single-address path never partial: {v}");

        // #38: all-inbox sweep where EVERY inbox fails → hard error
        // (not silent empty success). Both failures still named in
        // inboxErrors for agent debugging.
        let resp = handle_messages(&params(&[("project", &path)]));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "not_ready");
        let errs = v["inboxErrors"].as_array().expect("inboxErrors array present");
        assert_eq!(errs.len(), 2, "both failing inboxes reported: {v}");
        let mut seen: Vec<&str> = errs.iter().map(|e| e["address"].as_str().unwrap()).collect();
        seen.sort();
        assert_eq!(seen, vec![addr_a.as_str(), addr_b.as_str()], "{v}");
        for e in errs {
            assert_eq!(e["code"], "not_ready", "{e}");
            assert!(!e["hint"].as_str().unwrap().is_empty(), "hint carries the reason: {e}");
        }

        cleanup_project(&project_id);
    }

    #[test]
    fn messages_validates_triage_flags_before_any_engine_dial() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        // Flag validation runs after identity resolve — use a registered
        // workspace so usage errors surface (unregistered paths 404 first).
        let (name0, path0) = unique("triage-flags");
        let project0 = insert_project(&name0, &path0);

        let resp = handle_messages(&params(&[("project", &path0), ("offset", "abc")]));
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        for k in ["since", "before"] {
            let resp = handle_messages(&params(&[("project", &path0), (k, "yesteryear")]));
            assert_eq!(resp.status, "400 Bad Request", "{k}");
            assert_eq!(body_json(&resp)["error"]["code"], "usage");
        }

        let resp = handle_messages(&params(&[
            ("project", &path0),
            ("junk", "1"),
            ("folder", "Archive"),
        ]));
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("--junk or --folder"));
        cleanup_project(&project0);

        // A valid date window + folder on an addressless workspace is an
        // empty 200 (accepted, no engine dial) — and an over-cap limit
        // clamps rather than erroring.
        let (name, path) = unique("triage");
        let project_id = insert_project(&name, &path);
        let resp = handle_messages(&params(&[
            ("project", &path),
            ("since", "7d"),
            ("before", "2999-01-01"),
            ("folder", "Junk"),
            ("limit", "201"),
            ("offset", "50"),
        ]));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = body_json(&resp);
        assert_eq!(v["count"], 0);
        assert!(v.get("nextOffset").is_none(), "no cursor on an empty page");
        cleanup_project(&project_id);
    }

    // ── read ──

    #[test]
    fn read_validates_ids_and_masks_foreign_messages() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        let resp = handle_read(&HashMap::new());
        assert_eq!(resp.status, "400 Bad Request");

        let (name, path) = unique("read");
        let project_id = insert_project(&name, &path);

        // Missing id → usage; malformed id → usage.
        let resp = handle_read(&params(&[("project", &path)]));
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("id"));
        let resp = handle_read(&params(&[("project", &path), ("id", "garbage")]));
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        // A WELL-FORMED id naming a foreign address → masked
        // message-level not_found (ownership precedes any engine work).
        let (name2, path2) = unique("read-foreign");
        let project2 = insert_project(&name2, &path2);
        let foreign_addr = format!("theirs@{name2}.example");
        seed_address(&project2, &foreign_addr, "active", Some("acc-f"));
        let token = messages::encode_message_id(&foreign_addr, "M1");
        let resp = handle_read(&params(&[("project", &path), ("id", &token)]));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");
        // The masked hint names the TOKEN, never the address.
        assert!(
            !body_json(&resp)["error"]["hint"].as_str().unwrap().contains(&foreign_addr),
            "masked hint must not leak the address"
        );

        // An owned id with no mail server → 503 not_ready.
        let mine = format!("mine@{name}.example");
        seed_address(&project_id, &mine, "active", Some("acc-1"));
        let token = messages::encode_message_id(&mine, "M1");
        let resp = handle_read(&params(&[("project", &path), ("id", &token)]));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);

        cleanup_project(&project_id);
        cleanup_project(&project2);
    }

    // ── attachments ──

    #[test]
    fn attachments_validate_get_out_and_reject_escaping_paths_before_engine() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        let (name, path) = unique("attach");
        let project_id = insert_project(&name, &path);
        std::fs::create_dir_all(&path).expect("workspace root");
        let mine = format!("mine@{name}.example");
        seed_address(&project_id, &mine, "active", Some("acc-1"));
        let token = messages::encode_message_id(&mine, "M1");

        // get without out → usage.
        let resp = handle_attachments(&params(&[
            ("project", &path),
            ("id", &token),
            ("get", "1"),
        ]));
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("out"));

        // get=0 / non-numeric → usage (1-based, §11.1.11).
        for bad in ["0", "x"] {
            let resp = handle_attachments(&params(&[
                ("project", &path),
                ("id", &token),
                ("get", bad),
                ("out", "a.bin"),
            ]));
            assert_eq!(resp.status, "400 Bad Request", "get={bad}");
        }

        // Escaping out-paths are refused BEFORE any engine dial (the
        // 400 here, not the 503 the engine would answer).
        for evil in ["../evil.bin", "/etc/evil.bin", "a/../../evil.bin"] {
            let resp = handle_attachments(&params(&[
                ("project", &path),
                ("id", &token),
                ("get", "1"),
                ("out", evil),
            ]));
            assert_eq!(resp.status, "400 Bad Request", "out={evil}: {}", resp.body);
            assert_eq!(body_json(&resp)["error"]["code"], "usage");
        }

        // A safe path proceeds to the engine → 503 without a server.
        let resp = handle_attachments(&params(&[
            ("project", &path),
            ("id", &token),
            ("get", "1"),
            ("out", "downloads/a.bin"),
        ]));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);

        cleanup_project(&project_id);
        let _ = std::fs::remove_dir_all(&path);
    }

    // ── wait ──

    #[test]
    fn wait_validates_timeout_requires_addresses_and_rate_limits() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();
        let resp = handle_wait(&HashMap::new());
        assert_eq!(resp.status, "400 Bad Request");

        let (name, path) = unique("wait");
        let project_id = insert_project(&name, &path);

        // Timeout bounds: >900 → capped-with-guidance usage; 0/garbage
        // → usage (checked before identity work).
        let resp = handle_wait(&params(&[("project", &path), ("timeout", "901")]));
        assert_eq!(resp.status, "400 Bad Request");
        assert!(body_json(&resp)["error"]["hint"].as_str().unwrap().contains("900"));
        for bad in ["0", "abc"] {
            let resp = handle_wait(&params(&[("project", &path), ("timeout", bad)]));
            assert_eq!(resp.status, "400 Bad Request", "timeout={bad}");
        }

        // No addresses → actionable not_found (never a 300 s hang).
        let resp = handle_wait(&params(&[("project", &path)]));
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(body_json(&resp)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("k2 mail create"));

        // Foreign `to` → masked not_found.
        let (name2, path2) = unique("wait-foreign");
        let project2 = insert_project(&name2, &path2);
        seed_address(&project2, &format!("theirs@{name2}.example"), "active", Some("acc-f"));
        let resp = handle_wait(&params(&[
            ("project", &path),
            ("to", &format!("theirs@{name2}.example")),
        ]));
        assert_eq!(resp.status, "404 Not Found");

        // With an owned address: the slot cap answers 429 when the
        // workspace's 4 slots are held (pre-mortem #10); with a free
        // slot the engine's absence answers 503 (and the held slot was
        // RELEASED on that early return — drop-based).
        seed_address(&project_id, &format!("mine@{name}.example"), "active", Some("acc-1"));
        let _held: Vec<_> = (0..4)
            .map(|_| WaitSlot::try_acquire(&project_id).expect("slot"))
            .collect();
        assert!(WaitSlot::try_acquire(&project_id).is_none(), "cap enforced");
        let resp = handle_wait(&params(&[("project", &path)]));
        assert_eq!(resp.status, "429 Too Many Requests", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "rate_limited");
        drop(_held);
        let resp = handle_wait(&params(&[("project", &path)]));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        let resp = handle_wait(&params(&[("project", &path)]));
        assert_eq!(resp.status, "503 Service Unavailable", "slot released on early return");

        cleanup_project(&project_id);
        cleanup_project(&project2);
    }

    // ── management + delete gating (0081) ──

    #[test]
    fn manage_verbs_validate_gate_on_caps_and_mask() {
        let _g = crate::mail::mail_server_test_lock();
        clear_mail_server();

        // move: missing id / missing folder → usage.
        let resp = handle_move(br#"{"project":"/tmp/x"}"#);
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        let (name, path) = unique("mmanage");
        let project_id = insert_project(&name, &path);
        let mine = format!("mine@{name}.example");
        seed_address(&project_id, &mine, "active", Some("acc-1"));
        let token = messages::encode_message_id(&mine, "M1");

        // move without a folder → usage.
        let resp = handle_move(
            serde_json::json!({ "project": path, "id": token }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");

        // Owned address but NO manage cap (default OFF) → masked
        // not_found, BEFORE any engine dial.
        let resp = handle_move(
            serde_json::json!({ "project": path, "id": token, "folder": "Archive" })
                .to_string().as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");
        // delete with no manage at all → masked (never leaks existence).
        let resp = handle_delete(
            serde_json::json!({ "project": path, "id": token }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found");

        // Grant manage (no delete): move now passes the gate → 503 (no
        // server); delete TEACHES (manage but no delete → usage 400).
        crate::mail::access::set_manage(&mine, &project_id, true, false).expect("manage on");
        let resp = handle_move(
            serde_json::json!({ "project": path, "id": token, "folder": "Archive" })
                .to_string().as_bytes(),
        );
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        let resp = handle_delete(
            serde_json::json!({ "project": path, "id": token }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        // Enable delete → the delete gate passes → 503 (no server).
        crate::mail::access::set_manage(&mine, &project_id, true, true).expect("delete on");
        let resp = handle_delete(
            serde_json::json!({ "project": path, "id": token }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);

        // flag needs at least one of read/flagged.
        let resp = handle_flag(
            serde_json::json!({ "project": path, "id": token }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");

        // folder/list on the managed inbox reaches the engine → 503.
        let resp = handle_folder_list(&params(&[("project", &path), ("address", &mine)]));
        assert_eq!(resp.status, "503 Service Unavailable", "{}", resp.body);
        // folder/list missing address → usage.
        let resp = handle_folder_list(&params(&[("project", &path)]));
        assert_eq!(resp.status, "400 Bad Request");

        cleanup_project(&project_id);
    }

    // ── M3: the vault seam never echoes a raw backend error ──

    /// A scripted vault whose `resolve` returns whatever shape the test
    /// wants — the point is to drive the `Err` arm deterministically.
    struct ScriptedVault(Result<Option<String>, String>);
    impl crate::mail::secrets::SecretStore for ScriptedVault {
        fn store(&self, _kind: &str, _secret: &str) -> Result<String, String> {
            unreachable!("linked inboxes use store_exact")
        }
        fn resolve(&self, _sref: &str) -> Result<Option<String>, String> {
            self.0.clone()
        }
        fn delete(&self, _sref: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn seed_linked_inbox(project_id: &str, address: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, \
             display_name, kind, host, port, tls, username, drafts_folder, status, created_at) \
             VALUES (?1, ?2, ?3, NULL, 'imap', 'imap.example.com', 993, 'implicit-tls', \
             ?3, NULL, 'connected', 100)",
            rusqlite::params![id, project_id, address],
        )
        .expect("seed linked inbox");
        id
    }

    #[test]
    fn linked_password_resolution_never_leaks_the_raw_vault_error() {
        let (name, path) = unique("vaultm3");
        let project_id = insert_project(&name, &path);
        let address = format!("mine@{name}.example");
        let row_id = seed_linked_inbox(&project_id, &address);
        let row = external::inbox_for_address(&address).expect("row");

        // A raw vault error carrying a filesystem path / keychain detail
        // must NOT reach the caller's hint (M3).
        let raw_leak = format!(
            "keychain read denied: /Users/rosson/Library/Keychains/login.keychain-db \
             (item mailext_{row_id})"
        );
        let resp = resolve_linked_password(
            &ScriptedVault(Err(raw_leak.clone())),
            &row,
            &address,
        )
        .expect_err("vault error → 503");
        assert_eq!(resp.status, "503 Service Unavailable");
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "not_ready");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(!hint.contains("keychain"), "raw error leaked: {hint}");
        assert!(!hint.contains("/Users/"), "path leaked: {hint}");
        assert!(!hint.contains(&row_id), "row id leaked: {hint}");
        assert!(hint.contains("reconnect"), "generic reconnect guidance: {hint}");

        // A missing credential (Ok(None)) still teaches the same generic
        // reconnect story, and a present one flows through unchanged.
        let missing = resolve_linked_password(&ScriptedVault(Ok(None)), &row, &address)
            .expect_err("missing → 503");
        assert_eq!(missing.status, "503 Service Unavailable");
        assert!(body_json(&missing)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("reconnect"));
        let Ok(pw) = resolve_linked_password(
            &ScriptedVault(Ok(Some("app-pw".to_string()))),
            &row,
            &address,
        ) else {
            panic!("a present credential should resolve to the password");
        };
        assert_eq!(pw, "app-pw");

        cleanup_linked_inbox(&row_id);
        cleanup_project(&project_id);
    }

    fn cleanup_linked_inbox(id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE id = ?1",
            rusqlite::params![id],
        );
    }
}
