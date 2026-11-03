//! `POST /cli/agent/refresh` (0.46.8, Rosson 2026-11-03): `k2 agent refresh`
//! starts a new pinned chat on the SAME harness, carrying on from the old one.
//! A long chat goes stale; this is the routine way to start clean.
//!
//! **Who.** A person at the Admin floor (the owner token, an Owner or Admin
//! login, a person in a K2 shell tab) any time. An agent only for its own
//! workspace's main session: with "Can refresh its own pinned chat" on it
//! just runs; off, it gets [`NEEDS_PERSON`], which the walls teach as the
//! `chat.refresh` card (`permission::actions::chat_refresh`). A sidecar has
//! its own chat, so it is refused ([`MAIN_ONLY`]), and so is a sandboxed
//! session. Another workspace's chat: `other_workspace` (later work).
//!
//! **What a refresh keeps** (`pinned_chat::run_swap`, which treats any swap
//! onto the same harness as a refresh):
//! - `workspace_sessions.session_id` moves to the new chat. The old chat's
//!   transcript stays where the CLI wrote it, in Chat history.
//! - The Thread moves with it (`hold_thread_for_new_chat`): the id it's keyed
//!   on is the provider's chat id, so a new chat would otherwise open empty.
//!   A premint harness moves it at the pin; a self-minting one (codex, pi, …)
//!   when K2 finds the new id, up to about a minute later, and until then
//!   the workspace address shows the old key. A daemon restart in that window
//!   strands the history on the old key (kept, not shown). For two minutes the
//!   resolver won't converge on the newest chat on disk, which is the old one
//!   until the new chat writes something.
//! - The next chat gets a summary of the old one, the agent's note and the
//!   handoff file, once its CLI is ready (`send_message_after_spawn`).
//!
//! **What it stops.** The pinned chat's process, at once: there is no wait for
//! idle, so a running turn stops where it is (what it already wrote is in the
//! old transcript, and the summary is taken before the kill). The k2-ptyd
//! session ends with it (`kill` then `AckExit`). Sidecars keep running.
//! Heartbeats on the pinned chat follow it; ones with their own session stay
//! on theirs and are listed in the answer.
//!
//! **Ids.** Three things are called a session id here and they differ: a
//! passport's is the daemon's PTY id, `K2_SESSION_ID` is the provider's chat
//! id when the argv names one (else the PTY id, fixed at spawn), and
//! `workspace_sessions.session_id` is the provider's chat id (NULL until a
//! self-minting CLI's is found). So "is this the main session" compares the
//! passport's map key, never `K2_SESSION_ID`, and "which chat is being
//! replaced" reads the row, never the caller's environment.

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::{AgentCaller, Caller};
use k2_core::workspace::canonical_swap::{normalize_harness, store_refresh_note, NOTES_CHAR_CAP};

pub const ROUTE: &str = "/cli/agent/refresh";
pub const MAX_BODY: usize = crate::agent_harness::MAX_BODY;
/// The card action its walls teach.
pub const CARD_ACTION: &str = "chat.refresh";
/// An agent asking with the switch off.
pub const NEEDS_PERSON: &str = "chat_refresh_needs_person";
/// A sidecar, or a sandboxed session.
pub const MAIN_ONLY: &str = "chat_refresh_main_only";
/// The last refresh's new chat has no id yet.
pub const SETTLING: &str = "chat_refresh_settling";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Req {
    #[serde(default)]
    workspace: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    dry_run: bool,
}

fn refused(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "error": { "code": code, "hint": hint.into() }, "refreshed": false, "swapped": false }).to_string(),
    }
}

fn audit(caller: &Caller, outcome: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "chat-refresh",
        &caller.audit_user(),
        outcome.to_string(),
        ingress,
        "-",
        "cli",
    ));
}

/// The name a refresh's handoff is stamped from when an agent runs it.
pub(crate) fn agent_name(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

/// May this agent passport refresh this (its own) workspace's chat now? The
/// one rule for both doors (`k2 agent refresh` and `k2 workspace
/// swap-canonical` on the same harness). `Err` is the refusal to send.
pub(crate) fn agent_gate(agent: &AgentCaller, project_id: &str, project_path: &str, notes: &str, mode: &str) -> Result<(), CliResponse> {
    if agent_may_refresh(agent, project_id)? {
        return Ok(());
    }
    Err(needs_person(project_id, project_path, notes, mode))
}

/// `Ok(true)` it may run, `Ok(false)` a person has to allow it, `Err` it can't
/// ask at all.
fn agent_may_refresh(agent: &AgentCaller, project_id: &str) -> Result<bool, CliResponse> {
    if agent.project_id != project_id {
        return Err(refused(
            "403 Forbidden",
            "other_workspace",
            "an agent can't refresh another workspace's pinned chat; a person runs `k2 agent refresh <workspace>` from a terminal outside K2",
        ));
    }
    // The passport's map key, not K2_SESSION_ID (see the module doc).
    if agent.map_key.as_deref() != Some(project_id) || agent.sandboxed {
        return Err(refused(
            "403 Forbidden",
            MAIN_ONLY,
            "a sidecar can't refresh itself. A new sidecar knows nothing of this chat, so write a handover to a file first, then ask your main session (`k2 msg <workspace>`) to stop you and open a sidecar under a new name with `--brief <file>`. `k2 sidecar new` on the same name resumes this chat.",
        ));
    }
    Ok(k2_core::workspace::settings::agents_can_refresh_chat(project_id))
}

/// The refusal an agent gets with the switch off. With cards on for it, its
/// note is kept and named by hash, so the printed `Ask with:` line carries it
/// (the walls read `/error/notes` and `/error/mode`).
fn needs_person(project_id: &str, project_path: &str, notes: &str, mode: &str) -> CliResponse {
    let ws = agent_name(project_id);
    let hint = format!(
        "Refreshing ends this chat and starts a new one, so a person allows it. Finish your work first: when they run it, this chat ends \
         at once. Your note was not kept: save it to a file and give your person the whole command, with the file's full path, \
         `k2 agent refresh {ws} --notes-file <path>`, to run from a terminal outside K2. Or ask them to pick \"Continue in a new chat…\" in \
         the pinned chat's menu (tell them where the note is), or to turn on \"Can refresh its own pinned chat\" (Settings → Workspaces \
         → {ws} → Agent)."
    );
    let mut error = json!({ "code": NEEDS_PERSON, "hint": hint, "workspace": ws });
    if k2_core::permission::gate::enabled_for(project_id) {
        error["hint"] = json!(format!(
            "Refreshing ends this chat and starts a new one, so a person allows it. Save your work first, then ask with the line below: \
             K2 keeps your note for the card. Once you've asked, this chat can end at any moment, so start nothing long. Don't run \
             `k2 agent refresh` again: the card does it. Allowed, this chat ends and the new chat is told; denied, a `[permission:]` line \
             comes here. {ws}'s owner or an admin may also allow it from now on when they answer."
        ));
        if !notes.trim().is_empty() {
            match store_refresh_note(project_path, notes.trim()) {
                Ok(sha) => error["notes"] = json!(sha),
                Err(e) => return refused("500 Internal Server Error", "notes_not_saved", format!("could not keep your note for the card: {e}")),
            }
        }
        if mode == "full" {
            error["mode"] = json!("full");
        }
    }
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({ "error": error, "refreshed": false, "swapped": false }).to_string(),
    }
}

/// Handler for `POST /cli/agent/refresh`. Body: `{workspace?, notes?, mode?,
/// dryRun?}`; an agent may leave `workspace` out (its own).
pub fn handle(body: &[u8], caller: &Caller, ingress: &str) -> CliResponse {
    match caller {
        Caller::WrongCredential => {
            return refused(
                "403 Forbidden",
                "wrong_credential",
                "k2 agent refresh takes the owner token, a Connect login, or a K2 session passport (not an app pass, API key, or API cell)",
            )
        }
        Caller::Invalid => return refused("403 Forbidden", "invalid_token", "invalid or missing token"),
        _ => {}
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return refused("400 Bad Request", "usage", format!("parse agent refresh request: {}", e.to_string().replace('"', "'"))),
    };
    let mode = match req.mode.trim() {
        "" | "recent" => "recent",
        "full" => "full",
        other => return refused("400 Bad Request", "usage", format!("mode must be recent or full (got {other})")),
    };
    if req.notes.chars().count() > NOTES_CHAR_CAP {
        return refused("400 Bad Request", "notes_too_long", format!("notes are at most {NOTES_CHAR_CAP} characters"));
    }
    let token = match (req.workspace.trim(), caller) {
        ("", Caller::Agent(agent)) => agent.project_id.clone(),
        ("", _) => return refused("400 Bad Request", "usage", "usage: k2 agent refresh <workspace> [--note <text> | --notes-file <path>] [--full] [--dry-run]"),
        (w, _) => w.to_string(),
    };
    let target = match crate::agent_harness::resolve(&token) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let mut needs_a_person = false;
    let from = match caller {
        Caller::Agent(agent) => {
            // A dry run keeps no note and hands out no line to ask with: run
            // on that line, a card would carry no note.
            match agent_may_refresh(agent, &target.id) {
                Err(r) => {
                    audit(caller, "refused", ingress);
                    return r;
                }
                Ok(false) if req.dry_run => needs_a_person = true,
                Ok(false) => {
                    audit(caller, "refused", ingress);
                    return needs_person(&target.id, &target.path, &req.notes, mode);
                }
                Ok(true) => {}
            }
            agent_name(&target.id)
        }
        Caller::Login { username } => {
            use k2_core::connect_users::Role;
            match k2_core::connect_users::role_for_user(username) {
                Some(Role::Owner) | Some(Role::Admin) => username.clone(),
                other => {
                    audit(caller, "refused:admin_only", ingress);
                    let role = other.map(|r| r.as_wire()).unwrap_or("unknown");
                    return refused(
                        "403 Forbidden",
                        "admin_only",
                        format!("refreshing a pinned chat needs the owner or an Admin login; you are signed in as {role}"),
                    );
                }
            }
        }
        // 0.46.0 stamp rule: the owner's linked username, else `owner`.
        _ => k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken),
    };

    let current = crate::agent_harness::current_harness(&target.id, &target.path);
    let harness = match normalize_harness(&current) {
        Ok(h) => h,
        Err(_) => {
            return refused(
                "409 Conflict",
                "chat_refresh_custom",
                format!("{} runs on {current}, a custom program, so K2 has no chat to summarize. Restart it with `k2 agent restart`.", target.handle),
            )
        }
    };
    let launches = match crate::pinned_chat::preflight_harness(&target.path, harness) {
        Ok(found) => found,
        Err(r) => {
            audit(caller, &format!("refused:{}:{harness}", crate::pinned_chat::HARNESS_NOT_INSTALLED), ingress);
            return r;
        }
    };
    let own_session = crate::agent_harness::own_session_heartbeats(&target.id);
    if req.dry_run {
        return CliResponse::ok_json(
            json!({
                "refreshed": false,
                "dryRun": true,
                "workspace": target.handle,
                "path": target.path,
                "harness": harness,
                "launches": launches,
                "noteChars": req.notes.trim().chars().count(),
                "needsPerson": needs_a_person,
                "ownSessionHeartbeats": own_session,
            })
            .to_string(),
        );
    }
    match crate::pinned_chat::run_swap(&target.path, harness, mode, &req.notes, &from, None) {
        Err(r) => {
            audit(caller, &format!("failed:{harness}"), ingress);
            r
        }
        Ok(mut v) => {
            audit(caller, &format!("refreshed:{harness}"), ingress);
            let from_session = v["fromSessionId"].as_str().unwrap_or("").to_string();
            let o = v.as_object_mut().expect("run_swap answers an object");
            o.insert("workspace".into(), Value::String(target.handle.clone()));
            o.insert("path".into(), Value::String(target.path.clone()));
            o.insert("harness".into(), Value::String(harness.to_string()));
            o.insert("oldChatInHistory".into(), json!(crate::agent_harness::listed_in_history(&target.path, &from_session)));
            o.insert("ownSessionHeartbeats".into(), json!(own_session));
            CliResponse::ok_json(v.to_string())
        }
    }
}

#[cfg(test)]
#[path = "chat_refresh_tests.rs"]
mod tests;
