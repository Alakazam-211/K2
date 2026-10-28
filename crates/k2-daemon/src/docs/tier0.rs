//! Tier 0 on `/cli/docs/*`: any file that isn't Markdown is a blob doc
//! (kit amendment 3, `BlobSession`), with the same caller, target, rights
//! and shares as Markdown (PRD K-T0-1…9).
//!
//! | Route | Right | Kit call |
//! |---|---|---|
//! | `status` | read (+ history for the conflict copies) | `status`, `conflict_copies` |
//! | `flush` | read | `flush` |
//! | `read` | read | `read` (the current bytes, verified, as base64) |
//! | `who` | read | `roster` (the lease holder is an editor; `region`) |
//! | `open` / `leave` | read | `join_shared(Held)` / `leave` |
//! | `view` | read | `set_region` (`region`: "page 3"; `lines` needs a text doc) |
//! | `lease` | edit (release: read; break: owner) | `acquire_lease`, `renew_lease`, `release_lease`, `break_lease` |
//! | `write` | edit | `write_bytes` from `base` (a version; `live` is refused: never last-writer-wins) |
//! | `versions`, `log` | history | `versions` |
//! | `show` | history | `read_version` (bytes, base64) |
//! | `restore` | edit + history | `restore` |
//! | `name` | edit | `name_version` (the current one by default) |
//! | `blame` | history | `blame` (the whole blob: who set the current version, the history summarised) |
//! | `mv` | edit | `rekey` + `file_moved` |
//!
//! Every other verb needs a text doc and refuses with `not_a_text_doc`
//! (exit 2), naming what tier 0 has. Which kit verbs are which is
//! [`TIER0_VERBS`] / [`TEXT_ONLY_VERBS`], checked against the kit's
//! registry (`KIT_VERBS`) by a test, so a new kit verb fails until it is
//! classified. The lease is K2's verb (`lease`, `lock`, `unlock`) until the
//! kit's registry has one.
//!
//! **Not built (kit K-T0-9):** on-behalf writes (their approvals), so an
//! agent's write, restore or lease `--on-behalf-of` someone is refused;
//! purge stays off `k2 doc`.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use k2_mpc_base::session::{BlobSession, BlobWriteResult, Joined, LeaseInfo, SessionError, Stay};
use k2_mpc_base::store::{BlobVersion, DocKey, VersionId};

use super::caller::refusal;
use super::routes::{self, Call, VerbError};
use super::target::Target;
use crate::cli_response::CliResponse;
use k2_mpc_base::tools::exit;

/// Kit verbs (`KIT_VERBS` names) that work on a tier-0 doc.
pub const TIER0_VERBS: &[&str] = &[
    "open",
    "leave",
    "who",
    "follow",
    "write",
    "status",
    "flush",
    "watch",
    "tail -f",
    "log",
    "show --version",
    "blame",
    "restore",
    "version --name",
    "mv",
];

/// Kit verbs that need a text doc (tier 1).
pub const TEXT_ONLY_VERBS: &[&str] = &[
    "cat",
    "wc",
    "head",
    "tail",
    "sed",
    "view",
    "grep",
    "cursor",
    "insert",
    "replace",
    "delete",
    "sed -i",
    "patch",
    "apply",
    "append",
    "undo",
    "redo",
    "undo --drop",
    "undo --partial",
    "diff --version",
    "diff",
    "approve",
    "decline",
    "log --pending",
    "revert",
];

/// K2's own tier-0 verbs (the lease), not in the kit's registry yet.
pub const LEASE_VERBS: &[&str] = &["lease", "lock", "unlock"];

/// What a tier-0 doc offers, for the teaching refusal.
pub fn offered() -> String {
    let mut v: Vec<&str> = TIER0_VERBS.iter().copied().filter(|v| *v != "tail -f").collect();
    v.push("lease");
    v.join(", ")
}

/// A verb that needs a text doc, on a tier-0 doc (exit 2).
pub fn not_a_text_doc(t: &Target, verb: &str) -> CliResponse {
    let body = json!({
        "ok": false,
        "error": {
            "code": "not_a_text_doc",
            "hint": format!(
                "`{verb}` works on text docs; {} is an any-file doc (tier 0), which has: {}. Read its bytes with k2 doc show --version V (k2 doc status shows V), or open the file itself",
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

/// The verb a route names when it needs a text doc (`None`: tier 0 has it).
fn text_only_route(path: &str, c: &Call) -> Option<String> {
    match path {
        routes::TOOL => {
            let argv: Vec<String> = c
                .input
                .body
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            Some(if argv.is_empty() { "tool".to_string() } else { k2_mpc_base::parity::verb_form(&argv) })
        }
        routes::EDIT => Some("edit".into()),
        routes::CURSOR => Some("cursor".into()),
        routes::UNDO => Some("undo".into()),
        routes::REDO => Some("redo".into()),
        routes::REVERT => Some("revert".into()),
        routes::DIFF => Some("diff --version".into()),
        routes::APPROVE => Some("approve".into()),
        routes::APPROVALS => Some("approvals".into()),
        routes::VIEW if c.input.str("lines").is_some() => Some("view --lines".into()),
        routes::LOG if c.input.flag("pending") => Some("log --pending".into()),
        routes::BLAME if c.input.str("lines").is_some_and(|l| !l.trim().is_empty()) => Some("blame -L".into()),
        _ => None,
    }
}

/// Routes that write (refused on someone's behalf until the kit records
/// approvals for blob writes, K-T0-9).
fn writes(path: &str) -> bool {
    matches!(path, routes::WRITE | routes::RESTORE | routes::LEASE | routes::NAME | routes::MV)
}

/// The tier-0 entry, after the caller, target and rights checks.
pub(super) fn handle(path: &str, rt: &Arc<super::Runtime>, c: &Call) -> CliResponse {
    if let Some(verb) = text_only_route(path, c) {
        return not_a_text_doc(c.target, &verb);
    }
    if c.on_behalf.is_some() && writes(path) {
        return routes::with_exit(
            refusal(
                "400 Bad Request",
                "on_behalf_not_built",
                "on-behalf changes to any-file docs need approvals, which aren't built yet; make the change as yourself",
            ),
            exit::USAGE,
        );
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

/// Load the blob doc synced with its file (`open_blob_file`).
fn open(rt: &Arc<super::Runtime>, t: &Target) -> Result<Arc<BlobSession>, CliResponse> {
    let key = t.key();
    if let Some(s) = rt.get_blob(&key) {
        return Ok(s);
    }
    rt.open_blob(&key, &t.abs, &t.root).map_err(|e| routes::session_error(&e))
}

fn join(s: &BlobSession, c: &Call, stay: Stay) -> Result<Joined, SessionError> {
    s.join_shared(c.caller.participant(c.on_behalf), stay)
}

/// `{ok, tier: 0, doc, ...}`.
fn ok0(t: &Target, mut v: Value) -> CliResponse {
    v["tier"] = json!(0);
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

fn verb(path: &str, rt: &Arc<super::Runtime>, s: &Arc<BlobSession>, c: &Call) -> Result<CliResponse, VerbError> {
    let t = c.target;
    let input = c.input;
    match path {
        routes::STATUS => {
            let j = join(s, c, Stay::UntilIdle)?;
            let st = s.status()?;
            let copies = if c.rights.history {
                let files = s.conflict_copies(j.conn)?;
                Some(files.iter().map(|p| copy_rel(t, p)).collect::<Vec<_>>())
            } else {
                None
            };
            let declined = match &st.disk {
                Some(k2_mpc_base::session::DiskState::Declined(reason)) => Some(reason.clone()),
                _ => None,
            };
            Ok(ok0(
                t,
                json!({
                    "version": st.version,
                    "current": version_json(st.version, &st.current, st.version),
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
            Ok(ok0(t, json!({ "result": write_json(&w) })))
        }
        routes::VERSIONS => {
            let j = join(s, c, Stay::UntilIdle)?;
            let cur = s.status()?.version;
            let vs = s.versions(j.conn)?;
            Ok(ok0(t, json!({ "versions": vs.iter().map(|(id, v)| version_json(*id, v, cur)).collect::<Vec<_>>() })))
        }
        routes::LOG => {
            let by = match input.str("by").filter(|p| !p.trim().is_empty()) {
                Some(b) => Some(routes::any_pid(&b, c.caller).map_err(VerbError::Response)?),
                None => None,
            };
            let for_person = match input.str("forPerson").filter(|p| !p.trim().is_empty()) {
                Some(p) => Some(routes::any_pid(&p, c.caller).map_err(VerbError::Response)?),
                None => None,
            };
            let since = input.u64("sinceMs").map_err(VerbError::Response)?;
            let n = input.u64("n").map_err(VerbError::Response)?;
            let j = join(s, c, Stay::UntilIdle)?;
            let cur = s.status()?.version;
            let rows = log_rows(s.versions(j.conn)?, by.as_deref(), for_person.as_deref(), since, n);
            Ok(ok0(
                t,
                json!({
                    "log": rows.iter().map(|(id, v)| {
                        let mut x = version_json(*id, v, cur);
                        x["type"] = json!("version");
                        x
                    }).collect::<Vec<_>>(),
                }),
            ))
        }
        routes::SHOW => {
            let v = input
                .u64("version")
                .map_err(VerbError::Response)?
                .ok_or_else(|| VerbError::Response(routes::bad("version is required (k2 doc versions)")))?;
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
            let v = input
                .u64("version")
                .map_err(VerbError::Response)?
                .ok_or_else(|| VerbError::Response(routes::bad("version is required: a version id from k2 doc versions")))?;
            let j = join(s, c, Stay::UntilIdle)?;
            let w = s.restore(j.conn, v)?;
            Ok(ok0(t, json!({ "restored": v, "result": write_json(&w) })))
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
                    "versions": b.versions,
                    "authors": authors,
                    "rendered": render_blame(&t.rel, b.version, &b.current, b.versions, &b.authors),
                }),
            ))
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

/// `lease`: `action` acquire (default) | renew | release | break, `ttlSecs`.
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
            Ok(ok0(t, json!({ "action": "acquire", "lease": lease_json(&l, c) })))
        }
        "renew" => {
            let l = s.renew_lease(j.conn, ttl_ms)?;
            Ok(ok0(t, json!({ "action": "renew", "lease": lease_json(&l, c) })))
        }
        "release" => {
            let released = s.release_lease(j.conn)?;
            Ok(ok0(t, json!({ "action": "release", "released": released })))
        }
        "break" => {
            let broken = s.break_lease(j.conn)?;
            Ok(ok0(t, json!({ "action": "break", "broken": broken.as_ref().map(|l| lease_json(l, c)) })))
        }
        other => Err(VerbError::Response(routes::bad(format!("action must be acquire, renew, release or break (got {other})")))),
    }
}

/// The versions a `log` shows: newest first, filtered by who set them, for
/// whom, since when, at most `n`.
fn log_rows(
    all: Vec<(VersionId, BlobVersion)>,
    by: Option<&str>,
    for_person: Option<&str>,
    since_ms: Option<u64>,
    n: Option<u64>,
) -> Vec<(VersionId, BlobVersion)> {
    let mut rows: Vec<(VersionId, BlobVersion)> = all
        .into_iter()
        .rev()
        .filter(|(_, v)| by.is_none_or(|b| v.author == b))
        .filter(|(_, v)| for_person.is_none_or(|p| v.on_behalf_of.as_deref() == Some(p)))
        .filter(|(_, v)| since_ms.is_none_or(|s| v.at_ms >= s))
        .collect();
    if let Some(n) = n {
        rows.truncate(n as usize);
    }
    rows
}

/// A conflict copy as the doc's workspace names it (else its file name).
fn copy_rel(t: &Target, p: &Path) -> String {
    p.strip_prefix(&t.root)
        .ok()
        .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/"))
        .or_else(|| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default()
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
fn utc(ms: u64) -> String {
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
            body.push_str(&format!(", also beside the file as \"{copy}\""));
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
    use k2_mpc_base::blob::BlobId;
    use k2_mpc_base::store::BlobReason;

    /// K-PAR-5 for tier 0: every verb of the kit's registry is classified
    /// (works on any file, or needs a text doc), nothing twice, nothing
    /// stale. A new kit verb fails here until K2 decides.
    #[test]
    fn every_kit_verb_is_classified_for_tier_0() {
        let names: Vec<&str> = k2_mpc_base::parity::KIT_VERBS.iter().map(|v| v.name).collect();
        let mut missing = Vec::new();
        for n in &names {
            let a = TIER0_VERBS.contains(n);
            let b = TEXT_ONLY_VERBS.contains(n);
            if a == b {
                missing.push(format!("{n} (tier0 {a}, text-only {b})"));
            }
        }
        assert!(missing.is_empty(), "kit verbs not classified exactly once: {missing:?}");
        for v in TIER0_VERBS.iter().chain(TEXT_ONLY_VERBS) {
            assert!(names.contains(v), "{v} is classified but isn't a kit verb any more");
        }
        for v in LEASE_VERBS {
            assert!(!names.contains(v), "{v} is a kit verb now: classify it there and drop it from LEASE_VERBS");
        }
    }

    /// The CLI routes every tier-0 verb and the lease verbs, and its help
    /// shows them.
    #[test]
    fn the_cli_routes_and_documents_the_lease_verbs() {
        let cli = include_str!("../../../../cli/k2");
        let start = cli.find("\ncmd_doc() {").expect("cmd_doc in cli/k2");
        let end = cli[start..].find("# END_CLI_DOC").expect("END_CLI_DOC") + start;
        let body = &cli[start..end];
        for v in LEASE_VERBS {
            assert!(
                body.lines().any(|l| l.trim_start().split(')').next().is_some_and(|p| p.split('|').any(|w| w == *v))),
                "cmd_doc has no case for {v}"
            );
        }
        let usage_start = cli.find("\n_doc_usage() {").expect("_doc_usage");
        let usage = &cli[usage_start..cli[usage_start..].find("\nEOF\n").expect("usage end") + usage_start];
        assert!(usage.contains("k2 doc lease"), "help lacks k2 doc lease");
        assert!(usage.contains("Any other file"), "help lacks the tier-0 section");
    }

    fn v(author: &str, at: u64, behalf: Option<&str>) -> BlobVersion {
        let mut x = BlobVersion::new(BlobId::from_slice(&[7u8; 32]).expect("id"), 3, at, author, author, "cli", BlobReason::Write);
        x.on_behalf_of = behalf.map(str::to_string);
        x
    }

    #[test]
    fn log_is_newest_first_and_filters_by_author_person_time_and_count() {
        let all = vec![
            (1, v("kit:import", 10, None)),
            (2, v("agent:a", 20, Some("person:owner"))),
            (3, v("person:owner", 30, None)),
            (4, v("agent:a", 40, None)),
        ];
        let ids = |r: Vec<(VersionId, BlobVersion)>| r.into_iter().map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(ids(log_rows(all.clone(), None, None, None, None)), vec![4, 3, 2, 1]);
        assert_eq!(ids(log_rows(all.clone(), Some("agent:a"), None, None, None)), vec![4, 2]);
        assert_eq!(ids(log_rows(all.clone(), None, Some("person:owner"), None, None)), vec![2]);
        assert_eq!(ids(log_rows(all.clone(), None, None, Some(25), None)), vec![4, 3]);
        assert_eq!(ids(log_rows(all, None, None, None, Some(1))), vec![4]);
    }

    #[test]
    fn utc_dates_are_civil() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_793_093_400_000), "2026-10-27 09:30:00");
    }

    #[test]
    fn a_version_carries_metadata_never_the_blob_id() {
        let j = version_json(5, &v("person:owner", 1, None), 5);
        assert_eq!(j["current"], true);
        assert_eq!(j["reason"], "write");
        assert!(j.get("blob").is_none(), "{j}");
        assert!(!j.to_string().contains(&"07".repeat(32)), "the keyed id leaked: {j}");
    }
}
