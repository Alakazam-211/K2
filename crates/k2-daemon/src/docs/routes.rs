//! `/cli/docs/*`: the live-doc verbs behind `k2 doc` (Markdown multiplayer
//! v1, K2). One handler for every transport: the TCP dispatcher (owner
//! token, Connect logins, agent passports on Loopback) and the agent's cell
//! socket both resolve a [`DocCaller`] first and call [`handle`].
//!
//! | Route | Method | Right | Kit call | Parity |
//! |---|---|---|---|---|
//! | `/cli/docs/status` | GET | read (no path: pool stats) | `status` | P17, P25 |
//! | `/cli/docs/read` | GET | read | `join_shared` + `read` (+ `set_view` with `lines`) | P3, P4, P24 |
//! | `/cli/docs/who` | GET | read | `roster` | P6, P7 |
//! | `/cli/docs/log` | GET | **history** | store versions (metadata) | P19 |
//! | `/cli/docs/open` | POST | read | `join_shared(Held)` | P1 |
//! | `/cli/docs/leave` | POST | read | `leave` | P2 |
//! | `/cli/docs/view` | POST | read | `set_view` | P4 |
//! | `/cli/docs/cursor` | POST | edit | `set_cursor` | P8 |
//! | `/cli/docs/edit` | POST | edit | `edit_with`, or `edit` with `ifVersion` | P9–P11, P15 |
//! | `/cli/docs/write` | POST | edit | `write_base` / `replace_text` (`base: "live"`) | P14 |
//! | `/cli/docs/undo`, `/cli/docs/redo` | POST | edit | `undo` / `redo` | P16 |
//!
//! Every param may come in the query string or the JSON body (the body
//! wins): `workspace` (handle, name, id or root; an agent's own when
//! absent) and `path` (workspace-relative, or absolute).
//!
//! Refusals use the comms shape `{"ok":false,"error":{"code","hint"}}` so
//! the permission-card wrapper can teach them (`not_connected` →
//! `k2 connections add`). Status codes, for the CLI's exit codes: 400 bad
//! input (exit 2), 403 refused (3), 404 no such file or doc (4), 409 stale,
//! conflicts or disk sync pending (1), 422 declined (4), 429 rate-limited
//! (2), 503 docs unavailable.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use k2_multiplayer_base::session::position::{self, At, LineCol, LineRange};
use k2_multiplayer_base::session::{
    EditResult, EditWithError, Need, RosterEntry, Role, Session, SessionError, Stay, TextFilePack, UndoOutcome,
    WriteOutcome,
};
use k2_multiplayer_base::text::merge::Conflict;
use k2_multiplayer_base::text::Hunk;
use k2_multiplayer_base::{Edit, ParticipantKind};

use super::caller::{refusal, DocCaller};
use super::host::{self, CrossRefusal, DocRights, Principal};
use super::target::{self, Target};
use crate::cli_response::CliResponse;

pub const STATUS: &str = "/cli/docs/status";
pub const READ: &str = "/cli/docs/read";
pub const WHO: &str = "/cli/docs/who";
pub const LOG: &str = "/cli/docs/log";
pub const OPEN: &str = "/cli/docs/open";
pub const LEAVE: &str = "/cli/docs/leave";
pub const VIEW: &str = "/cli/docs/view";
pub const CURSOR: &str = "/cli/docs/cursor";
pub const EDIT: &str = "/cli/docs/edit";
pub const WRITE: &str = "/cli/docs/write";
pub const UNDO: &str = "/cli/docs/undo";
pub const REDO: &str = "/cli/docs/redo";

/// The family's prefix (passport allowlist, cell arm, dispatcher arm).
pub const PREFIX: &str = "/cli/docs/";

/// Largest POST body: a 4 MiB doc as a JSON string, escaped, plus room.
pub const MAX_BODY: usize = 12 << 20;

/// Most undo/redo steps one call may ask for.
const MAX_STEPS: usize = 100;

const ROOT: &str = TextFilePack::ROOT;

/// Every route, for the dispatcher arm and the tests.
pub const ROUTES: &[&str] = &[STATUS, READ, WHO, LOG, OPEN, LEAVE, VIEW, CURSOR, EDIT, WRITE, UNDO, REDO];

fn is_post_route(path: &str) -> bool {
    matches!(path, "/cli/docs/open" | "/cli/docs/leave" | "/cli/docs/view" | "/cli/docs/cursor")
        || is_edit_route(path)
}

fn is_edit_route(path: &str) -> bool {
    matches!(path, "/cli/docs/edit" | "/cli/docs/write" | "/cli/docs/undo" | "/cli/docs/redo")
}

/// Request input: the JSON body over the query params.
struct Input<'a> {
    params: &'a HashMap<String, String>,
    body: Value,
}

impl Input<'_> {
    fn str(&self, k: &str) -> Option<String> {
        match self.body.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => self.params.get(k).cloned(),
        }
    }

    fn u64(&self, k: &str) -> Result<Option<u64>, CliResponse> {
        match self.body.get(k) {
            Some(Value::Number(n)) => n.as_u64().map(Some).ok_or_else(|| bad(format!("{k} must be a whole number"))),
            Some(Value::Null) | None => match self.params.get(k) {
                Some(v) => v.trim().parse::<u64>().map(Some).map_err(|_| bad(format!("{k} must be a whole number"))),
                None => Ok(None),
            },
            Some(_) => Err(bad(format!("{k} must be a whole number"))),
        }
    }
}

fn bad(msg: impl Into<String>) -> CliResponse {
    refusal("400 Bad Request", "bad_request", msg)
}

fn ok(mut v: Value) -> CliResponse {
    if let Some(o) = v.as_object_mut() {
        o.insert("ok".into(), Value::Bool(true));
    }
    CliResponse::ok_json(v.to_string())
}

fn target_refusal(r: target::Refusal) -> CliResponse {
    refusal(r.status, r.code, r.message)
}

/// The entry for every transport. `caller` is resolved; `method` is the
/// HTTP method; `params` the query (or the cell's parsed params); `body`
/// the raw POST body (JSON object, may be empty).
pub fn handle(method: &str, path: &str, params: &HashMap<String, String>, body: &[u8], caller: &DocCaller) -> CliResponse {
    let is_post = method.eq_ignore_ascii_case("POST");
    if !ROUTES.contains(&path) {
        return CliResponse::not_found();
    }
    // POST guard (feedback_post_only_route_guards): mutating verbs are
    // POST-only on every transport; reads are GET.
    if is_post_route(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if !is_post_route(path) && is_post {
        return refusal("405 Method Not Allowed", "method_not_allowed", "use GET for this route");
    }
    let body = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Object(Default::default())
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => return bad("body must be a JSON object"),
            Err(e) => return bad(format!("body is not JSON: {e}")),
        }
    };
    let input = Input { params, body };

    if path == STATUS && input.str("path").is_none_or(|p| p.trim().is_empty()) {
        return ok(json!({ "pool": super::pool_json() }));
    }
    if is_edit_route(path) && !super::edits_allowed() {
        return refusal(
            "409 Conflict",
            "disk_sync_pending",
            "live editing waits for disk sync (the kit's next slice); nothing was changed. Edit the file directly for now",
        );
    }
    let rt = match super::runtime() {
        Ok(rt) => rt,
        Err(u) => return refusal("503 Service Unavailable", "docs_unavailable", u.message()),
    };
    let target = match target::resolve(input.str("workspace").as_deref(), &input.str("path").unwrap_or_default(), caller.default_workspace()) {
        Ok(t) => t,
        Err(r) => return target_refusal(r),
    };
    let rights = match rights_or_refusal(caller, &target, input.str("workspace").as_deref()) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if path == LOG {
        return log(&rt, &target, rights);
    }
    let session = match open(&rt, &target) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let r = verb(path, &session, caller, &target, &input, rights);
    match r {
        // The pool unloaded the session between open and the call (it was
        // idle and nobody held it): open again, once.
        Err(VerbError::Session(SessionError::Unloaded)) => match open(&rt, &target) {
            Ok(s) => verb(path, &s, caller, &target, &input, rights).unwrap_or_else(VerbError::into_response),
            Err(resp) => resp,
        },
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// K2's own rights check before the kit's (the kit re-asks on every call):
/// it gives the refusal its teaching code.
fn rights_or_refusal(caller: &DocCaller, t: &Target, workspace_param: Option<&str>) -> Result<DocRights, CliResponse> {
    let rights = match caller.principal() {
        Principal::Person(k) => host::person_rights(&k),
        Principal::Agent(id) => match host::agent_rights(&id, &t.project_id, &t.rel) {
            Ok(r) => r,
            Err(CrossRefusal::NotConnected) => {
                let shown = workspace_param.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&t.project_id).to_string();
                return Err(refusal("403 Forbidden", crate::comms::NOT_CONNECTED, crate::comms::not_connected_hint(&shown)));
            }
            Err(CrossRefusal::NotShared) => {
                return Err(refusal(
                    "403 Forbidden",
                    "doc_not_shared",
                    format!(
                        "{} is in another workspace and its owner hasn't shared it with you. Ask the owner to share the file or its folder",
                        t.rel
                    ),
                ))
            }
        },
    };
    if !rights.read {
        return Err(refusal("403 Forbidden", "doc_forbidden", "you can't read this doc"));
    }
    Ok(rights)
}

/// Load or import the doc. The file is read only when no session is loaded
/// (a stored doc ignores it; reconciling with disk is the kit's C4).
fn open(rt: &Arc<super::Runtime>, t: &Target) -> Result<Arc<Session>, CliResponse> {
    let key = t.key();
    if let Some(s) = rt.get(&key) {
        return Ok(s);
    }
    let len = std::fs::metadata(&t.abs).map(|m| m.len()).unwrap_or(0);
    if len > k2_multiplayer_markdown::MAX_FILE_BYTES as u64 {
        return Err(refusal(
            "422 Unprocessable Entity",
            "too_large",
            format!("{} is {} bytes; live docs are at most {} bytes", t.rel, len, k2_multiplayer_markdown::MAX_FILE_BYTES),
        ));
    }
    let bytes = std::fs::read(&t.abs).map_err(|e| refusal("404 Not Found", "file_not_found", format!("can't read {}: {e}", t.rel)))?;
    rt.open(&key, Some(&bytes)).map_err(|e| VerbError::Session(e).into_response())
}

enum VerbError {
    Session(SessionError),
    Response(CliResponse),
}

impl From<SessionError> for VerbError {
    fn from(e: SessionError) -> Self {
        VerbError::Session(e)
    }
}

impl VerbError {
    fn into_response(self) -> CliResponse {
        match self {
            VerbError::Response(r) => r,
            VerbError::Session(e) => session_error(&e),
        }
    }
}

/// The kit's errors on the wire (SESSIONS.md "Errors").
pub fn session_error(e: &SessionError) -> CliResponse {
    match e {
        SessionError::Forbidden { need } => refusal(
            "403 Forbidden",
            "doc_forbidden",
            match need {
                Need::Read => "you can't read this doc",
                Need::Edit => "you can't edit this doc (read-only)",
            },
        ),
        SessionError::NotJoined { .. } => refusal("409 Conflict", "not_joined", "your connection to this doc closed; run the command again"),
        SessionError::Stale { expected, actual } => CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "stale", "hint": format!("the doc moved from version {expected} to {actual}; read it again and retry"), "expected": expected, "actual": actual } }).to_string(),
        },
        SessionError::RateLimited { retry_after_ms } => CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "rate_limited", "hint": format!("too many edits; retry in {retry_after_ms} ms"), "retryAfterMs": retry_after_ms } }).to_string(),
        },
        SessionError::Declined(d) => CliResponse {
            status: "422 Unprocessable Entity",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "declined", "reason": d.code, "hint": format!("live editing is off for this file: {}", d.message) } }).to_string(),
        },
        SessionError::NotFound(_) => refusal("404 Not Found", "doc_not_found", "no such doc"),
        SessionError::PackMismatch { stored, pack } => {
            refusal("409 Conflict", "pack_mismatch", format!("this doc was stored as {stored}, not {pack}"))
        }
        SessionError::BadParticipant(m) | SessionError::BadPosition(m) | SessionError::BadEdit(m) => bad(m.clone()),
        SessionError::Doc(d) => bad(d.to_string()),
        SessionError::Released | SessionError::Unloaded => {
            refusal("503 Service Unavailable", "docs_unavailable", "live docs are restarting; run the command again")
        }
        other => refusal("500 Internal Server Error", "docs_failed", other.to_string()),
    }
}

/// A one-shot verb's connection: the pid's shared CLI connection (one
/// client ID across verbs), held when `stay` is `Held`.
fn join(s: &Session, caller: &DocCaller, stay: Stay) -> Result<k2_multiplayer_base::session::Joined, SessionError> {
    s.join_shared(caller.participant(), stay)
}

fn verb(
    path: &str,
    s: &Arc<Session>,
    caller: &DocCaller,
    t: &Target,
    input: &Input,
    rights: DocRights,
) -> Result<CliResponse, VerbError> {
    match path {
        STATUS => {
            let st = s.status()?;
            Ok(ok(json!({
                "doc": doc_json(t),
                "version": st.version,
                "participants": st.participants,
                "connections": st.connections,
                "logEntries": st.log_entries,
                "estimatedBytes": st.estimated_bytes,
                "declined": st.declined,
                "diskSync": super::KIT_DISK_SYNC,
                "rights": rights.json(),
            })))
        }
        READ => {
            let j = join(s, caller, Stay::UntilIdle)?;
            if let Some(lines) = input.str("lines") {
                let (first, last) = parse_lines(&lines).map_err(|m| VerbError::Response(bad(m)))?;
                s.set_view(j.conn, LineRange { root: ROOT.to_string(), first, last })?;
            }
            let r = s.read(j.conn)?;
            let text = String::from_utf8(r.bytes.clone()).map_err(|_| VerbError::Response(bad("doc bytes are not UTF-8")))?;
            let disk_matches = std::fs::read(&t.abs).map(|b| b == r.bytes).unwrap_or(false);
            Ok(ok(json!({
                "doc": doc_json(t),
                "version": r.version,
                "text": text,
                "contributorsSinceLastRead": r.contributors_since_last_read,
                "diskMatches": disk_matches,
                "rights": rights.json(),
            })))
        }
        WHO => {
            let j = join(s, caller, Stay::UntilIdle)?;
            let roster = s.roster(j.conn)?;
            Ok(ok(json!({ "doc": doc_json(t), "roster": roster.iter().map(roster_json).collect::<Vec<_>>() })))
        }
        OPEN => {
            let j = join(s, caller, Stay::Held)?;
            Ok(ok(json!({ "doc": doc_json(t), "version": j.version, "client": j.client, "rights": rights.json() })))
        }
        LEAVE => {
            let j = join(s, caller, Stay::UntilIdle)?;
            let left = s.leave(j.conn)?;
            Ok(ok(json!({ "doc": doc_json(t), "left": left })))
        }
        VIEW => {
            let lines = input.str("lines").ok_or_else(|| VerbError::Response(bad("lines is required (a,b)")))?;
            let (first, last) = parse_lines(&lines).map_err(|m| VerbError::Response(bad(m)))?;
            let j = join(s, caller, Stay::UntilIdle)?;
            let r = s.set_view(j.conn, LineRange { root: ROOT.to_string(), first, last })?;
            Ok(ok(json!({ "doc": doc_json(t), "view": { "first": r.first, "last": r.last } })))
        }
        CURSOR => {
            let at = input.str("at").ok_or_else(|| VerbError::Response(bad("at is required (L:C)")))?;
            let anchor = parse_line_col(&at).map_err(|m| VerbError::Response(bad(m)))?;
            let head = match input.str("to") {
                Some(to) => parse_line_col(&to).map_err(|m| VerbError::Response(bad(m)))?,
                None => anchor,
            };
            let j = join(s, caller, Stay::UntilIdle)?;
            let c = s.set_cursor(j.conn, ROOT, At::Line(anchor), At::Line(head))?;
            Ok(ok(json!({ "doc": doc_json(t), "cursor": { "anchor": c.anchor, "head": c.head } })))
        }
        EDIT => edit(s, caller, t, input),
        WRITE => write(s, caller, t, input),
        UNDO | REDO => {
            let steps = input.u64("steps").map_err(VerbError::Response)?.unwrap_or(1) as usize;
            if steps == 0 || steps > MAX_STEPS {
                return Err(VerbError::Response(bad(format!("steps must be 1..={MAX_STEPS}"))));
            }
            let j = join(s, caller, Stay::UntilIdle)?;
            let out = if path == UNDO { s.undo(j.conn, steps)? } else { s.redo(j.conn, steps)? };
            Ok(undo_response(t, out))
        }
        _ => Ok(CliResponse::not_found()),
    }
}

fn edit(s: &Arc<Session>, caller: &DocCaller, t: &Target, input: &Input) -> Result<CliResponse, VerbError> {
    let ops = match input.body.get("edits") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        _ => return Err(VerbError::Response(bad("edits is required: a non-empty list"))),
    };
    let if_version = input.u64("ifVersion").map_err(VerbError::Response)?;
    let j = join(s, caller, Stay::UntilIdle)?;
    let result = match if_version {
        // Positions are from version V: refuse at once if the doc moved,
        // else the kit re-checks V atomically with the edit.
        Some(v) => {
            let r = s.read(j.conn)?;
            if r.version != v {
                return Err(VerbError::Session(SessionError::Stale { expected: v, actual: r.version }));
            }
            let text = r.text_by_root.iter().find(|(root, _)| root == ROOT).map(|(_, t)| t.clone()).unwrap_or_default();
            let hunks = hunks_for(&text, &ops).map_err(|m| VerbError::Response(bad(m)))?;
            s.edit(j.conn, vec![Edit::Text { root: ROOT.to_string(), hunks }], Some(v))?
        }
        // Positions against the live text, computed and applied under the
        // session lock.
        None => {
            match s.edit_with(j.conn, ROOT, |live| hunks_for(live, &ops).map(|h| splice(live, &h))) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    let mut r = EditResult::default();
                    r.version = s.status()?.version;
                    r
                }
                Err(EditWithError::Compute(m)) => return Err(VerbError::Response(bad(m))),
                Err(EditWithError::Session(e)) => return Err(VerbError::Session(e)),
            }
        }
    };
    Ok(ok(json!({ "doc": doc_json(t), "result": edit_json(&result) })))
}

fn write(s: &Arc<Session>, caller: &DocCaller, t: &Target, input: &Input) -> Result<CliResponse, VerbError> {
    let text = match input.body.get("text") {
        Some(Value::String(s)) => s.clone(),
        _ => return Err(VerbError::Response(bad("text is required (a string in the JSON body)"))),
    };
    let base = input.str("base").ok_or_else(|| VerbError::Response(bad("base is required: a version you read, or \"live\"")))?;
    let j = join(s, caller, Stay::UntilIdle)?;
    if base.trim() == "live" {
        let r = s.replace_text(j.conn, ROOT, &text)?;
        let mut v = json!({ "doc": doc_json(t), "result": edit_json(&r) });
        if matches!(caller, DocCaller::Agent { .. }) {
            v["warning"] = json!("--base live replaced whatever was there, including other people's edits since you read it; use --base <version> to merge");
        }
        return Ok(ok(v));
    }
    let base: u64 = base.trim().parse().map_err(|_| VerbError::Response(bad("base must be a version number or \"live\"")))?;
    match s.write_base(j.conn, ROOT, base, &text)? {
        WriteOutcome::Applied(r) => Ok(ok(json!({ "doc": doc_json(t), "result": edit_json(&r) }))),
        WriteOutcome::Conflicts(c) => Ok(conflicts_response(t, None, &c)),
        WriteOutcome::BaseExpired => Ok(refusal(
            "409 Conflict",
            "base_expired",
            format!("version {base} is no longer kept as a base; read the doc again and retry"),
        )),
    }
}

/// `log`: the doc's stored versions, metadata only (who, when, why), for a
/// caller with the history right (K-PERM-9).
fn log(rt: &Arc<super::Runtime>, t: &Target, rights: DocRights) -> CliResponse {
    if !rights.history {
        return refusal("403 Forbidden", "history_forbidden", "you may read this doc but not its history");
    }
    let Some(store) = rt.store() else {
        return refusal("503 Service Unavailable", "docs_unavailable", "live docs are restarting; run the command again");
    };
    let id = match store.find(&t.key()) {
        Ok(Some(id)) => id,
        Ok(None) => return ok(json!({ "doc": doc_json(t), "versions": [] })),
        Err(e) => return refusal("500 Internal Server Error", "docs_failed", e.to_string()),
    };
    match store.versions(id) {
        Ok(vs) => ok(json!({
            "doc": doc_json(t),
            "versions": vs.iter().map(|(vid, v)| json!({
                "id": vid,
                "version": v.seq,
                "atMs": v.at_ms,
                "reason": format!("{:?}", v.reason).to_ascii_lowercase(),
                "name": v.name,
                "authors": v.authors,
            })).collect::<Vec<_>>(),
        })),
        Err(e) => refusal("500 Internal Server Error", "docs_failed", e.to_string()),
    }
}

// ── shapes ────────────────────────────────────────────────────────────

fn doc_json(t: &Target) -> Value {
    json!({ "workspace": t.project_id, "path": t.rel })
}

fn edit_json(r: &EditResult) -> Value {
    json!({
        "version": r.version,
        "changed": r.changed.iter().map(|l| json!({ "first": l.first, "last": l.last })).collect::<Vec<_>>(),
        "linesAdded": r.lines_added,
        "linesRemoved": r.lines_removed,
    })
}

fn conflicts_json(c: &[Conflict]) -> Value {
    Value::Array(
        c.iter()
            .map(|c| json!({ "lines": [c.lines.0, c.lines.1], "base": c.base_text, "ours": c.ours, "theirs": c.theirs }))
            .collect(),
    )
}

fn conflicts_response(t: &Target, step: Option<usize>, c: &[Conflict]) -> CliResponse {
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "doc": doc_json(t),
            "error": { "code": "conflicts", "hint": "your change overlaps edits made since; nothing was applied", "step": step },
            "conflicts": conflicts_json(c),
        })
        .to_string(),
    }
}

fn undo_response(t: &Target, out: UndoOutcome) -> CliResponse {
    match out {
        UndoOutcome::Applied { steps, result } => ok(json!({ "doc": doc_json(t), "steps": steps, "result": edit_json(&result) })),
        UndoOutcome::Conflicts { step, conflicts } => conflicts_response(t, Some(step), &conflicts),
        UndoOutcome::Nothing => ok(json!({ "doc": doc_json(t), "steps": 0, "nothing": true })),
    }
}

fn kind_str(k: ParticipantKind) -> &'static str {
    match k {
        ParticipantKind::Person => "person",
        ParticipantKind::Agent => "agent",
        ParticipantKind::App => "app",
        ParticipantKind::External => "external",
        ParticipantKind::Restore => "restore",
        ParticipantKind::Import => "import",
    }
}

fn roster_json(e: &RosterEntry) -> Value {
    let p = &e.participant;
    json!({
        "pid": p.pid,
        "name": p.name,
        "colour": p.colour,
        "kind": kind_str(p.kind),
        "via": p.via,
        "home": p.home,
        "role": match e.role { Role::Editor => "editor", Role::Viewer => "viewer" },
        "away": e.away,
        "cursor": e.cursor.as_ref().map(|c| json!({
            "anchor": format!("{}:{}", c.anchor.line, c.anchor.col),
            "head": format!("{}:{}", c.head.line, c.head.col),
        })),
        "view": e.view.as_ref().map(|v| json!({ "first": v.first, "last": v.last })),
        "lastEditMs": e.last_edit_ms,
        "held": matches!(e.stay, Stay::Held),
        "connections": e.connections,
    })
}

// ── positions and edit ops ────────────────────────────────────────────

/// `L:C`, 1-based.
pub fn parse_line_col(s: &str) -> Result<LineCol, String> {
    let (l, c) = s.trim().split_once(':').ok_or_else(|| format!("{s:?} is not L:C"))?;
    let line: usize = l.trim().parse().map_err(|_| format!("{s:?} is not L:C"))?;
    let col: usize = c.trim().parse().map_err(|_| format!("{s:?} is not L:C"))?;
    if line == 0 || col == 0 {
        return Err(format!("{s:?}: lines and columns start at 1"));
    }
    Ok(LineCol { line, col })
}

/// `a,b` (or a single `a`), 1-based inclusive.
pub fn parse_lines(s: &str) -> Result<(usize, usize), String> {
    let (a, b) = match s.trim().split_once(',') {
        Some((a, b)) => (a, b),
        None => (s.trim(), s.trim()),
    };
    let first: usize = a.trim().parse().map_err(|_| format!("{s:?} is not a,b"))?;
    let last: usize = b.trim().parse().map_err(|_| format!("{s:?} is not a,b"))?;
    if first == 0 || last < first {
        return Err(format!("{s:?}: lines start at 1 and a <= b"));
    }
    Ok((first, last))
}

fn at(text: &str, v: &Value, what: &str) -> Result<usize, String> {
    let lc = match v {
        Value::String(s) => parse_line_col(s)?,
        Value::Object(o) => LineCol {
            line: o.get("line").and_then(Value::as_u64).ok_or(format!("{what}.line is required"))? as usize,
            col: o.get("col").and_then(Value::as_u64).ok_or(format!("{what}.col is required"))? as usize,
        },
        _ => return Err(format!("{what} must be \"L:C\" or {{line, col}}")),
    };
    position::offset(text, lc).ok_or_else(|| format!("{what} {}:{} is past the end of the doc", lc.line, lc.col))
}

/// Byte offset where line `n + 1` starts, or the end (`after: n`).
fn after_line(text: &str, n: usize) -> Result<usize, String> {
    if n == 0 {
        return Ok(0);
    }
    let lines = position::line_count(text);
    if n > lines {
        return Err(format!("after {n}: the doc has {lines} lines"));
    }
    Ok(position::line_start(text, n + 1).unwrap_or(text.len()))
}

fn with_newline(t: &str) -> String {
    if t.is_empty() || t.ends_with('\n') {
        t.to_string()
    } else {
        format!("{t}\n")
    }
}

/// One op as a hunk on `text`:
/// - `{at: "L:C", text}`: insert;
/// - `{from: "L:C", to: "L:C", text}`: replace a range (empty text deletes);
/// - `{lines: "a,b", text}`: replace whole lines a..=b (empty text deletes them);
/// - `{after: n, text}`: insert lines after line n (0 = top).
fn hunk_for(text: &str, op: &Value) -> Result<Hunk, String> {
    let o = op.as_object().ok_or("each edit must be an object")?;
    let insert = match o.get("text") {
        Some(Value::String(s)) => s.clone(),
        None => String::new(),
        Some(_) => return Err("text must be a string".into()),
    };
    if let Some(v) = o.get("lines") {
        let s = v.as_str().ok_or("lines must be \"a,b\"")?;
        let (first, last) = parse_lines(s)?;
        let lines = position::line_count(text);
        if last > lines {
            return Err(format!("lines {first},{last}: the doc has {lines} lines"));
        }
        let start = position::line_start(text, first).ok_or("line out of range")?;
        let end = position::line_start(text, last + 1).unwrap_or(text.len());
        let insert = if end == text.len() && !text.ends_with('\n') { insert } else { with_newline(&insert) };
        return Ok(Hunk { start, end, insert });
    }
    if let Some(v) = o.get("after") {
        let n = v.as_u64().ok_or("after must be a line number")? as usize;
        let pos = after_line(text, n)?;
        let insert = if pos == text.len() && !text.is_empty() && !text.ends_with('\n') {
            format!("\n{}", insert.trim_end_matches('\n'))
        } else {
            with_newline(&insert)
        };
        return Ok(Hunk { start: pos, end: pos, insert });
    }
    if let Some(v) = o.get("at") {
        let pos = at(text, v, "at")?;
        return Ok(Hunk { start: pos, end: pos, insert });
    }
    if let (Some(f), Some(t)) = (o.get("from"), o.get("to")) {
        let start = at(text, f, "from")?;
        let end = at(text, t, "to")?;
        if end < start {
            return Err("to is before from".into());
        }
        return Ok(Hunk { start, end, insert });
    }
    Err("each edit needs one of: at, from+to, lines, after".into())
}

/// All ops as hunks, sorted; overlapping ops are refused.
pub fn hunks_for(text: &str, ops: &[Value]) -> Result<Vec<Hunk>, String> {
    let mut hunks = ops.iter().map(|op| hunk_for(text, op)).collect::<Result<Vec<_>, _>>()?;
    hunks.sort_by_key(|h| (h.start, h.end));
    for w in hunks.windows(2) {
        if w[1].start < w[0].end || (w[1].start == w[0].start && w[0].start == w[0].end && w[1].start == w[1].end) {
            return Err("edits overlap; send them as one edit".into());
        }
    }
    Ok(hunks)
}

/// `text` with sorted, non-overlapping `hunks` applied.
pub fn splice(text: &str, hunks: &[Hunk]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for h in hunks {
        out.push_str(&text[at..h.start]);
        out.push_str(&h.insert);
        at = h.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, ops: Value) -> Result<String, String> {
        let ops = ops.as_array().expect("array").clone();
        hunks_for(text, &ops).map(|h| splice(text, &h))
    }

    #[test]
    fn line_ops_replace_insert_and_delete_whole_lines() {
        let t = "a\nb\nc\n";
        assert_eq!(apply(t, json!([{ "lines": "2,2", "text": "B" }])).expect("replace"), "a\nB\nc\n");
        assert_eq!(apply(t, json!([{ "lines": "1,2", "text": "" }])).expect("delete"), "c\n");
        assert_eq!(apply(t, json!([{ "after": 0, "text": "top" }])).expect("top"), "top\na\nb\nc\n");
        assert_eq!(apply(t, json!([{ "after": 3, "text": "end" }])).expect("end"), "a\nb\nc\nend\n");
        assert_eq!(apply("a\nb", json!([{ "after": 2, "text": "c" }])).expect("no final newline"), "a\nb\nc");
        assert_eq!(apply("a\nb", json!([{ "lines": "2,2", "text": "B" }])).expect("last line"), "a\nB");
    }

    #[test]
    fn positional_ops_use_line_col() {
        let t = "héllo\nworld\n";
        assert_eq!(apply(t, json!([{ "at": "1:3", "text": "X" }])).expect("insert"), "héXllo\nworld\n");
        assert_eq!(apply(t, json!([{ "from": "2:1", "to": "2:6", "text": "there" }])).expect("replace"), "héllo\nthere\n");
        assert_eq!(apply(t, json!([{ "at": { "line": 2, "col": 6 }, "text": "!" }])).expect("eol"), "héllo\nworld!\n");
    }

    #[test]
    fn bad_ops_are_refused_and_change_nothing() {
        let t = "a\nb\n";
        for ops in [
            json!([{ "at": "9:1", "text": "x" }]),
            json!([{ "lines": "2,5", "text": "x" }]),
            json!([{ "lines": "0,1", "text": "x" }]),
            json!([{ "from": "2:1", "to": "1:1", "text": "x" }]),
            json!([{ "text": "x" }]),
            json!([{ "lines": "1,2", "text": "x" }, { "at": "2:1", "text": "y" }]),
            json!([{ "at": "1:1", "text": "x" }, { "at": "1:1", "text": "y" }]),
            json!(["nope"]),
        ] {
            assert!(apply(t, ops.clone()).is_err(), "{ops}");
        }
    }

    #[test]
    fn several_ops_apply_together() {
        let t = "a\nb\nc\n";
        let got = apply(t, json!([{ "lines": "3,3", "text": "C" }, { "at": "1:1", "text": "> " }])).expect("two");
        assert_eq!(got, "> a\nb\nC\n");
    }

    #[test]
    fn line_and_col_parsers_are_strict() {
        assert_eq!(parse_line_col("2:3"), Ok(LineCol { line: 2, col: 3 }));
        assert!(parse_line_col("0:1").is_err());
        assert!(parse_line_col("2").is_err());
        assert_eq!(parse_lines("4"), Ok((4, 4)));
        assert_eq!(parse_lines("2,5"), Ok((2, 5)));
        assert!(parse_lines("5,2").is_err());
    }

    #[test]
    fn every_mutating_route_is_post_only_and_reads_are_get() {
        let caller = super::super::caller::owner();
        let params = HashMap::new();
        for r in ROUTES {
            let wrong = if is_post_route(r) { "GET" } else { "POST" };
            let resp = handle(wrong, r, &params, b"", &caller);
            assert!(resp.status.starts_with("405"), "{wrong} {r} answered {}", resp.status);
        }
        assert!(handle("GET", "/cli/docs/nope", &params, b"", &caller).status.starts_with("404"));
    }
}
