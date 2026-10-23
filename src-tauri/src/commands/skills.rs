//! Workspace Skills panel — list (`k2_core::skills::crud::list`).
//!
//! 0.46.4: this app no longer links k2-core. `k2so_skills_list` forwards to
//! `POST /cli/skills/list`, which runs the same fn; create / remove already
//! go through `/cli/skills/{create,remove}` from the renderer, so their
//! unused Tauri twins (and `k2so_skills_profile`) were removed.

#[tauri::command]
pub async fn k2so_skills_list(project_path: String) -> Result<serde_json::Value, String> {
    // Off the main thread: a slow daemon must not freeze the UI.
    crate::daemon_client::off_main(move || {
        crate::daemon_client::DaemonClient::try_connect()?
            .mirror_post("/cli/skills/list", &serde_json::json!({ "project_path": project_path }))
    })
    .await
}
