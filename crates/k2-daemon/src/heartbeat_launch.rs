//! Smart heartbeat launch — single entry point for the Launch button,
//! the `k2so heartbeat launch <name>` CLI verb, and the cron-tick
//! scheduler. Per the daemon-first principle, all the decision +
//! spawn logic lives here in the daemon; the Tauri command and CLI
//! sub-command are thin proxies that hit the `/cli/heartbeat/launch`
//! HTTP route which calls [`smart_launch`].
//!
//! Decision tree (matches Rosson's spec in the heartbeats PRD):
//!
//! 1. `agent_heartbeats.last_session_id` is None
//!    → Fresh fire. Spawns a PTY whose `--append-system-prompt` is
//!      the WAKEUP.md body. Post-spawn deferred-save thread writes
//!      the new Claude session id back to the row.
//!
//! 2. `last_session_id` is Some + a live PTY in `session_lookup`
//!    has `--resume <session_id>` in its args
//!    → Inject. Writes the WAKEUP.md body + `\r` to the live PTY's
//!      input — same content the fresh path would send via
//!      `--append-system-prompt`, just delivered as a turn message
//!      into the running session.
//!
//! 3. `last_session_id` is Some + no live PTY
//!    → Resume + new PTY with both `--resume <session_id>` AND
//!      `--append-system-prompt <wakeup>` so Claude resumes the
//!      saved session and immediately receives the wakeup
//!      directive.
//!
//! In all three cases a `heartbeat_fires` audit row is written so
//! `k2so heartbeat status <name>` reflects the decision.

use std::path::Path;

use k2_core::workspace::agent_identity::{find_primary_agent, resolve_project_id};
use k2_core::workspace::wake_prompts as wake;
use k2_core::db::schema::{AgentHeartbeat, HeartbeatFire};
use k2_core::session::SessionId;

use crate::session_lookup;

/// Decision returned by the planner half of smart-launch. Useful for
/// callers (and tests) that want to assert what would happen without
/// performing the spawn / write side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchDecision {
    FreshFire,
    Inject {
        /// k2so session id of the live PTY we'll write into.
        target_session_id: String,
    },
    ResumeAndFire {
        claude_session_id: String,
    },
    SkippedArchived,
    SkippedNotFound,
    SkippedNoAgent,
    SkippedWakeupMissing,
}

/// Run the full smart-launch flow. Returns a JSON value matching the
/// shape `triage::handle_heartbeat_fire` returns so existing CLI/UI
/// callers parse it without changes.
pub fn smart_launch(project_path: &str, name: &str) -> serde_json::Value {
    if name.is_empty() {
        return error_value("error", "missing 'name' parameter", name);
    }

    // Look up the heartbeat row + agent.
    let (hb, agent_name, project_id) = match resolve_row(project_path, name) {
        Ok(t) => t,
        Err(decision) => return decision,
    };

    // Validate WAKEUP.md is present so we can deliver content in any
    // of the three branches below.
    let wakeup_abs = Path::new(project_path).join(&hb.wakeup_path);
    if !wakeup_abs.exists() {
        write_audit(&project_id, &agent_name, &hb, "wakeup_file_missing",
            &format!("manual launch failed: {} not found", hb.wakeup_path));
        return error_value("wakeup_file_missing",
            &format!("WAKEUP.md missing at {}", hb.wakeup_path), name);
    }

    // Atomic claim of the in-flight lease — fixes the pre-existing
    // TOCTOU between scheduler eval and spawn. Honors the row's
    // `concurrency_policy`: under `forbid` (default) a second caller
    // sees `in_flight_started_at IS NOT NULL` and gets `false`.
    // Boot-time `sweep_stale_leases` clears leases left behind by
    // a daemon that crashed mid-spawn.
    if !acquire_lease(&project_id, &hb.name) {
        write_audit(&project_id, &agent_name, &hb, "skipped_locked",
            "smart_launch: heartbeat already in flight");
        return serde_json::json!({
            "success": false,
            "decision": "skipped_locked",
            "reason": "heartbeat already in flight",
            "name": hb.name,
        });
    }

    // 0.37.8 — opt-in: deliver into the workspace's pinned chat
    // session via `workspace_msg::deliver_live`, the same smart
    // cascade `k2so msg --wake` uses. Reuses the four-branch primitive
    // (active_terminal_id alive → inject / argv-scan / saved
    // session_id → resume_and_fire / nothing → fresh_fire) keyed on
    // `workspace_sessions` columns so heartbeat-driven activity lands
    // in the same JSONL the workspace's chat tab is reading from.
    //
    // The heartbeat's own `last_session_id` and `active_terminal_id`
    // stay untouched on this path — un-checking the flag restores the
    // legacy behavior with the historical session intact.
    if hb.use_workspace_session {
        return run_workspace_session_delivery(
            project_path,
            &project_id,
            &agent_name,
            &hb,
            &wakeup_abs,
        );
    }

    let saved_session = hb.last_session_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // ── Resolve live-PTY candidates (keeping the handles) ──────────────
    //
    // 0.39.x dismiss-reap safety: a candidate PTY is only eligible for
    // Branch 2 (inject) if its child is actually alive. The dismiss-reap
    // path (15s after a workspace leaves the Active bar) calls v2/close →
    // unregister, which clears this row's active_terminal_id and kills
    // the child — so the lookups below normally miss. But a stale pointer
    // (daemon restart mid-teardown, a zombie child whose ChildExit landed
    // before unregister) could still resolve a registered-but-dead
    // session. `find_live_for_resume` (Branch 2b) already filters dead
    // children; here we resolve the stamped-pointer candidate (Branch 2a)
    // and probe its liveness so the pure planner can reject a corpse and
    // fall through to Branch 3 (resume + fresh PTY) instead of injecting
    // a WAKEUP into a dead PTY.

    // Branch 2a candidate: the SQL-stamped active_terminal_id. This is the
    // source of truth for which PTY this heartbeat is bonded to — honored
    // first so every fire lands in the same PTY a previous fire chose,
    // even when multiple live PTYs share the same `--resume <id>` argv.
    let mut active_candidate: Option<session_lookup::LiveSession> = None;
    let mut stale_active_stamp = false;
    if let Some(active_id_str) = hb.active_terminal_id.as_deref()
        .filter(|s| !s.is_empty())
    {
        match SessionId::parse(active_id_str)
            .and_then(|sid| session_lookup::lookup_by_session_id(&sid))
        {
            Some(live) if live.is_child_alive() => active_candidate = Some(live),
            // Stamp pointed at a corpse, a dead/zombie child, or an
            // unparseable id — mark it for cleanup so we don't keep
            // stumbling on the same dead pointer next fire.
            _ => stale_active_stamp = true,
        }
    }

    // Branch 2b candidate: argv-scan fallback for the cold-start case
    // (first fire after restart, or active_terminal_id was just cleared).
    // Only alive children are returned (filtered inside the helper).
    let argv_candidate = saved_session
        .as_deref()
        .and_then(find_live_for_resume);

    // JSONL existence for the self-heal branch — a saved session whose
    // JSONL was never written (daemon restart in the spawn → wakeup-write
    // window) would make `claude --resume <ghost>` fail; the planner
    // routes that to a fresh fire instead.
    let saved_jsonl_exists = saved_session
        .as_deref()
        .map(|s| k2_core::chat_history::claude_session_file_exists(s, project_path))
        .unwrap_or(false);

    // ── Pure branch selection ──────────────────────────────────────────
    let inputs = LaunchInputs {
        saved_session_id: saved_session.clone(),
        active_terminal_candidate: active_candidate.as_ref().map(|live| LiveCandidate {
            session_id: live.session_id().to_string(),
            child_alive: true, // already gated above
        }),
        argv_scan_candidate: argv_candidate.as_ref().map(|(_, live)| LiveCandidate {
            session_id: live.session_id().to_string(),
            child_alive: true, // find_live_for_resume only returns alive
        }),
        saved_jsonl_exists,
    };
    let decision = plan_launch_decision(&inputs);

    // Clear a stale active_terminal_id stamp once the decision is made
    // (regardless of branch) so the next fire starts from clean state.
    if stale_active_stamp {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = AgentHeartbeat::clear_active_terminal_id(&conn, &project_id, &hb.name);
    }

    // ── Dispatch on the decision, reusing the held live handles ─────────
    match decision {
        LaunchDecision::FreshFire => {
            // Distinguish the two fresh-fire causes for the audit trail:
            // (a) no saved session at all, vs (b) a saved session whose
            // JSONL vanished — the latter clears the ghost id first.
            if saved_session.is_some() && !saved_jsonl_exists {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = AgentHeartbeat::clear_session_id(&conn, &project_id, &hb.name);
            }
            run_fresh_fire(project_path, &project_id, &agent_name, &hb, &wakeup_abs)
        }
        LaunchDecision::Inject { .. } => {
            // Prefer the stamped (2a) handle; fall back to the argv (2b)
            // handle. The planner already guaranteed one is present + alive.
            let session_id = saved_session.clone().unwrap_or_default();
            if let Some(live) = active_candidate {
                run_inject(project_path, &project_id, &agent_name, &hb,
                    &wakeup_abs, &session_id, String::new(), live)
            } else if let Some((live_agent, live)) = argv_candidate {
                run_inject(project_path, &project_id, &agent_name, &hb,
                    &wakeup_abs, &session_id, live_agent, live)
            } else {
                // Unreachable: planner returned Inject only when a
                // candidate existed. Defensive fall-through to resume.
                run_resume_and_fire(project_path, &project_id, &agent_name, &hb,
                    &wakeup_abs, &session_id)
            }
        }
        LaunchDecision::ResumeAndFire { claude_session_id } => {
            run_resume_and_fire(project_path, &project_id, &agent_name, &hb,
                &wakeup_abs, &claude_session_id)
        }
        // The pure planner never returns the Skipped* variants — those
        // are produced earlier in this function before branch selection.
        LaunchDecision::SkippedArchived
        | LaunchDecision::SkippedNotFound
        | LaunchDecision::SkippedNoAgent
        | LaunchDecision::SkippedWakeupMissing => {
            run_fresh_fire(project_path, &project_id, &agent_name, &hb, &wakeup_abs)
        }
    }
}

/// A candidate live PTY surfaced to the decision planner. Decouples the
/// pure branch-selection logic from the concrete `session_lookup`
/// machinery so the heartbeat-safety decision is unit-testable without
/// spawning a real child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCandidate {
    /// k2so session id of the matched PTY.
    pub session_id: String,
    /// Whether the matched PTY's child process is still alive. A reaped
    /// (dismiss-reap) or zombie PTY reports `false` and MUST NOT be
    /// chosen for inject.
    pub child_alive: bool,
}

/// Inputs to the pure heartbeat-launch decision planner. Mirrors the
/// branch-selection inputs `smart_launch` reads off the DB row +
/// session map, minus the spawn/inject side effects.
#[derive(Debug, Clone)]
pub struct LaunchInputs {
    /// `agent_heartbeats.last_session_id` (None/"" → no saved session).
    pub saved_session_id: Option<String>,
    /// The PTY resolved from the row's stamped `active_terminal_id`, if
    /// the id parsed AND the v2 map still holds it. None when the row
    /// has no stamp, the stamp is unparseable, or the session was
    /// already unregistered (e.g. by the dismiss-reap v2/close).
    pub active_terminal_candidate: Option<LiveCandidate>,
    /// The PTY found by argv-scan (`--resume`/`--session-id <saved>`),
    /// if any is registered. The dismiss-reap path unregisters the
    /// session, so this is None after a reap.
    pub argv_scan_candidate: Option<LiveCandidate>,
    /// Whether the saved session's JSONL exists on disk. False → the
    /// ghost is cleared and we fall through to a fresh fire.
    pub saved_jsonl_exists: bool,
}

/// Pure heartbeat-launch branch selection. Extracted from
/// `smart_launch` so the dismiss-reap safety invariant — a
/// reaped/dead/zombie PTY must NOT be chosen for Branch 2 (inject) —
/// is asserted directly against the decision rather than indirectly
/// through spawn side effects.
///
/// Maps to the legacy four-branch tree:
///   • no saved session                          → `FreshFire`        (Branch 1)
///   • stamped active_terminal_id, child alive    → `Inject`          (Branch 2a)
///   • argv-scan match, child alive               → `Inject`          (Branch 2b)
///   • saved session, JSONL missing               → `FreshFire`       (self-heal)
///   • saved session, no live PTY, JSONL present  → `ResumeAndFire`   (Branch 3)
///
/// CRITICAL: a candidate with `child_alive == false` is treated as
/// absent — it can never produce `Inject`. This is what makes the
/// reap-then-heartbeat sequence resume (Branch 3) instead of writing a
/// WAKEUP into a dead PTY.
pub fn plan_launch_decision(inputs: &LaunchInputs) -> LaunchDecision {
    // Branch 1: no saved session — fresh fire.
    let Some(session_id) = inputs
        .saved_session_id
        .as_deref()
        .filter(|s| !s.is_empty())
    else {
        return LaunchDecision::FreshFire;
    };

    // Branch 2a: stamped active_terminal_id, but only if its child is
    // alive. A reaped/zombie PTY is treated as absent.
    if let Some(cand) = &inputs.active_terminal_candidate {
        if cand.child_alive {
            return LaunchDecision::Inject {
                target_session_id: cand.session_id.clone(),
            };
        }
    }

    // Branch 2b: argv-scan match, same liveness gate.
    if let Some(cand) = &inputs.argv_scan_candidate {
        if cand.child_alive {
            return LaunchDecision::Inject {
                target_session_id: cand.session_id.clone(),
            };
        }
    }

    // Self-heal: saved session whose JSONL was never written → fresh.
    if !inputs.saved_jsonl_exists {
        return LaunchDecision::FreshFire;
    }

    // Branch 3: saved session, no live PTY, JSONL present — resume.
    LaunchDecision::ResumeAndFire {
        claude_session_id: session_id.to_string(),
    }
}

// ── Implementation ───────────────────────────────────────────────────

fn resolve_row(
    project_path: &str,
    name: &str,
) -> Result<(AgentHeartbeat, String, String), serde_json::Value> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path).ok_or_else(|| {
        error_value("error", &format!("project not found: {project_path}"), name)
    })?;
    let hb = AgentHeartbeat::get_by_name(&conn, &project_id, name).ok().flatten().ok_or_else(|| {
        error_value("error", &format!("no heartbeat named '{name}'"), name)
    })?;
    if hb.archived_at.is_some() {
        return Err(error_value("skipped_archived",
            &format!("heartbeat '{name}' is archived"), name));
    }
    let agent_name = find_primary_agent(project_path).ok_or_else(|| {
        error_value("error", "no scheduleable agent in this workspace", name)
    })?;
    Ok((hb, agent_name, project_id))
}

fn find_live_for_resume(session_id: &str) -> Option<(String, session_lookup::LiveSession)> {
    // Walk every live session in the daemon's two maps; collect every
    // PTY whose args claim ownership of this session_id via either:
    //
    //   --session-id <uuid>   pinned at spawn time (fresh fire path,
    //                         post-P6 default — eliminates the
    //                         deferred-save race)
    //   --resume <uuid>       attached on a resume_and_fire branch,
    //                         on the user's tab, or any subsequent
    //                         interactive resume
    //
    // Multiple PTYs CAN match (e.g. user opens a tab on a session
    // that was just resumed by cron, or has multiple tabs). When that
    // happens we prefer `tab-*` agent names — those are tabs the user
    // is actively watching in the UI — over `__lead__` / agent-named
    // sessions, which are daemon-internal PTYs the user never sees.
    // Without this preference, inject writes into a hidden PTY and
    // the user wonders where their wakeup went.
    let mut matches: Vec<(String, session_lookup::LiveSession)> = Vec::new();
    for (agent, live) in session_lookup::snapshot_all() {
        let args = live.args();
        let mut i = 0;
        let mut found = false;
        while i + 1 < args.len() {
            if (args[i] == "--session-id" || args[i] == "--resume")
                && args[i + 1] == session_id
            {
                found = true;
                break;
            }
            i += 1;
        }
        // 0.39.x dismiss-reap safety: only argv-matched PTYs whose
        // child is actually alive count. A registered-but-dead session
        // (ChildExit observed, unregister not yet run) must NOT be
        // chosen for inject — fall through to Branch 3 (resume) instead.
        if found && live.is_child_alive() {
            matches.push((agent, live));
        }
    }
    // Stable sort: tab-* first (rank 0), everything else (rank 1).
    matches.sort_by_key(|(agent, _)| if agent.starts_with("tab-") { 0 } else { 1 });
    matches.into_iter().next()
}

fn run_fresh_fire(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
) -> serde_json::Value {
    let Some(prompt) = wake::compose_wake_prompt_from_path(wakeup_abs) else {
        return auto_disable_missing_wakeup(project_id, agent_name, hb, wakeup_abs);
    };

    // 0.37.0: every daemon-driven heartbeat fresh-fire goes through
    // v2 via `wake_headless::spawn_wake_headless`. The
    // `use_session_stream` column-driven branching (legacy
    // SessionStreamSession or in-process Alacritty Legacy) is
    // retired — both legacy backends are mothballed for headless
    // wakes. The `--print` semantics, `--session-id` pinning, DB
    // writes (workspace_sessions lock, heartbeat last_session_id +
    // active_terminal_id), and HookEvent emission are unchanged.
    let result = crate::wake_headless::spawn_wake_headless(
        agent_name,
        project_path,
        &prompt,
        Some(&hb.name),
    );

    match result {
        Ok(terminal_id) => {
            stamp_fired_and_release(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "fired",
                "smart_launch: no saved session — fresh fire");
            serde_json::json!({
                "success": true,
                "decision": "fired",
                "branch": "fresh_fire",
                "name": hb.name,
                "agent": agent_name,
                "terminalId": terminal_id,
            })
        }
        Err(e) => {
            release_lease(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "error",
                &format!("fresh fire spawn failed: {e}"));
            error_value("error", &format!("spawn failed: {e}"), &hb.name)
        }
    }
}

/// 0.37.8 — opt-in branch when `hb.use_workspace_session = true`.
/// Delegates to `workspace_msg::deliver_live` so the WAKEUP.md prompt
/// lands in the workspace's pinned chat session (same JSONL the chat
/// tab attaches to) instead of the heartbeat's own saved session.
///
/// All four cascade branches (live PTY inject / argv-scan / resume +
/// fire / fresh fire) are inherited from `deliver_live`, keyed on
/// `workspace_sessions` columns rather than `workspace_heartbeats`.
fn run_workspace_session_delivery(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
) -> serde_json::Value {
    let Some(prompt) = wake::compose_wake_prompt_from_path(wakeup_abs) else {
        return auto_disable_missing_wakeup(project_id, agent_name, hb, wakeup_abs);
    };

    // 0.38.6: heartbeat-initiated delivery is internal-to-daemon, not
    // a user-typed `k2so msg` call. Sender identity is "heartbeat"
    // so the wake prompt arrives in the recipient PTY tagged with a
    // clear origin. The workspace token can be the project path
    // directly (it's already a registered project).
    // Heartbeat wake prompts carry no slash-command (empty `command`).
    let result =
        crate::workspace_msg::deliver_live(project_path, &prompt, "heartbeat", "");

    if result.success {
        let branch = result
            .branch
            .clone()
            .unwrap_or_else(|| "workspace_session".to_string());
        let target_id = result
            .target_session_id
            .clone()
            .unwrap_or_default();
        stamp_fired_and_release(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "fired",
            &format!("smart_launch (use_workspace_session): {branch} → {target_id}"));
        serde_json::json!({
            "success": true,
            "decision": "fired",
            "branch": format!("workspace_session:{branch}"),
            "name": hb.name,
            "agent": agent_name,
            "targetSessionId": target_id,
        })
    } else {
        // Compose a single audit string from the canonical reason +
        // hint so operators see both in one line.
        let reason = result
            .reason
            .as_deref()
            .unwrap_or("workspace_session delivery failed");
        let hint = result.hint.as_deref().unwrap_or("");
        let audit_msg = if hint.is_empty() {
            reason.to_string()
        } else {
            format!("{reason}: {hint}")
        };
        release_lease(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "error", &audit_msg);
        error_value("error", &audit_msg, &hb.name)
    }
}

fn run_inject(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
    session_id: &str,
    _live_agent: String,
    live: session_lookup::LiveSession,
) -> serde_json::Value {
    let body_raw = match std::fs::read_to_string(wakeup_abs) {
        Ok(s) => s,
        Err(e) => {
            release_lease(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "error",
                &format!("inject failed reading WAKEUP.md: {e}"));
            return error_value("error",
                &format!("could not read WAKEUP.md: {e}"), &hb.name);
        }
    };
    let body = wake::strip_frontmatter(&body_raw);
    let body_trimmed = body.trim();
    if body_trimmed.is_empty() {
        release_lease(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "error", "WAKEUP.md body empty");
        return error_value("error", "WAKEUP.md body is empty", &hb.name);
    }

    if let Err(e) = live.write(body_trimmed.as_bytes()) {
        release_lease(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "error",
            &format!("inject write failed: {e}"));
        return error_value("error",
            &format!("write to live PTY failed: {e}"), &hb.name);
    }
    // Two-phase: paste body, send Enter after a brief settle. Same
    // pattern the awareness-bus inject uses.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let _ = live.write(b"\r");

    stamp_fired_and_release(project_id, &hb.name);
    let target_id = live.session_id().to_string();

    // Stamp `active_terminal_id` on the heartbeat row — the live PTY
    // we just injected into is now the canonical "live" PTY for this
    // heartbeat. Future openHeartbeatTab calls will surface this PTY
    // directly instead of spawning a fresh resume. See migration 0036.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = AgentHeartbeat::save_active_terminal_id(
            &conn, project_id, &hb.name, &target_id,
        );
    }
    // #677.1 — heartbeat is now live (injected into an existing PTY).
    crate::session_events::emit_heartbeat_live("", project_id, &hb.name, true);

    write_audit(project_id, agent_name, hb, "fired",
        &format!("smart_launch: injected into live session {target_id}"));
    serde_json::json!({
        "success": true,
        "decision": "fired",
        "branch": "injected",
        "name": hb.name,
        "agent": agent_name,
        "claudeSessionId": session_id,
        "targetSessionId": target_id,
    })
}

fn run_resume_and_fire(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
    session_id: &str,
) -> serde_json::Value {
    let Some(prompt) = wake::compose_wake_prompt_from_path(wakeup_abs) else {
        return auto_disable_missing_wakeup(project_id, agent_name, hb, wakeup_abs);
    };

    // Resume + --print: rejoin the saved conversation, deliver the
    // wakeup as the next user turn, claude responds + exits. PTY is
    // ephemeral so it doesn't accumulate stale entries in the daemon
    // session map. The user's tab (if/when they open one) becomes
    // the canonical long-lived view via openHeartbeatTab's interactive
    // --resume.
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--print".to_string(),
        "--resume".to_string(),
        session_id.to_string(),
        prompt,
    ];

    // 0.37.8 — register the resumed PTY under the heartbeat's own
    // canonical key (`<project_id>:hb:<name>`) so it doesn't collide
    // with the chat tab's bare-`<project_id>` slot. See the matching
    // override in `wake_headless::spawn_wake_headless`.
    let canonical_key_override = if !hb.name.is_empty() {
        Some(format!("{project_id}:hb:{}", hb.name))
    } else {
        None
    };
    // project_id is already a function parameter; no need to look it up.
    let outcome = crate::spawn::spawn_agent_session_v2_blocking(
        crate::spawn::SpawnWorkspaceSessionRequest {
            agent_name: agent_name.to_string(),
            project_id: Some(project_id.to_string()),
            cwd: project_path.to_string(),
            command: Some("claude".to_string()),
            args: Some(args),
            cols: 120,
            rows: 38,
            canonical_key: canonical_key_override,
        },
    );

    match outcome {
        Ok(out) => {
            // 0.37.8 — heartbeat fires don't touch workspace_sessions;
            // that row is the chat tab's lane. The pre-fix
            // `k2so_agents_lock` call stamped the chat tab's
            // `active_terminal_id` to the heartbeat PTY id and
            // contributed to the lane collapse. Heartbeat's
            // `active_terminal_id` is stamped below on
            // `workspace_heartbeats` only.

            // Stamp `active_terminal_id` so openHeartbeatTab can attach
            // a tab to this newly-spawned PTY without spawning a second
            // resume. See migration 0036 + the heartbeat-active-session
            // PRD.
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = AgentHeartbeat::save_active_terminal_id(
                    &conn, project_id, &hb.name, &out.session_id.to_string(),
                );
            }
            // #677.1 — heartbeat just went live (fresh PTY spawned).
            crate::session_events::emit_heartbeat_live(
                project_path, project_id, &hb.name, true,
            );
            // Surface the new PTY to any attached UI so a tab gets
            // created (gated by show_heartbeat_sessions on the
            // renderer side per P2.6).
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::CliTerminalSpawnBackground,
                serde_json::json!({
                    "terminalId": out.session_id.to_string(),
                    "command": "claude",
                    "cwd": project_path,
                    "projectPath": project_path,
                    "agentName": agent_name,
                    "heartbeatName": hb.name,
                }),
            );
            stamp_fired_and_release(&project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "fired",
                "smart_launch: resumed session, fired wakeup");
            serde_json::json!({
                "success": true,
                "decision": "fired",
                "branch": "resume_and_fire",
                "name": hb.name,
                "agent": agent_name,
                "claudeSessionId": session_id,
                "targetSessionId": out.session_id.to_string(),
            })
        }
        Err(e) => {
            release_lease(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "error",
                &format!("resume spawn failed: {e}"));
            error_value("error", &format!("resume spawn failed: {e}"), &hb.name)
        }
    }
}

// ── Lease + stamp helpers ─────────────────────────────────────────

fn acquire_lease(project_id: &str, hb_name: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::try_acquire_heartbeat(&conn, project_id, hb_name).unwrap_or(false)
}

fn release_lease(project_id: &str, hb_name: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = AgentHeartbeat::release_heartbeat_lease(&conn, project_id, hb_name);
}

/// Atomic stamp of `last_fired` + clear the in-flight lease. Called
/// only on successful spawn paths; the failure paths use
/// `release_lease` and leave `last_fired` untouched so the heartbeat
/// stays eligible for the next tick.
fn stamp_fired_and_release(project_id: &str, hb_name: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = AgentHeartbeat::stamp_fired_and_release(&conn, project_id, hb_name);
}

fn write_audit(
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    decision: &str,
    reason: &str,
) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = HeartbeatFire::insert_with_schedule(
        &conn, project_id, Some(agent_name), Some(&hb.name),
        &hb.frequency, decision, Some(reason),
        None, None, None,
    );
}

fn error_value(decision: &str, reason: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "success": false,
        "decision": decision,
        "reason": reason,
        "name": name,
    })
}

/// 0.38.12: when `compose_wake_prompt_from_path` returns None
/// (almost always because WAKEUP.md is missing or unreadable),
/// auto-disable the heartbeat so the daemon doesn't keep firing
/// every tick.
///
/// Before this, every tick would re-attempt compose, fail, write
/// `failed to compose wake prompt` to the audit log, and try again
/// next tick — producing the chronic log spam noted in the
/// memory-leak C3PO ticket (`c9b0d9a9`).
///
/// Flipping `enabled=false` takes the row out of
/// `AgentHeartbeat::list_enabled` so subsequent ticks skip it
/// entirely. The user can re-enable from Settings → Heartbeats
/// after they restore WAKEUP.md. The audit entry uses the distinct
/// `auto_disabled` decision so it shows up clearly in the fires
/// table + sidebar.
fn auto_disable_missing_wakeup(
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
) -> serde_json::Value {
    release_lease(project_id, &hb.name);
    let reason = format!(
        "WAKEUP.md missing or unreadable at {}; heartbeat auto-disabled",
        wakeup_abs.display()
    );
    write_audit(project_id, agent_name, hb, "auto_disabled", &reason);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = AgentHeartbeat::set_enabled(&conn, project_id, &hb.name, false);
    }
    k2_core::log_debug!(
        "[heartbeat] auto-disabled hb={} project={} agent={} reason=missing-wakeup path={}",
        hb.name,
        project_id,
        agent_name,
        wakeup_abs.display()
    );
    error_value("auto_disabled", &reason, &hb.name)
}

#[cfg(test)]
mod decision_tests {
    use super::*;

    fn alive(id: &str) -> LiveCandidate {
        LiveCandidate { session_id: id.to_string(), child_alive: true }
    }
    fn dead(id: &str) -> LiveCandidate {
        LiveCandidate { session_id: id.to_string(), child_alive: false }
    }

    /// Branch 1 — no saved session is a fresh fire regardless of the
    /// other inputs.
    #[test]
    fn no_saved_session_is_fresh_fire() {
        let inputs = LaunchInputs {
            saved_session_id: None,
            active_terminal_candidate: Some(alive("term-1")),
            argv_scan_candidate: Some(alive("term-2")),
            saved_jsonl_exists: true,
        };
        assert_eq!(plan_launch_decision(&inputs), LaunchDecision::FreshFire);
    }

    /// Branch 2a — a live stamped PTY injects.
    #[test]
    fn live_active_terminal_injects() {
        let inputs = LaunchInputs {
            saved_session_id: Some("claude-abc".into()),
            active_terminal_candidate: Some(alive("term-live")),
            argv_scan_candidate: None,
            saved_jsonl_exists: true,
        };
        assert_eq!(
            plan_launch_decision(&inputs),
            LaunchDecision::Inject { target_session_id: "term-live".into() },
        );
    }

    /// THE DISMISS-REAP INVARIANT: after a reap the session is
    /// unregistered, so BOTH candidates are None and the JSONL still
    /// exists on disk → the next fire MUST resume (Branch 3), never
    /// inject into the dead PTY.
    #[test]
    fn reaped_session_resumes_not_injects() {
        let inputs = LaunchInputs {
            saved_session_id: Some("claude-reaped".into()),
            active_terminal_candidate: None, // v2/close unregistered it
            argv_scan_candidate: None,       // gone from the map too
            saved_jsonl_exists: true,        // JSONL survives the reap
        };
        let decision = plan_launch_decision(&inputs);
        assert_eq!(
            decision,
            LaunchDecision::ResumeAndFire { claude_session_id: "claude-reaped".into() },
            "a reaped/dismissed session must resume via Branch 3, not inject into a dead PTY",
        );
        // Belt-and-suspenders: assert it is specifically NOT an inject.
        assert!(
            !matches!(decision, LaunchDecision::Inject { .. }),
            "reaped session decision must never be Inject (Branch 2)",
        );
    }

    /// DEFENSE-IN-DEPTH: even if a stale `active_terminal_id` still
    /// resolves a registered-but-DEAD (zombie) PTY, the liveness gate
    /// rejects it and we resume instead of injecting into the corpse.
    #[test]
    fn stale_dead_active_terminal_resumes_not_injects() {
        let inputs = LaunchInputs {
            saved_session_id: Some("claude-zombie".into()),
            active_terminal_candidate: Some(dead("term-corpse")),
            argv_scan_candidate: None,
            saved_jsonl_exists: true,
        };
        let decision = plan_launch_decision(&inputs);
        assert_eq!(
            decision,
            LaunchDecision::ResumeAndFire { claude_session_id: "claude-zombie".into() },
        );
        assert!(!matches!(decision, LaunchDecision::Inject { .. }));
    }

    /// A dead argv-scan candidate is likewise ignored (falls to resume).
    #[test]
    fn dead_argv_scan_candidate_resumes_not_injects() {
        let inputs = LaunchInputs {
            saved_session_id: Some("claude-x".into()),
            active_terminal_candidate: None,
            argv_scan_candidate: Some(dead("term-dead-argv")),
            saved_jsonl_exists: true,
        };
        let decision = plan_launch_decision(&inputs);
        assert!(!matches!(decision, LaunchDecision::Inject { .. }));
        assert_eq!(
            decision,
            LaunchDecision::ResumeAndFire { claude_session_id: "claude-x".into() },
        );
    }

    /// Self-heal: saved session whose JSONL was never written → fresh
    /// fire (no live PTY to inject into, nothing to resume).
    #[test]
    fn missing_jsonl_falls_to_fresh_fire() {
        let inputs = LaunchInputs {
            saved_session_id: Some("claude-ghost".into()),
            active_terminal_candidate: None,
            argv_scan_candidate: None,
            saved_jsonl_exists: false,
        };
        assert_eq!(plan_launch_decision(&inputs), LaunchDecision::FreshFire);
    }
}
