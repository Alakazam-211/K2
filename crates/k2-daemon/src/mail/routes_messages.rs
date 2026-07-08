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
//! [`messages::backend_for_address`] — today always local Stalwart via
//! `domains::engine_from_db` (503 `not_ready` when the server isn't
//! up). No handler talks to the JMAP client directly.
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
use crate::mail::domains;
use crate::mail::messages::{self, MailBackend, ReadBackend, ReadError, WatchAddress};

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

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// Resolve the caller's workspace to `(path, project_id)` — the
/// server-side identity step every workspace-scoped route uses
/// (mirrors routes_addresses; identity from the resolved workspace,
/// never from raw params).
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

/// §17.5: resolve one address's backend and construct it. Today every
/// address answers `LocalStalwart` → the production JMAP client from
/// the `mail_server` row (503 `not_ready` when absent/stopped). V2
/// external backends slot in as new match arms — callers only see
/// `dyn ReadBackend`.
fn backend_for(address: &str) -> Result<Box<dyn ReadBackend>, CliResponse> {
    match messages::backend_for_address(address) {
        MailBackend::LocalStalwart => {
            let (client, _hostname) = domains::engine_from_db().map_err(|hint| {
                error_response("503 Service Unavailable", "not_ready", &hint)
            })?;
            Ok(Box::new(messages::StalwartReadBackend::new(client)))
        }
    }
}

/// The (address, account) watch list for a request: an explicit
/// address must pass the masked ownership gate; no address = every
/// ACTIVE address the workspace owns. Rows minted in a degraded state
/// (no Stalwart account) fail loudly when explicitly targeted and are
/// skipped in the all-addresses sweep (doc'd on `mail::addresses`).
fn watch_list(
    project_id: &str,
    explicit: Option<&str>,
) -> Result<Vec<WatchAddress>, CliResponse> {
    if let Some(addr) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        let row = messages::owned_active_address(project_id, addr)
            .map_err(read_error_response)?;
        let Some(account_id) = row.stalwart_account_id else {
            return Err(error_response(
                "502 Bad Gateway",
                "engine",
                &format!("address '{}' has no mailbox on the mail server", row.address),
            ));
        };
        return Ok(vec![WatchAddress { address: row.address, account_id }]);
    }
    let watch: Vec<WatchAddress> = messages::active_addresses_for_project(project_id)
        .into_iter()
        .filter_map(|r| {
            let account_id = r.stalwart_account_id?;
            Some(WatchAddress { address: r.address, account_id })
        })
        .collect();
    Ok(watch)
}

/// Decode + ownership-gate an opaque message id: the address inside
/// the token must be an ACTIVE address of the CALLING workspace. Every
/// mismatch answers the same masked message-level not_found.
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
    let row = match messages::owned_active_address(project_id, &address) {
        Ok(row) => row,
        Err(ReadError::Usage(hint)) => {
            return Err(error_response("400 Bad Request", "usage", &hint))
        }
        Err(_) => return Err(masked()),
    };
    let Some(account_id) = row.stalwart_account_id else {
        return Err(masked());
    };
    Ok((row.address, account_id, email_id))
}

// ── GET /cli/mail/messages ──────────────────────────────────────────────

/// Newest-first summaries for owned addresses (`k2 mail messages`).
/// Params: `project` (required) · `address` (optional; default = all
/// the caller's addresses) · `unread` · `limit` (default 20, max 100)
/// · `query` (subject AND body) · `from` (From header address +
/// display name, §11.1.8). Summaries only — no bodies ride this route.
pub fn handle_messages(params: &HashMap<String, String>) -> CliResponse {
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
        Some(Ok(n)) if n >= 1 => n.min(100), // §11: max 100 — clamp, documented
        _ => {
            return error_response(
                "400 Bad Request",
                "usage",
                "invalid 'limit' — a number from 1 to 100 (default 20)",
            )
        }
    };
    let (_path, project_id) = match resolve_caller(&project) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
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
        from: crate::cli::opt_param(params, "from"),
        since_unix: None,
    };
    let mut shaped: Vec<(i64, serde_json::Value)> = Vec::new();
    for w in &watch {
        let backend = match backend_for(&w.address) {
            Ok(b) => b,
            Err(resp) => return resp,
        };
        let summaries = match backend.list_inbox(&w.account_id, &filter, limit) {
            Ok(s) => s,
            Err(hint) => return read_error_response(ReadError::Engine(hint)),
        };
        for s in &summaries {
            shaped.push((
                messages::iso_to_unix(&s.received_at),
                messages::shape_summary_json(&w.address, s),
            ));
        }
    }
    // Merge across addresses newest-first, then apply the limit.
    shaped.sort_by(|a, b| b.0.cmp(&a.0));
    shaped.truncate(limit);
    let list: Vec<serde_json::Value> = shaped.into_iter().map(|(_, v)| v).collect();
    ok_json(serde_json::json!({ "ok": true, "count": list.len(), "messages": list }))
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
    // §17.5: all watched addresses resolve their backend through the
    // seam (one shared instance while everything is local Stalwart).
    let backend = match backend_for(&watch[0].address) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let filters = messages::WaitFilters {
        from: crate::cli::opt_param(params, "from"),
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
        backend.as_ref(),
        &watch,
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

        // Bad limit → usage (0 and non-numeric both die).
        for bad in ["0", "abc"] {
            let resp = handle_messages(&params(&[("project", "/tmp/x"), ("limit", bad)]));
            assert_eq!(resp.status, "400 Bad Request", "limit={bad}");
        }

        // Unknown workspace → the shared 404 shape.
        let resp = handle_messages(&params(&[("project", "no-such-workspace-xyz")]));
        assert_eq!(resp.status, "404 Not Found");

        let (name, path) = unique("messages");
        let project_id = insert_project(&name, &path);

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
}
