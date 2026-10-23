//! Workspace Skills panel — list (`k2_core::skills::crud::list`).
//!
//! 0.46.4: this app no longer links k2-core. `k2so_skills_list` forwards to
//! `POST /cli/skills/list`, which runs the same fn; create / remove already
//! go through `/cli/skills/{create,remove}` from the renderer, so their
//! unused Tauri twins (and `k2so_skills_profile`) were removed.

#[tauri::command]
pub fn k2so_skills_list(project_path: String) -> Result<serde_json::Value, String> {
    crate::daemon_client::DaemonClient::try_connect()?
        .mirror_post("/cli/skills/list", &serde_json::json!({ "project_path": project_path }))
}
