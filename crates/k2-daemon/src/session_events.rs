//! 0.38.0 Commit 4 — daemon-owned session lifecycle broadcast.
//!
//! Push-driven counterpart to `/cli/sessions/list-for-workspace`.
//! Every `v2_session_map::register` / `unregister` call publishes a
//! `SessionEvent` onto a process-wide `tokio::sync::broadcast` channel.
//! `session_events_ws::serve_session_events_connection` fans those
//! events out to each `/cli/sessions/events?path=<workspace>` WS
//! subscriber, filtering by `cwd starts_with workspace_path` (same
//! rule the existing list-for-workspace endpoint uses in `cli.rs`).
//!
//! Why a broadcast bus instead of per-subscriber polling: the
//! renderer used to discover new daemon-owned PTYs only on workspace
//! switch (`loadLayoutForWorkspace` triggers `reconcileWithDaemon`).
//! That left every other connected window stale until the user
//! manually re-opened the workspace. Push-driven adoption + drop
//! means Cmd+T in window A surfaces the new tab in window B + the
//! mobile companion within one network hop.
//!
//! Initialization: a `OnceLock<Sender<SessionEvent>>` lazily creates
//! the channel on first `sender()` call. Test environments that
//! never call into `register`/`unregister` won't initialize it;
//! emit sites use `let _ = ` to ignore the case where the bus has
//! no live subscribers (broadcast::send returns an Err in that
//! case but we treat it as a no-op).

use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::broadcast;

/// Capacity for the broadcast channel. Small enough that a stalled
/// subscriber doesn't bloat memory, large enough that bursts (e.g.
/// opening 10 tabs back-to-back) don't drop events under realistic
/// network jitter. Lagged subscribers get `RecvError::Lagged` and
/// the WS handler in `session_events_ws.rs` treats that as a hint
/// to re-emit the current truth via a fresh snapshot.
pub const EVENT_CHANNEL_CAP: usize = 256;

/// One lifecycle event for a daemon-owned PTY session. Serialized
/// as JSON and pushed to every matching WS subscriber. Discriminated
/// by `kind` so callers can switch on the variant cheaply.
///
/// **Wire stability:** the renderer + mobile companion both consume
/// this shape. Field renames are breaking. Adding new variants is
/// safe — clients should ignore unknown `kind` values.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// A daemon-owned PTY session has been registered in the v2 map.
    /// Emitted from `v2_session_map::register`. `agent_name` is the
    /// canonical key under which the session was registered (e.g.
    /// `tab-<paneGroupId>` for Cmd+T tabs, bare project UUID for
    /// pinned chat, ad-hoc shapes for heartbeats).
    SessionAdded {
        /// Absolute cwd of the spawned PTY. Subscribers filter on
        /// `cwd starts_with subscriber's workspace_path` before
        /// forwarding to their socket.
        workspace_path: String,
        /// `tab-<paneGroupId>` extracted from `agent_name` when the
        /// prefix matches; absent otherwise (pinned chat, heartbeat).
        /// The renderer uses this as the canonical paneGroup id when
        /// adopting the session as a new tab.
        pane_group_id: Option<String>,
        /// Canonical `v2_session_map` key. The renderer filters on
        /// the `tab-` prefix to decide whether to surface the event
        /// as a tab; non-tab agent_names are forwarded so the mobile
        /// companion sees the full session inventory.
        agent_name: String,
        /// Spawn command. May be `null` when the daemon spawned the
        /// user's default login shell.
        command: Option<String>,
        /// Spawn args. Empty when none.
        args: Vec<String>,
        /// Daemon-side session UUID. Mirrors
        /// `list-for-workspace`'s `sessionId` field.
        session_id: String,
        /// Always `true` for v2 sessions today. Reserved for the
        /// post-cleanup phase that may add legacy events.
        #[serde(rename = "isV2")]
        is_v2: bool,
    },
    /// A session has been removed from the v2 map. Emitted from
    /// `v2_session_map::unregister` (which is the single chokepoint
    /// for "session goes away" — child exit observer + explicit
    /// close route + watchdog escalation all funnel through it).
    SessionRemoved {
        workspace_path: String,
        pane_group_id: Option<String>,
        agent_name: String,
    },
    /// Canonical Active-set changed (task #672 — daemon-owned Active).
    /// Carries the FULL set (not a diff) so client convergence is
    /// trivial + race-free (last-write-wins on a monotonic snapshot).
    /// Emitted after every recompute that can change membership:
    /// activate / pin / dismiss / window-tick / reaper close.
    ///
    /// **App-level, not workspace-scoped.** Unlike `SessionAdded`/
    /// `SessionRemoved` (filtered by `cwd starts_with path`), this
    /// event has no single workspace — `session_events_ws` forwards it
    /// to EVERY subscriber regardless of their `?path=` so each client
    /// mirrors the global union (see `event_matches_workspace`).
    ///
    /// **Wire shape (frozen — the renderer codes against these EXACT
    /// names):** `{ "kind": "active_changed", "activeProjectIds":
    /// string[], "activeWindowHours": number }`. The `#[serde(tag =
    /// "kind", rename_all = "snake_case")]` on the enum yields the
    /// `active_changed` tag; the per-field `rename` yields the camelCase
    /// field names.
    ActiveChanged {
        #[serde(rename = "activeProjectIds")]
        active_project_ids: Vec<String>,
        #[serde(rename = "activeWindowHours")]
        active_window_hours: u32,
    },
    /// Session label/title changed. Reserved variant — currently
    /// unused on the emit side because the daemon's existing label
    /// broadcast (`sessions_grid_ws::Outbound::LabelChanged`) is
    /// already wired into the per-session WS. Kept in the schema
    /// so a future caller (mobile companion: "what's the current
    /// title?") doesn't break wire compat.
    #[allow(dead_code)]
    SessionRenamed {
        workspace_path: String,
        pane_group_id: Option<String>,
        title: String,
    },

    // ─── 0.39.39 daemon-canonical broadcast spine (#675/#676/#677) ───
    //
    // These variants let the renderer DROP its polling intervals and
    // subscribe to the daemon's source-of-truth instead. Each is emitted
    // at the daemon's canonical mutation point. Wire-frozen: the renderer
    // Wave-C consumers code against the EXACT `kind` tag + field names
    // below. Adding fields later is breaking — append a new variant
    // instead. Field names are camelCase on the wire (per-field
    // `rename`), matching the existing SessionEvent convention.
    //
    // **Routing classes** (see `session_events_ws::event_matches_workspace`):
    //   - APP-LEVEL  (forwarded to EVERY subscriber regardless of `?path=`):
    //     `LlmStatusChanged`, `TunnelStatusChanged`, `AgentStatusChanged`.
    //   - WORKSPACE-SCOPED (forwarded only when the carried path matches
    //     the subscriber's `?path=` via the cwd-prefix rule):
    //     `ReviewQueueChanged`, `ReviewChanged`, `TabTitleChanged`,
    //     `TabOrderChanged`, `HeartbeatStateChanged` — each carries a
    //     `workspacePath` used by the prefix filter.

    /// #675.1 — local LLM (AI Workspace Assistant) model status changed.
    /// APP-LEVEL. Replaces the renderer's `/cli/llm/status` poll
    /// (assistant.ts). Emitted on every transition that the renderer
    /// cares about: model load, download begin/end, and download
    /// progress ticks.
    ///
    /// Wire: `{ "kind": "llm_status_changed", "loaded": bool,
    /// "modelPath": string|null, "downloading": bool,
    /// "downloadPercent": number|null }`.
    ///
    /// `downloadPercent` is `Some(0.0..=100.0)` while a download is in
    /// flight (carried on the progress emits), `None` otherwise.
    LlmStatusChanged {
        loaded: bool,
        #[serde(rename = "modelPath")]
        model_path: Option<String>,
        downloading: bool,
        #[serde(rename = "downloadPercent")]
        download_percent: Option<f64>,
    },

    /// #675.2 — an agent's working/idle status flipped. APP-LEVEL.
    /// Replaces the renderer's terminal/list-running + agent-status poll
    /// (active-agents.ts) so spinners are push-driven. Emitted from the
    /// daemon's agent-lifecycle hook chokepoint
    /// (`handle_hook_complete`), which maps every raw harness hook into
    /// the canonical `start`/`stop`/`permission` buckets.
    ///
    /// Wire: `{ "kind": "agent_status_changed", "paneId": string,
    /// "tabId": string, "status": "start"|"stop"|"permission" }`.
    ///
    /// `paneId` is the `K2SO_PANE_ID` (== terminal id) the PTY was
    /// spawned with — the same key `agent:lifecycle` carries today and
    /// the renderer already correlates spinners against.
    AgentStatusChanged {
        #[serde(rename = "paneId")]
        pane_id: String,
        #[serde(rename = "tabId")]
        tab_id: String,
        /// Canonical bucket: `start` (working) | `stop` (idle) |
        /// `permission` (awaiting approval).
        status: String,
    },

    /// #675.3 — the per-workspace review queue changed (a review was
    /// approved / rejected / had changes requested, or a checklist
    /// mutation altered queue membership). WORKSPACE-SCOPED. Replaces
    /// the renderer's `/cli/agents/review-queue` poll (review-queue.ts).
    ///
    /// Wire: `{ "kind": "review_queue_changed", "workspacePath":
    /// string }`. The renderer re-fetches the queue for the matching
    /// workspace on receipt.
    ReviewQueueChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
    },

    /// #675.4 + #677.2 — a specific review (or its checklist) changed.
    /// WORKSPACE-SCOPED. Replaces the renderer's reviews+chats poll
    /// (ReviewPanel.tsx) AND the review-checklist poll — ONE event
    /// covers both. Emitted on review approve/reject/request-changes
    /// and on every review-checklist write/toggle/init.
    ///
    /// Wire: `{ "kind": "review_changed", "workspacePath": string,
    /// "agent": string|null }`. `agent` is the agent/branch the review
    /// belongs to when the mutation point knows it (checklist + queue
    /// mutations carry it); `null` for queue-wide changes. The renderer
    /// re-fetches the review detail + checklist for the matching
    /// workspace on receipt.
    ReviewChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        agent: Option<String>,
    },

    /// #675.5 — K2 Connect tunnel connector state transitioned.
    /// APP-LEVEL. Replaces the renderer's `/cli/tunnel/status` poll
    /// (CompanionSection.tsx). Emitted on start / stop. Carries the
    /// new `running` flag + predicted public URL so the renderer can
    /// render without an immediate follow-up GET.
    ///
    /// Wire: `{ "kind": "tunnel_status_changed", "running": bool,
    /// "publicUrl": string|null }`.
    TunnelStatusChanged {
        running: bool,
        #[serde(rename = "publicUrl")]
        public_url: Option<String>,
    },

    /// #676 — a tab title was set daemon-side. WORKSPACE-SCOPED.
    /// Push counterpart to the new `POST /cli/workspace/set-tab-title`
    /// route + the daemon-canonical `tab_titles` store. Replaces the
    /// renderer-local-only `setTabTitle` so a rename in window A shows
    /// up in window B + the mobile companion.
    ///
    /// Wire: `{ "kind": "tab_title_changed", "workspacePath": string,
    /// "project": string, "tabId": string, "title": string }`.
    /// `workspacePath` is the project path used by the prefix filter;
    /// `project` is the same value echoed under the contract name the
    /// route request uses (kept distinct so the filter field can't
    /// drift from the addressing field).
    TabTitleChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        project: String,
        #[serde(rename = "tabId")]
        tab_id: String,
        title: String,
    },

    /// #677.3 — workspace tab-order persistence advanced. WORKSPACE-
    /// SCOPED. Carries the monotonic `revision` the daemon stamped on
    /// the write so concurrent clients resolve last-write-wins
    /// deterministically: a renderer drops a stale local write whose
    /// base revision is behind this one.
    ///
    /// Wire: `{ "kind": "tab_order_changed", "workspacePath": string,
    /// "project": string, "workspace": string, "revision": number }`.
    /// `project` + `workspace` are the `(project_id, workspace_id)` key
    /// of the `workspace_layouts` row; `workspacePath` is the project
    /// path used by the prefix filter.
    TabOrderChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        project: String,
        workspace: String,
        revision: i64,
    },

    /// #677.1 — a heartbeat session's live (PTY-attached) state flipped.
    /// WORKSPACE-SCOPED. The daemon owns the PTY lifecycle; this fires
    /// when a heartbeat's `active_terminal_id` is stamped (live=true on
    /// wake/spawn) or nulled (live=false on PTY exit). Lets every client
    /// converge the heartbeat live-dot without polling.
    ///
    /// Wire: `{ "kind": "heartbeat_state_changed", "workspacePath":
    /// string, "project": string, "agent": string, "live": bool }`.
    /// `project` is the project_id; `agent` is the heartbeat name;
    /// `workspacePath` is the project path used by the prefix filter
    /// (empty string when the spawn path doesn't have it resolved — the
    /// renderer then matches on `project`).
    HeartbeatStateChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        project: String,
        agent: String,
        live: bool,
    },

    /// 0.39.45 (GH #18/#26) — the registered project set changed (a
    /// project was added, removed, or re-registered). APP-LEVEL. The
    /// renderer re-fetches the project list on receipt, so a workspace
    /// added in another window, by the CLI (`k2so workspace create`),
    /// or by an onboarding flow appears in the nav/Settings WITHOUT a
    /// manual window reload. Pre-0.39.45 the only signal was the local
    /// Tauri `sync:projects` event, which never fires for CLI/remote
    /// mutations (and doesn't reach K2 Connect clients at all).
    ///
    /// Wire: `{ "kind": "projects_changed" }`. Deliberately payload-
    /// free — consumers re-fetch the canonical list rather than
    /// patching from a diff.
    ProjectsChanged {},
}

static SENDER: OnceLock<broadcast::Sender<SessionEvent>> = OnceLock::new();

/// Lazy accessor for the broadcast sender. First caller creates the
/// channel. Cheap — the OnceLock is a single atomic load on the hot
/// path.
pub fn sender() -> &'static broadcast::Sender<SessionEvent> {
    SENDER.get_or_init(|| broadcast::channel(EVENT_CHANNEL_CAP).0)
}

/// Take a fresh `Receiver`. One per WS subscriber. Drops cleanly
/// when the subscriber disconnects.
pub fn subscribe() -> broadcast::Receiver<SessionEvent> {
    sender().subscribe()
}

/// Best-effort emit. Returns `Ok(n)` with the number of subscribers
/// the event was queued for; `Err` when there are zero subscribers
/// (treated as a no-op by callers via `let _ =`). The emit cost is
/// bounded — broadcast doesn't synchronize beyond a single atomic
/// per subscriber.
pub fn emit(event: SessionEvent) -> Result<usize, broadcast::error::SendError<SessionEvent>> {
    sender().send(event)
}

/// 0.39.39 (#677.1) — best-effort broadcast that a heartbeat session's
/// live (PTY-attached) state flipped. Call with `live=true` right after
/// stamping `active_terminal_id` on a heartbeat spawn, and `live=false`
/// when the PTY exits (the unregister chokepoint does the latter). The
/// broadcast `let _ =`-swallows the no-subscribers case. `workspace_path`
/// may be empty when the spawn path doesn't have it resolved — the
/// renderer then matches on `project` (the project_id).
pub fn emit_heartbeat_live(workspace_path: &str, project_id: &str, agent: &str, live: bool) {
    let _ = emit(SessionEvent::HeartbeatStateChanged {
        workspace_path: workspace_path.to_string(),
        project: project_id.to_string(),
        agent: agent.to_string(),
        live,
    });
}

/// Extract `tab-<paneGroupId>` from a `v2_session_map` agent_name.
/// Returns `None` for non-tab-shaped keys (pinned chat = bare UUID,
/// heartbeats = bare workspace key, legacy `<pid>:<agent>` shapes).
/// The renderer uses this to decide whether to surface the event as
/// a new tab — non-tab events are forwarded so the mobile companion
/// can see the full inventory without the renderer adopting them.
pub fn pane_group_id_from_agent(agent_name: &str) -> Option<String> {
    agent_name
        .strip_prefix("tab-")
        .map(|rest| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FROZEN WIRE CONTRACT (task #672): the renderer codes against
    /// EXACTLY `{ "kind": "active_changed", "activeProjectIds":
    /// string[], "activeWindowHours": number }`. Pin the serialized
    /// shape so any field rename / tag drift fails CI loudly.
    #[test]
    fn active_changed_serializes_to_frozen_contract() {
        let ev = SessionEvent::ActiveChanged {
            active_project_ids: vec!["pid-a".to_string(), "pid-b".to_string()],
            active_window_hours: 24,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(json["kind"], "active_changed");
        assert_eq!(
            json["activeProjectIds"],
            serde_json::json!(["pid-a", "pid-b"])
        );
        assert_eq!(json["activeWindowHours"], 24);
        // No stray snake_case leakage.
        assert!(json.get("active_project_ids").is_none());
        assert!(json.get("active_window_hours").is_none());
        // Exactly the three documented keys.
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 3, "unexpected extra fields: {obj:?}");
    }

    // ── 0.39.39 (#675/#676/#677) FROZEN WIRE CONTRACTS ──────────────
    // Each test pins the EXACT serialized `kind` tag + the EXACT field
    // key set the Wave-C renderer consumers code against. A field rename
    // or tag drift fails CI loudly.

    fn as_json(ev: &SessionEvent) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(ev).unwrap()).unwrap()
    }

    fn assert_keys(json: &serde_json::Value, keys: &[&str]) {
        let obj = json.as_object().unwrap();
        for k in keys {
            assert!(obj.contains_key(*k), "missing key {k} in {obj:?}");
        }
        assert_eq!(
            obj.len(),
            keys.len(),
            "unexpected key set: {obj:?} (expected exactly {keys:?})"
        );
    }

    #[test]
    fn llm_status_changed_frozen_contract() {
        let json = as_json(&SessionEvent::LlmStatusChanged {
            loaded: true,
            model_path: Some("/m/q.gguf".into()),
            downloading: false,
            download_percent: Some(42.5),
        });
        assert_eq!(json["kind"], "llm_status_changed");
        assert_eq!(json["loaded"], true);
        assert_eq!(json["modelPath"], "/m/q.gguf");
        assert_eq!(json["downloading"], false);
        assert_eq!(json["downloadPercent"], 42.5);
        assert_keys(&json, &["kind", "loaded", "modelPath", "downloading", "downloadPercent"]);
        // null model_path + percent serialize as JSON null (present key).
        let json2 = as_json(&SessionEvent::LlmStatusChanged {
            loaded: false,
            model_path: None,
            downloading: true,
            download_percent: None,
        });
        assert!(json2["modelPath"].is_null());
        assert!(json2["downloadPercent"].is_null());
    }

    #[test]
    fn agent_status_changed_frozen_contract() {
        let json = as_json(&SessionEvent::AgentStatusChanged {
            pane_id: "term-1".into(),
            tab_id: "tab-1".into(),
            status: "start".into(),
        });
        assert_eq!(json["kind"], "agent_status_changed");
        assert_eq!(json["paneId"], "term-1");
        assert_eq!(json["tabId"], "tab-1");
        assert_eq!(json["status"], "start");
        assert_keys(&json, &["kind", "paneId", "tabId", "status"]);
    }

    #[test]
    fn review_queue_changed_frozen_contract() {
        let json = as_json(&SessionEvent::ReviewQueueChanged {
            workspace_path: "/x/foo".into(),
        });
        assert_eq!(json["kind"], "review_queue_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_keys(&json, &["kind", "workspacePath"]);
    }

    #[test]
    fn review_changed_frozen_contract() {
        let json = as_json(&SessionEvent::ReviewChanged {
            workspace_path: "/x/foo".into(),
            agent: Some("scout".into()),
        });
        assert_eq!(json["kind"], "review_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["agent"], "scout");
        assert_keys(&json, &["kind", "workspacePath", "agent"]);
        let json2 = as_json(&SessionEvent::ReviewChanged {
            workspace_path: "/x/foo".into(),
            agent: None,
        });
        assert!(json2["agent"].is_null());
    }

    #[test]
    fn tunnel_status_changed_frozen_contract() {
        let json = as_json(&SessionEvent::TunnelStatusChanged {
            running: true,
            public_url: Some("https://rosson.k2.dev".into()),
        });
        assert_eq!(json["kind"], "tunnel_status_changed");
        assert_eq!(json["running"], true);
        assert_eq!(json["publicUrl"], "https://rosson.k2.dev");
        assert_keys(&json, &["kind", "running", "publicUrl"]);
    }

    #[test]
    fn tab_title_changed_frozen_contract() {
        let json = as_json(&SessionEvent::TabTitleChanged {
            workspace_path: "/x/foo".into(),
            project: "proj-uuid".into(),
            tab_id: "tab-7".into(),
            title: "My Tab".into(),
        });
        assert_eq!(json["kind"], "tab_title_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["project"], "proj-uuid");
        assert_eq!(json["tabId"], "tab-7");
        assert_eq!(json["title"], "My Tab");
        assert_keys(&json, &["kind", "workspacePath", "project", "tabId", "title"]);
    }

    #[test]
    fn tab_order_changed_frozen_contract() {
        let json = as_json(&SessionEvent::TabOrderChanged {
            workspace_path: "/x/foo".into(),
            project: "proj-uuid".into(),
            workspace: "ws-uuid".into(),
            revision: 5,
        });
        assert_eq!(json["kind"], "tab_order_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["project"], "proj-uuid");
        assert_eq!(json["workspace"], "ws-uuid");
        assert_eq!(json["revision"], 5);
        assert_keys(&json, &["kind", "workspacePath", "project", "workspace", "revision"]);
    }

    #[test]
    fn heartbeat_state_changed_frozen_contract() {
        let json = as_json(&SessionEvent::HeartbeatStateChanged {
            workspace_path: "/x/foo".into(),
            project: "proj-uuid".into(),
            agent: "nightly".into(),
            live: true,
        });
        assert_eq!(json["kind"], "heartbeat_state_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["project"], "proj-uuid");
        assert_eq!(json["agent"], "nightly");
        assert_eq!(json["live"], true);
        assert_keys(&json, &["kind", "workspacePath", "project", "agent", "live"]);
    }

    #[test]
    fn pane_group_id_extracts_tab_prefix() {
        assert_eq!(
            pane_group_id_from_agent("tab-abc123"),
            Some("abc123".to_string()),
        );
        assert_eq!(
            pane_group_id_from_agent("tab-uuid-with-hyphens-here"),
            Some("uuid-with-hyphens-here".to_string()),
        );
    }

    #[test]
    fn pane_group_id_is_none_for_non_tab_agents() {
        assert_eq!(pane_group_id_from_agent(""), None);
        assert_eq!(
            pane_group_id_from_agent("c3c5b9d6-e123-4456-89ab-cdef01234567"),
            None,
        );
        assert_eq!(pane_group_id_from_agent("__lead__"), None);
        assert_eq!(
            pane_group_id_from_agent("project-id:agent-name"),
            None,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn emit_with_no_subscribers_is_a_no_op() {
        // Fresh OnceLock is fine for this test only because no other
        // subscribers exist in this process at this moment. Best-effort
        // assertion: emit returns Err (zero subscribers) and we don't
        // care.
        let res = emit(SessionEvent::SessionRemoved {
            workspace_path: "/tmp".into(),
            pane_group_id: None,
            agent_name: "test-no-subs".into(),
        });
        // Either Err (no subs) or Ok(0+) — both fine; emitter callers
        // never inspect the result.
        let _ = res;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_emitted_event() {
        // The broadcast channel is process-global, so concurrent
        // tests (especially `register`/`unregister` integrations on
        // a different test thread) can leak events into our
        // receiver. Use a unique session_id as a probe and drain
        // events until we find the one we emitted — anything else
        // is contamination and gets ignored.
        let probe_id = format!(
            "test-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let mut rx = subscribe();
        let _ = emit(SessionEvent::SessionAdded {
            workspace_path: "/x/foo".into(),
            pane_group_id: Some("pg-1".into()),
            agent_name: "tab-pg-1".into(),
            command: Some("zsh".into()),
            args: vec![],
            session_id: probe_id.clone(),
            is_v2: true,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("did not receive probe event in time");
            }
            let got = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(_)) => panic!("receiver closed"),
                Err(_) => panic!("timed out waiting for probe event"),
            };
            if let SessionEvent::SessionAdded { ref session_id, .. } = got {
                if session_id == &probe_id {
                    match got {
                        SessionEvent::SessionAdded { workspace_path, pane_group_id, agent_name, .. } => {
                            assert_eq!(workspace_path, "/x/foo");
                            assert_eq!(pane_group_id.as_deref(), Some("pg-1"));
                            assert_eq!(agent_name, "tab-pg-1");
                        }
                        _ => unreachable!(),
                    }
                    break;
                }
            }
            // Otherwise: contamination from another test — keep
            // draining.
        }
    }

    #[test]
    fn workspace_path_filter_rules_match_cli_endpoint() {
        // Document the filter rule in a test. The actual filter is
        // applied in session_events_ws.rs's per-subscriber loop;
        // this test just pins the rule.
        fn matches(cwd: &str, workspace_path: &str) -> bool {
            let trimmed = workspace_path.trim_end_matches('/');
            let prefix_with_slash = if trimmed.is_empty() {
                "/".to_string()
            } else {
                format!("{}/", trimmed)
            };
            let cwd_trim = cwd.trim_end_matches('/');
            cwd_trim == trimmed || cwd.starts_with(&prefix_with_slash)
        }
        assert!(matches("/x/K2SO", "/x/K2SO"));
        assert!(matches("/x/K2SO/sub", "/x/K2SO"));
        assert!(matches("/x/K2SO/sub/deeper", "/x/K2SO"));
        // Sibling — must NOT match (the exact bug `list-for-workspace`
        // got bitten by before).
        assert!(!matches("/x/K2SO-website", "/x/K2SO"));
        assert!(!matches("/x/other", "/x/K2SO"));
        // Trailing-slash tolerance.
        assert!(matches("/x/K2SO/", "/x/K2SO"));
        assert!(matches("/x/K2SO", "/x/K2SO/"));
    }
}
