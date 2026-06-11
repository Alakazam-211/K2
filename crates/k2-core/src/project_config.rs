use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teardown_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_editor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_group_name: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn read_json_file(file_path: &Path) -> Option<serde_json::Value> {
    if !file_path.exists() {
        return None;
    }
    let raw = fs::read_to_string(file_path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn merge_value(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    if let (Some(base_map), Some(overlay_map)) = (base.as_object_mut(), overlay.as_object()) {
        for (key, val) in overlay_map {
            if val.is_object() && base_map.get(key).map_or(false, |v| v.is_object()) {
                let mut base_val = base_map.get(key).unwrap().clone();
                merge_value(&mut base_val, val);
                base_map.insert(key.clone(), base_val);
            } else {
                base_map.insert(key.clone(), val.clone());
            }
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────────

/// Read and merge project configuration with three-tier resolution:
/// local overlay -> project config -> defaults
pub fn get_project_config(project_path: &str) -> ProjectConfig {
    let config_dir = crate::workspace_dot_dir(project_path);
    let project_config_path = config_dir.join("config.json");
    let local_config_path = config_dir.join("config.local.json");

    let project_conf = read_json_file(&project_config_path);
    let local_conf = read_json_file(&local_config_path);

    // Start with default (empty) JSON object
    let mut merged = serde_json::to_value(ProjectConfig::default()).unwrap_or_default();

    // Layer on project config
    if let Some(pc) = &project_conf {
        merge_value(&mut merged, pc);
    }

    // Layer on local config (highest priority)
    if let Some(lc) = &local_conf {
        merge_value(&mut merged, lc);
    }

    serde_json::from_value(merged).unwrap_or_default()
}

/// Quick check if a project has a run command configured.
pub fn has_run_command(project_path: &str) -> bool {
    let config = get_project_config(project_path);
    config.run_command.is_some() && !config.run_command.as_ref().unwrap().is_empty()
}

/// Set a single key-value pair in the project's `.k2so/config.json`.
/// Uses atomic writes (temp file + rename) for safety.
pub fn set_project_config_value(
    project_path: &str,
    key: &str,
    value: Option<&str>,
) -> Result<(), String> {
    let config_dir = crate::workspace_dot_dir(project_path);
    let config_path = config_dir.join("config.json");

    // Ensure .k2so/ directory exists
    fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;

    // Read existing config (or start with empty object)
    let mut existing: serde_json::Map<String, serde_json::Value> = if config_path.exists() {
        let raw = fs::read_to_string(&config_path).unwrap_or_default();
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    // Set or remove the key
    match value {
        Some(v) => {
            existing.insert(
                key.to_string(),
                serde_json::Value::String(v.to_string()),
            );
        }
        None => {
            existing.remove(key);
        }
    }

    // Atomic write: write to temp file, then rename
    let tmp_path = config_dir.join(format!("config.{}.tmp", uuid::Uuid::new_v4()));
    let json_str = serde_json::to_string_pretty(&existing)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    fs::write(&tmp_path, format!("{}\n", json_str))
        .map_err(|e| format!("Failed to write temp config: {}", e))?;
    fs::rename(&tmp_path, &config_path)
        .map_err(|e| format!("Failed to rename config file: {}", e))?;

    Ok(())
}
