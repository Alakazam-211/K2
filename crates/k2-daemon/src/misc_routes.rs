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
                            // B2 (0.40.24): the CLI validates the canonical
                            // spelling `k2`; the stored/legacy spelling is
                            // `k2so` (see settings::stored_agent_mode_value).
                            // Accept BOTH here — this check runs on the RAW
                            // requested value, before the write-side
                            // normalization maps k2 → k2so.
                            let bot_mode = matches!(mode.as_str(),
                                "custom" | "manager" | "k2so" | "k2");
                            let agent_md =
                                k2_core::workspace_dot_dir(&p).join("agent/AGENT.md");
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
        // #67 — per-workspace remote-instruct opt-in. GET with `?enable=`
        // mirrors the `/cli/worktree` pattern (path-scoped write via
        // `update_project_setting`). Default OFF / fail-closed; the daemon
        // still ENFORCES the gate server-side in `authorize_send_message`
        // (this route only records the per-workspace opt-in + drives the
        // renderer composer-hide). The owner is always allowed regardless.
        "/cli/remote-instruct" => match need_project(params) {
            Ok(p) => {
                let enable = bool_param(params, "enable");
                let value = if enable { "1" } else { "0" };
                match k2_core::workspace::settings::update_project_setting(&p, "allow_remote_instruct", value) {
                    Ok(()) => {
                        k2_core::agent_hooks::emit(
                            k2_core::agent_hooks::HookEvent::SyncProjects,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "allowRemoteInstruct": enable})
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
        // GET /cli/tunnel/subdomains — the daemon's cached Pro nested-
        // subdomain routing map (URLs drawer + K2 Connect settings): the
        // primary label plus every nested label's `{target, projectId}` —
        // the internal host:port the E2E TLS listener routes by overlaid
        // with the 0074 workspace attribution. Read-only serialization of
        // `session_events::tunnel_subdomains_snapshot()` (the SAME builder
        // the `tunnel_subdomains_changed` push twin uses — one truth, two
        // transports). Empty (`{"primary":"","targets":{}}`) until the
        // refresh loop has learned the account's map — e.g. tunnel down
        // or E2E off.
        "/cli/tunnel/subdomains" => {
            let (primary, targets) = crate::session_events::tunnel_subdomains_snapshot();
            CliResponse::ok_json(
                serde_json::json!({
                    "primary": primary,
                    "targets": targets,
                })
                .to_string(),
            )
        }
        // 0074 — claim/unclaim/refresh are POST-only mutations (handled in
        // the dispatcher's POST arms); a GET landing here gets an explicit
        // 405, never a silent no-op (feedback_post_only_route_guards).
        "/cli/tunnel/subdomains/claim"
        | "/cli/tunnel/subdomains/unclaim"
        | "/cli/tunnel/subdomains/refresh" => CliResponse::method_not_allowed(),
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

                    // GH#22/#23/#24 defense-in-depth: pre-0.40.41 CLIs
                    // misparsed `--help`/subcommand words as a schedule
                    // frequency and POSTed the junk here verbatim — this
                    // route wrote it into `projects` with success:true.
                    // The CLI is fixed, but stale CLIs in the field still
                    // hit this route: validate before touching the row so
                    // they get a loud 400 instead of a poisoned schedule.
                    if let Err(msg) =
                        validate_heartbeat_schedule_write(&mode, schedule.as_deref())
                    {
                        drop(conn);
                        CliResponse::bad_request(msg)
                    } else {
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
                        .map(|h| h.join(".k2/hooks/notify.sh").to_string_lossy().to_string())
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

        // Reliability overhaul — tick-transport health for the UI.
        // `lastTickAt` is stamped by every scheduler tick
        // (scheduler_meta KV); a stale value while heartbeats are
        // enabled means the transport (launchd agent / crontab /
        // daemon) is not delivering ticks — the Settings page renders
        // a "heartbeat transport down since <t>" banner from this.
        // Daemon-wide status, so no project param (must precede the
        // project-scoped catch-all arm below).
        "/cli/heartbeat/scheduler-status" => {
            let (last_tick_at, enabled_count) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let last = k2_core::db::schema::SchedulerMeta::get(
                    &conn,
                    k2_core::db::schema::SchedulerMeta::LAST_TICK_AT,
                );
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM workspace_heartbeats \
                         WHERE enabled = 1 AND archived_at IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                (last, count)
            };
            let stale_secs = last_tick_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds());
            CliResponse::ok_json(
                serde_json::json!({
                    "lastTickAt": last_tick_at,
                    "staleSecs": stale_secs,
                    "enabledCount": enabled_count,
                    "transportInstalled":
                        k2_core::heartbeats::install::transport_installed(),
                    "wakeMode": k2_core::app_settings::load().wake_scheduler.mode,
                })
                .to_string(),
            )
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

        // ── GH #8: daemon-side HITL detector + extractor ────────────
        // Read-only GET. Resolves the session's rendered screen (same
        // text as /cli/terminal/read), runs a regex fast-path then the
        // bundled qwen classifier, and returns
        // `{is_hitl, source, kind, questions}`. The load-bearing
        // detection primitive `talk` auto-routing (Phase 2) will
        // consume. Not in `post_allowed` — no 405 guard needed.
        "/cli/terminal/classify" => crate::classify_routes::handle_classify(params),

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
        // 0.40.22 — poll a server-side compress job (start = POST
        // /cli/fs/compress in the dispatcher).
        "/cli/fs/compress-status" => crate::fs_routes::handle_compress_status(params),
        // 0.40.22 — ranged file read; the download-to-local stream loops it.
        "/cli/fs/read-range" => crate::fs_routes::handle_read_range(params),
        // 0.40.22 — poll a "Clone to this computer" pull-pack job (start =
        // POST /cli/clone/pack in the dispatcher).
        "/cli/clone/pack-status" => crate::clone_routes::handle_clone_pack_status(params),

        // ── Phase 2 Unit 6: chat history (GET) ────────────────────
        "/cli/chat/list" => crate::chat_routes::handle_list(params),
        "/cli/sandbox/list" => crate::sandbox_chat_routes::handle_sandbox_list(params),
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
        // W6 (0.40.30) — one preset with its migration-0070 metadata.
        "/cli/presets/get" => crate::db_routes::handle_presets_get(params),
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

/// B3a (sandbox) — set/clear the PER-WORKSPACE Anthropic API key (BYO key).
///
/// Body (JSON): `{"project": "<workspace path>", "key": "<api key>"}`. An
/// empty/whitespace `key` CLEARS the stored key. The path is resolved to a
/// `projects.id` server-side; an unregistered path fails LOUDLY.
///
/// The key is staged into a microVM-backed sandbox cell's guest env at spawn
/// (per-workspace scoping → right key → right cell). This handler NEVER logs
/// or echoes the key — the success body returns only `{ "success": true,
/// "keySet": <bool> }`. OWNER-gated + POST-gated at the dispatcher arm.
pub fn handle_set_workspace_api_key(body: &[u8]) -> crate::cli_response::CliResponse {
    use crate::cli_response::CliResponse;
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    // Accept either `project` or `project_path` for the workspace path.
    let project_path = v
        .get("project")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("project_path").and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string();
    if project_path.is_empty() {
        return CliResponse::bad_request("missing 'project' (workspace path)");
    }
    // `key` may be absent/empty → that CLEARS the stored key.
    let key = v.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string();

    // Resolve path → projects.id (server-side; the body never supplies an id).
    let db = k2_core::db::shared();
    let project_id = {
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &project_path)
    };
    let Some(project_id) = project_id else {
        return CliResponse::bad_request(format!(
            "workspace not registered: {project_path}"
        ));
    };

    match k2_core::workspace::settings::set_workspace_api_key(&project_id, &key) {
        // NEVER echo the key — only whether one is now set.
        Ok(()) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "keySet": !key.trim().is_empty() })
                .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ─────────────────────────────────────────────────────────────────────
// P3a (sandbox / K2-as-a-server) — API-key auth tier management + /v1/ping.
// ─────────────────────────────────────────────────────────────────────

/// Shared truthy-env parse for the `/v1` gate flags: `1`/`true`/`yes`/`on`,
/// case-insensitive; anything else — including unset — is OFF. (Mirrors
/// `k2_core::federation::enabled`'s env-flag shape.)
fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// F3 gate split (prd-v1-api-completion §5): true iff the SANDBOX route
/// families of the `/v1/*` surface exist — `POST /v1/sandboxes`, the
/// `/v1/sandboxes/<id>/…` reads, and the workspace-scoped sandbox-session
/// routes under `/v1/w/<ws>/sessions…` — gated on the `K2_SANDBOX_API` env
/// flag. **Default OFF.** With it off those families 404 exactly like any
/// unknown `/v1` path (surface-absent; never a 405/409 oracle). Pre-split this
/// flag meant "the whole /v1 surface exists"; that role moved to `K2_API`
/// (see [`api_enabled`]), which this flag still implies for back-compat.
///
/// (The owner-gated `/cli/api-keys/*` MANAGEMENT routes are intentionally NOT
/// gated on this — the owner can pre-mint keys before flipping the external
/// surface live; minting is harmless while `/v1/*` is dark.)
pub(crate) fn sandbox_api_enabled() -> bool {
    env_truthy("K2_SANDBOX_API")
}

/// F3 gate split (prd-v1-api-completion §5): true iff the external `/v1/*`
/// surface EXISTS at all — auth tier, `/v1/ping`, `POST /v1/w/<ws>/message`.
/// Gated on the `K2_API` env flag OR (0.40.43 item 1c) the owner's persisted
/// `apiEnabled` app-setting, **default OFF**; with both off EVERY `/v1/*`
/// path 404s as if it didn't exist (the dispatcher consults this before
/// routing any `/v1/*` request), so the whole external surface is dark and
/// off is byte-identical to no surface.
///
/// 0.40.43 (1c): the settings leg reads the
/// [`k2_core::app_settings::api_enabled_setting`] runtime mirror — synced at
/// boot and after every `/cli/settings/{update,reset}` — and this function is
/// evaluated PER REQUEST, so flipping the Settings toggle takes effect with
/// NO daemon restart (the settings path deliberately skips the confirm+reboot
/// dialog). The env flag stays a valid force-on override for headless boxes
/// (nsi's systemd drop-in keeps working untouched).
///
/// BACK-COMPAT: the legacy `K2_SANDBOX_API` (which pre-split meant "the whole
/// /v1 surface") still implies the surface is on — existing Dedicated units
/// set only that var and must keep working. When the surface is enabled ONLY
/// via the legacy var, log a one-time deprecation info line.
pub(crate) fn api_enabled() -> bool {
    let via_new = env_truthy("K2_API");
    let via_legacy = sandbox_api_enabled();
    let via_setting = k2_core::app_settings::api_enabled_setting();
    if via_legacy && !via_new {
        static DEPRECATION_LOGGED: std::sync::Once = std::sync::Once::new();
        DEPRECATION_LOGGED.call_once(|| {
            k2_core::log_debug!(
                "[v1-api] INFO: /v1 surface enabled via legacy K2_SANDBOX_API only. \
                 Set K2_API=1 for the surface; K2_SANDBOX_API now narrows to the \
                 sandbox route families (F3 gate split)."
            );
        });
    }
    via_new || via_legacy || via_setting
}

/// F3 capability object:
/// `{"enabled": <bool>, "hostSessions": <bool>, "sandboxes": "microvm"|"none"}`.
/// `enabled` = the `/v1` surface exists ([`api_enabled`]); `hostSessions` =
/// the F1 non-sandboxed host-session family is served (PRD §5 — it ships
/// with the surface itself, no extra gate, so it equals `enabled`; kept as
/// its own key so clients never have to infer it); `sandboxes` = whether
/// THIS daemon can deliver a real microVM cell
/// ([`crate::v2_spawn::can_sandbox`] — the same source of truth the spawn
/// route's 409 refusal consults, so they can never diverge). Surfaced on the
/// UNAUTHENTICATED `/boot-status` (additive + forward-compatible like
/// `scopedHooks`; PROTOCOL not bumped) and echoed by `/v1/ping`. Nothing here
/// is sensitive: all three facts are already observable by probing `/v1`
/// routes (surface-404 vs 401, spawn 409 vs 200).
pub(crate) fn api_capability() -> serde_json::Value {
    serde_json::json!({
        "enabled": api_enabled(),
        "hostSessions": api_enabled(),
        "sandboxes": if crate::v2_spawn::can_sandbox() { "microvm" } else { "none" },
    })
}

/// OWNER-TIER (owner token OR Owner-role session — F4): mint a new API key.
/// Body (JSON): `{"label": "<tag>", "anthropicKey": "<sk-ant-…>"?}`. Returns
/// `{"id": "<uuid>", "key": "k2sk_…"}` — **the RAW key is returned exactly
/// ONCE here and is never recoverable afterward** (only its SHA-256 digest is
/// stored). The owner must surface it to the caller now. NEVER logs the raw
/// key or the anthropic key.
///
/// W5 (0.40.30, migration 0071) — ADDITIVE provider metadata (existing
/// request shapes keep working byte-identically):
/// - `"llmKey"` / `"llm_key"` — provider-neutral alias for the credential
///   (`anthropicKey` still accepted and takes precedence when both appear).
/// - `"provider"` — the credential's LLM provider. Validated against
///   [`k2_core::api_keys::LlmProvider`] and stored CANONICAL (`anthropic`,
///   `openai`, `google`, `xai`; aliases `gemini`/`grok` normalize). An
///   UNKNOWN value is a 400 (never stored — a typo'd provider would
///   otherwise fail closed at staging and boot the agent unauthenticated).
///   Absent/blank → NULL = anthropic (today's behavior).
/// - `"baseUrl"` / `"base_url"` — optional endpoint override, staged as
///   `OPENAI_BASE_URL` for openai keys; stored verbatim otherwise-unused.
///
/// `actor` is the dispatcher-resolved NON-secret acting identity
/// (`"owner-token"` or `"user:<name>"`, `api_key_manager_identity`) — the F4
/// audit trail: every mint is logged with who did it.
pub fn handle_api_key_create(body: &[u8], actor: &str) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        // An empty body is allowed (no label, no anthropic key).
        Ok(v) => v,
        Err(_) if body.iter().all(|b| b.is_ascii_whitespace()) => serde_json::json!({}),
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let label = v.get("label").and_then(|x| x.as_str()).unwrap_or("");
    // Accept either `anthropicKey` (camel) or `anthropic_key` (snake) — plus
    // the W5 provider-neutral aliases `llmKey`/`llm_key` (the same stored
    // credential; the historical names win when both are present).
    let anthropic_key = v
        .get("anthropicKey")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("anthropic_key").and_then(|x| x.as_str()))
        .or_else(|| v.get("llmKey").and_then(|x| x.as_str()))
        .or_else(|| v.get("llm_key").and_then(|x| x.as_str()));

    // W5 — optional provider (validated + canonicalized; absent/blank → NULL
    // = anthropic). Reject-at-mint: a typo'd provider would silently fail
    // closed at STAGING time (nothing staged), so surface it here instead.
    let provider_raw = v.get("provider").and_then(|x| x.as_str()).unwrap_or("");
    let provider_canonical: Option<&'static str> = if provider_raw.trim().is_empty() {
        None
    } else {
        match k2_core::api_keys::LlmProvider::parse(provider_raw) {
            Some(p) => Some(p.canonical_name()),
            None => {
                return CliResponse::bad_request(format!(
                    "unknown provider {:?}; accepted: {}",
                    provider_raw.trim(),
                    k2_core::api_keys::LlmProvider::ACCEPTED,
                ))
            }
        }
    };
    // W5 — optional endpoint override (OPENAI_BASE_URL pass-through for
    // openai keys). Stored verbatim; blank → NULL.
    let base_url = v
        .get("baseUrl")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("base_url").and_then(|x| x.as_str()));

    // Sandbox v2 (PRD §G2 #4) — normalize the optional per-key WORKSPACE GRANT
    // from the body into the raw TEXT column stored by `create_api_key`.
    // Accepts `workspaces` (or `allowedWorkspaces`/`allowed_workspaces`) as
    // EITHER the string `"*"` (all) OR an array of slugs (`["ai","docs"]`).
    // ABSENT / unrecognized → `None`, which stores NULL → the minted key is
    // FAIL-CLOSED (reaches no workspace) until it is re-minted with a grant.
    let grant_val = v
        .get("workspaces")
        .or_else(|| v.get("allowedWorkspaces"))
        .or_else(|| v.get("allowed_workspaces"));
    let workspaces_grant: Option<String> = match grant_val {
        Some(serde_json::Value::String(s)) if s.trim() == "*" => Some("*".to_string()),
        Some(serde_json::Value::Array(a)) => {
            let slugs: Vec<String> = a
                .iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            // An empty/all-blank array stays None (fail-closed), never `[]`.
            if slugs.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&slugs).unwrap_or_default())
            }
        }
        _ => None,
    };

    match k2_core::api_keys::create_api_key(
        label,
        anthropic_key,
        workspaces_grant.as_deref(),
        provider_canonical,
        base_url,
    ) {
        // The ONLY place the raw key is ever returned. Not logged — the audit
        // line carries the id + label + ACTOR only, never a secret.
        Ok((id, raw)) => {
            k2_core::log_debug!("[api-keys] created key {id} (label {label:?}) by {actor}");
            CliResponse::ok_json(serde_json::json!({ "id": id, "key": raw }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// OWNER-TIER (owner token OR Owner-role session — F4): revoke an API key by
/// id. Body (JSON): `{"id": "<uuid>"}`. Returns `{"success": <bool>}` —
/// `true` if a live key was just revoked, `false` for an unknown/
/// already-revoked id (idempotent). Revocation is immediate: the key fails
/// the `/v1/*` gate on its next use.
///
/// `actor` is the dispatcher-resolved NON-secret acting identity (F4 audit
/// trail); an actual revocation is logged with who did it.
pub fn handle_api_key_revoke(body: &[u8], actor: &str) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    if id.is_empty() {
        return CliResponse::bad_request("missing 'id'");
    }
    match k2_core::api_keys::revoke_api_key(&id) {
        Ok(success) => {
            // Only a REAL revocation is audit-logged (idempotent no-ops on an
            // unknown/already-revoked id aren't state changes).
            if success {
                k2_core::log_debug!("[api-keys] revoked key {id} by {actor}");
            }
            CliResponse::ok_json(serde_json::json!({ "success": success }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// OWNER-TIER (owner token OR Owner-role session — F4): list API keys as
/// redacted metadata. NEVER returns the raw key (unrecoverable) or the
/// anthropic key (only `anthropicKeySet`).
pub fn handle_api_key_list() -> CliResponse {
    match k2_core::api_keys::list_api_keys() {
        Ok(keys) => {
            let arr: Vec<serde_json::Value> = keys
                .into_iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "label": m.label,
                        "scope": m.scope,
                        "createdAt": m.created_at,
                        "revokedAt": m.revoked_at,
                        "keySet": m.key_set,
                        "anthropicKeySet": m.anthropic_key_set,
                        // Non-secret (slugs) — surface the raw workspace grant
                        // so the owner can audit which workspaces the key can
                        // address (null = fail-closed, no grant).
                        "allowedWorkspaces": m.allowed_workspaces,
                        // W5 (0071) — ADDITIVE non-secret provider metadata
                        // (null provider = anthropic default). Never the
                        // credential itself.
                        "provider": m.provider,
                        "baseUrl": m.base_url,
                    })
                })
                .collect();
            CliResponse::ok_json(serde_json::json!({ "keys": arr }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `GET /v1/ping` — the minimal P3a test route proving the auth tier resolves a
/// principal. `principal_id` is the caller's NON-secret identity (`"owner"` or
/// the API key's id). Carries no secret. F3 adds the `api` capability object
/// (same shape `/boot-status` advertises) so an authenticated caller can
/// feature-detect the sandbox tier without probing for 404s/409s.
pub fn handle_v1_ping(principal_id: &str) -> CliResponse {
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "principal": principal_id,
            "api": api_capability(),
        })
        .to_string(),
    )
}

// ── 0074 — nested-subdomain workspace attribution (claim / unclaim) ────

/// Resolve the `project` param (workspace PATH from the CLI's `$PROJECT`
/// context, or a `projects.id` from the renderer) to the canonical
/// project id. Attribution rows store the ID so they survive a folder
/// move/rename.
fn resolve_project_id(conn: &rusqlite::Connection, project: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1 OR id = ?1",
        rusqlite::params![project],
        |r| r.get(0),
    )
    .map_err(|_| format!("no registered workspace matches {project:?}"))
}

/// POST /cli/tunnel/subdomains/claim — attribute a nested subdomain
/// label to the acting workspace (0074). `label` = the nested label
/// (e.g. `staging`); `project` = the workspace path (CLI) or project id
/// (renderer). Called by the `k2 publish subdomain create/point` stamp
/// seams AND the explicit `k2 publish subdomain claim` adopt verb — the
/// label need NOT be in the cached routing map yet (a freshly-created
/// label lands there on the next refresh tick; a claim of a
/// never-provisioned label is harmless, snapshot readers ignore it).
/// Emits `tunnel_subdomains_changed` so both UIs converge.
pub fn handle_subdomain_claim(params: &HashMap<String, String>) -> CliResponse {
    let label = str_param(params, "label");
    if label.trim().is_empty() {
        return CliResponse::bad_request("Missing label parameter".to_string());
    }
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let project_id = match resolve_project_id(&conn, &project) {
            Ok(id) => id,
            Err(e) => return CliResponse::bad_request(e),
        };
        if let Err(e) =
            k2_core::db::schema::SubdomainWorkspace::claim(&conn, &label, &project_id)
        {
            return CliResponse::bad_request(e.to_string());
        }
        project_id
    };
    // Attribution changed while the routing map didn't — store() won't
    // fire, so emit the broadcast from THIS seam.
    crate::session_events::emit_tunnel_subdomains_changed();
    // A claim often follows a control-plane CREATE the cached map hasn't
    // seen yet (the snapshot ignores attributed-but-unmapped labels) —
    // nudge an immediate map re-pull so the new URL shows up without
    // waiting for the periodic poll. Best-effort: attribution + its emit
    // already landed above.
    nudge_subdomain_map_refresh();
    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "label": label.trim().to_ascii_lowercase(),
            "projectId": project_id,
        })
        .to_string(),
    )
}

/// POST /cli/tunnel/subdomains/unclaim — drop a label's workspace
/// attribution (0074). Called by the `k2 publish subdomain rm` stamp
/// seam and the explicit `unclaim` verb. `removed:false` = the label
/// wasn't attributed (reported honestly, still a 200 — the desired end
/// state holds). Emits `tunnel_subdomains_changed` only when a row was
/// actually removed.
pub fn handle_subdomain_unclaim(params: &HashMap<String, String>) -> CliResponse {
    let label = str_param(params, "label");
    if label.trim().is_empty() {
        return CliResponse::bad_request("Missing label parameter".to_string());
    }
    let removed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, &label) {
            Ok(removed) => removed,
            Err(e) => return CliResponse::bad_request(e.to_string()),
        }
    };
    if removed {
        crate::session_events::emit_tunnel_subdomains_changed();
    }
    // The unclaim stamp follows a control-plane DELETE (`rm`) — nudge an
    // immediate map re-pull so the removed URL disappears without waiting
    // for the periodic poll. Best-effort; unconditional (even removed:false
    // — the control-plane row may be gone while no attribution existed).
    nudge_subdomain_map_refresh();
    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "label": label.trim().to_ascii_lowercase(),
            "removed": removed,
        })
        .to_string(),
    )
}

/// POST /cli/tunnel/subdomains/refresh — pull the subdomain map from the
/// control plane NOW instead of waiting for the connector's periodic poll.
/// Called (best-effort) by the CLI right after a successful control-plane
/// `create`/`point`/`rm`, and by the claim/unclaim nudges, so freshly
/// published URLs appear in the UIs in realtime. Runs the SAME fetch the
/// periodic loop uses ([`k2_core::tunnel::subdomains::refresh_once`]); the
/// fetched map lands via `store()`, whose change-detect broadcasts
/// `tunnel_subdomains_changed` automatically — nothing is emitted here.
/// `changed:false` = the map was already current (e.g. a poll just ran).
pub fn handle_subdomain_refresh() -> CliResponse {
    let cfg = match k2_core::tunnel::config::load() {
        Ok(c) => c,
        Err(e) => return CliResponse::bad_request(e),
    };
    // Mirror the connector loop's gate: primary label + bearer token come
    // from the tunnel config; without both there is nothing to fetch.
    let primary = cfg.subdomain.trim().to_string();
    let token = cfg.token.trim().to_string();
    if primary.is_empty() || token.is_empty() {
        return CliResponse::bad_request("tunnel not configured");
    }
    match k2_core::tunnel::subdomains::refresh_once(&primary, &token) {
        Ok((_, changed)) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "changed": changed }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Best-effort subdomain-map refresh after an attribution write. Never
/// fails the caller (the attribution + its emit already landed); a miss
/// just means the map converges on the next periodic poll. Runs on a
/// detached thread so a slow control plane can't stall the claim/unclaim
/// response (the broadcast fires from `store()` whenever the fetch lands).
fn nudge_subdomain_map_refresh() {
    // Test seam: count the nudge instead of touching the tunnel config or
    // the live network (no network in unit tests — push_routes precedent).
    #[cfg(test)]
    {
        test_refresh_nudges::bump();
    }
    #[cfg(not(test))]
    {
        std::thread::spawn(|| {
            let resp = handle_subdomain_refresh();
            if resp.status != "200 OK" {
                k2_core::log_debug!(
                    "[tunnel/subdomains] post-attribution refresh nudge skipped/failed \
                     (map converges on the next poll): {}",
                    resp.body
                );
            }
        });
    }
}

/// Test-only nudge counter for [`nudge_subdomain_map_refresh`].
#[cfg(test)]
pub(crate) mod test_refresh_nudges {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNT: AtomicUsize = AtomicUsize::new(0);

    pub(crate) fn bump() {
        COUNT.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn count() -> usize {
        COUNT.load(Ordering::SeqCst)
    }
}

/// GH#22/#23/#24 defense-in-depth: validate a `/cli/heartbeat/schedule`
/// write before it reaches `projects`. Pre-0.40.41 CLIs misparsed
/// `--help` (and bare subcommand words like "add"/"list"/"remove") on
/// heartbeat subcommands as a schedule frequency and POSTed the junk
/// here verbatim; the route wrote it with success:true. The CLI is
/// fixed separately, but stale CLIs in the field still hit this route —
/// reject anything outside the real vocabulary. `Err` carries the
/// CLI-facing message (rendered as a 400 by the caller).
fn validate_heartbeat_schedule_write(mode: &str, schedule: Option<&str>) -> Result<(), String> {
    const MODES: &str = r#""off" | "hourly" | "scheduled""#;
    const FREQUENCIES: &str = r#""daily" | "weekly" | "monthly" | "yearly""#;
    match mode {
        // "off" clears the schedule — nothing further to validate.
        "off" => Ok(()),
        "hourly" => {
            let raw = schedule.ok_or_else(|| {
                "Invalid schedule for mode \"hourly\": missing schedule param \
                 (JSON object with a numeric every_seconds)"
                    .to_string()
            })?;
            let parsed: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
                format!("Invalid schedule for mode \"hourly\": not valid JSON ({e})")
            })?;
            if parsed.get("every_seconds").is_some_and(|v| v.is_number()) {
                Ok(())
            } else {
                Err(format!(
                    "Invalid schedule for mode \"hourly\": {raw:?} — \
                     JSON must carry a numeric every_seconds"
                ))
            }
        }
        "scheduled" => {
            let raw = schedule.ok_or_else(|| {
                format!(
                    "Invalid schedule for mode \"scheduled\": missing schedule param \
                     (JSON object with a frequency of {FREQUENCIES})"
                )
            })?;
            let parsed: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
                format!("Invalid schedule for mode \"scheduled\": not valid JSON ({e})")
            })?;
            // Exactly the GH#22/#23/#24 junk vector: mode was always the
            // valid "scheduled", the frequency carried the garbage.
            match parsed.get("frequency").and_then(|v| v.as_str()) {
                Some(f) if matches!(f, "daily" | "weekly" | "monthly" | "yearly") => Ok(()),
                Some(f) => Err(format!(
                    "Invalid schedule frequency {f:?}: must be one of {FREQUENCIES}"
                )),
                None => Err(format!(
                    "Invalid schedule for mode \"scheduled\": {raw:?} — \
                     JSON must carry a string frequency (one of {FREQUENCIES})"
                )),
            }
        }
        other => Err(format!(
            "Invalid heartbeat mode {other:?}: must be one of {MODES}"
        )),
    }
}

#[cfg(test)]
mod heartbeat_schedule_validation_tests {
    use super::validate_heartbeat_schedule_write as validate;

    /// The real vocabulary passes: off (no schedule needed), hourly with
    /// a numeric every_seconds, scheduled with each of the four valid
    /// frequencies.
    #[test]
    fn accepts_the_real_vocabulary() {
        assert_eq!(validate("off", None), Ok(()));
        // "off" ignores whatever schedule tags along.
        assert_eq!(validate("off", Some("anything")), Ok(()));
        assert_eq!(
            validate("hourly", Some(r#"{"start":"00:00","end":"23:59","every_seconds":300}"#)),
            Ok(())
        );
        for freq in ["daily", "weekly", "monthly", "yearly"] {
            assert_eq!(
                validate("scheduled", Some(&format!(r#"{{"frequency":"{freq}","time":"09:00"}}"#))),
                Ok(()),
                "frequency {freq:?} must be accepted"
            );
        }
    }

    /// GH#22/#23/#24: junk modes from stale CLIs (misparsed flags and
    /// subcommand words) are rejected, and the message names both the
    /// offending value and the allowed set.
    #[test]
    fn rejects_junk_modes_from_stale_clis() {
        for junk in ["--help", "add", "list", "remove", ""] {
            let err = validate(junk, None).expect_err(&format!("mode {junk:?} must be rejected"));
            assert!(err.contains(&format!("{junk:?}")), "error must name the value: {err}");
            assert!(
                err.contains(r#""off" | "hourly" | "scheduled""#),
                "error must list the allowed modes: {err}"
            );
        }
    }

    /// GH#22/#23/#24: the exact junk vector — mode was the valid
    /// "scheduled" but the frequency carried garbage. Missing schedule,
    /// malformed JSON, missing frequency, and out-of-vocabulary
    /// frequency all reject.
    #[test]
    fn rejects_junk_scheduled_frequencies() {
        assert!(validate("scheduled", None).is_err(), "missing schedule must reject");
        assert!(
            validate("scheduled", Some("--help")).is_err(),
            "non-JSON schedule must reject"
        );
        assert!(
            validate("scheduled", Some(r#"{"time":"09:00"}"#)).is_err(),
            "missing frequency must reject"
        );
        assert!(
            validate("scheduled", Some(r#"{"frequency":42}"#)).is_err(),
            "non-string frequency must reject"
        );
        for junk in ["--help", "add", "list", "remove", "fortnightly"] {
            let err = validate("scheduled", Some(&format!(r#"{{"frequency":"{junk}"}}"#)))
                .expect_err(&format!("frequency {junk:?} must be rejected"));
            assert!(err.contains(&format!("{junk:?}")), "error must name the value: {err}");
            assert!(
                err.contains(r#""daily" | "weekly" | "monthly" | "yearly""#),
                "error must list the allowed frequencies: {err}"
            );
        }
    }

    /// hourly requires a JSON schedule with a numeric every_seconds.
    #[test]
    fn rejects_hourly_without_numeric_every_seconds() {
        assert!(validate("hourly", None).is_err(), "missing schedule must reject");
        assert!(validate("hourly", Some("not json")).is_err(), "non-JSON must reject");
        assert!(
            validate("hourly", Some(r#"{"start":"00:00"}"#)).is_err(),
            "missing every_seconds must reject"
        );
        assert!(
            validate("hourly", Some(r#"{"every_seconds":"300"}"#)).is_err(),
            "string every_seconds must reject"
        );
    }
}

#[cfg(test)]
mod api_key_route_tests {
    use super::*;

    /// Serializes the env-mutating gate tests in this module against each
    /// other across threads (`K2_API` / `K2_SANDBOX_API` are process-wide).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());


    fn restore_env(name: &str, prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }

    /// The `K2_SANDBOX_API` flag defaults OFF and only the canonical truthy
    /// values flip it on (serialized via the module env lock to avoid racing
    /// other env-mutating tests).
    #[test]
    fn sandbox_api_flag_defaults_off() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os("K2_SANDBOX_API");
        std::env::remove_var("K2_SANDBOX_API");
        assert!(!sandbox_api_enabled(), "K2_SANDBOX_API must default OFF");
        std::env::set_var("K2_SANDBOX_API", "1");
        assert!(sandbox_api_enabled(), "K2_SANDBOX_API=1 enables");
        std::env::set_var("K2_SANDBOX_API", "off");
        assert!(!sandbox_api_enabled(), "K2_SANDBOX_API=off disables");
        std::env::set_var("K2_SANDBOX_API", "true");
        assert!(sandbox_api_enabled(), "K2_SANDBOX_API=true enables");
        restore_env("K2_SANDBOX_API", prev);
    }

    /// F3 gate split: `api_enabled()` = `K2_API` truthy OR the legacy
    /// `K2_SANDBOX_API` (back-compat implies) OR (0.40.43 1c) the persisted
    /// `apiEnabled` setting's runtime mirror, default OFF; the capability
    /// object mirrors it and reports `sandboxes` from `can_sandbox()`.
    #[test]
    fn api_enabled_combos_and_capability_shape() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // ALSO hold the crate-wide home lock: the settings-mirror leg below
        // shares process state (`app_settings::API_SETTING_ENABLED`) with the
        // settings_routes tests, which serialize on this lock via
        // with_temp_home. Holding both prevents a parallel settings test's
        // transient mirror flip from racing the default-OFF assertions here.
        let _h = crate::test_support::lock_home();
        let prev_api = std::env::var_os("K2_API");
        let prev_sbx = std::env::var_os("K2_SANDBOX_API");
        // Defensive: a previously-failed test could leak the mirror ON.
        k2_core::app_settings::set_api_enabled(false);

        // Both env flags unset + setting off → surface dark.
        std::env::remove_var("K2_API");
        std::env::remove_var("K2_SANDBOX_API");
        assert!(!api_enabled(), "K2_API must default OFF");
        assert_eq!(api_capability()["enabled"], serde_json::json!(false));

        // 0.40.43 (1c): the persisted-setting mirror alone turns the surface
        // on — the no-restart Settings-toggle path — and back off. Env flags
        // stay unset throughout, proving the OR's third leg is independent.
        k2_core::app_settings::set_api_enabled(true);
        assert!(api_enabled(), "apiEnabled setting alone must enable the surface");
        assert!(
            !sandbox_api_enabled(),
            "…without implying the sandbox gate (setting maps to K2_API, not K2_SANDBOX_API)"
        );
        assert_eq!(api_capability()["enabled"], serde_json::json!(true));
        k2_core::app_settings::set_api_enabled(false);
        assert!(!api_enabled(), "apiEnabled setting OFF must go dark again");

        // K2_API alone turns the surface on.
        std::env::set_var("K2_API", "1");
        assert!(api_enabled(), "K2_API=1 enables the surface");
        assert!(!sandbox_api_enabled(), "…without implying the sandbox gate");

        // Legacy K2_SANDBOX_API alone still implies the surface (back-compat).
        std::env::remove_var("K2_API");
        std::env::set_var("K2_SANDBOX_API", "true");
        assert!(api_enabled(), "legacy K2_SANDBOX_API=true implies K2_API");

        // Falsy values never enable.
        std::env::set_var("K2_API", "0");
        std::env::set_var("K2_SANDBOX_API", "off");
        assert!(!api_enabled(), "falsy values must not enable the surface");

        // Capability object shape: enabled + hostSessions bools (hostSessions
        // ships with the surface — F1) + sandboxes tier string from
        // can_sandbox() (on a mac/feature-off test build that is "none").
        std::env::set_var("K2_API", "1");
        let cap = api_capability();
        assert_eq!(cap["enabled"], serde_json::json!(true));
        assert_eq!(
            cap["hostSessions"],
            serde_json::json!(true),
            "hostSessions ships with the surface gate (F1); got {cap}"
        );
        let expect_tier = if crate::v2_spawn::can_sandbox() { "microvm" } else { "none" };
        assert_eq!(cap["sandboxes"], serde_json::json!(expect_tier));
        assert_eq!(
            cap.as_object().map(|o| o.len()),
            Some(3),
            "capability object is FROZEN wire shape: exactly enabled+hostSessions+sandboxes; got {cap}"
        );

        restore_env("K2_API", prev_api);
        restore_env("K2_SANDBOX_API", prev_sbx);
    }

    /// create → list shows the key (redacted); revoke flips it; the create
    /// response carries the raw key but list NEVER does, nor the anthropic key.
    #[test]
    fn create_list_revoke_route_round_trip_redacts_secrets() {
        let secret = "sk-ant-route-secret-qqq";
        let body = serde_json::json!({ "label": "route-test", "anthropicKey": secret })
            .to_string();
        let resp = handle_api_key_create(body.as_bytes(), "owner-token");
        assert_eq!(resp.status, "200 OK");
        let created: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        let id = created["id"].as_str().expect("id").to_string();
        let raw = created["key"].as_str().expect("raw key").to_string();
        assert!(raw.starts_with("k2sk_"));

        // List: present, redacted, and NEITHER secret leaks.
        let list = handle_api_key_list();
        assert_eq!(list.status, "200 OK");
        assert!(list.body.contains(&id), "list includes the id");
        assert!(!list.body.contains(secret), "list must not leak the anthropic key");
        let raw_body = raw.strip_prefix("k2sk_").unwrap();
        assert!(!list.body.contains(raw_body), "list must not leak the raw key");
        let parsed: serde_json::Value = serde_json::from_str(&list.body).expect("json");
        let mine = parsed["keys"].as_array().unwrap().iter()
            .find(|k| k["id"] == serde_json::json!(id)).expect("our key");
        assert_eq!(mine["anthropicKeySet"], serde_json::json!(true));
        assert_eq!(mine["revokedAt"], serde_json::Value::Null);

        // Revoke → success true; second revoke → false (idempotent).
        let rev = handle_api_key_revoke(
            serde_json::json!({ "id": id }).to_string().as_bytes(),
            "owner-token",
        );
        assert_eq!(rev.status, "200 OK");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rev.body).unwrap()["success"],
            serde_json::json!(true),
        );
        let rev2 = handle_api_key_revoke(
            serde_json::json!({ "id": id }).to_string().as_bytes(),
            "owner-token",
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rev2.body).unwrap()["success"],
            serde_json::json!(false),
        );
    }

    #[test]
    fn revoke_missing_id_is_bad_request() {
        let resp = handle_api_key_revoke(b"{}", "owner-token");
        assert_eq!(resp.status, "400 Bad Request");
    }

    /// W5 — ADDITIVE provider fields on create/list: a provider'd create
    /// round-trips (canonicalized), the `llmKey` alias stores the credential,
    /// and a legacy provider-less create keeps working with null metadata.
    #[test]
    fn create_with_provider_round_trips_additively() {
        let secret = "sk-oai-route-secret-w5";
        // Aliased spellings: `llmKey` + alias provider `gemini` (→ google).
        let body = serde_json::json!({
            "label": "route-w5-google",
            "llmKey": secret,
            "provider": "Gemini",
        })
        .to_string();
        let resp = handle_api_key_create(body.as_bytes(), "owner-token");
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let id = serde_json::from_str::<serde_json::Value>(&resp.body).unwrap()["id"]
            .as_str()
            .expect("id")
            .to_string();

        // openai + baseUrl round trip.
        let body = serde_json::json!({
            "label": "route-w5-openai",
            "anthropicKey": secret,
            "provider": "openai",
            "baseUrl": "https://oai.example/v1",
        })
        .to_string();
        let resp = handle_api_key_create(body.as_bytes(), "owner-token");
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let id_oai = serde_json::from_str::<serde_json::Value>(&resp.body).unwrap()["id"]
            .as_str()
            .expect("id")
            .to_string();

        let list = handle_api_key_list();
        assert_eq!(list.status, "200 OK");
        assert!(!list.body.contains(secret), "list must never leak the credential");
        let parsed: serde_json::Value = serde_json::from_str(&list.body).expect("json");
        let keys = parsed["keys"].as_array().expect("keys");
        let google = keys.iter().find(|k| k["id"] == serde_json::json!(id)).expect("google key");
        assert_eq!(
            google["provider"],
            serde_json::json!("google"),
            "alias gemini canonicalizes to google; got {google}"
        );
        assert_eq!(google["baseUrl"], serde_json::Value::Null);
        assert_eq!(google["anthropicKeySet"], serde_json::json!(true), "llmKey alias stored");
        let oai = keys.iter().find(|k| k["id"] == serde_json::json!(id_oai)).expect("oai key");
        assert_eq!(oai["provider"], serde_json::json!("openai"));
        assert_eq!(oai["baseUrl"], serde_json::json!("https://oai.example/v1"));

        // FROZEN-SHAPE guard: the legacy provider-less body still mints, and
        // its listed metadata is null (= anthropic default at staging).
        let resp = handle_api_key_create(
            serde_json::json!({ "label": "route-w5-legacy" }).to_string().as_bytes(),
            "owner-token",
        );
        assert_eq!(resp.status, "200 OK", "legacy shape must keep working");
        let id_legacy = serde_json::from_str::<serde_json::Value>(&resp.body).unwrap()["id"]
            .as_str()
            .expect("id")
            .to_string();
        let list = handle_api_key_list();
        let parsed: serde_json::Value = serde_json::from_str(&list.body).expect("json");
        let legacy = parsed["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["id"] == serde_json::json!(id_legacy))
            .expect("legacy key")
            .clone();
        assert_eq!(legacy["provider"], serde_json::Value::Null);
        assert_eq!(legacy["baseUrl"], serde_json::Value::Null);
    }

    /// W5 — an UNKNOWN provider is rejected at mint (400, nothing stored):
    /// a typo would otherwise fail closed at staging and boot the agent
    /// credential-less with no visible error.
    #[test]
    fn create_with_unknown_provider_is_bad_request() {
        let body = serde_json::json!({
            "label": "route-w5-badprov",
            "provider": "azure-openai",
        })
        .to_string();
        let resp = handle_api_key_create(body.as_bytes(), "owner-token");
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        assert!(
            resp.body.contains("unknown provider") && resp.body.contains("anthropic"),
            "error names the accepted set; body={}",
            resp.body
        );
        // Nothing minted.
        let list = handle_api_key_list();
        assert!(
            !list.body.contains("route-w5-badprov"),
            "a rejected create must not mint a row"
        );
    }

    #[test]
    fn ping_echoes_principal_and_no_secret() {
        let resp = handle_v1_ping("owner");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true));
        assert_eq!(v["principal"], serde_json::json!("owner"));
        // F3: ping echoes the capability object (gate-state coverage lives in
        // api_enabled_combos_and_capability_shape + api_gate_integration.rs).
        assert!(v["api"]["enabled"].is_boolean(), "body={}", resp.body);
        assert!(
            matches!(v["api"]["sandboxes"].as_str(), Some("microvm") | Some("none")),
            "body={}",
            resp.body
        );
    }
}

#[cfg(test)]
mod subdomain_attribution_route_tests {
    //! 0074 — `POST /cli/tunnel/subdomains/{claim,unclaim}` handler
    //! coverage against the shared in-memory test DB. Labels/paths are
    //! process-unique sentinels so parallel tests sharing that DB can't
    //! collide; each test cleans up its rows.
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn make_project(suffix: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/attrib-{suffix}-{}", std::process::id());
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, "attrib-test", path],
        )
        .expect("insert project");
        (id, path)
    }

    fn attribution_of(label: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SubdomainWorkspace::map(&conn)
            .expect("map")
            .get(&label.trim().to_ascii_lowercase())
            .cloned()
    }

    fn cleanup(label: &str, project_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, label);
        let _ = conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
        );
    }

    /// Claim by workspace PATH (the CLI's `$PROJECT` context) resolves to
    /// the project ID, writes the row, and echoes both back; unclaim then
    /// removes it and reports `removed:true` / `false` honestly.
    #[test]
    fn claim_by_path_then_unclaim_roundtrip() {
        let (id, path) = make_project("roundtrip");
        let label = format!("rt-{}", std::process::id());

        let resp = handle_subdomain_claim(&params(&[("label", &label), ("project", &path)]));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["success"], serde_json::json!(true));
        assert_eq!(v["label"], serde_json::json!(label));
        assert_eq!(v["projectId"].as_str(), Some(id.as_str()));
        assert_eq!(attribution_of(&label).as_deref(), Some(id.as_str()));

        let resp = handle_subdomain_unclaim(&params(&[("label", &label)]));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["removed"], serde_json::json!(true));
        assert_eq!(attribution_of(&label), None);

        // Second unclaim: still 200, but honestly removed:false.
        let resp = handle_subdomain_unclaim(&params(&[("label", &label)]));
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["removed"], serde_json::json!(false));

        cleanup(&label, &id);
    }

    /// Claim also accepts the project ID directly (the renderer's
    /// context), and a re-claim by another workspace repoints the label.
    #[test]
    fn claim_by_id_and_reclaim_repoints() {
        let (id_a, _) = make_project("repoint-a");
        let (id_b, _) = make_project("repoint-b");
        let label = format!("rp-{}", std::process::id());

        let resp = handle_subdomain_claim(&params(&[("label", &label), ("project", &id_a)]));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        assert_eq!(attribution_of(&label).as_deref(), Some(id_a.as_str()));

        let resp = handle_subdomain_claim(&params(&[("label", &label), ("project", &id_b)]));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        assert_eq!(
            attribution_of(&label).as_deref(),
            Some(id_b.as_str()),
            "re-claim must repoint to the new workspace"
        );

        cleanup(&label, &id_a);
        cleanup(&label, &id_b);
    }

    /// Fail-loud inputs: a missing/blank label, a missing project, and an
    /// unregistered workspace are 400s — never a silent half-write.
    #[test]
    fn claim_rejects_bad_inputs() {
        let (id, path) = make_project("badinput");
        let label = format!("bad-{}", std::process::id());

        let resp = handle_subdomain_claim(&params(&[("project", &path)]));
        assert_eq!(resp.status, "400 Bad Request", "missing label must 400");
        let resp = handle_subdomain_claim(&params(&[("label", "  "), ("project", &path)]));
        assert_eq!(resp.status, "400 Bad Request", "blank label must 400");
        let resp = handle_subdomain_claim(&params(&[("label", &label)]));
        assert_eq!(resp.status, "400 Bad Request", "missing project must 400");
        let resp = handle_subdomain_claim(&params(&[
            ("label", &label),
            ("project", "/nowhere/not-registered"),
        ]));
        assert_eq!(resp.status, "400 Bad Request", "unknown workspace must 400");
        assert_eq!(attribution_of(&label), None, "no row on any rejected path");

        let resp = handle_subdomain_unclaim(&params(&[]));
        assert_eq!(resp.status, "400 Bad Request", "unclaim without label must 400");

        cleanup(&label, &id);
    }

    /// The GET chain 405s the POST-only claim/unclaim/refresh paths
    /// (feedback_post_only_route_guards) while the sibling read route
    /// stays a 200 GET.
    #[test]
    fn get_chain_405s_claim_unclaim_and_refresh() {
        let p = HashMap::new();
        for path in [
            "/cli/tunnel/subdomains/claim",
            "/cli/tunnel/subdomains/unclaim",
            "/cli/tunnel/subdomains/refresh",
        ] {
            let resp = dispatch(path, &p).expect("route must be known to the GET chain");
            assert_eq!(resp.status, "405 Method Not Allowed", "path={path}");
        }
        let resp = dispatch("/cli/tunnel/subdomains", &p).expect("read route");
        assert_eq!(resp.status, "200 OK");
    }

    /// Refresh with no tunnel configured (blank subdomain/token under a
    /// sandboxed `$HOME`) is an explicit 400 — never a silent no-op and
    /// never a control-plane call. Covers both the missing-file default
    /// config and a saved-but-blank config.
    #[test]
    fn refresh_without_tunnel_config_is_400() {
        crate::test_support::with_temp_home(|| {
            let resp = handle_subdomain_refresh();
            assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            assert_eq!(v["error"], serde_json::json!("tunnel not configured"));

            // A config with a subdomain but NO token is still unconfigured.
            let cfg = k2_core::tunnel::TunnelConfig {
                subdomain: "rosson".to_string(),
                ..Default::default()
            };
            k2_core::tunnel::config::save(&cfg).expect("save sandboxed config");
            let resp = handle_subdomain_refresh();
            assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
            assert_eq!(v["error"], serde_json::json!("tunnel not configured"));
        });
    }

    /// Successful claim and unclaim both fire the best-effort map-refresh
    /// nudge (the cfg(test) seam counts instead of touching the network);
    /// a rejected claim (bad input) fires nothing.
    #[test]
    fn claim_and_unclaim_nudge_a_map_refresh() {
        let (id, path) = make_project("nudge");
        let label = format!("ndg-{}", std::process::id());

        let before = test_refresh_nudges::count();
        let resp = handle_subdomain_claim(&params(&[("label", &label), ("project", &path)]));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let after_claim = test_refresh_nudges::count();
        assert!(after_claim > before, "claim must nudge a refresh");

        let resp = handle_subdomain_unclaim(&params(&[("label", &label)]));
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        assert!(
            test_refresh_nudges::count() > after_claim,
            "unclaim must nudge a refresh"
        );

        // (No exact-equality "rejected input doesn't nudge" assert here:
        // the counter is process-global and other tests bump it in
        // parallel. The nudge sits strictly AFTER every early 400 return
        // in the handlers — see `claim_rejects_bad_inputs` for the 400s.)
        cleanup(&label, &id);
    }
}
