//! `WS /cli/activity/events?workspace=` — agent working for one room.
//!
//! Subscribes to the session-event broadcast and forwards only
//! `SessionActivityChanged`. Every other kind is dropped. The event's
//! `workspacePath` is `session.cwd`; a frame is sent when that cwd is the
//! room root, a worktree root from `tree_roots`, or a directory under one
//! of those, on a path boundary. The guest frame has no absolute path.
//!
//! Skin: pass `Some(SkinPass)`. Room, then `activity:read`, both before
//! `accept_async`. Owner/Connect: `None` (still requires `workspace=`).

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
                if !pass.has_cap_in_room(&ws.project_id, crate::skin_routes::ACTIVITY_READ) {
                    let r =
                        crate::skin_routes::missing_cap_response(crate::skin_routes::ACTIVITY_READ);
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

    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/activity_events_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = session_events::subscribe();

    loop {
        tokio::select! {
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
        RoomPolicy, SkinPass, CAP_ACTIVITY_READ, CAP_FILES_READ, CAP_FILES_WRITE, CAP_THREAD_POST,
        CAP_THREAD_READ,
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
}
