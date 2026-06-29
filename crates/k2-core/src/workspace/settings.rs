//! Per-project settings accessors — thin DB wrappers.
//!
//! The CLI's `k2so mode`, `k2so heartbeat on/off`, `k2so worktree`,
//! `k2so settings` commands all land here. Each is a read-or-write
//! against the `projects` table filtered by path. Kept separate from
//! the broader `AppSettings` (in `src-tauri/src/commands/settings.rs`)
//! because that struct is mostly UI preferences; these are per-project
//! mode flags that affect agent behavior.
//!
//! Moved to core so the daemon can serve `/cli/mode`, `/cli/worktree`,
//! `/cli/settings` headlessly.

/// Update a single project setting. Field names are allowlisted —
/// the SQL interpolates the column name directly so any arbitrary
/// string from query params would be an injection vector without
/// this check.
pub fn update_project_setting(
    project_path: &str,
    field: &str,
    value: &str,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();

    let allowed = [
        "agent_mode",
        "worktree_mode",
        "heartbeat_enabled",
        "agent_enabled",
        "pinned",
        "tier_id",
        // 0.34.0 Session Stream opt-in (Phase 2). Values: 'on' | 'off'.
        "use_session_stream",
        // #67 — per-workspace remote-instruct opt-in. Values: '1' | '0'
        // (default 0/OFF, fail-closed). Gates the composer's connect-user
        // path for THIS workspace; see `remote_instruct_allowed_for_path`.
        "allow_remote_instruct",
    ];
    if !allowed.contains(&field) {
        return Err(format!("Unknown setting: {}", field));
    }
    // Validate the per-workspace remote-instruct flag so a typo can't
    // silently leave a security gate in an undefined state. Stored as a
    // 0/1 int column (migration 0054).
    if field == "allow_remote_instruct" && value != "0" && value != "1" {
        return Err(format!(
            "allow_remote_instruct must be '0' or '1', got {value:?}"
        ));
    }
    // Validate value for the new enum-like setting so a typo doesn't
    // silently leave a project in a broken half-state. Existing fields
    // keep their bare string/int semantics for back-compat.
    if field == "use_session_stream" && value != "on" && value != "off" {
        return Err(format!(
            "use_session_stream must be 'on' or 'off', got {value:?}"
        ));
    }

    let sql = format!("UPDATE projects SET {} = ?1 WHERE path = ?2", field);
    let rows = conn
        .execute(&sql, rusqlite::params![value, project_path])
        .map_err(|e| format!("DB update failed: {}", e))?;

    if rows == 0 {
        return Err(format!("Project not found in DB: {}", project_path));
    }

    // Keep agent_enabled in sync with agent_mode — the UI derives one
    // from the other and the CLI expects them coherent.
    if field == "agent_mode" {
        let enabled = if value == "off" { "0" } else { "1" };
        let _ = conn.execute(
            "UPDATE projects SET agent_enabled = ?1 WHERE path = ?2",
            rusqlite::params![enabled, project_path],
        );
    }

    // Drop the DB lock before returning. Turning a workspace into an
    // agent does NOTHING to its harness fan-out marker — there is no
    // auto-apply, ever. Fan-out is enabled ONLY by the explicit
    // per-workspace "Canonical Agent" checkbox (with its confirmation
    // modal), which writes the marker via the
    // `POST /cli/onboarding/set-harness-fanout-enabled` route. The former
    // global "default for new agents" flag has been removed entirely
    // because auto-applying symlink fan-out to a workspace that already
    // has data could overwrite the user's harness files.
    drop(conn);

    Ok(())
}

/// Read every exposed per-project setting as a JSON blob. Shape
/// matches what the React frontend expects from
/// `invoke('projects_get_settings', ...)`.
pub fn get_project_settings(project_path: &str) -> Result<serde_json::Value, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    conn.query_row(
        // `heartbeat_enabled` computed live as a true aggregate (any enabled,
        // non-archived heartbeat) — see `Project::list` in db/schema.rs. The
        // stored `projects.heartbeat_enabled` column is legacy and drifts.
        "SELECT agent_mode, worktree_mode, \
                (EXISTS(SELECT 1 FROM workspace_heartbeats wh WHERE wh.project_id = projects.id AND wh.enabled = 1 AND wh.archived_at IS NULL)) AS heartbeat_enabled, \
                agent_enabled, \
                pinned, name, tier_id, use_session_stream, allow_remote_instruct \
         FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| {
            // `use_session_stream` landed in migration 0032 with
            // default 'off'; expose as a bool for React consumers
            // (matching every other toggle shape in this struct).
            let uss_raw = row
                .get::<_, Option<String>>(7)
                .unwrap_or(None)
                .unwrap_or_else(|| "off".to_string());
            Ok(serde_json::json!({
                "mode": row.get::<_, String>(0).unwrap_or_else(|_| "off".to_string()),
                "worktreeMode": row.get::<_, i64>(1).unwrap_or(0) == 1,
                "heartbeatEnabled": row.get::<_, i64>(2).unwrap_or(0) == 1,
                "agentEnabled": row.get::<_, i64>(3).unwrap_or(0) == 1,
                "pinned": row.get::<_, i64>(4).unwrap_or(0) == 1,
                "name": row.get::<_, String>(5).unwrap_or_default(),
                "stateId": row.get::<_, Option<String>>(6).unwrap_or(None),
                "useSessionStream": uss_raw == "on",
                // #67 — per-workspace remote-instruct opt-in (default 0/OFF).
                "allowRemoteInstruct": row.get::<_, i64>(8).unwrap_or(0) == 1,
            }))
        },
    )
    .map_err(|e| format!("Project not found: {}", e))
}

/// Read the global agentic-systems toggle from
/// `~/.k2so/settings.json` via [`crate::app_settings`]. Defaults to
/// `false` when the field is missing — the AppSettings serde default
/// for `agentic_systems_enabled`.
///
/// **0.39.0 migration:** these accessors used to read the SQLite
/// `app_settings (key, value)` table created by migration 0050. That
/// table is now dead-but-inert (kept in the migration ladder for
/// rollback safety only); the canonical store is the JSON file, which
/// shares the same writer lock (`SETTINGS_LOCK`) as every other
/// daemon-side setting and so closes the F3 two-writers race for this
/// toggle too.
pub fn get_agentic_enabled() -> bool {
    crate::app_settings::load().agentic_systems_enabled
}

/// Set the global agentic-systems toggle in `~/.k2so/settings.json`
/// via [`crate::app_settings::update`]. The update is atomic across
/// concurrent callers (the `SETTINGS_LOCK` mutex serializes the
/// load+merge+save critical section).
pub fn set_agentic_enabled(enabled: bool) -> Result<(), String> {
    crate::app_settings::update(serde_json::json!({
        "agenticSystemsEnabled": enabled,
    }))
    .map(|_| ())
}

/// Read the "keep daemon running when K2SO quits" preference from
/// `~/.k2so/settings.json`. Defaults to `true` — matches the
/// persistent-agents flagship: if the user installed K2SO and opted
/// into heartbeats, they presumably want them to keep firing when the
/// window closes. The menubar icon provides visibility into what's
/// running, so defaulting ON doesn't leave the user wondering.
///
/// **0.39.0 migration:** see [`get_agentic_enabled`] — same story.
/// Migration 0050's `app_settings (key, value)` table is no longer
/// the source of truth; `AppSettings::keep_daemon_on_quit` is.
pub fn get_keep_daemon_on_quit() -> bool {
    crate::app_settings::load().keep_daemon_on_quit
}

/// Set the "keep daemon running when K2SO quits" preference in
/// `~/.k2so/settings.json`. Atomic via [`crate::app_settings::update`].
pub fn set_keep_daemon_on_quit(keep: bool) -> Result<(), String> {
    crate::app_settings::update(serde_json::json!({
        "keepDaemonOnQuit": keep,
    }))
    .map(|_| ())
}

/// Return `true` if the given project has opted into the 0.34.0
/// Session Stream pipeline (Phase 2). Defaults to `false` when the
/// project doesn't exist or the column reads NULL (rows inserted
/// before migration 0032 applied — the ALTER default backfills to
/// 'off', so NULL here means "unknown project").
///
/// Callers pair this with the compile-time `session_stream` feature
/// flag: both must be true for the dual-emit reader to kick in.
pub fn get_use_session_stream(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT use_session_stream FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, Option<String>>(0),
    )
    .map(|v| v.as_deref() == Some("on"))
    .unwrap_or(false)
}

/// #67 — read the PER-WORKSPACE remote-instruct opt-in for `project_path`.
/// Returns `false` (fail-closed) when the project isn't registered or the
/// column reads NULL. Does NOT consider the app-level master flag — use
/// [`remote_instruct_allowed_for_path`] for the effective gate decision.
pub fn get_allow_remote_instruct(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT allow_remote_instruct FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// #67 — the EFFECTIVE remote-instruct gate decision for `project_path`.
///
/// A workspace is opted in iff its per-workspace flag is set OR the
/// app-level `allowRemoteInstruct` master is on. The app-level flag is
/// kept as a GLOBAL MASTER for back-compat: deployments that enabled the
/// old app-level (0.40.12) flag keep working — every workspace stays
/// opted in — while the default (both OFF) denies, fail-closed.
///
/// This gates ONLY the connect-user path; the owner token is always
/// allowed by [`crate::routes`]'s `authorize_send_message` regardless.
/// Fail-closed: an unknown/unregistered workspace + app-level off → false.
pub fn remote_instruct_allowed_for_path(project_path: &str) -> bool {
    // App-level master switch opts in ALL workspaces (back-compat).
    if crate::app_settings::load().allow_remote_instruct {
        return true;
    }
    // Otherwise consult the per-workspace flag.
    get_allow_remote_instruct(project_path)
}

// ── B3a (sandbox) — per-workspace Anthropic API key (BYO key) ──────────
//
// A PER-WORKSPACE configured API key staged as `ANTHROPIC_API_KEY=<key>`
// into a microVM-backed sandbox cell's guest env at spawn (see the daemon's
// `session_token::scoped_cell_env_for_token` + `v2_spawn`). K2 is NOT in the
// Anthropic API path — the in-cell Claude Code reads the env key + calls
// Anthropic directly, skipping interactive auth.
//
// SECURITY: the value is a tenant's OWN key, scoped to their OWN workspace
// (right key → right cell only); the microVM jail isolates cells. The
// column is PLAINTEXT at rest in k2so.db (root-only box DB) — at-rest
// encryption is a follow-up. NEVER log/echo this value.

/// Read the PER-WORKSPACE Anthropic API key for `project_path`.
///
/// Returns `None` when the project isn't registered, the column reads NULL
/// (the default — no key configured), or the stored value is blank. A
/// blank/whitespace value is treated as absent so the spawn door never
/// stages an empty `ANTHROPIC_API_KEY=` (which would mask the on-device
/// fallback and confuse the in-cell agent). Never logs the key.
pub fn get_workspace_api_key(project_path: &str) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let raw: Option<String> = conn
        .query_row(
            "SELECT anthropic_api_key FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    match raw {
        Some(k) if !k.trim().is_empty() => Some(k.trim().to_string()),
        _ => None,
    }
}

/// Set (or clear) the PER-WORKSPACE Anthropic API key for `project_id`
/// (`projects.id`). Passing an empty/whitespace `key` CLEARS it (stores
/// NULL) so a workspace can drop back to no-credential. Fails LOUDLY when
/// the project id is unknown. Never logs the key.
pub fn set_workspace_api_key(project_id: &str, key: &str) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    // Empty → NULL (clear); otherwise store the trimmed key.
    let trimmed = key.trim();
    let stored: Option<&str> = if trimmed.is_empty() { None } else { Some(trimmed) };
    let rows = conn
        .execute(
            "UPDATE projects SET anthropic_api_key = ?1 WHERE id = ?2",
            rusqlite::params![stored, project_id],
        )
        // Deliberately do NOT include the key in any error string.
        .map_err(|e| format!("DB update failed: {}", e))?;
    if rows == 0 {
        return Err(format!("Project not found in DB: id={}", project_id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.1 coverage for the workspace-settings DB wrappers.
    //!
    //! These tests use the shared in-memory test DB (initialized on
    //! first call to `db::shared()` under `cfg(test)`), so each test
    //! inserts its own unique project row (random UUID + unique path)
    //! to avoid collisions with sibling tests sharing the same handle.
    use super::*;
    use uuid::Uuid;

    fn insert_project(path: &str) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, "settings-test", path],
        )
        .expect("insert project row");
        id
    }

    fn unique_path(label: &str) -> String {
        format!(
            "/tmp/k2so-settings-test-{}-{}-{}",
            label,
            std::process::id(),
            Uuid::new_v4(),
        )
    }

    #[test]
    fn update_project_setting_rejects_unknown_field() {
        let path = unique_path("unknown-field");
        let _pid = insert_project(&path);

        let err = update_project_setting(&path, "not_a_real_field", "x")
            .expect_err("unknown field must be rejected");
        assert!(
            err.contains("Unknown setting"),
            "error should describe unknown setting, got {err:?}",
        );
    }

    #[test]
    fn update_project_setting_roundtrips_agent_mode_and_syncs_agent_enabled() {
        let path = unique_path("agent-mode-sync");
        let _pid = insert_project(&path);

        // Setting agent_mode to "off" should also flip agent_enabled to 0.
        update_project_setting(&path, "agent_mode", "off").expect("set agent_mode off");
        let settings = get_project_settings(&path).expect("read settings");
        assert_eq!(settings["mode"], "off");
        assert_eq!(settings["agentEnabled"], false);

        // Setting agent_mode to any non-"off" value should flip agent_enabled to 1.
        update_project_setting(&path, "agent_mode", "manager").expect("set agent_mode manager");
        let settings = get_project_settings(&path).expect("read settings");
        assert_eq!(settings["mode"], "manager");
        assert_eq!(settings["agentEnabled"], true);
    }

    #[test]
    fn update_project_setting_validates_use_session_stream_enum() {
        let path = unique_path("uss-enum");
        let _pid = insert_project(&path);

        let err = update_project_setting(&path, "use_session_stream", "bogus")
            .expect_err("invalid enum value must be rejected");
        assert!(
            err.contains("use_session_stream"),
            "error should reference the field name, got {err:?}",
        );

        // Valid values pass and the read converts to bool.
        update_project_setting(&path, "use_session_stream", "on").expect("set on");
        let settings = get_project_settings(&path).expect("read");
        assert_eq!(settings["useSessionStream"], true);
        assert!(get_use_session_stream(&path), "convenience accessor agrees");

        update_project_setting(&path, "use_session_stream", "off").expect("set off");
        let settings = get_project_settings(&path).expect("read");
        assert_eq!(settings["useSessionStream"], false);
        assert!(!get_use_session_stream(&path));
    }

    #[test]
    fn update_project_setting_fails_loudly_on_missing_project() {
        // No insert — the path doesn't exist in `projects`.
        let path = unique_path("missing");
        let err = update_project_setting(&path, "agent_mode", "off")
            .expect_err("missing project must error");
        assert!(
            err.contains("Project not found"),
            "expected 'Project not found' diagnostic, got {err:?}",
        );
    }

    #[test]
    fn get_project_settings_returns_default_use_session_stream_off_for_fresh_project() {
        let path = unique_path("default-uss");
        let _pid = insert_project(&path);
        // Migration 0032's default backfills 'off' for existing rows; new
        // INSERTs (without explicit column) should also read as Off via
        // the unwrap_or in the row mapper. Sanity-check both the JSON
        // shape and the convenience bool accessor.
        let settings = get_project_settings(&path).expect("read");
        assert_eq!(settings["useSessionStream"], false);
        assert!(!get_use_session_stream(&path));
    }

    // ── #67 per-workspace remote-instruct opt-in ───────────────────

    /// A fresh workspace row defaults to remote-instruct OFF (fail-closed),
    /// surfaces as `allowRemoteInstruct: false` in the settings JSON, and
    /// round-trips through `update_project_setting`.
    #[test]
    fn allow_remote_instruct_defaults_off_and_round_trips() {
        let path = unique_path("remote-instruct");
        let _pid = insert_project(&path);

        // Fresh row: the security default is OFF.
        let settings = get_project_settings(&path).expect("read default");
        assert_eq!(settings["allowRemoteInstruct"], false);
        assert!(!get_allow_remote_instruct(&path));

        // Opt in → reads true.
        update_project_setting(&path, "allow_remote_instruct", "1").expect("opt in");
        assert!(get_allow_remote_instruct(&path));
        let settings = get_project_settings(&path).expect("read on");
        assert_eq!(settings["allowRemoteInstruct"], true);

        // Opt back out → reads false.
        update_project_setting(&path, "allow_remote_instruct", "0").expect("opt out");
        assert!(!get_allow_remote_instruct(&path));
    }

    /// A non-'0'/'1' value must be rejected loudly so a typo can't leave
    /// the security gate in an undefined state.
    #[test]
    fn allow_remote_instruct_rejects_bad_value() {
        let path = unique_path("remote-instruct-bad");
        let _pid = insert_project(&path);
        let err = update_project_setting(&path, "allow_remote_instruct", "true")
            .expect_err("non 0/1 value must be rejected");
        assert!(
            err.contains("allow_remote_instruct"),
            "error should reference the field, got {err:?}",
        );
    }

    /// An unregistered workspace path fails CLOSED — `false`, never panics.
    #[test]
    fn get_allow_remote_instruct_fails_closed_for_unknown_path() {
        let path = unique_path("remote-instruct-missing");
        // No insert — the path doesn't exist in `projects`.
        assert!(!get_allow_remote_instruct(&path));
    }

    /// The EFFECTIVE gate decision (`remote_instruct_allowed_for_path`):
    ///   - both flags OFF (default)            → deny (fail-closed)
    ///   - per-workspace ON, app-level OFF     → allow
    ///   - per-workspace OFF, app-level ON     → allow (global master, back-compat)
    /// HOME is pointed at a tempdir so the app-level flag in
    /// `~/.k2/settings.json` is isolated; the per-workspace flag lives in
    /// the shared in-memory DB.
    #[test]
    fn remote_instruct_effective_or_semantics() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        let path = unique_path("remote-instruct-effective");
        let _pid = insert_project(&path);

        // Both OFF → deny.
        crate::app_settings::update(serde_json::json!({ "allowRemoteInstruct": false }))
            .expect("app off");
        update_project_setting(&path, "allow_remote_instruct", "0").expect("ws off");
        assert!(
            !remote_instruct_allowed_for_path(&path),
            "both flags off must deny (fail-closed)",
        );

        // Per-workspace ON, app-level OFF → allow.
        update_project_setting(&path, "allow_remote_instruct", "1").expect("ws on");
        assert!(
            remote_instruct_allowed_for_path(&path),
            "per-workspace opt-in must allow even with app-level off",
        );

        // Per-workspace OFF, app-level ON → allow (global master).
        update_project_setting(&path, "allow_remote_instruct", "0").expect("ws off again");
        crate::app_settings::update(serde_json::json!({ "allowRemoteInstruct": true }))
            .expect("app on");
        assert!(
            remote_instruct_allowed_for_path(&path),
            "app-level master must opt in every workspace (back-compat)",
        );
    }

    // ── B3a per-workspace Anthropic API key (BYO key) ──────────────

    /// A fresh workspace has NO key (NULL → None); set-by-id round-trips
    /// through the path-based getter; clearing (empty key) returns to None.
    #[test]
    fn workspace_api_key_defaults_none_and_round_trips() {
        let path = unique_path("ws-api-key");
        let pid = insert_project(&path);

        // Fresh row: no key configured.
        assert_eq!(get_workspace_api_key(&path), None);

        // Set by project_id, read back by path.
        set_workspace_api_key(&pid, "sk-ant-test-abc123").expect("set key");
        assert_eq!(
            get_workspace_api_key(&path).as_deref(),
            Some("sk-ant-test-abc123"),
        );

        // Whitespace is trimmed on read.
        set_workspace_api_key(&pid, "  sk-ant-padded  ").expect("set padded");
        assert_eq!(get_workspace_api_key(&path).as_deref(), Some("sk-ant-padded"));

        // Clear (empty key) → back to None (no empty string stored).
        set_workspace_api_key(&pid, "").expect("clear key");
        assert_eq!(get_workspace_api_key(&path), None);
    }

    /// A blank/whitespace stored value reads as None so the spawn door never
    /// stages an empty `ANTHROPIC_API_KEY=`.
    #[test]
    fn workspace_api_key_blank_value_reads_none() {
        let path = unique_path("ws-api-key-blank");
        let pid = insert_project(&path);
        set_workspace_api_key(&pid, "   ").expect("set blank");
        assert_eq!(get_workspace_api_key(&path), None);
    }

    /// Setting against an unknown project id fails LOUDLY (never silently).
    #[test]
    fn set_workspace_api_key_fails_loudly_on_missing_project() {
        let err = set_workspace_api_key("no-such-project-id", "sk-ant-x")
            .expect_err("missing project must error");
        assert!(
            err.contains("Project not found"),
            "expected 'Project not found' diagnostic, got {err:?}",
        );
    }

    /// An unregistered workspace path reads None (never panics).
    #[test]
    fn get_workspace_api_key_none_for_unknown_path() {
        let path = unique_path("ws-api-key-missing");
        assert_eq!(get_workspace_api_key(&path), None);
    }

    // ── app_settings JSON accessors ────────────────────────────────
    //
    // 0.39.0 moved the four global toggles (agentic_systems_enabled,
    // keep_daemon_on_quit) off of the SQLite `app_settings (key, value)`
    // table and onto `~/.k2so/settings.json`. These tests pin the
    // round-trip through the JSON store so we never regress that
    // canonicalization.
    //
    // The previous SQLite tests (commit df244efe) shared a per-process
    // mutex over the global row — fine for that backend because the
    // shared in-memory DB is process-wide. The new JSON backend reads
    // `$HOME/.k2so/settings.json`, so each test instead points `$HOME`
    // at a fresh tempdir (matching the pattern in `app_settings::tests`).
    // We share the crate-wide `themes::HOME_LOCK` mutex with the other
    // HOME-mutating test modules so two of them don't race on `$HOME`
    // at once — see the long comment in `app_settings::tests` for why
    // a single shared lock matters.
    use crate::themes::HOME_LOCK as HOME_TEST_LOCK;

    /// Point `$HOME` at a freshly-created tempdir for the lifetime of
    /// the guard. Mirrors the pattern in `app_settings::tests` —
    /// kept local here rather than re-exported so workspace/settings
    /// tests don't depend on the private `tempdir_lite` module in
    /// `app_settings::tests`.
    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        path: std::path::PathBuf,
    }

    impl HomeGuard {
        fn new() -> Self {
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir()
                .join(format!("k2so-workspace-settings-test-{pid}-{nanos}"));
            std::fs::create_dir_all(&path).expect("create tempdir for HOME");
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", &path);
            Self { original, path }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn agentic_enabled_round_trips_through_app_settings() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Default when the JSON file is absent is `true` — the
        // `AppSettings::default()` value for `agentic_systems_enabled`.
        assert!(
            get_agentic_enabled(),
            "fresh ~/.k2so/settings.json → agentic_systems_enabled defaults to true",
        );

        // Set true → read true.
        set_agentic_enabled(true).expect("set true");
        assert!(get_agentic_enabled(), "after set(true), read must be true");

        // Set false → read false (update() rewrites the JSON in place).
        set_agentic_enabled(false).expect("set false");
        assert!(
            !get_agentic_enabled(),
            "after set(false), read must be false",
        );

        // And confirm a fresh `app_settings::load()` (the canonical
        // round-trip path) sees the same value — guards against the
        // accessor accidentally caching in-process.
        assert!(!crate::app_settings::load().agentic_systems_enabled);
    }

    #[test]
    fn keep_daemon_on_quit_round_trips_through_app_settings() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Default when the JSON file is absent is `true` — see the doc
        // comment on `get_keep_daemon_on_quit` for the rationale.
        assert!(
            get_keep_daemon_on_quit(),
            "fresh ~/.k2so/settings.json → keep_daemon_on_quit defaults to true",
        );

        set_keep_daemon_on_quit(false).expect("set false");
        assert!(
            !get_keep_daemon_on_quit(),
            "after set(false), read must be false",
        );

        set_keep_daemon_on_quit(true).expect("set true");
        assert!(
            get_keep_daemon_on_quit(),
            "after set(true), read must be true",
        );

        // Fresh load sees the persisted value.
        assert!(crate::app_settings::load().keep_daemon_on_quit);
    }

    // ── Canonical-agents: turning into an agent NEVER touches fan-out ──
    //
    // The global "default for new agents" flag was removed entirely. The
    // only surface that ever writes the per-workspace
    // `.k2/.harness-fanout-enabled` marker is the explicit, user-confirmed
    // per-workspace checkbox (route `POST /cli/onboarding/set-harness-fanout-enabled`).
    // This test pins the load-bearing guarantee: an off→on `agent_mode`
    // transition must NOT auto-apply fan-out to the workspace's marker.

    /// Insert a project row whose path is a REAL temp directory, so the
    /// per-workspace `.k2*/` marker writes (if any were attempted) would
    /// land on disk. Returns the path.
    fn insert_project_with_real_dir(label: &str) -> (String, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "k2so-fanout-default-{}-{}-{}",
            label,
            std::process::id(),
            Uuid::new_v4(),
        ));
        std::fs::create_dir_all(&dir).expect("create workspace dir");
        let path = dir.to_string_lossy().to_string();
        insert_project(&path);
        (path, dir)
    }

    #[test]
    fn turning_workspace_into_agent_never_applies_fanout() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        let (path, dir) = insert_project_with_real_dir("become-agent");
        // Starts off (insert default) with no marker.
        update_project_setting(&path, "agent_mode", "off").expect("seed off");
        assert!(!crate::workspace::onboarding::harness_fanout_enabled(&path));

        // off→on transition: turning into an agent must NOT touch the
        // per-workspace fan-out marker — there is no auto-apply, ever.
        update_project_setting(&path, "agent_mode", "manager").expect("turn into agent");
        assert!(
            !crate::workspace::onboarding::harness_fanout_enabled(&path),
            "turning a workspace into an agent must NEVER auto-apply fan-out",
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_agentic_enabled_is_idempotent_under_repeated_writes() {
        // Regression guard for the deep-merge path — calling set twice
        // with the same value must leave the JSON in the expected
        // state and not corrupt or duplicate the field.
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        set_agentic_enabled(true).expect("first set");
        set_agentic_enabled(true).expect("second set — must not error");
        assert!(get_agentic_enabled());

        // Sibling fields must not be perturbed by the partial update —
        // deep_merge in `app_settings` should only touch the one key
        // we passed in.
        let loaded = crate::app_settings::load();
        assert!(
            loaded.keep_daemon_on_quit,
            "agentic toggle must not clobber keep_daemon_on_quit default",
        );
    }
}
