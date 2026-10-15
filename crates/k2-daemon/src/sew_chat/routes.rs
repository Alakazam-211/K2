//! `/cli/sew/chat/*` — the agent routes of `k2 chat` (S30 K17), and the
//! shared request plumbing for `/cli/sew/approvals*` (K18,
//! [`super::routes_approvals`]).
//!
//! **Who is calling.** The house classifier (`compute_routes::Who`, built
//! by [`who_from_tcp`] / [`who_from_cell`]). The chat routes take a
//! passport only: a harness session (`Agent`) or a K2 shell tab (`Shell`),
//! both agent-level — a shell tab is never the owner here. The owner token
//! and Connect logins get 403 `agent_only` (people use the Sew app); app
//! passes / API keys 403 `wrong_credential`; nothing valid 401.
//! Identity never comes from the body: bodies deny unknown fields, so
//! `from`, `project`, `workspace` or `agent` are a 400.
//!
//! **Order of checks** (every route): caller → body/params → Sew on
//! (503 `chat_unavailable`) → any access at all (403 `not_granted`, about
//! the caller itself, not masked) → Sew resolves the target AS the agent
//! (a hidden target is Sew's masked 404, whatever the level) → the
//! Space's access row → level (403 `level_denied`, teaching) → approval
//! mode. "No row, no send": every write gets a `sew_chat_actions` row
//! before any Sew write; an insert failure is a 500 with nothing sent.
//!
//! Transports: the TCP dispatcher and the per-cell socket both refuse a GET
//! to a POST-only row with 405 before [`handle`] runs; [`handle`] checks
//! again. Handlers are blocking (DB + the blocking [`SewChat`] seam).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::sew_chat::{self as core, store, Action, ApprovalMode, Level};

use super::sew::{self, ActAs, Addr, Channel, Message, NewMessage, SewChat, SewError};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

/// Largest request body (a 16 KiB message plus the envelope).
pub const MAX_BODY: usize = 64 * 1024;

/// Every Sew chat route (agent chat + approvals).
pub const ROUTES: &[&str] = &[
    "/cli/sew/approvals",
    "/cli/sew/approvals/approve",
    "/cli/sew/approvals/approve-batch",
    "/cli/sew/approvals/deny",
    "/cli/sew/approvals/policy",
    "/cli/sew/approvals/policy/set",
    "/cli/sew/chat/access",
    "/cli/sew/chat/channels",
    "/cli/sew/chat/delete",
    "/cli/sew/chat/dm",
    "/cli/sew/chat/edit",
    "/cli/sew/chat/messages",
    "/cli/sew/chat/outbox",
    "/cli/sew/chat/outbox/cancel",
    "/cli/sew/chat/read",
    "/cli/sew/chat/reply",
    "/cli/sew/chat/send",
    "/cli/sew/chat/spaces",
    "/cli/sew/chat/wait",
    "/cli/sew/chat/whoami",
];

/// POST-only rows (405 on GET).
pub const POST_ROUTES: &[&str] = &[
    "/cli/sew/approvals/approve",
    "/cli/sew/approvals/approve-batch",
    "/cli/sew/approvals/deny",
    "/cli/sew/approvals/policy/set",
    "/cli/sew/chat/delete",
    "/cli/sew/chat/dm",
    "/cli/sew/chat/edit",
    "/cli/sew/chat/outbox/cancel",
    "/cli/sew/chat/reply",
    "/cli/sew/chat/send",
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// Classify a TCP caller (the dispatcher's effective auth query).
pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Who {
    crate::compute_routes::who_from_tcp(query, bearer, owner_token)
}

/// Classify a per-cell socket caller (the bound session's passport).
pub fn who_from_cell(session_id: &str, principal: &crate::session_token::HookPrincipal) -> Who {
    crate::compute_routes::who_from_cell(session_id, principal)
}

// ── errors ───────────────────────────────────────────────────────────

/// A refusal: `{"ok":false,"error":{"code","hint",...details}}`.
#[derive(Debug, Clone)]
pub struct ChatErr {
    pub code: String,
    pub status: u16,
    pub hint: String,
    pub details: serde_json::Map<String, Value>,
}

impl ChatErr {
    pub fn new(code: &str, status: u16, hint: impl Into<String>) -> Self {
        Self { code: code.into(), status, hint: hint.into(), details: Default::default() }
    }

    pub fn with(mut self, k: &str, v: impl Into<Value>) -> Self {
        self.details.insert(k.into(), v.into());
        self
    }

    pub fn response(&self) -> CliResponse {
        let mut e = json!({ "code": self.code, "hint": self.hint });
        if !self.details.is_empty() {
            e["details"] = Value::Object(self.details.clone());
        }
        CliResponse {
            status: status_line(self.status),
            content_type: "application/json",
            body: json!({ "ok": false, "error": e }).to_string(),
        }
    }
}

pub type R<T> = Result<T, ChatErr>;

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

pub(crate) fn internal(e: impl std::fmt::Display) -> ChatErr {
    ChatErr::new("internal", 500, format!("chat store error: {e}"))
}

pub(crate) fn usage(hint: impl Into<String>) -> ChatErr {
    ChatErr::new("usage", 400, hint)
}

/// 413 for a body over [`MAX_BODY`].
pub fn too_large() -> CliResponse {
    ChatErr::new("too_large", 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)).response()
}

/// 405 for a GET on a POST-only path.
pub fn method_not_allowed() -> CliResponse {
    ChatErr::new("method_not_allowed", 405, "POST required").response()
}

/// Sew's answer → K2's wire (Sew S30 §11.2). A 404 passes through with
/// Sew's hint unchanged (masked); nothing becomes more revealing.
pub(crate) fn sew_err(e: SewError) -> ChatErr {
    match (e.status, e.code.as_str()) {
        (404, _) => ChatErr::new("not_found", 404, e.hint),
        (403, "missing_permission" | "policy_denied") => {
            let mut c = ChatErr::new(
                &e.code,
                403,
                format!("{} — Sew roles decide where you can post; ask a human in the chat app", e.hint),
            );
            if let Some(r) = e.reason {
                c = c.with("sewReason", r);
            }
            c
        }
        (403, _) => ChatErr::new("chat_not_ready", 503, "your chat account is not active yet (Sew hasn't provisioned it) — retry later")
            .with("sewCode", e.code),
        (422, _) => {
            let mut c = ChatErr::new("invalid_state", 422, e.hint);
            if let Some(r) = e.reason {
                c = c.with("reason", r);
            }
            c
        }
        (400, _) => ChatErr::new("usage", 400, e.hint),
        (413, _) => ChatErr::new("too_large", 413, e.hint),
        (409, _) => ChatErr::new(&e.code, 409, e.hint),
        (429, _) => {
            let mut c = ChatErr::new("rate_limited", 429, "Sew is rate-limiting you; wait and retry");
            if let Some(ms) = e.retry_after_ms {
                c = c.with("retryAfterMs", ms);
            }
            c
        }
        _ => ChatErr::new("chat_unavailable", 503, "Sew didn't answer; try again shortly").with("sewStatus", e.status),
    }
}

// ── seams: clock, hooks, Sew ─────────────────────────────────────────

/// Seconds since the epoch (tests shift it to cross the 24 h expiry).
pub(crate) fn now() -> i64 {
    #[cfg(test)]
    {
        core::now_secs() + test_clock::offset()
    }
    #[cfg(not(test))]
    {
        core::now_secs()
    }
}

#[cfg(test)]
pub(crate) mod test_clock {
    use std::cell::Cell;
    thread_local! { static OFF: Cell<i64> = const { Cell::new(0) }; }
    pub(crate) fn set(secs: i64) {
        OFF.with(|c| c.set(secs));
    }
    pub(super) fn offset() -> i64 {
        OFF.with(Cell::get)
    }
}

/// Emit a hook event (and record it under test).
pub(crate) fn emit_hook(event: k2_core::agent_hooks::HookEvent, payload: Value) {
    #[cfg(test)]
    test_hooks::record(event.event_name(), &payload);
    k2_core::agent_hooks::emit(event, payload);
}

#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::RefCell;
    thread_local! { static SEEN: RefCell<Vec<(String, serde_json::Value)>> = const { RefCell::new(Vec::new()) }; }
    pub(super) fn record(name: &str, payload: &serde_json::Value) {
        SEEN.with(|s| s.borrow_mut().push((name.to_string(), payload.clone())));
    }
    pub(crate) fn take() -> Vec<(String, serde_json::Value)> {
        SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }
}

/// Lazy expiry (24 h → denied by `system:expired`), with the decided hook
/// for each expired item. Runs before every queue read or decision.
pub(crate) fn expire_now(conn: &rusqlite::Connection) -> R<()> {
    for r in store::expire_due(conn, now()).map_err(internal)? {
        emit_hook(
            k2_core::agent_hooks::HookEvent::SewChatApprovalDecided,
            json!({"outboxId": r.id, "status": r.status, "decidedKind": r.decided_kind}),
        );
    }
    Ok(())
}

pub(crate) fn backend() -> R<std::sync::Arc<dyn SewChat>> {
    sew::backend().ok_or_else(|| ChatErr::new("chat_unavailable", 503, "Sew chat is not enabled on this box"))
}

// ── caller ───────────────────────────────────────────────────────────

/// The agent workspace a chat request acts for.
#[derive(Debug, Clone)]
pub struct Caller {
    pub project_id: String,
    pub session_id: String,
    pub handle: String,
}

impl Caller {
    pub fn act_as(&self, on_behalf_of: Option<String>) -> ActAs {
        ActAs {
            workspace_uuid: self.project_id.to_ascii_lowercase(),
            k2_session: self.session_id.clone(),
            on_behalf_of,
            actor: None,
        }
    }
}

pub(crate) fn credential(who: &Who) -> R<()> {
    match who {
        Who::Invalid => Err(ChatErr::new("invalid_token", 401, "invalid or missing token")),
        Who::Wrong => Err(ChatErr::new("wrong_credential", 403, "app passes, API keys and stream tokens can't use Sew chat")),
        _ => Ok(()),
    }
}

/// A chat caller: an agent or K2 shell-tab passport (agent-level).
pub(crate) fn caller(who: &Who) -> R<Caller> {
    credential(who)?;
    let Some((project_id, session_id)) = who.passport() else {
        return Err(ChatErr::new(
            "agent_only",
            403,
            "k2 chat is for agents (it runs as the agent's Sew member). People use the Sew app; the box's Sew \
             server is `k2 hostchat`",
        ));
    };
    let ws = crate::compute_routes::ws_by_id(project_id)
        .ok_or_else(|| ChatErr::new("forbidden", 403, "your workspace is not registered"))?;
    Ok(Caller { project_id: ws.id, session_id: session_id.to_string(), handle: ws.handle })
}

// ── entry ────────────────────────────────────────────────────────────

/// Run one Sew chat route. The transport already refused a GET to a
/// POST-only path and an oversized body.
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>, is_post: bool) -> CliResponse {
    if is_post_route(path) && !is_post {
        return method_not_allowed();
    }
    let r = if path.starts_with("/cli/sew/approvals") {
        super::routes_approvals::handle(path, &who, body, params)
    } else {
        agent(path, &who, body, params)
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => e.response(),
    }
}

fn agent(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    let c = caller(who)?;
    match path {
        "/cli/sew/chat/access" => access(&c),
        "/cli/sew/chat/whoami" => whoami(&c),
        "/cli/sew/chat/spaces" => spaces(&c),
        "/cli/sew/chat/channels" => channels(&c, params),
        "/cli/sew/chat/messages" => messages(&c, params),
        "/cli/sew/chat/read" => read(&c, params),
        "/cli/sew/chat/wait" => wait(&c, params),
        "/cli/sew/chat/outbox" => outbox(&c, params),
        "/cli/sew/chat/outbox/cancel" => outbox_cancel(&c, body),
        "/cli/sew/chat/send" => write_route(&c, Action::Send, body),
        "/cli/sew/chat/reply" => write_route(&c, Action::Reply, body),
        "/cli/sew/chat/dm" => write_route(&c, Action::Dm, body),
        "/cli/sew/chat/edit" => write_route(&c, Action::Edit, body),
        "/cli/sew/chat/delete" => write_route(&c, Action::Delete, body),
        _ => Err(ChatErr::new("not_found", 404, "no such chat route")),
    }
}

pub(crate) fn parse_body<T: for<'de> Deserialize<'de>>(body: &[u8]) -> R<T> {
    let b = if body.iter().all(u8::is_ascii_whitespace) { b"{}".as_slice() } else { body };
    serde_json::from_slice(b).map_err(|e| usage(format!("bad request body: {e}")))
}

fn param<'a>(p: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    p.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

// ── access lookups ───────────────────────────────────────────────────

fn all_access(c: &Caller) -> R<Vec<store::Access>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::list_access(&conn, &c.project_id).map_err(internal)
}

fn need_any_access(c: &Caller) -> R<Vec<store::Access>> {
    let all = all_access(c)?;
    if all.is_empty() {
        return Err(ChatErr::new(
            "not_granted",
            403,
            "this workspace has no chat access — ask your human to grant it (k2 chat access grant <space> <workspace>)",
        ));
    }
    Ok(all)
}

/// The Space that governs a channel: its own, or the default Space for a
/// DM outside every Space.
fn governing_space(sew: &dyn SewChat, a: &ActAs, ch: &Channel) -> R<Option<String>> {
    match &ch.space_id {
        Some(s) => Ok(Some(s.clone())),
        None => Ok(sew.default_space(a).map_err(sew_err)?.map(|s| s.id)),
    }
}

/// The caller's access in the channel's Space (403 `not_granted` without).
fn access_for(c: &Caller, sew: &dyn SewChat, a: &ActAs, ch: &Channel) -> R<store::Access> {
    let space = governing_space(sew, a, ch)?;
    let row = match &space {
        Some(s) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::get_access(&conn, &c.project_id, s).map_err(internal)?
        }
        None => None,
    };
    row.ok_or_else(|| {
        ChatErr::new(
            "not_granted",
            403,
            format!(
                "you have no chat access in {} — ask your human (k2 chat access grant)",
                ch.space_slug.as_deref().unwrap_or("this Space")
            ),
        )
    })
}

// ── reads ────────────────────────────────────────────────────────────

fn access(c: &Caller) -> R<Value> {
    let rows = all_access(c)?;
    Ok(json!({
        "ok": true,
        "workspace": c.handle,
        "chatManage": k2_core::workspace::settings::agents_can_manage_chat(&c.project_id),
        "access": rows.iter().map(|a| json!({
            "space": a.space_slug, "spaceId": a.space_id, "level": a.level.as_str(), "approval": a.approval.as_str(),
        })).collect::<Vec<_>>(),
    }))
}

fn whoami(c: &Caller) -> R<Value> {
    let sew = backend()?;
    let rows = need_any_access(c)?;
    let me = sew.me(&c.act_as(None)).map_err(sew_err)?;
    let queued = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::queued_count(&conn, &c.project_id).map_err(internal)?
    };
    Ok(json!({
        "ok": true,
        "workspace": c.handle,
        "memberId": me.member_id,
        "username": me.username,
        "chatManage": k2_core::workspace::settings::agents_can_manage_chat(&c.project_id),
        "spaces": rows.iter().map(|a| json!({"space": a.space_slug, "level": a.level.as_str(), "approval": a.approval.as_str()})).collect::<Vec<_>>(),
        "queued": queued,
    }))
}

fn spaces(c: &Caller) -> R<Value> {
    let sew = backend()?;
    let rows = need_any_access(c)?;
    let seen = sew.spaces(&c.act_as(None)).map_err(sew_err)?;
    let out: Vec<Value> = seen
        .iter()
        .filter_map(|s| {
            rows.iter().find(|a| a.space_id == s.id).map(|a| {
                json!({"id": s.id, "space": s.slug, "name": s.name, "level": a.level.as_str(), "approval": a.approval.as_str()})
            })
        })
        .collect();
    Ok(json!({ "ok": true, "spaces": out }))
}

fn channel_json(ch: &Channel) -> Value {
    json!({
        "addr": sew::channel_addr(ch), "id": ch.id, "name": ch.name, "kind": ch.kind.as_str(),
        "private": ch.private, "archived": ch.archived, "canSend": ch.can_send,
    })
}

fn channels(c: &Caller, params: &HashMap<String, String>) -> R<Value> {
    let sew = backend()?;
    let rows = need_any_access(c)?;
    let a = c.act_as(None);
    let wanted: Vec<&store::Access> = match param(params, "space") {
        Some(slug) => {
            let r = rows.iter().find(|r| r.space_slug.eq_ignore_ascii_case(slug)).ok_or_else(|| {
                ChatErr::new("not_granted", 403, format!("you have no chat access in {slug} (k2 chat spaces lists yours)"))
            })?;
            vec![r]
        }
        None => rows.iter().collect(),
    };
    let mut out = Vec::new();
    for r in wanted {
        for ch in sew.channels(&a, Some(&r.space_id)).map_err(sew_err)? {
            out.push(channel_json(&ch));
        }
    }
    Ok(json!({ "ok": true, "channels": out }))
}

/// One message on the wire. Other people's bodies are untrusted content;
/// the CLI wraps them in UNTRUSTED markers (like mail).
pub(crate) fn message_json(m: &Message, ch: &Channel, me: &str) -> Value {
    let mut v = json!({
        "id": sew::wire_msg_id(&m.id),
        "addr": sew::channel_addr(ch),
        "author": {"id": m.author_id, "username": m.author_username, "kind": if m.author_is_agent { "agent" } else { "person" }},
        "body": if m.deleted { Value::Null } else { Value::String(m.body.clone()) },
        "createdAt": m.created_at,
        "edited": m.edited_at.is_some(),
        "deleted": m.deleted,
        "replyTo": m.reply_to.as_deref().map(sew::wire_msg_id),
        "replyCount": m.reply_count,
        "following": m.following,
        "mentionsMe": m.mentions_me,
        "mine": m.author_id == me,
    });
    if let Some(o) = &m.on_behalf_of {
        v["onBehalfOf"] = o.clone().into();
    }
    // KM1-ready: only echoed when Sew sent them (never required).
    if let Some(k) = &m.message_kind {
        v["kind"] = k.clone().into();
    }
    if let Some(w) = m.wake {
        v["wake"] = w.into();
    }
    if let Some(r) = &m.response {
        v["response"] = r.clone();
    }
    v
}

fn resolve_addr(sew: &dyn SewChat, a: &ActAs, raw: &str) -> R<Channel> {
    let addr = Addr::parse(raw).map_err(usage)?;
    sew.resolve(a, &addr.space, &addr.channel).map_err(sew_err)
}

fn messages(c: &Caller, params: &HashMap<String, String>) -> R<Value> {
    let sew = backend()?;
    need_any_access(c)?;
    let raw = param(params, "in").or_else(|| param(params, "addr")).ok_or_else(|| usage("name a channel: in=<space>/<channel>"))?;
    let limit: u32 = match param(params, "limit") {
        Some(l) => l.parse().ok().filter(|n| (1..=100).contains(n)).ok_or_else(|| usage("limit is 1–100"))?,
        None => 20,
    };
    let before = match param(params, "before") {
        Some(b) => Some(sew::sew_msg_id(b).ok_or_else(|| usage("before takes a msg_… id"))?),
        None => None,
    };
    let a = c.act_as(None);
    let ch = resolve_addr(sew.as_ref(), &a, raw)?;
    access_for(c, sew.as_ref(), &a, &ch)?; // read is the floor: any row reads
    let me = sew.me(&a).map_err(sew_err)?;
    let page = sew.messages(&a, &ch.id, limit, before.as_deref()).map_err(sew_err)?;
    let next = page.messages.first().filter(|_| page.has_more).map(|m| sew::wire_msg_id(&m.id));
    Ok(json!({
        "ok": true,
        "addr": sew::channel_addr(&ch),
        "messages": page.messages.iter().map(|m| message_json(m, &ch, &me.member_id)).collect::<Vec<_>>(),
        "hasMore": page.has_more,
        "nextBefore": next,
    }))
}

fn read(c: &Caller, params: &HashMap<String, String>) -> R<Value> {
    let sew = backend()?;
    need_any_access(c)?;
    let id = param(params, "id").and_then(sew::sew_msg_id).ok_or_else(|| usage("name a message: id=msg_…"))?;
    let a = c.act_as(None);
    let m = sew.message(&a, &id).map_err(sew_err)?;
    let ch = sew.channel(&a, &m.channel_id).map_err(sew_err)?;
    access_for(c, sew.as_ref(), &a, &ch)?;
    let me = sew.me(&a).map_err(sew_err)?;
    Ok(json!({ "ok": true, "message": message_json(&m, &ch, &me.member_id) }))
}

// ── wait ─────────────────────────────────────────────────────────────

static WAITS: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

/// One of the workspace's [`core::WAITS_PER_WORKSPACE`] wait slots.
struct WaitSlot(String);

impl WaitSlot {
    fn take(project_id: &str) -> R<Self> {
        let mut g = WAITS.lock().expect("wait slots");
        let m = g.get_or_insert_with(HashMap::new);
        let n = m.entry(project_id.to_string()).or_insert(0);
        if *n >= core::WAITS_PER_WORKSPACE {
            return Err(ChatErr::new(
                "rate_limited",
                429,
                format!("at most {} waits at once per workspace", core::WAITS_PER_WORKSPACE),
            ));
        }
        *n += 1;
        Ok(WaitSlot(project_id.to_string()))
    }
}

impl Drop for WaitSlot {
    fn drop(&mut self) {
        if let Some(m) = WAITS.lock().expect("wait slots").as_mut() {
            if let Some(n) = m.get_mut(&self.0) {
                *n = n.saturating_sub(1);
            }
        }
    }
}

/// `wait [--in <space>/<channel>] [--mention] [--outbox [out_…]]`: returns
/// on the FIRST match; `timedOut: true` (CLI exit 2) at the deadline.
fn wait(c: &Caller, params: &HashMap<String, String>) -> R<Value> {
    let timeout = core::parse_timeout(param(params, "timeout")).map_err(usage)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
    if let Some(o) = params.get("outbox") {
        let _slot = WaitSlot::take(&c.project_id)?;
        return wait_outbox(c, Some(o.trim()).filter(|s| !s.is_empty() && *s != "any"), deadline);
    }
    let mention = matches!(param(params, "mention"), Some("1" | "true" | "yes"));
    let Some(raw) = param(params, "in") else {
        return Err(ChatErr::new(
            "needs_delivery_hub",
            400,
            "wait without --in needs the chat delivery hub (coming with k2 chat follow); use --in <space>/<channel> or --outbox",
        ));
    };
    let sew = backend()?;
    need_any_access(c)?;
    let a = c.act_as(None);
    let ch = resolve_addr(sew.as_ref(), &a, raw)?;
    access_for(c, sew.as_ref(), &a, &ch)?;
    let me = sew.me(&a).map_err(sew_err)?;
    let _slot = WaitSlot::take(&c.project_id)?;
    // Cursor: the newest message now (so only NEW messages return).
    let mut cursor = sew.messages(&a, &ch.id, 1, None).map_err(sew_err)?.messages.last().map(|m| m.id.clone());
    if let Some(after) = param(params, "after") {
        cursor = Some(sew::sew_msg_id(after).ok_or_else(|| usage("after takes a msg_… id"))?);
    }
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Ok(json!({ "ok": true, "timedOut": true, "nextAfter": cursor.as_deref().map(sew::wire_msg_id) }));
        }
        let wait_ms = (left.as_millis() as u64).min(core::SEW_WAIT_MS_MAX);
        let got = sew.wait(&a, &ch.id, cursor.as_deref(), wait_ms).map_err(sew_err)?;
        if let Some(last) = got.last() {
            cursor = Some(last.id.clone());
        }
        let hits: Vec<&Message> = got.iter().filter(|m| m.author_id != me.member_id && (!mention || m.mentions_me)).collect();
        if !hits.is_empty() {
            return Ok(json!({
                "ok": true,
                "timedOut": false,
                "messages": hits.iter().map(|m| message_json(m, &ch, &me.member_id)).collect::<Vec<_>>(),
                "nextAfter": cursor.as_deref().map(sew::wire_msg_id),
            }));
        }
        if got.is_empty() {
            // A Sew long-poll that returned early with nothing: don't spin.
            std::thread::sleep(std::time::Duration::from_millis(wait_ms.min(250)));
        }
    }
}

fn wait_outbox(c: &Caller, id: Option<&str>, deadline: std::time::Instant) -> R<Value> {
    // Which item(s): one, or any of the caller's queued ones right now.
    let watched: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        expire_now(&conn)?;
        match id {
            Some(id) => {
                let row = store::outbox(&conn, id).map_err(internal)?.filter(|r| r.project_id == c.project_id);
                vec![row.ok_or_else(|| ChatErr::new("not_found", 404, format!("no queued post '{id}'")))?.id]
            }
            None => store::outbox_for(&conn, &c.project_id, 500)
                .map_err(internal)?
                .into_iter()
                .filter(|r| r.status == "queued")
                .map(|r| r.id)
                .collect(),
        }
    };
    if watched.is_empty() {
        return Err(ChatErr::new("not_found", 404, "you have no queued posts to wait for"));
    }
    loop {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            expire_now(&conn)?;
            for id in &watched {
                if let Some(r) = store::outbox(&conn, id).map_err(internal)? {
                    if r.status != "queued" {
                        return Ok(json!({ "ok": true, "timedOut": false, "item": outbox_json(&r) }));
                    }
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return Ok(json!({ "ok": true, "timedOut": true, "watching": watched }));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

// ── outbox ───────────────────────────────────────────────────────────

/// An outbox item as its agent sees it (no body; the body is cleared on
/// every decision anyway).
pub(crate) fn outbox_json(r: &store::OutboxRow) -> Value {
    json!({
        "id": r.id,
        "action": r.action,
        "addr": r.addr,
        "status": r.status,
        "onBehalfOf": r.on_behalf_of,
        "bodyBytes": r.body_bytes,
        "createdAt": r.created_at,
        "expiresAt": r.expires_at,
        "decidedAt": r.decided_at,
        "decidedBy": r.decided_by,
        "decidedKind": r.decided_kind,
        "confirmMethod": r.confirm_method,
        "note": r.note,
        "delivery": r.delivery,
        "messageId": r.message_id.as_deref().map(sew::wire_msg_id),
        "sewCode": r.sew_code,
    })
}

fn outbox(c: &Caller, params: &HashMap<String, String>) -> R<Value> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    expire_now(&conn)?;
    if let Some(id) = param(params, "id") {
        // Another workspace's id is the same 404 as an unknown one.
        let r = store::outbox(&conn, id)
            .map_err(internal)?
            .filter(|r| r.project_id == c.project_id)
            .ok_or_else(|| ChatErr::new("not_found", 404, format!("no queued post '{id}'")))?;
        return Ok(json!({ "ok": true, "items": [outbox_json(&r)] }));
    }
    let limit: i64 = param(params, "limit").and_then(|l| l.parse().ok()).filter(|n| (1..=200).contains(n)).unwrap_or(50);
    let rows = store::outbox_for(&conn, &c.project_id, limit).map_err(internal)?;
    Ok(json!({ "ok": true, "items": rows.iter().map(outbox_json).collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct IdBody {
    id: String,
}

fn outbox_cancel(c: &Caller, body: &[u8]) -> R<Value> {
    let b: IdBody = parse_body(body)?;
    let id = b.id.trim();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        expire_now(&conn)?;
        let r = store::outbox(&conn, id)
            .map_err(internal)?
            .filter(|r| r.project_id == c.project_id)
            .ok_or_else(|| ChatErr::new("not_found", 404, format!("no queued post '{id}'")))?;
        let by = format!("agent:{}", c.handle);
        let d = store::Decision {
            status: store::OutboxStatus::Cancelled,
            by: &by,
            kind: store::DecidedKind::System,
            method: None,
            note: Some("cancelled by the agent"),
        };
        store::decide(&conn, &r.id, &d, now()).map_err(|e| match e {
            store::DecideErr::NotQueued(s) => ChatErr::new("not_queued", 409, format!("{id} is already {s}; only a queued post can be cancelled")),
            store::DecideErr::NotFound => ChatErr::new("not_found", 404, format!("no queued post '{id}'")),
            store::DecideErr::Db(e) => internal(e),
        })?
    };
    emit_hook(
        k2_core::agent_hooks::HookEvent::SewChatApprovalDecided,
        json!({"outboxId": row.id, "status": row.status, "decidedKind": row.decided_kind}),
    );
    Ok(json!({ "ok": true, "id": row.id, "status": row.status }))
}

// ── writes ───────────────────────────────────────────────────────────

/// One write body. Identity fields (`from`, `project`, `workspace`,
/// `agent`) are unknown fields → 400.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WriteBody {
    /// send: `<space>/<channel>`; reply: `msg_…`; dm: a K2 username.
    #[serde(default)]
    to: Option<String>,
    /// edit/delete: the `msg_…`.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    /// `--as <user>`.
    #[serde(default, rename = "as")]
    on_behalf_of: Option<String>,
    #[serde(default)]
    broadcast: bool,
    /// KM1, optional (`text` / true / absent are the only values until
    /// Sew KM1 is on the pinned tag).
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    wake: Option<bool>,
    #[serde(default)]
    respond_to: Option<String>,
}

/// The resolved target of a write.
struct Target {
    channel: Channel,
    /// reply: the thread root; edit/delete: the message.
    target_id: Option<String>,
    /// edit/delete: the message being changed.
    message: Option<Message>,
    root: Option<Message>,
}

fn write_route(c: &Caller, action: Action, body: &[u8]) -> R<Value> {
    let b: WriteBody = parse_body(body)?;
    // 1. Parse and validate (before any Sew call or row).
    let to = match action {
        Action::Send | Action::Reply | Action::Dm => {
            b.to.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| usage("name where to post: to"))?.to_string()
        }
        Action::Edit | Action::Delete => {
            b.id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| usage("name the message: id=msg_…"))?.to_string()
        }
    };
    let text = if action.is_content() {
        let t = b.text.clone().ok_or_else(|| usage("text is required"))?;
        core::check_body(&t).map_err(|e| match e {
            core::BodyError::Empty => usage("text is empty"),
            core::BodyError::TooLarge => ChatErr::new("too_large", 413, format!("a message is at most {} bytes", core::BODY_MAX)),
        })?;
        Some(t)
    } else {
        if b.text.is_some() {
            return Err(usage("delete takes no text"));
        }
        None
    };
    if b.broadcast && action != Action::Reply {
        return Err(usage("--broadcast is for reply"));
    }
    if b.kind.as_deref().is_some_and(|k| k != "text") || b.wake == Some(false) || b.respond_to.is_some() {
        return Err(ChatErr::new("needs_km1", 422, "--kind, --no-wake and --respond-to need Sew KM1, which this box doesn't have yet"));
    }
    let key = match b.key.as_deref() {
        Some(k) => {
            core::check_key(k).map_err(usage)?;
            k.to_string()
        }
        None => core::generated_key(),
    };
    let on_behalf = b.on_behalf_of.as_deref().map(|u| u.trim().trim_start_matches('@').to_string()).filter(|u| !u.is_empty());
    let sha = text.as_deref().map(core::body_sha256);
    let target_word = match action {
        Action::Dm => format!("@{}", to.trim_start_matches('@')),
        _ => to.clone(),
    };

    // 2. Sew on; the caller has chat somewhere.
    let sew = backend()?;
    need_any_access(c)?;

    // 3. Key replay: the same live attempt returns its first result.
    let now0 = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        expire_now(&conn)?;
        if let Some(prev) = store::live_action_by_key(&conn, &c.project_id, &key).map_err(internal)? {
            if prev.action == action.as_str() && prev.addr == target_word && prev.body_sha256 == sha {
                return Ok(replay(&conn, &prev, &key)?);
            }
            return Err(ChatErr::new(
                "idempotency_conflict",
                409,
                format!("key '{key}' was already used for a different message; pick a new --key"),
            ));
        }
        // 4. K2's own limits (counted over every attempt; restart-proof).
        rate_limit(&conn, &c.project_id, action, now0)?;
    }

    // 5. Sew resolves the target AS the agent (masked 404 first).
    let a = c.act_as(on_behalf.clone());
    let me = sew.me(&a).map_err(sew_err)?;
    let t = resolve_target(sew.as_ref(), &a, action, &to)?;

    // 6. The Space's access row and the level.
    let acc = access_for(c, sew.as_ref(), &a, &t.channel)?;
    let base = store::ActionRow {
        id: core::new_id("act_"),
        project_id: c.project_id.clone(),
        k2_session: c.session_id.clone(),
        action: action.as_str().into(),
        space_id: Some(acc.space_id.clone()),
        channel_id: Some(t.channel.id.clone()),
        target_id: t.target_id.clone(),
        addr: target_word.clone(),
        on_behalf_of: on_behalf.clone(),
        level: Some(acc.level.as_str().into()),
        approval: Some(acc.approval.as_str().into()),
        addressed: Some(is_addressed(&t, &me.member_id)),
        status: "refused".into(),
        client_key: key.clone(),
        body_sha256: sha.clone(),
        body_bytes: text.as_ref().map(|t| t.len() as i64),
        created_at: now0,
        updated_at: now0,
        ..Default::default()
    };
    if let Some(m) = &t.message {
        if m.author_id != me.member_id {
            refuse(&base, "not_author")?;
            return Err(ChatErr::new("not_author", 403, "you can only edit or delete your own messages"));
        }
    }
    let need = core::level_needed(action, t.message.as_ref().is_some_and(|m| m.reply_to.is_some()));
    if acc.level < need {
        refuse(&base, "level_denied")?;
        return Err(level_denied(acc.level, need, action, &acc.space_slug));
    }

    // 7. Approval required → queue (content writes only).
    if action.is_content() && acc.approval == ApprovalMode::Required {
        return enqueue(c, base, &t, text.expect("content write has text"), b.broadcast, &key, b.kind, b.wake, now0);
    }

    // 8. No row, no send.
    let row = store::ActionRow { status: "sending".into(), ..base };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::insert_action(&conn, &row).map_err(|e| internal(format!("{e} (nothing was sent)")))?;
    }
    let idem = format!("k2c-{key}");
    let res = match action {
        Action::Send | Action::Reply | Action::Dm => sew
            .create(
                &a,
                &NewMessage {
                    channel_id: t.channel.id.clone(),
                    body: text.clone().unwrap_or_default(),
                    reply_to: t.target_id.clone().filter(|_| action == Action::Reply),
                    broadcast: b.broadcast,
                    idempotency_key: idem.clone(),
                },
            )
            .map(|m| Some(m.id)),
        Action::Edit => sew.edit(&a, t.target_id.as_deref().unwrap_or(""), text.as_deref().unwrap_or(""), &idem).map(|m| Some(m.id)),
        Action::Delete => sew.delete(&a, t.target_id.as_deref().unwrap_or(""), &idem).map(|_| t.target_id.clone()),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match res {
        Ok(mid) => {
            store::finish_action(&conn, &row.id, "posted", mid.as_deref(), None, None, now()).map_err(internal)?;
            Ok(json!({
                "ok": true,
                "id": mid.as_deref().map(sew::wire_msg_id),
                "status": if action == Action::Delete { "deleted" } else { "posted" },
                "key": key,
                "addr": sew::channel_addr(&t.channel),
            }))
        }
        Err(e) => {
            store::finish_action(&conn, &row.id, "failed", None, Some(&e.code), Some(i64::from(e.status)), now()).map_err(internal)?;
            let mut err = sew_err(e);
            if err.status >= 500 {
                err = err.with("key", key.clone());
                err.hint = format!("{} (re-run with --key {key} to retry without a double post)", err.hint);
            }
            Err(err)
        }
    }
}

fn resolve_target(sew: &dyn SewChat, a: &ActAs, action: Action, to: &str) -> R<Target> {
    match action {
        Action::Send => {
            let channel = resolve_addr(sew, a, to)?;
            Ok(Target { channel, target_id: None, message: None, root: None })
        }
        Action::Dm => {
            let user = to.trim_start_matches('@');
            if user.is_empty() || user.contains('/') {
                return Err(usage("dm takes a K2 username: dm <user> \"text\""));
            }
            let channel = sew.open_dm(a, user).map_err(sew_err)?;
            Ok(Target { channel, target_id: None, message: None, root: None })
        }
        Action::Reply => {
            let id = sew::sew_msg_id(to).ok_or_else(|| usage("reply takes a msg_… id"))?;
            let m = sew.message(a, &id).map_err(sew_err)?;
            // Replying to a reply answers in the same thread.
            let root = match &m.reply_to {
                Some(r) => sew.message(a, r).map_err(sew_err)?,
                None => m,
            };
            let channel = sew.channel(a, &root.channel_id).map_err(sew_err)?;
            Ok(Target { channel, target_id: Some(root.id.clone()), message: None, root: Some(root) })
        }
        Action::Edit | Action::Delete => {
            let id = sew::sew_msg_id(to).ok_or_else(|| usage("name the message: id=msg_…"))?;
            let m = sew.message(a, &id).map_err(sew_err)?;
            let channel = sew.channel(a, &m.channel_id).map_err(sew_err)?;
            Ok(Target { channel, target_id: Some(m.id.clone()), message: Some(m), root: None })
        }
    }
}

/// The addressed rules (A1–A4) for the audit row.
fn is_addressed(t: &Target, me: &str) -> bool {
    let f = core::AddressFacts {
        dm_member: t.channel.kind.is_dm() && t.channel.dm_member_ids.iter().any(|m| m == me),
        root_mentions_me: t.root.as_ref().is_some_and(|r| r.mentions_me),
        following: t.root.as_ref().is_some_and(|r| r.following),
        root_author_is_me: t.root.as_ref().is_some_and(|r| r.author_id == me),
        recent_reply_mentions_me: false,
    };
    core::addressed(&f).is_some()
}

fn refuse(base: &store::ActionRow, reason: &str) -> R<()> {
    let row = store::ActionRow { status: "refused".into(), reason: Some(reason.into()), ..base.clone() };
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::insert_action(&conn, &row).map_err(internal)
}

fn level_denied(have: Level, need: Level, action: Action, space: &str) -> ChatErr {
    let hint = match (have, action) {
        (Level::Read, _) => format!("your chat level in {space} is 'read': you can read but not write — ask your human for 'reply' or 'send'"),
        (Level::Reply, Action::Send) => format!(
            "your chat level in {space} is 'reply': answer under a message (k2 chat reply msg_… \"…\") — or ask your human for 'send'"
        ),
        (Level::Reply, Action::Dm) => format!("your chat level in {space} is 'reply': starting a DM needs 'send' — ask your human"),
        _ => format!("your chat level in {space} is '{}'; this needs '{}' — ask your human", have.as_str(), need.as_str()),
    };
    ChatErr::new("level_denied", 403, hint).with("level", have.as_str()).with("needs", need.as_str())
}

fn rate_limit(conn: &rusqlite::Connection, project_id: &str, action: Action, now: i64) -> R<()> {
    let windows: &[(i64, i64)] = if action.is_content() {
        &[(60, core::CONTENT_PER_MIN), (3600, core::CONTENT_PER_HOUR)]
    } else {
        &[(60, core::LIGHT_PER_MIN)]
    };
    for (secs, cap) in windows {
        let (n, oldest) = store::count_since(conn, project_id, action.is_content(), now - secs).map_err(|e| {
            ChatErr::new("unavailable", 503, format!("can't read the chat rate counter ({e}); nothing was sent"))
        })?;
        if n >= *cap {
            let retry = oldest.map(|o| (o + secs - now).max(1)).unwrap_or(*secs);
            return Err(ChatErr::new("rate_limited", 429, format!("at most {cap} of these per {secs} s; retry in {retry} s"))
                .with("retryAfter", retry)
                .with("window", *secs));
        }
    }
    Ok(())
}

fn replay(conn: &rusqlite::Connection, prev: &store::ActionRow, key: &str) -> R<Value> {
    match prev.outbox_id.as_deref() {
        Some(out) => {
            let r = store::outbox(conn, out).map_err(internal)?.ok_or_else(|| internal("replayed item is gone"))?;
            Ok(json!({ "ok": true, "id": r.id, "status": "queued", "outboxStatus": r.status, "key": key, "replayed": true }))
        }
        None => Ok(json!({
            "ok": true,
            "id": prev.message_id.as_deref().map(sew::wire_msg_id),
            "status": if prev.status == "sending" { "sending" } else if prev.action == "delete" { "deleted" } else { "posted" },
            "key": key,
            "replayed": true,
        })),
    }
}

#[allow(clippy::too_many_arguments)]
fn enqueue(
    c: &Caller,
    base: store::ActionRow,
    t: &Target,
    text: String,
    broadcast: bool,
    key: &str,
    kind: Option<String>,
    wake: Option<bool>,
    now0: i64,
) -> R<Value> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = store::queued_count(&conn, &c.project_id).map_err(internal)?;
    if n >= core::QUEUE_CAP {
        drop(conn);
        refuse(&base, "queue_full")?;
        return Err(ChatErr::new(
            "queue_full",
            429,
            format!("{n} posts already wait for approval — wait for your human (k2 chat outbox)"),
        ));
    }
    let out_id = core::new_id("out_");
    let action_row = store::ActionRow { status: "queued".into(), outbox_id: Some(out_id.clone()), ..base.clone() };
    let o = store::OutboxRow {
        id: out_id.clone(),
        action_id: base.id.clone(),
        project_id: c.project_id.clone(),
        k2_session: c.session_id.clone(),
        agent_name: c.handle.clone(),
        action: base.action.clone(),
        space_id: base.space_id.clone(),
        channel_id: t.channel.id.clone(),
        target_id: t.target_id.clone(),
        addr: sew::channel_addr(&t.channel),
        on_behalf_of: base.on_behalf_of.clone(),
        broadcast,
        client_key: key.to_string(),
        body_sha256: core::body_sha256(&text),
        body_bytes: text.len() as i64,
        body: Some(text),
        message_kind: kind,
        wake,
        response: None,
        status: "queued".into(),
        created_at: now0,
        expires_at: store::expires_at(now0),
        ..Default::default()
    };
    store::queue(&conn, &action_row, &o).map_err(|e| internal(format!("{e} (nothing was queued)")))?;
    drop(conn);
    // The notice never carries the body, the channel or the person.
    emit_hook(
        k2_core::agent_hooks::HookEvent::SewChatApprovalRequested,
        json!({"outboxId": out_id, "projectId": c.project_id, "agentName": c.handle}),
    );
    crate::push_routes::notify_chat_approval_requested(&c.handle, &out_id, &approver_usernames());
    Ok(json!({
        "ok": true,
        "id": out_id,
        "status": "queued",
        "key": key,
        "expiresAt": o.expires_at,
        "hint": format!("queued for approval ({out_id}) — track it with k2 chat outbox {out_id}, or k2 chat wait --outbox {out_id}"),
    }))
}

/// Who gets the approval push: the owner and admin logins (the people who
/// may approve; agents learn through hooks and `k2 hostchat approvals`).
fn approver_usernames() -> Vec<String> {
    let mut v = vec![k2_core::notify_prefs::OWNER.to_string()];
    if let Ok(users) = k2_core::connect_users::list_users() {
        v.extend(
            users
                .into_iter()
                .filter(|u| !u.disabled && u.role >= k2_core::connect_users::Role::Admin)
                .map(|u| u.username),
        );
    }
    v
}
