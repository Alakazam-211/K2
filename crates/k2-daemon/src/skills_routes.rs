//! Daemon-owned `/cli/skills/*` route handlers — workspace skill CRUD +
//! the canonical-agent opt-in/harness-fanout writes.
//!
//! ## Why these exist (K2 Connect host-awareness GAP)
//!
//! The renderer previously called the matching `k2so_skills_*` /
//! `k2so_*_harness_*` Tauri commands via LOCAL `invoke()`. Those run
//! in-process against the LOCAL daemon's filesystem, so when the
//! renderer is driving a REMOTE host (K2 Connect) the write lands on
//! the wrong machine — or fails outright because there's no Tauri
//! backend on the remote. These routes give the renderer a host-aware
//! HTTP surface that always targets the daemon it's actually talking
//! to.
//!
//! Each handler wraps the SAME `k2_core` fn the Tauri command called,
//! so the local and remote paths stay byte-for-byte identical.
//!
//! ## Routes (all POST, JSON body, method-gated in the dispatcher)
//!
//! - `POST /cli/skills/create`  → `skills::crud::create`
//! - `POST /cli/skills/remove`  → `skills::crud::remove`
//! - `POST /cli/skills/write-opt-in` → `skills::content::write_opt_in_skill`
//! - `POST /cli/onboarding/set-harness-fanout-enabled`
//!       → `workspace::onboarding::set_harness_fanout_enabled`
//!
//! All are workspace-scoped (a `project_path` in the body), NOT
//! owner-only — they're the same writes any logged-in user performs
//! from the workspace Settings panel, so they take the same auth as
//! every other `/cli/*` data route (owner token OR a connect-user
//! session via `token_ok`). The dispatcher provides the POST method
//! gate + token gate before this module sees the call.

use serde::Deserialize;

use k2_core::skills;
use k2_core::skills::content::OptInSkill;

use crate::cli_response::CliResponse;

// ──────────────────────────────────────────────────────────────────────
// Body shapes
// ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct CreateBody {
    /// Absolute workspace path the skill is created under.
    project_path: String,
    /// New skill name (`.k2so/skills/<name>/`). Alphanumeric + `-`/`_`.
    name: String,
    /// Optional seed skill to copy frontmatter/body from.
    from_skill: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RemoveBody {
    project_path: String,
    name: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct WriteOptInBody {
    project_path: String,
    /// One of `workspace-manager` | `k2-agent` | `k2-canonical-agent`.
    skill: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SetHarnessFanoutBody {
    project_path: String,
    enabled: bool,
}

/// Deserialize a JSON body, returning a `400` `CliResponse` on parse
/// failure. Empty bodies fall back to `Default` so a missing required
/// field surfaces as the handler's own "missing X" error rather than a
/// serde error.
fn parse<T: serde::de::DeserializeOwned + Default>(body: &[u8]) -> Result<T, CliResponse> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| CliResponse::bad_request(format!("invalid body: {e}")))
}

// ──────────────────────────────────────────────────────────────────────
// Handlers
// ──────────────────────────────────────────────────────────────────────

/// Handler for `POST /cli/skills/create`.
///
/// Wraps `k2_core::skills::crud::create`. Returns the created
/// [`skills::crud::SkillSummary`] as JSON. Mirrors the
/// `k2so_skills_create` Tauri command.
pub fn handle_create(body: &[u8]) -> CliResponse {
    let b: CreateBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    if b.name.is_empty() {
        return CliResponse::bad_request("missing name");
    }
    match skills::crud::create(&b.project_path, &b.name, b.from_skill.as_deref()) {
        Ok(summary) => CliResponse::ok_json(
            serde_json::to_string(&summary).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/skills/remove`.
///
/// Wraps `k2_core::skills::crud::remove`, which TRASHES the skill dir
/// (recoverable via the OS recycle bin) — never a hard `remove_dir_all`.
/// Mirrors the `k2so_skills_remove` Tauri command.
pub fn handle_remove(body: &[u8]) -> CliResponse {
    let b: RemoveBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    if b.name.is_empty() {
        return CliResponse::bad_request("missing name");
    }
    match skills::crud::remove(&b.project_path, &b.name) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/skills/write-opt-in`.
///
/// Wraps `k2_core::skills::content::write_opt_in_skill`. Writes one of
/// the three canonical opt-in skills to `.k2so/skills/<name>/SKILL.md`
/// and returns the absolute path written. Mirrors the
/// `k2so_write_opt_in_skill` Tauri command (including its
/// unknown-skill-name error).
pub fn handle_write_opt_in(body: &[u8]) -> CliResponse {
    let b: WriteOptInBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    let opt_in = match b.skill.as_str() {
        "workspace-manager" => OptInSkill::WorkspaceManager,
        "k2-agent" => OptInSkill::K2Agent,
        "k2-canonical-agent" => OptInSkill::K2CanonicalAgent,
        other => return CliResponse::bad_request(format!("unknown opt-in skill: {other}")),
    };
    let path = skills::content::write_opt_in_skill(&b.project_path, opt_in);
    CliResponse::ok_json(
        serde_json::json!({ "success": true, "path": path.to_string_lossy() }).to_string(),
    )
}

/// Handler for `POST /cli/onboarding/set-harness-fanout-enabled`.
///
/// Wraps `k2_core::workspace::onboarding::set_harness_fanout_enabled`,
/// which writes/removes the `.k2so/.harness-fanout-enabled` marker.
/// Mirrors the `k2so_set_harness_fanout_enabled` Tauri command.
pub fn handle_set_harness_fanout_enabled(body: &[u8]) -> CliResponse {
    let b: SetHarnessFanoutBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    match k2_core::workspace::onboarding::set_harness_fanout_enabled(&b.project_path, b.enabled) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_rejects_missing_project_path() {
        let r = handle_create(br#"{"name":"foo"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }

    #[test]
    fn create_rejects_missing_name() {
        let r = handle_create(br#"{"project_path":"/tmp/x"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("name"), "body={}", r.body);
    }

    #[test]
    fn create_rejects_garbage_body() {
        let r = handle_create(b"not json");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("invalid body"), "body={}", r.body);
    }

    #[test]
    fn remove_rejects_missing_name() {
        let r = handle_remove(br#"{"project_path":"/tmp/x"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("name"), "body={}", r.body);
    }

    #[test]
    fn write_opt_in_rejects_unknown_skill() {
        let r = handle_write_opt_in(br#"{"project_path":"/tmp/x","skill":"bogus"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("unknown opt-in skill"), "body={}", r.body);
    }

    #[test]
    fn write_opt_in_rejects_missing_project_path() {
        let r = handle_write_opt_in(br#"{"skill":"k2-agent"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }

    #[test]
    fn create_and_write_opt_in_round_trip_on_tempdir() {
        // Real filesystem round-trip: create a skill, then write an
        // opt-in skill, asserting both land on disk under .k2so/skills/.
        let tmp = std::env::temp_dir().join(format!(
            "k2so-skills-routes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("mk tempdir");
        let pp = tmp.to_string_lossy().to_string();

        let body = serde_json::json!({ "project_path": pp, "name": "my-skill" }).to_string();
        let r = handle_create(body.as_bytes());
        assert_eq!(r.status, "200 OK", "create body={}", r.body);
        assert!(tmp.join(".k2so/skills/my-skill/SKILL.md").exists());

        let body =
            serde_json::json!({ "project_path": pp, "skill": "k2-agent" }).to_string();
        let r = handle_write_opt_in(body.as_bytes());
        assert_eq!(r.status, "200 OK", "write-opt-in body={}", r.body);
        assert!(tmp.join(".k2so/skills/k2-agent/SKILL.md").exists());

        // NOTE: we deliberately do NOT exercise handle_remove here — it
        // trashes via the OS recycle bin, which triggers a macOS Finder
        // Touch ID prompt under `cargo test`
        // (see feedback_recycle_bin_tests). Its arg validation is covered
        // above; the trash path is shared core code tested elsewhere.

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn set_harness_fanout_enabled_writes_marker() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-fanout-routes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("mk tempdir");
        let pp = tmp.to_string_lossy().to_string();

        let body = serde_json::json!({ "project_path": pp, "enabled": true }).to_string();
        let r = handle_set_harness_fanout_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "enable body={}", r.body);
        assert!(
            k2_core::workspace::onboarding::harness_fanout_enabled(&pp),
            "marker should report enabled after the write"
        );

        let body = serde_json::json!({ "project_path": pp, "enabled": false }).to_string();
        let r = handle_set_harness_fanout_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "disable body={}", r.body);
        assert!(
            !k2_core::workspace::onboarding::harness_fanout_enabled(&pp),
            "marker should report disabled after the second write"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
