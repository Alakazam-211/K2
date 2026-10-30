//! Files on ticket messages (0.46.6, `ticket-attachments-v1`;
//! `.k2/prds/prd-ticket-attachments-v1.md` + its vs-live TA1–TA29).
//!
//! Tickets reuse Thread attachments whole: the model, limits and send-time
//! checks (`k2_core::overlay::attachments`), the per-viewer views and the
//! read door's answers (`crate::thread_attachments`). Only where the JSON
//! lives differs: `feedback_comments.attachments_json` (migration 0232)
//! instead of the overlay doc. This module is the ticket side's glue:
//!
//! - **Write** (TK3/TK4): `attachments` on `create`, `comment`, `answer`
//!   and `resolve` (answered) are parsed ([`parse_field`]), then checked and
//!   snapshotted against the **ticket's** workspace, never the caller's
//!   ([`check_and_snapshot`]). An App pass attaches only files its own
//!   `files:read` scope covers, named room-relative (the uniform 403 before
//!   the disk is touched). Any refusal refuses the whole write.
//! - **Show** (TK8): each comment's files as the caller sees them
//!   ([`views`]): live `ok` / `changed` / `gone`, `hidden` for a guest
//!   whose scope doesn't cover the path, `canDownload` for people.
//! - **Inject** (TK6): a person's message with files reaches the agent with
//!   one `Attached: <abs path>` line per file ([`with_attached_lines`]);
//!   the lines are never stored.
//! - **The door** `GET /cli/feedback/attachment` ([`handle_get`]): the
//!   ticket's read rights (an App pass: `tickets:read` in its room; a
//!   Connect Member: no owner notices), the comment must belong to the
//!   ticket, then `thread_attachments::serve` (the viewer's own file rights,
//!   the secret re-check, thumbnails, downloads). Agent passports are
//!   refused: an agent reads files from disk. POST is 405 (route policy).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use k2_core::feedback;
use k2_core::overlay::attachments::{self as att, AttachError, AttachmentRequest, ThreadAttachment};
use k2_core::skin::SkinPass;

use crate::cli::str_param;
use crate::cli_response::CliResponse;
use crate::thread_attachments as ta;

/// `/boot-status` feature key (TK10).
pub const FEATURE: &str = "ticket-attachments-v1";
/// The read door (TK9).
pub const ROUTE: &str = "/cli/feedback/attachment";
/// The wire hint for a wrong-typed `attachments` (TA18).
pub const FIELD_HINT: &str = "attachments must be a JSON array of {relPath, line?, lineEnd?}";

fn error_json(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

fn usage(hint: impl std::fmt::Display) -> CliResponse {
    error_json("400 Bad Request", "usage", hint)
}

/// The body's `attachments` (TA18): absent or `null` is none; anything but
/// an array is a 400 `usage` naming the field; the array goes through
/// Thread's `parse_requests` (at most 10, path length, line rules, no
/// unknown keys).
pub(crate) fn parse_field(v: Option<&serde_json::Value>) -> Result<Vec<AttachmentRequest>, CliResponse> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(a @ serde_json::Value::Array(_)) => {
            att::parse_requests(&a.to_string()).map_err(|e| match e {
                AttachError::Usage(m) => usage(m),
                other => error_json("400 Bad Request", other.code(), other.hint()),
            })
        }
        Some(_) => Err(usage(FIELD_HINT)),
    }
}

/// The ticket's workspace folder (TK3), or the clear 404 when it was
/// removed.
pub(crate) fn ticket_root(project_id: &str) -> Result<PathBuf, CliResponse> {
    ta::project_root(project_id).ok_or_else(|| {
        error_json(
            "404 Not Found",
            "not_found",
            "this ticket's workspace is not registered; files can't be attached to it",
        )
    })
}

/// Check and snapshot `reqs` against the ticket's workspace `project_id`
/// (TK3/TK4), all or nothing, before anything is stored. `ticket` names
/// the ticket on the audit line (`new` for a create). Returns the
/// snapshots and the root (`None` when no files were asked for).
pub(crate) fn check_and_snapshot(
    ticket: &str,
    project_id: &str,
    pass: Option<&SkinPass>,
    reqs: &[AttachmentRequest],
    who: &str,
) -> Result<(Vec<ThreadAttachment>, Option<PathBuf>), CliResponse> {
    if reqs.is_empty() {
        return Ok((Vec::new(), None));
    }
    let root = ticket_root(project_id)?;
    // An App pass attaches only what its own file rights cover, named
    // room-relative; decided before the disk is touched (no oracle).
    if let Some(p) = pass {
        if let Err(r) = ta::guest_may_attach(p, project_id, &root, reqs) {
            ta::audit(
                "ticket.attachment.refused",
                who,
                format!("ticket={ticket} reason=guest_scope count={}", reqs.len()),
            );
            return Err(r);
        }
    }
    let home = dirs::home_dir();
    match att::snapshot_all(&root, home.as_deref(), reqs) {
        Ok(a) => Ok((a, Some(root))),
        Err(e) => {
            // The path for pattern/outside refusals; a content hit logs
            // the path, never the matched text.
            let what = match &e {
                AttachError::Secret { rel, .. } => rel.clone(),
                AttachError::Outside { input } | AttachError::NotAFile { input } => input.clone(),
                AttachError::Usage(_) => "-".to_string(),
            };
            ta::audit(
                "ticket.attachment.refused",
                who,
                format!("ticket={ticket} reason={} path={what:?}", e.reason()),
            );
            Err(error_json("400 Bad Request", e.code(), ticket_hint(&e)))
        }
    }
}

/// Thread's hint, said for a ticket.
fn ticket_hint(e: &AttachError) -> String {
    e.hint().replace("this Thread's workspace", "this ticket's workspace").replace("can't ride a Thread message", "can't ride a ticket").replace(", not the Thread", "")
}

/// The audit line for files stored on a message.
pub(crate) fn audit_post(ticket: &str, comment_id: &str, who: &str, atts: &[ThreadAttachment]) {
    if atts.is_empty() {
        return;
    }
    ta::audit(
        "ticket.attachment.post",
        who,
        format!(
            "ticket={ticket} comment={comment_id} count={} rels={:?}",
            atts.len(),
            atts.iter().map(|a| a.path.as_str()).collect::<Vec<_>>()
        ),
    );
}

/// One message's files as this caller sees them (TK8): a pass is a guest
/// (its own file rights decide `hidden`), anyone else a person (TA26: the
/// ticket routes install no principal, so an agent on the owner token
/// reads as a person). `live`: stat each file.
pub(crate) fn views(project_id: &str, atts: &[ThreadAttachment], pass: Option<&SkinPass>, live: bool) -> Vec<serde_json::Value> {
    if atts.is_empty() {
        return Vec::new();
    }
    let root = ta::project_root(project_id);
    let viewer = match pass {
        Some(p) => ta::Viewer::Guest { pass: p, project_id },
        None => ta::person_or_agent(),
    };
    ta::view_list(root.as_deref(), atts, &viewer, live)
}

/// What the agent gets for a person's message with files (TK6): the text,
/// then one `Attached: <abs path>` line per file. Never stored.
pub(crate) fn with_attached_lines(text: &str, root: Option<&Path>, atts: &[ThreadAttachment]) -> String {
    let Some(root) = root else { return text.to_string() };
    let mut t = text.to_string();
    for a in atts {
        if !t.is_empty() {
            t.push('\n');
        }
        t.push_str(&format!("Attached: {}", attached_path(root, &a.path).display()));
    }
    t
}

/// The absolute path of an attachment: the workspace root plus its stored
/// path, which is always '/'-separated. On Windows each part is joined, so
/// the line reads `C:\ws\out\a.png`, not `C:\ws\out/a.png`; elsewhere it is
/// `root.join(rel)`, as before.
pub(crate) fn attached_path(root: &Path, rel: &str) -> PathBuf {
    #[cfg(windows)]
    {
        rel.split('/').filter(|part| !part.is_empty()).fold(root.to_path_buf(), |p, part| p.join(part))
    }
    #[cfg(not(windows))]
    {
        root.join(rel)
    }
}

/// The last payload injected into each ticket's agent (the newest 64
/// tickets), for the headless tests: proof that `Attached:` lines reach
/// the agent while the stored body stays the person's text. Debug builds
/// only.
#[cfg(debug_assertions)]
fn last_injects() -> &'static std::sync::Mutex<Vec<(String, String)>> {
    static L: std::sync::OnceLock<std::sync::Mutex<Vec<(String, String)>>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[cfg(debug_assertions)]
pub(crate) fn note_last_inject(ticket_id: &str, payload: &str) {
    let mut l = last_injects().lock().unwrap_or_else(|p| p.into_inner());
    l.retain(|(t, _)| t != ticket_id);
    l.push((ticket_id.to_string(), payload.to_string()));
    let over = l.len().saturating_sub(64);
    l.drain(..over);
}

/// What the last inject for ticket `ticket_id` (FULL id) said. Debug builds
/// only (the headless tests).
#[cfg(debug_assertions)]
pub fn last_inject(ticket_id: &str) -> Option<String> {
    let l = last_injects().lock().unwrap_or_else(|p| p.into_inner());
    l.iter().find(|(t, _)| t == ticket_id).map(|(_, p)| p.clone())
}

/// Who may read which tickets on the door: an App pass (its rooms,
/// `tickets:read`), or a person who may or may not see K2's own notices.
pub(crate) enum DoorCaller {
    Pass(SkinPass),
    Person { sees_notices: bool },
}

/// `GET /cli/feedback/attachment?id=<ticket id or prefix>&commentId=&n=&mode=meta|preview|thumb|range|download[&px=][&offset=&len=]`
/// (TK9). The dispatcher authenticated the caller and admitted an App
/// pass's `tickets:read`; this checks the ticket, the comment and then the
/// viewer's own file rights (`thread_attachments::serve`).
pub(crate) fn handle_get(params: &HashMap<String, String>, caller: &DoorCaller) -> CliResponse {
    if crate::caller_workspace::principal_from_params(params).is_some() {
        return error_json("403 Forbidden", "forbidden", "an agent reads attached files from disk; this door is for people");
    }
    let id = str_param(params, "id");
    if id.trim().is_empty() {
        return usage("missing id (a ticket id or unique prefix)");
    }
    let comment_id = str_param(params, "commentId");
    if comment_id.trim().is_empty() {
        return usage("missing commentId (the message's id from show)");
    }
    let req = match ta::parse_door_request(params) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let not_found = || error_json("404 Not Found", "not_found", format!("no ticket matches '{}'", id.trim()));
    let item = match caller {
        DoorCaller::Pass(pass) => match crate::feedback_routes::skin_load_item(pass, id.trim(), crate::skin_routes::TICKETS_READ) {
            Ok(item) => item,
            Err(r) => return r,
        },
        DoorCaller::Person { sees_notices } => {
            let Ok(full) = feedback::resolve_id_prefix(id.trim()) else { return not_found() };
            let Some(item) = feedback::get_item(&full) else { return not_found() };
            // TA9: a notice hides exactly as `show` hides it.
            if !sees_notices && crate::feedback_routes::is_owner_notice(&item.project_id) {
                return not_found();
            }
            item
        }
    };
    let Some(comment) = feedback::get_comment(&item.id, comment_id.trim()) else {
        return error_json("404 Not Found", "not_found", "no such message on this ticket");
    };
    let Some(a) = comment.attachments.get(req.n) else {
        return error_json("404 Not Found", "not_found", "no such attachment on this message");
    };
    let pass = match caller {
        DoorCaller::Pass(p) => Some(p),
        DoorCaller::Person { .. } => None,
    };
    let audit_ctx = ta::DoorAudit { surface: "ticket", detail: format!("ticket={} comment={}", item.id, comment.id) };
    ta::serve(params, &req, a, &item.project_id, pass, &audit_ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_is_an_array_or_nothing() {
        assert!(parse_field(None).ok().expect("none").is_empty());
        assert!(parse_field(Some(&serde_json::Value::Null)).ok().expect("null").is_empty());
        let got = parse_field(Some(&serde_json::json!([{ "relPath": "docs/a.md", "line": 2 }]))).ok().expect("an array");
        assert_eq!((got[0].path.as_str(), got[0].line), ("docs/a.md", Some(2)));
        for bad in [serde_json::json!({ "path": "a" }), serde_json::json!("[]"), serde_json::json!(3), serde_json::json!(true)] {
            let r = parse_field(Some(&bad)).unwrap_err();
            assert_eq!(r.status, "400 Bad Request", "{bad}");
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            assert_eq!(v["error"]["code"], "usage", "{bad}");
            assert_eq!(v["error"]["hint"], FIELD_HINT, "{bad}");
        }
        let eleven = serde_json::Value::Array((0..11).map(|_| serde_json::json!({ "path": "a" })).collect());
        let r = parse_field(Some(&eleven)).unwrap_err();
        assert!(r.body.contains("at most 10 files per message"), "{}", r.body);
        let r = parse_field(Some(&serde_json::json!([{ "path": "a", "grant": true }]))).unwrap_err();
        assert!(r.body.contains("\"usage\""), "{}", r.body);
    }

    #[test]
    fn attached_lines_follow_the_text_and_are_absolute() {
        let a = ThreadAttachment {
            kind: "file".into(),
            path: "out/a.png".into(),
            name: "a.png".into(),
            size: 1,
            mime: "image/png".into(),
            modified_at: 1,
            sha256: None,
            line: None,
            line_end: None,
        };
        #[cfg(not(windows))]
        let (root, want) = (Path::new("/ws/sales"), "/ws/sales/out/a.png");
        // One separator on Windows: the line names a real C:\ path.
        #[cfg(windows)]
        let (root, want) = (Path::new(r"C:\ws\sales"), r"C:\ws\sales\out\a.png");
        assert_eq!(with_attached_lines("B", Some(root), std::slice::from_ref(&a)), format!("B\nAttached: {want}"));
        assert_eq!(with_attached_lines("", Some(root), std::slice::from_ref(&a)), format!("Attached: {want}"));
        assert_eq!(with_attached_lines("B", None, std::slice::from_ref(&a)), "B");
    }

    #[test]
    fn attached_paths_use_the_platform_separator() {
        #[cfg(not(windows))]
        assert_eq!(attached_path(Path::new("/ws"), "docs/sub/a.md"), Path::new("/ws/docs/sub/a.md"));
        #[cfg(windows)]
        {
            let p = attached_path(Path::new(r"C:\ws"), "docs/sub/a.md");
            assert_eq!(p.to_str(), Some(r"C:\ws\docs\sub\a.md"));
            assert!(!p.to_string_lossy().contains('/'), "{}", p.display());
        }
    }

    #[test]
    fn hints_name_the_ticket() {
        let e = AttachError::Outside { input: "/etc/x".into() };
        assert!(ticket_hint(&e).contains("this ticket's workspace"), "{}", ticket_hint(&e));
        let e = AttachError::Secret { rel: ".env".into(), hit: k2_core::secret_paths::SecretHit::Path };
        let h = ticket_hint(&e);
        assert!(h.contains("can't ride a ticket") && h.contains("k2 vault"), "{h}");
    }
}
