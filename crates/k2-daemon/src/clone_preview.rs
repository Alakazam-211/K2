//! `POST /cli/clone/preview` (prd-clone-carries-threads-and-vault-v1 §4.1,
//! §7.1, CL25): the names-only plan the Clone dialog and `k2 clone
//! preview` show before anything is built.
//!
//! ```text
//! POST /cli/clone/preview {project_path | workspace, sinceDays?,
//!                          …slice A's CarryAsk: vault_target, clone_id, secrets,
//!                          own_box_move, send_env_files (alias carry_secrets)}
//! → { ok, workspace:{id,handle,path}, history:{sessions,files,entries},
//!     threads:{available, conversations, docs, dropped, participants[], roles{}},
//!     secrets:{available, rows:[{name,kind,group,default,choice,copyable,note?,envNames?}],
//!              notCarried:["NAME (reason)"], plan, needsConfirm, confirm?},
//!     secretFiles:[rel paths], envFiles (the same list, for older clients),
//!     envFilesHint, callerCanCopySecrets }
//! ```
//!
//! `secretFiles` is every file the clone leaves behind unless secrets are
//! carried: the shared secret list (`.env*`, key files, `.ssh/`, `.npmrc`,
//! `.netrc`, a file with a credential in it), not only `.env` files.
//!
//! Gated by the clone gate (door `preview`) in the dispatcher. Never a
//! value: secret rows are names/kinds/choices, `.env` files are paths.
//!
//! The Thread block reads slice B's export (`ThreadsExport::{summary,
//! participants}`); the Secrets block is slice A's plan (`clone_vault::
//! plan_json`), so `secrets.confirm` is the exact `clone-carry` binding the
//! person confirms next (`/cli/clone/vault/confirm`, or `vault.sock`
//! `confirm/finish` with `clonePlan`). Without a `vault_target` every
//! unnamed row is `skip` (nothing can travel yet).

use crate::cli_response::CliResponse;
use crate::compute_routes::{no, refusal_response, Who, R};
use crate::vault::actor::VaultCaller;
use k2_core::clone;
use k2_core::clone::DestinationClass;
use k2_core::connect_users::Role;
use serde_json::{json, Value as J};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewBody {
    /// The workspace's absolute path, or (CLI) its name, handle or id.
    #[serde(default, rename = "project_path", alias = "projectPath")]
    project_path: String,
    #[serde(default)]
    workspace: String,
    #[serde(default)]
    since_days: Option<u32>,
}

/// Where the Thread summary and secret rows come from (slices B and A).
pub(crate) mod sources {
    use super::*;
    use k2_core::overlay::transfer as t;

    /// `{conversations, docs, dropped, participants[], roles{}}` for the
    /// Thread block (slice B's export, built and dropped: names, counts).
    pub fn threads(project_id: &str, since_days: Option<u32>) -> Option<J> {
        let choice = match since_days {
            Some(d) => t::ThreadsChoice::SinceDays(d),
            None => t::ThreadsChoice::All,
        };
        let db = k2_core::db::shared();
        let conn = db.lock();
        let export = match t::export_project_threads(&conn, project_id, &t::ExportOptions::new("preview", t::default_source_host(), choice)) {
            Ok(e) => e,
            Err(e) => {
                k2_core::log_debug!("[clone/preview] thread summary for {project_id}: {e}");
                return None;
            }
        };
        drop(conn);
        let s = export.summary(0);
        let mut roles = serde_json::Map::new();
        for c in &export.conversations {
            let n = roles.get(&c.role).and_then(J::as_u64).unwrap_or(0);
            roles.insert(c.role.clone(), json!(n + 1));
        }
        Some(json!({
            "conversations": s.conversations,
            "docs": s.docs,
            "dropped": s.dropped,
            "participants": export.participants(),
            "roles": roles,
        }))
    }

    /// Slice A's plan for this caller and ask, reshaped for the dialog.
    pub fn secrets(caller: Option<&VaultCaller>, project_path: &str, body: &[u8]) -> Option<J> {
        let caller = caller?;
        let vault = crate::vault::get().ok()?;
        let mut ask: crate::clone_vault::CarryAsk = serde_json::from_slice(body).unwrap_or_default();
        ask.project_path = Some(project_path.to_string());
        ask.project_id = None;
        ask.confirm_token = None;
        let v = match crate::clone_vault::plan_json(&vault, caller, &ask) {
            Ok(v) => v,
            Err(e) => {
                k2_core::log_debug!("[clone/preview] vault plan: {} {}", e.code, e.hint);
                return None;
            }
        };
        let plan = &v["plan"];
        let rows: Vec<J> = plan["rows"]
            .as_array()
            .map(|rs| {
                rs.iter()
                    .map(|r| {
                        let mut o = json!({
                            "name": r["name"], "kind": r["kind"], "group": r["group"],
                            "default": r["default"], "choice": r["choice"],
                            "copyable": r["copyAllowed"].as_bool().unwrap_or(false),
                        });
                        if !r["note"].is_null() {
                            o["note"] = r["note"].clone();
                        }
                        if let Some(e) = r.get("envNames") {
                            o["envNames"] = e.clone();
                        }
                        o
                    })
                    .collect()
            })
            .unwrap_or_default();
        let not_carried: Vec<String> = plan["notCarried"]
            .as_array()
            .map(|ns| {
                ns.iter()
                    .map(|n| format!("{} ({})", n["name"].as_str().unwrap_or("?"), n["reason"].as_str().unwrap_or("?")))
                    .collect()
            })
            .unwrap_or_default();
        Some(json!({
            "rows": rows,
            "notCarried": not_carried,
            "member": plan["member"],
            "needsConfirm": v["needsConfirm"],
            "confirm": v.get("confirm").cloned().unwrap_or(J::Null),
            "envFilesHint": v["envFilesHint"],
            "plan": plan.clone(),
        }))
    }
}

/// D1: only an Owner/Admin person can confirm a secret copy.
pub fn caller_can_copy_secrets(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

pub fn handle(who: &Who, caller: Option<&VaultCaller>, body: &[u8]) -> CliResponse {
    match preview(who, caller, body) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        // `error` stays a string (the dialog shows it verbatim), like the
        // clone gate's refusals.
        Err(e) => {
            let mut r = refusal_response(&e);
            r.body = serde_json::json!({ "error": e.hint, "code": e.code, "hint": e.hint }).to_string();
            r
        }
    }
}

fn preview(who: &Who, caller: Option<&VaultCaller>, body: &[u8]) -> R<J> {
    let b: PreviewBody =
        serde_json::from_slice(body).map_err(|e| no("bad_request", 400, format!("invalid JSON body: {e}")))?;
    let token = if b.project_path.trim().is_empty() { b.workspace.trim() } else { b.project_path.trim() };
    if token.is_empty() {
        return Err(no("bad_request", 400, "name the workspace (project_path or workspace)"));
    }
    let ws = if token.starts_with('/') {
        // The path as registered first (a /tmp spelling stays /tmp), then
        // its canonical form (validate_path refuses traversal).
        let canonical = k2_core::fs_commands::validate_path(token).map_err(|e| no("bad_request", 400, e))?;
        crate::compute_routes::ws_by_token(token).or_else(|| crate::compute_routes::ws_by_token(&canonical.to_string_lossy()))
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
    // the scrubber would hold back (the `.env` option, D4).
    let inv = clone::inventory(&ws.path, clone::CloneOptions { include_all_history: true, carry_secrets: false, ..Default::default() })
        .map_err(|e| no("bad_request", 400, format!("inventory failed: {e}")))?;
    let sessions = inv
        .entries
        .iter()
        .filter(|e| matches!(e.class, DestinationClass::Session | DestinationClass::Provider))
        .count();
    let files = inv.entries.iter().filter(|e| e.class == DestinationClass::Workspace).count();
    // Every file the shared secret list holds back (not only `.env`).
    let mut secret_files: Vec<String> = inv.scrubbed_secrets.clone();
    secret_files.sort();
    let can_copy = caller_can_copy_secrets(who);
    let threads = match sources::threads(&ws.id, b.since_days) {
        Some(mut t) => {
            t["available"] = json!(true);
            t
        }
        None => json!({ "available": false }),
    };
    let secrets = match sources::secrets(caller, &ws.path, body) {
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
        "secretFiles": secret_files,
        // 0.46.0 clients read this name; it is the same list.
        "envFiles": secret_files,
        "callerCanCopySecrets": can_copy,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The preview names every file the clone would leave behind as a
    /// secret, not only `.env`: key files, `.ssh/`, `.npmrc`, `.netrc`
    /// (the shared secret list). `envFiles` stays the same list for 0.46.0
    /// clients; ordinary files are never in it.
    #[test]
    fn preview_lists_every_secret_file_the_clone_leaves_behind() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let id = format!("cp-secret-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let dir = std::env::temp_dir().join(format!("k2-clone-preview-{id}"));
        let w = |rel: &str, body: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        w(".env", "API_KEY=x\n");
        w("deploy/server.pem", "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n");
        w(".ssh/id_ed25519", "not really a key\n");
        w(".npmrc", "//registry.npmjs.org/:_authToken=abc\n");
        w(".netrc", "machine example.com login a password b\n");
        w("README.md", "hello\n");
        let path = dir.to_string_lossy().to_string();
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![id, path])
                .expect("project");
        }
        let v = preview(&Who::Owner, None, json!({ "project_path": path }).to_string().as_bytes()).expect("preview");
        let files: Vec<&str> = v["secretFiles"].as_array().expect("secretFiles").iter().filter_map(J::as_str).collect();
        for want in [".env", "deploy/server.pem", ".ssh/id_ed25519", ".npmrc", ".netrc"] {
            assert!(files.contains(&want), "{want} missing from {files:?}");
        }
        assert!(!files.contains(&"README.md"), "{files:?}");
        assert_eq!(v["envFiles"], v["secretFiles"], "envFiles is the same list for older clients");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
