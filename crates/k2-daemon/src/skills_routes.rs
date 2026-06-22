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
//! - `POST /cli/onboarding/harness-fanout-enabled` (read)
//!       → `workspace::onboarding::harness_fanout_enabled`
//! - `POST /cli/canonical/detect-state` (read)
//!       → `workspace::canonical::detect_canonical_state`
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

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProjectPathBody {
    project_path: String,
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
    // Enabling fan-out must also clear the legacy `.skip-harness-management`
    // flag. That flag is the HARDER "never touch my files" override, and
    // `harness_fanout_enabled()` returns false whenever it's present — so
    // without this, checking the box writes the marker but the immediate
    // read-back still reports false and the checkbox snaps back unchecked.
    if b.enabled {
        if let Err(e) = k2_core::workspace::onboarding::unskip_harness_management(&b.project_path) {
            return CliResponse::bad_request(format!("clear skip-harness flag: {e}"));
        }
    }
    if let Err(e) =
        k2_core::workspace::onboarding::set_harness_fanout_enabled(&b.project_path, b.enabled)
    {
        return CliResponse::bad_request(e);
    }
    // GAP 1: enabling the fan-out must materialize the symlink/copy
    // mirrors IMMEDIATELY, not on the next daemon boot. The fan-out
    // funnels through `regenerate_workspace_skill`, which is gated by
    // `harness_fanout_enabled()` — now that the marker is set above, this
    // call lays down CLAUDE.md / GEMINI.md / .cursor (etc.) mirrors of the
    // canonical `.k2/AGENTS.md`. Daemon-first: the regen runs here, not in
    // any client. A regen failure is surfaced (the marker write already
    // succeeded, so the box stays checked and the next boot retries).
    if b.enabled {
        if let Err(e) =
            k2_core::workspace::skill_regen::regenerate_workspace_skill(b.project_path.clone())
        {
            return CliResponse::bad_request(format!("regen after enable: {e}"));
        }
    }
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// GET-equivalent read: `POST /cli/onboarding/harness-fanout-enabled` → `{ "enabled": bool }`.
/// Host-aware mirror of the `k2so_harness_fanout_enabled` Tauri command — wraps the SAME
/// `k2_core::workspace::onboarding::harness_fanout_enabled` so local + remote stay identical.
pub fn handle_harness_fanout_enabled(body: &[u8]) -> CliResponse {
    let b: ProjectPathBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    let enabled = k2_core::workspace::onboarding::harness_fanout_enabled(&b.project_path);
    CliResponse::ok_json(serde_json::json!({ "enabled": enabled }).to_string())
}

/// `POST /cli/canonical/detect-state` → the `Vec<HarnessProbe>` JSON array.
/// Host-aware mirror of the `k2so_detect_canonical_state` Tauri command.
pub fn handle_detect_canonical_state(body: &[u8]) -> CliResponse {
    let b: ProjectPathBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    let probes = k2_core::workspace::canonical::detect_canonical_state(&b.project_path);
    match serde_json::to_string(&probes) {
        Ok(j) => CliResponse::ok_json(j),
        Err(e) => CliResponse::bad_request(format!("serialize probes: {e}")),
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
        // opt-in skill, asserting both land on disk under .k2/skills/.
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
        assert!(tmp.join(".k2/skills/my-skill/SKILL.md").exists());

        let body =
            serde_json::json!({ "project_path": pp, "skill": "k2-agent" }).to_string();
        let r = handle_write_opt_in(body.as_bytes());
        assert_eq!(r.status, "200 OK", "write-opt-in body={}", r.body);
        assert!(tmp.join(".k2/skills/k2-agent/SKILL.md").exists());

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

        // GAP 1: enabling must ALSO regen immediately so the harness
        // mirrors materialize without waiting for the next boot. With the
        // fan-out marker set, `regenerate_workspace_skill` GENERATES the
        // canonical `.k2/AGENTS.md` AND symlinks the harness mirrors at it —
        // the root `AGENTS.md` (the cross-tool standard) proves the fan-out
        // ran, not just the canonical write. (New AGENTS.md-canonical shape;
        // the old composed `.k2/skills/k2so/SKILL.md` was reaped.)
        assert!(
            tmp.join(".k2/AGENTS.md").exists(),
            "canonical .k2/AGENTS.md should exist after enable+regen"
        );
        assert!(
            tmp.join("AGENTS.md").exists(),
            "root AGENTS.md mirror should materialize immediately on enable"
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

    #[test]
    fn harness_fanout_enabled_read_mirrors_the_write() {
        // The host-awareness fix: the READ route must report the SAME state
        // the WRITE route just persisted, so the remote checkbox stops
        // snapping back to unchecked.
        let tmp = std::env::temp_dir().join(format!(
            "k2so-fanout-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("mk tempdir");
        let pp = tmp.to_string_lossy().to_string();

        // Enable via the write route, then read back via the read route.
        let body = serde_json::json!({ "project_path": pp, "enabled": true }).to_string();
        let r = handle_set_harness_fanout_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "enable body={}", r.body);

        let body = serde_json::json!({ "project_path": pp }).to_string();
        let r = handle_harness_fanout_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "read body={}", r.body);
        assert!(
            r.body.contains("\"enabled\":true"),
            "read after enable should report enabled, body={}",
            r.body
        );

        // Disable via the write route, then read back false.
        let body = serde_json::json!({ "project_path": pp, "enabled": false }).to_string();
        let r = handle_set_harness_fanout_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "disable body={}", r.body);

        let body = serde_json::json!({ "project_path": pp }).to_string();
        let r = handle_harness_fanout_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "read body={}", r.body);
        assert!(
            r.body.contains("\"enabled\":false"),
            "read after disable should report disabled, body={}",
            r.body
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn harness_fanout_enabled_read_rejects_missing_project_path() {
        let r = handle_harness_fanout_enabled(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }

    #[test]
    fn detect_canonical_state_rejects_missing_project_path() {
        let r = handle_detect_canonical_state(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }
}
