//! Phase 2.5b follow-up: Tauri command surface for the workspace
//! settings "Skills" panel. Each verb is a one-line forward to the
//! matching `k2_core::skills::crud::*` function — the renderer talks
//! to these instead of going through HTTP so the in-process daemon
//! doesn't have to round-trip for a synchronous list/read.
//!
//! The CLI side (`cli/k2so cmd_skills`) shells through the daemon's
//! existing `/cli/agents/*` HTTP routes; the parity test
//! `tests/cli/skills_tauri_list_parity.sh` confirms that both surfaces
//! agree on which skills exist for a given workspace.

use k2_core::skills::{self, crud::SkillSummary};

#[tauri::command]
pub fn k2so_skills_list(project_path: String) -> Result<Vec<SkillSummary>, String> {
    skills::crud::list(&project_path)
}

#[tauri::command]
pub fn k2so_skills_profile(project_path: String, name: String) -> Result<String, String> {
    skills::crud::profile(&project_path, &name)
}

#[tauri::command]
pub fn k2so_skills_create(
    project_path: String,
    name: String,
    from_skill: Option<String>,
) -> Result<SkillSummary, String> {
    skills::crud::create(&project_path, &name, from_skill.as_deref())
}

#[tauri::command]
pub fn k2so_skills_remove(project_path: String, name: String) -> Result<(), String> {
    skills::crud::remove(&project_path, &name)
}
