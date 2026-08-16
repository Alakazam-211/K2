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
//! - `POST /cli/onboarding/set-agents-md-generate-enabled`
//!       → `workspace::onboarding::set_agents_md_generate_enabled`
//! - `POST /cli/onboarding/agents-md-generate-enabled` (read)
//!       → `workspace::onboarding::agents_md_generate_enabled`
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

/// Handler for `POST /cli/onboarding/set-agents-md-generate-enabled`.
///
/// On: write the generate marker + compose + plant. If plant skips
/// (user-authored cwd `AGENTS.md`), the marker stays on and the
/// response includes `skipped`. Off: remove the marker; do **not**
/// delete cwd `AGENTS.md`. If leftover fan-out is on, retarget links.
pub fn handle_set_agents_md_generate_enabled(body: &[u8]) -> CliResponse {
    let b: SetHarnessFanoutBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    if let Err(e) =
        k2_core::workspace::onboarding::set_agents_md_generate_enabled(&b.project_path, b.enabled)
    {
        return CliResponse::bad_request(e);
    }
    if b.enabled {
        if let Err(e) =
            k2_core::workspace::skill_regen::regenerate_workspace_skill(b.project_path.clone())
        {
            return CliResponse::bad_request(format!("regen after enable: {e}"));
        }
        // Regen already plants. Inspect only — do not stamp cwd twice.
        let plant = k2_core::workspace::onboarding::inspect_root_agents_md_plant(&b.project_path);
        if let k2_core::workspace::onboarding::PlantResult::Skipped { reason } = plant {
            return CliResponse::ok_json(
                serde_json::json!({ "success": true, "skipped": reason }).to_string(),
            );
        }
    } else {
        k2_core::workspace::skill_regen::apply_leftover_harness_fanout(&b.project_path);
    }
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// GET-equivalent read: `POST /cli/onboarding/agents-md-generate-enabled` → `{ "enabled": bool }`.
pub fn handle_agents_md_generate_enabled(body: &[u8]) -> CliResponse {
    let b: ProjectPathBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.project_path.is_empty() {
        return CliResponse::bad_request("missing project_path");
    }
    let enabled = k2_core::workspace::onboarding::agents_md_generate_enabled(&b.project_path);
    CliResponse::ok_json(serde_json::json!({ "enabled": enabled }).to_string())
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

        // Fan-out enable plants leftover names, not cwd AGENTS.md
        // (generate is a separate marker, off here).
        assert!(
            tmp.join(".k2/AGENTS.md").exists(),
            "canonical .k2/AGENTS.md should exist after enable+regen"
        );
        assert!(
            !tmp.join("AGENTS.md").exists(),
            "fan-out must not plant cwd AGENTS.md (generate owns that path)"
        );
        assert!(
            tmp.join("CLAUDE.md").exists(),
            "leftover CLAUDE.md must materialize immediately on fan-out enable"
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

    #[test]
    fn set_agents_md_generate_on_plants_and_off_leaves_file() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-generate-routes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("mk tempdir");
        let pp = tmp.to_string_lossy().to_string();

        let body = serde_json::json!({ "project_path": pp, "enabled": true }).to_string();
        let r = handle_set_agents_md_generate_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "enable body={}", r.body);
        assert!(
            k2_core::workspace::onboarding::agents_md_generate_enabled(&pp),
            "marker must report enabled after the write"
        );
        assert!(
            tmp.join(".k2/AGENTS.md").is_file(),
            "canonical .k2/AGENTS.md must exist after set-on"
        );
        let root = tmp.join("AGENTS.md");
        let meta = std::fs::symlink_metadata(&root).expect("cwd AGENTS.md planted");
        assert!(
            meta.file_type().is_file() && !meta.file_type().is_symlink(),
            "set-on plants a real cwd file"
        );
        let planted = std::fs::read_to_string(&root).expect("read planted");
        assert!(
            planted.contains("<!-- GENERATED by K2"),
            "planted file must carry the compose banner"
        );
        let marker = "<!-- GENERATED by K2 at ";
        let i = planted.find(marker).expect("banner");
        let rest = &planted[i + marker.len()..];
        let stamp = rest[..rest.find(' ').expect("stamp")].to_string();
        let r = handle_set_agents_md_generate_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "re-enable body={}", r.body);
        let again = std::fs::read_to_string(&root).expect("read after second set-on");
        let i = again.find(marker).expect("banner after re-enable");
        let rest = &again[i + marker.len()..];
        assert_eq!(
            &rest[..rest.find(' ').expect("stamp after")],
            stamp.as_str(),
            "set-generate-on must plant once; a second enable must not restamp"
        );
        assert!(
            !tmp.join("CLAUDE.md").exists(),
            "set-generate must not plant leftover names"
        );

        let body = serde_json::json!({ "project_path": pp, "enabled": false }).to_string();
        let r = handle_set_agents_md_generate_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "disable body={}", r.body);
        assert!(
            !k2_core::workspace::onboarding::agents_md_generate_enabled(&pp),
            "marker must report disabled after set-off"
        );
        assert!(
            root.is_file(),
            "set-off must leave cwd AGENTS.md in place"
        );
        let after_off = std::fs::read_to_string(&root).expect("file remains");
        assert_eq!(after_off, planted, "set-off must not rewrite or delete the file");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn set_agents_md_generate_on_skips_user_file() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-generate-skip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("mk tempdir");
        let pp = tmp.to_string_lossy().to_string();
        std::fs::write(tmp.join("AGENTS.md"), "# human notes\nkeep me\n").unwrap();

        let body = serde_json::json!({ "project_path": pp, "enabled": true }).to_string();
        let r = handle_set_agents_md_generate_enabled(body.as_bytes());
        assert_eq!(r.status, "200 OK", "enable body={}", r.body);
        assert!(
            r.body.contains("skipped"),
            "user file must return skipped reason, body={}",
            r.body
        );
        assert!(
            k2_core::workspace::onboarding::agents_md_generate_enabled(&pp),
            "toggle stays on even when plant skips"
        );
        let kept = std::fs::read_to_string(tmp.join("AGENTS.md")).expect("user file");
        assert_eq!(kept, "# human notes\nkeep me\n");
        assert!(
            !tmp.join(".k2/migration").exists(),
            "generate must not archive the user file"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn agents_md_generate_enabled_read_rejects_missing_project_path() {
        let r = handle_agents_md_generate_enabled(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("project_path"), "body={}", r.body);
    }
}
