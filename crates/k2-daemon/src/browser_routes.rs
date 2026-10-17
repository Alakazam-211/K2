//! 0.40.34 — `POST /cli/browser/open-url`: surface a URL in the
//! CONNECTED K2 app instead of opening a browser on the daemon's box.
//!
//! The staged `~/.k2/bin/k2-open` shim (see `k2_core::open_shim`)
//! intercepts `xdg-open <url>` / `$BROWSER` inside K2 terminal sessions
//! and POSTs the URL here. The handler validates it (http/https only,
//! ≤2048 chars) and broadcasts an APP-LEVEL
//! [`SessionEvent::OpenUrl`](crate::session_events::SessionEvent) on the
//! `/cli/sessions/events` bus — which crosses the K2 Connect tunnel, so
//! a REMOTE viewer of a headless server gets the URL too. The daemon
//! itself never shells out to `open`/`xdg-open` on this route.
//!
//! Reachable over TWO channels:
//!   - loopback TCP (`routes::dispatcher`): owner-token / connect-user
//!     session gate (`token_ok`, standard `?token=` query), POST-only.
//!   - the per-cell UDS (`cell_server`): scoped hook token — the verb is
//!     on the `session_token::is_agent_verb` allowlist so a sandboxed /
//!     scoped session's shim can use `K2_HOOK_SOCK` + `K2_HOOK_TOKEN`.
//!
//! Body shapes accepted (the shim sends the form one via curl
//! `--data-urlencode`): JSON `{"url": "..."}`, form `url=...`, or a
//! `url` query param.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::session_events::{self, SessionEvent};

/// Hard cap on an accepted URL. Browsers/servers commonly cap around
/// 2k; anything longer is refused outright (400) rather than truncated.
pub const MAX_URL_LEN: usize = 2048;

/// Validate a raw url string for the open-url surface. PURE.
///
/// Accepts ONLY absolute `http://` / `https://` URLs (case-insensitive
/// scheme) with a non-empty remainder, at most [`MAX_URL_LEN`] chars,
/// containing no whitespace or control characters. Everything else —
/// `file://`, `javascript:`, bare paths, empty — is an `Err` (→ 400):
/// this event drives a browser tab in every connected client, so the
/// scheme allowlist is deliberately closed.
pub fn validate_open_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("missing url".to_string());
    }
    if url.len() > MAX_URL_LEN {
        return Err(format!("url too long (max {MAX_URL_LEN} chars)"));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("url must not contain whitespace or control characters".to_string());
    }
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"));
    match rest {
        Some(r) if !r.is_empty() => Ok(url.to_string()),
        _ => Err("only http:// and https:// urls can be opened".to_string()),
    }
}

/// Pull the raw url out of a request: JSON body `{"url": ...}` first,
/// then a form body `url=...`, then the merged query/form `params`.
fn extract_url(params: &HashMap<String, String>, body: &[u8]) -> Option<String> {
    let first_non_ws = body.iter().find(|b| !b.is_ascii_whitespace());
    if first_non_ws == Some(&b'{') {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
            if let Some(u) = v.get("url").and_then(|u| u.as_str()) {
                return Some(u.to_string());
            }
        }
    } else if !body.is_empty() {
        let form = crate::routes::http::parse_form_body(body);
        if let Some(u) = form.get("url") {
            return Some(u.clone());
        }
    }
    params.get("url").cloned()
}

/// `POST /cli/browser/open-url` — validate + broadcast. Method gating
/// (POST-only) and auth are upstream (dispatcher arm / cell server);
/// `source` is stamped by the CALLING channel, never the request
/// (`"shim"` for both transports today).
pub fn handle_open_url(
    params: &HashMap<String, String>,
    body: &[u8],
    source: &str,
) -> CliResponse {
    let raw = extract_url(params, body).unwrap_or_default();
    let url = match validate_open_url(&raw) {
        Ok(u) => u,
        Err(e) => return CliResponse::bad_request(e),
    };
    // Best-effort broadcast (the bus convention): zero subscribers is
    // Err in tokio's broadcast — report it as subscribers=0, still 200
    // (the request was valid; there's just nobody watching right now).
    let subscribers = session_events::emit(SessionEvent::OpenUrl {
        url,
        source: source.to_string(),
    })
    .unwrap_or(0);
    CliResponse::ok_json(
        serde_json::json!({ "success": true, "subscribers": subscribers }).to_string(),
    )
}

/// GET-chain guard (the `feedback_routes`/`push_routes` pattern): a
/// stray GET on the POST-only mutation must 405 loudly, not fall to a
/// confusing 404 (`feedback_post_only_route_guards` house rule).
pub fn dispatch(path: &str, _params: &HashMap<String, String>) -> Option<CliResponse> {
    match path {
        "/cli/browser/open-url" | "/cli/browser/notes" | "/cli/browser/notes/redeliver" => {
            Some(CliResponse::method_not_allowed())
        }
        _ => None,
    }
}

// ── Design mode notes (prd-browser-design-mode-v1 D-A9, D-A10, D-A13) ──

/// Who called `/cli/browser/notes`, decided by the transport from the
/// credential (never the body). An agent's passport identity rides the
/// stamped params (`principal_bound`), so `Agent` carries nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotesCaller {
    /// The owner daemon token.
    Owner,
    /// A K2 Connect login (Member or above).
    ConnectUser(String),
    /// A scoped agent passport (TCP hook token or the cell socket).
    Agent,
}

impl NotesCaller {
    /// The `session_author` the Thread post core expects.
    fn session_author(&self) -> &str {
        match self {
            NotesCaller::ConnectUser(u) => u.as_str(),
            NotesCaller::Owner | NotesCaller::Agent => "owner",
        }
    }
}

/// The workspace a notes call targets: the body's `workspace`, or for an
/// agent with none, its own. Returns `(path, project_id, thread addr)`.
fn resolve_notes_workspace(
    workspace: &str,
    params: &HashMap<String, String>,
) -> Result<(String, String, String), CliResponse> {
    let token = if workspace.trim().is_empty() {
        match crate::caller_workspace::principal_from_params(params) {
            Some(p) if !p.workspace_uuid.trim().is_empty() => p.workspace_uuid,
            _ => return Err(CliResponse::bad_request("missing 'workspace'")),
        }
    } else {
        workspace.trim().to_string()
    };
    let Some(path) = crate::workspace_msg::resolve_workspace(&token) else {
        return Err(crate::workspace_routes::workspace_not_found_response(&token));
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(project_id) = k2_core::workspace::agent_identity::resolve_project_id(&conn, &path) else {
        return Err(crate::workspace_routes::workspace_not_found_response(&token));
    };
    let addr = k2_core::workspace_session_handles::workspace_address_name(&conn, &project_id)
        .map_err(|e| CliResponse {
            status: "500 Internal Server Error",
            content_type: "application/json",
            body: serde_json::json!({ "error": format!("workspace address: {e}") }).to_string(),
        })?;
    Ok((path, project_id, addr))
}

/// #67: a Connect login may only instruct an agent whose workspace (or the
/// app) opted in to remote instruction, the same gate as send-message.
/// A Thread post from a Connect login does not check it, so this route
/// applies it itself (vs-live D-A13: never weaker than send-message).
fn remote_instruct_gate(caller: &NotesCaller, path: &str) -> Result<(), CliResponse> {
    match caller {
        NotesCaller::ConnectUser(_) if !k2_core::workspace::settings::remote_instruct_allowed_for_path(path) => {
            Err(CliResponse {
                status: "403 Forbidden",
                content_type: "application/json",
                body: serde_json::json!({
                    "error": "remote_instruction_disabled",
                    "message": "This workspace does not take instructions from remote logins. The owner can turn on remote instructions in Workspace settings.",
                })
                .to_string(),
            })
        }
        _ => Ok(()),
    }
}

fn delivery_json(d: &crate::overlay_routes::DesignDelivery, notes: Option<usize>) -> CliResponse {
    let mut v = serde_json::json!({
        "ok": true,
        "threadItemId": d.thread_item_id,
        "seq": d.seq,
        "delivered": d.delivered,
        "targetSessionId": d.target_session_id,
        "addr": d.addr,
        "conversationId": d.conversation_id,
        "from": d.from,
    });
    if let Some(n) = notes {
        v["notes"] = serde_json::json!(n);
    }
    CliResponse::ok_json(v.to_string())
}

/// `POST /cli/browser/notes` — clamp, format and post the batched notes
/// as ONE Thread item with a `design` origin, which wakes and injects the
/// pinned Chat like the Thread send button (D-A9). Method gating and auth
/// are upstream (dispatcher arm / cell server); app passes never reach it.
/// Body: `{ workspace, page: {url, title, viewport}, notes: [{payload,
/// comment, intent}], itemId? }`. Reply: `{ ok, threadItemId, delivered,
/// targetSessionId, addr, notes }`. `delivered: false` keeps the item:
/// Retry is `/cli/browser/notes/redeliver`, never a second post.
pub fn handle_notes(params: &HashMap<String, String>, body: &[u8], caller: &NotesCaller) -> CliResponse {
    let req: k2_core::browser_notes::NotesRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let req = match k2_core::browser_notes::clamp_request(&req) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(e),
    };
    let (path, _project_id, addr) = match resolve_notes_workspace(&req.workspace, params) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = remote_instruct_gate(caller, &path) {
        return e;
    }
    // The details go to a file in the workspace; the Thread gets the
    // person's notes and a pointer (Rosson 2026-10-16: no DOM blob in the
    // Thread or the PTY). A file that cannot be written fails the Send:
    // never a post pointing at nothing.
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let file_name = k2_core::browser_notes::details_file_name(&stamp, &req);
    let details = k2_core::browser_notes::format_details(&req);
    let rel = match k2_core::browser_notes::write_details(std::path::Path::new(&path), &file_name, &details) {
        Ok(rel) => rel,
        Err(k2_core::browser_notes::DetailsError::Escape(m)) => return CliResponse::bad_request(m),
        Err(k2_core::browser_notes::DetailsError::Io(m)) => {
            return CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("could not write the design details: {m}") }).to_string(),
            }
        }
    };
    let text = k2_core::browser_notes::format_message(&req, &rel);
    let origin = k2_core::overlay::DesignOrigin {
        item_id: req.item_id.clone(),
        url: req.page.url.clone(),
        notes: req.notes.len() as u32,
    };
    match crate::overlay_routes::design_post(params, &addr, &text, origin, caller.session_author()) {
        Ok(d) => delivery_json(&d, Some(req.notes.len())),
        Err(e) => e,
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RedeliverBody {
    workspace: String,
    thread_item_id: String,
}

/// `POST /cli/browser/notes/redeliver` `{ workspace, threadItemId }` —
/// deliver a stored notes post again (Retry). Never a second Thread item.
pub fn handle_notes_redeliver(params: &HashMap<String, String>, body: &[u8], caller: &NotesCaller) -> CliResponse {
    let b: RedeliverBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let (path, _project_id, addr) = match resolve_notes_workspace(&b.workspace, params) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = remote_instruct_gate(caller, &path) {
        return e;
    }
    match crate::overlay_routes::design_redeliver(params, &addr, &b.thread_item_id, caller.session_author()) {
        Ok(d) => delivery_json(&d, None),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_params() -> HashMap<String, String> {
        HashMap::new()
    }

    // ── validation ───────────────────────────────────────────────────

    #[test]
    fn validate_accepts_http_and_https_only() {
        assert_eq!(
            validate_open_url("https://example.com/a?b=1#c").unwrap(),
            "https://example.com/a?b=1#c"
        );
        assert_eq!(
            validate_open_url("http://127.0.0.1:8080/x").unwrap(),
            "http://127.0.0.1:8080/x"
        );
        // Scheme match is case-insensitive but the URL is passed through
        // unmodified.
        assert_eq!(
            validate_open_url("HTTPS://Example.com").unwrap(),
            "HTTPS://Example.com"
        );
        // Leading/trailing whitespace is trimmed, not rejected.
        assert_eq!(
            validate_open_url("  https://example.com  ").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn validate_rejects_non_http_schemes_and_garbage() {
        for bad in [
            "",
            "   ",
            "ftp://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "chrome://settings",
            "example.com",         // no scheme
            "/usr/local/thing",    // bare path
            "http://",             // empty remainder
            "https://",            // empty remainder
            "httpsx://example.com",
        ] {
            assert!(
                validate_open_url(bad).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn validate_rejects_oversized_and_control_chars() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(validate_open_url(&long).is_err(), "over the length cap");
        // Exactly at the cap is fine.
        let at_cap = format!(
            "https://e.com/{}",
            "a".repeat(MAX_URL_LEN - "https://e.com/".len())
        );
        assert_eq!(at_cap.len(), MAX_URL_LEN);
        assert!(validate_open_url(&at_cap).is_ok(), "at the cap is allowed");
        assert!(validate_open_url("https://exam ple.com").is_err(), "inner space");
        assert!(validate_open_url("https://ex\nample.com").is_err(), "newline");
        assert!(validate_open_url("https://ex\x07ample.com").is_err(), "control char");
    }

    // ── handler ──────────────────────────────────────────────────────

    #[test]
    fn handler_accepts_form_body_the_shim_sends() {
        // curl --data-urlencode "url=<...>" → urlencoded form body.
        let resp = handle_open_url(
            &no_params(),
            b"url=https%3A%2F%2Fexample.com%2Fdocs%3Fq%3D1",
            "shim",
        );
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["success"], true);
        assert!(v["subscribers"].is_number());
    }

    #[test]
    fn handler_accepts_json_body_and_query_param() {
        let resp = handle_open_url(
            &no_params(),
            br#"{"url":"https://example.com"}"#,
            "shim",
        );
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);

        let mut params = no_params();
        params.insert("url".to_string(), "http://localhost:3000".to_string());
        let resp = handle_open_url(&params, b"", "terminal-link");
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
    }

    #[test]
    fn handler_rejects_invalid_targets_with_400() {
        for body in [
            &br#"{"url":"file:///etc/passwd"}"#[..],
            &br#"{"url":"javascript:alert(1)"}"#[..],
            &b"url=ftp%3A%2F%2Fx"[..],
            &b""[..], // no url anywhere
        ] {
            let resp = handle_open_url(&no_params(), body, "shim");
            assert_eq!(
                resp.status, "400 Bad Request",
                "body {:?} → {}",
                String::from_utf8_lossy(body),
                resp.body
            );
        }
    }

    #[test]
    fn handler_delivers_the_event_to_a_bus_subscriber() {
        // End-to-end through the broadcast bus: what a
        // /cli/sessions/events subscriber (the connected app) receives.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let probe = format!(
                "https://probe.example/{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let mut rx = crate::session_events::subscribe();
            let body = serde_json::json!({ "url": probe }).to_string();
            let resp = handle_open_url(&no_params(), body.as_bytes(), "shim");
            assert_eq!(resp.status, "200 OK", "body: {}", resp.body);

            // Drain until our probe arrives (global bus — other tests
            // may interleave events).
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    panic!("did not receive probe OpenUrl in time");
                }
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Ok(SessionEvent::OpenUrl { url, source })) if url == probe => {
                        assert_eq!(source, "shim");
                        break;
                    }
                    Ok(Ok(_)) => continue, // contamination from another test
                    Ok(Err(_)) => panic!("receiver closed"),
                    Err(_) => panic!("timed out waiting for probe"),
                }
            }
        });
    }

    #[test]
    fn get_chain_dispatch_405s_the_post_only_route() {
        // feedback_post_only_route_guards: a stray GET must 405, never
        // silently mutate or 404.
        let resp = dispatch("/cli/browser/open-url", &no_params())
            .expect("route claimed by browser_routes");
        assert_eq!(resp.status, "405 Method Not Allowed");
        assert!(dispatch("/cli/browser/other", &no_params()).is_none());
    }

    // ── Design notes (D-A9, D-A10, D-A13) ───────────────────────────

    use crate::overlay_routes::{arm_test_delivery, recorded_deliveries};

    /// A workspace row (handle = name) with a pinned Chat conversation.
    fn seed_room(tag: &str) -> (String, String, String) {
        crate::vault::ensure_test_vault();
        let handle = format!("dmnotes{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let id = uuid::Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(format!("dm-notes-{handle}")).to_string_lossy().into_owned();
        std::fs::create_dir_all(&path).expect("workspace folder");
        let conv = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![id, handle, path],
        )
        .expect("seed project");
        k2_core::db::schema::WorkspaceSession::upsert(
            &conn,
            &format!("ws-{conv}"),
            &id,
            None,
            Some(&conv),
            "claude",
            "system",
            "running",
        )
        .expect("pin");
        (handle, id, conv)
    }

    fn note_body(workspace: &str, notes: usize) -> Vec<u8> {
        serde_json::json!({
            "workspace": workspace,
            "itemId": "design-item-1",
            "page": {
                "url": "http://localhost:5173/pricing?access_token=sekrit#plans",
                "title": "Pricing",
                "viewport": {"width": 1280, "height": 800},
            },
            "notes": (0..notes).map(|i| serde_json::json!({
                "comment": format!("make button {i} the primary colour"),
                "intent": "change",
                "payload": {
                    "selector": "main > button.cta",
                    "tag": "button",
                    "classes": ["cta"],
                    "role": "button",
                    "name": "Buy",
                    "text": "Buy",
                    "html": "<button class=\"cta\" onclick=\"steal()\" data-api-key=\"sk-live-123\">Buy</button><script>evil()</script>",
                    "styles": {"color": "rgb(0, 0, 0)", "background-image": "url(x)"},
                    "rect": {"x": 10, "y": 20, "width": 160, "height": 40},
                },
            })).collect::<Vec<_>>(),
        })
        .to_string()
        .into_bytes()
    }

    fn json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", resp.body))
    }

    fn thread(conv: &str) -> Vec<k2_core::overlay::OverlayItem> {
        k2_core::overlay::read_thread(conv, 0).expect("read thread")
    }

    #[test]
    fn notes_post_is_one_item_one_delivery_no_compose_history_and_an_audit_line() {
        let _home_lock = crate::test_support::lock_home();
        let _home = crate::test_support::TempHome::new();
        let (handle, project_id, conv) = seed_room("one");
        arm_test_delivery(&handle, true);
        let resp = handle_notes(&no_params(), &note_body(&handle, 2), &NotesCaller::Owner);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = json(&resp);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["delivered"], true, "{v}");
        assert_eq!(v["notes"], 2, "{v}");
        assert_eq!(v["addr"], handle.as_str(), "{v}");
        assert_eq!(v["targetSessionId"], format!("test-session-{handle}"), "{v}");

        let items = thread(&conv);
        assert_eq!(items.len(), 1, "exactly one Thread item: {items:?}");
        let doc = &items[0].doc;
        assert_eq!(items[0].id, v["threadItemId"].as_str().expect("id"));
        assert_eq!(doc.via.as_deref(), Some("compose"), "the person's own post");
        let design = doc.design.as_ref().expect("design origin");
        assert_eq!(design.url, "http://localhost:5173/pricing");
        assert_eq!(design.notes, 2);
        assert_eq!(design.item_id.as_deref(), Some("design-item-1"));
        assert!(doc.widget.is_none());
        assert_eq!(doc.author.as_ref().map(|a| a.kind.as_str()), Some(k2_core::overlay::doc::AUTHOR_OWNER));

        let deliveries = recorded_deliveries(&handle);
        assert_eq!(deliveries.len(), 1, "exactly one wake + inject: {deliveries:?}");
        assert!(deliveries[0].starts_with(&format!("[thread:{handle}] Design notes (2) on http://localhost:5173/pricing")), "{deliveries:?}");
        // The inject is the notes and a pointer, never the page blob.
        assert!(deliveries[0].contains("1. make button 0 the primary colour\n2. make button 1 the primary colour\nDetails: `.k2/design/"), "{deliveries:?}");
        assert!(!deliveries[0].contains("selector") && !deliveries[0].contains("<button"), "{deliveries:?}");

        let hist = k2_core::workspace_compose_history::list_compose_send_history(&project_id).expect("history");
        assert!(hist.is_empty(), "design notes never enter compose history: {hist:?}");

        let audit = k2_core::auth_audit::tail(200).expect("audit tail");
        let line = audit
            .iter()
            .find(|e| e["event"] == "design.note.post" && e["outcome"].as_str().is_some_and(|o| o.contains(&handle)))
            .unwrap_or_else(|| panic!("no design.note.post audit line: {audit:?}"));
        let outcome = line["outcome"].as_str().expect("outcome");
        assert!(outcome.contains("notes=2") && outcome.contains("url=http://localhost:5173/pricing"), "{line}");
        assert!(!outcome.contains("primary colour"), "the audit line never holds the text: {line}");
    }

    /// The workspace folder a seeded handle points at.
    fn room_path(handle: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(crate::workspace_msg::resolve_workspace(handle).expect("seeded workspace"))
    }

    /// The one `.k2/design/*.md` a Send wrote, from the post's pointer.
    fn details_from_pointer(handle: &str, body: &str) -> (String, String) {
        let rel = body
            .rsplit_once("Details: `")
            .and_then(|(_, rest)| rest.strip_suffix('`'))
            .unwrap_or_else(|| panic!("no Details pointer at the end: {body}"))
            .to_string();
        assert!(rel.starts_with(".k2/design/") && rel.ends_with(".md") && !rel.contains(".."), "{rel}");
        let text = std::fs::read_to_string(room_path(handle).join(&rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        (rel, text)
    }

    #[test]
    fn the_post_is_the_notes_and_a_pointer_and_the_details_land_in_the_workspace() {
        let (handle, _id, conv) = seed_room("file");
        let resp = handle_notes(&no_params(), &note_body(&handle, 1), &NotesCaller::Owner);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let body = thread(&conv)[0].doc.body.clone().expect("body");
        assert!(body.starts_with("Design notes (1) on http://localhost:5173/pricing\n1. make button 0 the primary colour\nDetails: `.k2/design/"), "{body}");
        assert!(!body.contains("selector") && !body.contains("<button") && !body.contains("rgb("), "no blob in the Thread: {body}");
        let (rel, text) = details_from_pointer(&handle, &body);
        assert!(rel.contains("-make-button-0-the-primary-colour"), "{rel}");
        let files: Vec<_> = std::fs::read_dir(room_path(&handle).join(".k2/design")).expect("design dir").collect();
        assert_eq!(files.len(), 1, "one file per send");
        assert!(text.contains("- selector: `main > button.cta`"), "{text}");
        assert!(text.contains("- element: tag `button`, role `button`, name \"Buy\""), "{text}");
        assert!(text.contains("- box: 160×40 at (10, 20)"), "{text}");
        assert!(text.contains("- styles: color: rgb(0, 0, 0)"), "the note says colour: {text}");
        // Clamped and stripped: no query/hash, no HTML, no secrets, no
        // un-allowlisted style.
        assert!(!text.contains("sekrit") && !text.contains("access_token") && !text.contains("#plans"), "{text}");
        assert!(!text.contains("onclick") && !text.contains("sk-live-123") && !text.contains("evil()"), "{text}");
        assert!(!text.contains("<button class") && !text.contains("background-image"), "no HTML: {text}");
    }

    #[cfg(unix)]
    #[test]
    fn a_design_folder_linked_outside_the_workspace_is_refused_and_nothing_posts() {
        let (handle, _id, conv) = seed_room("escape");
        let outside = std::env::temp_dir().join(format!("dm-notes-outside-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&outside).expect("outside");
        let ws = room_path(&handle);
        std::fs::create_dir_all(ws.join(".k2")).expect(".k2");
        std::os::unix::fs::symlink(&outside, ws.join(".k2/design")).expect("link");
        let resp = handle_notes(&no_params(), &note_body(&handle, 1), &NotesCaller::Owner);
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(resp.body.contains("outside the workspace"), "{}", resp.body);
        assert_eq!(std::fs::read_dir(&outside).expect("outside").count(), 0, "nothing written outside");
        assert!(thread(&conv).is_empty(), "no post that points at nothing");
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn redeliver_after_a_failed_delivery_adds_no_second_item() {
        let (handle, _id, conv) = seed_room("retry");
        arm_test_delivery(&handle, false);
        let resp = handle_notes(&no_params(), &note_body(&handle, 1), &NotesCaller::Owner);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = json(&resp);
        assert_eq!(v["delivered"], false, "{v}");
        assert!(v["targetSessionId"].is_null(), "{v}");
        let item_id = v["threadItemId"].as_str().expect("id").to_string();
        assert_eq!(thread(&conv).len(), 1);

        arm_test_delivery(&handle, true);
        let body = serde_json::json!({"workspace": handle, "threadItemId": item_id}).to_string();
        let resp = handle_notes_redeliver(&no_params(), body.as_bytes(), &NotesCaller::Owner);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let r = json(&resp);
        assert_eq!(r["delivered"], true, "{r}");
        assert_eq!(r["threadItemId"], item_id.as_str(), "{r}");
        assert_eq!(thread(&conv).len(), 1, "retry never posts a second Thread item");
        let deliveries = recorded_deliveries(&handle);
        assert_eq!(deliveries.len(), 2, "{deliveries:?}");
        assert_eq!(deliveries[0], deliveries[1], "the retry delivers the same text");
    }

    #[test]
    fn redeliver_finds_the_item_after_a_wake_re_pinned_the_chat() {
        let (handle, project_id, conv) = seed_room("repin");
        arm_test_delivery(&handle, false);
        let resp = handle_notes(&no_params(), &note_body(&handle, 1), &NotesCaller::Owner);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let item_id = json(&resp)["threadItemId"].as_str().expect("id").to_string();
        // The wake started a fresh session: the Chat is pinned to a new
        // conversation now; the item stays in the old one.
        let new_conv = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::WorkspaceSession::upsert(
                &conn,
                &format!("ws-{new_conv}"),
                &project_id,
                None,
                Some(&new_conv),
                "claude",
                "system",
                "running",
            )
            .expect("re-pin");
        }
        arm_test_delivery(&handle, true);
        let body = serde_json::json!({"workspace": handle, "threadItemId": item_id}).to_string();
        let resp = handle_notes_redeliver(&no_params(), body.as_bytes(), &NotesCaller::Owner);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let r = json(&resp);
        assert_eq!(r["threadItemId"], item_id.as_str(), "{r}");
        assert_eq!(r["delivered"], true, "{r}");
        assert_eq!(thread(&conv).len(), 1, "the item stays where it was posted");
        assert!(thread(&new_conv).is_empty(), "no second item in the new conversation");

        // Another workspace cannot redeliver it by id.
        let (other, _oid, _oconv) = seed_room("repin2");
        let body = serde_json::json!({"workspace": other, "threadItemId": item_id}).to_string();
        let resp = handle_notes_redeliver(&no_params(), body.as_bytes(), &NotesCaller::Owner);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(recorded_deliveries(&other).is_empty());
    }

    #[test]
    fn redeliver_is_only_for_design_items_and_their_sender() {
        let (handle, _id, conv) = seed_room("redeliv");
        // A plain Thread item is not redeliverable here.
        let plain = crate::overlay_routes::dispatch_post(
            "/cli/thread/post",
            &no_params(),
            serde_json::json!({"addr": handle, "text": "hello", "via": "compose"}).to_string().as_bytes(),
        );
        assert_eq!(plain.status, "200 OK", "{}", plain.body);
        let plain_id = json(&plain)["id"].as_str().expect("id").to_string();
        let body = serde_json::json!({"workspace": handle, "threadItemId": plain_id}).to_string();
        let resp = handle_notes_redeliver(&no_params(), body.as_bytes(), &NotesCaller::Owner);
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);

        let missing = serde_json::json!({"workspace": handle, "threadItemId": "nope"}).to_string();
        let resp = handle_notes_redeliver(&no_params(), missing.as_bytes(), &NotesCaller::Owner);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);

        // The owner's notes: another login may not re-send them as theirs.
        let _home_lock = crate::test_support::lock_home();
        let _home = crate::test_support::TempHome::new();
        k2_core::workspace::settings::update_project_setting(&room_path(&handle).to_string_lossy(), "allow_remote_instruct", "1")
            .expect("opt in");
        let resp = handle_notes(&no_params(), &note_body(&handle, 1), &NotesCaller::Owner);
        let owner_item = json(&resp)["threadItemId"].as_str().expect("id").to_string();
        let body = serde_json::json!({"workspace": handle, "threadItemId": owner_item}).to_string();
        let resp = handle_notes_redeliver(&no_params(), body.as_bytes(), &NotesCaller::ConnectUser("mallory".into()));
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        assert_eq!(thread(&conv).len(), 2, "no item added by any refused call");
    }

    #[test]
    fn the_persons_mistakes_are_400_and_an_unknown_workspace_404() {
        let (handle, _id, conv) = seed_room("err");
        for (body, want) in [
            (note_body(&handle, 0), "no notes"),
            (note_body(&handle, k2_core::browser_notes::MAX_NOTES + 1), "at most"),
            (b"{not json".to_vec(), "invalid JSON"),
            (
                serde_json::json!({"workspace": handle, "page": {"url": "file:///etc/passwd"}, "notes": [{"comment": "x"}]})
                    .to_string()
                    .into_bytes(),
                "http or https",
            ),
            (
                serde_json::json!({"workspace": handle, "page": {"url": "http://a/"}, "notes": [{"comment": " "}]})
                    .to_string()
                    .into_bytes(),
                "no text",
            ),
            (
                serde_json::json!({"page": {"url": "http://a/"}, "notes": [{"comment": "x"}]}).to_string().into_bytes(),
                "missing 'workspace'",
            ),
        ] {
            let resp = handle_notes(&no_params(), &body, &NotesCaller::Owner);
            assert_eq!(resp.status, "400 Bad Request", "{want}: {}", resp.body);
            assert!(resp.body.contains(want), "want {want:?} in {}", resp.body);
        }
        let resp = handle_notes(&no_params(), &note_body("no-such-workspace-dm", 1), &NotesCaller::Owner);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(thread(&conv).is_empty(), "no refused call posts anything");
    }

    #[test]
    fn a_connect_login_needs_the_remote_instruct_opt_in() {
        let _home_lock = crate::test_support::lock_home();
        let _home = crate::test_support::TempHome::new();
        let (handle, _id, conv) = seed_room("remote");
        let alice = NotesCaller::ConnectUser("alice".into());
        let resp = handle_notes(&no_params(), &note_body(&handle, 1), &alice);
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        assert!(resp.body.contains("remote_instruction_disabled"), "{}", resp.body);
        assert!(thread(&conv).is_empty());
        assert!(recorded_deliveries(&handle).is_empty(), "a refused call never injects");

        k2_core::workspace::settings::update_project_setting(&room_path(&handle).to_string_lossy(), "allow_remote_instruct", "1")
            .expect("opt in");
        let resp = handle_notes(&no_params(), &note_body(&handle, 1), &alice);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let items = thread(&conv);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].doc.from, "alice", "from comes from the login, never the body");
        assert_eq!(items[0].doc.author.as_ref().map(|a| a.kind.as_str()), Some(k2_core::overlay::doc::AUTHOR_USER));
    }

    fn agent_params(project_id: &str, from: &str) -> HashMap<String, String> {
        let mut p = HashMap::new();
        p.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        p.insert("project_id".to_string(), project_id.to_string());
        p.insert("from".to_string(), from.to_string());
        p
    }

    #[test]
    fn an_agent_passport_posts_only_on_its_own_workspace() {
        let (handle, id, conv) = seed_room("agent");
        let (other, _other_id, other_conv) = seed_room("other");
        // Own workspace, `workspace` omitted: defaults to the passport's.
        let mut body: serde_json::Value = serde_json::from_slice(&note_body(&handle, 1)).expect("body");
        body.as_object_mut().expect("obj").remove("workspace");
        let resp = handle_notes(&agent_params(&id, &handle), body.to_string().as_bytes(), &NotesCaller::Agent);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let items = thread(&conv);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].doc.from, handle, "the agent's stamp");
        assert_eq!(items[0].doc.via.as_deref(), Some("thread"), "an agent post is not the person's compose");
        assert!(items[0].doc.author.is_none());
        assert_eq!(recorded_deliveries(&handle).len(), 1);

        // Another workspace: refused, nothing posted there.
        let resp = handle_notes(&agent_params(&id, &handle), &note_body(&other, 1), &NotesCaller::Agent);
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        assert!(thread(&other_conv).is_empty());
        assert!(recorded_deliveries(&other).is_empty());
    }

    #[test]
    fn get_chain_405s_the_notes_routes() {
        for p in ["/cli/browser/notes", "/cli/browser/notes/redeliver"] {
            let resp = dispatch(p, &no_params()).expect("claimed");
            assert_eq!(resp.status, "405 Method Not Allowed", "{p}");
        }
    }
}
