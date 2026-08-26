//! `/cli/thread`, `/cli/chatter`, `/cli/chatterlog` + POST `/cli/thread/post`.
//!
//! GET mutations 405. Overlay is keyed by named conversation_id
//! (handles / pinned Chat), never `v2_session_map`.

use std::collections::HashMap;

use k2_core::overlay::{self, OverlayItem};
use k2_core::workspace::agent_identity::resolve_project_id;
use k2_core::db::schema::WorkspaceSession;

use crate::cli::{opt_param, str_param};
use crate::cli_response::CliResponse;
use crate::session_token::HookPrincipal;
use crate::workspace_msg::{self, MsgTarget};

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

fn resolve_addr(addr: &str) -> Result<ResolvedOverlay, CliResponse> {
    let addr = addr.trim();
    if addr.is_empty() {
        return Err(usage("missing addr"));
    }
    let canonical_alias = k2_core::workspace_session_handles::split_workspace_handle(addr).is_none();
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

fn snapshot_json(collection: &str, resolved: &ResolvedOverlay, items: Vec<OverlayItem>) -> String {
    serde_json::json!({
        "ok": true,
        "collection": collection,
        "addr": resolved.addr,
        "conversation_id": resolved.conversation_id,
        "items": items,
    })
    .to_string()
}

fn handle_get_thread(params: &HashMap<String, String>) -> CliResponse {
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
    let since = parse_since(params);
    match overlay::read_thread(&resolved.conversation_id, since) {
        Ok(items) => CliResponse::ok_json(snapshot_json("thread", &resolved, items)),
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
    let since = parse_since(params);
    match overlay::read_chatter(&resolved.conversation_id, since) {
        Ok(items) => CliResponse::ok_json(snapshot_json("chatter", &resolved, items)),
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

fn handle_post(params: &HashMap<String, String>) -> CliResponse {
    let addr = str_param(params, "addr");
    let text = str_param(params, "text");
    if addr.is_empty() {
        return usage("missing addr");
    }
    if text.is_empty() {
        return usage("missing text");
    }
    let resolved = match resolve_addr(&addr) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let principal = crate::caller_workspace::principal_from_params(params);
    let from = {
        let explicit = opt_param(params, "from").unwrap_or_default();
        if !explicit.is_empty() {
            explicit
        } else {
            "k2".to_string()
        }
    };
    if let Err(e) = authorize_write(principal.as_ref(), &resolved, &from) {
        return e;
    }
    let via = opt_param(params, "via").unwrap_or_else(|| "thread".to_string());
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

pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        "/cli/thread" => handle_get_thread(params),
        "/cli/chatter" => handle_get_chatter(params),
        "/cli/chatterlog" => handle_get_chatterlog(params),
        "/cli/thread/post" | "/cli/thread/ask" | "/cli/thread/secret"
        | "/cli/thread/answer" | "/cli/thread/void" => CliResponse::method_not_allowed(),
        _ => return None,
    };
    Some(resp)
}

pub fn dispatch_post(
    path: &str,
    params: &HashMap<String, String>,
    body: &[u8],
) -> CliResponse {
    let mut params = params.clone();
    merge_body(&mut params, body);
    match path {
        "/cli/thread/post" => handle_post(&params),
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
        serde_json::from_str(&resp.body).unwrap_or_else(|e| {
            panic!("response JSON parse failed: {e}; body={}", resp.body)
        })
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
        assert_eq!(posted["from"], "k2", "json must have from: {posted}");
        assert!(
            posted["seq"].as_i64().is_some(),
            "json must have seq: {posted}"
        );
        assert_eq!(posted["conversation_id"], conv);

        let get = dispatch(
            "/cli/thread",
            &params_of(&[("addr", handle.as_str())]),
        )
        .expect("GET thread");
        assert_eq!(get.status, "200 OK", "{}", get.body);
        let snap = json_body(&get);
        let items = snap["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1, "{snap}");
        assert_eq!(items[0]["doc"]["body"], "hi");
        assert_eq!(items[0]["doc"]["from"], "k2");
        assert_eq!(items[0]["seq"], posted["seq"]);
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

        let thread = dispatch("/cli/thread", &params_of(&[("addr", addr.as_str())]))
            .expect("GET thread");
        let t = json_body(&thread);
        let t_items = t["items"].as_array().expect("thread items");
        assert!(
            t_items.is_empty(),
            "GET thread must not contain the ping: {t}"
        );

        let chatter = dispatch("/cli/chatter", &params_of(&[("addr", addr.as_str())]))
            .expect("GET chatter");
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
        let chatter = dispatch("/cli/chatter", &params_of(&[("addr", addr.as_str())]))
            .expect("chatter");
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

        let sales = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())]))
            .expect("sales read");
        let s = json_body(&sales);
        let s_items = s["items"].as_array().expect("items");
        assert_eq!(s_items.len(), 1, "sales is B's overlay: {s}");
        assert_eq!(s_items[0]["doc"]["body"], "from-B");

        let durable = format!("{handle}/alice");
        let a_read = dispatch("/cli/thread", &params_of(&[("addr", durable.as_str())]))
            .expect("A durable");
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
        assert!(
            k2_core::overlay::catalog::get(&c, "tab-pane-not-overlay")
                .expect("missing")
                .is_none()
        );
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
}
