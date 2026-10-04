//! `WS /cli/activity/events?workspace=` — agent working for one room.
//!
//! Subscribes to the session-event broadcast and forwards only
//! `SessionActivityChanged`. Every other kind is dropped. The event's
//! `workspacePath` is `session.cwd`; a frame is sent when that cwd is the
//! room root, a worktree root from `tree_roots`, or a directory under one
//! of those, on a path boundary. The guest frame has no absolute path.
//!
//! Skin: pass `Some(SkinPass)`. Room, then `activity:read` OR
//! `heartbeats:read` (AH20/AH29), both before `accept_async`. Each frame
//! then needs its own cap: `session_activity_changed` → `activity:read`,
//! `heartbeat_changed` → `heartbeats:read`. Owner/Connect: `None` (both
//! kinds; still requires `workspace=`).
//!
//! Heartbeat frames (prd-app-heartbeats-surface-v1 AH21): one guest kind,
//! `{"kind":"heartbeat_changed","workspace":"<handle>"}` from
//! `heartbeat_roster_changed` (no row data — refetch), plus `name` and
//! `live` from `heartbeat_state_changed`. A frame belongs to the room when
//! its project id is the room's; a roster event with an empty project id
//! falls back to its path equal to the room root. No path on the wire.
//! Roster frames are coalesced to one per room per 250 ms (AH31), with a
//! trailing frame so the last change is never lost.
//!
//! Ticket frames (prd-app-tickets-websocket-v1, 0.43.3): `tickets:read`
//! also opens the socket, and each `ticket_changed` session event whose
//! `projectId` is the room's becomes
//! `{"kind":"ticket_changed","workspace":"<handle>","id","change","status",
//! "via","hasBrief"}`. Ids and metadata only: no title, body, comment
//! text, assignee names or brief HTML (refetch `show`; the brief stays
//! `show?brief=1`). Not coalesced: one stored mutation, one frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::skin::SkinPass;

use crate::fs_routes::{resolve_owner_workspace, resolve_skin_workspace, SkinWorkspace};
use crate::session_events::{self, SessionEvent};

/// Wire path. Not `/cli/sessions/events`.
pub const ACTIVITY_EVENTS_WS_PATH: &str = "/cli/activity/events";

/// AH29 — the caps that open this socket (any one, in the room).
/// `tickets:read` joined in 0.43.3 (prd-app-tickets-websocket-v1).
pub const SOCKET_CAPS: [&str; 3] = [
    crate::skin_routes::ACTIVITY_READ,
    k2_core::skin::CAP_HEARTBEATS_READ,
    crate::skin_routes::TICKETS_READ,
];

/// AH20/AH29 refusal cap text: names every socket cap, so the 403 says
/// `missing capability activity:read, heartbeats:read or tickets:read`.
pub const SOCKET_CAPS_TEXT: &str = "activity:read, heartbeats:read or tickets:read";

/// AH31 — at most one roster-derived `heartbeat_changed` per room per this.
pub const HEARTBEAT_COALESCE: std::time::Duration = std::time::Duration::from_millis(250);

/// What a heartbeat event means for one room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatFrame {
    /// `heartbeat_roster_changed` → refetch (coalesced).
    Roster,
    /// `heartbeat_state_changed` → sent as-is.
    State(String),
}

/// AH21 — map a heartbeat event to a guest frame for the room
/// (`project_id`, `room_root`), or `None` (other room / other kind).
pub fn heartbeat_frame(
    project_id: &str,
    room_root: &str,
    wire_workspace: &str,
    event: &SessionEvent,
) -> Option<HeartbeatFrame> {
    match event {
        SessionEvent::HeartbeatRosterChanged {
            workspace_path,
            project_id: pid,
        } => {
            let ours = if pid.trim().is_empty() {
                same_path(workspace_path, room_root)
            } else {
                pid == project_id
            };
            ours.then_some(HeartbeatFrame::Roster)
        }
        SessionEvent::HeartbeatStateChanged {
            project,
            agent,
            live,
            ..
        } => (project == project_id).then(|| {
            HeartbeatFrame::State(
                serde_json::json!({
                    "kind": "heartbeat_changed",
                    "workspace": wire_workspace,
                    "name": agent,
                    "live": live,
                })
                .to_string(),
            )
        }),
        _ => None,
    }
}

/// prd-app-tickets-websocket-v1 — the guest frame for a ticket event in
/// this room (`project_id`), or `None` (other room / other kind). Matches
/// on the project id only: a ticket belongs to exactly one room.
pub fn ticket_frame(project_id: &str, wire_workspace: &str, event: &SessionEvent) -> Option<String> {
    let SessionEvent::TicketChanged {
        project_id: pid,
        id,
        change,
        status,
        via,
        has_brief,
    } = event
    else {
        return None;
    };
    if project_id.trim().is_empty() || pid != project_id {
        return None;
    }
    Some(
        serde_json::json!({
            "kind": "ticket_changed",
            "workspace": wire_workspace,
            "id": id,
            "change": change,
            "status": status,
            "via": via,
            "hasBrief": has_brief,
        })
        .to_string(),
    )
}

/// The roster frame text (no row data).
pub fn heartbeat_roster_json(wire_workspace: &str) -> String {
    serde_json::json!({ "kind": "heartbeat_changed", "workspace": wire_workspace }).to_string()
}

/// Canonical path equality (`/tmp` vs `/private/tmp`).
fn same_path(a: &str, b: &str) -> bool {
    if slash(a).is_empty() || slash(b).is_empty() {
        return false;
    }
    if slash(a) == slash(b) {
        return true;
    }
    match (std::fs::canonicalize(a.trim()), std::fs::canonicalize(b.trim())) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn slash(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// `/sales` matches `/sales` and `/sales/app`. It does not match `/sales-other`.
fn boundary(path: &str, root: &str) -> bool {
    let path = slash(path);
    let root = slash(root);
    !root.is_empty() && (path == root || path.starts_with(&format!("{root}/")))
}

/// True when `cwd` is a root or a directory under one, on a path boundary.
/// Canonicalize when the path exists so `/tmp` and `/private/tmp` agree.
pub fn cwd_under_any_root(cwd: &str, roots: &[PathBuf]) -> bool {
    if slash(cwd).is_empty() {
        return false;
    }
    let cwd_canon = std::fs::canonicalize(Path::new(cwd.trim()))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    for root in roots {
        let root_s = root.to_string_lossy().into_owned();
        if boundary(cwd, &root_s) {
            return true;
        }
        if let Some(ref cc) = cwd_canon {
            if boundary(cc, &root_s) {
                return true;
            }
        }
        if let Ok(rc) = std::fs::canonicalize(root) {
            let rs = rc.to_string_lossy().into_owned();
            if boundary(cwd, &rs) {
                return true;
            }
            if let Some(ref cc) = cwd_canon {
                if boundary(cc, &rs) {
                    return true;
                }
            }
        }
    }
    false
}

fn activity_json(
    workspace: &str,
    agent_name: &str,
    pane_group_id: &Option<String>,
    status: &str,
) -> String {
    serde_json::json!({
        "kind": "session_activity_changed",
        "workspace": workspace,
        "agentName": agent_name,
        "paneGroupId": pane_group_id,
        "status": status,
    })
    .to_string()
}

/// One guest frame, or nothing. DB errors drop the event.
fn activity_frame(project_id: &str, wire_workspace: &str, event: &SessionEvent) -> Option<String> {
    let SessionEvent::SessionActivityChanged {
        workspace_path,
        agent_name,
        pane_group_id,
        status,
    } = event
    else {
        return None;
    };
    let roots = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::workspace_resources::tree_roots(&conn, project_id) {
            Ok(roots) => roots,
            Err(_) => return None,
        }
    };
    if !cwd_under_any_root(workspace_path, &roots) {
        return None;
    }
    Some(activity_json(
        wire_workspace,
        agent_name,
        pane_group_id,
        status,
    ))
}

async fn write_http(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

/// WS handler. Dispatcher already token-authed. Requires `workspace=`.
/// Skin: room, then `activity:read`, both before upgrade.
pub async fn serve_activity_events_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
    skin_pass: Option<SkinPass>,
) {
    let workspace_q = params
        .get("workspace")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(workspace_q) = workspace_q else {
        log_debug!("[daemon/activity_events_ws] missing workspace=");
        write_http(
            stream,
            "400 Bad Request",
            r#"{"error":"missing workspace query parameter"}"#,
        )
        .await;
        return;
    };

    let resolved: SkinWorkspace = if let Some(ref pass) = skin_pass {
        match resolve_skin_workspace(pass, &workspace_q) {
            Ok(ws) => {
                // AH29: any one of the socket caps opens it; each frame
                // checks its own below.
                if !pass.has_any_cap_in_room(&ws.project_id, &SOCKET_CAPS) {
                    let r = crate::skin_routes::missing_cap_response(SOCKET_CAPS_TEXT);
                    write_http(stream, r.status, &r.body).await;
                    return;
                }
                ws
            }
            Err(r) => {
                write_http(stream, r.status, &r.body).await;
                return;
            }
        }
    } else {
        match resolve_owner_workspace(&workspace_q) {
            Ok(ws) => ws,
            Err(r) => {
                write_http(stream, r.status, &r.body).await;
                return;
            }
        }
    };

    let wire_workspace = if resolved.handle.is_empty() {
        workspace_q.clone()
    } else {
        resolved.handle.clone()
    };
    let project_id = resolved.project_id.clone();
    let room_root = resolved.path.clone();
    let (want_activity, want_heartbeats, want_tickets) = match skin_pass {
        Some(ref pass) => (
            pass.has_cap_in_room(&project_id, crate::skin_routes::ACTIVITY_READ),
            pass.has_cap_in_room(&project_id, k2_core::skin::CAP_HEARTBEATS_READ),
            pass.has_cap_in_room(&project_id, crate::skin_routes::TICKETS_READ),
        ),
        None => (true, true, true),
    };

    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/activity_events_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = session_events::subscribe();
    // AH31: last roster frame sent, and a trailing frame owed.
    let mut last_roster: Option<tokio::time::Instant> = None;
    let mut roster_owed = false;

    loop {
        let flush_at = if roster_owed {
            last_roster.map(|t| t + HEARTBEAT_COALESCE)
        } else {
            None
        };
        tokio::select! {
            _ = async {
                match flush_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                roster_owed = false;
                last_roster = Some(tokio::time::Instant::now());
                if write.send(Message::Text(heartbeat_roster_json(&wire_workspace))).await.is_err() {
                    break;
                }
            }
            incoming = read.next() => {
                match incoming {
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_)))
                    | Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(event) => {
                        if let SessionEvent::TicketChanged { .. } = event {
                            // Fail closed: no tickets:read in this room, no frame.
                            if !want_tickets {
                                continue;
                            }
                            let Some(frame) = ticket_frame(&project_id, &wire_workspace, &event) else {
                                continue;
                            };
                            if write.send(Message::Text(frame)).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        if want_heartbeats {
                            match heartbeat_frame(&project_id, &room_root, &wire_workspace, &event) {
                                Some(HeartbeatFrame::Roster) => {
                                    let now = tokio::time::Instant::now();
                                    let recent = last_roster
                                        .is_some_and(|t| now.duration_since(t) < HEARTBEAT_COALESCE);
                                    if recent {
                                        roster_owed = true;
                                    } else {
                                        last_roster = Some(now);
                                        if write
                                            .send(Message::Text(heartbeat_roster_json(&wire_workspace)))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    continue;
                                }
                                Some(HeartbeatFrame::State(text)) => {
                                    if write.send(Message::Text(text)).await.is_err() {
                                        break;
                                    }
                                    continue;
                                }
                                None => {}
                            }
                        }
                        if !want_activity {
                            continue;
                        }
                        let Some(frame) = activity_frame(&project_id, &wire_workspace, &event) else {
                            continue;
                        };
                        if write.send(Message::Text(frame)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    use futures_util::StreamExt;
    use k2_core::skin::{
        RoomPolicy, SkinPass, CAP_ACTIVITY_READ, CAP_FILES_READ, CAP_FILES_WRITE,
        CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE, CAP_THREAD_POST, CAP_THREAD_READ,
        CAP_TICKETS_POST, CAP_TICKETS_READ,
    };
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    fn session_pass(project_id: &str, caps: &[&str]) -> SkinPass {
        let mut room_policy = RoomPolicy::new();
        room_policy.insert(
            project_id.to_string(),
            caps.iter().map(|c| (*c).to_string()).collect(),
        );
        SkinPass {
            id: uuid::Uuid::new_v4().to_string(),
            principal_id: Some(uuid::Uuid::new_v4().to_string()),
            username: "guest".to_string(),
            caps: caps.iter().map(|c| (*c).to_string()).collect(),
            rooms: vec![project_id.to_string()],
            session: true,
            room_policy,
        }
    }

    fn insert_project(path: &str, handle: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, handle, path, handle],
        )
        .expect("insert project");
        id
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-activity-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.canonicalize().expect("canon temp")
    }

    #[test]
    fn cwd_boundary_matches_root_subdir_and_worktree_not_sibling() {
        let roots = vec![PathBuf::from("/sales"), PathBuf::from("/wt/feat")];
        assert!(cwd_under_any_root("/sales", &roots));
        assert!(cwd_under_any_root("/sales/app", &roots));
        assert!(!cwd_under_any_root("/sales-other", &roots));
        assert!(!cwd_under_any_root("/sales-other/app", &roots));
        assert!(cwd_under_any_root("/wt/feat", &roots));
        assert!(cwd_under_any_root("/wt/feat/src", &roots));
        assert!(!cwd_under_any_root("/wt/feature", &roots));
        assert!(!cwd_under_any_root("", &roots));
    }

    async fn pre_upgrade(pass: Option<SkinPass>, workspace: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let workspace = workspace.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut params = HashMap::new();
            if !workspace.is_empty() {
                params.insert("workspace".to_string(), workspace);
            }
            serve_activity_events_connection(&mut stream, params, pass).await;
        });
        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .expect("timed out — handler may have upgraded")
            .expect("read");
        assert!(n > 0, "empty pre-upgrade response");
        String::from_utf8(buf[..n].to_vec()).expect("utf8 response")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn thread_files_caps_do_not_open_activity_socket() {
        let dir = temp_dir("cap");
        let handle = format!("act{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let id = insert_project(&dir.to_string_lossy(), &handle);
        for caps in [
            vec![CAP_THREAD_READ, CAP_THREAD_POST],
            vec![CAP_THREAD_READ, CAP_THREAD_POST, CAP_FILES_READ],
            vec![CAP_THREAD_READ, CAP_THREAD_POST, CAP_FILES_WRITE],
        ] {
            let raw = pre_upgrade(Some(session_pass(&id, &caps)), &handle).await;
            assert!(
                raw.starts_with("HTTP/1.1 403"),
                "caps {caps:?} must 403 before upgrade: {raw}"
            );
            assert!(
                raw.contains("missing capability activity:read"),
                "caps {caps:?}: {raw}"
            );
            assert!(!raw.contains("101"), "must not upgrade: {raw}");
            assert!(!raw.contains("skin_room"), "room is on the pass: {raw}");
        }
        let other = session_pass(&uuid::Uuid::new_v4().to_string(), &[CAP_ACTIVITY_READ]);
        let raw = pre_upgrade(Some(other), &handle).await;
        assert!(raw.starts_with("HTTP/1.1 403"), "{raw}");
        assert!(raw.contains("skin_room"), "{raw}");
        assert!(
            !raw.contains("missing capability"),
            "unknown room is skin_room, not a cap miss: {raw}"
        );
        assert!(!raw.contains("101"), "{raw}");
        let raw = pre_upgrade(Some(session_pass(&id, &[CAP_ACTIVITY_READ])), "").await;
        assert!(raw.starts_with("HTTP/1.1 400"), "{raw}");
        assert!(raw.contains("missing workspace"), "{raw}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn connect_room(
        pass: Option<SkinPass>,
        workspace: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let workspace = workspace.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut params = HashMap::new();
            params.insert("workspace".to_string(), workspace);
            serve_activity_events_connection(&mut stream, params, pass).await;
        });
        let (ws, resp) =
            tokio_tungstenite::connect_async(format!("ws://{addr}{ACTIVITY_EVENTS_WS_PATH}"))
                .await
                .expect("activity handshake");
        assert_eq!(resp.status(), 101, "upgrade status");
        ws
    }

    fn emit_activity(cwd: &str, agent: &str, pane: Option<&str>, status: &str) {
        session_events::emit(SessionEvent::SessionActivityChanged {
            workspace_path: cwd.to_string(),
            agent_name: agent.to_string(),
            pane_group_id: pane.map(|s| s.to_string()),
            status: status.to_string(),
        })
        .expect("activity subscriber is connected");
    }

    async fn take_text(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        wait: Duration,
    ) -> Option<String> {
        match tokio::time::timeout(wait, ws.next()).await {
            Err(_) => None,
            Ok(Some(Ok(Message::Text(t)))) => Some(t.to_string()),
            Ok(Some(Ok(Message::Ping(_)))) => match tokio::time::timeout(wait, ws.next()).await {
                Err(_) => None,
                Ok(Some(Ok(Message::Text(t)))) => Some(t.to_string()),
                Ok(other) => panic!("unexpected after ping: {other:?}"),
            },
            Ok(other) => panic!("unexpected frame: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn room_a_hears_cwd_under_a_and_drops_other_kinds_and_room_b() {
        let root_a = temp_dir("room-a");
        let sub_a = root_a.join("app");
        std::fs::create_dir_all(&sub_a).expect("subdir");
        let wt = temp_dir("wt-a");
        let sibling = PathBuf::from(format!("{}-other", root_a.display()));
        std::fs::create_dir_all(&sibling).expect("sibling");
        let root_b = temp_dir("room-b");
        let other = temp_dir("other-project");

        let handle_a = format!("aa{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let handle_b = format!("bb{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let id_a = insert_project(&root_a.to_string_lossy(), &handle_a);
        let id_b = insert_project(&root_b.to_string_lossy(), &handle_b);
        let _id_other = insert_project(
            &other.to_string_lossy(),
            &format!("cc{}", &uuid::Uuid::new_v4().to_string()[..8]),
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO workspaces (id, project_id, name, worktree_path) VALUES (?1, ?2, 'wt', ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), id_a, wt.to_string_lossy().to_string()],
            )
            .expect("insert worktree");
        }

        let pass_a = session_pass(&id_a, &[CAP_ACTIVITY_READ]);
        let pass_b = session_pass(&id_b, &[CAP_ACTIVITY_READ]);
        let mut sock_a = connect_room(Some(pass_a), &handle_a).await;
        let mut sock_b = connect_room(Some(pass_b), &handle_b).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        emit_activity(
            &sub_a.to_string_lossy(),
            "ada-sub",
            Some("pane-7"),
            "working",
        );
        session_events::emit(SessionEvent::FsChanged {
            workspace_path: root_a.to_string_lossy().to_string(),
            paths: vec![root_a.join("notes.md").to_string_lossy().to_string()],
        })
        .expect("fs emit");
        session_events::emit(SessionEvent::ActiveChanged {
            active_project_ids: vec![id_a.clone()],
            active_window_hours: 1,
        })
        .expect("active emit");
        session_events::emit(SessionEvent::AgentStatusChanged {
            pane_id: "pane-7".into(),
            tab_id: "tab-7".into(),
            status: "start".into(),
            workspace_path: Some(root_a.to_string_lossy().to_string()),
        })
        .expect("agent status emit");
        session_events::emit(SessionEvent::MailChanged {
            reason: "server-state-changed".into(),
        })
        .expect("mail emit");
        emit_activity(&root_b.to_string_lossy(), "bob-b", None, "working");
        emit_activity(&sibling.to_string_lossy(), "sib", None, "working");
        emit_activity(&root_a.to_string_lossy(), "ada-root", None, "working");
        emit_activity(&wt.to_string_lossy(), "ada-wt", Some("pane-wt"), "idle");
        emit_activity(&other.to_string_lossy(), "other-proj", None, "working");

        let mut agents_a = Vec::new();
        for _ in 0..3 {
            let text = take_text(&mut sock_a, Duration::from_secs(2))
                .await
                .expect("room A frame");
            let v: serde_json::Value = serde_json::from_str(&text).expect("json");
            assert!(v.get("workspacePath").is_none(), "no absolute cwd: {v}");
            assert_eq!(v["kind"], "session_activity_changed", "{v}");
            assert_eq!(v["workspace"], handle_a, "{v}");
            let mut keys: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
            keys.sort();
            assert_eq!(
                keys,
                vec![
                    "agentName".to_string(),
                    "kind".to_string(),
                    "paneGroupId".to_string(),
                    "status".to_string(),
                    "workspace".to_string(),
                ],
                "{v}"
            );
            agents_a.push(v["agentName"].as_str().expect("agent").to_string());
            match v["agentName"].as_str().expect("agent") {
                "ada-sub" => {
                    assert_eq!(v["paneGroupId"], "pane-7");
                    assert_eq!(v["status"], "working");
                }
                "ada-root" => {
                    assert!(v["paneGroupId"].is_null(), "{v}");
                    assert_eq!(v["status"], "working");
                }
                "ada-wt" => {
                    assert_eq!(v["status"], "idle");
                    assert_eq!(v["paneGroupId"], "pane-wt");
                }
                other_name => panic!("room A forwarded {other_name}: {v}"),
            }
        }
        agents_a.sort();
        assert_eq!(
            agents_a,
            vec![
                "ada-root".to_string(),
                "ada-sub".to_string(),
                "ada-wt".to_string()
            ]
        );
        assert!(
            take_text(&mut sock_a, Duration::from_millis(300))
                .await
                .is_none(),
            "room A must not see fs_changed, active_changed, mail, or room B"
        );

        let text_b = take_text(&mut sock_b, Duration::from_secs(2))
            .await
            .expect("room B frame");
        let vb: serde_json::Value = serde_json::from_str(&text_b).expect("json");
        assert_eq!(vb["kind"], "session_activity_changed");
        assert_eq!(vb["agentName"], "bob-b");
        assert_eq!(vb["workspace"], handle_b);
        assert_eq!(vb["status"], "working");
        assert!(vb.get("workspacePath").is_none(), "{vb}");
        assert!(
            take_text(&mut sock_b, Duration::from_millis(300))
                .await
                .is_none(),
            "room B must not see room A"
        );

        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&wt);
        let _ = std::fs::remove_dir_all(&sibling);
        let _ = std::fs::remove_dir_all(&root_b);
        let _ = std::fs::remove_dir_all(&other);
    }

    fn emit_roster(path: &str, project_id: &str) {
        session_events::emit(SessionEvent::HeartbeatRosterChanged {
            workspace_path: path.to_string(),
            project_id: project_id.to_string(),
        })
        .expect("heartbeat subscriber is connected");
    }

    /// T11 (prd-app-heartbeats-surface-v1 AH20/AH21/AH29/AH31).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn heartbeat_frames_follow_their_own_cap_and_room() {
        let root_a = temp_dir("hb-a");
        let root_b = temp_dir("hb-b");
        let handle_a = format!("ha{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let handle_b = format!("hb{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let id_a = insert_project(&root_a.to_string_lossy(), &handle_a);
        let id_b = insert_project(&root_b.to_string_lossy(), &handle_b);

        // Neither socket cap → 403 before upgrade, naming heartbeats:read.
        for caps in [
            vec![CAP_THREAD_READ, CAP_THREAD_POST],
            vec![CAP_FILES_READ, CAP_HEARTBEATS_WRITE],
        ] {
            let raw = pre_upgrade(Some(session_pass(&id_a, &caps)), &handle_a).await;
            assert!(raw.starts_with("HTTP/1.1 403"), "{caps:?}: {raw}");
            assert!(raw.contains("heartbeats:read"), "{caps:?}: {raw}");
            assert!(raw.contains("activity:read"), "{caps:?}: {raw}");
        }

        let mut hb_only = connect_room(Some(session_pass(&id_a, &[CAP_HEARTBEATS_READ])), &handle_a).await;
        let mut act_only = connect_room(Some(session_pass(&id_a, &[CAP_ACTIVITY_READ])), &handle_a).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Room B and agent activity never reach the heartbeats-only pass.
        emit_roster(&root_b.to_string_lossy(), &id_b);
        emit_activity(&root_a.to_string_lossy(), "ada", None, "working");
        // A burst of five roster changes for room A: one frame now, one
        // trailing frame after the 250 ms window (AH31).
        for _ in 0..5 {
            emit_roster(&root_a.to_string_lossy(), &id_a);
        }
        let first = take_text(&mut hb_only, Duration::from_secs(2)).await.expect("first roster frame");
        let v: serde_json::Value = serde_json::from_str(&first).expect("json");
        assert_eq!(v, serde_json::json!({"kind": "heartbeat_changed", "workspace": handle_a}));
        let trailing = take_text(&mut hb_only, Duration::from_secs(2)).await.expect("trailing frame");
        let v: serde_json::Value = serde_json::from_str(&trailing).expect("json");
        assert_eq!(v, serde_json::json!({"kind": "heartbeat_changed", "workspace": handle_a}));
        assert!(
            take_text(&mut hb_only, Duration::from_millis(500)).await.is_none(),
            "a burst coalesces to two frames; no activity, no room B"
        );

        // Empty projectId falls back to the room root path.
        emit_roster(&root_a.to_string_lossy(), "");
        let fallback = take_text(&mut hb_only, Duration::from_secs(2)).await.expect("path fallback frame");
        assert!(fallback.contains("heartbeat_changed"), "{fallback}");
        assert!(!fallback.contains(&*root_a.to_string_lossy()), "no path on the wire: {fallback}");

        // State frames carry name + live, matched on the project id.
        session_events::emit(SessionEvent::HeartbeatStateChanged {
            workspace_path: "/somewhere/else".into(),
            project: id_a.clone(),
            agent: "inbox-sweep".into(),
            live: true,
        })
        .expect("emit state");
        let state = take_text(&mut hb_only, Duration::from_secs(2)).await.expect("state frame");
        let v: serde_json::Value = serde_json::from_str(&state).expect("json");
        assert_eq!(
            v,
            serde_json::json!({"kind": "heartbeat_changed", "workspace": handle_a, "name": "inbox-sweep", "live": true})
        );

        // activity:read only: the agent frame, never a heartbeat frame.
        let act = take_text(&mut act_only, Duration::from_secs(2)).await.expect("activity frame");
        assert!(act.contains("session_activity_changed"), "{act}");
        assert!(
            take_text(&mut act_only, Duration::from_millis(500)).await.is_none(),
            "activity-only pass must not get heartbeat frames"
        );

        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    fn emit_ticket(project_id: &str, id: &str, change: &str, status: &str, via: Option<&str>) {
        session_events::emit(SessionEvent::TicketChanged {
            project_id: project_id.to_string(),
            id: id.to_string(),
            change: change.to_string(),
            status: status.to_string(),
            via: via.map(str::to_string),
            has_brief: true,
        })
        .expect("ticket subscriber is connected");
    }

    #[test]
    fn ticket_frame_matches_room_by_project_id_only() {
        let ev = SessionEvent::TicketChanged {
            project_id: "p1".into(),
            id: "t1".into(),
            change: "answered".into(),
            status: "answered".into(),
            via: Some("option_pick".into()),
            has_brief: false,
        };
        let frame = ticket_frame("p1", "sales", &ev).expect("same room");
        let v: serde_json::Value = serde_json::from_str(&frame).expect("json");
        assert_eq!(
            v,
            serde_json::json!({"kind": "ticket_changed", "workspace": "sales", "id": "t1",
                               "change": "answered", "status": "answered",
                               "via": "option_pick", "hasBrief": false})
        );
        assert!(ticket_frame("p2", "sales", &ev).is_none(), "other room");
        assert!(ticket_frame("", "sales", &ev).is_none(), "empty room id never matches");
        let other_kind = SessionEvent::FeedbackChanged { reason: "created".into() };
        assert!(ticket_frame("p1", "sales", &other_kind).is_none());
    }

    /// prd-app-tickets-websocket-v1 T1/T4: tickets:read opens the room
    /// socket; each frame goes only to its own room; a pass without
    /// tickets:read in the room never gets a ticket frame (fail closed).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ticket_frames_follow_tickets_read_and_room() {
        let root_a = temp_dir("tk-a");
        let root_b = temp_dir("tk-b");
        let handle_a = format!("ta{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let handle_b = format!("tb{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let id_a = insert_project(&root_a.to_string_lossy(), &handle_a);
        let id_b = insert_project(&root_b.to_string_lossy(), &handle_b);

        // tickets:post alone, or thread only → 403 before upgrade, naming tickets:read.
        for caps in [vec![CAP_TICKETS_POST], vec![CAP_THREAD_READ, CAP_THREAD_POST]] {
            let raw = pre_upgrade(Some(session_pass(&id_a, &caps)), &handle_a).await;
            assert!(raw.starts_with("HTTP/1.1 403"), "{caps:?}: {raw}");
            assert!(raw.contains("tickets:read"), "{caps:?}: {raw}");
            assert!(!raw.contains("101"), "{caps:?}: {raw}");
        }
        // tickets:read in room B does not open room A.
        let raw = pre_upgrade(Some(session_pass(&id_b, &[CAP_TICKETS_READ])), &handle_a).await;
        assert!(raw.starts_with("HTTP/1.1 403"), "{raw}");
        assert!(raw.contains("skin_room"), "{raw}");

        let mut tk_a = connect_room(Some(session_pass(&id_a, &[CAP_TICKETS_READ])), &handle_a).await;
        let mut tk_b = connect_room(Some(session_pass(&id_b, &[CAP_TICKETS_READ])), &handle_b).await;
        let mut act_a = connect_room(Some(session_pass(&id_a, &[CAP_ACTIVITY_READ])), &handle_a).await;
        let mut hb_a = connect_room(Some(session_pass(&id_a, &[CAP_HEARTBEATS_READ])), &handle_a).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        emit_ticket(&id_b, "ticket-b", "created", "waiting", None);
        emit_ticket(&uuid::Uuid::new_v4().to_string(), "ticket-x", "created", "waiting", None);
        emit_ticket(&id_a, "ticket-a", "commented", "needs_discussion", Some("free_text"));

        let a = take_text(&mut tk_a, Duration::from_secs(2)).await.expect("room A frame");
        let v: serde_json::Value = serde_json::from_str(&a).expect("json");
        assert_eq!(
            v,
            serde_json::json!({"kind": "ticket_changed", "workspace": handle_a, "id": "ticket-a",
                               "change": "commented", "status": "needs_discussion",
                               "via": "free_text", "hasBrief": true})
        );
        assert!(!a.contains(&*root_a.to_string_lossy()), "no path on the wire: {a}");
        assert!(!a.contains(&id_a), "the room is the handle, not the id: {a}");
        assert!(
            take_text(&mut tk_a, Duration::from_millis(400)).await.is_none(),
            "room A hears only its own ticket, once"
        );

        let b = take_text(&mut tk_b, Duration::from_secs(2)).await.expect("room B frame");
        let vb: serde_json::Value = serde_json::from_str(&b).expect("json");
        assert_eq!(vb["id"], "ticket-b", "{vb}");
        assert_eq!(vb["workspace"], handle_b.as_str(), "{vb}");
        assert_eq!(vb["via"], serde_json::Value::Null, "{vb}");
        assert!(
            take_text(&mut tk_b, Duration::from_millis(400)).await.is_none(),
            "room B never hears room A"
        );

        assert!(
            take_text(&mut act_a, Duration::from_millis(400)).await.is_none(),
            "activity:read alone gets no ticket frame"
        );
        assert!(
            take_text(&mut hb_a, Duration::from_millis(400)).await.is_none(),
            "heartbeats:read alone gets no ticket frame"
        );

        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }
}
