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
    ];
    if !allowed.contains(&field) {
        return Err(format!("Unknown setting: {}", field));
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
                pinned, name, tier_id, use_session_stream \
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

        // Default when the JSON file is absent is `false` — the
        // `AppSettings::default()` value for `agentic_systems_enabled`.
        assert!(
            !get_agentic_enabled(),
            "fresh ~/.k2so/settings.json → agentic_systems_enabled defaults to false",
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
