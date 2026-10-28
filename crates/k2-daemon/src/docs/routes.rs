//! `/cli/docs/*`: the live-doc verbs behind `k2 doc` (Markdown multiplayer
//! v1, K2). One handler for every transport: the TCP dispatcher (owner
//! token, Connect logins, agent passports on Loopback) and the agent's cell
//! socket both resolve a [`DocCaller`] first and call [`handle`].
//!
//! | Route | Method | Right | Kit call | Parity |
//! |---|---|---|---|---|
//! | `/cli/docs/status` | GET | read (no path: pool stats) | `status` (with `disk`) | P17, P25 |
//! | `/cli/docs/flush` | POST | read | `flush` (write back now) | P17 |
//! | `/cli/docs/tool` | POST | read or edit (per verb) | `run_tool`: cat head tail wc grep sed diff view, sed -i patch append insert replace delete | P3–P5, P9–P13 |
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
//! | `/cli/docs/approve` | POST | the named person | `set_approval` | P29 |
//! | `/cli/docs/approvals` | GET | read (others' records: history) | `approvals` | P30 |
//! | `/cli/docs/shares` | GET | a person | host table ([`super::shares`]) | MI25 |
//! | `/cli/docs/share`, `/cli/docs/unshare` | POST | a person (Admin floor for logins) | host table | MI25 |
//!
//! Every param may come in the query string or the JSON body (the body
//! wins): `workspace` (handle, name, id or root; an agent's own when
//! absent) and `path` (workspace-relative, or absolute). An agent may add
//! `onBehalfOf` (a person) to any verb (K-ID-10): its edits then carry that
//! person and wait for their approval, without being held.
//!
//! Refusals use the comms shape `{"ok":false,"error":{"code","hint"}}` so
//! the permission-card wrapper can teach them (`not_connected` →
//! `k2 connections add`). Status codes, for the CLI's exit codes: 400 bad
//! input or path (exit 2), 403 refused (3), 404 no such file or doc (4),
//! 409 stale or conflicts (1), 422 declined (4), 429 rate-limited (2),
//! 500 disk error (1), 503 docs unavailable.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use k2_mpc_base::session::position::{self, At, LineCol, LineRange};
use k2_mpc_base::session::{
    Approval, ApprovalState, DiskState, EditResult, EditWithError, Need, RosterEntry, Role, Session, SessionError, Stay,
    TextFilePack, UndoOutcome, WriteOutcome,
};
use k2_mpc_base::store::DocStore;
use k2_mpc_base::text::merge::Conflict;
use k2_mpc_base::text::Hunk;
use k2_mpc_base::{Edit, ParticipantKind};

use super::caller::{refusal, DocCaller};
use super::host::{self, CrossRefusal, DocRights, Principal};
use super::shares::{self, Share};
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
pub const APPROVE: &str = "/cli/docs/approve";
pub const APPROVALS: &str = "/cli/docs/approvals";
pub const SHARES: &str = "/cli/docs/shares";
pub const SHARE: &str = "/cli/docs/share";
pub const UNSHARE: &str = "/cli/docs/unshare";
pub const FLUSH: &str = "/cli/docs/flush";
pub const TOOL: &str = "/cli/docs/tool";

/// The family's prefix (passport allowlist, cell arm, dispatcher arm).
pub const PREFIX: &str = "/cli/docs/";

/// Largest POST body: a 4 MiB doc as a JSON string, escaped, plus room.
pub const MAX_BODY: usize = 12 << 20;

/// Most undo/redo steps one call may ask for.
const MAX_STEPS: usize = 100;

const ROOT: &str = TextFilePack::ROOT;

/// Every route, for the dispatcher arm and the tests.
pub const ROUTES: &[&str] = &[
    STATUS, READ, WHO, LOG, OPEN, LEAVE, VIEW, CURSOR, EDIT, WRITE, UNDO, REDO, APPROVE, APPROVALS, SHARES, SHARE,
    UNSHARE, FLUSH, TOOL,
];

fn is_post_route(path: &str) -> bool {
    matches!(
        path,
        "/cli/docs/open"
            | "/cli/docs/leave"
            | "/cli/docs/view"
            | "/cli/docs/cursor"
            | "/cli/docs/approve"
            | "/cli/docs/share"
            | "/cli/docs/unshare"
            | "/cli/docs/flush"
            | "/cli/docs/tool"
    ) || is_edit_route(path)
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
    if matches!(path, SHARES | SHARE | UNSHARE) {
        return shares_route(path, caller, &input);
    }
    let on_behalf = match super::caller::on_behalf_pid(caller, input.str("onBehalfOf").as_deref()) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let rt = match super::runtime() {
        Ok(rt) => rt,
        Err(u) => return refusal("503 Service Unavailable", "docs_unavailable", u.message()),
    };
    let target = match target::resolve(input.str("workspace").as_deref(), &input.str("path").unwrap_or_default(), caller.default_workspace()) {
        Ok(t) => t,
        Err(r) => return target_refusal(r),
    };
    let store = rt.store();
    let rights = match rights_or_refusal(caller, &target, input.str("workspace").as_deref(), store.as_deref()) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    drop(store);
    if path == LOG {
        return log(&rt, &target, rights);
    }
    let session = match open(&rt, &target) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let call = Call { caller, on_behalf: on_behalf.as_deref(), target: &target, input: &input, rights };
    let r = verb(path, &session, &call);
    match r {
        // The pool unloaded the session between open and the call (it was
        // idle and nobody held it): open again, once.
        Err(VerbError::Session(SessionError::Unloaded)) => match open(&rt, &target) {
            Ok(s) => verb(path, &s, &call).unwrap_or_else(VerbError::into_response),
            Err(resp) => resp,
        },
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// One doc call, resolved.
struct Call<'a> {
    caller: &'a DocCaller,
    /// `person:<key>` when an agent acts for that person.
    on_behalf: Option<&'a str>,
    target: &'a Target,
    input: &'a Input<'a>,
    rights: DocRights,
}

/// K2's own rights check before the kit's (the kit re-asks on every call):
/// it gives the refusal its teaching code.
fn rights_or_refusal(
    caller: &DocCaller,
    t: &Target,
    workspace_param: Option<&str>,
    store: Option<&DocStore>,
) -> Result<DocRights, CliResponse> {
    let rights = match caller.principal() {
        Principal::Person(k) => host::person_rights(&k),
        Principal::Agent(id) => match host::agent_rights(&id, &t.project_id, &t.rel, store) {
            Ok(r) => r,
            Err(CrossRefusal::NotConnected) => {
                let shown = workspace_param.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&t.project_id).to_string();
                return Err(refusal("403 Forbidden", crate::comms::NOT_CONNECTED, crate::comms::not_connected_hint(&shown)));
            }
            Err(CrossRefusal::NotShared) => {
                // `workspace` + `path` let the permission wrapper fill in
                // `k2 permission request docs.share from= path=` (walls.rs).
                let ws = workspace_label(&t.project_id);
                return Err(CliResponse {
                    status: "403 Forbidden",
                    content_type: "application/json",
                    body: json!({
                        "ok": false,
                        "error": {
                            "code": "doc_not_shared",
                            "hint": format!(
                                "{} is in workspace {ws}, which hasn't shared it with yours. A person shares it (k2 doc share), or you ask with a permission card",
                                t.rel
                            ),
                            "workspace": ws,
                            "path": t.rel,
                        },
                    })
                    .to_string(),
                });
            }
        },
    };
    if !rights.read {
        return Err(refusal("403 Forbidden", "doc_forbidden", "you can't read this doc"));
    }
    Ok(rights)
}

/// Load the doc synced with its file (`open_file`): import a new one,
/// reconcile a stored one with edits made while it was closed. A loaded
/// session is returned as is (its file is kept true by the watcher and the
/// kit's stat backup, which also checks on every join).
fn open(rt: &Arc<super::Runtime>, t: &Target) -> Result<Arc<Session>, CliResponse> {
    let key = t.key();
    if let Some(s) = rt.get(&key) {
        return Ok(s);
    }
    rt.open(&key, &t.abs, &t.root).map_err(|e| VerbError::Session(e).into_response())
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
        SessionError::Forbidden { need } => match need {
            Need::Read => refusal("403 Forbidden", "doc_forbidden", "you can't read this doc"),
            Need::Edit => refusal("403 Forbidden", "doc_forbidden", "you can't edit this doc (read-only)"),
            Need::History => refusal("403 Forbidden", "history_forbidden", "you may read this doc but not its history"),
            Need::Approver => refusal(
                "403 Forbidden",
                "not_the_approver",
                "only the person this change was made for can approve or decline it",
            ),
            _ => refusal("403 Forbidden", "doc_forbidden", "refused"),
        },
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
        SessionError::NoApproval { seq } => {
            refusal("404 Not Found", "no_approval", format!("change {seq} wasn't made on anyone's behalf"))
        }
        SessionError::BadApproval(m) => refusal("409 Conflict", "bad_approval", m.clone()),
        SessionError::BadPath(m) => refusal("400 Bad Request", "bad_path", m.clone()),
        SessionError::Disk(m) => refusal("500 Internal Server Error", "disk_error", format!("the file couldn't be read or written: {m}")),
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
/// client ID across verbs; the kit keeps one per pid, via and
/// on-behalf-of), held when `stay` is `Held`.
fn join(s: &Session, c: &Call, stay: Stay) -> Result<k2_mpc_base::session::Joined, SessionError> {
    s.join_shared(c.caller.participant(c.on_behalf), stay)
}

fn verb(path: &str, s: &Arc<Session>, c: &Call) -> Result<CliResponse, VerbError> {
    let (t, input, rights) = (c.target, c.input, c.rights);
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
                "disk": disk_json(st.disk.as_ref()),
                "rights": rights.json(),
            })))
        }
        TOOL => tool(s, c),
        FLUSH => {
            let j = join(s, c, Stay::UntilIdle)?;
            let f = s.flush(j.conn)?;
            Ok(ok(json!({
                "doc": doc_json(t),
                "written": f.written,
                "hash": f.hash,
                "disk": disk_json(f.disk.as_ref()),
            })))
        }
        READ => {
            let j = join(s, c, Stay::UntilIdle)?;
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
            let j = join(s, c, Stay::UntilIdle)?;
            let roster = s.roster(j.conn)?;
            Ok(ok(json!({ "doc": doc_json(t), "roster": roster.iter().map(roster_json).collect::<Vec<_>>() })))
        }
        OPEN => {
            let j = join(s, c, Stay::Held)?;
            Ok(ok(json!({ "doc": doc_json(t), "version": j.version, "client": j.client, "rights": rights.json() })))
        }
        LEAVE => {
            let j = join(s, c, Stay::UntilIdle)?;
            let left = s.leave(j.conn)?;
            Ok(ok(json!({ "doc": doc_json(t), "left": left })))
        }
        VIEW => {
            let lines = input.str("lines").ok_or_else(|| VerbError::Response(bad("lines is required (a,b)")))?;
            let (first, last) = parse_lines(&lines).map_err(|m| VerbError::Response(bad(m)))?;
            let j = join(s, c, Stay::UntilIdle)?;
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
            let j = join(s, c, Stay::UntilIdle)?;
            let cur = s.set_cursor(j.conn, ROOT, At::Line(anchor), At::Line(head))?;
            Ok(ok(json!({ "doc": doc_json(t), "cursor": { "anchor": cur.anchor, "head": cur.head } })))
        }
        EDIT => edit(s, c),
        WRITE => write(s, c),
        UNDO | REDO => {
            let steps = input.u64("steps").map_err(VerbError::Response)?.unwrap_or(1) as usize;
            if steps == 0 || steps > MAX_STEPS {
                return Err(VerbError::Response(bad(format!("steps must be 1..={MAX_STEPS}"))));
            }
            let j = join(s, c, Stay::UntilIdle)?;
            let out = if path == UNDO { s.undo(j.conn, steps)? } else { s.redo(j.conn, steps)? };
            Ok(undo_response(c, out))
        }
        APPROVE => {
            let seq = input.u64("change").map_err(VerbError::Response)?.ok_or_else(|| {
                VerbError::Response(bad("change is required: the change number from approvals or the edit"))
            })?;
            let state = match input.str("state").as_deref().map(str::trim) {
                Some("approved") | Some("approve") => ApprovalState::Approved,
                Some("declined") | Some("decline") => ApprovalState::Declined,
                _ => return Err(VerbError::Response(bad("state must be approved or declined"))),
            };
            let j = join(s, c, Stay::UntilIdle)?;
            let a = s.set_approval(j.conn, seq, state)?;
            Ok(ok(json!({ "doc": doc_json(t), "change": seq, "approval": approval_json(&a) })))
        }
        APPROVALS => {
            let pending_only = input.str("pending").is_some_and(|v| v == "1" || v == "true");
            let mine = input.str("mine").is_some_and(|v| v == "1" || v == "true");
            let me = c.caller.pid();
            let j = join(s, c, Stay::UntilIdle)?;
            let rows: Vec<Value> = s
                .approvals(j.conn)?
                .into_iter()
                .filter(|(_, a)| !pending_only || a.state == ApprovalState::Pending)
                .filter(|(_, a)| !mine || a.on_behalf_of == me)
                .map(|(seq, a)| json!({ "change": seq, "approval": approval_json(&a) }))
                .collect();
            Ok(ok(json!({ "doc": doc_json(t), "approvals": rows })))
        }
        _ => Ok(CliResponse::not_found()),
    }
}

/// The edit's own approval, when an agent made it on someone's behalf (the
/// kit records `Pending` with the change; edits are never held).
fn edit_reply(s: &Session, c: &Call, r: &EditResult, extra: Option<(&str, Value)>) -> Result<CliResponse, VerbError> {
    let mut v = json!({ "doc": doc_json(c.target), "result": edit_json(r) });
    if let Some(person) = c.on_behalf {
        v["onBehalfOf"] = json!(person);
        if !r.changed.is_empty() || r.lines_added + r.lines_removed > 0 {
            let j = join(s, c, Stay::UntilIdle)?;
            if let Some(a) = s.approval(j.conn, r.version)? {
                v["approval"] = approval_json(&a);
            }
        }
    }
    if let Some((k, val)) = extra {
        v[k] = val;
    }
    Ok(ok(v))
}

fn edit(s: &Arc<Session>, c: &Call) -> Result<CliResponse, VerbError> {
    let ops = match c.input.body.get("edits") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        _ => return Err(VerbError::Response(bad("edits is required: a non-empty list"))),
    };
    let if_version = c.input.u64("ifVersion").map_err(VerbError::Response)?;
    let j = join(s, c, Stay::UntilIdle)?;
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
        None => match s.edit_with(j.conn, ROOT, |live| hunks_for(live, &ops).map(|h| splice(live, &h))) {
            Ok(Some(r)) => r,
            Ok(None) => {
                let mut r = EditResult::default();
                r.version = s.status()?.version;
                r
            }
            Err(EditWithError::Compute(m)) => return Err(VerbError::Response(bad(m))),
            Err(EditWithError::Session(e)) => return Err(VerbError::Session(e)),
        },
    };
    edit_reply(s, c, &result, None)
}

fn write(s: &Arc<Session>, c: &Call) -> Result<CliResponse, VerbError> {
    let text = match c.input.body.get("text") {
        Some(Value::String(s)) => s.clone(),
        _ => return Err(VerbError::Response(bad("text is required (a string in the JSON body)"))),
    };
    let base = c
        .input
        .str("base")
        .ok_or_else(|| VerbError::Response(bad("base is required: a version you read, or \"live\"")))?;
    let j = join(s, c, Stay::UntilIdle)?;
    if base.trim() == "live" {
        let r = s.replace_text(j.conn, ROOT, &text)?;
        let warning = matches!(c.caller, DocCaller::Agent { .. }).then(|| {
            (
                "warning",
                json!("--base live replaced whatever was there, including other people's edits since you read it; use --base <version> to merge"),
            )
        });
        return edit_reply(s, c, &r, warning);
    }
    let base: u64 = base.trim().parse().map_err(|_| VerbError::Response(bad("base must be a version number or \"live\"")))?;
    match s.write_base(j.conn, ROOT, base, &text)? {
        WriteOutcome::Applied(r) => edit_reply(s, c, &r, None),
        WriteOutcome::Conflicts(conf) => Ok(conflicts_response(c.target, None, &conf)),
        WriteOutcome::BaseExpired => Ok(refusal(
            "409 Conflict",
            "base_expired",
            format!("version {base} is no longer kept as a base; read the doc again and retry"),
        )),
    }
}

/// Most argv words a tool call may carry.
const MAX_ARGV: usize = 64;

/// The prefix fence lines name (`┄┄ k2 doc <nonce> BEGIN …`).
pub const FENCE_PREFIX: &str = "k2 doc";

/// `tool`: one `k2 doc <verb>` on the kit's text tools (`Session::run_tool`).
/// Body: `{argv: ["grep", "-n", "x"], stdin?: "<text>"}` (the doc path is
/// `path`, never in argv). Answers 200 with the tool's own outcome
/// (`exit` 0, 1 = the tool's "no", 2 = refused usage, nothing applied);
/// K2 and session refusals keep their HTTP status, so the permission
/// wrapper can teach them.
fn tool(s: &Arc<Session>, c: &Call) -> Result<CliResponse, VerbError> {
    let argv: Vec<String> = match c.input.body.get("argv") {
        Some(Value::Array(a)) if !a.is_empty() && a.len() <= MAX_ARGV => a
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| VerbError::Response(bad("argv must be a list of strings")))?,
        _ => return Err(VerbError::Response(bad(format!("argv is required: the verb and its flags (at most {MAX_ARGV} words)")))),
    };
    let stdin = match c.input.body.get("stdin") {
        Some(Value::String(s)) => s.clone().into_bytes(),
        None | Some(Value::Null) => Vec::new(),
        Some(_) => return Err(VerbError::Response(bad("stdin must be a string"))),
    };
    let j = join(s, c, Stay::UntilIdle)?;
    let out = match s.run_tool(j.conn, ROOT, &argv, &stdin) {
        Ok(o) => o,
        Err(k2_mpc_base::tools::ToolRunError::Tool(e)) => {
            return Ok(ok(json!({
                "doc": doc_json(c.target),
                "exit": e.exit(),
                "stdout": "",
                "stderr": format!("k2 doc {}: {e}", argv[0]),
            })))
        }
        Err(k2_mpc_base::tools::ToolRunError::Session(e)) => return Err(VerbError::Session(e)),
    };
    let me = c.caller.participant(c.on_behalf);
    let stderr = if out.changed.is_some() {
        format!("{} as {}", out.stderr_summary, me.name)
    } else {
        out.stderr_summary.clone()
    };
    let (stdout, stdout_b64) = match String::from_utf8(out.stdout) {
        Ok(text) => (Value::String(text), Value::Null),
        Err(e) => (Value::Null, Value::String(b64(e.as_bytes()))),
    };
    let fence = out.fence.as_ref().map(|f| {
        json!({
            "begin": f.begin(FENCE_PREFIX, pid_label),
            "end": f.end(FENCE_PREFIX),
            "nonce": f.nonce,
        })
    });
    let mut v = json!({
        "doc": doc_json(c.target),
        "exit": out.exit,
        "stdout": stdout,
        "stderr": stderr,
        "version": out.version,
        "changed": out.changed.as_ref().map(edit_json),
        "fence": fence,
    });
    if !stdout_b64.is_null() {
        v["stdoutB64"] = stdout_b64;
    }
    if let (Some(person), Some(r)) = (c.on_behalf, out.changed.as_ref()) {
        v["onBehalfOf"] = json!(person);
        if let Some(a) = s.approval(j.conn, r.version)? {
            v["approval"] = approval_json(&a);
        }
    }
    Ok(ok(v))
}

/// How a pid reads in a fence's contributors line: `Sam (person)`,
/// `Writer (agent)`, `an outside edit`.
fn pid_label(pid: &str) -> String {
    if pid == k2_mpc_base::session::EXTERNAL_PID {
        return "an outside edit".to_string();
    }
    match Principal::parse(pid) {
        Some(Principal::Person(k)) => {
            let people = k2_core::notify_prefs::People::load().unwrap_or_default();
            format!("{} (person)", k2_core::person_name::effective(&k, &people).display_name)
        }
        Some(p @ Principal::Agent(_)) => {
            format!("{} (agent)", DocCaller::Agent { project_id: pid_project(&p) }.participant(None).name)
        }
        None => pid.to_string(),
    }
}

fn pid_project(p: &Principal) -> String {
    match p {
        Principal::Agent(id) | Principal::Person(id) => id.clone(),
    }
}

/// Standard base64 (non-UTF-8 tool output is rare: docs are UTF-8 text).
fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `{state: clean|dirty|missing|declined, sinceMs?, reason?}`, or null for
/// a session without a file.
fn disk_json(d: Option<&DiskState>) -> Value {
    match d {
        None => Value::Null,
        Some(DiskState::Clean) => json!({ "state": "clean" }),
        Some(DiskState::Dirty { since_ms }) => json!({ "state": "dirty", "sinceMs": since_ms }),
        Some(DiskState::Missing) => json!({ "state": "missing" }),
        Some(DiskState::Declined(reason)) => json!({ "state": "declined", "reason": reason }),
        Some(other) => json!({ "state": format!("{other:?}").to_ascii_lowercase() }),
    }
}

fn approval_json(a: &Approval) -> Value {
    json!({
        "state": match a.state {
            ApprovalState::Pending => "pending",
            ApprovalState::Approved => "approved",
            ApprovalState::Declined => "declined",
        },
        "by": a.by,
        "atMs": a.at_ms,
        "author": a.author,
        "onBehalfOf": a.on_behalf_of,
    })
}

// ── shares (MI25) ─────────────────────────────────────────────────────

/// `shares` / `share` / `unshare`: people only (the owner token, a login
/// at the Admin floor, a shell tab). An agent asks with a Permission Card
/// (`docs.share`); it never shares by itself.
fn shares_route(path: &str, caller: &DocCaller, input: &Input) -> CliResponse {
    let Principal::Person(by) = caller.principal() else {
        return refusal(
            "403 Forbidden",
            "person_only",
            "only a person shares files with another workspace; ask with a permission card",
        );
    };
    let rt = match super::runtime() {
        Ok(rt) => rt,
        Err(u) => return refusal("503 Service Unavailable", "docs_unavailable", u.message()),
    };
    let Some(store) = rt.store() else {
        return refusal("503 Service Unavailable", "docs_unavailable", "live docs are restarting; run the command again");
    };
    let ws = |k: &str| -> Result<Option<String>, CliResponse> {
        match input.str(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(t) => target::workspace_root(&t).map(|(id, _)| Some(id)).map_err(target_refusal),
        }
    };
    if path == SHARES {
        let (owner, with) = match (ws("workspace"), ws("with")) {
            (Ok(o), Ok(w)) => (o, w),
            (Err(r), _) | (_, Err(r)) => return r,
        };
        return match shares::list(&store, owner.as_deref(), with.as_deref()) {
            Ok(list) => ok(json!({ "shares": list })),
            Err(e) => refusal("500 Internal Server Error", "docs_failed", e.to_string()),
        };
    }
    let owner_tok = match input.str("workspace").filter(|s| !s.trim().is_empty()) {
        Some(t) => t,
        None => return bad("workspace is required: the workspace whose file you share"),
    };
    let with_tok = match input.str("with").filter(|s| !s.trim().is_empty()) {
        Some(t) => t,
        None => return bad("with is required: the workspace you share it with"),
    };
    let (owner, root) = match target::workspace_root(&owner_tok) {
        Ok(x) => x,
        Err(r) => return target_refusal(r),
    };
    let (sharee, _) = match target::workspace_root(&with_tok) {
        Ok(x) => x,
        Err(r) => return target_refusal(r),
    };
    if owner == sharee {
        return bad("a workspace always has its own files; name another workspace");
    }
    let raw_path = input.str("path").unwrap_or_default();
    let pid = format!("{}{by}", host::PERSON_PREFIX);
    if path == UNSHARE {
        let rel = match target::share_rel(&root, &raw_path, false) {
            Ok((rel, _)) => rel,
            Err(r) => return target_refusal(r),
        };
        return match shares::remove(&store, &owner, &sharee, &rel) {
            Ok(Some(s)) => {
                host::audit("doc-unshare", &pid, format!("{owner}:{} with={sharee}", show_rel(&s.path)));
                ok(json!({ "removed": s }))
            }
            Ok(None) => refusal("404 Not Found", "share_not_found", "no such share"),
            Err(e) => refusal("500 Internal Server Error", "docs_failed", e.to_string()),
        };
    }
    let (rel, folder) = match target::share_rel(&root, &raw_path, true) {
        Ok(x) => x,
        Err(r) => return target_refusal(r),
    };
    let flag = |k: &str| match input.body.get(k) {
        Some(Value::Bool(b)) => *b,
        _ => input.str(k).is_some_and(|v| v == "1" || v == "true"),
    };
    let s = Share {
        owner: owner.clone(),
        sharee: sharee.clone(),
        path: rel,
        folder,
        write: flag("write"),
        history: flag("history"),
        by: pid.clone(),
        at_ms: host::now_ms(),
    };
    match shares::put(&store, &s) {
        Ok(prev) => {
            host::audit(
                "doc-share",
                &pid,
                format!(
                    "{owner}:{} with={sharee} folder={} write={} history={}",
                    show_rel(&s.path),
                    s.folder,
                    s.write,
                    s.history
                ),
            );
            ok(json!({ "share": s, "replaced": prev }))
        }
        Err(e) => refusal("500 Internal Server Error", "docs_failed", e.to_string()),
    }
}

/// The share route's entry for a permission card's apply (`docs.share`):
/// the same handler, as the answering person (the card checks the Admin
/// floor first).
pub fn share_entry(by: &DocCaller, from: &str, path: &str, with_project: &str, write: bool, history: bool) -> CliResponse {
    let params = HashMap::new();
    let input = Input {
        params: &params,
        body: json!({ "workspace": from, "path": path, "with": with_project, "write": write, "history": history }),
    };
    shares_route(SHARE, by, &input)
}

/// A workspace's handle, else its name, else its id (refusal words and the
/// card's `from=`, which the workspace resolver takes back).
fn workspace_label(project_id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Project::get(&conn, project_id)
        .ok()
        .map(|p| if p.handle.trim().is_empty() { p.name } else { p.handle })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| project_id.to_string())
}

fn show_rel(rel: &str) -> &str {
    if rel.is_empty() {
        "."
    } else {
        rel
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

fn undo_response(c: &Call, out: UndoOutcome) -> CliResponse {
    let t = c.target;
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

    #[test]
    fn an_agent_never_shares_and_a_person_never_acts_on_behalf() {
        let agent = DocCaller::Agent { project_id: "p1".into() };
        let params = HashMap::new();
        for (m, r) in [("GET", SHARES), ("POST", SHARE), ("POST", UNSHARE)] {
            let resp = handle(m, r, &params, br#"{"workspace":"x","path":"a.md","with":"y"}"#, &agent);
            assert!(resp.status.starts_with("403"), "{m} {r}: {}", resp.status);
            assert!(resp.body.contains("person_only"), "{}", resp.body);
        }
        let person = super::super::caller::owner();
        let resp = handle("GET", READ, &params, br#"{"path":"a.md","onBehalfOf":"owner"}"#, &person);
        assert!(resp.status.starts_with("400"), "{}", resp.status);
    }
}
