//! `POST /cli/agent/retire` — 0.40.24 S4, the safe-decommission half of
//! the agent CLI (`.k2/prds/prd-agent-cli-0.40.24.md` §3).
//!
//! Retire NEVER deletes. The flow is: preflight guards → stop the live
//! canonical session → remove connection edges → deregister → clean
//! dependent rows the deregister cascade misses → ARCHIVE (move) the
//! folder.
//!
//! ## Guards (refuse, exit 3 — never prompt)
//!
//! Per the locked PRD decision "no interactive prompts, ever", the
//! guards REFUSE with a stable `{error:{code:"refused"}}` shape instead
//! of asking:
//!
//! - **git**: uncommitted work (`status --porcelain`) or unpushed
//!   commits (`rev-list @{upstream}..HEAD`). Not-a-repo ⇒ no git guard.
//!   Detached HEAD / no upstream / unborn HEAD ⇒ we CANNOT prove the
//!   work is pushed, so that counts as a refusal with a clear hint
//!   (unpushed-unknown), not a silent pass.
//! - **secrets**: filename-pattern sniff (v1 — no content scan):
//!   `.env*`, ssh key material (`id_rsa`/`id_dsa`/`id_ecdsa`/
//!   `id_ed25519`), and `*.pem|*.key|*.p12|*.pfx`. `.git/` is skipped.
//!
//! Ahead of both (Projects V1 §4.5): the **PoC removal guard** — a
//! workspace that is the Point of Contact of any project group refuses
//! with 409 `poc_of_projects` ("X is the Point of Contact for: A, B.
//! Reassign the PoC first."). NOT `--force`-overridable; the only way
//! through is `set-poc`-ing a successor (no auto-reassignment).
//!
//! `force: true` overrides both guards. `dryRun: true` previews the
//! full plan (guards evaluated + would-do actions) and touches nothing;
//! its verdict mirrors the real run (a dry-run that WOULD refuse
//! returns the same refusal shape so callers see the same exit code).
//!
//! ## What the cascade already covers
//!
//! `remove_workspace_db_only` deletes the `projects` row; with
//! `PRAGMA foreign_keys = ON` (set by `db::shared`) that cascades
//! `workspaces`, `workspace_sections`, `workspace_sessions`,
//! `workspace_layouts`, `workspace_tab_sessions`, `tab_titles`,
//! `workspace_relations` (both directions), `workspace_remote_connections`,
//! `workspace_heartbeats`, `heartbeat_fires`, and `activity_feed`
//! (`time_entries.project_id` is `ON DELETE SET NULL` by design).
//! Deliberately added here because nothing cascades them:
//!
//! - `chat_session_names.agent_project_id` (migration 0014 `ALTER TABLE`
//!   column — SQLite can't attach an FK retroactively).
//! - the LIVE canonical PTY session (in-memory `v2_session_map`; a
//!   session running in a folder we're about to move away).
//!
//! Connection edges are still counted + removed EXPLICITLY (before the
//! project delete) so the response's `actions` can report how many
//! edges went away — and as defense-in-depth if the pragma is ever off.

use std::path::{Path, PathBuf};

use crate::cli_response::CliResponse;

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct AgentRetireBody {
    /// Workspace token: name | absolute path | UUID.
    q: String,
    /// Override the safety guards.
    force: bool,
    #[serde(rename = "dryRun")]
    dry_run: bool,
    /// FINAL destination dir for the archive move. Empty ⇒ the default
    /// `~/.k2/archive/<name>-<YYYY-MM-DD>/`. Collisions get `-2`, `-3`…
    #[serde(rename = "archiveTo")]
    archive_to: String,
}

// ── Guard evaluation ─────────────────────────────────────────────────

#[derive(Debug, Default)]
struct GitGuard {
    /// `<dir>/.git` exists — without it there is NO git guard.
    present: bool,
    uncommitted_files: usize,
    /// `Some(n)` = commits ahead of upstream; `None` = UNKNOWN
    /// (detached / no upstream / unborn HEAD / git invocation failed).
    unpushed_commits: Option<usize>,
}

impl GitGuard {
    fn trips(&self) -> bool {
        self.present
            && (self.uncommitted_files > 0
                || self.unpushed_commits.is_none()
                || self.unpushed_commits.unwrap_or(0) > 0)
    }

    /// Human fragment for the refusal hint, e.g.
    /// "uncommitted git work (3 files) and 1 unpushed commit".
    fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.uncommitted_files > 0 {
            parts.push(format!(
                "uncommitted git work ({} file{})",
                self.uncommitted_files,
                if self.uncommitted_files == 1 { "" } else { "s" }
            ));
        }
        match self.unpushed_commits {
            None => parts.push(
                "unverifiable push state (detached HEAD or no upstream configured — \
                 K2 cannot prove the work is pushed)"
                    .to_string(),
            ),
            Some(n) if n > 0 => parts.push(format!(
                "{n} unpushed commit{}",
                if n == 1 { "" } else { "s" }
            )),
            _ => {}
        }
        parts.join(" and ")
    }
}

fn run_git(dir: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}

fn evaluate_git_guard(path: &str) -> GitGuard {
    if !Path::new(path).join(".git").exists() {
        return GitGuard::default(); // not a repo — no git guard
    }
    let status = run_git(path, &["status", "--porcelain"]);
    let uncommitted_files = status
        .as_deref()
        .map(|out| out.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    // `.git` exists but `git status` itself failed → we can't prove
    // anything about the tree; bias to refusal via the unknown-unpushed
    // channel (None). Otherwise ask how far ahead of upstream we are —
    // any failure there (detached / no upstream / unborn HEAD) is the
    // documented unknown ⇒ refuse case.
    let unpushed_commits = if status.is_some() {
        run_git(path, &["rev-list", "--count", "@{upstream}..HEAD"])
            .and_then(|out| out.parse::<usize>().ok())
    } else {
        None
    };
    GitGuard {
        present: true,
        uncommitted_files,
        unpushed_commits,
    }
}

/// Filename-pattern secrets sniff (v1 contract: patterns suffice, no
/// content scan). Returns repo-relative sample paths, capped.
const SECRETS_SAMPLE_CAP: usize = 10;
const SECRETS_WALK_CAP: usize = 20_000;

fn is_secret_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.starts_with(".env") {
        return true;
    }
    if matches!(lower.as_str(), "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519") {
        return true;
    }
    match lower.rsplit_once('.') {
        Some((_, ext)) => matches!(ext, "pem" | "key" | "p12" | "pfx"),
        None => false,
    }
}

fn scan_secrets(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut visited = 0usize;
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > SECRETS_WALK_CAP {
                return found;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if name == ".git" {
                    continue;
                }
                stack.push(entry.path());
            } else if ft.is_file() && is_secret_filename(&name) {
                if found.len() < SECRETS_SAMPLE_CAP {
                    let rel = entry
                        .path()
                        .strip_prefix(root)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or(name);
                    found.push(rel);
                }
            }
        }
    }
    found.sort();
    found
}

// ── Archive destination ──────────────────────────────────────────────

/// Resolve the FINAL archive destination. `archive_to` (when non-empty)
/// IS the destination; empty falls back to
/// `~/.k2/archive/<name>-<YYYY-MM-DD>`. Either way an existing dir gets
/// a `-2`, `-3`… suffix (never merged into, never overwritten).
fn resolve_archive_dest(archive_to: &str, name: &str) -> Result<PathBuf, String> {
    let base = if archive_to.is_empty() {
        let home = dirs::home_dir().ok_or("cannot resolve the daemon's home directory")?;
        let date = chrono::Local::now().format("%Y-%m-%d");
        home.join(".k2").join("archive").join(format!("{name}-{date}"))
    } else {
        PathBuf::from(archive_to)
    };
    if !base.exists() {
        return Ok(base);
    }
    for n in 2..100 {
        let candidate = base.with_file_name(format!(
            "{}-{n}",
            base.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "archive".to_string())
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not find a free archive destination near {} (tried -2 … -99)",
        base.display()
    ))
}

// ── The handler ──────────────────────────────────────────────────────

/// Handler for `POST /cli/agent/retire`.
pub fn handle_agent_retire(body: &[u8]) -> CliResponse {
    let b: AgentRetireBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if b.q.is_empty() {
        return CliResponse::bad_request("missing 'q' (agent name | path | UUID)");
    }
    let Some(path) = crate::workspace_msg::resolve_workspace(&b.q) else {
        return crate::workspace_routes::workspace_not_found_response(&b.q);
    };
    let name = k2_core::workspace::display::agent_display_name(&path);

    // Projects V1 §4.5 — the PoC removal guard, FIRST (before the
    // git/secrets guards): a workspace that is the Point of Contact of
    // any project group cannot be retired until every affected project
    // has a successor. Deliberately NOT `--force`-overridable — the
    // only way through is reassigning the PoC (no auto-reassignment).
    if let Some(project_id) = crate::canonical_session::lookup_project_id(&path) {
        if let Some(resp) = crate::project_group_routes::poc_removal_block(&project_id, &name) {
            return resp;
        }
    }

    let folder_exists = Path::new(&path).is_dir();

    // ── Guards (always evaluated — the response reports them even
    // under --force / --dry-run so operators see what was overridden).
    let git = if folder_exists {
        evaluate_git_guard(&path)
    } else {
        GitGuard::default()
    };
    let secrets = if folder_exists {
        scan_secrets(Path::new(&path))
    } else {
        Vec::new()
    };
    let guards_json = serde_json::json!({
        "git": {
            "present": git.present,
            "uncommittedFiles": git.uncommitted_files,
            "unpushedCommits": git.unpushed_commits,
        },
        "secrets": { "count": secrets.len(), "sample": secrets },
    });

    // Refusal (mirrored by dry-run so its exit code previews the real
    // run's): guards trip and no --force.
    let trips_git = git.trips();
    let trips_secrets = !secrets.is_empty();
    if (trips_git || trips_secrets) && !b.force {
        let mut reasons: Vec<String> = Vec::new();
        if trips_git {
            reasons.push(git.describe());
        }
        if trips_secrets {
            reasons.push(format!(
                "likely secrets in the folder ({})",
                secrets
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let hint = format!(
            "{name} has {}. Nothing was changed. Resolve it (commit/push the work, move the \
             secrets out), or re-run with --force to retire anyway.",
            reasons.join(" and ")
        );
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "refused": true,
                "dryRun": b.dry_run,
                "name": name,
                "path": path,
                "guards": guards_json,
                "error": { "code": "refused", "hint": hint },
            })
            .to_string(),
        };
    }

    // ── Resolve identifiers + counts for the plan.
    let project_id = crate::canonical_session::lookup_project_id(&path);
    let canonical_key = project_id
        .as_deref()
        .map(crate::canonical_session::canonical_key_for);
    let live = canonical_key
        .as_deref()
        .and_then(crate::session_lookup::lookup_any)
        .filter(|s| s.is_child_alive());
    let (edge_count, chat_rows) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let pid = project_id.clone().unwrap_or_default();
        let edges: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM workspace_relations \
                          WHERE source_project_id = ?1 OR target_project_id = ?1) \
                      + (SELECT COUNT(*) FROM workspace_remote_connections \
                          WHERE source_project_id = ?1)",
                rusqlite::params![pid],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let chats: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_session_names WHERE agent_project_id = ?1",
                rusqlite::params![pid],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (edges as usize, chats as usize)
    };
    let dest = match resolve_archive_dest(&b.archive_to, &name) {
        Ok(d) => d,
        Err(e) => return CliResponse::bad_request(e),
    };

    // ── Build the plan (pending / skipped), mockup-shaped.
    let mut actions: Vec<serde_json::Value> = Vec::new();
    actions.push(match &live {
        Some(s) => serde_json::json!({
            "step": "stop-session", "status": "pending",
            "sessionId": s.session_id().to_string(),
        }),
        None => serde_json::json!({
            "step": "stop-session", "status": "skipped", "reason": "not live",
        }),
    });
    actions.push(if edge_count > 0 {
        serde_json::json!({ "step": "remove-connections", "status": "pending", "edges": edge_count })
    } else {
        serde_json::json!({ "step": "remove-connections", "status": "skipped", "reason": "none" })
    });
    actions.push(serde_json::json!({ "step": "deregister-workspace", "status": "pending" }));
    actions.push(if chat_rows > 0 {
        serde_json::json!({ "step": "clean-dependents", "status": "pending", "chatSessionNames": chat_rows })
    } else {
        serde_json::json!({ "step": "clean-dependents", "status": "skipped", "reason": "none" })
    });
    actions.push(if folder_exists {
        serde_json::json!({
            "step": "archive-folder", "status": "pending",
            "to": dest.to_string_lossy(),
        })
    } else {
        serde_json::json!({
            "step": "archive-folder", "status": "skipped",
            "reason": "folder missing on disk",
        })
    });

    if b.dry_run {
        return CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "dryRun": true,
                "changed": false,
                "name": name,
                "path": path,
                "archiveTo": dest.to_string_lossy(),
                "guards": guards_json,
                "forced": b.force && (trips_git || trips_secrets),
                "actions": actions,
            })
            .to_string(),
        );
    }

    // ── Apply ────────────────────────────────────────────────────────
    let fail = |actions: Vec<serde_json::Value>,
                name: &str,
                path: &str,
                hint: String|
     -> CliResponse {
        CliResponse {
            status: "500 Internal Server Error",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "changed": true,
                "name": name,
                "path": path,
                "actions": actions,
                "error": { "code": "retire_failed", "hint": hint },
            })
            .to_string(),
        }
    };

    // 1. Stop the live canonical session (must happen while the
    //    projects row still exists and before the folder moves).
    if actions[0]["status"] == "pending" {
        if let Some(key) = canonical_key.as_deref() {
            // unregister() force-kills + reaps the child and flips the
            // workspace_sessions row to sleeping — the single "v2
            // session goes away" chokepoint.
            let _ = crate::v2_session_map::unregister(key);
        }
        actions[0]["status"] = serde_json::json!("done");
    }

    // 2. Remove connection edges (both directions + remote rows).
    //    Counted explicitly so the report is concrete; the project
    //    delete below would cascade them anyway.
    if actions[1]["status"] == "pending" {
        let pid = project_id.clone().unwrap_or_default();
        let removed: Result<usize, String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            (|| {
                let a = conn
                    .execute(
                        "DELETE FROM workspace_relations \
                         WHERE source_project_id = ?1 OR target_project_id = ?1",
                        rusqlite::params![pid],
                    )
                    .map_err(|e| e.to_string())?;
                let b = conn
                    .execute(
                        "DELETE FROM workspace_remote_connections WHERE source_project_id = ?1",
                        rusqlite::params![pid],
                    )
                    .map_err(|e| e.to_string())?;
                Ok(a + b)
            })()
        };
        match removed {
            Ok(n) => {
                actions[1]["status"] = serde_json::json!("done");
                actions[1]["removed"] = serde_json::json!(n);
            }
            Err(e) => {
                actions[1]["status"] = serde_json::json!("failed");
                return fail(actions, &name, &path, format!("removing connections failed: {e}"));
            }
        }
    }

    // 3. Deregister (projects + workspaces rows; FK cascade cleans the
    //    dependent tables listed in the module docs).
    match k2_core::workspace::lifecycle::remove_workspace_db_only(&path) {
        Ok(_) => actions[2]["status"] = serde_json::json!("done"),
        Err(e) => {
            actions[2]["status"] = serde_json::json!("failed");
            return fail(actions, &name, &path, format!("deregister failed: {e}"));
        }
    }

    // 4. Dependent rows the cascade misses (chat_session_names has no
    //    FK on agent_project_id — 0014 ALTER column).
    if actions[3]["status"] == "pending" {
        let pid = project_id.clone().unwrap_or_default();
        let res = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "DELETE FROM chat_session_names WHERE agent_project_id = ?1",
                rusqlite::params![pid],
            )
        };
        match res {
            Ok(n) => {
                actions[3]["status"] = serde_json::json!("done");
                actions[3]["chatSessionNames"] = serde_json::json!(n);
            }
            Err(e) => {
                actions[3]["status"] = serde_json::json!("failed");
                return fail(
                    actions,
                    &name,
                    &path,
                    format!("cleaning chat_session_names failed: {e}"),
                );
            }
        }
    }

    // 5. Archive move — LAST, so a failure here leaves a fully
    //    deregistered agent with its folder intact in place.
    if actions[4]["status"] == "pending" {
        let move_res = (|| -> Result<(), String> {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            std::fs::rename(&path, &dest).map_err(|e| {
                format!(
                    "cannot move {} → {}: {e} (the agent is already deregistered; \
                     move the folder manually or leave it in place)",
                    path,
                    dest.display()
                )
            })
        })();
        match move_res {
            Ok(()) => actions[4]["status"] = serde_json::json!("done"),
            Err(e) => {
                actions[4]["status"] = serde_json::json!("failed");
                return fail(actions, &name, &path, e);
            }
        }
    }

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "changed": true,
            "name": name,
            "path": path,
            "archivedTo": if actions[4]["status"] == "done" {
                serde_json::json!(dest.to_string_lossy())
            } else {
                serde_json::Value::Null
            },
            "guards": guards_json,
            "forced": b.force && (trips_git || trips_secrets),
            "actions": actions,
        })
        .to_string(),
    )
}

// ──────────────────────────────────────────────────────────────────────
// Tests — shared in-memory DB (unique names/paths per test) + real temp
// dirs on disk for the guard/archive behavior.
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(label: &str) -> (String, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "k2-retire-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create workspace dir");
        (dir.to_string_lossy().to_string(), dir)
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project row");
        id
    }

    fn project_exists(id: &str) -> bool {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get::<_, i64>(0),
        )
        .expect("count") > 0
    }

    fn body(q: &str, force: bool, dry_run: bool, archive_to: &str) -> Vec<u8> {
        serde_json::json!({
            "q": q, "force": force, "dryRun": dry_run, "archiveTo": archive_to,
        })
        .to_string()
        .into_bytes()
    }

    fn unique_name(label: &str) -> String {
        format!("retire-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn retire_unknown_token_404s() {
        let resp = handle_agent_retire(&body("no-such-agent-zz", false, false, ""));
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
    }

    #[test]
    fn retire_refuses_on_secrets_and_changes_nothing() {
        let name = unique_name("secrets");
        let (path, dir) = temp_workspace("secrets");
        let id = insert_project(&name, &path);
        std::fs::write(dir.join(".env"), "API_KEY=shh").unwrap();

        let resp = handle_agent_retire(&body(&name, false, false, ""));
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["refused"], true);
        assert_eq!(v["error"]["code"], "refused");
        assert!(
            v["error"]["hint"].as_str().unwrap().contains("secrets"),
            "hint must name the guard: {}",
            resp.body
        );
        assert_eq!(v["guards"]["secrets"]["count"], 1);
        // Nothing was changed: row still registered, folder in place.
        assert!(project_exists(&id), "refusal must not deregister");
        assert!(dir.is_dir(), "refusal must not move the folder");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retire_git_guard_refuses_dirty_repo() {
        let name = unique_name("gitdirty");
        let (path, dir) = temp_workspace("gitdirty");
        let id = insert_project(&name, &path);
        // Real repo with an uncommitted file. (No upstream either — both
        // reasons trip; the hint must surface the uncommitted work.)
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .expect("run git");
            assert!(out.status.success(), "git {args:?} failed: {out:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("notes.md"), "wip").unwrap();

        let resp = handle_agent_retire(&body(&name, false, false, ""));
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let hint = v["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("uncommitted git work (1 file)"), "hint={hint}");
        assert!(project_exists(&id));
        assert!(dir.is_dir());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retire_dry_run_previews_and_touches_nothing() {
        let name = unique_name("dry");
        let (path, dir) = temp_workspace("dry");
        let id = insert_project(&name, &path);
        let archive = std::env::temp_dir().join(format!("k2-retire-dry-dest-{id}"));

        let resp = handle_agent_retire(&body(
            &name,
            false,
            true,
            &archive.to_string_lossy(),
        ));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["dryRun"], true);
        assert_eq!(v["changed"], false);
        let actions = v["actions"].as_array().unwrap();
        assert!(
            actions.iter().all(|a| a["status"] == "pending" || a["status"] == "skipped"),
            "dry-run must only plan: {actions:?}"
        );
        // The would-do plan includes the archive destination.
        assert_eq!(v["archiveTo"].as_str(), Some(archive.to_string_lossy().as_ref()));
        // Nothing happened.
        assert!(project_exists(&id));
        assert!(dir.is_dir());
        assert!(!archive.exists(), "dry-run must not create the archive");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retire_dry_run_mirrors_the_refusal_verdict() {
        // A dry-run that WOULD refuse returns the refusal shape (so the
        // CLI previews the same exit-3 the real run would produce).
        let name = unique_name("dryrefuse");
        let (path, dir) = temp_workspace("dryrefuse");
        let id = insert_project(&name, &path);
        std::fs::write(dir.join("server.key"), "not really").unwrap();

        let resp = handle_agent_retire(&body(&name, false, true, ""));
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["refused"], true);
        assert_eq!(v["dryRun"], true);
        assert!(project_exists(&id));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retire_force_archives_deregisters_and_cleans_edges() {
        let name = unique_name("force");
        let (path, dir) = temp_workspace("force");
        let id = insert_project(&name, &path);
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        std::fs::write(dir.join("work.md"), "the agent's files").unwrap();

        // A peer with edges in BOTH directions + a remote edge + a
        // chat_session_names row keyed to this agent.
        let peer_name = unique_name("force-peer");
        let (peer_path, peer_dir) = temp_workspace("force-peer");
        let peer_id = insert_project(&peer_name, &peer_path);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO workspace_relations (id, source_project_id, target_project_id, relation_type) \
                 VALUES (?1, ?2, ?3, 'peer'), (?4, ?3, ?2, 'peer')",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    id,
                    peer_id,
                    uuid::Uuid::new_v4().to_string(),
                ],
            )
            .expect("insert relation rows");
            conn.execute(
                "INSERT INTO workspace_remote_connections (id, source_project_id, remote_addr, host, agent) \
                 VALUES (?1, ?2, 'ai@rpm.k2.dev', 'rpm.k2.dev', 'ai')",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), id],
            )
            .expect("insert remote edge");
            conn.execute(
                "INSERT INTO chat_session_names (provider, session_id, custom_name, agent_project_id) \
                 VALUES ('claude', ?1, 'old chat', ?2)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), id],
            )
            .expect("insert chat_session_names row");
        }

        let archive = std::env::temp_dir().join(format!("k2-retire-force-dest-{id}"));
        let resp = handle_agent_retire(&body(
            &name,
            true,
            false,
            &archive.to_string_lossy(),
        ));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["changed"], true);
        assert_eq!(v["forced"], true, "guards were overridden: {}", resp.body);
        assert_eq!(v["archivedTo"].as_str(), Some(archive.to_string_lossy().as_ref()));

        // Folder MOVED (never deleted): archive holds the files, source gone.
        assert!(!dir.exists(), "source folder must be gone");
        assert!(archive.join("work.md").is_file(), "files must survive the move");
        assert!(archive.join(".env").is_file(), "even the secrets move (never deleted)");

        // Deregistered + edges gone in BOTH directions + remote row +
        // chat_session_names row.
        assert!(!project_exists(&id));
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let edges: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM workspace_relations \
                     WHERE source_project_id = ?1 OR target_project_id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(edges, 0, "all edges touching the agent must be gone");
            let remotes: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM workspace_remote_connections WHERE source_project_id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(remotes, 0);
            let chats: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM chat_session_names WHERE agent_project_id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(chats, 0, "non-cascaded dependents must be cleaned");
        }
        // The peer itself is untouched.
        assert!(project_exists(&peer_id));

        // Action report is complete.
        let actions = v["actions"].as_array().unwrap();
        let by_step = |s: &str| {
            actions
                .iter()
                .find(|a| a["step"] == s)
                .unwrap_or_else(|| panic!("missing step {s}: {actions:?}"))
                .clone()
        };
        assert_eq!(by_step("remove-connections")["status"], "done");
        assert_eq!(by_step("remove-connections")["removed"], 3);
        assert_eq!(by_step("deregister-workspace")["status"], "done");
        assert_eq!(by_step("clean-dependents")["chatSessionNames"], 1);
        assert_eq!(by_step("archive-folder")["status"], "done");

        std::fs::remove_dir_all(&archive).ok();
        std::fs::remove_dir_all(&peer_dir).ok();
    }

    #[test]
    fn retire_clean_folder_needs_no_force() {
        // No repo, no secrets → no guard, retire proceeds without force.
        let name = unique_name("clean");
        let (path, dir) = temp_workspace("clean");
        let id = insert_project(&name, &path);
        std::fs::write(dir.join("README.md"), "hi").unwrap();

        let archive = std::env::temp_dir().join(format!("k2-retire-clean-dest-{id}"));
        let resp = handle_agent_retire(&body(&name, false, false, &archive.to_string_lossy()));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["forced"], false);
        assert!(!project_exists(&id));
        assert!(archive.join("README.md").is_file());
        assert!(!dir.exists());

        std::fs::remove_dir_all(&archive).ok();
    }

    #[test]
    fn archive_destination_collision_appends_suffix() {
        let name = unique_name("collide");
        let (path, dir) = temp_workspace("collide");
        let id = insert_project(&name, &path);
        std::fs::write(dir.join("data.md"), "x").unwrap();

        let archive = std::env::temp_dir().join(format!("k2-retire-collide-dest-{id}"));
        std::fs::create_dir_all(&archive).unwrap(); // occupy the primary dest

        let resp = handle_agent_retire(&body(&name, false, false, &archive.to_string_lossy()));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let expected = archive.with_file_name(format!(
            "{}-2",
            archive.file_name().unwrap().to_string_lossy()
        ));
        assert_eq!(v["archivedTo"].as_str(), Some(expected.to_string_lossy().as_ref()));
        assert!(expected.join("data.md").is_file());
        assert!(!dir.exists());

        std::fs::remove_dir_all(&archive).ok();
        std::fs::remove_dir_all(&expected).ok();
    }
}
