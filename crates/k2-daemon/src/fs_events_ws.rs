//! `WS /cli/fs/events?workspace=` — skin files live nudge.
//!
//! Distinct from `/cli/sessions/events` (host-wide APP-LEVEL, including
//! `fs_changed` plus presence/mail) and from overlay/grid. Frames:
//! `{kind:"fs_changed", workspace, paths:[relative]}`.
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
                    Ok(SessionEvent::FsChanged { workspace_path, paths }) => {
                        if !same_root(&workspace_path, &root) {
                            continue;
                        }
                        let rel = map_changed_paths(&root, &paths);
                        if rel.is_empty() {
                            continue;
                        }
                        if write
                            .send(Message::Text(wire_json(&wire_workspace, &rel)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => continue,
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
}
