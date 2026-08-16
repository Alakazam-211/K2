//! Workspace agent identity resolution + path helpers.
//!
//! Phase 2.5c extraction. Pre-2.5c these helpers lived at the
//! `agents/mod.rs` root because every `agents/*` submodule needed
//! them. Post-Phase-2.1 the workspace owns "which agent answers for
//! this workspace?" semantically, so the helpers belong under
//! [`crate::workspace`] — the folder that holds workspace lifecycle +
//! state + launch helpers.
//!
//! `agents/mod.rs` re-exports each public function via `pub use` so
//! every existing `crate::agents::find_primary_agent` /
//! `crate::agents::agent_dir` etc. call site keeps resolving without
//! churn during the deprecation window.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Canonical authored persona filename (compose input). Capitals, same
/// convention as `PROJECT.md` / `AGENTS.md`.
pub const PERSONA_MD_NAME: &str = "ROLE.md";

/// Pre-heal authored persona filename. Dual-read only; never a new write dest.
/// Distinct from leftover cwd `./AGENT.md` (fan-out alias of the compose).
pub const LEGACY_PERSONA_MD_NAME: &str = "AGENT.md";

/// Resolve a project's primary-key id from its filesystem path. `None`
/// when the project hasn't been registered via `projects` yet.
pub fn resolve_project_id(conn: &rusqlite::Connection, path: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| r.get(0),
    )
    .ok()
}

/// Root of the legacy multi-agent tree for a given workspace:
/// `<project>/.k2so/agents/`. Post-0.37.0 unification this directory
/// is removed by the migration sweep and only sticks around for
/// fresh workspaces that haven't been onboarded yet (it gets created
/// briefly by older code paths that still scaffold it). Prefer
/// [`workspace_agent_path`] / [`agent_template_dir`] for new code.
pub fn agents_dir(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("agents")
}

/// Post-0.37.0: `<project>/.k2so/agent/` — the single workspace
/// agent's directory. Created by the unification migration; every
/// call site that historically did `agent_dir(project, primary_name)`
/// converges on this path after the migration runs.
pub fn workspace_agent_path(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("agent")
}

/// `<project>/.k2/agent/ROLE.md` — write path for the workspace agent's
/// authored persona. Always `ROLE.md`; never the pre-heal `AGENT.md`.
pub fn workspace_agent_md_path(project_path: &str) -> PathBuf {
    workspace_agent_path(project_path).join(PERSONA_MD_NAME)
}

/// Resolve the on-disk persona file in `dir`.
///
/// Prefer `ROLE.md` if present, else `AGENT.md`, else `ROLE.md` (new writes).
/// Does not newly accept lowercase-only `role.md` as a live name (APFS
/// `exists("ROLE.md")` is true for `role.md`; heal that case separately).
pub fn persona_md_in(dir: impl AsRef<Path>) -> PathBuf {
    let dir = dir.as_ref();
    let role = dir.join(PERSONA_MD_NAME);
    if role.exists() {
        return role;
    }
    let legacy = dir.join(LEGACY_PERSONA_MD_NAME);
    if legacy.exists() {
        return legacy;
    }
    role
}

/// True when `dir` holds an authored persona (`ROLE.md` or pre-heal `AGENT.md`).
pub fn persona_present_in(dir: impl AsRef<Path>) -> bool {
    let dir = dir.as_ref();
    dir.join(PERSONA_MD_NAME).exists() || dir.join(LEGACY_PERSONA_MD_NAME).exists()
}

/// After a successful write to `ROLE.md`, if sibling `AGENT.md` remains,
/// move it into `agent-backups/` (do not clobber `ROLE.md`).
pub fn backup_sibling_legacy_persona(dir: impl AsRef<Path>) {
    let dir = dir.as_ref();
    let legacy = dir.join(LEGACY_PERSONA_MD_NAME);
    if !legacy.exists() {
        return;
    }
    // ROLE.md must already be the live file; otherwise this is a heal/rename.
    if !dir.join(PERSONA_MD_NAME).exists() {
        return;
    }
    let backup_dir = dir.join("agent-backups");
    if fs::create_dir_all(&backup_dir).is_err() {
        return;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut dest = backup_dir.join(format!("agent-{ts}.md"));
    if dest.exists() {
        dest = backup_dir.join(format!("agent-{ts}-{}.md", std::process::id()));
    }
    if fs::rename(&legacy, &dest).is_err() {
        if fs::copy(&legacy, &dest).is_ok() {
            let _ = fs::remove_file(&legacy);
        }
    }
}

/// Post-0.37.0: `<project>/.k2so/agent-templates/<template_name>/` —
/// role personas for delegation/worktrees.
pub fn agent_template_dir(project_path: &str, template_name: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path)
        .join("agent-templates")
        .join(template_name)
}

/// Post-0.37.0: `<project>/.k2so/heartbeats/` — workspace-level
/// heartbeat directory. Each schedule lives at
/// `<this>/<schedule_name>/WAKEUP.md`. Pre-0.37.0 heartbeats lived
/// under each agent's directory (`agent_dir/heartbeats/<sched>/`).
/// The unification migration moves them to this workspace-level
/// path; new heartbeats scaffold here directly.
pub fn workspace_heartbeats_dir(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("heartbeats")
}

/// Post-Phase-2.5b: `<project>/.k2so/skills/` — the unified home for
/// every skill profile the workspace-agent owns. A workspace IS an
/// agent (hands, memory, self-built tools, proactivity via
/// heartbeats); skills are the documentation profiles that describe
/// how this workspace-agent operates. Replaces the pre-2.5b split
/// across `.k2so/agents/<name>/` and `.k2so/agent-templates/<role>/`
/// which carried over from the earlier sub-agent model. Migration
/// runs at daemon boot per upgraded workspace; see
/// [`crate::skills::consolidation::consolidate_skills_v1`].
pub fn skills_dir(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("skills")
}

/// Post-Phase-2.5b: `<project>/.k2so/skills/<skill_name>/` — a single
/// skill's directory. Each skill's body lives at `<this>/SKILL.md`.
pub fn skill_dir(project_path: &str, skill_name: &str) -> PathBuf {
    skills_dir(project_path).join(skill_name)
}

/// Resolve the on-disk directory for an agent name within a workspace.
///
/// In the workspace==agent model `agent_name` is mostly a routing
/// key for legacy call sites — the primary workspace-agent is keyed
/// on the workspace itself, so callers that pass a name still
/// resolve onto the unified path transparently.
///
/// **Layout-aware.** Probes in this order:
/// 1. `<project>/.k2/agent/` has `ROLE.md` **or** pre-heal `AGENT.md` —
///    post-0.37.0 primary workspace-agent. `agent_name` is ignored
///    here; the primary is keyed on workspace, not name. Historic
///    call sites (e.g. `agent_dir(project, "pod-leader")`) keep
///    working without changes during the deprecation window.
///
///    Probing for the persona file (not just the dir) matters because
///    the unification migration `mkdir -p`s `.k2/agent/` BEFORE it
///    moves any files. During that window the dir exists but has
///    no persona yet, and the migration's own primary-detection
///    walk needs `agent_type_for` to read from the LEGACY
///    `.k2so/agents/<name>/AGENT.md` to determine the primary.
///    Gating the probe on the populated state avoids a
///    chicken-and-egg failure during migration.
/// 2. `<project>/.k2so/skills/<agent_name>/` exists with either
///    SKILL.md or AGENT.md — post-Phase-2.5b unified home for skill
///    profiles. Probed AFTER the workspace primary so a skill named
///    the same as the workspace-agent doesn't accidentally shadow
///    the primary persona.
/// 3. `<project>/.k2so/agent-templates/<agent_name>/AGENT.md`
///    exists — legacy template path. Pre-Phase-2.5b workspaces that
///    haven't been first-boot-migrated yet still answer here; on
///    upgraded workspaces this dir was trashed by
///    `consolidate_skills_v1` and the probe returns false.
/// 4. Legacy `<project>/.k2so/agents/<agent_name>/` — pre-0.37.0
///    workspaces that haven't been migrated yet (rare; the daemon's
///    boot sweep migrates every registered workspace).
pub fn agent_dir(project_path: &str, agent_name: &str) -> PathBuf {
    let primary = workspace_agent_path(project_path);
    if persona_present_in(&primary) {
        return primary;
    }
    // Phase 2.5b: probe the consolidated `.k2so/skills/<name>/` home
    // before the legacy `.k2so/agent-templates/` / `.k2so/agents/`
    // paths. The probe checks for either SKILL.md (new shape) or
    // AGENT.md (transitional shape for workspaces caught mid-migration).
    let skill = skill_dir(project_path, agent_name);
    if skill.join("SKILL.md").exists() || skill.join("AGENT.md").exists() {
        return skill;
    }
    let template = agent_template_dir(project_path, agent_name);
    if template.join("AGENT.md").exists() {
        return template;
    }
    agents_dir(project_path).join(agent_name)
}

/// Extract YAML-ish `key: value` frontmatter from a markdown blob.
/// Tolerant: empty keys/values skipped, missing closing fence returns
/// an empty map. Used by [`agent_type_for`] and
/// [`crate::workspace::scheduler`] consumers to read agent.md
/// metadata without pulling a full YAML dep.
pub fn parse_frontmatter(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if !content.starts_with("---") {
        return map;
    }
    if let Some(end) = content[3..].find("---") {
        let frontmatter = &content[3..3 + end];
        for line in frontmatter.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if !key.is_empty() && !value.is_empty() {
                    map.insert(key, value);
                }
            }
        }
    }
    map
}

/// Determine an agent's type from its `agent.md` frontmatter. Returns
/// `"agent-template"` if no frontmatter or no `type:` field is found
/// (same default the scheduler uses elsewhere).
pub fn agent_type_for(project_path: &str, agent_name: &str) -> String {
    let md = persona_md_in(agent_dir(project_path, agent_name));
    if let Ok(content) = fs::read_to_string(&md) {
        let fm = parse_frontmatter(&content);
        if let Some(t) = fm.get("type") {
            return t.clone();
        }
    }
    "agent-template".to_string()
}

/// Endgame L2 (Stage A): `'k2'` and legacy `'k2so'` are the **same**
/// builtin agent type. Every reader comparison must go through here so
/// a later Stage-B value migration (`agent_type='k2'`) cannot strand a
/// reader that still only accepts the legacy spelling.
///
/// Writer flip is **not** this stage — stored values stay `"k2so"` until
/// Stage B; this only makes readers dual-tolerant.
pub fn is_builtin_agent_type(t: &str) -> bool {
    t == "k2" || t == "k2so"
}

/// True when `a` and `b` name the same agent type under Stage-A dual-read
/// (builtin `k2`/`k2so` are synonyms; everything else is exact).
pub fn agent_types_equal(a: &str, b: &str) -> bool {
    a == b || (is_builtin_agent_type(a) && is_builtin_agent_type(b))
}

/// Find the workspace's primary scheduleable agent.
///
/// A workspace is one-of Custom / K2SO Agent / Workspace Manager
/// (mutually exclusive by design), but agent-mode swaps can leave
/// orphan directories from prior modes on disk. This fn uses
/// `projects.agent_mode` as the source of truth and only returns an
/// agent dir whose type matches the workspace's declared mode.
/// Agent-templates are never scheduleable.
///
/// **0.37.0 unification.** Post-migration the workspace's single
/// agent lives at `.k2/agent/ROLE.md` (pre-heal: `AGENT.md`) and the
/// legacy `.k2so/agents/<name>/` tree is gone. We probe the unified
/// path first — if it has a persona file, parse the frontmatter
/// `name:` and return it. Pre-0.37.0 workspaces (or freshly-created
/// ones that haven't been migrated yet) fall through to the legacy
/// scan. Without this probe, every heartbeat fire on a migrated
/// workspace silently failed at "no scheduleable agent in this
/// workspace" because `agents_root.exists()` returned false.
pub fn find_primary_agent(project_path: &str) -> Option<String> {
    // Post-0.37.0 unified primary. ROLE.md wins when both exist.
    let unified_md = persona_md_in(workspace_agent_path(project_path));
    if let Ok(content) = fs::read_to_string(&unified_md) {
        let fm = parse_frontmatter(&content);
        if let Some(name) = fm.get("name") {
            if !name.is_empty() {
                return Some(name.clone());
            }
        }
    }

    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return None;
    }

    // Resolve the declared workspace mode from the DB. Prevents
    // alphabetical scan order from picking a stale orphan (e.g.
    // returning pod-leader before sarah when the workspace is actually
    // a Custom agent workspace for sarah).
    let declared_mode: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT agent_mode FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    };

    let type_for_mode = |mode: &str| match mode {
        "custom" => "custom",
        "manager" => "manager",
        // Stage A dual-read: CLI-canonical `k2`, stored legacy `k2so`,
        // and the UI historic `agent` synonym all mean the builtin type.
        "k2so" | "k2" | "agent" => "k2so",
        _ => "",
    };

    // Pass 1: prefer the agent whose type matches the declared mode.
    if let Some(ref mode) = declared_mode {
        let wanted = type_for_mode(mode);
        if !wanted.is_empty() {
            if let Ok(entries) = fs::read_dir(&agents_root) {
                for entry in entries.flatten() {
                    if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    // Dual-read: frontmatter may already say `type: k2`.
                    if agent_types_equal(&agent_type_for(project_path, &name), wanted) {
                        return Some(name);
                    }
                }
            }
            // 0.39.0f: dropped the pre-0.37.0 `__lead__` sentinel
            // fallback. Post-unification the workspace's primary
            // agent ALWAYS materializes as `.k2so/agent/AGENT.md`
            // (handled by the unified probe above). If we reach
            // here with `wanted == "manager"` it means the legacy
            // `.k2so/agents/<n>/` scan didn't find a manager-type
            // dir AND the unified file is missing — i.e., the
            // workspace is in manager mode but its primary agent
            // hasn't been scaffolded yet. Return None so callers
            // can handle the missing-primary case (e.g., audit a
            // `skipped_no_primary` decision) instead of routing to
            // a sentinel name no consumer would recognize.
        }
    }

    // Pass 2 (fallback, no declared mode): first scheduleable dir wins.
    let Ok(entries) = fs::read_dir(&agents_root) else {
        return None;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let agent_type = agent_type_for(project_path, &name);
        if matches!(agent_type.as_str(), "custom" | "manager") || is_builtin_agent_type(&agent_type)
        {
            return Some(name);
        }
    }
    None
}

/// Is this workspace's agent CONFIGURED according to the DB — independent
/// of whether the persona FILE `.k2/agent/AGENT.md` exists on disk?
///
/// Mirrors the #9 msg/talk fix's existence test
/// (`crates/k2-daemon/src/workspace_msg.rs`): a workspace HAS an agent when
/// either
///   (a) it has a saved canonical chat session
///       (`workspace_sessions.session_id` non-empty), or
///   (b) `projects.agent_enabled == 1`.
///
/// The persona file is frequently MISSING for configured workspaces post
/// AGENTS.md cutover (only the generated root-mirror `<ws>/AGENT.md`
/// survives), so it is NOT a reliable existence signal. This is the
/// authoritative DB-canonical check used by [`resolve_agent_name`].
pub fn workspace_has_configured_agent(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else {
        return false;
    };
    // (a) Saved canonical chat session.
    if let Ok(Some(ws)) = crate::db::schema::WorkspaceSession::get(&conn, &project_id) {
        if ws.session_id.as_deref().map_or(false, |s| !s.is_empty()) {
            return true;
        }
    }
    // (b) agent_enabled flag on the project row.
    if let Ok(p) = crate::db::schema::Project::get(&conn, &project_id) {
        if p.agent_enabled == 1 {
            return true;
        }
    }
    false
}

/// Resolve a workspace's primary-agent NAME for identity / addressing /
/// display — sourced from the DB-canonical truth, NOT solely the persona
/// file.
///
/// **Why this exists (issue #70).** [`find_primary_agent`] resolves the
/// name purely from the on-disk persona file (`.k2/agent/AGENT.md`'s
/// `name:` frontmatter, or the legacy `.k2/agents/<name>/` tree). That
/// file is frequently ABSENT for newly-created or DB-configured
/// workspaces, so every identity/addressing caller that gated on
/// `find_primary_agent` (heartbeat fire, inbox-wake routing, canonical
/// session ensurance, lock checks) silently returned `None` and treated a
/// perfectly-configured workspace as having no agent.
///
/// **Resolution order** (matches the #9 `resolve_spawn_name` fallback):
///   1. The persona file's declared `name:` via [`find_primary_agent`] —
///      preserves explicit naming and keeps the file-based path working
///      with zero regression for workspaces that DO have an AGENT.md.
///   2. If the DB says the workspace has a configured agent
///      ([`workspace_has_configured_agent`]) → the workspace folder
///      basename, the product-default display name ("agent display name
///      defaults to the workspace basename").
///   3. Otherwise `None` — genuinely no agent configured.
///
/// Persona-CONTENT callers (rendering AGENT.md body, launch-profile
/// parsing, writing into the persona file) must keep using
/// [`find_primary_agent`] / direct file reads — the name from this
/// resolver is an addressing label, not a guarantee the file exists.
pub fn resolve_agent_name(project_path: &str) -> Option<String> {
    // 1. Persona file declared name (no regression for file-backed workspaces).
    if let Some(name) = find_primary_agent(project_path) {
        if !name.trim().is_empty() {
            return Some(name);
        }
    }
    // 2. DB-canonical: a configured workspace falls back to its basename.
    if workspace_has_configured_agent(project_path) {
        if let Some(base) = Path::new(project_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.trim().is_empty())
        {
            return Some(base);
        }
    }
    // 3. No agent configured.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_reads_simple_kv() {
        let md = "---\nname: test\ntype: custom\n---\n# body\n";
        let fm = parse_frontmatter(md);
        assert_eq!(fm.get("name"), Some(&"test".to_string()));
        assert_eq!(fm.get("type"), Some(&"custom".to_string()));
    }

    #[test]
    fn parse_frontmatter_empty_when_no_fence() {
        let fm = parse_frontmatter("# heading only\n");
        assert!(fm.is_empty());
    }

    #[test]
    fn parse_frontmatter_skips_empty_keys_and_values() {
        // `: lonely` has empty key; `key:` has empty value. Both skipped.
        let fm = parse_frontmatter("---\n: lonely\nkey:\nrole: eng\n---\n");
        assert_eq!(fm.len(), 1);
        assert_eq!(fm.get("role"), Some(&"eng".to_string()));
    }

    #[test]
    fn is_builtin_agent_type_accepts_k2_and_legacy_k2so() {
        assert!(is_builtin_agent_type("k2"));
        assert!(is_builtin_agent_type("k2so"));
        assert!(!is_builtin_agent_type("custom"));
        assert!(!is_builtin_agent_type("manager"));
        assert!(!is_builtin_agent_type("agent"));
        assert!(!is_builtin_agent_type(""));
    }

    #[test]
    fn agent_types_equal_treats_k2_and_k2so_as_synonyms() {
        assert!(agent_types_equal("k2", "k2so"));
        assert!(agent_types_equal("k2so", "k2"));
        assert!(agent_types_equal("custom", "custom"));
        assert!(!agent_types_equal("k2", "custom"));
        assert!(!agent_types_equal("manager", "k2so"));
    }

    // ── #70: DB-canonical name resolution ───────────────────────────
    //
    // These tests share the process-wide in-memory test DB (first
    // `db::shared()` under cfg(test)). Each inserts its own unique
    // project row (random UUID + unique path) so siblings don't collide.

    fn unique_ws_path(label: &str) -> String {
        let base = std::env::temp_dir().join(format!(
            "k2-id70-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        base.to_string_lossy().into_owned()
    }

    fn insert_project(path: &str, agent_enabled: i64) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path, agent_enabled) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, "id70-test", path, agent_enabled],
        )
        .expect("insert project row");
        id
    }

    fn save_canonical_session(project_id: &str, session_id: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::db::schema::WorkspaceSession::upsert(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            project_id,
            None,
            Some(session_id),
            "claude",
            "system",
            "stopped",
        )
        .expect("upsert workspace_sessions");
    }

    /// THE BUG REPRO: a configured workspace with a saved canonical
    /// session but NO `.k2/agent/AGENT.md` file must still resolve its
    /// name (to the workspace folder basename), where the file-only
    /// `find_primary_agent` returns None.
    #[test]
    fn resolve_agent_name_saved_session_no_agent_md() {
        let path = unique_ws_path("session");
        let project_id = insert_project(&path, 0);
        save_canonical_session(&project_id, "claude-sess-abc123");

        // No AGENT.md on disk — the file-based resolver finds nothing.
        assert_eq!(find_primary_agent(&path), None);

        // DB-canonical resolver recovers the basename.
        let expected_basename = Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resolve_agent_name(&path), Some(expected_basename));
        assert!(workspace_has_configured_agent(&path));
    }

    /// agent_enabled=1 (no saved session, no file) also resolves.
    #[test]
    fn resolve_agent_name_agent_enabled_no_agent_md() {
        let path = unique_ws_path("enabled");
        let _project_id = insert_project(&path, 1);

        assert_eq!(find_primary_agent(&path), None);
        let expected_basename = Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resolve_agent_name(&path), Some(expected_basename));
    }

    /// NO regression: a workspace WITH a valid AGENT.md resolves to its
    /// declared `name:` (the file path wins over the basename fallback).
    #[test]
    fn resolve_agent_name_prefers_agent_md_name() {
        let path = unique_ws_path("withfile");
        let _project_id = insert_project(&path, 1);

        let agent_dir = workspace_agent_path(&path);
        fs::create_dir_all(&agent_dir).expect("mkdir .k2/agent");
        fs::write(
            agent_dir.join("AGENT.md"),
            "---\nname: scout\ntype: custom\n---\n# persona\n",
        )
        .expect("write AGENT.md");

        assert_eq!(find_primary_agent(&path), Some("scout".to_string()));
        // Declared name wins over the basename fallback.
        assert_eq!(resolve_agent_name(&path), Some("scout".to_string()));

        let _ = fs::remove_dir_all(crate::workspace_dot_dir(&path));
    }

    /// An UNCONFIGURED workspace (registered but no agent, no session,
    /// no file) still resolves to None — the resolver does not invent an
    /// agent for every project row.
    #[test]
    fn resolve_agent_name_none_when_unconfigured() {
        let path = unique_ws_path("unconfigured");
        let _project_id = insert_project(&path, 0);

        assert_eq!(find_primary_agent(&path), None);
        assert!(!workspace_has_configured_agent(&path));
        assert_eq!(resolve_agent_name(&path), None);
    }

    #[test]
    fn agents_dir_and_agent_dir_are_consistent() {
        // A fresh (non-existent) root resolves via workspace_dot_dir to the
        // 0.40.x default `.k2/`. Use a unique temp path so the result never
        // depends on a stray `/tmp/proj/.k2so` left by another run.
        let base = std::env::temp_dir().join(format!("k2-agdir-{}", std::process::id()));
        let root_str = base.to_string_lossy().into_owned();
        let root = agents_dir(&root_str);
        assert_eq!(root, base.join(".k2").join("agents"));
        let agent = agent_dir(&root_str, "foo");
        assert_eq!(agent, root.join("foo"));
    }

    fn unique_persona_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-persona-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("mkdir persona dir");
        dir
    }

    #[test]
    fn persona_md_in_reads_legacy_agent_md() {
        let dir = unique_persona_dir("old-only");
        fs::write(dir.join("AGENT.md"), "---\nname: scout\n---\nold\n").unwrap();
        let got = persona_md_in(&dir);
        assert_eq!(got, dir.join("AGENT.md"));
        assert!(persona_present_in(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn persona_md_in_prefers_role_md() {
        let dir = unique_persona_dir("new-only");
        fs::write(dir.join("ROLE.md"), "---\nname: scout\n---\nnew\n").unwrap();
        assert_eq!(persona_md_in(&dir), dir.join("ROLE.md"));
        assert!(persona_present_in(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn persona_md_in_role_wins_when_both_exist() {
        let dir = unique_persona_dir("both");
        fs::write(dir.join("AGENT.md"), "legacy\n").unwrap();
        fs::write(dir.join("ROLE.md"), "role\n").unwrap();
        assert_eq!(persona_md_in(&dir), dir.join("ROLE.md"));
        assert_eq!(fs::read_to_string(persona_md_in(&dir)).unwrap(), "role\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn persona_md_in_defaults_to_role_md_when_absent() {
        let dir = unique_persona_dir("absent");
        assert!(!persona_present_in(&dir));
        assert_eq!(persona_md_in(&dir), dir.join("ROLE.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_agent_md_path_is_always_role_md() {
        let path = unique_ws_path("write-dest");
        assert_eq!(
            workspace_agent_md_path(&path),
            workspace_agent_path(&path).join("ROLE.md")
        );
    }

    #[test]
    fn backup_sibling_legacy_persona_moves_agent_md() {
        let dir = unique_persona_dir("backup");
        fs::write(dir.join("ROLE.md"), "role\n").unwrap();
        fs::write(dir.join("AGENT.md"), "legacy\n").unwrap();
        backup_sibling_legacy_persona(&dir);
        assert!(dir.join("ROLE.md").exists());
        assert!(!dir.join("AGENT.md").exists());
        let backups: Vec<_> = fs::read_dir(dir.join("agent-backups"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "legacy\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_primary_agent_reads_old_new_and_both() {
        let path = unique_ws_path("primary");
        let dir = workspace_agent_path(&path);
        fs::create_dir_all(&dir).unwrap();

        fs::write(
            dir.join("AGENT.md"),
            "---\nname: oldname\ntype: custom\n---\nold\n",
        )
        .unwrap();
        assert_eq!(find_primary_agent(&path), Some("oldname".to_string()));

        fs::write(
            dir.join("ROLE.md"),
            "---\nname: newname\ntype: custom\n---\nnew\n",
        )
        .unwrap();
        assert_eq!(
            find_primary_agent(&path),
            Some("newname".to_string()),
            "ROLE.md wins when both exist"
        );

        fs::remove_file(dir.join("AGENT.md")).unwrap();
        assert_eq!(find_primary_agent(&path), Some("newname".to_string()));

        let _ = fs::remove_dir_all(crate::workspace_dot_dir(&path));
    }

    #[test]
    fn agent_dir_primary_probe_accepts_role_or_agent_md() {
        let path = unique_ws_path("probe");
        let dir = workspace_agent_path(&path);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ROLE.md"), "---\nname: r\n---\n").unwrap();
        assert_eq!(agent_dir(&path, "ignored"), dir);
        fs::remove_file(dir.join("ROLE.md")).unwrap();
        fs::write(dir.join("AGENT.md"), "---\nname: a\n---\n").unwrap();
        assert_eq!(agent_dir(&path, "ignored"), dir);
        let _ = fs::remove_dir_all(crate::workspace_dot_dir(&path));
    }
}
