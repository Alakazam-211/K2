//! The first pinned chat asks which LLM to start (0.46.11, Rosson
//! 2026-11-10).
//!
//! A new agent is stamped with the global default agent, which is `claude`
//! on a new install (`projects_ops::stamp_default_agent`), so the Chat tab's
//! `ensure-pinned-chat` used to start Claude on its own (or fail with
//! `claude: not found` when it isn't installed). Now, on a server that has
//! never had a pinned chat, a never-chatted agent whose chat would start on
//! that untouched default (or on a CLI that isn't installed) answers
//! `409 harness_choice_needed` with the harnesses to pick from, and starts
//! nothing. The person picks one in the Chat tab (signing in or installing
//! first if they need to), and the renderer sends the same ensure with
//! `harness`: [`choose`] saves it as the workspace's default agent
//! (`projects.default_agent`, the existing per-workspace setting) and the
//! ensure starts that harness as it always has.
//!
//! Everything else is as before: an agent that already has a pinned chat
//! (any `workspace_sessions` row), any agent on a server that has had one,
//! and an agent whose default was set to another harness that is installed.

use serde::Serialize;

use k2_core::workspace::agent_resolve::{parse_command_string, resolve_agent_command};
use k2_core::workspace::provider_resume::provider_resume_for_command;

use crate::cli_response::CliResponse;

/// Refusal code: the first chat waits for the person to pick its harness.
pub const CHOICE_NEEDED: &str = "harness_choice_needed";

/// Offered even when not installed: the tools K2 signs in to (and installs).
const ALWAYS_OFFERED: &[&str] = &["claude", "codex", "grok"];

/// One harness the person can start the chat on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    /// Provider key (`claude`, `codex`, `grok`, …): what `harness` takes.
    pub harness: String,
    /// The enabled preset's label.
    pub label: String,
    /// The program is on the PATH agents run with.
    pub installed: bool,
    /// K2 can install it (`POST /cli/agents/ensure-cli`).
    pub install: bool,
    /// A K2 sign-in for it is signed in, or an API token is saved.
    pub signed_in: bool,
    /// K2 can sign it in here (a subscription tool, not air-gapped).
    pub sign_in: bool,
}

/// What [`facts`] reads under one DB lock.
struct Facts {
    project_id: String,
    has_pinned_chat: bool,
    server_had_pinned_chat: bool,
    default_command: String,
    default_is_claude: bool,
}

fn facts(project_path: &str) -> Option<Facts> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let project_id: String = conn
        .query_row("SELECT id FROM projects WHERE path = ?1", [project_path], |r| r.get(0))
        .ok()?;
    // A read error counts as "has one": the ensure then runs as before.
    let exists = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> bool {
        conn.query_row(sql, params, |r| r.get::<_, bool>(0)).unwrap_or(true)
    };
    let has_pinned_chat = exists(
        "SELECT EXISTS(SELECT 1 FROM workspace_sessions WHERE project_id = ?1)",
        &[&project_id],
    );
    let server_had_pinned_chat = exists("SELECT EXISTS(SELECT 1 FROM workspace_sessions)", &[]);
    let default_cmd = resolve_agent_command(&conn, project_path);
    Some(Facts {
        project_id,
        has_pinned_chat,
        server_had_pinned_chat,
        default_is_claude: default_cmd.is_claude(),
        default_command: default_cmd.command,
    })
}

/// Can a never-chatted chat of `provider` start here (same program and PATH
/// the spawn would use)?
fn launchable(project_path: &str, provider: &str) -> bool {
    let Ok((program, env)) = k2_core::workspace::resume_chat::never_chatted_program(project_path, provider) else {
        return false;
    };
    let env: std::collections::HashMap<String, String> = env.map(|m| m.into_iter().collect()).unwrap_or_default();
    k2_core::terminal::daemon_pty::preflight_program(&program, &env).is_ok()
}

/// `Some(choices)` when this workspace's pinned chat must ask before it
/// starts; `None` to start it as before.
pub fn needs_choice(project_path: &str) -> Option<Vec<Choice>> {
    let f = facts(project_path)?;
    if f.has_pinned_chat || f.server_had_pinned_chat {
        return None;
    }
    // A custom preset as the default was chosen on purpose.
    let adapter = provider_resume_for_command(&f.default_command)?;
    if !f.default_is_claude && launchable(project_path, adapter.provider) {
        return None;
    }
    k2_core::log_debug!(
        "[daemon/pinned-chat] first chat of {} waits for a harness pick (default {})",
        f.project_id,
        adapter.provider
    );
    Some(choices(project_path))
}

/// The enabled presets K2 can start a pinned chat on, one per harness, in
/// launch-bar order: Claude, Codex and Grok always, any other only when
/// installed.
pub fn choices(project_path: &str) -> Vec<Choice> {
    let (presets, logins) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (enabled_presets(&conn), k2_core::llm_accounts::list(&conn).unwrap_or_default())
    };
    let airgap = k2_core::airgap::enabled();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (_, label, command) in presets {
        let (program, _) = parse_command_string(&command);
        let Some(adapter) = provider_resume_for_command(&program) else { continue };
        if !seen.insert(adapter.provider) {
            continue;
        }
        let installed = launchable(project_path, adapter.provider);
        if !installed && !ALWAYS_OFFERED.contains(&adapter.provider) {
            continue;
        }
        let basename = program.rsplit(['/', '\\']).next().unwrap_or(&program);
        let tool = k2_core::llm_accounts::Tool::parse(adapter.provider);
        out.push(Choice {
            harness: adapter.provider.to_string(),
            label,
            installed,
            install: k2_core::terminal::ensure_cli::fixed_install_command(basename).is_some(),
            signed_in: logins.iter().any(|e| {
                e.tool == adapter.provider
                    && (e.state == k2_core::llm_accounts::state::SIGNED_IN || e.is_api_key())
            }),
            sign_in: !airgap && tool.is_some_and(|t| t.subscription_supported()),
        });
    }
    out
}

/// (id, label, command) of every enabled preset, in display order.
fn enabled_presets(conn: &rusqlite::Connection) -> Vec<(String, String, String)> {
    let Ok(mut stmt) =
        conn.prepare("SELECT id, label, command FROM agent_presets WHERE enabled = 1 ORDER BY sort_order, label")
    else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// The 409 the ensure answers while the first chat waits for a pick.
pub fn choice_needed_response(choices: &[Choice]) -> CliResponse {
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: serde_json::json!({
            "error": {
                "code": CHOICE_NEEDED,
                "hint": "Choose the LLM this agent's chat starts with in its Chat tab (Claude, Codex, Grok or another one installed here). Nothing has started yet.",
                "harnesses": choices,
            },
        })
        .to_string(),
    }
}

/// The person picked `harness` for this workspace's first chat: save it as
/// the workspace's default agent, so the ensure that follows starts it. An
/// agent that already has a pinned chat keeps it (the pick is ignored; the
/// chat menu's "Move pinned chat to …" changes a harness). `Err` is the
/// answer to send instead of starting anything.
pub fn choose(project_path: &str, harness: &str) -> Result<(), CliResponse> {
    let target = k2_core::workspace::canonical_swap::normalize_harness(harness).map_err(CliResponse::bad_request)?;
    let Some(f) = facts(project_path) else {
        return Err(CliResponse::bad_request(format!("project not registered: {project_path}")));
    };
    if f.has_pinned_chat {
        return Ok(());
    }
    let preset = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        enabled_presets(&conn).into_iter().find(|(_, _, command)| {
            let (program, _) = parse_command_string(command);
            provider_resume_for_command(&program).map(|a| a.provider) == Some(target)
        })
    };
    let Some((preset_id, label, command)) = preset else {
        return Err(CliResponse::bad_request(format!(
            "no enabled agent preset runs {target}; turn one on in Settings → Agents"
        )));
    };
    // Same check as the spawn, before anything is saved.
    if !launchable(project_path, target) {
        let (program, _) = parse_command_string(&command);
        return Err(CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: serde_json::json!({
                "error": {
                    "code": crate::pinned_chat::HARNESS_NOT_INSTALLED,
                    "hint": format!(
                        "{}. Nothing has started yet.",
                        k2_core::sidecar::install_hint(&program)
                    ),
                    "harness": target,
                    "label": label,
                    "program": program,
                },
            })
            .to_string(),
        });
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET default_agent = ?1 WHERE id = ?2",
        rusqlite::params![preset_id, f.project_id],
    )
    .map_err(|e| CliResponse::internal_error(format!("save the pick: {e}")))?;
    Ok(())
}
