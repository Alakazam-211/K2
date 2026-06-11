//! D2 `use_session_stream` project-setting tests.
//!
//! Covers the 0032_add_use_session_stream migration, the setting
//! allowlist extension, value validation, and the `get_use_session_stream`
//! helper. Each test constructs a fresh in-memory DB via the
//! `test-util` feature so the shared DB state doesn't contaminate
//! the actual ~/.k2so/k2so.db.

use k2_core::workspace::settings::{
    get_use_session_stream, update_project_setting,
};
use k2_core::db::{init_for_tests, shared};

fn fresh_db() {
    init_for_tests();
}

fn insert_project(path: &str) {
    // `projects.id` is `TEXT PRIMARY KEY NOT NULL`. Using `path` as
    // the id here is safe because each test uses a unique path — a
    // v4 UUID would work too but the path is already unique per test
    // and simpler to debug.
    let db = shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, path, name) VALUES (?1, ?1, ?1)",
        rusqlite::params![path],
    )
    .unwrap();
}

fn read_raw(path: &str) -> Option<String> {
    let db = shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT use_session_stream FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

#[test]
fn migration_adds_column_with_off_default() {
    fresh_db();
    insert_project("/tmp/test-d2-default");
    // Fresh row inserted after migration applied → column defaults 'off'.
    assert_eq!(
        read_raw("/tmp/test-d2-default").as_deref(),
        Some("off")
    );
}

#[test]
fn get_helper_returns_false_when_off() {
    fresh_db();
    insert_project("/tmp/test-d2-helper-off");
    assert!(!get_use_session_stream("/tmp/test-d2-helper-off"));
}

#[test]
fn get_helper_returns_false_for_unknown_project() {
    fresh_db();
    assert!(!get_use_session_stream("/tmp/test-d2-does-not-exist"));
}

#[test]
fn set_on_round_trips_through_get_helper() {
    fresh_db();
    insert_project("/tmp/test-d2-on");
    update_project_setting("/tmp/test-d2-on", "use_session_stream", "on")
        .expect("update should succeed");
    assert!(get_use_session_stream("/tmp/test-d2-on"));
    assert_eq!(read_raw("/tmp/test-d2-on").as_deref(), Some("on"));
}

#[test]
fn set_off_reverts_to_false() {
    fresh_db();
    insert_project("/tmp/test-d2-toggle");
    update_project_setting("/tmp/test-d2-toggle", "use_session_stream", "on")
        .unwrap();
    assert!(get_use_session_stream("/tmp/test-d2-toggle"));
    update_project_setting("/tmp/test-d2-toggle", "use_session_stream", "off")
        .unwrap();
    assert!(!get_use_session_stream("/tmp/test-d2-toggle"));
}

#[test]
fn invalid_value_is_rejected() {
    fresh_db();
    insert_project("/tmp/test-d2-bad-value");
    let err = update_project_setting(
        "/tmp/test-d2-bad-value",
        "use_session_stream",
        "maybe",
    )
    .unwrap_err();
    assert!(err.contains("'on' or 'off'"), "error was: {err}");
    // DB unchanged.
    assert_eq!(
        read_raw("/tmp/test-d2-bad-value").as_deref(),
        Some("off")
    );
}

#[test]
fn updating_setting_on_missing_project_errors() {
    fresh_db();
    let err = update_project_setting(
        "/tmp/test-d2-nonexistent",
        "use_session_stream",
        "on",
    )
    .unwrap_err();
    assert!(
        err.contains("Project not found"),
        "error was: {err}"
    );
}

#[test]
fn unknown_field_still_rejected() {
    // Regression: the allowlist should still reject arbitrary
    // field names. Adding use_session_stream shouldn't have
    // loosened that.
    fresh_db();
    insert_project("/tmp/test-d2-unknown-field");
    let err = update_project_setting(
        "/tmp/test-d2-unknown-field",
        "arbitrary_column; DROP TABLE projects;",
        "on",
    )
    .unwrap_err();
    assert!(err.contains("Unknown setting"), "error was: {err}");
}

// ─────────────────────────────────────────────────────────────────────
// G6 — settings JSON exposes useSessionStream for the UI toggle
// ─────────────────────────────────────────────────────────────────────

use k2_core::workspace::settings::get_project_settings;

#[test]
fn get_project_settings_exposes_use_session_stream_bool() {
    fresh_db();
    insert_project("/tmp/test-g6-reads-off");
    let settings = get_project_settings("/tmp/test-g6-reads-off").unwrap();
    // Default from the migration is 'off' → false in the JSON.
    assert_eq!(
        settings.get("useSessionStream").and_then(|v| v.as_bool()),
        Some(false),
        "settings JSON: {settings:#}"
    );

    update_project_setting(
        "/tmp/test-g6-reads-off",
        "use_session_stream",
        "on",
    )
    .unwrap();
    let settings = get_project_settings("/tmp/test-g6-reads-off").unwrap();
    assert_eq!(
        settings.get("useSessionStream").and_then(|v| v.as_bool()),
        Some(true),
        "after set 'on', useSessionStream should flip: {settings:#}"
    );
}
