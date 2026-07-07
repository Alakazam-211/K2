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

use k2_core::workspace::agent_identity::{resolve_agent_name, resolve_project_id};
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
}

/// Run the full smart-launch flow. Returns a JSON value matching the
/// shape `triage::handle_heartbeat_fire` returns so existing CLI/UI
/// callers parse it without changes.
///
/// Entry point for manual launches (Launch button, `k2 heartbeat
/// launch`) — always an on-time fire. The scheduler tick calls
/// [`smart_launch_with_origin`] so catch-up fires carry their
/// originally-scheduled time into the audit trail.
pub fn smart_launch(project_path: &str, name: &str) -> serde_json::Value {
    smart_launch_with_origin(project_path, name, None)
}

/// [`smart_launch`] with fire provenance. `catchup_of` is the RFC3339
/// originally-scheduled time when this fire recovers a missed
/// occurrence (evaluator returned `DueCatchUp`); success then audits
/// `fired_catchup` instead of `fired` so the trail distinguishes
/// on-time fires from recovered misses (owner decision 1 of the
/// reliability overhaul).
pub fn smart_launch_with_origin(
    project_path: &str,
    name: &str,
    catchup_of: Option<&str>,
) -> serde_json::Value {
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
            catchup_of,
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

    // On-disk existence for the self-heal branch — a saved session
    // whose on-disk conversation was never written (daemon restart in
    // the spawn → wakeup-write window) would make a `--resume <ghost>`
    // fail; the planner routes that to a fresh fire instead.
    //
    // Slice 3b: the probe goes through the RESOLVED default agent's
    // ProviderResume adapter (was a hardcoded
    // `claude_session_file_exists`). A claude default keeps the
    // byte-identical probe; a non-claude default probes its own store
    // (so a grok-preminted `last_session_id` verifies against grok's
    // sessions dir); an unknown provider can never verify → treated as
    // missing, so the planner self-heals to a fresh fire (clearing the
    // ghost id) instead of attempting a wrong-grammar resume. The same
    // gate also self-heals a CROSS-provider stale id (default agent
    // changed after the id was stamped): the new provider's store
    // doesn't contain it → fresh fire.
    let resolved_default = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_resolve::resolve_agent_command(&conn, project_path)
    };
    let saved_jsonl_exists = saved_session
        .as_deref()
        .map(|s| {
            k2_core::workspace::provider_resume::provider_resume_for_command(
                &resolved_default.command,
            )
            .map(|adapter| adapter.session_file_exists(s, project_path))
            .unwrap_or(false)
        })
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
            run_fresh_fire(project_path, &project_id, &agent_name, &hb, &wakeup_abs, catchup_of)
        }
        LaunchDecision::Inject { .. } => {
            // Prefer the stamped (2a) handle; fall back to the argv (2b)
            // handle. The planner already guaranteed one is present + alive.
            let session_id = saved_session.clone().unwrap_or_default();
            if let Some(live) = active_candidate {
                run_inject(project_path, &project_id, &agent_name, &hb,
                    &wakeup_abs, &session_id, String::new(), live, catchup_of)
            } else if let Some((live_agent, live)) = argv_candidate {
                run_inject(project_path, &project_id, &agent_name, &hb,
                    &wakeup_abs, &session_id, live_agent, live, catchup_of)
            } else {
                // Unreachable: planner returned Inject only when a
                // candidate existed. Defensive fall-through to resume.
                run_resume_and_fire(project_path, &project_id, &agent_name, &hb,
                    &wakeup_abs, &session_id, &resolved_default, catchup_of)
            }
        }
        LaunchDecision::ResumeAndFire { claude_session_id } => {
            run_resume_and_fire(project_path, &project_id, &agent_name, &hb,
                &wakeup_abs, &claude_session_id, &resolved_default, catchup_of)
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
    // #70: DB-canonical name so a fileless configured workspace can still
    // fire a named heartbeat.
    let agent_name = resolve_agent_name(project_path).ok_or_else(|| {
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
        // Slice 3b: grammar-aware match (adds pi's `--session`, codex's
        // leading `resume <id>` subcommand, `-r`) — strictly widens the
        // old `--session-id`/`--resume` pair scan.
        let found = k2_core::workspace::provider_resume::argv_references_session(
            &live.args(),
            session_id,
        );
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
    catchup_of: Option<&str>,
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
            let decision = write_fired_audit(project_id, agent_name, hb,
                "smart_launch: no saved session — fresh fire", catchup_of);
            serde_json::json!({
                "success": true,
                "decision": decision,
                "branch": "fresh_fire",
                "name": hb.name,
                "agent": agent_name,
                "terminalId": terminal_id,
            })
        }
        Err(e) => record_fire_failure(
            project_id, agent_name, hb, &format!("fresh fire spawn failed: {e}"),
        ),
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
    catchup_of: Option<&str>,
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
    // Issue #9: heartbeats are the canonical wake initiator — they MUST
    // wake a dormant canonical session (that's the whole point of the
    // schedule firing), so pass `wake: true` with the default bounded
    // readiness timeout.
    let result = crate::workspace_msg::deliver_live(
        project_path,
        &prompt,
        "heartbeat",
        "",
        true,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
    );

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
        let decision = write_fired_audit(project_id, agent_name, hb,
            &format!("smart_launch (use_workspace_session): {branch} → {target_id}"),
            catchup_of);
        serde_json::json!({
            "success": true,
            "decision": decision,
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
        record_fire_failure(project_id, agent_name, hb, &audit_msg)
    }
}

fn run_inject(
    _project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
    session_id: &str,
    _live_agent: String,
    live: session_lookup::LiveSession,
    catchup_of: Option<&str>,
) -> serde_json::Value {
    let body_raw = match std::fs::read_to_string(wakeup_abs) {
        Ok(s) => s,
        Err(e) => {
            return record_fire_failure(
                project_id, agent_name, hb,
                &format!("inject failed reading WAKEUP.md: {e}"),
            );
        }
    };
    let body = wake::strip_frontmatter(&body_raw);
    let body_trimmed = body.trim();
    if body_trimmed.is_empty() {
        return record_fire_failure(project_id, agent_name, hb, "WAKEUP.md body is empty");
    }

    if let Err(e) = live.write(body_trimmed.as_bytes()) {
        return record_fire_failure(
            project_id, agent_name, hb, &format!("inject write to live PTY failed: {e}"),
        );
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

    let decision = write_fired_audit(project_id, agent_name, hb,
        &format!("smart_launch: injected into live session {target_id}"), catchup_of);
    serde_json::json!({
        "success": true,
        "decision": decision,
        "branch": "injected",
        "name": hb.name,
        "agent": agent_name,
        "claudeSessionId": session_id,
        "targetSessionId": target_id,
    })
}

/// How the wakeup prompt reaches a resumed session
/// ([`plan_resume_fire`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumePromptDelivery {
    /// Claude's `--print` one-shot: the prompt rides argv as the
    /// positional user turn; claude responds + exits (ephemeral PTY).
    PositionalArg,
    /// Every other known provider: interactive resume, prompt written
    /// into the PTY after a startup settle (the same two-phase write
    /// `wake_headless` uses). No flags are invented.
    PtyWrite(String),
}

/// Pure argv assembly for the ResumeAndFire branch (Slice 3b).
///
/// - claude → BYTE-IDENTICAL to the pre-Slice-3 hardcode:
///   `<preset args> --dangerously-skip-permissions? --print --resume
///   <sid> <prompt>` (the skip flag is guaranteed once, claude-only),
///   delivered positionally.
/// - any other provider with a ProviderResume adapter → the adapter's
///   own resume grammar (`grok --resume <sid>`, `codex resume <sid>`,
///   `pi --session <sid>`), spawned interactively with the prompt
///   PTY-written post-spawn.
/// - unknown provider → `None`; the caller degrades to a fresh fire
///   (the Slice-2 gate behavior, now scoped to genuinely unknown
///   commands like hermes).
pub fn plan_resume_fire(
    resolved: &k2_core::workspace::agent_resolve::ResolvedAgentCommand,
    session_id: &str,
    prompt: &str,
) -> Option<(Vec<String>, ResumePromptDelivery)> {
    if resolved.is_claude() {
        // Resume + --print: rejoin the saved conversation, deliver the
        // wakeup as the next user turn, claude responds + exits. PTY is
        // ephemeral so it doesn't accumulate stale entries in the
        // daemon session map. Base args come from the resolved preset
        // (so a customized claude preset keeps its flags); headless
        // fires can't answer permission prompts, so the skip flag is
        // guaranteed (CLAUDE-ONLY — never invented for others).
        let mut args = resolved.args.clone();
        k2_core::workspace::agent_resolve::ensure_flag(
            &mut args,
            "--dangerously-skip-permissions",
        );
        args.extend([
            "--print".to_string(),
            "--resume".to_string(),
            session_id.to_string(),
            prompt.to_string(),
        ]);
        return Some((args, ResumePromptDelivery::PositionalArg));
    }
    let adapter = k2_core::workspace::provider_resume::provider_resume_for_command(
        &resolved.command,
    )?;
    Some((
        adapter.resume_args(&resolved.args, session_id),
        ResumePromptDelivery::PtyWrite(prompt.to_string()),
    ))
}

#[allow(clippy::too_many_arguments)]
fn run_resume_and_fire(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
    session_id: &str,
    resolved: &k2_core::workspace::agent_resolve::ResolvedAgentCommand,
    catchup_of: Option<&str>,
) -> serde_json::Value {
    let Some(prompt) = wake::compose_wake_prompt_from_path(wakeup_abs) else {
        return auto_disable_missing_wakeup(project_id, agent_name, hb, wakeup_abs);
    };

    // Slice 3b: the resume argv + prompt-delivery mode come from the
    // provider adapter (claude byte-identical; other known providers
    // resume interactively in their own grammar). A resolved default
    // with NO adapter (hermes, custom commands) can't resume the saved
    // session — degrade to a fresh fire, exactly the Slice-2 gate.
    let Some((args, delivery)) = plan_resume_fire(resolved, session_id, &prompt) else {
        return run_fresh_fire(project_path, project_id, agent_name, hb, wakeup_abs, catchup_of);
    };

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
            command: Some(resolved.command.clone()),
            args: Some(args),
            cols: 120,
            rows: 38,
            canonical_key: canonical_key_override,
            // W2: the resolved preset's migration-0070 env.
            env: resolved.env_map(),
        },
    );

    match outcome {
        Ok(out) => {
            // Slice 3b — non-claude resume is interactive; deliver the
            // wakeup into the freshly-resumed PTY with the same
            // two-phase settle write `wake_headless` uses (body →
            // 150ms → CR after a startup beat). Claude's --print path
            // carried the prompt positionally and skips this.
            if let ResumePromptDelivery::PtyWrite(prompt) = delivery {
                if let Some(session) =
                    crate::v2_session_map::lookup_by_session_id(&out.session_id)
                {
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        session.write(prompt.into_bytes());
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        session.write(b"\r".to_vec());
                    });
                } else {
                    k2_core::log_debug!(
                        "[heartbeat] post-resume lookup miss for session={} — wakeup body not delivered",
                        out.session_id
                    );
                }
            }

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
                    "command": resolved.command.as_str(),
                    "cwd": project_path,
                    "projectPath": project_path,
                    "agentName": agent_name,
                    "heartbeatName": hb.name,
                }),
            );
            stamp_fired_and_release(&project_id, &hb.name);
            let decision = write_fired_audit(project_id, agent_name, hb,
                "smart_launch: resumed session, fired wakeup", catchup_of);
            serde_json::json!({
                "success": true,
                "decision": decision,
                "branch": "resume_and_fire",
                "name": hb.name,
                "agent": agent_name,
                "claudeSessionId": session_id,
                "targetSessionId": out.session_id.to_string(),
            })
        }
        Err(e) => record_fire_failure(
            project_id, agent_name, hb, &format!("resume spawn failed: {e}"),
        ),
    }
}

// ── Failure policy (backoff + auto-disable) ────────────────────────

/// Consecutive failed fire-attempts before a heartbeat is auto-disabled
/// (owner decision: 5). A success resets the counter; a manual
/// re-enable clears the disabled state.
pub(crate) const MAX_CONSECUTIVE_FAILURES: i64 = 5;

/// Backoff base: after the Nth consecutive failure the next retry is
/// `base * 2^(N-1)` seconds out (1/2/4/8 min for N=1..4), capped below.
const FAILURE_BACKOFF_BASE_SECS: i64 = 60;
const FAILURE_BACKOFF_CAP_SECS: i64 = 3600;

/// Central failure bookkeeping for every failed fire-attempt: release
/// the lease, write the `error` audit row, bump the consecutive-failure
/// counter with its exponential-backoff window, and — at
/// [`MAX_CONSECUTIVE_FAILURES`] — auto-disable the row with the
/// distinct `auto_disabled_failing` decision + `disabled_reason =
/// 'failures'` so the UI renders a "disabled after repeated failures"
/// badge. Replaces the pre-overhaul blind retry-every-tick-forever
/// (one `error` row per minute, indefinitely).
///
/// Returns the same error JSON the failure sites used to build, so
/// callers stay one-expression.
fn record_fire_failure(
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    reason: &str,
) -> serde_json::Value {
    release_lease(project_id, &hb.name);

    // The row was loaded at smart_launch entry and the in-flight lease
    // makes fire-attempts single-flight, so `hb.consecutive_failures`
    // is the authoritative prior count.
    let new_count = hb.consecutive_failures + 1;
    let backoff_secs = FAILURE_BACKOFF_BASE_SECS
        .saturating_mul(1i64 << (new_count - 1).clamp(0, 30))
        .min(FAILURE_BACKOFF_CAP_SECS);
    let next_retry = (chrono::Utc::now() + chrono::Duration::seconds(backoff_secs)).to_rfc3339();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = AgentHeartbeat::note_fire_failure(
            &conn, project_id, &hb.name, Some(&next_retry),
        );
    }
    write_audit(project_id, agent_name, hb, "error",
        &format!("{reason} (failure {new_count}/{MAX_CONSECUTIVE_FAILURES})"));

    if new_count >= MAX_CONSECUTIVE_FAILURES {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = AgentHeartbeat::auto_disable(&conn, project_id, &hb.name, "failures");
        }
        let disable_reason = format!(
            "auto-disabled after {new_count} consecutive failed fire-attempts; \
             last error: {reason}. Re-enable from Settings → Heartbeats to retry."
        );
        write_audit(project_id, agent_name, hb, "auto_disabled_failing", &disable_reason);
        k2_core::log_debug!(
            "[heartbeat] auto-disabled hb={} project={} after {} consecutive failures",
            hb.name,
            project_id,
            new_count
        );
    }

    error_value("error", reason, &hb.name)
}

/// Watchdog half of the spawn-timeout wedge fix (misfire study §1.6.2):
/// when a `smart_launch` outlives its deadline AND its grace period,
/// conditionally release the in-flight lease it is still holding so the
/// heartbeat isn't wedged until the next daemon restart. The release is
/// age-gated (`min_lease_age_secs`, derived from the attempt's actual
/// elapsed time) so a lease re-acquired by a LATER attempt is never
/// clobbered. No-op (returns false) when the hung spawn actually
/// finished and handled its own lease.
///
/// A real release counts as a failed fire-attempt — same audit +
/// backoff + auto-disable accounting as any other failure.
pub(crate) fn force_release_hung_lease(
    project_path: &str,
    hb_name: &str,
    min_lease_age_secs: i64,
) -> bool {
    let (hb, agent_name, project_id) = match resolve_row(project_path, hb_name) {
        Ok(t) => t,
        Err(e) => {
            k2_core::log_debug!(
                "[heartbeat] watchdog release skipped — row unresolvable: {e}"
            );
            return false;
        }
    };
    let cleared = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::release_lease_if_stale(
            &conn, &project_id, hb_name, min_lease_age_secs,
        )
        .unwrap_or(0)
    };
    if cleared == 0 {
        k2_core::log_debug!(
            "[heartbeat] watchdog release no-op for {hb_name} — lease already \
             handled (spawn finished or a newer attempt holds it)"
        );
        return false;
    }
    let _ = record_fire_failure(
        &project_id,
        &agent_name,
        &hb,
        &format!(
            "spawn hung for more than {min_lease_age_secs}s; \
             in-flight lease force-released by the watchdog"
        ),
    );
    true
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

/// Success-path audit: `fired` for an on-time fire, `fired_catchup`
/// (reason carries the originally-scheduled time) when the fire
/// recovers a missed occurrence. Returns the decision string so the
/// caller can mirror it in its response JSON.
fn write_fired_audit(
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    base_reason: &str,
    catchup_of: Option<&str>,
) -> &'static str {
    let (decision, reason) = match catchup_of {
        Some(t) => (
            "fired_catchup",
            format!("{base_reason} — catch-up of occurrence scheduled {t}"),
        ),
        None => ("fired", base_reason.to_string()),
    };
    write_audit(project_id, agent_name, hb, decision, &reason);
    decision
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
        // 0062: record WHY via disabled_reason so Settings shows a
        // "WAKEUP.md missing" badge instead of a silently-off toggle.
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = AgentHeartbeat::auto_disable(&conn, project_id, &hb.name, "wakeup_missing");
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

    // ── plan_resume_fire — Slice 3b adapter-aware resume argv ────────

    use k2_core::workspace::agent_resolve::{ResolvedAgentCommand, ResolvedAgentSource};

    fn resolved_cmd(command: &str, args: &[&str]) -> ResolvedAgentCommand {
        ResolvedAgentCommand {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            preset_id: None,
            source: ResolvedAgentSource::FallbackClaude,
            danger_flags: None,
            env: None,
            readiness: None,
        }
    }

    /// CLAUDE PIN — the ResumeAndFire argv must stay byte-identical to
    /// the pre-Slice-3 hardcode, prompt delivered positionally.
    #[test]
    fn plan_resume_fire_claude_argv_is_byte_identical() {
        let r = resolved_cmd("claude", &["--dangerously-skip-permissions"]);
        let (args, delivery) =
            plan_resume_fire(&r, "SID-123", "WAKE BODY").expect("claude always plans");
        assert_eq!(
            args,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--print".to_string(),
                "--resume".to_string(),
                "SID-123".to_string(),
                "WAKE BODY".to_string(),
            ],
            "claude resume-and-fire argv must be byte-identical to pre-Slice-3"
        );
        assert_eq!(delivery, ResumePromptDelivery::PositionalArg);

        // A customized claude preset keeps its flags and the skip flag
        // is guaranteed exactly once.
        let r = resolved_cmd("claude", &["--model", "opus"]);
        let (args, _) = plan_resume_fire(&r, "S", "P").unwrap();
        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "opus".to_string(),
                "--dangerously-skip-permissions".to_string(),
                "--print".to_string(),
                "--resume".to_string(),
                "S".to_string(),
                "P".to_string(),
            ]
        );
    }

    /// A grok default resumes with grok's own flag grammar — no claude
    /// flags (`--print`/`--dangerously-skip-permissions`) invented; the
    /// prompt is PTY-written post-spawn.
    #[test]
    fn plan_resume_fire_grok_uses_adapter_grammar_no_claude_flags() {
        let r = resolved_cmd("grok", &["--always-approve"]);
        let (args, delivery) =
            plan_resume_fire(&r, "GROK-SID", "WAKE").expect("grok has an adapter");
        assert_eq!(
            args,
            vec![
                "--always-approve".to_string(),
                "--resume".to_string(),
                "GROK-SID".to_string(),
            ]
        );
        assert_eq!(delivery, ResumePromptDelivery::PtyWrite("WAKE".to_string()));
    }

    /// Codex resumes subcommand-style, dropping preset args (TS argv
    /// convention).
    #[test]
    fn plan_resume_fire_codex_is_subcommand_style() {
        let r = resolved_cmd("codex", &["--dangerously-bypass-approvals-and-sandbox"]);
        let (args, delivery) = plan_resume_fire(&r, "CDX", "WAKE").unwrap();
        assert_eq!(args, vec!["resume".to_string(), "CDX".to_string()]);
        assert_eq!(delivery, ResumePromptDelivery::PtyWrite("WAKE".to_string()));
    }

    /// Unknown providers (custom commands) can't resume — the caller
    /// must degrade to a fresh fire (Slice-2 gate parity). Stale-assert
    /// note: hermes moved OUT of this bucket when Slice 6 gave it a
    /// ProviderResume adapter (`hermes --resume <id>`), so it now plans
    /// a resume like the rest of the Big 7.
    #[test]
    fn plan_resume_fire_unknown_provider_returns_none() {
        assert!(plan_resume_fire(&resolved_cmd("aider", &[]), "S", "P").is_none());
        assert!(plan_resume_fire(&resolved_cmd("my-agent", &["--x"]), "S", "P").is_none());
    }

    /// Hermes gained an adapter in Slice 6: flag-style resume, preset
    /// args kept.
    #[test]
    fn plan_resume_fire_hermes_is_flag_style() {
        let (args, delivery) =
            plan_resume_fire(&resolved_cmd("hermes", &[]), "HRM-SID", "WAKE")
                .expect("hermes has an adapter since Slice 6");
        assert_eq!(args, vec!["--resume".to_string(), "HRM-SID".to_string()]);
        assert_eq!(delivery, ResumePromptDelivery::PtyWrite("WAKE".to_string()));
    }
}
