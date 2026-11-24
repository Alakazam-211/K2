//! Files on project chat lines (project-chat-thread-v1, P5).
//!
//! A project has no folder of its own, so a file sent on a project line is
//! kept in its **point of contact's** workspace, the agent that gets the
//! line: the attachment names that workspace (`workspace`), so it still
//! opens after the point of contact changes, and reads `gone` once that
//! workspace is removed, as any Thread file does.
//!
//! - A person's or an App guest's file is named against the point of
//!   contact's workspace (the desktop uploads into its
//!   `.k2/downloads/projects/<group id>/` first), with the Thread's checks
//!   (inside the workspace, a file, not a secret; a guest only what its
//!   own file rights cover there).
//! - A member agent's file is named against **its own** workspace (the
//!   Thread's checks there), then copied into the point of contact's
//!   `.k2/downloads/projects/<group id>/` and checked again. An agent never
//!   sends a file from outside its own workspace.
//! - The point of contact gets one `Attached: <abs path>` line per file
//!   ([`with_attached_lines`]), never stored.

use std::path::{Path, PathBuf};

use k2_core::overlay::attachments::{self as att, AttachError, AttachmentRequest, ThreadAttachment};
use k2_core::project_groups::ProjectGroup;
use k2_core::skin::SkinPass;

use crate::cli_response::CliResponse;
use crate::thread_attachments as ta;

/// A line's checked files and the folder they are relative to.
#[derive(Debug, Default)]
pub(crate) struct Files {
    pub attachments: Vec<ThreadAttachment>,
    /// The point of contact's folder (`None` with no files).
    pub root: Option<PathBuf>,
}

fn error_json(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

/// Thread's refusal, said for a project chat.
fn refusal(e: &AttachError) -> CliResponse {
    let hint = e
        .hint()
        .replace("this Thread's workspace", "this project's point of contact's workspace")
        .replace("can't ride a Thread message", "can't ride a project line");
    error_json("400 Bad Request", e.code(), hint)
}

/// The folder a project's files are copied into, relative to the point
/// of contact's workspace.
pub(crate) fn downloads_rel(group_id: &str) -> String {
    format!(".k2/downloads/projects/{}", group_id.trim())
}

/// Check (and, for a member agent's own files, copy) the files a project
/// line asks for, all or nothing, before anything is stored. `raw`: the
/// body's `attachments` (the Thread post's shape). `author_workspace`: the
/// posting agent's workspace (`None` for people and guests).
pub(crate) fn check_and_snapshot(
    group: &ProjectGroup,
    raw: Option<&serde_json::Value>,
    author_workspace: Option<&str>,
    who: &str,
) -> Result<Files, CliResponse> {
    check_and_snapshot_as(group, raw, author_workspace, None, who)
}

/// [`check_and_snapshot`] for an App pass too (`pass`: a guest's files are
/// what its own file rights cover in the point of contact's room).
pub(crate) fn check_and_snapshot_as(
    group: &ProjectGroup,
    raw: Option<&serde_json::Value>,
    author_workspace: Option<&str>,
    pass: Option<&SkinPass>,
    who: &str,
) -> Result<Files, CliResponse> {
    let reqs = crate::ticket_attachments::parse_field(raw)?;
    if reqs.is_empty() {
        return Ok(Files::default());
    }
    let Some(poc) = group.poc_workspace_id.as_deref().filter(|p| !p.trim().is_empty()) else {
        return Err(error_json(
            "400 Bad Request",
            "no_poc",
            "this project has no point of contact yet, so it has nowhere to keep files: add a member first",
        ));
    };
    let poc_root = ta::project_root(poc).ok_or_else(|| {
        error_json("404 Not Found", "not_found", "this project's point of contact's workspace is not registered; files can't be attached")
    })?;
    let home = dirs::home_dir();
    let audit_refused = |reason: &str, what: &str| {
        ta::audit("project.attachment.refused", who, format!("project={} reason={reason} path={what:?}", group.id));
    };
    if let Some(p) = pass {
        if let Err(r) = ta::guest_may_attach(p, poc, &poc_root, &reqs) {
            audit_refused("guest_scope", "-");
            return Err(r);
        }
    }
    let requests: Vec<AttachmentRequest> = match author_workspace.filter(|ws| *ws != poc) {
        // A member agent: its own files, copied over.
        Some(ws) => {
            let own_root = ta::project_root(ws)
                .ok_or_else(|| error_json("404 Not Found", "not_found", "this agent's workspace is not registered"))?;
            let own = att::snapshot_all(&own_root, home.as_deref(), &reqs).map_err(|e| {
                audit_refused(e.reason(), &refused_path(&e));
                refusal(&e)
            })?;
            let dir = poc_root.join(downloads_rel(&group.id));
            std::fs::create_dir_all(&dir).map_err(|e| error_json("500 Internal Server Error", "copy_failed", format!("can't make the project's file folder: {e}")))?;
            let mut out = Vec::with_capacity(own.len());
            for (a, r) in own.iter().zip(&reqs) {
                let src = crate::ticket_attachments::attached_path(&own_root, &a.path);
                let (dest, mut file) = k2_core::fs_commands::create_new_free(&dir, &a.name)
                    .map_err(|e| error_json("500 Internal Server Error", "copy_failed", e))?;
                let mut from = std::fs::File::open(&src).map_err(|e| error_json("500 Internal Server Error", "copy_failed", format!("{}: {e}", a.path)))?;
                std::io::copy(&mut from, &mut file).map_err(|e| error_json("500 Internal Server Error", "copy_failed", format!("{}: {e}", a.path)))?;
                let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| a.name.clone());
                out.push(AttachmentRequest { path: format!("{}/{name}", downloads_rel(&group.id)), line: r.line, line_end: r.line_end });
            }
            out
        }
        None => reqs,
    };
    let mut atts = att::snapshot_all(&poc_root, home.as_deref(), &requests).map_err(|e| {
        audit_refused(e.reason(), &refused_path(&e));
        refusal(&e)
    })?;
    for a in &mut atts {
        a.workspace = Some(poc.to_string());
    }
    ta::audit(
        "project.attachment.post",
        who,
        format!("project={} count={} rels={:?}", group.id, atts.len(), atts.iter().map(|a| a.path.as_str()).collect::<Vec<_>>()),
    );
    Ok(Files { attachments: atts, root: Some(poc_root) })
}

fn refused_path(e: &AttachError) -> String {
    match e {
        AttachError::Secret { rel, .. } => rel.clone(),
        AttachError::Outside { input } | AttachError::NotAFile { input } => input.clone(),
        AttachError::Usage(_) => "-".to_string(),
    }
}

/// What the point of contact gets for a line with files: the text, then
/// one `Attached: <abs path>` line per file (each in the workspace it is
/// kept in). Never stored.
pub(crate) fn with_attached_lines(text: &str, atts: &[ThreadAttachment], root: Option<&Path>) -> String {
    let mut t = text.to_string();
    for a in atts {
        let own = a.workspace.as_deref().and_then(ta::project_root);
        let Some(r) = own.as_deref().or(root) else { continue };
        if !t.is_empty() {
            t.push('\n');
        }
        t.push_str(&format!("Attached: {}", crate::ticket_attachments::attached_path(r, &a.path).display()));
    }
    t
}

thread_local! {
    /// The App pass a project read answers for (`project.read`), so each
    /// line's files are what that guest may see.
    static VIEW_PASS: std::cell::RefCell<Option<SkinPass>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with project reads answering for `pass`.
pub(crate) fn with_view_pass<T>(pass: &SkinPass, f: impl FnOnce() -> T) -> T {
    VIEW_PASS.with(|slot| {
        let prev = slot.replace(Some(pass.clone()));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// The files of serialized project lines as this reader sees them (the
/// Thread's per-viewer views): live `ok` / `changed` / `gone`, `hidden`
/// for a guest whose own file rights don't cover one, `canDownload` for a
/// person. A line without files is untouched.
pub(crate) fn rewrite_lines(lines: &mut serde_json::Value) {
    let Some(arr) = lines.as_array_mut() else { return };
    if !arr.iter().any(|m| m["attachments"].as_array().is_some_and(|a| !a.is_empty())) {
        return;
    }
    VIEW_PASS.with(|slot| {
        let pass = slot.borrow();
        let viewer = match pass.as_ref() {
            // Each file names its own room (`workspace`).
            Some(p) => ta::Viewer::Guest { pass: p, project_id: "" },
            None => ta::person_or_agent(),
        };
        for m in arr.iter_mut() {
            ta::rewrite_doc(m, None, &viewer, true);
        }
    });
}

/// A live frame's files (`project.events`): never more than "a file" (the
/// App reads its own view with `project.read`).
pub(crate) fn hide_files(doc: &mut serde_json::Value) {
    if let Some(arr) = doc.get_mut("attachments").and_then(|a| a.as_array_mut()) {
        for a in arr.iter_mut() {
            *a = att::hidden_json();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_lines_name_each_file_in_its_workspace() {
        let a = ThreadAttachment {
            kind: "file".into(),
            path: ".k2/downloads/projects/g1/a.png".into(),
            name: "a.png".into(),
            size: 1,
            mime: "image/png".into(),
            modified_at: 1,
            sha256: None,
            line: None,
            line_end: None,
            workspace: None,
        };
        let t = with_attached_lines("look", std::slice::from_ref(&a), Some(Path::new("/ws/poc")));
        assert_eq!(t, "look\nAttached: /ws/poc/.k2/downloads/projects/g1/a.png");
        assert_eq!(with_attached_lines("plain", &[], None), "plain");
        assert_eq!(downloads_rel(" g1 "), ".k2/downloads/projects/g1");
    }
}
