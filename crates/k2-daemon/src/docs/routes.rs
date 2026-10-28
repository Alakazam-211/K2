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
//! | `/cli/docs/lease` | POST | edit (release: read; break: owner) | tier 0 only: `acquire_lease` … `break_lease` | K-T0-2 |
//!
//! **Any other file** (not Markdown) is a tier-0 doc on the same routes
//! ([`super::tier0`]): presence, a lease, versions, restore, `write --base`
//! with bytes; a verb that needs a text doc refuses with `not_a_text_doc`
//! (exit 2). Byte payloads ride as base64 (`bytesB64`): `write` sends them,
//! `read` and `show` answer with them.
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
    Approval, ApprovalState, ClaimId, ClaimInfo, ClaimPolicy, ConflictRegion, DiskState, EditResult, EditWithError,
    LogItem, LogQuery, Need, Notes, RosterEntry, Role, Session, SessionError, Stay, UndoMode, UndoOutcome, UndoRequest,
    VersionInfo, WriteOutcome,
};
use k2_core::log_debug;
use k2_mpc_base::blob::BlobError;
use k2_mpc_base::store::{DocKey, DocStore, StoreError};
use k2_mpc_base::text::merge::Conflict;
use k2_mpc_base::text::Hunk;
use k2_mpc_base::ParticipantKind;

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

/// Largest `write` body: a whole tier-0 file at the cap
/// ([`super::BLOB_MAX_BYTES`]) as base64, plus room.
pub const MAX_WRITE_BODY: usize = (super::BLOB_MAX_BYTES as usize).div_ceil(3) * 4 + (1 << 20);

/// The body cap for one request to `path` (query string allowed): `write`
/// carries a whole file, the rest at most a Markdown doc.
pub fn max_body(path: &str) -> usize {
    let route = path.split('?').next().unwrap_or(path);
    if route == WRITE {
        MAX_WRITE_BODY.max(MAX_BODY)
    } else {
        MAX_BODY
    }
}

/// Most undo/redo steps one call may ask for.
const MAX_STEPS: usize = 100;

const ROOT: &str = k2_mpc_markdown::ROOT;

/// Every route, for the dispatcher arm and the tests.
pub const ROUTES: &[&str] = &[
    STATUS, READ, WHO, LOG, OPEN, LEAVE, VIEW, CURSOR, EDIT, WRITE, UNDO, REDO, APPROVE, APPROVALS, SHARES, SHARE,
    UNSHARE, FLUSH, TOOL, VERSIONS, SHOW, DIFF, RESTORE, NAME, REVERT, MV, BLAME, LEASE, CLAIM, ACTIVITY,
];

/// Tier 0's lease (K-T0-2): acquire, renew, release, break.
pub const LEASE: &str = "/cli/docs/lease";
/// Agent presence (kit amendment 13): take, renew, release a claim.
pub const CLAIM: &str = "/cli/docs/claim";
/// Agent presence: the doc's recent reads, edits and claims.
pub const ACTIVITY: &str = "/cli/docs/activity";

pub const VERSIONS: &str = "/cli/docs/versions";
pub const SHOW: &str = "/cli/docs/show";
pub const DIFF: &str = "/cli/docs/diff";
pub const RESTORE: &str = "/cli/docs/restore";
pub const NAME: &str = "/cli/docs/name";
pub const REVERT: &str = "/cli/docs/revert";
pub const MV: &str = "/cli/docs/mv";
pub const BLAME: &str = "/cli/docs/blame";

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
            | "/cli/docs/name"
            | "/cli/docs/mv"
            | "/cli/docs/lease"
            | "/cli/docs/claim"
    ) || is_edit_route(path)
}

fn is_edit_route(path: &str) -> bool {
    matches!(
        path,
        "/cli/docs/edit"
            | "/cli/docs/write"
            | "/cli/docs/undo"
            | "/cli/docs/redo"
            | "/cli/docs/restore"
            | "/cli/docs/revert"
    )
}

/// Request input: the JSON body over the query params.
pub(super) struct Input<'a> {
    pub(super) params: &'a HashMap<String, String>,
    pub(super) body: Value,
}

impl Input<'_> {
    pub(super) fn str(&self, k: &str) -> Option<String> {
        match self.body.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => self.params.get(k).cloned(),
        }
    }

    pub(super) fn u64(&self, k: &str) -> Result<Option<u64>, CliResponse> {
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

pub(super) fn bad(msg: impl Into<String>) -> CliResponse {
    with_exit(refusal("400 Bad Request", "bad_request", msg), k2_mpc_base::tools::exit::USAGE)
}

impl Input<'_> {
    /// A boolean flag: JSON `true`, or `1` / `true` in the query.
    pub(super) fn flag(&self, k: &str) -> bool {
        match self.body.get(k) {
            Some(Value::Bool(b)) => *b,
            _ => self.str(k).is_some_and(|v| v == "1" || v == "true"),
        }
    }
}

/// A person on this box, by login (or `owner`, or `person:<key>`), as a pid.
fn person_pid(named: &str) -> Result<String, CliResponse> {
    let login = named.trim();
    let login = login.strip_prefix(host::PERSON_PREFIX).unwrap_or(login);
    let key = k2_core::notify_prefs::People::load()
        .map(|p| p.person_key(login))
        .unwrap_or_else(|_| login.to_ascii_lowercase());
    if host::person_exists(&key) {
        Ok(format!("{}{key}", host::PERSON_PREFIX))
    } else {
        Err(refusal("404 Not Found", "person_not_found", format!("no person '{named}' on this server")))
    }
}

/// Anyone a `log` filter names: `me`, a pid as is, a person by login, or an
/// agent by its workspace.
pub(super) fn any_pid(named: &str, caller: &DocCaller) -> Result<String, CliResponse> {
    let n = named.trim();
    if n.eq_ignore_ascii_case("me") {
        return Ok(caller.pid());
    }
    if Principal::parse(n).is_some() || n.starts_with("kit:") {
        return Ok(n.to_string());
    }
    if let Ok(p) = person_pid(n) {
        return Ok(p);
    }
    match target::workspace_root(n) {
        Ok((id, _)) => Ok(format!("{}{id}", host::AGENT_PREFIX)),
        Err(_) => Err(refusal("404 Not Found", "participant_not_found", format!("no person or agent '{named}' here"))),
    }
}

pub(super) fn ok(mut v: Value) -> CliResponse {
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
    // Provenance (who put restored text back) is the kit's alone: it stamps
    // it in its own undo, redo and revert. A request naming it is refused
    // before anything reaches the kit.
    if let Some(k) = provenance_field(params, &body) {
        return with_exit(
            refusal(
                "400 Bad Request",
                "provenance_is_kit_only",
                format!("`{k}` can't be set: who restored text is recorded by the docs kit itself, in its own undo, redo and revert"),
            ),
            k2_mpc_base::tools::exit::USAGE,
        );
    }
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
    // History reads need the history right (K-PERM-9); the kit checks it
    // too, this gives the refusal its own code first.
    if matches!(path, LOG | VERSIONS | SHOW | DIFF | RESTORE | BLAME) && !rights.history {
        return with_exit(
            refusal("403 Forbidden", "history_forbidden", "you may read this doc but not its history"),
            k2_mpc_base::tools::exit::REFUSED,
        );
    }
    let call = Call { caller, on_behalf: on_behalf.as_deref(), target: &target, input: &input, rights };
    if target.tier == target::Tier::Blob {
        return super::tier0::handle(path, &rt, &call, None);
    }
    // A text file a pack refuses (too_large, not_utf8, binary,
    // mixed_line_endings) is a tier-0 doc instead, with the reason on every
    // answer; so is one already stored as a tier-0 doc (it stays one).
    let session = match open(&rt, &target) {
        Ok(s) => s,
        Err(SessionError::Declined(d)) => {
            let why = super::tier0::Fallback { code: d.code.to_string(), message: d.message.clone() };
            return super::tier0::handle(path, &rt, &call, Some(&why));
        }
        Err(SessionError::PackMismatch { stored, .. }) if stored == k2_mpc_base::store::BLOB_DOC_PACK => {
            let why = super::tier0::Fallback {
                code: "stored_as_any_file".into(),
                message: "it was opened as an any-file doc earlier (its text was refused then), and stays one".into(),
            };
            return super::tier0::handle(path, &rt, &call, Some(&why));
        }
        Err(e) => return session_error(&e),
    };
    if path == LEASE {
        return with_exit(
            refusal(
                "400 Bad Request",
                "no_lease_on_text",
                format!(
                    "{} is a live text doc: everyone's edits merge live, so it has no lease. Leases are for files that aren't text (k2 doc lease photo.png)",
                    target.rel
                ),
            ),
            k2_mpc_base::tools::exit::USAGE,
        );
    }
    if path == MV {
        let s = &session;
        return mv(
            &rt,
            &call,
            || {
                let j = join(s, &call, Stay::UntilIdle)?;
                s.flush(j.conn).map(|_| ())
            },
            |p: &std::path::Path| s.file_moved(p).map(|_| ()),
        );
    }
    let r = verb(path, &session, &call);
    let (r, s) = match r {
        // The pool unloaded the session between open and the call (it was
        // idle and nobody held it): open again, once.
        Err(VerbError::Session(SessionError::Unloaded)) => match open(&rt, &target) {
            Ok(s) => (verb(path, &s, &call), s),
            Err(e) => return session_error(&e),
        },
        r => (r, session),
    };
    match r {
        Ok(resp) => resp,
        // An agent's edit (or claim) ran into someone's claim: it waits in
        // line, and the bounce reaches its terminal and Chatter.
        Err(VerbError::Session(e @ SessionError::Claimed { .. })) => super::claims::bounced(&s, &call, &e),
        Err(e) => e.into_response(),
    }
}

/// The claim policy for this caller's edits: agents respect each other's
/// claims (a bounce, exit 5); a person is never held back by one.
fn policy(c: &Call) -> ClaimPolicy {
    match c.caller {
        DocCaller::Agent { .. } => ClaimPolicy::Respect,
        DocCaller::Person { .. } => ClaimPolicy::Ignore,
    }
}

/// Heads-up notes for the CLI's stderr: one `note: …` line each.
fn notes_text(n: &Notes) -> Value {
    if n.is_empty() {
        Value::Null
    } else {
        Value::String(n.to_string())
    }
}

fn claim_json(c: &ClaimInfo) -> Value {
    json!({
        "id": c.id.to_string(),
        "pid": c.pid,
        "name": c.name,
        "label": pid_label(&c.pid),
        "lines": [c.lines.first, c.lines.last],
        "takenMs": c.taken_ms,
        "expiresMs": c.expires_ms,
        "remainingMs": c.remaining_ms,
        "note": c.note,
        "hold": c.hold,
        "forEdit": c.for_edit,
        "waiters": c.waiters,
    })
}

/// `claim`: `action` take (default: `lines` a,b, `ttlMs`, `note`, `hold`) |
/// renew (`id`, `ttlMs`, `hold`) | release (`id`; none: all yours). Needs
/// edit (release: read).
fn claim(s: &Session, c: &Call) -> Result<CliResponse, VerbError> {
    let input = c.input;
    let ttl = input.u64("ttlMs").map_err(VerbError::Response)?;
    let id = match input.str("id").filter(|i| !i.trim().is_empty()) {
        Some(i) => Some(
            ClaimId::parse(i.trim()).ok_or_else(|| VerbError::Response(bad(format!("{i:?} is not a claim id (claim-12 or 12)"))))?,
        ),
        None => None,
    };
    let hold = match input.body.get("hold") {
        Some(Value::Bool(b)) => Some(*b),
        _ => input.str("hold").map(|h| h == "1" || h == "true"),
    };
    let action = input.str("action").map(|a| a.trim().to_ascii_lowercase()).unwrap_or_else(|| "take".into());
    let j = join(s, c, Stay::UntilIdle)?;
    match action.as_str() {
        "take" | "" => {
            let lines = input.str("lines").ok_or_else(|| VerbError::Response(bad("lines is required (a,b)")))?;
            let (first, last) = parse_lines(&lines).map_err(|m| VerbError::Response(bad(m)))?;
            let note = input.str("note").map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            let info = s.claim(j.conn, LineRange { root: ROOT.to_string(), first, last }, ttl, note, hold.unwrap_or(false))?;
            Ok(ok(json!({ "doc": doc_json(c.target), "action": "take", "claim": claim_json(&info) })))
        }
        "renew" => {
            let info = s.renew_claim(j.conn, id, ttl, hold)?;
            Ok(ok(json!({ "doc": doc_json(c.target), "action": "renew", "claim": claim_json(&info) })))
        }
        "release" => {
            let ids = s.release_claim(j.conn, id)?;
            let exit = if ids.is_empty() { k2_mpc_base::tools::exit::NO } else { k2_mpc_base::tools::exit::OK };
            Ok(ok(json!({
                "doc": doc_json(c.target),
                "action": "release",
                "released": ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "exit": exit,
            })))
        }
        other => Err(VerbError::Response(bad(format!("action must be take, renew or release (got {other})")))),
    }
}

/// Fields that would name a restore's provenance (the kit's spec U23):
/// refused on every route, in the query or the JSON body, at any depth.
const PROVENANCE_FIELDS: &[&str] =
    &["restores", "restored", "restoredBy", "restoredIn", "restored_by", "restored_in", "provenance", "root", "roots"];

/// The first provenance field a request names, if any.
pub(super) fn provenance_field(params: &HashMap<String, String>, body: &Value) -> Option<String> {
    fn walk(v: &Value) -> Option<String> {
        match v {
            Value::Object(o) => o.iter().find_map(|(k, v)| {
                if PROVENANCE_FIELDS.iter().any(|f| f.eq_ignore_ascii_case(k)) {
                    Some(k.clone())
                } else {
                    walk(v)
                }
            }),
            Value::Array(a) => a.iter().find_map(walk),
            _ => None,
        }
    }
    params.keys().find(|k| PROVENANCE_FIELDS.iter().any(|f| f.eq_ignore_ascii_case(k))).cloned().or_else(|| walk(body))
}

/// One doc call, resolved.
pub(super) struct Call<'a> {
    pub(super) caller: &'a DocCaller,
    /// `person:<key>` when an agent acts for that person.
    pub(super) on_behalf: Option<&'a str>,
    pub(super) target: &'a Target,
    pub(super) input: &'a Input<'a>,
    pub(super) rights: DocRights,
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
fn open(rt: &Arc<super::Runtime>, t: &Target) -> Result<Arc<Session>, SessionError> {
    let key = t.key();
    if let Some(s) = rt.get(&key) {
        return Ok(s);
    }
    if rt.get_blob(&key).is_some() {
        return Err(SessionError::PackMismatch {
            stored: k2_mpc_base::store::BLOB_DOC_PACK.to_string(),
            pack: t.tier.pack().to_string(),
        });
    }
    let pack: Arc<dyn k2_mpc_base::session::Pack> = match t.tier {
        target::Tier::Text(_) => Arc::new(k2_mpc_text::TextPack::for_path(&t.rel).unwrap_or(k2_mpc_text::PACK)),
        _ => Arc::new(k2_mpc_markdown::PACK),
    };
    rt.open(&key, &t.abs, &t.root, pack)
}

pub(super) enum VerbError {
    Session(SessionError),
    Response(CliResponse),
}

impl From<SessionError> for VerbError {
    fn from(e: SessionError) -> Self {
        VerbError::Session(e)
    }
}

impl VerbError {
    pub(super) fn into_response(self) -> CliResponse {
        match self {
            VerbError::Response(r) => r,
            VerbError::Session(e) => session_error(&e),
        }
    }
}

/// `r` with `error.exit` set: the CLI exits with it (the kit's K-EXIT-1
/// table, `tools::session_exit`, for every session error).
pub(super) fn with_exit(mut r: CliResponse, exit: u8) -> CliResponse {
    if let Ok(mut v) = serde_json::from_str::<Value>(&r.body) {
        if let Some(e) = v.get_mut("error").and_then(Value::as_object_mut) {
            e.insert("exit".into(), json!(exit));
            r.body = v.to_string();
        }
    }
    r
}

/// The kit's errors on the wire (SESSIONS.md "Errors"), each with its exit
/// code (`error.exit`).
pub fn session_error(e: &SessionError) -> CliResponse {
    with_exit(session_error_body(e), k2_mpc_base::tools::session_exit(e))
}

/// `4m 10s`, `35s`.
fn human_ms(ms: u64) -> String {
    let s = ms.div_ceil(1000);
    match s {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s / 60 % 60),
    }
}

fn session_error_body(e: &SessionError) -> CliResponse {
    match e {
        // Tier 0 (kit amendment 3).
        SessionError::Leased { holder, name, expires_ms } => {
            let left = expires_ms.saturating_sub(host::now_ms());
            CliResponse {
                status: "409 Conflict",
                content_type: "application/json",
                body: json!({
                    "ok": false,
                    "error": {
                        "code": "leased",
                        "hint": format!(
                            "leased by {} for another {}: nothing was written. Try again after it expires, or ask them to let go (k2 doc lease --release)",
                            pid_label(holder),
                            human_ms(left)
                        ),
                        "holder": holder,
                        "holderName": name,
                        "expiresMs": expires_ms,
                        "retryAfterMs": left,
                    },
                })
                .to_string(),
            }
        }
        // Agent presence (kit amendment 13). An agent's bounce is answered
        // by `claims::bounced` (it waits in line); this is everyone else's.
        SessionError::Claimed { id, by, lines, remaining_ms, note, .. } => CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: json!({
                "ok": false,
                "error": {
                    "code": "claimed",
                    "hint": format!(
                        "lines {}–{} are claimed by {}{} for another {}; nothing applied",
                        lines.0,
                        lines.1,
                        pid_label(by),
                        note.as_deref().map(|n| format!(" (\"{n}\")")).unwrap_or_default(),
                        human_ms(*remaining_ms)
                    ),
                    "claim": id.to_string(),
                    "by": by,
                    "lines": [lines.0, lines.1],
                    "remainingMs": remaining_ms,
                    "retryAfterMs": remaining_ms,
                },
            })
            .to_string(),
        },
        SessionError::NoClaim(id) => refusal(
            "404 Not Found",
            "no_claim",
            match id {
                Some(id) => format!("no live claim {id} here (it ended); k2 doc who lists the claims"),
                None => "you hold no claim here".to_string(),
            },
        ),
        SessionError::ClaimContended { id, waiters, remaining_ms } => refusal(
            "409 Conflict",
            "claim_contended",
            format!(
                "{waiters} waiting for {id}: you can't extend it or hold it now (it ends in {}); finish, or release it",
                human_ms(*remaining_ms)
            ),
        ),
        SessionError::BadClaim(m) => bad(m.clone()),
        SessionError::NoLease => refusal(
            "409 Conflict",
            "no_lease",
            "nobody holds a lease on it (a daemon restart drops leases); take one with k2 doc lease",
        ),
        SessionError::NoBlobStore => refusal(
            "503 Service Unavailable",
            "any_file_off",
            "any-file docs are off on this server (their store didn't open; see the daemon log)",
        ),
        SessionError::Blob(BlobError::TooLarge { limit }) => refusal(
            "422 Unprocessable Entity",
            "too_large",
            format!("that is over the {} limit for any-file docs", target::human_bytes(*limit)),
        ),
        SessionError::Blob(err) => refusal("500 Internal Server Error", "blob_error", format!("the stored file failed: {err}")),
        SessionError::Store(StoreError::NoVersion(_, v)) => {
            refusal("404 Not Found", "no_such_version", format!("no version {v} of this doc (k2 doc versions)"))
        }
        SessionError::Store(StoreError::PurgedVersion(_, v)) => {
            refusal("404 Not Found", "purged_version", format!("version {v} was purged; its content is gone"))
        }
        SessionError::Store(StoreError::CurrentVersion(_, v)) => {
            refusal("409 Conflict", "current_version", format!("version {v} is the current one; write or restore another first"))
        }
        SessionError::Held { reason } => refusal(
            "409 Conflict",
            "held",
            format!("held ({reason}): too many of your edits for this person wait for their approval; ask them to approve or decline some"),
        ),
        SessionError::RevertTooOld { seq } => refusal(
            "409 Conflict",
            "revert_too_old",
            format!("change {seq} is older than the revert window; restore an older version instead (k2 doc restore --version V)"),
        ),
        SessionError::NoSuchChange(seq) => refusal("404 Not Found", "no_such_change", format!("no change {seq} in this doc")),
        SessionError::Deleted => refusal("404 Not Found", "doc_deleted", "this doc was removed (its workspace is gone)"),
        SessionError::Store(err) => refusal("500 Internal Server Error", "store_error", format!("the docs store failed: {err}")),
        SessionError::Forbidden { need } => match need {
            Need::Read => refusal("403 Forbidden", "doc_forbidden", "you can't read this doc"),
            Need::Edit => refusal("403 Forbidden", "doc_forbidden", "you can't edit this doc (read-only)"),
            Need::History => refusal("403 Forbidden", "history_forbidden", "you may read this doc but not its history"),
            Need::Approver => refusal(
                "403 Forbidden",
                "not_the_approver",
                "only the person this change was made for can approve or decline it",
            ),
            Need::AuthorOrOwner => refusal(
                "403 Forbidden",
                "not_author_or_owner",
                "only the change's author, the person it was made for, or an owner can revert it",
            ),
            Need::Owner => refusal(
                "403 Forbidden",
                "not_the_owner",
                "only the doc's owner (the box owner or an Admin login) can do that, e.g. break someone else's lease",
            ),
            _ => refusal("403 Forbidden", "doc_forbidden", "refused"),
        },
        SessionError::NotJoined { .. } => refusal("409 Conflict", "not_joined", "your connection to this doc closed; run the command again"),
        SessionError::Stale { expected, actual } => CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "stale", "hint": format!("doc changed since {expected} (now {actual}): nothing applied. Read it again (k2 doc cat) and redo the edit from what you see"), "expected": expected, "actual": actual } }).to_string(),
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
            // Any verb joins (as the kit's reference binding does): status
            // makes the caller present too.
            join(s, c, Stay::UntilIdle)?;
            let st = s.status()?;
            Ok(ok(json!({
                "doc": doc_json(t),
                "tier": 1,
                "pack": t.tier.pack(),
                "family": t.tier.family(),
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
        CLAIM => claim(s, c),
        ACTIVITY => {
            let n = input.u64("n").map_err(VerbError::Response)?.unwrap_or(20).clamp(1, 256) as usize;
            let j = join(s, c, Stay::UntilIdle)?;
            let rows = s.activity(j.conn, n)?;
            let rendered: String = rows
                .iter()
                .map(|a| {
                    format!(
                        "{}  {:<28} {:<12} {:<16}{}\n",
                        super::tier0::utc(a.at_ms),
                        pid_label(&a.pid),
                        a.kind.as_str(),
                        a.verb,
                        a.lines.as_ref().map(|l| format!(" lines {}-{}", l.first, l.last)).unwrap_or_default()
                    )
                })
                .collect();
            Ok(ok(json!({
                "doc": doc_json(t),
                "activity": rows.iter().map(|a| json!({
                    "pid": a.pid,
                    "name": a.name,
                    "label": pid_label(&a.pid),
                    "kind": a.kind.as_str(),
                    "verb": a.verb,
                    "lines": a.lines.as_ref().map(|l| json!([l.first, l.last])),
                    "atMs": a.at_ms,
                    "claim": a.claim.map(|c| c.to_string()),
                })).collect::<Vec<_>>(),
                "rendered": rendered,
            })))
        }
        BLAME => {
            // P22, `blame [-L a,b]`: read + history (gated above and by the
            // kit). Each line's most recent author by change id.
            let range = match input.str("lines").filter(|l| !l.trim().is_empty()) {
                Some(l) => {
                    let (first, last) = parse_lines(&l).map_err(|m| VerbError::Response(bad(m)))?;
                    Some(LineRange { root: ROOT.to_string(), first, last })
                }
                None => None,
            };
            let j = join(s, c, Stay::UntilIdle)?;
            let b = s.blame(j.conn, ROOT, range)?;
            Ok(ok(json!({
                "doc": doc_json(t),
                "version": b.version,
                "lines": b.lines.iter().map(|l| json!({
                    "line": l.line,
                    "text": l.text,
                    "author": l.author,
                    "authorLabel": pid_label(&l.author),
                    "change": l.change,
                    "kind": l.kind.as_ref().map(lower),
                    "via": l.via,
                    "atMs": l.at_ms,
                    "onBehalfOf": l.on_behalf_of,
                    "approval": l.approval.as_ref().map(lower),
                    "erased": l.erased,
                    // Text an undo, redo or revert put back: blamed on its
                    // original author; who put it back, and in which change.
                    "restoredBy": l.restored_by,
                    "restoredByLabel": l.restored_by.as_deref().map(pid_label),
                    "restoredIn": l.restored_in,
                })).collect::<Vec<_>>(),
                "rendered": b.render(|pid| Some(pid_label(pid))),
                "porcelain": b.porcelain(),
            })))
        }
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
            super::claims::record_read(c.caller, &t.key(), r.version);
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
            Ok(ok(json!({
                "doc": doc_json(t),
                "tier": 1,
                "pack": t.tier.pack(),
                "family": t.tier.family(),
                "version": j.version,
                "client": j.client,
                "rights": rights.json(),
            })))
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
            let mode = match input.str("mode").as_deref().map(str::trim) {
                None | Some("") | Some("all") => UndoMode::All,
                Some("partial") => UndoMode::Partial,
                Some("drop") => UndoMode::Drop,
                Some(other) => return Err(VerbError::Response(bad(format!("mode must be all, partial or drop (got {other})")))),
            };
            let mut req = UndoRequest::steps(steps).mode(mode);
            if let Some(p) = input.str("forPerson").filter(|p| !p.trim().is_empty()) {
                req = req.for_person(person_pid(&p).map_err(VerbError::Response)?);
            }
            let j = join(s, c, Stay::UntilIdle)?;
            let out = if path == UNDO { s.undo_with(j.conn, &req)? } else { s.redo_with(j.conn, &req)? };
            own_change(c, &out);
            Ok(undo_response(c, out))
        }
        REVERT => {
            let change = input
                .u64("change")
                .map_err(VerbError::Response)?
                .ok_or_else(|| VerbError::Response(bad("change is required: the change number from k2 doc log")))?;
            let partial = input.flag("partial");
            let j = join(s, c, Stay::UntilIdle)?;
            let out = s.revert(j.conn, change, partial)?;
            own_change(c, &out);
            Ok(undo_response(c, out))
        }
        RESTORE => {
            let v = input
                .u64("version")
                .map_err(VerbError::Response)?
                .ok_or_else(|| VerbError::Response(bad("version is required: a version id from k2 doc versions")))?;
            let j = join(s, c, Stay::UntilIdle)?;
            let r = s.restore(j.conn, v)?;
            super::claims::record_read(c.caller, &t.key(), r.version);
            edit_reply(s, c, &r, Some(("restored", json!(v))))
        }
        NAME => {
            let name = input
                .str("name")
                .filter(|n| !n.trim().is_empty())
                .ok_or_else(|| VerbError::Response(bad("name is required")))?;
            let v = input.u64("version").map_err(VerbError::Response)?;
            let j = join(s, c, Stay::UntilIdle)?;
            let id = s.name_version(j.conn, name.trim(), v)?;
            Ok(ok(json!({ "doc": doc_json(t), "version": id, "name": name.trim() })))
        }
        VERSIONS => {
            let j = join(s, c, Stay::UntilIdle)?;
            let vs = s.versions(j.conn)?;
            Ok(ok(json!({ "doc": doc_json(t), "versions": vs.iter().map(version_json).collect::<Vec<_>>() })))
        }
        LOG => {
            let mut q = LogQuery::default();
            if let Some(by) = input.str("by").filter(|p| !p.trim().is_empty()) {
                q = q.by(any_pid(&by, c.caller).map_err(VerbError::Response)?);
            }
            if let Some(since) = input.u64("sinceMs").map_err(VerbError::Response)? {
                q = q.since_ms(since);
            }
            if let Some(n) = input.u64("n").map_err(VerbError::Response)? {
                q = q.limit(n as usize);
            }
            if let Some(p) = input.str("forPerson").filter(|p| !p.trim().is_empty()) {
                q = q.on_behalf_of(any_pid(&p, c.caller).map_err(VerbError::Response)?);
            }
            if input.flag("pending") {
                q = q.pending();
            }
            let j = join(s, c, Stay::UntilIdle)?;
            let items = s.log(j.conn, &q)?;
            Ok(ok(json!({ "doc": doc_json(t), "log": items.iter().map(log_item_json).collect::<Vec<_>>() })))
        }
        SHOW => {
            let v = input
                .u64("version")
                .map_err(VerbError::Response)?
                .ok_or_else(|| VerbError::Response(bad("version is required")))?;
            let j = join(s, c, Stay::UntilIdle)?;
            let roots = s.show(j.conn, v)?;
            let text = roots.into_iter().find(|(r, _)| r == ROOT).map(|(_, t)| t).unwrap_or_default();
            Ok(ok(json!({ "doc": doc_json(t), "version": v, "text": text })))
        }
        DIFF => {
            // `fromVersion`, not `from`: the cell socket stamps `from` (the
            // sender) on every call.
            let from = input
                .u64("fromVersion")
                .map_err(VerbError::Response)?
                .ok_or_else(|| VerbError::Response(bad("fromVersion is required: a version id")))?;
            let to = input.u64("toVersion").map_err(VerbError::Response)?;
            let context = input.u64("context").map_err(VerbError::Response)?.unwrap_or(3).min(1000) as usize;
            let j = join(s, c, Stay::UntilIdle)?;
            let d = s.diff_version(j.conn, ROOT, from, to, context)?;
            Ok(ok(json!({ "doc": doc_json(t), "from": from, "to": to, "text": d.text, "differs": d.differs, "exit": d.exit() })))
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

/// An agent's own undo, redo or revert: it knows that change, so its next
/// edit is checked from the version after it.
fn own_change(c: &Call, out: &UndoOutcome) {
    if let UndoOutcome::Applied { result, .. } = out {
        super::claims::record_read(c.caller, &c.target.key(), result.version);
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
    // `ifVersion`, else (an agent) the version it last read: positions are
    // from that version, and the edit applies only if the doc is still
    // there (the live text under the lock must be the text read at V).
    let key = c.target.key();
    let if_version = match c.input.u64("ifVersion").map_err(VerbError::Response)? {
        Some(v) => Some(v),
        None => super::claims::last_read(c.caller, &key),
    };
    let j = join(s, c, Stay::UntilIdle)?;
    let pol = policy(c);
    const MOVED: &str = "\u{0}moved";
    let (expected, at_v) = match if_version {
        Some(v) => {
            let r = s.read(j.conn)?;
            if r.version != v {
                return Err(VerbError::Session(SessionError::Stale { expected: v, actual: r.version }));
            }
            let text = r.text_by_root.iter().find(|(root, _)| root == ROOT).map(|(_, t)| t.clone()).unwrap_or_default();
            (Some(v), Some(text))
        }
        None => (None, None),
    };
    // Positions against the live text, computed and applied under the
    // session lock, with the caller's claim policy.
    let result = match s.edit_with_policy(j.conn, ROOT, pol, |live| {
        if at_v.as_deref().is_some_and(|t| t != live) {
            return Err(MOVED.to_string());
        }
        hunks_for(live, &ops).map(|h| splice(live, &h))
    }) {
        Ok(Some(r)) => r,
        Ok(None) => {
            let mut r = EditResult::default();
            r.version = s.status()?.version;
            r
        }
        Err(EditWithError::Compute(m)) if m == MOVED => {
            let actual = s.status()?.version;
            return Err(VerbError::Session(SessionError::Stale { expected: expected.unwrap_or(actual), actual }));
        }
        Err(EditWithError::Compute(m)) => return Err(VerbError::Response(bad(m))),
        Err(EditWithError::Session(e)) => return Err(VerbError::Session(e)),
    };
    super::claims::record_read(c.caller, &key, result.version);
    edit_reply(s, c, &result, None)
}

fn write(s: &Arc<Session>, c: &Call) -> Result<CliResponse, VerbError> {
    // `text`, or `bytesB64` (what `k2 doc write` sends for every doc: the
    // same bytes for any file) holding UTF-8.
    let text = match (c.input.body.get("text"), c.input.body.get("bytesB64")) {
        (Some(Value::String(s)), _) => s.clone(),
        (None, Some(Value::String(b))) => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b.as_bytes())
                .map_err(|e| VerbError::Response(bad(format!("bytesB64 is not base64: {e}"))))?;
            String::from_utf8(bytes).map_err(|_| VerbError::Response(bad("a Markdown doc's new text must be UTF-8")))?
        }
        _ => return Err(VerbError::Response(bad("text is required (a string in the JSON body)"))),
    };
    // A text file's `cat` gives its exact bytes, BOM included; edit verbs
    // never see the BOM, so a body made from `cat` drops its own (else it
    // would be written as a second one). The text pack's HOST-INTEGRATION §6.
    let text = match c.target.tier {
        target::Tier::Text(_) => text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text),
        _ => text,
    };
    let base = c
        .input
        .str("base")
        .ok_or_else(|| VerbError::Response(bad("base is required: a version you read, or \"live\"")))?;
    let j = join(s, c, Stay::UntilIdle)?;
    let key = c.target.key();
    if base.trim() == "live" {
        let r = s.replace_text_with(j.conn, ROOT, &text, policy(c))?;
        super::claims::record_read(c.caller, &key, r.version);
        let warning = matches!(c.caller, DocCaller::Agent { .. }).then(|| {
            (
                "warning",
                json!("--base live replaced whatever was there, including other people's edits since you read it; use --base <version> to merge"),
            )
        });
        return edit_reply(s, c, &r, warning);
    }
    let base: u64 = base.trim().parse().map_err(|_| VerbError::Response(bad("base must be a version number or \"live\"")))?;
    let report = s.write_base_with(j.conn, ROOT, base, &text, policy(c))?;
    let out = report.outcome;
    let exit = out.exit();
    match out {
        WriteOutcome::Applied(r) => {
            super::claims::record_read(c.caller, &key, r.version);
            edit_reply(s, c, &r, Some(("notes", notes_text(&report.notes))))
        }
        WriteOutcome::Conflicts(conf) => {
            // Who claimed or last edited each conflicting region.
            let who: Vec<String> = report.conflict_notes.iter().map(|(i, n)| format!("conflict {}: {n}", i + 1)).collect();
            let mut r = conflicts_response(
                c.target,
                None,
                conflicts_json(&conf),
                "conflicts",
                "your change overlaps edits made since; nothing was applied",
            );
            if !who.is_empty() {
                if let Ok(mut v) = serde_json::from_str::<Value>(&r.body) {
                    v["notes"] = json!(who.join("\n"));
                    r.body = v.to_string();
                }
            }
            Ok(r)
        }
        WriteOutcome::BaseExpired => Ok(with_exit(
            refusal(
                "409 Conflict",
                "base_expired",
                format!("version {base} is no longer kept as a base; read the doc again and retry"),
            ),
            exit,
        )),
    }
}

/// `mv` (P21): rename the doc's file within its workspace, keeping the doc
/// (history, presence, undo). Needs edit on the doc and at the new name.
/// Unwritten edits are flushed to the old name first; the file is renamed;
/// the store record and session move (`rekey`, `file_moved`). A failed
/// rekey renames the file back. File shares of the old name follow it
/// (K-PERM-11); a folder share covers the new name only if it is inside.
///
/// `flush` writes the doc's unwritten changes to the old name (it joins);
/// `moved` tells the session its file's new path (text or tier 0).
pub(super) fn mv(
    rt: &Arc<super::Runtime>,
    c: &Call,
    flush: impl FnOnce() -> Result<(), SessionError>,
    moved: impl FnOnce(&std::path::Path) -> Result<(), SessionError>,
) -> CliResponse {
    let t = c.target;
    if !c.rights.edit {
        return session_error(&SessionError::Forbidden { need: Need::Edit });
    }
    let Some(to) = c.input.str("to") else {
        return bad("to is required: the new name (relative to the workspace)");
    };
    let (new_rel, new_abs) = match target::resolve_new(t, &to) {
        Ok(x) => x,
        Err(r) => return target_refusal(r),
    };
    let new_key = DocKey { space: t.project_id.clone(), path: new_rel.clone() };
    let store = rt.store();
    if !host::rights_for(&c.caller.pid(), &new_key, store.as_deref()).edit {
        return with_exit(
            refusal("403 Forbidden", "doc_forbidden", format!("you can't create {new_rel} there")),
            k2_mpc_base::tools::exit::REFUSED,
        );
    }
    if let Err(e) = flush() {
        return session_error(&e);
    }
    if let Err(e) = std::fs::rename(&t.abs, &new_abs) {
        return with_exit(
            refusal("500 Internal Server Error", "disk_error", format!("rename failed: {e}")),
            k2_mpc_base::tools::exit::NO,
        );
    }
    if let Err(e) = rt.rekey(&t.key(), &new_key, &new_abs) {
        let _ = std::fs::rename(&new_abs, &t.abs);
        return session_error(&e);
    }
    if let Err(e) = moved(&new_abs) {
        log_debug!("[docs] WARN: file_moved after mv to {new_rel}: {e}");
    }
    let mut moved_shares = 0usize;
    if let Some(store) = store.as_deref() {
        if let Ok(list) = shares::list(store, Some(&t.project_id), None) {
            for sh in list.into_iter().filter(|sh| !sh.folder && sh.path == t.rel) {
                if shares::remove(store, &sh.owner, &sh.sharee, &sh.path).is_ok() {
                    let moved = Share { path: new_rel.clone(), ..sh };
                    if shares::put(store, &moved).is_ok() {
                        moved_shares += 1;
                    }
                }
            }
        }
    }
    ok(json!({
        "doc": { "workspace": t.project_id, "path": new_rel },
        "from": t.rel,
        "sharesMoved": moved_shares,
    }))
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
/// The Markdown pack's verb registry (the kit's text verbs and the pack's).
fn registry() -> &'static k2_mpc_base::parity::Registry {
    static R: std::sync::OnceLock<k2_mpc_base::parity::Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| k2_mpc_base::parity::Registry::for_pack(&k2_mpc_markdown::PACK).expect("the Markdown pack's verb registry"))
}

/// True when `argv` is an edit verb that `run_tool` runs (the kit's or the
/// pack's): the ones `--if-version` guards.
pub(super) fn is_tool_edit(argv: &[String]) -> bool {
    use k2_mpc_base::parity::{Api, VerbKind};
    registry()
        .get(&k2_mpc_base::parity::verb_form(argv))
        .is_some_and(|v| v.kind == VerbKind::Edit && v.api.contains(&Api::RunTool))
}

fn tool(s: &Arc<Session>, c: &Call) -> Result<CliResponse, VerbError> {
    let mut argv: Vec<String> = match c.input.body.get("argv") {
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
    let key = c.target.key();
    let is_edit = is_tool_edit(&argv);
    // An agent's edit verb is guarded by the version it last read (unless
    // it named one): an edit from a stale read is refused, exit 1.
    let guarded = if is_edit { super::claims::guard_argv(c.caller, &key, &mut argv) } else { None };
    let j = join(s, c, Stay::UntilIdle)?;
    let opts = k2_mpc_base::tools::ToolOptions::new().policy(policy(c));
    let out = match s.run_tool_with(j.conn, ROOT, &argv, &stdin, &opts) {
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
    if !is_edit || out.changed.is_some() {
        super::claims::record_read(c.caller, &key, out.version);
    }
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
        // Heads-up notes and `--for-edit`'s outcome: stderr lines.
        "notes": notes_text(&out.notes),
        "forEdit": out.for_edit.as_ref().map(|f| f.to_string()),
        "ifVersion": guarded,
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
pub(super) fn pid_label(pid: &str) -> String {
    if pid == k2_mpc_base::session::EXTERNAL_PID {
        return "an outside edit".to_string();
    }
    if pid == k2_mpc_base::session::IMPORT_PID {
        return "the original file".to_string();
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
pub(super) fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `{state: clean|dirty|missing|declined, sinceMs?, reason?}`, or null for
/// a session without a file.
pub(super) fn disk_json(d: Option<&DiskState>) -> Value {
    match d {
        None => Value::Null,
        Some(DiskState::Clean) => json!({ "state": "clean" }),
        Some(DiskState::Dirty { since_ms }) => json!({ "state": "dirty", "sinceMs": since_ms }),
        Some(DiskState::Missing) => json!({ "state": "missing" }),
        Some(DiskState::Declined(reason)) => json!({ "state": "declined", "reason": reason }),
        Some(other) => json!({ "state": format!("{other:?}").to_ascii_lowercase() }),
    }
}

pub(super) fn approval_json(a: &Approval) -> Value {
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

// ── shapes ────────────────────────────────────────────────────────────

pub(super) fn doc_json(t: &Target) -> Value {
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

pub(super) fn lower<T: std::fmt::Debug>(v: &T) -> String {
    let s = format!("{v:?}");
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn version_json(v: &VersionInfo) -> Value {
    json!({
        "id": v.id,
        "version": v.seq,
        "atMs": v.at_ms,
        "reason": lower(&v.reason),
        "name": v.name,
        "authors": v.authors,
        "bytes": v.bytes,
    })
}

fn log_item_json(i: &LogItem) -> Value {
    match i {
        LogItem::Change { info, approval } => json!({
            "type": "change",
            "change": info.seq,
            "kind": lower(&info.kind),
            "of": info.of,
            "author": info.author,
            "via": info.via,
            "atMs": info.at_ms,
            "onBehalfOf": info.on_behalf_of,
            "approval": approval.map(|a| lower(&a)),
            "linesAdded": info.lines_added,
            "linesRemoved": info.lines_removed,
        }),
        LogItem::Version(v) => {
            let mut j = version_json(v);
            j["type"] = json!("version");
            j
        }
        other => json!({ "type": "other", "item": format!("{other:?}") }),
    }
}

/// merge3 conflicts (`write --base`).
fn conflicts_json(c: &[Conflict]) -> Value {
    Value::Array(
        c.iter()
            .map(|c| json!({ "lines": [c.lines.0, c.lines.1], "base": c.base_text, "ours": c.ours, "theirs": c.theirs }))
            .collect(),
    )
}

/// Undo / redo / revert conflict regions (live coordinates, a kind).
fn regions_json(c: &[ConflictRegion]) -> Value {
    Value::Array(
        c.iter()
            .map(|c| {
                json!({
                    "lines": [c.lines.0, c.lines.1],
                    "kind": lower(&c.kind),
                    "base": c.base,
                    "ours": c.ours,
                    "theirs": c.theirs,
                })
            })
            .collect(),
    )
}

fn conflicts_response(t: &Target, step: Option<usize>, conflicts: Value, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "doc": doc_json(t),
            "error": { "code": code, "hint": hint, "step": step, "exit": k2_mpc_base::tools::exit::NO },
            "conflicts": conflicts,
        })
        .to_string(),
    }
}

/// Undo, redo and revert outcomes. Applied (all or the clean part) is 200
/// with `exit` (1 when regions were skipped or a later step blocked);
/// nothing applied because of conflicts is 409 (exit 1).
fn undo_response(c: &Call, out: UndoOutcome) -> CliResponse {
    let t = c.target;
    let exit = out.exit();
    match out {
        UndoOutcome::Applied { steps, result, skipped, blocked, .. } => ok(json!({
            "doc": doc_json(t),
            "steps": steps,
            "result": edit_json(&result),
            "skipped": regions_json(&skipped),
            "blocked": blocked.as_deref().map(regions_json),
            "exit": exit,
        })),
        UndoOutcome::Conflicts { step, conflicts, .. } => conflicts_response(
            t,
            Some(step),
            regions_json(&conflicts),
            "conflicts",
            "it overlaps edits made since; nothing was applied (--partial applies the clean parts)",
        ),
        UndoOutcome::Blocked { conflicts, .. } => conflicts_response(
            t,
            None,
            regions_json(&conflicts),
            "blocked",
            "every part of it overlaps edits made since; nothing was applied (--drop removes the step)",
        ),
        UndoOutcome::Dropped { change, .. } => ok(json!({ "doc": doc_json(t), "dropped": change, "exit": exit })),
        UndoOutcome::Nothing => ok(json!({ "doc": doc_json(t), "steps": 0, "nothing": true, "exit": exit })),
        UndoOutcome::HistoryLost => ok(json!({
            "doc": doc_json(t),
            "steps": 0,
            "historyLost": true,
            "hint": "your undo history went with the last reload; use k2 doc revert <change> (k2 doc log)",
            "exit": exit,
        })),
        UndoOutcome::ClearedByPurge => ok(json!({
            "doc": doc_json(t),
            "steps": 0,
            "clearedByPurge": true,
            "hint": "your undo history was cleared by a purge",
            "exit": exit,
        })),
        other => ok(json!({ "doc": doc_json(t), "outcome": format!("{other:?}"), "exit": exit })),
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

pub(super) fn roster_json(e: &RosterEntry) -> Value {
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
        // Tier 0: what they look at ("page 3") and whether they hold the lease.
        "region": e.region,
        "holdsLease": e.holds_lease,
        // Agent presence: live claims, and the lines of their latest edit.
        "claims": e.claims.iter().map(claim_json).collect::<Vec<_>>(),
        "lastEdit": e.last_edit.as_ref().map(|l| json!([l.first, l.last])),
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

    /// The undo promise (kit v0.9.0) is in `k2 doc --help` and `k2 study
    /// docs` word for word (line breaks aside).
    #[test]
    fn the_undo_promise_is_in_the_help_and_the_study_page_verbatim() {
        const PROMISE: &str = "Undo, redo and revert only ever remove text that the change they reverse put there. Your undo never removes text someone else wrote, even if it is identical to yours, and someone else's undo never stands in the way of yours. Text put back by an undo or redo keeps its original author: blame shows who wrote it and who restored it, and later undos and reverts treat it as the original author's. If someone else has since changed the same lines, a plain undo applies nothing and reports the conflict; with --partial it applies the parts it cleanly can and reports the rest. It never overwrites their change. Nothing is lost: undone text stays in history and can be restored. This holds across restarts.";
        let cli = include_str!("../../../../cli/k2");
        let page = |start: &str| {
            let s = cli.find(start).unwrap_or_else(|| panic!("{start} in cli/k2"));
            let body = &cli[s..s + cli[s..].find("\nEOF\n").expect("heredoc end")];
            body.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        for start in ["\n_doc_usage() {", "\n_study_page_docs() {"] {
            assert!(page(start).contains(PROMISE), "the promise isn't verbatim in {start}");
        }
    }

    /// Provenance is kit-stamped only (spec U23, `sec.client_provenance_refused`):
    /// a caller naming it, on any route, in the body (at any depth) or the
    /// query, is refused before the docs runtime, a target or the kit is
    /// touched (this test has no runtime: reaching it would answer 503).
    #[test]
    fn a_provenance_field_is_refused_on_every_route_and_never_reaches_the_kit() {
        let agent = DocCaller::Agent { project_id: "p1".into() };
        let owner = super::super::caller::owner();
        for r in ROUTES {
            let m = if is_post_route(r) { "POST" } else { "GET" };
            for caller in [&agent, &owner] {
                for body in [
                    r#"{"path":"a.md","restores":"agent:x"}"#,
                    r#"{"path":"a.md","edits":[{"lines":"1,1","text":"x","provenance":{"root":"1:2"}}]}"#,
                    r#"{"path":"a.md","argv":["undo"],"restoredBy":"person:owner"}"#,
                ] {
                    let resp = handle(m, r, &HashMap::new(), if m == "POST" { body.as_bytes() } else { b"" }, caller);
                    if m == "GET" {
                        continue;
                    }
                    assert!(resp.status.starts_with("400"), "{m} {r} {body}: {} {}", resp.status, resp.body);
                    assert!(resp.body.contains("provenance_is_kit_only"), "{m} {r}: {}", resp.body);
                }
                let mut q = HashMap::new();
                q.insert("restored_in".to_string(), "7".to_string());
                let resp = handle(m, r, &q, b"", caller);
                assert!(resp.body.contains("provenance_is_kit_only"), "{m} {r} (query): {}", resp.body);
            }
        }
        assert_eq!(provenance_field(&HashMap::new(), &json!({"path": "a.md", "text": "restores"})), None, "a value is not a field");
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

    /// K-PAR-5: every verb of the kit's registry for the Markdown pack is a
    /// `k2 doc` verb. The CLI either names it in `cmd_doc` or forwards it to
    /// the daemon's tools (kit tools and the pack's own verbs), and its help
    /// shows it. A new kit or pack verb fails here until the CLI has it.
    #[test]
    fn every_registry_verb_is_a_k2_doc_verb() {
        let cli = include_str!("../../../../cli/k2");
        let start = cli.find("\ncmd_doc() {").expect("cmd_doc in cli/k2");
        let end = cli[start..].find("# END_CLI_DOC").expect("END_CLI_DOC") + start;
        let body = &cli[start..end];
        let mut labels: Vec<String> = Vec::new();
        for line in body.lines() {
            let l = line.trim_start();
            if let Some((pat, _)) = l.split_once(')') {
                if !pat.is_empty() && pat.chars().all(|c| c.is_ascii_lowercase() || c == '|' || c == '-') {
                    labels.extend(pat.split('|').map(str::to_string));
                }
            }
        }
        let usage_start = cli.find("\n_doc_usage() {").expect("_doc_usage");
        let usage = &cli[usage_start..cli[usage_start..].find("\nEOF\n").expect("usage end") + usage_start];
        let md = k2_mpc_base::parity::Registry::for_pack(&k2_mpc_markdown::PACK).expect("registry");
        // The text pack (any other text): the same kit verbs, none of its own.
        let text = k2_mpc_base::parity::Registry::for_pack(&k2_mpc_text::PACK).expect("text registry");
        let mut missing = Vec::new();
        for v in md.verbs().iter().chain(text.verbs()) {
            let word = v.name.split_whitespace().next().unwrap_or(v.name);
            let routed = labels.iter().any(|l| l == word)
                || k2_mpc_base::tools::is_kit_verb(word)
                || k2_mpc_base::session::Pack::verbs(&k2_mpc_markdown::PACK).iter().any(|p| p.name() == word);
            let documented = usage.contains(&format!("k2 doc {word}")) || usage.contains(&format!(" {word} ")) || usage.contains(&format!("|{word}")) || usage.contains(&format!("{word}|"));
            if !routed || !documented {
                missing.push(format!("{} (routed {routed}, in help {documented})", v.name));
            }
        }
        assert!(missing.is_empty(), "registry verbs k2 doc lacks: {missing:?}");
    }
}
