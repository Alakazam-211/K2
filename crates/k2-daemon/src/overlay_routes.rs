//! `/cli/thread`, `/cli/chatter`, `/cli/chatterlog` + POST
//! `/cli/thread/{post,ask,secret,answer,void}`.
//!
//! GET mutations 405. Overlay is keyed by named conversation_id
//! (handles / pinned Chat), never `v2_session_map`.

use std::cell::RefCell;
use std::collections::HashMap;

use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay::{self, CardCallback, OverlayPage};
use k2_core::skin::SkinPass;
use k2_core::workspace::agent_identity::resolve_project_id;

use crate::cli::{bool_param, opt_param, str_param};
use crate::cli_response::CliResponse;
use crate::overlay_ws::OverlayFrame;
use crate::session_token::HookPrincipal;
use crate::workspace_msg::{self, MsgTarget};

/// `POST /cli/thread/catchup` (inbox-catchup-v1, 0.46.3).
pub(crate) mod catchup;

thread_local! {
    static REQUEST_SKIN: RefCell<Option<SkinPass>> = const { RefCell::new(None) };
}

/// Bind a skin pass for the duration of overlay GET/POST dispatch.
pub fn with_request_skin<T>(pass: Option<SkinPass>, f: impl FnOnce() -> T) -> T {
    REQUEST_SKIN.with(|slot| {
        let prev = slot.replace(pass);
        let out = f();
        slot.replace(prev);
        out
    })
}

fn request_skin() -> Option<SkinPass> {
    REQUEST_SKIN.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
static TEST_INJECTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn record_test_inject(line: &str) {
    #[cfg(test)]
    if let Ok(mut g) = TEST_INJECTS.lock() {
        g.push(line.to_string());
    }
    let _ = line;
}

#[cfg(test)]
pub(crate) fn recorded_injects() -> Vec<String> {
    TEST_INJECTS.lock().map(|g| g.clone()).unwrap_or_default()
}

#[derive(Debug, Clone)]
struct ResolvedOverlay {
    conversation_id: String,
    project_id: String,
    /// The address the conversation answers at **now** (S2). The inject
    /// line, the stored `to`, the activity turn and the JSON `addr` use
    /// it. A post to `k2/3` after a rename resolves to `k2/reviewer`.
    addr: String,
    /// The address the caller sent.
    requested_addr: String,
    /// `sales` / pinned Chat — not a `sales/reviewer` sidecar address.
    canonical_alias: bool,
}

impl ResolvedOverlay {
    /// `movedFrom` for the JSON: the caller's address when the chat was
    /// renamed away from it (a local rename only, TR17).
    fn moved_from(&self) -> Option<&str> {
        (!self.canonical_alias && self.requested_addr != self.addr)
            .then_some(self.requested_addr.as_str())
    }
}

/// Add `movedFrom` to a Thread route's JSON body when the caller used an
/// older address (the CLI prints `note: <old> is now <new>`).
fn with_moved_from(body: serde_json::Value, resolved: &ResolvedOverlay) -> String {
    moved_from_value(body, resolved).to_string()
}

/// [`with_moved_from`] as a value (the shared Thread reader builds bodies
/// the catch-up measures and embeds).
fn moved_from_value(mut body: serde_json::Value, resolved: &ResolvedOverlay) -> serde_json::Value {
    if let Some(old) = resolved.moved_from() {
        body["movedFrom"] = serde_json::json!(old);
    }
    body
}

/// inbox-catchup-v1 (0.46.3, §3.1): a Thread read answer carries the
/// overlay bus `rev` taken BEFORE its store read, and this daemon run's
/// `instanceId` (the same value as the session hello and the push stamp).
///
/// Never for an App guest or App pass (C4): `rev` is a server-wide write
/// counter, so it would tell a guest how busy rooms it can't see are, and
/// guests never use the inbox stream it pairs with.
fn with_rev_and_instance(mut body: serde_json::Value, rev: u64) -> serde_json::Value {
    if request_skin().is_some() {
        return body;
    }
    body["rev"] = serde_json::json!(rev);
    body["instanceId"] = serde_json::json!(crate::boot_status::instance_id());
    body
}

/// The conversation an address reaches now, for the activity tracker's
/// stamped-address fallback (TR9). Never a skin resolution.
pub fn conversation_for_addr(addr: &str) -> Option<String> {
    resolve_addr(addr).ok().map(|r| r.conversation_id)
}

fn error_json(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint.to_string() },
        })
        .to_string(),
    }
}

fn usage(hint: impl std::fmt::Display) -> CliResponse {
    error_json("400 Bad Request", "usage", hint)
}

fn not_found(hint: impl std::fmt::Display) -> CliResponse {
    error_json("404 Not Found", "not_found", hint)
}

fn forbidden(hint: impl std::fmt::Display) -> CliResponse {
    error_json("403 Forbidden", "forbidden", hint)
}

/// Any Thread use of a `gb:` address other than a person's read.
/// 409 `imported_read_only` (CL31): a write to Thread history that came
/// with a clone (an imported line, or an `import:` conversation).
fn imported_read_only() -> CliResponse {
    error_json(
        "409 Conflict",
        "imported_read_only",
        "this is imported Thread history from a clone; it is read-only",
    )
}

/// `Some(409)` when the resolved conversation (or `doc_id` in it) is
/// imported history.
fn refuse_if_imported(resolved: &ResolvedOverlay, doc_id: Option<&str>) -> Option<CliResponse> {
    match k2_core::overlay::transfer::is_read_only(&resolved.conversation_id, doc_id) {
        Ok(false) => None,
        Ok(true) => Some(imported_read_only()),
        Err(e) => Some(error_json("500 Internal Server Error", "overlay", e)),
    }
}

fn gb_refused() -> CliResponse {
    error_json(
        "403 Forbidden",
        "grokbots_humans_only",
        "gb: Threads hold a Grok Bot's messages for people signed in to this server: \
         agents and App guests can't read them, and nobody posts there (message the bot \
         from its chat in K2).",
    )
}

fn skin_room_denied() -> CliResponse {
    crate::skin_routes::skin_room_response()
}

fn require_skin_cap_in_resolved(
    resolved: &ResolvedOverlay,
    cap: &str,
) -> Result<(), CliResponse> {
    let Some(pass) = request_skin() else {
        return Ok(());
    };
    if !pass.has_cap_in_room(&resolved.project_id, cap) {
        return Err(crate::skin_routes::missing_cap_response(cap));
    }
    Ok(())
}

fn pinned_chat_id(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<Option<String>, CliResponse> {
    let Some(session) = WorkspaceSession::get(conn, project_id)
        .map_err(|e| error_json("500 Internal Server Error", "db", e))?
    else {
        return Ok(None);
    };
    Ok(session
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string))
}

/// Skin HTTP: resolve `addr` → project_id without requiring pinned Chat first
/// (no existence oracle). In-rooms + no pin → today's 404. Sidecar / other
/// UUID → `skin_room`. Do not use `canonical_alias`.
fn resolve_skin_thread_addr(addr: &str, pass: &SkinPass) -> Result<ResolvedOverlay, CliResponse> {
    let addr = addr.trim();
    if addr.is_empty() {
        return Err(usage("missing addr"));
    }
    if crate::grokbots_routes::is_gb_addr(addr) {
        return Err(gb_refused());
    }
    if k2_core::workspace_session_handles::split_workspace_handle(addr).is_some() {
        return Err(skin_room_denied());
    }

    let db = k2_core::db::shared();
    let conn = db.lock();

    if k2_core::workspace_session_handles::is_uuid_shape(addr) {
        let Some(project_id) =
            k2_core::workspace_session_handles::project_id_for_session_id(&conn, addr)
                .map_err(|e| error_json("500 Internal Server Error", "db", e))?
        else {
            return Err(skin_room_denied());
        };
        if !pass.has_room(&project_id) {
            return Err(skin_room_denied());
        }
        let Some(pin) = pinned_chat_id(&conn, &project_id)? else {
            return Err(skin_room_denied());
        };
        if pin != addr {
            return Err(skin_room_denied());
        }
        return Ok(ResolvedOverlay {
            conversation_id: pin,
            project_id,
            addr: addr.to_string(),
            requested_addr: addr.to_string(),
            canonical_alias: true,
        });
    }

    let project_id = match k2_core::skin::resolve_room_tokens(&[addr.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids.into_iter().next().unwrap(),
        _ => return Err(skin_room_denied()),
    };
    if !pass.has_room(&project_id) {
        return Err(skin_room_denied());
    }
    let Some(pin) = pinned_chat_id(&conn, &project_id)? else {
        return Err(not_found(format!(
            "no pinned Chat conversation for '{addr}'"
        )));
    };
    Ok(ResolvedOverlay {
        conversation_id: pin,
        project_id,
        addr: addr.to_string(),
        requested_addr: addr.to_string(),
        canonical_alias: true,
    })
}

fn resolve_thread_addr(addr: &str) -> Result<ResolvedOverlay, CliResponse> {
    if let Some(pass) = request_skin() {
        return resolve_skin_thread_addr(addr, &pass);
    }
    resolve_addr(addr)
}

fn resolve_addr(addr: &str) -> Result<ResolvedOverlay, CliResponse> {
    let addr = addr.trim();
    if addr.is_empty() {
        return Err(usage("missing addr"));
    }
    // `gb:<name>`: a Grok Bot's collected Thread. People read it over TCP
    // (the dispatcher hands it to `grokbots_routes::thread_by_addr`);
    // everything that lands here (an agent's cell socket, a post, a
    // card, Chatter, `thread/latest`) is refused: no wake, no inject.
    if crate::grokbots_routes::is_gb_addr(addr) {
        return Err(gb_refused());
    }
    // `import:<id>`: imported history with no session of its own (a
    // detached sidecar, or a clone's main Thread before its first pin).
    // Readable by address; every write is refused (CL30/CL31).
    if k2_core::overlay::transfer::is_import_key(addr) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(project_id) = k2_core::overlay::transfer::project_for_import_key(&conn, addr)
            .map_err(|e| error_json("500 Internal Server Error", "db", e))?
        else {
            return Err(not_found(format!("imported Thread not found: {addr}")));
        };
        return Ok(ResolvedOverlay {
            conversation_id: addr.to_string(),
            project_id,
            addr: addr.to_string(),
            requested_addr: addr.to_string(),
            canonical_alias: false,
        });
    }
    let canonical_alias =
        k2_core::workspace_session_handles::split_workspace_handle(addr).is_none();
    match workspace_msg::resolve_msg_target(addr) {
        Some(MsgTarget::WorkspaceCanonical { path }) => {
            let (project_id, pinned, custom_harness) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let Some(project_id) = resolve_project_id(&conn, &path) else {
                    return Err(not_found(format!("workspace not found: {addr}")));
                };
                let row = WorkspaceSession::get(&conn, &project_id)
                    .map_err(|e| error_json("500 Internal Server Error", "db", e))?;
                let custom_harness = row.as_ref().is_some_and(|r| {
                    k2_core::workspace::provider_resume::provider_resume_for_provider(&r.harness)
                        .is_none()
                });
                let pinned = row
                    .and_then(|s| s.session_id)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                (project_id, pinned, custom_harness)
            };
            // GL44: a custom-preset agent (no model CLI, e.g. a keeper
            // program) has no provider conversation to pin; K2 owns its
            // Thread id. Pin it here too, so the address works before the
            // first launch after an upgrade, and a stale id from before a
            // Claude → custom switch is replaced. Rows of known providers
            // are untouched (their own resolver paths pin them).
            let pinned = if pinned.is_none() || custom_harness {
                k2_core::workspace::resume_chat::ensure_custom_thread_pin(&path).or(pinned)
            } else {
                pinned
            };
            // A clone's imported main Thread (CL5): settle it onto the pin
            // once one is known; until then the address shows the pending
            // history read-only instead of a 404.
            let pinned = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                match pinned {
                    Some(pin) => {
                        if let Err(e) = k2_core::overlay::transfer::settle_pending(&conn, &project_id) {
                            k2_core::log_debug!("[clone/threads] settle {project_id} on resolve: {e}");
                        }
                        Some(pin)
                    }
                    None => k2_core::overlay::transfer::pending_canonical_for(&conn, &project_id)
                        .map_err(|e| error_json("500 Internal Server Error", "db", e))?,
                }
            };
            let Some(conversation_id) = pinned else {
                return Err(not_found(format!(
                    "no pinned Chat conversation for '{addr}'"
                )));
            };
            Ok(ResolvedOverlay {
                conversation_id,
                project_id,
                addr: addr.to_string(),
                requested_addr: addr.to_string(),
                canonical_alias: true,
            })
        }
        Some(MsgTarget::Sidecar {
            path,
            conversation_key,
            current_handle,
            ..
        }) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let Some(project_id) = resolve_project_id(&conn, &path) else {
                return Err(not_found(format!("workspace not found: {addr}")));
            };
            // A Chats name on the pinned conversation is `ws/<name>`-shaped
            // but it is the main Chat: canonical-only writes apply (S2).
            if k2_core::workspace_session_handles::conversation_is_canonical(
                &conn,
                &project_id,
                &conversation_key,
            ) {
                let ws = k2_core::workspace_session_handles::workspace_address_name(&conn, &project_id)
                    .unwrap_or_else(|_| addr.to_string());
                return Ok(ResolvedOverlay {
                    conversation_id: conversation_key,
                    project_id,
                    addr: ws,
                    requested_addr: addr.to_string(),
                    canonical_alias: true,
                });
            }
            Ok(ResolvedOverlay {
                conversation_id: conversation_key,
                project_id,
                addr: workspace_msg::current_sidecar_address(addr, &current_handle),
                requested_addr: addr.to_string(),
                canonical_alias: false,
            })
        }
        Some(MsgTarget::Session { session_id }) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let Some(project_id) =
                k2_core::workspace_session_handles::project_id_for_session_id(&conn, &session_id)
                    .map_err(|e| error_json("500 Internal Server Error", "db", e))?
            else {
                return Err(not_found(format!("session not found: {addr}")));
            };
            Ok(ResolvedOverlay {
                conversation_id: session_id,
                project_id,
                addr: addr.to_string(),
                requested_addr: addr.to_string(),
                canonical_alias,
            })
        }
        None => Err(not_found(match workspace_msg::explain_unresolved_sidecar(addr) {
            Some(why) => format!("unknown overlay addr '{addr}': {why}"),
            None => format!("unknown overlay addr '{addr}'"),
        })),
    }
}

fn authorize_read(
    principal: Option<&HookPrincipal>,
    resolved: &ResolvedOverlay,
) -> Result<(), CliResponse> {
    let Some(p) = principal else {
        return Ok(());
    };
    if p.workspace_uuid.trim() != resolved.project_id {
        return Err(forbidden(
            "same-workspace agents can read overlay in that workspace only",
        ));
    }
    Ok(())
}

/// T22: write Chat overlay (`sales`) = canonical. Write `sales/reviewer` =
/// that session (own sidecar; canonical may write it too).
fn authorize_write(
    principal: Option<&HookPrincipal>,
    resolved: &ResolvedOverlay,
    stamped_from: &str,
) -> Result<(), CliResponse> {
    let Some(p) = principal else {
        return Ok(());
    };
    if p.workspace_uuid.trim() != resolved.project_id {
        return Err(forbidden("cannot write overlay in another workspace"));
    }
    let caller_is_sidecar = stamped_from.contains('/');
    if resolved.canonical_alias && caller_is_sidecar {
        return Err(forbidden(
            "write Chat overlay (k2 thread <workspace>) is canonical-only",
        ));
    }
    Ok(())
}

/// Thread items as THIS caller may see them (thread-attachments-v1): each
/// attachment gains its live `state`, and an App guest sees only the files
/// its own file rights cover (the rest are `{kind, state:"hidden"}`, with
/// no name, path or size). Every Thread read that returns items goes
/// through here: the page (`snapshot_json`) and the thread-delta-v1
/// `items` and `changed` (`thread_delta_json`), so the two can't drift.
fn thread_items_view(items: &[overlay::OverlayItem], resolved: &ResolvedOverlay, pass: Option<&SkinPass>) -> serde_json::Value {
    let mut v = serde_json::to_value(items).unwrap_or_else(|_| serde_json::json!([]));
    crate::thread_attachments::rewrite_items(&mut v, &resolved.project_id, pass);
    v
}

/// The page body (`ok:true`, newest page / `since_seq` / `before_seq`).
/// `rev` must be read before the store read that produced `page`.
fn snapshot_json(collection: &str, resolved: &ResolvedOverlay, page: OverlayPage, rev: u64) -> serde_json::Value {
    let items = if collection == "thread" {
        thread_items_view(&page.items, resolved, request_skin().as_ref())
    } else {
        serde_json::to_value(&page.items).unwrap_or_else(|_| serde_json::json!([]))
    };
    let body = serde_json::json!({
        "ok": true,
        "collection": collection,
        "addr": resolved.addr,
        "conversation_id": resolved.conversation_id,
        "items": items,
        "has_more": page.has_more,
        // Q5: every address this chat answered at before, so a client
        // renders old rows' `from`/`to` under the current name.
        "pastAddresses": past_addresses(resolved),
    });
    with_rev_and_instance(moved_from_value(body, resolved), rev)
}

/// `ws/<n>` and each retired `ws/<name>` of a sidecar conversation.
fn past_addresses(resolved: &ResolvedOverlay) -> Vec<String> {
    if resolved.canonical_alias {
        return Vec::new();
    }
    let Some((ws, _)) = k2_core::workspace_session_handles::split_workspace_handle(&resolved.addr)
    else {
        return Vec::new();
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace_session_handles::past_handles_for(&conn, &resolved.project_id, &resolved.conversation_id)
        .unwrap_or_default()
        .into_iter()
        .map(|h| k2_core::workspace_session_handles::format_address(ws, Some(&h)))
        .collect()
}

const OVERLAY_PAGE_DEFAULT: usize = 25;
const OVERLAY_PAGE_MAX: usize = 500;

/// Absent `since_seq` = initial/tail page. Present (including 0) = seq > since_seq.
fn parse_since_opt(params: &HashMap<String, String>) -> Option<i64> {
    opt_param(params, "since_seq").and_then(|s| s.parse::<i64>().ok())
}

/// thread-delta-v1 `after_seq=N`: absent = the old paging; present must be
/// a whole number `>= 0` (400 `usage` otherwise, never silently ignored:
/// a client that asked for a delta must not get a newest-N page).
pub(crate) fn parse_after_seq(params: &HashMap<String, String>) -> Result<Option<i64>, CliResponse> {
    match params.get("after_seq") {
        None => Ok(None),
        Some(raw) => match raw.trim().parse::<i64>() {
            Ok(n) if n >= 0 => Ok(Some(n)),
            _ => Err(usage(format!("invalid after_seq '{raw}': expected a whole number >= 0"))),
        },
    }
}

/// The `after_seq` response (thread-delta-v1): the snapshot shape with
/// `items` OLDEST first after N and `has_more` meaning NEWER items remain,
/// plus `after_seq`, `changed` (items at or below N in their current
/// state) and `removed` (`{id, seq}` of items that left this Thread).
///
/// `items` and `changed` carry the same per-caller attachment view as the
/// page ([`thread_items_view`]: live `state`, the App guest strip);
/// `removed` is `{id, seq}` only.
fn thread_delta_json(resolved: &ResolvedOverlay, after_seq: i64, delta: overlay::ThreadDelta, rev: u64) -> serde_json::Value {
    let pass = request_skin();
    let body = serde_json::json!({
        "ok": true,
        "collection": "thread",
        "addr": resolved.addr,
        "conversation_id": resolved.conversation_id,
        "after_seq": after_seq,
        "items": thread_items_view(&delta.items, resolved, pass.as_ref()),
        "has_more": delta.has_more,
        "changed": thread_items_view(&delta.changed, resolved, pass.as_ref()),
        "removed": removed_json(&delta.removed),
        "pastAddresses": past_addresses(resolved),
    });
    with_rev_and_instance(moved_from_value(body, resolved), rev)
}

/// `removed` rows of a delta as `[{id, seq}]`.
pub fn removed_json(removed: &[(i64, String)]) -> serde_json::Value {
    serde_json::Value::Array(
        removed.iter().map(|(seq, id)| serde_json::json!({ "id": id, "seq": seq })).collect(),
    )
}

fn parse_before_seq(params: &HashMap<String, String>) -> Option<i64> {
    opt_param(params, "before_seq").and_then(|s| s.parse::<i64>().ok())
}

/// Default 25. Explicit `limit=0` is unbounded (CLI `--all`). Else clamp 1..=500.
fn parse_overlay_limit(params: &HashMap<String, String>) -> usize {
    match opt_param(params, "limit") {
        None => OVERLAY_PAGE_DEFAULT,
        Some(raw) => match raw.parse::<i64>() {
            Ok(0) => 0,
            Ok(n) if n < 0 => OVERLAY_PAGE_DEFAULT,
            Ok(n) => (n as usize).clamp(1, OVERLAY_PAGE_MAX),
            Err(_) => OVERLAY_PAGE_DEFAULT,
        },
    }
}

/// How one Thread read pages (inbox-catchup-v1 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadReadMode {
    /// The plain page: `since_seq` / `before_seq` (both `None` = newest).
    Page { since: Option<i64>, before: Option<i64> },
    /// thread-delta-v1 `after_seq=N`.
    After(i64),
    /// The newest `limit` items (a catch-up `newest:true` entry). The same
    /// read as `Page { since: None, before: None }`.
    Newest,
}

/// Steps 1–3 of every Thread read: resolve `addr` the way THIS caller may
/// (an App guest: its rooms' pinned Chats only), `thread:read` in that
/// room for a guest, and the agent passport's own-workspace rule.
fn thread_read_target(addr: &str, principal: Option<&HookPrincipal>) -> Result<ResolvedOverlay, CliResponse> {
    if addr.trim().is_empty() {
        return Err(usage("missing addr"));
    }
    let resolved = resolve_thread_addr(addr)?;
    require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_READ)?;
    authorize_read(principal, &resolved)?;
    Ok(resolved)
}

/// Step 4: the store read and the body (`ok:true`, items through
/// [`thread_items_view`], `rev` read BEFORE the store read, `instanceId`).
fn thread_read_body(resolved: &ResolvedOverlay, mode: ThreadReadMode, limit: usize) -> Result<serde_json::Value, CliResponse> {
    let rev = crate::overlay_ws::current_rev();
    let (since, before) = match mode {
        ThreadReadMode::After(after) => {
            return overlay::read_thread_delta(&resolved.conversation_id, after, limit)
                .map(|delta| thread_delta_json(resolved, after, delta, rev))
                .map_err(|e| error_json("500 Internal Server Error", "store", e));
        }
        ThreadReadMode::Page { since, before } => (since, before),
        ThreadReadMode::Newest => (None, None),
    };
    overlay::read_thread_page(&resolved.conversation_id, since, before, limit)
        .map(|page| snapshot_json("thread", resolved, page, rev))
        .map_err(|e| error_json("500 Internal Server Error", "store", e))
}

/// One Thread read for the caller bound on this thread (the App guest's
/// pass via [`with_request_skin`]) and `principal` (an agent passport).
/// The ONE reader behind `GET /cli/thread` and every `POST
/// /cli/thread/catchup` entry, so a catch-up entry can never drift from
/// the plain read for any caller (the 0.46.1 delta leak, `c650ea6bb`, was
/// exactly such a drift). Ok = the body with `ok:true`; Err = the plain
/// read's error response (400 / 403 / 404 / 500).
pub(crate) fn thread_read_for(
    addr: &str,
    mode: ThreadReadMode,
    limit: usize,
    principal: Option<&HookPrincipal>,
) -> Result<serde_json::Value, CliResponse> {
    let resolved = thread_read_target(addr, principal)?;
    thread_read_body(&resolved, mode, limit)
}

fn handle_get_thread(params: &HashMap<String, String>) -> CliResponse {
    let addr = str_param(params, "addr");
    let principal = crate::caller_workspace::principal_from_params(params);
    let resolved = match thread_read_target(&addr, principal.as_ref()) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let limit = parse_overlay_limit(params);
    let mode = match parse_after_seq(params) {
        Err(e) => return e,
        Ok(Some(after)) => {
            if opt_param(params, "since_seq").is_some() || opt_param(params, "before_seq").is_some() {
                return usage("after_seq cannot be combined with since_seq or before_seq");
            }
            ThreadReadMode::After(after)
        }
        Ok(None) => ThreadReadMode::Page { since: parse_since_opt(params), before: parse_before_seq(params) },
    };
    match thread_read_body(&resolved, mode, limit) {
        Ok(body) => CliResponse::ok_json(body.to_string()),
        Err(e) => e,
    }
}

/// Most addrs one `thread/latest` call may name (Zen Z41).
pub const THREAD_LATEST_MAX_ADDRS: usize = 50;
/// Server-side preview cut (chars, ellipsis included).
pub const THREAD_PREVIEW_CHARS: usize = 140;

/// One line of preview text for a Thread item (Zen Z41). A choice card is
/// "Asked: <prompt>"; a secret card never shows anything but "Asked for a
/// secret" (the value never lives in the Thread, T28). Whitespace is
/// collapsed; the result is at most [`THREAD_PREVIEW_CHARS`] chars.
pub fn thread_preview(doc: &k2_core::overlay::OverlayDoc) -> String {
    let raw = match doc.kind.as_str() {
        "choice" => {
            let prompt = doc
                .choice
                .as_ref()
                .map(|c| c.prompt.clone())
                .or_else(|| doc.body.clone())
                .unwrap_or_default();
            format!("Asked: {prompt}")
        }
        "secret" => "Asked for a secret".to_string(),
        _ => doc.body.clone().unwrap_or_default(),
    };
    let mut collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    // A post with only files previews as `[file]` / `[N files]`: never a name.
    if collapsed.is_empty() {
        if let Some(tag) = k2_core::overlay::attachments::placeholder(doc.attachments.len()) {
            collapsed = tag;
        }
    }
    if collapsed.chars().count() <= THREAD_PREVIEW_CHARS {
        collapsed
    } else {
        let mut cut: String = collapsed.chars().take(THREAD_PREVIEW_CHARS - 1).collect();
        cut = cut.trim_end().to_string();
        cut.push('…');
        cut
    }
}

/// `GET /cli/thread/latest?addrs=a,b,c` (Zen Z41, vs-live Z66): the newest
/// Thread item per addr, one request per server for list previews. Each
/// addr resolves and authorizes on its own; an addr that fails is an item
/// with `error`, never a failure of the batch. Reads exactly one item per
/// addr (never `limit=0`).
///
/// Room-scope R5 (0.46.1): a guest or App pass (bound by the dispatcher)
/// resolves each addr the way `GET /cli/thread` does for it (its rooms'
/// pinned Chats only; anything else is the uniform `skin_room`) and needs
/// `thread:read` in that room, else the item's error is `cap_not_granted`.
/// The batch limit stays [`THREAD_LATEST_MAX_ADDRS`] (Q17).
fn handle_get_thread_latest(params: &HashMap<String, String>) -> CliResponse {
    let raw = str_param(params, "addrs");
    let mut addrs: Vec<String> = Vec::new();
    for a in raw.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        if !addrs.iter().any(|x| x == a) {
            addrs.push(a.to_string());
        }
    }
    if addrs.is_empty() {
        return usage("missing addrs (comma-separated Thread addresses)");
    }
    if addrs.len() > THREAD_LATEST_MAX_ADDRS {
        return usage(format!(
            "at most {THREAD_LATEST_MAX_ADDRS} addrs per request, got {}",
            addrs.len()
        ));
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    // thread-read-v1: a person (never an agent) also gets their own
    // `unread` / `waiting` / `muted` / `lastReadSeq` on each row.
    let reader = match crate::read_routes::request_login() {
        Some(login) if principal.is_none() => crate::read_routes::person_of_login(&login).ok(),
        _ => None,
    };
    let read_db = reader.as_ref().map(|_| k2_core::db::shared());
    let read_conn = read_db.as_ref().map(|db| db.lock());
    let read_ready = read_conn.as_ref().is_some_and(|c| k2_core::read_state::tables_exist(c));
    let now = k2_core::read_state::now_secs();
    let with_read = |mut row: serde_json::Value, conversation_id: &str| -> serde_json::Value {
        let (Some(person), Some(conn), true) = (reader.as_ref(), read_conn.as_ref(), read_ready) else {
            return row;
        };
        match k2_core::read_state::item(conn, person, k2_core::read_state::KIND_THREAD, conversation_id, now) {
            Ok(it) => {
                row["unread"] = serde_json::json!(it.unread);
                row["waiting"] = serde_json::json!(it.waiting);
                row["muted"] = serde_json::json!(it.muted);
                row["lastReadSeq"] = serde_json::json!(it.last_read_seq);
            }
            // A Thread with no posts yet has no catalog row: nothing to read.
            Err(k2_core::read_state::ReadError::NotFound(_)) => {
                row["unread"] = serde_json::json!(0);
                row["waiting"] = serde_json::json!(0);
                row["muted"] = serde_json::json!(false);
                row["lastReadSeq"] = serde_json::json!(0);
            }
            Err(e) => k2_core::log_debug!("[thread/latest] read state for {conversation_id}: {e}"),
        }
        row
    };
    let error_of = |r: CliResponse| -> serde_json::Value {
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
        let code = v["error"]["code"].as_str().unwrap_or("error").to_string();
        let hint = v["error"]["hint"].as_str().map(str::to_string).unwrap_or_else(|| r.body.clone());
        serde_json::json!({ "code": code, "hint": hint })
    };
    let items: Vec<serde_json::Value> = addrs
        .iter()
        .map(|addr| {
            let resolved = match resolve_thread_addr(addr) {
                Ok(r) => r,
                Err(e) => return serde_json::json!({ "addr": addr, "error": error_of(e) }),
            };
            if let Err(e) = authorize_read(principal.as_ref(), &resolved) {
                return serde_json::json!({ "addr": addr, "error": error_of(e) });
            }
            if require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_READ).is_err() {
                return serde_json::json!({
                    "addr": addr,
                    "error": { "code": "cap_not_granted", "hint": format!("missing capability {}", k2_core::skin::CAP_THREAD_READ) },
                });
            }
            match overlay::read_thread_page(&resolved.conversation_id, None, None, 1) {
                Ok(page) => match page.items.last() {
                    Some(item) => with_read(
                        serde_json::json!({
                            "addr": addr,
                            "conversationId": resolved.conversation_id,
                            "seq": item.seq,
                            "at": item.doc.created_at,
                            "from": item.doc.from,
                            "via": item.doc.via,
                            "author": item.doc.author,
                            "kind": item.doc.kind,
                            "preview": thread_preview(&item.doc),
                            // thread-attachments-v1: a count only.
                            "attachmentCount": item.doc.attachments.len(),
                        }),
                        &resolved.conversation_id,
                    ),
                    None => with_read(
                        serde_json::json!({
                            "addr": addr,
                            "conversationId": resolved.conversation_id,
                            "seq": null,
                            "at": null,
                            "from": null,
                            "via": null,
                            "author": null,
                            "kind": null,
                            "preview": null,
                        }),
                        &resolved.conversation_id,
                    ),
                },
                Err(e) => serde_json::json!({
                    "addr": addr,
                    "error": { "code": "store", "hint": e },
                }),
            }
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "ok": true, "items": items }).to_string())
}

fn handle_get_chatter(params: &HashMap<String, String>) -> CliResponse {
    let addr = str_param(params, "addr");
    if addr.is_empty() {
        return usage("missing addr");
    }
    let resolved = match resolve_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_read(principal.as_ref(), &resolved) {
        return e;
    }
    let since = parse_since_opt(params);
    let before = parse_before_seq(params);
    let limit = parse_overlay_limit(params);
    let rev = crate::overlay_ws::current_rev();
    match overlay::read_chatter_page(&resolved.conversation_id, since, before, limit) {
        Ok(page) => CliResponse::ok_json(snapshot_json("chatter", &resolved, page, rev).to_string()),
        Err(e) => error_json("500 Internal Server Error", "store", e),
    }
}

fn handle_get_chatterlog(params: &HashMap<String, String>) -> CliResponse {
    let since = parse_since(params);
    match overlay::read_chatterlog(since) {
        Ok(items) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "collection": "chatterlog",
                "items": items,
            })
            .to_string(),
        ),
        Err(e) => error_json("500 Internal Server Error", "store", e),
    }
}

fn parse_since(params: &HashMap<String, String>) -> i64 {
    opt_param(params, "since_seq")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
}

/// Skin overlay + PTY stamp: `SkinPass.username`, never body `from`.
fn skin_from_stamp(pass: &SkinPass) -> String {
    let u = pass.username.trim();
    if u.is_empty() {
        "skin".to_string()
    } else {
        u.to_string()
    }
}

/// Human Message-the-agent on Thread: token identity, never body `from`.
/// Owner token (`"owner"`) → the stamp rule (linked username, else `owner`). Connect-user → username.
fn compose_from_for_session(session_author: &str) -> String {
    let s = session_author.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("owner") {
        workspace_msg::resolve_owner_from()
    } else {
        s.to_string()
    }
}

/// The `author` of a human compose post, from the same token identity as
/// [`compose_from_for_session`]: the owner token is `owner`, a Connect
/// session is `user` with its username.
fn compose_author_for_session(session_author: &str) -> overlay::PostAuthor {
    let s = session_author.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("owner") {
        overlay::PostAuthor::owner()
    } else {
        overlay::PostAuthor::user(s)
    }
}

/// Who answered or dismissed a card, for the `[from <name>]` on the line
/// the agent gets. Credential-derived, never the body: an app guest's pass
/// username, a Connect user's login, the owner's display name for the
/// owner token, or (an agent answering its own card through its passport)
/// the stamped `from`. Same sources as a compose post's `from`.
fn card_actor_from(params: &HashMap<String, String>, resolved: &ResolvedOverlay, session_author: &str) -> String {
    if let Some(pass) = request_skin() {
        skin_from_stamp(&pass)
    } else if principal_is_bound(params) {
        overlay_sender_from(params, resolved)
    } else {
        compose_from_for_session(session_author)
    }
}

/// The card's `answeredBy`, from the same credential as [`card_actor_from`]
/// (and as a compose post's `author`). None for an agent's own answer. The
/// owner token's carries the owner's display name (`name`), the same name
/// the agent hears in `[from <name>]` and an owner post shows in `from`.
fn card_actor_author(params: &HashMap<String, String>, session_author: &str) -> Option<overlay::PostAuthor> {
    if let Some(pass) = request_skin() {
        Some(overlay::PostAuthor::guest(&skin_from_stamp(&pass)))
    } else if principal_is_bound(params) {
        None
    } else {
        Some(with_owner_name(compose_author_for_session(session_author), &workspace_msg::resolve_owner_from()))
    }
}

/// An owner author gains the owner's display name; any other kind is as is.
fn with_owner_name(a: overlay::PostAuthor, owner_name: &str) -> overlay::PostAuthor {
    if a.kind == k2_core::overlay::doc::AUTHOR_OWNER {
        overlay::PostAuthor::owner_named(owner_name)
    } else {
        a
    }
}

/// The `answeredBy` JSON an owner-token answer stores today (tests).
#[cfg(test)]
fn owner_answered_by_json() -> serde_json::Value {
    serde_json::to_value(overlay::PostAuthor::owner_named(&workspace_msg::resolve_owner_from())).expect("author json")
}

/// `principal_bound` is exactly `"1"`. Boolean `true` stringifies to `"true"`
/// and must not count. A body flag with no prior stamp must not count either.
fn principal_is_bound(params: &HashMap<String, String>) -> bool {
    params
        .get(crate::caller_workspace::PRINCIPAL_BOUND_KEY)
        .map(String::as_str)
        == Some("1")
}

/// No principal: trim `from` first. Empty, `k2`, `owner`, or
/// `resolve_owner_from()` (ASCII case-insensitive) become the room handle.
/// Any other explicit `from` is that trimmed string.
fn unbound_overlay_from(params: &HashMap<String, String>, resolved: &ResolvedOverlay) -> String {
    let trimmed = params.get("from").map(|s| s.trim()).unwrap_or("");
    let owner = workspace_msg::resolve_owner_from();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("k2")
        || trimmed.eq_ignore_ascii_case("owner")
        || trimmed.eq_ignore_ascii_case(&owner)
    {
        thread_from_room_handle(resolved)
    } else {
        trimmed.to_string()
    }
}

/// Bound principal: the restored stamp, even when that handle is `owner`.
/// Otherwise [`unbound_overlay_from`].
fn overlay_sender_from(params: &HashMap<String, String>, resolved: &ResolvedOverlay) -> String {
    if principal_is_bound(params) {
        params.get("from").cloned().unwrap_or_default()
    } else {
        unbound_overlay_from(params, resolved)
    }
}

/// One audit line per Thread post a Garden widget sends for the person
/// (`zen.widget.post`): which widget, which Garden, to whom, how long.
/// Never the text itself.
fn audit_widget_post(origin: &overlay::WidgetOrigin, from: &str, addr: &str, text: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "zen.widget.post",
        from,
        format!(
            "widget={:?} garden={} to={addr} chars={}",
            origin.widget,
            origin.garden,
            text.chars().count()
        ),
        "-",
        "-",
        "cli",
    ));
}

/// The cap a Garden widget needs to post, answer a card or draft.
const WIDGET_THREAD_POST_CAP: &str = "thread:post";

/// Parse a request's `origin` (UWB12a + the 0.46.1 placement claim). Only
/// the person's own post or answer may carry one (unbound, not an app
/// pass); `what` names the route for the usage message.
fn widget_origin_param(
    params: &HashMap<String, String>,
    allowed: bool,
    what: &str,
) -> Result<Option<(overlay::WidgetOrigin, Option<overlay::WidgetPlacementClaim>)>, CliResponse> {
    let Some(raw) = params.get("origin") else { return Ok(None) };
    if !allowed {
        return Err(usage(format!("origin is only for the person's own {what} from a Garden widget")));
    }
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| usage("origin must be {widget, garden, placement?, hash?}"))?;
    overlay::WidgetOrigin::parse_claim(&v).map(Some).map_err(usage)
}

/// 0.46.1: the daemon's own `thread:post` check for a Garden widget's post
/// or card answer. Before this only the renderer's custom layer checked the
/// cap (`zen-custom-bridge.ts`).
///
/// The daemon can't see which widget is calling (prd-zen-user-widgets-v2
/// §4.1, as for `/cli/zen/call`): a widget runs in a sealed frame with no
/// token and reaches K2 only through the app's custom layer, which marks
/// every post it makes with `origin` = {widget, garden, placement, hash}.
/// So a widget can't drop the marking; only the app (the owner token) could,
/// and the app can always post as the person anyway. The check is the third
/// layer of UW26 (daemon decides the caps, renderer re-checks, every server
/// checks the role): a guardrail against a renderer mistake or an injected
/// prompt, not a wall against a same-user process.
///
/// Who checks: the daemon that holds the Garden (this computer's), because
/// only it knows the placement's approved manifest:
/// - `origin` with `placement` + `hash` (the app's post to an agent on this
///   computer): the placement must be on this computer, place that widget,
///   run that live hash, and its approved caps must include `thread:post`.
///   Unknown Garden or placement, another widget, a stale hash →
///   403 `widget_not_approved`; caps without `thread:post` (never asked, or
///   dropped by the Sources taint) → 403 `widget_cap_missing`.
/// - `origin` without them that names a Garden on this computer: refused
///   (403 `widget_not_approved`): this computer's widgets always say where
///   they sit, so the label alone can't skip the check.
/// - `origin` without them naming a Garden this computer doesn't hold: a
///   widget on another computer posting to an agent here. That computer's
///   daemon already checked it (`POST /cli/zen/widget/check` before the app
///   posts); this daemon keeps the label only, as before.
///
/// K2's own template widgets (the Garden's Ask box, `compose`) are K2's
/// code granted by the template, not sealed frames: they post as the
/// person's compose bar with no `origin`, so an unmarked post is unchanged.
/// K2's built-in custom widgets (`k2:<name>@<n>`) are placements like any
/// other and pass on their manifests' caps.
fn widget_post_gate(
    origin: &overlay::WidgetOrigin,
    claim: Option<&overlay::WidgetPlacementClaim>,
    from: &str,
    addr: &str,
) -> Result<(), CliResponse> {
    let refusal = match claim {
        Some(c) => {
            let f = k2_core::zen::ZenFiles::local();
            match k2_core::zen::widget_access::check_placement_cap(
                &f,
                &origin.garden,
                &c.placement,
                &origin.widget,
                &c.hash,
                WIDGET_THREAD_POST_CAP,
            ) {
                Ok(_) => return Ok(()),
                Err(r) => r,
            }
        }
        None => {
            // Read-only (no real-home guard needed, like `zen::is_set_up`).
            let f = k2_core::zen::ZenFiles::new(k2_core::zen::zen_root());
            if !k2_core::zen::widget_access::holds_garden(&f, &origin.garden) {
                return Ok(());
            }
            k2_core::zen::widget_access::PlacementRefusal {
                code: k2_core::zen::widget_access::WIDGET_NOT_APPROVED,
                message: format!(
                    "a widget in Garden '{}' must name its placement and hash; K2 reloads the widget",
                    origin.garden
                ),
            }
        }
    };
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "zen.widget.post_refused",
        from,
        format!(
            "widget={:?} garden={} placement={} to={addr} code={}",
            origin.widget,
            origin.garden,
            claim.map_or("-", |c| c.placement.as_str()),
            refusal.code
        ),
        "-",
        "-",
        "cli",
    ));
    Err(error_json("403 Forbidden", refusal.code, refusal.message))
}

/// One daemon-written line on a workspace's main Thread, for its human
/// (clone proposals, prd-clone-carries-threads-and-vault-v1 §4.4). Plain
/// text doc; no inject, no wake, no push. Returns the doc id.
pub(crate) fn post_workspace_notice(addr: &str, from: &str, text: &str) -> Result<String, String> {
    let resolved = resolve_addr(addr).map_err(|e| e.body)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) = overlay::post_thread(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        from,
        &resolved.addr,
        text,
        "thread",
    )?;
    // inbox-catchup-v1 (0.46.3): publish while the write's lock is held,
    // so bus (and `rev`) order is commit order.
    crate::overlay_ws::emit_links(&links, &item.doc);
    drop(conn);
    Ok(item.id)
}

fn handle_post(params: &HashMap<String, String>, session_author: &str) -> CliResponse {
    // A23: a Thread turn's `startedAt` is the daemon's ms receipt time.
    let received_at = chrono::Utc::now().timestamp_millis();
    let addr = str_param(params, "addr");
    let text = str_param(params, "text");
    if addr.is_empty() {
        return usage("missing addr");
    }
    // thread-attachments-v1: `attachments` = [{path, line?, lineEnd?}]
    // (JSON; a form body carries it as a JSON string). Text may be empty
    // when files ride the post.
    let attach_reqs = match params.get("attachments").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => Vec::new(),
        Some(raw) => match k2_core::overlay::attachments::parse_requests(raw) {
            Ok(r) => r,
            Err(e) => return error_json("400 Bad Request", e.code(), e.hint()),
        },
    };
    if text.is_empty() && attach_reqs.is_empty() {
        return usage("missing text");
    }
    let via_in = opt_param(params, "via").unwrap_or_else(|| "thread".to_string());
    // Skin 403 stays first. A skin token on this arm has no principal.
    if request_skin().is_some() && via_in == "compose" {
        return error_json(
            "403 Forbidden",
            "forbidden",
            "skin tokens cannot use via=compose",
        );
    }
    // App guests can't attach in v1: an attachment would point other
    // viewers at room files (their uploads already arrive as text).
    if request_skin().is_some() && !attach_reqs.is_empty() {
        return error_json("403 Forbidden", "forbidden", "App guests can't attach files to a Thread message");
    }
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(r) = refuse_if_imported(&resolved, None) {
        return r;
    }
    if let Err(e) = require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_POST) {
        return e;
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    let bound = principal_is_bound(params);
    // A bound caller is not the human compose bar. Do not store via=compose
    // and do not run the human side effects below.
    let via = if bound && via_in == "compose" {
        "thread".to_string()
    } else {
        via_in
    };
    // Skin: pass username only (never body `from`). No principal +
    // via=compose: session actor, still ignoring body `from`. Bound:
    // restored stamp. Else owner-shaped `from` becomes the room handle.
    let from = if let Some(pass) = request_skin() {
        skin_from_stamp(&pass)
    } else if !bound && via == "compose" {
        compose_from_for_session(session_author)
    } else {
        overlay_sender_from(params, &resolved)
    };
    // Who sent a person's post, from the same credential as `from` (never
    // the body): the shared Thread paints "You" only on the viewer's own.
    // Agent posts (bound, or an unbound non-compose post) carry none.
    let author = if let Some(pass) = request_skin() {
        Some(overlay::PostAuthor::guest(&skin_from_stamp(&pass)))
    } else if !bound && via == "compose" {
        Some(compose_author_for_session(session_author))
    } else {
        None
    };
    let command = if via == "compose" {
        match workspace_msg::normalize_composer_slash_command(
            &opt_param(params, "command").unwrap_or_default(),
        ) {
            Ok(Some(cmd)) => cmd.to_string(),
            Ok(None) => String::new(),
            Err(e) => return usage(e),
        }
    } else {
        String::new()
    };
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, &from) {
        return e;
    }
    // UWB12a: a Garden widget's post for the person carries `origin`. It is
    // only the person's own post (unbound via=compose, not an app pass, not
    // a bound agent); anything else naming an origin is refused. 0.46.1:
    // the daemon checks the widget's approved `thread:post` too
    // ([`widget_post_gate`]).
    let origin = match widget_origin_param(
        params,
        request_skin().is_none() && !bound && via == "compose",
        "post (via=compose)",
    ) {
        Ok(None) => None,
        Ok(Some((o, claim))) => {
            if let Err(e) = widget_post_gate(&o, claim.as_ref(), &from, &resolved.addr) {
                return e;
            }
            Some(o)
        }
        Err(e) => return e,
    };
    // Check and snapshot the files before anything is stored: inside the
    // Thread's own workspace (realpath), a regular file, not a secret. Any
    // refusal refuses the whole post.
    let (attachments, workspace_root) = if attach_reqs.is_empty() {
        (Vec::new(), None)
    } else {
        let Some(root) = crate::thread_attachments::project_root(&resolved.project_id) else {
            return not_found("this Thread's workspace folder is not registered");
        };
        let home = dirs::home_dir();
        match k2_core::overlay::attachments::snapshot_all(&root, home.as_deref(), &attach_reqs) {
            Ok(a) => {
                crate::thread_attachments::audit(
                    "thread.attachment.post",
                    &from,
                    format!(
                        "to={} count={} rels={:?}",
                        resolved.addr,
                        a.len(),
                        a.iter().map(|x| x.path.as_str()).collect::<Vec<_>>()
                    ),
                );
                (a, Some(root))
            }
            Err(e) => {
                // The path for pattern/outside refusals; a content hit logs
                // the path, never the matched text.
                let what = match &e {
                    k2_core::overlay::attachments::AttachError::Secret { rel, .. } => rel.clone(),
                    k2_core::overlay::attachments::AttachError::Outside { input }
                    | k2_core::overlay::attachments::AttachError::NotAFile { input } => input.clone(),
                    k2_core::overlay::attachments::AttachError::Usage(_) => "-".to_string(),
                };
                crate::thread_attachments::audit(
                    "thread.attachment.refused",
                    &from,
                    format!("to={} reason={} path={what:?}", resolved.addr, e.reason()),
                );
                return error_json("400 Bad Request", e.code(), e.hint());
            }
        }
    };
    let n_files = attachments.len();
    // What the agent gets on a person's compose post with files: the
    // absolute paths ride the injected line (the agent reads from disk).
    let inject_text = match &workspace_root {
        Some(root) if via == "compose" && !bound => {
            let mut t = text.clone();
            for a in &attachments {
                if !t.is_empty() {
                    t.push('\n');
                }
                t.push_str(&format!("Attached: {}", root.join(&a.path).display()));
            }
            t
        }
        _ => text.clone(),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match overlay::post_thread_with_attachments(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        &from,
        &resolved.addr,
        &text,
        &via,
        origin.clone(),
        author,
        attachments,
    ) {
        Ok((item, links)) => {
            // thread-read-v1: a person's own post advances their own
            // cursor (they have read up to what they answered), before
            // the frames go out.
            crate::read_routes::note_own_thread_read(&conn, item.doc.author.as_ref(), &resolved.conversation_id, item.seq);
            crate::overlay_ws::emit_links(&links, &item.doc);
            drop(conn);
            if !bound && via == "compose" {
                // A person's prose never closes a pending card: they may
                // discuss it first and answer by tapping later. Only a tap,
                // a dismiss / `k2 thread void`, or the card's expiry close it.
                let turn = ThreadTurn::new(&resolved, &item.id, received_at);
                inject_thread_compose(&resolved, &from, &inject_text, &command, turn);
                // A widget's posts never fill the person's compose history
                // (UWB12a: a busy widget would flood the last 50). Each
                // one writes an audit line instead: widgets need no
                // permission (Rosson 2026-10-08), so the log is the record.
                if let Some(o) = &origin {
                    audit_widget_post(o, &from, &resolved.addr, &text);
                } else {
                    let _ = k2_core::workspace_compose_history::record_compose_send(
                        &resolved.project_id,
                        &text,
                        &from,
                    );
                }
            } else if request_skin().is_some() {
                inject_skin_thread(&resolved, &from, &text, ThreadTurn::new(&resolved, &item.id, received_at));
            } else {
                // TW4 (a): the agent answered in Thread (not the compose
                // bar, not an app guest): the strip ends with the reply.
                crate::thread_activity::note_reply(&resolved.conversation_id);
                // Thread push (§6.1): an agent reply may push; a human
                // `from` (an owner-token shell post) never does (TP14).
                push_agent_post(&resolved, &item.doc.from, item.seq, k2_core::push::ThreadMsgKind::Text, &text, n_files);
            }
            // thread-attachments-v1: the stored snapshots, so the sender
            // sees what landed (state `ok` at post time).
            let attachments_json = crate::thread_attachments::view_list(
                workspace_root.as_deref(),
                &item.doc.attachments,
                &crate::thread_attachments::Viewer::Full,
                false,
            );
            CliResponse::ok_json(with_moved_from(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "seq": item.seq,
                    "from": item.doc.from,
                    "to": item.doc.to,
                    "kind": item.doc.kind,
                    "body": item.doc.body,
                    "via": item.doc.via,
                    "widget": item.doc.widget,
                    "author": item.doc.author,
                    "attachments": attachments_json,
                    // The client stamps its own post with the server's time
                    // ("You · just now"), falling back to its send time.
                    "created_at": item.doc.created_at,
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                }),
                &resolved,
            ))
        }
        Err(e) => error_json("500 Internal Server Error", "store", e),
    }
}

/// Thread push trigger (prd-thread-push-notifications-v1 §6.1) for the
/// three agent-reply sites. App guests are never a push source (TP13).
/// `text` is the reply's text or a choice card's question (its push
/// preview, 0.46.1); a secret card passes `""` and never previews.
fn push_agent_post(
    resolved: &ResolvedOverlay,
    from: &str,
    seq: i64,
    kind: k2_core::push::ThreadMsgKind,
    text: &str,
    files: usize,
) {
    if request_skin().is_some() {
        return;
    }
    crate::thread_push::on_agent_post(
        &resolved.conversation_id,
        &resolved.project_id,
        &resolved.addr,
        from,
        seq,
        kind,
        text,
        files,
    );
}

fn reject_wait(params: &HashMap<String, String>) -> Result<(), CliResponse> {
    if bool_param(params, "wait") || opt_param(params, "timeout").is_some() {
        return Err(usage(
            "thread cards are fire-and-forget; no --wait / --timeout",
        ));
    }
    Ok(())
}

fn collect_ask_options(params: &HashMap<String, String>) -> Result<Vec<String>, CliResponse> {
    let options = opt_param(params, "options");
    let option = opt_param(params, "option");
    match (options.as_deref(), option.as_deref()) {
        (Some(_), Some(_)) => Err(usage("mix of --options and --option is not allowed")),
        (Some(raw), None) | (None, Some(raw)) => overlay::parse_options_value(raw).map_err(usage),
        (None, None) => Err(usage("ask requires --options or --option")),
    }
}



fn handle_ask(params: &HashMap<String, String>) -> CliResponse {
    if let Err(e) = reject_wait(params) {
        return e;
    }
    let addr = str_param(params, "addr");
    let prompt = {
        let p = str_param(params, "prompt");
        if p.is_empty() {
            str_param(params, "text")
        } else {
            p
        }
    };
    if addr.is_empty() {
        return usage("missing addr");
    }
    if prompt.trim().is_empty() {
        return usage("missing prompt");
    }
    let options = match collect_ask_options(params) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let allow_custom = bool_param(params, "allow_custom") || bool_param(params, "allow-custom");
    let expires_at = match choice_card_expiry(params, overlay::now_secs()) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let resolved = match resolve_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(r) = refuse_if_imported(&resolved, None) {
        return r;
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    let from = overlay_sender_from(params, &resolved);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, &from) {
        return e;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match overlay::post_choice(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        &from,
        &resolved.addr,
        prompt.trim(),
        options,
        allow_custom,
        expires_at,
    ) {
        Ok((item, links)) => {
            crate::overlay_ws::emit_links(&links, &item.doc);
            drop(conn);
            // TW4 (a): an agent's card is its Thread reply.
            crate::thread_activity::note_reply(&resolved.conversation_id);
            push_agent_post(&resolved, &item.doc.from, item.seq, k2_core::push::ThreadMsgKind::Choice, prompt.trim(), 0);
            let choice = item.doc.choice.as_ref();
            let labels: Vec<String> = choice
                .map(|c| c.options.iter().map(|o| o.label.clone()).collect())
                .unwrap_or_default();
            CliResponse::ok_json(with_moved_from(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "from": item.doc.from,
                    "prompt": choice.map(|c| c.prompt.clone()).unwrap_or_else(|| prompt.trim().to_string()),
                    "options": labels,
                    "allow_custom": choice.map(|c| c.allow_custom).unwrap_or(allow_custom),
                    "status": choice.map(|c| c.status.clone()).unwrap_or_else(|| "pending".to_string()),
                    "expiresAt": choice.and_then(|c| c.expires_at),
                    "kind": item.doc.kind,
                    "seq": item.seq,
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                }),
                &resolved,
            ))
        }
        Err(e) => error_json("400 Bad Request", "usage", e),
    }
}

fn handle_secret(params: &HashMap<String, String>) -> CliResponse {
    if let Err(e) = reject_wait(params) {
        return e;
    }
    let addr = str_param(params, "addr");
    let name = str_param(params, "name");
    if addr.is_empty() {
        return usage("missing addr");
    }
    if name.trim().is_empty() {
        return usage("secret --name is required");
    }
    let dest = opt_param(params, "dest").unwrap_or_else(|| "vault".to_string());
    if dest.trim() != "vault" {
        return usage("--dest vault only");
    }
    let prompt = opt_param(params, "prompt");
    // VS13: the prompt is plain text, at most 200 characters.
    if prompt.as_deref().is_some_and(|p| p.chars().count() > 200) {
        return usage("--prompt is at most 200 characters");
    }
    // K2 vault (prd-k2-vault-v1 §10.1, KV9): the proposal. Agents propose
    // exact hosts only (VS4); people can change everything on the card.
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return error_json("503 Service Unavailable", e.code, e.hint),
    };
    let proposal = match secret_proposal(params) {
        Ok(p) => p,
        Err(e) => return error_json("400 Bad Request", e.code, e.hint),
    };
    let resolved = match resolve_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(r) = refuse_if_imported(&resolved, None) {
        return r;
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    let from = overlay_sender_from(params, &resolved);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, &from) {
        return e;
    }
    // VS13: at most 3 pending secret cards per chat.
    match vault.pending_requests_in(&resolved.conversation_id) {
        Ok(n) if n >= k2_vault::vault::GUEST_PENDING_MAX => {
            return error_json(
                "409 Conflict",
                "too_many_pending_cards",
                "this chat already has 3 secret cards waiting; wait for an answer first",
            )
        }
        Ok(_) => {}
        Err(e) => return error_json("500 Internal Server Error", e.code, e.hint),
    }
    let ttl_secs = match opt_param(params, "ttl").as_deref().map(parse_ttl) {
        Some(Ok(t)) => Some(t),
        Some(Err(h)) => return usage(h),
        None => None,
    };
    let session_id = params.get("cell_session_id").cloned();
    match create_secret_card(&vault, &resolved, &from, name.trim(), prompt.as_deref(), proposal, session_id, ttl_secs, false) {
        Err(e) => e,
        Ok(item) => {
            let secret = item.doc.secret.as_ref();
            CliResponse::ok_json(with_moved_from(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "from": item.doc.from,
                    "name": secret.map(|s| s.name.clone()).unwrap_or_else(|| name.trim().to_string()),
                    "status": secret.map(|s| s.status.clone()).unwrap_or_else(|| "pending".to_string()),
                    "expiresAt": secret.and_then(|s| s.expires_at),
                    "kind": item.doc.kind,
                    "seq": item.seq,
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                }),
                &resolved,
            ))
        }
    }
}

/// Post a secret card and create its vault request together (§10.1).
/// Shared by `thread/secret` and the clone's leave-blank cards
/// (prd-clone-carries-threads-and-vault-v1 §6.6). `quiet`: no push and no
/// activity turn (a clone posts its cards in a batch, as history).
#[allow(clippy::too_many_arguments)]
fn create_secret_card(
    vault: &k2_vault::LocalVault,
    resolved: &ResolvedOverlay,
    from: &str,
    name: &str,
    prompt: Option<&str>,
    proposal: overlay::SecretProposal,
    session_id: Option<String>,
    ttl_secs: Option<i64>,
    quiet: bool,
) -> Result<overlay::OverlayItem, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) = overlay::post_secret(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        from,
        &resolved.addr,
        name,
        prompt,
        proposal.clone(),
        Some(overlay::now_secs() + k2_vault::vault::REQUEST_EXPIRY_DAYS * 24 * 60 * 60),
    )
    .map_err(|e| error_json("400 Bad Request", "usage", e))?;
    drop(conn);
    let req = k2_vault::vault::SecretRequest {
        card_id: item.id.clone(),
        conversation: resolved.conversation_id.clone(),
        session_id,
        workspace_id: resolved.project_id.clone(),
        requested_by: from.to_string(),
        name: name.to_string(),
        kind: proposal.kind.as_deref().map(k2_vault::Kind::parse).and_then(Result::ok).unwrap_or(k2_vault::Kind::ApiKey),
        proposed_scope: proposal.scope.clone().unwrap_or_else(|| "workspace".into()),
        proposed_hosts: proposal.hosts.clone(),
        proposed_delivery: proposal
            .delivery
            .as_deref()
            .map(crate::vault_routes::request_delivery)
            .and_then(Result::ok)
            .unwrap_or(k2_vault::Delivery::Fetch),
        prompt: prompt.map(str::to_string).unwrap_or_default(),
        ttl_secs,
        status: String::new(),
        answered_by: None,
        secret_id: None,
        created_at: String::new(),
        answered_at: None,
    };
    if let Err(e) = vault.request_create(req) {
        let _ = overlay::mark_secret(&resolved.conversation_id, &item.id, "voided", None, "card withdrawn".into(), None);
        return Err(error_json("400 Bad Request", e.code, e.hint));
    }
    crate::overlay_ws::emit_links(&links, &item.doc);
    if !quiet {
        crate::thread_activity::note_reply(&resolved.conversation_id);
        // A secret card pushes name-only, never its name or prompt (§7).
        push_agent_post(resolved, &item.doc.from, item.seq, k2_core::push::ThreadMsgKind::Secret, "", 0);
    }
    Ok(item)
}

/// Post a `handoff` card on the receiving workspace's Thread
/// (prd-agent-secret-handoff-v1 §10.1). `workspace` is the receiving
/// workspace's folder (or any address `resolve_addr` takes). Returns
/// `<conversation id>/<card id>` for the vault row. Names only.
pub(crate) fn post_handoff_card(
    workspace: &str,
    from: &str,
    body: &str,
    card: overlay::HandoffCardBody,
) -> Result<String, String> {
    let resolved = resolve_addr(workspace).map_err(|r| r.body)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) =
        overlay::post_handoff(&conn, &resolved.conversation_id, &resolved.project_id, from, &resolved.addr, body, card)?;
    // Publish under the write's lock (bus order = commit order, 0.46.3).
    crate::overlay_ws::emit_links(&links, &item.doc);
    drop(conn);
    // Name-only push, like a secret card (§7): never a preview.
    push_agent_post(&resolved, &item.doc.from, item.seq, k2_core::push::ThreadMsgKind::Secret, "", 0);
    Ok(format!("{}/{}", resolved.conversation_id, item.id))
}

/// Close a `handoff` card (`landed` / `declined` / `expired`) by the ref
/// [`post_handoff_card`] returned. A card already closed is a no-op.
pub(crate) fn close_handoff_card(card_ref: &str, status: &str, reference: Option<String>, by: Option<overlay::PostAuthor>) {
    let Some((conversation, card_id)) = card_ref.rsplit_once('/') else { return };
    match overlay::mark_handoff(conversation, card_id, status, reference, by) {
        Ok(cb) => publish_card_frame(&cb),
        Err(e) if e == overlay::CARD_NOT_PENDING => {}
        Err(e) => k2_core::log_debug!("[overlay] close handoff card {card_id}: {e}"),
    }
}

/// A clone's leave-blank card on its own Thread (`conversation_id`, owned
/// by `project_id`): from the clone itself, workspace scope. Outside the
/// VS13 3-pending cap (the caller caps clone cards at 10, CL17). Returns
/// the card id, or why it wasn't posted (names only).
#[allow(clippy::too_many_arguments)]
pub(crate) fn post_clone_secret_card(
    conversation_id: &str,
    project_id: &str,
    addr: &str,
    name: &str,
    kind: &str,
    hosts: &[String],
    delivery: &str,
    prompt: &str,
) -> Result<String, String> {
    let vault = crate::vault::get().map_err(|e| e.code.to_string())?;
    let resolved = ResolvedOverlay {
        conversation_id: conversation_id.to_string(),
        project_id: project_id.to_string(),
        addr: addr.to_string(),
        requested_addr: addr.to_string(),
        canonical_alias: true,
    };
    let proposal = overlay::SecretProposal {
        scope: Some("workspace".into()),
        hosts: hosts.to_vec(),
        delivery: Some(delivery.to_string()).filter(|d| !d.is_empty() && d != "none"),
        kind: Some(kind.to_string()),
    };
    create_secret_card(&vault, &resolved, addr, name, Some(prompt), proposal, None, None, true)
        .map(|item| item.id)
        .map_err(|r| r.body)
}

/// One plain Thread line on a clone's Thread (the leave-blank overflow).
pub(crate) fn post_clone_text_line(conversation_id: &str, project_id: &str, addr: &str, body: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match overlay::post_thread(&conn, conversation_id, project_id, addr, addr, body, "import") {
        Ok((item, links)) => {
            // Publish under the write's lock (bus order = commit order, 0.46.3).
            crate::overlay_ws::emit_links(&links, &item.doc);
            drop(conn);
        }
        Err(e) => k2_core::log_debug!("[clone-vault] overflow line not posted: {e}"),
    }
}

/// The card's proposal from `--scope/--for/--delivery/--kind` (KV9).
fn secret_proposal(params: &HashMap<String, String>) -> Result<overlay::SecretProposal, k2_vault::VaultError> {
    let scope = opt_param(params, "scope").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(s) = &scope {
        if !matches!(s.as_str(), "workspace" | "server" | "session") {
            return Err(k2_vault::VaultError::bad_request("bad_scope", "--scope is workspace, server or session"));
        }
    }
    let raw_hosts = opt_param(params, "for").or_else(|| opt_param(params, "hosts")).unwrap_or_default();
    let list: Vec<String> = if raw_hosts.trim_start().starts_with('[') {
        serde_json::from_str(&raw_hosts).map_err(|_| k2_vault::VaultError::bad_request("bad_host", "hosts must be a list"))?
    } else {
        raw_hosts.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect()
    };
    let hosts = k2_vault::hosts::normalise_list(&list, false)?;
    let delivery = opt_param(params, "delivery").map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    if let Some(d) = &delivery {
        if !matches!(d.as_str(), "fetch" | "file" | "env") {
            return Err(k2_vault::VaultError::bad_request("bad_delivery", "--delivery is fetch, file or env"));
        }
    }
    let kind = opt_param(params, "kind").map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    if let Some(k) = &kind {
        k2_vault::Kind::parse(k)?;
    }
    Ok(overlay::SecretProposal { scope, hosts, delivery, kind })
}

/// `--ttl 24h` / `90m` / `3600` → seconds (chat secrets).
fn parse_ttl(s: &str) -> Result<i64, &'static str> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('h') => (&s[..s.len() - 1], 3600),
        Some('m') => (&s[..s.len() - 1], 60),
        Some('d') => (&s[..s.len() - 1], 86400),
        Some('s') => (&s[..s.len() - 1], 1),
        _ => (s, 1),
    };
    let n: i64 = num.parse().map_err(|_| "--ttl is like 24h, 90m or 3600")?;
    if n <= 0 {
        return Err("--ttl must be positive");
    }
    Ok(n * mult)
}

fn card_id_of(params: &HashMap<String, String>) -> String {
    let id = str_param(params, "id");
    if id.is_empty() {
        str_param(params, "card_id")
    } else {
        id
    }
}

/// The person answering a secret card, in vault terms. Passports (agents
/// and K2 shell tabs) are refused (KV8).
fn card_answerer(params: &HashMap<String, String>, resolved: &ResolvedOverlay, session_author: &str) -> Result<k2_vault::Actor, CliResponse> {
    let passport = principal_is_bound(params)
        || crate::caller_workspace::principal_from_params(params).is_some()
        || params.contains_key("cell_session_id");
    if passport {
        return Err(error_json(
            "403 Forbidden",
            "secret_card_human_only",
            "secret cards are answered by a person, never by an agent or a K2 terminal",
        ));
    }
    if let Some(pass) = request_skin() {
        return Ok(k2_vault::Actor::Guest { username: pass.username.clone(), workspace_id: resolved.project_id.clone() });
    }
    if session_author == "owner" {
        return Ok(k2_vault::Actor::Owner);
    }
    let role = match k2_core::connect_users::role_for_user(session_author) {
        Some(k2_core::connect_users::Role::Owner) => k2_vault::Role::Owner,
        Some(k2_core::connect_users::Role::Admin) => k2_vault::Role::Admin,
        Some(_) => k2_vault::Role::Member,
        None => return Err(forbidden("that login no longer exists")),
    };
    Ok(k2_vault::Actor::Login { username: session_author.to_string(), role })
}

fn vault_error(e: &k2_vault::VaultError) -> CliResponse {
    let status = match e.status {
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    error_json(status, e.code, &e.hint)
}

/// A secret card answer: value → vault; the card turns `set` with the
/// reference; the agent's inject line names the reference, never a value.
fn answer_secret_card(params: &HashMap<String, String>, resolved: &ResolvedOverlay, card_id: &str, session_author: &str) -> CliResponse {
    let actor = match card_answerer(params, resolved, session_author) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return vault_error(&e),
    };
    let req = match vault.request_get(card_id) {
        Ok(r) => r,
        Err(e) => return vault_error(&e),
    };
    if req.workspace_id != resolved.project_id || req.conversation != resolved.conversation_id {
        return forbidden("that card belongs to another chat");
    }
    let Some(value) = opt_param(params, "secret").or_else(|| opt_param(params, "value")) else {
        return usage("secret value required");
    };
    let hosts = match opt_param(params, "hosts") {
        None => None,
        Some(h) if h.trim_start().starts_with('[') => match serde_json::from_str::<Vec<String>>(&h) {
            Ok(v) => Some(v),
            Err(_) => return usage("hosts must be a list"),
        },
        Some(h) => Some(h.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()),
    };
    let delivery = match opt_param(params, "delivery").map(|d| crate::vault_routes::request_delivery(&d)) {
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => return vault_error(&e),
        None => None,
    };
    let ans = k2_vault::vault::CardAnswer {
        scope: opt_param(params, "scope"),
        hosts,
        delivery,
        confirmed_warnings: opt_param(params, "confirm").is_some_and(|c| c == "1" || c == "true"),
    };
    let result = match vault.request_answer(card_id, k2_vault::SecretBytes::from_string(value), &ans, &actor) {
        Ok(r) => r,
        Err(e) => {
            // The vault found the request past its 7 days: the card says so.
            if e.code == "card_not_pending" && vault.request_get(card_id).is_ok_and(|r| r.status == "expired") {
                close_expired_card(&resolved.conversation_id, card_id);
            }
            return vault_error(&e);
        }
    };
    let m = &result.meta;
    let reference = crate::vault::display_ref(m).unwrap_or_else(|| result.reference.clone());
    let sites: Vec<String> = m
        .hosts
        .iter()
        .map(|h| h.split("://").nth(1).and_then(|x| x.rsplit_once(':')).map(|x| x.0.to_string()).unwrap_or_else(|| h.clone()))
        .collect();
    let inject_line = format!(
        "secret {} set → {} ({}{}). Use: {}",
        m.name,
        reference,
        m.default_delivery,
        if sites.is_empty() { String::new() } else { format!("; {}", sites.join(", ")) },
        crate::vault_routes::use_hint(m),
    );
    let cb = match overlay::mark_secret(&resolved.conversation_id, card_id, "set", Some(reference.clone()), inject_line, card_actor_author(params, session_author)) {
        Ok(cb) => cb,
        Err(e) => return error_json("409 Conflict", "card_not_pending", e),
    };
    let from = card_actor_from(params, resolved, session_author);
    note_card_reader(params, session_author, &cb);
    fire_card_callbacks(&resolved.addr, &from, vec![cb.clone()], true);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": cb.doc_id,
            "seq": cb.seq,
            "kind": cb.doc.kind,
            "status": "set",
            "name": m.name,
            "ref": reference,
            "trimmed": result.trimmed,
            "agentCanSee": m.agent_can_see(),
            "conversation_id": resolved.conversation_id,
            "addr": resolved.addr,
        })
        .to_string(),
    )
}

fn handle_answer(params: &HashMap<String, String>, session_author: &str) -> CliResponse {
    let addr = str_param(params, "addr");
    let card_id = card_id_of(params);
    if addr.is_empty() {
        return usage("missing addr");
    }
    if card_id.is_empty() {
        return usage("missing card id");
    }
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(r) = refuse_if_imported(&resolved, Some(&card_id)) {
        return r;
    }
    if let Err(e) = require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_POST) {
        return e;
    }
    // 0.46.3: a settings (action) card is the person's alone, checked
    // before any agent write rule so every agent (its own workspace's or
    // another's) gets the same refusal. Other choice cards are unchanged.
    match overlay::card_action(&resolved.conversation_id, &card_id) {
        Ok(Some(action)) => return answer_action_card(params, &resolved, &card_id, action, session_author),
        Ok(None) => {}
        Err(e) => return card_error(e),
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, "k2") {
        return e;
    }
    // 0.46.1: a Garden widget's card answer (`thread.answer`, cap
    // `thread:post`) carries `origin` like its posts, and the daemon checks
    // the placement's approved caps ([`widget_post_gate`]). A widget
    // answers with a choice only, never a secret (UW50).
    match widget_origin_param(params, request_skin().is_none() && !principal_is_bound(params), "card answer") {
        Ok(None) => {}
        Ok(Some((o, claim))) => {
            if opt_param(params, "secret").is_some() || opt_param(params, "value").is_some() {
                return usage("a Garden widget answers with a choice only, never a secret");
            }
            let from = card_actor_from(params, &resolved, session_author);
            if let Err(e) = widget_post_gate(&o, claim.as_ref(), &from, &resolved.addr) {
                return e;
            }
        }
        Err(e) => return e,
    }
    if let Some(r) = refuse_if_expired(&resolved, &card_id) {
        return r;
    }
    // KV8: only a person answers a secret card. Any session passport
    // (agent harness, plain shell tab, API cell) is refused, whichever
    // door it came through (TCP passport or the cell socket).
    if principal.is_some() {
        let is_secret_card = opt_param(params, "secret").is_some()
            || opt_param(params, "value").is_some()
            || match overlay::card_is_secret(&resolved.conversation_id, &card_id) {
                Ok(b) => b,
                Err(e) => return card_error(e),
            };
        if is_secret_card {
            return secret_card_human_only();
        }
    }
    // Secret cards are answered into the K2 vault (prd-k2-vault-v1 §10.1).
    if crate::vault::get().is_ok_and(|v| v.request_get(&card_id).is_ok()) {
        return answer_secret_card(params, &resolved, &card_id, session_author);
    }
    if opt_param(params, "secret").is_some() || opt_param(params, "value").is_some() {
        return error_json("409 Conflict", "card_not_pending", "that card isn't waiting for a secret");
    }
    let answer = opt_param(params, "answer").or_else(|| opt_param(params, "option"));
    match overlay::answer_card(
        &resolved.conversation_id,
        &resolved.project_id,
        &card_id,
        answer.as_deref(),
        None,
        card_actor_author(params, session_author),
    ) {
        Ok(cb) => {
            // TW1: a person answering a card starts a Thread turn (an
            // agent's own answer does not).
            let from = card_actor_from(params, &resolved, session_author);
            note_card_reader(params, session_author, &cb);
            fire_card_callbacks(&resolved.addr, &from, vec![cb.clone()], !principal_is_bound(params));
            let mut body = serde_json::json!({
                "ok": true,
                "id": cb.doc_id,
                "seq": cb.seq,
                "kind": cb.doc.kind,
                "conversation_id": resolved.conversation_id,
                "addr": resolved.addr,
            });
            if let Some(choice) = &cb.doc.choice {
                body["status"] = serde_json::json!(choice.status);
                if let Some(a) = &choice.answer {
                    body["answer"] = serde_json::json!(a);
                }
            }
            if let Some(secret_body) = &cb.doc.secret {
                body["status"] = serde_json::json!(secret_body.status);
                body["name"] = serde_json::json!(secret_body.name);
            }
            CliResponse::ok_json(with_moved_from(body, &resolved))
        }
        Err(e) => card_error(e),
    }
}

/// KV8 refusal: a session passport tried to answer a secret card.
fn secret_card_human_only() -> CliResponse {
    error_json(
        "403 Forbidden",
        "secret_card_human_only",
        "only a person can answer a secret card (from the Thread tab, a Connect login or an app); \
         agents and K2 terminal tabs cannot",
    )
}

// ── Action cards (0.46.3): agents present settings cards ────────────────
//
// Rosson 2026-10-20: "agents should be able to present cards to the user so
// that they don't have to go hunting around in the settings to make those
// changes." K2 posts a choice card carrying an action (id + params) on the
// person's Thread with an agent; one tap by the PERSON runs it
// ([`crate::action_cards::run`]). An agent, a K2 terminal, a cell, a Garden
// widget (its code may be agent-written) and an App guest are refused, so an
// agent can never answer its own card. Settings stays the place to undo.

/// Where [`present_action_card`] put the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Presented {
    /// Posted on the person's Thread with the agent at `addr`.
    Card { id: String, addr: String },
    /// One for the same thing is still waiting; nothing new was posted.
    Waiting { id: String },
    /// The workspace has no Thread (no pinned Chat) a person answers on.
    NoThread,
}

/// Post an action card on the person's Thread with the agent of
/// `project_id`: `prompt`, one button per `options`, 7 days to answer. One
/// waiting card per `action.id` + `action.dedupe`: with `replace` the
/// waiting one is voided and this (newer) one posted, else nothing new.
pub(crate) fn present_action_card(
    project_id: &str,
    prompt: &str,
    options: &[&str],
    action: overlay::CardAction,
    replace: bool,
) -> Result<Presented, String> {
    let conversation_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match pinned_chat_id(&conn, project_id) {
            Ok(Some(c)) => c,
            Ok(None) => return Ok(Presented::NoThread),
            Err(r) => return Err(format!("no Thread for {project_id}: {}", r.body)),
        }
    };
    let addr = k2_core::workspace_session_handles::workspace_address_name_shared(project_id)?;
    let now = overlay::now_secs();
    let waiting = overlay::pending_action_cards(&conversation_id, &action.id, &action.dedupe, now)?;
    if let Some(last) = waiting.last() {
        if !replace {
            return Ok(Presented::Waiting { id: last.clone() });
        }
        for id in &waiting {
            // A newer change: the old card no longer describes what's live.
            let cb = overlay::void_card(&conversation_id, project_id, id, None)?;
            publish_card_frame(&cb);
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) = overlay::post_choice_with_action(
        &conn,
        &conversation_id,
        project_id,
        &addr,
        &addr,
        prompt,
        options.iter().map(|o| o.to_string()).collect(),
        false,
        Some(now + overlay::DEFAULT_CARD_EXPIRY_SECS),
        Some(action),
    )?;
    crate::overlay_ws::emit_links(&links, &item.doc);
    drop(conn);
    let resolved = ResolvedOverlay {
        conversation_id,
        project_id: project_id.to_string(),
        addr: addr.clone(),
        requested_addr: addr.clone(),
        canonical_alias: true,
    };
    push_agent_post(&resolved, &addr, item.seq, k2_core::push::ThreadMsgKind::Choice, prompt, 0);
    Ok(Presented::Card { id: item.id, addr })
}

/// 403 `action_card_person_only`.
fn action_card_person_only(hint: &str) -> CliResponse {
    error_json("403 Forbidden", "action_card_person_only", hint)
}

/// Who may answer or dismiss a settings (action) card: the person, at the
/// floor of the Settings control it stands in for. Never wider than
/// Settings: "It's mine" there takes the owner token only, so a card takes
/// the owner token or an Owner Connect login (the phone, the web). An agent
/// passport (its own or another workspace's), a K2 terminal cell, a Garden
/// widget, an App guest, an Admin or a Member login is refused. `verb`:
/// "answers" or "dismisses". The daemon's own replace (a newer change voids
/// the waiting card) never comes through a route, so it isn't checked here.
fn require_action_card_person(params: &HashMap<String, String>, session_author: &str, verb: &str) -> Result<(), CliResponse> {
    let passport = principal_is_bound(params)
        || crate::caller_workspace::principal_from_params(params).is_some()
        || params.contains_key("cell_session_id");
    if passport {
        return Err(action_card_person_only(&format!(
            "this card changes your settings, so only you {verb} it (the Thread tab, an owner login); an agent or a K2 terminal never can"
        )));
    }
    if params.contains_key("origin") {
        return Err(action_card_person_only(&format!("a Garden widget never {verb} a settings card; do it in the Thread")));
    }
    if request_skin().is_some() {
        return Err(action_card_person_only(&format!("an App guest never {verb} a settings card")));
    }
    let owner = session_author == "owner"
        || matches!(k2_core::connect_users::role_for_user(session_author), Some(k2_core::connect_users::Role::Owner));
    if !owner {
        return Err(action_card_person_only(&format!(
            "only this server's owner {verb} a settings card (the same people as Settings → Gardens → Local Data)"
        )));
    }
    Ok(())
}

/// A person's tap on an action card: check it's a person who may change
/// this server's settings, check the option, run the action, then close
/// the card (the agent hears the choice like any card answer).
fn answer_action_card(
    params: &HashMap<String, String>,
    resolved: &ResolvedOverlay,
    card_id: &str,
    action: overlay::CardAction,
    session_author: &str,
) -> CliResponse {
    if let Err(r) = require_action_card_person(params, session_author, "answers") {
        return r;
    }
    if let Some(r) = refuse_if_expired(resolved, card_id) {
        return r;
    }
    let Some(answer) = opt_param(params, "answer").or_else(|| opt_param(params, "option")).map(|a| a.trim().to_string()).filter(|a| !a.is_empty())
    else {
        return usage("answer required");
    };
    let doc = match overlay::get_thread_doc(&resolved.conversation_id, card_id) {
        Ok(Some(d)) => d,
        Ok(None) => return not_found(format!("card not found: {card_id}")),
        Err(e) => return card_error(e),
    };
    let Some(choice) = doc.choice.as_ref() else {
        return usage("that card has no options");
    };
    if choice.status != "pending" {
        return card_error(overlay::CARD_NOT_PENDING.to_string());
    }
    if !choice.options.iter().any(|o| o.label == answer) {
        return usage(format!("'{answer}' is not an option on this card"));
    }
    let result = match crate::action_cards::run(&action, &answer, session_author) {
        Ok(r) => r,
        Err(e) => return error_json("409 Conflict", "action_failed", e),
    };
    match overlay::answer_card(
        &resolved.conversation_id,
        &resolved.project_id,
        card_id,
        Some(&answer),
        None,
        card_actor_author(params, session_author),
    ) {
        Ok(mut cb) => {
            // The card keeps what the action did (`choice.action.result`,
            // whose `message` every client shows under the chosen option),
            // and the frame that goes out carries it.
            match overlay::record_action_result(&resolved.conversation_id, card_id, result.clone()) {
                Ok(with_result) => cb.doc = with_result.doc,
                Err(e) => k2_core::log_debug!("[overlay] action card {card_id}: result not stored: {e}"),
            }
            let from = card_actor_from(params, resolved, session_author);
            note_card_reader(params, session_author, &cb);
            fire_card_callbacks(&resolved.addr, &from, vec![cb.clone()], true);
            CliResponse::ok_json(with_moved_from(
                serde_json::json!({
                    "ok": true,
                    "id": cb.doc_id,
                    "seq": cb.seq,
                    "kind": cb.doc.kind,
                    "status": "answered",
                    "answer": answer,
                    "action": { "id": action.id, "result": result },
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                }),
                resolved,
            ))
        }
        Err(e) => card_error(e),
    }
}

/// Map a card store error: a card that is no longer pending is 409
/// `card_not_pending` (its answer is final); anything else is usage.
fn card_error(e: String) -> CliResponse {
    if e == overlay::IMPORTED_READ_ONLY {
        return imported_read_only();
    }
    if e == overlay::CARD_NOT_PENDING {
        error_json(
            "409 Conflict",
            "card_not_pending",
            "this card was already answered or dismissed",
        )
    } else {
        error_json("400 Bad Request", "usage", e)
    }
}

fn handle_void(params: &HashMap<String, String>, session_author: &str) -> CliResponse {
    let addr = str_param(params, "addr");
    let card_id = card_id_of(params);
    if addr.is_empty() {
        return usage("missing addr");
    }
    if card_id.is_empty() {
        return usage("missing card id");
    }
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(r) = refuse_if_imported(&resolved, Some(&card_id)) {
        return r;
    }
    if let Err(e) = require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_POST) {
        return e;
    }
    // 0.46.3: dismissing a settings (action) card is the person's alone,
    // like answering it; other cards are unchanged.
    match overlay::card_action(&resolved.conversation_id, &card_id) {
        Ok(Some(_)) => {
            if let Err(r) = require_action_card_person(params, session_author, "dismisses") {
                return r;
            }
        }
        Ok(None) => {}
        Err(e) => return card_error(e),
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, "k2") {
        return e;
    }
    if let Some(r) = refuse_if_expired(&resolved, &card_id) {
        return r;
    }
    match overlay::void_card(&resolved.conversation_id, &resolved.project_id, &card_id, card_actor_author(params, session_author)) {
        Ok(cb) => {
            if cb.doc.secret.is_some() {
                void_vault_request(&card_id);
            }
            let from = card_actor_from(params, &resolved, session_author);
            note_card_reader(params, session_author, &cb);
            fire_card_callbacks(&resolved.addr, &from, vec![cb.clone()], !principal_is_bound(params));
            let status = cb
                .doc
                .choice
                .as_ref()
                .map(|c| c.status.clone())
                .or_else(|| cb.doc.secret.as_ref().map(|s| s.status.clone()))
                .unwrap_or_else(|| "voided".to_string());
            CliResponse::ok_json(with_moved_from(
                serde_json::json!({
                    "ok": true,
                    "id": cb.doc_id,
                    "status": status,
                    "seq": cb.seq,
                    "kind": cb.doc.kind,
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                }),
                &resolved,
            ))
        }
        Err(e) => card_error(e),
    }
}

/// A secret card voided in the Thread: its vault request goes too.
fn void_vault_request(card_id: &str) {
    if let Ok(v) = crate::vault::get() {
        if let Err(e) = v.request_void(card_id, &k2_vault::Actor::System("thread".into())) {
            k2_core::log_debug!("[vault] void request {card_id}: {}", e.code);
        }
    }
}

/// Human Message-the-agent on the Thread tab: same `[from <user>]` stamp
/// as Terminal inject, plus `[thread:<addr>]` so the agent can tell the
/// two throats apart. `addr` is the overlay address (`sales` or
/// `sales/reviewer`). Optional composer slash-command is prepended
/// (`/compact [from user] [thread:addr] text`).
fn format_thread_compose_pty_line(from: &str, addr: &str, text: &str) -> String {
    crate::workspace_msg::format_message_user(from, &format!("[thread:{addr}] {text}"))
}

fn format_thread_compose_pty_line_with_command(
    from: &str,
    addr: &str,
    text: &str,
    command: &str,
) -> Result<String, String> {
    crate::workspace_msg::format_message_user_with_command(
        from,
        &format!("[thread:{addr}] {text}"),
        command,
    )
}

/// TW1: the Thread turn a human inject starts: the triggering doc's id
/// and the post's ms receipt time (A23), on that doc's conversation.
#[derive(Debug, Clone, Copy)]
struct ThreadTurn<'a> {
    conversation_id: &'a str,
    turn_id: &'a str,
    started_at: i64,
}

impl<'a> ThreadTurn<'a> {
    fn new(resolved: &'a ResolvedOverlay, turn_id: &'a str, started_at: i64) -> Self {
        Self { conversation_id: &resolved.conversation_id, turn_id, started_at }
    }
}

fn inject_thread_compose(resolved: &ResolvedOverlay, from: &str, text: &str, command: &str, turn: ThreadTurn<'_>) {
    let payload = format!("[thread:{}] {text}", resolved.addr);
    let line = format_thread_compose_pty_line_with_command(from, &resolved.addr, text, command)
        .unwrap_or_else(|_| format_thread_compose_pty_line(from, &resolved.addr, text));
    record_test_inject(&line);
    // Same throat as k2 talk / Projects Chat / Feedback: wake a dormant
    // session, then inject+submit. `via=compose` skips Chatter (already on Thread).
    deliver_thread_to_pty(&resolved.addr, &payload, from, "compose", command, Some(turn));
}

/// Skin Thread post (default `via=thread`): same `[from user] [thread:addr]`
/// line as compose, delivered as `via=thread` so it is not compose-bar
/// (no slash-command, no compose-history). Skin `via=compose` stays 403.
fn inject_skin_thread(resolved: &ResolvedOverlay, from: &str, text: &str, turn: ThreadTurn<'_>) {
    let line = format_thread_compose_pty_line(from, &resolved.addr, text);
    record_test_inject(&line);
    deliver_thread_to_pty(
        &resolved.addr,
        &format!("[thread:{}] {text}", resolved.addr),
        from,
        "thread",
        "",
        Some(turn),
    );
}

/// A card's new status, to every open view of its Thread. Its `waiting`
/// count changed for everyone (thread-read-v1): the overlay publish sends
/// the read frames, and every person's phone may get a corrected badge.
fn publish_card_frame(cb: &CardCallback) {
    crate::overlay_ws::publish(OverlayFrame {
        rev: 0,
        collection: "thread".to_string(),
        seq: cb.seq,
        id: cb.doc_id.clone(),
        doc: Some(cb.doc.clone()),
        activity: None,
        conversation_id: Some(cb.conversation_id.clone()),
    });
    crate::badge_push::trigger_everyone();
}

/// thread-read-v1: a person who answered or dismissed a card has read the
/// Thread up to it (rounds toward read, like an own post). An agent's own
/// answer or an app guest's moves nobody's cursor.
fn note_card_reader(params: &HashMap<String, String>, session_author: &str, cb: &CardCallback) {
    if request_skin().is_some() {
        return;
    }
    let author = card_actor_author(params, session_author);
    let db = k2_core::db::shared();
    let conn = db.lock();
    crate::read_routes::note_own_thread_read(&conn, author.as_ref(), &cb.conversation_id, cb.seq);
}

/// `--expires 2h|3d|90m|3600` (from now), `--no-expiry` / `expires=never`
/// (open until answered or voided), else the 7-day default (Rosson
/// 2026-10-13: a card is not an open gate forever).
fn choice_card_expiry(params: &HashMap<String, String>, now: i64) -> Result<Option<i64>, CliResponse> {
    if bool_param(params, "no_expiry") || bool_param(params, "no-expiry") {
        return Ok(None);
    }
    match opt_param(params, "expires").map(|s| s.trim().to_ascii_lowercase()) {
        None => Ok(Some(now + overlay::DEFAULT_CARD_EXPIRY_SECS)),
        Some(s) if s.is_empty() => Ok(Some(now + overlay::DEFAULT_CARD_EXPIRY_SECS)),
        Some(s) if matches!(s.as_str(), "never" | "none" | "off") => Ok(None),
        Some(s) => parse_ttl(&s)
            .map(|secs| Some(now + secs))
            .map_err(|_| usage("--expires is like 2h, 3d, 90m or 3600 seconds (or --no-expiry)")),
    }
}

/// Check-on-touch before an answer or a void: a card past its time turns
/// `expired` now (its frame goes out) and the caller gets 409.
fn refuse_if_expired(resolved: &ResolvedOverlay, card_id: &str) -> Option<CliResponse> {
    match overlay::expire_if_due(&resolved.conversation_id, card_id, overlay::now_secs()) {
        Ok(Some(cb)) => {
            publish_card_frame(&cb);
            Some(error_json("409 Conflict", "card_not_pending", "this card expired"))
        }
        // Not due, or not found: the normal path answers.
        Ok(None) | Err(_) => None,
    }
}

/// A card whose time ran out (the sweep, or its vault request expired):
/// it turns `expired` and every open view updates. The agent is not woken
/// (nobody acted); it sees the card expired when it reads the Thread.
pub fn close_expired_card(conversation_id: &str, card_id: &str) {
    match overlay::expire_card(conversation_id, card_id) {
        Ok(cb) => publish_card_frame(&cb),
        Err(e) if e == overlay::CARD_NOT_PENDING => {}
        Err(e) => k2_core::log_debug!("[overlay] expire card {card_id}: {e}"),
    }
}

/// One expiry sweep: every pending card past its `expiresAt` turns
/// `expired`, with its frame. Returns how many.
pub fn sweep_expired_cards(now: i64) -> usize {
    match overlay::expire_due_cards(now) {
        Ok(cbs) => {
            for cb in &cbs {
                publish_card_frame(cb);
            }
            cbs.len()
        }
        Err(e) => {
            k2_core::log_debug!("[overlay] card expiry sweep: {e}");
            0
        }
    }
}

/// The daemon's card expiry loop (headless-safe, never a client timer):
/// a sweep at start, then every minute. Starts once per process.
pub fn spawn_card_expiry() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let r = std::thread::Builder::new().name("thread-card-expiry".into()).spawn(|| loop {
        sweep_expired_cards(overlay::now_secs());
        std::thread::sleep(std::time::Duration::from_secs(60));
    });
    if let Err(e) = r {
        k2_core::log_debug!("[overlay] could not start the card expiry sweep: {e}");
    }
}

/// `from`: who answered or dismissed the card ([`card_actor_from`]), so
/// the agent sees `[from <guest>]` for an app guest, never the owner.
/// `start_turns`: a person answered or dismissed the card (TW1).
fn fire_card_callbacks(addr: &str, from: &str, cbs: Vec<CardCallback>, start_turns: bool) {
    let started_at = chrono::Utc::now().timestamp_millis();
    for cb in cbs {
        publish_card_frame(&cb);
        let payload = format!("[thread:{addr}] {}", cb.inject_line);
        record_test_inject(&format_thread_compose_pty_line(from, addr, &cb.inject_line));
        let turn = start_turns.then_some(ThreadTurn {
            conversation_id: &cb.conversation_id,
            turn_id: &cb.doc_id,
            started_at,
        });
        deliver_thread_to_pty(addr, &payload, from, "thread", "", turn);
    }
}

/// Feedback / Projects Chat / `k2 talk`: `deliver_live(..., wake=true)`.
/// Overlay unit tests have no Tokio reactor; skip the live wake there.
///
/// With `turn`, the inject starts a Thread turn (TW1): `delivering` until
/// delivery returns, then it follows the v2 session that took the inject
/// (`target_session_id`), or ends `delivery_failed`.
fn deliver_thread_to_pty(
    addr: &str,
    payload: &str,
    from: &str,
    via: &str,
    command: &str,
    turn: Option<ThreadTurn<'_>>,
) -> ThreadDelivery {
    if cfg!(test) && tokio::runtime::Handle::try_current().is_err() {
        return test_delivery(addr, payload);
    }
    if let Some(t) = turn {
        crate::thread_activity::start_turn(t.conversation_id, addr, t.turn_id, t.started_at);
    }
    let resp = crate::workspace_msg::deliver_live_with_via(
        addr,
        payload,
        from,
        command,
        true,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
        via,
    );
    if !resp.success {
        k2_core::log_debug!(
            "[overlay] deliver_live failed addr={addr} via={via} reason={:?}",
            resp.reason
        );
    }
    if let Some(t) = turn {
        let target = resp.target_session_id.as_deref().filter(|_| resp.success);
        // A wake can re-pin the agent's Chat to a new conversation; the
        // strip follows the Thread the clients now read.
        let now_conversation = resolve_addr(addr).ok().map(|r| r.conversation_id);
        crate::thread_activity::delivered(t.conversation_id, t.turn_id, target, now_conversation.as_deref());
    }
    ThreadDelivery {
        success: resp.success,
        target_session_id: resp.target_session_id.filter(|_| resp.success),
    }
}

/// What [`deliver_thread_to_pty`] did: whether the wake + inject reached a
/// live session, and which one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ThreadDelivery {
    pub success: bool,
    pub target_session_id: Option<String>,
}

/// Unit tests have no Tokio reactor, so a delivery is recorded instead of
/// run (`(addr, payload)`), and succeeds only for addrs a test armed with
/// [`arm_test_delivery`].
#[cfg(test)]
static TEST_DELIVERIES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
#[cfg(test)]
static TEST_DELIVERY_OK: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn recorded_deliveries(addr: &str) -> Vec<String> {
    TEST_DELIVERIES
        .lock()
        .expect("deliveries lock")
        .iter()
        .filter(|(a, _)| a == addr)
        .map(|(_, p)| p.clone())
        .collect()
}

/// `ok`: later test deliveries to `addr` succeed (true) or fail (false).
#[cfg(test)]
pub(crate) fn arm_test_delivery(addr: &str, ok: bool) {
    let mut g = TEST_DELIVERY_OK.lock().expect("delivery ok lock");
    g.retain(|a| a != addr);
    if ok {
        g.push(addr.to_string());
    }
}

fn test_delivery(addr: &str, payload: &str) -> ThreadDelivery {
    #[cfg(test)]
    {
        TEST_DELIVERIES
            .lock()
            .expect("deliveries lock")
            .push((addr.to_string(), payload.to_string()));
        let ok = TEST_DELIVERY_OK.lock().expect("delivery ok lock").iter().any(|a| a == addr);
        ThreadDelivery {
            success: ok,
            target_session_id: ok.then(|| format!("test-session-{addr}")),
        }
    }
    #[cfg(not(test))]
    {
        let _ = (addr, payload);
        ThreadDelivery::default()
    }
}

/// Agent Thread `from`: room handle, never `k2`. UUID pin → handle via
/// `display_addr_for`; otherwise the canonical addr is already the handle.
fn thread_from_room_handle(resolved: &ResolvedOverlay) -> String {
    if k2_core::workspace_session_handles::is_uuid_shape(&resolved.addr) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        display_addr_for(&conn, &resolved.project_id, &resolved.conversation_id)
    } else {
        resolved.addr.clone()
    }
}

/// The address a conversation answers at now: the workspace handle for
/// the pinned Chat, `ws/<handle>` for a sidecar (S2; was the workspace
/// handle for every conversation, so a sidecar Terminal's card answer
/// went to the main Chat — Side finding B).
fn display_addr_for(
    conn: &rusqlite::Connection,
    project_id: &str,
    conversation_id: &str,
) -> String {
    match k2_core::workspace_session_handles::current_address_for(conn, project_id, conversation_id) {
        Ok(Some(addr)) => addr,
        _ => k2_core::workspace_session_handles::workspace_address_name(conn, project_id)
            .unwrap_or_else(|_| conversation_id.to_string()),
    }
}

/// `GET /cli/thread/activity?addr=` (TW9, A26): the conversation's live
/// Thread turn as its latest `activity` frame body, or `null`. A client
/// calls it on every overlay socket (re)open, because `since_seq` never
/// replays ephemeral frames. A Member read; app passes are refused by the
/// dispatcher (AP1) and again here.
fn handle_get_thread_activity(params: &HashMap<String, String>) -> CliResponse {
    if request_skin().is_some() {
        return forbidden("app passes cannot read thread activity");
    }
    let addr = str_param(params, "addr");
    if addr.is_empty() {
        return usage("missing addr");
    }
    let resolved = match resolve_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_read(principal.as_ref(), &resolved) {
        return e;
    }
    let turn = crate::thread_activity::current_turn(&resolved.conversation_id);
    CliResponse::ok_json(with_moved_from(
        serde_json::json!({
            "ok": true,
            "addr": resolved.addr,
            "conversation_id": resolved.conversation_id,
            "turn": turn,
        }),
        &resolved,
    ))
}

/// The outcome of a Design-notes post or redelivery (D-A10).
#[derive(Debug, Clone)]
pub(crate) struct DesignDelivery {
    pub thread_item_id: String,
    /// None when a retried item now sits in an earlier conversation.
    pub seq: Option<i64>,
    pub delivered: bool,
    pub target_session_id: Option<String>,
    pub addr: String,
    pub conversation_id: String,
    pub from: String,
}

/// Who a Design-notes post is from, from the credential: a bound agent's
/// stamp, else the session (owner display name or Connect login). Never
/// the body. Returns `(from, author, via)`.
fn design_sender(
    params: &HashMap<String, String>,
    resolved: &ResolvedOverlay,
    session_author: &str,
) -> (String, Option<overlay::PostAuthor>, &'static str) {
    if principal_is_bound(params) {
        (overlay_sender_from(params, resolved), None, "thread")
    } else {
        (
            compose_from_for_session(session_author),
            Some(compose_author_for_session(session_author)),
            "compose",
        )
    }
}

/// One audit line per Design-notes post or redelivery (`design.note.post`
/// / `design.note.redeliver`), like a Garden widget's: who, where, which
/// page, how many notes, how long. Never the text.
fn audit_design(event: &str, origin: &overlay::DesignOrigin, from: &str, addr: &str, text: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        from,
        format!(
            "to={addr} url={} notes={} item={} chars={}",
            origin.url,
            origin.notes,
            origin.item_id.as_deref().unwrap_or("-"),
            text.chars().count()
        ),
        "-",
        "-",
        "cli",
    ));
}

/// `POST /cli/browser/notes` core (prd-browser-design-mode-v1 D-A9/D-A10):
/// ONE Thread item with a `design` origin, then the same wake + inject a
/// person's Thread post gets (`[from <name>] [thread:<addr>] …` into the
/// pinned Chat, a Thread turn for the activity strip). Never send-message,
/// never a second item, never compose history. `text` is already clamped
/// and formatted (`k2_core::browser_notes`). App passes are refused.
pub(crate) fn design_post(
    params: &HashMap<String, String>,
    addr: &str,
    text: &str,
    origin: overlay::DesignOrigin,
    session_author: &str,
) -> Result<DesignDelivery, CliResponse> {
    let received_at = chrono::Utc::now().timestamp_millis();
    if request_skin().is_some() {
        return Err(forbidden("app passes cannot send design notes"));
    }
    if text.trim().is_empty() {
        return Err(usage("missing text"));
    }
    let resolved = resolve_addr(addr)?;
    let principal = crate::caller_workspace::principal_from_params(params);
    let (from, author, via) = design_sender(params, &resolved, session_author);
    authorize_write(principal.as_ref(), &resolved, &from)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) = overlay::post_thread_design(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        &from,
        &resolved.addr,
        text,
        via,
        origin.clone(),
        author,
    )
    .map_err(|e| error_json("500 Internal Server Error", "store", e))?;
    crate::read_routes::note_own_thread_read(&conn, item.doc.author.as_ref(), &resolved.conversation_id, item.seq);
    crate::overlay_ws::emit_links(&links, &item.doc);
    drop(conn);
    audit_design("design.note.post", &origin, &from, &resolved.addr, text);
    let delivery = deliver_design(&resolved, &item.id, &from, via, text, received_at);
    Ok(DesignDelivery {
        thread_item_id: item.id,
        seq: Some(item.seq),
        delivered: delivery.success,
        target_session_id: delivery.target_session_id,
        addr: resolved.addr.clone(),
        conversation_id: resolved.conversation_id.clone(),
        from,
    })
}

fn deliver_design(
    resolved: &ResolvedOverlay,
    item_id: &str,
    from: &str,
    via: &str,
    text: &str,
    started_at: i64,
) -> ThreadDelivery {
    record_test_inject(&format_thread_compose_pty_line(from, &resolved.addr, text));
    deliver_thread_to_pty(
        &resolved.addr,
        &format!("[thread:{}] {text}", resolved.addr),
        from,
        via,
        "",
        Some(ThreadTurn::new(resolved, item_id, started_at)),
    )
}

/// `POST /cli/browser/notes/redeliver` core (D-A10 Retry): deliver an
/// already-stored Design-notes item again. Never posts a second item. The
/// caller must be allowed to write this Thread and be the item's sender
/// (the owner token may retry anyone's).
pub(crate) fn design_redeliver(
    params: &HashMap<String, String>,
    addr: &str,
    thread_item_id: &str,
    session_author: &str,
) -> Result<DesignDelivery, CliResponse> {
    let started_at = chrono::Utc::now().timestamp_millis();
    if request_skin().is_some() {
        return Err(forbidden("app passes cannot send design notes"));
    }
    let id = thread_item_id.trim();
    if id.is_empty() {
        return Err(usage("missing threadItemId"));
    }
    let resolved = resolve_addr(addr)?;
    let principal = crate::caller_workspace::principal_from_params(params);
    let (caller_from, _, _) = design_sender(params, &resolved, session_author);
    authorize_write(principal.as_ref(), &resolved, &caller_from)?;
    let missing = || not_found(format!("no Thread item {id} in {}", resolved.addr));
    let (doc, seq) = match overlay::find_thread_item(&resolved.conversation_id, id)
        .map_err(|e| error_json("500 Internal Server Error", "store", e))?
    {
        Some(item) => (item.doc, Some(item.seq)),
        // A wake can re-pin the agent's Chat to a new conversation, and the
        // stored item stays in the one it was posted to. Find it by id and
        // check it is a Design post addressed to THIS workspace.
        None => {
            let doc = overlay::get_doc(id)
                .map_err(|e| error_json("500 Internal Server Error", "store", e))?
                .filter(|d| d.kind == "text" && d.design.is_some())
                .ok_or_else(missing)?;
            let same_room = doc.to.as_deref().is_some_and(|to| {
                to == resolved.addr || resolve_addr(to).is_ok_and(|r| r.project_id == resolved.project_id)
            });
            if !same_room {
                return Err(missing());
            }
            (doc, None)
        }
    };
    let Some(origin) = doc.design.clone() else {
        return Err(usage("only a design-notes post can be redelivered"));
    };
    let owner_token = !principal_is_bound(params)
        && (session_author.trim().is_empty() || session_author.trim().eq_ignore_ascii_case("owner"));
    if !owner_token && doc.from != caller_from {
        return Err(forbidden("only the sender can redeliver these notes"));
    }
    let text = doc.body.clone().unwrap_or_default();
    let via = doc.via.clone().unwrap_or_else(|| "compose".to_string());
    audit_design("design.note.redeliver", &origin, &caller_from, &resolved.addr, &text);
    let delivery = deliver_design(&resolved, &doc.id, &doc.from, &via, &text, started_at);
    Ok(DesignDelivery {
        thread_item_id: doc.id,
        seq,
        delivered: delivery.success,
        target_session_id: delivery.target_session_id,
        addr: resolved.addr.clone(),
        conversation_id: resolved.conversation_id.clone(),
        from: doc.from,
    })
}

/// `GET /cli/thread/attachment` (thread-attachments-v1): the Thread's own
/// read gate (an App guest: its room's pinned Chat and `thread:read`),
/// then `thread_attachments::handle_get` adds the viewer's file rights.
/// Agent passports are refused: an agent reads its workspace's files from
/// disk.
fn handle_get_attachment(params: &HashMap<String, String>) -> CliResponse {
    let addr = opt_param(params, "addr")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| opt_param(params, "conversationId"))
        .unwrap_or_default();
    if addr.trim().is_empty() {
        return usage("missing addr (or conversationId)");
    }
    if crate::caller_workspace::principal_from_params(params).is_some() {
        return forbidden("an agent reads attached files from disk; this door is for people");
    }
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // This route's refusals all carry a code (`cap_not_granted`, and the
    // resolver's own `skin_room`), so a phone never string-matches.
    if require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_READ).is_err() {
        return crate::thread_attachments::cap_refused(k2_core::skin::CAP_THREAD_READ);
    }
    let pass = request_skin();
    crate::thread_attachments::handle_get(
        params,
        &crate::thread_attachments::ResolvedThread {
            conversation_id: resolved.conversation_id.clone(),
            project_id: resolved.project_id.clone(),
        },
        pass.as_ref(),
    )
}

pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        "/cli/thread" => handle_get_thread(params),
        crate::thread_attachments::ROUTE => handle_get_attachment(params),
        "/cli/thread/latest" => handle_get_thread_latest(params),
        "/cli/thread/activity" => handle_get_thread_activity(params),
        "/cli/chatter" => handle_get_chatter(params),
        "/cli/chatterlog" => handle_get_chatterlog(params),
        "/cli/thread/post" | "/cli/thread/ask" | "/cli/thread/secret" | "/cli/thread/answer"
        | "/cli/thread/void" | catchup::ROUTE => CliResponse::method_not_allowed(),
        _ => return None,
    };
    Some(resp)
}

pub fn dispatch_post(path: &str, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    dispatch_post_as(path, params, body, "owner")
}

/// `session_author` is `"owner"` (host owner token) or a connect-user
/// username — same as project chat / feedback. Used only for
/// `via=compose` human posts (D3: never trust body `from`).
pub fn dispatch_post_as(
    path: &str,
    params: &HashMap<String, String>,
    body: &[u8],
    session_author: &str,
) -> CliResponse {
    // thread-read-v1: never served here (the dispatcher's person arm is
    // the only door); an agent's cell POST is refused.
    if crate::read_routes::is_read_route(path) {
        return crate::read_routes::not_a_person();
    }
    // inbox-catchup-v1: a read with a JSON body (never merged into params;
    // the stamped principal on `params` is the caller). TCP and the cell
    // socket call `catchup::handle` themselves with the classified caller;
    // this in-process door has none, so a stamped passport's project chat
    // entries are refused (fail closed).
    if path == catchup::ROUTE {
        let flight_key = params
            .get("cell_session_id")
            .map(|s| format!("cell:{s}"))
            .or_else(|| {
                crate::caller_workspace::principal_from_params(params)
                    .map(|p| format!("passport:{}:{}", p.workspace_uuid, p.agent_address))
            });
        return catchup::handle(params, body, None, flight_key.as_deref(), None);
    }
    let mut params = params.clone();
    // Stamp is already on this map (cell and HTTP stamp, then enter here).
    // Capture before merge_body. Do not capture the cell's pre-stamp form.
    let captured_from = params.get("from").cloned();
    let captured_bound = params
        .get(crate::caller_workspace::PRINCIPAL_BOUND_KEY)
        .cloned();
    let captured_project_id = params.get("project_id").cloned();
    let stamped = captured_bound.as_deref() == Some("1");
    merge_body(&mut params, body);
    if stamped {
        restore_param(&mut params, "from", captured_from);
        restore_param(
            &mut params,
            crate::caller_workspace::PRINCIPAL_BOUND_KEY,
            captured_bound,
        );
        restore_param(&mut params, "project_id", captured_project_id);
    } else {
        // A body principal_bound must not survive. Leave body `from` and
        // project_id for the no-principal path.
        restore_param(
            &mut params,
            crate::caller_workspace::PRINCIPAL_BOUND_KEY,
            captured_bound,
        );
    }
    match path {
        "/cli/thread/post" => handle_post(&params, session_author),
        "/cli/thread/ask" => handle_ask(&params),
        "/cli/thread/secret" => handle_secret(&params),
        "/cli/thread/answer" => handle_answer(&params, session_author),
        "/cli/thread/void" => handle_void(&params, session_author),
        _ => CliResponse::not_found(),
    }
}

fn restore_param(params: &mut HashMap<String, String>, key: &str, captured: Option<String>) {
    match captured {
        Some(value) => {
            params.insert(key.to_string(), value);
        }
        None => {
            params.remove(key);
        }
    }
}

fn merge_body(params: &mut HashMap<String, String>, body: &[u8]) {
    if body.is_empty() {
        return;
    }
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(obj) = v.as_object() {
            for (k, val) in obj {
                if let Some(s) = val.as_str() {
                    params.insert(k.clone(), s.to_string());
                } else if val.is_number() || val.is_boolean() {
                    params.insert(k.clone(), val.to_string());
                } else if val.is_array() || (val.is_object() && k == "origin") {
                    // `origin` (UWB12a) is the one object field a Thread
                    // post takes; it rides as its JSON text.
                    params.insert(k.clone(), val.to_string());
                }
            }
            return;
        }
    }
    for (k, v) in crate::routes::http::parse_form_body(body) {
        params.insert(k, v);
    }
}

/// Record a successful `k2 msg` / `k2 talk` / inject sibling as chatter.
/// Best-effort: a store error is logged and does not fail PTY delivery.
/// `delivery_id`: the send's `msg_deliveries` receipt (`m_…`), when it
/// wrote one; the doc carries it as `deliveryId`.
pub fn record_inject_chatter(
    workspace_token: &str,
    from: &str,
    text: &str,
    via: &str,
    delivery_id: Option<&str>,
) {
    let recipient = match resolve_addr(workspace_token) {
        Ok(r) => r,
        Err(_) => return,
    };
    let sender = resolve_local_sender(from);
    let sender_pair = sender
        .as_ref()
        .map(|s| (s.conversation_id.as_str(), s.project_id.as_str()));
    let db = k2_core::db::shared();
    let conn = db.lock();
    match overlay::record_chatter(
        &conn,
        &recipient.conversation_id,
        &recipient.project_id,
        sender_pair,
        from,
        workspace_token,
        text,
        via,
        "accepted",
        delivery_id,
    ) {
        Ok((doc, links)) => crate::overlay_ws::emit_links(&links, &doc),
        Err(e) => k2_core::log_debug!("[overlay] record chatter failed: {e}"),
    }
}

fn resolve_local_sender(from: &str) -> Option<ResolvedOverlay> {
    let from = from.trim();
    if from.is_empty() || from == "external" || from == "owner" || from == "k2" {
        return None;
    }
    resolve_addr(from).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seed(handle: &str) -> (String, String) {
        // Secret cards live in the vault: every overlay test has the
        // shared test vault (temp dir, fake key store).
        crate::vault::ensure_test_vault();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/ovl-route-{handle}-{id}");
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            params![id, handle, path],
        )
        .expect("seed project");
        (id, handle.to_string())
    }

    fn pin(project_id: &str, session_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::upsert(
            &conn,
            &format!("ws-{session_id}"),
            project_id,
            None,
            Some(session_id),
            "claude",
            "system",
            "running",
        )
        .expect("pin");
    }

    fn sidecar(project_id: &str, conv: &str, slug: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace_session_handles::allocate_ordinal(&conn, project_id, conv)
            .expect("ordinal");
        conn.execute(
            "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
             VALUES ('claude', ?1, ?2, 0, unixepoch()) \
             ON CONFLICT(provider, session_id) DO UPDATE SET custom_name = ?2",
            params![conv, slug],
        )
        .expect("name");
        conn.execute(
            "INSERT INTO workspace_tab_sessions \
             (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch()) \
             ON CONFLICT(project_id, pane_group_id) DO UPDATE SET session_id = excluded.session_id",
            params![
                project_id,
                format!("pane-{conv}"),
                format!("tab-pane-{conv}"),
                conv
            ],
        )
        .expect("tab");
    }

    fn params_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn json_body(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body)
            .unwrap_or_else(|e| panic!("response JSON parse failed: {e}; body={}", resp.body))
    }

    fn post_json(path: &str, params: &HashMap<String, String>, body: serde_json::Value) -> CliResponse {
        dispatch_post(path, params, body.to_string().as_bytes())
    }

    fn passport_params(project_id: &str, from: &str) -> HashMap<String, String> {
        params_of(&[
            (crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"),
            ("project_id", project_id),
            ("from", from),
        ])
    }

    fn assert_error_code(resp: &CliResponse, status: &str, code: &str) {
        assert_eq!(resp.status, status, "{}", resp.body);
        let v = json_body(resp);
        assert_eq!(v["error"]["code"], code, "{v}");
    }

    /// KV8: a session passport (agent, shell tab, API cell) never answers
    /// a secret card — stamped params (cell socket / TCP stamp) and the
    /// request-scoped principal slot both refuse, with or without a value,
    /// and nothing reaches the vault. A person (no passport) still can,
    /// and a second answer is 409 with the first value kept.
    #[test]
    fn secret_card_answer_is_human_only_and_final() {
        use k2_vault::SecretStore as _;
        let vault = crate::vault::ensure_test_vault();
        let handle = format!("ovlkv8{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let name = "KV8_TOKEN";
        let posted = post_json(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "name": name, "for": "api.example.com", "from": "k2" }),
        );
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let id = json_body(&posted)["id"].as_str().expect("id").to_string();

        for key in ["secret", "value"] {
            let mut body = serde_json::json!({ "addr": handle, "id": id });
            body[key] = serde_json::json!("agent-typed");
            let resp = post_json(
                "/cli/thread/answer",
                &passport_params(&project_id, &handle),
                body,
            );
            assert_error_code(&resp, "403 Forbidden", "secret_card_human_only");
        }
        let no_value = post_json(
            "/cli/thread/answer",
            &passport_params(&project_id, &handle),
            serde_json::json!({ "addr": handle, "id": id }),
        );
        assert_error_code(&no_value, "403 Forbidden", "secret_card_human_only");
        let slot = crate::caller_workspace::with_request_principal(
            Some(crate::session_token::HookPrincipal {
                workspace_uuid: project_id.clone(),
                agent_address: handle.clone(),
            }),
            || {
                post_json(
                    "/cli/thread/answer",
                    &HashMap::new(),
                    serde_json::json!({ "addr": handle, "id": id, "secret": "slot-typed" }),
                )
            },
        );
        assert_error_code(&slot, "403 Forbidden", "secret_card_human_only");
        assert!(
            !k2_core::overlay::vault::exists(&project_id, name),
            "no refused answer may reach the vault"
        );

        let human = post_json(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": id, "secret": "human-typed" }),
        );
        assert_eq!(human.status, "200 OK", "{}", human.body);
        assert_eq!(json_body(&human)["status"], "set");

        let again = post_json(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": id, "secret": "second-try" }),
        );
        assert_error_code(&again, "409 Conflict", "card_not_pending");
        let void_after = post_json(
            "/cli/thread/void",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": id }),
        );
        assert_error_code(&void_after, "409 Conflict", "card_not_pending");
        let got = vault
            .resolve(
                &k2_vault::Selector::user(k2_vault::Scope::workspace(project_id.clone()), name),
                k2_vault::Use::System,
                &k2_vault::Requester::System("test".into()),
            )
            .expect("the vault holds the first human answer");
        assert_eq!(got.value.expose(), b"human-typed", "the first human answer is final");
        assert!(!k2_core::overlay::vault::exists(&project_id, name), "nothing in the old folder");
    }

    /// KV8 is scoped to secret cards: an agent may still tap its own
    /// choice card (unchanged behaviour).
    #[test]
    fn passport_may_still_answer_a_choice_card() {
        let handle = format!("ovlkv8c{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let ask = post_json(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "Ship it?",
                "options": "Go,Stop",
                "from": "k2",
            }),
        );
        assert_eq!(ask.status, "200 OK", "{}", ask.body);
        let id = json_body(&ask)["id"].as_str().expect("id").to_string();
        let tap = post_json(
            "/cli/thread/answer",
            &passport_params(&project_id, &handle),
            serde_json::json!({ "addr": handle, "id": id, "answer": "Go" }),
        );
        assert_eq!(tap.status, "200 OK", "{}", tap.body);
        assert_eq!(json_body(&tap)["status"], "answered");
    }

    // ── action cards (0.46.3): agents present settings cards ──────────────

    fn keep_action(widget: &str, hash: &str) -> overlay::CardAction {
        overlay::CardAction {
            id: crate::action_cards::KEEP_WIDGET.to_string(),
            params: serde_json::json!({ "widget": widget, "hash": hash, "agent": "builder" }),
            dedupe: widget.to_string(),
            result: None,
        }
    }

    const KEEP_OPTIONS: [&str; 2] = [crate::action_cards::KEEP_AS_MINE, "Leave it as builder's"];

    fn maker(widget: &str, hash: &str) -> k2_core::garden_data::MadeBy {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::garden_data::authors::maker_of(&conn, widget, hash).expect("maker")
    }

    fn action_card_status(conv: &str, id: &str) -> String {
        overlay::get_thread_doc(conv, id).expect("doc").expect("card").choice.expect("choice").status
    }

    /// The agent can never answer its own settings card: its passport (the
    /// stamped params and the request principal slot), a K2 terminal cell
    /// and a Garden widget are all refused, the card stays pending and
    /// nothing is kept. The person's tap keeps the widget, once.
    #[test]
    fn an_action_card_is_answered_by_the_person_never_the_agent() {
        let handle = format!("ovlact{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let widget = format!("kw{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let hash = "a".repeat(64);
        let id = match present_action_card(&project_id, "builder changed it. Keep it?", &KEEP_OPTIONS, keep_action(&widget, &hash), true)
            .expect("present")
        {
            Presented::Card { id, addr } => {
                assert_eq!(addr, handle);
                id
            }
            other => panic!("a pinned Thread gets a card: {other:?}"),
        };
        let doc = overlay::get_thread_doc(&conv, &id).expect("doc").expect("card on the pinned Thread");
        let choice = doc.choice.expect("choice");
        assert_eq!(choice.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), KEEP_OPTIONS.to_vec());
        assert_eq!(choice.action.as_ref().map(|a| a.id.as_str()), Some(crate::action_cards::KEEP_WIDGET));
        assert!(choice.expires_at.is_some(), "a settings card expires like any card");

        let body = serde_json::json!({ "addr": handle, "id": id, "answer": crate::action_cards::KEEP_AS_MINE });
        // The agent's own passport (stamped params).
        let r = post_json("/cli/thread/answer", &passport_params(&project_id, &handle), body.clone());
        assert_error_code(&r, "403 Forbidden", "action_card_person_only");
        // The agent's passport in the request slot (TCP).
        let r = crate::caller_workspace::with_request_principal(
            Some(crate::session_token::HookPrincipal { workspace_uuid: project_id.clone(), agent_address: handle.clone() }),
            || post_json("/cli/thread/answer", &HashMap::new(), body.clone()),
        );
        assert_error_code(&r, "403 Forbidden", "action_card_person_only");
        // A K2 terminal cell.
        let r = post_json("/cli/thread/answer", &params_of(&[("cell_session_id", "cell-1")]), body.clone());
        assert_error_code(&r, "403 Forbidden", "action_card_person_only");
        // A Garden widget (its code may be an agent's).
        let r = post_json(
            "/cli/thread/answer",
            &params_of(&[("origin", r#"{"widget":"board","garden":"g-1","placement":"p1"}"#)]),
            body.clone(),
        );
        assert_error_code(&r, "403 Forbidden", "action_card_person_only");
        assert_eq!(action_card_status(&conv, &id), "pending", "a refused answer leaves the card waiting");
        assert_eq!(maker(&widget, &hash), k2_core::garden_data::MadeBy::Unclaimed, "nothing was kept");

        // Not an option: refused before anything runs.
        let r = post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": handle, "id": id, "answer": "Sure" }));
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert_eq!(maker(&widget, &hash), k2_core::garden_data::MadeBy::Unclaimed);

        // The person (owner token): one tap keeps it.
        let r = post_json("/cli/thread/answer", &HashMap::new(), body.clone());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = json_body(&r);
        assert_eq!((v["status"].as_str(), v["action"]["id"].as_str()), (Some("answered"), Some(crate::action_cards::KEEP_WIDGET)), "{v}");
        assert_eq!(v["action"]["result"]["kept"], true, "{v}");
        assert_eq!(v["action"]["result"]["message"], "Kept as yours. Undo in Settings → Gardens → Local Data.", "{v}");
        // The card itself keeps it (every client renders it under the option).
        let stored = overlay::get_thread_doc(&conv, &id).expect("doc").expect("card").choice.expect("choice");
        assert_eq!(
            stored.action.and_then(|a| a.result).map(|r| r["message"].clone()),
            Some(serde_json::json!("Kept as yours. Undo in Settings → Gardens → Local Data."))
        );
        assert_eq!(maker(&widget, &hash), k2_core::garden_data::MadeBy::Owner);
        assert_eq!(action_card_status(&conv, &id), "answered");
        let r = post_json("/cli/thread/answer", &HashMap::new(), body);
        assert_error_code(&r, "409 Conflict", "card_not_pending");
    }

    /// "Leave it as builder's" keeps nothing; a login that isn't this
    /// server's owner can't answer a settings card.
    #[test]
    fn let_sync_keeps_nothing_and_a_member_cannot_answer() {
        let handle = format!("ovlact2{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let widget = format!("kw{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let hash = "b".repeat(64);
        let Presented::Card { id, .. } = present_action_card(&project_id, "Keep it?", &KEEP_OPTIONS, keep_action(&widget, &hash), true).expect("present")
        else {
            panic!("a pinned Thread gets a card")
        };
        let body = serde_json::json!({ "addr": handle, "id": id, "answer": "Leave it as builder's" });
        let r = dispatch_post_as("/cli/thread/answer", &HashMap::new(), body.to_string().as_bytes(), "no-such-login");
        assert_error_code(&r, "403 Forbidden", "action_card_person_only");
        let r = post_json("/cli/thread/answer", &HashMap::new(), body);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(json_body(&r)["action"]["result"]["kept"], false);
        assert_eq!(json_body(&r)["action"]["result"]["message"], "Left as builder's.");
        assert_eq!(maker(&widget, &hash), k2_core::garden_data::MadeBy::Unclaimed);
    }

    /// One waiting card per thing: without `replace` the waiting card is
    /// returned; with it the old card is voided and a new one posted. No
    /// pinned Chat: NoThread (the caller falls back to an inbox note).
    #[test]
    fn one_waiting_action_card_per_thing_and_no_thread_says_so() {
        let handle = format!("ovlact3{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        assert_eq!(
            present_action_card(&project_id, "Keep it?", &KEEP_OPTIONS, keep_action("kwx", &"c".repeat(64)), true).expect("present"),
            Presented::NoThread
        );
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let Presented::Card { id: first, .. } =
            present_action_card(&project_id, "Keep it?", &KEEP_OPTIONS, keep_action("kwx", &"c".repeat(64)), true).expect("present")
        else {
            panic!("card")
        };
        assert_eq!(
            present_action_card(&project_id, "Keep it?", &KEEP_OPTIONS, keep_action("kwx", &"d".repeat(64)), false).expect("present"),
            Presented::Waiting { id: first.clone() }
        );
        let Presented::Card { id: second, .. } =
            present_action_card(&project_id, "Keep it?", &KEEP_OPTIONS, keep_action("kwx", &"d".repeat(64)), true).expect("present")
        else {
            panic!("card")
        };
        assert_ne!(first, second);
        assert_eq!(action_card_status(&conv, &first), "voided", "a newer change replaces the waiting card");
        assert_eq!(action_card_status(&conv, &second), "pending");
        // Another widget is another thing.
        assert!(matches!(
            present_action_card(&project_id, "Keep it?", &KEEP_OPTIONS, keep_action("kwy", &"c".repeat(64)), false).expect("present"),
            Presented::Card { .. }
        ));
    }

    #[test]
    fn get_thread_post_is_405() {
        let resp = dispatch("/cli/thread/post", &HashMap::new())
            .expect("GET /cli/thread/post must be handled");
        assert_eq!(
            resp.status, "405 Method Not Allowed",
            "GET mutation must 405, got {} body={}",
            resp.status, resp.body
        );
    }

    #[test]
    fn thread_write_then_read_json_has_from_and_seq() {
        let handle = format!("ovlsales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);

        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": "hi",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(post.status, "200 OK", "post failed: {}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["ok"], true, "{posted}");
        assert_eq!(
            posted["from"], handle,
            "empty/k2 from stamps the room handle: {posted}"
        );
        assert!(
            posted["seq"].as_i64().is_some(),
            "json must have seq: {posted}"
        );
        assert_eq!(posted["conversation_id"], conv);

        let get =
            dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread");
        assert_eq!(get.status, "200 OK", "{}", get.body);
        let snap = json_body(&get);
        let items = snap["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1, "{snap}");
        assert_eq!(items[0]["doc"]["body"], "hi");
        assert_eq!(items[0]["doc"]["from"], handle);
        assert_eq!(items[0]["seq"], posted["seq"]);
        let created_at = posted["created_at"].as_i64().expect("post json must carry created_at (ms)");
        assert!(created_at > 0, "created_at must be the stored time: {posted}");
        assert_eq!(items[0]["doc"]["created_at"], posted["created_at"]);
    }

    #[test]
    fn compose_via_thread_post_records_workspace_history() {
        // Reads `$HOME` (the owner display name): hold the env lock so a
        // concurrent temp-HOME test cannot swap it mid-test.
        let _home_lock = crate::test_support::lock_home();
        let handle = format!("ovlhist{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let body = format!("compose-hist-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": body,
                "via": "compose",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(post.status, "200 OK", "post failed: {}", post.body);
        let hist = k2_core::workspace_compose_history::list_compose_send_history(&project_id)
            .expect("list compose history");
        assert!(
            hist.iter().any(|e| e.body == body),
            "via=compose must record workspace compose-history, got {hist:?}"
        );
        let author = crate::workspace_msg::resolve_owner_from();
        let posted = json_body(&post);
        assert_eq!(
            posted["from"].as_str().expect("from"),
            author.as_str(),
            "via=compose must stamp the session actor, not body from; {posted}"
        );
        let want = format_thread_compose_pty_line(&author, &handle, &body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &want),
            "compose must inject [from user] [thread:addr] msg into the PTY; want {want:?} got {injects:?}"
        );
    }

    #[test]
    fn widget_post_stores_origin_wakes_the_agent_and_skips_compose_history() {
        // Reads `$HOME` (the owner display name): hold the env lock so a
        // concurrent temp-HOME test cannot swap it mid-test.
        let _home_lock = crate::test_support::lock_home();
        // The audit line goes to `$HOME/.k2/auth-audit.jsonl`: a temp HOME.
        let _home = crate::test_support::TempHome::new();
        let handle = format!("ovlwidget{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let body = format!("from-arcade-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": body,
                "via": "compose",
                "origin": {"widget": "Agent Arcade", "garden": "g-test0001"},
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(post.status, "200 OK", "post failed: {}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["via"], "compose", "still the person's own post: {posted}");
        assert_eq!(
            posted["widget"],
            serde_json::json!({"widget": "Agent Arcade", "garden": "g-test0001"})
        );

        let get = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread");
        let snap = json_body(&get);
        let item = snap["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["doc"]["body"] == body.as_str())
            .unwrap_or_else(|| panic!("stored post missing: {snap}"))
            .clone();
        assert_eq!(item["doc"]["widget"]["widget"], "Agent Arcade", "{item}");
        assert_eq!(item["doc"]["widget"]["garden"], "g-test0001", "{item}");

        let hist = k2_core::workspace_compose_history::list_compose_send_history(&project_id)
            .expect("compose history");
        assert!(
            hist.iter().all(|e| e.body != body),
            "a widget post must not enter compose history: {hist:?}"
        );
        // One audit line per widget post (no permissions, so the log is the
        // record), without the text.
        let audit = k2_core::auth_audit::tail(200).expect("audit tail");
        let line = audit
            .iter()
            .find(|e| e["event"] == "zen.widget.post" && e["outcome"].as_str().is_some_and(|o| o.contains(&handle)))
            .unwrap_or_else(|| panic!("no zen.widget.post audit line for {handle}: {audit:?}"));
        let outcome = line["outcome"].as_str().expect("outcome");
        assert!(outcome.contains("widget=\"Agent Arcade\"") && outcome.contains("garden=g-test0001"), "{line}");
        assert!(outcome.contains(&format!("chars={}", body.chars().count())), "{line}");
        assert!(!outcome.contains(&body), "the audit line never holds the text: {line}");
        let author = crate::workspace_msg::resolve_owner_from();
        let want = format_thread_compose_pty_line(&author, &handle, &body);
        assert!(
            recorded_injects().iter().any(|l| l == &want),
            "a widget post wakes the agent like the person's own post"
        );
    }

    #[test]
    fn widget_origin_is_refused_off_the_persons_own_post_or_when_malformed() {
        let handle = format!("ovlworig{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let origin = serde_json::json!({"widget": "Agent Arcade", "garden": "g-test0001"});
        let cases = [
            serde_json::json!({"addr": handle, "text": "a", "origin": origin}),
            serde_json::json!({"addr": handle, "text": "b", "via": "thread", "origin": origin}),
            serde_json::json!({"addr": handle, "text": "c", "via": "compose", "origin": {"widget": "A"}}),
            serde_json::json!({"addr": handle, "text": "d", "via": "compose", "origin": {"widget": "", "garden": "g"}}),
            serde_json::json!({"addr": handle, "text": "e", "via": "compose", "origin": {"widget": "A", "garden": "g", "x": 1}}),
            serde_json::json!({"addr": handle, "text": "f", "via": "compose", "origin": "Agent Arcade"}),
        ];
        for body in cases {
            let post = dispatch_post("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes());
            assert_eq!(post.status, "400 Bad Request", "{body} -> {}", post.body);
        }
        // A bound agent caller can't claim to be a widget.
        let mut params = HashMap::new();
        params.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        params.insert("project_id".to_string(), project_id.clone());
        params.insert("from".to_string(), handle.clone());
        let post = post_as_owner(
            &params,
            serde_json::json!({"addr": handle, "text": "g", "via": "compose", "origin": origin}),
        );
        assert_eq!(post.status, "400 Bad Request", "{}", post.body);
        let get = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread");
        let snap = json_body(&get);
        assert_eq!(
            snap["items"].as_array().map(Vec::len),
            Some(0),
            "nothing refused was stored: {snap}"
        );
    }

    // ── 0.46.1: the daemon's own `thread:post` check for Garden widgets ──

    /// The Garden page: custom placement `b1` of widget `board`, and `b2`
    /// of K2's built-in `k2:diary@1`.
    const WIDGET_PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n\n[[widget]]\nid = \"b2\"\nkind = \"custom\"\nwidget = \"k2:diary@1\"\ncolumn = 0\n";

    /// A temp HOME whose Zen folder's first Garden places `board` (asking
    /// for `caps`) and the built-in diary, plus a pinned agent to post to.
    struct WidgetGarden {
        _home: crate::test_support::TempHome,
        f: k2_core::zen::ZenFiles,
        garden: String,
        handle: String,
        project_id: String,
    }

    impl WidgetGarden {
        fn new(caps: &[&str]) -> Self {
            let home = crate::test_support::TempHome::new();
            let f = k2_core::zen::ZenFiles::local();
            f.setup().expect("zen setup");
            f.new_widget("board", Some("hello")).expect("widget folder");
            let handle = format!("ovlwgate{}", &uuid::Uuid::new_v4().to_string()[..8]);
            let (project_id, _) = seed(&handle);
            pin(&project_id, &uuid::Uuid::new_v4().to_string());
            let g = WidgetGarden { _home: home, garden: f.gardens()[0].id.clone(), f, handle, project_id };
            g.set_caps(caps);
            std::fs::write(g.f.path_of(&k2_core::zen::ZenFile::Garden(g.garden.clone())), WIDGET_PAGE)
                .expect("garden page");
            g
        }

        fn set_caps(&self, caps: &[&str]) {
            let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
            let sources = if caps.iter().any(|c| c == "\"sources:read\"") { ",\n  \"sources\": \"config\"" } else { "" };
            let manifest = format!(
                "{{\n  \"schema\": 1,\n  \"name\": \"Board\",\n  \"entry\": \"index.html\",\n  \"caps\": [{}]{sources}\n}}\n",
                caps.join(", ")
            );
            std::fs::write(self.f.widget_dir("board").join("manifest.json"), manifest).expect("manifest");
        }

        fn hash(&self, widget: &str) -> String {
            let h = self.f.garden_widget_status(&self.garden, widget).hash().to_string();
            assert_eq!(h.len(), 64, "{widget} bundles clean");
            h
        }

        fn origin(&self, placement: &str, widget: &str) -> serde_json::Value {
            serde_json::json!({
                "widget": widget,
                "garden": self.garden,
                "placement": placement,
                "hash": self.hash(widget),
            })
        }

        fn post(&self, text: &str, origin: Option<serde_json::Value>) -> CliResponse {
            let mut body = serde_json::json!({ "addr": self.handle, "text": text, "via": "compose" });
            if let Some(o) = origin {
                body["origin"] = o;
            }
            dispatch_post("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes())
        }

        fn bodies(&self) -> Vec<String> {
            let get = dispatch("/cli/thread", &params_of(&[("addr", self.handle.as_str())])).expect("GET thread");
            json_body(&get)["items"]
                .as_array()
                .expect("items")
                .iter()
                .filter_map(|i| i["doc"]["body"].as_str().map(str::to_string))
                .collect()
        }
    }

    fn refused_code(resp: &CliResponse) -> String {
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        json_body(resp)["error"]["code"].as_str().expect("error.code").to_string()
    }

    #[test]
    fn approved_widget_with_thread_post_posts_and_the_doc_keeps_the_label_only() {
        let g = WidgetGarden::new(&["agents:read", "thread:read", "thread:post"]);
        let post = g.post("from the board", Some(g.origin("b1", "board")));
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["via"], "compose", "{posted}");
        assert_eq!(
            posted["widget"],
            serde_json::json!({"widget": "board", "garden": g.garden}),
            "placement and hash are checked, never stored: {posted}"
        );
        assert_eq!(g.bodies(), vec!["from the board".to_string()]);
    }

    #[test]
    fn approved_widget_without_thread_post_is_refused_widget_cap_missing() {
        let g = WidgetGarden::new(&["agents:read", "thread:read"]);
        assert_eq!(refused_code(&g.post("nope", Some(g.origin("b1", "board")))), "widget_cap_missing");
        assert!(g.bodies().is_empty(), "nothing refused was stored");
        // 0.46.1 (Rosson 2026-10-17): a widget that reads sources keeps the
        // thread:post it asks for (the old sources taint is gone).
        g.set_caps(&["thread:post", "sources:read"]);
        let resp = g.post("from sources", Some(g.origin("b1", "board")));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(g.bodies(), vec!["from sources".to_string()], "only the allowed post was stored");
        let audit = k2_core::auth_audit::tail(200).expect("audit tail");
        assert!(
            audit.iter().any(|e| e["event"] == "zen.widget.post_refused"
                && e["outcome"].as_str().is_some_and(|o| o.contains(&g.handle) && o.contains("widget_cap_missing"))),
            "a refused widget post writes an audit line: {audit:?}"
        );
    }

    #[test]
    fn stale_hash_unknown_placement_or_another_widget_is_refused_widget_not_approved() {
        let g = WidgetGarden::new(&["thread:post"]);
        let mut stale = g.origin("b1", "board");
        stale["hash"] = serde_json::json!("0".repeat(64));
        assert_eq!(refused_code(&g.post("stale", Some(stale))), "widget_not_approved");
        // The widget changed after its frame loaded: the old hash is stale.
        let old = g.origin("b1", "board");
        let entry = g.f.widget_dir("board").join("index.html");
        let html = std::fs::read_to_string(&entry).expect("read entry");
        std::fs::write(&entry, format!("{html}\n<!-- changed -->\n")).expect("edit widget");
        assert_ne!(g.hash("board"), old["hash"].as_str().expect("hash"), "editing the widget moves its hash");
        assert_eq!(refused_code(&g.post("old frame", Some(old))), "widget_not_approved");
        let mut unknown = g.origin("b1", "board");
        unknown["placement"] = serde_json::json!("b9");
        assert_eq!(refused_code(&g.post("unknown placement", Some(unknown))), "widget_not_approved");
        let mut elsewhere = g.origin("b1", "board");
        elsewhere["garden"] = serde_json::json!("g-nothere1");
        assert_eq!(refused_code(&g.post("unknown garden", Some(elsewhere))), "widget_not_approved");
        let mut other = g.origin("b1", "board");
        other["widget"] = serde_json::json!("k2:diary@1");
        assert_eq!(refused_code(&g.post("another widget", Some(other))), "widget_not_approved");
        assert!(g.bodies().is_empty(), "nothing refused was stored");
    }

    #[test]
    fn a_label_only_origin_naming_this_computers_garden_is_refused_but_another_computers_keeps_the_label() {
        let g = WidgetGarden::new(&["thread:post"]);
        let local_label = serde_json::json!({"widget": "board", "garden": g.garden});
        assert_eq!(refused_code(&g.post("no claim", Some(local_label))), "widget_not_approved");
        // A widget on another computer, checked by that computer's daemon.
        let remote = serde_json::json!({"widget": "Agent Arcade", "garden": "g-remote01"});
        let post = g.post("from another computer", Some(remote.clone()));
        assert_eq!(post.status, "200 OK", "{}", post.body);
        assert_eq!(json_body(&post)["widget"], remote);
        assert_eq!(g.bodies(), vec!["from another computer".to_string()]);
    }

    #[test]
    fn k2_builtin_custom_widgets_pass_on_their_own_manifest_caps() {
        let g = WidgetGarden::new(&[]);
        let post = g.post("dear diary", Some(g.origin("b2", "k2:diary@1")));
        assert_eq!(post.status, "200 OK", "{}", post.body);
        // The board asked for nothing, so it can't post.
        assert_eq!(refused_code(&g.post("board", Some(g.origin("b1", "board")))), "widget_cap_missing");
    }

    /// 0.46.1 (Rosson 2026-10-18): the Kanban's message flow on its newest
    /// board (`k2:kanban@2` then, `k2:kanban@3` since 0.46.3). A Kanban Garden from New Garden places the new board;
    /// its Thread panel's post (placement `board`, the frame's hash) reaches
    /// the agent as the person's (`via=compose`, the widget's label kept),
    /// and the agent's reply comes back in the same Thread the panel reads,
    /// after it, as the agent's. An own copy still on `k2:kanban@1` posts
    /// too (its frame hash, its own manifest caps).
    #[test]
    fn the_kanban_2_board_posts_to_its_agent_and_reads_the_reply() {
        let g = WidgetGarden::new(&[]);
        let kanban = g.f.new_garden("Kanban", Some("kanban"), None, None).expect("New Garden → Kanban");
        assert_eq!(kanban.template, k2_core::zen::garden_catalog::KANBAN_TEMPLATE_ID);
        let latest = k2_core::zen::builtin_widgets::KANBAN_WIDGET_LATEST;
        let hash = g.f.garden_widget_status(&kanban.id, latest).hash().to_string();
        assert_eq!(hash.len(), 64, "the newest Kanban bundles clean");
        let origin = serde_json::json!({"widget": latest, "garden": kanban.id, "placement": "board", "hash": hash});
        let post = g.post("Can you take the Doing column?", Some(origin));
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["via"], "compose", "the panel posts as the person: {posted}");
        assert_eq!(posted["widget"], serde_json::json!({"widget": latest, "garden": kanban.id}), "{posted}");

        // The agent answers in its Thread; the panel's subscription reads it.
        let reply = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({"addr": g.handle, "text": "On it: **Doing** is mine.", "from": "k2"}).to_string().as_bytes(),
        );
        assert_eq!(reply.status, "200 OK", "{}", reply.body);
        let get = dispatch("/cli/thread", &params_of(&[("addr", g.handle.as_str())])).expect("GET thread");
        let items = json_body(&get)["items"].as_array().expect("items").clone();
        let rows: Vec<(String, String, bool)> = items
            .iter()
            .map(|i| {
                (
                    i["doc"]["body"].as_str().expect("body").to_string(),
                    i["doc"]["from"].as_str().unwrap_or_default().to_string(),
                    i["doc"]["via"] == "compose",
                )
            })
            .collect();
        assert_eq!(rows.len(), 2, "{items:?}");
        assert_eq!((rows[0].0.as_str(), rows[0].2), ("Can you take the Doing column?", true), "the person's post, right side: {items:?}");
        // The daemon stamps the agent's own address as `from` (the panel
        // shows its name, on the left).
        assert_eq!(
            (rows[1].0.as_str(), rows[1].1.as_str(), rows[1].2),
            ("On it: **Doing** is mine.", g.handle.as_str(), false),
            "the agent's reply, left side: {items:?}"
        );
        assert!(items[1]["seq"].as_i64() > items[0]["seq"].as_i64(), "the reply comes after the post");
        assert!(items.iter().all(|i| i["doc"]["created_at"].is_number()), "every message has a time for \"author · time\"");

        // An own copy on @1 still posts on its own frame.
        let old = g.f.new_garden("Old board", Some("k2.kanban@1"), None, None).expect("a @1 Kanban");
        g.f.set_garden_sync(&old.id, k2_core::zen::sync::Parts::Both, false).expect("own copy");
        let one = k2_core::zen::builtin_widgets::KANBAN_WIDGET;
        let old_hash = g.f.garden_widget_status(&old.id, one).hash().to_string();
        let post = g.post("from the old board", Some(serde_json::json!({"widget": one, "garden": old.id, "placement": "board", "hash": old_hash})));
        assert_eq!(post.status, "200 OK", "{}", post.body);
        // The newest frame can't claim the @1 copy's placement.
        let wrong = serde_json::json!({"widget": latest, "garden": old.id, "placement": "board", "hash": hash});
        assert_eq!(refused_code(&g.post("wrong frame", Some(wrong))), "widget_not_approved");
    }

    #[test]
    fn an_unmarked_post_is_unchanged_by_the_widget_check() {
        let g = WidgetGarden::new(&[]);
        let post = g.post("the person typing", None);
        assert_eq!(post.status, "200 OK", "{}", post.body);
        assert_eq!(json_body(&post)["widget"], serde_json::Value::Null);
        let hist = k2_core::workspace_compose_history::list_compose_send_history(&g.project_id).expect("history");
        assert!(hist.iter().any(|e| e.body == "the person typing"), "a person's post still fills history: {hist:?}");
    }

    #[test]
    fn a_widget_card_answer_is_checked_and_never_answers_a_secret() {
        let g = WidgetGarden::new(&["thread:read"]);
        let card = ask_card(&g.handle, "Go,Stop");
        let answer = |origin: serde_json::Value, extra: serde_json::Value| {
            let mut body = serde_json::json!({ "addr": g.handle, "id": card, "origin": origin });
            for (k, v) in extra.as_object().expect("extra").iter() {
                body[k] = v.clone();
            }
            dispatch_post("/cli/thread/answer", &HashMap::new(), body.to_string().as_bytes())
        };
        let refused = answer(g.origin("b1", "board"), serde_json::json!({"answer": "Go"}));
        assert_eq!(refused_code(&refused), "widget_cap_missing");
        assert_eq!(card_status(&g.handle, &card), "pending", "a refused answer leaves the card open");
        let secret = answer(g.origin("b2", "k2:diary@1"), serde_json::json!({"secret": "hunter2"}));
        assert_eq!(secret.status, "400 Bad Request", "{}", secret.body);
        let ok = answer(g.origin("b2", "k2:diary@1"), serde_json::json!({"answer": "Go"}));
        assert_eq!(ok.status, "200 OK", "{}", ok.body);
        assert_eq!(card_status(&g.handle, &card), "answered");
    }

    #[test]
    fn zen_widget_check_route_answers_the_same_rule() {
        let g = WidgetGarden::new(&["thread:read"]);
        let check = |owner: bool, caller: &crate::sidecar_routes::Caller, body: serde_json::Value| {
            let r = crate::zen_routes::handle(
                crate::zen_routes::WIDGET_CHECK_ROUTE,
                owner,
                caller,
                "local",
                &HashMap::new(),
                body.to_string().as_bytes(),
            );
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body));
            (r.status, v)
        };
        let owner = crate::sidecar_routes::Caller::Owner;
        let req = |placement: &str, widget: &str, hash: String| {
            serde_json::json!({"garden": g.garden, "placement": placement, "widget": widget, "hash": hash, "cap": "thread:post"})
        };
        let (status, v) = check(true, &owner, req("b2", "k2:diary@1", g.hash("k2:diary@1")));
        assert_eq!((status, v["ok"].clone()), ("200 OK", serde_json::json!(true)), "{v}");
        assert!(v["caps"].as_array().expect("caps").iter().any(|c| c == "thread:post"), "{v}");
        let (status, v) = check(true, &owner, req("b1", "board", g.hash("board")));
        assert_eq!((status, v["error"].as_str()), ("403 Forbidden", Some("widget_cap_missing")), "{v}");
        let (status, v) = check(true, &owner, req("b1", "board", "f".repeat(64)));
        assert_eq!((status, v["error"].as_str()), ("403 Forbidden", Some("widget_not_approved")), "{v}");
        let (status, v) = check(true, &owner, req("b9", "board", g.hash("board")));
        assert_eq!((status, v["error"].as_str()), ("403 Forbidden", Some("widget_not_approved")), "{v}");
        let mut bad_cap = req("b2", "k2:diary@1", g.hash("k2:diary@1"));
        bad_cap["cap"] = serde_json::json!("files:write");
        let (status, _) = check(true, &owner, bad_cap);
        assert_eq!(status, "400 Bad Request");
        // Zen is this computer's person's: an agent passport gets zen_local_only.
        let agent = crate::sidecar_routes::Caller::Agent(crate::sidecar_routes::AgentCaller {
            project_id: "w-agent".into(),
            map_key: None,
            args: Vec::new(),
            sandboxed: false,
        });
        let (status, v) = check(false, &agent, req("b2", "k2:diary@1", g.hash("k2:diary@1")));
        assert_eq!((status, v["error"].as_str()), ("403 Forbidden", Some("zen_local_only")), "{v}");
    }

    #[test]
    fn compose_via_uses_connect_user_session_not_body_from_or_owner() {
        let handle = format!("ovluser{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let body = format!("alice-says-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let post = dispatch_post_as(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": body,
                "via": "compose",
                "from": "spoofed-owner",
            })
            .to_string()
            .as_bytes(),
            "alice",
        );
        assert_eq!(post.status, "200 OK", "post failed: {}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["from"], "alice", "must ignore body from: {posted}");
        let want = format_thread_compose_pty_line("alice", &handle, &body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &want),
            "connect-user compose must inject [from alice]; want {want:?} got {injects:?}"
        );
        assert!(
            injects.iter().all(|l| !l.contains("spoofed-owner")),
            "must not stamp body from; got {injects:?}"
        );
    }

    /// A Thread is shared: the owner, Connect logins and app guests each
    /// get a distinct, credential-derived `author` on their posts, stored on
    /// the doc and echoed by the post. A body `author` (or `from`) never
    /// changes it, and an agent's post carries none.
    #[test]
    fn thread_posts_carry_a_server_derived_author_per_poster() {
        let _home_lock = crate::test_support::lock_home();
        let handle = format!("ovlauth{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let spoof = serde_json::json!({"kind": "owner", "login": "owner"});
        let body_of = |text: &str, via: Option<&str>| {
            let mut b = serde_json::json!({
                "addr": handle,
                "text": text,
                "from": "owner",
                "author": spoof,
            });
            if let Some(v) = via {
                b["via"] = serde_json::json!(v);
            }
            b.to_string()
        };

        // The owner token (the dispatcher's "owner" session author).
        let owner = dispatch_post_as(
            "/cli/thread/post",
            &HashMap::new(),
            body_of("from-owner", Some("compose")).as_bytes(),
            "owner",
        );
        assert_eq!(owner.status, "200 OK", "{}", owner.body);
        assert_eq!(
            json_body(&owner)["author"],
            serde_json::json!({"kind": "owner", "login": "owner"}),
            "{}",
            owner.body
        );

        // A Connect login: its username, whatever the body claims.
        let user = dispatch_post_as(
            "/cli/thread/post",
            &HashMap::new(),
            body_of("from-alice", Some("compose")).as_bytes(),
            "alice",
        );
        assert_eq!(user.status, "200 OK", "{}", user.body);
        assert_eq!(
            json_body(&user)["author"],
            serde_json::json!({"kind": "user", "login": "alice"}),
            "a body author must not replace the session's: {}",
            user.body
        );

        // An app guest: the pass username.
        let pass = skin_pass(&[project_id.clone()]);
        let guest = with_request_skin(Some(pass), || {
            dispatch_post("/cli/thread/post", &HashMap::new(), body_of("from-guest", None).as_bytes())
        });
        assert_eq!(guest.status, "200 OK", "{}", guest.body);
        assert_eq!(
            json_body(&guest)["author"],
            serde_json::json!({"kind": "guest", "login": "guest"}),
            "{}",
            guest.body
        );

        // An agent (bound passport) asking for compose: no author at all.
        let agent = post_json(
            "/cli/thread/post",
            &passport_params(&project_id, &handle),
            serde_json::json!({"addr": handle, "text": "from-agent", "via": "compose", "author": spoof}),
        );
        assert_eq!(agent.status, "200 OK", "{}", agent.body);
        assert!(json_body(&agent)["author"].is_null(), "agent posts carry no author: {}", agent.body);

        // The stored docs (what every viewer and the Companion read).
        let get = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread");
        assert_eq!(get.status, "200 OK", "{}", get.body);
        let snap = json_body(&get);
        let items = snap["items"].as_array().expect("items");
        let author_of = |text: &str| -> serde_json::Value {
            let it = items
                .iter()
                .find(|it| it["doc"]["body"] == text)
                .unwrap_or_else(|| panic!("no stored doc {text}: {snap}"));
            it["doc"]["author"].clone()
        };
        assert_eq!(author_of("from-owner"), serde_json::json!({"kind": "owner", "login": "owner"}));
        assert_eq!(author_of("from-alice"), serde_json::json!({"kind": "user", "login": "alice"}));
        assert_eq!(author_of("from-guest"), serde_json::json!({"kind": "guest", "login": "guest"}));
        assert!(author_of("from-agent").is_null(), "{snap}");
        let agent_doc = items.iter().find(|it| it["doc"]["body"] == "from-agent").expect("agent doc");
        assert!(
            agent_doc["doc"].as_object().expect("doc object").get("author").is_none(),
            "an absent author is not written: {snap}"
        );
        // `from` keeps today's values: the owner's display name, the username.
        let from_of = |text: &str| items.iter().find(|it| it["doc"]["body"] == text).expect("doc")["doc"]["from"].clone();
        assert_eq!(from_of("from-owner"), serde_json::json!(crate::workspace_msg::resolve_owner_from()));
        assert_eq!(from_of("from-alice"), serde_json::json!("alice"));
        assert_eq!(from_of("from-guest"), serde_json::json!("guest"));

        // `/cli/thread/latest` (Zen row previews) names the newest post's author.
        let latest_of = || {
            let r = dispatch("/cli/thread/latest", &params_of(&[("addrs", handle.as_str())])).expect("latest");
            assert_eq!(r.status, "200 OK", "{}", r.body);
            json_body(&r)
        };
        let lv = latest_of();
        assert_eq!(lv["items"][0]["preview"], "from-agent", "{lv}");
        assert!(lv["items"][0]["author"].is_null(), "{lv}");
        let bob = dispatch_post_as(
            "/cli/thread/post",
            &HashMap::new(),
            body_of("from-bob", Some("compose")).as_bytes(),
            "bob",
        );
        assert_eq!(bob.status, "200 OK", "{}", bob.body);
        let lv = latest_of();
        assert_eq!(lv["items"][0]["preview"], "from-bob", "{lv}");
        assert_eq!(lv["items"][0]["author"], serde_json::json!({"kind": "user", "login": "bob"}), "{lv}");
    }

    #[test]
    fn compose_slash_command_prepends_on_thread_inject() {
        let handle = format!("ovlslash{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let post = dispatch_post_as(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": "wrap it up",
                "via": "compose",
                "command": "/compact",
            })
            .to_string()
            .as_bytes(),
            "alice",
        );
        assert_eq!(post.status, "200 OK", "post failed: {}", post.body);
        let want =
            format_thread_compose_pty_line_with_command("alice", &handle, "wrap it up", "/compact")
                .expect("compact");
        assert_eq!(
            want,
            format!("/compact [from alice] [thread:{handle}] wrap it up")
        );
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &want),
            "slash command must lead the PTY line; want {want:?} got {injects:?}"
        );
    }

    #[test]
    fn compose_unknown_slash_command_is_400() {
        let handle = format!("ovlbad{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": "nope",
                "via": "compose",
                "command": "/exit",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(post.status, "400 Bad Request", "{}", post.body);
    }

    #[test]
    fn sidecar_compose_inject_line_uses_ws_slash_sidecar_addr() {
        let line = format_thread_compose_pty_line("Rosson", "sales/reviewer", "ship it");
        assert_eq!(line, "[from Rosson] [thread:sales/reviewer] ship it");
    }

    #[test]
    fn msg_chatter_not_on_thread() {
        let handle = format!("ovlmsg{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let pinned = uuid::Uuid::new_v4().to_string();
        let reviewer = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &pinned);
        sidecar(&project_id, &reviewer, "reviewer");
        let addr = format!("{handle}/reviewer");

        record_inject_chatter(&addr, &handle, "ping", "msg", None);

        let thread =
            dispatch("/cli/thread", &params_of(&[("addr", addr.as_str())])).expect("GET thread");
        let t = json_body(&thread);
        let t_items = t["items"].as_array().expect("thread items");
        assert!(
            t_items.is_empty(),
            "GET thread must not contain the ping: {t}"
        );

        let chatter =
            dispatch("/cli/chatter", &params_of(&[("addr", addr.as_str())])).expect("GET chatter");
        let c = json_body(&chatter);
        let c_items = c["items"].as_array().expect("chatter items");
        assert_eq!(c_items.len(), 1, "reviewer chatter: {c}");
        assert_eq!(c_items[0]["doc"]["body"], "ping");
        assert_eq!(c_items[0]["doc"]["via"], "msg");
        assert_eq!(c_items[0]["doc"]["kind"], "chatter");
        let id = c_items[0]["id"].as_str().expect("id");

        let sender_chatter = dispatch("/cli/chatter", &params_of(&[("addr", handle.as_str())]))
            .expect("sender chatter");
        let s = json_body(&sender_chatter);
        let s_items = s["items"].as_array().expect("sender items");
        assert_eq!(s_items.len(), 1, "sender chatter: {s}");
        assert_eq!(s_items[0]["id"], id, "one docs/{{id}} shared");

        let log = dispatch("/cli/chatterlog", &HashMap::new()).expect("chatterlog");
        let l = json_body(&log);
        let l_items = l["items"].as_array().expect("log items");
        assert!(
            l_items.iter().any(|i| i["id"] == id),
            "chatterlog missing {id}: {l}"
        );

        let (thread_n, chatter_n, log_n) =
            k2_core::overlay::store::debug_pointer_count(id).expect("pointers");
        assert_eq!(thread_n, 0, "no Thread link");
        assert_eq!(chatter_n, 2, "reviewer + sender");
        assert_eq!(log_n, 1);
    }

    #[test]
    fn talk_via_talk() {
        let handle = format!("ovltalk{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let reviewer = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        sidecar(&project_id, &reviewer, "reviewer");
        let addr = format!("{handle}/reviewer");
        record_inject_chatter(&addr, &handle, "ping-talk", "talk", None);
        let chatter =
            dispatch("/cli/chatter", &params_of(&[("addr", addr.as_str())])).expect("chatter");
        let c = json_body(&chatter);
        let items = c["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "{c}");
        assert_eq!(
            items[0]["doc"]["via"], "talk",
            "via: talk must be stamped; got {c}"
        );
    }

    /// A live `k2 msg` (the recorded path behind `/cli/workspace/msg`)
    /// writes one `msg_deliveries` row and one Chatter doc carrying the
    /// same `deliveryId`, and only the id.
    #[cfg(unix)]
    #[test]
    fn live_msg_chatter_carries_the_delivery_id() {
        use k2_core::session::SessionId;
        use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession, LabelSource};

        let handle = format!("ovldlv{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let reviewer = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        sidecar(&project_id, &reviewer, "reviewer");
        let addr = format!("{handle}/reviewer");
        let sender_handle = format!("ovldls{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (sender_project, _) = seed(&sender_handle);
        pin(&sender_project, &uuid::Uuid::new_v4().to_string());

        // A live `cat` stands in for the reviewer's agent PTY (registering
        // it starts the activity observer, which needs a tokio reactor).
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let _enter = rt.enter();
        let agent_name = format!("tab-pane-{reviewer}");
        let session = DaemonPtySession::spawn(DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(std::path::PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            durable_args: None,
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
            sandbox: k2_core::terminal::SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
        })
        .expect("spawn cat");
        crate::v2_session_map::register(agent_name.clone(), session.clone());

        let sender = crate::hire_routes::Sender {
            workspace_id: Some(sender_project.clone()),
            via: "passport",
            actor: format!("agent:{sender_handle}"),
            on_behalf_of: None,
        };
        let (resp, id) = crate::hire_routes::deliver_recorded(
            &addr,
            "ping with receipt",
            &sender_handle,
            "",
            false,
            std::time::Duration::from_secs(5),
            "msg",
            &sender,
        );
        crate::v2_session_map::unregister(&agent_name);
        session.kill();
        assert!(resp.success, "live delivery: {resp:?}");
        let id = id.expect("a msg_deliveries row was written");

        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::hire::deliveries::get(&conn, &id).expect("msg_deliveries row")
        };
        assert!(row.delivered, "{row:?}");
        assert_eq!(row.sender_workspace_id.as_deref(), Some(sender_project.as_str()));

        let chatter =
            dispatch("/cli/chatter", &params_of(&[("addr", addr.as_str())])).expect("chatter");
        let c = json_body(&chatter);
        let items = c["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "{c}");
        let doc = &items[0]["doc"];
        assert_eq!(doc["deliveryId"], id.as_str(), "{c}");
        assert_eq!(doc["body"], "ping with receipt", "{c}");
        assert_eq!(doc["from"], sender_handle.as_str(), "the id rides in deliveryId, not in from: {c}");
        for k in ["actor", "sender", "onBehalfOf", "senderWorkspaceId"] {
            assert!(doc.get(k).is_none(), "no receipt field {k} on the doc: {c}");
        }

        // The sender's own mailbox has the same doc (the bare from resolves).
        let mine = dispatch("/cli/chatter", &params_of(&[("addr", sender_handle.as_str())]))
            .expect("sender chatter");
        let m = json_body(&mine);
        let m_items = m["items"].as_array().expect("sender items");
        assert_eq!(m_items.len(), 1, "{m}");
        assert_eq!(m_items[0]["id"], items[0]["id"], "{m}");
        assert_eq!(m_items[0]["doc"]["deliveryId"], id.as_str(), "{m}");

        let log = dispatch("/cli/chatterlog", &HashMap::new()).expect("chatterlog");
        let l = json_body(&log);
        let entry = l["items"]
            .as_array()
            .expect("log items")
            .iter()
            .find(|i| i["id"] == items[0]["id"])
            .cloned()
            .expect("chatterlog has the line");
        assert_eq!(entry["doc"]["deliveryId"], id.as_str(), "{entry}");
    }

    #[test]
    fn pin_swap_sales_follows_new_chat_old_docs_stay() {
        let handle = format!("ovlpin{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        sidecar(&project_id, &a, "alice");
        pin(&project_id, &a);

        let post_a = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({"addr": handle, "text": "from-A", "from": "k2"})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(post_a.status, "200 OK", "{}", post_a.body);
        let posted_a = json_body(&post_a);
        assert_eq!(posted_a["conversation_id"], a);

        pin(&project_id, &b);
        let post_b = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({"addr": handle, "text": "from-B", "from": "k2"})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(post_b.status, "200 OK", "{}", post_b.body);
        let posted_b = json_body(&post_b);
        assert_eq!(
            posted_b["conversation_id"], b,
            "k2 thread {handle} must follow pin-swap to B"
        );

        let sales =
            dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("sales read");
        let s = json_body(&sales);
        let s_items = s["items"].as_array().expect("items");
        assert_eq!(s_items.len(), 1, "sales is B's overlay: {s}");
        assert_eq!(s_items[0]["doc"]["body"], "from-B");

        let durable = format!("{handle}/alice");
        let a_read =
            dispatch("/cli/thread", &params_of(&[("addr", durable.as_str())])).expect("A durable");
        let ar = json_body(&a_read);
        let a_items = ar["items"].as_array().expect("A items");
        assert_eq!(a_items.len(), 1, "A's docs stay on A: {ar}");
        assert_eq!(a_items[0]["doc"]["body"], "from-A");
        assert_eq!(ar["conversation_id"], a);
    }

    #[test]
    fn sidecar_addr_while_pinned_as_chat_lands_on_that_conversation() {
        let handle = format!("ovlchat{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let reviewer = uuid::Uuid::new_v4().to_string();
        sidecar(&project_id, &reviewer, "reviewer");
        pin(&project_id, &reviewer);
        let addr = format!("{handle}/reviewer");
        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({"addr": addr, "text": "x", "from": "k2"})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(
            posted["conversation_id"], reviewer,
            "must land on reviewer conversation, not 404/rewrite: {posted}"
        );
        assert_ne!(post.status, "404 Not Found");
    }

    #[test]
    fn catalog_uses_handle_key_not_v2_map() {
        let handle = format!("ovlcat{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        sidecar(&project_id, &conv, "reviewer");
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let addr = format!("{handle}/reviewer");
        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({"addr": addr, "text": "cat", "from": "k2"})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["conversation_id"], conv);
        let db = k2_core::db::shared();
        let c = db.lock();
        let row = k2_core::overlay::catalog::get(&c, &conv)
            .expect("catalog")
            .expect("row");
        assert_eq!(row.0, project_id);
        assert!(k2_core::overlay::catalog::get(&c, "tab-pane-not-overlay")
            .expect("missing")
            .is_none());
    }

    #[test]
    fn sidecar_cannot_write_canonical_overlay() {
        let handle = format!("ovlt22{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let mut params = HashMap::new();
        params.insert("principal_bound".to_string(), "1".to_string());
        params.insert("project_id".to_string(), project_id.clone());
        params.insert("from".to_string(), format!("{handle}/reviewer"));
        params.insert("addr".to_string(), handle.clone());
        params.insert("text".to_string(), "nope".to_string());
        let post = dispatch_post("/cli/thread/post", &params, b"");
        assert_eq!(
            post.status, "403 Forbidden",
            "sidecar write to Chat overlay must fail loud: {}",
            post.body
        );

        let mut read_params = HashMap::new();
        read_params.insert("principal_bound".to_string(), "1".to_string());
        read_params.insert("project_id".to_string(), project_id);
        read_params.insert("addr".to_string(), handle);
        let get = dispatch("/cli/thread", &read_params).expect("read");
        assert_eq!(
            get.status, "200 OK",
            "same-workspace sidecar can read Chat overlay: {}",
            get.body
        );
    }

    #[test]
    fn get_ask_secret_answer_void_are_405() {
        for path in [
            "/cli/thread/ask",
            "/cli/thread/secret",
            "/cli/thread/answer",
            "/cli/thread/void",
        ] {
            let resp = dispatch(path, &HashMap::new()).expect("GET mutation handled");
            assert_eq!(
                resp.status, "405 Method Not Allowed",
                "GET {path} must 405, got {} body={}",
                resp.status, resp.body
            );
        }
    }

    #[test]
    fn ask_returns_immediately_pending_then_tap_injects() {
        // Reads $HOME (the owner display name) at tap time and again at assert time: hold the env lock so a concurrent temp-HOME test cannot swap it in between.
        let _home_lock = crate::test_support::lock_home();
        let handle = format!("ovlask{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);

        let ask = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "Ship it?",
                "options": "Go,Stop",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask.status, "200 OK", "ask failed: {}", ask.body);
        let posted = json_body(&ask);
        assert_eq!(posted["ok"], true, "{posted}");
        let id = posted["id"].as_str().expect("id").to_string();
        assert!(!id.is_empty(), "ask must return an id: {posted}");
        assert_eq!(posted["prompt"], "Ship it?", "{posted}");
        let opts = posted["options"].as_array().expect("options array");
        assert_eq!(
            opts,
            &vec![serde_json::json!("Go"), serde_json::json!("Stop")]
        );
        assert_eq!(posted["status"], "pending", "{posted}");

        let get =
            dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread");
        let snap = json_body(&get);
        let items = snap["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "{snap}");
        assert_eq!(items[0]["doc"]["kind"], "choice");
        assert_eq!(items[0]["doc"]["choice"]["status"], "pending");

        let answer = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "id": id,
                "answer": "Go",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(answer.status, "200 OK", "answer failed: {}", answer.body);
        let answered = json_body(&answer);
        assert_eq!(answered["status"], "answered", "{answered}");
        assert_eq!(answered["answer"], "Go", "{answered}");
        let injects = recorded_injects();
        let owner = workspace_msg::resolve_owner_from();
        assert!(
            injects
                .iter()
                .any(|l| l == &format!("[from {owner}] [thread:{handle}] chose Go")),
            "async inject must fire after tap, from the owner who tapped; got {injects:?}"
        );
        assert_eq!(answered_doc_by(&handle, &id), owner_answered_by_json());

        let get2 = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())]))
            .expect("GET after tap");
        let snap2 = json_body(&get2);
        assert_eq!(
            snap2["items"][0]["doc"]["choice"]["status"], "answered",
            "{snap2}"
        );
        assert_eq!(snap2["items"][0]["doc"]["choice"]["answer"], "Go");
    }

    #[test]
    fn mix_options_and_option_is_400() {
        let handle = format!("ovlmix{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let resp = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "?",
                "options": "Go,Stop",
                "option": ["Hold"],
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(
            resp.status, "400 Bad Request",
            "mix of --options and --option must 400: {}",
            resp.body
        );
    }

    /// The stored card's `answeredBy` (or Null).
    fn answered_doc_by(addr: &str, card_id: &str) -> serde_json::Value {
        let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", addr)])).expect("read"));
        let item = snap["items"].as_array().expect("items").iter().find(|i| i["id"] == card_id).cloned().expect("card");
        let body = if item["doc"]["kind"] == "choice" { &item["doc"]["choice"] } else { &item["doc"]["secret"] };
        assert!(body["answeredBy"].is_null() || body["answeredAt"].as_i64().is_some_and(|t| t > 0), "answeredAt rides answeredBy: {item}");
        body["answeredBy"].clone()
    }

    fn ask_card(addr: &str, options: &str) -> String {
        let ask = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({ "addr": addr, "prompt": "Ship?", "options": options, "from": "k2" }).to_string().as_bytes(),
        );
        assert_eq!(ask.status, "200 OK", "{}", ask.body);
        json_body(&ask)["id"].as_str().expect("id").to_string()
    }

    fn card_status(addr: &str, card_id: &str) -> String {
        let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", addr)])).expect("read"));
        let item = snap["items"].as_array().expect("items").iter().find(|i| i["id"] == card_id).cloned().expect("card");
        let body = if item["doc"]["kind"] == "choice" { &item["doc"]["choice"] } else { &item["doc"]["secret"] };
        body["status"].as_str().expect("status").to_string()
    }

    /// Rosson 2026-10-13: a person may talk about a card before answering
    /// it. Their prose (compose bar, a Garden widget, the exact option
    /// label, an app guest) never closes a pending choice or secret card,
    /// and the card can still be tapped afterwards.
    #[test]
    fn prose_leaves_pending_cards_open_and_still_answerable() {
        let handle = format!("ovlprose{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let choice = ask_card(&handle, "Go,Stop");
        let secret = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "name": "PROSE_TOKEN", "from": "k2" }).to_string().as_bytes(),
        );
        assert_eq!(secret.status, "200 OK", "{}", secret.body);
        let secret = json_body(&secret)["id"].as_str().expect("id").to_string();

        for (text, origin) in [
            ("hmm, what does Stop mean exactly?", None),
            ("Go", None),
            ("I'll paste it later", None),
            ("from my widget", Some(serde_json::json!({ "widget": "Agent Arcade", "garden": "g-test0001" }))),
        ] {
            let mut body = serde_json::json!({ "addr": handle, "text": text, "via": "compose" });
            if let Some(o) = origin {
                body["origin"] = o;
            }
            let posted = post_as_owner(&HashMap::new(), body);
            assert_eq!(posted.status, "200 OK", "{}", posted.body);
        }
        let guest = with_request_skin(Some(skin_pass(&[project_id.clone()])), || {
            dispatch_post(
                "/cli/thread/post",
                &HashMap::new(),
                serde_json::json!({ "addr": handle, "text": "a guest chimes in" }).to_string().as_bytes(),
            )
        });
        assert_eq!(guest.status, "200 OK", "{}", guest.body);

        assert_eq!(card_status(&handle, &choice), "pending", "prose must not close a choice card");
        assert_eq!(card_status(&handle, &secret), "pending", "prose must not close a secret card");
        let vault = crate::vault::get().expect("vault");
        assert_eq!(vault.request_get(&secret).expect("req").status, "pending");
        let injects = recorded_injects();
        assert!(
            !injects.iter().any(|l| l.contains(&format!("[thread:{handle}] card voided")) || l.contains(&format!("[thread:{handle}] chose"))),
            "prose fires no card callback: {injects:?}"
        );

        // Still answerable by tapping after all that talk.
        let tap = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": choice, "answer": "Stop" }).to_string().as_bytes(),
        );
        assert_eq!(tap.status, "200 OK", "{}", tap.body);
        assert_eq!(card_status(&handle, &choice), "answered");
    }

    fn ask_with(addr: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut body = serde_json::json!({ "addr": addr, "prompt": "Ship?", "options": "Go,Stop", "from": "k2" });
        for (k, v) in extra.as_object().expect("object") {
            body[k] = v.clone();
        }
        let ask = dispatch_post("/cli/thread/ask", &HashMap::new(), body.to_string().as_bytes());
        assert_eq!(ask.status, "200 OK", "{}", ask.body);
        json_body(&ask)
    }

    fn card_expires_at(addr: &str, card_id: &str) -> serde_json::Value {
        let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", addr)])).expect("read"));
        let item = snap["items"].as_array().expect("items").iter().find(|i| i["id"] == card_id).cloned().expect("card");
        let body = if item["doc"]["kind"] == "choice" { &item["doc"]["choice"] } else { &item["doc"]["secret"] };
        body["expiresAt"].clone()
    }

    /// The frames on `rx` so far for `id`.
    fn frames_for(rx: &mut tokio::sync::broadcast::Receiver<OverlayFrame>, id: &str) -> Vec<OverlayFrame> {
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(f) if f.id == id => out.push(f),
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => return out,
            }
        }
    }

    /// Rosson 2026-10-13: a card has a time frame so it is not an open gate.
    /// Default 7 days; `--expires` sets it; `--no-expiry` keeps it open; a
    /// bad value is refused; prose never shortens or removes it.
    #[test]
    fn ask_card_time_frames_default_custom_and_none() {
        let handle = format!("ovlexp{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let now = overlay::now_secs();
        let near = |got: &serde_json::Value, want: i64| {
            let t = got.as_i64().unwrap_or_else(|| panic!("expiresAt is a number: {got}"));
            assert!((want - 5..=want + 5).contains(&t), "expiresAt {t} not near {want}");
        };

        let default = ask_with(&handle, serde_json::json!({}));
        near(&default["expiresAt"], now + 7 * 24 * 3600);
        let id = default["id"].as_str().expect("id").to_string();
        near(&card_expires_at(&handle, &id), now + 7 * 24 * 3600);

        let custom = ask_with(&handle, serde_json::json!({ "expires": "2h" }));
        near(&custom["expiresAt"], now + 7_200);
        let days = ask_with(&handle, serde_json::json!({ "expires": "3d" }));
        near(&days["expiresAt"], now + 3 * 86_400);

        let open = ask_with(&handle, serde_json::json!({ "no_expiry": true }));
        assert!(open["expiresAt"].is_null(), "{open}");
        let open_id = open["id"].as_str().expect("id").to_string();
        assert!(card_expires_at(&handle, &open_id).is_null(), "no time frame stored");
        let never = ask_with(&handle, serde_json::json!({ "expires": "never" }));
        assert!(never["expiresAt"].is_null(), "{never}");

        let bad = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "prompt": "?", "options": "Go", "expires": "soon" }).to_string().as_bytes(),
        );
        assert_eq!(bad.status, "400 Bad Request", "{}", bad.body);
        assert!(bad.body.contains("--expires"), "{}", bad.body);

        // Talking never changes a card's time frame.
        let before = card_expires_at(&handle, &id);
        let talk = post_as_owner(&HashMap::new(), serde_json::json!({ "addr": handle, "text": "let me think", "via": "compose" }));
        assert_eq!(talk.status, "200 OK", "{}", talk.body);
        assert_eq!(card_expires_at(&handle, &id), before);
        assert_eq!(card_status(&handle, &id), "pending");
    }

    /// The daemon sweep turns a due card `expired` with a frame; an answer
    /// or a void after the time is refused 409 (check-on-touch closes it
    /// first, with its own frame, when the sweep hasn't run yet).
    #[test]
    fn expired_cards_close_on_the_sweep_and_refuse_late_answers() {
        let handle = format!("ovlexpd{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let mut rx = crate::overlay_ws::subscribe();

        let swept = ask_with(&handle, serde_json::json!({ "expires": "1" }));
        let swept_id = swept["id"].as_str().expect("id").to_string();
        let touched = ask_with(&handle, serde_json::json!({ "expires": "1" }));
        let touched_id = touched["id"].as_str().expect("id").to_string();
        let lasting = ask_with(&handle, serde_json::json!({ "expires": "1h" }));
        let lasting_id = lasting["id"].as_str().expect("id").to_string();
        let _ = frames_for(&mut rx, &swept_id);
        std::thread::sleep(std::time::Duration::from_millis(2_100));

        // Check-on-touch: the answer is refused and the card closes now.
        let late = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": touched_id, "answer": "Go" }).to_string().as_bytes(),
        );
        assert_eq!(late.status, "409 Conflict", "{}", late.body);
        assert!(late.body.contains("card_not_pending") && late.body.contains("expired"), "{}", late.body);
        assert_eq!(card_status(&handle, &touched_id), "expired");
        let f = frames_for(&mut rx, &touched_id);
        assert_eq!(f.len(), 1, "one frame for the touched card: {f:?}");
        assert_eq!(f[0].doc.as_ref().expect("doc").choice.as_ref().expect("c").status, "expired");

        // The sweep (as the daemon loop runs it) closes the other one.
        assert!(sweep_expired_cards(overlay::now_secs()) >= 1);
        assert_eq!(card_status(&handle, &swept_id), "expired");
        assert_eq!(card_status(&handle, &lasting_id), "pending", "time left");
        let f = frames_for(&mut rx, &swept_id);
        assert_eq!(f.len(), 1, "the sweep's frame: {f:?}");
        assert_eq!(f[0].conversation_id.as_deref(), Some(json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("read"))["conversation_id"].as_str().expect("conv")));

        for (route, body) in [
            ("/cli/thread/answer", serde_json::json!({ "addr": handle, "id": swept_id, "answer": "Go" })),
            ("/cli/thread/void", serde_json::json!({ "addr": handle, "id": swept_id })),
        ] {
            let r = dispatch_post(route, &HashMap::new(), body.to_string().as_bytes());
            assert_eq!(r.status, "409 Conflict", "{route}: {}", r.body);
        }
        let injects = recorded_injects();
        assert!(
            !injects.iter().any(|l| l.contains(&format!("[thread:{handle}] card expired"))),
            "expiry wakes no one: {injects:?}"
        );
    }

    /// A vault secret request past its 7 days: the vault's tick expires
    /// it and its Thread card turns `expired` with a frame; an answer to a
    /// card the vault already expired closes the card too.
    #[test]
    fn expired_vault_requests_mark_their_secret_cards_expired() {
        let vault = crate::vault::ensure_test_vault();
        let handle = format!("ovlvexp{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let post = |name: &str| {
            let r = dispatch_post(
                "/cli/thread/secret",
                &HashMap::new(),
                serde_json::json!({ "addr": handle, "name": name, "from": "k2" }).to_string().as_bytes(),
            );
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let b = json_body(&r);
            let at = b["expiresAt"].as_i64().expect("secret cards show the request's 7 days");
            assert!((at - overlay::now_secs() - 7 * 86_400).abs() <= 5, "{b}");
            b["id"].as_str().expect("id").to_string()
        };
        let ticked = post("TICK_TOKEN");
        let answered_late = post("LATE_TOKEN");
        let mut rx = crate::overlay_ws::subscribe();
        vault.backdate_request_for_tests(&ticked, 8).expect("backdate");
        vault.backdate_request_for_tests(&answered_late, 8).expect("backdate");

        // An answer after the vault's 7 days: refused, and the card closes.
        let late = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": answered_late, "secret": "late-value-xyz" }).to_string().as_bytes(),
        );
        assert_eq!(late.status, "409 Conflict", "{}", late.body);
        assert_eq!(vault.request_get(&answered_late).expect("req").status, "expired");
        assert_eq!(card_status(&handle, &answered_late), "expired");
        assert_eq!(frames_for(&mut rx, &answered_late).len(), 1);

        // The vault tick (as its minute loop runs it) closes the other.
        let expired = vault.tick().expect("tick");
        let mine: Vec<_> = expired.iter().filter(|r| r.card_id == ticked).collect();
        assert_eq!(mine.len(), 1, "the tick returns what it expired");
        close_expired_card(&mine[0].conversation, &mine[0].card_id);
        assert_eq!(card_status(&handle, &ticked), "expired");
        let f = frames_for(&mut rx, &ticked);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].doc.as_ref().expect("doc").secret.as_ref().expect("s").status, "expired");
        close_expired_card(&mine[0].conversation, &mine[0].card_id);
        assert!(frames_for(&mut rx, &ticked).is_empty(), "closing twice is a no-op");
    }

    /// The agent hears who really answered: an app guest's pass username,
    /// a Connect user's login, the owner's name. `answeredBy` on the card
    /// is credential-derived too, never the body.
    #[test]
    fn card_answers_inject_and_record_the_real_answerer() {
        // Reads $HOME (the owner display name) at tap time and again at assert time: hold the env lock so a concurrent temp-HOME test cannot swap it in between.
        let _home_lock = crate::test_support::lock_home();
        let handle = format!("ovlwho{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let owner = workspace_msg::resolve_owner_from();

        // App guest through the gateway's pass.
        let guest_name = format!("guest{}", &uuid::Uuid::new_v4().to_string()[..6]);
        let mut pass = skin_pass(&[project_id.clone()]);
        pass.username = guest_name.clone();
        let g_card = ask_card(&handle, "GuestGo,Stop");
        let g = with_request_skin(Some(pass.clone()), || {
            dispatch_post(
                "/cli/thread/answer",
                &HashMap::new(),
                serde_json::json!({ "addr": handle, "id": g_card, "answer": "GuestGo", "from": "spoofed-owner" })
                    .to_string()
                    .as_bytes(),
            )
        });
        assert_eq!(g.status, "200 OK", "{}", g.body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &format!("[from {guest_name}] [thread:{handle}] chose GuestGo")),
            "guest answer must say [from <guest>]: {injects:?}"
        );
        assert!(!injects.iter().any(|l| l.contains("spoofed-owner")), "body from is never used: {injects:?}");
        assert_eq!(answered_doc_by(&handle, &g_card), serde_json::json!({"kind": "guest", "login": guest_name}));

        // Connect user.
        let user = format!("cu{}", &uuid::Uuid::new_v4().to_string()[..6]);
        let u_card = ask_card(&handle, "UserGo,Stop");
        let u = dispatch_post_as(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": u_card, "answer": "UserGo", "from": owner }).to_string().as_bytes(),
            &user,
        );
        assert_eq!(u.status, "200 OK", "{}", u.body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &format!("[from {user}] [thread:{handle}] chose UserGo")),
            "Connect user's answer must say [from <login>]: {injects:?}"
        );
        assert_eq!(answered_doc_by(&handle, &u_card), serde_json::json!({"kind": "user", "login": user}));

        // Owner token (desktop, CLI outside K2, Zen widgets).
        let o_card = ask_card(&handle, "OwnerGo,Stop");
        let o = dispatch_post_as(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": o_card, "answer": "OwnerGo" }).to_string().as_bytes(),
            "owner",
        );
        assert_eq!(o.status, "200 OK", "{}", o.body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &format!("[from {owner}] [thread:{handle}] chose OwnerGo")),
            "owner answer keeps the owner's name: {injects:?}"
        );
        assert_eq!(answered_doc_by(&handle, &o_card), owner_answered_by_json());
        if owner != "owner" {
            assert_eq!(answered_doc_by(&handle, &o_card)["name"], owner.as_str(), "the owner's display name rides answeredBy");
        }

        // A guest dismissing a card: the agent hears the guest too.
        let d_card = ask_card(&handle, "DismissMe,Stop");
        let d = with_request_skin(Some(pass), || {
            dispatch_post(
                "/cli/thread/void",
                &HashMap::new(),
                serde_json::json!({ "addr": handle, "id": d_card }).to_string().as_bytes(),
            )
        });
        assert_eq!(d.status, "200 OK", "{}", d.body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &format!("[from {guest_name}] [thread:{handle}] card voided")),
            "guest dismiss must say [from <guest>]: {injects:?}"
        );
        assert_eq!(answered_doc_by(&handle, &d_card), serde_json::json!({"kind": "guest", "login": guest_name}));
    }

    /// A guest's secret card answer is stamped with the guest, not the owner.
    #[test]
    fn guest_secret_answer_injects_from_the_guest() {
        crate::vault::ensure_test_vault();
        let handle = format!("ovlgsec{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let posted = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "name": "GUEST_TOKEN", "for": "api.example.com", "from": "k2" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let id = json_body(&posted)["id"].as_str().expect("id").to_string();
        let guest_name = format!("gsec{}", &uuid::Uuid::new_v4().to_string()[..6]);
        let mut pass = skin_pass(&[project_id]);
        pass.username = guest_name.clone();
        let set = with_request_skin(Some(pass), || {
            dispatch_post(
                "/cli/thread/answer",
                &HashMap::new(),
                serde_json::json!({ "addr": handle, "id": id, "secret": "guest-value-xyz" }).to_string().as_bytes(),
            )
        });
        assert_eq!(set.status, "200 OK", "{}", set.body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l.starts_with(&format!("[from {guest_name}] [thread:{handle}] secret GUEST_TOKEN set"))),
            "guest secret answer must say [from <guest>]: {injects:?}"
        );
        assert!(injects.iter().all(|l| !l.contains("guest-value-xyz")), "never the value: {injects:?}");
        assert_eq!(answered_doc_by(&handle, &id), serde_json::json!({"kind": "guest", "login": guest_name}));
    }

    #[test]
    fn secret_card_answer_goes_to_the_vault_and_never_in_json() {
        use k2_vault::SecretStore as _;
        let vault = crate::vault::ensure_test_vault();
        let handle = format!("ovlsec{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let secret_val = "s3cr3t-NEVER-IN-GET-xyz";

        let posted = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "name": "API_TOKEN",
                "prompt": "Paste the Grok token",
                "for": "api.x.ai",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let body = json_body(&posted);
        assert_eq!(body["name"], "API_TOKEN", "{body}");
        assert_eq!(body["status"], "pending", "{body}");
        let id = body["id"].as_str().expect("id").to_string();
        let req = vault.request_get(&id).expect("vault request");
        assert_eq!(req.proposed_hosts, vec!["https://api.x.ai:443".to_string()]);

        // An agent or K2 terminal can't answer (KV8).
        let mut passport = HashMap::new();
        passport.insert("cell_session_id".to_string(), "sess-x".to_string());
        let refused = dispatch_post(
            "/cli/thread/answer",
            &passport,
            serde_json::json!({ "addr": handle, "id": id, "secret": secret_val }).to_string().as_bytes(),
        );
        assert_eq!(refused.status, "403 Forbidden", "{}", refused.body);
        assert!(refused.body.contains("secret_card_human_only"), "{}", refused.body);

        let set = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": id, "secret": secret_val }).to_string().as_bytes(),
        );
        assert_eq!(set.status, "200 OK", "{}", set.body);
        let set_body = json_body(&set);
        assert_eq!(set_body["status"], "set", "{set_body}");
        assert_eq!(set_body["ref"], format!("k2v://ws/{handle}/API_TOKEN"), "{set_body}");
        assert!(!set.body.contains(secret_val), "answer JSON must not contain secret: {}", set.body);
        let got = vault
            .resolve(
                &k2_vault::Selector::user(k2_vault::Scope::workspace(project_id.clone()), "API_TOKEN"),
                k2_vault::Use::System,
                &k2_vault::Requester::System("test".into()),
            )
            .expect("vault holds it");
        assert_eq!(got.value.expose(), secret_val.as_bytes());
        assert!(got.meta.human_given);
        // A second answer is refused (no overwrite).
        let again = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "id": id, "secret": "other" }).to_string().as_bytes(),
        );
        assert_eq!(again.status, "409 Conflict", "{}", again.body);
        let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET"));
        let snap_s = snap.to_string();
        assert!(!snap_s.contains(secret_val), "GET snapshot must not contain secret: {snap_s}");
        assert!(
            !k2_core::overlay::store::debug_docs_contain(secret_val).expect("scan"),
            "redb docs must not contain secret bytes"
        );
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l.contains(&format!("[thread:{handle}] secret API_TOKEN set → k2v://ws/{handle}/API_TOKEN (fetch; api.x.ai)"))),
            "set inject: {injects:?}"
        );
        assert!(injects.iter().all(|l| !l.contains(secret_val)), "inject must never carry secret bytes: {injects:?}");
        assert!(!k2_core::overlay::vault::exists(&project_id, "API_TOKEN"), "nothing in the old folder");

        let handle2 = format!("ovlvoid{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id2, _) = seed(&handle2);
        pin(&project_id2, &uuid::Uuid::new_v4().to_string());
        let pending = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": handle2, "name": "OTHER_TOKEN", "from": "k2" }).to_string().as_bytes(),
        );
        assert_eq!(pending.status, "200 OK", "{}", pending.body);
        let pid = json_body(&pending)["id"].as_str().expect("id").to_string();
        let talk = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({ "addr": handle2, "text": "I'll paste later", "from": "owner", "via": "compose" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(talk.status, "200 OK", "{}", talk.body);
        let snap2 = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle2.as_str())])).expect("read"));
        let secret_item = snap2["items"].as_array().expect("items").iter().find(|i| i["id"] == pid).expect("secret card");
        assert_eq!(secret_item["doc"]["secret"]["status"], "pending", "talking must leave the secret card open: {snap2}");
        assert_eq!(vault.request_get(&pid).expect("req").status, "pending");
        // The person dismisses it: void, and the vault request goes too.
        let dismissed = dispatch_post(
            "/cli/thread/void",
            &HashMap::new(),
            serde_json::json!({ "addr": handle2, "id": pid }).to_string().as_bytes(),
        );
        assert_eq!(dismissed.status, "200 OK", "{}", dismissed.body);
        assert_eq!(vault.request_get(&pid).expect("req").status, "void");
        let _ = project_id2;
    }

    #[test]
    fn secret_cards_refuse_agent_wildcards_and_long_prompts() {
        crate::vault::ensure_test_vault();
        let handle = format!("ovlwc{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let wc = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "name": "K", "for": "*.stripe.com", "from": "k2" }).to_string().as_bytes(),
        );
        assert_eq!(wc.status, "400 Bad Request", "{}", wc.body);
        assert!(wc.body.contains("wildcard_not_allowed"), "{}", wc.body);
        let long = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "name": "K", "prompt": "x".repeat(201), "from": "k2" }).to_string().as_bytes(),
        );
        assert_eq!(long.status, "400 Bad Request", "{}", long.body);
    }

    #[test]
    fn get_thread_defaults_to_newest_25_with_has_more() {
        let handle = format!("ovlpage{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for i in 1..=40 {
                overlay::post_thread(
                    &conn,
                    &conv,
                    &project_id,
                    "k2",
                    &handle,
                    &format!("m{i}"),
                    "thread",
                )
                .expect("post");
            }
        }

        let get =
            dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread");
        assert_eq!(get.status, "200 OK", "{}", get.body);
        let snap = json_body(&get);
        assert_eq!(snap["ok"], true, "{snap}");
        assert_eq!(snap["has_more"], true, "40 items default page 25: {snap}");
        let items = snap["items"].as_array().expect("items array");
        assert_eq!(items.len(), 25, "{snap}");
        assert_eq!(items[0]["seq"], 16);
        assert_eq!(items[24]["seq"], 40);

        let older = dispatch(
            "/cli/thread",
            &params_of(&[
                ("addr", handle.as_str()),
                ("before_seq", "16"),
                ("limit", "25"),
            ]),
        )
        .expect("GET older");
        assert_eq!(older.status, "200 OK", "{}", older.body);
        let snap2 = json_body(&older);
        assert_eq!(snap2["has_more"], false, "{snap2}");
        let items2 = snap2["items"].as_array().expect("older items");
        assert_eq!(items2.len(), 15, "{snap2}");
        assert_eq!(items2[0]["seq"], 1);
        assert_eq!(items2[14]["seq"], 15);
    }

    fn skin_pass(rooms: &[String]) -> k2_core::skin::SkinPass {
        k2_core::skin::SkinPass {
            id: "skin-pass".into(),
            principal_id: Some("prin".into()),
            username: "guest".into(),
            caps: vec!["thread:read".into(), "thread:post".into()],
            rooms: rooms.to_vec(),
            session: false,
            room_policy: k2_core::skin::RoomPolicy::new(),
            app: String::new(),
            room_paths: Default::default(),
        }
    }

    #[test]
    fn skin_thread_rooms_pinned_only() {
        let handle = format!("ovlskin{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("ovloth{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let (other_id, _) = seed(&other);
        let pin_id = uuid::Uuid::new_v4().to_string();
        let side = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &pin_id);
        pin(&other_id, &uuid::Uuid::new_v4().to_string());
        sidecar(&project_id, &side, "reviewer");

        let pass = skin_pass(&[project_id.clone()]);
        let get = with_request_skin(Some(pass.clone()), || {
            dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET")
        });
        assert_eq!(get.status, "200 OK", "{}", get.body);

        let other_get = with_request_skin(Some(pass.clone()), || {
            dispatch("/cli/thread", &params_of(&[("addr", other.as_str())])).expect("GET other")
        });
        assert_eq!(other_get.status, "403 Forbidden", "{}", other_get.body);
        assert!(other_get.body.contains("skin_room"), "{}", other_get.body);

        let sidecar_addr = format!("{handle}/reviewer");
        let side_get = with_request_skin(Some(pass.clone()), || {
            dispatch(
                "/cli/thread",
                &params_of(&[("addr", sidecar_addr.as_str())]),
            )
            .expect("GET sidecar")
        });
        assert_eq!(side_get.status, "403 Forbidden", "{}", side_get.body);

        let compose = with_request_skin(Some(pass), || {
            dispatch_post(
                "/cli/thread/post",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "text": "nope",
                    "via": "compose",
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(compose.status, "403 Forbidden", "{}", compose.body);
    }

    #[test]
    fn skin_thread_post_injects_agent_pty_and_stamps_pass_username() {
        let handle = format!("ovlskinj{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let pin_id = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &pin_id);
        let body = format!("skin-pty-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let pass = skin_pass(&[project_id]);

        let post = with_request_skin(Some(pass.clone()), || {
            dispatch_post(
                "/cli/thread/post",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "text": body,
                    "from": "owner",
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(post.status, "200 OK", "skin post failed: {}", post.body);
        let posted = json_body(&post);
        assert_eq!(
            posted["from"].as_str().expect("from"),
            "guest",
            "must stamp SkinPass.username, not body from; {posted}"
        );
        let want = format_thread_compose_pty_line("guest", &handle, &body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &want),
            "skin Thread post must inject [from guest] [thread:addr] into the agent PTY; want {want:?} got {injects:?}"
        );
        assert!(
            injects
                .iter()
                .filter(|l| l.contains(&body))
                .all(|l| l.contains("[from guest]") && !l.contains("[from owner]")),
            "must not stamp spoofed body from; got {injects:?}"
        );

        let before = recorded_injects();
        let compose_text = format!("compose-blocked-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let compose = with_request_skin(Some(pass), || {
            dispatch_post(
                "/cli/thread/post",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "text": compose_text,
                    "via": "compose",
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(compose.status, "403 Forbidden", "{}", compose.body);
        let after = recorded_injects();
        assert_eq!(
            after, before,
            "via=compose 403 must not add a compose inject; after={after:?}"
        );
        assert!(
            after.iter().all(|l| !l.contains(&compose_text)),
            "via=compose 403 must not inject the compose body; got {after:?}"
        );
    }

    #[test]
    fn thread_post_empty_or_k2_from_stamps_room_handle_not_k2() {
        let handle = format!("dannon-cherokee{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let pin_id = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &pin_id);

        let no_from = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": "no-from",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(no_from.status, "200 OK", "{}", no_from.body);
        let posted = json_body(&no_from);
        assert_eq!(
            posted["from"].as_str().expect("from"),
            handle.as_str(),
            "missing from must stamp the room handle, not k2: {posted}"
        );
        assert_ne!(posted["from"], "k2");

        let as_k2 = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": "from-k2",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(as_k2.status, "200 OK", "{}", as_k2.body);
        let posted_k2 = json_body(&as_k2);
        assert_eq!(
            posted_k2["from"].as_str().expect("from"),
            handle.as_str(),
            "from=k2 must rewrite to the room handle: {posted_k2}"
        );
        assert_ne!(posted_k2["from"], "k2");

        let pass = skin_pass(&[project_id]);
        let skin_post = with_request_skin(Some(pass), || {
            dispatch_post(
                "/cli/thread/post",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "text": "skin-guest",
                    "from": "k2",
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(skin_post.status, "200 OK", "{}", skin_post.body);
        let skin_posted = json_body(&skin_post);
        assert_eq!(
            skin_posted["from"].as_str().expect("from"),
            "guest",
            "skin post must stamp the pass username, not the room handle: {skin_posted}"
        );
        assert_ne!(skin_posted["from"], handle);
    }

    #[test]
    fn skin_answer_and_void_use_thread_rooms() {
        let handle = format!("ovlskans{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("ovlskoth{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let (other_id, _) = seed(&other);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        pin(&other_id, &uuid::Uuid::new_v4().to_string());
        let pass = skin_pass(&[project_id.clone()]);

        let ask = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "Ship it?",
                "options": "Go,Stop",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask.status, "200 OK", "ask failed: {}", ask.body);
        let id = json_body(&ask)["id"].as_str().expect("id").to_string();
        assert!(!id.is_empty(), "ask must return an id: {}", ask.body);

        let other_ask = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": other,
                "prompt": "Other?",
                "options": "A,B",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(other_ask.status, "200 OK", "{}", other_ask.body);
        let other_id = json_body(&other_ask)["id"]
            .as_str()
            .expect("other id")
            .to_string();

        let denied = with_request_skin(Some(pass.clone()), || {
            dispatch_post(
                "/cli/thread/answer",
                &HashMap::new(),
                serde_json::json!({
                    "addr": other,
                    "id": other_id,
                    "answer": "A",
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(denied.status, "403 Forbidden", "{}", denied.body);
        assert!(
            denied.body.contains("skin_room"),
            "addr not in rooms must be skin_room: {}",
            denied.body
        );

        let answered = with_request_skin(Some(pass.clone()), || {
            dispatch_post(
                "/cli/thread/answer",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "id": id,
                    "answer": "Go",
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(answered.status, "200 OK", "skin answer: {}", answered.body);
        let body = json_body(&answered);
        assert_eq!(body["ok"], true, "{body}");
        assert_eq!(body["status"], "answered", "{body}");
        assert_eq!(body["answer"], "Go", "{body}");

        let ask2 = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "Void me?",
                "options": "Go,Stop",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask2.status, "200 OK", "{}", ask2.body);
        let id2 = json_body(&ask2)["id"].as_str().expect("id2").to_string();
        let voided = with_request_skin(Some(pass), || {
            dispatch_post(
                "/cli/thread/void",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "id": id2,
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(voided.status, "200 OK", "skin void: {}", voided.body);
        let vbody = json_body(&voided);
        assert_eq!(vbody["ok"], true, "{vbody}");
        assert_eq!(vbody["status"], "voided", "{vbody}");
    }

    #[test]
    fn skin_secret_answer_omits_value() {
        crate::vault::ensure_test_vault();
        let handle = format!("ovlsksec{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let secret_val = "s3cr3t-NEVER-IN-SKIN-ANSWER-xyz";
        let posted = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "name": "API_TOKEN",
                "prompt": "Paste the token",
                "for": "api.example.com",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let id = json_body(&posted)["id"].as_str().expect("id").to_string();
        let pass = skin_pass(&[project_id]);
        let set = with_request_skin(Some(pass), || {
            dispatch_post(
                "/cli/thread/answer",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "id": id,
                    "secret": secret_val,
                })
                .to_string()
                .as_bytes(),
            )
        });
        assert_eq!(set.status, "200 OK", "{}", set.body);
        assert!(
            !set.body.contains(secret_val),
            "answer JSON must not contain secret: {}",
            set.body
        );
        let set_body = json_body(&set);
        assert_eq!(set_body["ok"], true, "{set_body}");
        assert_eq!(set_body["status"], "set", "{set_body}");
        assert_eq!(set_body["name"], "API_TOKEN", "{set_body}");
        assert!(
            set_body.get("secret").is_none() || set_body["secret"].as_str().is_none(),
            "must not echo secret value: {set_body}"
        );
        assert!(
            set_body.get("value").is_none(),
            "must not echo value: {set_body}"
        );
    }

    fn stamp_canonical(project_id: &str) -> HashMap<String, String> {
        let principal = HookPrincipal {
            workspace_uuid: project_id.to_string(),
            agent_address: project_id.to_string(),
        };
        let mut params = HashMap::new();
        crate::caller_workspace::stamp_principal(&mut params, &principal);
        params
    }

    /// `cell_session_id` is set before the stamp, matching the cell and HTTP arms.
    fn stamp_sidecar(project_id: &str, cell_session_id: &str) -> HashMap<String, String> {
        let principal = HookPrincipal {
            workspace_uuid: project_id.to_string(),
            agent_address: project_id.to_string(),
        };
        let mut params = HashMap::new();
        params.insert("cell_session_id".to_string(), cell_session_id.to_string());
        crate::caller_workspace::stamp_principal(&mut params, &principal);
        params
    }

    fn post_as_owner(params: &HashMap<String, String>, body: serde_json::Value) -> CliResponse {
        dispatch_post_as(
            "/cli/thread/post",
            params,
            body.to_string().as_bytes(),
            "owner",
        )
    }

    fn stored_text(addr: &str, body: &str) -> (String, String) {
        let get = dispatch("/cli/thread", &params_of(&[("addr", addr)])).expect("GET thread");
        assert_eq!(get.status, "200 OK", "{}", get.body);
        let snap = json_body(&get);
        let items = snap["items"].as_array().expect("items");
        let item = items
            .iter()
            .find(|i| i["doc"]["body"] == body)
            .unwrap_or_else(|| panic!("missing stored body {body}: {snap}"));
        (
            item["doc"]["from"]
                .as_str()
                .expect("stored from")
                .to_string(),
            item["doc"]["via"].as_str().expect("stored via").to_string(),
        )
    }

    #[test]
    fn stamped_post_keeps_handle_when_body_says_owner() {
        let handle = format!("ovlstamp{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("ovlother{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let (other_id, _) = seed(&other);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        pin(&other_id, &uuid::Uuid::new_v4().to_string());
        let params = stamp_canonical(&project_id);
        let stamped = params.get("from").expect("stamp from").clone();
        assert_eq!(stamped, handle, "canonical stamp is the workspace handle");
        assert!(
            !stamped.contains('/'),
            "canonical stamp must not be a sidecar: {stamped}"
        );
        assert_eq!(
            params.get("project_id").expect("stamp project_id").as_str(),
            project_id,
        );

        let text = format!("stamp-owner-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let post = post_as_owner(
            &params,
            serde_json::json!({
                "addr": handle,
                "text": text,
                "from": "owner",
                "principal_bound": 0,
                "project_id": other_id,
            }),
        );
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["from"].as_str().expect("json from"), handle, "{posted}");
        assert_eq!(posted["via"].as_str().expect("json via"), "thread", "{posted}");
        assert_ne!(posted["from"], "owner");
        let (stored_from, stored_via) = stored_text(&handle, &text);
        assert_eq!(stored_from, handle);
        assert_eq!(stored_via, "thread");

        let explicit = format!("explicit-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let kept = post_as_owner(
            &params,
            serde_json::json!({
                "addr": handle,
                "text": explicit,
                "from": "billing-bot",
            }),
        );
        assert_eq!(kept.status, "200 OK", "{}", kept.body);
        let kept_body = json_body(&kept);
        assert_eq!(
            kept_body["from"].as_str().expect("json from"),
            handle,
            "body from must not replace the stamp: {kept_body}"
        );
        assert_ne!(kept_body["from"], "billing-bot");

        let slashed = format!("slash-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let allowed = post_as_owner(
            &params,
            serde_json::json!({
                "addr": handle,
                "text": slashed,
                "from": format!("{handle}/spoof"),
            }),
        );
        assert_eq!(
            allowed.status, "200 OK",
            "canonical stamp with no slash must not be treated as a sidecar: {}",
            allowed.body
        );
        let allowed_body = json_body(&allowed);
        assert_eq!(
            allowed_body["from"].as_str().expect("json from"),
            handle,
            "{allowed_body}"
        );
        assert!(
            !allowed_body["from"].as_str().expect("from").contains('/'),
            "{allowed_body}"
        );

        let denied = post_as_owner(
            &params,
            serde_json::json!({
                "addr": other,
                "text": "cross",
                "from": "owner",
                "project_id": other_id,
            }),
        );
        assert_eq!(
            denied.status, "403 Forbidden",
            "body project_id must not replace the stamp: {}",
            denied.body
        );
        assert!(
            denied.body.contains("another workspace"),
            "cross-workspace write must stay refused: {}",
            denied.body
        );
    }

    #[test]
    fn sidecar_stamp_forbids_canonical_when_body_from_is_bare_handle() {
        let handle = format!("ovlside{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let pinned = uuid::Uuid::new_v4().to_string();
        let reviewer = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &pinned);
        sidecar(&project_id, &reviewer, "reviewer");
        let params = stamp_sidecar(&project_id, &reviewer);
        let stamped = params.get("from").expect("sidecar stamp from").clone();
        assert_eq!(
            stamped,
            format!("{handle}/reviewer"),
            "stamp_principal must write workspace/handle, not a hand-built slash: {stamped}"
        );
        assert!(stamped.contains('/'), "{stamped}");
        assert!(!handle.contains('/'));
        assert_ne!(handle, stamped);

        let post = post_as_owner(
            &params,
            serde_json::json!({
                "addr": handle,
                "text": "bare-sidecar",
                "from": handle,
            }),
        );
        assert_eq!(
            post.status, "403 Forbidden",
            "sidecar stamp must forbid the canonical room when body from drops the slash: {}",
            post.body
        );
        assert!(
            post.body.contains("canonical-only"),
            "must be the sidecar write gate: {}",
            post.body
        );
        let get = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET");
        assert_eq!(get.status, "200 OK", "{}", get.body);
        let snap = json_body(&get);
        let items = snap["items"].as_array().expect("items");
        assert!(
            items.iter().all(|i| i["doc"]["body"] != "bare-sidecar"),
            "forbidden write must not store: {snap}"
        );
    }

    #[test]
    fn owner_token_owner_shaped_from_stores_room_handle() {
        // Reads `$HOME` (the owner display name): hold the env lock so a
        // concurrent temp-HOME test cannot swap it mid-test.
        let _home_lock = crate::test_support::lock_home();
        let handle = format!("ovlown{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("ovlpeer{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let (_, _) = seed(&other);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let owner_name = workspace_msg::resolve_owner_from();
        assert!(!handle.eq_ignore_ascii_case("owner"));
        assert!(!handle.eq_ignore_ascii_case("k2"));
        assert!(!handle.eq_ignore_ascii_case(&owner_name));
        assert!(!other.eq_ignore_ascii_case("owner"));
        assert!(!other.eq_ignore_ascii_case("k2"));
        assert!(!other.eq_ignore_ascii_case(&owner_name));
        assert_ne!(other, handle);

        let shaped = [
            "owner".to_string(),
            " owner".to_string(),
            "owner\n".to_string(),
            format!(" {}\n", owner_name.to_ascii_uppercase()),
        ];
        for from in shaped {
            let text = format!("shaped-{}-{}", from.len(), uuid::Uuid::new_v4());
            let post = dispatch_post(
                "/cli/thread/post",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "text": text,
                    "from": from,
                })
                .to_string()
                .as_bytes(),
            );
            assert_eq!(post.status, "200 OK", "from {from:?}: {}", post.body);
            let posted = json_body(&post);
            assert_eq!(
                posted["from"].as_str().expect("json from"),
                handle,
                "owner-shaped from {from:?} must store the room handle: {posted}"
            );
            assert_ne!(posted["from"], "owner");
            assert_eq!(posted["via"].as_str().expect("via"), "thread");
            let (stored_from, stored_via) = stored_text(&handle, &text);
            assert_eq!(stored_from, handle, "stored from for {from:?}");
            assert_eq!(stored_via, "thread");
        }

        let text = format!("peer-{}", uuid::Uuid::new_v4());
        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": text,
                "from": format!(" {other} "),
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(
            posted["from"].as_str().expect("json from"),
            other,
            "explicit non-owner handle is stored trimmed, not rewritten: {posted}"
        );
        let (stored_from, _) = stored_text(&handle, &text);
        assert_eq!(stored_from, other);
    }

    #[test]
    fn compose_without_principal_ignores_body_from() {
        // Reads `$HOME` (the owner display name): hold the env lock so a
        // concurrent temp-HOME test cannot swap it mid-test.
        let _home_lock = crate::test_support::lock_home();
        let handle = format!("ovlcmp{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let author = workspace_msg::resolve_owner_from();
        let text = format!("compose-ignore-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": text,
                "via": "compose",
                "from": " owner",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(
            posted["from"].as_str().expect("json from"),
            author.as_str(),
            "via=compose must use compose_from_for_session, not body from: {posted}"
        );
        assert_eq!(posted["via"].as_str().expect("via"), "compose");
        assert_ne!(posted["from"], " owner");
        let hist = k2_core::workspace_compose_history::list_compose_send_history(&project_id)
            .expect("compose history");
        assert!(
            hist.iter().any(|e| e.body == text),
            "no-principal compose must still record history: {hist:?}"
        );
        let want = format_thread_compose_pty_line(&author, &handle, &text);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &want),
            "no-principal compose must still inject; want {want:?} got {injects:?}"
        );
    }

    #[test]
    fn bound_principal_via_compose_stores_thread_not_human() {
        let handle = format!("ovlbnd{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let params = stamp_canonical(&project_id);
        let stamped = params.get("from").expect("stamp from").clone();
        assert_eq!(stamped, handle);
        let text = format!("bound-compose-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let post = post_as_owner(
            &params,
            serde_json::json!({
                "addr": handle,
                "text": text,
                "from": "owner",
                "via": "compose",
            }),
        );
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(posted["from"].as_str().expect("json from"), stamped, "{posted}");
        assert_eq!(posted["via"].as_str().expect("json via"), "thread", "{posted}");
        assert_ne!(posted["via"], "compose");
        let (stored_from, stored_via) = stored_text(&handle, &text);
        assert_eq!(stored_from, stamped);
        assert_eq!(stored_via, "thread");
        let hist = k2_core::workspace_compose_history::list_compose_send_history(&project_id)
            .expect("compose history");
        assert!(
            hist.iter().all(|e| e.body != text),
            "bound principal must not record compose history: {hist:?}"
        );
        let injects = recorded_injects();
        assert!(
            injects.iter().all(|l| !l.contains(&text)),
            "bound principal must not inject a human compose line: {injects:?}"
        );
    }

    #[test]
    fn bound_stamp_owner_is_not_rewritten_to_room_handle() {
        let handle = format!("ovlraw{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        assert_ne!(handle, "owner");
        let mut params = HashMap::new();
        params.insert(
            crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(),
            "1".to_string(),
        );
        params.insert("project_id".to_string(), project_id.clone());
        params.insert("from".to_string(), "owner".to_string());
        let text = format!("raw-owner-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let post = post_as_owner(
            &params,
            serde_json::json!({
                "addr": handle,
                "text": text,
                "from": "billing-bot",
                "via": "compose",
            }),
        );
        assert_eq!(post.status, "200 OK", "{}", post.body);
        let posted = json_body(&post);
        assert_eq!(
            posted["from"].as_str().expect("json from"),
            "owner",
            "stamp string owner must win over the room handle and body from: {posted}"
        );
        assert_ne!(posted["from"], handle);
        assert_eq!(posted["via"].as_str().expect("via"), "thread");
        let (stored_from, stored_via) = stored_text(&handle, &text);
        assert_eq!(stored_from, "owner");
        assert_eq!(stored_via, "thread");
        let hist = k2_core::workspace_compose_history::list_compose_send_history(&project_id)
            .expect("compose history");
        assert!(
            hist.iter().all(|e| e.body != text),
            "owner-string stamp must not take the human path: {hist:?}"
        );
    }

    #[test]
    fn forged_principal_bound_does_not_keep_from_owner() {
        let handle = format!("ovlforge{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        for (label, flag) in [
            ("number", serde_json::json!(1)),
            ("bool", serde_json::json!(true)),
            ("string", serde_json::json!("1")),
        ] {
            let text = format!("forged-{label}-{}", uuid::Uuid::new_v4());
            let post = dispatch_post(
                "/cli/thread/post",
                &HashMap::new(),
                serde_json::json!({
                    "addr": handle,
                    "text": text,
                    "from": "owner",
                    "principal_bound": flag,
                })
                .to_string()
                .as_bytes(),
            );
            assert_eq!(post.status, "200 OK", "{label}: {}", post.body);
            let posted = json_body(&post);
            assert_eq!(
                posted["from"].as_str().expect("json from"),
                handle,
                "forged principal_bound ({label}) must not keep from=owner: {posted}"
            );
            assert_ne!(posted["from"], "owner");
            let (stored_from, _) = stored_text(&handle, &text);
            assert_eq!(stored_from, handle, "{label}");
        }
    }

    #[test]
    fn ask_and_secret_follow_from_rules_and_echo_from() {
        // Reads `$HOME` (the owner display name): hold the env lock so a
        // concurrent temp-HOME test cannot swap it mid-test.
        let _home_lock = crate::test_support::lock_home();
        let handle = format!("ovlcard{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("ovlcardp{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let (_, _) = seed(&other);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let params = stamp_canonical(&project_id);
        let stamped = params.get("from").expect("stamp from").clone();
        assert_eq!(stamped, handle);
        assert!(!other.eq_ignore_ascii_case("owner"));
        assert!(!other.eq_ignore_ascii_case("k2"));
        assert!(!other.eq_ignore_ascii_case(&workspace_msg::resolve_owner_from()));

        let ask_stamp = dispatch_post_as(
            "/cli/thread/ask",
            &params,
            serde_json::json!({
                "addr": handle,
                "prompt": "Stamp ask?",
                "options": "Go,Stop",
                "from": "owner",
            })
            .to_string()
            .as_bytes(),
            "owner",
        );
        assert_eq!(ask_stamp.status, "200 OK", "{}", ask_stamp.body);
        let ask_stamp_body = json_body(&ask_stamp);
        assert_eq!(
            ask_stamp_body["from"].as_str().expect("ask json from"),
            stamped,
            "{ask_stamp_body}"
        );
        let ask_id = ask_stamp_body["id"].as_str().expect("ask id");
        let get = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET");
        let snap = json_body(&get);
        let ask_item = snap["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["id"] == ask_id)
            .expect("ask item");
        assert_eq!(ask_item["doc"]["from"], stamped);

        let secret_name = format!("TOK_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let secret_stamp = dispatch_post_as(
            "/cli/thread/secret",
            &params,
            serde_json::json!({
                "addr": handle,
                "name": secret_name,
                "prompt": "Stamp secret",
                "from": " owner",
            })
            .to_string()
            .as_bytes(),
            "owner",
        );
        assert_eq!(secret_stamp.status, "200 OK", "{}", secret_stamp.body);
        let secret_stamp_body = json_body(&secret_stamp);
        assert_eq!(
            secret_stamp_body["from"].as_str().expect("secret json from"),
            stamped,
            "{secret_stamp_body}"
        );

        let ask_owner = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "Owner ask?",
                "options": "Go,Stop",
                "from": "owner\n",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask_owner.status, "200 OK", "{}", ask_owner.body);
        let ask_owner_body = json_body(&ask_owner);
        assert_eq!(
            ask_owner_body["from"].as_str().expect("ask json from"),
            handle,
            "unbound ask owner-shaped from must be the room handle: {ask_owner_body}"
        );
        assert_ne!(ask_owner_body["from"], "owner");

        let ask_peer = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "Peer ask?",
                "options": "Go,Stop",
                "from": other,
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask_peer.status, "200 OK", "{}", ask_peer.body);
        let ask_peer_body = json_body(&ask_peer);
        assert_eq!(
            ask_peer_body["from"].as_str().expect("ask json from"),
            other,
            "{ask_peer_body}"
        );

        let secret_owner_name = format!("OWN_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let secret_owner = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "name": secret_owner_name,
                "prompt": "Owner secret",
                "from": " owner",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(secret_owner.status, "200 OK", "{}", secret_owner.body);
        let secret_owner_body = json_body(&secret_owner);
        assert_eq!(
            secret_owner_body["from"].as_str().expect("secret json from"),
            handle,
            "{secret_owner_body}"
        );

        let secret_peer_name = format!("PEER_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let secret_peer = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "name": secret_peer_name,
                "prompt": "Peer secret",
                "from": format!(" {other} "),
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(secret_peer.status, "200 OK", "{}", secret_peer.body);
        let secret_peer_body = json_body(&secret_peer);
        assert_eq!(
            secret_peer_body["from"].as_str().expect("secret json from"),
            other,
            "explicit secret from is the trimmed handle: {secret_peer_body}"
        );
    }

    // ── Thread survives a tab rename (prd-thread-survives-tab-rename-v1) ──

    fn project_path(project_id: &str) -> String {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            params![project_id],
            |r| r.get::<_, String>(0),
        )
        .expect("project path")
    }

    /// A sidecar with ordinal 1 and no name, like a fresh app tab.
    fn unnamed_sidecar(project_id: &str, conv: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace_session_handles::allocate_ordinal(&conn, project_id, conv)
            .expect("ordinal");
        conn.execute(
            "INSERT INTO workspace_tab_sessions \
             (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
            params![project_id, format!("pane-{conv}"), format!("tab-pane-{conv}"), conv],
        )
        .expect("tab");
    }

    /// The app's rename: `POST /cli/chat/rename` with the workspace path.
    fn rename_via_route(conv: &str, name: &str, path: &str) -> serde_json::Value {
        let resp = crate::chat_routes::handle_rename(
            serde_json::json!({
                "provider": "claude",
                "session_id": conv,
                "custom_name": name,
                "project_path": path,
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "rename to {name:?} failed: {}", resp.body);
        json_body(&resp)
    }

    fn post_thread(addr: &str, text: &str, via: Option<&str>) -> serde_json::Value {
        let mut body = serde_json::json!({ "addr": addr, "text": text, "from": "k2" });
        if let Some(v) = via {
            body["via"] = serde_json::json!(v);
        }
        let resp = dispatch_post("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes());
        assert_eq!(resp.status, "200 OK", "post to {addr} failed: {}", resp.body);
        json_body(&resp)
    }

    fn read_thread(addr: &str) -> serde_json::Value {
        let get = dispatch("/cli/thread", &params_of(&[("addr", addr), ("limit", "0")]))
            .expect("GET thread");
        assert_eq!(get.status, "200 OK", "read {addr}: {}", get.body);
        json_body(&get)
    }

    /// Tests 1, 2, 8: the old ordinal and old names keep posting into the
    /// same Thread, answer with the current address + `movedFrom`, and the
    /// stored `to` is the address current at post time.
    #[test]
    fn old_addresses_keep_posting_after_renames() {
        let handle = format!("ovlrn{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let conv = uuid::Uuid::new_v4().to_string();
        unnamed_sidecar(&project_id, &conv);
        let path = project_path(&project_id);
        let ord = format!("{handle}/1");

        let first = post_thread(&ord, "before", None);
        assert_eq!(first["addr"], ord.as_str());
        assert!(first.get("movedFrom").is_none(), "no rename yet: {first}");

        let renamed = rename_via_route(&conv, "Reviewer", &path);
        assert_eq!(renamed["address"], format!("{handle}/reviewer"), "{renamed}");
        assert_eq!(renamed["previousAddress"], ord.as_str(), "{renamed}");

        let after = post_thread(&ord, "after", None);
        assert_eq!(after["conversation_id"], conv.as_str());
        assert_eq!(after["addr"], format!("{handle}/reviewer"), "{after}");
        assert_eq!(after["movedFrom"], ord.as_str(), "{after}");
        assert_eq!(after["to"], format!("{handle}/reviewer"), "stored to is current: {after}");

        let snap = read_thread(&format!("{handle}/reviewer"));
        let items = snap["items"].as_array().expect("items");
        let bodies: Vec<&str> = items.iter().map(|i| i["doc"]["body"].as_str().expect("body")).collect();
        assert_eq!(bodies, vec!["before", "after"], "{snap}");
        assert_eq!(items[0]["doc"]["to"], ord.as_str(), "history keeps its address");
        assert_eq!(
            snap["pastAddresses"],
            serde_json::json!([ord.clone()]),
            "Q5: old addresses for render-time mapping: {snap}"
        );

        rename_via_route(&conv, "Critic", &path);
        let via_old_name = post_thread(&format!("{handle}/reviewer"), "third", None);
        assert_eq!(via_old_name["conversation_id"], conv.as_str());
        assert_eq!(via_old_name["addr"], format!("{handle}/critic"));
        assert_eq!(via_old_name["movedFrom"], format!("{handle}/reviewer"));
        let via_ord = post_thread(&ord, "fourth", None);
        assert_eq!(via_ord["conversation_id"], conv.as_str());
        let snap = read_thread(&ord);
        assert_eq!(snap["items"].as_array().expect("items").len(), 4, "{snap}");
        assert_eq!(snap["addr"], format!("{handle}/critic"));
        assert_eq!(snap["movedFrom"], ord.as_str());
    }

    /// Test 7: a compose post to the old ordinal injects the CURRENT
    /// address, so the agent learns the new name from the next message.
    #[test]
    fn compose_to_an_old_address_injects_the_current_one() {
        let handle = format!("ovlinj{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let conv = uuid::Uuid::new_v4().to_string();
        unnamed_sidecar(&project_id, &conv);
        rename_via_route(&conv, "Reviewer", &project_path(&project_id));
        let text = format!("lint-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let posted = post_thread(&format!("{handle}/1"), &text, Some("compose"));
        let from = posted["from"].as_str().expect("from").to_string();
        let want = format_thread_compose_pty_line(&from, &format!("{handle}/reviewer"), &text);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l == &want),
            "want {want:?} in {injects:?}"
        );
        assert!(
            !injects.iter().any(|l| l.contains(&format!("[thread:{handle}/1] {text}"))),
            "the stale address must not be injected: {injects:?}"
        );
    }

    /// Test 9: a Chats name on the pinned conversation is the main Chat:
    /// canonical_alias, so a sidecar may not write it through `ws/<name>`.
    #[test]
    fn a_named_pinned_chat_stays_canonical_only() {
        let handle = format!("ovllead{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let pinned = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &pinned);
        let reviewer = uuid::Uuid::new_v4().to_string();
        sidecar(&project_id, &reviewer, "reviewer");
        rename_via_route(&pinned, "Lead", &project_path(&project_id));

        let owner = post_thread(&format!("{handle}/lead"), "owner-ok", None);
        assert_eq!(owner["conversation_id"], pinned.as_str());
        assert_eq!(owner["addr"], handle.as_str(), "the pinned Chat answers at the workspace: {owner}");
        assert!(owner.get("movedFrom").is_none(), "movedFrom means a sidecar rename only: {owner}");

        let params = stamp_sidecar(&project_id, &reviewer);
        let post = post_as_owner(
            &params,
            serde_json::json!({ "addr": format!("{handle}/lead"), "text": "sneaky" }),
        );
        assert_eq!(post.status, "403 Forbidden", "{}", post.body);
        assert!(post.body.contains("canonical-only"), "{}", post.body);
        let ws = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET");
        assert_eq!(json_body(&ws)["conversation_id"], pinned.as_str(), "`ws` unchanged");
    }

    /// Test 10 (Side finding B): a sidecar's card answer injects into the
    /// sidecar's address, never the workspace's. (Typing in the Terminal no
    /// longer answers a card: only a tap does.)
    #[test]
    fn sidecar_card_answer_goes_to_the_sidecar_address() {
        let handle = format!("ovlcard{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let conv = uuid::Uuid::new_v4().to_string();
        sidecar(&project_id, &conv, "reviewer");
        let label = format!("Ship{}", &uuid::Uuid::new_v4().to_string()[..6]);
        let ask = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": format!("{handle}/reviewer"),
                "prompt": "Go?",
                "options": format!("{label},Wait"),
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask.status, "200 OK", "{}", ask.body);
        let id = json_body(&ask)["id"].as_str().expect("id").to_string();
        let answer = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({ "addr": format!("{handle}/reviewer"), "id": id, "answer": label })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(answer.status, "200 OK", "{}", answer.body);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l.contains(&format!("[thread:{handle}/reviewer] ")) && l.contains(&label)),
            "card callback must address the sidecar: {injects:?}"
        );
        assert!(
            !injects.iter().any(|l| l.contains(&format!("[thread:{handle}] ")) && l.contains(&label)),
            "never the main Chat: {injects:?}"
        );
    }

    /// Test 11: the working-strip read follows an old address.
    #[test]
    fn thread_activity_answers_on_an_old_address() {
        let handle = format!("ovlact{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let conv = uuid::Uuid::new_v4().to_string();
        unnamed_sidecar(&project_id, &conv);
        rename_via_route(&conv, "Reviewer", &project_path(&project_id));
        let resp = dispatch("/cli/thread/activity", &params_of(&[("addr", &format!("{handle}/1"))]))
            .expect("activity");
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let body = json_body(&resp);
        assert_eq!(body["conversation_id"], conv.as_str());
        assert_eq!(body["addr"], format!("{handle}/reviewer"));
        assert_eq!(body["movedFrom"], format!("{handle}/1"));
    }

    /// An unknown sidecar address is still a 404, and the hint lists the
    /// workspace's sidecars instead of a bare "unknown".
    #[test]
    fn unknown_sidecar_address_404_lists_sidecars() {
        let handle = format!("ovlunk{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        sidecar(&project_id, &conv, "reviewer");
        let resp = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({ "addr": format!("{handle}/nobody"), "text": "x" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(resp.body.contains(&format!("{handle}/reviewer")), "{}", resp.body);
    }

    // ── Thread push trigger (prd-thread-push-notifications-v1 §6.1) ──

    /// A workspace with a pinned conversation and an owner phone, in this
    /// test's own DB. Returns (handle, conversation id).
    fn push_ws(tag: &str) -> (String, String) {
        let handle = format!("ovlpush{tag}{}", &uuid::Uuid::new_v4().to_string()[..6]);
        let (project_id, _) = seed(&handle);
        let conv = uuid::Uuid::new_v4().to_string();
        pin(&project_id, &conv);
        (handle, conv)
    }

    fn thread_pushes(mark: usize, conv: &str) -> Vec<k2_core::push::ThreadPush> {
        crate::push_routes::test_push_capture::since(mark)
            .into_iter()
            .filter_map(|e| match e {
                k2_core::push::PushEvent::ThreadPost(t) if t.conversation_id == conv => Some(t),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn agent_posts_at_all_three_sites_push_previews_but_never_secrets() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        k2_core::push_devices::upsert_device("ovlpush-dev", "apns", "ovlpush-tok", "owner").expect("phone");
        let (text_ws, text_conv) = push_ws("t");
        let (ask_ws, ask_conv) = push_ws("c");
        let (sec_ws, sec_conv) = push_ws("s");
        let mark = crate::push_routes::test_push_capture::mark();

        let r = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": text_ws, "text": "## Plan\n\nThe **launch** plan is ready. Read it tonight. Then sleep.", "from": "k2" }));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let seq = json_body(&r)["seq"].as_i64().expect("seq");
        let r = post_json(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({ "addr": ask_ws, "prompt": "Ship *it*?", "options": "Go,Stop", "from": "k2" }),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let r = post_json(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": sec_ws, "name": "STRIPE_KEY", "for": "api.stripe.com", "from": "k2" }),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);

        let t = thread_pushes(mark, &text_conv);
        assert_eq!(t.len(), 1, "text reply pushes once");
        assert_eq!((t[0].msg_kind, t[0].seq, t[0].count), (k2_core::push::ThreadMsgKind::Text, seq, 1));
        assert_eq!(t[0].principal, "owner");
        assert_eq!(t[0].device_usernames, vec!["owner".to_string()]);
        assert_eq!(t[0].addr, text_ws);
        let c = thread_pushes(mark, &ask_conv);
        assert_eq!(c[0].msg_kind, k2_core::push::ThreadMsgKind::Choice);
        assert_eq!(k2_core::push::PushEvent::ThreadPost(c[0].clone()).body(), "Ship it?", "a choice card shows its question");
        let s = thread_pushes(mark, &sec_conv);
        assert_eq!(s[0].msg_kind, k2_core::push::ThreadMsgKind::Secret);
        let ev = k2_core::push::PushEvent::ThreadPost(s[0].clone());
        let wire = k2_core::push::K2Cloud::payload(&ev, &k2_core::push::PushStamp::default(), "owner").to_string();
        assert!(!wire.contains("STRIPE") && !wire.contains("stripe"), "a secret card never names the secret: {wire}");
        let wire = k2_core::push::K2Cloud::payload(&k2_core::push::PushEvent::ThreadPost(t[0].clone()), &Default::default(), "owner").to_string();
        // 0.46.1: the text reply previews its first two sentences, markdown stripped.
        assert_eq!(t[0].preview, "Plan — The launch plan is ready.");
        assert!(wire.contains("\"body\":\"Plan — The launch plan is ready.\""), "preview in the body: {wire}");
        assert!(!wire.contains("Then sleep") && !wire.contains("**"), "two sentences, stripped: {wire}");
        assert!(s[0].preview.is_empty(), "a secret card carries no preview");
    }

    /// Push titles name the agent by its DISPLAY name (`projects.name`, as
    /// `/cli/projects/list` shows it), not its handle — in both the alert
    /// title and `data.agentName`; a sidecar reads "<Display> · <sidecar>";
    /// a blank name falls back to the handle; an unknown `from` is as is.
    #[test]
    fn push_titles_use_the_display_name_not_the_handle() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        k2_core::push_devices::upsert_device("ovlpush-dev-name", "apns", "ovlpush-tok-name", "owner").expect("phone");
        let tag = &uuid::Uuid::new_v4().to_string()[..6];
        let handle = format!("k2-companion-{tag}");
        let bare = format!("bare-{tag}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for (h, name) in [(handle.as_str(), "K2 Companion"), (bare.as_str(), "")] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                    params![uuid::Uuid::new_v4().to_string(), name, format!("/tmp/ovl-name-{h}"), h],
                )
                .expect("seed");
            }
        }
        let title_for = |from: &str| {
            let (ws, conv) = push_ws("name");
            let mark = crate::push_routes::test_push_capture::mark();
            let r = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": ws, "text": "Done.", "from": from }));
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let t = thread_pushes(mark, &conv);
            assert_eq!(t.len(), 1, "from {from}: one push");
            let ev = k2_core::push::PushEvent::ThreadPost(t[0].clone());
            assert_eq!(ev.data().get("agentName"), Some(ev.title().as_str()), "title and agentName agree");
            ev.title()
        };
        assert_eq!(title_for(&handle), "K2 Companion");
        assert_eq!(title_for(&format!("{handle}/reviewer")), "K2 Companion · reviewer");
        assert_eq!(title_for(&bare), bare, "no display name: the handle");
        assert_eq!(title_for(&format!("{bare}/qa")), format!("{bare} · qa"));
        assert_eq!(title_for("nobody-here-x9"), "nobody-here-x9", "not a workspace: as is");
    }

    #[test]
    fn human_compose_guest_and_human_from_posts_never_push() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        k2_core::connect_users::add_user("ovlpushgina", "password123").expect("gina");
        k2_core::push_devices::upsert_device("ovlpush-dev2", "apns", "ovlpush-tok2", "owner").expect("phone");
        let (compose_ws, compose_conv) = push_ws("compose");
        let (human_ws, human_conv) = push_ws("human");
        let (guest_ws, guest_conv) = push_ws("guest");
        let mark = crate::push_routes::test_push_capture::mark();

        let r = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": compose_ws, "text": "hi", "via": "compose" }));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        // An owner-token shell post signed as a Connect user is a human.
        let r = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": human_ws, "text": "hi", "from": "ovlpushgina" }));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let guest_project = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row("SELECT id FROM projects WHERE handle = ?1", params![guest_ws], |r| r.get::<_, String>(0))
                .expect("guest project")
        };
        let r = with_request_skin(Some(skin_pass(&[guest_project])), || {
            post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": guest_ws, "text": "hello" }))
        });
        assert_eq!(r.status, "200 OK", "{}", r.body);
        for conv in [&compose_conv, &human_conv, &guest_conv] {
            assert!(thread_pushes(mark, conv).is_empty(), "no push for {conv}");
        }
    }

    #[test]
    fn airgap_on_means_no_thread_push() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        k2_core::push_devices::upsert_device("ovlpush-dev3", "apns", "ovlpush-tok3", "owner").expect("phone");
        let (ws, conv) = push_ws("air");
        crate::push_routes::set_test_airgap(true);
        let mark = crate::push_routes::test_push_capture::mark();
        let r = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": ws, "text": "x", "from": "k2" }));
        crate::push_routes::set_test_airgap(false);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert!(thread_pushes(mark, &conv).is_empty(), "air-gap: zero sends");
    }

    /// A clone's Threads (CL5/CL30/CL31): with no pin yet the clone's
    /// address shows the pending imported history and refuses writes
    /// (409 `imported_read_only`); `import:<id>` resolves too. Once a pin
    /// exists, the next resolve settles the history onto it, new posts
    /// work, and imported cards stay read-only.
    #[test]
    fn imported_thread_is_read_only_and_settles_on_resolve() {
        use k2_core::overlay::transfer as t;
        fn resolved_ok(addr: &str) -> ResolvedOverlay {
            match resolve_addr(addr) {
                Ok(r) => r,
                Err(e) => panic!("resolve {addr}: {} {}", e.status, e.body),
            }
        }
        let uid = || uuid::Uuid::new_v4().to_string();
        let src = format!("ovlctsrc{}", &uid()[..8]);
        let (src_id, _) = seed(&src);
        let src_pin = uid();
        pin(&src_id, &src_pin);
        let export = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            overlay::post_thread(&conn, &src_pin, &src_id, &src, &src, "history line", "thread").unwrap();
            overlay::post_choice(&conn, &src_pin, &src_id, &src, &src, "Ship?", vec!["Go".into()], false, None).unwrap();
            t::export_project_threads(&conn, &src_id, &t::ExportOptions::new("ct-route", "src.k2.dev", t::ThreadsChoice::All))
                .unwrap()
        };
        let dst = format!("ovlctdst{}", &uid()[..8]);
        let (dst_id, _) = seed(&dst);
        let rep = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            t::import_threads(&conn, &export, &t::ImportContext { project_id: dst_id.clone(), ..Default::default() }).unwrap()
        };
        assert!(rep.pending, "{rep:?}");
        let key = rep.conversations[0].local_conversation_id.clone();
        let card = overlay::read_thread(&key, 0)
            .unwrap()
            .into_iter()
            .find(|i| i.doc.choice.is_some())
            .expect("imported card")
            .id;

        assert_eq!(resolved_ok(&dst).conversation_id, key);
        assert_eq!(resolved_ok(&key).project_id, dst_id);
        let posted = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": dst, "text": "hi", "from": "k2" }));
        assert_error_code(&posted, "409 Conflict", "imported_read_only");
        let posted = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": key, "text": "hi", "from": "k2" }));
        assert_error_code(&posted, "409 Conflict", "imported_read_only");
        let asked = post_json(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({ "addr": key, "prompt": "?", "options": "a,b", "from": "k2" }),
        );
        assert_error_code(&asked, "409 Conflict", "imported_read_only");
        let tap = post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": key, "id": card, "answer": "Go" }));
        assert_error_code(&tap, "409 Conflict", "imported_read_only");

        // The clone's first chat pins: the next resolve settles the history.
        let new_pin = uid();
        pin(&dst_id, &new_pin);
        assert_eq!(resolved_ok(&dst).conversation_id, new_pin);
        assert_eq!(overlay::read_thread(&new_pin, 0).unwrap().len(), 3, "divider + 2 imported lines");
        assert!(overlay::read_thread(&key, 0).unwrap().is_empty());
        let posted = post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": dst, "text": "live", "from": "k2" }));
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let tap = post_json("/cli/thread/answer", &HashMap::new(), serde_json::json!({ "addr": dst, "id": card, "answer": "Go" }));
        assert_error_code(&tap, "409 Conflict", "imported_read_only");
        let void = post_json("/cli/thread/void", &HashMap::new(), serde_json::json!({ "addr": dst, "id": card }));
        assert_error_code(&void, "409 Conflict", "imported_read_only");
    }
}
