//! Tauri command for the workspace inbox badge (`k2_core::inbox`).
//!
//! 0.46.4: this app no longer links k2-core. The one inbox command the
//! renderer invokes (`k2so_inbox_count`, the badge) forwards to
//! `GET /cli/inbox/count`, which runs the same `inbox::list_root(..).len()`.
//! Every other inbox verb already went through the daemon's `/cli/inbox/*`
//! routes; their unused Tauri twins were removed.

/// Number of items at the inbox root (not subfolders).
#[tauri::command]
pub fn k2so_inbox_count(project_path: String) -> Result<serde_json::Value, String> {
    crate::daemon_client::DaemonClient::try_connect()?
        .mirror_get("/cli/inbox/count", &[("project", &project_path)])
}
