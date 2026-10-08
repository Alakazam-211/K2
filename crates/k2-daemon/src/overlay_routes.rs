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
fn recorded_injects() -> Vec<String> {
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
fn with_moved_from(mut body: serde_json::Value, resolved: &ResolvedOverlay) -> String {
    if let Some(old) = resolved.moved_from() {
        body["movedFrom"] = serde_json::json!(old);
    }
    body.to_string()
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
    let canonical_alias =
        k2_core::workspace_session_handles::split_workspace_handle(addr).is_none();
    match workspace_msg::resolve_msg_target(addr) {
        Some(MsgTarget::WorkspaceCanonical { path }) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let Some(project_id) = resolve_project_id(&conn, &path) else {
                return Err(not_found(format!("workspace not found: {addr}")));
            };
            let Some(session) = WorkspaceSession::get(&conn, &project_id)
                .map_err(|e| error_json("500 Internal Server Error", "db", e))?
            else {
                return Err(not_found(format!(
                    "no pinned Chat conversation for '{addr}'"
                )));
            };
            let Some(conversation_id) = session
                .session_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
            else {
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

fn snapshot_json(collection: &str, resolved: &ResolvedOverlay, page: OverlayPage) -> String {
    let body = serde_json::json!({
        "ok": true,
        "collection": collection,
        "addr": resolved.addr,
        "conversation_id": resolved.conversation_id,
        "items": page.items,
        "has_more": page.has_more,
        // Q5: every address this chat answered at before, so a client
        // renders old rows' `from`/`to` under the current name.
        "pastAddresses": past_addresses(resolved),
    });
    with_moved_from(body, resolved)
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

fn handle_get_thread(params: &HashMap<String, String>) -> CliResponse {
    let addr = str_param(params, "addr");
    if addr.is_empty() {
        return usage("missing addr");
    }
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Err(e) = require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_READ) {
        return e;
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_read(principal.as_ref(), &resolved) {
        return e;
    }
    let since = parse_since_opt(params);
    let before = parse_before_seq(params);
    let limit = parse_overlay_limit(params);
    match overlay::read_thread_page(&resolved.conversation_id, since, before, limit) {
        Ok(page) => CliResponse::ok_json(snapshot_json("thread", &resolved, page)),
        Err(e) => error_json("500 Internal Server Error", "store", e),
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
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
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
/// addr (never `limit=0`). App passes are refused by the dispatcher.
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
            match overlay::read_thread_page(&resolved.conversation_id, None, None, 1) {
                Ok(page) => match page.items.last() {
                    Some(item) => serde_json::json!({
                        "addr": addr,
                        "conversationId": resolved.conversation_id,
                        "seq": item.seq,
                        "at": item.doc.created_at,
                        "from": item.doc.from,
                        "via": item.doc.via,
                        "kind": item.doc.kind,
                        "preview": thread_preview(&item.doc),
                    }),
                    None => serde_json::json!({
                        "addr": addr,
                        "conversationId": resolved.conversation_id,
                        "seq": null,
                        "at": null,
                        "from": null,
                        "via": null,
                        "kind": null,
                        "preview": null,
                    }),
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
    match overlay::read_chatter_page(&resolved.conversation_id, since, before, limit) {
        Ok(page) => CliResponse::ok_json(snapshot_json("chatter", &resolved, page)),
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
/// Owner token (`"owner"`) → `owner_display_name`. Connect-user → username.
fn compose_from_for_session(session_author: &str) -> String {
    let s = session_author.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("owner") {
        workspace_msg::resolve_owner_from()
    } else {
        s.to_string()
    }
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

fn handle_post(params: &HashMap<String, String>, session_author: &str) -> CliResponse {
    // A23: a Thread turn's `startedAt` is the daemon's ms receipt time.
    let received_at = chrono::Utc::now().timestamp_millis();
    let addr = str_param(params, "addr");
    let text = str_param(params, "text");
    if addr.is_empty() {
        return usage("missing addr");
    }
    if text.is_empty() {
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
    let resolved = match resolve_thread_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
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
    let db = k2_core::db::shared();
    let conn = db.lock();
    match overlay::post_thread(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        &from,
        &resolved.addr,
        &text,
        &via,
    ) {
        Ok((item, links)) => {
            crate::overlay_ws::emit_links(&links, &item.doc);
            drop(conn);
            if !bound && via == "compose" {
                apply_human_prose(
                    &resolved.conversation_id,
                    &resolved.project_id,
                    &resolved.addr,
                    &text,
                );
                let turn = ThreadTurn::new(&resolved, &item.id, received_at);
                inject_thread_compose(&resolved, &from, &text, &command, turn);
                let _ = k2_core::workspace_compose_history::record_compose_send(
                    &resolved.project_id,
                    &text,
                    &from,
                );
            } else if request_skin().is_some() {
                inject_skin_thread(&resolved, &from, &text, ThreadTurn::new(&resolved, &item.id, received_at));
            } else {
                // TW4 (a): the agent answered in Thread (not the compose
                // bar, not an app guest): the strip ends with the reply.
                crate::thread_activity::note_reply(&resolved.conversation_id);
            }
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
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                }),
                &resolved,
            ))
        }
        Err(e) => error_json("500 Internal Server Error", "store", e),
    }
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
    let resolved = match resolve_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
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
    ) {
        Ok((item, links)) => {
            crate::overlay_ws::emit_links(&links, &item.doc);
            drop(conn);
            // TW4 (a): an agent's card is its Thread reply.
            crate::thread_activity::note_reply(&resolved.conversation_id);
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
    let resolved = match resolve_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let principal = crate::caller_workspace::principal_from_params(params);
    let from = overlay_sender_from(params, &resolved);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, &from) {
        return e;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match overlay::post_secret(
        &conn,
        &resolved.conversation_id,
        &resolved.project_id,
        &from,
        &resolved.addr,
        name.trim(),
        prompt.as_deref(),
    ) {
        Ok((item, links)) => {
            crate::overlay_ws::emit_links(&links, &item.doc);
            drop(conn);
            crate::thread_activity::note_reply(&resolved.conversation_id);
            let secret = item.doc.secret.as_ref();
            CliResponse::ok_json(with_moved_from(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "from": item.doc.from,
                    "name": secret.map(|s| s.name.clone()).unwrap_or_else(|| name.trim().to_string()),
                    "status": secret.map(|s| s.status.clone()).unwrap_or_else(|| "pending".to_string()),
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

fn card_id_of(params: &HashMap<String, String>) -> String {
    let id = str_param(params, "id");
    if id.is_empty() {
        str_param(params, "card_id")
    } else {
        id
    }
}

fn handle_answer(params: &HashMap<String, String>) -> CliResponse {
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
    if let Err(e) = require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_POST) {
        return e;
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, "k2") {
        return e;
    }
    let answer = opt_param(params, "answer").or_else(|| opt_param(params, "option"));
    let secret = opt_param(params, "secret").or_else(|| opt_param(params, "value"));
    // KV8: only a person answers a secret card. Any session passport
    // (agent harness, plain shell tab, API cell) is refused, whichever
    // door it came through (TCP passport or the cell socket).
    if principal.is_some() {
        let is_secret_card = secret.is_some()
            || match overlay::card_is_secret(&resolved.conversation_id, &card_id) {
                Ok(b) => b,
                Err(e) => return card_error(e),
            };
        if is_secret_card {
            return secret_card_human_only();
        }
    }
    let secret_bytes = secret.as_deref().map(str::as_bytes);
    match overlay::answer_card(
        &resolved.conversation_id,
        &resolved.project_id,
        &card_id,
        answer.as_deref(),
        secret_bytes,
    ) {
        Ok(cb) => {
            // TW1: a person answering a card starts a Thread turn (an
            // agent's own answer does not).
            fire_card_callbacks(&resolved.addr, vec![cb.clone()], !principal_is_bound(params));
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

/// Map a card store error: a card that is no longer pending is 409
/// `card_not_pending` (its answer is final); anything else is usage.
fn card_error(e: String) -> CliResponse {
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

fn handle_void(params: &HashMap<String, String>) -> CliResponse {
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
    if let Err(e) = require_skin_cap_in_resolved(&resolved, k2_core::skin::CAP_THREAD_POST) {
        return e;
    }
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, "k2") {
        return e;
    }
    match overlay::void_card(&resolved.conversation_id, &resolved.project_id, &card_id) {
        Ok(cb) => {
            fire_card_callbacks(&resolved.addr, vec![cb.clone()], !principal_is_bound(params));
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

/// `start_turns`: a person answered or dismissed the card (TW1). Cards a
/// compose post's prose resolves ride that post's own turn.
fn fire_card_callbacks(addr: &str, cbs: Vec<CardCallback>, start_turns: bool) {
    let started_at = chrono::Utc::now().timestamp_millis();
    for cb in cbs {
        crate::overlay_ws::publish(OverlayFrame {
            collection: "thread".to_string(),
            seq: cb.seq,
            id: cb.doc_id.clone(),
            doc: Some(cb.doc.clone()),
            activity: None,
            conversation_id: Some(cb.conversation_id.clone()),
        });
        let payload = format!("[thread:{addr}] {}", cb.inject_line);
        record_test_inject(&payload);
        let from = crate::workspace_msg::resolve_owner_from();
        let turn = start_turns.then_some(ThreadTurn {
            conversation_id: &cb.conversation_id,
            turn_id: &cb.doc_id,
            started_at,
        });
        deliver_thread_to_pty(addr, &payload, &from, "thread", "", turn);
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
) {
    if cfg!(test) && tokio::runtime::Handle::try_current().is_err() {
        return;
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
}

fn apply_human_prose(conversation_id: &str, project_id: &str, addr: &str, text: &str) {
    match overlay::apply_prose(conversation_id, project_id, text) {
        Ok(cbs) if !cbs.is_empty() => fire_card_callbacks(addr, cbs, false),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[overlay] apply_prose failed: {e}"),
    }
}

/// T25 for Terminal Message-the-agent. Best-effort; never fails the inject.
pub fn on_human_pty_text(session_id: &str, text: &str) {
    let session_id = session_id.trim();
    let text = text.trim();
    if session_id.is_empty() || text.is_empty() {
        return;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some((project_id, _, _)) = overlay::catalog::get(&conn, session_id).ok().flatten() else {
        return;
    };
    let addr = display_addr_for(&conn, &project_id, session_id);
    drop(conn);
    apply_human_prose(session_id, &project_id, &addr, text);
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

pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        "/cli/thread" => handle_get_thread(params),
        "/cli/thread/latest" => handle_get_thread_latest(params),
        "/cli/thread/activity" => handle_get_thread_activity(params),
        "/cli/chatter" => handle_get_chatter(params),
        "/cli/chatterlog" => handle_get_chatterlog(params),
        "/cli/thread/post" | "/cli/thread/ask" | "/cli/thread/secret" | "/cli/thread/answer"
        | "/cli/thread/void" => CliResponse::method_not_allowed(),
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
        "/cli/thread/answer" => handle_answer(&params),
        "/cli/thread/void" => handle_void(&params),
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
                } else if val.is_array() {
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
pub fn record_inject_chatter(workspace_token: &str, from: &str, text: &str, via: &str) {
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
        let handle = format!("ovlkv8{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());
        let name = "KV8_TOKEN";
        let posted = post_json(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({ "addr": handle, "name": name, "from": "k2" }),
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
        assert_eq!(
            k2_core::overlay::vault::debug_read(&project_id, name).expect("vault"),
            b"human-typed",
            "the first human answer is final"
        );
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
    }

    #[test]
    fn compose_via_thread_post_records_workspace_history() {
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

        record_inject_chatter(&addr, &handle, "ping", "msg");

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
        record_inject_chatter(&addr, &handle, "ping-talk", "talk");
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
        assert!(
            injects
                .iter()
                .any(|l| l == &format!("[thread:{handle}] chose Go")),
            "async inject must fire after tap; got {injects:?}"
        );

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

    #[test]
    fn compose_prose_voids_pending_exact_label_marks() {
        let handle = format!("ovlprose{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _) = seed(&handle);
        pin(&project_id, &uuid::Uuid::new_v4().to_string());

        let ask = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "prompt": "?",
                "options": "Go,Stop",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask.status, "200 OK", "{}", ask.body);

        let void_post = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "text": "never mind",
                "from": "owner",
                "via": "compose",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(void_post.status, "200 OK", "{}", void_post.body);
        let snap = json_body(
            &dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("read"),
        );
        let items = snap["items"].as_array().expect("items");
        let choice = items
            .iter()
            .find(|i| i["doc"]["kind"] == "choice")
            .expect("choice card");
        assert_eq!(
            choice["doc"]["choice"]["status"], "voided",
            "human thread text must void pending: {snap}"
        );
        let injects = recorded_injects();
        assert!(
            injects
                .iter()
                .any(|l| l.contains(&format!("[thread:{handle}]"))
                    && l.contains("card voided — human replied in chat")),
            "void inject: {injects:?}"
        );

        let handle2 = format!("ovlmark{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id2, _) = seed(&handle2);
        pin(&project_id2, &uuid::Uuid::new_v4().to_string());
        let ask2 = dispatch_post(
            "/cli/thread/ask",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle2,
                "prompt": "?",
                "options": "Go,Stop",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(ask2.status, "200 OK", "{}", ask2.body);
        let mark = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle2,
                "text": "Go",
                "from": "owner",
                "via": "compose",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(mark.status, "200 OK", "{}", mark.body);
        let snap2 = json_body(
            &dispatch("/cli/thread", &params_of(&[("addr", handle2.as_str())])).expect("read2"),
        );
        let choice2 = snap2["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["doc"]["kind"] == "choice")
            .expect("choice");
        assert_eq!(
            choice2["doc"]["choice"]["status"], "answered",
            "exact Go must mark rather than void: {snap2}"
        );
        assert_eq!(choice2["doc"]["choice"]["answer"], "Go");
        let injects2 = recorded_injects();
        assert!(
            injects2
                .iter()
                .any(|l| l == &format!("[thread:{handle2}] chose Go")),
            "chose inject: {injects2:?}"
        );
    }

    #[test]
    fn secret_submit_vault_and_never_in_json() {
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
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(posted.status, "200 OK", "{}", posted.body);
        let body = json_body(&posted);
        assert_eq!(body["name"], "API_TOKEN", "{body}");
        assert_eq!(body["status"], "pending", "{body}");
        assert!(body.get("secret").is_none() || body["secret"].as_str().is_none());
        assert!(
            !posted.body.contains(secret_val),
            "create must not echo a value: {}",
            posted.body
        );
        let id = body["id"].as_str().expect("id").to_string();

        let set = dispatch_post(
            "/cli/thread/answer",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle,
                "id": id,
                "secret": secret_val,
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(set.status, "200 OK", "{}", set.body);
        let set_body = json_body(&set);
        assert_eq!(set_body["status"], "set", "{set_body}");
        assert!(
            !set.body.contains(secret_val),
            "answer JSON must not contain secret: {}",
            set.body
        );
        assert!(
            k2_core::overlay::vault::exists(&project_id, "API_TOKEN"),
            "vault must hold the bytes"
        );
        let got =
            k2_core::overlay::vault::debug_read(&project_id, "API_TOKEN").expect("vault read");
        assert_eq!(got, secret_val.as_bytes());
        let snap = json_body(
            &dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET"),
        );
        let snap_s = snap.to_string();
        assert!(
            !snap_s.contains(secret_val),
            "GET snapshot must not contain secret: {snap_s}"
        );
        assert!(
            !k2_core::overlay::store::debug_docs_contain(secret_val).expect("scan"),
            "redb docs must not contain secret bytes"
        );
        let injects = recorded_injects();
        assert!(
            injects
                .iter()
                .any(|l| l == &format!("[thread:{handle}] secret API_TOKEN set")),
            "set inject: {injects:?}"
        );
        assert!(
            injects.iter().all(|l| !l.contains(secret_val)),
            "inject must never carry secret bytes: {injects:?}"
        );

        let handle2 = format!("ovlvoid{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id2, _) = seed(&handle2);
        pin(&project_id2, &uuid::Uuid::new_v4().to_string());
        let pending = dispatch_post(
            "/cli/thread/secret",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle2,
                "name": "OTHER_TOKEN",
                "from": "k2",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(pending.status, "200 OK", "{}", pending.body);
        let pid = json_body(&pending)["id"].as_str().expect("id").to_string();
        let voided = dispatch_post(
            "/cli/thread/post",
            &HashMap::new(),
            serde_json::json!({
                "addr": handle2,
                "text": "I'll paste later",
                "from": "owner",
                "via": "compose",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(voided.status, "200 OK", "{}", voided.body);
        let snap2 = json_body(
            &dispatch("/cli/thread", &params_of(&[("addr", handle2.as_str())])).expect("read void"),
        );
        let secret_item = snap2["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["id"] == pid)
            .expect("secret card");
        assert_eq!(
            secret_item["doc"]["secret"]["status"], "voided",
            "chat instead must void secret: {snap2}"
        );
        assert!(
            !k2_core::overlay::vault::exists(&project_id2, "OTHER_TOKEN"),
            "vault empty after chat-instead void"
        );
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

    /// Test 10 (Side finding B): a card answered by typing in a sidecar's
    /// Terminal injects into the sidecar's address, never the workspace's.
    #[test]
    fn terminal_card_answer_goes_to_the_sidecar_address() {
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
        on_human_pty_text(&conv, &label);
        let injects = recorded_injects();
        assert!(
            injects.iter().any(|l| l.starts_with(&format!("[thread:{handle}/reviewer] ")) && l.contains(&label)),
            "card callback must address the sidecar: {injects:?}"
        );
        assert!(
            !injects.iter().any(|l| l.starts_with(&format!("[thread:{handle}] ")) && l.contains(&label)),
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
}
