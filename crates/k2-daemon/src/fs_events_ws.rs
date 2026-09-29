//! `WS /cli/fs/events?workspace=` — skin files live nudge.
//!
//! Distinct from `/cli/sessions/events` (host-wide APP-LEVEL, including
//! `fs_changed` plus presence/mail) and from overlay/grid. Frames:
//! `{kind:"fs_changed", workspace, paths:[relative]}` and, for this
//! room's project id only,
//! `{kind:"workspace_resources_changed", workspace, workspaceId}`.
//!
//! Skin: pass `Some(SkinPass)` so rooms are checked **before**
//! `accept_async`. Owner/Connect: `None` (still requires `workspace=`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::skin::SkinPass;

use crate::fs_routes::{resolve_owner_workspace, resolve_skin_workspace, SkinWorkspace};
use crate::session_events::{self, SessionEvent};

/// Wire path. Tests assert this is not `session_events` / overlay / grid.
pub const FS_EVENTS_WS_PATH: &str = "/cli/fs/events";

fn wire_json(workspace: &str, paths: &[String]) -> String {
    serde_json::json!({
        "kind": "fs_changed",
        "workspace": workspace,
        "paths": paths,
    })
    .to_string()
}

fn resources_changed_json(workspace: &str, workspace_id: &str) -> String {
    serde_json::json!({
        "kind": "workspace_resources_changed",
        "workspace": workspace,
        "workspaceId": workspace_id,
    })
    .to_string()
}

/// One guest frame, or nothing. Resources filter by project id. `fs_changed`
/// stays on the single room root (`same_root`).
pub fn subscriber_frame(
    project_id: &str,
    root: &str,
    wire_workspace: &str,
    event: &SessionEvent,
) -> Option<String> {
    match event {
        SessionEvent::FsChanged {
            workspace_path,
            paths,
        } => {
            if !same_root(workspace_path, root) {
                return None;
            }
            let rel = map_changed_paths(root, paths);
            if rel.is_empty() {
                return None;
            }
            Some(wire_json(wire_workspace, &rel))
        }
        SessionEvent::WorkspaceResourcesChanged { workspace_id } => {
            if workspace_id != project_id {
                return None;
            }
            Some(resources_changed_json(wire_workspace, workspace_id))
        }
        _ => None,
    }
}

fn same_root(a: &str, b: &str) -> bool {
    let na = a.replace('\\', "/");
    let nb = b.replace('\\', "/");
    let na = na.trim_end_matches('/');
    let nb = nb.trim_end_matches('/');
    if na == nb {
        return true;
    }
    match (Path::new(a).canonicalize(), Path::new(b).canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// Map an absolute host path to a workspace-relative path. Drop escapes
/// and other roots.
pub fn relative_to_root(root: &str, path: &str) -> Option<String> {
    let try_strip = |r: &str, p: &str| -> Option<String> {
        let r = r.replace('\\', "/");
        let r = r.trim_end_matches('/');
        let p = p.replace('\\', "/");
        if p == r {
            return Some(".".to_string());
        }
        let prefix = format!("{r}/");
        p.strip_prefix(&prefix).map(|s| s.to_string())
    };
    if let Some(s) = try_strip(root, path) {
        if s.contains("..") {
            return None;
        }
        return Some(s);
    }
    let rc = Path::new(root).canonicalize().ok()?;
    let pc = Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path));
    let s = try_strip(&rc.to_string_lossy(), &pc.to_string_lossy())?;
    if s.contains("..") {
        None
    } else {
        Some(s)
    }
}

fn map_changed_paths(root: &str, abs_paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in abs_paths {
        if p.is_empty() {
            continue;
        }
        if let Some(rel) = relative_to_root(root, p) {
            if !out.iter().any(|x| x == &rel) {
                out.push(rel);
            }
        }
    }
    out
}

async fn write_http(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::io::AsyncWriteExt::write_all(stream, resp.as_bytes()).await;
}

/// WS handler. Dispatcher already token-authed. Requires `workspace=`.
/// Skin: rooms + cap already checked in the dispatcher for `files:read`;
/// this still 403s `skin_room` before upgrade if the named workspace is
/// not on the pass.
pub async fn serve_fs_events_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
    skin_pass: Option<SkinPass>,
) {
    let workspace_q = params
        .get("workspace")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(workspace_q) = workspace_q else {
        log_debug!("[daemon/fs_events_ws] missing workspace=");
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
                if !pass.has_cap_in_room(&ws.project_id, crate::skin_routes::FILES_READ) {
                    let r = crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_READ);
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
    let root = resolved.path.clone();
    let project_id = resolved.project_id.clone();

    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/fs_events_ws] handshake failed: {e}");
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
                        let Some(frame) = subscriber_frame(&project_id, &root, &wire_workspace, &event) else {
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
    use futures_util::StreamExt;
    use std::collections::HashMap;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn fs_events_ws_path_is_not_host_wide_or_pty() {
        assert_eq!(FS_EVENTS_WS_PATH, "/cli/fs/events");
        assert_ne!(FS_EVENTS_WS_PATH, "/cli/sessions/events");
        assert_ne!(FS_EVENTS_WS_PATH, "/cli/overlay/events");
        assert_ne!(FS_EVENTS_WS_PATH, "/cli/sessions/grid");
        assert_ne!(FS_EVENTS_WS_PATH, "/events");
        assert!(!FS_EVENTS_WS_PATH.contains("grid"));
        assert!(!FS_EVENTS_WS_PATH.ends_with("/cli/fs/*"));
    }

    #[test]
    fn relative_to_root_strips_prefix_drops_other_trees() {
        assert_eq!(
            relative_to_root("/tmp/sales", "/tmp/sales/README.md").as_deref(),
            Some("README.md")
        );
        assert_eq!(
            relative_to_root("/tmp/sales", "/tmp/sales").as_deref(),
            Some(".")
        );
        assert_eq!(
            relative_to_root("/tmp/sales", "/tmp/sales/src/app.ts").as_deref(),
            Some("src/app.ts")
        );
        assert_eq!(relative_to_root("/tmp/sales", "/tmp/julie/x.md"), None);
        assert_eq!(relative_to_root("/tmp/sales", "/etc/passwd"), None);
    }

    #[test]
    fn map_changed_paths_drops_other_roots() {
        let mapped = map_changed_paths(
            "/tmp/sales",
            &[
                "/tmp/sales/a.md".into(),
                "/tmp/julie/b.md".into(),
                "/tmp/sales/src/x.ts".into(),
            ],
        );
        assert_eq!(mapped, vec!["a.md".to_string(), "src/x.ts".to_string()]);
    }
    #[test]
    fn resources_frame_is_project_id_and_has_no_file_list() {
        let ev = crate::session_events::SessionEvent::WorkspaceResourcesChanged {
            workspace_id: "proj-a".into(),
        };
        let frame = subscriber_frame("proj-a", "/tmp/sales", "sales", &ev).expect("room frame");
        let v: serde_json::Value = serde_json::from_str(&frame).expect("json");
        let mut keys: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "kind".to_string(),
                "workspace".to_string(),
                "workspaceId".to_string(),
            ]
        );
        assert_eq!(v["kind"], "workspace_resources_changed");
        assert_eq!(v["workspace"], "sales");
        assert_eq!(v["workspaceId"], "proj-a");
        assert!(v.get("paths").is_none(), "{v}");
        assert!(v.get("filePath").is_none(), "{v}");
        assert!(v.get("docs").is_none(), "{v}");
        assert!(subscriber_frame("proj-b", "/tmp/other", "other", &ev).is_none());
        let active = crate::session_events::SessionEvent::ActiveChanged {
            active_project_ids: vec!["proj-a".into()],
            active_window_hours: 1,
        };
        assert!(subscriber_frame("proj-a", "/tmp/sales", "sales", &active).is_none());
    }

    fn session_pass(project_id: &str, caps: &[&str]) -> k2_core::skin::SkinPass {
        let mut room_policy = k2_core::skin::RoomPolicy::new();
        room_policy.insert(
            project_id.to_string(),
            caps.iter().map(|c| (*c).to_string()).collect(),
        );
        k2_core::skin::SkinPass {
            id: uuid::Uuid::new_v4().to_string(),
            principal_id: Some(uuid::Uuid::new_v4().to_string()),
            username: "guest".to_string(),
            caps: caps.iter().map(|c| (*c).to_string()).collect(),
            rooms: vec![project_id.to_string()],
            session: true,
            room_policy,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resources_changed_reaches_only_the_room_that_added() {
        use std::time::Duration;
        use tokio::net::TcpListener;

        let dir_a = std::env::temp_dir().join(format!(
            "k2-fs-ev-a-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let dir_b = std::env::temp_dir().join(format!(
            "k2-fs-ev-b-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir_a).expect("mkdir a");
        std::fs::create_dir_all(&dir_b).expect("mkdir b");
        std::fs::write(dir_a.join("a.csv"), b"a").expect("file a");
        std::fs::write(dir_b.join("b.csv"), b"b").expect("file b");
        let root_a = dir_a.canonicalize().expect("canon a");
        let root_b = dir_b.canonicalize().expect("canon b");
        let handle_a = format!("fa{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let handle_b = format!("fb{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let id_a = {
            let id = uuid::Uuid::new_v4().to_string();
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, handle_a, root_a.to_string_lossy().to_string(), handle_a],
            )
            .expect("insert a");
            id
        };
        let id_b = {
            let id = uuid::Uuid::new_v4().to_string();
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, handle_b, root_b.to_string_lossy().to_string(), handle_b],
            )
            .expect("insert b");
            id
        };
        let caps = [
            k2_core::skin::CAP_FILES_READ,
            k2_core::skin::CAP_FILES_WRITE,
        ];
        let pass_a = session_pass(&id_a, &caps);
        let pass_b = session_pass(&id_b, &caps);

        async fn open(
            pass: k2_core::skin::SkinPass,
            workspace: String,
        ) -> tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        > {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut params = HashMap::new();
                params.insert("workspace".to_string(), workspace);
                serve_fs_events_connection(&mut stream, params, Some(pass)).await;
            });
            let (ws, resp) =
                tokio_tungstenite::connect_async(format!("ws://{addr}{FS_EVENTS_WS_PATH}"))
                    .await
                    .expect("fs events handshake");
            assert_eq!(resp.status(), 101, "upgrade");
            ws
        }

        let mut sock_a = open(pass_a.clone(), handle_a.clone()).await;
        let mut sock_b = open(pass_b.clone(), handle_b.clone()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        let added = crate::workspace_resources_routes::handle_add_gated(
            &HashMap::from([
                ("workspace".to_string(), handle_a.clone()),
                ("path".to_string(), "a.csv".to_string()),
            ]),
            Some(pass_a),
        );
        assert_eq!(added.status, "200 OK", "{}", added.body);

        let text = tokio::time::timeout(Duration::from_secs(2), sock_a.next())
            .await
            .expect("room A timed out")
            .expect("room A open")
            .expect("room A message");
        let text = match text {
            Message::Text(t) => t.to_string(),
            other => panic!("expected text, got {other:?}"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v["kind"], "workspace_resources_changed", "{v}");
        assert_eq!(v["workspace"], handle_a, "{v}");
        assert_eq!(v["workspaceId"], id_a, "{v}");
        assert!(v.get("paths").is_none(), "{v}");
        assert!(v.get("filePath").is_none(), "{v}");
        assert!(v.get("docs").is_none(), "{v}");
        let mut keys: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "kind".to_string(),
                "workspace".to_string(),
                "workspaceId".to_string(),
            ]
        );

        match tokio::time::timeout(Duration::from_millis(300), sock_b.next()).await {
            Err(_) => {}
            Ok(other) => panic!("room B must not see room A's resources frame: {other:?}"),
        }

        let added_b = crate::workspace_resources_routes::handle_add_gated(
            &HashMap::from([
                ("workspace".to_string(), handle_b.clone()),
                ("path".to_string(), "b.csv".to_string()),
            ]),
            Some(pass_b),
        );
        assert_eq!(added_b.status, "200 OK", "{}", added_b.body);
        let text_b = tokio::time::timeout(Duration::from_secs(2), sock_b.next())
            .await
            .expect("room B timed out")
            .expect("room B open")
            .expect("room B message");
        let text_b = match text_b {
            Message::Text(t) => t.to_string(),
            other => panic!("expected text, got {other:?}"),
        };
        let vb: serde_json::Value = serde_json::from_str(&text_b).expect("json");
        assert_eq!(vb["workspaceId"], id_b, "{vb}");
        assert_eq!(vb["workspace"], handle_b, "{vb}");
        assert_eq!(vb["kind"], "workspace_resources_changed");

        match tokio::time::timeout(Duration::from_millis(300), sock_a.next()).await {
            Err(_) => {}
            Ok(other) => panic!("room A must not see room B's resources frame: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }
}
