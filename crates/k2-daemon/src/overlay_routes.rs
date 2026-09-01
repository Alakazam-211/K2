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
    addr: String,
    /// `sales` / pinned Chat — not a `sales/reviewer` sidecar address.
    canonical_alias: bool,
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
                canonical_alias: true,
            })
        }
        Some(MsgTarget::Sidecar {
            path,
            conversation_key,
            ..
        }) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let Some(project_id) = resolve_project_id(&conn, &path) else {
                return Err(not_found(format!("workspace not found: {addr}")));
            };
            Ok(ResolvedOverlay {
                conversation_id: conversation_key,
                project_id,
                addr: addr.to_string(),
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
                canonical_alias: canonical_alias,
            })
        }
        None => Err(not_found(format!("unknown overlay addr '{addr}'"))),
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
    serde_json::json!({
        "ok": true,
        "collection": collection,
        "addr": resolved.addr,
        "conversation_id": resolved.conversation_id,
        "items": page.items,
        "has_more": page.has_more,
    })
    .to_string()
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

fn handle_post(params: &HashMap<String, String>, session_author: &str) -> CliResponse {
    let addr = str_param(params, "addr");
    let text = str_param(params, "text");
    if addr.is_empty() {
        return usage("missing addr");
    }
    if text.is_empty() {
        return usage("missing text");
    }
    let via = opt_param(params, "via").unwrap_or_else(|| "thread".to_string());
    if request_skin().is_some() && via == "compose" {
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
    let principal = crate::caller_workspace::principal_from_params(params);
    // Skin: authenticated pass username only (never body `from`). Overlay
    // store and PTY stamp must match. via=compose: session actor. Else
    // empty/`k2` stamps the room handle (CLI still defaults from=k2).
    let from = if let Some(pass) = request_skin() {
        skin_from_stamp(&pass)
    } else if via == "compose" {
        compose_from_for_session(session_author)
    } else {
        let explicit = opt_param(params, "from").unwrap_or_default();
        if explicit.is_empty() || explicit.eq_ignore_ascii_case("k2") {
            thread_from_room_handle(&resolved)
        } else {
            explicit
        }
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
            if via == "compose" {
                apply_human_prose(
                    &resolved.conversation_id,
                    &resolved.project_id,
                    &resolved.addr,
                    &text,
                );
                inject_thread_compose(&resolved, &from, &text, &command);
                let _ = k2_core::workspace_compose_history::record_compose_send(
                    &resolved.project_id,
                    &text,
                    &from,
                );
            } else if request_skin().is_some() {
                inject_skin_thread(&resolved, &from, &text);
            }
            CliResponse::ok_json(
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
                })
                .to_string(),
            )
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

fn stamped_from(params: &HashMap<String, String>) -> String {
    let explicit = opt_param(params, "from").unwrap_or_default();
    if !explicit.is_empty() {
        explicit
    } else {
        "k2".to_string()
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
    let from = stamped_from(params);
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
            let choice = item.doc.choice.as_ref();
            let labels: Vec<String> = choice
                .map(|c| c.options.iter().map(|o| o.label.clone()).collect())
                .unwrap_or_default();
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "prompt": choice.map(|c| c.prompt.clone()).unwrap_or_else(|| prompt.trim().to_string()),
                    "options": labels,
                    "allow_custom": choice.map(|c| c.allow_custom).unwrap_or(allow_custom),
                    "status": choice.map(|c| c.status.clone()).unwrap_or_else(|| "pending".to_string()),
                    "kind": item.doc.kind,
                    "seq": item.seq,
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                })
                .to_string(),
            )
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
    let from = stamped_from(params);
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
            let secret = item.doc.secret.as_ref();
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "name": secret.map(|s| s.name.clone()).unwrap_or_else(|| name.trim().to_string()),
                    "status": secret.map(|s| s.status.clone()).unwrap_or_else(|| "pending".to_string()),
                    "kind": item.doc.kind,
                    "seq": item.seq,
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                })
                .to_string(),
            )
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
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, "k2") {
        return e;
    }
    let answer = opt_param(params, "answer").or_else(|| opt_param(params, "option"));
    let secret = opt_param(params, "secret").or_else(|| opt_param(params, "value"));
    let secret_bytes = secret.as_deref().map(str::as_bytes);
    match overlay::answer_card(
        &resolved.conversation_id,
        &resolved.project_id,
        &card_id,
        answer.as_deref(),
        secret_bytes,
    ) {
        Ok(cb) => {
            fire_card_callbacks(&resolved.addr, vec![cb.clone()]);
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
            CliResponse::ok_json(body.to_string())
        }
        Err(e) => error_json("400 Bad Request", "usage", e),
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
    let principal = crate::caller_workspace::principal_from_params(params);
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, "k2") {
        return e;
    }
    match overlay::void_card(&resolved.conversation_id, &resolved.project_id, &card_id) {
        Ok(cb) => {
            fire_card_callbacks(&resolved.addr, vec![cb.clone()]);
            let status = cb
                .doc
                .choice
                .as_ref()
                .map(|c| c.status.clone())
                .or_else(|| cb.doc.secret.as_ref().map(|s| s.status.clone()))
                .unwrap_or_else(|| "voided".to_string());
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "id": cb.doc_id,
                    "status": status,
                    "seq": cb.seq,
                    "kind": cb.doc.kind,
                    "conversation_id": resolved.conversation_id,
                    "addr": resolved.addr,
                })
                .to_string(),
            )
        }
        Err(e) => error_json("400 Bad Request", "usage", e),
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

fn inject_thread_compose(resolved: &ResolvedOverlay, from: &str, text: &str, command: &str) {
    let payload = format!("[thread:{}] {text}", resolved.addr);
    let line = format_thread_compose_pty_line_with_command(from, &resolved.addr, text, command)
        .unwrap_or_else(|_| format_thread_compose_pty_line(from, &resolved.addr, text));
    record_test_inject(&line);
    // Same throat as k2 talk / Projects Chat / Feedback: wake a dormant
    // session, then inject+submit. `via=compose` skips Chatter (already on Thread).
    deliver_thread_to_pty(&resolved.addr, &payload, from, "compose", command);
}

/// Skin Thread post (default `via=thread`): same `[from user] [thread:addr]`
/// line as compose, delivered as `via=thread` so it is not compose-bar
/// (no slash-command, no compose-history). Skin `via=compose` stays 403.
fn inject_skin_thread(resolved: &ResolvedOverlay, from: &str, text: &str) {
    let line = format_thread_compose_pty_line(from, &resolved.addr, text);
    record_test_inject(&line);
    deliver_thread_to_pty(
        &resolved.addr,
        &format!("[thread:{}] {text}", resolved.addr),
        from,
        "thread",
        "",
    );
}

fn fire_card_callbacks(addr: &str, cbs: Vec<CardCallback>) {
    for cb in cbs {
        crate::overlay_ws::publish(OverlayFrame {
            collection: "thread".to_string(),
            seq: cb.seq,
            id: cb.doc_id.clone(),
            doc: Some(cb.doc.clone()),
            conversation_id: Some(cb.conversation_id.clone()),
        });
        let payload = format!("[thread:{addr}] {}", cb.inject_line);
        record_test_inject(&payload);
        let from = crate::workspace_msg::resolve_owner_from();
        deliver_thread_to_pty(addr, &payload, &from, "thread", "");
    }
}

/// Feedback / Projects Chat / `k2 talk`: `deliver_live(..., wake=true)`.
/// Overlay unit tests have no Tokio reactor; skip the live wake there.
fn deliver_thread_to_pty(addr: &str, payload: &str, from: &str, via: &str, command: &str) {
    if cfg!(test) && tokio::runtime::Handle::try_current().is_err() {
        return;
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
}

fn apply_human_prose(conversation_id: &str, project_id: &str, addr: &str, text: &str) {
    match overlay::apply_prose(conversation_id, project_id, text) {
        Ok(cbs) if !cbs.is_empty() => fire_card_callbacks(addr, cbs),
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

fn display_addr_for(
    conn: &rusqlite::Connection,
    project_id: &str,
    conversation_id: &str,
) -> String {
    let ws = conn
        .query_row(
            "SELECT handle FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let pinned = WorkspaceSession::get(conn, project_id)
        .ok()
        .flatten()
        .and_then(|s| s.session_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match (ws, pinned) {
        (Some(ws), Some(pin)) if pin == conversation_id => ws,
        (Some(ws), _) => ws,
        (None, _) => conversation_id.to_string(),
    }
}

pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        "/cli/thread" => handle_get_thread(params),
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
    merge_body(&mut params, body);
    match path {
        "/cli/thread/post" => handle_post(&params, session_author),
        "/cli/thread/ask" => handle_ask(&params),
        "/cli/thread/secret" => handle_secret(&params),
        "/cli/thread/answer" => handle_answer(&params),
        "/cli/thread/void" => handle_void(&params),
        _ => CliResponse::not_found(),
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
}
