//! `POST /cli/clone/preview` (prd-clone-carries-threads-and-vault-v1 §4.1,
//! §7.1, CL25): the names-only plan the Clone dialog and `k2 clone
//! preview` show before anything is built.
//!
//! ```text
//! POST /cli/clone/preview {project_path | workspace, sinceDays?}
//! → { ok, workspace:{id,handle,path}, history:{sessions,files,entries},
//!     threads:{available, …B's summary…},
//!     secrets:{available, rows:[{name,kind,group,default,copyable,note}], notCarried:[…]},
//!     envFiles:[rel paths], callerCanCopySecrets }
//! ```
//!
//! Gated by the clone gate (door `preview`) in the dispatcher. Never a
//! value: secret rows are names/kinds/defaults, `.env` files are paths.
//!
//! The Thread summary comes from the Threads slice and the secret rows
//! from the vault slice; until those land, [`sources`] answers
//! `available: false` and the dialog shows the Thread tick (always
//! offered, R1) without counts and no Secrets block.

use crate::cli_response::CliResponse;
use crate::compute_routes::{no, refusal_response, Who, R};
use k2_core::clone;
use k2_core::clone::DestinationClass;
use k2_core::connect_users::Role;
use serde_json::{json, Value as J};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewBody {
    /// The workspace's absolute path, or (CLI) its name, handle or id.
    #[serde(default)]
    project_path: String,
    #[serde(default)]
    workspace: String,
    #[serde(default)]
    since_days: Option<u32>,
}

/// Where the Thread summary and secret rows come from (the Threads and
/// vault slices). Each answers `None` when its slice isn't built in.
pub(crate) mod sources {
    use serde_json::Value as J;

    /// `{conversations, docs, participants:[…], …}` for the Thread block.
    pub fn threads(_project_id: &str, _since_days: Option<u32>) -> Option<J> {
        None
    }

    /// `{rows:[{name,kind,group,default,copyable,note}], notCarried:[…]}`.
    pub fn secrets(_project_id: &str, _member: bool) -> Option<J> {
        None
    }
}

/// D1: only an Owner/Admin person can confirm a secret copy.
pub fn caller_can_copy_secrets(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

pub fn handle(who: &Who, body: &[u8]) -> CliResponse {
    match preview(who, body) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

fn preview(who: &Who, body: &[u8]) -> R<J> {
    let b: PreviewBody =
        serde_json::from_slice(body).map_err(|e| no("bad_request", 400, format!("invalid JSON body: {e}")))?;
    let token = if b.project_path.trim().is_empty() { b.workspace.trim() } else { b.project_path.trim() };
    if token.is_empty() {
        return Err(no("bad_request", 400, "name the workspace (project_path or workspace)"));
    }
    let ws = if token.starts_with('/') {
        let canonical = k2_core::fs_commands::validate_path(token).map_err(|e| no("bad_request", 400, e))?;
        crate::compute_routes::ws_by_token(&canonical.to_string_lossy())
    } else {
        crate::compute_routes::ws_by_token(token)
    }
    .ok_or_else(|| no("bad_request", 400, format!("not a registered workspace on this server: {token}")))?;
    if let Some(d) = b.since_days {
        if d == 0 || d > 3650 {
            return Err(no("bad_request", 400, "sinceDays is 1–3650"));
        }
    }
    // Names only: the inventory lists what would travel and which files
    // the scrubber would hold back (the legacy `.env` option, D4).
    let inv = clone::inventory(&ws.path, clone::CloneOptions { include_all_history: true, carry_secrets: false, home_override: None })
        .map_err(|e| no("bad_request", 400, format!("inventory failed: {e}")))?;
    let sessions = inv
        .entries
        .iter()
        .filter(|e| matches!(e.class, DestinationClass::Session | DestinationClass::Provider))
        .count();
    let files = inv.entries.iter().filter(|e| e.class == DestinationClass::Workspace).count();
    let mut env_files: Vec<String> = inv.scrubbed_secrets.clone();
    env_files.sort();
    let member = !caller_can_copy_secrets(who);
    let threads = match sources::threads(&ws.id, b.since_days) {
        Some(mut t) => {
            t["available"] = json!(true);
            t
        }
        None => json!({ "available": false }),
    };
    let secrets = match sources::secrets(&ws.id, member) {
        Some(mut s) => {
            s["available"] = json!(true);
            s
        }
        None => json!({ "available": false, "rows": [], "notCarried": [] }),
    };
    Ok(json!({
        "ok": true,
        "workspace": { "id": ws.id, "handle": ws.handle, "path": ws.path },
        "history": { "sessions": sessions, "files": files, "entries": inv.entries.len() },
        "threads": threads,
        "secrets": secrets,
        "envFiles": env_files,
        "callerCanCopySecrets": !member,
    }))
}
