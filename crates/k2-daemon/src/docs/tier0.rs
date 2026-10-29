//! Tier 0 on `/cli/docs/*`: a file that isn't text (or text its pack refused) is a blob doc
//! (kit amendments 3, 10, 11: `BlobSession`), with the same caller, target,
//! rights and shares as Markdown (PRD K-T0-1…9).
//!
//! | Route | Right | Kit call |
//! |---|---|---|
//! | `status` | read (+ history for the conflict copies) | `status` (`conflict_copies`) |
//! | `flush` | read | `flush` |
//! | `read`, `tool` `cat` | read | `read` (the current bytes, verified; `cat` takes no flags) |
//! | `who` | read | `roster` (the lease holder is an editor; `region`) |
//! | `open` / `leave` | read | `join_shared(Held)` / `leave` |
//! | `view` | read | `set_region` (`region`: "page 3") |
//! | `lease` | edit (release: read; break: owner) | `acquire_lease`, `renew_lease`, `release_lease`, `break_lease` |
//! | `write` | edit | `write_bytes` from `base` (a version; `live` is refused: never last-writer-wins) |
//! | `versions` | history | `versions` |
//! | `log` | history | `log` (`--pending`: pending approvals) |
//! | `show` | history | `read_version` (bytes, base64) |
//! | `restore` | edit + history | `restore` |
//! | `name` | edit | `name_version` (the current one by default) |
//! | `blame` | history | `blame` (the whole blob) |
//! | `approve` | the named person | `set_approval` (a decline says how to restore) |
//! | `approvals` | read | `approvals` |
//! | `mv` | edit | `rekey` + `file_moved` (not under someone else's lease) |
//!
//! **Which verbs.** The kit's registry says which verbs work on any file
//! (`VerbSpec::tiers`, [`registry`] = `Registry::for_tier(Tier::Blob)`); a
//! route whose verb isn't there refuses with `not_a_text_doc` (exit 2),
//! naming what tier 0 has. K2 adds `lock` / `unlock` (the CLI's names for
//! `lease` / `lease --release`) and `view --region`.
//!
//! **On someone's behalf** (K-T0-9, kit amendment 11): an agent's write or
//! restore `--on-behalf-of` a person applies at once and waits for that
//! person's approval (`approve` / `decline <version>`); a decline names the
//! version to restore, nothing is restored for them. Purge stays off
//! `k2 doc`.

use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};

use k2_mpc_base::parity::{Registry, Tier};
use k2_mpc_base::session::{
    ApprovalState, BlobLogEntry, BlobSession, BlobWriteResult, ConflictCopy, Joined, LeaseInfo,
    SessionError, Stay,
};
use k2_mpc_base::store::{BlobVersion, DocKey, VersionId};

use super::caller::refusal;
use super::routes::{self, Call, VerbError};
use super::target::Target;
use crate::cli_response::CliResponse;
use k2_mpc_base::tools::exit;

/// The verbs that work on a tier-0 doc: the kit's registry for tier 0.
pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Registry::for_tier(Tier::Blob))
}

/// K2's own names on top of the registry (CLI aliases and K2's verbs).
pub const K2_VERBS: &[&str] = &["lock", "unlock", "view --region", "versions", "watch"];

/// What a tier-0 doc offers, for the teaching refusal.
pub fn offered() -> String {
    let mut v: Vec<&str> = registry().verbs().iter().map(|v| v.name).collect();
    v.extend(K2_VERBS.iter().copied());
    v.join(", ")
}

/// A verb that needs a text doc, on a tier-0 doc (exit 2).
pub fn not_a_text_doc(t: &Target, verb: &str) -> CliResponse {
    let body = json!({
        "ok": false,
        "error": {
            "code": "not_a_text_doc",
            "hint": format!(
                "`{verb}` works on text docs; {} is an any-file doc (tier 0), which has: {}. k2 doc cat prints its bytes",
                t.rel,
                offered()
            ),
            "verb": verb,
            "tier": 0,
            "exit": exit::USAGE,
        },
    });
    CliResponse { status: "400 Bad Request", content_type: "application/json", body: body.to_string() }
}

/// The registry verb a route runs (its CLI form), for the routes that map
/// to one; `None` for K2's own routes (status, read, versions, view
/// --region...), which tier 0 serves.
fn route_verb(path: &str, c: &Call) -> Option<String> {
    let v = match path {
        routes::TOOL => {
            let argv: Vec<String> = c
                .input
                .body
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            return Some(if argv.is_empty() { "tool".to_string() } else { k2_mpc_base::parity::verb_form(&argv) });
        }
        routes::EDIT => "edit",
        routes::CURSOR => "cursor",
        routes::UNDO => "undo",
        routes::REDO => "redo",
        routes::REVERT => "revert",
        routes::DIFF => "diff --version",
        routes::CLAIM => "claim",
        routes::ACTIVITY => "activity",
        // The kit's alias of `log --pending` (P30), on both tiers.
        routes::APPROVALS => "approvals",
        routes::VIEW if c.input.str("lines").is_some() => "view",
        routes::BLAME if c.input.str("lines").is_some_and(|l| !l.trim().is_empty()) => "blame -L",
        _ => return None,
    };
    Some(v.to_string())
}

/// Why a text file is a tier-0 doc instead: its pack's refusal (`too_large`,
/// `not_utf8`, `binary`, `mixed_line_endings`, with the pack's message), or
/// `stored_as_any_file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    pub code: String,
    pub message: String,
}

/// The tier-0 entry, after the caller, target and rights checks.
/// `fallback`: a text file served at tier 0, and why; every answer then
/// carries it (`fallback: {code, message}`), and a text verb's refusal says
/// it.
pub(super) fn handle(path: &str, rt: &Arc<super::Runtime>, c: &Call, fallback: Option<&Fallback>) -> CliResponse {
    let r = handle_inner(path, rt, c);
    match fallback {
        Some(f) => with_fallback(r, f),
        None => r,
    }
}

fn handle_inner(path: &str, rt: &Arc<super::Runtime>, c: &Call) -> CliResponse {
    if let Some(verb) = route_verb(path, c) {
        if registry().get(&verb).is_none() {
            return not_a_text_doc(c.target, &verb);
        }
    }
    let s = match open(rt, c.target) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match verb(path, rt, &s, c) {
        // The pool unloaded the doc between open and the call: once more.
        Err(VerbError::Session(SessionError::Unloaded)) => match open(rt, c.target) {
            Ok(s) => verb(path, rt, &s, c).unwrap_or_else(VerbError::into_response),
            Err(r) => r,
        },
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `r` with the fallback named: at the top of an answer, and in an error
/// (whose hint then says the file isn't live text, and why).
pub fn with_fallback(mut r: CliResponse, f: &Fallback) -> CliResponse {
    let Ok(mut v) = serde_json::from_str::<Value>(&r.body) else { return r };
    let fb = json!({ "code": f.code, "message": f.message });
    if let Some(e) = v.get_mut("error").and_then(Value::as_object_mut) {
        if let Some(Value::String(h)) = e.get_mut("hint") {
            h.push_str(&format!(" (It isn't live text: {} ({}); it opened as an any-file doc.)", f.message, f.code));
        }
        e.insert("fallback".into(), fb.clone());
    }
    if let Some(o) = v.as_object_mut() {
        o.insert("fallback".into(), fb);
    }
    r.body = v.to_string();
    r
}

/// Load the blob doc synced with its file (`open_blob_file`).
pub(super) fn open(rt: &Arc<super::Runtime>, t: &Target) -> Result<Arc<BlobSession>, CliResponse> {
    let key = t.key();
    if let Some(s) = rt.get_blob(&key) {
        return Ok(s);
    }
    rt.open_blob(&key, &t.abs, &t.root).map_err(|e| routes::session_error(&e))
}

fn join(s: &BlobSession, c: &Call, stay: Stay) -> Result<Joined, SessionError> {
    s.join_shared(c.caller.participant(c.on_behalf), stay)
}

/// `{ok, tier: 0, pack: blob, family: none, doc, ...}`.
fn ok0(t: &Target, mut v: Value) -> CliResponse {
    v["tier"] = json!(0);
    v["pack"] = json!("blob");
    v["family"] = json!("none");
    v["doc"] = routes::doc_json(t);
    routes::ok(v)
}

/// Every byte a reader yields, verified by the kit as it streams (only a
/// clean end means they are good: a damaged blob is never served).
fn read_all(mut r: impl Read) -> Result<Vec<u8>, VerbError> {
    let mut out = Vec::new();
    r.read_to_end(&mut out).map_err(|e| {
        VerbError::Response(routes::with_exit(
            refusal("500 Internal Server Error", "blob_unreadable", format!("the stored bytes couldn't be read: {e}")),
            exit::NO,
        ))
    })?;
    Ok(out)
}

/// Bytes a write sends: `bytesB64` (base64, any bytes), or `text` (UTF-8).
fn body_bytes(c: &Call) -> Result<Vec<u8>, VerbError> {
    use base64::Engine as _;
    match (c.input.body.get("bytesB64"), c.input.body.get("text")) {
        (Some(Value::String(b)), _) => base64::engine::general_purpose::STANDARD
            .decode(b.as_bytes())
            .map_err(|e| VerbError::Response(routes::bad(format!("bytesB64 is not base64: {e}")))),
        (None, Some(Value::String(t))) => Ok(t.clone().into_bytes()),
        _ => Err(VerbError::Response(routes::bad("the new content is required: bytesB64 (base64) or text"))),
    }
}

/// A version number param (`what` names it in the refusal).
fn version_param(c: &Call, key: &str, what: &str) -> Result<VersionId, VerbError> {
    c.input.u64(key).map_err(VerbError::Response)?.ok_or_else(|| VerbError::Response(routes::bad(format!("{key} is required: {what}"))))
}

fn verb(path: &str, rt: &Arc<super::Runtime>, s: &Arc<BlobSession>, c: &Call) -> Result<CliResponse, VerbError> {
    let t = c.target;
    let input = c.input;
    match path {
        routes::STATUS => {
            let j = join(s, c, Stay::UntilIdle)?;
            let st = s.status()?;
            let declined = match &st.disk {
                Some(k2_mpc_base::session::DiskState::Declined(reason)) => Some(reason.clone()),
                _ => None,
            };
            // Conflict copies hold earlier content: history only.
            let copies = c.rights.history.then(|| st.conflict_copies.iter().map(|cc| copy_json(t, cc)).collect::<Vec<_>>());
            let approval = s.approval(j.conn, st.version).ok().flatten();
            Ok(ok0(
                t,
                json!({
                    "version": st.version,
                    "current": version_json(st.version, &st.current, st.version),
                    "approval": approval.as_ref().map(|a| routes::lower(&a.state)),
                    "participants": st.participants,
                    "connections": st.connections,
                    "lease": st.lease.as_ref().map(|l| lease_json(l, c)),
                    "disk": routes::disk_json(st.disk.as_ref()),
                    "declined": declined,
                    "conflictCopies": copies,
                    "rights": c.rights.json(),
                    "owner": c.rights.owner,
                }),
            ))
        }
        routes::FLUSH => {
            let j = join(s, c, Stay::UntilIdle)?;
            let f = s.flush(j.conn)?;
            Ok(ok0(t, json!({ "written": f.written, "hash": f.hash, "disk": routes::disk_json(f.disk.as_ref()) })))
        }
        routes::READ => {
            let j = join(s, c, Stay::UntilIdle)?;
            let r = s.read(j.conn)?;
            let (version, info) = (r.version, r.info.clone());
            let bytes = read_all(r.reader)?;
            Ok(ok0(
                t,
                json!({
                    "version": version,
                    "current": version_json(version, &info, version),
                    "size": bytes.len(),
                    "bytesB64": routes::b64(&bytes),
                    "rights": c.rights.json(),
                }),
            ))
        }
        // `cat` on any file (the registry's tier-0 form): the bytes, as a
        // text tool answers (stdout, or stdoutB64 when not UTF-8).
        routes::TOOL => {
            let argv: Vec<&str> = input.body.get("argv").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            if argv.len() > 1 {
                return Ok(ok0(
                    t,
                    json!({
                        "exit": exit::USAGE,
                        "stdout": "",
                        "stderr": format!("k2 doc cat: {} is an any-file doc: cat prints its bytes and takes no flags", t.rel),
                    }),
                ));
            }
            let j = join(s, c, Stay::UntilIdle)?;
            let r = s.read(j.conn)?;
            let version = r.version;
            let bytes = read_all(r.reader)?;
            let mut v = json!({ "exit": exit::OK, "version": version, "stderr": "" });
            match String::from_utf8(bytes) {
                Ok(text) => v["stdout"] = json!(text),
                Err(e) => {
                    v["stdout"] = Value::Null;
                    v["stdoutB64"] = json!(routes::b64(e.as_bytes()));
                }
            }
            Ok(ok0(t, v))
        }
        routes::WHO => {
            let j = join(s, c, Stay::UntilIdle)?;
            let roster = s.roster(j.conn)?;
            Ok(ok0(t, json!({ "roster": roster.iter().map(routes::roster_json).collect::<Vec<_>>() })))
        }
        routes::OPEN => {
            let j = join(s, c, Stay::Held)?;
            Ok(ok0(t, json!({ "version": j.version, "client": j.client, "rights": c.rights.json() })))
        }
        routes::LEAVE => {
            let j = join(s, c, Stay::UntilIdle)?;
            let left = s.leave(j.conn)?;
            Ok(ok0(t, json!({ "left": left })))
        }
        routes::VIEW => {
            let region = input
                .str("region")
                .ok_or_else(|| VerbError::Response(routes::bad("region is required (what you are looking at: \"page 3\"; empty clears it)")))?;
            let region = Some(region.trim().to_string()).filter(|r| !r.is_empty());
            let j = join(s, c, Stay::UntilIdle)?;
            s.set_region(j.conn, region.clone())?;
            Ok(ok0(t, json!({ "region": region })))
        }
        routes::LEASE => lease(s, c),
        routes::WRITE => {
            let base = input
                .str("base")
                .ok_or_else(|| VerbError::Response(routes::bad("base is required: the version you are replacing (k2 doc status)")))?;
            if base.trim() == "live" {
                return Err(VerbError::Response(routes::bad(
                    "any-file docs never take --base live (a write never overwrites someone else's version unseen); give the version you are replacing (k2 doc status)",
                )));
            }
            let base: VersionId =
                base.trim().parse().map_err(|_| VerbError::Response(routes::bad("base must be a version number (k2 doc status)")))?;
            let bytes = body_bytes(c)?;
            let j = join(s, c, Stay::UntilIdle)?;
            let w = s.write_bytes(j.conn, &bytes, base)?;
            on_behalf_reply(s, c, j.conn, json!({ "result": write_json(&w) }), w.changed.then_some(w.version))
        }
        routes::VERSIONS => {
            let j = join(s, c, Stay::UntilIdle)?;
            let cur = s.status()?.version;
            let vs = s.versions(j.conn)?;
            Ok(ok0(t, json!({ "versions": vs.iter().map(|(id, v)| version_json(*id, v, cur)).collect::<Vec<_>>() })))
        }
        routes::LOG => {
            let any = |k: &str| -> Result<Option<String>, VerbError> {
                match input.str(k).filter(|p| !p.trim().is_empty()) {
                    Some(b) => Ok(Some(routes::any_pid(&b, c.caller).map_err(VerbError::Response)?)),
                    None => Ok(None),
                }
            };
            let (by, for_person) = (any("by")?, any("forPerson")?);
            let since = input.u64("sinceMs").map_err(VerbError::Response)?;
            let n = input.u64("n").map_err(VerbError::Response)?;
            let pending = input.flag("pending");
            let j = join(s, c, Stay::UntilIdle)?;
            let f = LogFilter { by: by.as_deref(), for_person: for_person.as_deref(), since_ms: since, pending };
            let rows = log_rows(s.log(j.conn)?, &f, n);
            Ok(ok0(t, json!({ "log": rows.iter().map(|e| log_json(t, e)).collect::<Vec<_>>() })))
        }
        routes::SHOW => {
            let v = version_param(c, "version", "a version id (k2 doc versions)")?;
            let j = join(s, c, Stay::UntilIdle)?;
            let r = s.read_version(j.conn, v)?;
            let info = r.info.clone();
            let bytes = read_all(r.reader)?;
            let cur = s.status()?.version;
            Ok(ok0(
                t,
                json!({
                    "version": v,
                    "info": version_json(v, &info, cur),
                    "size": bytes.len(),
                    "bytesB64": routes::b64(&bytes),
                }),
            ))
        }
        routes::RESTORE => {
            let v = version_param(c, "version", "a version id from k2 doc versions")?;
            let j = join(s, c, Stay::UntilIdle)?;
            let w = s.restore(j.conn, v)?;
            on_behalf_reply(s, c, j.conn, json!({ "restored": v, "result": write_json(&w) }), w.changed.then_some(w.version))
        }
        routes::NAME => {
            let name = input
                .str("name")
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .ok_or_else(|| VerbError::Response(routes::bad("name is required")))?;
            let j = join(s, c, Stay::UntilIdle)?;
            let v = match input.u64("version").map_err(VerbError::Response)? {
                Some(v) => v,
                None => s.status()?.version,
            };
            s.name_version(j.conn, v, &name)?;
            Ok(ok0(t, json!({ "version": v, "name": name })))
        }
        routes::BLAME => {
            let j = join(s, c, Stay::UntilIdle)?;
            let b = s.blame(j.conn)?;
            let authors: Vec<Value> = b
                .authors
                .iter()
                .map(|a| json!({ "pid": a.pid, "label": routes::pid_label(&a.pid), "versions": a.versions, "lastMs": a.last_ms }))
                .collect();
            Ok(ok0(
                t,
                json!({
                    "version": b.version,
                    "current": version_json(b.version, &b.current, b.version),
                    "approval": b.approval.as_ref().map(routes::lower),
                    "versions": b.versions,
                    "authors": authors,
                    "rendered": render_blame(&t.rel, b.version, &b.current, b.versions, &b.authors),
                }),
            ))
        }
        // `approve` / `decline <version>`: only the person it was made for.
        routes::APPROVE => {
            let v = version_param(c, "change", "the version (k2 doc log --pending)")?;
            let state = match input.str("state").as_deref().map(str::trim) {
                Some("approved") | Some("approve") => ApprovalState::Approved,
                Some("declined") | Some("decline") => ApprovalState::Declined,
                _ => return Err(VerbError::Response(routes::bad("state must be approved or declined"))),
            };
            let j = join(s, c, Stay::UntilIdle)?;
            let a = s.set_approval(j.conn, v, state)?;
            Ok(ok0(t, approval_reply(v, routes::approval_json(&a.approval), a.approval.state, a.restore_to, &a.later)))
        }
        routes::APPROVALS => {
            let pending_only = input.flag("pending");
            let for_person = routes::approvals_for(input, c.caller).map_err(VerbError::Response)?;
            let j = join(s, c, Stay::UntilIdle)?;
            let rows: Vec<Value> = s
                .approvals(j.conn)?
                .into_iter()
                .filter(|(_, a)| !pending_only || a.state == ApprovalState::Pending)
                .filter(|(_, a)| for_person.as_ref().is_none_or(|p| &a.on_behalf_of == p))
                .map(|(v, a)| json!({ "change": v, "approval": routes::approval_json(&a) }))
                .collect();
            Ok(ok0(t, json!({ "approvals": rows })))
        }
        routes::MV => {
            // A rename moves the file under its lease holder: theirs only.
            if let Some(l) = s.lease()?.filter(|l| l.pid != c.caller.pid()) {
                return Err(VerbError::Session(SessionError::Leased { holder: l.pid, name: l.name, expires_ms: l.expires_ms }));
            }
            Ok(routes::mv(
                rt,
                c,
                || {
                    let j = join(s, c, Stay::UntilIdle)?;
                    s.flush(j.conn).map(|_| ())
                },
                |p: &Path| s.file_moved(p).map(|_| ()),
            ))
        }
        _ => Ok(CliResponse::not_found()),
    }
}

/// A write's or restore's answer, with the pending approval when an agent
/// made it on a person's behalf.
fn on_behalf_reply(
    s: &BlobSession,
    c: &Call,
    conn: k2_mpc_base::session::ConnId,
    mut v: Value,
    new_version: Option<VersionId>,
) -> Result<CliResponse, VerbError> {
    if let Some(person) = c.on_behalf {
        v["onBehalfOf"] = json!(person);
        if let Some(a) = new_version.map(|nv| s.approval(conn, nv)).transpose()?.flatten() {
            v["approval"] = routes::approval_json(&a);
        }
    }
    Ok(ok0(c.target, v))
}

/// `approve` / `decline`: the record, and on a decline the way back
/// (nothing is restored for the person: blobs don't merge).
fn approval_reply(v: VersionId, approval: Value, state: ApprovalState, restore_to: Option<VersionId>, later: &[VersionId]) -> Value {
    let mut out = json!({ "change": v, "approval": approval });
    if state == ApprovalState::Declined {
        out["restoreTo"] = json!(restore_to);
        out["later"] = json!(later);
        out["hint"] = json!(match (restore_to, later.is_empty()) {
            (Some(r), true) => format!("declined; to undo it: k2 doc restore --version {r}"),
            (Some(r), false) => format!(
                "declined; to undo it: k2 doc restore --version {r} (versions {} came after it and would be replaced too)",
                later.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            ),
            (None, _) => "declined; there is no earlier version to restore".to_string(),
        });
    }
    out
}

/// `lease`: `action` acquire (default) | renew | release | break, `ttlSecs`.
/// Release without holding it and break with nobody holding it exit 1 (the
/// kit's registry).
fn lease(s: &Arc<BlobSession>, c: &Call) -> Result<CliResponse, VerbError> {
    let t = c.target;
    let ttl_ms = match c.input.u64("ttlSecs").map_err(VerbError::Response)? {
        Some(0) => return Err(VerbError::Response(routes::bad("ttlSecs must be at least 1"))),
        Some(secs) => Some(secs.saturating_mul(1000)),
        None => None,
    };
    let action = c.input.str("action").map(|a| a.trim().to_ascii_lowercase()).unwrap_or_else(|| "acquire".into());
    let j = join(s, c, Stay::UntilIdle)?;
    match action.as_str() {
        "acquire" | "" => {
            let l = s.acquire_lease(j.conn, ttl_ms)?;
            Ok(ok0(t, json!({ "action": "acquire", "lease": lease_json(&l, c), "exit": exit::OK })))
        }
        "renew" => {
            let l = s.renew_lease(j.conn, ttl_ms)?;
            Ok(ok0(t, json!({ "action": "renew", "lease": lease_json(&l, c), "exit": exit::OK })))
        }
        "release" => {
            let released = s.release_lease(j.conn)?;
            Ok(ok0(t, json!({ "action": "release", "released": released, "exit": if released { exit::OK } else { exit::NO } })))
        }
        "break" => {
            let broken = s.break_lease(j.conn)?;
            Ok(ok0(
                t,
                json!({
                    "action": "break",
                    "broken": broken.as_ref().map(|l| lease_json(l, c)),
                    "exit": if broken.is_some() { exit::OK } else { exit::NO },
                }),
            ))
        }
        other => Err(VerbError::Response(routes::bad(format!("action must be acquire, renew, release or break (got {other})")))),
    }
}

/// What a `log` keeps: who set the version, for whom, since when, pending
/// approvals only.
struct LogFilter<'a> {
    by: Option<&'a str>,
    for_person: Option<&'a str>,
    since_ms: Option<u64>,
    pending: bool,
}

impl LogFilter<'_> {
    fn keeps(&self, author: &str, on_behalf_of: Option<&str>, at_ms: u64, approval: Option<ApprovalState>) -> bool {
        self.by.is_none_or(|b| author == b)
            && self.for_person.is_none_or(|p| on_behalf_of == Some(p))
            && self.since_ms.is_none_or(|s| at_ms >= s)
            && (!self.pending || approval == Some(ApprovalState::Pending))
    }
}

/// The entries a `log` shows: newest first, filtered, at most `n`.
fn log_rows(all: Vec<BlobLogEntry>, f: &LogFilter, n: Option<u64>) -> Vec<BlobLogEntry> {
    all.into_iter()
        .rev()
        .filter(|e| f.keeps(&e.author, e.on_behalf_of.as_deref(), e.at_ms, e.approval))
        .take(n.map_or(usize::MAX, |n| n as usize))
        .collect()
}

/// A path as the doc's workspace names it (else its file name).
fn copy_rel(t: &Target, p: &Path) -> String {
    p.strip_prefix(&t.root)
        .ok()
        .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/"))
        .or_else(|| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default()
}

fn copy_json(t: &Target, c: &ConflictCopy) -> Value {
    json!({ "path": copy_rel(t, &c.path), "version": c.version, "kept": c.kept, "atMs": c.at_ms })
}

/// One log entry (metadata only).
fn log_json(t: &Target, e: &BlobLogEntry) -> Value {
    json!({
        "type": "version",
        "id": e.id,
        "kind": routes::lower(&e.kind),
        "reason": routes::lower(&e.kind),
        "of": e.of,
        "author": e.author,
        "authorLabel": routes::pid_label(&e.author),
        "via": e.via,
        "atMs": e.at_ms,
        "onBehalfOf": e.on_behalf_of,
        "approval": e.approval.as_ref().map(routes::lower),
        "name": e.name,
        "redacted": e.redacted,
        "size": e.size,
        "conflictCopy": e.conflict_copy.as_deref().map(|p| copy_rel(t, p)),
    })
}

fn lease_json(l: &LeaseInfo, c: &Call) -> Value {
    json!({
        "pid": l.pid,
        "name": l.name,
        "label": routes::pid_label(&l.pid),
        "acquiredMs": l.acquired_ms,
        "expiresMs": l.expires_ms,
        "auto": l.auto,
        "mine": l.pid == c.caller.pid(),
    })
}

fn write_json(w: &BlobWriteResult) -> Value {
    json!({ "version": w.version, "size": w.size, "changed": w.changed, "written": w.written })
}

/// One version (metadata only: never the bytes or the blob id).
pub fn version_json(id: VersionId, v: &BlobVersion, current: VersionId) -> Value {
    json!({
        "id": id,
        "atMs": v.at_ms,
        "reason": routes::lower(&v.reason),
        "name": v.name,
        "author": v.author,
        "authorLabel": routes::pid_label(&v.author),
        "via": v.via,
        "onBehalfOf": v.on_behalf_of,
        "size": v.size,
        "mtimeMs": v.mtime_ms,
        "restoredFrom": v.restored_from,
        "conflictCopy": v.conflict_copy,
        "redacted": v.redacted,
        "current": id == current,
    })
}

/// `blame` for a whole blob, as a person reads it.
pub fn render_blame(
    rel: &str,
    version: VersionId,
    cur: &BlobVersion,
    count: usize,
    authors: &[k2_mpc_base::session::BlobAuthor],
) -> String {
    let mut out = format!(
        "{rel} · version {version} set by {} at {} via {} ({}){}\n",
        routes::pid_label(&cur.author),
        utc(cur.at_ms),
        cur.via,
        routes::lower(&cur.reason),
        cur.restored_from.map(|v| format!(", restored from version {v}")).unwrap_or_default(),
    );
    out.push_str(&format!("{count} version{} in its history; set by:\n", if count == 1 { "" } else { "s" }));
    for a in authors {
        out.push_str(&format!(
            "  {:<32} {} version{}, last {}\n",
            routes::pid_label(&a.pid),
            a.versions,
            if a.versions == 1 { "" } else { "s" },
            utc(a.last_ms)
        ));
    }
    out
}

/// `YYYY-MM-DD HH:MM:SS` UTC.
pub(super) fn utc(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // H. Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

/// `K-T0-3`: a conflicting save tells both parties. An inbox note in the
/// doc's workspace, and in the lease holder's when that is another
/// workspace's agent. Off the session lock (the kit calls the host with it
/// held); file names and version numbers only.
pub fn notify_conflict(key: DocKey, current: VersionId, kept: VersionId, copy: Option<String>, holder: Option<String>) {
    let _ = std::thread::Builder::new().name("k2-docs-conflict".into()).spawn(move || {
        let holder_label = holder.as_deref().map(routes::pid_label);
        let title = format!("Conflicting save: {}", key.path);
        let mut body = format!(
            "{} was saved by another program {}. Nothing was lost:\n\n- the save is current: version {current}\n- what it replaced is version {kept}",
            key.path,
            match &holder_label {
                Some(h) => format!("while {h} held its lease"),
                None => "over a version the file didn't have yet".to_string(),
            },
        );
        if let Some(copy) = &copy {
            body.push_str(&format!(", also kept as the copy \"{copy}\" (next to the file's folder, under .k2/conflicts)"));
        }
        body.push_str(&format!(
            "\n\nk2 doc versions {p} lists them; k2 doc restore --version {kept} {p} makes the replaced one current again.",
            p = key.path
        ));
        let mut spaces = vec![key.space.clone()];
        if let Some(super::host::Principal::Agent(id)) = holder.as_deref().and_then(super::host::Principal::parse) {
            if id != key.space {
                spaces.push(id);
            }
        }
        for space in spaces {
            let path = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::db::schema::Project::get(&conn, &space).ok().map(|p| p.path)
            };
            let Some(path) = path else { continue };
            if let Err(e) = k2_core::inbox::compose(Path::new(&path), &title, &body, Some("high"), Some("docs"), Some("k2 doc")) {
                k2_core::log_debug!("[docs] WARN: conflict notice for {}: {e}", super::host::key_label(&key));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kit decides which verbs work on any file (`VerbSpec::tiers`):
    /// the leases are there, the text tools aren't, and K2's own names
    /// aren't the kit's.
    #[test]
    fn the_tier_0_verbs_come_from_the_kits_registry() {
        let names: Vec<&str> = registry().verbs().iter().map(|v| v.name).collect();
        for v in ["open", "leave", "cat", "who", "write", "status", "flush", "log", "show --version", "restore",
            "version --name", "mv", "blame", "lease", "lease --renew", "lease --release", "lease --break", "approve",
            "decline", "log --pending"]
        {
            assert!(names.contains(&v), "{v} is not a tier-0 verb in the kit's registry: {names:?}");
        }
        for v in ["grep", "sed -i", "undo", "claim", "replace", "diff --version", "revert", "cursor"] {
            assert!(!names.contains(&v), "{v} is a tier-0 verb now: route it in tier0.rs");
        }
        for v in K2_VERBS {
            assert!(registry().get(v).is_none(), "{v} is a kit verb now: drop it from K2_VERBS");
        }
    }

    /// `k2 doc approvals` is the kit's verb since base v0.9.1: its alias of
    /// `log --pending` (P30) on any file too, so the approvals route on a
    /// tier-0 doc goes through the registry like the kit's other verbs.
    #[test]
    fn approvals_is_the_kits_alias_of_log_pending_on_tier_0() {
        let a = registry().get("approvals").expect("approvals in the kit's tier-0 registry");
        assert_eq!(a.alias_of(), Some("log --pending"));
    }

    /// K-PAR-5 for tier 0: every verb of the kit's tier-0 registry is a
    /// `k2 doc` verb the CLI routes, and its help shows it; K2's lease
    /// aliases too.
    #[test]
    fn every_tier_0_verb_is_a_k2_doc_verb() {
        let cli = include_str!("../../../../cli/k2");
        let start = cli.find("\ncmd_doc() {").expect("cmd_doc in cli/k2");
        let end = cli[start..].find("# END_CLI_DOC").expect("END_CLI_DOC") + start;
        let body = &cli[start..end];
        let labels: Vec<&str> = body
            .lines()
            .filter_map(|l| l.trim_start().split_once(')').map(|(p, _)| p))
            .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '|' || c == '-'))
            .flat_map(|p| p.split('|'))
            .collect();
        let usage_start = cli.find("\n_doc_usage() {").expect("_doc_usage");
        let usage = &cli[usage_start..cli[usage_start..].find("\nEOF\n").expect("usage end") + usage_start];
        let mut missing = Vec::new();
        for name in registry().verbs().iter().map(|v| v.name).chain(["lock", "unlock"]) {
            let word = name.split_whitespace().next().unwrap_or(name);
            let routed = labels.contains(&word);
            let documented = usage.contains(&format!("k2 doc {word}")) || usage.contains(&format!("|{word}")) || usage.contains(&format!("{word}|")) || usage.contains(&format!(" {word} "));
            if !routed || !documented {
                missing.push(format!("{name} (routed {routed}, in help {documented})"));
            }
        }
        assert!(missing.is_empty(), "tier-0 verbs k2 doc lacks: {missing:?}");
        assert!(usage.contains("Any-file docs (tier 0"), "help lacks the tier-0 section");
    }

    #[test]
    fn log_filters_by_author_person_time_and_pending() {
        let all = LogFilter { by: None, for_person: None, since_ms: None, pending: false };
        assert!(all.keeps("kit:import", None, 10, None));
        let by = LogFilter { by: Some("agent:a"), ..all };
        assert!(by.keeps("agent:a", None, 1, None) && !by.keeps("person:owner", None, 1, None));
        let behalf = LogFilter { for_person: Some("person:owner"), ..all };
        assert!(behalf.keeps("agent:a", Some("person:owner"), 1, None) && !behalf.keeps("agent:a", None, 1, None));
        let since = LogFilter { since_ms: Some(25), ..all };
        assert!(since.keeps("x", None, 25, None) && !since.keeps("x", None, 24, None));
        let pending = LogFilter { pending: true, ..all };
        assert!(pending.keeps("agent:a", Some("person:owner"), 1, Some(ApprovalState::Pending)));
        assert!(!pending.keeps("agent:a", Some("person:owner"), 1, Some(ApprovalState::Approved)));
        assert!(!pending.keeps("person:owner", None, 1, None));
    }

    #[test]
    fn utc_dates_are_civil() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_793_093_400_000), "2026-10-27 09:30:00");
    }

    #[test]
    fn a_version_carries_metadata_never_the_blob_id() {
        let v = BlobVersion::new(
            k2_mpc_base::blob::BlobId::from_slice(&[7u8; 32]).expect("id"),
            3,
            1,
            "person:owner",
            "Owner",
            "cli",
            k2_mpc_base::store::BlobReason::Write,
        );
        let j = version_json(5, &v, 5);
        assert_eq!(j["current"], true);
        assert_eq!(j["reason"], "write");
        assert!(j.get("blob").is_none(), "{j}");
        assert!(!j.to_string().contains(&"07".repeat(32)), "the keyed id leaked: {j}");
    }

    #[test]
    fn a_decline_says_how_to_restore_and_what_else_it_replaces() {
        let r = approval_reply(7, json!({}), ApprovalState::Declined, Some(6), &[8, 9]);
        assert_eq!(r["restoreTo"], 6);
        assert!(r["hint"].as_str().expect("hint").contains("restore --version 6 (versions 8, 9 came after it"), "{r}");
        assert_eq!(
            approval_reply(7, json!({}), ApprovalState::Declined, Some(6), &[])["hint"],
            "declined; to undo it: k2 doc restore --version 6"
        );
        assert!(approval_reply(7, json!({}), ApprovalState::Approved, Some(6), &[]).get("hint").is_none());
    }
}
