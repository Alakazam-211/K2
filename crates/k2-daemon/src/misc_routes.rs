//! Daemon-side catch-all `/cli/*` dispatch for the long tail of route
//! domains (task #578 extraction).
//!
//! After the big clusters (`agents_routes`, `workspace_routes`) split
//! off, the remaining ~90 arms covered many small domains:
//!   - per-project mode / settings / worktree / agentic toggles
//!   - review queue (reviews / review/{approve,reject,feedback})
//!   - companion tunnel + globals, K2 Connect tunnel status
//!   - workspace connections / states
//!   - agent channel (status / done / reserve / release)
//!   - skills fan-out, activity feed, AI-commit emit
//!   - per-project heartbeat schedule, hook diagnostic
//!   - the `/cli/heartbeat/*` CRUD passthrough
//!   - terminal IO / lifecycle / spawn (delegate to terminal_routes)
//!   - companion session enumeration, session lookups
//!   - onboarding flow, What's New popup, claude-auth status
//!   - the Phase 2 Unit 4/6 GET delegators (db / git / fs / chat /
//!     themes / skill-layers / review-checklist / project-config /
//!     inbox / glossary)
//!
//! Each arm is byte-for-byte the original inline body; only the code
//! location moved. Shared param/respond helpers live in `crate::cli`.

use std::collections::HashMap;

use crate::cli::{bool_param, need_project, opt_param, respond, respond_unit, str_param};
use crate::cli_response::CliResponse;

/// Long-tail `/cli/*` dispatch. Returns `Some(resp)` for a handled
/// path, `None` if unknown (caller renders 404).
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        // ── Per-project mode + settings toggles ─────────────────────
        "/cli/mode" => match need_project(params) {
            Ok(p) => {
                if let Some(mode) = opt_param(params, "set") {
                    match k2_core::workspace::settings::update_project_setting(&p, "agent_mode", &mode) {
                        Ok(()) => {
                            k2_core::agent_hooks::emit(
                                k2_core::agent_hooks::HookEvent::SyncProjects,
                                serde_json::Value::Null,
                            );
                            // 0.37.2: when mode flips to a bot mode AND
                            // AGENT.md exists, proactively spawn the
                            // canonical PTY + register workspace_sessions.
                            // Without this, the SMS-bridge race window
                            // (mode → AGENT.md write → first webhook
                            // inbound, all sub-second) lets `--wake`
                            // race ahead and spawn a session that the
                            // sidebar's window pane never sees. Filed by
                            // nsi-checkin Scout deployment as the
                            // "canonical PTY initialization" issue.
                            // Best-effort — not having an agent yet
                            // (operator is between `mode` and AGENT.md
                            // write) is the common case and isn't an
                            // error; just log and let the next caller
                            // (or boot sweep) handle it.
                            let bot_mode = matches!(mode.as_str(),
                                "custom" | "manager" | "k2so");
                            let agent_md = std::path::PathBuf::from(&p)
                                .join(".k2so/agent/AGENT.md");
                            let mut ensure_summary = serde_json::Value::Null;
                            if bot_mode && agent_md.exists() {
                                match crate::canonical_session::ensure_canonical_session(&p) {
                                    Ok(out) => {
                                        ensure_summary = serde_json::json!({
                                            "session_id": out.session_id,
                                            "agent": out.agent_name,
                                            "reused": out.reused,
                                        });
                                    }
                                    Err(e) => {
                                        k2_core::log_debug!(
                                            "[daemon/canonical] mode={mode} \
                                             ensure_canonical_session skipped \
                                             for {p}: {e}"
                                        );
                                    }
                                }
                            }
                            CliResponse::ok_json(
                                serde_json::json!({
                                    "success": true,
                                    "mode": mode,
                                    "canonical": ensure_summary,
                                }).to_string(),
                            )
                        }
                        Err(e) => CliResponse::bad_request(e),
                    }
                } else {
                    // Read current mode. Falls back to filesystem-
                    // detection if DB has no row.
                    match k2_core::workspace::settings::get_project_settings(&p) {
                        Ok(settings) => CliResponse::ok_json(
                            serde_json::to_string(&settings).unwrap_or_default(),
                        ),
                        Err(_) => {
                            let k2so_dir = k2_core::workspace_dot_dir(&p);
                            let agents_dir = k2so_dir.join("agents");
                            let has_agents = agents_dir.exists()
                                && std::fs::read_dir(&agents_dir)
                                    .map(|e| e.count() > 0)
                                    .unwrap_or(false);
                            let claude_md =
                                std::path::PathBuf::from(&p).join("CLAUDE.md");
                            let mode = if !claude_md.exists() {
                                "off"
                            } else if has_agents {
                                "manager"
                            } else {
                                "agent"
                            };
                            CliResponse::ok_json(
                                serde_json::json!({"mode": mode}).to_string(),
                            )
                        }
                    }
                }
            }
            Err(r) => r,
        },
        "/cli/settings" => match need_project(params) {
            Ok(p) => match k2_core::workspace::settings::get_project_settings(&p) {
                Ok(s) => CliResponse::ok_json(serde_json::to_string(&s).unwrap_or_default()),
                Err(e) => CliResponse::bad_request(e),
            },
            Err(r) => r,
        },
        "/cli/worktree" => match need_project(params) {
            Ok(p) => {
                let enable = bool_param(params, "enable");
                let value = if enable { "1" } else { "0" };
                match k2_core::workspace::settings::update_project_setting(&p, "worktree_mode", value) {
                    Ok(()) => {
                        k2_core::agent_hooks::emit(
                            k2_core::agent_hooks::HookEvent::SyncProjects,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "worktreeMode": enable})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },
        "/cli/agentic" => {
            // Global toggle, not project-specific.
            if let Some(enable) = opt_param(params, "enable") {
                let on = enable == "1" || enable == "true" || enable == "on";
                match k2_core::workspace::settings::set_agentic_enabled(on) {
                    Ok(()) => {
                        k2_core::agent_hooks::emit(
                            k2_core::agent_hooks::HookEvent::SyncSettings,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "agenticEnabled": on})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            } else {
                let enabled = k2_core::workspace::settings::get_agentic_enabled();
                CliResponse::ok_json(
                    serde_json::json!({"agenticEnabled": enabled}).to_string(),
                )
            }
        }

        // ── Review queue ────────────────────────────────────────────
        "/cli/reviews" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::reviews::review_queue(&p)),
            Err(r) => r,
        },
        "/cli/review/approve" => match need_project(params) {
            Ok(p) => {
                let branch = str_param(params, "branch");
                let agent = str_param(params, "agent");
                let agent_for_emit = agent.clone();
                let project_for_emit = p.clone();
                match k2_core::workspace::reviews::review_approve(p, branch, agent) {
                    Ok(msg) => {
                        // #675.3/.4 — the queue + this review changed.
                        emit_review_changed(&project_for_emit, Some(&agent_for_emit));
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "message": msg}).to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },
        "/cli/review/reject" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let project_for_emit = p.clone();
                let result = k2_core::workspace::reviews::review_reject(
                    p,
                    agent.clone(),
                    opt_param(params, "reason"),
                );
                if result.is_ok() {
                    emit_review_changed(&project_for_emit, Some(&agent));
                }
                respond_unit(result)
            }
            Err(r) => r,
        },
        "/cli/review/feedback" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let project_for_emit = p.clone();
                let result = k2_core::workspace::reviews::review_request_changes(
                    p,
                    agent.clone(),
                    str_param(params, "feedback"),
                );
                if result.is_ok() {
                    emit_review_changed(&project_for_emit, Some(&agent));
                }
                respond_unit(result)
            }
            Err(r) => r,
        },

        // ── Settings (Phase 2 Unit 7a) ──────────────────────────────
        // GET only; update + reset are POST-allowlisted in main.rs
        // because they have bodies / are destructive.
        "/cli/settings/get" => crate::settings_routes::handle_settings_get(),

        // ── Companion tunnel + globals ──────────────────────────────
        "/cli/companion/start" => match k2_core::companion::start_companion() {
            Ok(url) => CliResponse::ok_json(
                serde_json::json!({"ok": true, "url": url}).to_string(),
            ),
            Err(e) => CliResponse::bad_request(e),
        },
        "/cli/companion/stop" => match k2_core::companion::stop_companion() {
            Ok(()) => CliResponse::ok_json(r#"{"ok":true}"#.to_string()),
            Err(e) => CliResponse::bad_request(e),
        },
        "/cli/companion/status" => {
            CliResponse::ok_json(k2_core::companion::companion_status().to_string())
        }

        // ── K2 Connect tunnel status (read-only) ────────────────────
        // start/stop are POST-allowlisted in the dispatcher (mutating);
        // status is a cheap GET reporting running? + the predicted
        // public URL (https://<subdomain>.k2.dev).
        "/cli/tunnel/status" => CliResponse::ok_json(
            serde_json::to_string(&k2_core::tunnel::tunnel_status())
                .unwrap_or_else(|_| r#"{"running":false}"#.to_string()),
        ),
        "/cli/companion/presets" => match k2_core::companion::cli_routes::list_presets() {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::bad_request(e),
        },
        "/cli/companion/projects" => match k2_core::companion::cli_routes::list_projects() {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::bad_request(e),
        },

        // ── Workspace connections ───────────────────────────────────
        "/cli/connections" => match need_project(params) {
            Ok(p) => {
                let action = params
                    .get("action")
                    .cloned()
                    .unwrap_or_else(|| "list".to_string());
                let target = opt_param(params, "target");
                let rel_type = opt_param(params, "type");
                match k2_core::connections::connections(
                    &p,
                    &action,
                    target.as_deref(),
                    rel_type.as_deref(),
                ) {
                    Ok(body) => CliResponse::ok_json(body),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Workspace states ────────────────────────────────────────
        "/cli/states/list" => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            match k2_core::db::schema::WorkspaceState::list(&conn) {
                Ok(rows) => CliResponse::ok_json(
                    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
                ),
                Err(e) => CliResponse::bad_request(e.to_string()),
            }
        }
        "/cli/states/set" => match need_project(params) {
            Ok(p) => {
                let state_id = str_param(params, "state_id");
                match k2_core::workspace::settings::update_project_setting(&p, "tier_id", &state_id)
                {
                    Ok(()) => {
                        k2_core::agent_hooks::emit(
                            k2_core::agent_hooks::HookEvent::SyncProjects,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "stateId": state_id})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Agent channel ops (status / done / reserve / release) ──
        "/cli/status" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent_channel::status(
                p,
                str_param(params, "agent"),
                str_param(params, "message"),
            )),
            Err(r) => r,
        },
        "/cli/done" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent_channel::done(
                p,
                str_param(params, "agent"),
                opt_param(params, "blocked"),
            )),
            Err(r) => r,
        },
        "/cli/reserve" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent_channel::reserve(
                p,
                str_param(params, "agent"),
                str_param(params, "paths"),
            )),
            Err(r) => r,
        },
        "/cli/release" => match need_project(params) {
            Ok(p) => respond(k2_core::workspace::agent_channel::release(
                p,
                str_param(params, "agent"),
                str_param(params, "paths"),
            )),
            Err(r) => r,
        },

        // ── Skill fan-out ───────────────────────────────────────────
        "/cli/skills/regenerate" => match need_project(params) {
            Ok(p) => respond(k2_core::skills::crud::regenerate_skills(p)),
            Err(r) => r,
        },

        // ── Activity feed ───────────────────────────────────────────
        "/cli/feed" => match need_project(params) {
            Ok(p) => {
                let limit = params
                    .get("limit")
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(20);
                let agent = opt_param(params, "agent");

                let db = k2_core::db::shared();
                let conn = db.lock();

                let project_id: String = match conn.query_row(
                    "SELECT id FROM projects WHERE path = ?1",
                    rusqlite::params![p],
                    |row| row.get(0),
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(CliResponse::bad_request(format!("Project not found: {}", e)))
                    }
                };

                let entries = match agent {
                    Some(agent_name) => k2_core::db::schema::ActivityFeedEntry::list_by_actor(
                        &conn, &project_id, &agent_name, limit,
                    ),
                    None => k2_core::db::schema::ActivityFeedEntry::list_by_project(
                        &conn, &project_id, limit, 0,
                    ),
                };

                match entries {
                    Ok(entries) => {
                        let items: Vec<serde_json::Value> = entries
                            .iter()
                            .map(|e| {
                                serde_json::json!({
                                    "id": e.id,
                                    "actor": e.actor,
                                    "type": e.event_type,
                                    "from": e.from_workspace,
                                    "to": e.to_workspace,
                                    "summary": e.summary,
                                    "at": e.created_at,
                                })
                            })
                            .collect();
                        CliResponse::ok_json(serde_json::json!({ "feed": items }).to_string())
                    }
                    Err(e) => CliResponse::bad_request(e.to_string()),
                }
            }
            Err(r) => r,
        },

        // ── AI-assisted commit (emit-only) ──────────────────────────
        // /cli/commit and /cli/commit-merge both emit HookEvent::CliAiCommit
        // — Tauri-side sink spawns the commit terminal. Daemon has no PTY
        // of its own to spawn, so emission is the whole job.
        "/cli/commit" | "/cli/commit-merge" => match need_project(params) {
            Ok(p) => {
                let include_merge = path == "/cli/commit-merge";
                let message = str_param(params, "message");
                let git_context = k2_core::git::gather_git_context(&p);
                let event_payload = serde_json::json!({
                    "projectPath": p,
                    "includeMerge": include_merge,
                    "message": message,
                    "gitContext": git_context,
                });
                k2_core::agent_hooks::emit(
                    k2_core::agent_hooks::HookEvent::CliAiCommit,
                    event_payload,
                );
                CliResponse::ok_json(
                    serde_json::json!({
                        "success": true,
                        "action": if include_merge { "commit-merge" } else { "commit" },
                        "note": "AI commit terminal session will be launched by K2SO"
                    })
                    .to_string(),
                )
            }
            Err(r) => r,
        },

        // ── Per-project heartbeat schedule (distinct from per-agent) ─
        "/cli/heartbeat/schedule" => match need_project(params) {
            Ok(p) => {
                let db = k2_core::db::shared();
                let conn = db.lock();

                if let Some(mode) = opt_param(params, "mode") {
                    let schedule = opt_param(params, "schedule");
                    let hb_enabled = if mode == "off" { "0" } else { "1" };

                    let res = conn
                        .execute(
                            "UPDATE projects SET heartbeat_mode = ?1, heartbeat_schedule = ?2, heartbeat_enabled = ?3 WHERE path = ?4",
                            rusqlite::params![mode, schedule, hb_enabled, p],
                        )
                        .map(|_| ())
                        .map_err(|e| format!("DB update failed: {}", e));
                    drop(conn);

                    match res {
                        Ok(()) => {
                            // Nudge the Tauri side to refresh its
                            // launchd/cron installer via SyncProjects.
                            k2_core::agent_hooks::emit(
                                k2_core::agent_hooks::HookEvent::SyncProjects,
                                serde_json::Value::Null,
                            );
                            CliResponse::ok_json(
                                serde_json::json!({
                                    "success": true,
                                    "mode": mode,
                                    "schedule": schedule,
                                })
                                .to_string(),
                            )
                        }
                        Err(e) => CliResponse::bad_request(e),
                    }
                } else {
                    let res = conn.query_row(
                        "SELECT heartbeat_mode, heartbeat_schedule, heartbeat_last_fire FROM projects WHERE path = ?1",
                        rusqlite::params![p],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, Option<String>>(2)?,
                            ))
                        },
                    );
                    drop(conn);
                    match res {
                        Ok((mode, schedule, last_fire)) => CliResponse::ok_json(
                            serde_json::json!({
                                "mode": mode,
                                "schedule": schedule,
                                "lastFire": last_fire,
                            })
                            .to_string(),
                        ),
                        Err(e) => CliResponse::bad_request(format!("Project not found: {}", e)),
                    }
                }
            }
            Err(r) => r,
        },

        // ── Hook diagnostic ─────────────────────────────────────────
        "/cli/hooks/status" => {
            let limit = params
                .get("limit")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(20)
                .min(50);
            let mut events: Vec<_> = k2_core::agent_hooks::get_recent_events();
            events.reverse();
            events.truncate(limit);
            CliResponse::ok_json(
                serde_json::json!({
                    "port": k2_core::hook_config::get_port(),
                    "notify_script": dirs::home_dir()
                        .map(|h| h.join(".k2so/hooks/notify.sh").to_string_lossy().to_string())
                        .unwrap_or_default(),
                    // H7.1: scan per-CLI config files for notify.sh
                    // injection so `k2so hooks status` reports the
                    // full pipeline state (claude/cursor/gemini). Core
                    // helper moved from src-tauri as part of H7.
                    "injections": k2_core::agent_hooks::check_hook_injections(),
                    "recent_events": events,
                    "recent_events_cap": 50,
                })
                .to_string(),
            )
        }

        // P5.6: DB-as-source-of-truth replacement for the legacy
        // ~/.k2so/heartbeat-projects.txt file. heartbeat.sh now calls
        // this once per cron tick and iterates the response, calling
        // /cli/scheduler-tick per project. Newline-delimited plain
        // text so bash can `while read` without a JSON parser.
        // Returns every project path with at least one enabled,
        // non-archived agent_heartbeats row — derived state, never
        // stale.
        "/cli/heartbeat/active-projects" => {
            CliResponse::ok_text(crate::triage::handle_active_projects())
        }

        // ── Heartbeat CRUD + fires ──────────────────────────────────
        p if p.starts_with("/cli/heartbeat/") || p == "/cli/heartbeat-log" => {
            match need_project(params) {
                Ok(pp) => {
                    let result = if p == "/cli/heartbeat-log" {
                        crate::heartbeat_routes::dispatch_log(&pp, params)
                    } else {
                        crate::heartbeat_routes::dispatch_get(p, &pp, params)
                    };
                    match result {
                        Ok(body) => CliResponse::ok_json(body),
                        Err(msg) => CliResponse::bad_request(msg),
                    }
                }
                Err(r) => r,
            }
        }

        // ── Phase 4 H1: daemon-side terminal IO ─────────────────────
        // Session-stream-aware read + write against daemon-owned
        // sessions. `id` is a SessionId UUID. See
        // `terminal_routes` for behavior details.
        "/cli/terminal/read" => crate::terminal_routes::handle_read(params),
        "/cli/terminal/write" => crate::terminal_routes::handle_write(params),

        // ── Phase 2 Unit 3: terminal lifecycle GETs ─────────────────
        // Read-only inspection routes for the TerminalManager
        // singleton. Mutating siblings (create/kill/resize/...) are
        // POST routes in `main.rs` with method-gated handlers.
        "/cli/terminal/active-count" => {
            crate::terminal_lifecycle_routes::handle_active_count(params)
        }
        "/cli/terminal/foreground-cmd" => {
            crate::terminal_lifecycle_routes::handle_foreground_cmd(params)
        }
        "/cli/terminal/exists" => {
            crate::terminal_lifecycle_routes::handle_exists(params)
        }
        "/cli/terminal/get-grid" => {
            crate::terminal_lifecycle_routes::handle_get_grid(params)
        }
        "/cli/terminal/list-running" => {
            crate::terminal_lifecycle_routes::handle_list_running(params)
        }

        // ── Phase 4.5 I7: resize a live session ─────────────────────
        // Resizes both the PTY and the alacritty Term so the child
        // re-flows for the new dimensions. Called by Kessel's
        // ResizeObserver on DOM pane resize.
        "/cli/sessions/resize" => crate::terminal_routes::handle_sessions_resize(params),

        // 0.37.4 (Phase B): set a session's authoritative label.
        // Optional `lock` query param (default true) flips the
        // label_source to `Locked` so future PTY title events
        // can't override. Broadcasts `LabelChanged` to every WS
        // subscriber of this session — both windows of the same
        // workspace, the mobile companion, etc.
        //
        // Params: `id=<session-uuid>&label=<text>[&lock=true|false]`
        "/cli/sessions/label" => crate::terminal_routes::handle_sessions_label(params),

        // ── Phase 4 H3: daemon-side terminal spawn ──────────────────
        // Thin wrappers over `spawn::spawn_agent_session` (the same
        // helper /cli/sessions/spawn uses). Emits HookEvents so
        // attached UIs can react, matching the legacy Tauri
        // endpoint shape.
        "/cli/terminal/spawn" => match need_project(params) {
            Ok(p) => crate::terminal_routes::handle_terminal_spawn(params, &p),
            Err(r) => r,
        },
        "/cli/terminal/spawn-background" => match need_project(params) {
            Ok(p) => crate::terminal_routes::handle_terminal_spawn_background(params, &p),
            Err(r) => r,
        },

        // ── Phase 4 H4: companion cross-workspace enumeration ──────
        // Global session list + per-project summary. No project
        // param — these are intentionally cross-workspace (the
        // companion UI shows every workspace at once).
        "/cli/companion/sessions" => crate::companion_routes::handle_companion_sessions(params),
        "/cli/companion/projects-summary" => {
            crate::companion_routes::handle_companion_projects_summary(params)
        }

        // Look up a live session by agent_name across both legacy
        // session_map and v2_session_map. Used by the workspace
        // chat tab on mount to detect "is this agent already
        // running headless?" and pass attachAgentName to TerminalPane
        // so /cli/sessions/v2/spawn returns reused=true instead of
        // spawning a duplicate. Mirrors the role of
        // /cli/heartbeat/active-session, but keyed by agent_name
        // (heartbeats key by their own name).
        "/cli/sessions/lookup-by-agent" => {
            let agent = str_param(params, "agent");
            if agent.is_empty() {
                CliResponse::bad_request("Missing agent parameter")
            } else {
                let body = match crate::session_lookup::lookup_any(&agent) {
                    Some(live) => serde_json::json!({
                        "agentName": agent,
                        "sessionId": live.session_id().to_string(),
                        "sessionAlive": true,
                        "isV2": live.is_v2(),
                    }),
                    None => serde_json::json!({
                        "agentName": agent,
                        "sessionId": null,
                        "sessionAlive": false,
                        "isV2": false,
                    }),
                };
                CliResponse::ok_json(body.to_string())
            }
        }

        // 0.37.11 A9 phase 4a — every live session whose cwd is
        // under `path`. The renderer's `tabsStore.loadLayoutForWorkspace`
        // hits this BEFORE running `launchDefaultAgent` so a second
        // window opening the same workspace adopts the daemon's
        // existing PTYs instead of spawning duplicates.
        //
        // Returns JSON array; one object per live session:
        //   { sessionId, agentName, command, args, cwd, isV2 }
        //
        // Filter rule: longest cwd-prefix match against `path`, mirroring
        // the companion routes' grouping. Workspaces with `path` that
        // doesn't match any session return an empty array.
        "/cli/sessions/list-for-workspace" => {
            let path = str_param(params, "path");
            if path.is_empty() {
                CliResponse::bad_request("Missing path parameter")
            } else {
                // Match rule: session.cwd is either EXACTLY `path` or a
                // subdirectory of `path`. The previous loose `starts_with`
                // matched siblings — e.g. `/x/K2SO` would match
                // `/x/K2SO-website`. Require the next character to be
                // either end-of-string or `/` so siblings can't sneak in.
                let trimmed = path.trim_end_matches('/').to_string();
                let prefix_with_slash = if trimmed.is_empty() {
                    "/".to_string()
                } else {
                    format!("{}/", trimmed)
                };
                let live = crate::session_lookup::snapshot_all();
                let mut out: Vec<serde_json::Value> = Vec::new();
                for (agent_name, session) in live {
                    let cwd = session.cwd();
                    let cwd_trim = cwd.trim_end_matches('/');
                    let matches = cwd_trim == trimmed.as_str()
                        || cwd.starts_with(&prefix_with_slash);
                    if !matches {
                        continue;
                    }
                    out.push(serde_json::json!({
                        "sessionId": session.session_id().to_string(),
                        "agentName": agent_name,
                        "command": session.command(),
                        "args": session.args(),
                        "cwd": cwd,
                        "isV2": session.is_v2(),
                    }));
                }
                CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "[]".into()))
            }
        }

        // ── Onboarding (workspace-add three-option flow) ────────
        //
        // Logic lives in `k2_core::workspace::onboarding`; the
        // daemon owns the onboarding routes (Phase 2.5c moved the
        // command surface out of the legacy `k2_core::agents::*`
        // umbrella). Daemon exposes the four ops over HTTP so the
        // `k2so onboarding` CLI subcommand and any other headless
        // caller can drive the same flow as the Tauri
        // `WorkspaceOnboardingModal`.
        // Adopt + Start Fresh fire the workspace-regen bridge —
        // a no-op when the host hasn't registered a regen impl
        // (next Tauri launch picks up the staged PROJECT.md).
        "/cli/onboarding/scan" => match need_project(params) {
            Ok(p) => respond(Ok::<_, String>(
                k2_core::workspace::onboarding::scan_harness_files(&p),
            )),
            Err(r) => r,
        },
        "/cli/onboarding/adopt" => match need_project(params) {
            Ok(p) => {
                let source = str_param(params, "source");
                if source.is_empty() {
                    CliResponse::bad_request("Missing source parameter")
                } else {
                    match k2_core::workspace::onboarding::adopt_harness_as_project_md(
                        &p,
                        std::path::Path::new(&source),
                    ) {
                        Ok(outcome) => {
                            // Unit 7c: regen directly (workspace_regen
                            // bridge retired — body lives in k2so-core).
                            k2_core::workspace::skill_regen::write_workspace_skill_file(&p);
                            respond(Ok::<_, String>(outcome))
                        }
                        Err(e) => CliResponse::bad_request(e),
                    }
                }
            }
            Err(r) => r,
        },
        "/cli/onboarding/skip" => match need_project(params) {
            Ok(p) => respond_unit(k2_core::workspace::onboarding::skip_harness_management(&p)),
            Err(r) => r,
        },
        "/cli/onboarding/start-fresh" => match need_project(params) {
            Ok(p) => {
                if let Err(e) = k2_core::workspace::onboarding::unskip_harness_management(&p) {
                    return Some(CliResponse::bad_request(e));
                }
                // Unit 7c: regen directly (bridge retired — body in core).
                k2_core::workspace::skill_regen::write_workspace_skill_file(&p);
                CliResponse::ok_json(r#"{"success":true}"#.to_string())
            }
            Err(r) => r,
        },

        // Note: `/cli/heartbeat/active-session` lives in
        // `heartbeat_routes::dispatch_get` (alongside the rest of the
        // heartbeat CRUD), reached via the `/cli/heartbeat/*` arm above.

        // ── Phase 2 Unit 6: filesystem (GET) ──────────────────────
        //
        // POST routes (mutations) live in main.rs's dispatcher;
        // these GETs use the query-string interface common to the
        // rest of /cli/*.
        "/cli/fs/info" => crate::fs_routes::handle_info(params),
        "/cli/fs/read-dir" => crate::fs_routes::handle_read_dir(params),
        "/cli/fs/read-file" => crate::fs_routes::handle_read_file(params),
        "/cli/fs/read-binary" => crate::fs_routes::handle_read_binary(params),
        "/cli/fs/clipboard-paths" => crate::fs_routes::handle_clipboard_paths(params),

        // ── Phase 2 Unit 6: chat history (GET) ────────────────────
        "/cli/chat/list" => crate::chat_routes::handle_list(params),
        "/cli/chat/storage-paths" => crate::chat_routes::handle_storage_paths(params),
        "/cli/chat/custom-names" => crate::chat_routes::handle_custom_names(params),
        "/cli/chat/pinned" => crate::chat_routes::handle_pinned(params),
        "/cli/chat/detect-active" => crate::chat_routes::handle_detect_active(params),
        "/cli/chat/discover-ide" => crate::chat_routes::handle_discover_ide(params),
        "/cli/chat/session-exists" => crate::chat_routes::handle_session_exists(params),

        // ── Phase 2 Unit 6: themes (GET) ──────────────────────────
        "/cli/themes/list" => crate::themes_routes::handle_list(params),
        "/cli/themes/get-dir" => crate::themes_routes::handle_get_dir(params),
        "/cli/themes/ensure-dir" => crate::themes_routes::handle_ensure_dir(params),

        // ── Phase 2 Unit 6: skill layers (GET) ────────────────────
        "/cli/skill-layers/list" => crate::skill_layers_routes::handle_list(params),
        "/cli/skill-layers/get-content" => {
            crate::skill_layers_routes::handle_get_content(params)
        }

        // ── Phase 2 Unit 6: review checklist (GET) ────────────────
        "/cli/review-checklist/read" => {
            crate::review_checklist_routes::handle_read(params)
        }

        // ── Phase 2 Unit 6: project config (GET) ──────────────────
        "/cli/project-config/get" => crate::project_config_routes::handle_get(params),
        "/cli/project-config/has-run-command" => {
            crate::project_config_routes::handle_has_run_command(params)
        }
        "/cli/project-config/run-command" => {
            crate::project_config_routes::handle_run_command(params)
        }

        // ── 0.38.7: What's New popup ──────────────────────────────
        //
        // GET /cli/whats_new                — returns WhatsNewCheck JSON
        //                                     (current, last_seen, has_new, content)
        // POST /cli/whats_new/mark_seen     — writes current version to state file
        // POST /cli/whats_new/reset         — clears state file (forces re-show)
        //
        // Daemon-side `env!("CARGO_PKG_VERSION")` is the truth source for
        // the current version; the bundled `WHATS_NEW.md` is embedded
        // into the binary at build time.
        "/cli/whats_new" => {
            let check = k2_core::whats_new::check_for_user(env!("CARGO_PKG_VERSION"));
            let body = serde_json::to_string(&check)
                .unwrap_or_else(|_| "{\"has_new\":false}".to_string());
            CliResponse::ok_json(body)
        }
        "/cli/whats_new/mark_seen" => {
            match k2_core::whats_new::write_last_seen(env!("CARGO_PKG_VERSION")) {
                Ok(()) => CliResponse::ok_json(format!(
                    r#"{{"success":true,"marked":"{}"}}"#,
                    env!("CARGO_PKG_VERSION")
                )),
                Err(e) => CliResponse::bad_request(format!("failed to write state: {e}")),
            }
        }
        "/cli/whats_new/reset" => {
            match k2_core::whats_new::clear_last_seen() {
                Ok(()) => CliResponse::ok_json(r#"{"success":true,"cleared":true}"#.to_string()),
                Err(e) => CliResponse::bad_request(format!("failed to clear state: {e}")),
            }
        }

        // ── Phase 2 Unit 5: Claude Auth (GET status only) ───────────
        //
        // The three mutating routes — refresh-now, install-scheduler,
        // uninstall-scheduler — are wired as explicit POST branches
        // in `main.rs` (mirrors how Unit 1 wired the POST companion
        // routes). Only the read-only status check goes through the
        // generic GET dispatch.
        "/cli/claude-auth/status" => crate::claude_auth_host::handle_status(),

        // ── Phase 2 Unit 4: states / workspaces / focus-groups / sections /
        //                    layouts / timer / presets / window-state /
        //                    projects / git (GET endpoints) ─────────────
        // `/cli/states/{list,get,set}` already exist above — Unit 4 only
        // adds the POST mutations (`create`/`update`/`delete`).
        "/cli/workspaces/list" => crate::db_routes::handle_workspaces_list(params),
        "/cli/focus-groups/list" => crate::db_routes::handle_focus_groups_list(),
        "/cli/sections/list" => crate::db_routes::handle_sections_list(params),
        "/cli/workspace-layouts/load" => crate::db_routes::handle_layout_load(params),
        "/cli/workspace-layouts/load-all" => crate::db_routes::handle_layout_load_all(),
        // 0.39.39 #676 — daemon-canonical tab titles read (GET).
        "/cli/workspace/tab-titles" => crate::db_routes::handle_tab_titles_list(params),
        "/cli/timer/entries-list" => crate::db_routes::handle_timer_entries_list(params),
        "/cli/timer/entries-export" => crate::db_routes::handle_timer_entries_export(params),
        "/cli/presets/list" => crate::db_routes::handle_presets_list(),
        "/cli/window-state/get" => crate::db_routes::handle_window_state_get(),
        "/cli/projects/list" => crate::db_routes::handle_projects_list(),
        // task #672 — canonical Active-set snapshot (GET).
        "/cli/projects/active" => crate::db_routes::handle_projects_active(),
        "/cli/projects/get-icon" => crate::db_routes::handle_projects_get_icon(params),
        "/cli/projects/get-editors" => crate::db_routes::handle_projects_get_editors(),
        "/cli/projects/get-all-editors" => crate::db_routes::handle_projects_get_all_editors(),
        // Git GETs — libgit2 operations. Per F5, these can block the
        // accept loop on large repos. The dispatch is sync (matches
        // existing fs/* pattern). Acceptable today; if a slow handler
        // starves the accept loop in practice, lift to spawn_blocking
        // in main.rs via a `starts_with("/cli/git/")` GET arm.
        "/cli/git/info" => crate::git_routes::handle_git_info(params),
        "/cli/git/branches" => crate::git_routes::handle_git_branches(params),
        "/cli/git/worktrees" => crate::git_routes::handle_git_worktrees(params),
        "/cli/git/changes" => crate::git_routes::handle_git_changes(params),
        "/cli/git/diff-file" => crate::git_routes::handle_git_diff_file(params),
        "/cli/git/diff-summary" => crate::git_routes::handle_git_diff_summary(params),
        "/cli/git/diff-between" => crate::git_routes::handle_git_diff_between_branches(params),
        "/cli/git/file-at-ref" => crate::git_routes::handle_git_file_at_ref(params),
        "/cli/git/merge-status" => crate::git_routes::handle_git_merge_status(params),

        // ── Phase 2.1: Workspace inbox (read endpoints) ───────────
        // The default-list (`/cli/inbox`) and the explicit-list
        // (`/cli/inbox/list`) both route to the same handler — A22's
        // mock has `inbox` as the default verb that's equivalent to
        // `inbox list`. Empty `folder` param means top-level.
        "/cli/inbox" | "/cli/inbox/list" => crate::inbox_routes::handle_list(params),
        "/cli/inbox/read" => crate::inbox_routes::handle_read(params),
        "/cli/inbox/folders" => crate::inbox_routes::handle_folders(params),
        "/cli/inbox/search" => crate::inbox_routes::handle_search(params),

        // ── Phase 2.1: Glossary ──────────────────────────────────
        "/cli/glossary" | "/cli/glossary/list" => crate::inbox_routes::handle_glossary_list(),
        "/cli/glossary/get" => crate::inbox_routes::handle_glossary_get(params),

        _ => return None,
    };
    Some(resp)
}

/// 0.39.39 (#675.3 + #675.4) — push the canonical review-queue +
/// review-detail change onto the `/cli/sessions/events` spine so the
/// renderer can drop the `/cli/agents/review-queue` poll (review-queue.ts)
/// AND the reviews+chats poll (ReviewPanel.tsx). ONE call emits BOTH a
/// `ReviewQueueChanged` (queue membership may have shifted) and a
/// `ReviewChanged` (this specific review's detail/checklist changed).
/// Best-effort: the broadcast `let _ =`-swallows the no-subscribers case.
pub(crate) fn emit_review_changed(workspace_path: &str, agent: Option<&str>) {
    use crate::session_events::{self, SessionEvent};
    let _ = session_events::emit(SessionEvent::ReviewQueueChanged {
        workspace_path: workspace_path.to_string(),
    });
    let _ = session_events::emit(SessionEvent::ReviewChanged {
        workspace_path: workspace_path.to_string(),
        agent: agent.map(|a| a.to_string()),
    });
}
