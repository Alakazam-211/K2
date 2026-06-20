//! `/cli/sessions/events?path=<workspace>` WebSocket endpoint.
//!
//! Push-driven session lifecycle stream. Wire format defined in
//! `session_events.rs::SessionEvent`. Each subscriber filters events
//! by `cwd starts_with path`, mirroring the existing
//! `/cli/sessions/list-for-workspace` HTTP endpoint's filter rule
//! (see `cli.rs`).
//!
//! Lifecycle:
//!   1. `main.rs::handle_connection` token-auths the request and
//!      extracts `?path=<workspace>` before handing the raw stream
//!      to `serve_session_events_connection`.
//!   2. Handshake via `tokio_tungstenite::accept_async`.
//!   3. Subscribe to the process-wide broadcast bus
//!      (`session_events::subscribe`).
//!   4. Emit a `hello` event immediately so the renderer knows the
//!      subscription is live and can decide whether to re-reconcile
//!      after a drop+reconnect.
//!   5. For each broadcast event whose `workspace_path` matches the
//!      subscriber's `path`, serialize + forward as JSON text.
//!   6. Ping/pong + graceful close mirror `sessions_grid_ws.rs` and
//!      `awareness_ws.rs`.
//!
//! Token check happens in the dispatcher (`main.rs`) before we're
//! called — same pattern as the other WS routes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;

use crate::session_events::{self, SessionEvent};

/// Monotonically-increasing subscriber id generator. Used so each
/// `hello` event carries a unique id the renderer can log for
/// debug + dedupe across reconnects.
static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(1);

/// Outbound `hello` frame sent immediately after WS handshake.
/// Distinct shape from `SessionEvent` because the renderer
/// special-cases it (acknowledges that subscription is live).
#[derive(Debug, Serialize)]
struct HelloEvent {
    kind: &'static str,
    workspace_path: String,
    subscriber_id: u64,
}

/// Pre-handshake error frame. Sent then immediately closed.
#[derive(Debug, Serialize)]
struct ErrorEvent {
    kind: &'static str,
    message: String,
}

/// WS handler entry point. Called by `main.rs::handle_connection`
/// after token auth + query parsing. `params` carries the parsed
/// query string; we require `path` and 400 on absence.
pub async fn serve_session_events_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
    owner_token: String,
) {
    // The token that authorized this connection. Re-validated on a timer
    // below so a revoked connect-user is dropped from this live event
    // stream within seconds (mirrors the grid-WS re-auth heartbeat).
    let token = params.get("token").cloned().unwrap_or_default();

    // 0.39.7: stream borrowed (was owned). See events.rs.
    let workspace_path = match params.get("path").map(String::as_str) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => {
            send_error_then_close(stream, "missing 'path' query param").await;
            return;
        }
    };

    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/session_events_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();

    let subscriber_id = NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed);
    let mut rx = session_events::subscribe();

    log_debug!(
        "[daemon/session_events_ws] subscriber {} attached for workspace={}",
        subscriber_id,
        workspace_path,
    );

    // Send the hello immediately so the renderer can mark itself
    // subscribed before any session events arrive.
    let hello = HelloEvent {
        kind: "hello",
        workspace_path: workspace_path.clone(),
        subscriber_id,
    };
    if send_json(&mut write, &hello).await.is_err() {
        return;
    }

    // Re-auth heartbeat (see grid-WS). Every 5s, confirm the connecting
    // token is still valid; a revoked connect-user is kicked off this live
    // event stream rather than left subscribed until they disconnect.
    let mut auth_recheck =
        tokio::time::interval(std::time::Duration::from_secs(5));
    auth_recheck.tick().await; // burn the immediate first tick

    loop {
        tokio::select! {
            _ = auth_recheck.tick() => {
                if !crate::routes::http::token_still_valid(&token, &owner_token) {
                    log_debug!(
                        "[daemon/session_events_ws] token revoked mid-session; \
                         closing events WS for subscriber {}",
                        subscriber_id,
                    );
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            }
            recv = rx.recv() => {
                match recv {
                    Ok(event) => {
                        if !event_matches_workspace(&event, &workspace_path) {
                            continue;
                        }
                        if send_json(&mut write, &event).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Subscriber fell behind. Skip the dropped
                        // events — the renderer's reconcile-on-
                        // reconnect path is the recovery story.
                        log_debug!(
                            "[daemon/session_events_ws] subscriber {} lagged {n} events",
                            subscriber_id,
                        );
                        continue;
                    }
                    Err(RecvError::Closed) => {
                        // Bus channel closed — only happens on daemon
                        // shutdown. Exit cleanly.
                        log_debug!(
                            "[daemon/session_events_ws] event bus closed for subscriber {}",
                            subscriber_id,
                        );
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        let _ = write.send(Message::Close(frame)).await;
                        break;
                    }
                    Some(Ok(_)) => {
                        // Text/Binary inbound not used by this
                        // protocol — clients are receive-only.
                    }
                    None => break,
                    Some(Err(e)) => {
                        log_debug!(
                            "[daemon/session_events_ws] read error: {e}"
                        );
                        break;
                    }
                }
            }
        }
    }

    log_debug!(
        "[daemon/session_events_ws] subscriber {} detached (workspace={})",
        subscriber_id,
        workspace_path,
    );
}

/// Apply the same prefix filter the `list-for-workspace` HTTP endpoint
/// uses (`cli.rs`). Documented and unit-tested in
/// `session_events::tests::workspace_path_filter_rules_match_cli_endpoint`.
fn event_matches_workspace(event: &SessionEvent, workspace_path: &str) -> bool {
    let cwd = match event {
        SessionEvent::SessionAdded { workspace_path: cwd, .. } => cwd,
        SessionEvent::SessionRemoved { workspace_path: cwd, .. } => cwd,
        SessionEvent::SessionRenamed { workspace_path: cwd, .. } => cwd,
        // task #672 — ActiveChanged is APP-LEVEL (the canonical Active
        // union for the whole daemon), not tied to one workspace path.
        // Forward it to EVERY subscriber so each client mirrors the
        // global set regardless of which `?path=` it subscribed under.
        // The renderer's app-level `subscribeToActiveState` consumer
        // filters for `kind === "active_changed"`.
        SessionEvent::ActiveChanged { .. } => return true,

        // 0.39.39 (#675) APP-LEVEL events — daemon-global state with no
        // single workspace. Forward to EVERY subscriber so each client
        // mirrors the truth regardless of `?path=`. The renderer's
        // app-level consumers filter by `kind`.
        //   - LlmStatusChanged    — one model host per daemon.
        //   - TunnelStatusChanged — one tunnel per daemon.
        //   - AgentStatusChanged  — keyed by paneId (terminal id), not a
        //     workspace path; the renderer correlates by paneId (same as
        //     the existing `agent:lifecycle` consumer).
        SessionEvent::LlmStatusChanged { .. }
        | SessionEvent::TunnelStatusChanged { .. }
        | SessionEvent::AgentStatusChanged { .. } => return true,

        // 0.39.45 (GH #18/#26) APP-LEVEL — the registered project set
        // changed. Every client re-fetches its project list; there is
        // no single workspace to scope to (the new project isn't in
        // any subscriber's `?path=` yet, by definition).
        SessionEvent::ProjectsChanged {} => return true,

        // 0.39.39 WORKSPACE-SCOPED events — each carries a project path
        // in `workspace_path`; the cwd-prefix filter below routes them to
        // the matching subscriber exactly like SessionAdded/Removed.
        SessionEvent::ReviewQueueChanged { workspace_path: cwd } => cwd,
        SessionEvent::ReviewChanged { workspace_path: cwd, .. } => cwd,
        SessionEvent::TabTitleChanged { workspace_path: cwd, .. } => cwd,
        SessionEvent::TabOrderChanged { workspace_path: cwd, .. } => cwd,
        SessionEvent::HeartbeatStateChanged { workspace_path: cwd, .. } => cwd,
    };
    let trimmed = workspace_path.trim_end_matches('/');
    let prefix_with_slash = if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", trimmed)
    };
    let cwd_trim = cwd.trim_end_matches('/');
    cwd_trim == trimmed || cwd.starts_with(&prefix_with_slash)
}

async fn send_json<W, T>(write: &mut W, msg: &T) -> Result<(), ()>
where
    W: futures_util::SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    T: Serialize,
{
    let text = match serde_json::to_string(msg) {
        Ok(s) => s,
        Err(e) => {
            log_debug!(
                "[daemon/session_events_ws] serialize failed: {e}"
            );
            return Err(());
        }
    };
    write.send(Message::Text(text.into())).await.map_err(|e| {
        log_debug!("[daemon/session_events_ws] send failed: {e}");
    })
}

async fn send_error_then_close(stream: &mut TcpStream, msg: &str) {
    let err = ErrorEvent {
        kind: "error",
        message: msg.to_string(),
    };
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, _read) = ws.split();
    let _ = send_json(&mut write, &err).await;
    let _ = write.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_events::SessionEvent;

    fn added(cwd: &str) -> SessionEvent {
        SessionEvent::SessionAdded {
            workspace_path: cwd.into(),
            pane_group_id: Some("pg".into()),
            agent_name: "tab-pg".into(),
            command: None,
            args: vec![],
            session_id: "00000000-0000-0000-0000-000000000000".into(),
            is_v2: true,
        }
    }

    fn removed(cwd: &str) -> SessionEvent {
        SessionEvent::SessionRemoved {
            workspace_path: cwd.into(),
            pane_group_id: Some("pg".into()),
            agent_name: "tab-pg".into(),
        }
    }

    #[test]
    fn matches_exact_path() {
        assert!(event_matches_workspace(&added("/x/foo"), "/x/foo"));
    }

    #[test]
    fn matches_subdirectory_of_workspace() {
        assert!(event_matches_workspace(&added("/x/foo/bar"), "/x/foo"));
    }

    #[test]
    fn rejects_sibling_with_shared_prefix() {
        // Same bug the cli endpoint was bitten by — /x/foo must NOT
        // match a session under /x/foo-website.
        assert!(!event_matches_workspace(&added("/x/foo-website/x"), "/x/foo"));
    }

    #[test]
    fn rejects_unrelated_path() {
        assert!(!event_matches_workspace(&removed("/x/other"), "/x/foo"));
    }

    #[test]
    fn trailing_slash_tolerance() {
        assert!(event_matches_workspace(&added("/x/foo/"), "/x/foo"));
        assert!(event_matches_workspace(&added("/x/foo"), "/x/foo/"));
    }

    // ── 0.39.39 routing classes ──────────────────────────────────────

    #[test]
    fn app_level_events_forward_to_every_subscriber() {
        // LLM / tunnel / agent-status are daemon-global: must match ANY
        // ?path= (same rule as ActiveChanged).
        let llm = SessionEvent::LlmStatusChanged {
            loaded: true,
            model_path: None,
            downloading: false,
            download_percent: None,
        };
        let tunnel = SessionEvent::TunnelStatusChanged {
            running: true,
            public_url: None,
        };
        let agent = SessionEvent::AgentStatusChanged {
            pane_id: "t".into(),
            tab_id: "tab".into(),
            status: "start".into(),
        };
        for ev in [&llm, &tunnel, &agent] {
            assert!(event_matches_workspace(ev, "/x/foo"));
            assert!(event_matches_workspace(ev, "/totally/unrelated"));
            assert!(event_matches_workspace(ev, ""));
        }
    }

    #[test]
    fn workspace_scoped_events_use_prefix_filter() {
        let review = SessionEvent::ReviewChanged {
            workspace_path: "/x/foo".into(),
            agent: None,
        };
        let queue = SessionEvent::ReviewQueueChanged {
            workspace_path: "/x/foo".into(),
        };
        let title = SessionEvent::TabTitleChanged {
            workspace_path: "/x/foo".into(),
            project: "p".into(),
            tab_id: "t".into(),
            title: "T".into(),
        };
        let order = SessionEvent::TabOrderChanged {
            workspace_path: "/x/foo".into(),
            project: "p".into(),
            workspace: "w".into(),
            revision: 1,
        };
        let hb = SessionEvent::HeartbeatStateChanged {
            workspace_path: "/x/foo".into(),
            project: "p".into(),
            agent: "a".into(),
            live: true,
        };
        for ev in [&review, &queue, &title, &order, &hb] {
            // Matches its own workspace + descendants.
            assert!(event_matches_workspace(ev, "/x/foo"));
            assert!(event_matches_workspace(ev, "/x"));
            // Sibling with shared prefix must NOT match.
            assert!(!event_matches_workspace(ev, "/x/foobar"));
            // Unrelated workspace must NOT match.
            assert!(!event_matches_workspace(ev, "/y/bar"));
        }
    }
}
